//! IPC control channel for external process communication.
//!
//! When enabled via `--ipc-socket <name>`, a background thread listens on a
//! local socket for single-line commands. This is a fire-and-forget protocol:
//! clients connect, send one command line, and disconnect. No response is sent.
//!
//! Commands that touch the window are dispatched via `invoke_from_event_loop`
//! to the Slint event loop thread, following the recommended Slint cross-thread
//! pattern. Commands that only read process-wide state are answered directly on
//! the listener thread.
//!
//! Supported commands:
//! - `quit` — triggers `slint::quit_event_loop()`
//! - `show-window` — makes the main window visible
//! - `hide-window` — hides the main window
//! - `export-test` — attempts a small GPU export, signals success/failure
//! - `query-memory` — reports the current process memory counters
//! - `memory-report` — prints the full memory report between two signal lines
//! - `set-wallpaper` — renders and publishes the wallpaper, signalling the
//!   outcome once the engine reports it
//! - `displays` — reports the session's monitors and the plan they come to

use std::io::{BufRead, BufReader, Write};

use crate::engine_client::EngineLink;
use interprocess::local_socket::traits::ListenerExt;
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, ToNsName};
use slint::ComponentHandle;
use tracing::{debug, info, warn};

/// Spawn a background thread that listens for IPC commands on a local socket.
///
/// Commands are dispatched directly via `invoke_from_event_loop` to the Slint
/// event loop thread. Returns a `JoinHandle` for the listener thread.
pub fn spawn_ipc_listener(
    socket_name: &str,
    window_weak: slint::Weak<crate::MainWindow>,
    engine: EngineLink,
) -> std::thread::JoinHandle<()> {
    let name = socket_name
        .to_ns_name::<GenericNamespaced>()
        .expect("failed to convert IPC socket name");

    let listener = ListenerOptions::new()
        .name(name)
        .create_sync()
        .expect("failed to create IPC listener");

    info!("ipc listener ready on {socket_name}");
    println!("SIGNAL:ipc_listener_ready");

    std::thread::Builder::new()
        .name("ipc-listener".into())
        .spawn(move || {
            for conn in listener.incoming() {
                match conn {
                    Ok(stream) => {
                        let mut reader = BufReader::new(stream);
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            continue;
                        }
                        let cmd = line.trim().to_owned();
                        drop(reader);
                        dispatch_command(&cmd, &window_weak, &engine);
                    }
                    Err(e) => {
                        warn!("ipc accept error: {e}");
                    }
                }
            }
        })
        .expect("failed to spawn ipc-listener thread")
}

/// Parse and dispatch a single IPC command via `invoke_from_event_loop`.
fn dispatch_command(cmd: &str, window_weak: &slint::Weak<crate::MainWindow>, engine: &EngineLink) {
    match cmd {
        "quit" => {
            debug!("ipc: received quit command, dispatching to event loop");
            slint::invoke_from_event_loop(|| {
                debug!("ipc: executing quit_event_loop");
                slint::quit_event_loop().ok();
            })
            .ok();
        }
        "show-window" => {
            debug!("ipc: received show-window command");
            let ww = window_weak.clone();
            let engine = engine.clone();
            slint::invoke_from_event_loop(move || {
                if let Some(win) = ww.upgrade() {
                    debug!("ipc: showing window");
                    win.show().ok();
                }
                engine.set_preview_enabled(true);
                signal("window_shown");
            })
            .ok();
        }
        "hide-window" => {
            debug!("ipc: received hide-window command");
            let ww = window_weak.clone();
            let engine = engine.clone();
            slint::invoke_from_event_loop(move || {
                if let Some(win) = ww.upgrade() {
                    debug!("ipc: hiding window");
                    win.hide().ok();
                }
                engine.set_preview_enabled(false);
                signal("window_hidden");
            })
            .ok();
        }
        "export-test" => {
            debug!("ipc: received export-test command");
            // Answered on this thread: the engine owns the GPU and replies on
            // its own channel, so the probe works while the window is hidden
            // and the Slint event loop has nothing to do.
            match engine.export_pixels(64, 64) {
                Ok(_) => {
                    debug!("ipc: export-test succeeded");
                    signal("export_test_ok");
                }
                Err(e) => {
                    debug!("ipc: export-test failed: {e}");
                    signal("export_test_failed");
                }
            }
        }
        "set-wallpaper" => {
            debug!("ipc: received set-wallpaper command");
            // Fire and forget: the engine renders at the sink's native
            // resolution and publishes, then reports on its own channel. The
            // `wallpaper_set` or `wallpaper_failed` signal comes from the event
            // forwarder when that reply arrives, so a test waits for the
            // outcome rather than for this command to return.
            engine.send(sunlit_core::engine::EngineCommand::RenderWallpaperNow);
        }
        "displays" => {
            debug!("ipc: received displays command");
            report_displays();
        }
        "query-memory" => {
            debug!("ipc: received query-memory command");
            // Answered on this thread: GetProcessMemoryInfo is process-wide,
            // so the reply is correct even when the event loop is idle or busy.
            match sunlit_core::memory::snapshot() {
                Some(snap) => signal(&format!(
                    "memory rss_bytes={} peak_rss_bytes={} private_bytes={}",
                    snap.rss_bytes, snap.peak_rss_bytes, snap.private_bytes
                )),
                None => signal("memory_unavailable"),
            }
        }
        "memory-report" => {
            debug!("ipc: received memory-report command");
            // Answered on this thread for the same reason `export-test` is: the
            // engine owns the device and replies on its own channel, so the
            // report arrives whether or not the event loop has anything to do.
            //
            // A separate command from `query-memory`, whose single line is a
            // parsing contract the e2e suite depends on. This one brackets its
            // own output instead, because the report is many lines and only the
            // section names are promised.
            match engine.memory_report() {
                Ok(report) => {
                    signal("memory_report_begin");
                    print!("{report}");
                    let _ = std::io::stdout().flush();
                    signal("memory_report_end");
                }
                Err(e) => {
                    debug!("ipc: memory-report failed: {e}");
                    signal("memory_report_failed");
                }
            }
        }
        "" => {}
        _ => {
            warn!("unknown ipc command: {cmd}");
        }
    }
}

/// Answer the `displays` command with the session's layout on one line.
///
/// Answered on the listener thread, like `query-memory`: the monitor list is a
/// platform query rather than window state, and the mode and the anchor are
/// written to the config the moment either changes, so the file is the same plan
/// the engine is holding. One line in the shape `query-memory` established,
/// because the e2e suite parses it.
fn report_displays() {
    use sunlit_core::engine::wallpaper_sink::{SystemWallpaper, WallpaperSink};

    let config = sunlit_core::config::load_config();
    match SystemWallpaper.monitors() {
        Ok(monitors) => signal(&format!(
            "displays {}",
            crate::displays::signal_line(
                &monitors,
                config.display_mode,
                config.anchor().as_deref()
            )
        )),
        Err(e) => {
            debug!("ipc: the monitors could not be listed: {e}");
            signal("displays_unavailable");
        }
    }
}

pub(crate) fn signal(name: &str) {
    let msg = format!("SIGNAL:{name}\n");
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(msg.as_bytes());
    let _ = lock.flush();
}
