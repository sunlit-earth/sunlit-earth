#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use slint::ComponentHandle;
use tracing::{debug, error, info, warn};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

use sunlit_core::assets::cloud_fetcher;
use sunlit_core::assets::cloud_source::HttpCloudSource;
use sunlit_core::assets::texture_loader;
use sunlit_core::config::{self, AppConfig, QualityTier};
use sunlit_core::display;
use sunlit_core::engine::clock::SystemClock;
use sunlit_core::engine::wallpaper_sink::SystemWallpaper;
use sunlit_core::engine::{self, EngineCommand, EngineConfig, EngineHandle};
use sunlit_core::params::SceneParams;
use sunlit_core::renderer;
use sunlit_core::scene::datetime;
use sunlit_earth::MainWindow;
use sunlit_earth::displays;
use sunlit_earth::engine_client::{self, EngineLink};
use sunlit_earth::ui_callbacks;

/// How long the render subcommand waits for textures before exporting anyway.
const RENDER_TEXTURE_TIMEOUT: Duration = Duration::from_mins(2);

/// How often the app checks whether the preview viewport changed size.
const VIEWPORT_POLL: Duration = Duration::from_millis(200);

/// Sunlit Earth: get a realistic 3D view of Earth as seen from space and set it as your wallpaper
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Force software rendering (CPU-based, no GPU required)
    #[arg(long)]
    software_rendering: bool,

    /// Quality tier, overriding the saved config [possible values: low, medium, high]
    ///
    /// Debug builds default to low, release builds to high.
    #[arg(long, value_enum)]
    quality: Option<Quality>,

    /// Surface texture width, overriding the saved config [possible values: 8192, 4096, 2048]
    ///
    /// Lower widths are downscaled from the 8K sources once and cached.
    #[arg(long, value_enum)]
    texture_resolution: Option<TextureResolution>,

    /// Path to the textures directory
    #[arg(long)]
    textures_dir: Option<PathBuf>,

    /// Log level [possible values: error, warn, info, debug, trace]
    ///
    /// Takes precedence over the `RUST_LOG` environment variable.
    #[arg(long)]
    log_level: Option<String>,

    /// Startup mode: tray (default, minimize-to-tray on close) or window (close exits)
    #[arg(long, value_enum, default_value_t = Mode::Tray)]
    mode: Mode,

    /// Initial window visibility in tray mode: visible (default) or hidden
    #[arg(long, value_enum, default_value_t = TrayStart::Visible)]
    tray_start: TrayStart,

    /// Name of a local IPC socket to listen on for control commands (quit, show-window, hide-window)
    #[arg(long)]
    ipc_socket: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

/// CLI mirror of `QualityTier`, so clap owns the parsing and core stays free
/// of a clap dependency.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Quality {
    Low,
    Medium,
    High,
}

impl From<Quality> for QualityTier {
    fn from(value: Quality) -> Self {
        match value {
            Quality::Low => Self::Low,
            Quality::Medium => Self::Medium,
            Quality::High => Self::High,
        }
    }
}

/// CLI mirror of the texture resolution setting, so clap rejects a width that
/// is not on offer instead of the app correcting it after the fact.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum TextureResolution {
    #[value(name = "8192")]
    Full,
    #[value(name = "4096")]
    Half,
    #[value(name = "2048")]
    Quarter,
}

