use slint::{ComponentHandle, GraphicsAPI, Image, RenderingState};
use wgpu::util::DeviceExt;

use crate::MainWindow;
use crate::camera::OrbitalCamera;
use crate::earth_texture::DecodedImage;
use crate::grid_texture;
use crate::sphere::{self, Vertex};

const DEFAULT_WIDTH: u32 = 800;
const DEFAULT_HEIGHT: u32 = 600;
/// Render dimensions are rounded to this granularity to avoid
/// creating new GPU textures on every pixel change during resize.
const SIZE_GRANULARITY: u32 = 64;

/// Build the `ComboBox` labels and find the default index (preferring 4x MSAA).
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

/// GPU resources created during `RenderingSetup`.
struct GpuResources {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform_buffer: wgpu::Buffer,
    grid_bind_group: wgpu::BindGroup,
    earth_bind_group: Option<wgpu::BindGroup>,
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
}

/// Snapshot of inputs that affect the rendered image.
#[derive(Clone, PartialEq)]
struct FrameState {
    longitude: f32,
    latitude: f32,
    zoom: f32,
    sample_count: u32,
    rotation: f32,
    texture_index: i32,
    width: u32,
    height: u32,
}

/// Register the rendering notifier on the given Slint window.
pub fn setup_rendering_notifier(
    window: &MainWindow,
    aa_counts: Vec<u32>,
    earth_pixels: Option<DecodedImage>,
) {
    let window_weak = window.as_weak();
    let earth_pixels = std::cell::RefCell::new(earth_pixels);

    window
        .window()
        .set_rendering_notifier(move |state, graphics_api| {
            rendering_callback(
                state,
                graphics_api,
                &window_weak,
                &aa_counts,
                &earth_pixels,
            );
        })
        .expect("Failed to set rendering notifier — is the wgpu backend active?");
}

// Persistent state across rendering callbacks, stored in a thread-local.
// The callback is `FnMut` and Slint calls it from the UI thread.
thread_local! {
    static GPU_RESOURCES: std::cell::RefCell<Option<GpuResources>> = const { std::cell::RefCell::new(None) };
}

