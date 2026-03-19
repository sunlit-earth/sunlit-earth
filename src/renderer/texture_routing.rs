use slint::Model;

use crate::MainWindow;

use super::GpuResources;
use super::textures::{maybe_spawn_texture_load, resolve_render_index};
use super::{BLEND_MODE_INDEX, CLOUDS_SLOT, DAY_SLOT, NIGHT_SLOT};

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
    res: &mut GpuResources,
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

    // Kick off cloud texture loading unconditionally
    if CLOUDS_SLOT < res.texture_slots.len() {
        maybe_spawn_texture_load(res, CLOUDS_SLOT);
    }

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

/// Update the loading indicator text on the UI.
pub(super) fn update_loading_text(
    win: &MainWindow,
    res: &GpuResources,
    raw_index: usize,
) {
    let is_blend_mode = raw_index == BLEND_MODE_INDEX;
    let slot_index = raw_index.min(res.texture_slots.len().saturating_sub(1));

    if is_blend_mode {
        let day_loading = res.texture_slots[DAY_SLOT].loading;
        let night_loading = res.texture_slots[NIGHT_SLOT].loading;
        let text = match (day_loading, night_loading) {
            (true, true) => "Loading Day and Night...".to_owned(),
            (true, false) => "Loading Day...".to_owned(),
            (false, true) => "Loading Night...".to_owned(),
            (false, false) => String::new(),
        };
        win.set_loading_text(text.into());
    } else {
        let loading = res.texture_slots[slot_index].loading;
        if loading {
            let name = win
                .get_texture_options()
                .row_data(slot_index)
                .unwrap_or_default();
            win.set_loading_text(format!("Loading {name}...").into());
        } else {
            win.set_loading_text(slint::SharedString::default());
        }
    }
}
