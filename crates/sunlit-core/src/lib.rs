//! Headless core of Sunlit Earth.
//!
//! Everything in this crate must run without a window and without Slint: scene
//! math, asset loading, the wgpu pipeline, and (from Step 4 on) the engine that
//! owns them. The Slint shell in `sunlit-app` is one client of this crate.

pub mod assets;
pub mod config;
pub mod geometry;
pub mod memory;
pub mod params;
pub mod renderer;
pub mod scene;
#[cfg(windows)]
pub mod wallpaper;
pub mod wgpu_init;

/// Read an environment override, treating unset and blank values as absent.
///
/// Shared by every `SUNLIT_EARTH_*` knob that carries a value (paths, URLs,
/// intervals) so they all agree on what "not set" means. Presence-only flags
/// such as `SUNLIT_EARTH_NO_CLOUDS` do not use this and activate on any value.
pub fn env_override(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}
