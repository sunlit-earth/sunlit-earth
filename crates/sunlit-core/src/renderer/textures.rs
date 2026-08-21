use std::path::PathBuf;

use tracing::{error, info};

use crate::assets::mailbox::DecodedTextureMessage;
use crate::assets::{texture_cache, texture_loader};

/// Descriptor for a texture that can be loaded on demand.
pub(super) struct TextureSlot {
    /// The GPU bind group, populated on first use.
    pub bind_group: Option<wgpu::BindGroup>,
    /// Filesystem path to the texture file (`None` for procedural textures).
    pub source_path: Option<PathBuf>,
    /// `true` while a background thread is decoding this slot's texture.
    pub loading: bool,
}

/// Drain the mailbox of completed background texture decodes and create
/// GPU resources (mipmapped texture + bind group) for each one.
///
/// Sets `texture_dirty` and returns `true` if at least one decoded texture was
/// processed, signaling that a re-render is needed even if the frame state
/// hasn't changed.
pub(super) fn process_decoded_textures(res: &mut super::Renderer) -> bool {
    let messages = res.texture_mailbox.take_all();
    let received_any = !messages.is_empty();
    res.texture_dirty |= received_any;
    for msg in messages {
        match msg.result {
            Ok(img) => {
                let tex = create_mipmapped_texture(
                    &res.device,
                    &res.queue,
                    &format!("texture_slot_{}", msg.slot_index),
                    img.width,
                    img.height,
                    img.pixels,
                );
                // Flush staging buffers so they don't accumulate across textures
                let _ = res.device.poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                });
                info!(slot = msg.slot_index, "GPU texture created");
                crate::memory::log_memory_usage("after texture upload");
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
                error!(slot = msg.slot_index, error = %e, "texture decode failed");
                // Mark source_path as None so we don't retry
                res.texture_slots[msg.slot_index].source_path = None;
                res.texture_slots[msg.slot_index].loading = false;
            }
        }
    }
    received_any
}

/// Create the composite bind group if both day and night texture views are available.
pub(super) fn maybe_create_composite_bind_group(res: &mut super::Renderer) {
    if let (Some(day_view), Some(night_view)) = (&res.day_texture_view, &res.night_texture_view) {
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
pub(super) fn maybe_create_cloud_bind_group(res: &mut super::Renderer) {
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
pub(super) fn maybe_spawn_texture_load(res: &mut super::Renderer, slot_index: usize) {
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

    let mailbox = res.texture_mailbox.clone();
    let notify = std::sync::Arc::clone(&res.notify);
    let target_width = res.texture_resolution;
    let cache_dir = res.texture_cache_dir.clone();

    std::thread::spawn(move || {
        texture_loader::register_jxl_hook();
        let start = std::time::Instant::now();
        let result =
            match texture_cache::load_at_resolution(&path, target_width, cache_dir.as_deref()) {
                Ok(img) => {
                    info!(
                        width = img.width,
                        height = img.height,
                        path = %path.display(),
                        elapsed_secs = format_args!("{:.2}", start.elapsed().as_secs_f64()),
                        "loaded texture"
                    );
                    crate::memory::log_memory_usage("after texture decode");
                    Ok(img)
                }
                Err(e) => Err(e),
            };

        // Park the result for the consumer to pick up, then wake it
        mailbox.post(DecodedTextureMessage { slot_index, result });
        notify();
    });
}

/// Determine which texture slot to render with: the requested slot if loaded,
/// otherwise `last_rendered_index` as a fallback.
pub(super) fn resolve_render_index(res: &mut super::Renderer, slot_index: usize) -> usize {
    if res.texture_slots[slot_index].bind_group.is_some() {
        res.last_rendered_index = slot_index;
        slot_index
    } else {
        res.last_rendered_index
    }
}

/// Create a texture from RGBA8 pixel data with CPU-generated mipmaps.
///
/// Takes ownership of `rgba_pixels` to avoid a 128 MB clone for 8K textures.
/// The buffer is reused in-place for mipmap downsampling.
#[tracing::instrument(skip(device, queue, rgba_pixels), fields(label, width, height))]
pub(super) fn create_mipmapped_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    rgba_pixels: Vec<u8>,
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
    upload_mip(queue, &texture, 0, width, height, &rgba_pixels);
    crate::memory::log_memory_usage("mipmap: after level 0 upload");

    // Generate subsequent mip levels by box-filtering the previous level.
    // We take ownership of the pixel buffer to avoid cloning 128 MB for 8K textures.
    let mut pixels = rgba_pixels;
    let mut w = width;
    let mut h = height;
    for level in 1..mip_count {
        pixels = texture_loader::downsample_2x(&pixels, w, h);
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        upload_mip(queue, &texture, level, w, h, &pixels);
    }
    crate::memory::log_memory_usage("mipmap: after all levels");

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
