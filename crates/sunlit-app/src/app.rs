//! The program itself: the entry point every mode goes through, and the
//! windowed and tray-mode run that most of them end in.

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::Parser;
use slint::ComponentHandle;
use tracing::{debug, error, info, warn};

use sunlit_core::assets::texture_loader;
use sunlit_core::config::{self, AppConfig, QualityTier};
use sunlit_core::display;
use sunlit_core::engine::{self, EngineCommand, EngineHandle};
use sunlit_core::renderer;
use sunlit_core::scene::datetime;

use crate::MainWindow;
use crate::cli::{Cli, Commands, Mode, TrayStart};
use crate::displays;
use crate::engine_client::{self, EngineLink};
use crate::headless::{run_displays, run_render};
use crate::logging::init_logging;
use crate::startup::{effective_texture_resolution, engine_config};
use crate::ui_callbacks;

/// How often the app checks whether the preview viewport changed size.
const VIEWPORT_POLL: Duration = Duration::from_millis(200);

/// How long the auto-refresh interval must sit still before it is written.
///
/// A drag is written when the handle is released, and the keyboard's arrows,
/// Home and End release too. What has no release at all is the accessibility
/// action a screen reader drives, which every Slint style routes straight to
/// `changed`, so without this the value would reach the engine and never reach
/// the disk.
const AUTO_REFRESH_SAVE_DELAY: Duration = Duration::from_secs(1);

/// Parse the command line, install logging, and run whichever mode was asked
/// for.
///
/// The binary is a `main` over this and the console attach that has to happen
/// before it; everything else about the program lives in the library, where the
/// tests can reach it.
pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let _guard = init_logging(cli.log_level.as_deref());
    info!("sunlit earth v{}", env!("CARGO_PKG_VERSION"));

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
                match crate::tray::acquire_single_instance(&mutex_name) {
                    crate::tray::InstanceCheck::AlreadyRunning => {
                        info!("another instance is already running, exiting");
                        return ExitCode::SUCCESS;
                    }
                    crate::tray::InstanceCheck::Alone(guard) => guard,
                }
            } else {
                None
            };
            run_app(cli, &config, instance_guard)
        }
    }
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
/// the change is settled. A change that never settles because nothing released
/// the handle is written by [`AUTO_REFRESH_SAVE_DELAY`]'s timer instead, since
/// closing the window saves the geometry and not the settings.
fn register_auto_refresh_callback(
    window: &MainWindow,
    link: &EngineLink,
    config: &AppConfig,
    tray: Option<std::rc::Rc<crate::TrayIcon>>,
) {
    let save_timer = std::rc::Rc::new(slint::Timer::default());

    let window_weak = window.as_weak();
    let engine = link.clone();
    let prev_enabled = std::cell::Cell::new(config.auto_refresh_enabled);
    let settled = std::rc::Rc::clone(&save_timer);
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

        if enabled && !was_enabled {
            info!("auto-refresh: immediate refresh on enable");
            engine.push_params(&win);
            engine.send(EngineCommand::RenderWallpaperNow);
        }

        // What this writes covers anything the timer below was still holding.
        settled.stop();
        config::save_config(&ui_callbacks::read_config_from_window(&win, &engine));
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_auto_refresh_interval_moved(move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        send_auto_refresh(&engine, &win);

        let deferred_window = window_weak.clone();
        let deferred_engine = engine.clone();
        save_timer.start(
            slint::TimerMode::SingleShot,
            AUTO_REFRESH_SAVE_DELAY,
            move || {
                if let Some(win) = deferred_window.upgrade() {
                    config::save_config(&ui_callbacks::read_config_from_window(
                        &win,
                        &deferred_engine,
                    ));
                }
            },
        );
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
        crate::ipc::bind(name)
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
    let about = crate::about::AboutController::default();
    about.register_settings_callback(&window);

    // The tray icon is a top-level Slint component of its own; it must exist
    // before the auto-refresh callback so the two views of that setting can be
    // kept in step.
    let tray = if use_tray {
        crate::tray::create_tray(&window, &link, &about).map(std::rc::Rc::new)
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
    // join on a worker thread.
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
    let _session_end =
        crate::session_end::install(Some(crate::session_end::FORCE_EXIT_AFTER), || {
            let _ = slint::invoke_from_event_loop(|| {
                debug!("session end: quitting the event loop");
                slint::quit_event_loop().ok();
            });
        });

    info!("entering event loop");
    if let Err(e) = slint::run_event_loop_until_quit() {
        error!("the event loop stopped: {e}");
        return ExitCode::FAILURE;
    }
    info!("event loop exited");
    ExitCode::SUCCESS
}
