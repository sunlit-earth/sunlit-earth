use tracing::debug;
use wgpu::util::DeviceExt;

use crate::assets::stars;
use crate::geometry::grid_texture;
use crate::geometry::sphere::{self, Vertex};

use super::textures::{TextureSlot, create_bind_group, create_mipmapped_texture};
use super::uniforms::Uniforms;
use super::{Renderer, RendererConfig, SLOT_LABELS};

/// Usage flags for the offscreen preview target. `COPY_SRC` is what the engine
/// reads the frame back through; no client binds the texture itself, because
/// preview frames cross to the UI as pixel buffers.
const PREVIEW_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT
    .union(wgpu::TextureUsages::TEXTURE_BINDING)
    .union(wgpu::TextureUsages::COPY_SRC);

/// Format of every color target, resolve and MSAA alike.
pub(super) const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Format of every depth target.
///
/// Named alongside the color one so the memory report can price the depth and
/// MSAA targets, which are kept as views rather than textures and so cannot be
/// asked what they are.
pub(super) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

const GRID_TEX_WIDTH: u32 = 2048;
const GRID_TEX_HEIGHT: u32 = 1024;

#[allow(clippy::too_many_lines)]
#[tracing::instrument(skip_all, fields(width, height, sample_count))]
pub(super) fn create_renderer(
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: RendererConfig,
) -> Renderer {
    let RendererConfig {
        sample_count,
        width,
        height,
        texture_paths,
        texture_resolution,
        texture_cache_dir,
        mailbox: texture_mailbox,
        notify,
    } = config;
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

    let star_catalog = stars::embedded_catalog();
    let star_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("star_instances"),
        contents: star_catalog.instance_bytes(),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let planet_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("planet_instances"),
        contents: &[0; 5 * stars::RECORD_SIZE],
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
    });

    // One shared uniform buffer feeds the globe, atmosphere, and sky pipelines.
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
        SLOT_LABELS[0],
        GRID_TEX_WIDTH,
        GRID_TEX_HEIGHT,
        grid_texture::generate(GRID_TEX_WIDTH, GRID_TEX_HEIGHT),
    );
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
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
        texture: Some(grid_tex),
        source_path: None,
        loading: false,
    }];
    for path in &texture_paths {
        texture_slots.push(TextureSlot {
            bind_group: None,
            texture: None,
            source_path: path.clone(),
            loading: false,
        });
    }
    // The cloud overlay, always last because it comes from the fetcher rather
    // than from a file. `SlotLayout::clouds` names its index.
    texture_slots.push(TextureSlot {
        bind_group: None,
        texture: None,
        source_path: None,
        loading: false,
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sphere_pipeline_layout"),
        bind_group_layouts: &[&bind_group_layout],
        immediate_size: 0,
    });

    let wgsl_source = format!(
        "{}\n{}",
        include_str!("../../shaders/blend.wgsl"),
        include_str!("../../shaders/sphere.wgsl"),
    );
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sphere_shader"),
        source: wgpu::ShaderSource::Wgsl(wgsl_source.into()),
    });

    let (render_texture, depth_texture, msaa_texture_view, msaa_depth_view) =
        create_render_textures(&device, width, height, sample_count, PREVIEW_USAGE);

    let pipeline = create_pipeline(&device, &pipeline_layout, &shader, sample_count);
    let star_pipeline = create_star_pipeline(&device, &pipeline_layout, &shader, sample_count);
    let milky_way_pipeline = create_sky_quad_pipeline(
        &device,
        &pipeline_layout,
        &shader,
        sample_count,
        "milky_way_pipeline",
        "vs_milky_way",
        "fs_milky_way",
    );
    let sun_disk_pipeline = create_sky_quad_pipeline(
        &device,
        &pipeline_layout,
        &shader,
        sample_count,
        "sun_disk_pipeline",
        "vs_sun_disk",
        "fs_sun_disk",
    );
    let sun_glare_pipeline = create_sky_quad_pipeline(
        &device,
        &pipeline_layout,
        &shader,
        sample_count,
        "sun_glare_pipeline",
        "vs_sun_glare",
        "fs_sun_glare",
    );
    let moon_pipeline = create_moon_pipeline(&device, &pipeline_layout, &shader, sample_count);
    let cloud_pipeline = create_cloud_pipeline(&device, &pipeline_layout, &shader, sample_count);
    let rayleigh_pipeline =
        create_rayleigh_pipeline(&device, &pipeline_layout, &shader, sample_count);
    let nightglow_orange_pipeline =
        create_nightglow_orange_pipeline(&device, &pipeline_layout, &shader, sample_count);
    let nightglow_green_pipeline =
        create_nightglow_green_pipeline(&device, &pipeline_layout, &shader, sample_count);

    crate::memory::log_memory_usage("after GPU resource creation");

    Renderer {
        pipeline,
        star_pipeline,
        milky_way_pipeline,
        sun_disk_pipeline,
        sun_glare_pipeline,
        moon_pipeline,
        star_buffer,
        planet_buffer,
        vertex_buffer,
        index_buffer,
        #[allow(clippy::cast_possible_truncation)]
        index_count: mesh.indices.len() as u32,
        uniform_buffer,
        bind_group_layout,
        sampler,
        texture_slots,
        texture_resolution,
        texture_generation: 0,
        texture_cache_dir,
        last_rendered_index: 0,
        depth_texture,
        render_texture,
        msaa_texture_view,
        msaa_depth_view,
        sample_count,
        render_width: width,
        render_height: height,
        last_state: None,
        last_params: None,
        last_inputs: None,
        last_resolved: None,
        shader,
        pipeline_layout,
        device,
        queue,
        texture_mailbox,
        texture_dirty: false,
        notify,
        dummy_texture_view,
        composite_bind_group: None,
        day_texture_view: None,
        night_texture_view: None,
        rayleigh_pipeline,
        nightglow_orange_pipeline,
        nightglow_green_pipeline,
        cloud_pipeline,
        cloud_bind_group: None,
        cloud_texture_view: None,
    }
}

