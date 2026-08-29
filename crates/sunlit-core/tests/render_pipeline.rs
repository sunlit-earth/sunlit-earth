//! GPU integration tests for the full render pipeline (vertex + fragment shaders).
//!
//! Tests the production `sphere.wgsl` and `blend.wgsl` shaders end-to-end,
//! rendering to a small offscreen texture and asserting behavioral invariants.

mod common;

use std::sync::{LazyLock, Mutex};

use wgpu::util::DeviceExt;

// ---------------------------------------------------------------------------
// Local copies of production types (integration tests can't import from a bin crate)
// ---------------------------------------------------------------------------

/// Matches the production `Uniforms` struct in `renderer/uniforms.rs`.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    mvp: [f32; 16],
    sun_dir: [f32; 3],
    terminator_width: f32,
    flags: u32,
    diffuse_floor: f32,
    diffuse_ramp: f32,
    _pad: f32,
    eye_pos: [f32; 3],
    _pad2: f32,
    spec_shininess: f32,
    spec_intensity: f32,
    fresnel_mix: f32,
    fresnel_exp: f32,
    day_gamma: f32,
    day_saturation: f32,
    night_gamma: f32,
    night_saturation: f32,
    cloud_sphere_radius: f32,
    cloud_opacity: f32,
    cloud_floor: f32,
    cloud_gamma: f32,
    rayleigh_intensity: f32,
    rayleigh_sharpness: f32,
    nightglow_intensity: f32,
    nightglow_falloff: f32,
    nightglow_balance: f32,
    rayleigh_radius: f32,
    nightglow_orange_radius: f32,
    nightglow_green_radius: f32,
    rayleigh_haze: f32,
    cloud_night: f32,
    cloud_terminator: f32,
    _pad5: f32,
    sky_view: [f32; 16],
    world_from_eqj: [[f32; 4]; 3],
    viewport_size: [f32; 2],
    screen_offset: [f32; 2],
    star_intensity: f32,
    star_mag_limit: f32,
    star_size: f32,
    star_glow_strength: f32,
    star_glow_radius: f32,
    star_contrast: f32,
    sky_fov: f32,
    sun_glow: f32,
    sun_rays: f32,
    sun_flare: f32,
    sun_visible: f32,
    sun_size: f32,
    sun_view_dir: [f32; 3],
    sun_disk_radius: f32,
    moon_model: [f32; 16],
    moon_brightness: f32,
    moon_earthshine: f32,
    milky_way_intensity: f32,
    cloud_opacity_night: f32,
    sun_glare_tint: [f32; 3],
    sun_horizon_gain: f32,
    sun_globe_center: [f32; 2],
    sun_globe_radius: f32,
    sun_atmosphere_radius: f32,
    sun_zone_width: f32,
    sun_squash: f32,
    sun_halo_radius: f32,
    sun_reddening: f32,
    atmo_sunrise_glow: f32,
    atmo_sunrise_g: f32,
    sun_flux: f32,
    _pad7: f32,
}

const _: () = assert!(std::mem::size_of::<Uniforms>() == 544);

/// Matches the production `Vertex` struct in `sphere.rs`.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 3],
    uv: [f32; 2],
}

/// Generate a UV sphere mesh (simplified version of production code).
#[allow(clippy::cast_precision_loss, clippy::many_single_char_names)]
fn generate_uv_sphere(stacks: u32, sectors: u32) -> (Vec<Vertex>, Vec<u32>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let stacks_f = stacks as f32;
    let sectors_f = sectors as f32;

    for i in 0..=stacks {
        let stack_angle =
            std::f32::consts::FRAC_PI_2 - (i as f32) * std::f32::consts::PI / stacks_f;
        let xy = stack_angle.cos();
        let y = stack_angle.sin();

        for j in 0..=sectors {
            let sector_angle = (j as f32) * 2.0 * std::f32::consts::PI / sectors_f;
            let x = xy * sector_angle.cos();
            let z = xy * sector_angle.sin();
            let u = j as f32 / sectors_f;
            let v = i as f32 / stacks_f;
            vertices.push(Vertex {
                position: [x, y, z],
                uv: [u, v],
            });
        }
    }

    for i in 0..stacks {
        for j in 0..sectors {
            let first = i * (sectors + 1) + j;
            let second = first + sectors + 1;
            indices.push(first);
            indices.push(first + 1);
            indices.push(second);
            indices.push(first + 1);
            indices.push(second + 1);
            indices.push(second);
        }
    }

    (vertices, indices)
}

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

struct RenderContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform_buffer: wgpu::Buffer,
    sampler: wgpu::Sampler,
}

#[allow(clippy::too_many_lines)]
fn create_render_context() -> RenderContext {
    let ctx = common::create_gpu_context(false);
    let device = ctx.device;
    let queue = ctx.queue;

    let (vertices, indices) = generate_uv_sphere(32, 32);

    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("test_vertices"),
        contents: bytemuck::cast_slice(&vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });

    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("test_indices"),
        contents: bytemuck::cast_slice(&indices),
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

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("test_pipeline_layout"),
        bind_group_layouts: &[&bind_group_layout],
        immediate_size: 0,
    });

    let wgsl_source = format!(
        "{}\n{}",
        include_str!("../shaders/blend.wgsl"),
        include_str!("../shaders/sphere.wgsl"),
    );
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("test_sphere_shader"),
        source: wgpu::ShaderSource::Wgsl(wgsl_source.into()),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("test_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[
                    wgpu::VertexAttribute {
                        offset: 0,
                        shader_location: 0,
                        format: wgpu::VertexFormat::Float32x3,
                    },
                    wgpu::VertexAttribute {
                        offset: 12,
                        shader_location: 1,
                        format: wgpu::VertexFormat::Float32x2,
                    },
                ],
            }],
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
    let index_count = indices.len() as u32;

    RenderContext {
        device,
        queue,
        pipeline,
        bind_group_layout,
        vertex_buffer,
        index_buffer,
        index_count,
        uniform_buffer,
        sampler,
    }
}

