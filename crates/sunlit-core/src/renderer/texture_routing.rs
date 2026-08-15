use super::Renderer;
use super::textures::{maybe_spawn_texture_load, resolve_render_index};
use super::{BLEND_MODE_INDEX, DAY_SLOT, NIGHT_SLOT, TEXTURE_LABELS};

/// Result of texture resolution: identifies which bind group to use.
pub(super) enum ResolvedTexture {
    /// Use the composite (day+night) bind group in blend mode.
    Composite,
    /// Use a single-texture slot bind group. The `usize` is the slot index.
    Slot(usize),
}

/// Determine the bind group and blend mode for the current frame.
///
/// Spawns background texture loads as needed, resolves which bind group to use,
/// and returns a `ResolvedTexture` indicating the bind group along with whether
/// blend uniforms should be active.
pub(super) fn resolve_textures(
    res: &mut Renderer,
    raw_index: usize,
) -> (ResolvedTexture, bool) {
    let is_blend_mode = raw_index == BLEND_MODE_INDEX;
    let slot_index = raw_index.min(res.texture_slots.len().saturating_sub(1));

    // Kick off background loading if needed
    if is_blend_mode {
        maybe_spawn_texture_load(res, DAY_SLOT);
        maybe_spawn_texture_load(res, NIGHT_SLOT);
    } else {
        maybe_spawn_texture_load(res, slot_index);
    }

    // Cloud texture is populated by the cloud fetcher thread, not by
    // file-based texture loading. No need to call maybe_spawn_texture_load.

    // Resolve which bind group to use
    if is_blend_mode {
        if res.composite_bind_group.is_some() {
            (ResolvedTexture::Composite, true)
        } else {
            let fallback_index = resolve_render_index(res, DAY_SLOT);
            (ResolvedTexture::Slot(fallback_index), false)
        }
    } else {
        let render_index = resolve_render_index(res, slot_index);
        (ResolvedTexture::Slot(render_index), false)
    }
}

/// The loading indicator text for the current texture selection.
pub(super) fn loading_text(res: &Renderer, raw_index: usize) -> String {
    let slot_index = raw_index.min(res.texture_slots.len().saturating_sub(1));

    if raw_index == BLEND_MODE_INDEX {
        let day_loading = res.texture_slots[DAY_SLOT].loading;
        let night_loading = res.texture_slots[NIGHT_SLOT].loading;
        match (day_loading, night_loading) {
            (true, true) => "Loading Day and Night...".to_owned(),
            (true, false) => "Loading Day...".to_owned(),
            (false, true) => "Loading Night...".to_owned(),
            (false, false) => String::new(),
        }
    } else if res.texture_slots[slot_index].loading {
        let name = TEXTURE_LABELS.get(slot_index).copied().unwrap_or_default();
        format!("Loading {name}...")
    } else {
        String::new()
    }
}
