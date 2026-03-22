use std::sync::mpsc;

use slint::Image;

use crate::scene::camera::{CameraParams, OrbitalCamera, zoom_to_distance};

use super::GpuResources;
use super::frame::FrameState;
use super::uniforms::Uniforms;

/// Shading parameters for a render pass.
#[derive(Clone, Copy)]
pub(super) struct ShadingParams {
    pub sun_dir: glam::Vec3,
    pub use_blend: bool,
    pub terminator_width: f32,
    pub diffuse_shading: bool,
    pub diffuse_floor: f32,
    pub diffuse_ramp: f32,
    pub spec_shininess: f32,
    pub spec_intensity: f32,
    pub fresnel_mix: f32,
    pub fresnel_exp: f32,
    pub day_gamma: f32,
    pub day_saturation: f32,
    pub night_gamma: f32,
    pub night_saturation: f32,
    pub cloud_sphere_radius: f32,
    pub cloud_opacity: f32,
    pub cloud_floor: f32,
    pub cloud_gamma: f32,
    pub rayleigh_intensity: f32,
    pub rayleigh_sharpness: f32,
    pub nightglow_intensity: f32,
    pub nightglow_falloff: f32,
    pub nightglow_balance: f32,
    pub rayleigh_radius: f32,
    pub nightglow_orange_radius: f32,
    pub nightglow_green_radius: f32,
    pub rayleigh_haze: f32,
}

/// Texture views to render into. Decouples render pass encoding from
/// which textures are used (preview vs export).
pub(super) struct RenderTarget<'a> {
    color_view: &'a wgpu::TextureView,
    resolve_target: Option<&'a wgpu::TextureView>,
    depth_view: &'a wgpu::TextureView,
}

impl<'a> RenderTarget<'a> {
    /// Build a render target, resolving MSAA if present.
    ///
    /// `resolve_view` is the 1x sample-count texture that receives the
    /// resolved output. When MSAA views are provided, they become the
    /// primary color/depth attachments with `resolve_view` as the resolve
    /// target.
    pub fn new(
        resolve_view: &'a wgpu::TextureView,
        msaa_color_view: Option<&'a wgpu::TextureView>,
        depth_view: &'a wgpu::TextureView,
        msaa_depth_view: Option<&'a wgpu::TextureView>,
    ) -> Self {
        let (color_view, resolve_target) = match msaa_color_view {
            Some(msaa) => (msaa, Some(resolve_view)),
            None => (resolve_view, None),
        };
        Self {
            color_view,
            resolve_target,
            depth_view: msaa_depth_view.unwrap_or(depth_view),
        }
    }
}

