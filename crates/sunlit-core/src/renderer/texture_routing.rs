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

/// Whether a cube the frame needs for `mode` is on its way, the day side's
/// and the night side's.
///
/// The mask counts on the side the mode draws, when the mode or the cloud
/// layer reads it: the day's in blend and day mode, the night's in night mode.
/// Without a cube surface nothing is on its way.
pub(super) fn cubes_waiting(res: &Renderer, mode: TextureMode, clouds: bool) -> (bool, bool) {
    let Some(surface) = &res.surface else {
        return (false, false);
    };
    let waiting = |layer| surface.state(layer) == LayerState::Waiting;
    let mask = waiting(SurfaceLayer::Mask)
        && super::surface::needs(mode, surface.month, clouds).contains(&SurfaceLayer::Mask);
    let day = waiting(SurfaceLayer::Day(surface.month)) || (mode != TextureMode::Night && mask);
    let night = waiting(SurfaceLayer::Night) || (mode == TextureMode::Night && mask);
    match mode {
        TextureMode::Grid => (false, false),
        TextureMode::Day => (day, false),
        TextureMode::Night => (false, night),
        TextureMode::Blend => (day, night),
    }
}
