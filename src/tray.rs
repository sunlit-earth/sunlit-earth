//! System tray icon with context menu and single-instance enforcement.
//!
//! This module provides:
//! - A programmatically generated 32x32 Earth-like tray icon
//! - A background thread with a platform-specific message pump for tray events
//! - Single-instance enforcement via an OS-level mutex
//!
//! The tray icon, menu, and event handlers are cross-platform (via the
//! `tray-icon` crate). Only the message pump is platform-specific — currently
//! implemented for Windows only.

use slint::ComponentHandle;
use tray_icon::Icon;
use tracing::{debug, info};

/// Icon dimensions (width and height in pixels).
const ICON_SIZE: u32 = 32;

/// Generate a 32x32 RGBA tray icon with an Earth-like blue-green gradient.
///
/// The icon is a filled circle on a transparent background. The left side
/// is ocean blue, the right side is land green, giving an Earth-like feel.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn create_icon() -> Icon {
    let size = ICON_SIZE as usize;
    let mut rgba = vec![0u8; size * size * 4];
    let center = size as f32 / 2.0;
    let radius = center - 1.0;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();

            let idx = (y * size + x) * 4;
            if dist <= radius {
                // Blend from ocean blue (left) to land green (right)
                let t = dx / radius * 0.5 + 0.5; // 0.0 = left, 1.0 = right
                rgba[idx] = (30.0 + t * 30.0) as u8; // R: 30-60
                rgba[idx + 1] = (80.0 + t * 100.0) as u8; // G: 80-180
                rgba[idx + 2] = (180.0 - t * 120.0) as u8; // B: 180-60
                rgba[idx + 3] = 255;
            }
            // else: transparent (already zeroed)
        }
    }

    Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE).expect("valid 32x32 RGBA icon")
}

/// Check that no other instance of Sunlit Earth is running.
///
/// Returns the `SingleInstance` guard, which must be kept alive for the
/// entire process lifetime. Dropping it releases the OS mutex and allows
/// another instance to start.
///
/// If another instance is already running, this function logs a message
/// and exits the process with code 0.
pub fn enforce_single_instance(mutex_name: &str) -> single_instance::SingleInstance {
    let instance =
        single_instance::SingleInstance::new(mutex_name).expect("failed to create single-instance mutex");
    if !instance.is_single() {
        info!("another instance is already running, exiting");
        std::process::exit(0);
    }
    instance
}

/// Spawn a background thread that creates a system tray icon with a
/// context menu ("Open" / "Exit") and runs a Win32 message pump.
///
/// The thread runs until the message pump receives `WM_QUIT` (which
/// happens when the process is exiting). The `TrayIcon` is `!Send` and
/// lives entirely on the spawned thread.
///
/// `window_weak` is a weak reference to the main Slint window, used
/// to show/hide the window from tray menu actions. Returns a
/// `JoinHandle` for the tray thread (caller should keep it alive).
pub fn spawn_tray_thread(window_weak: slint::Weak<crate::MainWindow>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("tray-icon".into())
        .spawn(move || {
            run_tray_event_loop(window_weak);
        })
        .expect("failed to spawn tray-icon thread")
}

/// Create the tray icon, register event handlers, and run the Win32
/// message pump. This function blocks until the message pump exits.
fn run_tray_event_loop(window_weak: slint::Weak<crate::MainWindow>) {
    use tray_icon::menu::{Menu, MenuItem};
    use tray_icon::{TrayIconBuilder, TrayIconEvent};

    let menu = Menu::new();
    let open_item = MenuItem::new("Open", true, None);
    let exit_item = MenuItem::new("Exit", true, None);
    menu.append(&open_item).expect("failed to add Open menu item");
    menu.append(&exit_item).expect("failed to add Exit menu item");

    let open_id = open_item.id().clone();
    let exit_id = exit_item.id().clone();

    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Sunlit Earth")
        .with_icon(create_icon())
        .with_menu_on_left_click(false)
        .build()
        .expect("failed to build tray icon");

    // Menu event handler: "Open" shows the window, "Exit" quits.
    let window_weak_menu = window_weak.clone();
    tray_icon::menu::MenuEvent::set_event_handler(Some(move |event: tray_icon::menu::MenuEvent| {
        if event.id == open_id {
            let ww = window_weak_menu.clone();
            slint::invoke_from_event_loop(move || {
                if let Some(win) = ww.upgrade() {
                    debug!("main window shown (from tray menu)");
                    crate::memory::log_memory_usage("after window shown");
                    win.show().ok();
                }
            })
            .ok();
        } else if event.id == exit_id {
            debug!("exit requested from tray menu");
            slint::invoke_from_event_loop(move || {
                slint::quit_event_loop().ok();
            })
            .ok();
        }
    }));

    // Left-click on the tray icon shows the window.
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click { button: tray_icon::MouseButton::Left, .. } = event {
            let ww = window_weak.clone();
            slint::invoke_from_event_loop(move || {
                if let Some(win) = ww.upgrade() {
                    debug!("main window shown (from tray left-click)");
                    crate::memory::log_memory_usage("after window shown");
                    win.show().ok();
                }
            })
            .ok();
        }
    }));

    // Run the platform message pump so tray events are dispatched.
    run_message_pump();

    // _tray_icon is dropped here when the thread exits.
}

/// Run the platform message pump so `tray-icon` events are dispatched.
///
/// This blocks until the pump exits (e.g. `WM_QUIT` on Windows). Each
/// platform needs its own event loop — currently only Windows is
/// implemented.
#[cfg(windows)]
#[allow(unsafe_code)]
fn run_message_pump() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MSG, TranslateMessage,
    };

    let mut msg: MSG = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: `GetMessageW` is safe to call with a valid MSG pointer and
        // null HWND (receives messages for all windows on this thread).
        // The zero min/max filter values mean all messages are received.
        let ret = unsafe { GetMessageW(&raw mut msg, std::ptr::null_mut(), 0, 0) };
        if ret <= 0 {
            break;
        }
        // SAFETY: `TranslateMessage` and `DispatchMessageW` are safe to call
        // with a valid MSG pointer obtained from `GetMessageW`.
        unsafe {
            TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    }
}

/// Placeholder message pump for non-Windows platforms.
///
/// Parks the thread indefinitely — tray events won't fire until a
/// platform-specific event loop (e.g. GLib on Linux, CFRunLoop on macOS)
/// is implemented here.
#[cfg(not(windows))]
fn run_message_pump() {
    std::thread::park();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_is_32x32() {
        // create_icon() validates dimensions internally and panics on mismatch,
        // so simply calling it is the assertion.
        let _icon = create_icon();
    }
}
