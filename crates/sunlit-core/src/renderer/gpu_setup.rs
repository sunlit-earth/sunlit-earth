use tracing::debug;
use wgpu::util::DeviceExt;

use crate::assets::stars;
use crate::geometry::grid_texture;
use crate::geometry::sphere::{self, Vertex};

use super::slots::SLOT_LABELS;
use super::textures::{TextureSlot, create_bind_group, create_mipmapped_texture};
use super::uniforms::Uniforms;
use super::{Renderer, RendererConfig};

/// Usage flags for the offscreen preview target. `COPY_SRC` is what the engine
/// reads the frame back through; no client binds the texture itself, because
/// preview frames cross to the UI as pixel buffers. `TEXTURE_BINDING` therefore
/// has no reader today and is still here as an open question rather than a
/// decision: no test in the suite would show that dropping it is safe.
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

#[expect(
    clippy::too_many_lines,
    reason = "one linear setup sequence; the pipelines it builds are a table in Pipelines::build"
)]
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

    // 1x1 black placeholder at binding 3 for every bind group that reads one
    // texture.
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

    let pipelines = Pipelines::build(&device, &pipeline_layout, &shader, sample_count);

    crate::memory::log_memory_usage("after GPU resource creation");

    Renderer {
        pipelines,
        star_buffer,
        planet_buffer,
        vertex_buffer,
        index_buffer,
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a sphere mesh has thousands of indices"
        )]
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
        cloud_bind_group: None,
        cloud_texture_view: None,
    }
}

/// Every render pipeline the pass can bind.
///
/// They are built together because they share a sample count: an MSAA change
/// invalidates all ten at once, and a pipeline left behind at the old count is
/// a wgpu validation error on the engine thread rather than a wrong picture.
pub(super) struct Pipelines {
    pub sphere: wgpu::RenderPipeline,
    pub star: wgpu::RenderPipeline,
    pub milky_way: wgpu::RenderPipeline,
    pub sun_disk: wgpu::RenderPipeline,
    pub sun_glare: wgpu::RenderPipeline,
    pub moon: wgpu::RenderPipeline,
    pub cloud: wgpu::RenderPipeline,
    pub rayleigh: wgpu::RenderPipeline,
    pub nightglow_orange: wgpu::RenderPipeline,
    pub nightglow_green: wgpu::RenderPipeline,
}

impl Pipelines {
    pub(super) fn build(
        device: &wgpu::Device,
        pipeline_layout: &wgpu::PipelineLayout,
        shader: &wgpu::ShaderModule,
        sample_count: u32,
    ) -> Self {
        let [
            sphere,
            star,
            milky_way,
            sun_disk,
            sun_glare,
            moon,
            cloud,
            rayleigh,
            nightglow_orange,
            nightglow_green,
        ] = SPECS.map(|spec| create_pipeline(device, pipeline_layout, shader, sample_count, &spec));
        Self {
            sphere,
            star,
            milky_way,
            sun_disk,
            sun_glare,
            moon,
            cloud,
            rayleigh,
            nightglow_orange,
            nightglow_green,
        }
    }
}

/// One pipeline's label and entry points, plus the geometry and blending it
/// differs from its neighbours by.
struct Spec {
    label: &'static str,
    vs_entry: &'static str,
    fs_entry: &'static str,
    kind: Kind,
}

/// What a pipeline draws and how its fragments reach the target.
#[derive(Clone, Copy)]
enum Kind {
    /// A screen-aligned quad the vertex shader generates from `vertex_index`
    /// alone, additive, with the depth test out of the way.
    ///
    /// Three draws use it. The Milky Way is the pass's first, where everything
    /// after it overdraws it; the Sun's disk is scheduled with the sky, where
    /// the opaque globe drawn afterwards covers whatever falls inside its
    /// painted disc; the glare is scheduled last, where nothing covers it,
    /// which is what veiling glare does. None of them reads depth, so they
    /// differ only in when they run and which entry points they carry.
    SkyQuad,
    /// One additive quad per catalog record, from the instance buffer, with the
    /// depth test out of the way.
    StarSprite,
    /// The sphere mesh, back-face culled.
    Mesh {
        blend: Option<wgpu::BlendState>,
        depth_write: bool,
        depth_compare: wgpu::CompareFunction,
    },
}

/// Source and destination both at full weight, which is what every emissive
/// draw in the pass adds itself to the frame with.
const ADDITIVE: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent::OVER,
};

/// Premultiplied alpha for the Rayleigh shell. The shader outputs RGB =
/// in-scattered light, A = extinction (haze), so the result is
/// `scatter + dst * (1 - extinction)`. At haze=0 this equals additive (no
/// extinction). At haze>0 the atmosphere partially blocks what's behind it,
/// tinting bright surfaces like clouds blue.
const PREMULTIPLIED: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent::OVER,
};

