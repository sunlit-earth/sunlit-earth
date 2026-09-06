//! The wgpu render pipeline.
//!
//! `Renderer` owns every GPU object and renders offscreen into its own texture.
//! It knows nothing about windows, event loops, or Slint: callers hand it a
//! `SceneParams` plus a sky state and read the frame back as pixels.

mod frame;
mod gpu_setup;
mod render_pass;
mod sizing;
mod slots;
mod texture_routing;
mod textures;
pub mod uniforms;

pub use render_pass::read_texture_rgba8;
pub use sizing::build_aa_options;
pub use slots::{SlotLayout, TEXTURE_LABELS};

pub(crate) use sizing::{quantize_to_granularity, resolve_sample_count};

use std::path::PathBuf;

use tracing::debug;

use crate::assets::cloud_fetcher::NotifyFn;
use crate::assets::mailbox::TextureMailbox;
use crate::memory_report::{ExpectedTexture, MemoryReport};
use crate::params::SceneParams;
use crate::scene::sky::{PlanetKind, SkyState};

use frame::{FrameState, build_frame_state};
use gpu_setup::{
    COLOR_FORMAT, DEPTH_FORMAT, Pipelines, create_render_textures, rebuild_msaa_resources,
    rebuild_render_textures,
};
use slots::{SLOT_LABELS, TextureMode};
use textures::{TextureSlot, maybe_spawn_texture_load, process_decoded_textures};

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
    /// One entry per file-backed texture slot, in slot order after the grid.
    pub texture_paths: Vec<Option<PathBuf>>,
    /// Width the file-backed textures are loaded at, as a cap: a source
    /// narrower than this is loaded as it is.
    pub texture_resolution: u32,
    /// Where downscaled copies of the file-backed textures are kept. `None`
    /// re-derives them on every run.
    pub texture_cache_dir: Option<PathBuf>,
    /// Shared with the cloud fetcher and the background decode threads.
    pub mailbox: TextureMailbox,
    /// Invoked from decode threads once a result has been parked.
    pub notify: NotifyFn,
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
    sampler: wgpu::Sampler,
    texture_slots: Vec<TextureSlot>,
    /// Width the file-backed slots load at, as a cap.
    texture_resolution: u32,
    /// Bumped on every resolution change, and stamped on each load it spawns,
    /// so a decode that was already running when the width changed is
    /// recognizable as something nobody asked for any more.
    texture_generation: u64,
    /// Where downscaled copies of the file-backed textures are kept.
    texture_cache_dir: Option<PathBuf>,
    /// Index of the most recently successfully rendered texture slot.
    /// Used as fallback when the requested slot is not loaded or fails.
    last_rendered_index: usize,
    depth_texture: wgpu::TextureView,
    render_texture: wgpu::Texture,
    msaa_texture_view: Option<wgpu::TextureView>,
    msaa_depth_view: Option<wgpu::TextureView>,
    sample_count: u32,
    render_width: u32,
    render_height: u32,
    /// Last rendered state for dirty-checking. `None` means first frame.
    last_state: Option<FrameState>,
    /// Scene parameters from the last rendered frame, replayed by the export
    /// path so the wallpaper matches what the preview shows.
    last_params: Option<SceneParams>,
    /// Per-frame inputs (sky state, blend flag) from the last rendered frame.
    /// The scheduler overwrites the sky here so a hidden window still exports
    /// with current astronomy.
    last_inputs: Option<render_pass::FrameInputs>,
    /// Bind group resolution from the last rendered frame, used by the
    /// export path to reuse the same texture binding without re-resolving.
    last_resolved: Option<texture_routing::ResolvedTexture>,
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
    /// 1x1 black texture standing in at binding 3 for every bind group that
    /// reads one texture: the Grid, Day and Night modes, and the cloud overlay.
    dummy_texture_view: wgpu::TextureView,
    /// Bind group containing both day and night textures, used in blend mode.
    /// Created once both day and night texture slots have loaded.
    composite_bind_group: Option<wgpu::BindGroup>,
    /// Stored texture view for the day texture, needed to build the composite
    /// bind group when both become available.
    day_texture_view: Option<wgpu::TextureView>,
    /// Stored texture view for the night texture, needed to build the composite
    /// bind group when both become available.
    night_texture_view: Option<wgpu::TextureView>,
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
    /// are freed before the reload is spawned, so going down actually lowers
    /// the process's footprint rather than adding to it; the caller is expected
    /// to mark itself dirty, since the next frame falls back to the procedural
    /// grid until the new textures arrive.
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
    /// Slot textures report their own shape, because the renderer holds them.
    /// The depth and MSAA targets and the 1x1 placeholder are kept only as
    /// views, so their rows are computed from the size, format, and sample
    /// count they were created with; `create_render_textures` is the other side
    /// of that agreement, and the two formats are shared constants so the rows
    /// cannot drift from the descriptors.
    fn expected_textures(&self) -> Vec<ExpectedTexture> {
        let target = |label: &str, format, sample_count| ExpectedTexture {
            label: label.to_owned(),
            width: self.render_width,
            height: self.render_height,
            format,
            mip_levels: 1,
            sample_count,
        };

        let mut expected: Vec<ExpectedTexture> = self
            .texture_slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let texture = slot.texture.as_ref()?;
                Some(ExpectedTexture {
                    label: self.slot_label(index),
                    width: texture.width(),
                    height: texture.height(),
                    format: texture.format(),
                    mip_levels: texture.mip_level_count(),
                    sample_count: texture.sample_count(),
                })
            })
            .collect();

        expected.push(ExpectedTexture {
            label: "dummy_1x1".to_owned(),
            width: 1,
            height: 1,
            format: COLOR_FORMAT,
            mip_levels: 1,
            sample_count: 1,
        });
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

    /// Whether every texture the current mode needs has finished loading.
    /// The clouds, the Moon and the Milky Way are excluded: they are overlays,
    /// not requirements.
    pub(crate) fn textures_ready(&self, texture_index: i32) -> bool {
        let layout = self.layout();
        let mode = TextureMode::from_index(texture_index);
        if mode == TextureMode::Blend {
            self.slot_loaded(layout.globe(TextureMode::Day))
                && self.slot_loaded(layout.globe(TextureMode::Night))
                && self.composite_bind_group.is_some()
        } else {
            self.slot_loaded(layout.globe(mode))
        }
    }

    fn slot_loaded(&self, slot: usize) -> bool {
        let slot = &self.texture_slots[slot];
        slot.bind_group.is_some() && !slot.loading
    }

    /// Whether a texture the current mode needs is still on its way.
    ///
    /// True from the moment a resolution switch purges a slot until its reload
    /// lands, and from startup until the first load does. Deliberately not the
    /// negation of `textures_ready`: a slot with no file behind it, and one
    /// whose decode failed and had its path cleared, are both terminal states
    /// where nothing further is coming, so there is nothing to wait for.
    pub(crate) fn textures_pending(&self, texture_index: i32) -> bool {
        let layout = self.layout();
        let mode = TextureMode::from_index(texture_index);
        if mode == TextureMode::Blend {
            self.slot_pending(layout.globe(TextureMode::Day))
                || self.slot_pending(layout.globe(TextureMode::Night))
        } else {
            self.slot_pending(layout.globe(mode))
        }
    }

    fn slot_pending(&self, slot: usize) -> bool {
        let slot = &self.texture_slots[slot];
        slot.source_path.is_some() && slot.bind_group.is_none()
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
    /// Kicks off background texture loads for the selected mode whether or not
    /// the frame is skipped, so a mode switch starts loading immediately.
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
        // Like the globe's loads, this happens whether or not the frame is
        // skipped.
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

        let bind_group_ref = match &resolved {
            texture_routing::ResolvedTexture::Composite => self
                .composite_bind_group
                .as_ref()
                .expect("composite bind group must exist when Composite is returned"),
            texture_routing::ResolvedTexture::Slot(idx) => self.texture_slots[*idx]
                .bind_group
                .as_ref()
                .expect("render_index must always point to a loaded slot"),
        };
        self.last_resolved = Some(resolved);
        self.last_params = Some(*params);
        self.last_inputs = Some(render_pass::FrameInputs {
            sky: sky.clone(),
            use_blend,
        });
        self.update_planets(sky);

        let first_frame = self.last_state.is_none();
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

    /// Render the last frame's scene at a different resolution and return raw
    /// RGBA8 pixels.
    ///
    /// Creates temporary GPU textures with `COPY_SRC` at the target resolution,
    /// renders using the existing pipeline and bind groups (matching the last
    /// displayed frame), reads back the pixels, and drops the temporaries.
    ///
    /// Returns `Err` when no frame has been rendered yet, since there is then
    /// no resolved texture binding to replay.
    pub(crate) fn export_image(
        &self,
        target_width: u32,
        target_height: u32,
    ) -> Result<Vec<u8>, String> {
        let params = self.last_params.ok_or("No frame rendered yet")?;
        self.export_image_with(&params, target_width, target_height)
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
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn export_image_with(
        &self,
        params: &SceneParams,
        target_width: u32,
        target_height: u32,
    ) -> Result<Vec<u8>, String> {
        let inputs = self.last_inputs.as_ref().ok_or("No frame rendered yet")?;

        let bind_group = match self.last_resolved.as_ref().ok_or("No frame rendered yet")? {
            texture_routing::ResolvedTexture::Composite => self
                .composite_bind_group
                .as_ref()
                .ok_or("No bind group available")?,
            texture_routing::ResolvedTexture::Slot(idx) => self.texture_slots[*idx]
                .bind_group
                .as_ref()
                .ok_or("No bind group available")?,
        };

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
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
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
