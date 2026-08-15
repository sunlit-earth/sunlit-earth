//! The wgpu render pipeline.
//!
//! `Renderer` owns every GPU object and renders offscreen into its own texture.
//! It knows nothing about windows, event loops, or Slint: callers hand it a
//! `SceneParams` plus a sun direction and either read the preview texture back
//! or bind it directly.

mod frame;
mod gpu_setup;
mod render_pass;
mod texture_routing;
mod textures;
pub(crate) mod uniforms;

pub use render_pass::read_texture_rgba8;

use std::path::PathBuf;

use glam::Vec3;
use tracing::debug;

use crate::assets::cloud_fetcher::NotifyFn;
use crate::assets::mailbox::TextureMailbox;
use crate::params::SceneParams;

use frame::{FrameState, build_frame_state};
use gpu_setup::{create_render_textures, rebuild_msaa_resources, rebuild_render_textures};
use textures::{TextureSlot, process_decoded_textures};

/// Render dimensions are rounded to this granularity to avoid
/// creating new GPU textures on every pixel change during resize.
const SIZE_GRANULARITY: u32 = 64;

/// Texture slot index for the day texture (JXL).
const DAY_SLOT: usize = 1;
/// Texture slot index for the night texture (JXL).
const NIGHT_SLOT: usize = 2;
/// Texture combobox index for the day/night blend mode.
const BLEND_MODE_INDEX: usize = 3;
/// Texture slot index for the cloud overlay texture.
/// This shares its numeric value with `BLEND_MODE_INDEX` (a combobox index),
/// but the two are used in different contexts: `CLOUDS_SLOT` indexes into
/// `texture_slots` for loading/bind-group creation, while `BLEND_MODE_INDEX`
/// is compared against the UI combobox value in `resolve_textures`.
pub const CLOUDS_SLOT: usize = 3;

/// Display names of the texture modes, in combo box order. The index into this
/// array is `SceneParams::texture_index`.
pub const TEXTURE_LABELS: [&str; 4] = ["Grid", "Day", "Night", "Day/Night Blend"];

/// Build the anti-aliasing option labels and find the default index
/// (preferring 8x MSAA).
///
/// `max_samples` is the quality tier's cap. Filtering here rather than
/// clamping inside the renderer keeps the combo box honest: it never offers a
/// setting the tier would silently ignore.
pub fn build_aa_options(supported: &[u32], max_samples: u32) -> (Vec<String>, Vec<u32>, i32) {
    let mut labels = vec!["None".to_owned()];
    let mut counts = vec![1];

    for &sc in supported {
        if sc > 1 && sc <= max_samples {
            labels.push(format!("MSAA {sc}\u{d7}"));
            counts.push(sc);
        }
    }

    // Default to 8x if available, otherwise the highest available option
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let default_index = counts
        .iter()
        .position(|&c| c == 8)
        .unwrap_or(counts.len() - 1) as i32;

    (labels, counts, default_index)
}

/// Resolve a requested MSAA sample count against what the adapter supports and
/// what the quality tier allows.
///
/// A sample count the adapter does not support is not a warning inside wgpu, it
/// is a validation error that kills whichever thread creates the texture, so
/// this has to happen before any render target is built. The rule mirrors the
/// combo box: take the highest supported count that is at most the requested
/// one, and if the request is below everything on offer, take the lowest thing
/// on offer instead. `1` is always a valid answer.
pub fn resolve_sample_count(requested: u32, supported: &[u32], max_samples: u32) -> u32 {
    let allowed: Vec<u32> = supported
        .iter()
        .copied()
        .filter(|&c| c >= 1 && c <= max_samples)
        .collect();
    if allowed.is_empty() {
        return 1;
    }
    allowed
        .iter()
        .copied()
        .filter(|&c| c <= requested)
        .max()
        .or_else(|| allowed.iter().copied().min())
        .unwrap_or(1)
}

/// Quantize width and height to the nearest multiple of `SIZE_GRANULARITY`,
/// with a minimum of one granularity unit in each dimension.
pub fn quantize_to_granularity(w: u32, h: u32) -> (u32, u32) {
    let qw = (w / SIZE_GRANULARITY).max(1) * SIZE_GRANULARITY;
    let qh = (h / SIZE_GRANULARITY).max(1) * SIZE_GRANULARITY;
    (qw, qh)
}

/// What a call to [`Renderer::render`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderOutcome {
    /// Nothing changed since the last frame; the preview texture still holds it.
    Skipped,
    /// A new frame was drawn into the preview texture.
    Rendered { first_frame: bool },
}

