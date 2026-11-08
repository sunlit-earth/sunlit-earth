//! What full screen needs from outside `main.slint`.
//!
//! Full screen itself is the window's `full-screen` property, which the
//! `.slint` side toggles and the winit backend keeps in step with the system.
//! Left for Rust: the geometry saved before the window turns into the
//! monitor, the report that the black frame a switch starts with is on
//! screen, the cursor while the controls idle, and leaving before a hide.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use slint::ComponentHandle;
use tracing::debug;

use crate::MainWindow;

/// How long a switch waits at most for the report that its black frame was
/// drawn.
const BLANK_REPORT_AT_MOST_MS: i64 = 500;

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

    watch_for_the_blank(window);

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

/// Tell the window when the black frame a switch starts with is on screen.
///
/// The renderers that draw through OpenGL report each frame after drawing it
/// and before presenting it, so the switch goes on the event loop's next turn,
/// which comes after the present. The software renderer reports nothing, and
/// there the window's own `switch-wait` is all the switch waits for. Where the
/// report comes, `switch-wait` stays as the bound on it, for a window that
/// draws nothing while it is covered or minimized.
fn watch_for_the_blank(window: &MainWindow) {
    let armed = Rc::new(Cell::new(false));
    let window_weak = window.as_weak();
    let seen = Rc::clone(&armed);
    let notifier = window.window().set_rendering_notifier(move |state, _| {
        if matches!(state, slint::RenderingState::AfterRendering) && seen.take() {
            let window_weak = window_weak.clone();
            slint::Timer::single_shot(Duration::ZERO, move || {
                if let Some(win) = window_weak.upgrade() {
                    win.invoke_blank_presented();
                }
            });
        }
    });
    match notifier {
        Ok(()) => window.set_switch_wait(BLANK_REPORT_AT_MOST_MS),
        Err(e) => debug!("full screen: no frame reports ({e}), the switch waits a fixed time"),
    }
    window.on_blank_requested(move || armed.set(true));
}

/// Leave full screen if the window is in it, or drop a switch not yet made,
/// before it hides.
///
/// The window shows again windowed, and the geometry stays the one saved on
/// entering, since the size the window reports right after this is still the
/// monitor's until the system has restored it.
///
/// :param window: the main window
pub fn leave(window: &MainWindow) {
    window.invoke_cancel_switch();
    if window.window().is_fullscreen() {
        debug!("leaving full screen before the window hides");
        // Through the window rather than `leave-full-screen`, which hands the
        // change to the backend only on its next update, after the hide.
        window.window().set_fullscreen(false);
    }
}