impl From<TextureResolution> for u32 {
    fn from(value: TextureResolution) -> Self {
        match value {
            TextureResolution::Full => 8192,
            TextureResolution::Half => 4096,
            TextureResolution::Quarter => 2048,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Mode {
    Tray,
    Window,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum TrayStart {
    Visible,
    Hidden,
}

#[derive(Subcommand)]
enum Commands {
    /// Render the scene to a PNG file and exit.
    Render {
        /// Output file path
        #[arg(short, long, default_value = "render.png")]
        output: PathBuf,

        /// Image width in pixels
        #[arg(long, default_value_t = 1920)]
        width: u32,

        /// Image height in pixels
        #[arg(long, default_value_t = 1080)]
        height: u32,

        /// Path to a config file (TOML). If omitted, uses the saved user config.
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    /// Print the monitors this session has and the wallpaper plan they come to.
    Displays {
        /// Write the plan's images into this directory instead of the desktop.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

/// How loud each dependency is allowed to be, whatever this app's level is.
///
/// The decoders and the graphics stack log per frame and per tile at their own
/// default, which buries everything this program has to say.
const DEPENDENCY_LEVELS: [&str; 13] = [
    "wgpu_core=warn",
    "wgpu_hal=error",
    "naga=warn",
    "winit=warn",
    "ureq=warn",
    "ureq_proto=warn",
    "rustls=warn",
    "jxl_render=warn",
    "jxl_grid=warn",
    "jxl_modular=warn",
    "jxl_bitstream=warn",
    "jxl_frame=warn",
    "jxl_color=warn",
];

/// Initialize the global tracing subscriber.
///
/// `cli_level` is the default log level from the `--log-level` CLI flag.
/// It is ignored when the `RUST_LOG` environment variable is set.
///
/// When `SUNLIT_EARTH_SYNC_LOG` is set in the environment, a synchronous
/// stderr writer is used instead of the non-blocking one. This eliminates
/// pipe buffer congestion and stderr lock contention that cause log messages
/// to arrive late (or not at all) in e2e tests. The trade-off is that log
/// writes block the calling thread, which is acceptable in test mode.
///
/// Returns a `WorkerGuard` that must be kept alive for the duration of the
/// program so that buffered log lines are flushed before exit. In sync mode,
/// no guard is needed and `None` is returned.
fn init_logging(cli_level: Option<&str>) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let sync_log = std::env::var("SUNLIT_EARTH_SYNC_LOG").is_ok();

    let base_filter = match cli_level {
        Some(level) => EnvFilter::new(level),
        None => EnvFilter::from_default_env().add_directive(LevelFilter::INFO.into()),
    };

    let env_filter = DEPENDENCY_LEVELS
        .iter()
        .filter_map(|directive| directive.parse().ok())
        .fold(base_filter, EnvFilter::add_directive);

    if sync_log {
        let fmt_layer = fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(true)
            .with_thread_ids(true)
            .with_span_events(FmtSpan::CLOSE);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .init();

        None
    } else {
        let (non_blocking, guard) = tracing_appender::non_blocking(std::io::stderr());

        let fmt_layer = fmt::layer()
            .with_writer(non_blocking)
            .with_target(true)
            .with_thread_ids(true)
            .with_span_events(FmtSpan::CLOSE);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .init();

        Some(guard)
    }
}

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
fn resolve_texture_paths(cli_dir: Option<&std::path::Path>) -> Vec<Option<PathBuf>> {
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
fn have_globe_texture(paths: &[Option<PathBuf>]) -> bool {
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
fn effective_texture_resolution(cli: &Cli, config: &AppConfig) -> u32 {
    cli.texture_resolution
        .map_or(config.texture_resolution, u32::from)
}

/// Build the engine configuration shared by every startup mode.
fn engine_config(
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

/// The `displays` subcommand: no window, no desktop, and no GPU unless asked.
///
/// It exists so that checking this feature on a borrowed two-screen machine is
/// thirty seconds rather than an afternoon, and so that a bug report can carry
/// the layout. Without `--out` nothing is rendered and no device is created at
/// all: the plan is a pure function of the monitor list, and the enumeration is
/// the half of this feature that no test on another machine can stand in for.
fn run_displays(cli: &Cli, config: &AppConfig, out: Option<&std::path::Path>) -> ExitCode {
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
fn run_render(
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

    // Waiting for `TexturesReady` only makes sense if there is a globe texture
    // to wait for. A slot with no path never gets a bind group and so never
    // reports ready, and the overlays are not what readiness is about, so a
    // directory holding only the Moon's file or the panorama's would turn the
    // wait below into a guaranteed two-minute stall ending in an error about a
    // problem that does not exist. What the globe draws from is the same either
    // way, the procedural grid; an overlay is drawn if its own decode has landed
    // by then, which is what waiting on `TexturesReady` never promised anyway.
    //
    // This covers a missing file, not a broken one. A texture that fails to
    // decode leaves the slot in the same terminal state and still hangs the
    // wait; that is a gap in `Renderer::textures_ready` itself, affecting every
    // client rather than only this one, and it is on the roadmap as its own
    // fix rather than patched around here.
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

/// Set up the UI models and register every callback.
///
/// `texture_resolution` is what the engine started with, which is the config
/// value unless `--texture-resolution` overrode it.
fn init_ui(
    window: &MainWindow,
    config: &AppConfig,
    texture_resolution: u32,
    link: &EngineLink,
    screens: &displays::SharedMonitors,
) {
    let aa_labels: Vec<slint::SharedString> = link
        .aa_labels()
        .iter()
        .map(|label| slint::SharedString::from(label.as_str()))
        .collect();
    window.set_aa_options(slint::ModelRc::new(slint::VecModel::from(aa_labels)));

    let texture_labels: Vec<slint::SharedString> = renderer::TEXTURE_LABELS
        .iter()
        .map(|name| slint::SharedString::from(*name))
        .collect();
    window.set_texture_options(slint::ModelRc::new(slint::VecModel::from(texture_labels)));

    let resolution_labels: Vec<slint::SharedString> = config::TEXTURE_RESOLUTIONS
        .iter()
        .map(|w| slint::SharedString::from(w.to_string()))
        .collect();
    window.set_texture_resolution_options(slint::ModelRc::new(slint::VecModel::from(
        resolution_labels,
    )));

    let (base_year, end_year) = datetime::year_range();
    let year_labels: Vec<slint::SharedString> = (base_year..=end_year)
        .map(|y| slint::SharedString::from(y.to_string()))
        .collect();
    window.set_year_options(slint::ModelRc::new(slint::VecModel::from(year_labels)));

    // The query the settings window starts from. The engine re-queries on every
    // publish and announces a layout that moved, which is what replaces this
    // list; `None` is a platform with no way to ask and leaves the group with
    // nothing to show, which is what hides it.
    let monitors = display::monitors().unwrap_or_default();
    displays::apply_models_to_window(window, &monitors);
    displays::apply_diagram_to_window(window, &monitors, config.anchor().as_deref());

    ui_callbacks::apply_config_to_window(window, config);
    ui_callbacks::defer_combobox_indices(
        &window.as_weak(),
        ui_callbacks::ComboIndices::of(
            config,
            link.aa_counts(),
            texture_resolution,
            &displays::screen_ids(&monitors),
        ),
    );

    displays::set_monitors(screens, monitors);

    ui_callbacks::register_change_callbacks(window, base_year, link);
    ui_callbacks::register_mouse_callbacks(window, link);
    ui_callbacks::register_action_callbacks(window, link, screens);
    ui_callbacks::register_display_callbacks(window, link, screens);
}

/// Wire the auto-refresh controls to the engine's scheduler and the tray mark.
///
/// Two callbacks, because the interval slider reports every pixel of a drag:
/// the schedule follows the handle, and the config file is written once, when
/// the change is settled.
fn register_auto_refresh_callback(
    window: &MainWindow,
    link: &EngineLink,
    config: &AppConfig,
    tray: Option<std::rc::Rc<sunlit_earth::TrayIcon>>,
) {
    let window_weak = window.as_weak();
    let engine = link.clone();
    let prev_enabled = std::cell::Cell::new(config.auto_refresh_enabled);
    window.on_auto_refresh_changed(move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let enabled = win.get_auto_refresh_enabled();
        let was_enabled = prev_enabled.replace(enabled);

        // Keep the tray checkmark in step with the settings window.
        if let Some(tray) = tray.as_ref() {
            tray.set_auto_refresh_enabled(enabled);
        }

        send_auto_refresh(&engine, &win);

        // Refresh immediately when toggling on (not on slider change)
        if enabled && !was_enabled {
            info!("auto-refresh: immediate refresh on enable");
            engine.push_params(&win);
            engine.send(EngineCommand::RenderWallpaperNow);
        }

        config::save_config(&ui_callbacks::read_config_from_window(&win, &engine));
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_auto_refresh_interval_moved(move || {
        if let Some(win) = window_weak.upgrade() {
            send_auto_refresh(&engine, &win);
        }
    });
}

/// Tell the engine what the auto-refresh controls now say.
fn send_auto_refresh(engine: &EngineLink, window: &MainWindow) {
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let interval_minutes = u64::from(window.get_auto_refresh_interval().max(1.0) as u32);
    engine.send(EngineCommand::SetAutoRefresh {
        enabled: window.get_auto_refresh_enabled(),
        interval: Duration::from_mins(interval_minutes),
    });
}

/// Poll the preview viewport size and tell the engine when it changes.
///
/// Slint has no size-changed callback we can bind from Rust here, and polling
/// a pair of properties every 200 ms is far cheaper than any alternative. The
/// engine quantizes the value, so this sends at most one command per real
/// resize step.
fn start_viewport_timer(window: &MainWindow, link: &EngineLink) -> slint::Timer {
    let timer = slint::Timer::default();
    let window_weak = window.as_weak();
    let engine = link.clone();
    let last = std::cell::Cell::new((0u32, 0u32));
    timer.start(slint::TimerMode::Repeated, VIEWPORT_POLL, move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let size = {
            let scale = win.window().scale_factor();
            (
                (win.get_viewport_width() * scale) as u32,
                (win.get_viewport_height() * scale) as u32,
            )
        };
        if size != last.get() && size.0 > 0 && size.1 > 0 {
            last.set(size);
            engine.send(EngineCommand::SetPreviewSize(size.0, size.1));
        }
    });
    timer
}

/// Run the windowed or tray-mode application.
///
/// `instance_guard` holds the single-instance mutex in tray mode; it is
/// acquired in `main` so a second instance can exit before creating a window
/// or a GPU device.
#[allow(clippy::too_many_lines, clippy::needless_pass_by_value)]
fn run_app(
    cli: Cli,
    config: &AppConfig,
    instance_guard: Option<single_instance::SingleInstance>,
) -> ExitCode {
    texture_loader::register_jxl_hook();
    debug!("registered JXL decoding hook");

    let use_tray = matches!(cli.mode, Mode::Tray);
    let tray_start = cli.tray_start;

    // Ahead of the window and the engine: a name the platform refuses, or one
    // another instance is already holding, is an answer worth having before the
    // adapter is selected and every texture decoded rather than six seconds
    // after. The app runs without the control channel either way.
    let ipc = cli.ipc_socket.as_deref().and_then(|name| {
        info!("binding the IPC socket {name}");
        sunlit_earth::ipc::bind(name)
            .inspect_err(|e| warn!("{e}; carrying on without IPC"))
            .ok()
    });

    let Ok(window) = MainWindow::new().inspect_err(|e| error!("no window could be created: {e}"))
    else {
        return ExitCode::FAILURE;
    };
    window.set_version(env!("CARGO_PKG_VERSION").into());
    sunlit_core::memory::log_memory_usage("after window creation");

    // The engine event callback needs the window; the UI callbacks need the
    // engine. Break the cycle by building the callback first and moving it
    // into the config.
    let auto_refresh_at_startup = config.auto_refresh_enabled;
    let startup_refresh_done = Arc::new(AtomicBool::new(!auto_refresh_at_startup));
    let refresh_flag = Arc::clone(&startup_refresh_done);
    let (refresh_tx, refresh_rx) = crossbeam_channel::bounded::<()>(1);
    // One monitor list for the whole window: the Displays group's callbacks read
    // it, and the engine's `MonitorsChanged` event replaces it.
    let screens = displays::shared_monitors();
    let on_event = engine_client::event_forwarder(&window, &screens, move || {
        if !refresh_flag.swap(true, Ordering::SeqCst) {
            let _ = refresh_tx.try_send(());
        }
    });

    let mut engine_config = engine_config(&cli, config, (800, 600), true);
    engine_config.on_event = on_event;
    let engine: EngineHandle = match engine::start(engine_config) {
        Ok(engine) => engine,
        Err(e) => {
            // The window that would have carried this message is never shown,
            // so the console is the only place left to put it. A release build
            // attaches the parent's console at startup for exactly this.
            error!("the renderer could not start: {e}");
            eprintln!("sunlit earth: the renderer could not start: {e}");
            if !cli.software_rendering {
                eprintln!("try --software-rendering if this machine has no usable GPU driver");
            }
            return ExitCode::FAILURE;
        }
    };
    window.set_renderer_info(engine.adapter_info().into());

    // The display watcher's one consumer is the engine, and its condition is
    // "always": it runs from here until the teardown below, whether or not a
    // window is shown. A hint is a nudge and nothing more, so a platform that
    // cannot watch (macOS, a session with no `DISPLAY`) leaves the app exactly
    // as it was, with the monitor list re-queried on every publish.
    let hint_tx = engine.sender();
    let display_watch = display::watch::start(Arc::new(move || {
        let _ = hint_tx.send(EngineCommand::DisplaysChanged);
    }));

    let quality = cli.quality.map_or(config.quality_tier, QualityTier::from);
    let (aa_labels, aa_counts, _) =
        renderer::build_aa_options(engine.supported_sample_counts(), quality.max_sample_count());
    let link = EngineLink::new(engine.sender(), aa_labels, aa_counts);
    // The window is about to show the override, and a save must not write it.
    link.set_resolution_is_one_run_only(cli.texture_resolution.is_some());

    init_ui(
        &window,
        config,
        effective_texture_resolution(&cli, config),
        &link,
        &screens,
    );
    let about = sunlit_earth::about::AboutController::default();
    about.register_settings_callback(&window);

    // The tray icon is a top-level Slint component of its own; it must exist
    // before the auto-refresh callback so the two views of that setting can be
    // kept in step.
    let tray = if use_tray {
        sunlit_earth::tray::create_tray(&window, &link, &about).map(std::rc::Rc::new)
    } else {
        None
    };
    // A session with no tray host leaves the window as the only way to reach
    // the app, so from here on this run is a windowed one: the close button
    // ends it and the window is shown whatever `--tray-start` asked for.
    let use_tray = tray.is_some();
    register_auto_refresh_callback(&window, &link, config, tray.clone());
    if let Some((x, y, w, h)) = config::validated_window_geometry(config) {
        window
            .window()
            .set_position(slint::PhysicalPosition::new(x, y));
        window.window().set_size(slint::PhysicalSize::new(w, h));
    }
    debug!("loaded config from disk");

    // The engine started from the config; push once more so anything the UI
    // clamped on the way in (day-of-year, year index) reaches it too.
    link.push_params(&window);

    let viewport_timer = start_viewport_timer(&window, &link);

    // When auto-refresh is on at startup, the first wallpaper update waits for
    // the textures rather than firing against the grid placeholder.
    let startup_refresh_timer = std::rc::Rc::new(slint::Timer::default());
    if auto_refresh_at_startup {
        let engine_link = link.clone();
        let timer = std::rc::Rc::clone(&startup_refresh_timer);
        startup_refresh_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(200),
            move || {
                if refresh_rx.try_recv().is_ok() {
                    info!("auto-refresh: initial wallpaper update on startup");
                    engine_link.send(EngineCommand::RenderWallpaperNow);
                    timer.stop();
                }
            },
        );
    }

    if use_tray {
        debug!("startup mode: tray");
    } else {
        debug!("startup mode: windowed");
    }

    let _instance_guard = instance_guard;

    // Close handler depends on mode:
    // - Tray: save geometry, hide window (stays in tray)
    // - Windowed: quit the event loop (app exits)
    if use_tray {
        let window_weak = window.as_weak();
        let engine_link = link.clone();
        window.window().on_close_requested(move || {
            if let Some(win) = window_weak.upgrade() {
                let size = win.window().size();
                let pos = win.window().position();
                config::save_window_geometry(pos.x, pos.y, size.width, size.height);
            }
            debug!("main window hidden (minimized to tray)");
            sunlit_core::memory::log_memory_usage("after window hidden");
            // Slint does the hiding for us, so the engine has to be told
            // separately that nobody is looking any more.
            engine_link.set_preview_enabled(false);
            slint::CloseRequestResponse::HideWindow
        });
    } else {
        window.window().on_close_requested(|| {
            debug!("window closed, quitting event loop");
            slint::quit_event_loop().ok();
            slint::CloseRequestResponse::KeepWindowShown
        });
    }

    // The socket taken at the top of this function starts being answered here,
    // now that there is a window and an engine to answer for.
    let _ipc_thread = ipc.and_then(|listener| {
        listener
            .serve(window.as_weak(), link.clone())
            .inspect_err(|e| warn!("{e}; carrying on without IPC"))
            .ok()
    });

    let status = run_event_loop(&window, &link, use_tray, tray_start);

    sunlit_core::memory::log_memory_usage("before exit");

    // The engine owns the GPU device, so shutting it down here is an ordinary
    // join on a worker thread. Slint holds no wgpu objects any more, which is
    // what retired the process::exit(0) that used to dodge a thread-local
    // destruction panic in wgpu's Queue::drop.
    engine.shutdown();
    // After the engine, so a hint that arrives during the teardown has an
    // engine to reach; the watcher's thread is woken and joined here.
    if let Some(watcher) = display_watch {
        watcher.stop();
    }
    drop(viewport_timer);
    drop(startup_refresh_timer);
    drop(tray);

    debug!("exiting");
    status
}

/// Show the window and run the event loop until something quits it.
///
/// Both steps answer with a `PlatformError` when the session has no display
/// server or no usable backend, which is a message for the user rather than a
/// backtrace. The caller tears the engine down either way.
fn run_event_loop(
    window: &MainWindow,
    link: &EngineLink,
    use_tray: bool,
    tray_start: TrayStart,
) -> ExitCode {
    if let Err(e) = window.show() {
        error!("the window could not be shown: {e}");
        return ExitCode::FAILURE;
    }

    // In tray mode with --tray-start hidden, defer the hide to a zero-duration
    // timer so it fires after the event loop is running. Hiding synchronously
    // before run_event_loop_until_quit() causes the loop to exit immediately.
    if use_tray && matches!(tray_start, TrayStart::Hidden) {
        let ww = window.as_weak();
        let engine_link = link.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            if let Some(win) = ww.upgrade() {
                debug!("hiding window for --tray-start hidden (deferred)");
                win.hide().ok();
            }
            engine_link.set_preview_enabled(false);
            println!("SIGNAL:window_hidden_deferred");
        });
    }

    // Windows asks its top-level windows for permission before a reboot and
    // then tells them the session is ending; a Linux session sends SIGTERM and
    // kills what has not gone. The listener quits the loop, and the ordinary
    // teardown then runs.
    let _session_end = sunlit_earth::session_end::install(
        Some(sunlit_earth::session_end::FORCE_EXIT_AFTER),
        || {
            let _ = slint::invoke_from_event_loop(|| {
                debug!("session end: quitting the event loop");
                slint::quit_event_loop().ok();
            });
        },
    );

    info!("entering event loop");
    if let Err(e) = slint::run_event_loop_until_quit() {
        error!("the event loop stopped: {e}");
        return ExitCode::FAILURE;
    }
    info!("event loop exited");
    ExitCode::SUCCESS
}

/// Attach to the parent process's console so that stdout/stderr work when
/// invoked from a terminal.  In release builds the `windows_subsystem = "windows"`
/// attribute suppresses the console, which makes `--help` / `--version` silent.
/// `AttachConsole(ATTACH_PARENT_PROCESS)` re-establishes the connection without
/// creating a new console window when launched from Explorer.
#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};

    // SAFETY: AttachConsole is a well-documented Win32 function with no
    // preconditions. It returns FALSE (harmlessly) when no parent console
    // exists, e.g. when launched from Explorer.
    #[allow(unsafe_code)]
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

fn main() -> ExitCode {
    #[cfg(windows)]
    attach_parent_console();

    let cli = Cli::parse();
    let _guard = init_logging(cli.log_level.as_deref());
    info!("sunlit earth v{}", env!("CARGO_PKG_VERSION"));

    // Validate: --tray-start hidden only makes sense with --mode tray
    if matches!(cli.tray_start, TrayStart::Hidden) && matches!(cli.mode, Mode::Window) {
        eprintln!("error: --tray-start hidden is only valid with --mode tray");
        return ExitCode::from(2);
    }

    debug!(
        software_rendering = cli.software_rendering,
        texture_resolution = ?cli.texture_resolution,
        textures_dir = ?cli.textures_dir,
        log_level = ?cli.log_level,
        mode = ?cli.mode,
        tray_start = ?cli.tray_start,
        ipc_socket = ?cli.ipc_socket,
        "parsed CLI arguments"
    );

    // Load config: from --config path if the render subcommand specifies one,
    // otherwise from the user's saved config on disk.
    let config = match &cli.command {
        Some(Commands::Render {
            config: Some(path), ..
        }) => config::load_config_from(path),
        _ => config::load_config(),
    };

    match &cli.command {
        Some(Commands::Render {
            output,
            width,
            height,
            ..
        }) => {
            let (output, width, height) = (output.clone(), *width, *height);
            run_render(&cli, &config, &output, width, height)
        }
        Some(Commands::Displays { out }) => {
            let out = out.clone();
            run_displays(&cli, &config, out.as_deref())
        }
        None => {
            // Single-instance enforcement (tray mode only), before the window
            // and the GPU device exist. Returning from `main` rather than
            // exiting in place drops the logging guard, which is what flushes
            // the message below to stderr.
            let instance_guard = if matches!(cli.mode, Mode::Tray) {
                let mutex_name = match &cli.ipc_socket {
                    Some(name) => format!("sunlit-earth-{name}"),
                    None => "sunlit-earth-app".to_string(),
                };
                match sunlit_earth::tray::acquire_single_instance(&mutex_name) {
                    sunlit_earth::tray::InstanceCheck::AlreadyRunning => {
                        info!("another instance is already running, exiting");
                        return ExitCode::SUCCESS;
                    }
                    sunlit_earth::tray::InstanceCheck::Alone(guard) => guard,
                }
            } else {
                None
            };
            run_app(cli, &config, instance_guard)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pointer file exists, so `exists()` is not the question to ask.
    ///
    /// The smallest of the four assets is 285 KB and a checkout without the LFS
    /// objects holds a couple of hundred bytes under the same name. What naming
    /// one costs is a decode failure and an error line for a checkout that is
    /// only incomplete, where the same run without the file at all is quiet and
    /// draws the same picture.
    #[test]
    fn a_git_lfs_pointer_is_not_a_texture_path() {
        let dir = std::env::temp_dir().join("sunlit_earth_test_texture_pointers");
        std::fs::create_dir_all(&dir).expect("create the fixture directory");
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

        let paths = resolve_texture_paths(Some(&dir));
        assert!(paths[0].is_some(), "the day map is the asset here");
        assert_eq!(paths[1], None, "the night map is not in the directory");
        assert_eq!(paths[2], None, "the moon map is a pointer");
        assert_eq!(paths[3], None, "the panorama is not in the directory");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An overlay is not something to wait for.
    ///
    /// The wait in `run_render` is for `TexturesReady`, which the renderer
    /// reports from the globe's own slots, so a textures directory holding only
    /// the Moon's file or the panorama's leaves nothing to wait for and asking
    /// whether any path at all is present costs the two-minute timeout and an
    /// error line for a picture that was never going to change.
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