/// Everything needed to build a [`Renderer`] beyond the device and queue.
pub struct RendererConfig {
    pub sample_count: u32,
    pub width: u32,
    pub height: u32,
    /// One entry per file-backed texture slot, in slot order after the grid.
    pub texture_paths: Vec<Option<PathBuf>>,
    /// Shared with the cloud fetcher and the background decode threads.
    pub mailbox: TextureMailbox,
    /// Invoked from decode threads once a result has been parked.
    pub notify: NotifyFn,
}

/// The GPU pipeline and every resource it owns.
pub struct Renderer {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform_buffer: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    texture_slots: Vec<TextureSlot>,
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
    /// Per-frame inputs (sun direction, blend flag) from the last rendered
    /// frame. The scheduler overwrites `sun_dir` here so a hidden window still
    /// exports with the current sun position.
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
    /// 1x1 black texture used as the night texture placeholder in single-texture
    /// bind groups (Grid, Day, Night modes).
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
    /// Render pipeline for the Rayleigh scattering atmosphere shell.
    rayleigh_pipeline: wgpu::RenderPipeline,
    /// Render pipeline for the orange nightglow atmosphere shell.
    nightglow_orange_pipeline: wgpu::RenderPipeline,
    /// Render pipeline for the green nightglow atmosphere shell.
    nightglow_green_pipeline: wgpu::RenderPipeline,
    /// Render pipeline for the cloud overlay sphere.
    cloud_pipeline: wgpu::RenderPipeline,
    /// Bind group for the cloud texture (populated after async load completes).
    cloud_bind_group: Option<wgpu::BindGroup>,
    /// Stored texture view for the cloud texture, used to rebuild the bind group.
    cloud_texture_view: Option<wgpu::TextureView>,
}