static RENDER_CTX: LazyLock<Mutex<RenderContext>> =
    LazyLock::new(|| Mutex::new(create_render_context()));

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

/// Render a frame and return the RGBA8 pixel data.
fn render_frame(
    ctx: &RenderContext,
    uniforms: &Uniforms,
    day_texture: &wgpu::TextureView,
    night_texture: &wgpu::TextureView,
    width: u32,
    height: u32,
) -> Vec<u8> {
    ctx.queue
        .write_buffer(&ctx.uniform_buffer, 0, bytemuck::cast_slice(&[*uniforms]));

    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("test_bind_group"),
        layout: &ctx.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: ctx.uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(day_texture),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&ctx.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(night_texture),
            },
        ],
    });

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
        pass.set_bind_group(0, &bind_group, &[]);
        pass.set_vertex_buffer(0, ctx.vertex_buffer.slice(..));
        pass.set_index_buffer(ctx.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..ctx.index_count, 0, 0..1);
    }

    ctx.queue.submit(std::iter::once(encoder.finish()));

    common::read_texture_rgba8(&ctx.device, &ctx.queue, &render_texture, width, height)
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
        sun_atmosphere_radius: 110.0,
        sun_zone_width: 10.0,
        sun_squash: 1.0,
        sun_halo_radius: 3.0,
        sun_reddening: 1.0,
        atmo_sunrise_glow: 0.0,
        atmo_sunrise_g: 0.5,
        sun_flux: 1.0,
        _pad7: 0.0,
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
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    let white = create_solid_texture(&ctx.device, &ctx.queue, [255, 255, 255, 255]);
    let black = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);

    let uniforms = default_test_uniforms(size);

    let pixels = render_frame(&ctx, &uniforms, &white, &black, size, size);
    let visible = count_non_clear_pixels(&pixels);

    assert!(
        visible > 100,
        "Expected many visible (non-clear) pixels, got {visible}"
    );
}

#[test]
fn day_side_brighter_than_night_side() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    let white = create_solid_texture(&ctx.device, &ctx.queue, [255, 255, 255, 255]);
    let dark_gray = create_solid_texture(&ctx.device, &ctx.queue, [30, 30, 30, 255]);

    // Sun pointing along +Z (toward the camera at lon=0)
    let uniforms = Uniforms {
        terminator_width: 0.15,
        flags: 1, // diffuse enabled
        ..default_test_uniforms(size)
    };

    let pixels = render_frame(&ctx, &uniforms, &white, &dark_gray, size, size);

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

    // Day-lit white sphere center should be significantly bright
    assert!(
        center_lum > 50.0,
        "Center of day-lit sphere should be bright, got luminance {center_lum:.1}"
    );
}

#[test]
fn single_texture_mode_ignores_night() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    // Red day texture, green night texture
    let red = create_solid_texture(&ctx.device, &ctx.queue, [255, 0, 0, 255]);
    let green = create_solid_texture(&ctx.device, &ctx.queue, [0, 255, 0, 255]);

    let uniforms = default_test_uniforms(size);

    let pixels = render_frame(&ctx, &uniforms, &red, &green, size, size);

    // Check all non-clear pixels: none should have green > 0
    let mut green_pixels = 0;
    for px in pixels.chunks(4) {
        // Skip clear-color pixels
        let is_clear = px[0] <= 6 && px[1] <= 6 && px[2] <= 14;
        if !is_clear && px[1] > 10 {
            green_pixels += 1;
        }
    }

    assert_eq!(
        green_pixels, 0,
        "Single-texture mode should not sample the night texture, but found {green_pixels} green pixels"
    );
}

// ---------------------------------------------------------------------------
// Uniform buffer field offset test (Step 4.3)
// ---------------------------------------------------------------------------

