//! The two subcommands that answer without a window.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, error, info};

use sunlit_core::assets::texture_loader;
use sunlit_core::config::AppConfig;
use sunlit_core::engine::wallpaper_sink::SystemWallpaper;
use sunlit_core::engine::{self, EngineCommand};
use sunlit_core::params::SceneParams;

use crate::cli::Cli;
use crate::displays;
use crate::startup::{engine_config, have_globe_texture};

/// How long the render subcommand waits for textures before exporting anyway.
const RENDER_TEXTURE_TIMEOUT: Duration = Duration::from_mins(2);

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
    let settings = sunlit_core::display::layout::Framing {
        camera_fov: params.camera.fov_deg,
        sky_fov: params.sky_fov,
        offset_x: params.camera.offset_x,
        offset_y: params.camera.offset_y,
    };
    print!(
        "{}",
        displays::report(
            &monitors,
            config.display_mode,
            config.anchor().as_deref(),
            settings
        )
    );
    let Some(dir) = out else {
        return ExitCode::SUCCESS;
    };

    texture_loader::register_jxl_hook();
    let (done_tx, done_rx) = crossbeam_channel::bounded::<Result<String, String>>(1);
    let mut engine_config = engine_config(cli, config, (640, 360), false);
    engine_config.preview_enabled = false;
    engine_config.wallpaper = Arc::new(displays::DirectorySink::new(
        dir.to_path_buf(),
        monitors.clone(),
    ));
    engine_config.on_event = Arc::new(move |event| {
        if let engine::EngineEvent::WallpaperSet(result) = event {
            let _ = done_tx.try_send(result.clone());
        }
    });

    let engine = match engine::start(engine_config) {
        Ok(engine) => engine,
        Err(e) => {
            error!("the renderer could not start: {e}");
            return ExitCode::FAILURE;
        }
    };
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
    let mut engine_config = engine_config(cli, config, (width, height), false);
    engine_config.on_event = Arc::new(move |event| {
        if matches!(event, engine::EngineEvent::TexturesReady) {
            let _ = ready_tx.try_send(());
        }
    });

    let have_globe = have_globe_texture(&engine_config.texture_paths);

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
