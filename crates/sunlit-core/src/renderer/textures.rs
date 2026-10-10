use std::path::PathBuf;

use tracing::{debug, error, info};

use crate::assets::mailbox::DecodedTextureMessage;
use crate::assets::texture_loader;

/// Descriptor for a texture that can be loaded on demand.
pub(super) struct TextureSlot {
    /// The GPU bind group, populated on first use.
    pub bind_group: Option<wgpu::BindGroup>,
    /// The texture the bind group samples.
    ///
    /// Held rather than let go of after its view is made, so that a resolution
    /// switch can call `Texture::destroy` and release the allocation at a known
    /// moment instead of whenever the last derived view happens to drop.
    pub texture: Option<wgpu::Texture>,
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
    let mut applied_any = false;
    for msg in messages {
        if !is_current(msg.generation, res.texture_generation) {
            debug!(
                slot = msg.slot_index,
                generation = ?msg.generation,
                current = res.texture_generation,
                "discarding a decode from a superseded texture resolution"
            );
            // Deliberately nothing else. `loading` says a decode is on its way
            // to this slot, and a discarded post is never that decode: a post is
            // discarded only when the generation has moved on, which happens
            // exactly in `set_texture_resolution`, which purges every
            // file-backed slot in the same breath. So the flag was either
            // cleared there or has since been set by the reload, and clearing it
            // here would say a live load is not running, which puts a second
            // decode of the same source in flight beside the first.
            continue;
        }
        applied_any = true;
        match msg.result {
            Ok(img) => {
                let tex = create_mipmapped_texture(
                    &res.device,
                    &res.queue,
                    &res.slot_label(msg.slot_index),
                    img,
                );
                // Flush staging buffers so they don't accumulate across textures
                let _ = res.device.poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                });
                info!(slot = msg.slot_index, "GPU texture created");
                crate::memory::log_memory_usage("after texture upload");
                let tex_view = tex.create_view(&wgpu::TextureViewDescriptor::default());
                let bind_group =
                    res.flat_bind_group(&tex_view, &format!("bind_group_slot_{}", msg.slot_index));
                let slot = &mut res.texture_slots[msg.slot_index];
                slot.bind_group = Some(bind_group);
                slot.loading = false;
                // Dropping the previous handle here, not destroying it: the
                // bind group being replaced below may still reference it, and
                // wgpu's own refcount frees it once nothing does. Destroying is
                // for the purge, where the point is to release before the
                // replacement is allocated.
                slot.texture = Some(tex);

                if msg.slot_index == res.layout().clouds() {
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
    res.texture_dirty |= applied_any;
    applied_any
}

/// Whether a decoded texture is still wanted.
///
/// A post with no generation is from a producer the resolution setting does not
/// govern and is always current; one that carries a generation is wanted only
/// while it is the generation in force.
fn is_current(posted: Option<u64>, current: u64) -> bool {
    posted.is_none_or(|g| g == current)
}

/// Free the textures of every file-backed slot and let them reload.
///
/// This is what makes a switch to a lower resolution lower the Milky Way's
/// memory: the bind groups that reference the old textures are cleared first,
/// so `Texture::destroy` has nothing left holding the allocation, and the
/// reload allocates only after that. The same nil-before-recreate order as
/// `gpu_setup::replace_render_textures`.
///
/// A slot is file-backed when it has a source path, which is exactly the slots
/// the resolution governs: the grid is procedural and the cloud overlay arrives
/// from the fetcher.
///
/// `last_state` is cleared so the next frame is drawn rather than skipped as
/// unchanged.
pub(super) fn purge_file_backed_slots(res: &mut super::Renderer) {
    res.last_resolved = None;
    res.last_state = None;

    for slot in &mut res.texture_slots {
        if slot.source_path.is_none() {
            continue;
        }
        slot.bind_group = None;
        // A decode of the old width may still be running. It is left to finish
        // and post; the generation it carries is what gets it discarded, and
        // clearing the flag is what lets the reload start now rather than
        // waiting for it.
        //
        // This is the only place a load that has not delivered stops being
        // recorded, and every generation change comes through here, which is
        // what lets the discard path leave the flag alone: a decode whose post
        // will be discarded had its flag cleared right here, in the same call
        // that made it stale.
        slot.loading = false;
        if let Some(texture) = slot.texture.take() {
            texture.destroy();
        }
    }
}

/// Create the cloud bind group if the cloud texture view is available.
pub(super) fn maybe_create_cloud_bind_group(res: &mut super::Renderer) {
    if let Some(cloud_view) = &res.cloud_texture_view {
        res.cloud_bind_group = Some(res.flat_bind_group(cloud_view, "cloud_bind_group"));
    }
}

/// Spawn a background thread to decode the texture for `slot_index` if
/// it is not already loaded or in flight.
pub(super) fn maybe_spawn_texture_load(res: &mut super::Renderer, slot_index: usize) {
    let slot = &res.texture_slots[slot_index];

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
    let generation = res.texture_generation;

    std::thread::spawn(move || {
        texture_loader::register_jxl_hook();
        let start = std::time::Instant::now();
        let result = texture_loader::load_capped(&path, target_width).inspect(|img| {
            info!(
                width = img.width,
                height = img.height,
                path = %path.display(),
                elapsed_secs = format_args!("{:.2}", start.elapsed().as_secs_f64()),
                "loaded texture"
            );
            crate::memory::log_memory_usage("after texture decode");
        });

        // Park the result for the consumer to pick up, then wake it
        mailbox.post(DecodedTextureMessage {
            slot_index,
            result,
            generation: Some(generation),
        });
        notify();
    });
}

/// Create a texture from decoded pixels with CPU-generated mipmaps:
/// `Rgba8Unorm` for an RGBA image and `R8Unorm` for one channel, which the
/// shader samples as red, the channel it reads.
///
/// Takes ownership of the image to avoid cloning a decoded texture. Each level
/// is made from the one before and replaces it in the same [`Pixels`], so the
/// frame stays one live frame in [`decoded_pixels`] until its last level has
/// been uploaded and it drops here.
///
/// [`Pixels`]: texture_loader::Pixels
/// [`decoded_pixels`]: texture_loader::decoded_pixels
#[tracing::instrument(skip(device, queue, image), fields(label, width = image.width, height = image.height))]
pub(super) fn create_mipmapped_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    image: texture_loader::DecodedImage,
) -> wgpu::Texture {
    let texture_loader::DecodedImage {
        mut pixels,
        width,
        height,
        channels,
    } = image;
    let mip_count = width.max(height).ilog2() + 1;
    let format = match channels {
        texture_loader::Channels::Rgba => wgpu::TextureFormat::Rgba8Unorm,
        texture_loader::Channels::Red => wgpu::TextureFormat::R8Unorm,
    };
    let bytes_per_pixel = format
        .block_copy_size(None)
        .expect("an uncompressed format has a texel size");

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
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    upload_mip(queue, &texture, 0, width, height, bytes_per_pixel, &pixels);
    crate::memory::log_memory_usage("mipmap: after level 0 upload");

    // Generate subsequent mip levels by box-filtering the previous level.
    let mut w = width;
    let mut h = height;
    for level in 1..mip_count {
        let next = texture_loader::downsample_2x_channels(&pixels, w, h, channels.count());
        pixels.replace(next);
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        upload_mip(queue, &texture, level, w, h, bytes_per_pixel, &pixels);
    }
    crate::memory::log_memory_usage("mipmap: after all levels");

    texture
}

/// What one bind group of the shared layout holds besides the uniforms.
///
/// A group that reads one flat texture, the Moon, the Milky Way or the
/// clouds, has the dummy cube in all three cube places. A group that draws the
/// globe from a cube, the grid's or the surface's, has the dummy 1x1 texture as
/// its flat texture and the surface sampler. Every group but the surface's has
/// the dummy tile array and the dummy page table, which draws the floor
/// everywhere.
pub(super) struct Bindings<'a> {
    /// Binding 1.
    pub texture: &'a wgpu::TextureView,
    /// Binding 2.
    pub sampler: &'a wgpu::Sampler,
    /// Bindings 4 to 6: the day or grid cube, the night cube and the mask.
    pub cubes: [&'a wgpu::TextureView; 3],
    /// Bindings 7 and 8: the tile array and the page table.
    pub tiles: [&'a wgpu::TextureView; 2],
}

/// Create a bind group of the shared layout.
pub(super) fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform_buffer: &wgpu::Buffer,
    bindings: &Bindings<'_>,
    label: &str,
) -> wgpu::BindGroup {
    let view = wgpu::BindingResource::TextureView;
    let [day_cube, night_cube, mask_cube] = bindings.cubes;
    let [tile_array, page_table] = bindings.tiles;
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
                resource: view(bindings.texture),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(bindings.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: view(day_cube),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: view(night_cube),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: view(mask_cube),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: view(tile_array),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: view(page_table),
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
    bytes_per_pixel: u32,
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
            bytes_per_row: Some(width * bytes_per_pixel),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_post_from_the_generation_in_force_is_current() {
        assert!(is_current(Some(0), 0));
        assert!(is_current(Some(7), 7));
    }

    /// The whole point: a decode of the previous width finishing after the
    /// switch must not land on top of its replacement.
    #[test]
    fn a_post_from_a_superseded_generation_is_not_current() {
        assert!(!is_current(Some(0), 1));
        assert!(!is_current(Some(3), 9));
    }

    /// A generation ahead of the current one cannot happen, but treating it as
    /// stale is the safe reading of "not the generation in force".
    #[test]
    fn a_post_from_an_unknown_later_generation_is_not_current() {
        assert!(!is_current(Some(2), 1));
    }

    /// The cloud fetcher posts without a generation, and its frames keep
    /// arriving across as many resolution switches as the user makes.
    #[test]
    fn a_post_without_a_generation_is_always_current() {
        for current in [0, 1, 42] {
            assert!(is_current(None, current));
        }
    }
}