/// Build a `Uniforms` struct and write it to the GPU buffer.
#[allow(clippy::cast_precision_loss)]
pub(super) fn write_uniforms(
    queue: &wgpu::Queue,
    uniform_buffer: &wgpu::Buffer,
    camera_params: &CameraParams,
    aspect: f32,
    shading: &ShadingParams,
) {
    let mut camera = OrbitalCamera::new(
        camera_params.longitude,
        camera_params.latitude,
        zoom_to_distance(camera_params.zoom),
    );
    camera.offset_x = camera_params.offset_x;
    camera.offset_y = camera_params.offset_y;
    camera.tilt_deg = camera_params.tilt_deg;
    camera.yaw_deg = camera_params.yaw_deg;
    camera.pitch_deg = camera_params.pitch_deg;
    let mvp = camera.mvp_matrix(aspect);
    let eye_pos = camera.eye_position();
    let uniforms = Uniforms {
        mvp: mvp.to_cols_array(),
        sun_dir: shading.sun_dir.into(),
        terminator_width: if shading.use_blend {
            shading.terminator_width
        } else {
            -1.0
        },
        flags: u32::from(shading.use_blend && shading.diffuse_shading),
        diffuse_floor: shading.diffuse_floor,
        diffuse_ramp: shading.diffuse_ramp,
        _pad: 0.0,
        eye_pos: eye_pos.into(),
        _pad2: 0.0,
        spec_shininess: shading.spec_shininess,
        spec_intensity: shading.spec_intensity,
        fresnel_mix: shading.fresnel_mix,
        fresnel_exp: shading.fresnel_exp,
        day_gamma: shading.day_gamma,
        day_saturation: shading.day_saturation,
        night_gamma: shading.night_gamma,
        night_saturation: shading.night_saturation,
        cloud_sphere_radius: shading.cloud_sphere_radius,
        cloud_opacity: shading.cloud_opacity,
        cloud_floor: shading.cloud_floor,
        cloud_gamma: shading.cloud_gamma,
        rayleigh_intensity: shading.rayleigh_intensity,
        rayleigh_sharpness: shading.rayleigh_sharpness,
        nightglow_intensity: shading.nightglow_intensity,
        nightglow_falloff: shading.nightglow_falloff,
        nightglow_balance: shading.nightglow_balance,
        rayleigh_radius: shading.rayleigh_radius,
        nightglow_orange_radius: shading.nightglow_orange_radius,
        nightglow_green_radius: shading.nightglow_green_radius,
        rayleigh_haze: shading.rayleigh_haze,
        _pad3: 0.0,
        _pad4: 0.0,
        _pad5: 0.0,
    };
    queue.write_buffer(uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
}

/// Encode and submit a render pass with the given target and bind group.
///
/// Draw order: Earth sphere, cloud overlay (alpha blended), Rayleigh scattering
/// (premultiplied alpha), nightglow orange (additive), nightglow green (additive).
/// Clouds draw before atmosphere because they're in the troposphere, well below
/// the scattering and airglow layers. All overlays reuse the already-bound
/// vertex and index buffers from the Earth draw.
#[allow(clippy::too_many_arguments)]
pub(super) fn encode_and_submit(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &RenderTarget,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    vertex_buffer: &wgpu::Buffer,
    index_buffer: &wgpu::Buffer,
    index_count: u32,
    rayleigh_pipeline: Option<&wgpu::RenderPipeline>,
    rayleigh_bind_group: Option<&wgpu::BindGroup>,
    nightglow_orange_pipeline: Option<&wgpu::RenderPipeline>,
    nightglow_orange_bind_group: Option<&wgpu::BindGroup>,
    nightglow_green_pipeline: Option<&wgpu::RenderPipeline>,
    nightglow_green_bind_group: Option<&wgpu::BindGroup>,
    cloud_pipeline: Option<&wgpu::RenderPipeline>,
    cloud_bind_group: Option<&wgpu::BindGroup>,
) {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("sphere_encoder"),
    });

    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sphere_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.color_view,
                depth_slice: None,
                resolve_target: target.resolve_target,
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
                view: target.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });

        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..index_count, 0, 0..1);

        // Cloud overlay (alpha blended, drawn before atmosphere so glow
        // layers render on top — clouds are in the troposphere, well below
        // the Rayleigh scattering and nightglow layers)
        if let (Some(cloud_pipe), Some(cloud_bg)) = (cloud_pipeline, cloud_bind_group) {
            pass.set_pipeline(cloud_pipe);
            pass.set_bind_group(0, cloud_bg, &[]);
            // Vertex and index buffers remain bound from the Earth draw
            pass.draw_indexed(0..index_count, 0, 0..1);
        }

        // Rayleigh scattering overlay (premultiplied alpha, simulates both
        // in-scattering and extinction at the limb)
        if let (Some(pipe), Some(bg)) = (rayleigh_pipeline, rayleigh_bind_group) {
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, bg, &[]);
            pass.draw_indexed(0..index_count, 0, 0..1);
        }

        // Nightglow orange overlay (additive, sodium D + FeO, ~1.014 radius)
        if let (Some(pipe), Some(bg)) = (nightglow_orange_pipeline, nightglow_orange_bind_group) {
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, bg, &[]);
            pass.draw_indexed(0..index_count, 0, 0..1);
        }

        // Nightglow green overlay (additive, OI 557.7nm, ~1.015 radius)
        if let (Some(pipe), Some(bg)) = (nightglow_green_pipeline, nightglow_green_bind_group) {
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, bg, &[]);
            pass.draw_indexed(0..index_count, 0, 0..1);
        }
    }

    queue.submit(std::iter::once(encoder.finish()));
}

