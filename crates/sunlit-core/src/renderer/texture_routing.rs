use super::textures::{maybe_spawn_texture_load, resolve_render_index};
use super::{Renderer, TextureMode};

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
pub(super) fn resolve_textures(res: &mut Renderer, mode: TextureMode) -> (ResolvedTexture, bool) {
    let layout = res.layout();
    let day_slot = layout.globe(TextureMode::Day);
    let night_slot = layout.globe(TextureMode::Night);

    // Kick off background loading if needed
    if mode == TextureMode::Blend {
        maybe_spawn_texture_load(res, day_slot);
        maybe_spawn_texture_load(res, night_slot);
    } else {
        maybe_spawn_texture_load(res, layout.globe(mode));
    }

    // Cloud texture is populated by the cloud fetcher thread, not by
    // file-based texture loading. No need to call maybe_spawn_texture_load.

    // Resolve which bind group to use
    if mode == TextureMode::Blend {
        if res.composite_bind_group.is_some() {
            (ResolvedTexture::Composite, true)
        } else {
            let fallback_index = resolve_render_index(res, day_slot);
            (ResolvedTexture::Slot(fallback_index), false)
        }
    } else {
        let render_index = resolve_render_index(res, layout.globe(mode));
        (ResolvedTexture::Slot(render_index), false)
    }
}

/// The loading indicator text for the current texture selection.
pub(super) fn loading_text(res: &Renderer, mode: TextureMode) -> String {
    let layout = res.layout();
    let slot_index = layout.globe(mode);

    if mode == TextureMode::Blend {
        let day_loading = res.texture_slots[layout.globe(TextureMode::Day)].loading;
        let night_loading = res.texture_slots[layout.globe(TextureMode::Night)].loading;
        match (day_loading, night_loading) {
            (true, true) => "Loading Day and Night...".to_owned(),
            (true, false) => "Loading Day...".to_owned(),
            (false, true) => "Loading Night...".to_owned(),
            (false, false) => String::new(),
        }
    } else if res.texture_slots[slot_index].loading {
        format!("Loading {}...", mode.label())
    } else {
        String::new()
    }
}