#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
fn rendering_callback(
    state: RenderingState,
    graphics_api: &GraphicsAPI,
    window_weak: &slint::Weak<MainWindow>,
    aa_counts: &[u32],
    earth_pixels: &std::cell::RefCell<Option<DecodedImage>>,
) {
    let lookup_sample_count = |win: &MainWindow| {
        let idx = usize::try_from(win.get_aa_index()).unwrap_or(0);
        aa_counts.get(idx).copied().unwrap_or(1)
    };

    let get_viewport_size = |win: &MainWindow| -> (u32, u32) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let w = (win.get_viewport_width() * win.window().scale_factor()) as u32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let h = (win.get_viewport_height() * win.window().scale_factor()) as u32;
        // Quantize to SIZE_GRANULARITY to reduce texture churn during resize
        let w = (w / SIZE_GRANULARITY).max(1) * SIZE_GRANULARITY;
        let h = (h / SIZE_GRANULARITY).max(1) * SIZE_GRANULARITY;
        (w, h)
    };

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
                        let (w, h) = get_viewport_size(&win);
                        (lookup_sample_count(&win), w, h)
                    });
            // Take earth pixels out of the RefCell — they're consumed during texture upload
            let earth = earth_pixels.borrow_mut().take();
            let resources = create_gpu_resources(
                device.clone(),
                queue.clone(),
                sample_count,
                width,
                height,
                earth.as_ref(),
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

                // Check if sample count changed
                let desired = lookup_sample_count(&win);
                if desired != res.sample_count {
                    rebuild_msaa_resources(res, desired);
                }

                // Check if viewport size changed
                let (vw, vh) = get_viewport_size(&win);
                if vw != res.render_width || vh != res.render_height {
                    rebuild_render_textures(res, vw, vh);
                }

                // Skip rendering if nothing changed since last frame
                let current_state = FrameState {
                    longitude: win.get_camera_longitude(),
                    latitude: win.get_camera_latitude(),
                    zoom: win.get_camera_zoom(),
                    rotation: win.get_earth_rotation(),
                    sample_count: res.sample_count,
                    texture_index: win.get_texture_index(),
                    width: res.render_width,
                    height: res.render_height,
                };
                if res.last_state.as_ref() == Some(&current_state) {
                    return;
                }
                res.last_state = Some(current_state);

                let camera = OrbitalCamera::new(
                    win.get_camera_longitude(),
                    win.get_camera_latitude(),
                    win.get_camera_zoom(),
                );

                #[allow(clippy::cast_precision_loss)]
                let aspect = res.render_width as f32 / res.render_height as f32;
                let mvp = camera.mvp_matrix(aspect);

                // Model matrix: rotate the earth around Y axis
                let rotation_rad = win.get_earth_rotation().to_radians();
                let model = glam::Mat4::from_rotation_y(rotation_rad);

                // Write MVP + model matrices to uniform buffer
                let mut uniform_data = [0u8; 128];
                uniform_data[..64].copy_from_slice(bytemuck::cast_slice(mvp.as_ref()));
                uniform_data[64..].copy_from_slice(bytemuck::cast_slice(model.as_ref()));
                res.queue
                    .write_buffer(&res.uniform_buffer, 0, &uniform_data);

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
                    let bind_group = if win.get_texture_index() == 1 {
                        res.earth_bind_group.as_ref().unwrap_or(&res.grid_bind_group)
                    } else {
                        &res.grid_bind_group
                    };
                    pass.set_bind_group(0, bind_group, &[]);
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

#[allow(clippy::too_many_lines)]
fn create_gpu_resources(
    device: wgpu::Device,
    queue: wgpu::Queue,
    sample_count: u32,
    width: u32,
    height: u32,
    earth_pixels: Option<&DecodedImage>,
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

    // Uniform buffer for MVP + model matrices (2 × mat4x4 = 128 bytes)
    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("uniforms"),
        size: 128,
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
                visibility: wgpu::ShaderStages::VERTEX,
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
        ],
    });

    // Grid texture
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
        "grid_bind_group",
    );

    // Earth texture (if available)
    let earth_bind_group = earth_pixels.map(|img| {
        let earth_tex = create_mipmapped_texture(
            &device,
            &queue,
            "earth_texture",
            img.width,
            img.height,
            &img.pixels,
        );
        let earth_tex_view = earth_tex.create_view(&wgpu::TextureViewDescriptor::default());
        create_bind_group(
            &device,
            &bind_group_layout,
            &uniform_buffer,
            &earth_tex_view,
            &sampler,
            "earth_bind_group",
        )
    });

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

    GpuResources {
        pipeline,
        vertex_buffer,
        index_buffer,
        #[allow(clippy::cast_possible_truncation)]
        index_count: mesh.indices.len() as u32,
        uniform_buffer,
        grid_bind_group,
        earth_bind_group,
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
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
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

/// Drop old size-dependent textures so wgpu can reclaim the memory
/// before we allocate new ones.
fn drop_render_textures(res: &mut GpuResources) {
    res.msaa_texture_view = None;
    res.msaa_depth_view = None;
    // render_texture and depth_texture are replaced by assignment below,
    // but the Slint Image may still hold an Arc to the old render_texture.
    // We can't force that drop, but clearing MSAA textures frees the bulk.
}

/// Rebuild the pipeline and MSAA textures when sample count changes.
fn rebuild_msaa_resources(res: &mut GpuResources, sample_count: u32) {
    drop_render_textures(res);
    let (render_texture, depth_texture, msaa_texture_view, msaa_depth_view) =
        create_render_textures(
            &res.device,
            res.render_width,
            res.render_height,
            sample_count,
        );
    res.render_texture = render_texture;
    res.depth_texture = depth_texture;
    res.msaa_texture_view = msaa_texture_view;
    res.msaa_depth_view = msaa_depth_view;
    res.pipeline = create_pipeline(&res.device, &res.pipeline_layout, &res.shader, sample_count);
    res.sample_count = sample_count;
}

/// Rebuild all size-dependent textures when viewport dimensions change.
fn rebuild_render_textures(res: &mut GpuResources, width: u32, height: u32) {
    drop_render_textures(res);
    let (render_texture, depth_texture, msaa_texture_view, msaa_depth_view) =
        create_render_textures(&res.device, width, height, res.sample_count);
    res.render_texture = render_texture;
    res.depth_texture = depth_texture;
    res.msaa_texture_view = msaa_texture_view;
    res.msaa_depth_view = msaa_depth_view;
    res.render_width = width;
    res.render_height = height;
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
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
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

/// Create a bind group with a uniform buffer, texture view, and sampler.
fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform_buffer: &wgpu::Buffer,
    texture_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
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