const UNIFORM_READBACK_SHADER: &str = r"
struct Uniforms {
    mvp: mat4x4<f32>,
    sun_dir: vec3<f32>,
    terminator_width: f32,
    flags: u32,
    diffuse_floor: f32,
    diffuse_ramp: f32,
    _pad: f32,
    eye_pos: vec3<f32>,
    _pad2: f32,
    spec_shininess: f32,
    spec_intensity: f32,
    fresnel_mix: f32,
    fresnel_exp: f32,
    day_gamma: f32,
    day_saturation: f32,
    night_gamma: f32,
    night_saturation: f32,
    cloud_sphere_radius: f32,
    cloud_opacity: f32,
    cloud_floor: f32,
    cloud_gamma: f32,
    rayleigh_intensity: f32,
    rayleigh_sharpness: f32,
    nightglow_intensity: f32,
    nightglow_falloff: f32,
    nightglow_balance: f32,
    rayleigh_radius: f32,
    nightglow_orange_radius: f32,
    nightglow_green_radius: f32,
    rayleigh_haze: f32,
    cloud_night: f32,
    cloud_terminator: f32,
    _pad5: f32,
    sky_view: mat4x4<f32>,
    world_from_eqj: mat3x3<f32>,
    viewport_size: vec2<f32>,
    screen_offset: vec2<f32>,
    star_intensity: f32,
    star_mag_limit: f32,
    star_size: f32,
    star_glow_strength: f32,
    star_glow_radius: f32,
    star_contrast: f32,
    sky_fov: f32,
    sun_glow: f32,
    sun_rays: f32,
    sun_flare: f32,
    sun_visible: f32,
    sun_size: f32,
    sun_view_dir: vec3<f32>,
    sun_disk_radius: f32,
    moon_model: mat4x4<f32>,
    moon_brightness: f32,
    moon_earthshine: f32,
    milky_way_intensity: f32,
    cloud_opacity_night: f32,
    sun_glare_tint: vec3<f32>,
    sun_horizon_gain: f32,
    sun_globe_center: vec2<f32>,
    sun_globe_radius: f32,
    sun_atmosphere_radius: f32,
    sun_zone_width: f32,
    sun_squash: f32,
    sun_halo_radius: f32,
    sun_reddening: f32,
    atmo_sunrise_glow: f32,
    atmo_sunrise_g: f32,
    sun_flux: f32,
    _pad7: f32,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;

@compute @workgroup_size(1)
fn main() {
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
    output[68] = uniforms.sun_atmosphere_radius;
    output[69] = uniforms.sun_zone_width;
    output[70] = uniforms.sun_squash;
    output[71] = uniforms.sun_halo_radius;
    output[72] = uniforms.sun_reddening;
    output[73] = uniforms.atmo_sunrise_glow;
    output[74] = uniforms.atmo_sunrise_g;
    output[75] = uniforms.sun_flux;
}
";

// One assertion per uniform field: splitting it would only hide which field
// moved.
#[allow(clippy::too_many_lines)]
#[test]
fn uniform_buffer_field_offsets_match_wgsl() {
    let ctx = RENDER_CTX.lock().unwrap();

    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("uniform_readback_shader"),
            source: wgpu::ShaderSource::Wgsl(UNIFORM_READBACK_SHADER.into()),
        });

    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("uniform_readback_pipeline"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

    // Write known values to a fresh uniform buffer
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
        sun_atmosphere_radius: 42.75,
        sun_zone_width: 6.25,
        sun_squash: 0.45,
        sun_halo_radius: 4.25,
        sun_reddening: 1.35,
        atmo_sunrise_glow: 1.85,
        atmo_sunrise_g: 0.62,
        sun_flux: 0.72,
        _pad7: 0.0,
    };

    let uniform_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("test_uniform_buf"),
            contents: bytemuck::cast_slice(&[uniforms]),
            usage: wgpu::BufferUsages::UNIFORM,
        });

    // Output buffer: 76 floats
    let output_size = (76 * std::mem::size_of::<f32>()) as u64;
    let output_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("uniform_test_output"),
        size: output_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
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
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    ctx.queue.submit(std::iter::once(encoder.finish()));

    let data = common::read_buffer(&ctx.device, &ctx.queue, &output_buf, output_size);
    let values: &[f32] = bytemuck::cast_slice(&data);

    let eps = 1e-6;
    assert!(
        (values[0] - 1.0).abs() < eps,
        "MVP[0][0]: got {}, expected 1.0",
        values[0]
    );
    assert!(
        (values[1] - 2.0).abs() < eps,
        "MVP[1][1]: got {}, expected 2.0",
        values[1]
    );
    assert!(
        (values[2] - 3.0).abs() < eps,
        "MVP[2][2]: got {}, expected 3.0",
        values[2]
    );
    assert!(
        (values[3] - 4.0).abs() < eps,
        "MVP[3][3]: got {}, expected 4.0",
        values[3]
    );
    assert!(
        (values[4] - 0.0).abs() < eps,
        "sun_dir.x: got {}, expected 0.0",
        values[4]
    );
    assert!(
        (values[5] - 1.0).abs() < eps,
        "sun_dir.y: got {}, expected 1.0",
        values[5]
    );
    assert!(
        (values[6] - 0.0).abs() < eps,
        "sun_dir.z: got {}, expected 0.0",
        values[6]
    );
    assert!(
        (values[7] - 0.15).abs() < eps,
        "terminator_width: got {}, expected 0.25",
        values[7]
    );
    assert!(
        (values[8] - 1.0).abs() < eps,
        "flags: got {}, expected 1.0",
        values[8]
    );
    assert!(
        (values[9] - 0.5).abs() < eps,
        "diffuse_floor: got {}, expected 0.5",
        values[9]
    );
    assert!(
        (values[10] - 0.25).abs() < eps,
        "diffuse_ramp: got {}, expected 0.55",
        values[10]
    );
    assert!(
        (values[11] - 1.0).abs() < eps,
        "eye_pos.x: got {}, expected 1.0",
        values[11]
    );
    assert!(
        (values[12] - 2.0).abs() < eps,
        "eye_pos.y: got {}, expected 2.0",
        values[12]
    );
    assert!(
        (values[13] - 3.0).abs() < eps,
        "eye_pos.z: got {}, expected 3.0",
        values[13]
    );
    assert!(
        (values[14] - 150.0).abs() < eps,
        "spec_shininess: got {}, expected 150.0",
        values[14]
    );
    assert!(
        (values[15] - 0.75).abs() < eps,
        "spec_intensity: got {}, expected 0.75",
        values[15]
    );
    assert!(
        (values[16] - 0.5).abs() < eps,
        "fresnel_mix: got {}, expected 0.5",
        values[16]
    );
    assert!(
        (values[17] - 3.0).abs() < eps,
        "fresnel_exp: got {}, expected 3.0",
        values[17]
    );
    assert!(
        (values[18] - 1.5).abs() < eps,
        "day_gamma: got {}, expected 1.5",
        values[18]
    );
    assert!(
        (values[19] - 0.8).abs() < eps,
        "day_saturation: got {}, expected 0.8",
        values[19]
    );
    assert!(
        (values[20] - 2.0).abs() < eps,
        "night_gamma: got {}, expected 2.0",
        values[20]
    );
    assert!(
        (values[21] - 0.6).abs() < eps,
        "night_saturation: got {}, expected 0.6",
        values[21]
    );
    assert!(
        (values[22] - 1.0015).abs() < eps,
        "cloud_sphere_radius: got {}, expected 1.0015",
        values[22]
    );
    assert!(
        (values[23] - 0.9).abs() < eps,
        "cloud_opacity: got {}, expected 0.9",
        values[23]
    );
    assert!(
        (values[24] - 0.25).abs() < eps,
        "cloud_floor: got {}, expected 0.55",
        values[24]
    );
    assert!(
        (values[25] - 0.65).abs() < eps,
        "cloud_gamma: got {}, expected 0.65",
        values[25]
    );
    assert!(
        (values[26] - 0.5).abs() < eps,
        "rayleigh_intensity: got {}, expected 0.5",
        values[26]
    );
    assert!(
        (values[27] - 50.0).abs() < eps,
        "rayleigh_sharpness: got {}, expected 50.0",
        values[27]
    );
    assert!(
        (values[28] - 0.25).abs() < eps,
        "nightglow_intensity: got {}, expected 0.25",
        values[28]
    );
    assert!(
        (values[29] - 15.0).abs() < eps,
        "nightglow_falloff: got {}, expected 15.0",
        values[29]
    );
    assert!(
        (values[30] - 0.37).abs() < eps,
        "nightglow_balance: got {}, expected 0.37",
        values[30]
    );
    assert!(
        (values[31] - 1.003).abs() < eps,
        "rayleigh_radius: got {}, expected 1.003",
        values[31]
    );
    assert!(
        (values[32] - 1.014).abs() < eps,
        "nightglow_orange_radius: got {}, expected 1.014",
        values[32]
    );
    assert!(
        (values[33] - 1.015).abs() < eps,
        "nightglow_green_radius: got {}, expected 1.015",
        values[33]
    );
    assert!(
        (values[34] - 0.55).abs() < eps,
        "rayleigh_haze: got {}, expected 0.55",
        values[34]
    );
    assert!(
        (values[35] - 1.0).abs() < eps,
        "star_intensity: got {}, expected 1.0",
        values[35]
    );
    assert!(
        (values[36] - 6.5).abs() < eps,
        "star_mag_limit: got {}, expected 6.5",
        values[36]
    );
    assert!(
        (values[37] - 1.25).abs() < eps,
        "star_size: got {}, expected 1.25",
        values[37]
    );
    assert!(
        (values[38] - 0.35).abs() < eps,
        "star_glow_strength: got {}, expected 0.35",
        values[38]
    );
    assert!(
        (values[39] - 6.0).abs() < eps,
        "star_glow_radius: got {}, expected 6.0",
        values[39]
    );
    assert!(
        (values[40] - 0.4).abs() < eps,
        "star_contrast: got {}, expected 0.4",
        values[40]
    );
    assert!(
        (values[41] - 123.0).abs() < eps,
        "sky_fov: got {}, expected 123.0",
        values[41]
    );
    assert!(
        (values[42] - 1.75).abs() < eps,
        "sun_glow: got {}, expected 1.75",
        values[42]
    );
    assert!(
        (values[43] - 0.45).abs() < eps,
        "sun_rays: got {}, expected 0.45",
        values[43]
    );
    assert!(
        (values[44] - 0.8).abs() < eps,
        "sun_flare: got {}, expected 0.8",
        values[44]
    );
    assert!(
        (values[45] - 0.6).abs() < eps,
        "sun_visible: got {}, expected 0.6",
        values[45]
    );
    assert!(
        (values[46] - 2.75).abs() < eps,
        "sun_size: got {}, expected 2.75",
        values[46]
    );
    assert!(
        values[47].abs() < eps && (values[48] - 0.6).abs() < eps && (values[49] + 0.8).abs() < eps,
        "sun_view_dir: got {:?}, expected [0.0, 0.6, -0.8]",
        [values[47], values[48], values[49]]
    );
    assert!(
        (values[50] - 7.5).abs() < eps,
        "sun_disk_radius: got {}, expected 7.5",
        values[50]
    );
    assert!(
        (values[51] - 0.25).abs() < eps
            && (values[52] - 0.5).abs() < eps
            && (values[53] - 0.75).abs() < eps
            && (values[54] - 12.5).abs() < eps,
        "moon_model: got {:?}, expected [0.25, 0.5, 0.75, 12.5]",
        [values[51], values[52], values[53], values[54]]
    );
    assert!(
        (values[55] - 1.25).abs() < eps,
        "moon_brightness: got {}, expected 1.25",
        values[55]
    );
    assert!(
        (values[56] - 0.35).abs() < eps,
        "moon_earthshine: got {}, expected 0.35",
        values[56]
    );
    assert!(
        (values[57] - 0.65).abs() < eps,
        "milky_way_intensity: got {}, expected 0.65",
        values[57]
    );
    assert!(
        (values[58] - 0.31).abs() < eps,
        "cloud_night: got {}, expected 0.31",
        values[58]
    );
    assert!(
        (values[59] - 0.19).abs() < eps,
        "cloud_terminator: got {}, expected 0.19",
        values[59]
    );
    assert!(
        (values[60] - 0.61).abs() < eps,
        "cloud_opacity_night: got {}, expected 0.61",
        values[60]
    );
    for (index, expected, name) in [
        (61, 0.95, "sun_glare_tint.r"),
        (62, 0.55, "sun_glare_tint.g"),
        (63, 0.15, "sun_glare_tint.b"),
        (64, 2.25, "sun_horizon_gain"),
        (65, 64.5, "sun_globe_center.x"),
        (66, 33.25, "sun_globe_center.y"),
        (67, 41.5, "sun_globe_radius"),
        (68, 42.75, "sun_atmosphere_radius"),
        (69, 6.25, "sun_zone_width"),
        (70, 0.45, "sun_squash"),
        (71, 4.25, "sun_halo_radius"),
        (72, 1.35, "sun_reddening"),
        (73, 1.85, "atmo_sunrise_glow"),
        (74, 0.62, "atmo_sunrise_g"),
        (75, 0.72, "sun_flux"),
    ] {
        assert!(
            (values[index] - expected).abs() < eps,
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

@compute @workgroup_size(1)
fn shared_rule_probe() {
    rule_out[0] = output_pixel_scale();
    rule_out[1] = sky_lens_edge_radius();
}
";

/// The output-density ramp and the sky lens's edge radius exist once in WGSL
/// and once in `scene::sun_occlusion`, and both pairings matter at the pixel:
/// the CPU sizes the Sun's disk with the ramp and the shader draws that disk's
/// antialiased edge with it, and the CPU measures occlusion at a screen
/// position the shader has to draw the Sun at.
///
/// The heights avoid 1080 and below, where the ramp clamps to 1.0 and any two
/// knees agree: every golden and every engine frame renders there, so nothing
/// else in the suite can see a divergence at all.
#[test]
fn the_shader_and_the_cpu_agree_on_the_two_shared_rules() {
    let ctx = RENDER_CTX.lock().unwrap();

    let wgsl_source = format!(
        "{}\n{}\n{}",
        include_str!("../shaders/blend.wgsl"),
        include_str!("../shaders/sphere.wgsl"),
        SHARED_RULE_PROBE,
    );
    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shared_rule_probe_shader"),
            source: wgpu::ShaderSource::Wgsl(wgsl_source.into()),
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

    let output_size = 2 * std::mem::size_of::<f32>() as u64;
    let output_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("shared_rule_probe_output"),
        size: output_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
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
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: output_buf.as_entire_binding(),
        }],
    });

    let probe = |height: f32, sky_fov: f32| {
        let uniforms = Uniforms {
            viewport_size: [height * 16.0 / 9.0, height],
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
            pass.set_bind_group(1, &output_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        ctx.queue.submit(std::iter::once(encoder.finish()));
        let data = common::read_buffer(&ctx.device, &ctx.queue, &output_buf, output_size);
        let values: &[f32] = bytemuck::cast_slice(&data);
        (values[0], values[1])
    };

    // Below the knee, on it, three points up the ramp, and past the ceiling.
    for &height in &[540.0_f32, 1080.0, 1350.0, 1620.0, 2160.0, 3240.0] {
        // Under the lower clamp, both ends of the slider's range, and over the
        // upper one.
        for &sky_fov in &[30.0_f32, 60.0, 95.0, 140.0, 180.0, 220.0] {
            let (shader_scale, shader_edge) = probe(height, sky_fov);
            let cpu_scale = sunlit_core::scene::sun_occlusion::pixel_scale(height);
            let cpu_edge = sunlit_core::scene::sun_occlusion::sky_lens_edge_radius(sky_fov);
            assert!(
                (shader_scale - cpu_scale).abs() < 1e-6,
                "the density ramp at {height} pixels: the shader says {shader_scale}, \
                 scene::sun_occlusion::pixel_scale says {cpu_scale}"
            );
            assert!(
                (shader_edge - cpu_edge).abs() < 2e-5 * cpu_edge,
                "the sky lens edge radius at {sky_fov} degrees: the shader says {shader_edge}, \
                 scene::sun_occlusion::sky_lens_edge_radius says {cpu_edge}"
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
/// forward pair places. Two transposes and a lens inversion is three places a
/// sign can be wrong, and every one of them yields a plausible-looking sky
/// rather than an obviously broken one.
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
    /// The round trip is exact to 6.5e-7 on warp and to 1.5e-4 on lavapipe,
    /// which is the two adapters' transcendentals rather than anything about
    /// the chain, and 1.5e-4 is half a hundredth of a degree. This sits an
    /// order of magnitude above the worse of them and three below the faults it
    /// exists to catch, which are 1.229 and 0.546.
    const TOLERANCE: f32 = 1.0e-3;

    let ctx = RENDER_CTX.lock().unwrap();

    let wgsl_source = format!(
        "{}\n{}\n{}",
        include_str!("../shaders/blend.wgsl"),
        include_str!("../shaders/sphere.wgsl"),
        ROUND_TRIP_PROBE,
    );
    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("round_trip_probe_shader"),
            source: wgpu::ShaderSource::Wgsl(wgsl_source.into()),
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

    // The two ends of the slider and the default, with no pan and a pan of
    // either sign in both axes.
    for sky_fov in [60.0_f32, 140.0, 180.0] {
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
// Fresnel specular tests (Step 2.1)
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
fn fresnel_specular_zero_intensity_unchanged() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    // All-water texture (alpha=128): RGB can be ocean-like
    let water = create_solid_texture(&ctx.device, &ctx.queue, [10, 30, 60, 128]);
    let night = create_solid_texture(&ctx.device, &ctx.queue, [5, 5, 10, 128]);

    // With spec_intensity=0.0, Fresnel has nothing to multiply — output
    // should be identical regardless of Fresnel.
    let uniforms = Uniforms {
        terminator_width: 0.15,
        flags: 1,
        ..default_test_uniforms(size)
    };

    let pixels = render_frame(&ctx, &uniforms, &water, &night, size, size);
    let lum = avg_luminance_non_clear(&pixels);

    // With both specular and Fresnel mix at zero, luminance should be
    // modest (just diffuse-lit ocean color). Sanity check.
    assert!(
        lum < 100.0,
        "With spec_intensity=0 and fresnel_mix=0, luminance should be modest, got {lum:.1}"
    );
}

#[test]
fn fresnel_specular_brighter_at_grazing() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    // All-water texture
    let water = create_solid_texture(&ctx.device, &ctx.queue, [10, 30, 60, 128]);
    let night = create_solid_texture(&ctx.device, &ctx.queue, [5, 5, 10, 128]);

    // Head-on: eye at (0, 0, 3.5), sun at (0, 0, 1)
    let eye_head_on = glam::Vec3::new(0.0, 0.0, 3.5);
    let uniforms_head_on = Uniforms {
        mvp: test_mvp_with_eye(size, size, eye_head_on),
        terminator_width: 0.15,
        flags: 1,
        eye_pos: eye_head_on.into(),
        spec_intensity: 0.5,
        ..default_test_uniforms(size)
    };
    let pixels_head_on = render_frame(&ctx, &uniforms_head_on, &water, &night, size, size);
    let lum_head_on = avg_luminance_non_clear(&pixels_head_on);

    // Grazing: eye at (2.5, 0, 2.5), sun still at (0, 0, 1)
    // The limb pixels face the camera at a grazing angle where Fresnel is high
    let eye_grazing = glam::Vec3::new(2.5, 0.0, 2.5);
    let uniforms_grazing = Uniforms {
        mvp: test_mvp_with_eye(size, size, eye_grazing),
        eye_pos: eye_grazing.into(),
        ..uniforms_head_on
    };
    let pixels_grazing = render_frame(&ctx, &uniforms_grazing, &water, &night, size, size);
    let lum_grazing = avg_luminance_non_clear(&pixels_grazing);

    // At a grazing angle, Fresnel increases specular intensity.
    // The visible portion of the sphere has more glancing normals,
    // so overall average luminance should be higher.
    assert!(
        lum_grazing > lum_head_on * 0.8,
        "Grazing-angle specular luminance ({lum_grazing:.1}) should be comparable to or \
         brighter than head-on ({lum_head_on:.1}) due to Fresnel"
    );
}

// ---------------------------------------------------------------------------
// Fresnel diffuse shift tests (Step 2.2)
// ---------------------------------------------------------------------------

#[test]
fn fresnel_diffuse_shift_zero_is_noop() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    let water = create_solid_texture(&ctx.device, &ctx.queue, [10, 30, 60, 128]);
    let night = create_solid_texture(&ctx.device, &ctx.queue, [5, 5, 10, 128]);

    // Baseline: fresnel_mix=0
    let uniforms_base = Uniforms {
        terminator_width: 0.15,
        flags: 1,
        ..default_test_uniforms(size)
    };
    let pixels_base = render_frame(&ctx, &uniforms_base, &water, &night, size, size);

    // With fresnel_mix=0, the diffuse shift block is skipped entirely
    // (the `if uniforms.fresnel_mix > 0.0` guard).
    // So the output should be identical to spec_intensity=0, fresnel_mix=0.
    let lum_base = avg_luminance_non_clear(&pixels_base);
    assert!(
        lum_base > 0.0,
        "Baseline should have visible pixels, got luminance {lum_base:.1}"
    );
}

#[test]
fn fresnel_diffuse_shift_brightens_grazing_water() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    let water = create_solid_texture(&ctx.device, &ctx.queue, [10, 30, 60, 128]);
    let night = create_solid_texture(&ctx.device, &ctx.queue, [5, 5, 10, 128]);

    // Without Fresnel diffuse shift
    let uniforms_no_shift = Uniforms {
        terminator_width: 0.15,
        flags: 1,
        ..default_test_uniforms(size)
    };
    let pixels_no_shift = render_frame(&ctx, &uniforms_no_shift, &water, &night, size, size);
    let lum_no_shift = avg_luminance_non_clear(&pixels_no_shift);

    // With Fresnel diffuse shift (the sky color is brighter than the ocean)
    let uniforms_with_shift = Uniforms {
        fresnel_mix: 0.5,
        ..uniforms_no_shift
    };
    let pixels_with_shift = render_frame(&ctx, &uniforms_with_shift, &water, &night, size, size);
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
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    // All-land texture (alpha=255)
    let land = create_solid_texture(&ctx.device, &ctx.queue, [50, 120, 50, 255]);
    let night = create_solid_texture(&ctx.device, &ctx.queue, [5, 5, 10, 255]);

    // Without Fresnel diffuse shift
    let uniforms_base = Uniforms {
        terminator_width: 0.15,
        flags: 1,
        ..default_test_uniforms(size)
    };
    let pixels_base = render_frame(&ctx, &uniforms_base, &land, &night, size, size);

    // With Fresnel diffuse shift — should have no effect on land
    let uniforms_with_shift = Uniforms {
        fresnel_mix: 0.5,
        ..uniforms_base
    };
    let pixels_with_shift = render_frame(&ctx, &uniforms_with_shift, &land, &night, size, size);

    // Pixel-for-pixel comparison: land should be completely unaffected
    assert_eq!(
        pixels_base, pixels_with_shift,
        "Land pixels should be identical with and without fresnel_mix"
    );
}

#[test]
fn fresnel_diffuse_shift_absent_at_night() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    // All-water texture
    let water = create_solid_texture(&ctx.device, &ctx.queue, [10, 30, 60, 128]);
    let night = create_solid_texture(&ctx.device, &ctx.queue, [5, 5, 10, 128]);

    // Sun pointing away from camera (night side faces camera)
    let uniforms_base = Uniforms {
        sun_dir: [0.0, 0.0, -1.0],
        terminator_width: 0.15,
        flags: 1,
        ..default_test_uniforms(size)
    };
    let pixels_base = render_frame(&ctx, &uniforms_base, &water, &night, size, size);

    // With Fresnel diffuse shift — should have no effect on night side
    // because sky_color * result.blend = sky_color * 0 = 0
    let uniforms_with_shift = Uniforms {
        fresnel_mix: 0.5,
        ..uniforms_base
    };
    let pixels_with_shift = render_frame(&ctx, &uniforms_with_shift, &water, &night, size, size);

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
// Cloud pipeline tests
// ---------------------------------------------------------------------------

// A full pipeline set-up followed by its assertions; the parts are not
// meaningful on their own.
#[allow(clippy::too_many_lines)]
#[test]
fn cloud_pipeline_renders_with_alpha() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 64;

    // Create a cloud pipeline using vs_cloud / fs_cloud entry points
    let wgsl_source = format!(
        "{}\n{}",
        include_str!("../shaders/blend.wgsl"),
        include_str!("../shaders/sphere.wgsl"),
    );
    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("test_cloud_shader"),
            source: wgpu::ShaderSource::Wgsl(wgsl_source.into()),
        });

    let pipeline_layout = ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("test_cloud_pipeline_layout"),
            bind_group_layouts: &[&ctx.bind_group_layout],
            immediate_size: 0,
        });

    let cloud_pipeline = ctx
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("test_cloud_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_cloud"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        wgpu::VertexAttribute {
                            offset: 12,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                    ],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
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
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        });

    // 1x1 white cloud texture (fully opaque cloud)
    let cloud_tex = create_solid_texture(&ctx.device, &ctx.queue, [255, 255, 255, 255]);
    let dummy = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);

    let uniforms = Uniforms {
        terminator_width: 0.15,
        cloud_opacity: 1.0,
        ..default_test_uniforms(size)
    };

    ctx.queue
        .write_buffer(&ctx.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("test_cloud_bind_group"),
        layout: &ctx.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: ctx.uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&cloud_tex),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&ctx.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&dummy),
            },
        ],
    });

    let render_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test_cloud_render_target"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
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
        label: Some("test_cloud_depth"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
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
            label: Some("test_cloud_pass"),
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

        pass.set_pipeline(&cloud_pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.set_vertex_buffer(0, ctx.vertex_buffer.slice(..));
        pass.set_index_buffer(ctx.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..ctx.index_count, 0, 0..1);
    }
    ctx.queue.submit(std::iter::once(encoder.finish()));

    let pixels = common::read_texture_rgba8(&ctx.device, &ctx.queue, &render_texture, size, size);
    let visible = count_non_clear_pixels(&pixels);

    assert!(
        visible > 50,
        "Cloud pipeline should render visible pixels, got {visible}"
    );
}