/// A screen-aligned quad the vertex shader generates from `vertex_index`
/// alone, additive, with the depth test out of the way.
///
/// Three draws use it. The Milky Way is the pass's first, where everything
/// after it overdraws it; the Sun's disk is scheduled with the sky, where the
/// opaque globe drawn afterwards covers whatever falls inside its painted disc;
/// the glare is scheduled last, where nothing covers it, which is what veiling
/// glare does. None of them reads depth, so they differ only in when they run
/// and which entry points they carry.
#[allow(clippy::too_many_arguments)]
pub(super) fn create_sky_quad_pipeline(
    device: &wgpu::Device,
    pipeline_layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    sample_count: u32,
    label: &str,
    vs_entry: &str,
    fs_entry: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vs_entry),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fs_entry),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent::OVER,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            strip_index_format: None,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Always,
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

pub(super) fn create_star_pipeline(
    device: &wgpu::Device,
    pipeline_layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    sample_count: u32,
) -> wgpu::RenderPipeline {
    const ATTRIBUTES: [wgpu::VertexAttribute; 2] = [
        wgpu::VertexAttribute {
            offset: 0,
            shader_location: 2,
            format: wgpu::VertexFormat::Float32x3,
        },
        wgpu::VertexAttribute {
            offset: 12,
            shader_location: 3,
            format: wgpu::VertexFormat::Unorm8x4,
        },
    ];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("star_pipeline"),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_star"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: stars::RECORD_SIZE as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &ATTRIBUTES,
            }],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_star"),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent::OVER,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            strip_index_format: None,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Always,
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

pub(super) fn create_pipeline(
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

/// The Moon: opaque, back-face culled, and out of the depth test's way.
///
/// Opaque because it has to cover the Sun's additive disk, which would show
/// through anything else; back-face culled because that is what a convex
/// sphere's own front-to-back needs and a depth comparison between the sky lens
/// and the Earth's would compare two different projections; and no depth write,
/// so the Earth still draws over it wherever the painted globe covers it.
pub(super) fn create_moon_pipeline(
    device: &wgpu::Device,
    pipeline_layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    sample_count: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("moon_pipeline"),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_moon"),
            buffers: &[Vertex::buffer_layout()],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_moon"),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
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
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Always,
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

pub(super) fn create_cloud_pipeline(
    device: &wgpu::Device,
    pipeline_layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    sample_count: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("cloud_pipeline"),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_cloud"),
            buffers: &[Vertex::buffer_layout()],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_cloud"),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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
            depth_write_enabled: false,
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

/// Shared additive-blend atmosphere pipeline descriptor, differing only in
/// vertex/fragment entry points and label.
fn create_atmo_shell_pipeline(
    device: &wgpu::Device,
    pipeline_layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    sample_count: u32,
    label: &str,
    vs_entry: &str,
    fs_entry: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vs_entry),
            buffers: &[Vertex::buffer_layout()],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fs_entry),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent::OVER,
                }),
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
            depth_write_enabled: false,
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

pub(super) fn create_rayleigh_pipeline(
    device: &wgpu::Device,
    pipeline_layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    sample_count: u32,
) -> wgpu::RenderPipeline {
    // Rayleigh uses premultiplied alpha blending (One + OneMinusSrcAlpha).
    // The shader outputs RGB = in-scattered light, A = extinction (haze).
    // result = scatter + dst * (1 - extinction). At haze=0 this equals
    // additive (no extinction). At haze>0 the atmosphere partially blocks
    // what's behind it, tinting bright surfaces like clouds blue.
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("rayleigh_pipeline"),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_rayleigh"),
            buffers: &[Vertex::buffer_layout()],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_rayleigh"),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent::OVER,
                }),
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
            depth_write_enabled: false,
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

