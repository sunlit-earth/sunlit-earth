//! The wgpu render pipeline.
//!
//! `Renderer` owns every GPU object and renders offscreen into its own texture.
//! It knows nothing about windows, event loops, or Slint: callers hand it a
//! `SceneParams` plus a sky state and read the frame back as pixels.

mod frame;
mod gpu_setup;
mod render_pass;
pub mod residency;
mod sizing;
mod slots;
mod surface;
mod texture_routing;
mod textures;
pub mod tiles;
pub mod uniforms;

pub use render_pass::read_texture_rgba8;
pub use sizing::build_aa_options;
pub use slots::{SlotLayout, TEXTURE_LABELS};
pub use surface::surface_sampler_descriptor;

pub(crate) use sizing::{quantize_to_granularity, resolve_sample_count};
pub(crate) use surface::{SurfaceFormats, SurfaceLayer};

use std::path::PathBuf;

use tracing::debug;

use crate::assets::cloud_fetcher::NotifyFn;
use crate::assets::mailbox::TextureMailbox;
use crate::assets::tiles::{Geometry, Pack};
use crate::memory_report::{ExpectedTexture, MemoryReport};
use crate::params::SceneParams;
use crate::scene::sky::{PlanetKind, SkyState};

use frame::{FrameState, build_frame_state};
use gpu_setup::{
    COLOR_FORMAT, DEPTH_FORMAT, Pipelines, create_render_textures, rebuild_msaa_resources,
    rebuild_render_textures,
};
use slots::{SLOT_LABELS, TextureMode};
use surface::{ResidentCube, SurfaceSet};
use texture_routing::ResolvedTexture;
use textures::{
    Bindings, TextureSlot, create_bind_group, maybe_spawn_texture_load, process_decoded_textures,
};
use tiles::{CellLevels, SurfaceTiles, TileId, TileLayers, TileUpload};

/// What a call to [`Renderer::render`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderOutcome {
    /// Nothing changed since the last frame; the preview texture still holds it.
    Skipped,
    /// A new frame was drawn into the preview texture.
    Rendered { first_frame: bool },
}

/// Everything needed to build a [`Renderer`] beyond the device and queue.
pub(crate) struct RendererConfig {
    pub sample_count: u32,
    pub width: u32,
    pub height: u32,
    /// One entry per file-backed texture slot, in slot order after the grid:
    /// the Moon, then the Milky Way.
    pub texture_paths: Vec<Option<PathBuf>>,
    /// Width the file-backed textures are loaded at, as a cap: a source
    /// narrower than this is loaded as it is.
    pub texture_resolution: u32,
    /// Shared with the cloud fetcher and the background decode threads.
    pub mailbox: TextureMailbox,
    /// Invoked from decode threads once a result has been parked.
    pub notify: NotifyFn,
    /// The month in force, January 0, when the globe is drawn from the cube
    /// surface the engine hands over pack by pack; `None` draws the grid.
    pub cube_month: Option<usize>,
    /// Whether the adapter is a CPU, which gets the surfaces decoded and
    /// sampled without anisotropy.
    pub cpu_adapter: bool,
    /// What the packs the cube surface comes from are cut to, which sizes the
    /// page table and the tile array's layers.
    pub tile_geometry: Geometry,
    /// Layers of the tile array; `None` takes the budget for the adapter,
    /// `tiles::TILE_LAYER_BUDGET` or `tiles::CPU_TILE_LAYER_BUDGET`.
    pub tile_layers: Option<u32>,
}

