//! What every startup mode agrees on before it diverges: where the textures
//! are, which of them the globe needs, and the engine configuration built from
//! the flags and the stored config.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tracing::info;

use sunlit_core::assets::cloud_fetcher;
use sunlit_core::assets::cloud_source::HttpCloudSource;
use sunlit_core::assets::texture_loader;
use sunlit_core::config::{AppConfig, QualityTier};
use sunlit_core::engine::EngineConfig;
use sunlit_core::engine::clock::SystemClock;
use sunlit_core::engine::wallpaper_sink::SystemWallpaper;
use sunlit_core::params::SceneParams;
use sunlit_core::renderer;

use crate::cli::Cli;

/// Smaller than any of the four assets and far larger than a Git LFS pointer.
///
/// `textures/**` is Git LFS, and a checkout without the objects holds pointer
/// files of a couple of hundred bytes, which are there as far as anything that
/// only asks whether the file exists is concerned. Naming one is worse than
/// naming nothing: the decode fails and logs an error for a checkout that is
/// merely incomplete. Size is what tells the two apart, which is the same rule
/// the guest staging in `xtask` and the engine tests use.
const TEXTURE_MIN_BYTES: u64 = 64 * 1024;

/// Resolve the day, night, moon and Milky Way texture paths from the textures
/// directory.
///
/// In slot order after the grid, which is what the renderer's `SlotLayout`
/// expects: a path missing from disk is `None` and its slot stays empty rather
/// than moving the ones after it.
pub(crate) fn resolve_texture_paths(cli_dir: Option<&std::path::Path>) -> Vec<Option<PathBuf>> {
    let dir = texture_loader::resolve_textures_dir(cli_dir);
    let pick = |name: &str| {
        dir.as_ref()
            .map(|d| d.join(name))
            .filter(|p| std::fs::metadata(p).is_ok_and(|meta| meta.len() >= TEXTURE_MIN_BYTES))
    };
    let paths = vec![
        pick("world.topo.200405.jxl"),
        pick("BlackMarble_2016.jxl"),
        pick("lroc_color_poles_1k.jxl"),
        pick("milkyway_2020_4k.jxl"),
    ];
    // package-managers.yml reads this line, its wording and the `day` field, to
    // tell whether an installed command found its textures.
    info!(
        textures_dir = ?dir,
        day = ?paths[0],
        night = ?paths[1],
        moon = ?paths[2],
        milky_way = ?paths[3],
        "resolved texture paths"
    );
    paths
}

/// Whether any of the resolved paths is one of the globe's own maps.
///
/// `TexturesReady` is about the globe: `Renderer::textures_ready` asks for the
/// day and night maps and the overlays are excluded from it, so a directory
/// holding only the Moon's file or the panorama's has nothing for the wait in
/// `run_render` to wait for. The paths are in slot order after the grid, which
/// is why the slot is the index plus one.
pub(crate) fn have_globe_texture(paths: &[Option<PathBuf>]) -> bool {
    let layout = renderer::SlotLayout::new(paths.len());
    paths
        .iter()
        .enumerate()
        .any(|(index, path)| path.is_some() && layout.is_globe(index + 1))
}

/// The surface texture width this run uses.
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

    EngineConfig {
        force_software: cli.software_rendering,
        texture_paths: resolve_texture_paths(cli.textures_dir.as_deref()),
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
            dir.join("world.topo.200405.jxl"),
            vec![0_u8; usize::try_from(TEXTURE_MIN_BYTES).expect("a small threshold")],
        )
        .expect("write the asset stand-in");
        std::fs::write(
            dir.join("lroc_color_poles_1k.jxl"),
            b"version https://git-lfs.github.com/spec/v1\noid sha256:0\nsize 291720\n",
        )
        .expect("write the pointer stand-in");

        let paths = resolve_texture_paths(Some(dir.path()));
        assert!(paths[0].is_some(), "the day map is the asset here");
        assert_eq!(paths[1], None, "the night map is not in the directory");
        assert_eq!(paths[2], None, "the moon map is a pointer");
        assert_eq!(paths[3], None, "the panorama is not in the directory");
    }

    /// An overlay is not something to wait for.
    #[test]
    fn only_a_globe_texture_is_worth_waiting_for() {
        let path = || Some(PathBuf::from("stand-in.jxl"));
        assert!(!have_globe_texture(&[None, None, None, None]));
        assert!(!have_globe_texture(&[None, None, path(), None]), "the Moon");
        assert!(
            !have_globe_texture(&[None, None, None, path()]),
            "the panorama"
        );
        assert!(!have_globe_texture(&[None, None, path(), path()]), "both");
        assert!(
            have_globe_texture(&[path(), None, None, None]),
            "the day map"
        );
        assert!(
            have_globe_texture(&[None, path(), None, None]),
            "the night map"
        );
        assert!(have_globe_texture(&[path(), path(), path(), path()]));
    }
}
