//! GPU integration tests for the full render pipeline (vertex + fragment shaders).
//!
//! Tests the production `sphere.wgsl` and `blend.wgsl` shaders end-to-end,
//! rendering to a small offscreen texture and asserting behavioral invariants.

mod common;

use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

use wgpu::util::DeviceExt;

use sunlit_core::assets::tiles::{Geometry, PackKind, TileKey};
use sunlit_core::geometry::sphere::{Vertex, generate_uv_sphere};
use sunlit_core::renderer::tiles::{
    CellLevels, PageEntry, PageSurface, PageTable, TileId, TileLayers,
};
use sunlit_core::renderer::uniforms::Uniforms;

/// Build a perspective MVP matrix looking at the origin from distance 3.5.
fn test_mvp(width: u32, height: u32) -> [f32; 16] {
    test_mvp_with_eye(width, height, glam::Vec3::new(0.0, 0.0, 3.5))
}

/// Build a perspective MVP matrix looking at the origin from a custom eye position.
fn test_mvp_with_eye(width: u32, height: u32, eye: glam::Vec3) -> [f32; 16] {
    let center = glam::Vec3::ZERO;
    let up = glam::Vec3::Y;
    let view = glam::Mat4::look_at_rh(eye, center, up);
    #[allow(clippy::cast_precision_loss)]
    let aspect = width as f32 / height as f32;
    let proj = glam::Mat4::perspective_rh(20.0_f32.to_radians(), aspect, 0.1, 100.0);
    (proj * view).to_cols_array()
}

// ---------------------------------------------------------------------------
// Shared render pipeline context
// ---------------------------------------------------------------------------

/// The two shader files the renderer concatenates, with a probe appended.
///
/// Every entry point a test compiles reads production's own declarations, so a
/// field or a function it names is the one `sphere.wgsl` declares.
fn production_shaders(probe: &str) -> String {
    format!(
        "{}\n{}\n{probe}",
        include_str!("../shaders/blend.wgsl"),
        include_str!("../shaders/sphere.wgsl"),
    )
}

struct RenderContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    shader: wgpu::ShaderModule,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform_buffer: wgpu::Buffer,
    sampler: wgpu::Sampler,
    /// The renderer's own surface sampler, which the cube path reads through.
    surface_sampler: wgpu::Sampler,
    /// A 1x1 black cube for every cube place a case leaves empty.
    dummy_cube: wgpu::TextureView,
    /// A one-layer black tile array and a page table of one cell per face that
    /// draws the floor, for every case that draws no tile.
    dummy_tiles: [wgpu::TextureView; 2],
}

#[allow(clippy::too_many_lines)]
fn create_render_context() -> RenderContext {
    let ctx = common::create_gpu_context(false);
    let device = ctx.device;
    let queue = ctx.queue;

    let mesh = generate_uv_sphere(32, 32);

    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("test_vertices"),
        contents: bytemuck::cast_slice(&mesh.vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });

    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("test_indices"),
        contents: bytemuck::cast_slice(&mesh.indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test_uniforms"),
        size: std::mem::size_of::<Uniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("test_sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("test_bind_group_layout"),
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
            cube_entry(4),
            cube_entry(5),
            cube_entry(6),
            wgpu::BindGroupLayoutEntry {
                binding: 7,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 8,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Uint,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
        ],
    });
    let dummy_cube = create_solid_cube(&device, &queue, [[0, 0, 0, 255]; 6]);
    let dummy_tiles = create_dummy_tiles(&device, &queue);
    let surface_sampler = device.create_sampler(
        &sunlit_core::renderer::surface_sampler_descriptor(ctx.cpu_adapter),
    );

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("test_pipeline_layout"),
        bind_group_layouts: &[&bind_group_layout],
        immediate_size: 0,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("test_sphere_shader"),
        source: wgpu::ShaderSource::Wgsl(production_shaders("").into()),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("test_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[Vertex::buffer_layout()],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
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
            count: 1,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview_mask: None,
        cache: None,
    });

    #[allow(clippy::cast_possible_truncation)]
    let index_count = mesh.indices.len() as u32;

    RenderContext {
        device,
        queue,
        shader,
        pipeline,
        bind_group_layout,
        vertex_buffer,
        index_buffer,
        index_count,
        uniform_buffer,
        sampler,
        surface_sampler,
        dummy_cube,
        dummy_tiles,
    }
}

/// A one-layer black tile array and a page table of one floor cell per face.
fn create_dummy_tiles(device: &wgpu::Device, queue: &wgpu::Queue) -> [wgpu::TextureView; 2] {
    let texture = |label, format, layers, data: &[u8]| {
        device
            .create_texture_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: layers,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                data,
            )
            .create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
    };
    [
        texture(
            "dummy_tile_array",
            wgpu::TextureFormat::Rgba8Unorm,
            1,
            &[0, 0, 0, 255],
        ),
        texture(
            "dummy_page_table",
            wgpu::TextureFormat::R32Uint,
            6,
            &[0; 24],
        ),
    ]
}

static RENDER_CTX: LazyLock<Mutex<RenderContext>> =
    LazyLock::new(|| Mutex::new(create_render_context()));

fn render_ctx() -> MutexGuard<'static, RenderContext> {
    RENDER_CTX.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Clear color: dark navy (matches production)
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.02,
    g: 0.02,
    b: 0.05,
    a: 1.0,
};

/// Create a 1x1 solid-color texture.
fn create_solid_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    rgba: [u8; 4],
) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("solid_texture"),
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
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &rgba,
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
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

/// A cube texture binding of the production layout.
fn cube_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::Cube,
            multisampled: false,
        },
        count: None,
    }
}

/// An RGBA8 cube `size` texels wide with one level, face `f` from
/// `texels(f)`, in cube layer order.
fn create_cube(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: u32,
    texels: impl Fn(usize) -> Vec<u8>,
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test_cube"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for face in 0..6_u32 {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: face,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &texels(face as usize),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * size),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
    }
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::Cube),
        ..Default::default()
    })
}

/// A 1x1 cube with one solid color per face.
fn create_solid_cube(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    faces: [[u8; 4]; 6],
) -> wgpu::TextureView {
    create_cube(device, queue, 1, |face| faces[face].to_vec())
}

/// A bind group of the production layout, with no tiles.
fn test_bind_group(
    ctx: &RenderContext,
    flat: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    cubes: [&wgpu::TextureView; 3],
) -> wgpu::BindGroup {
    let [tiles, pages] = &ctx.dummy_tiles;
    bind_group_with(ctx, flat, sampler, cubes, [tiles, pages])
}

/// A bind group of the production layout, with the tile array and the page
/// table `tiles`.
fn bind_group_with(
    ctx: &RenderContext,
    flat: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    cubes: [&wgpu::TextureView; 3],
    tiles: [&wgpu::TextureView; 2],
) -> wgpu::BindGroup {
    let view = wgpu::BindingResource::TextureView;
    ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("test_bind_group"),
        layout: &ctx.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: ctx.uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: view(flat),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: view(cubes[0]),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: view(cubes[1]),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: view(cubes[2]),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: view(tiles[0]),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: view(tiles[1]),
            },
        ],
    })
}

/// Render a frame from a solid day color and a solid night color and return
/// the RGBA8 pixel data.
///
/// The day color's alpha is the water: 255 is land and 128 open water, as the
/// shader reads a water cube of 254, so a case states a surface in one color
/// apiece.
fn render_frame(
    ctx: &RenderContext,
    uniforms: &Uniforms,
    day: [u8; 4],
    night: [u8; 4],
    width: u32,
    height: u32,
) -> Vec<u8> {
    let water = u8::try_from((255 - u16::from(day[3])) * 2).unwrap_or(u8::MAX);
    let day_cube = create_solid_cube(&ctx.device, &ctx.queue, [[day[0], day[1], day[2], 255]; 6]);
    let night_cube = create_solid_cube(&ctx.device, &ctx.queue, [night; 6]);
    let water_cube = create_solid_cube(&ctx.device, &ctx.queue, [[water, 0, 0, 255]; 6]);
    render_cube_frame(
        ctx,
        uniforms,
        [&day_cube, &night_cube, &water_cube],
        width,
        height,
    )
}

/// Render a frame from the day, night and water cubes through the surface
/// sampler and return the RGBA8 pixel data.
fn render_cube_frame(
    ctx: &RenderContext,
    uniforms: &Uniforms,
    cubes: [&wgpu::TextureView; 3],
    width: u32,
    height: u32,
) -> Vec<u8> {
    let flat = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);
    let bind_group = test_bind_group(ctx, &flat, &ctx.surface_sampler, cubes);
    render_with(ctx, uniforms, &bind_group, width, height)
}

/// Render the globe with `bind_group` and return the RGBA8 pixel data.
fn render_with(
    ctx: &RenderContext,
    uniforms: &Uniforms,
    bind_group: &wgpu::BindGroup,
    width: u32,
    height: u32,
) -> Vec<u8> {
    ctx.queue
        .write_buffer(&ctx.uniform_buffer, 0, bytemuck::cast_slice(&[*uniforms]));

    let render_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test_render_target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });

    let depth_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test_depth"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });

    let color_view = render_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());

    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("test_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });

        pass.set_pipeline(&ctx.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.set_vertex_buffer(0, ctx.vertex_buffer.slice(..));
        pass.set_index_buffer(ctx.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..ctx.index_count, 0, 0..1);
    }

    ctx.queue.submit(std::iter::once(encoder.finish()));

    common::read_texture_rgba8(&ctx.device, &ctx.queue, &render_texture, width, height)
        .expect("reading the render target back")
}

/// Count pixels that are NOT the clear color.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn count_non_clear_pixels(pixels: &[u8]) -> usize {
    let clear_r = (CLEAR_COLOR.r * 255.0) as u8;
    let clear_g = (CLEAR_COLOR.g * 255.0) as u8;
    let clear_b = (CLEAR_COLOR.b * 255.0) as u8;

    pixels
        .chunks(4)
        .filter(|px| {
            let dr = px[0].abs_diff(clear_r);
            let dg = px[1].abs_diff(clear_g);
            let db = px[2].abs_diff(clear_b);
            dr > 1 || dg > 1 || db > 1
        })
        .count()
}

/// Average luminance of pixels in a rectangular region.
fn avg_luminance_region(pixels: &[u8], width: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> f64 {
    let mut sum = 0.0_f64;
    let mut count = 0u64;
    for y in y0..y1 {
        for x in x0..x1 {
            let idx = ((y * width + x) * 4) as usize;
            let r = f64::from(pixels[idx]);
            let g = f64::from(pixels[idx + 1]);
            let b = f64::from(pixels[idx + 2]);
            sum += 0.2126 * r + 0.7152 * g + 0.0722 * b;
            count += 1;
        }
    }
    #[allow(clippy::cast_precision_loss)]
    {
        sum / count as f64
    }
}

/// Helper: default uniforms with identity color correction and no clouds.
#[expect(
    clippy::cast_precision_loss,
    reason = "test viewport dimensions are small integers"
)]
fn default_test_uniforms(size: u32) -> Uniforms {
    Uniforms {
        mvp: test_mvp(size, size),
        sun_dir: [0.0, 0.0, 1.0],
        terminator_width: -1.0,
        flags: 0,
        diffuse_floor: 0.1,
        diffuse_ramp: 0.6,
        _pad: 0.0,
        eye_pos: [0.0, 0.0, 3.5],
        _pad2: 0.0,
        spec_shininess: 150.0,
        spec_intensity: 0.0,
        fresnel_mix: 0.0,
        fresnel_exp: 5.0,
        day_gamma: 1.0,
        day_saturation: 1.0,
        night_gamma: 1.0,
        night_saturation: 1.0,
        cloud_sphere_radius: 1.0015,
        cloud_opacity: 0.0,
        cloud_floor: 0.0,
        cloud_gamma: 1.0,
        rayleigh_intensity: 0.0,
        rayleigh_sharpness: 50.0,
        nightglow_intensity: 0.0,
        nightglow_falloff: 15.0,
        nightglow_balance: 0.37,
        rayleigh_radius: 1.003,
        nightglow_orange_radius: 1.014,
        nightglow_green_radius: 1.015,
        rayleigh_haze: 0.55,
        cloud_night: 0.35,
        cloud_terminator: sunlit_core::params::CLOUD_TERMINATOR_WIDTH,
        _pad5: 0.0,
        sky_view: glam::Mat4::IDENTITY.to_cols_array(),
        world_from_eqj: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ],
        viewport_size: [size as f32, size as f32],
        screen_offset: [0.0, 0.0],
        star_intensity: 2.0,
        star_mag_limit: 6.5,
        star_size: 1.0,
        star_glow_strength: 0.5,
        star_glow_radius: 8.0,
        star_contrast: 0.3,
        sky_fov: 140.0,
        // These GPU cases render the Earth and its shells; the Sun's own two
        // draws are covered by the engine and golden suites, and leaving the
        // glare out here keeps a shading assertion measuring shading.
        sun_glow: 0.0,
        sun_rays: 0.0,
        sun_flare: 0.0,
        sun_visible: 1.0,
        sun_size: 1.0,
        sun_view_dir: [0.0, 0.0, -1.0],
        sun_disk_radius: 2.0,
        moon_model: MOON_MODEL_IDENTITY,
        moon_brightness: 0.0,
        moon_earthshine: 0.0,
        // These cases bind the Earth's own texture rather than a panorama, and
        // the layer's draw is not among the ones they encode.
        milky_way_intensity: 0.0,
        cloud_opacity_night: 0.55,
        sun_glare_tint: [1.0, 1.0, 1.0],
        sun_horizon_gain: 1.0,
        // A globe of a hundred pixels with a ten pixel band around it, so the
        // shell's own forward lobe has a height to read even where these cases
        // draw no Sun.
        sun_globe_center: [size as f32 * 0.5, size as f32 * 0.5],
        sun_globe_radius: 100.0,
        sun_zone_width: 10.0,
        sun_squash: 1.0,
        sun_halo_radius: 3.0,
        sun_reddening: 1.0,
        atmo_sunrise_glow: 0.0,
        atmo_sunrise_g: 0.5,
        sun_flux: 1.0,
        day_ocean: 0,
        night_ocean: 0,
        tile_texels: 0.0,
        tile_gutter: 0.0,
        _pad9: 0.0,
        _pad10: 0.0,
    }
}

/// A model matrix that puts a unit Moon at the world origin. These cases draw
/// no Moon (`moon_brightness` is zero), so what it has to be is valid rather
/// than meaningful.
const MOON_MODEL_IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn sphere_renders_visible_pixels() {
    let ctx = render_ctx();
    let size = 128;

    let white = [255, 255, 255, 255];
    let black = [0, 0, 0, 255];

    let uniforms = default_test_uniforms(size);

    let pixels = render_frame(&ctx, &uniforms, white, black, size, size);
    let visible = count_non_clear_pixels(&pixels);

    assert!(
        visible > 100,
        "Expected many visible (non-clear) pixels, got {visible}"
    );
}

#[test]
fn day_side_brighter_than_night_side() {
    let ctx = render_ctx();
    let size = 128;

    let white = [255, 255, 255, 255];
    let dark_gray = [30, 30, 30, 255];

    let uniforms = Uniforms {
        terminator_width: 0.15,
        flags: 1, // diffuse enabled
        ..default_test_uniforms(size)
    };

    let pixels = render_frame(&ctx, &uniforms, white, dark_gray, size, size);

    // The sphere faces the camera (along +Z). Sun is also along +Z.
    // Center of the image should be brightly lit.
    let center_lum = avg_luminance_region(
        &pixels,
        size,
        size / 4,
        size / 4,
        3 * size / 4,
        3 * size / 4,
    );

    assert!(
        center_lum > 50.0,
        "Center of day-lit sphere should be bright, got luminance {center_lum:.1}"
    );
}

#[test]
fn single_texture_mode_ignores_the_night_side() {
    let ctx = render_ctx();
    let size = 128;

    let red = [255, 0, 0, 255];
    let green = [0, 255, 0, 255];
    let black = [0, 0, 0, 255];

    // A negative terminator width is single-texture mode, where the fragment
    // shader returns before it reaches the night binding.
    let uniforms = default_test_uniforms(size);
    let pixels_green_night = render_frame(&ctx, &uniforms, red, green, size, size);

    let extreme = Uniforms {
        night_gamma: 0.3,
        night_saturation: 0.0,
        ..uniforms
    };
    let pixels_black_night = render_frame(&ctx, &extreme, red, black, size, size);

    assert_eq!(
        pixels_green_night, pixels_black_night,
        "neither the night texture nor its color correction may reach the frame \
         in single-texture mode"
    );
}

// ---------------------------------------------------------------------------
// Uniform buffer field offset test
// ---------------------------------------------------------------------------

/// A compute entry point appended to the production shaders, so the offsets it
/// reads back are the ones `sphere.wgsl` declares rather than a copy of them.
const UNIFORM_READBACK_PROBE: &str = r"
@group(1) @binding(0) var<storage, read_write> output: array<f32>;

