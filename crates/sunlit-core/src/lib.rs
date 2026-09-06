//! Headless core of Sunlit Earth.
//!
//! Everything in this crate must run without a window and without Slint: scene
//! math, asset loading, the wgpu pipeline, and (from Step 4 on) the engine that
//! owns them. The Slint shell in `sunlit-app` is one client of this crate.

pub mod assets;
pub mod config;
pub mod desktop;
pub mod display;
pub mod engine;
mod files;
pub mod geometry;
pub mod memory;
pub mod memory_report;
pub mod params;
pub mod renderer;
pub mod scene;
#[cfg(test)]
mod test_support;
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

/// The directory this application keeps its own files in.
///
/// `%LOCALAPPDATA%\SunlitEarth` on Windows and `~/.local/share/SunlitEarth` on
/// Linux, which is what `dirs::data_local_dir` answers on each. The config
/// file, the cloud cache, the memory metrics and the published wallpapers all
/// live here and all ask here, so they cannot disagree about where "here" is.
/// Asked through `dirs` rather than through `LOCALAPPDATA` directly for the
/// same reason. `None` on a system with no local data directory.
pub fn app_data_dir() -> Option<std::path::PathBuf> {
    Some(dirs::data_local_dir()?.join("SunlitEarth"))
}
