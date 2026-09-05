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
use sunlit_core::memory::MemorySnapshot;
use tracing::{debug, info, warn};

/// A socket that is bound and not yet being served.
///
/// Binding and serving are two steps because the name can only be taken once:
/// a name the platform refuses, or one another instance already holds, is an
/// answer the caller wants before it selects an adapter and builds every
/// texture, not after.
pub struct IpcListener {
    listener: interprocess::local_socket::Listener,
    socket_name: String,
}

/// Take the local socket `socket_name` names.
///
/// The error is a sentence for the log: an app that cannot be reached over IPC
/// still draws the globe, so the caller is expected to carry on without it.
///
/// :param `socket_name`: the name from `--ipc-socket`
/// :returns: the bound socket, or why it could not be taken
pub fn bind(socket_name: &str) -> Result<IpcListener, String> {
    let name = socket_name
        .to_ns_name::<GenericNamespaced>()
        .map_err(|e| format!("`{socket_name}` is not a usable socket name: {e}"))?;
    let listener = ListenerOptions::new()
        .name(name)
        .create_sync()
        .map_err(|e| format!("the socket `{socket_name}` could not be opened: {e}"))?;
    Ok(IpcListener {
        listener,
        socket_name: socket_name.to_owned(),
    })
}

impl IpcListener {
    /// Spawn a background thread that dispatches the commands this socket
    /// receives.
    ///
    /// Commands are dispatched directly via `invoke_from_event_loop` to the
    /// Slint event loop thread. The readiness signal is written once the
    /// accepting thread exists, rather than at the bind, so a client that waits
    /// for it finds a socket somebody is answering on.
    ///
    /// :param `window_weak`: the window the window commands reach
    /// :param engine: the engine the rest reach
    /// :returns: the listener thread, or why it could not be started
    pub fn serve(
        self,
        window_weak: slint::Weak<crate::MainWindow>,
        engine: EngineLink,
    ) -> Result<std::thread::JoinHandle<()>, String> {
        let Self {
            listener,
            socket_name,
        } = self;
        let thread = std::thread::Builder::new()
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
            .map_err(|e| format!("the ipc listener thread could not be started: {e}"))?;

        info!("ipc listener ready on {socket_name}");
        println!("SIGNAL:ipc_listener_ready");
        Ok(thread)
    }
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
                Some(snap) => signal(&MemorySignal::from(&snap).line()),
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

/// The counters `query-memory` answers with.
///
/// Both halves of that contract in one place. The e2e suite reads the line back
/// through [`MemorySignal::parse`], so a field renamed here is a compile error
/// in the suite rather than a panic in a guest half an hour later. What the line
/// looks like on the wire does not change: `CLAUDE.md` calls it a parsing
/// contract and this makes it one the compiler can see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemorySignal {
    pub rss_bytes: u64,
    pub peak_rss_bytes: u64,
    pub private_bytes: u64,
}

impl MemorySignal {
    /// The signal body, without the `SIGNAL:` prefix.
    #[must_use]
    pub fn line(&self) -> String {
        format!(
            "memory rss_bytes={} peak_rss_bytes={} private_bytes={}",
            self.rss_bytes, self.peak_rss_bytes, self.private_bytes
        )
    }

    /// Read the counters back out of a `SIGNAL:memory ...` line.
    ///
    /// `None` where a field is missing or is not a number, which is a line this
    /// program did not write.
    #[must_use]
    pub fn parse(line: &str) -> Option<Self> {
        Some(Self {
            rss_bytes: signal_field(line, "rss_bytes")?.parse().ok()?,
            peak_rss_bytes: signal_field(line, "peak_rss_bytes")?.parse().ok()?,
            private_bytes: signal_field(line, "private_bytes")?.parse().ok()?,
        })
    }
}

impl From<&MemorySnapshot> for MemorySignal {
    fn from(snapshot: &MemorySnapshot) -> Self {
        Self {
            rss_bytes: snapshot.rss_bytes,
            peak_rss_bytes: snapshot.peak_rss_bytes,
            private_bytes: snapshot.private_bytes,
        }
    }
}

/// The plan `displays` answers with, read back off the line.
///
/// The other side of [`crate::displays::signal_line`], which builds it. The
/// producer stays where the plan is computed; this is the one reader, and the
/// round trip is unit-tested below, so a renamed field fails in `cargo unit`
/// rather than in a guest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplaysSignal {
    pub monitors: usize,
    pub mode: String,
    pub anchor: i32,
    pub fell_back: bool,
    /// One `x,y,width,height` per monitor, in the session's own order.
    pub rects: Vec<String>,
    /// One `width x height` per image the plan would export.
    pub images: Vec<String>,
}

impl DisplaysSignal {
    /// Read the plan back out of a `SIGNAL:displays ...` line.
    ///
    /// `None` where a field is missing or is not the shape it is written in.
    #[must_use]
    pub fn parse(line: &str) -> Option<Self> {
        Some(Self {
            monitors: signal_field(line, "monitors")?.parse().ok()?,
            mode: signal_field(line, "mode")?.to_owned(),
            anchor: signal_field(line, "anchor")?.parse().ok()?,
            fell_back: signal_field(line, "fell_back")? != "0",
            rects: signal_list(line, "rects")?,
            images: signal_list(line, "images")?,
        })
    }
}