@compute @workgroup_size(1)
fn uniform_readback() {
    // Read MVP diagonal
    output[0] = uniforms.mvp[0][0];
    output[1] = uniforms.mvp[1][1];
    output[2] = uniforms.mvp[2][2];
    output[3] = uniforms.mvp[3][3];
    // sun_dir
    output[4] = uniforms.sun_dir.x;
    output[5] = uniforms.sun_dir.y;
    output[6] = uniforms.sun_dir.z;
    // terminator_width
    output[7] = uniforms.terminator_width;
    // flags (cast to f32)
    output[8] = f32(uniforms.flags);
    // diffuse_floor
    output[9] = uniforms.diffuse_floor;
    // diffuse_ramp
    output[10] = uniforms.diffuse_ramp;
    // eye_pos
    output[11] = uniforms.eye_pos.x;
    output[12] = uniforms.eye_pos.y;
    output[13] = uniforms.eye_pos.z;
    // spec params
    output[14] = uniforms.spec_shininess;
    output[15] = uniforms.spec_intensity;
    // fresnel params
    output[16] = uniforms.fresnel_mix;
    output[17] = uniforms.fresnel_exp;
    // color correction params
    output[18] = uniforms.day_gamma;
    output[19] = uniforms.day_saturation;
    output[20] = uniforms.night_gamma;
    output[21] = uniforms.night_saturation;
    // cloud params
    output[22] = uniforms.cloud_sphere_radius;
    output[23] = uniforms.cloud_opacity;
    output[24] = uniforms.cloud_floor;
    output[25] = uniforms.cloud_gamma;
    // atmosphere params
    output[26] = uniforms.rayleigh_intensity;
    output[27] = uniforms.rayleigh_sharpness;
    output[28] = uniforms.nightglow_intensity;
    output[29] = uniforms.nightglow_falloff;
    output[30] = uniforms.nightglow_balance;
    output[31] = uniforms.rayleigh_radius;
    output[32] = uniforms.nightglow_orange_radius;
    output[33] = uniforms.nightglow_green_radius;
    output[34] = uniforms.rayleigh_haze;
    // star params
    output[35] = uniforms.star_intensity;
    output[36] = uniforms.star_mag_limit;
    output[37] = uniforms.star_size;
    output[38] = uniforms.star_glow_strength;
    output[39] = uniforms.star_glow_radius;
    output[40] = uniforms.star_contrast;
    output[41] = uniforms.sky_fov;
    // sun params
    output[42] = uniforms.sun_glow;
    output[43] = uniforms.sun_rays;
    output[44] = uniforms.sun_flare;
    output[45] = uniforms.sun_visible;
    output[46] = uniforms.sun_size;
    output[47] = uniforms.sun_view_dir.x;
    output[48] = uniforms.sun_view_dir.y;
    output[49] = uniforms.sun_view_dir.z;
    output[50] = uniforms.sun_disk_radius;
    // moon params
    output[51] = uniforms.moon_model[0][0];
    output[52] = uniforms.moon_model[1][1];
    output[53] = uniforms.moon_model[2][2];
    output[54] = uniforms.moon_model[3][0];
    output[55] = uniforms.moon_brightness;
    output[56] = uniforms.moon_earthshine;
    // the Milky Way
    output[57] = uniforms.milky_way_intensity;
    // the cloud layer's own three
    output[58] = uniforms.cloud_night;
    output[59] = uniforms.cloud_terminator;
    output[60] = uniforms.cloud_opacity_night;
    // the horizon block
    output[61] = uniforms.sun_glare_tint.r;
    output[62] = uniforms.sun_glare_tint.g;
    output[63] = uniforms.sun_glare_tint.b;
    output[64] = uniforms.sun_horizon_gain;
    output[65] = uniforms.sun_globe_center.x;
    output[66] = uniforms.sun_globe_center.y;
    output[67] = uniforms.sun_globe_radius;
    output[68] = uniforms.sun_zone_width;
    output[69] = uniforms.sun_squash;
    output[70] = uniforms.sun_halo_radius;
    output[71] = uniforms.sun_reddening;
    output[72] = uniforms.atmo_sunrise_glow;
    output[73] = uniforms.atmo_sunrise_g;
    output[74] = uniforms.sun_flux;
    // the tiles
    output[75] = f32(uniforms.day_ocean);
    output[76] = f32(uniforms.night_ocean);
    output[77] = uniforms.tile_texels;
    output[78] = uniforms.tile_gutter;
}
";

/// What the probe writes into each slot, in the order it writes them: the
/// value the Rust struct below carries in that field, and the field's name.
///
/// One row per value rather than one per field, so a field that moves is named
/// by the row that fails instead of hiding inside a wider assertion.
const PROBED_FIELDS: [(f32, &str); 79] = [
    (1.0, "mvp[0][0]"),
    (2.0, "mvp[1][1]"),
    (3.0, "mvp[2][2]"),
    (4.0, "mvp[3][3]"),
    (0.0, "sun_dir.x"),
    (1.0, "sun_dir.y"),
    (0.0, "sun_dir.z"),
    (0.15, "terminator_width"),
    (1.0, "flags"),
    (0.5, "diffuse_floor"),
    (0.25, "diffuse_ramp"),
    (1.0, "eye_pos.x"),
    (2.0, "eye_pos.y"),
    (3.0, "eye_pos.z"),
    (150.0, "spec_shininess"),
    (0.75, "spec_intensity"),
    (0.5, "fresnel_mix"),
    (3.0, "fresnel_exp"),
    (1.5, "day_gamma"),
    (0.8, "day_saturation"),
    (2.0, "night_gamma"),
    (0.6, "night_saturation"),
    (1.0015, "cloud_sphere_radius"),
    (0.9, "cloud_opacity"),
    (0.25, "cloud_floor"),
    (0.65, "cloud_gamma"),
    (0.5, "rayleigh_intensity"),
    (50.0, "rayleigh_sharpness"),
    (0.25, "nightglow_intensity"),
    (15.0, "nightglow_falloff"),
    (0.37, "nightglow_balance"),
    (1.003, "rayleigh_radius"),
    (1.014, "nightglow_orange_radius"),
    (1.015, "nightglow_green_radius"),
    (0.55, "rayleigh_haze"),
    (1.0, "star_intensity"),
    (6.5, "star_mag_limit"),
    (1.25, "star_size"),
    (0.35, "star_glow_strength"),
    (6.0, "star_glow_radius"),
    (0.4, "star_contrast"),
    (123.0, "sky_fov"),
    (1.75, "sun_glow"),
    (0.45, "sun_rays"),
    (0.8, "sun_flare"),
    (0.6, "sun_visible"),
    (2.75, "sun_size"),
    (0.0, "sun_view_dir.x"),
    (0.6, "sun_view_dir.y"),
    (-0.8, "sun_view_dir.z"),
    (7.5, "sun_disk_radius"),
    (0.25, "moon_model[0][0]"),
    (0.5, "moon_model[1][1]"),
    (0.75, "moon_model[2][2]"),
    (12.5, "moon_model[3][0]"),
    (1.25, "moon_brightness"),
    (0.35, "moon_earthshine"),
    (0.65, "milky_way_intensity"),
    (0.31, "cloud_night"),
    (0.19, "cloud_terminator"),
    (0.61, "cloud_opacity_night"),
    (0.95, "sun_glare_tint.r"),
    (0.55, "sun_glare_tint.g"),
    (0.15, "sun_glare_tint.b"),
    (2.25, "sun_horizon_gain"),
    (64.5, "sun_globe_center.x"),
    (33.25, "sun_globe_center.y"),
    (41.5, "sun_globe_radius"),
    (6.25, "sun_zone_width"),
    (0.45, "sun_squash"),
    (4.25, "sun_halo_radius"),
    (1.35, "sun_reddening"),
    (1.85, "atmo_sunrise_glow"),
    (0.62, "atmo_sunrise_g"),
    (0.72, "sun_flux"),
    (3_939_850.0, "day_ocean"),
    (984_325.0, "night_ocean"),
    (128.0, "tile_texels"),
    (8.0, "tile_gutter"),
];

#[allow(clippy::too_many_lines)]
#[test]
fn uniform_buffer_field_offsets_match_wgsl() {
    let ctx = render_ctx();

    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("uniform_readback_shader"),
            source: wgpu::ShaderSource::Wgsl(production_shaders(UNIFORM_READBACK_PROBE).into()),
        });

    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("uniform_readback_pipeline"),
            layout: None,
            module: &shader,
            entry_point: Some("uniform_readback"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

    let mut mvp = [0.0f32; 16];
    mvp[0] = 1.0;
    mvp[5] = 2.0;
    mvp[10] = 3.0;
    mvp[15] = 4.0;

    let uniforms = Uniforms {
        mvp,
        sun_dir: [0.0, 1.0, 0.0],
        terminator_width: 0.15,
        flags: 1,
        diffuse_floor: 0.5,
        diffuse_ramp: 0.25,
        _pad: 0.0,
        eye_pos: [1.0, 2.0, 3.0],
        _pad2: 0.0,
        spec_shininess: 150.0,
        spec_intensity: 0.75,
        fresnel_mix: 0.5,
        fresnel_exp: 3.0,
        day_gamma: 1.5,
        day_saturation: 0.8,
        night_gamma: 2.0,
        night_saturation: 0.6,
        cloud_sphere_radius: 1.0015,
        cloud_opacity: 0.9,
        cloud_floor: 0.25,
        cloud_gamma: 0.65,
        rayleigh_intensity: 0.5,
        rayleigh_sharpness: 50.0,
        nightglow_intensity: 0.25,
        nightglow_falloff: 15.0,
        nightglow_balance: 0.37,
        rayleigh_radius: 1.003,
        nightglow_orange_radius: 1.014,
        nightglow_green_radius: 1.015,
        rayleigh_haze: 0.55,
        cloud_night: 0.31,
        cloud_terminator: 0.19,
        _pad5: 0.0,
        sky_view: mvp,
        world_from_eqj: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ],
        viewport_size: [128.0, 128.0],
        screen_offset: [0.0, 0.0],
        star_intensity: 1.0,
        star_mag_limit: 6.5,
        star_size: 1.25,
        star_glow_strength: 0.35,
        star_glow_radius: 6.0,
        star_contrast: 0.4,
        sky_fov: 123.0,
        sun_glow: 1.75,
        sun_rays: 0.45,
        sun_flare: 0.8,
        sun_visible: 0.6,
        sun_size: 2.75,
        sun_view_dir: [0.0, 0.6, -0.8],
        sun_disk_radius: 7.5,
        // Column major, so the last column's first component is the Moon's x
        // position and the diagonal is its scale.
        moon_model: [
            0.25, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.75, 0.0, 12.5, 0.0, 0.0, 1.0,
        ],
        moon_brightness: 1.25,
        moon_earthshine: 0.35,
        milky_way_intensity: 0.65,
        cloud_opacity_night: 0.61,
        sun_glare_tint: [0.95, 0.55, 0.15],
        sun_horizon_gain: 2.25,
        sun_globe_center: [64.5, 33.25],
        sun_globe_radius: 41.5,
        sun_zone_width: 6.25,
        sun_squash: 0.45,
        sun_halo_radius: 4.25,
        sun_reddening: 1.35,
        atmo_sunrise_glow: 1.85,
        atmo_sunrise_g: 0.62,
        sun_flux: 0.72,
        day_ocean: 0x003C_1E0A,
        night_ocean: 0x000F_0505,
        tile_texels: 128.0,
        tile_gutter: 8.0,
        _pad9: 0.0,
        _pad10: 0.0,
    };

    let uniform_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("test_uniform_buf"),
            contents: bytemuck::cast_slice(&[uniforms]),
            usage: wgpu::BufferUsages::UNIFORM,
        });

    let output_size = (PROBED_FIELDS.len() * std::mem::size_of::<f32>()) as u64;
    let output_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("uniform_test_output"),
        size: output_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let uniform_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buf.as_entire_binding(),
        }],
    });
    let output_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(1),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: output_buf.as_entire_binding(),
        }],
    });

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: None,
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &uniform_group, &[]);
        pass.set_bind_group(1, &output_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    ctx.queue.submit(std::iter::once(encoder.finish()));

    let data = common::read_buffer(&ctx.device, &ctx.queue, &output_buf, output_size);
    let values: &[f32] = bytemuck::cast_slice(&data);

    for (index, (expected, name)) in PROBED_FIELDS.iter().enumerate() {
        assert!(
            (values[index] - expected).abs() < 1e-6,
            "{name}: got {}, expected {expected}",
            values[index]
        );
    }
}

// ---------------------------------------------------------------------------
// The two rules both sides of the boundary spell out
// ---------------------------------------------------------------------------

/// A compute entry point appended to the production shaders, so what it
/// evaluates is the source the renderer compiles rather than a copy of it.
const SHARED_RULE_PROBE: &str = "
@group(1) @binding(0) var<storage, read_write> rule_out: array<f32>;
@group(1) @binding(1) var<storage, read> rule_heights: array<f32>;

@compute @workgroup_size(1)
fn shared_rule_probe() {
    rule_out[0] = output_pixel_scale();
    rule_out[1] = sky_lens_edge_radius();
    for (var i = 0u; i < arrayLength(&rule_heights); i = i + 1u) {
        let transmitted = limb_transmission_km(rule_heights[i]);
        rule_out[2u + i * 4u] = transmitted.r;
        rule_out[3u + i * 4u] = transmitted.g;
        rule_out[4u + i * 4u] = transmitted.b;
        rule_out[5u + i * 4u] = limb_disk_amplitude(transmitted.g);
    }
}
";

/// Heights through the band the third rule is compared at: the surface, the
/// few kilometers where the disk fades out, the rows the research table names,
/// and the top of the band where the path takes nothing.
const RULE_HEIGHTS_KM: [f32; 9] = [0.0, 2.0, 5.0, 8.0, 13.0, 20.0, 27.0, 50.0, 95.565];

/// Below the density ramp's knee, on it, three points up it, and past the
/// ceiling.
const RULE_VIEWPORT_HEIGHTS: [f32; 6] = [540.0, 1080.0, 1350.0, 1620.0, 2160.0, 3240.0];

/// Under the lens's lower clamp, both ends of the slider's range, two widths
/// only a spanned canvas derives, and over the upper clamp.
const RULE_SKY_FOVS: [f32; 8] = [30.0, 60.0, 95.0, 140.0, 180.0, 220.0, 330.0, 400.0];

/// Both ends of the reddening slider and the measured atmosphere in between;
/// zero is the white Sun and has to stay exactly white.
const RULE_REDDENINGS: [f32; 3] = [0.0, 1.0, 2.0];

