//! IPC control channel for external process communication.
//!
//! When enabled via `--ipc-socket <name>`, a background thread listens on a
//! local socket for single-line commands. This is a fire-and-forget protocol:
//! clients connect, send one command line, and disconnect. No response is sent.
//!
//! Commands are dispatched directly via `invoke_from_event_loop` to the Slint
//! event loop thread, following the recommended Slint cross-thread pattern.
//!
//! Supported commands:
//! - `quit` — triggers `slint::quit_event_loop()`
//! - `show-window` — makes the main window visible
//! - `hide-window` — hides the main window

use std::io::{BufRead, BufReader, Write};

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
                        dispatch_command(&cmd, &window_weak);
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
fn dispatch_command(cmd: &str, window_weak: &slint::Weak<crate::MainWindow>) {
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
            slint::invoke_from_event_loop(move || {
                if let Some(win) = ww.upgrade() {
                    debug!("ipc: showing window");
                    win.show().ok();
                }
                signal("window_shown");
            })
            .ok();
        }
        "hide-window" => {
            debug!("ipc: received hide-window command");
            let ww = window_weak.clone();
            slint::invoke_from_event_loop(move || {
                if let Some(win) = ww.upgrade() {
                    debug!("ipc: hiding window");
                    win.hide().ok();
                }
                signal("window_hidden");
            })
            .ok();
        }
        "" => {}
        _ => {
            warn!("unknown ipc command: {cmd}");
        }
    }
}

fn signal(name: &str) {
    let msg = format!("SIGNAL:{name}\n");
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(msg.as_bytes());
    let _ = lock.flush();
}
