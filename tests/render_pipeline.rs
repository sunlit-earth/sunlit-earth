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
    cloud_sphere_radius: f32,
    cloud_opacity: f32,
    _pad3: f32,
    _pad4: f32,
}

const _: () = assert!(std::mem::size_of::<Uniforms>() == 144);

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
            vertices.push(Vertex { position: [x, y, z], uv: [u, v] });
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

static RENDER_CTX: LazyLock<Mutex<RenderContext>> = LazyLock::new(|| {
    Mutex::new(create_render_context())
});

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
fn create_solid_texture(device: &wgpu::Device, queue: &wgpu::Queue, rgba: [u8; 4]) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("solid_texture"),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
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
        wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
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
    ctx.queue.write_buffer(&ctx.uniform_buffer, 0, bytemuck::cast_slice(&[*uniforms]));

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
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });

    let depth_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test_depth"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });

    let color_view = render_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());

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

    pixels.chunks(4).filter(|px| {
        let dr = px[0].abs_diff(clear_r);
        let dg = px[1].abs_diff(clear_g);
        let db = px[2].abs_diff(clear_b);
        dr > 1 || dg > 1 || db > 1
    }).count()
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
    { sum / count as f64 }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn sphere_renders_visible_pixels() {
    let ctx = RENDER_CTX.lock().unwrap();
    let size = 128;

    let white = create_solid_texture(&ctx.device, &ctx.queue, [255, 255, 255, 255]);
    let black = create_solid_texture(&ctx.device, &ctx.queue, [0, 0, 0, 255]);

    let uniforms = Uniforms {
        mvp: test_mvp(size, size),
        sun_dir: [0.0, 0.0, 1.0],
        terminator_width: -1.0, // single-texture mode
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
        cloud_sphere_radius: 1.0015,
        cloud_opacity: 0.0,
        _pad3: 0.0,
        _pad4: 0.0,
    };

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
        mvp: test_mvp(size, size),
        sun_dir: [0.0, 0.0, 1.0],
        terminator_width: 0.15,
        flags: 1, // diffuse enabled
        diffuse_floor: 0.1,
        diffuse_ramp: 0.6,
        _pad: 0.0,
        eye_pos: [0.0, 0.0, 3.5],
        _pad2: 0.0,
        spec_shininess: 150.0,
        spec_intensity: 0.0,
        fresnel_mix: 0.0,
        fresnel_exp: 5.0,
        cloud_sphere_radius: 1.0015,
        cloud_opacity: 0.0,
        _pad3: 0.0,
        _pad4: 0.0,
    };

    let pixels = render_frame(&ctx, &uniforms, &white, &dark_gray, size, size);

    // The sphere faces the camera (along +Z). Sun is also along +Z.
    // Center of the image should be brightly lit.
    let center_lum = avg_luminance_region(&pixels, size, size / 4, size / 4, 3 * size / 4, 3 * size / 4);

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

    let uniforms = Uniforms {
        mvp: test_mvp(size, size),
        sun_dir: [0.0, 0.0, 1.0],
        terminator_width: -1.0, // sentinel: single-texture mode
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
        cloud_sphere_radius: 1.0015,
        cloud_opacity: 0.0,
        _pad3: 0.0,
        _pad4: 0.0,
    };

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
    cloud_sphere_radius: f32,
    cloud_opacity: f32,
    _pad3: f32,
    _pad4: f32,
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
    // cloud params
    output[18] = uniforms.cloud_sphere_radius;
    output[19] = uniforms.cloud_opacity;
}
";

