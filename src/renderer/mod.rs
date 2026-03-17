mod frame;
mod gpu_setup;
mod render_pass;
mod texture_routing;
mod textures;
pub(crate) mod uniforms;

pub use render_pass::read_texture_rgba8;

use std::path::PathBuf;
use std::sync::mpsc;

use slint::{ComponentHandle, GraphicsAPI, RenderingState};

use crate::MainWindow;
use crate::scene::camera::{CameraParams, zoom_to_distance};
use crate::scene::datetime;
use crate::scene::sun;

use frame::{FrameState, build_frame_state};
use gpu_setup::{
    create_gpu_resources, create_render_textures, rebuild_msaa_resources,
    rebuild_render_textures,
};
use textures::{DecodedTextureMessage, TextureSlot, process_decoded_textures};

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

/// Render the current scene at the given resolution and return raw RGBA8 pixels.
///
/// Creates temporary GPU textures with `COPY_SRC` at the target resolution,
/// renders using the existing pipeline and bind groups (matching the last
/// displayed frame), reads back the pixels, and drops the temporary textures.
///
/// Returns `Err` if the GPU is not initialized, textures are still loading,
/// or no frame has been rendered yet.
#[allow(clippy::cast_precision_loss)]
pub fn export_wallpaper_image(target_width: u32, target_height: u32) -> Result<Vec<u8>, String> {
    GPU_RESOURCES.with(|r| {
        let borrow = r.borrow();
        let res = borrow.as_ref().ok_or("GPU not initialized")?;

        let state = res
            .last_state
            .as_ref()
            .ok_or("No frame rendered yet")?;
        let shading = res
            .last_shading
            .as_ref()
            .ok_or("No frame rendered yet")?;

        // Look up the bind group that was used for the last rendered frame
        let bind_group = match res
            .last_resolved
            .as_ref()
            .ok_or("No frame rendered yet")?
        {
            texture_routing::ResolvedTexture::Composite => res
                .composite_bind_group
                .as_ref()
                .ok_or("No bind group available")?,
            texture_routing::ResolvedTexture::Slot(idx) => res.texture_slots[*idx]
                .bind_group
                .as_ref()
                .ok_or("No bind group available")?,
        };

        // Create temporary render textures with COPY_SRC for readback
        let (export_texture, export_depth, msaa_color_view, msaa_depth_view) =
            create_render_textures(
                &res.device,
                target_width,
                target_height,
                res.sample_count,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            );

        let aspect = target_width as f32 / target_height as f32;
        let camera = CameraParams {
            longitude: state.longitude,
            latitude: state.latitude,
            zoom: state.zoom,
            offset_x: state.offset_x,
            offset_y: state.offset_y,
            tilt_deg: state.tilt,
            yaw_deg: state.yaw,
            pitch_deg: state.pitch,
        };
        render_pass::write_uniforms(
            &res.queue,
            &res.uniform_buffer,
            &camera,
            aspect,
            shading,
        );

        let resolve_view = export_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let target = render_pass::RenderTarget::new(
            &resolve_view,
            msaa_color_view.as_ref(),
            &export_depth,
            msaa_depth_view.as_ref(),
        );

        render_pass::encode_and_submit(
            &res.device,
            &res.queue,
            &target,
            &res.pipeline,
            bind_group,
            &res.vertex_buffer,
            &res.index_buffer,
            res.index_count,
        );

        Ok(render_pass::read_texture_rgba8(
            &res.device,
            &res.queue,
            &export_texture,
            target_width,
            target_height,
        ))
    })
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
    /// Raw shading parameters from the last rendered frame, used by the
    /// export path to avoid reverse-engineering quantized `FrameState` values.
    last_shading: Option<render_pass::ShadingParams>,
    /// Bind group resolution from the last rendered frame, used by the
    /// export path to reuse the same texture binding without re-resolving.
    last_resolved: Option<texture_routing::ResolvedTexture>,
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
                let sun_dir = if win.get_use_custom_datetime() {
                    let hour = win.get_custom_hour();
                    let doy = win.get_custom_day_of_year() as u16;
                    let year = win.get_custom_year_index() + datetime::base_year();
                    let (month, day) = datetime::day_of_year_to_month_day(doy.max(1), year);
                    let (h, m, s) = datetime::hour_float_to_hms(hour);
                    sun::sun_direction_at(year, i32::from(month), i32::from(day), h, m, s)
                } else {
                    sun::sun_direction_now()
                };

                // Read UI properties for blend mode
                let terminator_width_f = win.get_terminator_width();
                let diffuse_shading = win.get_diffuse_shading();
                let diffuse_floor_f = win.get_diffuse_floor();
                let diffuse_ramp_f = win.get_diffuse_ramp();
                let spec_shininess_f = win.get_spec_shininess();
                let spec_intensity_f = win.get_spec_intensity();

                // Build current frame state for dirty-checking
                let camera = CameraParams {
                    longitude: win.get_camera_longitude(),
                    latitude: win.get_camera_latitude(),
                    zoom: win.get_camera_zoom(),
                    offset_x: win.get_camera_offset_x(),
                    offset_y: win.get_camera_offset_y(),
                    tilt_deg: win.get_camera_tilt(),
                    yaw_deg: win.get_camera_yaw(),
                    pitch_deg: win.get_camera_pitch(),
                };
                win.set_zoom_display_distance(zoom_to_distance(camera.zoom));
                let current_state = build_frame_state(
                    &camera,
                    res.sample_count,
                    win.get_texture_index(),
                    res.render_width,
                    res.render_height,
                    sun_dir,
                    terminator_width_f,
                    diffuse_shading,
                    diffuse_floor_f,
                    diffuse_ramp_f,
                    spec_shininess_f,
                    spec_intensity_f,
                );

                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let raw_index = current_state.texture_index as usize;

                // Resolve textures and kick off background loads
                let (resolved, use_blend_uniforms) =
                    texture_routing::resolve_textures(res, raw_index);

                // Update loading indicator
                texture_routing::update_loading_text(&win, res, raw_index);

                // Skip rendering if nothing changed since last frame
                // (but always render if we just received a completed decode)
                if !received_any && res.last_state.as_ref() == Some(&current_state) {
                    return;
                }

                // Look up the bind group reference and store the resolution
                // for the export path
                let bind_group_ref = match &resolved {
                    texture_routing::ResolvedTexture::Composite => res
                        .composite_bind_group
                        .as_ref()
                        .expect("composite bind group must exist when Composite is returned"),
                    texture_routing::ResolvedTexture::Slot(idx) => res.texture_slots[*idx]
                        .bind_group
                        .as_ref()
                        .expect("render_index must always point to a loaded slot"),
                };
                res.last_resolved = Some(resolved);

                let shading = render_pass::ShadingParams {
                    sun_dir,
                    use_blend: use_blend_uniforms,
                    terminator_width: terminator_width_f,
                    diffuse_shading,
                    diffuse_floor: diffuse_floor_f,
                    diffuse_ramp: diffuse_ramp_f,
                    spec_shininess: spec_shininess_f,
                    spec_intensity: spec_intensity_f,
                };
                res.last_shading = Some(shading);

                let image = render_pass::execute_render_pass(
                    res,
                    &current_state,
                    bind_group_ref,
                    res.last_shading.as_ref().unwrap(),
                );
                res.last_state = Some(current_state);
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
}
