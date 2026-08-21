#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use slint::ComponentHandle;
use tracing::{debug, error, info};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

use sunlit_core::assets::cloud_fetcher;
use sunlit_core::assets::cloud_source::HttpCloudSource;
use sunlit_core::assets::texture_loader;
use sunlit_core::config::{self, AppConfig, QualityTier};
use sunlit_core::engine::clock::SystemClock;
use sunlit_core::engine::wallpaper_sink::SystemWallpaper;
use sunlit_core::engine::{self, EngineCommand, EngineConfig, EngineHandle};
use sunlit_core::params::SceneParams;
use sunlit_core::renderer;
use sunlit_core::scene::datetime;
use sunlit_earth::MainWindow;
use sunlit_earth::engine_client::{self, EngineLink};
use sunlit_earth::ui_callbacks;

/// How long the render subcommand waits for textures before exporting anyway.
const RENDER_TEXTURE_TIMEOUT: Duration = Duration::from_secs(120);

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
}

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
        None => {
            EnvFilter::from_default_env().add_directive("info".parse().expect("valid directive"))
        }
    };

    let env_filter = base_filter
        .add_directive("wgpu_core=warn".parse().expect("valid directive"))
        .add_directive("wgpu_hal=error".parse().expect("valid directive"))
        .add_directive("naga=warn".parse().expect("valid directive"))
        .add_directive("winit=warn".parse().expect("valid directive"))
        .add_directive("ureq=warn".parse().expect("valid directive"))
        .add_directive("ureq_proto=warn".parse().expect("valid directive"))
        .add_directive("rustls=warn".parse().expect("valid directive"))
        .add_directive("jxl_render=warn".parse().expect("valid directive"))
        .add_directive("jxl_grid=warn".parse().expect("valid directive"))
        .add_directive("jxl_modular=warn".parse().expect("valid directive"))
        .add_directive("jxl_bitstream=warn".parse().expect("valid directive"))
        .add_directive("jxl_frame=warn".parse().expect("valid directive"))
        .add_directive("jxl_color=warn".parse().expect("valid directive"));

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

/// Resolve the day and night texture paths from the textures directory.
fn resolve_texture_paths(cli_dir: Option<&std::path::Path>) -> Vec<Option<PathBuf>> {
    let dir = texture_loader::resolve_textures_dir(cli_dir);
    let pick = |name: &str| dir.as_ref().map(|d| d.join(name)).filter(|p| p.exists());
    let paths = vec![pick("world.topo.200405.jxl"), pick("BlackMarble_2016.jxl")];
    info!(textures_dir = ?dir, day = ?paths[0], night = ?paths[1], "resolved texture paths");
    paths
}

