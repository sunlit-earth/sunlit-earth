use super::Renderer;
use super::slots::TextureMode;
use super::surface::{LayerState, SurfaceGroup, SurfaceLayer};

/// Result of texture resolution: identifies which bind group to use.
pub(super) enum ResolvedTexture {
    /// The procedural grid, slot 0.
    Grid,
    /// One of the cube surface's bind groups.
    Surface(SurfaceGroup),
}

impl ResolvedTexture {
    /// Whether the cube this bind group draws alone is the night floor.
    pub(super) fn draws_the_night_alone(&self) -> bool {
        matches!(self, Self::Surface(SurfaceGroup::Night))
    }
}

/// Determine the bind group and blend mode for the current frame.
///
/// The surface's group for `mode` once what it draws is resident, and the grid
/// until then, in grid mode, and without a cube surface.
pub(super) fn resolve_textures(res: &Renderer, mode: TextureMode) -> (ResolvedTexture, bool) {
    res.surface
        .as_ref()
        .and_then(|surface| surface.route(mode))
        .map_or((ResolvedTexture::Grid, false), |(group, use_blend)| {
            (ResolvedTexture::Surface(group), use_blend)
        })
}

/// The loading indicator text for the current texture selection.
///
/// The day side counts the mask too in blend mode, the one mode that reads
/// it. Without a cube surface nothing is on its way.
pub(super) fn loading_text(res: &Renderer, mode: TextureMode) -> String {
    let Some(surface) = &res.surface else {
        return String::new();
    };
    let waiting = |layer| surface.state(layer) == LayerState::Waiting;
    let day_loading = waiting(SurfaceLayer::Day(surface.month))
        || (mode == TextureMode::Blend && waiting(SurfaceLayer::Mask));
    let night_loading = waiting(SurfaceLayer::Night);

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