/// The GPU pipeline and every resource it owns.
pub(crate) struct Renderer {
    pipelines: Pipelines,
    star_buffer: wgpu::Buffer,
    planet_buffer: wgpu::Buffer,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform_buffer: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
    /// What the flat textures are read through: the Moon, the Milky Way and
    /// the clouds.
    sampler: wgpu::Sampler,
    /// What every cube the globe draws from is read through.
    surface_sampler: wgpu::Sampler,
    texture_slots: Vec<TextureSlot>,
    /// The cube surface, when the globe is drawn from it.
    surface: Option<SurfaceSet>,
    /// Width the file-backed slots load at, as a cap.
    texture_resolution: u32,
    /// Bumped on every resolution change, and stamped on each load it spawns,
    /// so a decode that was already running when the width changed is
    /// recognizable as something nobody asked for any more.
    texture_generation: u64,
    depth_texture: wgpu::TextureView,
    render_texture: wgpu::Texture,
    msaa_texture_view: Option<wgpu::TextureView>,
    msaa_depth_view: Option<wgpu::TextureView>,
    sample_count: u32,
    render_width: u32,
    render_height: u32,
    /// Last rendered state for dirty-checking. `None` means first frame.
    last_state: Option<FrameState>,
    /// Per-frame inputs (sky state, blend flag) from the last rendered frame.
    /// The scheduler overwrites the sky here so a hidden window still exports
    /// with current astronomy.
    last_inputs: Option<render_pass::FrameInputs>,
    /// Bind group resolution from the last rendered frame, used by the
    /// export path to reuse the same texture binding without re-resolving.
    last_resolved: Option<ResolvedTexture>,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Latest-value mailbox for completed texture decodes. Cloned into each
    /// background decode thread and drained by `drain_texture_updates`.
    texture_mailbox: TextureMailbox,
    /// Set when a decoded texture was uploaded since the last render, cleared
    /// by `render`. Draining and rendering are separate calls, so the flag
    /// rather than the drain's own return value decides whether the next frame
    /// must be redrawn.
    texture_dirty: bool,
    /// Called from decode threads after posting to the mailbox, so a client
    /// that only renders on demand knows there is work waiting.
    notify: NotifyFn,
    /// 1x1 black texture standing in at binding 1 for every group that draws
    /// the globe from a cube.
    dummy_texture_view: wgpu::TextureView,
    /// 1x1 black cube standing in wherever a bind group has no cube to put.
    dummy_cube_view: wgpu::TextureView,
    /// The tile array and the page table of every bind group but the cube
    /// surface's: one black layer, and one cell per face that draws the floor.
    dummy_tile_view: wgpu::TextureView,
    dummy_page_view: wgpu::TextureView,
    /// Bind group for the cloud texture (populated after async load completes).
    cloud_bind_group: Option<wgpu::BindGroup>,
    /// Stored texture view for the cloud texture, used to rebuild the bind group.
    cloud_texture_view: Option<wgpu::TextureView>,
}

impl Renderer {
    /// Create the pipeline, the sphere mesh, the grid texture, and the
    /// offscreen render targets.
    pub(crate) fn new(device: wgpu::Device, queue: wgpu::Queue, config: RendererConfig) -> Self {
        gpu_setup::create_renderer(device, queue, config)
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        (self.render_width, self.render_height)
    }

    /// Whether a frame has ever been drawn into the preview texture.
    pub(crate) fn has_frame(&self) -> bool {
        self.last_state.is_some()
    }