/// Encode and submit the preview render pass, returning the Slint Image.
#[allow(clippy::cast_precision_loss)]
pub(super) fn execute_render_pass(
    res: &GpuResources,
    state: &FrameState,
    bind_group: &wgpu::BindGroup,
    shading: &ShadingParams,
) -> Image {
    let aspect = res.render_width as f32 / res.render_height as f32;

    let cam = CameraParams {
        longitude: state.longitude,
        latitude: state.latitude,
        zoom: state.zoom,
        offset_x: state.offset_x,
        offset_y: state.offset_y,
        tilt_deg: state.tilt,
        yaw_deg: state.yaw,
        pitch_deg: state.pitch,
    };
    write_uniforms(
        &res.queue,
        &res.uniform_buffer,
        &cam,
        aspect,
        shading,
    );

    let resolve_view = res
        .render_texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let target = RenderTarget::new(
        &resolve_view,
        res.msaa_texture_view.as_ref(),
        &res.depth_texture,
        res.msaa_depth_view.as_ref(),
    );

    // Only issue atmosphere draw calls when intensity > 0.
    // The atmosphere shells reuse the Earth's bind group (texture bindings
    // are present but ignored by the atmosphere fragment shaders).
    let (rayleigh_pipe, rayleigh_bg) = if shading.rayleigh_intensity > 0.0 {
        (Some(&res.rayleigh_pipeline), Some(bind_group))
    } else {
        (None, None)
    };
    let (nightglow_orange_pipe, nightglow_orange_bg) = if shading.nightglow_intensity > 0.0 {
        (Some(&res.nightglow_orange_pipeline), Some(bind_group))
    } else {
        (None, None)
    };
    let (nightglow_green_pipe, nightglow_green_bg) = if shading.nightglow_intensity > 0.0 {
        (Some(&res.nightglow_green_pipeline), Some(bind_group))
    } else {
        (None, None)
    };

    // Only issue the cloud draw call when the cloud texture has loaded
    // and the user has not disabled clouds (opacity > 0)
    let (cloud_pipe, cloud_bg) =
        if shading.cloud_opacity > 0.0 && res.cloud_bind_group.is_some() {
            (Some(&res.cloud_pipeline), res.cloud_bind_group.as_ref())
        } else {
            (None, None)
        };

    encode_and_submit(
        &res.device,
        &res.queue,
        &target,
        &res.pipeline,
        bind_group,
        &res.vertex_buffer,
        &res.index_buffer,
        res.index_count,
        rayleigh_pipe,
        rayleigh_bg,
        nightglow_orange_pipe,
        nightglow_orange_bg,
        nightglow_green_pipe,
        nightglow_green_bg,
        cloud_pipe,
        cloud_bg,
    );

    Image::try_from(res.render_texture.clone())
        .expect("Failed to convert wgpu texture to Slint image")
}

/// Read back a 2D `Rgba8Unorm` texture as raw RGBA8 pixel data.
///
/// Creates a staging buffer with 256-byte row alignment, copies the texture
/// into it, maps the buffer synchronously, and strips any row padding.
#[allow(clippy::cast_possible_truncation)]
pub fn read_texture_rgba8(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Vec<u8> {
    // bytes_per_row must be aligned to 256 for buffer-texture copies
    let bytes_per_row_unaligned = width * 4;
    let bytes_per_row = (bytes_per_row_unaligned + 255) & !255;
    let buffer_size = u64::from(bytes_per_row) * u64::from(height);

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).unwrap();
    });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().expect("buffer mapping failed");

    let mapped = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = (row * bytes_per_row) as usize;
        let end = start + (width * 4) as usize;
        pixels.extend_from_slice(&mapped[start..end]);
    }
    drop(mapped);
    readback.unmap();

    pixels
}