/// Three rules exist once in WGSL and once on the CPU, in `scene::sky_lens` and
/// `scene::limb_extinction`, and every
/// pairing matters at the pixel. The CPU sizes the Sun's disk with the density
/// ramp and the shader draws that disk's antialiased edge with it; the CPU
/// measures occlusion at a screen position the shader has to draw the Sun at;
/// and the CPU integrates the light path over the visible disk to decide what
/// color and how bright the glare is while the shader draws the disk that glare
/// is supposed to have come from.
///
/// The viewport heights avoid 1080 and below, where the ramp clamps to 1.0 and
/// any two knees agree: every golden and every engine frame renders there, so
/// nothing else in the suite can see a divergence at all.
#[test]
#[allow(clippy::too_many_lines)]
fn the_shader_and_the_cpu_agree_on_the_three_shared_rules() {
    let ctx = render_ctx();

    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shared_rule_probe_shader"),
            source: wgpu::ShaderSource::Wgsl(production_shaders(SHARED_RULE_PROBE).into()),
        });
    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("shared_rule_probe_pipeline"),
            layout: None,
            module: &shader,
            entry_point: Some("shared_rule_probe"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

    let output_size = ((2 + RULE_HEIGHTS_KM.len() * 4) * std::mem::size_of::<f32>()) as u64;
    let output_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("shared_rule_probe_output"),
        size: output_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let heights_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("shared_rule_probe_heights"),
            contents: bytemuck::cast_slice(&RULE_HEIGHTS_KM),
            usage: wgpu::BufferUsages::STORAGE,
        });
    let uniform_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("shared_rule_probe_uniforms"),
        size: std::mem::size_of::<Uniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let uniform_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buf.as_entire_binding(),
        }],
    });
    let output_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(1),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: output_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: heights_buf.as_entire_binding(),
            },
        ],
    });

    let probe = |height: f32, sky_fov: f32, reddening: f32| {
        let uniforms = Uniforms {
            viewport_size: [height * 16.0 / 9.0, height],
            sky_fov,
            sun_reddening: reddening,
            ..default_test_uniforms(64)
        };
        ctx.queue
            .write_buffer(&uniform_buf, 0, bytemuck::cast_slice(&[uniforms]));
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &uniform_group, &[]);
            pass.set_bind_group(1, &output_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        ctx.queue.submit(std::iter::once(encoder.finish()));
        let data = common::read_buffer(&ctx.device, &ctx.queue, &output_buf, output_size);
        bytemuck::cast_slice::<u8, f32>(&data).to_vec()
    };

    // Each of the three rules reads one uniform and nothing else, so walking
    // the three axes together covers every value a cross product would.
    for step in 0..RULE_SKY_FOVS.len() {
        let height = RULE_VIEWPORT_HEIGHTS[step % RULE_VIEWPORT_HEIGHTS.len()];
        let sky_fov = RULE_SKY_FOVS[step];
        let reddening = RULE_REDDENINGS[step % RULE_REDDENINGS.len()];

        let values = probe(height, sky_fov, reddening);
        let cpu_scale = sunlit_core::scene::sky_lens::pixel_scale(height);
        let cpu_edge = sunlit_core::scene::sky_lens::sky_lens_edge_radius(sky_fov);
        assert!(
            (values[0] - cpu_scale).abs() < 1e-6,
            "the density ramp at {height} pixels: the shader says {}, \
             scene::sky_lens::pixel_scale says {cpu_scale}",
            values[0]
        );
        assert!(
            (values[1] - cpu_edge).abs() < 2e-5 * cpu_edge,
            "the sky lens edge radius at {sky_fov} degrees: the shader says {}, \
             scene::sky_lens::sky_lens_edge_radius says {cpu_edge}",
            values[1]
        );
        for (index, km) in RULE_HEIGHTS_KM.iter().enumerate() {
            let cpu = sunlit_core::scene::limb_extinction::limb_transmission(*km, reddening);
            let shader = glam::Vec3::new(
                values[2 + index * 4],
                values[3 + index * 4],
                values[4 + index * 4],
            );
            // Relative, because the band runs over ten decades and an
            // absolute bound would say nothing at the bottom of it.
            let apart = (shader - cpu).abs() / cpu.abs().max(glam::Vec3::splat(1e-30));
            assert!(
                apart.max_element() < 2e-3,
                "the light path at {km} km and reddening {reddening}: the shader says \
                 {shader}, scene::limb_extinction::limb_transmission says {cpu}"
            );
            let cpu_fade = sunlit_core::scene::limb_extinction::limb_disk_amplitude(cpu.y);
            assert!(
                (values[5 + index * 4] - cpu_fade).abs() < 1e-3,
                "the disk's fade at {km} km: the shader says {}, \
                 scene::limb_extinction::limb_disk_amplitude says {cpu_fade}",
                values[5 + index * 4]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The panorama's reconstruction against the projection it inverts
// ---------------------------------------------------------------------------

/// A second compute entry point appended to the production shaders, so the
/// four functions it composes are the ones the renderer compiles.
const ROUND_TRIP_PROBE: &str = "
@group(1) @binding(0) var<storage, read> trip_directions: array<vec4<f32>>;
@group(1) @binding(1) var<storage, read_write> trip_results: array<vec4<f32>>;

@compute @workgroup_size(1)
fn round_trip_probe(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    let eqj = normalize(trip_directions[index].xyz);
    let point = sky_lens_project(view_from_eqj(eqj));
    let back = normalize(milky_way_direction(ndc_to_pixels(point.ndc)));
    trip_results[index] = vec4<f32>(back, point.theta);
}
";

/// `milky_way_direction` has to be the exact inverse of `sky_lens_project`
/// after `view_from_eqj`, because the panorama it samples sits under sprites the
/// forward pair places.
///
/// A round trip on the GPU rather than a re-derivation on the CPU: the forward
/// half is production's, the inverse half is production's, and nothing here
/// spells either of them out. Real camera and real astronomy, so both matrices
/// are ones the renderer actually writes, and both signs of pan, which no other
/// frame in this file carries.
#[test]
#[allow(clippy::too_many_lines)]
fn the_panoramas_reconstruction_inverts_the_projection_it_sits_under() {
    /// How far a direction may come back from where it went in, as a distance
    /// between two unit vectors.
    ///
    /// An order of magnitude above the worse adapter's transcendental noise,
    /// and three below the faults it exists to catch. The measurements are in
    /// `docs/rendering.md`.
    const TOLERANCE: f32 = 1.0e-3;

    let ctx = render_ctx();

    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("round_trip_probe_shader"),
            source: wgpu::ShaderSource::Wgsl(production_shaders(ROUND_TRIP_PROBE).into()),
        });
    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("round_trip_probe_pipeline"),
            layout: None,
            module: &shader,
            entry_point: Some("round_trip_probe"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

    // A spread of equatorial J2000 directions: the poles, the equator all the
    // way round, and two mid-latitude rings, so no component of either
    // transpose can be zero everywhere.
    let mut directions: Vec<[f32; 4]> = vec![[0.0, 0.0, 1.0, 0.0], [0.0, 0.0, -1.0, 0.0]];
    for declination in [-60.0_f32, -25.0, 0.0, 25.0, 60.0] {
        for step in 0..12 {
            let right_ascension = f32::from(u8::try_from(step).expect("a small index")) * 30.0;
            let (ra, dec) = (right_ascension.to_radians(), declination.to_radians());
            directions.push([dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin(), 0.0]);
        }
    }
    #[allow(clippy::cast_possible_truncation)]
    let count = directions.len() as u32;
    let buffer_size = std::mem::size_of_val(directions.as_slice()) as u64;

    let input_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("round_trip_probe_input"),
            contents: bytemuck::cast_slice(&directions),
            usage: wgpu::BufferUsages::STORAGE,
        });
    let output_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("round_trip_probe_output"),
        size: buffer_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let uniform_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("round_trip_probe_uniforms"),
        size: std::mem::size_of::<Uniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let uniform_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buf.as_entire_binding(),
        }],
    });
    let storage_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(1),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: input_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output_buf.as_entire_binding(),
            },
        ],
    });

    let datetime = sunlit_core::scene::sun::DateTimeInput {
        use_custom: true,
        custom_hour: 7.0,
        custom_day_of_year: 172,
        custom_year: 2026,
    };
    let sky = sunlit_core::scene::sky::compute_sky_state(&datetime);
    let world_from_eqj = [
        sky.world_from_eqj.x_axis.extend(0.0).into(),
        sky.world_from_eqj.y_axis.extend(0.0).into(),
        sky.world_from_eqj.z_axis.extend(0.0).into(),
    ];

    let probe = |sky_fov: f32, offset: glam::Vec2, viewport: glam::Vec2| {
        let camera = sunlit_core::scene::camera::OrbitalCamera::new(
            41.0,
            -17.0,
            sunlit_core::scene::camera::zoom_to_distance(0.45),
        );
        let uniforms = Uniforms {
            sky_view: camera.view_matrix().to_cols_array(),
            world_from_eqj,
            viewport_size: viewport.into(),
            screen_offset: offset.into(),
            sky_fov,
            ..default_test_uniforms(64)
        };
        ctx.queue
            .write_buffer(&uniform_buf, 0, bytemuck::cast_slice(&[uniforms]));
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &uniform_group, &[]);
            pass.set_bind_group(1, &storage_group, &[]);
            pass.dispatch_workgroups(count, 1, 1);
        }
        ctx.queue.submit(std::iter::once(encoder.finish()));
        let data = common::read_buffer(&ctx.device, &ctx.queue, &output_buf, buffer_size);
        bytemuck::cast_slice::<u8, [f32; 4]>(&data).to_vec()
    };

    // The two ends of the slider, the default, and a width only a spanned
    // canvas derives, each with no pan and a pan of either sign in both axes.
    for sky_fov in [60.0_f32, 140.0, 180.0, 300.0] {
        for offset in [
            glam::Vec2::ZERO,
            glam::Vec2::new(0.35, 0.2),
            glam::Vec2::new(-0.35, -0.2),
        ] {
            for viewport in [
                glam::Vec2::new(1280.0, 720.0),
                glam::Vec2::new(512.0, 512.0),
            ] {
                let results = probe(sky_fov, offset, viewport);
                let mut checked = 0;
                let mut worst = (0.0_f32, 0.0_f32);
                for (sent, got) in directions.iter().zip(&results) {
                    let sent = glam::Vec3::from_slice(&sent[0..3]).normalize();
                    let theta = got[3];
                    // Past this the projected radius is `tan(theta / 2)` of a
                    // very large number and single precision has nothing left;
                    // the shader clamps at the antipode for the same reason.
                    if theta > 170.0_f32.to_radians() {
                        continue;
                    }
                    checked += 1;
                    let back = glam::Vec3::from_slice(&got[0..3]);
                    // The straight-line distance between two unit vectors
                    // rather than the angle between them: `acos` of a dot
                    // product a couple of ULP short of one reports half a
                    // milliradian that is entirely the measurement's, a floor
                    // no round trip could ever get under.
                    let apart = (sent - back).length();
                    if apart > worst.0 {
                        worst = (apart, theta.to_degrees());
                    }
                    assert!(
                        apart < TOLERANCE,
                        "at {sky_fov} degrees of sky, pan {offset:?}, viewport {viewport:?}:                          {sent:?} came back as {back:?}, {apart} away"
                    );
                }
                println!(
                    "fov {sky_fov} pan {offset:?} viewport {viewport:?}: worst {:.3e} at theta {:.1}",
                    worst.0, worst.1
                );
                assert!(
                    checked > 40,
                    "only {checked} of the directions were inside the range this checks"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Fresnel specular tests
// ---------------------------------------------------------------------------

/// Average luminance of non-clear pixels in the frame.
fn avg_luminance_non_clear(pixels: &[u8]) -> f64 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let clear_r = (CLEAR_COLOR.r * 255.0) as u8;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let clear_g = (CLEAR_COLOR.g * 255.0) as u8;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let clear_b = (CLEAR_COLOR.b * 255.0) as u8;

    let mut sum = 0.0_f64;
    let mut count = 0u64;
    for px in pixels.chunks(4) {
        let dr = px[0].abs_diff(clear_r);
        let dg = px[1].abs_diff(clear_g);
        let db = px[2].abs_diff(clear_b);
        if dr > 1 || dg > 1 || db > 1 {
            let r = f64::from(px[0]);
            let g = f64::from(px[1]);
            let b = f64::from(px[2]);
            sum += 0.2126 * r + 0.7152 * g + 0.0722 * b;
            count += 1;
        }
    }
    if count == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    {
        sum / count as f64
    }
}

#[test]
fn the_water_effects_are_inert_while_their_gates_are_zero() {
    let ctx = render_ctx();
    let size = 128;

    // All-water texture (alpha=128): RGB can be ocean-like
    let water = [10, 30, 60, 128];
    let night = [5, 5, 10, 128];

    // Both gates are zero here, so neither the glint's exponent nor the
    // diffuse shift's Fresnel exponent may reach the frame.
    let closed = Uniforms {
        terminator_width: 0.15,
        flags: 1,
        ..default_test_uniforms(size)
    };
    let varied = Uniforms {
        spec_shininess: 8.0,
        fresnel_exp: 1.0,
        ..closed
    };
    let pixels_closed = render_frame(&ctx, &closed, water, night, size, size);
    let pixels_varied = render_frame(&ctx, &varied, water, night, size, size);
    assert_eq!(
        pixels_closed, pixels_varied,
        "spec_intensity and fresnel_mix at zero should skip both blocks, \
         so the parameters they read cannot change a pixel"
    );

    // The same two parameters through open gates, so the equality above is a
    // property of the gates rather than of parameters nothing reads.
    let open = Uniforms {
        spec_intensity: 0.5,
        fresnel_mix: 0.5,
        ..varied
    };
    let pixels_open = render_frame(&ctx, &open, water, night, size, size);
    assert_ne!(
        pixels_closed, pixels_open,
        "opening both gates should change the frame"
    );
}

#[test]
fn fresnel_specular_brighter_at_grazing() {
    let ctx = render_ctx();
    let size = 128;

    let water = [10, 30, 60, 128];
    let night = [5, 5, 10, 128];

    // The brightest pixel the glint adds, which is the highlight itself: the
    // two cameras see different amounts of the night side, so an average over
    // the frame would compare how much ocean is lit rather than how bright the
    // reflection is. An intensity of 0.1 keeps both highlights under the clip.
    let peak_glint = |eye: glam::Vec3| {
        let dark = Uniforms {
            mvp: test_mvp_with_eye(size, size, eye),
            terminator_width: 0.15,
            flags: 1,
            eye_pos: eye.into(),
            ..default_test_uniforms(size)
        };
        let lit = Uniforms {
            spec_intensity: 0.1,
            ..dark
        };
        let pixels_dark = render_frame(&ctx, &dark, water, night, size, size);
        let pixels_lit = render_frame(&ctx, &lit, water, night, size, size);
        let luminance = |px: &[u8]| {
            0.2126 * f64::from(px[0]) + 0.7152 * f64::from(px[1]) + 0.0722 * f64::from(px[2])
        };
        pixels_dark
            .chunks(4)
            .zip(pixels_lit.chunks(4))
            .map(|(before, after)| luminance(after) - luminance(before))
            .fold(f64::MIN, f64::max)
    };

    // Head-on the reflection sits where the water faces the camera and the
    // Schlick term is at its 0.02 floor. Swing the camera past the terminator
    // and the same highlight lands on water that turns away, where the term is
    // several times larger. Both cameras sit far enough back for the whole
    // globe to be in frame, because the grazing highlight is near the limb.
    let eye_at = |degrees: f32| {
        let angle = degrees.to_radians();
        glam::Vec3::new(12.0 * angle.sin(), 0.0, 12.0 * angle.cos())
    };
    let head_on = peak_glint(eye_at(0.0));
    let grazing = peak_glint(eye_at(140.0));

    assert!(
        grazing > head_on,
        "the highlight should be brighter where the water grazes the camera \
         ({grazing:.1}) than where it faces it ({head_on:.1})"
    );
}

// ---------------------------------------------------------------------------
// Fresnel diffuse shift tests
// ---------------------------------------------------------------------------

#[test]
fn fresnel_diffuse_shift_brightens_grazing_water() {
    let ctx = render_ctx();
    let size = 128;

    let water = [10, 30, 60, 128];
    let night = [5, 5, 10, 128];

    let uniforms_no_shift = Uniforms {
        terminator_width: 0.15,
        flags: 1,
        ..default_test_uniforms(size)
    };
    let pixels_no_shift = render_frame(&ctx, &uniforms_no_shift, water, night, size, size);
    let lum_no_shift = avg_luminance_non_clear(&pixels_no_shift);

    // With Fresnel diffuse shift (the sky color is brighter than the ocean)
    let uniforms_with_shift = Uniforms {
        fresnel_mix: 0.5,
        ..uniforms_no_shift
    };
    let pixels_with_shift = render_frame(&ctx, &uniforms_with_shift, water, night, size, size);
    let lum_with_shift = avg_luminance_non_clear(&pixels_with_shift);

    // The diffuse color shift mixes toward a brighter sky color,
    // so the average luminance should increase.
    assert!(
        lum_with_shift > lum_no_shift,
        "Fresnel diffuse shift should brighten water: with_shift={lum_with_shift:.1}, \
         no_shift={lum_no_shift:.1}"
    );
}

#[test]
fn fresnel_diffuse_shift_absent_on_land() {
    let ctx = render_ctx();
    let size = 128;

    // All-land texture (alpha=255)
    let land = [50, 120, 50, 255];
    let night = [5, 5, 10, 255];

    // Without Fresnel diffuse shift
    let uniforms_base = Uniforms {
        terminator_width: 0.15,
        flags: 1,
        ..default_test_uniforms(size)
    };
    let pixels_base = render_frame(&ctx, &uniforms_base, land, night, size, size);

    // With Fresnel diffuse shift — should have no effect on land
    let uniforms_with_shift = Uniforms {
        fresnel_mix: 0.5,
        ..uniforms_base
    };
    let pixels_with_shift = render_frame(&ctx, &uniforms_with_shift, land, night, size, size);

    // Pixel-for-pixel comparison: land should be completely unaffected
    assert_eq!(
        pixels_base, pixels_with_shift,
        "Land pixels should be identical with and without fresnel_mix"
    );
}

#[test]
fn fresnel_diffuse_shift_absent_at_night() {
    let ctx = render_ctx();
    let size = 128;

    let water = [10, 30, 60, 128];
    let night = [5, 5, 10, 128];

    // Sun pointing away from camera (night side faces camera)
    let uniforms_base = Uniforms {
        sun_dir: [0.0, 0.0, -1.0],
        terminator_width: 0.15,
        flags: 1,
        ..default_test_uniforms(size)
    };
    let pixels_base = render_frame(&ctx, &uniforms_base, water, night, size, size);

    // With Fresnel diffuse shift — should have no effect on night side
    // because sky_color * result.blend = sky_color * 0 = 0
    let uniforms_with_shift = Uniforms {
        fresnel_mix: 0.5,
        ..uniforms_base
    };
    let pixels_with_shift = render_frame(&ctx, &uniforms_with_shift, water, night, size, size);

    // Night side: result.blend is 0, so sky_color * 0 = black.
    // The mix should be toward black which shouldn't change the dark night pixels.
    // Allow for minor floating-point differences near the terminator.
    let lum_base = avg_luminance_non_clear(&pixels_base);
    let lum_with_shift = avg_luminance_non_clear(&pixels_with_shift);

    let diff = (lum_with_shift - lum_base).abs();
    assert!(
        diff < 2.0,
        "Night-side water should be nearly identical with and without fresnel_mix: \
         base={lum_base:.1}, shift={lum_with_shift:.1}, diff={diff:.1}"
    );
}

// ---------------------------------------------------------------------------
// The shells over the globe
// ---------------------------------------------------------------------------

/// Bits 3 and 4 of `Uniforms::flags`, as `render_pass.rs` names them: the
/// shells lit as day, or as night, everywhere.
const FLAG_UNIFORM_DAY: u32 = 8;
const FLAG_UNIFORM_NIGHT: u32 = 16;

/// One of the four shells drawn over the globe, with the blend production
/// gives it.
#[derive(Clone, Copy, Debug)]
enum Shell {
    Cloud,
    Rayleigh,
    NightglowOrange,
    NightglowGreen,
}

impl Shell {
    fn entry_points(self) -> (&'static str, &'static str) {
        match self {
            Self::Cloud => ("vs_cloud", "fs_cloud"),
            Self::Rayleigh => ("vs_rayleigh", "fs_rayleigh"),
            Self::NightglowOrange => ("vs_nightglow_orange", "fs_nightglow_orange"),
            Self::NightglowGreen => ("vs_nightglow_green", "fs_nightglow_green"),
        }
    }

    fn blend(self) -> wgpu::BlendState {
        let over = |dst_factor| wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        };
        match self {
            Self::Cloud => wgpu::BlendState::ALPHA_BLENDING,
            Self::Rayleigh => over(wgpu::BlendFactor::OneMinusSrcAlpha),
            Self::NightglowOrange | Self::NightglowGreen => over(wgpu::BlendFactor::One),
        }
    }
}

fn shell_pipeline(ctx: &RenderContext, shell: Shell) -> wgpu::RenderPipeline {
    let (vs_entry, fs_entry) = shell.entry_points();
    let layout = ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("test_shell_pipeline_layout"),
            bind_group_layouts: &[&ctx.bind_group_layout],
            immediate_size: 0,
        });
    ctx.device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("test_shell_pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &ctx.shader,
                entry_point: Some(vs_entry),
                buffers: &[Vertex::buffer_layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &ctx.shader,
                entry_point: Some(fs_entry),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(shell.blend()),
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
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        })
}

