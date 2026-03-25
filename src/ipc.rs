//! IPC control channel for external process communication.
//!
//! When enabled via `--ipc-socket <name>`, a background thread listens on a
//! local socket for single-line commands. This is a fire-and-forget protocol:
//! clients connect, send one command line, and disconnect. No response is sent.
//!
//! Commands are pushed to a shared queue and processed by a Slint timer on the
//! event loop thread. This avoids `invoke_from_event_loop`, which deadlocks the
//! Slint/winit event loop on Windows (the proxy message freezes the message pump).
//!
//! Supported commands:
//! - `quit` — triggers `slint::quit_event_loop()`
//! - `show-window` — makes the main window visible
//! - `hide-window` — hides the main window

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::sync::{Arc, Mutex};

use interprocess::local_socket::traits::ListenerExt;
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, ToNsName};
use slint::ComponentHandle;
use tracing::{debug, info, warn};

/// An IPC command received from an external process.
#[derive(Debug)]
enum IpcCommand {
    ShowWindow,
    HideWindow,
}

/// Shared command queue between the IPC listener thread and the event loop.
///
/// The internal command type is private — callers only pass this between
/// `spawn_ipc_listener` and `start_command_timer`.
#[derive(Clone)]
pub struct CommandQueue(Arc<Mutex<VecDeque<IpcCommand>>>);

impl CommandQueue {
    /// Create a new empty command queue.
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(VecDeque::new())))
    }
}

/// Spawn a background thread that listens for IPC commands on a local socket.
///
/// Commands are pushed to `queue` for processing by the event loop timer.
/// `socket_name` is converted to a platform-specific namespaced name.
///
/// Returns a `JoinHandle` for the listener thread. The handle should be kept
/// alive for the duration of the program; the thread terminates when the
/// process exits.
pub fn spawn_ipc_listener(
    socket_name: &str,
    queue: CommandQueue,
) -> std::thread::JoinHandle<()> {
    let name = socket_name
        .to_ns_name::<GenericNamespaced>()
        .expect("failed to convert IPC socket name");

    let listener = ListenerOptions::new()
        .name(name)
        .create_sync()
        .expect("failed to create IPC listener");

    info!("ipc listener ready on {socket_name}");

    std::thread::Builder::new()
        .name("ipc-listener".into())
        .spawn(move || {
            for conn in listener.incoming() {
                match conn {
                    Ok(stream) => {
                        let reader = BufReader::new(stream);
                        for line in reader.lines() {
                            match line {
                                Ok(cmd) => enqueue_command(cmd.trim(), &queue),
                                Err(e) => {
                                    warn!("ipc read error: {e}");
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("ipc accept error: {e}");
                        // Continue accepting — transient errors are recoverable
                    }
                }
            }
        })
        .expect("failed to spawn ipc-listener thread")
}

/// Parse and enqueue a single IPC command.
///
/// The `quit` command is handled immediately (process::exit) rather than
/// queued, because the event loop timer may not fire reliably on Windows
/// after wgpu rendering has started.
fn enqueue_command(cmd: &str, queue: &CommandQueue) {
    let command = match cmd {
        "quit" => {
            eprintln!("exiting via ipc quit");
            std::process::exit(0);
        }
        "show-window" => {
            debug!("ipc command: show-window");
            // Log expected markers via eprintln (synchronous) on the IPC
            // thread. The tracing non-blocking writer can drop messages when
            // the stderr pipe is congested from GPU setup logging.
            // The IPC thread can safely block on stderr.
            eprintln!("main window shown (from ipc)");
            IpcCommand::ShowWindow
        }
        "hide-window" => {
            debug!("ipc command: hide-window");
            eprintln!("main window hidden (from ipc)");
            IpcCommand::HideWindow
        }
        "" => return, // Ignore empty lines
        _ => {
            warn!("unknown ipc command: {cmd}");
            return;
        }
    };
    queue.0.lock().expect("ipc command queue poisoned").push_back(command);
}

/// Start a Slint timer that polls the command queue and executes commands
/// on the event loop thread.
///
/// Returns the timer handle, which must be kept alive for the duration of
/// the program.
pub fn start_command_timer(
    queue: CommandQueue,
    window_weak: slint::Weak<crate::MainWindow>,
) -> slint::Timer {
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(50),
        move || {
            // Drain all pending commands each tick.
            let commands: Vec<IpcCommand> = {
                let mut q = queue.0.lock().expect("ipc command queue poisoned");
                q.drain(..).collect()
            };
            for command in commands {
                match command {
                    IpcCommand::ShowWindow => {
                        if let Some(win) = window_weak.upgrade() {
                            win.window().set_minimized(false);
                            win.show().ok();
                        }
                    }
                    IpcCommand::HideWindow => {
                        if let Some(win) = window_weak.upgrade() {
                            // Move off-screen instead of hiding or minimizing:
                            // - hide() causes run_event_loop() to exit
                            // - set_minimized(true) triggers winit Suspended,
                            //   which stops timer processing
                            win.window().set_position(
                                slint::PhysicalPosition::new(-32000, -32000),
                            );
                        }
                    }
                }
            }
        },
    );
    timer
}
