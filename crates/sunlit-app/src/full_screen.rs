//! What full screen needs from outside `main.slint`.
//!
//! Full screen itself is the window's `full-screen` property, which the
//! `.slint` side toggles and the winit backend keeps in step with the system.
//! Left for Rust: the geometry saved before the window turns into the
//! monitor, the cursor while the controls idle, and leaving before a hide.

use std::time::Duration;

use slint::ComponentHandle;
use tracing::debug;

use crate::MainWindow;

/// Wire the window's full screen callbacks.
///
/// :param window: the main window
pub fn wire(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_full_screen_entering(move || {
        if let Some(win) = window_weak.upgrade() {
            crate::menu::save_geometry(&win);
        }
    });

    // Slint picks the cursor again only on a pointer event, so a pointer at
    // rest would keep the cursor it had. A move to where it already is makes
    // the pick without counting as activity. After the change handler, not
    // inside it, so the event is dispatched as any other.
    let window_weak = window.as_weak();
    window.on_cursor_hidden_changed(move |hidden, x, y| {
        debug!("full screen: cursor hidden {hidden}");
        let window_weak = window_weak.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            if let Some(win) = window_weak.upgrade() {
                win.window()
                    .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                        position: slint::LogicalPosition::new(x, y),
                    });
            }
        });
    });
}

/// Leave full screen if the window is in it, before it hides.
///
/// The window shows again windowed, and the geometry stays the one saved on
/// entering, since the size the window reports right after this is still the
/// monitor's until the system has restored it.
///
/// :param window: the main window
pub fn leave(window: &MainWindow) {
    if window.window().is_fullscreen() {
        debug!("leaving full screen before the window hides");
        // Through the window rather than `leave-full-screen`, which hands the
        // change to the backend only on its next update, after the hide.
        window.window().set_fullscreen(false);
    }
}