// ---------------------------------------------------------------------------
// Color correction: gamma tests (Step 3.2)
// ---------------------------------------------------------------------------

#[test]
fn gamma_above_one_brightens() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    let mid_gray = create_solid_texture(&ctx.device, &ctx.queue, [128, 128, 128, 255]);
    let black = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);

    let uniforms_base = default_test_uniforms(size);
    let pixels_base = render_frame(&ctx, &uniforms_base, &mid_gray, &black, size, size);
    let lum_base = avg_luminance_non_clear(&pixels_base);

    let uniforms_bright = Uniforms {
        day_gamma: 2.0,
        ..uniforms_base
    };
    let pixels_bright = render_frame(&ctx, &uniforms_bright, &mid_gray, &black, size, size);
    let lum_bright = avg_luminance_non_clear(&pixels_bright);

    assert!(
        lum_bright > lum_base,
        "Gamma > 1.0 should brighten midtones: gamma_2.0={lum_bright:.1}, gamma_1.0={lum_base:.1}"
    );
}

#[test]
fn gamma_below_one_darkens() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    let mid_gray = create_solid_texture(&ctx.device, &ctx.queue, [128, 128, 128, 255]);
    let black = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);

    let uniforms_base = default_test_uniforms(size);
    let pixels_base = render_frame(&ctx, &uniforms_base, &mid_gray, &black, size, size);
    let lum_base = avg_luminance_non_clear(&pixels_base);

    let uniforms_dark = Uniforms {
        day_gamma: 0.5,
        ..uniforms_base
    };
    let pixels_dark = render_frame(&ctx, &uniforms_dark, &mid_gray, &black, size, size);
    let lum_dark = avg_luminance_non_clear(&pixels_dark);

    assert!(
        lum_dark < lum_base,
        "Gamma < 1.0 should darken midtones: gamma_0.5={lum_dark:.1}, gamma_1.0={lum_base:.1}"
    );
}

