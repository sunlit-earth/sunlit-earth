use std::path::PathBuf;

use slint::ComponentHandle;

use crate::texture_loader;

/// Descriptor for a texture that can be loaded on demand.
pub(super) struct TextureSlot {
    /// The GPU bind group, populated on first use.
    pub bind_group: Option<wgpu::BindGroup>,
    /// Filesystem path to the texture file (`None` for procedural textures).
    pub source_path: Option<PathBuf>,
    /// `true` while a background thread is decoding this slot's texture.
    pub loading: bool,
}

/// Message sent from a background decode thread when texture loading completes.
pub struct DecodedTextureMessage {
    pub slot_index: usize,
    pub result: Result<texture_loader::DecodedImage, String>,
}

/// Drain the channel for completed background texture decodes and create
/// GPU resources (mipmapped texture + bind group) for each one.
///
/// Returns `true` if at least one decoded texture was processed, signaling
/// that a re-render is needed even if the frame state hasn't changed.
pub(super) fn process_decoded_textures(res: &mut super::GpuResources) -> bool {
    let mut received_any = false;
    while let Ok(msg) = res.texture_rx.try_recv() {
        received_any = true;
        match msg.result {
            Ok(img) => {
                let tex = create_mipmapped_texture(
                    &res.device,
                    &res.queue,
                    &format!("texture_slot_{}", msg.slot_index),
                    img.width,
                    img.height,
                    &img.pixels,
                );
                let tex_view = tex.create_view(&wgpu::TextureViewDescriptor::default());
                let bind_group = create_bind_group(
                    &res.device,
                    &res.bind_group_layout,
                    &res.uniform_buffer,
                    &tex_view,
                    &res.sampler,
                    &res.dummy_texture_view,
                    &format!("bind_group_slot_{}", msg.slot_index),
                );
                res.texture_slots[msg.slot_index].bind_group = Some(bind_group);
                res.texture_slots[msg.slot_index].loading = false;

                // Store texture views for composite/cloud bind group creation
                if msg.slot_index == super::DAY_SLOT {
                    res.day_texture_view = Some(tex_view);
                    maybe_create_composite_bind_group(res);
                } else if msg.slot_index == super::NIGHT_SLOT {
                    res.night_texture_view = Some(tex_view);
                    maybe_create_composite_bind_group(res);
                } else if msg.slot_index == super::CLOUDS_SLOT {
                    res.cloud_texture_view = Some(tex_view);
                    maybe_create_cloud_bind_group(res);
                }
            }
            Err(e) => {
                eprintln!("{e}");
                // Mark source_path as None so we don't retry
                res.texture_slots[msg.slot_index].source_path = None;
                res.texture_slots[msg.slot_index].loading = false;
            }
        }
    }
    received_any
}

/// Create the composite bind group if both day and night texture views are available.
pub(super) fn maybe_create_composite_bind_group(res: &mut super::GpuResources) {
    if let (Some(day_view), Some(night_view)) =
        (&res.day_texture_view, &res.night_texture_view)
    {
        res.composite_bind_group = Some(create_bind_group(
            &res.device,
            &res.bind_group_layout,
            &res.uniform_buffer,
            day_view,
            &res.sampler,
            night_view,
            "composite_bind_group",
        ));
    }
}

/// Create the cloud bind group if the cloud texture view is available.
pub(super) fn maybe_create_cloud_bind_group(res: &mut super::GpuResources) {
    if let Some(cloud_view) = &res.cloud_texture_view {
        res.cloud_bind_group = Some(create_bind_group(
            &res.device,
            &res.bind_group_layout,
            &res.uniform_buffer,
            cloud_view,
            &res.sampler,
            &res.dummy_texture_view, // binding 3 unused by fs_cloud
            "cloud_bind_group",
        ));
    }
}

/// Spawn a background thread to decode the texture for `slot_index` if
/// it is not already loaded or in flight.
pub(super) fn maybe_spawn_texture_load(res: &mut super::GpuResources, slot_index: usize) {
    let slot = &res.texture_slots[slot_index];

    // Already loaded, already loading, or no source path — nothing to do
    if slot.bind_group.is_some() || slot.loading || slot.source_path.is_none() {
        return;
    }

    let path = res.texture_slots[slot_index]
        .source_path
        .clone()
        .expect("checked above");
    res.texture_slots[slot_index].loading = true;

    let tx = res.texture_tx.clone();
    let window_weak = res.window_weak.clone();

    std::thread::spawn(move || {
        texture_loader::register_jxl_hook();
        let start = std::time::Instant::now();
        let result = match texture_loader::load(&path) {
            Ok(img) => {
                eprintln!(
                    "Decoded texture ({}\u{d7}{}) from {} in {:.2}s",
                    img.width,
                    img.height,
                    path.display(),
                    start.elapsed().as_secs_f64(),
                );
                Ok(img)
            }
            Err(e) => Err(e),
        };

        // Send the result to the UI thread; ignore errors (receiver dropped on teardown)
        let _ = tx.send(DecodedTextureMessage { slot_index, result });

        // Wake the event loop so BeforeRendering fires and picks up the result
        let _ = window_weak.upgrade_in_event_loop(|win| {
            win.window().request_redraw();
        });
    });
}

