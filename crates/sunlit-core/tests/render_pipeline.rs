//! GPU integration tests for the full render pipeline (vertex + fragment shaders).
//!
//! Tests the production `sphere.wgsl` and `blend.wgsl` shaders end-to-end,
//! rendering to a small offscreen texture and asserting behavioral invariants.

mod common;

use std::sync::{LazyLock, Mutex};

use wgpu::util::DeviceExt;

use sunlit_core::geometry::sphere::{Vertex, generate_uv_sphere};
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
        _pad7: 0.0,
        _pad8: 0.0,
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

    assert!(
        center_lum > 50.0,
        "Center of day-lit sphere should be bright, got luminance {center_lum:.1}"
    );
}

#[test]
fn single_texture_mode_ignores_the_night_side() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    let red = create_solid_texture(&ctx.device, &ctx.queue, [255, 0, 0, 255]);
    let green = create_solid_texture(&ctx.device, &ctx.queue, [0, 255, 0, 255]);
    let black = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);

    // A negative terminator width is single-texture mode, where the fragment
    // shader returns before it reaches the night binding.
    let uniforms = default_test_uniforms(size);
    let pixels_green_night = render_frame(&ctx, &uniforms, &red, &green, size, size);

    let extreme = Uniforms {
        night_gamma: 0.3,
        night_saturation: 0.0,
        ..uniforms
    };
    let pixels_black_night = render_frame(&ctx, &extreme, &red, &black, size, size);

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
}
";

/// What the probe writes into each slot, in the order it writes them: the
/// value the Rust struct below carries in that field, and the field's name.
///
/// One row per value rather than one per field, so a field that moves is named
/// by the row that fails instead of hiding inside a wider assertion.
const PROBED_FIELDS: [(f32, &str); 75] = [
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
];

#[allow(clippy::too_many_lines)]
#[test]
fn uniform_buffer_field_offsets_match_wgsl() {
    let ctx = RENDER_CTX.lock().unwrap();

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
        _pad7: 0.0,
        _pad8: 0.0,
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

/// Three rules exist once in WGSL and once in `scene::sun_occlusion`, and every
/// pairing matters at the pixel; `docs/rendering.md` says which pairing is
/// which.
///
/// The viewport heights avoid 1080 and below, where the ramp clamps to 1.0 and
/// any two knees agree: every golden and every engine frame renders there, so
/// nothing else in the suite can see a divergence at all.
#[test]
#[allow(clippy::too_many_lines)]
fn the_shader_and_the_cpu_agree_on_the_three_shared_rules() {
    let ctx = RENDER_CTX.lock().unwrap();

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
        let cpu_scale = sunlit_core::scene::sun_occlusion::pixel_scale(height);
        let cpu_edge = sunlit_core::scene::sun_occlusion::sky_lens_edge_radius(sky_fov);
        assert!(
            (values[0] - cpu_scale).abs() < 1e-6,
            "the density ramp at {height} pixels: the shader says {}, \
             scene::sun_occlusion::pixel_scale says {cpu_scale}",
            values[0]
        );
        assert!(
            (values[1] - cpu_edge).abs() < 2e-5 * cpu_edge,
            "the sky lens edge radius at {sky_fov} degrees: the shader says {}, \
             scene::sun_occlusion::sky_lens_edge_radius says {cpu_edge}",
            values[1]
        );
        for (index, km) in RULE_HEIGHTS_KM.iter().enumerate() {
            let cpu = sunlit_core::scene::sun_occlusion::limb_transmission(*km, reddening);
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
                 {shader}, scene::sun_occlusion::limb_transmission says {cpu}"
            );
            let cpu_fade = sunlit_core::scene::sun_occlusion::limb_disk_amplitude(cpu.y);
            assert!(
                (values[5 + index * 4] - cpu_fade).abs() < 1e-3,
                "the disk's fade at {km} km: the shader says {}, \
                 scene::sun_occlusion::limb_disk_amplitude says {cpu_fade}",
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

    let ctx = RENDER_CTX.lock().unwrap();

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
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    // All-water texture (alpha=128): RGB can be ocean-like
    let water = create_solid_texture(&ctx.device, &ctx.queue, [10, 30, 60, 128]);
    let night = create_solid_texture(&ctx.device, &ctx.queue, [5, 5, 10, 128]);

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
    let pixels_closed = render_frame(&ctx, &closed, &water, &night, size, size);
    let pixels_varied = render_frame(&ctx, &varied, &water, &night, size, size);
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
    let pixels_open = render_frame(&ctx, &open, &water, &night, size, size);
    assert_ne!(
        pixels_closed, pixels_open,
        "opening both gates should change the frame"
    );
}

#[test]
fn fresnel_specular_brighter_at_grazing() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    let water = create_solid_texture(&ctx.device, &ctx.queue, [10, 30, 60, 128]);
    let night = create_solid_texture(&ctx.device, &ctx.queue, [5, 5, 10, 128]);

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
        let pixels_dark = render_frame(&ctx, &dark, &water, &night, size, size);
        let pixels_lit = render_frame(&ctx, &lit, &water, &night, size, size);
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
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    let water = create_solid_texture(&ctx.device, &ctx.queue, [10, 30, 60, 128]);
    let night = create_solid_texture(&ctx.device, &ctx.queue, [5, 5, 10, 128]);

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
                module: &ctx.shader,
                entry_point: Some("vs_cloud"),
                buffers: &[Vertex::buffer_layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &ctx.shader,
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

    let pixels = common::read_texture_rgba8(&ctx.device, &ctx.queue, &render_texture, size, size)
        .expect("reading the render target back");
    let visible = count_non_clear_pixels(&pixels);

    assert!(
        visible > 50,
        "Cloud pipeline should render visible pixels, got {visible}"
    );
}

// ---------------------------------------------------------------------------
// Color correction: gamma tests
// ---------------------------------------------------------------------------

#[test]
fn gamma_moves_midtones_in_both_directions() {
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

    let uniforms_dark = Uniforms {
        day_gamma: 0.5,
        ..uniforms_base
    };
    let pixels_dark = render_frame(&ctx, &uniforms_dark, &mid_gray, &black, size, size);
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
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    let colorful = create_solid_texture(&ctx.device, &ctx.queue, [200, 100, 50, 255]);
    let black = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);

    let uniforms = default_test_uniforms(size);

    let pixels_a = render_frame(&ctx, &uniforms, &colorful, &black, size, size);
    let pixels_b = render_frame(&ctx, &uniforms, &colorful, &black, size, size);

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
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    let red = create_solid_texture(&ctx.device, &ctx.queue, [255, 0, 0, 255]);
    let black = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);

    let uniforms = Uniforms {
        day_saturation: 0.0,
        ..default_test_uniforms(size)
    };

    let pixels = render_frame(&ctx, &uniforms, &red, &black, size, size);

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
