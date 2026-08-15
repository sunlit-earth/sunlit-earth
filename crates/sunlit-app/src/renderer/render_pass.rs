use std::sync::mpsc;

use slint::Image;

use sunlit_core::params::{
    CLOUD_SPHERE_RADIUS, NIGHTGLOW_GREEN_RADIUS, NIGHTGLOW_ORANGE_RADIUS, RAYLEIGH_RADIUS,
    SceneParams,
};
use sunlit_core::scene::camera::{OrbitalCamera, zoom_to_distance};

use super::GpuResources;
use super::uniforms::Uniforms;

/// The per-frame values that are not part of `SceneParams`: the sun direction
/// derived from the clock, and whether the resolved bind group carries both a
/// day and a night texture.
#[derive(Clone, Copy)]
pub(super) struct FrameInputs {
    pub sun_dir: glam::Vec3,
    pub use_blend: bool,
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
///
/// This is one of the two translation points for `SceneParams` (the other is
/// the Slint bridge in the app): everything the shader reads is derived here
/// and nowhere else.
#[allow(clippy::cast_precision_loss)]
pub(super) fn write_uniforms(
    queue: &wgpu::Queue,
    uniform_buffer: &wgpu::Buffer,
    params: &SceneParams,
    aspect: f32,
    inputs: &FrameInputs,
) {
    let cam = &params.camera;
    let mut camera = OrbitalCamera::new(cam.longitude, cam.latitude, zoom_to_distance(cam.zoom));
    camera.offset_x = cam.offset_x;
    camera.offset_y = cam.offset_y;
    camera.tilt_deg = cam.tilt_deg;
    camera.yaw_deg = cam.yaw_deg;
    camera.pitch_deg = cam.pitch_deg;
    let mvp = camera.mvp_matrix(aspect);
    let eye_pos = camera.eye_position();
    let uniforms = Uniforms {
        mvp: mvp.to_cols_array(),
        sun_dir: inputs.sun_dir.into(),
        terminator_width: if inputs.use_blend {
            params.terminator_width
        } else {
            -1.0
        },
        flags: u32::from(inputs.use_blend && params.diffuse_shading),
        diffuse_floor: params.diffuse_floor,
        diffuse_ramp: params.diffuse_ramp,
        _pad: 0.0,
        eye_pos: eye_pos.into(),
        _pad2: 0.0,
        spec_shininess: params.spec_shininess,
        spec_intensity: params.spec_intensity,
        fresnel_mix: params.fresnel_mix,
        fresnel_exp: params.fresnel_exp,
        day_gamma: params.day_gamma,
        day_saturation: params.day_saturation,
        night_gamma: params.night_gamma,
        night_saturation: params.night_saturation,
        cloud_sphere_radius: CLOUD_SPHERE_RADIUS,
        cloud_opacity: params.cloud_opacity,
        cloud_floor: params.cloud_floor,
        cloud_gamma: params.cloud_gamma,
        rayleigh_intensity: params.effective_rayleigh_intensity(),
        rayleigh_sharpness: params.rayleigh_sharpness,
        nightglow_intensity: params.effective_nightglow_intensity(),
        nightglow_falloff: params.nightglow_falloff,
        nightglow_balance: params.nightglow_balance,
        rayleigh_radius: RAYLEIGH_RADIUS,
        nightglow_orange_radius: NIGHTGLOW_ORANGE_RADIUS,
        nightglow_green_radius: NIGHTGLOW_GREEN_RADIUS,
        rayleigh_haze: params.rayleigh_haze,
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
#[tracing::instrument(level = "trace", skip_all, fields(width = res.render_width, height = res.render_height))]
pub(super) fn execute_render_pass(
    res: &GpuResources,
    params: &SceneParams,
    bind_group: &wgpu::BindGroup,
    inputs: &FrameInputs,
) -> Image {
    let aspect = res.render_width as f32 / res.render_height as f32;

    write_uniforms(&res.queue, &res.uniform_buffer, params, aspect, inputs);

    let resolve_view = res
        .render_texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let target = RenderTarget::new(
        &resolve_view,
        res.msaa_texture_view.as_ref(),
        &res.depth_texture,
        res.msaa_depth_view.as_ref(),
    );

    let overlays = Overlays::select(res, params, bind_group);
    encode_and_submit(
        &res.device,
        &res.queue,
        &target,
        &res.pipeline,
        bind_group,
        &res.vertex_buffer,
        &res.index_buffer,
        res.index_count,
        overlays.rayleigh.0,
        overlays.rayleigh.1,
        overlays.nightglow_orange.0,
        overlays.nightglow_orange.1,
        overlays.nightglow_green.0,
        overlays.nightglow_green.1,
        overlays.cloud.0,
        overlays.cloud.1,
    );

    Image::try_from(res.render_texture.clone())
        .expect("Failed to convert wgpu texture to Slint image")
}

/// Which optional overlay shells to draw for a frame, and with which bind
/// group. Shared by the preview pass and the wallpaper export so the two
/// cannot drift apart.
pub(super) struct Overlays<'a> {
    pub rayleigh: (Option<&'a wgpu::RenderPipeline>, Option<&'a wgpu::BindGroup>),
    pub nightglow_orange: (Option<&'a wgpu::RenderPipeline>, Option<&'a wgpu::BindGroup>),
    pub nightglow_green: (Option<&'a wgpu::RenderPipeline>, Option<&'a wgpu::BindGroup>),
    pub cloud: (Option<&'a wgpu::RenderPipeline>, Option<&'a wgpu::BindGroup>),
}

impl<'a> Overlays<'a> {
    /// Atmosphere shells reuse the Earth's bind group (their fragment shaders
    /// ignore the texture bindings). Clouds need their own, and only exist once
    /// the cloud texture has been uploaded.
    pub fn select(
        res: &'a GpuResources,
        params: &SceneParams,
        bind_group: &'a wgpu::BindGroup,
    ) -> Self {
        let atmo = |on: bool, pipe: &'a wgpu::RenderPipeline| {
            if on { (Some(pipe), Some(bind_group)) } else { (None, None) }
        };
        let rayleigh_on = params.effective_rayleigh_intensity() > 0.0;
        let nightglow_on = params.effective_nightglow_intensity() > 0.0;
        Self {
            rayleigh: atmo(rayleigh_on, &res.rayleigh_pipeline),
            nightglow_orange: atmo(nightglow_on, &res.nightglow_orange_pipeline),
            nightglow_green: atmo(nightglow_on, &res.nightglow_green_pipeline),
            cloud: if params.cloud_opacity > 0.0 && res.cloud_bind_group.is_some() {
                (Some(&res.cloud_pipeline), res.cloud_bind_group.as_ref())
            } else {
                (None, None)
            },
        }
    }
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