/// Draw `shell` alone over the clear color, with a solid white cloud map, and
/// return the RGBA8 pixel data.
fn render_shell(ctx: &RenderContext, shell: Shell, uniforms: &Uniforms, size: u32) -> Vec<u8> {
    let pipeline = shell_pipeline(ctx, shell);
    let cloud_tex = create_solid_texture(&ctx.device, &ctx.queue, [255, 255, 255, 255]);
    let dummy_cube = &ctx.dummy_cube;
    let bind_group = test_bind_group(
        ctx,
        &cloud_tex,
        &ctx.sampler,
        [dummy_cube, dummy_cube, dummy_cube],
    );
    ctx.queue
        .write_buffer(&ctx.uniform_buffer, 0, bytemuck::cast_slice(&[*uniforms]));

    let extent = wgpu::Extent3d {
        width: size,
        height: size,
        depth_or_array_layers: 1,
    };
    let render_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test_shell_render_target"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test_shell_depth"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let color_view = render_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("test_shell_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.set_vertex_buffer(0, ctx.vertex_buffer.slice(..));
        pass.set_index_buffer(ctx.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..ctx.index_count, 0, 0..1);
    }
    ctx.queue.submit(std::iter::once(encoder.finish()));

    common::read_texture_rgba8(&ctx.device, &ctx.queue, &render_texture, size, size)
        .expect("reading the render target back")
}

#[test]
fn cloud_pipeline_renders_with_alpha() {
    let ctx = render_ctx();
    let size = 64;
    let uniforms = Uniforms {
        terminator_width: 0.15,
        cloud_opacity: 1.0,
        ..default_test_uniforms(size)
    };

    let pixels = render_shell(&ctx, Shell::Cloud, &uniforms, size);
    let visible = count_non_clear_pixels(&pixels);

    assert!(
        visible > 50,
        "Cloud pipeline should render visible pixels, got {visible}"
    );
}

/// The frame size of the shell lighting cases.
const SHELL_SIZE: u32 = 128;

/// Uniforms that put the whole globe in the frame, with every shell's
/// strength up and its rim widened, so each shell draws a hemisphere's worth
/// of pixels rather than a ring a pixel wide at the limb.
fn shell_case_uniforms(sun_dir: [f32; 3], flags: u32) -> Uniforms {
    let eye = glam::Vec3::new(0.0, 0.0, 8.0);
    Uniforms {
        mvp: test_mvp_with_eye(SHELL_SIZE, SHELL_SIZE, eye),
        eye_pos: eye.into(),
        sun_dir,
        flags,
        cloud_opacity: 1.0,
        rayleigh_intensity: 1.0,
        rayleigh_sharpness: 3.0,
        nightglow_intensity: 1.0,
        nightglow_falloff: 1.0,
        nightglow_balance: 0.5,
        ..default_test_uniforms(SHELL_SIZE)
    }
}

/// Mean of the three color channels over the left half of the frame and over
/// the right half. The camera looks down -z with +x to its right, so a Sun
/// along +x lights the right half.
#[allow(clippy::cast_precision_loss)]
fn half_means(pixels: &[u8], size: u32) -> (f64, f64) {
    let mut sums = [0u64; 2];
    for (index, px) in pixels.chunks(4).enumerate() {
        let x = u32::try_from(index).expect("a small frame") % size;
        let half = usize::from(x >= size / 2);
        sums[half] += u64::from(px[0]) + u64::from(px[1]) + u64::from(px[2]);
    }
    let count = f64::from(size / 2 * size * 3);
    (sums[0] as f64 / count, sums[1] as f64 / count)
}

const SUN_RIGHT: [f32; 3] = [1.0, 0.0, 0.0];
const SUN_LEFT: [f32; 3] = [-1.0, 0.0, 0.0];

/// How each shell differs between the hemispheres under the Sun, and what it
/// does in the two even modes.
///
/// In Day and Night the frame may not depend on where the Sun is at all, so
/// moving it to the other side changes no pixel, and the two halves of the
/// frame read alike. Under the Sun the same shell differs between the halves,
/// which is what says the even modes changed something. Day then has to read
/// nearer what the Sun gives the day half than what it gives the night half,
/// and Night the reverse. Not equal to it: the Sun's halves average a ramp
/// across the terminator, and the green nightglow is brightest at midnight,
/// which Night puts everywhere.
#[test]
fn the_even_modes_light_every_shell_the_same_on_both_hemispheres() {
    let ctx = render_ctx();
    for shell in [
        Shell::Cloud,
        Shell::Rayleigh,
        Shell::NightglowOrange,
        Shell::NightglowGreen,
    ] {
        let render = |sun_dir, flags| {
            render_shell(
                &ctx,
                shell,
                &shell_case_uniforms(sun_dir, flags),
                SHELL_SIZE,
            )
        };

        let (night_half, day_half) = half_means(&render(SUN_RIGHT, 0), SHELL_SIZE);
        let contrast = (day_half - night_half).abs();
        println!(
            "{shell:?} under the Sun: {night_half:.2} on the night half, {day_half:.2} on the day half"
        );
        assert!(
            contrast > 2.0,
            "{shell:?} reads {night_half:.2} on the night half and {day_half:.2} on the day \
             half under the Sun, so this case cannot tell the even modes from it"
        );

        let mut even = Vec::new();
        for (flags, mode) in [(FLAG_UNIFORM_DAY, "Day"), (FLAG_UNIFORM_NIGHT, "Night")] {
            let frame = render(SUN_RIGHT, flags);
            assert!(
                frame == render(SUN_LEFT, flags),
                "{shell:?} in {mode} changes when the Sun moves to the other side"
            );
            let (left, right) = half_means(&frame, SHELL_SIZE);
            println!("{shell:?} in {mode}: {left:.2} on the left half, {right:.2} on the right");
            assert!(
                (left - right).abs() < contrast * 0.05,
                "{shell:?} in {mode} reads {left:.2} on one half and {right:.2} on the other"
            );
            even.push(left);
        }

        let (day, night) = (even[0], even[1]);
        assert!(
            (day - day_half).abs() < (day - night_half).abs(),
            "{shell:?} in Day reads {day:.2} on each half, nearer the {night_half:.2} the \
             Sun gives the night half than the {day_half:.2} it gives the day half"
        );
        assert!(
            (night - night_half).abs() < (night - day_half).abs(),
            "{shell:?} in Night reads {night:.2} on each half, nearer the {day_half:.2} the \
             Sun gives the day half than the {night_half:.2} it gives the night half"
        );
    }
}

/// The Rayleigh shell's forward-scattering lobe is a terminator effect, so
/// neither even mode draws it, even where the Sun lighting would.
///
/// The lobe's gate reads the Sun's world direction and its angle reads the
/// Sun's view direction, two uniforms a case can set apart: lighting the
/// whole front of the shell opens the gate over all of it, and a view
/// direction straight ahead puts the lobe's peak in the middle of the frame.
/// Day lights every point as noon, which the gate alone would let through.
#[test]
fn the_even_modes_draw_no_sunrise_lobe() {
    let ctx = render_ctx();
    let render = |flags, glow| {
        let uniforms = Uniforms {
            sun_view_dir: [0.0, 0.0, -1.0],
            atmo_sunrise_glow: glow,
            ..shell_case_uniforms([0.0, 0.0, 1.0], flags)
        };
        render_shell(&ctx, Shell::Rayleigh, &uniforms, SHELL_SIZE)
    };

    let changed = |flags| {
        let without = render(flags, 0.0);
        let with = render(flags, 4.0);
        without
            .iter()
            .zip(&with)
            .filter(|(a, b)| a.abs_diff(**b) > 2)
            .count()
    };

    let under_the_sun = changed(0);
    assert!(
        under_the_sun > 100,
        "the lobe changes only {under_the_sun} channel values under the Sun, so this \
         camera does not show it"
    );
    for (flags, mode) in [(FLAG_UNIFORM_DAY, "Day"), (FLAG_UNIFORM_NIGHT, "Night")] {
        let lobe = changed(flags);
        assert_eq!(lobe, 0, "{mode} draws the lobe into {lobe} channel values");
    }
}

// ---------------------------------------------------------------------------
// Color correction: gamma tests
// ---------------------------------------------------------------------------

#[test]
fn gamma_moves_midtones_in_both_directions() {
    let ctx = render_ctx();
    let size = 128;

    let mid_gray = [128, 128, 128, 255];
    let black = [0, 0, 0, 255];

    let uniforms_base = default_test_uniforms(size);
    let pixels_base = render_frame(&ctx, &uniforms_base, mid_gray, black, size, size);
    let lum_base = avg_luminance_non_clear(&pixels_base);

    let uniforms_bright = Uniforms {
        day_gamma: 2.0,
        ..uniforms_base
    };
    let pixels_bright = render_frame(&ctx, &uniforms_bright, mid_gray, black, size, size);
    let lum_bright = avg_luminance_non_clear(&pixels_bright);

    let uniforms_dark = Uniforms {
        day_gamma: 0.5,
        ..uniforms_base
    };
    let pixels_dark = render_frame(&ctx, &uniforms_dark, mid_gray, black, size, size);
    let lum_dark = avg_luminance_non_clear(&pixels_dark);

    assert!(
        lum_bright > lum_base,
        "Gamma > 1.0 should brighten midtones: gamma_2.0={lum_bright:.1}, gamma_1.0={lum_base:.1}"
    );
    assert!(
        lum_dark < lum_base,
        "Gamma < 1.0 should darken midtones: gamma_0.5={lum_dark:.1}, gamma_1.0={lum_base:.1}"
    );
}

#[test]
fn consecutive_renders_are_identical() {
    let ctx = render_ctx();
    let size = 128;

    let colorful = [200, 100, 50, 255];
    let black = [0, 0, 0, 255];

    let uniforms = default_test_uniforms(size);

    let pixels_a = render_frame(&ctx, &uniforms, colorful, black, size, size);
    let pixels_b = render_frame(&ctx, &uniforms, colorful, black, size, size);

    assert_eq!(
        pixels_a, pixels_b,
        "the same uniforms and the same textures should give the same pixels, \
         which is what every pixel-equality case in this file rests on"
    );
}

// ---------------------------------------------------------------------------
// Color correction: saturation tests
// ---------------------------------------------------------------------------

#[test]
fn saturation_zero_produces_greyscale() {
    let ctx = render_ctx();
    let size = 128;

    let red = [255, 0, 0, 255];
    let black = [0, 0, 0, 255];

    let uniforms = Uniforms {
        day_saturation: 0.0,
        ..default_test_uniforms(size)
    };

    let pixels = render_frame(&ctx, &uniforms, red, black, size, size);

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let clear_r = (CLEAR_COLOR.r * 255.0) as u8;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let clear_g = (CLEAR_COLOR.g * 255.0) as u8;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let clear_b = (CLEAR_COLOR.b * 255.0) as u8;

    let mut non_grey_count = 0;
    for px in pixels.chunks(4) {
        let dr = px[0].abs_diff(clear_r);
        let dg = px[1].abs_diff(clear_g);
        let db = px[2].abs_diff(clear_b);
        if dr > 1 || dg > 1 || db > 1 {
            let max_ch = px[0].max(px[1]).max(px[2]);
            let min_ch = px[0].min(px[1]).min(px[2]);
            if max_ch - min_ch > 2 {
                non_grey_count += 1;
            }
        }
    }

    assert_eq!(
        non_grey_count, 0,
        "Saturation 0.0 should produce greyscale: found {non_grey_count} non-grey pixels"
    );
}

#[test]
fn saturation_above_one_increases_chroma() {
    let ctx = render_ctx();
    let size = 128;

    let colorful = [200, 100, 50, 255];
    let black = [0, 0, 0, 255];

    let uniforms_base = default_test_uniforms(size);
    let pixels_base = render_frame(&ctx, &uniforms_base, colorful, black, size, size);

    let uniforms_saturated = Uniforms {
        day_saturation: 2.0,
        ..uniforms_base
    };
    let pixels_saturated = render_frame(&ctx, &uniforms_saturated, colorful, black, size, size);

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let clear_r = (CLEAR_COLOR.r * 255.0) as u8;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let clear_g = (CLEAR_COLOR.g * 255.0) as u8;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let clear_b = (CLEAR_COLOR.b * 255.0) as u8;

    let avg_chroma = |pixels: &[u8]| -> f64 {
        let mut sum = 0.0_f64;
        let mut count = 0u64;
        for px in pixels.chunks(4) {
            let dr = px[0].abs_diff(clear_r);
            let dg = px[1].abs_diff(clear_g);
            let db = px[2].abs_diff(clear_b);
            if dr > 1 || dg > 1 || db > 1 {
                let max_ch = px[0].max(px[1]).max(px[2]);
                let min_ch = px[0].min(px[1]).min(px[2]);
                sum += f64::from(max_ch - min_ch);
                count += 1;
            }
        }
        if count == 0 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        {
            sum / count as f64
        }
    };

    let chroma_base = avg_chroma(&pixels_base);
    let chroma_saturated = avg_chroma(&pixels_saturated);

    assert!(
        chroma_saturated > chroma_base,
        "Saturation > 1.0 should increase chroma: sat_2.0={chroma_saturated:.1}, sat_1.0={chroma_base:.1}"
    );
}

// ---------------------------------------------------------------------------
// Color correction: independence tests
// ---------------------------------------------------------------------------

#[test]
fn day_and_night_corrections_independent() {
    let ctx = render_ctx();
    let size = 128;

    // Use mid-tones so gamma correction produces a visible difference
    // (pure white and pure black are fixed points of pow)
    let mid_gray = [128, 128, 128, 255];
    let dark_gray = [64, 64, 64, 255];

    let uniforms_day_bright = Uniforms {
        terminator_width: 0.15,
        flags: 1,
        day_gamma: 2.0,
        ..default_test_uniforms(size)
    };
    let pixels_day_bright =
        render_frame(&ctx, &uniforms_day_bright, mid_gray, dark_gray, size, size);

    let uniforms_night_bright = Uniforms {
        day_gamma: 1.0,
        night_gamma: 2.0,
        ..uniforms_day_bright
    };
    let pixels_night_bright = render_frame(
        &ctx,
        &uniforms_night_bright,
        mid_gray,
        dark_gray,
        size,
        size,
    );

    assert_ne!(
        pixels_day_bright, pixels_night_bright,
        "Day and night corrections should produce different output when targeting different textures"
    );
}

// ---------------------------------------------------------------------------
// The cube surface
// ---------------------------------------------------------------------------

/// The six axes in cube layer order, +X, -X, +Y, -Y, +Z, -Z.
const AXES: [glam::Vec3; 6] = [
    glam::Vec3::X,
    glam::Vec3::NEG_X,
    glam::Vec3::Y,
    glam::Vec3::NEG_Y,
    glam::Vec3::Z,
    glam::Vec3::NEG_Z,
];

/// Uniforms for a square frame looking at the globe's center from along
/// `axis`, the day surface alone.
fn looking_along(size: u32, axis: glam::Vec3) -> Uniforms {
    looking_along_from(size, axis, 3.5)
}