/// The value of one `key=` field of a signal line.
///
/// Every signal line is key=value pairs with no spaces in any value, which is
/// the shape `query-memory` established and the reason a field can be found by
/// splitting on whitespace.
fn signal_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    line.split_whitespace()
        .find_map(|token| token.strip_prefix(prefix.as_str()))
}

/// The value of one `key=` field that carries a `;`-separated list.
///
/// An empty field is an empty list rather than a list holding one empty entry.
fn signal_list(line: &str, key: &str) -> Option<Vec<String>> {
    let value = signal_field(line, key)?;
    if value.is_empty() {
        return Some(Vec::new());
    }
    Some(value.split(';').map(str::to_owned).collect())
}

pub(crate) fn signal(name: &str) {
    let msg = format!("SIGNAL:{name}\n");
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(msg.as_bytes());
    let _ = lock.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two screens side by side, the second of them primary.
    fn two_screens() -> Vec<sunlit_core::display::Monitor> {
        use sunlit_core::display::Monitor;
        vec![
            Monitor {
                id: "DP-1".to_owned(),
                label: "DP-1".to_owned(),
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                primary: false,
            },
            Monitor {
                id: "DP-2".to_owned(),
                label: "DP-2".to_owned(),
                x: 1920,
                y: 0,
                width: 2560,
                height: 1440,
                primary: true,
            },
        ]
    }

    #[test]
    fn the_memory_counters_survive_the_line_they_are_written_on() {
        let counters = MemorySignal {
            rss_bytes: 123_456,
            peak_rss_bytes: 234_567,
            private_bytes: 345_678,
        };
        let line = format!("SIGNAL:{}", counters.line());
        assert_eq!(MemorySignal::parse(&line), Some(counters));
    }

    /// A field whose name is the tail of another's must not be found by it.
    #[test]
    fn peak_rss_is_not_read_as_rss() {
        let counters = MemorySignal {
            rss_bytes: 1,
            peak_rss_bytes: 2,
            private_bytes: 3,
        };
        let parsed = MemorySignal::parse(&counters.line()).expect("the line it just wrote");
        assert_eq!(parsed, counters);
    }

    #[test]
    fn a_line_without_the_fields_is_not_a_memory_signal() {
        assert_eq!(MemorySignal::parse("SIGNAL:memory_unavailable"), None);
    }

    /// The display plan survives the line it is written on, which is what ties
    /// `displays::signal_line` to the only thing that reads it.
    #[test]
    fn the_display_plan_survives_the_line_it_is_written_on() {
        use sunlit_core::display::layout::DisplayMode;

        let monitors = two_screens();
        let line = format!(
            "SIGNAL:displays {}",
            crate::displays::signal_line(&monitors, DisplayMode::EveryScreen, Some("DP-1"))
        );
        let plan = DisplaysSignal::parse(&line).expect("the line the app just wrote");
        assert_eq!(plan.monitors, monitors.len());
        assert_eq!(plan.mode, DisplayMode::EveryScreen.name());
        assert_eq!(plan.anchor, 0, "the stored id names the first screen");
        assert!(!plan.fell_back);
        assert_eq!(plan.rects, vec!["0,0,1920,1080", "1920,0,2560,1440"]);
        assert_eq!(
            plan.images.len(),
            monitors.len(),
            "every-screen exports one image per screen: {plan:?}"
        );
    }

    /// An anchor this session does not have falls back, and says so.
    #[test]
    fn a_stored_screen_the_session_lost_is_reported_as_a_fallback() {
        use sunlit_core::display::layout::DisplayMode;

        let line = crate::displays::signal_line(
            &two_screens(),
            DisplayMode::OneScreen,
            Some("a-screen-that-is-not-here"),
        );
        let plan = DisplaysSignal::parse(&line).expect("the line the app just wrote");
        assert!(plan.fell_back);
        assert_eq!(plan.images.len(), 1, "one screen takes one image: {plan:?}");
    }

    /// A session with no screens has empty lists rather than lists holding one
    /// empty entry, which is what a naive split would produce.
    #[test]
    fn a_session_with_no_screens_parses_to_empty_lists() {
        use sunlit_core::display::layout::DisplayMode;

        let line = crate::displays::signal_line(&[], DisplayMode::OneScreen, None);
        let plan = DisplaysSignal::parse(&line).expect("the line the app just wrote");
        assert_eq!(plan.monitors, 0);
        assert_eq!(plan.anchor, -1);
        assert!(plan.rects.is_empty());
        assert!(plan.images.is_empty());
    }

    #[test]
    fn a_name_something_else_holds_is_an_error_rather_than_a_panic() {
        let name = format!("sunlit-earth-ipc-test-{}", std::process::id());
        let held = bind(&name).expect("the first listener takes the name");
        let second = bind(&name);
        drop(held);
        assert!(
            second.is_err(),
            "a name already taken has to be refused, not shared"
        );
    }
}
