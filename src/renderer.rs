use std::path::PathBuf;
use std::sync::mpsc;

use slint::{ComponentHandle, GraphicsAPI, Image, Model, RenderingState};
use wgpu::util::DeviceExt;

use crate::MainWindow;
use crate::camera::OrbitalCamera;
use crate::grid_texture;
use crate::sphere::{self, Vertex};
use crate::sun;
use crate::texture_loader;

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

/// GPU-side uniform buffer layout, matching the WGSL `Uniforms` struct.
///
/// Total: 96 bytes (must be a multiple of 16 for std140 alignment).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    mvp: [f32; 16],           // 64 bytes
    sun_dir: [f32; 3],        // 12 bytes
    terminator_width: f32,    // 4 bytes
    flags: u32,               // 4 bytes
    diffuse_floor: f32,       // 4 bytes
    diffuse_ramp: f32,        // 4 bytes
    _pad: f32,                // 4 bytes
}

const _: () = assert!(std::mem::size_of::<Uniforms>() == 96);

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

/// Message sent from a background decode thread when texture loading completes.
struct DecodedTextureMessage {
    slot_index: usize,
    result: Result<texture_loader::DecodedImage, String>,
}

/// Descriptor for a texture that can be loaded on demand.
struct TextureSlot {
    /// The GPU bind group, populated on first use.
    bind_group: Option<wgpu::BindGroup>,
    /// Filesystem path to the texture file (`None` for procedural textures).
    source_path: Option<PathBuf>,
    /// `true` while a background thread is decoding this slot's texture.
    loading: bool,
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
#[derive(Clone, PartialEq)]
struct FrameState {
    longitude: f32,
    latitude: f32,
    zoom: f32,
    sample_count: u32,
    texture_index: i32,
    width: u32,
    height: u32,
    /// Sun direction quantized to integer milliradians for stable comparison.
    sun_direction: [i32; 3],
    /// Terminator width quantized to integer milliradians.
    terminator_width: i32,
    /// Whether diffuse shading is enabled.
    diffuse_shading: bool,
    /// Diffuse floor quantized to integer thousandths.
    diffuse_floor: i32,
    /// Diffuse ramp quantized to integer thousandths.
    diffuse_ramp: i32,
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
    let w = (w / SIZE_GRANULARITY).max(1) * SIZE_GRANULARITY;
    let h = (h / SIZE_GRANULARITY).max(1) * SIZE_GRANULARITY;
    (w, h)
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

                // Quantize sun direction to milliradians for dirty-checking
                #[allow(clippy::cast_possible_truncation)]
                let sun_direction_quantized = [
                    (sun_dir.x * 1000.0) as i32,
                    (sun_dir.y * 1000.0) as i32,
                    (sun_dir.z * 1000.0) as i32,
                ];
                #[allow(clippy::cast_possible_truncation)]
                let terminator_width_quantized = (terminator_width_f * 1000.0) as i32;
                let diffuse_floor_f = win.get_diffuse_floor();
                let diffuse_ramp_f = win.get_diffuse_ramp();
                #[allow(clippy::cast_possible_truncation)]
                let diffuse_floor_quantized = (diffuse_floor_f * 1000.0) as i32;
                #[allow(clippy::cast_possible_truncation)]
                let diffuse_ramp_quantized = (diffuse_ramp_f * 1000.0) as i32;