/// [`looking_along`] from `distance` radii away.
fn looking_along_from(size: u32, axis: glam::Vec3, distance: f32) -> Uniforms {
    let eye = axis * distance;
    let up = if axis.y.abs() > 0.5 {
        glam::Vec3::Z
    } else {
        glam::Vec3::Y
    };
    let view = glam::Mat4::look_at_rh(eye, glam::Vec3::ZERO, up);
    let proj = glam::Mat4::perspective_rh(20.0_f32.to_radians(), 1.0, 0.1, 100.0);
    Uniforms {
        mvp: (proj * view).to_cols_array(),
        eye_pos: eye.into(),
        ..default_test_uniforms(size)
    }
}

/// The RGB of the pixel at the middle of a square frame.
fn middle_pixel(pixels: &[u8], size: u32) -> [u8; 3] {
    let at = (((size / 2) * size + size / 2) * 4) as usize;
    [pixels[at], pixels[at + 1], pixels[at + 2]]
}

/// Whether two colors agree within `tolerance` on every channel.
fn close(a: [u8; 3], b: [u8; 3], tolerance: u8) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= tolerance)
}

/// The globe read through the warped direction shows each face where the
/// camera looks along that face's axis, which is what the shader and the
/// hardware's cube table agree on.
#[test]
fn the_cube_path_shows_each_face_where_its_axis_points() {
    let ctx = render_ctx();
    let size = 64;
    let colors: [[u8; 4]; 6] = [
        [220, 40, 40, 255],
        [40, 220, 40, 255],
        [40, 40, 220, 255],
        [220, 220, 40, 255],
        [220, 40, 220, 255],
        [40, 220, 220, 255],
    ];
    // Four texels a face rather than one: the middle pixel sits half a pixel
    // off the face's center, where a one-texel face's filter already reaches
    // across the edges into its neighbors, three steps' worth on WARP.
    let day = create_cube(&ctx.device, &ctx.queue, 4, |face| colors[face].repeat(16));
    let dummy = &ctx.dummy_cube;
    for (face, axis) in AXES.into_iter().enumerate() {
        let pixels = render_cube_frame(
            &ctx,
            &looking_along(size, axis),
            [&day, dummy, dummy],
            size,
            size,
        );
        let [r, g, b, _] = colors[face];
        let seen = middle_pixel(&pixels, size);
        assert!(
            close(seen, [r, g, b], 2),
            "looking along {axis:?}: {seen:?}, the face holds {:?}",
            colors[face]
        );
    }
}

/// The water cube, not the day surface's alpha, is what the water effects
/// read on the cube path: the same day and night with open water everywhere
/// are brighter at the grazing edge than with land everywhere.
#[test]
fn the_water_cube_drives_the_water_effects() {
    let ctx = render_ctx();
    let size = 128;
    let day = create_solid_cube(&ctx.device, &ctx.queue, [[10, 30, 60, 255]; 6]);
    let night = create_solid_cube(&ctx.device, &ctx.queue, [[5, 5, 10, 255]; 6]);
    let open_water = create_solid_cube(&ctx.device, &ctx.queue, [[255, 255, 255, 255]; 6]);
    let land = create_solid_cube(&ctx.device, &ctx.queue, [[0, 0, 0, 255]; 6]);
    let uniforms = Uniforms {
        terminator_width: 0.15,
        flags: 1,
        fresnel_mix: 0.5,
        ..default_test_uniforms(size)
    };

    let over_water = avg_luminance_non_clear(&render_cube_frame(
        &ctx,
        &uniforms,
        [&day, &night, &open_water],
        size,
        size,
    ));
    let over_land = avg_luminance_non_clear(&render_cube_frame(
        &ctx,
        &uniforms,
        [&day, &night, &land],
        size,
        size,
    ));
    assert!(
        over_water > over_land + 1.0,
        "open water {over_water:.1} should read brighter than land {over_land:.1}"
    );
}

/// A day pack's floor, uploaded face by face in the pack's order, shows each
/// face's own texels where that face's axis points: the order the pack keeps
/// its faces in is the cube's layer order.
#[test]
fn a_packs_floor_faces_land_where_their_axes_point() {
    use std::sync::atomic::AtomicBool;

    use sunlit_core::assets::cube_layout::CubeTextures;
    use sunlit_core::assets::tiles::{self, PackKind};

    let scratch = common::test_support::ScratchDir::new("render_pipeline_floor");
    common::test_support::write_cube_fixture(&scratch.join("textures"));
    let textures = CubeTextures::resolve(&scratch.join("textures"));
    let cache = scratch.join("cache");
    tiles::ensure_pack(
        &cache,
        PackKind::Day(0),
        &textures,
        &tiles::FIXTURE,
        &AtomicBool::new(false),
    )
    .expect("build the fixture's January pack");
    let pack = tiles::Pack::open(&tiles::pack_path(&cache, PackKind::Day(0))).expect("open it");

    let faces: Vec<_> = pack.entries().iter().filter(|e| e.whole_face).collect();
    assert_eq!(faces.len(), 6, "a floor has six faces");
    let floor = 1_u32 << faces[0].key.level;
    let decoded: Vec<Vec<u8>> = faces
        .iter()
        .map(|entry| {
            let blob = pack.read(entry).expect("read a floor face");
            let finest = pack.mips(entry)[0];
            tiles::decode_bc7(
                &blob[finest.offset..finest.offset + finest.len],
                floor,
                floor,
            )
            .expect("decode a floor face")
        })
        .collect();
    for (face, entry) in faces.iter().enumerate() {
        assert_eq!(
            usize::from(entry.key.face),
            face,
            "the pack keeps its faces in order"
        );
    }

    let ctx = render_ctx();
    let size = 64;
    let cube = create_cube(&ctx.device, &ctx.queue, floor, |face| decoded[face].clone());
    let dummy = &ctx.dummy_cube;
    for (face, axis) in AXES.into_iter().enumerate() {
        let pixels = render_cube_frame(
            &ctx,
            &looking_along(size, axis),
            [&cube, dummy, dummy],
            size,
            size,
        );
        // The middle of a face is the corner its four middle texels share, so
        // a bilinear read there is their mean.
        let texel = |row: u32, col: u32, channel: usize| {
            u32::from(decoded[face][((row * floor + col) * 4) as usize + channel])
        };
        let (lo, hi) = (floor / 2 - 1, floor / 2);
        let expected: [u8; 3] = std::array::from_fn(|channel| {
            let sum = texel(lo, lo, channel)
                + texel(lo, hi, channel)
                + texel(hi, lo, channel)
                + texel(hi, hi, channel);
            u8::try_from((sum + 2) / 4).expect("a mean of bytes")
        });
        let seen = middle_pixel(&pixels, size);
        assert!(
            close(seen, expected, 3),
            "face {face}: {seen:?} where the floor's middle is {expected:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The warped direction across the face edges and through the poles
// ---------------------------------------------------------------------------

/// Texels along a face of the direction-coded cube.
const CODED_FACE: u32 = 32;

/// The color the direction-coded cube stands for at a direction: each
/// component of the unit direction taken from -1 to 1 onto 0 to 1.
fn coded_color(direction: glam::DVec3) -> glam::DVec3 {
    direction.normalize() * 0.5 + 0.5
}

/// The direction a coded color stands for, not normalized.
fn coded_direction(color: glam::DVec3) -> glam::DVec3 {
    color * 2.0 - 1.0
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a coded color is inside 0 to 1"
)]
fn to_byte(value: f64) -> u8 {
    (value * 255.0).round() as u8
}

/// One face of the direction-coded cube: every texel holds the coded color of
/// the direction through its center, placed by the geometry the bake and the
/// tile cutter place texels with.
fn coded_face(face: usize, size: u32) -> Vec<u8> {
    use sunlit_core::geometry::cube;
    let mut texels = Vec::with_capacity((size * size * 4) as usize);
    for row in 0..i64::from(size) {
        for col in 0..i64::from(size) {
            let direction = cube::direction(
                face,
                cube::texel_center(col, size),
                cube::texel_center(row, size),
            );
            let color = coded_color(direction);
            texels.extend([to_byte(color.x), to_byte(color.y), to_byte(color.z), 255]);
        }
    }
    texels
}

/// The face a warped direction selects, in cube layer order: its largest
/// component, and that component's sign.
fn face_of(w: glam::DVec3) -> usize {
    let a = w.abs();
    let (axis, value) = if a.x >= a.y && a.x >= a.z {
        (0, w.x)
    } else if a.y >= a.z {
        (1, w.y)
    } else {
        (2, w.z)
    };
    2 * axis + usize::from(value < 0.0)
}

/// The face whose center is `axis` scaled by `sign`.
fn face_along(axis: usize, sign: f64) -> usize {
    2 * axis + usize::from(sign < 0.0)
}

/// How much a component of the warped direction may change per radian the
/// normal turns. `atan(n / m) * 4 / pi` with `m` the largest component, which
/// is at least `1 / sqrt(3)`, changes by at most `8 sqrt(3) / pi`, about 4.4.
const WARP_LIPSCHITZ: f64 = 4.5;

/// A compute entry point appended to the production shaders: the warped
/// direction `equi_angular` gives each normal, and what the day cube holds
/// there through the globe's sampler at the finest level.
const CUBE_PROBE: &str = "
@group(1) @binding(0) var<storage, read> probe_normals: array<vec4<f32>>;
@group(1) @binding(1) var<storage, read_write> probe_results: array<vec4<f32>>;

@compute @workgroup_size(64)
fn cube_probe(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    if index >= arrayLength(&probe_normals) {
        return;
    }
    let w = equi_angular(normalize(probe_normals[index].xyz));
    probe_results[2u * index] = vec4<f32>(w, 0.0);
    probe_results[2u * index + 1u] = textureSampleLevel(day_cube, sphere_sampler, w, 0.0);
}
";

/// What the probe found at one normal.
#[derive(Clone, Copy)]
struct Probed {
    normal: glam::DVec3,
    warped: glam::DVec3,
    color: glam::DVec3,
}

/// Run the probe over `normals`, reading `cube` through the surface sampler.
fn probe_cube(
    ctx: &RenderContext,
    cube: &wgpu::TextureView,
    normals: &[glam::DVec3],
) -> Vec<Probed> {
    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cube_probe_shader"),
            source: wgpu::ShaderSource::Wgsl(production_shaders(CUBE_PROBE).into()),
        });
    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("cube_probe_pipeline"),
            layout: None,
            module: &shader,
            entry_point: Some("cube_probe"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a direction handed to the GPU in single precision"
    )]
    let input: Vec<[f32; 4]> = normals
        .iter()
        .map(|n| [n.x as f32, n.y as f32, n.z as f32, 0.0])
        .collect();
    let output_size = 2 * std::mem::size_of_val(input.as_slice()) as u64;
    let input_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cube_probe_input"),
            contents: bytemuck::cast_slice(&input),
            usage: wgpu::BufferUsages::STORAGE,
        });
    let output_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("cube_probe_output"),
        size: output_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let surface_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&ctx.surface_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(cube),
            },
        ],
    });
    let storage_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(1),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: input_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output_buf.as_entire_binding(),
            },
        ],
    });
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: None,
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &surface_group, &[]);
        pass.set_bind_group(1, &storage_group, &[]);
        let count = u32::try_from(input.len()).expect("a probe of a few thousand normals");
        pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
    }
    ctx.queue.submit(std::iter::once(encoder.finish()));
    let data = common::read_buffer(&ctx.device, &ctx.queue, &output_buf, output_size);
    let results = bytemuck::cast_slice::<u8, [f32; 4]>(&data);
    let vector = |v: [f32; 4]| glam::Vec3::from_slice(&v[..3]).as_dvec3();
    normals
        .iter()
        .zip(results.chunks_exact(2))
        .map(|(&normal, pair)| Probed {
            normal: normal.normalize(),
            warped: vector(pair[0]),
            color: vector(pair[1]),
        })
        .collect()
}

/// The largest per-channel difference between two colors, in steps of 255.
fn color_steps(a: glam::DVec3, b: glam::DVec3) -> f64 {
    (a - b).abs().max_element() * 255.0
}

/// The worst of what the probe measured, printed so a CI log carries it.
#[derive(Default)]
struct ProbeWorst {
    /// The largest change of a warped component per radian of the normal.
    warp_rate: f64,
    /// The largest color jump between two neighboring normals, in steps.
    jump: f64,
    /// The largest distance from the coded color of the normal, in steps.
    placement: f64,
}

/// The largest color jump two neighboring probe normals may show, in steps of
/// 255: the coded color turns by half the angle between them, a fraction of a
/// step at the spacings used here, and the filter adds its rounding. A face
/// sampled without its neighbor's texels across an edge jumps by a whole texel,
/// six steps at this cube's size; a mirrored or rotated face by far more.
const PROBE_JUMP: f64 = 1.5;

/// How far a sample may sit from the coded color of its normal, in steps of
/// 255. Inside a face that is the texels' rounding, half a step; where the
/// filter's footprint straddles an edge the adapters read up to a step and a
/// third (`docs/testing.md`). A face read through the plain cube coordinates
/// instead of the warp is several steps off away from its center and edges.
const PROBE_PLACEMENT: f64 = 2.0;

impl ProbeWorst {
    /// Check two samples a small turn apart: both where the geometry says they
    /// are, and nothing between them that the turn does not explain.
    fn neighbors(&mut self, a: &Probed, b: &Probed, context: &str) {
        let angle = a.normal.angle_between(b.normal);
        let warp = (a.warped - b.warped).abs().max_element();
        self.warp_rate = self.warp_rate.max(warp / angle);
        assert!(
            warp <= WARP_LIPSCHITZ * angle + 1.0e-5,
            "{context}: the warped direction moves {warp:.3e} over a turn of {angle:.3e} rad, \
             from {:?} to {:?}",
            a.warped,
            b.warped
        );
        let jump = color_steps(a.color, b.color);
        self.jump = self.jump.max(jump);
        assert!(
            jump <= PROBE_JUMP,
            "{context}: the sampled color jumps {jump:.2} steps over a turn of {angle:.3e} rad, \
             from {:?} to {:?}",
            a.color * 255.0,
            b.color * 255.0
        );
        self.placed(a, context);
        self.placed(b, context);
    }

    fn placed(&mut self, sample: &Probed, context: &str) {
        let off = color_steps(sample.color, coded_color(sample.normal));
        self.placement = self.placement.max(off);
        assert!(
            off <= PROBE_PLACEMENT,
            "{context}: the cube holds {:?} at {:?}, {off:.2} steps from its coded color {:?}",
            sample.color * 255.0,
            sample.normal,
            coded_color(sample.normal) * 255.0
        );
    }

    fn report(&self, what: &str) {
        println!(
            "{what}: warp rate up to {:.3} per rad, jumps up to {:.3} steps, placement within {:.3} steps",
            self.warp_rate, self.jump, self.placement
        );
    }
}

/// The unit vector along axis `index`, X 0, Y 1, Z 2.
fn unit(index: usize) -> glam::DVec3 {
    glam::DVec3::AXES[index]
}

/// How far apart the two normals of a probe pair are, as a fraction of the
/// edge point's distance from the center.
const STRADDLE: f64 = 5.0e-4;

/// Points along each edge the probe crosses it at.
const EDGE_STEPS: u32 = 64;

/// The warped direction, and the cube read through it, are continuous across
/// every one of the twelve face edges and the eight corners.
///
/// Each edge is crossed at 65 points from one corner to the other by a pair of
/// normals a milliradian apart, one on either face, and each corner by three,
/// one on each face that meets there. Every sample selects the face the
/// geometry says it lies on, the warped direction moves no more than its turn
/// explains, and the direction-coded cube, read at its finest level, shows the
/// color of the normal itself, on both sides of every edge. A face mirrored or
/// turned, a face in another's layer, or an edge sampled without the texels
/// across it breaks one of those at some edge.
#[test]
fn the_warped_direction_is_continuous_across_every_face_edge() {
    let ctx = render_ctx();
    let cube = create_cube(&ctx.device, &ctx.queue, CODED_FACE, |face| {
        coded_face(face, CODED_FACE)
    });

    let mut normals = Vec::new();
    let mut faces = Vec::new();
    let mut edges = 0;
    for (a, b, c) in [(0, 1, 2), (0, 2, 1), (1, 2, 0)] {
        for sa in [1.0, -1.0] {
            for sb in [1.0, -1.0] {
                edges += 1;
                let (on_a, on_b) = (unit(a) * sa, unit(b) * sb);
                let across = (on_a - on_b) * STRADDLE;
                for step in 0..=EDGE_STEPS {
                    let along = -0.98 + 1.96 * f64::from(step) / f64::from(EDGE_STEPS);
                    let point = on_a + on_b + unit(c) * along;
                    normals.extend([point + across, point - across]);
                    faces.extend([face_along(a, sa), face_along(b, sb)]);
                }
            }
        }
    }
    let edge_samples = normals.len();
    for corner in 0..8_u32 {
        let signs = [0, 1, 2].map(|bit| if corner >> bit & 1 == 0 { 1.0 } else { -1.0 });
        let point = glam::DVec3::from_array(signs);
        for (axis, sign) in signs.into_iter().enumerate() {
            normals.push(point + unit(axis) * sign * STRADDLE);
            faces.push(face_along(axis, sign));
        }
    }
    assert_eq!(edges, 12, "a cube has twelve edges");

    let probed = probe_cube(&ctx, &cube, &normals);
    for (sample, &face) in probed.iter().zip(&faces) {
        assert_eq!(
            face_of(sample.warped),
            face,
            "{:?} warps to {:?}, which is not on face {face}",
            sample.normal,
            sample.warped
        );
    }
    let mut worst = ProbeWorst::default();
    for pair in probed[..edge_samples].chunks_exact(2) {
        worst.neighbors(&pair[0], &pair[1], "across an edge");
    }
    for corner in probed[edge_samples..].chunks_exact(3) {
        for (i, j) in [(0, 1), (1, 2), (0, 2)] {
            worst.neighbors(&corner[i], &corner[j], "around a corner");
        }
    }
    worst.report("face edges and corners");
}

