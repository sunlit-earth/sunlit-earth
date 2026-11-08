//! The main window's menu bar, and the one way the app quits.
//!
//! The menu itself is the `MenuBar` in `main.slint`. Its entries that have a
//! control in the panel call that control's callback, and the View toggles
//! write the window's own properties, so what is left here is the callbacks
//! that only the menu has.

use slint::ComponentHandle;
use tracing::{debug, warn};

use sunlit_core::config;

use crate::{CameraReset, MainWindow};

/// Wire the menu bar's own callbacks: Close Window, the web links, Open Data
/// Folder and Reset Camera, and through [`crate::full_screen::wire`] what
/// Enter Full Screen and its F11 need.
///
/// Quit calls `quit-app`, which the version notice shares and
/// `activation::register_version_notice` wires to [`quit`].
///
/// :param window: the main window
pub fn wire(window: &MainWindow) {
    crate::full_screen::wire(window);

    let window_weak = window.as_weak();
    window.on_close_window(move || {
        if let Some(win) = window_weak.upgrade() {
            close(&win);
        }
    });

    window.on_open_url(|url| crate::open::url(&url));

    window.on_open_data_folder(|| {
        if let Some(dir) = sunlit_core::app_data_dir() {
            crate::open::folder(&dir);
        } else {
            warn!("this system has no data directory to open");
        }
    });

    let window_weak = window.as_weak();
    window.on_reset_camera(move || {
        if let Some(win) = window_weak.upgrade() {
            reset_camera(&win);
        }
    });
}

/// Ask the window to close, as its title bar's close button does.
///
/// The close handler `app.rs` installed decides what follows: a hide to the
/// tray, or [`quit`] when the run has no tray.
///
/// :param window: the main window
pub fn close(window: &MainWindow) {
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
}

/// Put north up and the look and the frame back to their defaults, each
/// easing as its own control on the camera panel does. Position and zoom stay.
///
/// :param window: the main window
pub fn reset_camera(window: &MainWindow) {
    for which in [CameraReset::North, CameraReset::Look, CameraReset::Frame] {
        window.invoke_camera_reset(which);
    }
}

/// End the app, keeping the window's position and size when it is on screen.
///
/// Every quit a person asks for comes through here: the File menu's Quit, the
/// tray's, the version notice's and the close button of a run with no tray.
///
/// :param window: the main window
pub fn quit(window: &MainWindow) {
    crate::export::close();
    save_geometry(window);
    debug!("quitting the event loop");
    slint::quit_event_loop().ok();
}

/// Store where the window is and how big, for the next start to open at, when
/// it is [on screen](is_on_screen) and not in full screen, whose size is the
/// monitor's. Otherwise the geometry saved before stays, which for full
/// screen is the window's as it was on entering.
///
/// :param window: the main window
pub fn save_geometry(window: &MainWindow) {
    let window = window.window();
    if !is_on_screen(window) {
        debug!("window not on screen, keeping the saved geometry");
        return;
    }
    if window.is_fullscreen() {
        debug!("window in full screen, keeping the saved geometry");
        return;
    }
    let size = window.size();
    let position = window.position();
    config::save_window_geometry(position.x, position.y, size.width, size.height);
}

/// Whether the window's position and size are the ones a person sees.
///
/// A minimized window is still visible, but Windows parks it at
/// (-32000, -32000), which the next start would reject as off screen.
///
/// :param window: the window to ask
/// :returns: true when it is shown and not minimized
pub fn is_on_screen(window: &slint::Window) -> bool {
    window.is_visible() && !window.is_minimized()
}
