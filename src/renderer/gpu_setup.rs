use std::path::PathBuf;
use std::sync::mpsc;

use wgpu::util::DeviceExt;

use crate::geometry::grid_texture;
use crate::geometry::sphere::{self, Vertex};
use crate::MainWindow;

use super::textures::{TextureSlot, create_bind_group, create_mipmapped_texture};
use super::uniforms::Uniforms;
use super::GpuResources;

const GRID_TEX_WIDTH: u32 = 2048;
const GRID_TEX_HEIGHT: u32 = 1024;

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
pub(super) fn create_gpu_resources(
    device: wgpu::Device,
    queue: wgpu::Queue,
    sample_count: u32,
    width: u32,
    height: u32,
    texture_paths: &[Option<PathBuf>],
    texture_tx: mpsc::Sender<super::textures::DecodedTextureMessage>,
    texture_rx: mpsc::Receiver<super::textures::DecodedTextureMessage>,
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
    // Slot 3 = cloud overlay (populated by the cloud fetcher thread, not file-based)
    texture_slots.push(TextureSlot {
        bind_group: None,
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
        create_render_textures(
            &device,
            width,
            height,
            sample_count,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );

    let pipeline = create_pipeline(&device, &pipeline_layout, &shader, sample_count);
    let cloud_pipeline = create_cloud_pipeline(&device, &pipeline_layout, &shader, sample_count);

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
        last_shading: None,
        last_resolved: None,
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
        cloud_pipeline,
        cloud_bind_group: None,
        cloud_texture_view: None,
    }
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

/// Create all size-dependent render textures (resolve target, depth, and optional MSAA).
///
/// `color_usage` controls the usage flags on the resolve (1x sample) color texture.
/// Preview passes use `RENDER_ATTACHMENT | TEXTURE_BINDING`; export passes use
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
        format: wgpu::TextureFormat::Rgba8Unorm,
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
pub(super) fn rebuild_msaa_resources(res: &mut GpuResources, sample_count: u32) {
    replace_render_textures(res, res.render_width, res.render_height, sample_count);
    res.pipeline = create_pipeline(&res.device, &res.pipeline_layout, &res.shader, sample_count);
    res.cloud_pipeline =
        create_cloud_pipeline(&res.device, &res.pipeline_layout, &res.shader, sample_count);
    res.sample_count = sample_count;
}

/// Rebuild all size-dependent textures when viewport dimensions change.
pub(super) fn rebuild_render_textures(res: &mut GpuResources, width: u32, height: u32) {
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
        create_render_textures(
            &res.device,
            width,
            height,
            sample_count,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
    res.render_texture = render_texture;
    res.depth_texture = depth_texture;
    res.msaa_texture_view = msaa_texture_view;
    res.msaa_depth_view = msaa_depth_view;
}