/// Samples along each arc through a pole, either side of it.
const POLE_STEPS: i32 = 150;

/// The turn between two neighboring samples on an arc through a pole.
const POLE_STEP_DEGREES: f64 = 0.1;

/// The poles are ordinary points of the surface: the centers of the +Y and -Y
/// faces, with nothing there that is not everywhere else.
///
/// The probe crosses each pole along four great circles, 15 degrees either
/// side of it in steps of a tenth of a degree, and every neighboring pair is
/// held to what `the_warped_direction_is_continuous_across_every_face_edge`
/// holds an edge crossing to. The globe is then drawn looking down on each
/// pole through the production shader, which is where a singularity would
/// show: in the screen derivatives of the warped direction, which choose the
/// level the sampler reads.
#[test]
fn the_poles_are_ordinary_points_of_the_cube() {
    let ctx = render_ctx();
    let cube = create_cube(&ctx.device, &ctx.queue, CODED_FACE, |face| {
        coded_face(face, CODED_FACE)
    });

    let mut normals = Vec::new();
    for pole in [glam::DVec3::Y, glam::DVec3::NEG_Y] {
        for longitude in [0.0_f64, 45.0, 90.0, 135.0] {
            let meridian = glam::DVec3::new(
                longitude.to_radians().sin(),
                0.0,
                longitude.to_radians().cos(),
            );
            for step in -POLE_STEPS..=POLE_STEPS {
                let turn = (f64::from(step) * POLE_STEP_DEGREES).to_radians();
                normals.push(pole * turn.cos() + meridian * turn.sin());
            }
        }
    }
    let probed = probe_cube(&ctx, &cube, &normals);
    let arc = usize::try_from(2 * POLE_STEPS + 1).expect("a short arc");
    let mut worst = ProbeWorst::default();
    for (index, samples) in probed.chunks_exact(arc).enumerate() {
        let face = if index < 4 { 2 } else { 3 };
        for sample in samples {
            assert_eq!(
                face_of(sample.warped),
                face,
                "{:?} is on the pole's face",
                sample.normal
            );
        }
        for pair in samples.windows(2) {
            worst.neighbors(&pair[0], &pair[1], "through a pole");
        }
    }
    worst.report("through the poles");

    let marked = create_marked_cube(&ctx.device, &ctx.queue, CODED_FACE, |face| {
        coded_face(face, CODED_FACE)
    });
    for axis in [glam::Vec3::Y, glam::Vec3::NEG_Y] {
        assert_no_seam_looking_along(&ctx, &marked, axis, 1);
    }
}

/// The color every level of the marked cube below the finest holds: mid grey,
/// which codes no direction at all, so a sample that reads any of it comes
/// out shorter than a unit direction by the share it read.
const MIP_MARKER: [u8; 4] = [128, 128, 128, 255];

/// The direction-coded cube with a full mip chain whose every level below the
/// finest is `MIP_MARKER`.
fn create_marked_cube(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: u32,
    texels: impl Fn(usize) -> Vec<u8>,
) -> wgpu::TextureView {
    create_cube_by_level(device, queue, size, |face, level, width| {
        if level == 0 {
            texels(face)
        } else {
            MIP_MARKER.repeat((width * width) as usize)
        }
    })
}

/// A cube `size` texels wide with a full mip chain, each level of each face
/// given as its texels by `texels(face, level, width)`.
fn create_cube_by_level(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: u32,
    texels: impl Fn(usize, u32, u32) -> Vec<u8>,
) -> wgpu::TextureView {
    let levels = size.ilog2() + 1;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("marked_cube"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for face in 0..6_u32 {
        for level in 0..levels {
            let width = size >> level;
            let data = texels(face as usize, level, width);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: face,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * width),
                    rows_per_image: Some(width),
                },
                wgpu::Extent3d {
                    width,
                    height: width,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::Cube),
        ..Default::default()
    })
}

/// The width of a frame the seam cases draw. The globe overfills it, and a
/// texel of the coded cube is several pixels across everywhere in it, so the
/// sampler reads the finest level wherever the screen derivatives are sound.
const SEAM_FRAME: u32 = 128;

/// How far a pixel's color may code a direction longer or shorter than a unit
/// one: the rounding of the texels and of the frame, a hundredth or so. A
/// pixel that read two percent of `MIP_MARKER` fails it.
const UNIT_TOLERANCE: f64 = 0.02;

/// The largest step between two neighboring pixels, in steps of 255: twice
/// what a sound frame shows at this framing (`docs/testing.md`), and a fraction
/// of what the colors either side of a mirrored or turned face's edge differ by.
const NEIGHBOR_STEPS: u8 = 4;

/// How far the direction a pixel's color codes may point from the surface
/// point under the pixel, in degrees: room for the rounding of the texels and
/// of the frame, and for the mesh's flat facets, across which the rasterizer
/// interpolates the normal the shader reads while the ray cast meets the
/// sphere itself. A face sampled through the plain cube coordinates instead of
/// the warp is four degrees off at 22 degrees from its center.
const PLACEMENT_DEGREES: f64 = 1.5;

/// The unit normal of the sphere where the ray through the middle of pixel
/// `(x, y)` of a square frame `size` wide first meets it, for a frame drawn
/// with the inverse of `inverse`.
fn normal_under_pixel(inverse: glam::DMat4, size: u32, x: u32, y: u32) -> Option<glam::DVec3> {
    let ndc_x = (f64::from(x) + 0.5) / f64::from(size) * 2.0 - 1.0;
    let ndc_y = 1.0 - (f64::from(y) + 0.5) / f64::from(size) * 2.0;
    let near = inverse.project_point3(glam::DVec3::new(ndc_x, ndc_y, 0.0));
    let far = inverse.project_point3(glam::DVec3::new(ndc_x, ndc_y, 1.0));
    let ray = (far - near).normalize();
    let half_b = near.dot(ray);
    let discriminant = half_b * half_b - (near.length_squared() - 1.0);
    (discriminant >= 0.0).then(|| (near + ray * (-half_b - discriminant.sqrt())).normalize())
}

/// Draw the globe from the marked cube looking along `axis` through the
/// production shader, and hold the frame to what a sound warp draws: every
/// pixel a unit direction's color read from the finest level and the
/// direction of the surface under it, no two neighbors apart by more than the
/// surface turns between them, and at least `faces` faces in the frame.
fn assert_no_seam_looking_along(
    ctx: &RenderContext,
    cube: &wgpu::TextureView,
    axis: glam::Vec3,
    faces: usize,
) {
    let dummy = &ctx.dummy_cube;
    assert_no_seam(axis, faces, |uniforms| {
        render_cube_frame(ctx, uniforms, [cube, dummy, dummy], SEAM_FRAME, SEAM_FRAME)
    });
}

/// [`assert_no_seam_looking_along`] for a frame `render` draws from the
/// uniforms looking along `axis`, and the surface normal under every pixel.
fn assert_no_seam(
    axis: glam::Vec3,
    faces: usize,
    render: impl Fn(&Uniforms) -> Vec<u8>,
) -> Vec<glam::DVec3> {
    let size = SEAM_FRAME;
    let uniforms = looking_along(size, axis);
    let inverse = glam::Mat4::from_cols_array(&uniforms.mvp)
        .as_dmat4()
        .inverse();
    let pixels = render(&uniforms);
    let color_at = |x: u32, y: u32| {
        let at = ((y * size + x) * 4) as usize;
        [pixels[at], pixels[at + 1], pixels[at + 2]]
    };
    let direction_at = |x: u32, y: u32| {
        coded_direction(glam::DVec3::from_array(
            color_at(x, y).map(|c| f64::from(c) / 255.0),
        ))
    };

    let mut seen = std::collections::BTreeSet::new();
    let mut worst_unit = 0.0_f64;
    let mut worst_placement = 0.0_f64;
    let mut normals = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            let direction = direction_at(x, y);
            let off = (direction.length() - 1.0).abs();
            worst_unit = worst_unit.max(off);
            assert!(
                off <= UNIT_TOLERANCE,
                "looking along {axis:?}, pixel ({x}, {y}) is {:?}, a direction {:.3} long: \
                 it read a coarser level than the finest",
                color_at(x, y),
                direction.length()
            );
            let surface = normal_under_pixel(inverse, size, x, y)
                .unwrap_or_else(|| panic!("the globe overfills the frame, but misses ({x}, {y})"));
            let apart = direction.angle_between(surface).to_degrees();
            worst_placement = worst_placement.max(apart);
            assert!(
                apart <= PLACEMENT_DEGREES,
                "looking along {axis:?}, pixel ({x}, {y}) shows {direction:?}, {apart:.2} degrees \
                 from the surface under it, {surface:?}"
            );
            seen.insert(face_of(direction));
            normals.push(surface);
        }
    }
    let mut worst_step = 0;
    for y in 0..size {
        for x in 0..size {
            let here = color_at(x, y);
            for (nx, ny) in [(x + 1, y), (x, y + 1)] {
                if nx >= size || ny >= size {
                    continue;
                }
                let there = color_at(nx, ny);
                let step = here
                    .iter()
                    .zip(there)
                    .map(|(a, b)| a.abs_diff(b))
                    .max()
                    .expect("three channels");
                worst_step = worst_step.max(step);
                assert!(
                    step <= NEIGHBOR_STEPS,
                    "looking along {axis:?}, pixels ({x}, {y}) {here:?} and ({nx}, {ny}) {there:?} \
                     are {step} steps apart: a seam"
                );
            }
        }
    }
    assert!(
        seen.len() >= faces,
        "looking along {axis:?}, the frame holds faces {seen:?}, fewer than {faces}"
    );
    println!(
        "looking along {axis:?}: faces {seen:?}, unit within {worst_unit:.4}, \
         placed within {worst_placement:.3} degrees, neighbors within {worst_step} steps"
    );
    normals
}

/// The globe drawn through the production shader shows no seam where two
/// faces meet or where three do.
///
/// A frame looks straight at the middle of each of the twelve edges and at
/// each of the eight corners, so the edges run through it. A discontinuity in
/// the warped direction shows twice here: as a jump between neighboring
/// pixels, and as a spike in its screen derivatives that sends the sampler to
/// a coarse level, which the marked cube paints grey.
#[test]
fn the_globe_draws_no_seam_where_faces_meet() {
    let ctx = render_ctx();
    let cube = create_marked_cube(&ctx.device, &ctx.queue, CODED_FACE, |face| {
        coded_face(face, CODED_FACE)
    });
    let axes = |signs: &[f32]| glam::Vec3::from_slice(signs).normalize();
    for (a, b) in [(0, 1), (0, 2), (1, 2)] {
        for sa in [1.0, -1.0] {
            for sb in [1.0, -1.0] {
                let mut signs = [0.0_f32; 3];
                signs[a] = sa;
                signs[b] = sb;
                assert_no_seam_looking_along(&ctx, &cube, axes(&signs), 2);
            }
        }
    }
    for corner in 0..8_u32 {
        let signs = [0, 1, 2].map(|bit| if corner >> bit & 1 == 0 { 1.0 } else { -1.0 });
        assert_no_seam_looking_along(&ctx, &cube, axes(&signs), 3);
    }
}

// ---------------------------------------------------------------------------
// The tiles above the floor
// ---------------------------------------------------------------------------

/// What the direction-coded tiles are cut to: two levels of 16 px tiles with a
/// 4 px gutter over faces of 64 texels, above a floor of 16.
const CODED_TILES: Geometry = Geometry {
    face: 64,
    levels: 2,
    tile: 16,
    gutter: 4,
    floor: 16,
    mask: 32,
};

/// Bit 2 of `Uniforms::flags`: the cube drawn alone is the night floor.
const FLAG_NIGHT_ALONE: u32 = 4;

/// The uniforms `base` names with the surface drawn from the cube and a tile's
/// sizes from `geometry`.
#[expect(
    clippy::cast_precision_loss,
    reason = "a tile's sizes are a few texels"
)]
fn with_tiles(base: &Uniforms, geometry: &Geometry) -> Uniforms {
    Uniforms {
        tile_texels: geometry.tile as f32,
        tile_gutter: geometry.gutter as f32,
        ..*base
    }
}

/// The layer of the direction-coded tile at `key`: every texel, the gutter's
/// too, holds the coded color of the direction through its center on the
/// face's grid extended past the face's edges, which is what the cutter's
/// gutter samples from the neighboring face.
fn coded_layer(key: TileKey, geometry: &Geometry) -> Vec<u8> {
    use sunlit_core::geometry::cube;
    let size = 1_u32 << key.level;
    let layer = geometry.layer();
    let origin =
        |index: u16| i64::from(u32::from(index) * geometry.tile) - i64::from(geometry.gutter);
    let (top, left) = (origin(key.row), origin(key.col));
    let mut texels = Vec::with_capacity((layer * layer * 4) as usize);
    for y in 0..i64::from(layer) {
        for x in 0..i64::from(layer) {
            let direction = cube::direction(
                usize::from(key.face),
                cube::texel_center(left + x, size),
                cube::texel_center(top + y, size),
            );
            let color = coded_color(direction);
            texels.extend([to_byte(color.x), to_byte(color.y), to_byte(color.z), 255]);
        }
    }
    texels
}

/// An RGBA8 tile array of one layer per entry of `layers`, each of its two
/// levels given as texels.
fn create_tile_array(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    geometry: &Geometry,
    layers: &[[Vec<u8>; 2]],
) -> wgpu::TextureView {
    let size = geometry.layer();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test_tile_array"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: u32::try_from(layers.len()).expect("a few layers"),
        },
        mip_level_count: 2,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (layer, levels) in (0..).zip(layers) {
        for (level, texels) in (0..).zip(levels) {
            let width = size >> level;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                texels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * width),
                    rows_per_image: Some(width),
                },
                wgpu::Extent3d {
                    width,
                    height: width,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

/// The page table texture `table` describes.
fn create_page_table(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    table: &PageTable,
) -> wgpu::TextureView {
    let cells = table.cells();
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("test_page_table"),
                size: wgpu::Extent3d {
                    width: cells,
                    height: cells,
                    depth_or_array_layers: 6,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R32Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            bytemuck::cast_slice(table.entries()),
        )
        .create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        })
}

/// The cell of the finest level the normal `n` lies in, and where within the
/// cell, 0 to 1 along a row and down a column.
fn cell_under(n: glam::DVec3, cells: u32) -> (u8, u32, u32, glam::DVec2) {
    use sunlit_core::geometry::cube;
    let (face, s, t) = cube::locate(n);
    let place = glam::DVec2::new(s, t).map(|w| f64::midpoint(w, 1.0) * f64::from(cells));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a coordinate on the face, clamped to its cells"
    )]
    let index = |v: f64| (v.floor() as u32).min(cells - 1);
    let (col, row) = (index(place.x), index(place.y));
    (
        u8::try_from(face).expect("six faces"),
        row,
        col,
        place - glam::DVec2::new(f64::from(col), f64::from(row)),
    )
}

/// The tiles the coded case makes resident: on every face a checker of the
/// coarse level, and finest tiles at every third cell, so that its frames meet
/// every pairing of the floor and the two levels, and meet them across the
/// face edges too.
fn coded_resident_set(geometry: &Geometry) -> Vec<TileId> {
    let finest = Geometry::level_of(geometry.face);
    let cells = u16::try_from(geometry.face / geometry.tile).expect("a few cells");
    let mut tiles = Vec::new();
    for face in 0..6_u8 {
        let id = |level, row, col| TileId {
            pack: PackKind::Day(0),
            key: TileKey {
                level,
                face,
                row,
                col,
            },
        };
        for row in 0..cells {
            for col in 0..cells {
                let (up_row, up_col) = (row / 2, col / 2);
                if row % 2 == 0 && col % 2 == 0 && (u16::from(face) + up_row + up_col) % 2 == 0 {
                    tiles.push(id(finest - 1, up_row, up_col));
                }
                if (u16::from(face) + row + 2 * col) % 3 == 0 {
                    tiles.push(id(finest, row, col));
                }
            }
        }
    }
    tiles
}