/// Determine which texture slot to render with: the requested slot if loaded,
/// otherwise `last_rendered_index` as a fallback.
pub(super) fn resolve_render_index(res: &mut super::GpuResources, slot_index: usize) -> usize {
    if res.texture_slots[slot_index].bind_group.is_some() {
        res.last_rendered_index = slot_index;
        slot_index
    } else {
        res.last_rendered_index
    }
}

/// Create a texture from RGBA8 pixel data with CPU-generated mipmaps.
pub(super) fn create_mipmapped_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    rgba_pixels: &[u8],
) -> wgpu::Texture {
    let mip_count = width.max(height).ilog2() + 1;

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: mip_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    // Upload mip level 0
    upload_mip(queue, &texture, 0, width, height, rgba_pixels);

    // Generate subsequent mip levels by box-filtering the previous level
    let mut pixels = rgba_pixels.to_vec();
    let mut w = width;
    let mut h = height;
    for level in 1..mip_count {
        pixels = downsample_2x(&pixels, w, h);
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        upload_mip(queue, &texture, level, w, h, &pixels);
    }

    texture
}

/// Create a bind group with a uniform buffer, day texture, sampler, and night texture.
///
/// For single-texture modes (Grid, Day, Night), pass the dummy 1x1 texture
/// as `night_texture_view`. For blend mode, pass the actual night texture.
pub(super) fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform_buffer: &wgpu::Buffer,
    texture_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    night_texture_view: &wgpu::TextureView,
    label: &str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(texture_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(night_texture_view),
            },
        ],
    })
}

fn upload_mip(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    mip_level: u32,
    width: u32,
    height: u32,
    data: &[u8],
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

/// Box-filter downsample: average each 2x2 block of RGBA pixels.
#[allow(clippy::cast_possible_truncation)]
fn downsample_2x(src: &[u8], src_w: u32, src_h: u32) -> Vec<u8> {
    let dst_w = (src_w / 2).max(1) as usize;
    let dst_h = (src_h / 2).max(1) as usize;
    let sw = src_w as usize;
    let sh = src_h as usize;
    let mut dst = vec![0u8; dst_w * dst_h * 4];

    for y in 0..dst_h {
        for x in 0..dst_w {
            let sx = x * 2;
            let sy = y * 2;
            // Clamp neighbor coordinates to stay within source bounds
            let sx1 = (sx + 1).min(sw - 1);
            let sy1 = (sy + 1).min(sh - 1);
            for c in 0..4 {
                let tl = u16::from(src[(sy * sw + sx) * 4 + c]);
                let tr = u16::from(src[(sy * sw + sx1) * 4 + c]);
                let bl = u16::from(src[(sy1 * sw + sx) * 4 + c]);
                let br = u16::from(src[(sy1 * sw + sx1) * 4 + c]);
                dst[(y * dst_w + x) * 4 + c] = ((tl + tr + bl + br + 2) / 4) as u8;
            }
        }
    }

    dst
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // downsample_2x
    // -----------------------------------------------------------------------

    #[test]
    fn downsample_2x2_uniform_red() {
        let src = vec![255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255];
        let dst = downsample_2x(&src, 2, 2);
        assert_eq!(dst, [255, 0, 0, 255]);
    }

    #[test]
    fn downsample_2x2_checkerboard() {
        // tl=[0,0,0,255], tr=[100,0,0,255], bl=[0,100,0,255], br=[0,0,100,255]
        #[rustfmt::skip]
        let src = vec![
            0, 0, 0, 255,    100, 0, 0, 255,
            0, 100, 0, 255,  0, 0, 100, 255,
        ];
        let dst = downsample_2x(&src, 2, 2);
        assert_eq!(dst, [25, 25, 25, 255]);
    }

    #[test]
    fn downsample_4x4_uniform_white() {
        let src = vec![255; 4 * 4 * 4]; // 4x4 RGBA all-white
        let dst = downsample_2x(&src, 4, 4);
        assert_eq!(dst.len(), 2 * 2 * 4);
        for chunk in dst.chunks(4) {
            assert_eq!(chunk, [255, 255, 255, 255]);
        }
    }

    #[test]
    fn downsample_output_length() {
        for (w, h) in [(2, 2), (4, 4), (8, 6), (16, 2), (2, 16)] {
            let src = vec![128u8; (w * h * 4) as usize];
            let dst = downsample_2x(&src, w, h);
            let expected_len = ((w / 2).max(1) * (h / 2).max(1) * 4) as usize;
            assert_eq!(dst.len(), expected_len, "failed for ({w}, {h})");
        }
    }

    proptest::proptest! {
        #[test]
        fn downsample_output_size_invariant(
            half_w in 1u32..=64,
            half_h in 1u32..=64,
            pixels in proptest::collection::vec(proptest::num::u8::ANY, 1..=128*128*4),
        ) {
            let w = half_w * 2;
            let h = half_h * 2;
            let expected_input_len = (w as usize) * (h as usize) * 4;
            proptest::prop_assume!(pixels.len() >= expected_input_len);
            let src = &pixels[..expected_input_len];

            let dst = downsample_2x(src, w, h);
            let expected_output_len = (half_w as usize) * (half_h as usize) * 4;
            proptest::prop_assert_eq!(dst.len(), expected_output_len);
        }
    }
}