                // Build current frame state for dirty-checking
                let current_state = FrameState {
                    longitude: win.get_camera_longitude(),
                    latitude: win.get_camera_latitude(),
                    zoom: win.get_camera_zoom(),
                    sample_count: res.sample_count,
                    texture_index: win.get_texture_index(),
                    width: res.render_width,
                    height: res.render_height,
                    sun_direction: sun_direction_quantized,
                    terminator_width: terminator_width_quantized,
                    diffuse_shading,
                    diffuse_floor: diffuse_floor_quantized,
                    diffuse_ramp: diffuse_ramp_quantized,
                };

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

/// Drain the channel for completed background texture decodes and create
/// GPU resources (mipmapped texture + bind group) for each one.
///
/// Returns `true` if at least one decoded texture was processed, signaling
/// that a re-render is needed even if the frame state hasn't changed.
fn process_decoded_textures(res: &mut GpuResources) -> bool {
    let mut received_any = false;
    while let Ok(msg) = res.texture_rx.try_recv() {
        received_any = true;
        match msg.result {
            Ok(img) => {
                let tex = create_mipmapped_texture(
                    &res.device,
                    &res.queue,
                    &format!("texture_slot_{}", msg.slot_index),
                    img.width,
                    img.height,
                    &img.pixels,
                );
                let tex_view = tex.create_view(&wgpu::TextureViewDescriptor::default());
                let bind_group = create_bind_group(
                    &res.device,
                    &res.bind_group_layout,
                    &res.uniform_buffer,
                    &tex_view,
                    &res.sampler,
                    &res.dummy_texture_view,
                    &format!("bind_group_slot_{}", msg.slot_index),
                );
                res.texture_slots[msg.slot_index].bind_group = Some(bind_group);
                res.texture_slots[msg.slot_index].loading = false;

                // Store texture views for composite bind group creation
                if msg.slot_index == DAY_SLOT {
                    res.day_texture_view = Some(tex_view);
                } else if msg.slot_index == NIGHT_SLOT {
                    res.night_texture_view = Some(tex_view);
                }

                // If both day and night textures are now available, create
                // the composite bind group for blend mode.
                maybe_create_composite_bind_group(res);
            }
            Err(e) => {
                eprintln!("{e}");
                // Mark source_path as None so we don't retry
                res.texture_slots[msg.slot_index].source_path = None;
                res.texture_slots[msg.slot_index].loading = false;
            }
        }
    }
    received_any
}

/// Create the composite bind group if both day and night texture views are available.
fn maybe_create_composite_bind_group(res: &mut GpuResources) {
    if let (Some(day_view), Some(night_view)) =
        (&res.day_texture_view, &res.night_texture_view)
    {
        res.composite_bind_group = Some(create_bind_group(
            &res.device,
            &res.bind_group_layout,
            &res.uniform_buffer,
            day_view,
            &res.sampler,
            night_view,
            "composite_bind_group",
        ));
    }
}

/// Spawn a background thread to decode the texture for `slot_index` if
/// it is not already loaded or in flight.
fn maybe_spawn_texture_load(res: &mut GpuResources, slot_index: usize) {
    let slot = &res.texture_slots[slot_index];

    // Already loaded, already loading, or no source path — nothing to do
    if slot.bind_group.is_some() || slot.loading || slot.source_path.is_none() {
        return;
    }

    let path = res.texture_slots[slot_index]
        .source_path
        .clone()
        .expect("checked above");
    res.texture_slots[slot_index].loading = true;

    let tx = res.texture_tx.clone();
    let window_weak = res.window_weak.clone();

    std::thread::spawn(move || {
        texture_loader::register_jxl_hook();
        let start = std::time::Instant::now();
        let result = match texture_loader::load(&path) {
            Ok(img) => {
                eprintln!(
                    "Decoded texture ({}\u{d7}{}) from {} in {:.2}s",
                    img.width,
                    img.height,
                    path.display(),
                    start.elapsed().as_secs_f64(),
                );
                Ok(img)
            }
            Err(e) => Err(e),
        };

        // Send the result to the UI thread; ignore errors (receiver dropped on teardown)
        let _ = tx.send(DecodedTextureMessage { slot_index, result });

        // Wake the event loop so BeforeRendering fires and picks up the result
        let _ = window_weak.upgrade_in_event_loop(|win| {
            win.window().request_redraw();
        });
    });
}

/// Determine which texture slot to render with: the requested slot if loaded,
/// otherwise `last_rendered_index` as a fallback.
fn resolve_render_index(res: &mut GpuResources, slot_index: usize) -> usize {
    if res.texture_slots[slot_index].bind_group.is_some() {
        res.last_rendered_index = slot_index;
        slot_index
    } else {
        res.last_rendered_index
    }
}

#[allow(clippy::too_many_lines)]
fn create_gpu_resources(
    device: wgpu::Device,
    queue: wgpu::Queue,
    sample_count: u32,
    width: u32,
    height: u32,
    texture_paths: &[Option<PathBuf>],
    window_weak: slint::Weak<MainWindow>,
) -> GpuResources {
    // Generate sphere mesh
    let mesh = sphere::generate_uv_sphere(64, 64);

    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("sphere_vertices"),
        contents: bytemuck::cast_slice(&mesh.vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });

    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("sphere_indices"),
        contents: bytemuck::cast_slice(&mesh.indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    // Uniform buffer: MVP (64) + sun_dir (12) + terminator_width (4) +
    // flags (4) + padding (12) = 96 bytes
    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("uniforms"),
        size: std::mem::size_of::<Uniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // Shared sampler for all textures
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("texture_sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear, // trilinear
        anisotropy_clamp: 16,
        ..Default::default()
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bind_group_layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ],
    });

