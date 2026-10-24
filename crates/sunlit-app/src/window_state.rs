//! Showing and hiding the settings window.
//!
//! Every place that does either goes through this module: the tray's "Open"
//! and its icon click, the close button in tray mode, the deferred hide of
//! `--tray-start hidden`, the IPC commands, and a second launch. Showing the
//! window also turns the preview on and hiding it turns the preview off, since
//! nobody looks at a preview nobody can see.

use slint::ComponentHandle;
use tracing::warn;

use crate::MainWindow;
use crate::engine_client::EngineLink;

/// Show the window and turn the preview on.
///
/// :param window: the settings window
/// :param engine: the engine whose preview follows the window
pub fn show(window: &MainWindow, engine: &EngineLink) {
    if let Err(e) = window.show() {
        warn!("the window could not be shown: {e}");
    }
    engine.set_preview_enabled(true);
}

/// Hide the window and turn the preview off.
///
/// :param window: the settings window
/// :param engine: the engine whose preview follows the window
pub fn hide(window: &MainWindow, engine: &EngineLink) {
    if let Err(e) = window.hide() {
        warn!("the window could not be hidden: {e}");
    }
    hidden(engine);
}

/// Show the window if it is hidden and hide it if it is shown.
///
/// :param window: the settings window
/// :param engine: the engine whose preview follows the window
/// :returns: whether the window is shown now
pub fn toggle(window: &MainWindow, engine: &EngineLink) -> bool {
    if window.window().is_visible() {
        hide(window, engine);
        false
    } else {
        show(window, engine);
        true
    }
}

/// What follows a hide that somebody else did: the close button in tray mode,
/// where Slint hides the window itself once the handler answers.
///
/// :param engine: the engine whose preview follows the window
pub fn hidden(engine: &EngineLink) {
    engine.set_preview_enabled(false);
}
