//! The two subcommands that answer without a window.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Receiver;
use tracing::{debug, error, info, warn};

use sunlit_core::assets::texture_loader;
use sunlit_core::config::AppConfig;
use sunlit_core::engine::wallpaper_sink::SystemWallpaper;
use sunlit_core::engine::{self, EngineCommand};
use sunlit_core::params::SceneParams;

use crate::cli::Cli;
use crate::displays;
use crate::startup::engine_config;

/// How long the render subcommand waits for textures before exporting anyway.
const RENDER_TEXTURE_TIMEOUT: Duration = Duration::from_mins(2);

/// How long a one-shot command waits for the cloud layer once its textures are
/// in: an 8192 px map of about 5 MB on a slow link, and its decode in a debug
/// build.
const CLOUD_TIMEOUT: Duration = Duration::from_mins(1);

/// The `displays` subcommand: no window, no desktop, and no GPU unless asked.
///
/// It exists so that checking this feature on a borrowed two-screen machine is
/// thirty seconds rather than an afternoon, and so that a bug report can carry
/// the layout. Without `--out` nothing is rendered and no device is created at
/// all: the plan is a pure function of the monitor list, and the enumeration is
/// the half of this feature that no test on another machine can stand in for.
pub(crate) fn run_displays(
    cli: &Cli,
    config: &AppConfig,
    out: Option<&std::path::Path>,
) -> ExitCode {
    use sunlit_core::engine::wallpaper_sink::WallpaperSink;

    debug!("startup mode: displays");
    let monitors = match SystemWallpaper.monitors() {
        Ok(monitors) => monitors,
        Err(e) => {
            error!("this session's monitors could not be listed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let params = SceneParams::from_config(config);
    let settings = sunlit_core::display::layout::Framing::from(&params);
    print!(
        "{}",
        displays::report(
            &monitors,
            config.display_mode,
            config.anchor().as_deref(),
            settings
        )
    );
    #[cfg(target_os = "linux")]
    match sunlit_core::desktop::choose_current() {
        Ok(choice) => println!("setter: {}", choice.explanation()),
        Err(refusal) => println!("setter: none; {refusal}"),
    }
    let Some(dir) = out else {
        return ExitCode::SUCCESS;
    };

    texture_loader::register_jxl_hook();
    let (done_tx, done_rx) = crossbeam_channel::bounded::<Result<String, String>>(1);
    let (clouds_tx, clouds_rx) = crossbeam_channel::bounded::<bool>(1);
    let mut engine_config = engine_config(cli, config, (640, 360), false);
    engine_config.preview_enabled = false;
    engine_config.wallpaper = Arc::new(displays::DirectorySink::new(
        dir.to_path_buf(),
        monitors.clone(),
    ));
    engine_config.on_event = Arc::new(move |event| match event {
        engine::EngineEvent::WallpaperSet(result) => {
            let _ = done_tx.try_send(result.clone());
        }
        engine::EngineEvent::CloudsSettled { drawn } => {
            let _ = clouds_tx.try_send(drawn);
        }
        _ => {}
    });
    let clouds = waits_for_clouds(&engine_config);

    let engine = match engine::start(engine_config) {
        Ok(engine) => engine,
        Err(e) => {
            error!("the renderer could not start: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The publish holds for the textures and for the cloud layer's first
    // answer on its own, for a minute at the most. Waiting here as well is
    // what puts the outcome in the log before the render starts.
    if clouds {
        wait_for_clouds(&clouds_rx, CLOUD_TIMEOUT);
    }
    engine.send(EngineCommand::RenderWallpaperNow);
    let status = match done_rx.recv_timeout(RENDER_TEXTURE_TIMEOUT) {
        Ok(Ok(_)) => ExitCode::SUCCESS,
        Ok(Err(e)) => {
            error!("the plan could not be rendered: {e}");
            ExitCode::FAILURE
        }
        Err(_) => {
            error!("the plan was not rendered within the timeout");
            ExitCode::FAILURE
        }
    };
    engine.shutdown();
    status
}

/// The `render` subcommand: no window, no Slint backend, no event loop.
pub(crate) fn run_render(
    cli: &Cli,
    config: &AppConfig,
    output: &std::path::Path,
    width: u32,
    height: u32,
) -> ExitCode {
    debug!("startup mode: render");
    texture_loader::register_jxl_hook();

    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<()>(1);
    let (clouds_tx, clouds_rx) = crossbeam_channel::bounded::<bool>(1);
    let mut engine_config = engine_config(cli, config, (width, height), false);
    engine_config.on_event = Arc::new(move |event| match event {
        engine::EngineEvent::TexturesReady => {
            let _ = ready_tx.try_send(());
        }
        engine::EngineEvent::CloudsSettled { drawn } => {
            let _ = clouds_tx.try_send(drawn);
        }
        _ => {}
    });
    let clouds = waits_for_clouds(&engine_config);

    // The cube surface is the globe's only texture, and on a first run the
    // wait below is what holds the render back until its first packs are built.
    let have_globe = engine_config.takes_cube();

    let engine = match engine::start(engine_config) {
        Ok(engine) => engine,
        Err(e) => {
            error!("the renderer could not start: {e}");
            return ExitCode::FAILURE;
        }
    };
    if have_globe {
        if ready_rx.recv_timeout(RENDER_TEXTURE_TIMEOUT).is_err() {
            error!("textures were not ready within the timeout, rendering anyway");
        }
    } else {
        info!("no globe texture found, rendering the procedural grid");
    }
    // An overlay the readiness above does not cover, and without the wait a
    // warm tile cache usually exports before the cached map is uploaded. An
    // export is not a publish, so the engine's own wait for the clouds does
    // not apply to it.
    if clouds {
        wait_for_clouds(&clouds_rx, CLOUD_TIMEOUT);
    }

    let status = match engine.render_to_file(output.to_path_buf(), width, height) {
        Ok(()) => {
            info!("render saved to {}", output.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            error!("render failed: {e}");
            ExitCode::FAILURE
        }
    };

    sunlit_core::memory::log_memory_usage("before exit");
    engine.shutdown();
    debug!("exiting");
    status
}

/// What the wait for the cloud layer came to.
#[derive(Debug, PartialEq, Eq)]
enum CloudWait {
    Drawn,
    /// No cloud map was cached and the first poll failed.
    Unavailable,
    TimedOut,
}

/// Whether a one-shot command with this configuration waits for the cloud
/// layer: one is drawn and there is a source to draw it from. Without either
/// the engine never says the clouds settled, so waiting would only time out.
fn waits_for_clouds(config: &engine::EngineConfig) -> bool {
    config.cloud.is_some() && config.params.draws_clouds()
}

/// Wait for the engine's `CloudsSettled`, which `settled` carries, saying in
/// the log why the picture goes out without clouds when it does.
fn wait_for_clouds(settled: &Receiver<bool>, timeout: Duration) -> CloudWait {
    match settled.recv_timeout(timeout) {
        Ok(true) => CloudWait::Drawn,
        Ok(false) => {
            warn!("no cloud map is cached and the first poll failed, rendering without clouds");
            CloudWait::Unavailable
        }
        Err(_) => {
            warn!(
                timeout_secs = timeout.as_secs(),
                "the cloud layer did not arrive within the timeout, rendering without clouds"
            );
            CloudWait::TimedOut
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: Duration = Duration::from_millis(20);

    #[test]
    fn a_drawn_cloud_layer_ends_the_wait() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        tx.send(true).unwrap();
        assert_eq!(wait_for_clouds(&rx, SHORT), CloudWait::Drawn);
    }

    #[test]
    fn a_cloud_layer_with_nothing_to_show_ends_the_wait() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        tx.send(false).unwrap();
        assert_eq!(wait_for_clouds(&rx, SHORT), CloudWait::Unavailable);
    }

    #[test]
    fn silence_ends_the_wait_at_the_timeout() {
        let (_tx, rx) = crossbeam_channel::bounded::<bool>(1);
        let start = std::time::Instant::now();
        assert_eq!(wait_for_clouds(&rx, SHORT), CloudWait::TimedOut);
        assert!(start.elapsed() >= SHORT);
    }

    #[test]
    fn no_wait_without_a_drawn_cloud_layer_or_a_source() {
        let mut config = engine::EngineConfig::headless((64, 64));
        config.params.clouds_enabled = true;
        config.params.cloud_opacity = 0.5;
        assert!(!waits_for_clouds(&config), "no source");

        config.cloud = Some(Arc::new(
            sunlit_core::assets::cloud_source::HttpCloudSource::new(
                "http://127.0.0.1:9/clouds.jpg".to_owned(),
            ),
        ));
        assert!(waits_for_clouds(&config));
        config.params.clouds_enabled = false;
        assert!(!waits_for_clouds(&config), "switched off");
        config.params.clouds_enabled = true;
        config.params.cloud_opacity = 0.0;
        config.params.cloud_opacity_night = 0.0;
        assert!(!waits_for_clouds(&config), "both opacities at zero");
    }
}