#[test]
fn uniform_buffer_field_offsets_match_wgsl() {
    let ctx = RENDER_CTX.lock().unwrap();

    let shader = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("uniform_readback_shader"),
        source: wgpu::ShaderSource::Wgsl(UNIFORM_READBACK_SHADER.into()),
    });

    let pipeline = ctx.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
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
        cloud_sphere_radius: 1.0015,
        cloud_opacity: 0.0,
        _pad3: 0.0,
        _pad4: 0.0,
    };

    let uniform_buf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("test_uniform_buf"),
        contents: bytemuck::cast_slice(&[uniforms]),
        usage: wgpu::BufferUsages::UNIFORM,
    });

    // Output buffer: 20 floats
    let output_size = (20 * std::mem::size_of::<f32>()) as u64;
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

    let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
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
    assert!((values[0] - 1.0).abs() < eps, "MVP[0][0]: got {}, expected 1.0", values[0]);
    assert!((values[1] - 2.0).abs() < eps, "MVP[1][1]: got {}, expected 2.0", values[1]);
    assert!((values[2] - 3.0).abs() < eps, "MVP[2][2]: got {}, expected 3.0", values[2]);
    assert!((values[3] - 4.0).abs() < eps, "MVP[3][3]: got {}, expected 4.0", values[3]);
    assert!((values[4] - 0.0).abs() < eps, "sun_dir.x: got {}, expected 0.0", values[4]);
    assert!((values[5] - 1.0).abs() < eps, "sun_dir.y: got {}, expected 1.0", values[5]);
    assert!((values[6] - 0.0).abs() < eps, "sun_dir.z: got {}, expected 0.0", values[6]);
    assert!((values[7] - 0.15).abs() < eps, "terminator_width: got {}, expected 0.15", values[7]);
    assert!((values[8] - 1.0).abs() < eps, "flags: got {}, expected 1.0", values[8]);
    assert!((values[9] - 0.5).abs() < eps, "diffuse_floor: got {}, expected 0.5", values[9]);
    assert!((values[10] - 0.25).abs() < eps, "diffuse_ramp: got {}, expected 0.25", values[10]);
    assert!((values[11] - 1.0).abs() < eps, "eye_pos.x: got {}, expected 1.0", values[11]);
    assert!((values[12] - 2.0).abs() < eps, "eye_pos.y: got {}, expected 2.0", values[12]);
    assert!((values[13] - 3.0).abs() < eps, "eye_pos.z: got {}, expected 3.0", values[13]);
    assert!((values[14] - 150.0).abs() < eps, "spec_shininess: got {}, expected 150.0", values[14]);
    assert!((values[15] - 0.75).abs() < eps, "spec_intensity: got {}, expected 0.75", values[15]);
    assert!((values[16] - 0.5).abs() < eps, "fresnel_mix: got {}, expected 0.5", values[16]);
    assert!((values[17] - 3.0).abs() < eps, "fresnel_exp: got {}, expected 3.0", values[17]);
    assert!((values[18] - 1.0015).abs() < eps, "cloud_sphere_radius: got {}, expected 1.0015", values[18]);
    assert!((values[19] - 0.0).abs() < eps, "cloud_opacity: got {}, expected 0.0", values[19]);
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
    if count == 0 { return 0.0; }
    #[allow(clippy::cast_precision_loss)]
    { sum / count as f64 }
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
        mvp: test_mvp(size, size),
        sun_dir: [0.0, 0.0, 1.0],
        terminator_width: 0.15,
        flags: 1,
        diffuse_floor: 0.1,
        diffuse_ramp: 0.6,
        _pad: 0.0,
        eye_pos: [0.0, 0.0, 3.5],
        _pad2: 0.0,
        spec_shininess: 150.0,
        spec_intensity: 0.0,
        fresnel_mix: 0.0,
        fresnel_exp: 5.0,
        cloud_sphere_radius: 1.0015,
        cloud_opacity: 0.0,
        _pad3: 0.0,
        _pad4: 0.0,
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
        sun_dir: [0.0, 0.0, 1.0],
        terminator_width: 0.15,
        flags: 1,
        diffuse_floor: 0.1,
        diffuse_ramp: 0.6,
        _pad: 0.0,
        eye_pos: eye_head_on.into(),
        _pad2: 0.0,
        spec_shininess: 150.0,
        spec_intensity: 0.5,
        fresnel_mix: 0.0,
        fresnel_exp: 5.0,
        cloud_sphere_radius: 1.0015,
        cloud_opacity: 0.0,
        _pad3: 0.0,
        _pad4: 0.0,
    };
    let pixels_head_on = render_frame(&ctx, &uniforms_head_on, &water, &night, size, size);
    let lum_head_on = avg_luminance_non_clear(&pixels_head_on);

    // Grazing: eye at (2.5, 0, 2.5), sun still at (0, 0, 1)
    // The limb pixels face the camera at a grazing angle where Fresnel is high
    let eye_grazing = glam::Vec3::new(2.5, 0.0, 2.5);
    let uniforms_grazing = Uniforms {
        mvp: test_mvp_with_eye(size, size, eye_grazing),
        sun_dir: [0.0, 0.0, 1.0],
        terminator_width: 0.15,
        flags: 1,
        diffuse_floor: 0.1,
        diffuse_ramp: 0.6,
        _pad: 0.0,
        eye_pos: eye_grazing.into(),
        _pad2: 0.0,
        spec_shininess: 150.0,
        spec_intensity: 0.5,
        fresnel_mix: 0.0,
        fresnel_exp: 5.0,
        cloud_sphere_radius: 1.0015,
        cloud_opacity: 0.0,
        _pad3: 0.0,
        _pad4: 0.0,
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
        mvp: test_mvp(size, size),
        sun_dir: [0.0, 0.0, 1.0],
        terminator_width: 0.15,
        flags: 1,
        diffuse_floor: 0.1,
        diffuse_ramp: 0.6,
        _pad: 0.0,
        eye_pos: [0.0, 0.0, 3.5],
        _pad2: 0.0,
        spec_shininess: 150.0,
        spec_intensity: 0.0,
        fresnel_mix: 0.0,
        fresnel_exp: 5.0,
        cloud_sphere_radius: 1.0015,
        cloud_opacity: 0.0,
        _pad3: 0.0,
        _pad4: 0.0,
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
        mvp: test_mvp(size, size),
        sun_dir: [0.0, 0.0, 1.0],
        terminator_width: 0.15,
        flags: 1,
        diffuse_floor: 0.1,
        diffuse_ramp: 0.6,
        _pad: 0.0,
        eye_pos: [0.0, 0.0, 3.5],
        _pad2: 0.0,
        spec_shininess: 150.0,
        spec_intensity: 0.0,
        fresnel_mix: 0.0,
        fresnel_exp: 5.0,
        cloud_sphere_radius: 1.0015,
        cloud_opacity: 0.0,
        _pad3: 0.0,
        _pad4: 0.0,
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
        mvp: test_mvp(size, size),
        sun_dir: [0.0, 0.0, 1.0],
        terminator_width: 0.15,
        flags: 1,
        diffuse_floor: 0.1,
        diffuse_ramp: 0.6,
        _pad: 0.0,
        eye_pos: [0.0, 0.0, 3.5],
        _pad2: 0.0,
        spec_shininess: 150.0,
        spec_intensity: 0.0,
        fresnel_mix: 0.0,
        fresnel_exp: 5.0,
        cloud_sphere_radius: 1.0015,
        cloud_opacity: 0.0,
        _pad3: 0.0,
        _pad4: 0.0,
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
        mvp: test_mvp(size, size),
        sun_dir: [0.0, 0.0, -1.0],
        terminator_width: 0.15,
        flags: 1,
        diffuse_floor: 0.1,
        diffuse_ramp: 0.6,
        _pad: 0.0,
        eye_pos: [0.0, 0.0, 3.5],
        _pad2: 0.0,
        spec_shininess: 150.0,
        spec_intensity: 0.0,
        fresnel_mix: 0.0,
        fresnel_exp: 5.0,
        cloud_sphere_radius: 1.0015,
        cloud_opacity: 0.0,
        _pad3: 0.0,
        _pad4: 0.0,
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
    let shader = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("test_cloud_shader"),
        source: wgpu::ShaderSource::Wgsl(wgsl_source.into()),
    });

    let pipeline_layout = ctx.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("test_cloud_pipeline_layout"),
        bind_group_layouts: &[&ctx.bind_group_layout],
        immediate_size: 0,
    });

    let cloud_pipeline = ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
        mvp: test_mvp(size, size),
        sun_dir: [0.0, 0.0, 1.0],
        terminator_width: 0.15,
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
        cloud_sphere_radius: 1.0015,
        cloud_opacity: 1.0,
        _pad3: 0.0,
        _pad4: 0.0,
    };

    ctx.queue.write_buffer(&ctx.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

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
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });

    let depth_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test_cloud_depth"),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });

    let color_view = render_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
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