/// The ten pipelines, in the order [`Pipelines::build`] destructures them.
const SPECS: [Spec; 10] = [
    Spec {
        label: "sphere_pipeline",
        vs_entry: "vs_main",
        fs_entry: "fs_main",
        kind: Kind::Mesh {
            blend: None,
            depth_write: true,
            depth_compare: wgpu::CompareFunction::Less,
        },
    },
    Spec {
        label: "star_pipeline",
        vs_entry: "vs_star",
        fs_entry: "fs_star",
        kind: Kind::StarSprite,
    },
    Spec {
        label: "milky_way_pipeline",
        vs_entry: "vs_milky_way",
        fs_entry: "fs_milky_way",
        kind: Kind::SkyQuad,
    },
    Spec {
        label: "sun_disk_pipeline",
        vs_entry: "vs_sun_disk",
        fs_entry: "fs_sun_disk",
        kind: Kind::SkyQuad,
    },
    Spec {
        label: "sun_glare_pipeline",
        vs_entry: "vs_sun_glare",
        fs_entry: "fs_sun_glare",
        kind: Kind::SkyQuad,
    },
    // The Moon: opaque so it covers the Sun's additive disk, back-face culled,
    // and out of the depth test's way. `docs/rendering.md` says why each of the
    // three.
    Spec {
        label: "moon_pipeline",
        vs_entry: "vs_moon",
        fs_entry: "fs_moon",
        kind: Kind::Mesh {
            blend: None,
            depth_write: false,
            depth_compare: wgpu::CompareFunction::Always,
        },
    },
    Spec {
        label: "cloud_pipeline",
        vs_entry: "vs_cloud",
        fs_entry: "fs_cloud",
        kind: Kind::Mesh {
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            depth_write: false,
            depth_compare: wgpu::CompareFunction::Less,
        },
    },
    Spec {
        label: "rayleigh_pipeline",
        vs_entry: "vs_rayleigh",
        fs_entry: "fs_rayleigh",
        kind: Kind::Mesh {
            blend: Some(PREMULTIPLIED),
            depth_write: false,
            depth_compare: wgpu::CompareFunction::Less,
        },
    },
    Spec {
        label: "nightglow_orange_pipeline",
        vs_entry: "vs_nightglow_orange",
        fs_entry: "fs_nightglow_orange",
        kind: Kind::Mesh {
            blend: Some(ADDITIVE),
            depth_write: false,
            depth_compare: wgpu::CompareFunction::Less,
        },
    },
    Spec {
        label: "nightglow_green_pipeline",
        vs_entry: "vs_nightglow_green",
        fs_entry: "fs_nightglow_green",
        kind: Kind::Mesh {
            blend: Some(ADDITIVE),
            depth_write: false,
            depth_compare: wgpu::CompareFunction::Less,
        },
    },
];

/// The instance layout every star and planet sprite is drawn from: a direction
/// and a packed color, one record per body.
const STAR_ATTRIBUTES: [wgpu::VertexAttribute; 2] = [
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

fn create_pipeline(
    device: &wgpu::Device,
    pipeline_layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    sample_count: u32,
    spec: &Spec,
) -> wgpu::RenderPipeline {
    let mesh_buffers = [Vertex::buffer_layout()];
    let star_buffers = [wgpu::VertexBufferLayout {
        array_stride: stars::RECORD_SIZE as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &STAR_ATTRIBUTES,
    }];
    let (buffers, topology, cull_mode): (&[wgpu::VertexBufferLayout], _, _) = match spec.kind {
        Kind::SkyQuad => (&[], wgpu::PrimitiveTopology::TriangleStrip, None),
        Kind::StarSprite => (&star_buffers, wgpu::PrimitiveTopology::TriangleStrip, None),
        Kind::Mesh { .. } => (
            &mesh_buffers,
            wgpu::PrimitiveTopology::TriangleList,
            Some(wgpu::Face::Back),
        ),
    };
    let (blend, depth_write_enabled, depth_compare) = match spec.kind {
        Kind::SkyQuad | Kind::StarSprite => (Some(ADDITIVE), false, wgpu::CompareFunction::Always),
        Kind::Mesh {
            blend,
            depth_write,
            depth_compare,
        } => (blend, depth_write, depth_compare),
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(spec.label),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(spec.vs_entry),
            buffers,
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(spec.fs_entry),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled,
            depth_compare,
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
    res.pipelines = Pipelines::build(&res.device, &res.pipeline_layout, &res.shader, sample_count);
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