/// Every frame the seam case draws: along each axis, at the middle of each
/// edge, and at each corner, with the fewest faces each has to show.
fn seam_frames() -> Vec<(glam::Vec3, usize)> {
    let mut frames: Vec<(glam::Vec3, usize)> = AXES.iter().map(|&axis| (axis, 1)).collect();
    let axes = |signs: &[f32]| glam::Vec3::from_slice(signs).normalize();
    for (a, b) in [(0, 1), (0, 2), (1, 2)] {
        for sa in [1.0, -1.0] {
            for sb in [1.0, -1.0] {
                let mut signs = [0.0_f32; 3];
                signs[a] = sa;
                signs[b] = sb;
                frames.push((axes(&signs), 2));
            }
        }
    }
    for corner in 0..8_u32 {
        let signs = [0, 1, 2].map(|bit| if corner >> bit & 1 == 0 { 1.0 } else { -1.0 });
        frames.push((axes(&signs), 3));
    }
    frames
}

/// The floor and both tiled levels meet without a seam.
///
/// The direction-coded floor, marked below its finest level, and
/// direction-coded tiles, marked below theirs, are drawn through a page table
/// the production rewrite made from a fixed resident set, and every frame of
/// the seam case is held to the same placement, finest-level and neighbor
/// rules. A tile read at the wrong place in its layer, a gutter that does not
/// continue its neighbor, or gradients that send the sampler to a coarser level
/// fail it, at a face edge as anywhere else.
#[test]
fn the_floor_and_the_two_tile_levels_meet_without_a_seam() {
    let ctx = render_ctx();
    let geometry = CODED_TILES;
    let floor = create_marked_cube(&ctx.device, &ctx.queue, CODED_FACE, |face| {
        coded_face(face, CODED_FACE)
    });

    let resident = coded_resident_set(&geometry);
    let mut layers = TileLayers::new(u32::try_from(resident.len()).expect("a few tiles"));
    let marker = MIP_MARKER.repeat((geometry.layer() / 2).pow(2) as usize);
    let mut texels = Vec::new();
    for &id in &resident {
        let (layer, _) = layers.claim(id, None).expect("room for every tile");
        assert_eq!(layer as usize, texels.len(), "layers are taken in order");
        texels.push([coded_layer(id.key, &geometry), marker.clone()]);
    }
    let ocean = std::collections::HashSet::new();
    let day = PageSurface {
        pack: PackKind::Day(0),
        ocean: &ocean,
    };
    let mut table = PageTable::new(geometry);
    table.rewrite([Some(day), None], &layers, &CellLevels::finest(&geometry));

    let tiles = create_tile_array(&ctx.device, &ctx.queue, &geometry, &texels);
    let pages = create_page_table(&ctx.device, &ctx.queue, &table);
    let flat = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);
    let dummy = &ctx.dummy_cube;
    let bind_group = bind_group_with(
        &ctx,
        &flat,
        &ctx.surface_sampler,
        [&floor, dummy, dummy],
        [&tiles, &pages],
    );
    let render = |uniforms: &Uniforms| {
        render_with(
            &ctx,
            &with_tiles(uniforms, &geometry),
            &bind_group,
            SEAM_FRAME,
            SEAM_FRAME,
        )
    };

    let mut drawn = std::collections::BTreeMap::new();
    for (axis, faces) in seam_frames() {
        for normal in assert_no_seam(axis, faces, render) {
            let (face, row, col, _) = cell_under(normal, table.cells());
            let kind = match table.at(face, row, col)[0].expect("an entry") {
                PageEntry::Floor => "the floor",
                PageEntry::Ocean => "ocean",
                PageEntry::Tile { steps: 0, .. } => "a finest tile",
                PageEntry::Tile { .. } => "a coarse tile",
            };
            *drawn.entry(kind).or_insert(0_usize) += 1;
        }
    }
    println!("pixels drawn from each source: {drawn:?}");
    for kind in ["the floor", "a finest tile", "a coarse tile"] {
        assert!(
            drawn.get(kind).copied().unwrap_or(0) > 10_000,
            "too few pixels were drawn from {kind} for the case to mean anything: {drawn:?}"
        );
    }
}

/// With the night drawn alone, the cube at the day's binding is the night
/// floor and the page table's night half is what refines it; otherwise the
/// day half is.
#[test]
fn the_night_drawn_alone_reads_the_night_half_of_the_page_table() {
    let ctx = render_ctx();
    let geometry = CODED_TILES;
    let finest = Geometry::level_of(geometry.face);
    let cells = u16::try_from(geometry.face / geometry.tile).expect("a few cells");
    let everywhere: std::collections::HashSet<TileKey> = (0..6_u8)
        .flat_map(|face| {
            (0..cells).flat_map(move |row| {
                (0..cells).map(move |col| TileKey {
                    level: finest,
                    face,
                    row,
                    col,
                })
            })
        })
        .collect();
    let mut table = PageTable::new(geometry);
    table.rewrite(
        [
            None,
            Some(PageSurface {
                pack: PackKind::Night,
                ocean: &everywhere,
            }),
        ],
        &TileLayers::new(1),
        &CellLevels::finest(&geometry),
    );
    let pages = create_page_table(&ctx.device, &ctx.queue, &table);
    let floor = create_solid_cube(&ctx.device, &ctx.queue, [[200, 40, 40, 255]; 6]);
    let flat = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);
    let dummy = &ctx.dummy_cube;
    let bind_group = bind_group_with(
        &ctx,
        &flat,
        &ctx.surface_sampler,
        [&floor, dummy, dummy],
        [&ctx.dummy_tiles[0], &pages],
    );
    let size = 64;
    let night_ocean = [5, 5, 15, 255];
    let base = Uniforms {
        day_ocean: u32::from_le_bytes([90, 90, 90, 255]),
        night_ocean: u32::from_le_bytes(night_ocean),
        ..with_tiles(&looking_along(size, glam::Vec3::Z), &geometry)
    };

    let day = middle_pixel(&render_with(&ctx, &base, &bind_group, size, size), size);
    assert!(
        close(day, [200, 40, 40], 2),
        "the day half is all floor: {day:?}"
    );
    let night_alone = Uniforms {
        flags: base.flags | FLAG_NIGHT_ALONE,
        ..base
    };
    let night = middle_pixel(
        &render_with(&ctx, &night_alone, &bind_group, size, size),
        size,
    );
    let [r, g, b, _] = night_ocean;
    assert!(
        close(night, [r, g, b], 1),
        "the night half is all ocean: {night:?}"
    );
}

/// The two levels of a tile coded by level.
const FINEST_LEVEL: [u8; 4] = [255, 0, 0, 255];
const SECOND_LEVEL: [u8; 4] = [0, 0, 255, 255];

/// How far away the face-on frames of the minified case look from, in radii:
/// where the surface faces the camera a texel of the coded tiles' 64 px faces
/// covers about half a pixel of the seam case's frame, so the footprint asks
/// for 0.93 to 1 of the way to the second level.
const FOOTPRINT_DISTANCE: f32 = 18.0;

/// How squarely the surface has to face the camera for a pixel to be held to
/// its footprint, as the cosine between its normal and the view: within about
/// 25 degrees, where the footprint is nearly round and runs along the face's
/// own axes.
const FACING: f64 = 0.9;

/// How far the level of detail a tile's pixel was read at may lie from the one
/// its footprint on the face asks for, in steps of 255 in the blue that codes
/// the second level: half a level. The tiles read within 12.6 steps of it on
/// the Radeon, and Metal's cube and array part by up to 0.38 of a level, which
/// the ray cast cannot say which of the two to blame for; gradients left
/// unscaled into the layer read the finest level, 238 steps or more away.
const FOOTPRINT_STEPS: f64 = 128.0;

/// How far away the seam case's frames look from when the minified case holds
/// the face edges: far enough that the sampler reads both levels over most of
/// the disc.
const EDGE_DISTANCE: f32 = 8.0;

/// How far one pixel quad across a face edge may read a level of detail
/// outside the range of the quads beside it inside the faces, in steps of 255
/// in the blue: 20 at most on the Radeon, WARP and lavapipe and 48.5 on Metal,
/// and 132 to 177 on the first two with the quotient rule's term for the major
/// axis left out.
const EDGE_LEVEL_STEPS: f64 = 96.0;

/// The most those quads may lie outside their neighbors' range on average:
/// 0.36 to 0.62 steps on the Radeon, WARP and lavapipe, and 4.9 to 23 with
/// the quotient rule's term left out, which on lavapipe moves no single quad
/// past 25.
const EDGE_MEAN_STEPS: f64 = 2.5;

/// A tile read at minification is read at the level of detail its footprint
/// on the face asks for, and that level runs on across a face edge.
///
/// Every finest tile is resident, its first level red and its second blue, so
/// a pixel's blue says which level of detail it was read at. The level of
/// detail comes from the gradients alone through the surface sampler's
/// isotropic form. Where the surface faces the camera in the face-on frames
/// it is held to the footprint a ray cast of the pixel and its neighbors puts
/// on the face; a floor cube is no reference, since Metal's cube reads 97
/// steps from its own array near a face's corner. A pixel quad that straddles
/// a face edge takes its derivatives across the edge, which is the one place
/// the quotient rule's term for the major axis is not zero, since the warp
/// keeps the major component of `w` at one across a face; there the tiles are
/// held to themselves, the level of detail of such a quad to the range of the
/// quads beside it inside the faces.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one case, its tiles, and its two comparisons"
)]
fn a_minified_tile_is_read_at_the_level_of_detail_its_footprint_asks_for() {
    use sunlit_core::geometry::cube;

    let ctx = render_ctx();
    let geometry = CODED_TILES;
    let solid = |color: [u8; 4], width: u32| color.repeat((width * width) as usize);
    let finest = Geometry::level_of(geometry.face);
    let cells = u16::try_from(geometry.face / geometry.tile).expect("a few cells");
    let mut layers = TileLayers::new(6 * u32::from(cells).pow(2));
    let mut texels = Vec::new();
    for face in 0..6_u8 {
        for row in 0..cells {
            for col in 0..cells {
                let id = TileId {
                    pack: PackKind::Day(0),
                    key: TileKey {
                        level: finest,
                        face,
                        row,
                        col,
                    },
                };
                let (layer, _) = layers.claim(id, None).expect("room for every tile");
                assert_eq!(layer as usize, texels.len(), "layers are taken in order");
                texels.push([
                    solid(FINEST_LEVEL, geometry.layer()),
                    solid(SECOND_LEVEL, geometry.layer() / 2),
                ]);
            }
        }
    }
    let ocean = std::collections::HashSet::new();
    let day = PageSurface {
        pack: PackKind::Day(0),
        ocean: &ocean,
    };
    let mut table = PageTable::new(geometry);
    table.rewrite([Some(day), None], &layers, &CellLevels::finest(&geometry));
    let tiles = create_tile_array(&ctx.device, &ctx.queue, &geometry, &texels);
    let pages = create_page_table(&ctx.device, &ctx.queue, &table);
    let flat = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);
    let dummy = &ctx.dummy_cube;
    let isotropic = ctx
        .device
        .create_sampler(&sunlit_core::renderer::surface_sampler_descriptor(true));
    let bind_group = bind_group_with(
        &ctx,
        &flat,
        &isotropic,
        [dummy, dummy, dummy],
        [&tiles, &pages],
    );
    let size = SEAM_FRAME;
    let at = |x: u32, y: u32| ((y * size + x) * 4) as usize;
    let draw = |axis: glam::Vec3, distance: f32| {
        let uniforms = with_tiles(&looking_along_from(size, axis, distance), &geometry);
        let inverse = glam::Mat4::from_cols_array(&uniforms.mvp)
            .as_dmat4()
            .inverse();
        (
            render_with(&ctx, &uniforms, &bind_group, size, size),
            inverse,
        )
    };

    let texels_per_unit = f64::from(geometry.face) / 2.0;
    let (mut held, mut footprint_worst) = (0_usize, (0.0_f64, String::new()));
    for axis in AXES {
        let (frame, inverse) = draw(axis, FOOTPRINT_DISTANCE);
        for y in 0..size - 1 {
            for x in 0..size - 1 {
                let normals = [(x, y), (x + 1, y), (x, y + 1)]
                    .map(|(px, py)| normal_under_pixel(inverse, size, px, py));
                let [Some(here), Some(right), Some(below)] = normals else {
                    continue;
                };
                if here.dot(axis.as_dvec3()) < FACING {
                    continue;
                }
                let [(face, s, t), (face_x, s_x, t_x), (face_y, s_y, t_y)] =
                    [here, right, below].map(cube::locate);
                if face_x != face || face_y != face {
                    continue;
                }
                let rho = (s_x - s).hypot(t_x - t).max((s_y - s).hypot(t_y - t)) * texels_per_unit;
                let wanted = rho.log2().clamp(0.0, 1.0) * 255.0;
                let blue = f64::from(frame[at(x, y) + 2]);
                let off = (blue - wanted).abs();
                held += 1;
                if off > footprint_worst.0 {
                    footprint_worst = (
                        off,
                        format!(
                            "looking along {axis:?}, pixel ({x}, {y}) reads a blue of {blue} from \
                             its tile where its footprint of {rho:.3} texels asks for {wanted:.1}"
                        ),
                    );
                }
            }
        }
    }

    let quads = size / 2;
    let (mut edges, mut edge_total) = (0_usize, 0.0_f64);
    let mut edge_worst = (0.0_f64, String::new());
    for (axis, _) in seam_frames() {
        let (frame, inverse) = draw(axis, EDGE_DISTANCE);
        let face_at = |x, y| normal_under_pixel(inverse, size, x, y).map(|n| cube::locate(n).0);
        // Each quad's faces, and the red and blue of its pixels averaged.
        let mut quad = Vec::with_capacity((quads * quads) as usize);
        for qy in 0..quads {
            for qx in 0..quads {
                let pixels =
                    [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(dx, dy)| (2 * qx + dx, 2 * qy + dy));
                let faces = pixels.map(|(x, y)| face_at(x, y));
                let mean = |channel: usize| {
                    pixels
                        .iter()
                        .map(|&(x, y)| f64::from(frame[at(x, y) + channel]))
                        .sum::<f64>()
                        / 4.0
                };
                quad.push((faces, mean(0), mean(2)));
            }
        }
        let inside = |(faces, red, blue): ([Option<usize>; 4], f64, f64)| {
            faces[0].is_some()
                && faces.iter().all(|&face| face == faces[0])
                && red > 3.0
                && blue > 3.0
        };
        for qy in 1..quads - 1 {
            for qx in 1..quads - 1 {
                let (faces, _, blue) = quad[(qy * quads + qx) as usize];
                let mut distinct: Vec<_> = faces.iter().flatten().collect();
                distinct.sort_unstable();
                distinct.dedup();
                // An edge between two faces, the whole quad on the globe.
                if faces.iter().any(Option::is_none) || distinct.len() != 2 {
                    continue;
                }
                let beside: Vec<f64> = (qy - 1..=qy + 1)
                    .flat_map(|ny| (qx - 1..=qx + 1).map(move |nx| (ny * quads + nx) as usize))
                    .map(|index| quad[index])
                    .filter(|&neighbor| inside(neighbor))
                    .map(|(_, _, blue)| blue)
                    .collect();
                if beside.len() < 2 {
                    continue;
                }
                let low = beside.iter().copied().fold(f64::INFINITY, f64::min);
                let high = beside.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let outside = (low - blue).max(blue - high).max(0.0);
                edges += 1;
                edge_total += outside;
                if outside > edge_worst.0 {
                    edge_worst = (
                        outside,
                        format!(
                            "looking along {axis:?}, the quad at ({}, {}) across a face edge reads \
                             a blue of {blue:.1} from its tiles, outside the {low:.1} to {high:.1} \
                             of the quads beside it",
                            2 * qx,
                            2 * qy
                        ),
                    );
                }
            }
        }
    }
    let edge_mean = edge_total / f64::from(u32::try_from(edges).expect("a few thousand quads"));
    println!(
        "minified tiles: {held} pixels facing the camera from {FOOTPRINT_DISTANCE} radii, within \
         {:.1} steps of their footprints; {edges} quads across an edge from {EDGE_DISTANCE} radii, \
         within {:.1} steps of their neighbors' range and {edge_mean:.2} on average",
        footprint_worst.0, edge_worst.0
    );
    assert!(
        held > 1000 && edges > 500,
        "only {held} pixels face the camera and {edges} quads lie across an edge, too few for \
         the case to mean anything"
    );
    assert!(
        footprint_worst.0 <= FOOTPRINT_STEPS,
        "{}: another level of detail, {:.1} steps away",
        footprint_worst.1,
        footprint_worst.0
    );
    assert!(
        edge_mean <= EDGE_MEAN_STEPS,
        "the quads across an edge lie {edge_mean:.2} steps outside their range on average"
    );
    assert!(
        edge_worst.0 <= EDGE_LEVEL_STEPS,
        "{}: its level of detail jumps at the edge, {:.1} steps",
        edge_worst.1,
        edge_worst.0
    );
}

