#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::{Parser, Subcommand, ValueEnum};
use slint::ComponentHandle;
use tracing::{debug, error, info};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

use sunlit_earth::config;
use sunlit_earth::renderer;
use sunlit_earth::scene::datetime;
use sunlit_earth::texture_loader;
use sunlit_earth::wgpu_init;
use sunlit_earth::MainWindow;

/// Sunlit Earth: get a realistic 3D view of Earth as seen from space and set it as your wallpaper
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Force software rendering (CPU-based, no GPU required)
    #[arg(long)]
    software_rendering: bool,

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
/// Returns a `WorkerGuard` that must be kept alive for the duration of the
/// program so that buffered log lines are flushed before exit.
fn init_logging(cli_level: Option<&str>) -> tracing_appender::non_blocking::WorkerGuard {
    let (non_blocking, guard) = tracing_appender::non_blocking(std::io::stderr());

    let base_filter = match cli_level {
        Some(level) => EnvFilter::new(level),
        None => EnvFilter::from_default_env().add_directive("info".parse().expect("valid directive")),
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

    let fmt_layer = fmt::layer()
        .with_writer(non_blocking)
        .with_target(true)
        .with_thread_ids(true)
        .with_span_events(FmtSpan::CLOSE);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .init();

    guard
}

/// Set up UI `ComboBox` models, apply initial config, and register callbacks.
///
/// Returns `(aa_counts, base_year)` needed by later initialization steps.
fn init_ui_models(
    window: &MainWindow,
    config: &config::AppConfig,
    supported_sample_counts: &[u32],
    textures_dir: Option<&std::path::Path>,
) -> (Vec<u32>, i32) {
    // Set up AA options from supported sample counts
    let (aa_labels, aa_counts, _aa_default) =
        renderer::build_aa_options(supported_sample_counts);
    let aa_model: slint::VecModel<slint::SharedString> = aa_labels.into();
    window.set_aa_options(slint::ModelRc::new(aa_model));

    // Register JXL decoding hook before any image loading
    texture_loader::register_jxl_hook();
    debug!("registered JXL decoding hook");

    // Set up texture options — always show all four
    let labels: Vec<slint::SharedString> = vec![
        "Grid".into(),
        "Day".into(),
        "Night".into(),
        "Day/Night Blend".into(),
    ];
    window.set_texture_options(slint::ModelRc::new(slint::VecModel::from(labels)));

    // Set up year ComboBox options (current year +/- 10)
    let (base_year, end_year) = datetime::year_range();
    let year_labels: Vec<slint::SharedString> =
        (base_year..=end_year).map(|y| slint::SharedString::from(y.to_string())).collect();
    window.set_year_options(slint::ModelRc::new(slint::VecModel::from(year_labels)));

    // Defer setting ComboBox indices so they apply after Slint processes model changes
    let config_aa_index = config::find_sample_count_index(&aa_counts, config.sample_count);
    sunlit_earth::ui_callbacks::defer_combobox_indices(
        &window.as_weak(),
        config_aa_index,
        config.texture_index,
    );

    // Register all UI callbacks
    sunlit_earth::ui_callbacks::register_change_callbacks(window, base_year);
    sunlit_earth::ui_callbacks::register_mouse_callbacks(window);
    sunlit_earth::ui_callbacks::register_action_callbacks(window, &aa_counts);

    // Log resolved texture paths (need textures_dir for logging only at this point)
    if let Some(dir) = textures_dir {
        debug!(?dir, "textures directory resolved");
    }

    (aa_counts, base_year)
}

/// Set up the rendering notifier, texture channels, and cloud fetcher.
///
/// Returns `textures_ready` for the render timer to poll.
fn init_texture_system(
    window: &MainWindow,
    aa_counts: Vec<u32>,
    texture_paths: Vec<Option<PathBuf>>,
) -> Arc<AtomicBool> {
    // Create the texture channel so both the renderer and the cloud fetcher can share it
    let (texture_tx, texture_rx) =
        std::sync::mpsc::channel::<sunlit_earth::renderer::DecodedTextureMessage>();

    let textures_ready = Arc::new(AtomicBool::new(false));

    renderer::setup_rendering_notifier(
        window,
        aa_counts,
        texture_paths,
        texture_tx.clone(),
        texture_rx,
        Arc::clone(&textures_ready),
    );

    // Spawn the background cloud fetcher (slot index 3 = CLOUDS_SLOT)
    sunlit_earth::cloud_fetcher::spawn_cloud_fetcher(
        texture_tx,
        window.as_weak(),
        3,
    );
    info!("spawned cloud fetcher background thread");

    textures_ready
}

/// Run the event loop with timers, tray setup, and shutdown logic.
///
/// This function does not return normally — it calls `process::exit(0)` after
/// the event loop exits to avoid panics from thread-local destruction ordering.
///
/// Takes ownership of `window` and `textures_ready` to ensure they live
/// until after the event loop exits.
#[allow(clippy::needless_pass_by_value)]
fn run_event_loop(
    window: MainWindow,
    cli_command: Option<Commands>,
    mode: Mode,
    tray_start: TrayStart,
    ipc_socket: Option<String>,
    textures_ready: Arc<AtomicBool>,
) -> ! {
    // Periodic timer to update the sun position (every 2 minutes)
    let window_weak = window.as_weak();
    let sun_timer = slint::Timer::default();
    sun_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(120),
        move || {
            if let Some(win) = window_weak.upgrade() {
                sunlit_earth::memory::log_memory_usage("sun timer tick");
                win.window().request_redraw();
            }
        },
    );

    // When the `render` subcommand is used, register a timer that polls the
    // texture-readiness flag and saves a rendered image when ready.
    let is_render = cli_command.is_some();
    let render_timer = if let Some(Commands::Render { output, width, height, .. }) = cli_command {
        let output_path = output;
        let render_width = width;
        let render_height = height;
        let textures_ready = Arc::clone(&textures_ready);
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(200),
            move || {
                if textures_ready.load(Ordering::Relaxed) {
                    match renderer::export_wallpaper_image(render_width, render_height) {
                        Ok(pixels) => {
                            if let Err(e) = sunlit_earth::ui_callbacks::save_render_png(&output_path, render_width, render_height, &pixels) {
                                error!("render failed: {e}");
                            } else {
                                info!("render saved to {}", output_path.display());
                            }
                        }
                        Err(e) => error!("render export failed: {e}"),
                    }
                    slint::quit_event_loop().ok();
                }
            },
        );
        Some(timer)
    } else {
        None
    };

    // Branch into one of three startup modes:
    // 1. Render subcommand — run the event loop and exit (no tray, no single-instance)
    // 2. Tray mode (default, Windows only) — tray icon, hide-on-close, single-instance
    // 3. Windowed mode (--windowed or non-Windows) — original behavior, close exits
    let use_tray = !is_render && matches!(mode, Mode::Tray);

    if is_render {
        debug!("startup mode: render");
    } else if use_tray {
        debug!("startup mode: tray");
    } else {
        debug!("startup mode: windowed");
    }

    let _instance_guard = if use_tray {
        // Scope the mutex name with the IPC socket name so test instances
        // don't conflict with the real app or each other.
        let mutex_name = match &ipc_socket {
            Some(name) => format!("sunlit-earth-{name}"),
            None => "sunlit-earth-app".to_string(),
        };
        Some(sunlit_earth::tray::enforce_single_instance(&mutex_name))
    } else {
        None
    };

    let _tray_handle = if use_tray {
        // Intercept the close button: save geometry, minimize the window,
        // and keep it shown. We use KeepWindowShown + minimize instead of
        // HideWindow because HideWindow causes run_event_loop() to exit
        // (treating the hidden window as "closed") and
        // run_event_loop_until_quit() stops processing timers after
        // re-entering run_app_on_demand. Minimizing keeps the window
        // "alive" from the event loop's perspective.
        let window_weak = window.as_weak();
        window.window().on_close_requested(move || {
            if let Some(win) = window_weak.upgrade() {
                let size = win.window().size();
                let pos = win.window().position();
                config::save_window_geometry(pos.x, pos.y, size.width, size.height);
                // Move off-screen instead of hiding or minimizing:
                // - hide() causes run_event_loop() to exit
                // - set_minimized(true) triggers winit Suspended, stopping timers
                win.window().set_position(slint::PhysicalPosition::new(-32000, -32000));
            }
            debug!("main window hidden (minimized to tray)");
            sunlit_earth::memory::log_memory_usage("after window hidden");
            slint::CloseRequestResponse::KeepWindowShown
        });
        Some(sunlit_earth::tray::spawn_tray_thread(window.as_weak()))
    } else {
        None
    };

    // Spawn IPC listener and command timer if --ipc-socket was provided.
    // Commands are dispatched via a shared queue + polling timer instead of
    // invoke_from_event_loop, which deadlocks the Slint/winit event loop
    // on Windows (the proxy message freezes the message pump).
    let _ipc_handle = ipc_socket.map(|name| {
        let queue = sunlit_earth::ipc::CommandQueue::new();
        let handle = sunlit_earth::ipc::spawn_ipc_listener(&name, queue.clone());
        let timer = sunlit_earth::ipc::start_command_timer(queue, window.as_weak());
        (handle, timer)
    });

    if use_tray {
        // Tray mode: show the window and use run_event_loop_until_quit()
        // so the loop stays alive when the window is hidden via KeepWindowShown.
        // run_event_loop() would exit when the window becomes invisible.
        if matches!(tray_start, TrayStart::Visible) {
            window.show().expect("Failed to show window");
        }
        slint::run_event_loop_until_quit().expect("Failed to run event loop");
    } else {
        // Windowed mode: show() + run_event_loop(). We avoid window.run()
        // because its hide() call after quit triggers wgpu teardown during
        // an unsafe state on Windows (STATUS_STACK_BUFFER_OVERRUN).
        window.show().expect("Failed to show window");
        slint::run_event_loop().expect("Failed to run event loop");
    }

    // Save window geometry on close. Skip after IPC-triggered quit because
    // the Slint backend may be partially torn down, making window accessors
    // unsafe. In tray mode, geometry is already saved in on_close_requested.
    if !is_render && _ipc_handle.is_none() {
        let size = window.window().size();
        let pos = window.window().position();
        config::save_window_geometry(pos.x, pos.y, size.width, size.height);
    }

    sunlit_earth::memory::log_memory_usage("before exit");

    // Intentionally leak timers — their Slint destructors can crash after
    // quit_event_loop() because the backend may be torn down.
    // process::exit() terminates everything anyway.
    std::mem::forget(sun_timer);
    std::mem::forget(render_timer);
    if let Some((_handle, ipc_timer)) = _ipc_handle {
        std::mem::forget(ipc_timer);
    }

    // Exit immediately to avoid a panic from thread-local destruction ordering.
    debug!("exiting");
    std::process::exit(0);
}

