//! IPC control channel for external process communication.
//!
//! When enabled via `--ipc-socket <name>`, a background thread listens on a
//! local socket for single-line commands. This is a fire-and-forget protocol:
//! clients connect, send one command line, and disconnect. No response is sent.
//!
//! Supported commands:
//! - `quit` — triggers `slint::quit_event_loop()`
//! - `show-window` — makes the main window visible
//! - `hide-window` — hides the main window

use std::io::{BufRead, BufReader};

use interprocess::local_socket::traits::ListenerExt;
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, ToNsName};
use slint::ComponentHandle;
use tracing::{debug, info, warn};

/// Spawn a background thread that listens for IPC commands on a local socket.
///
/// `socket_name` is converted to a platform-specific namespaced name.
/// `window_weak` is used to dispatch show/hide commands on the UI thread.
///
/// Returns a `JoinHandle` for the listener thread. The handle should be kept
/// alive for the duration of the program; the thread terminates when the
/// process exits.
pub fn spawn_ipc_listener(
    socket_name: &str,
    window_weak: slint::Weak<crate::MainWindow>,
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
                                Ok(cmd) => dispatch_command(cmd.trim(), &window_weak),
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

/// Dispatch a single IPC command to the appropriate Slint action.
fn dispatch_command(cmd: &str, window_weak: &slint::Weak<crate::MainWindow>) {
    match cmd {
        "quit" => {
            debug!("ipc command: quit");
            slint::invoke_from_event_loop(|| {
                slint::quit_event_loop().ok();
            })
            .ok();
        }
        "show-window" => {
            let weak = window_weak.clone();
            slint::invoke_from_event_loop(move || {
                if let Some(win) = weak.upgrade() {
                    win.show().ok();
                    debug!("main window shown (from ipc)");
                }
            })
            .ok();
        }
        "hide-window" => {
            let weak = window_weak.clone();
            slint::invoke_from_event_loop(move || {
                if let Some(win) = weak.upgrade() {
                    win.window().hide().ok();
                    debug!("main window hidden (from ipc)");
                }
            })
            .ok();
        }
        "" => {} // Ignore empty lines
        _ => {
            warn!("unknown ipc command: {cmd}");
        }
    }
}