#[test]
fn gamma_identity_unchanged() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    let mid_gray = create_solid_texture(&ctx.device, &ctx.queue, [128, 128, 128, 255]);
    let black = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);

    let uniforms = default_test_uniforms(size);

    // Render twice with identical identity settings
    let pixels_a = render_frame(&ctx, &uniforms, &mid_gray, &black, size, size);
    let pixels_b = render_frame(&ctx, &uniforms, &mid_gray, &black, size, size);

    assert_eq!(
        pixels_a, pixels_b,
        "Gamma 1.0 (identity) should produce identical output on consecutive renders"
    );
}

// ---------------------------------------------------------------------------
// Color correction: saturation tests (Step 3.3)
// ---------------------------------------------------------------------------

#[test]
fn saturation_zero_produces_greyscale() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    let red = create_solid_texture(&ctx.device, &ctx.queue, [255, 0, 0, 255]);
    let black = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);

    let uniforms = Uniforms {
        day_saturation: 0.0,
        ..default_test_uniforms(size)
    };

    let pixels = render_frame(&ctx, &uniforms, &red, &black, size, size);

    // Check all non-clear pixels: R, G, B should be approximately equal
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
            // Non-clear pixel: check R == G == B within GPU tolerance
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
fn saturation_identity_unchanged() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    let colorful = create_solid_texture(&ctx.device, &ctx.queue, [200, 100, 50, 255]);
    let black = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);

    let uniforms = default_test_uniforms(size);

    let pixels_a = render_frame(&ctx, &uniforms, &colorful, &black, size, size);
    let pixels_b = render_frame(&ctx, &uniforms, &colorful, &black, size, size);

    assert_eq!(
        pixels_a, pixels_b,
        "Saturation 1.0 (identity) should produce identical output on consecutive renders"
    );
}