    // 1x1 black dummy texture used as the night texture placeholder
    // in single-texture bind groups (Grid, Day, Night modes).
    let dummy_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("dummy_1x1"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &dummy_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[0, 0, 0, 255],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let dummy_texture_view = dummy_texture.create_view(&wgpu::TextureViewDescriptor::default());

    // Grid texture (always loaded at slot 0)
    let grid_tex = create_mipmapped_texture(
        &device,
        &queue,
        "grid_texture",
        GRID_TEX_WIDTH,
        GRID_TEX_HEIGHT,
        &grid_texture::generate(GRID_TEX_WIDTH, GRID_TEX_HEIGHT),
    );
    let grid_tex_view = grid_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let grid_bind_group = create_bind_group(
        &device,
        &bind_group_layout,
        &uniform_buffer,
        &grid_tex_view,
        &sampler,
        &dummy_texture_view,
        "grid_bind_group",
    );

    // Build texture slots: slot 0 = Grid (always loaded), slots 1+ = lazy from paths
    let mut texture_slots = vec![TextureSlot {
        bind_group: Some(grid_bind_group),
        source_path: None,
        loading: false,
    }];
    for path in texture_paths {
        texture_slots.push(TextureSlot {
            bind_group: None,
            source_path: path.clone(),
            loading: false,
        });
    }

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sphere_pipeline_layout"),
        bind_group_layouts: &[&bind_group_layout],
        immediate_size: 0,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sphere_shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/sphere.wgsl").into()),
    });

    let (render_texture, depth_texture, msaa_texture_view, msaa_depth_view) =
        create_render_textures(&device, width, height, sample_count);

    let pipeline = create_pipeline(&device, &pipeline_layout, &shader, sample_count);

    let (texture_tx, texture_rx) = mpsc::channel();

    GpuResources {
        pipeline,
        vertex_buffer,
        index_buffer,
        #[allow(clippy::cast_possible_truncation)]
        index_count: mesh.indices.len() as u32,
        uniform_buffer,
        bind_group_layout,
        sampler,
        texture_slots,
        last_rendered_index: 0,
        depth_texture,
        render_texture,
        msaa_texture_view,
        msaa_depth_view,
        sample_count,
        render_width: width,
        render_height: height,
        last_state: None,
        shader,
        pipeline_layout,
        device,
        queue,
        texture_tx,
        texture_rx,
        window_weak,
        dummy_texture_view,
        composite_bind_group: None,
        day_texture_view: None,
        night_texture_view: None,
    }
}

fn create_pipeline(
    device: &wgpu::Device,
    pipeline_layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    sample_count: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sphere_pipeline"),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[Vertex::buffer_layout()],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview_mask: None,
        cache: None,
    })
}

/// Create all size-dependent render textures (resolve target, depth, and optional MSAA).
fn create_render_textures(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    sample_count: u32,
) -> (
    wgpu::Texture,
    wgpu::TextureView,
    Option<wgpu::TextureView>,
    Option<wgpu::TextureView>,
) {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };

    let render_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("render_texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });

    let depth_texture = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth_texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());

    let (msaa_color, msaa_depth) = if sample_count > 1 {
        let msaa_color = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("msaa_color_texture"),
                size,
                mip_level_count: 1,
                sample_count,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());

        let msaa_depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("msaa_depth_texture"),
                size,
                mip_level_count: 1,
                sample_count,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());

        (Some(msaa_color), Some(msaa_depth))
    } else {
        (None, None)
    };

    (render_texture, depth_texture, msaa_color, msaa_depth)
}