/// The surface texture width this run uses.
///
/// The flag wins over the stored setting, and unlike `--quality` the choice has
/// a widget, so the window shows what the engine actually loaded rather than
/// what is on disk. A later save therefore persists the override, which is the
/// price of the window telling the truth.
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

    // Skip cloud fetching entirely when SUNLIT_EARTH_NO_CLOUDS is set; e2e
    // tests use it to keep the network out of the picture.
    let cloud = if std::env::var("SUNLIT_EARTH_NO_CLOUDS").is_ok() {
        info!("cloud fetcher disabled (SUNLIT_EARTH_NO_CLOUDS)");
        None
    } else {
        let url = cloud_fetcher::cloud_url(quality);
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
        texture_resolution: effective_texture_resolution(cli, config),
        clock: Arc::new(SystemClock::new()),
        cloud,
        cloud_poll_interval: cloud_fetcher::poll_interval(),
        cache_dir: cloud_fetcher::cache_dir(),
        auto_refresh: config.auto_refresh_enabled.then(|| {
            Duration::from_secs(u64::from(config.auto_refresh_interval_minutes.max(1)) * 60)
        }),
        wallpaper: Arc::new(SystemWallpaper),
        on_event: Arc::new(|_| {}),
        record_metrics: true,
    }
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

    // Waiting for `TexturesReady` only makes sense if there is a texture file
    // to wait for. A slot with no path never gets a bind group and so never
    // reports ready, which turns the wait below into a guaranteed two-minute
    // stall ending in an error about a problem that does not exist. The frame
    // is the same either way: the scene falls back to the procedural grid.
    //
    // This covers a missing file, not a broken one. A texture that fails to
    // decode leaves the slot in the same terminal state and still hangs the
    // wait; that is a gap in `Renderer::textures_ready` itself, affecting every
    // client rather than only this one, and it is on the roadmap as its own
    // fix rather than patched around here.
    let have_textures = engine_config.texture_paths.iter().any(Option::is_some);

    let engine = engine::start(engine_config);
    if have_textures {
        if ready_rx.recv_timeout(RENDER_TEXTURE_TIMEOUT).is_err() {
            error!("textures were not ready within the timeout, rendering anyway");
        }
    } else {
        info!("no texture files found, rendering the procedural grid");
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
fn init_ui(window: &MainWindow, config: &AppConfig, texture_resolution: u32, link: &EngineLink) {
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

    ui_callbacks::apply_config_to_window(window, config);
    let config_aa_index = config::find_sample_count_index(link.aa_counts(), config.sample_count);
    ui_callbacks::defer_combobox_indices(
        &window.as_weak(),
        config_aa_index,
        config.texture_index,
        config::find_texture_resolution_index(texture_resolution),
    );

    ui_callbacks::register_change_callbacks(window, base_year, link);
    ui_callbacks::register_mouse_callbacks(window, link);
    ui_callbacks::register_action_callbacks(window, link);
}

/// Wire the auto-refresh checkbox to the engine's scheduler and the tray mark.
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

        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let interval_secs = u64::from(win.get_auto_refresh_interval().max(1.0) as u32) * 60;
        engine.send(EngineCommand::SetAutoRefresh {
            enabled,
            interval: Duration::from_secs(interval_secs),
        });

        // Refresh immediately when toggling on (not on slider change)
        if enabled && !was_enabled {
            info!("auto-refresh: immediate refresh on enable");
            engine.push_params(&win);
            engine.send(EngineCommand::RenderWallpaperNow);
        }

        config::save_config(&ui_callbacks::read_config_from_window(&win, &engine));
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
    let ipc_socket = cli.ipc_socket.clone();

    let window = MainWindow::new().expect("Failed to create window");
    sunlit_core::memory::log_memory_usage("after window creation");

    // The engine event callback needs the window; the UI callbacks need the
    // engine. Break the cycle by building the callback first and moving it
    // into the config.
    let auto_refresh_at_startup = config.auto_refresh_enabled;
    let startup_refresh_done = Arc::new(AtomicBool::new(!auto_refresh_at_startup));
    let refresh_flag = Arc::clone(&startup_refresh_done);
    let (refresh_tx, refresh_rx) = crossbeam_channel::bounded::<()>(1);
    let on_event = engine_client::event_forwarder(&window, move || {
        if !refresh_flag.swap(true, Ordering::SeqCst) {
            let _ = refresh_tx.try_send(());
        }
    });

    let mut engine_config = engine_config(&cli, config, (800, 600), true);
    engine_config.on_event = on_event;
    let engine: EngineHandle = engine::start(engine_config);
    window.set_renderer_info(engine.adapter_info().into());

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
    );

    // The tray icon is a top-level Slint component of its own; it must exist
    // before the auto-refresh callback so the two views of that setting can be
    // kept in step.
    let tray = use_tray.then(|| std::rc::Rc::new(sunlit_earth::tray::create_tray(&window, &link)));
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

    // Spawn IPC listener if --ipc-socket was provided.
    let _ipc_handle = ipc_socket.map(|name| {
        info!("spawning IPC listener on {name}");
        sunlit_earth::ipc::spawn_ipc_listener(&name, window.as_weak(), link.clone())
    });

    window.show().expect("Failed to show window");

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
    // then tells them the session is ending; nothing here used to answer, so
    // the shutdown screen named this process as the one preventing it. The
    // listener quits the loop, and the ordinary teardown below then runs.
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
    slint::run_event_loop_until_quit().expect("Failed to run event loop");
    info!("event loop exited");

    sunlit_core::memory::log_memory_usage("before exit");

    // The engine owns the GPU device, so shutting it down here is an ordinary
    // join on a worker thread. Slint holds no wgpu objects any more, which is
    // what retired the process::exit(0) that used to dodge a thread-local
    // destruction panic in wgpu's Queue::drop.
    engine.shutdown();
    drop(viewport_timer);
    drop(startup_refresh_timer);
    drop(tray);

    debug!("exiting");
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
                let Some(guard) = sunlit_earth::tray::acquire_single_instance(&mutex_name) else {
                    info!("another instance is already running, exiting");
                    return ExitCode::SUCCESS;
                };
                Some(guard)
            } else {
                None
            };
            run_app(cli, &config, instance_guard)
        }
    }
}