    /// Resize the offscreen targets. The caller is expected to have quantized
    /// the size already; identical sizes are a no-op.
    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        if width == self.render_width && height == self.render_height {
            return;
        }
        debug!(width, height, "viewport size changed");
        rebuild_render_textures(self, width, height);
    }

    /// Load the file-backed textures at a different width.
    ///
    /// Returns whether anything changed. When it did, the textures in memory
    /// are freed before the reload is spawned, so going down lowers the
    /// process's footprint rather than adding to it; the caller is expected to
    /// mark itself dirty, and to purge the tiles, which every switch
    /// empties and which are the engine's to decide.
    ///
    /// Any width is accepted and acts as a cap. Which widths a user may choose
    /// between is a question for the config and the combo box, not for the
    /// renderer.
    pub(crate) fn set_texture_resolution(&mut self, width: u32) -> bool {
        if width == self.texture_resolution {
            return false;
        }
        debug!(
            from = self.texture_resolution,
            to = width,
            "texture resolution changed"
        );
        self.texture_resolution = width;
        self.texture_generation += 1;
        textures::purge_file_backed_slots(self);
        true
    }

    /// The width the file-backed textures are loaded at.
    pub(crate) fn texture_resolution(&self) -> u32 {
        self.texture_resolution
    }

    /// A memory report for this renderer's device, with the expected table
    /// filled in from what the renderer knows it owns.
    ///
    /// `adapter` is the slug from `wgpu_init::adapter_key`, which the renderer
    /// is not told and the engine is.
    pub(crate) fn memory_report(&self, adapter: &str) -> MemoryReport {
        crate::memory_report::collect(&self.device, adapter, self.expected_textures())
    }

    /// Every texture this renderer owns, and the shape each one should be.
    ///
    /// Slot textures and the surface's cubes report their own shape, because
    /// the renderer holds them. The depth and MSAA targets and the 1x1
    /// placeholders are kept only as views, so their rows are computed from
    /// the size, format, and sample
    /// count they were created with; `create_render_textures` is the other side
    /// of that agreement, and the two formats are shared constants so the rows
    /// cannot drift from the descriptors.
    fn expected_textures(&self) -> Vec<ExpectedTexture> {
        let target = |label: &str, format, sample_count| ExpectedTexture {
            label: label.to_owned(),
            width: self.render_width,
            height: self.render_height,
            layers: 1,
            format,
            mip_levels: 1,
            sample_count,
        };
        let held = |label: String, texture: &wgpu::Texture| ExpectedTexture {
            label,
            width: texture.width(),
            height: texture.height(),
            layers: texture.depth_or_array_layers(),
            format: texture.format(),
            mip_levels: texture.mip_level_count(),
            sample_count: texture.sample_count(),
        };

        let mut expected: Vec<ExpectedTexture> = self
            .texture_slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| Some(held(self.slot_label(index), slot.texture.as_ref()?)))
            .collect();
        if let Some(surface) = &self.surface {
            expected.extend(
                surface
                    .resident()
                    .map(|(label, texture)| held(label.to_owned(), texture)),
            );
        }

        for (label, layers, format) in [
            ("dummy_1x1", 1, COLOR_FORMAT),
            ("dummy_cube", 6, COLOR_FORMAT),
            ("dummy_tile_array", 1, wgpu::TextureFormat::Rgba8Unorm),
            ("dummy_page_table", 6, wgpu::TextureFormat::R32Uint),
        ] {
            expected.push(ExpectedTexture {
                label: label.to_owned(),
                width: 1,
                height: 1,
                layers,
                format,
                mip_levels: 1,
                sample_count: 1,
            });
        }
        expected.push(target("render_texture", COLOR_FORMAT, 1));
        expected.push(target("depth_texture", DEPTH_FORMAT, 1));
        if self.msaa_texture_view.is_some() {
            expected.push(target(
                "msaa_color_texture",
                COLOR_FORMAT,
                self.sample_count,
            ));
        }
        if self.msaa_depth_view.is_some() {
            expected.push(target(
                "msaa_depth_texture",
                DEPTH_FORMAT,
                self.sample_count,
            ));
        }
        expected
    }

    /// Upload every decoded texture parked in the mailbox.
    ///
    /// Deliberately independent of whether anything is being displayed: this is
    /// what keeps cloud updates flowing to the GPU while the window is hidden.
    /// Returns `true` if at least one texture was uploaded.
    pub(crate) fn drain_texture_updates(&mut self) -> bool {
        process_decoded_textures(self)
    }

    /// Where the textures sit, derived from how many file-backed slots this
    /// renderer was built with.
    fn layout(&self) -> SlotLayout {
        SlotLayout::new(self.texture_slots.len() - 2)
    }

    /// The GPU label the texture in `slot` carries.
    fn slot_label(&self, slot: usize) -> String {
        if slot == self.layout().clouds() {
            return "cloud_texture".to_owned();
        }
        SLOT_LABELS
            .get(slot)
            .map_or_else(|| format!("texture_slot_{slot}"), |name| (*name).to_owned())
    }

    /// The bind group holding the Moon's surface texture, once it has loaded.
    fn moon_bind_group(&self) -> Option<&wgpu::BindGroup> {
        let slot = self.layout().moon()?;
        self.texture_slots[slot].bind_group.as_ref()
    }

    /// The bind group holding the Milky Way panorama, once it has loaded.
    fn milky_way_bind_group(&self) -> Option<&wgpu::BindGroup> {
        let slot = self.layout().milky_way()?;
        self.texture_slots[slot].bind_group.as_ref()
    }

    /// The loading indicator text for the current texture selection, empty when
    /// nothing is loading.
    pub(crate) fn loading_text(&self, texture_index: i32) -> String {
        texture_routing::loading_text(self, TextureMode::from_index(texture_index))
    }

    /// Whether every texture the current mode needs is resident: its cubes,
    /// the floor of the month in force, the night floor, the mask. The clouds,
    /// the Moon and the Milky Way are excluded: they are overlays, not
    /// requirements. Without the cube surface only the grid is ever ready.
    pub(crate) fn textures_ready(&self, texture_index: i32) -> bool {
        let mode = TextureMode::from_index(texture_index);
        self.surface
            .as_ref()
            .map_or(mode == TextureMode::Grid, |surface| {
                surface.readiness(mode).0
            })
    }

    /// Whether a cube the current mode needs is still on its way.
    ///
    /// Deliberately not the negation of `textures_ready`: a cube whose pack
    /// failed is a terminal state where nothing further is coming, so there is
    /// nothing to wait for, and without the cube surface nothing ever is.
    pub(crate) fn textures_pending(&self, texture_index: i32) -> bool {
        let mode = TextureMode::from_index(texture_index);
        self.surface
            .as_ref()
            .is_some_and(|surface| surface.readiness(mode).1)
    }

    /// Make `month`, January 0, the one the surface is ready for. Its floor
    /// is what readiness asks for from now on, and the floor already resident
    /// is drawn until that one is handed over.
    pub(crate) fn set_surface_month(&mut self, month: usize) {
        if let Some(surface) = &mut self.surface {
            surface.month = month;
        }
    }

    /// Whether `layer` is resident.
    pub(crate) fn surface_resident(&self, layer: SurfaceLayer) -> bool {
        self.surface
            .as_ref()
            .is_some_and(|surface| surface.state(layer) == surface::LayerState::Resident)
    }

    /// Make `layer` resident from its pack, replacing the day floor of another
    /// month, and redraw on the next frame. A layer already resident is left
    /// as it is.
    pub(crate) fn install_surface(
        &mut self,
        layer: SurfaceLayer,
        pack: &Pack,
    ) -> Result<(), String> {
        let Some(surface) = self.surface.as_mut() else {
            return Err("the globe is not drawn from the cube surface".to_owned());
        };
        if surface.state(layer) == surface::LayerState::Resident {
            return Ok(());
        }
        let tiled = layer != SurfaceLayer::Mask;
        if tiled && let Some(tiles) = &surface.tiles {
            tiles.accepts(pack)?;
        }
        let texture =
            surface::cube_from_pack(&self.device, &self.queue, layer, pack, surface.formats)?;
        let cube = ResidentCube::new(texture);
        match layer {
            SurfaceLayer::Day(month) => surface.day = Some((month, cube)),
            SurfaceLayer::Night => surface.night = Some(cube),
            SurfaceLayer::Mask => surface.mask = Some(cube),
        }
        if tiled && let Some(tiles) = &mut surface.tiles {
            tiles.add_pack(&self.queue, pack)?;
        }
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        self.rebuild_surface_groups();
        self.texture_dirty = true;
        Ok(())
    }

    /// Record that `layer`'s pack failed, so nothing waits for it.
    pub(crate) fn mark_surface_failed(&mut self, layer: SurfaceLayer) {
        if let Some(surface) = &mut self.surface {
            surface.mark_failed(layer);
        }
    }

    /// Make `tiles` resident in the tile array and rewrite the page table over
    /// them, the two staged for the next submit together, and redraw on the
    /// next frame.
    ///
    /// Each tile goes into the layer it names, else the one it holds, else a
    /// free one; a tile that cannot be placed, or whose blob does not have a
    /// layer's layout, is returned with the reason and the others are made
    /// resident. The array is created with the first tile.
    pub(crate) fn upload_tiles(&mut self, tiles: Vec<TileUpload>) -> Vec<(TileId, String)> {
        let Some(surface_tiles) = self.surface.as_mut().and_then(|s| s.tiles.as_mut()) else {
            let why = "the globe is not drawn from the cube surface";
            return tiles.into_iter().map(|t| (t.id, why.to_owned())).collect();
        };
        let had_array = surface_tiles.array_view().is_some();
        let failed = surface_tiles.upload(&self.device, &self.queue, tiles);
        if !had_array && surface_tiles.array_view().is_some() {
            self.rebuild_surface_groups();
        }
        self.texture_dirty = true;
        failed
    }

    /// Free the layers `tiles` hold and rewrite the page table without them,
    /// and redraw on the next frame.
    pub(crate) fn evict_tiles(&mut self, tiles: &[TileId]) {
        if let Some(surface_tiles) = self.surface.as_mut().and_then(|s| s.tiles.as_mut()) {
            surface_tiles.evict(&self.queue, tiles);
            self.texture_dirty = true;
        }
    }

    /// Hold each cell of the page table to the finest level `cap` names there,
    /// the wanted set's (`residency::Wanted::cap`), and redraw on the next
    /// frame if that changes the table.
    pub(crate) fn set_tile_cap(&mut self, cap: CellLevels) {
        if let Some(surface_tiles) = self.surface.as_mut().and_then(|s| s.tiles.as_mut())
            && surface_tiles.set_cap(&self.queue, cap)
        {
            self.texture_dirty = true;
        }
    }

    /// Let go of every tile and of the tile array, and rewrite the page table
    /// without them, leaving the floors and the mask; the bind groups go back
    /// to the array's dummy until the next tile creates it again.
    pub(crate) fn purge_tiles(&mut self) {
        if let Some(surface_tiles) = self.surface.as_mut().and_then(|s| s.tiles.as_mut()) {
            surface_tiles.purge(&self.queue);
            self.rebuild_surface_groups();
            self.texture_dirty = true;
        }
    }

    /// The tile array's layers and the tile each holds, while the globe is
    /// drawn from the cube surface.
    pub(crate) fn tile_layers(&self) -> Option<&TileLayers> {
        self.surface
            .as_ref()
            .and_then(|surface| surface.tiles.as_ref())
            .map(SurfaceTiles::layers)
    }

    /// What the frame's uniforms need of the tiles: the ocean colors of the
    /// packs the page table draws from, and a tile's sizes.
    fn tile_uniforms(&self) -> render_pass::TileUniforms {
        self.surface
            .as_ref()
            .and_then(|surface| surface.tiles.as_ref())
            .map_or_else(render_pass::TileUniforms::default, |tiles| {
                render_pass::TileUniforms {
                    ocean: tiles.ocean_colors(),
                    tile: tiles.geometry().tile,
                    gutter: tiles.geometry().gutter,
                }
            })
    }

    /// Build the surface's bind groups from what is resident: the day floor and
    /// the night floor each alone, and the two with the mask for blend mode,
    /// with the dummy cube for a mask that is not there, each with the page
    /// table and the tile array, or the array's dummy until the first tile.
    fn rebuild_surface_groups(&mut self) {
        let Some(surface) = &self.surface else {
            return;
        };
        let dummy = &self.dummy_cube_view;
        let tiles = surface.tiles.as_ref().map_or(
            [&self.dummy_tile_view, &self.dummy_page_view],
            |tiles| {
                [
                    tiles.array_view().unwrap_or(&self.dummy_tile_view),
                    tiles.page_view(),
                ]
            },
        );
        let day = surface.day.as_ref().map(|(_, cube)| &cube.view);
        let night = surface.night.as_ref().map(|cube| &cube.view);
        let mask = surface.mask.as_ref().map_or(dummy, |cube| &cube.view);
        let group = |cubes, label| self.cube_bind_group(cubes, tiles, label);
        let day_group = day.map(|day| group([day, dummy, dummy], "surface_day"));
        let night_group = night.map(|night| group([night, dummy, dummy], "surface_night"));
        let blend_group = day
            .zip(night)
            .map(|(day, night)| group([day, night, mask], "surface_blend"));
        let surface = self.surface.as_mut().expect("checked above");
        surface.day_group = day_group;
        surface.night_group = night_group;
        surface.blend_group = blend_group;
    }

    /// A bind group of one flat texture.
    fn flat_bind_group(&self, texture: &wgpu::TextureView, label: &str) -> wgpu::BindGroup {
        let dummy = &self.dummy_cube_view;
        create_bind_group(
            &self.device,
            &self.bind_group_layout,
            &self.uniform_buffer,
            &Bindings {
                texture,
                sampler: &self.sampler,
                cubes: [dummy, dummy, dummy],
                tiles: [&self.dummy_tile_view, &self.dummy_page_view],
            },
            label,
        )
    }

    /// A bind group that draws the globe from `cubes`, refined through `tiles`,
    /// the tile array and the page table.
    fn cube_bind_group(
        &self,
        cubes: [&wgpu::TextureView; 3],
        tiles: [&wgpu::TextureView; 2],
        label: &str,
    ) -> wgpu::BindGroup {
        create_bind_group(
            &self.device,
            &self.bind_group_layout,
            &self.uniform_buffer,
            &Bindings {
                texture: &self.dummy_texture_view,
                sampler: &self.surface_sampler,
                cubes,
                tiles,
            },
            label,
        )
    }

    /// The bind group a resolution names, if it still exists.
    fn bind_group_for(&self, resolved: &ResolvedTexture) -> Option<&wgpu::BindGroup> {
        match resolved {
            ResolvedTexture::Grid => self.texture_slots[0].bind_group.as_ref(),
            ResolvedTexture::Surface(group) => self.surface.as_ref()?.group(*group),
        }
    }

    /// Overwrite the astronomy state the export path will use.
    ///
    /// The scheduler calls this before an unattended wallpaper export so the
    /// image reflects the current time even when no frame has been drawn since
    /// the window was hidden.
    pub(crate) fn set_sky_state(&mut self, sky: SkyState) {
        self.update_planets(&sky);
        if let Some(inputs) = &mut self.last_inputs {
            inputs.sky = sky;
        }
    }

    /// Draw a frame into the preview texture, unless nothing changed.
    ///
    /// Kicks off background loads for the overlays that are wanted whether or
    /// not the frame is skipped, so switching one on starts loading it
    /// immediately.
    pub(crate) fn render(&mut self, params: &SceneParams, sky: &SkyState) -> RenderOutcome {
        if params.sample_count != self.sample_count {
            debug!(
                sample_count = params.sample_count,
                "MSAA sample count changed"
            );
            rebuild_msaa_resources(self, params.sample_count);
        }

        let received_any = std::mem::take(&mut self.texture_dirty);
        let current_state = build_frame_state(params, self.render_width, self.render_height, sky);

        let (resolved, use_blend) =
            texture_routing::resolve_textures(self, TextureMode::from_index(params.texture_index));

        // An overlay's texture is loaded when the overlay is wanted and not
        // before, which is what keeps a switched-off one from costing a decode.
        for slot in [
            (params.moon_brightness > 0.0).then(|| self.layout().moon()),
            (params.milky_way_intensity > 0.0).then(|| self.layout().milky_way()),
        ] {
            if let Some(Some(slot)) = slot {
                maybe_spawn_texture_load(self, slot);
            }
        }

        if !received_any && self.last_state.as_ref() == Some(&current_state) {
            return RenderOutcome::Skipped;
        }

        self.last_inputs = Some(render_pass::FrameInputs {
            sky: sky.clone(),
            use_blend,
            night_alone: resolved.draws_the_night_alone(),
            tiles: self.tile_uniforms(),
        });
        self.last_resolved = Some(resolved);
        self.update_planets(sky);

        let first_frame = self.last_state.is_none();
        let bind_group_ref = self
            .bind_group_for(self.last_resolved.as_ref().expect("just assigned"))
            .expect("the routing names only bind groups that exist");
        render_pass::execute_render_pass(
            self,
            params,
            bind_group_ref,
            self.last_inputs.as_ref().expect("just assigned"),
        );
        self.last_state = Some(current_state);

        RenderOutcome::Rendered { first_frame }
    }

    /// Rewrite the planet instance buffer to match `sky`.
    ///
    /// Called from the two places that move `last_inputs.sky`, and from
    /// neither of them when the sky did not move: the buffer is what the draw
    /// reads and `last_inputs` is what a replayed export re-encodes, so the
    /// two have to name the same instant.
    fn update_planets(&self, sky: &SkyState) {
        self.queue
            .write_buffer(&self.planet_buffer, 0, &planet_instance_bytes(sky));
    }

    /// Read the preview texture back into RGBA8 pixels.
    ///
    /// Only valid when the renderer was built with `COPY_SRC` on its preview
    /// texture (see [`RendererConfig`] users that need readback).
    pub(crate) fn read_preview_pixels(&self) -> Result<Vec<u8>, String> {
        read_texture_rgba8(
            &self.device,
            &self.queue,
            &self.render_texture,
            self.render_width,
            self.render_height,
        )
    }

    /// The largest export this device will take, and the largest readback.
    ///
    /// `max_texture_dimension_2d` caps the canvas a spanned wallpaper renders
    /// into, and `max_buffer_size` caps the readback that comes out of it.
    /// `Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits())`
    /// copies the resolution limits from the adapter and leaves the buffer size
    /// at the downlevel default, so the two are not the same number and neither
    /// is worth guessing.
    pub(crate) fn export_limits(&self) -> (u32, u64) {
        let limits = self.device.limits();
        (limits.max_texture_dimension_2d, limits.max_buffer_size)
    }

    /// Replay the last frame's scene with a different framing, at a different
    /// size.
    ///
    /// The texture routing and the sky are the ones the last render resolved,
    /// so `params` may differ from it only in what `write_uniforms` reads. That
    /// is what the wallpaper path changes: the fields of view and the pan, all
    /// of which are derived per screen.
    pub(crate) fn export_image_with(
        &self,
        params: &SceneParams,
        target_width: u32,
        target_height: u32,
    ) -> Result<Vec<u8>, String> {
        let inputs = self.last_inputs.as_ref().ok_or("No frame rendered yet")?;

        let bind_group = self
            .bind_group_for(self.last_resolved.as_ref().ok_or("No frame rendered yet")?)
            .ok_or("No bind group available")?;

        let (export_texture, export_depth, msaa_color_view, msaa_depth_view) =
            create_render_textures(
                &self.device,
                target_width,
                target_height,
                self.sample_count,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            );

        let resolve_view = export_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let target = render_pass::RenderTarget::new(
            &resolve_view,
            msaa_color_view.as_ref(),
            &export_depth,
            msaa_depth_view.as_ref(),
        );

        crate::memory::log_memory_usage("wallpaper: before render");
        render_pass::draw_scene(
            self,
            params,
            bind_group,
            inputs,
            target_width,
            target_height,
            &target,
        );

        crate::memory::log_memory_usage("wallpaper: before pixel readback");
        let pixels = read_texture_rgba8(
            &self.device,
            &self.queue,
            &export_texture,
            target_width,
            target_height,
        )?;
        crate::memory::log_memory_usage("wallpaper: after pixel readback");
        Ok(pixels)
    }

    /// [`Self::export_image_with`] with the page table held to `cap`, the
    /// export's own (`residency::Residency::cap`), for this frame alone.
    ///
    /// The cap the table held before is put back afterwards and the redraw
    /// flag left as it was: the preview frame already drawn was drawn with
    /// that cap, and the two rewrites are staged around the export's submit.
    pub(crate) fn export_image_capped(
        &mut self,
        params: &SceneParams,
        target_width: u32,
        target_height: u32,
        cap: Option<CellLevels>,
    ) -> Result<Vec<u8>, String> {
        let queue = &self.queue;
        let held = cap.and_then(|cap| {
            let tiles = self.surface.as_mut()?.tiles.as_mut()?;
            let previous = tiles.cap().clone();
            tiles.set_cap(queue, cap);
            Some(previous)
        });
        let pixels = self.export_image_with(params, target_width, target_height);
        if let Some(previous) = held
            && let Some(tiles) = self.surface.as_mut().and_then(|s| s.tiles.as_mut())
        {
            tiles.set_cap(&self.queue, previous);
        }
        pixels
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the clamp and the scale put a magnitude in a byte before the rounding"
)]
fn planet_instance_bytes(sky: &SkyState) -> [u8; 5 * crate::assets::stars::RECORD_SIZE] {
    let mut bytes = [0; 5 * crate::assets::stars::RECORD_SIZE];
    let eqj_from_world = sky.world_from_eqj.transpose();
    for (index, planet) in sky.planets.iter().enumerate() {
        let record_start = index * crate::assets::stars::RECORD_SIZE;
        let direction = eqj_from_world * planet.direction;
        for (component_index, component) in direction.to_array().into_iter().enumerate() {
            let start = record_start + component_index * 4;
            bytes[start..start + 4].copy_from_slice(&component.to_le_bytes());
        }
        let color = match planet.kind {
            PlanetKind::Mercury => [170, 170, 170],
            PlanetKind::Venus => [255, 250, 235],
            PlanetKind::Mars => [255, 150, 120],
            PlanetKind::Jupiter => [255, 225, 185],
            PlanetKind::Saturn => [255, 235, 160],
        };
        bytes[record_start + 12..record_start + 15].copy_from_slice(&color);
        bytes[record_start + 15] =
            (((planet.magnitude.clamp(-2.0, 8.0) + 2.0) / 10.0) * 255.0).round() as u8;
    }
    bytes
}

#[cfg(test)]
mod tests {
    use crate::scene::sky::PlanetState;

    use super::*;

    #[test]
    fn planet_instances_are_stored_in_eqj_coordinates() {
        let rotation = glam::Mat3::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let planet = PlanetState {
            kind: PlanetKind::Mercury,
            direction: rotation * glam::Vec3::X,
            magnitude: 0.0,
        };
        let sky = SkyState {
            world_from_eqj: rotation,
            sun_direction: glam::Vec3::Z,
            planets: [planet; 5],
            moon_position: glam::Vec3::new(0.0, 0.0, 60.0),
            moon_rotation: glam::Mat3::IDENTITY,
        };
        let bytes = planet_instance_bytes(&sky);
        let component =
            |offset| f32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("four bytes"));
        let stored = glam::Vec3::new(component(0), component(4), component(8));
        assert!(
            stored.abs_diff_eq(glam::Vec3::X, 1.0e-6),
            "stored {stored:?}"
        );
    }
}
