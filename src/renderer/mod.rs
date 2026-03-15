mod gpu_setup;
mod textures;
pub(crate) mod uniforms;

use std::path::PathBuf;
use std::sync::mpsc;

use slint::{ComponentHandle, GraphicsAPI, Image, Model, RenderingState};

use crate::MainWindow;
use crate::camera::OrbitalCamera;
use crate::sun;

use gpu_setup::{
    create_gpu_resources, rebuild_msaa_resources, rebuild_render_textures,
};
use textures::{
    DecodedTextureMessage, TextureSlot, maybe_spawn_texture_load, process_decoded_textures,
    resolve_render_index,
};
use uniforms::Uniforms;

const DEFAULT_WIDTH: u32 = 800;
const DEFAULT_HEIGHT: u32 = 600;
/// Render dimensions are rounded to this granularity to avoid
/// creating new GPU textures on every pixel change during resize.
const SIZE_GRANULARITY: u32 = 64;

/// Texture slot index for the day texture (JXL).
const DAY_SLOT: usize = 1;
/// Texture slot index for the night texture (JXL).
const NIGHT_SLOT: usize = 2;
/// Texture combobox index for the day/night blend mode.
const BLEND_MODE_INDEX: usize = 3;

/// Build the `ComboBox` labels and find the default index (preferring 8x MSAA).
pub fn build_aa_options(supported: &[u32]) -> (Vec<slint::SharedString>, Vec<u32>, i32) {
    let mut labels = Vec::new();
    let mut counts = Vec::new();

    labels.push("None".into());
    counts.push(1);

    for &sc in supported {
        if sc > 1 {
            labels.push(format!("MSAA {sc}\u{d7}").into());
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

/// Quantize width and height to the nearest multiple of `SIZE_GRANULARITY`,
/// with a minimum of one granularity unit in each dimension.
pub(crate) fn quantize_to_granularity(w: u32, h: u32) -> (u32, u32) {
    let qw = (w / SIZE_GRANULARITY).max(1) * SIZE_GRANULARITY;
    let qh = (h / SIZE_GRANULARITY).max(1) * SIZE_GRANULARITY;
    (qw, qh)
}

/// GPU resources created during `RenderingSetup`.
struct GpuResources {
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
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Sender end of the channel for completed texture decodes.
    /// Cloned into each background decode thread.
    texture_tx: mpsc::Sender<DecodedTextureMessage>,
    /// Receiver end of the channel, polled via `try_recv()` in `BeforeRendering`.
    texture_rx: mpsc::Receiver<DecodedTextureMessage>,
    /// Weak reference to the main window, used by background threads to
    /// trigger a redraw via `upgrade_in_event_loop`.
    window_weak: slint::Weak<MainWindow>,
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
}

/// Snapshot of inputs that affect the rendered image.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FrameState {
    pub longitude: f32,
    pub latitude: f32,
    pub zoom: f32,
    pub sample_count: u32,
    pub texture_index: i32,
    pub width: u32,
    pub height: u32,
    /// Sun direction quantized to integer milliradians for stable comparison.
    pub sun_direction: [i32; 3],
    /// Terminator width quantized to integer milliradians.
    pub terminator_width: i32,
    /// Whether diffuse shading is enabled.
    pub diffuse_shading: bool,
    /// Diffuse floor quantized to integer thousandths.
    pub diffuse_floor: i32,
    /// Diffuse ramp quantized to integer thousandths.
    pub diffuse_ramp: i32,
}

/// Build a `FrameState` from raw values, quantizing floats to integer
/// milliradians/thousandths for stable dirty-check comparison.
#[allow(clippy::cast_possible_truncation, clippy::too_many_arguments)]
pub(crate) fn build_frame_state(
    longitude: f32,
    latitude: f32,
    zoom: f32,
    sample_count: u32,
    texture_index: i32,
    render_width: u32,
    render_height: u32,
    sun_dir: glam::Vec3,
    terminator_width: f32,
    diffuse_shading: bool,
    diffuse_floor: f32,
    diffuse_ramp: f32,
) -> FrameState {
    FrameState {
        longitude,
        latitude,
        zoom,
        sample_count,
        texture_index,
        width: render_width,
        height: render_height,
        sun_direction: [
            (sun_dir.x * 1000.0) as i32,
            (sun_dir.y * 1000.0) as i32,
            (sun_dir.z * 1000.0) as i32,
        ],
        terminator_width: (terminator_width * 1000.0) as i32,
        diffuse_shading,
        diffuse_floor: (diffuse_floor * 1000.0) as i32,
        diffuse_ramp: (diffuse_ramp * 1000.0) as i32,
    }
}

/// Register the rendering notifier on the given Slint window.
pub fn setup_rendering_notifier(
    window: &MainWindow,
    aa_counts: Vec<u32>,
    texture_paths: Vec<Option<PathBuf>>,
) {
    let window_weak = window.as_weak();

    window
        .window()
        .set_rendering_notifier(move |state, graphics_api| {
            rendering_callback(
                state,
                graphics_api,
                &window_weak,
                &aa_counts,
                &texture_paths,
            );
        })
        .expect("Failed to set rendering notifier — is the wgpu backend active?");
}

// Persistent state across rendering callbacks, stored in a thread-local.
// The callback is `FnMut` and Slint calls it from the UI thread.
thread_local! {
    static GPU_RESOURCES: std::cell::RefCell<Option<GpuResources>> = const { std::cell::RefCell::new(None) };
}

/// Look up the MSAA sample count from the AA combobox index.
fn lookup_sample_count(win: &MainWindow, aa_counts: &[u32]) -> u32 {
    let idx = usize::try_from(win.get_aa_index()).unwrap_or(0);
    aa_counts.get(idx).copied().unwrap_or(1)
}

/// Read the viewport size from the window, quantized to `SIZE_GRANULARITY`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn quantized_viewport_size(win: &MainWindow) -> (u32, u32) {
    let w = (win.get_viewport_width() * win.window().scale_factor()) as u32;
    let h = (win.get_viewport_height() * win.window().scale_factor()) as u32;
    quantize_to_granularity(w, h)
}

#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
fn rendering_callback(
    state: RenderingState,
    graphics_api: &GraphicsAPI,
    window_weak: &slint::Weak<MainWindow>,
    aa_counts: &[u32],
    texture_paths: &[Option<PathBuf>],
) {
    match state {
        RenderingState::RenderingSetup => {
            let GraphicsAPI::WGPU28 { device, queue, .. } = graphics_api else {
                eprintln!("Expected WGPU28 graphics API, got something else");
                return;
            };

            let (sample_count, width, height) =
                window_weak
                    .upgrade()
                    .map_or((4, DEFAULT_WIDTH, DEFAULT_HEIGHT), |win| {
                        let (w, h) = quantized_viewport_size(&win);
                        (lookup_sample_count(&win, aa_counts), w, h)
                    });
            let resources = create_gpu_resources(
                device.clone(),
                queue.clone(),
                sample_count,
                width,
                height,
                texture_paths,
                window_weak.clone(),
            );
            GPU_RESOURCES.with(|r| {
                *r.borrow_mut() = Some(resources);
            });
        }
        RenderingState::BeforeRendering => {
            let Some(win) = window_weak.upgrade() else {
                return;
            };

            GPU_RESOURCES.with(|r| {
                let mut borrow = r.borrow_mut();
                let Some(res) = borrow.as_mut() else {
                    return;
                };

                // Phase 1: Collect completed background texture decodes
                let received_any = process_decoded_textures(res);

                // Check if sample count changed
                let desired = lookup_sample_count(&win, aa_counts);
                if desired != res.sample_count {
                    rebuild_msaa_resources(res, desired);
                }

                // Check if viewport size changed
                let (vw, vh) = quantized_viewport_size(&win);
                if vw != res.render_width || vh != res.render_height {
                    rebuild_render_textures(res, vw, vh);
                }

                // Compute sun direction for this frame
                let sun_dir = sun::sun_direction_now();

                // Read UI properties for blend mode
                let terminator_width_f = win.get_terminator_width();
                let diffuse_shading = win.get_diffuse_shading();
                let diffuse_floor_f = win.get_diffuse_floor();
                let diffuse_ramp_f = win.get_diffuse_ramp();

                // Build current frame state for dirty-checking
                let current_state = build_frame_state(
                    win.get_camera_longitude(),
                    win.get_camera_latitude(),
                    win.get_camera_zoom(),
                    res.sample_count,
                    win.get_texture_index(),
                    res.render_width,
                    res.render_height,
                    sun_dir,
                    terminator_width_f,
                    diffuse_shading,
                    diffuse_floor_f,
                    diffuse_ramp_f,
                );

                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let raw_index = current_state.texture_index as usize;
                let is_blend_mode = raw_index == BLEND_MODE_INDEX;
                let slot_index = raw_index
                    .min(res.texture_slots.len().saturating_sub(1));

                // Phase 2: Kick off background loading if needed
                if is_blend_mode {
                    maybe_spawn_texture_load(res, DAY_SLOT);
                    maybe_spawn_texture_load(res, NIGHT_SLOT);
                } else {
                    maybe_spawn_texture_load(res, slot_index);
                }

                // Resolve which bind group to use and whether we're in blend mode
                let (bind_group_ref, use_blend_uniforms) = if is_blend_mode {
                    if let Some(composite) = res.composite_bind_group.as_ref() {
                        // Both textures loaded — use composite bind group
                        (composite, true)
                    } else {
                        // Fallback to single-texture while loading
                        let fallback_index = resolve_render_index(res, DAY_SLOT);
                        let bg = res.texture_slots[fallback_index]
                            .bind_group
                            .as_ref()
                            .expect("render_index must always point to a loaded slot");
                        (bg, false)
                    }
                } else {
                    let render_index = resolve_render_index(res, slot_index);
                    let bg = res.texture_slots[render_index]
                        .bind_group
                        .as_ref()
                        .expect("render_index must always point to a loaded slot");
                    (bg, false)
                };

                // Update loading indicator
                if is_blend_mode {
                    let day_loading = res.texture_slots[DAY_SLOT].loading;
                    let night_loading = res.texture_slots[NIGHT_SLOT].loading;
                    let text = match (day_loading, night_loading) {
                        (true, true) => "Loading Day and Night...".to_owned(),
                        (true, false) => "Loading Day...".to_owned(),
                        (false, true) => "Loading Night...".to_owned(),
                        (false, false) => String::new(),
                    };
                    win.set_loading_text(text.into());
                } else {
                    let loading = res.texture_slots[slot_index].loading;
                    if loading {
                        let name = win
                            .get_texture_options()
                            .row_data(slot_index)
                            .unwrap_or_default();
                        win.set_loading_text(format!("Loading {name}...").into());
                    } else {
                        win.set_loading_text(slint::SharedString::default());
                    }
                }

                // Skip rendering if nothing changed since last frame
                // (but always render if we just received a completed decode)
                if !received_any && res.last_state.as_ref() == Some(&current_state) {
                    return;
                }

                let camera = OrbitalCamera::new(
                    current_state.longitude,
                    current_state.latitude,
                    current_state.zoom,
                );
                res.last_state = Some(current_state);

                #[allow(clippy::cast_precision_loss)]
                let aspect = res.render_width as f32 / res.render_height as f32;
                let mvp = camera.mvp_matrix(aspect);

                // Build and write the full uniforms struct
                let uniforms = Uniforms {
                    mvp: mvp.to_cols_array(),
                    sun_dir: sun_dir.into(),
                    terminator_width: if use_blend_uniforms {
                        terminator_width_f
                    } else {
                        -1.0 // sentinel: single-texture mode
                    },
                    flags: u32::from(use_blend_uniforms && diffuse_shading),
                    diffuse_floor: win.get_diffuse_floor(),
                    diffuse_ramp: win.get_diffuse_ramp(),
                    _pad: 0.0,
                };
                res.queue
                    .write_buffer(&res.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

                // Render pass
                let mut encoder =
                    res.device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("sphere_encoder"),
                        });

                let resolve_view = res
                    .render_texture
                    .create_view(&wgpu::TextureViewDescriptor::default());

                // If MSAA is active, render to the multisampled texture and resolve to the output.
                // Otherwise, render directly to the output texture.
                let (color_view, resolve_target) =
                    if let Some(msaa_view) = res.msaa_texture_view.as_ref() {
                        (msaa_view, Some(&resolve_view))
                    } else {
                        (&resolve_view, None)
                    };

                let depth_view = res.msaa_depth_view.as_ref().unwrap_or(&res.depth_texture);

                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("sphere_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: color_view,
                            depth_slice: None,
                            resolve_target,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: 0.02,
                                    g: 0.02,
                                    b: 0.05,
                                    a: 1.0,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: depth_view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(1.0),
                                store: wgpu::StoreOp::Discard,
                            }),
                            stencil_ops: None,
                        }),
                        ..Default::default()
                    });

                    pass.set_pipeline(&res.pipeline);
                    pass.set_bind_group(0, bind_group_ref, &[]);
                    pass.set_vertex_buffer(0, res.vertex_buffer.slice(..));
                    pass.set_index_buffer(res.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..res.index_count, 0, 0..1);
                }

                res.queue.submit(std::iter::once(encoder.finish()));

                // Convert texture to Slint Image
                let image = Image::try_from(res.render_texture.clone())
                    .expect("Failed to convert wgpu texture to Slint image");
                win.set_rendered_image(image);
            });
        }
        RenderingState::RenderingTeardown => {
            GPU_RESOURCES.with(|r| {
                *r.borrow_mut() = None;
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // build_aa_options
    // -----------------------------------------------------------------------

    #[test]
    fn aa_options_single_sample() {
        let (labels, counts, default) = build_aa_options(&[1]);
        assert_eq!(labels.iter().map(slint::SharedString::as_str).collect::<Vec<_>>(), ["None"]);
        assert_eq!(counts, [1]);
        assert_eq!(default, 0);
    }

    #[test]
    fn aa_options_full_range() {
        let (labels, counts, default) = build_aa_options(&[1, 2, 4, 8]);
        assert_eq!(
            labels.iter().map(slint::SharedString::as_str).collect::<Vec<_>>(),
            ["None", "MSAA 2\u{d7}", "MSAA 4\u{d7}", "MSAA 8\u{d7}"]
        );
        assert_eq!(counts, [1, 2, 4, 8]);
        assert_eq!(default, 3); // index of 8x
    }

    #[test]
    fn aa_options_no_8x_falls_back_to_highest() {
        let (labels, counts, default) = build_aa_options(&[1, 2, 4]);
        assert_eq!(
            labels.iter().map(slint::SharedString::as_str).collect::<Vec<_>>(),
            ["None", "MSAA 2\u{d7}", "MSAA 4\u{d7}"]
        );
        assert_eq!(counts, [1, 2, 4]);
        assert_eq!(default, 2); // last entry
    }

    #[test]
    fn aa_options_skip_intermediates() {
        let (labels, counts, default) = build_aa_options(&[1, 8]);
        assert_eq!(
            labels.iter().map(slint::SharedString::as_str).collect::<Vec<_>>(),
            ["None", "MSAA 8\u{d7}"]
        );
        assert_eq!(counts, [1, 8]);
        assert_eq!(default, 1); // index of 8x
    }

    #[test]
    fn aa_options_empty_input() {
        let (labels, counts, default) = build_aa_options(&[]);
        assert_eq!(labels.iter().map(slint::SharedString::as_str).collect::<Vec<_>>(), ["None"]);
        assert_eq!(counts, [1]);
        assert_eq!(default, 0);
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
    // build_frame_state
    // -----------------------------------------------------------------------

    /// Helper: build a frame state with typical values, allowing overrides.
    fn default_frame_state() -> FrameState {
        build_frame_state(
            10.0,          // longitude
            20.0,          // latitude
            3.5,           // zoom
            4,             // sample_count
            0,             // texture_index
            1920,          // render_width
            1080,          // render_height
            glam::Vec3::new(0.1234, -0.5678, 0.9012), // sun_dir
            0.15,          // terminator_width
            true,          // diffuse_shading
            0.1,           // diffuse_floor
            0.6,           // diffuse_ramp
        )
    }

    #[test]
    fn frame_state_sun_direction_quantization() {
        let state = default_frame_state();
        // (0.1234 * 1000) as i32 = 123 (truncation toward zero)
        assert_eq!(state.sun_direction[0], 123);
        // (-0.5678 * 1000) as i32 = -567 (truncation toward zero)
        assert_eq!(state.sun_direction[1], -567);
        // (0.9012 * 1000) as i32 = 901
        assert_eq!(state.sun_direction[2], 901);
    }

    #[test]
    fn frame_state_terminator_width_quantization() {
        let state = default_frame_state();
        assert_eq!(state.terminator_width, 150); // 0.15 * 1000 = 150
    }

    #[test]
    fn frame_state_diffuse_params_quantization() {
        let state = default_frame_state();
        assert_eq!(state.diffuse_floor, 100); // 0.1 * 1000 = 100
        assert_eq!(state.diffuse_ramp, 600);  // 0.6 * 1000 = 600
    }

    #[test]
    fn frame_state_sub_threshold_change_compares_equal() {
        // Use a base value (0.1230) where adding 0.0005 stays in the same
        // integer bucket: (0.1230 * 1000) as i32 = 123, (0.1235 * 1000) as i32 = 123
        let state_a = build_frame_state(
            10.0, 20.0, 3.5, 4, 0, 1920, 1080,
            glam::Vec3::new(0.1230, -0.5670, 0.9010),
            0.15, true, 0.1, 0.6,
        );
        let state_b = build_frame_state(
            10.0, 20.0, 3.5, 4, 0, 1920, 1080,
            glam::Vec3::new(0.1235, -0.5670, 0.9010),
            0.15, true, 0.1, 0.6,
        );
        assert_eq!(state_a, state_b, "Sub-threshold changes should compare equal");
    }

    #[test]
    fn frame_state_at_threshold_change_compares_different() {
        // 0.1230 -> 123, 0.1240 -> 124: one quantum apart
        let state_a = build_frame_state(
            10.0, 20.0, 3.5, 4, 0, 1920, 1080,
            glam::Vec3::new(0.1230, -0.5670, 0.9010),
            0.15, true, 0.1, 0.6,
        );
        let state_b = build_frame_state(
            10.0, 20.0, 3.5, 4, 0, 1920, 1080,
            glam::Vec3::new(0.1240, -0.5670, 0.9010),
            0.15, true, 0.1, 0.6,
        );
        assert_ne!(state_a, state_b, "At-threshold changes should compare different");
    }

    #[test]
    fn frame_state_each_field_triggers_dirty() {
        let base = default_frame_state();

        // Changing longitude
        let modified = build_frame_state(
            11.0, 20.0, 3.5, 4, 0, 1920, 1080,
            glam::Vec3::new(0.1234, -0.5678, 0.9012),
            0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "longitude change should trigger dirty");

        // Changing latitude
        let modified = build_frame_state(
            10.0, 21.0, 3.5, 4, 0, 1920, 1080,
            glam::Vec3::new(0.1234, -0.5678, 0.9012),
            0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "latitude change should trigger dirty");

        // Changing zoom
        let modified = build_frame_state(
            10.0, 20.0, 4.0, 4, 0, 1920, 1080,
            glam::Vec3::new(0.1234, -0.5678, 0.9012),
            0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "zoom change should trigger dirty");

        // Changing sample_count
        let modified = build_frame_state(
            10.0, 20.0, 3.5, 8, 0, 1920, 1080,
            glam::Vec3::new(0.1234, -0.5678, 0.9012),
            0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "sample_count change should trigger dirty");

        // Changing texture_index
        let modified = build_frame_state(
            10.0, 20.0, 3.5, 4, 1, 1920, 1080,
            glam::Vec3::new(0.1234, -0.5678, 0.9012),
            0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "texture_index change should trigger dirty");

        // Changing width
        let modified = build_frame_state(
            10.0, 20.0, 3.5, 4, 0, 1024, 1080,
            glam::Vec3::new(0.1234, -0.5678, 0.9012),
            0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "width change should trigger dirty");

        // Changing height
        let modified = build_frame_state(
            10.0, 20.0, 3.5, 4, 0, 1920, 720,
            glam::Vec3::new(0.1234, -0.5678, 0.9012),
            0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "height change should trigger dirty");

        // Changing sun_direction
        let modified = build_frame_state(
            10.0, 20.0, 3.5, 4, 0, 1920, 1080,
            glam::Vec3::new(0.5, -0.5678, 0.9012),
            0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "sun_direction change should trigger dirty");

        // Changing terminator_width
        let modified = build_frame_state(
            10.0, 20.0, 3.5, 4, 0, 1920, 1080,
            glam::Vec3::new(0.1234, -0.5678, 0.9012),
            0.25, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "terminator_width change should trigger dirty");

        // Changing diffuse_shading
        let modified = build_frame_state(
            10.0, 20.0, 3.5, 4, 0, 1920, 1080,
            glam::Vec3::new(0.1234, -0.5678, 0.9012),
            0.15, false, 0.1, 0.6,
        );
        assert_ne!(base, modified, "diffuse_shading change should trigger dirty");

        // Changing diffuse_floor
        let modified = build_frame_state(
            10.0, 20.0, 3.5, 4, 0, 1920, 1080,
            glam::Vec3::new(0.1234, -0.5678, 0.9012),
            0.15, true, 0.2, 0.6,
        );
        assert_ne!(base, modified, "diffuse_floor change should trigger dirty");

        // Changing diffuse_ramp
        let modified = build_frame_state(
            10.0, 20.0, 3.5, 4, 0, 1920, 1080,
            glam::Vec3::new(0.1234, -0.5678, 0.9012),
            0.15, true, 0.1, 0.7,
        );
        assert_ne!(base, modified, "diffuse_ramp change should trigger dirty");
    }
}
