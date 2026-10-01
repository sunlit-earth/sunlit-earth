//! What every startup mode agrees on before it diverges: where the textures
//! are, and the engine configuration built from the flags and the stored
//! config.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tracing::info;

use sunlit_core::assets::cloud_fetcher;
use sunlit_core::assets::cloud_source::HttpCloudSource;
use sunlit_core::assets::cube_layout::CubeTextures;
use sunlit_core::assets::{texture_loader, tiles};
use sunlit_core::config::{AppConfig, QualityTier};
use sunlit_core::engine::clock::SystemClock;
use sunlit_core::engine::wallpaper_sink::SystemWallpaper;
use sunlit_core::engine::{EngineConfig, TileGate};
use sunlit_core::params::SceneParams;

use crate::cli::Cli;

/// Smaller than the Moon's and the Milky Way's assets and far larger than a
/// Git LFS pointer.
///
/// `textures/**` is Git LFS, and a checkout without the objects holds pointer
/// files of a couple of hundred bytes, which are there as far as anything that
/// only asks whether the file exists is concerned. Naming one is worse than
/// naming nothing: the decode fails and logs an error for a checkout that is
/// merely incomplete. Size is what tells the two apart, which is the same rule
/// the guest staging in `xtask` and the engine tests use.
const TEXTURE_MIN_BYTES: u64 = 64 * 1024;

/// What the textures directory holds for the engine.
pub(crate) struct ResolvedTextures {
    /// The Moon's and the Milky Way's paths, in slot order after the grid.
    pub(crate) paths: Vec<Option<PathBuf>>,
    /// The cube faces, which the globe is drawn from when every one is there.
    pub(crate) cube: CubeTextures,
}

/// Resolve the Moon's and the Milky Way's paths and the cube faces from the
/// textures directory.
///
/// The paths are in slot order after the grid, which is what the renderer's
/// `SlotLayout` expects: a path missing from disk is `None` and its slot stays
/// empty rather than moving the ones after it.
pub(crate) fn resolve_textures(cli_dir: Option<&std::path::Path>) -> ResolvedTextures {
    let dir = texture_loader::resolve_textures_dir(cli_dir);
    let pick = |name: &str| {
        dir.as_ref()
            .map(|d| d.join(name))
            .filter(|p| std::fs::metadata(p).is_ok_and(|meta| meta.len() >= TEXTURE_MIN_BYTES))
    };
    let paths = vec![
        pick("lroc_color_poles_1k.jxl"),
        pick("milkyway_2020_4k.jxl"),
    ];
    let cube = dir
        .as_deref()
        .map(CubeTextures::resolve)
        .unwrap_or_default();
    info!(
        textures_dir = ?dir,
        moon = ?paths[0],
        milky_way = ?paths[1],
        cube_files = cube.found(),
        cube_complete = cube.is_complete(),
        "resolved texture paths"
    );
    ResolvedTextures { paths, cube }
}

/// The resolution setting this run uses.
///
/// The flag wins over the stored setting, and unlike `--quality` the choice has
/// a widget, so the window shows what the engine actually loaded rather than
/// what is on disk. Keeping the override out of the config file then takes the
/// one-run flag on `EngineLink`, which is what a save consults instead of the
/// combo box until the user picks a width themselves.
pub(crate) fn effective_texture_resolution(cli: &Cli, config: &AppConfig) -> u32 {
    cli.texture_resolution
        .map_or(config.texture_resolution, u32::from)
}

/// Build the engine configuration shared by every startup mode.
pub(crate) fn engine_config(
    cli: &Cli,
    config: &AppConfig,
    preview_size: (u32, u32),
    preview_enabled: bool,
) -> EngineConfig {
    let quality = cli.quality.map_or(config.quality_tier, QualityTier::from);
    info!(?quality, "quality tier");
    let texture_resolution = effective_texture_resolution(cli, config);

    // Skip cloud fetching entirely when SUNLIT_EARTH_NO_CLOUDS is set; e2e
    // tests use it to keep the network out of the picture.
    let cloud = if std::env::var("SUNLIT_EARTH_NO_CLOUDS").is_ok() {
        info!("cloud fetcher disabled (SUNLIT_EARTH_NO_CLOUDS)");
        None
    } else {
        let url = cloud_fetcher::cloud_url(texture_resolution);
        info!(url = %url, "cloud source configured");
        Some(Arc::new(HttpCloudSource::new(url)) as Arc<_>)
    };

    let textures = resolve_textures(cli.textures_dir.as_deref());
    EngineConfig {
        force_software: cli.software_rendering,
        texture_paths: textures.paths,
        cube_textures: textures.cube,
        tile_geometry: tiles::GEOMETRY,
        tile_layers: None,
        tile_gate: TileGate::default(),
        preview_size,
        preview_enabled,
        params: SceneParams::from_config(config),
        quality,
        texture_resolution,
        clock: Arc::new(SystemClock::new()),
        cloud,
        cloud_poll_interval: cloud_fetcher::poll_interval(),
        cache_dir: cloud_fetcher::cache_dir(),
        auto_refresh: config
            .auto_refresh_enabled
            .then(|| Duration::from_mins(u64::from(config.auto_refresh_interval_minutes.max(1)))),
        wallpaper: Arc::new(SystemWallpaper),
        display_mode: config.display_mode,
        anchor_monitor: config.anchor(),
        on_event: Arc::new(|_| {}),
        record_metrics: true,
        mailbox: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ScratchDir;

    /// A pointer file exists, so `exists()` is not the question to ask.
    #[test]
    fn a_git_lfs_pointer_is_not_a_texture_path() {
        let dir = ScratchDir::new("texture_pointers");
        std::fs::write(
            dir.join("milkyway_2020_4k.jxl"),
            vec![0_u8; usize::try_from(TEXTURE_MIN_BYTES).expect("a small threshold")],
        )
        .expect("write the asset stand-in");
        std::fs::write(
            dir.join("lroc_color_poles_1k.jxl"),
            b"version https://git-lfs.github.com/spec/v1\noid sha256:0\nsize 291720\n",
        )
        .expect("write the pointer stand-in");

        let paths = resolve_textures(Some(dir.path())).paths;
        assert_eq!(paths[0], None, "the moon map is a pointer");
        assert!(paths[1].is_some(), "the panorama is the asset here");
    }
}
