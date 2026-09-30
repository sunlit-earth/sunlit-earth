use super::Renderer;
use super::slots::TextureMode;
use super::surface::{LayerState, SurfaceGroup, SurfaceLayer};
use super::textures::{maybe_spawn_texture_load, resolve_render_index};

/// Result of texture resolution: identifies which bind group to use.
pub(super) enum ResolvedTexture {
    /// Use the composite (day+night) bind group in blend mode.
    Composite,
    /// Use a single-texture slot bind group. The `usize` is the slot index.
    Slot(usize),
    /// Use one of the cube surface's bind groups.
    Surface(SurfaceGroup),
}

impl ResolvedTexture {
    /// Whether the globe reads this bind group through the cube rather than
    /// through the mesh's coordinates: the surface and the grid do, the flat
    /// maps do not.
    pub(super) fn draws_from_a_cube(&self) -> bool {
        matches!(self, Self::Surface(_) | Self::Slot(0))
    }

    /// Whether the cube this bind group draws alone is the night floor.
    pub(super) fn draws_the_night_alone(&self) -> bool {
        matches!(self, Self::Surface(SurfaceGroup::Night))
    }
}

/// Determine the bind group and blend mode for the current frame.
///
/// Spawns background texture loads as needed, resolves which bind group to use,
/// and returns a `ResolvedTexture` indicating the bind group along with whether
/// blend uniforms should be active.
pub(super) fn resolve_textures(res: &mut Renderer, mode: TextureMode) -> (ResolvedTexture, bool) {
    // With nothing of the surface to draw yet, the fallback below, whose slots
    // have no file behind them while the surface is in use.
    if let Some(surface) = &res.surface
        && let Some((group, use_blend)) = surface.route(mode)
    {
        return (ResolvedTexture::Surface(group), use_blend);
    }

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
///
/// With the cube surface in use, the day side counts the mask too in blend
/// mode, the one mode that reads it.
pub(super) fn loading_text(res: &Renderer, mode: TextureMode) -> String {
    let (day_loading, night_loading) = if let Some(surface) = &res.surface {
        let waiting = |layer| surface.state(layer) == LayerState::Waiting;
        (
            waiting(SurfaceLayer::Day(surface.month))
                || (mode == TextureMode::Blend && waiting(SurfaceLayer::Mask)),
            waiting(SurfaceLayer::Night),
        )
    } else {
        let layout = res.layout();
        (
            res.texture_slots[layout.globe(TextureMode::Day)].loading,
            res.texture_slots[layout.globe(TextureMode::Night)].loading,
        )
    };

    let loading = match mode {
        TextureMode::Grid => false,
        TextureMode::Day => day_loading,
        TextureMode::Night => night_loading,
        TextureMode::Blend => {
            return match (day_loading, night_loading) {
                (true, true) => "Loading Day and Night...".to_owned(),
                (true, false) => "Loading Day...".to_owned(),
                (false, true) => "Loading Night...".to_owned(),
                (false, false) => String::new(),
            };
        }
    };
    if loading {
        format!("Loading {}...", mode.label())
    } else {
        String::new()
    }
}