impl Renderer {
    /// Create the pipeline, the sphere mesh, the grid texture, and the
    /// offscreen render targets.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, config: RendererConfig) -> Self {
        gpu_setup::create_renderer(device, queue, config)
    }

    /// The offscreen color target holding the most recent frame.
    pub fn preview_texture(&self) -> &wgpu::Texture {
        &self.render_texture
    }

    pub fn size(&self) -> (u32, u32) {
        (self.render_width, self.render_height)
    }

    /// Whether a frame has ever been drawn into the preview texture.
    pub fn has_frame(&self) -> bool {
        self.last_state.is_some()
    }

    /// Resize the offscreen targets. The caller is expected to have quantized
    /// the size already; identical sizes are a no-op.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == self.render_width && height == self.render_height {
            return;
        }
        debug!(width, height, "viewport size changed");
        rebuild_render_textures(self, width, height);
    }

    /// Upload every decoded texture parked in the mailbox.
    ///
    /// Deliberately independent of whether anything is being displayed: this is
    /// what keeps cloud updates flowing to the GPU while the window is hidden.
    /// Returns `true` if at least one texture was uploaded.
    pub fn drain_texture_updates(&mut self) -> bool {
        process_decoded_textures(self)
    }

    /// The loading indicator text for the current texture selection, empty when
    /// nothing is loading.
    pub fn loading_text(&self, texture_index: i32) -> String {
        texture_routing::loading_text(self, slot_of(texture_index))
    }

    /// Whether every texture the current mode needs has finished loading.
    /// Clouds are excluded: they are an overlay, not a requirement.
    pub fn textures_ready(&self, texture_index: i32) -> bool {
        let raw_index = slot_of(texture_index);
        if raw_index == BLEND_MODE_INDEX {
            self.slot_loaded(DAY_SLOT)
                && self.slot_loaded(NIGHT_SLOT)
                && self.composite_bind_group.is_some()
        } else {
            self.slot_loaded(raw_index.min(self.texture_slots.len().saturating_sub(1)))
        }
    }

    fn slot_loaded(&self, slot: usize) -> bool {
        let slot = &self.texture_slots[slot];
        slot.bind_group.is_some() && !slot.loading
    }

    /// Overwrite the sun direction the export path will use.
    ///
    /// The scheduler calls this before an unattended wallpaper export so the
    /// image reflects the current time even when no frame has been drawn since
    /// the window was hidden.
    pub fn set_sun_direction(&mut self, sun_dir: Vec3) {
        if let Some(inputs) = &mut self.last_inputs {
            inputs.sun_dir = sun_dir;
        }
    }

    /// Draw a frame into the preview texture, unless nothing changed.
    ///
    /// Kicks off background texture loads for the selected mode whether or not
    /// the frame is skipped, so a mode switch starts loading immediately.
    pub fn render(&mut self, params: &SceneParams, sun_dir: Vec3) -> RenderOutcome {
        if params.sample_count != self.sample_count {
            debug!(
                sample_count = params.sample_count,
                "MSAA sample count changed"
            );
            rebuild_msaa_resources(self, params.sample_count);
        }

        let received_any = std::mem::take(&mut self.texture_dirty);
        let current_state =
            build_frame_state(params, self.render_width, self.render_height, sun_dir);
        let raw_index = slot_of(params.texture_index);

        let (resolved, use_blend) = texture_routing::resolve_textures(self, raw_index);

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
        self.last_inputs = Some(render_pass::FrameInputs { sun_dir, use_blend });

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

    /// Read the preview texture back into RGBA8 pixels.
    ///
    /// Only valid when the renderer was built with `COPY_SRC` on its preview
    /// texture (see [`RendererConfig`] users that need readback).
    pub fn read_preview_pixels(&self) -> Vec<u8> {
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
    #[allow(clippy::cast_precision_loss)]
    pub fn export_image(&self, target_width: u32, target_height: u32) -> Result<Vec<u8>, String> {
        let params = self.last_params.as_ref().ok_or("No frame rendered yet")?;
        let inputs = self.last_inputs.as_ref().ok_or("No frame rendered yet")?;

        // Look up the bind group that was used for the last rendered frame
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

        // Create temporary render textures with COPY_SRC for readback
        let (export_texture, export_depth, msaa_color_view, msaa_depth_view) =
            create_render_textures(
                &self.device,
                target_width,
                target_height,
                self.sample_count,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            );

        let aspect = target_width as f32 / target_height as f32;
        render_pass::write_uniforms(&self.queue, &self.uniform_buffer, params, aspect, inputs);

        let resolve_view = export_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let target = render_pass::RenderTarget::new(
            &resolve_view,
            msaa_color_view.as_ref(),
            &export_depth,
            msaa_depth_view.as_ref(),
        );

        let overlays = render_pass::Overlays::select(self, params, bind_group);

        crate::memory::log_memory_usage("wallpaper: before render");
        render_pass::encode_and_submit(
            &self.device,
            &self.queue,
            &target,
            &self.pipeline,
            bind_group,
            &self.vertex_buffer,
            &self.index_buffer,
            self.index_count,
            overlays.rayleigh.0,
            overlays.rayleigh.1,
            overlays.nightglow_orange.0,
            overlays.nightglow_orange.1,
            overlays.nightglow_green.0,
            overlays.nightglow_green.1,
            overlays.cloud.0,
            overlays.cloud.1,
        );

        crate::memory::log_memory_usage("wallpaper: before pixel readback");
        let pixels = read_texture_rgba8(
            &self.device,
            &self.queue,
            &export_texture,
            target_width,
            target_height,
        );
        crate::memory::log_memory_usage("wallpaper: after pixel readback");
        Ok(pixels)
    }
}

/// Clamp a (possibly negative) combo box index into a slot index.
#[allow(clippy::cast_sign_loss)]
fn slot_of(texture_index: i32) -> usize {
    texture_index.max(0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // build_aa_options
    // -----------------------------------------------------------------------

    #[test]
    fn aa_options_single_sample() {
        let (labels, counts, default) = build_aa_options(&[1], u32::MAX);
        assert_eq!(labels, ["None"]);
        assert_eq!(counts, [1]);
        assert_eq!(default, 0);
    }

    #[test]
    fn aa_options_full_range() {
        let (labels, counts, default) = build_aa_options(&[1, 2, 4, 8], u32::MAX);
        assert_eq!(
            labels,
            ["None", "MSAA 2\u{d7}", "MSAA 4\u{d7}", "MSAA 8\u{d7}"]
        );
        assert_eq!(counts, [1, 2, 4, 8]);
        assert_eq!(default, 3); // index of 8x
    }

    #[test]
    fn aa_options_no_8x_falls_back_to_highest() {
        let (labels, counts, default) = build_aa_options(&[1, 2, 4], u32::MAX);
        assert_eq!(labels, ["None", "MSAA 2\u{d7}", "MSAA 4\u{d7}"]);
        assert_eq!(counts, [1, 2, 4]);
        assert_eq!(default, 2); // last entry
    }

    #[test]
    fn aa_options_skip_intermediates() {
        let (labels, counts, default) = build_aa_options(&[1, 8], u32::MAX);
        assert_eq!(labels, ["None", "MSAA 8\u{d7}"]);
        assert_eq!(counts, [1, 8]);
        assert_eq!(default, 1); // index of 8x
    }

    #[test]
    fn aa_options_empty_input() {
        let (labels, counts, default) = build_aa_options(&[], u32::MAX);
        assert_eq!(labels, ["None"]);
        assert_eq!(counts, [1]);
        assert_eq!(default, 0);
    }

    #[test]
    fn aa_options_respect_the_tier_cap() {
        let (labels, counts, default) = build_aa_options(&[1, 2, 4, 8], 1);
        assert_eq!(labels, ["None"], "the low tier offers no multisampling");
        assert_eq!(counts, [1]);
        assert_eq!(default, 0);

        let (_, counts, _) = build_aa_options(&[1, 2, 4, 8], 4);
        assert_eq!(counts, [1, 2, 4], "the medium tier stops at 4x");
    }

    // -----------------------------------------------------------------------
    // resolve_sample_count
    // -----------------------------------------------------------------------

    /// What a typical desktop adapter reports for `Rgba8Unorm`.
    const FULL: [u32; 4] = [1, 2, 4, 8];

    #[test]
    fn supported_request_is_honored() {
        assert_eq!(resolve_sample_count(4, &FULL, u32::MAX), 4);
        assert_eq!(resolve_sample_count(8, &FULL, u32::MAX), 8);
        assert_eq!(resolve_sample_count(1, &FULL, u32::MAX), 1);
    }

    #[test]
    fn unsupported_request_falls_back_to_the_next_lower_option() {
        // The config asking for something absurd must not reach wgpu.
        assert_eq!(resolve_sample_count(64, &FULL, u32::MAX), 8);
        assert_eq!(resolve_sample_count(3, &FULL, u32::MAX), 2);
        assert_eq!(resolve_sample_count(0, &FULL, u32::MAX), 1);
    }

    #[test]
    fn adapter_without_8x_never_yields_8x() {
        assert_eq!(resolve_sample_count(8, &[1, 2, 4], u32::MAX), 4);
        assert_eq!(resolve_sample_count(8, &[1], u32::MAX), 1);
    }

    #[test]
    fn tier_cap_applies_on_top_of_adapter_support() {
        assert_eq!(resolve_sample_count(8, &FULL, 1), 1);
        assert_eq!(resolve_sample_count(8, &FULL, 4), 4);
        assert_eq!(resolve_sample_count(2, &FULL, 4), 2);
    }

    #[test]
    fn a_request_below_everything_offered_takes_the_lowest_option() {
        // An adapter that does not list 1x is not something we have seen, but
        // returning 0 or the request unchanged would be a validation error.
        assert_eq!(resolve_sample_count(1, &[4, 8], u32::MAX), 4);
    }

    #[test]
    fn an_empty_or_fully_filtered_list_still_yields_a_valid_count() {
        assert_eq!(resolve_sample_count(8, &[], u32::MAX), 1);
        assert_eq!(resolve_sample_count(8, &[4, 8], 2), 1);
    }

    #[test]
    fn every_resolution_is_actually_supported() {
        for supported in [vec![1], vec![1, 4], FULL.to_vec(), vec![1, 2, 4, 8, 16]] {
            for requested in [0, 1, 2, 3, 4, 7, 8, 16, 64, u32::MAX] {
                for cap in [1, 4, u32::MAX] {
                    let resolved = resolve_sample_count(requested, &supported, cap);
                    assert!(
                        supported.contains(&resolved) || resolved == 1,
                        "resolved {resolved} is not in {supported:?} \
                         (requested {requested}, cap {cap})"
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // quantize_to_granularity
    // -----------------------------------------------------------------------

    #[test]
    fn quantize_zero_gives_one_granularity() {
        assert_eq!(quantize_to_granularity(0, 0), (64, 64));
    }

    #[test]
    fn quantize_below_granularity() {
        assert_eq!(quantize_to_granularity(63, 63), (64, 64));
    }

    #[test]
    fn quantize_exact_boundary() {
        assert_eq!(quantize_to_granularity(64, 64), (64, 64));
    }

    #[test]
    fn quantize_just_above_boundary() {
        assert_eq!(quantize_to_granularity(65, 65), (64, 64));
    }

    #[test]
    fn quantize_double_boundary() {
        assert_eq!(quantize_to_granularity(128, 128), (128, 128));
    }

    #[test]
    fn quantize_mixed_dimensions() {
        assert_eq!(quantize_to_granularity(129, 200), (128, 192));
    }

    #[test]
    fn quantize_typical_display() {
        assert_eq!(quantize_to_granularity(1920, 1080), (1920, 1024));
    }

    // -----------------------------------------------------------------------
    // slot_of
    // -----------------------------------------------------------------------

    #[test]
    fn slot_of_clamps_negative_indices() {
        assert_eq!(slot_of(-1), 0);
        assert_eq!(slot_of(0), 0);
        assert_eq!(slot_of(3), 3);
    }

    #[test]
    fn texture_labels_cover_every_mode() {
        assert_eq!(TEXTURE_LABELS.len(), BLEND_MODE_INDEX + 1);
    }
}
