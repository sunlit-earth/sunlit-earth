use slint::{ComponentHandle, GraphicsAPI, Image, RenderingState};
use wgpu::util::DeviceExt;

use crate::MainWindow;
use crate::camera::OrbitalCamera;
use crate::grid_texture;
use crate::sphere::{self, Vertex};

const RENDER_WIDTH: u32 = 800;
const RENDER_HEIGHT: u32 = 600;

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
    bind_group: wgpu::BindGroup,
    depth_texture: wgpu::TextureView,
    render_texture: wgpu::Texture,
    msaa_texture_view: Option<wgpu::TextureView>,
    msaa_depth_view: Option<wgpu::TextureView>,
    sample_count: u32,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

/// Register the rendering notifier on the given Slint window.
pub fn setup_rendering_notifier(window: &MainWindow, aa_counts: Vec<u32>) {
    let window_weak = window.as_weak();

    window
        .window()
        .set_rendering_notifier(move |state, graphics_api| {
            rendering_callback(state, graphics_api, &window_weak, &aa_counts);
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
) {
    let lookup_sample_count = |win: &MainWindow| {
        let idx = usize::try_from(win.get_aa_index()).unwrap_or(0);
        aa_counts.get(idx).copied().unwrap_or(1)
    };

    match state {
        RenderingState::RenderingSetup => {
            let GraphicsAPI::WGPU28 { device, queue, .. } = graphics_api else {
                eprintln!("Expected WGPU28 graphics API, got something else");
                return;
            };

            let sample_count = window_weak
                .upgrade()
                .map_or(4, |win| lookup_sample_count(&win));
            let resources = create_gpu_resources(device.clone(), queue.clone(), sample_count);
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

                let camera = OrbitalCamera::new(
                    win.get_camera_longitude(),
                    win.get_camera_latitude(),
                    win.get_camera_zoom(),
                );

                #[allow(clippy::cast_precision_loss)]
                let aspect = RENDER_WIDTH as f32 / RENDER_HEIGHT as f32;
                let mvp = camera.mvp_matrix(aspect);

                // Write MVP matrix to uniform buffer
                res.queue
                    .write_buffer(&res.uniform_buffer, 0, bytemuck::cast_slice(mvp.as_ref()));

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
                    pass.set_bind_group(0, &res.bind_group, &[]);
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

    // Uniform buffer for the MVP matrix (4x4 f32 = 64 bytes)
    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mvp_uniform"),
        size: 64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // Generate and upload grid texture with mipmaps
    let grid_tex = create_grid_texture(&device, &queue);
    let grid_tex_view = grid_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let grid_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("grid_sampler"),
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

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bind_group"),
        layout: &bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&grid_tex_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&grid_sampler),
            },
        ],
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

    // The resolve target is always sample_count=1
    let render_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("render_texture"),
        size: wgpu::Extent3d {
            width: RENDER_WIDTH,
            height: RENDER_HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });

    let (msaa_texture_view, msaa_depth_view) = create_msaa_textures(&device, sample_count);

    let depth_texture = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth_texture"),
            size: wgpu::Extent3d {
                width: RENDER_WIDTH,
                height: RENDER_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());

    let pipeline = create_pipeline(&device, &pipeline_layout, &shader, sample_count);

    GpuResources {
        pipeline,
        vertex_buffer,
        index_buffer,
        #[allow(clippy::cast_possible_truncation)]
        index_count: mesh.indices.len() as u32,
        uniform_buffer,
        bind_group,
        depth_texture,
        render_texture,
        msaa_texture_view,
        msaa_depth_view,
        sample_count,
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

/// Create MSAA color and depth textures. Returns `(None, None)` when `sample_count == 1`.
fn create_msaa_textures(
    device: &wgpu::Device,
    sample_count: u32,
) -> (Option<wgpu::TextureView>, Option<wgpu::TextureView>) {
    if sample_count <= 1 {
        return (None, None);
    }

    let msaa_color = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("msaa_color_texture"),
            size: wgpu::Extent3d {
                width: RENDER_WIDTH,
                height: RENDER_HEIGHT,
                depth_or_array_layers: 1,
            },
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
            size: wgpu::Extent3d {
                width: RENDER_WIDTH,
                height: RENDER_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());

    (Some(msaa_color), Some(msaa_depth))
}

/// Rebuild the pipeline and MSAA textures when sample count changes.
fn rebuild_msaa_resources(res: &mut GpuResources, sample_count: u32) {
    let (msaa_texture_view, msaa_depth_view) = create_msaa_textures(&res.device, sample_count);
    res.msaa_texture_view = msaa_texture_view;
    res.msaa_depth_view = msaa_depth_view;
    res.pipeline = create_pipeline(&res.device, &res.pipeline_layout, &res.shader, sample_count);
    res.sample_count = sample_count;
}

const GRID_TEX_WIDTH: u32 = 2048;
const GRID_TEX_HEIGHT: u32 = 1024;

/// Create the grid texture with CPU-generated mipmaps.
fn create_grid_texture(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::Texture {
    let mip_count = GRID_TEX_WIDTH.max(GRID_TEX_HEIGHT).ilog2() + 1;

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("grid_texture"),
        size: wgpu::Extent3d {
            width: GRID_TEX_WIDTH,
            height: GRID_TEX_HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: mip_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    // Generate and upload mip level 0
    let mut pixels = grid_texture::generate(GRID_TEX_WIDTH, GRID_TEX_HEIGHT);
    upload_mip(queue, &texture, 0, GRID_TEX_WIDTH, GRID_TEX_HEIGHT, &pixels);

    // Generate subsequent mip levels by box-filtering the previous level
    let mut w = GRID_TEX_WIDTH;
    let mut h = GRID_TEX_HEIGHT;
    for level in 1..mip_count {
        pixels = downsample_2x(&pixels, w, h);
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        upload_mip(queue, &texture, level, w, h, &pixels);
    }

    texture
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