/// In blend mode the day half of a cell refines the day floor with the day's
/// ocean color and the night half the night floor with the night's: over a
/// table whose day half is the floor and whose night half is ocean, the lit
/// side shows the day floor and the dark side the night's ocean.
#[test]
fn blend_mode_reads_each_half_of_the_page_table_for_its_own_surface() {
    let ctx = render_ctx();
    let geometry = CODED_TILES;
    let finest = Geometry::level_of(geometry.face);
    let cells = u16::try_from(geometry.face / geometry.tile).expect("a few cells");
    let everywhere: std::collections::HashSet<TileKey> = (0..6_u8)
        .flat_map(|face| {
            (0..cells).flat_map(move |row| {
                (0..cells).map(move |col| TileKey {
                    level: finest,
                    face,
                    row,
                    col,
                })
            })
        })
        .collect();
    let nowhere = std::collections::HashSet::new();
    let mut table = PageTable::new(geometry);
    table.rewrite(
        [
            Some(PageSurface {
                pack: PackKind::Day(0),
                ocean: &nowhere,
            }),
            Some(PageSurface {
                pack: PackKind::Night,
                ocean: &everywhere,
            }),
        ],
        &TileLayers::new(1),
        &CellLevels::finest(&geometry),
    );
    let pages = create_page_table(&ctx.device, &ctx.queue, &table);
    let day_floor = create_solid_cube(&ctx.device, &ctx.queue, [[200, 40, 40, 255]; 6]);
    let night_floor = create_solid_cube(&ctx.device, &ctx.queue, [[40, 40, 200, 255]; 6]);
    let flat = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);
    let bind_group = bind_group_with(
        &ctx,
        &flat,
        &ctx.surface_sampler,
        [&day_floor, &night_floor, &ctx.dummy_cube],
        [&ctx.dummy_tiles[0], &pages],
    );
    let size = 64;
    let night_ocean = [20, 160, 20, 255];
    let base = Uniforms {
        terminator_width: 0.2,
        day_ocean: u32::from_le_bytes([90, 90, 90, 255]),
        night_ocean: u32::from_le_bytes(night_ocean),
        ..with_tiles(&looking_along(size, glam::Vec3::Z), &geometry)
    };

    let lit = Uniforms {
        sun_dir: [0.0, 0.0, 1.0],
        ..base
    };
    let day = middle_pixel(&render_with(&ctx, &lit, &bind_group, size, size), size);
    assert!(
        close(day, [200, 40, 40], 2),
        "the lit side shows the day floor, which the day half names: {day:?}"
    );
    let dark = Uniforms {
        sun_dir: [0.0, 0.0, -1.0],
        ..base
    };
    let night = middle_pixel(&render_with(&ctx, &dark, &bind_group, size, size), size);
    let [r, g, b, _] = night_ocean;
    assert!(
        close(night, [r, g, b], 1),
        "the dark side shows the night's ocean, which the night half names: {night:?}"
    );
}

/// What the Earth fixture's packs are cut to here, as the golden suite cuts
/// them: `EARTH_GEOMETRY` in `tests/golden.rs` says how it relates to the
/// shipped one.
const EARTH_TILES: Geometry = Geometry {
    face: 256,
    levels: 2,
    tile: 32,
    gutter: 4,
    floor: 64,
    mask: 128,
};

/// The level 0 texels of a pack entry's blob, decoded, and its width.
fn decoded_level(
    pack: &sunlit_core::assets::tiles::Pack,
    key: TileKey,
    level: usize,
) -> (u32, Vec<u8>) {
    let entry = pack.find(key).expect("the pack indexes every tile");
    let blob = pack.read(entry).expect("read a blob");
    let mip = pack.mips(entry)[level];
    let texels = sunlit_core::assets::tiles::decode_bc7(
        &blob[mip.offset..mip.offset + mip.len],
        mip.size,
        mip.size,
    )
    .expect("decode a blob");
    (mip.size, texels)
}

/// Face `face` of a surface pack at the tiled level `level` as one plane of
/// RGBA8, from the interiors of its tiles decoded, a tile the pack flags
/// constant ocean in the pack's ocean color.
fn assembled_face(pack: &sunlit_core::assets::tiles::Pack, level: u8, face: u8) -> Vec<u8> {
    let (tile, gutter) = (pack.tile() as usize, pack.gutter() as usize);
    let size = 1_usize << level;
    let per_side = u16::try_from(size / tile).expect("a few tiles");
    let mut plane = vec![0_u8; size * size * 4];
    for row in 0..per_side {
        for col in 0..per_side {
            let key = TileKey {
                level,
                face,
                row,
                col,
            };
            let entry = pack.find(key).expect("the pack indexes every tile");
            let layer = tile + 2 * gutter;
            let texels = if entry.ocean {
                pack.ocean().repeat(layer * layer)
            } else {
                decoded_level(pack, key, 0).1
            };
            for y in 0..tile {
                let from = ((y + gutter) * layer + gutter) * 4;
                let to = ((usize::from(row) * tile + y) * size + usize::from(col) * tile) * 4;
                plane[to..to + tile * 4].copy_from_slice(&texels[from..from + tile * 4]);
            }
        }
    }
    plane
}

/// A pack's floor as a cube with its full mip chain, decoded.
fn floor_cube(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pack: &sunlit_core::assets::tiles::Pack,
) -> wgpu::TextureView {
    let faces: Vec<_> = pack.entries().iter().filter(|e| e.whole_face).collect();
    let size = 1_u32 << faces[0].key.level;
    let levels = u32::try_from(pack.mips(faces[0]).len()).expect("a short chain");
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test_floor"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (face, entry) in (0..).zip(&faces) {
        for level in 0..levels {
            let (width, texels) = decoded_level(pack, entry.key, level as usize);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: face,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &texels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * width),
                    rows_per_image: Some(width),
                },
                wgpu::Extent3d {
                    width,
                    height: width,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::Cube),
        ..Default::default()
    })
}

/// A compute entry point appended to the production shaders: every texel of
/// both levels of one layer of the tile array, the finest level first.
fn tile_probe(layer: u32, size: u32) -> String {
    format!(
        "
@group(1) @binding(0) var<storage, read_write> probed_texels: array<vec4<f32>>;

@compute @workgroup_size(64)
fn tile_probe(@builtin(global_invocation_id) id: vec3<u32>) {{
    let fine = {size}u * {size}u;
    let coarse = ({size}u / 2u) * ({size}u / 2u);
    if id.x >= fine + coarse {{
        return;
    }}
    if id.x < fine {{
        probed_texels[id.x] = textureLoad(tile_array, vec2<u32>(id.x % {size}u, id.x / {size}u), {layer}u, 0);
    }} else {{
        let i = id.x - fine;
        let half = {size}u / 2u;
        probed_texels[id.x] = textureLoad(tile_array, vec2<u32>(i % half, i / half), {layer}u, 1);
    }}
}}
"
    )
}

/// The RGBA8 texels the tile array holds at `layer`, both levels, as bytes.
fn probe_tile_layer(
    ctx: &RenderContext,
    array: &wgpu::TextureView,
    layer: u32,
    size: u32,
) -> Vec<u8> {
    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tile_probe_shader"),
            source: wgpu::ShaderSource::Wgsl(production_shaders(&tile_probe(layer, size)).into()),
        });
    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("tile_probe_pipeline"),
            layout: None,
            module: &shader,
            entry_point: Some("tile_probe"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
    let count = size * size + (size / 2) * (size / 2);
    let output_size = u64::from(count) * 16;
    let output = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("tile_probe_output"),
        size: output_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let tiles_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 7,
            resource: wgpu::BindingResource::TextureView(array),
        }],
    });
    let output_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(1),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: output.as_entire_binding(),
        }],
    });
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: None,
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &tiles_group, &[]);
        pass.set_bind_group(1, &output_group, &[]);
        pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
    }
    ctx.queue.submit(std::iter::once(encoder.finish()));
    let data = common::read_buffer(&ctx.device, &ctx.queue, &output, output_size);
    bytemuck::cast_slice::<u8, f32>(&data)
        .iter()
        .map(|&v| to_byte(f64::from(v)))
        .collect()
}

/// How far a pixel drawn from a tile may be from the same pixel drawn from a
/// cube of the texels the tile was cut from, in steps of 255 on any channel:
/// the two read the same texels through one sampler, so what is left is the
/// arithmetic that places a sample in a layer rather than on a cube face.
const TILE_MATCH: u8 = 3;

/// A day pack's tiles, uploaded through the renderer's own upload into an RGBA8
/// array, which is what a CPU adapter and an adapter without block
/// compression get, are drawn in their cells as the texels they were cut from.
///
/// The Earth fixture is cut to the golden suite's geometry, a fixed set of
/// tiles of +Z is uploaded, and the globe is drawn looking down on +Z. Each
/// pixel, by the cell the surface under it lies in, is held to what that cell
/// names: a cell of a finest tile to a frame drawn from a cube of the finest
/// level's texels, a cell of a coarse tile to one of the coarse level's, a cell
/// of constant ocean to the pack's ocean color, and a cell of the floor to a
/// frame drawn from the floor alone. The layers themselves hold the blocks
/// decoded, level for level.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one case, its setup and its three comparisons"
)]
fn a_packs_tiles_are_drawn_in_their_cells_as_the_texels_they_were_cut_from() {
    use std::sync::atomic::AtomicBool;

    use sunlit_core::assets::cube_layout::CubeTextures;
    use sunlit_core::assets::tiles::{self, Pack};
    use sunlit_core::renderer::tiles::{SurfaceTiles, TileTexels, TileUpload};

    let scratch = common::test_support::ScratchDir::new("render_pipeline_tiles");
    common::test_support::write_earth_fixture(&scratch.join("textures"));
    let textures = CubeTextures::resolve(&scratch.join("textures"));
    let cache = scratch.join("cache");
    let day = PackKind::Day(6);
    tiles::ensure_pack(
        &cache,
        day,
        &textures,
        &EARTH_TILES,
        &AtomicBool::new(false),
    )
    .expect("cut the Earth fixture's July");
    let pack = Pack::open(&tiles::pack_path(&cache, day)).expect("open it");
    let geometry = EARTH_TILES;
    let finest = Geometry::level_of(geometry.face);
    let face = 4_u8;

    let per_side = u16::try_from(geometry.face / geometry.tile).expect("a few cells");
    let mut wanted = Vec::new();
    for row in 0..per_side {
        for col in 0..per_side {
            let fine = TileKey {
                level: finest,
                face,
                row,
                col,
            };
            let coarse = TileKey {
                level: finest - 1,
                face,
                row: row / 2,
                col: col / 2,
            };
            if row % 2 == 0 && col % 2 == 0 && (row / 2 + col / 2) % 2 == 0 {
                wanted.push(coarse);
            }
            if (row + 2 * col) % 3 == 0 {
                wanted.push(fine);
            }
        }
    }
    let uploads: Vec<TileUpload> = wanted
        .iter()
        .filter_map(|&key| {
            let entry = pack.find(key).expect("the pack indexes every tile");
            (!entry.ocean).then(|| TileUpload {
                id: TileId { pack: day, key },
                texels: TileTexels::Blocks(pack.read(entry).expect("read a tile")),
                layer: None,
            })
        })
        .collect();
    let first = uploads[0].id;

    let ctx = render_ctx();
    let mut surface_tiles =
        SurfaceTiles::new(&ctx.device, geometry, wgpu::TextureFormat::Rgba8Unorm, 64);
    surface_tiles
        .add_pack(&ctx.queue, &pack)
        .expect("a day pack");
    assert!(
        surface_tiles.set_month(&ctx.queue, 6),
        "the day half names July's tiles once July is the month in force"
    );
    let failed = surface_tiles.upload(&ctx.device, &ctx.queue, uploads);
    assert!(failed.is_empty(), "every tile uploads: {failed:?}");
    let array = surface_tiles
        .array_view()
        .expect("the first tile made the array");

    let layer = surface_tiles.layers().layer_of(first).expect("resident");
    let probed = probe_tile_layer(&ctx, array, layer, geometry.layer());
    let (_, fine_texels) = decoded_level(&pack, first.key, 0);
    let (_, coarse_texels) = decoded_level(&pack, first.key, 1);
    assert!(
        probed == [fine_texels, coarse_texels].concat(),
        "layer {layer} holds {:?} decoded, level for level",
        first.key
    );

    let floor = floor_cube(&ctx.device, &ctx.queue, &pack);
    let reference = |level: u8| {
        let size = 1_u32 << level;
        let plane = assembled_face(&pack, level, face);
        create_cube(&ctx.device, &ctx.queue, size, |f| {
            if f == usize::from(face) {
                plane.clone()
            } else {
                MIP_MARKER.repeat((size * size) as usize)
            }
        })
    };
    let (fine_cube, coarse_cube) = (reference(finest), reference(finest - 1));
    let size = 256;
    let uniforms = with_tiles(&looking_along(size, glam::Vec3::Z), &geometry);
    let ocean = pack.ocean();
    let uniforms = Uniforms {
        day_ocean: u32::from_le_bytes(ocean),
        ..uniforms
    };
    let flat = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);
    let dummy = &ctx.dummy_cube;
    let draw = |cube: &wgpu::TextureView, tiles: [&wgpu::TextureView; 2]| {
        let group = bind_group_with(
            &ctx,
            &flat,
            &ctx.surface_sampler,
            [cube, dummy, dummy],
            tiles,
        );
        render_with(&ctx, &uniforms, &group, size, size)
    };
    let [no_tiles, no_pages] = &ctx.dummy_tiles;
    let tiled = draw(&floor, [array, surface_tiles.page_view()]);
    let from_floor = draw(&floor, [no_tiles, no_pages]);
    let from_fine = draw(&fine_cube, [no_tiles, no_pages]);
    let from_coarse = draw(&coarse_cube, [no_tiles, no_pages]);

    let inverse = glam::Mat4::from_cols_array(&uniforms.mvp)
        .as_dmat4()
        .inverse();
    let pixel = |frame: &[u8], at: usize| [frame[at], frame[at + 1], frame[at + 2]];
    let apart = |a: [u8; 3], b: [u8; 3]| {
        a.iter()
            .zip(b)
            .map(|(x, y)| x.abs_diff(y))
            .max()
            .unwrap_or(0)
    };
    let mut checked = std::collections::BTreeMap::new();
    // Summed over the pixels of finest tiles: how far each is from the finest
    // level's frame, and how far that frame is from the coarse level's.
    let (mut off_finest, mut levels_apart) = (0_u64, 0_u64);
    for y in 0..size {
        for x in 0..size {
            let normal = normal_under_pixel(inverse, size, x, y).expect("the globe overfills");
            let (on, row, col, within) = cell_under(normal, surface_tiles.table().cells());
            assert_eq!(on, face, "the frame shows +Z alone");
            if within.min_element() < 0.15 || within.max_element() > 0.85 {
                continue;
            }
            let at = ((y * size + x) * 4) as usize;
            let seen = pixel(&tiled, at);
            let entry = surface_tiles.table().at(face, row, col)[0].expect("an entry");
            let (kind, expected, tolerance) = match entry {
                PageEntry::Tile { steps: 0, .. } => {
                    off_finest += u64::from(apart(seen, pixel(&from_fine, at)));
                    levels_apart +=
                        u64::from(apart(pixel(&from_fine, at), pixel(&from_coarse, at)));
                    ("a finest tile", pixel(&from_fine, at), TILE_MATCH)
                }
                PageEntry::Tile { .. } => ("a coarse tile", pixel(&from_coarse, at), TILE_MATCH),
                PageEntry::Ocean => ("ocean", [ocean[0], ocean[1], ocean[2]], 1),
                PageEntry::Floor => ("the floor", pixel(&from_floor, at), 1),
            };
            assert!(
                apart(seen, expected) <= tolerance,
                "pixel ({x}, {y}) in cell ({row}, {col}), {kind}, is {seen:?}, where {expected:?} was drawn"
            );
            *checked.entry(kind).or_insert(0_usize) += 1;
        }
    }
    println!("pixels checked against each source: {checked:?}");
    for kind in ["a finest tile", "a coarse tile", "ocean", "the floor"] {
        assert!(
            checked.get(kind).copied().unwrap_or(0) > 1_000,
            "too few pixels were drawn from {kind} to mean anything: {checked:?}"
        );
    }
    println!(
        "finest tiles: {off_finest} steps from the finest level in all, which is {levels_apart} from the coarse"
    );
    assert!(
        levels_apart > 4 * off_finest.max(1),
        "the two levels are too alike here for a match to the finest to mean anything"
    );

    // Capped at the floor, the same resident set draws the floor alone.
    let floor_level = Geometry::level_of(geometry.floor);
    let capped = CellLevels::uniform(&geometry, floor_level);
    assert!(surface_tiles.set_cap(&ctx.queue, capped.clone()));
    assert!(
        !surface_tiles.set_cap(&ctx.queue, capped),
        "the same cap again"
    );
    let array = surface_tiles.array_view().expect("still there");
    let capped = draw(&floor, [array, surface_tiles.page_view()]);
    assert!(
        capped == from_floor,
        "a cap at the floor draws the floor wherever tiles are resident"
    );
}