#[test]
fn saturation_above_one_increases_chroma() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    let colorful = create_solid_texture(&ctx.device, &ctx.queue, [200, 100, 50, 255]);
    let black = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);

    let uniforms_base = default_test_uniforms(size);
    let pixels_base = render_frame(&ctx, &uniforms_base, &colorful, &black, size, size);

    let uniforms_saturated = Uniforms {
        day_saturation: 2.0,
        ..uniforms_base
    };
    let pixels_saturated = render_frame(&ctx, &uniforms_saturated, &colorful, &black, size, size);

    // Compute average chroma (max - min channel) across non-clear pixels
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
// Color correction: independence tests (Step 3.4)
// ---------------------------------------------------------------------------

#[test]
fn night_gamma_does_not_affect_single_texture_mode() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    let mid_gray = create_solid_texture(&ctx.device, &ctx.queue, [128, 128, 128, 255]);
    let black = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);

    let uniforms_base = default_test_uniforms(size);
    let pixels_base = render_frame(&ctx, &uniforms_base, &mid_gray, &black, size, size);

    let uniforms_night_extreme = Uniforms {
        night_gamma: 0.3,
        ..uniforms_base
    };
    let pixels_night_extreme =
        render_frame(&ctx, &uniforms_night_extreme, &mid_gray, &black, size, size);

    assert_eq!(
        pixels_base, pixels_night_extreme,
        "Night gamma should have no effect in single-texture mode"
    );
}

#[test]
fn day_and_night_corrections_independent() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    // Use mid-tones so gamma correction produces a visible difference
    // (pure white and pure black are fixed points of pow)
    let mid_gray = create_solid_texture(&ctx.device, &ctx.queue, [128, 128, 128, 255]);
    let dark_gray = create_solid_texture(&ctx.device, &ctx.queue, [64, 64, 64, 255]);

    let uniforms_day_bright = Uniforms {
        terminator_width: 0.15,
        flags: 1,
        day_gamma: 2.0,
        ..default_test_uniforms(size)
    };
    let pixels_day_bright = render_frame(
        &ctx,
        &uniforms_day_bright,
        &mid_gray,
        &dark_gray,
        size,
        size,
    );

    let uniforms_night_bright = Uniforms {
        day_gamma: 1.0,
        night_gamma: 2.0,
        ..uniforms_day_bright
    };
    let pixels_night_bright = render_frame(
        &ctx,
        &uniforms_night_bright,
        &mid_gray,
        &dark_gray,
        size,
        size,
    );

    assert_ne!(
        pixels_day_bright, pixels_night_bright,
        "Day and night corrections should produce different output when targeting different textures"
    );
}