pub(super) fn create_nightglow_orange_pipeline(
    device: &wgpu::Device,
    pipeline_layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    sample_count: u32,
) -> wgpu::RenderPipeline {
    create_atmo_shell_pipeline(
        device,
        pipeline_layout,
        shader,
        sample_count,
        "nightglow_orange_pipeline",
        "vs_nightglow_orange",
        "fs_nightglow_orange",
    )
}

pub(super) fn create_nightglow_green_pipeline(
    device: &wgpu::Device,
    pipeline_layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    sample_count: u32,
) -> wgpu::RenderPipeline {
    create_atmo_shell_pipeline(
        device,
        pipeline_layout,
        shader,
        sample_count,
        "nightglow_green_pipeline",
        "vs_nightglow_green",
        "fs_nightglow_green",
    )
}

/// Create all size-dependent render textures (resolve target, depth, and optional MSAA).
///
/// `color_usage` controls the usage flags on the resolve (1x sample) color
/// texture. Preview passes use `PREVIEW_USAGE`; export passes use
/// `RENDER_ATTACHMENT | COPY_SRC` for GPU-to-CPU readback.
pub(super) fn create_render_textures(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    sample_count: u32,
    color_usage: wgpu::TextureUsages,
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
        format: COLOR_FORMAT,
        usage: color_usage,
        view_formats: &[],
    });

    let depth_texture = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth_texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
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
                format: COLOR_FORMAT,
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
                format: DEPTH_FORMAT,
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
pub(super) fn rebuild_msaa_resources(res: &mut Renderer, sample_count: u32) {
    debug!("rebuilding MSAA resources");
    replace_render_textures(res, res.render_width, res.render_height, sample_count);
    res.pipeline = create_pipeline(&res.device, &res.pipeline_layout, &res.shader, sample_count);
    res.star_pipeline =
        create_star_pipeline(&res.device, &res.pipeline_layout, &res.shader, sample_count);
    res.milky_way_pipeline = create_sky_quad_pipeline(
        &res.device,
        &res.pipeline_layout,
        &res.shader,
        sample_count,
        "milky_way_pipeline",
        "vs_milky_way",
        "fs_milky_way",
    );
    res.sun_disk_pipeline = create_sky_quad_pipeline(
        &res.device,
        &res.pipeline_layout,
        &res.shader,
        sample_count,
        "sun_disk_pipeline",
        "vs_sun_disk",
        "fs_sun_disk",
    );
    res.sun_glare_pipeline = create_sky_quad_pipeline(
        &res.device,
        &res.pipeline_layout,
        &res.shader,
        sample_count,
        "sun_glare_pipeline",
        "vs_sun_glare",
        "fs_sun_glare",
    );
    res.moon_pipeline =
        create_moon_pipeline(&res.device, &res.pipeline_layout, &res.shader, sample_count);
    res.cloud_pipeline =
        create_cloud_pipeline(&res.device, &res.pipeline_layout, &res.shader, sample_count);
    res.rayleigh_pipeline =
        create_rayleigh_pipeline(&res.device, &res.pipeline_layout, &res.shader, sample_count);
    res.nightglow_orange_pipeline = create_nightglow_orange_pipeline(
        &res.device,
        &res.pipeline_layout,
        &res.shader,
        sample_count,
    );
    res.nightglow_green_pipeline = create_nightglow_green_pipeline(
        &res.device,
        &res.pipeline_layout,
        &res.shader,
        sample_count,
    );
    res.sample_count = sample_count;
}

/// Rebuild all size-dependent textures when viewport dimensions change.
pub(super) fn rebuild_render_textures(res: &mut Renderer, width: u32, height: u32) {
    debug!(width, height, "rebuilding render textures");
    replace_render_textures(res, width, height, res.sample_count);
    res.render_width = width;
    res.render_height = height;
}

/// Replace all size/sample-dependent textures, dropping MSAA views first
/// so wgpu can reclaim memory before allocating new ones.
fn replace_render_textures(res: &mut Renderer, width: u32, height: u32, sample_count: u32) {
    res.msaa_texture_view = None;
    res.msaa_depth_view = None;
    let (render_texture, depth_texture, msaa_texture_view, msaa_depth_view) =
        create_render_textures(&res.device, width, height, sample_count, PREVIEW_USAGE);
    res.render_texture = render_texture;
    res.depth_texture = depth_texture;
    res.msaa_texture_view = msaa_texture_view;
    res.msaa_depth_view = msaa_depth_view;
}
