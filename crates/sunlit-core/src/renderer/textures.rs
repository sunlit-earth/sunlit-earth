use std::path::PathBuf;

use tracing::{debug, error, info};

use crate::assets::mailbox::DecodedTextureMessage;
use crate::assets::{texture_cache, texture_loader};

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
            // decode of the same 8K source in flight beside the first.
            continue;
        }
        applied_any = true;
        match msg.result {
            Ok(img) => {
                let tex = create_mipmapped_texture(
                    &res.device,
                    &res.queue,
                    &res.slot_label(msg.slot_index),
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
                let slot = &mut res.texture_slots[msg.slot_index];
                slot.bind_group = Some(bind_group);
                slot.loading = false;
                // Dropping the previous handle here, not destroying it: the
                // bind group being replaced below may still reference it, and
                // wgpu's own refcount frees it once nothing does. Destroying is
                // for the purge, where the point is to release before the
                // replacement is allocated.
                slot.texture = Some(tex);

                // Store texture views for composite/cloud bind group creation
                if msg.slot_index == super::DAY_SLOT {
                    res.day_texture_view = Some(tex_view);
                    maybe_create_composite_bind_group(res);
                } else if msg.slot_index == super::NIGHT_SLOT {
                    res.night_texture_view = Some(tex_view);
                    maybe_create_composite_bind_group(res);
                    // The cloud group samples this view too, and it was built
                    // with the dummy in that binding before the slot landed.
                    maybe_create_cloud_bind_group(res);
                } else if msg.slot_index == res.layout().clouds() {
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
/// This is what makes a switch to a lower resolution actually lower the
/// process's memory: the bind groups and views that reference the old textures
/// are cleared first, so `Texture::destroy` has nothing left holding the
/// allocation, and the reload allocates only after that. The same
/// nil-before-recreate order as `gpu_setup::replace_render_textures`.
///
/// A slot is file-backed when it has a source path, which is exactly the slots
/// the resolution governs: the grid is procedural and the cloud overlay arrives
/// from the fetcher.
///
/// `last_rendered_index` goes back to the grid because it is the one slot that
/// is always loaded, and `Renderer::render` treats it as a bind group that must
/// be there. `last_state` is cleared so the next frame is drawn rather than
/// skipped as unchanged.
pub(super) fn purge_file_backed_slots(res: &mut super::Renderer) {
    res.composite_bind_group = None;
    res.day_texture_view = None;
    res.night_texture_view = None;
    // The cloud slot is not file-backed, so the loop below leaves its group
    // alone, and that group holds the night view this line just dropped. A draw
    // through it after the destroy below is a validation error rather than a
    // wrong pixel, so it is rebuilt here with the dummy in that binding, and
    // again when the night slot lands.
    maybe_create_cloud_bind_group(res);
    res.last_resolved = None;
    res.last_state = None;
    res.last_rendered_index = 0;

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
///
/// Binding 3 is the night map, which `fs_cloud` samples for the city light a
/// cloud base picks up from below, and the dummy where there is none: in Grid
/// and Day modes, and in a checkout without the Git LFS objects, there is no
/// night texture and the dummy answers zero, which is the layer with the
/// coupling switched off.
///
/// So the group spans two slots with different lifetimes, and it has to be
/// rebuilt whenever either moves: when the night slot lands, and after the purge
/// destroys what it held.
pub(super) fn maybe_create_cloud_bind_group(res: &mut super::Renderer) {
    if let Some(cloud_view) = &res.cloud_texture_view {
        let night_view = res
            .night_texture_view
            .as_ref()
            .unwrap_or(&res.dummy_texture_view);
        res.cloud_bind_group = Some(create_bind_group(
            &res.device,
            &res.bind_group_layout,
            &res.uniform_buffer,
            cloud_view,
            &res.sampler,
            night_view,
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
    let generation = res.texture_generation;

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
        mailbox.post(DecodedTextureMessage {
            slot_index,
            result,
            generation: Some(generation),
        });
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