fn main() {
    let cli = Cli::parse();
    let _guard = init_logging(cli.log_level.as_deref());
    info!("sunlit earth v{}", env!("CARGO_PKG_VERSION"));
    // Validate: --tray-start hidden only makes sense with --mode tray
    if matches!(cli.tray_start, TrayStart::Hidden) && matches!(cli.mode, Mode::Window) {
        eprintln!("error: --tray-start hidden is only valid with --mode tray");
        std::process::exit(2);
    }

    debug!(
        software_rendering = cli.software_rendering,
        textures_dir = ?cli.textures_dir,
        log_level = ?cli.log_level,
        mode = ?cli.mode,
        tray_start = ?cli.tray_start,
        ipc_socket = ?cli.ipc_socket,
        "parsed CLI arguments"
    );

    let wgpu_context = wgpu_init::init(cli.software_rendering);
    sunlit_earth::memory::log_memory_usage("after wgpu init");

    slint::BackendSelector::new()
        .require_wgpu_28(wgpu_context.config)
        .select()
        .expect("Failed to select wgpu backend");

    let window = MainWindow::new().expect("Failed to create window");
    window.set_renderer_info(wgpu_context.adapter_info.into());
    sunlit_earth::memory::log_memory_usage("after window creation");

    // Load config: from --config path if render subcommand specifies one,
    // otherwise from the user's saved config on disk.
    let config = match &cli.command {
        Some(Commands::Render { config: Some(path), .. }) => config::load_config_from(path),
        _ => config::load_config(),
    };
    sunlit_earth::ui_callbacks::apply_config_to_window(&window, &config);
    if let Some((x, y, w, h)) = config::validated_window_geometry(&config) {
        window.window().set_position(slint::PhysicalPosition::new(x, y));
        window.window().set_size(slint::PhysicalSize::new(w, h));
    }
    debug!("loaded config from disk");

    // Resolve texture paths for JXL files (loaded lazily when selected)
    let textures_dir = texture_loader::resolve_textures_dir(cli.textures_dir.as_deref());
    let day_path = textures_dir
        .as_ref()
        .map(|d| d.join("world.topo.200405.jxl"))
        .filter(|p| p.exists());
    let night_path = textures_dir
        .as_ref()
        .map(|d| d.join("BlackMarble_2016.jxl"))
        .filter(|p| p.exists());
    let texture_paths = vec![day_path, night_path];
    info!(textures_dir = ?textures_dir, day = ?texture_paths[0], night = ?texture_paths[1], "resolved texture paths");

    let (aa_counts, _base_year) = init_ui_models(
        &window,
        &config,
        &wgpu_context.supported_sample_counts,
        textures_dir.as_deref(),
    );

    let textures_ready = init_texture_system(&window, aa_counts, texture_paths);

    run_event_loop(window, cli.command, cli.mode, cli.tray_start, cli.ipc_socket, textures_ready);
}