/// Rebuild the pipeline and MSAA textures when sample count changes.
fn rebuild_msaa_resources(res: &mut GpuResources, sample_count: u32) {
    replace_render_textures(res, res.render_width, res.render_height, sample_count);
    res.pipeline = create_pipeline(&res.device, &res.pipeline_layout, &res.shader, sample_count);
    res.sample_count = sample_count;
}

/// Rebuild all size-dependent textures when viewport dimensions change.
fn rebuild_render_textures(res: &mut GpuResources, width: u32, height: u32) {
    replace_render_textures(res, width, height, res.sample_count);
    res.render_width = width;
    res.render_height = height;
}

/// Replace all size/sample-dependent textures, dropping MSAA views first
/// so wgpu can reclaim memory before allocating new ones.
fn replace_render_textures(res: &mut GpuResources, width: u32, height: u32, sample_count: u32) {
    res.msaa_texture_view = None;
    res.msaa_depth_view = None;
    let (render_texture, depth_texture, msaa_texture_view, msaa_depth_view) =
        create_render_textures(&res.device, width, height, sample_count);
    res.render_texture = render_texture;
    res.depth_texture = depth_texture;
    res.msaa_texture_view = msaa_texture_view;
    res.msaa_depth_view = msaa_depth_view;
}

const GRID_TEX_WIDTH: u32 = 2048;
const GRID_TEX_HEIGHT: u32 = 1024;

/// Create a texture from RGBA8 pixel data with CPU-generated mipmaps.
fn create_mipmapped_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    rgba_pixels: &[u8],
) -> wgpu::Texture {
    let mip_count = width.max(height).ilog2() + 1;

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: mip_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    // Upload mip level 0
    upload_mip(queue, &texture, 0, width, height, rgba_pixels);

    // Generate subsequent mip levels by box-filtering the previous level
    let mut pixels = rgba_pixels.to_vec();
    let mut w = width;
    let mut h = height;
    for level in 1..mip_count {
        pixels = downsample_2x(&pixels, w, h);
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        upload_mip(queue, &texture, level, w, h, &pixels);
    }

    texture
}

/// Create a bind group with a uniform buffer, day texture, sampler, and night texture.
///
/// For single-texture modes (Grid, Day, Night), pass the dummy 1x1 texture
/// as `night_texture_view`. For blend mode, pass the actual night texture.
fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform_buffer: &wgpu::Buffer,
    texture_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    night_texture_view: &wgpu::TextureView,
    label: &str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(texture_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(night_texture_view),
            },
        ],
    })
}

fn upload_mip(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    mip_level: u32,
    width: u32,
    height: u32,
    data: &[u8],
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

/// Box-filter downsample: average each 2x2 block of RGBA pixels.
#[allow(clippy::cast_possible_truncation)]
fn downsample_2x(src: &[u8], src_w: u32, src_h: u32) -> Vec<u8> {
    let dst_w = (src_w / 2).max(1) as usize;
    let dst_h = (src_h / 2).max(1) as usize;
    let sw = src_w as usize;
    let sh = src_h as usize;
    let mut dst = vec![0u8; dst_w * dst_h * 4];

    for y in 0..dst_h {
        for x in 0..dst_w {
            let sx = x * 2;
            let sy = y * 2;
            // Clamp neighbor coordinates to stay within source bounds
            let sx1 = (sx + 1).min(sw - 1);
            let sy1 = (sy + 1).min(sh - 1);
            for c in 0..4 {
                let tl = u16::from(src[(sy * sw + sx) * 4 + c]);
                let tr = u16::from(src[(sy * sw + sx1) * 4 + c]);
                let bl = u16::from(src[(sy1 * sw + sx) * 4 + c]);
                let br = u16::from(src[(sy1 * sw + sx1) * 4 + c]);
                dst[(y * dst_w + x) * 4 + c] = ((tl + tr + bl + br + 2) / 4) as u8;
            }
        }
    }

    dst
}
