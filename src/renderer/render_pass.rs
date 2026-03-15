use slint::Image;

use crate::scene::camera::OrbitalCamera;

use super::GpuResources;
use super::frame::FrameState;
use super::uniforms::Uniforms;

/// Encode and submit the render pass, returning the rendered Slint Image.
///
/// Builds the camera/MVP matrix, writes uniforms to the GPU buffer, encodes
/// the render pass with the given bind group, submits, and converts the
/// render texture to a Slint Image.
#[allow(clippy::cast_precision_loss, clippy::too_many_arguments)]
pub(super) fn execute_render_pass(
    res: &GpuResources,
    state: &FrameState,
    sun_dir: glam::Vec3,
    bind_group: &wgpu::BindGroup,
    use_blend_uniforms: bool,
    terminator_width_f: f32,
    diffuse_shading: bool,
    diffuse_floor_f: f32,
    diffuse_ramp_f: f32,
) -> Image {
    let camera = OrbitalCamera::new(
        state.longitude,
        state.latitude,
        state.zoom,
    );

    let aspect = res.render_width as f32 / res.render_height as f32;
    let mvp = camera.mvp_matrix(aspect);

    let uniforms = Uniforms {
        mvp: mvp.to_cols_array(),
        sun_dir: sun_dir.into(),
        terminator_width: if use_blend_uniforms {
            terminator_width_f
        } else {
            -1.0
        },
        flags: u32::from(use_blend_uniforms && diffuse_shading),
        diffuse_floor: diffuse_floor_f,
        diffuse_ramp: diffuse_ramp_f,
        _pad: 0.0,
    };
    res.queue
        .write_buffer(&res.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

    let mut encoder =
        res.device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("sphere_encoder"),
            });

    let resolve_view = res
        .render_texture
        .create_view(&wgpu::TextureViewDescriptor::default());

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
        pass.set_bind_group(0, bind_group, &[]);
        pass.set_vertex_buffer(0, res.vertex_buffer.slice(..));
        pass.set_index_buffer(res.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..res.index_count, 0, 0..1);
    }

    res.queue.submit(std::iter::once(encoder.finish()));

    Image::try_from(res.render_texture.clone())
        .expect("Failed to convert wgpu texture to Slint image")
}
