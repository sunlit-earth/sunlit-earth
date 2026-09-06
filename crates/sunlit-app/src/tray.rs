//! System tray wiring and single-instance enforcement.
//!
//! The tray icon itself is a `SystemTrayIcon` declared in `ui/tray.slint`, so
//! Slint owns the platform integration (Shell notification area on Windows,
//! `NSStatusItem` on macOS, `StatusNotifierItem` on Linux). What is left here
//! is the icon bytes, the callback wiring, and the single-instance mutex.

use slint::ComponentHandle;
use tracing::{debug, info, warn};

use sunlit_core::engine::EngineCommand;

use crate::about::AboutController;
use crate::engine_client::EngineLink;
use crate::{MainWindow, TrayIcon};

/// Icon dimensions (width and height in pixels).
const ICON_SIZE: u32 = 32;

/// The Sunlit Earth mark at [`ICON_SIZE`], as straight RGBA8.
///
/// Raw pixels rather than a PNG, because this is the only image the app owns
/// and decoding one would put an image decoder in its dependency list for a
/// single 4 KiB asset. `cargo xtask bake icon` writes the file from
/// `assets/icon/sunlit-earth-32.svg`, the variant authored for this size.
const TRAY_RGBA: &[u8] = include_bytes!("../../../assets/icon/baked/tray-32.rgba");

// A bake at another size would otherwise fail inside Slint at startup, in tray
// mode only, which is the one path a headless test never reaches.
const _: () = assert!(TRAY_RGBA.len() == (ICON_SIZE * ICON_SIZE * 4) as usize);

/// The tray icon: the baked mark wrapped in the buffer Slint wants.
fn create_icon() -> slint::Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        TRAY_RGBA, ICON_SIZE, ICON_SIZE,
    );
    slint::Image::from_rgba8(buffer)
}

/// What the single-instance check found.
pub enum InstanceCheck {
    /// Nobody else holds the name. The guard, when there is one, must be kept
    /// alive for the entire process lifetime: dropping it releases the OS mutex
    /// and lets another instance start. `None` is a mutex that could not be
    /// created at all, where carrying on alone is the friendlier of the two
    /// wrong answers.
    Alone(Option<single_instance::SingleInstance>),
    /// Another instance holds the name, and this one is expected to exit.
    AlreadyRunning,
}

/// Check that no other instance of Sunlit Earth is running.
///
/// The name is derived from `--ipc-socket`, so it is as much the user's to get
/// wrong as the socket name is: on Windows a backslash in it is a namespace
/// separator and the mutex is refused outright.
///
/// :param `mutex_name`: the process-wide name this instance claims
/// :returns: what the check found
pub fn acquire_single_instance(mutex_name: &str) -> InstanceCheck {
    match single_instance::SingleInstance::new(mutex_name) {
        Ok(instance) if instance.is_single() => InstanceCheck::Alone(Some(instance)),
        Ok(_) => InstanceCheck::AlreadyRunning,
        Err(e) => {
            warn!("the single-instance mutex `{mutex_name}` could not be created: {e}");
            InstanceCheck::Alone(None)
        }
    }
}

/// Create the tray icon and wire its callbacks to the window and the engine.
///
/// The returned handle must be kept alive: dropping it removes the icon from
/// the tray. `None` is a session with no tray to put an icon in, a Linux
/// desktop running no `StatusNotifierItem` host being the ordinary case, and
/// leaves the caller to run windowed.
pub fn create_tray(
    window: &MainWindow,
    engine: &EngineLink,
    about: &AboutController,
) -> Option<TrayIcon> {
    let tray = TrayIcon::new()
        .inspect_err(|e| warn!("this session has no tray to put an icon in: {e}"))
        .ok()?;
    tray.set_tray_image(create_icon());
    tray.set_auto_refresh_enabled(window.get_auto_refresh_enabled());

    let window_weak = window.as_weak();
    let engine_link = engine.clone();
    tray.on_open_window(move || {
        if let Some(win) = window_weak.upgrade() {
            debug!("tray: showing window");
            sunlit_core::memory::log_memory_usage("after window shown");
            win.show().ok();
            engine_link.set_preview_enabled(true);
        }
    });

    let window_weak = window.as_weak();
    let engine_link = engine.clone();
    tray.on_toggle_window(move || {
        if let Some(win) = window_weak.upgrade() {
            if win.window().is_visible() {
                debug!("tray: hiding window (icon clicked)");
                win.hide().ok();
                engine_link.set_preview_enabled(false);
            } else {
                debug!("tray: showing window (icon clicked)");
                sunlit_core::memory::log_memory_usage("after window shown");
                win.show().ok();
                engine_link.set_preview_enabled(true);
            }
        }
    });

    let engine_link = engine.clone();
    tray.on_refresh_now(move || {
        info!("tray: refreshing wallpaper");
        engine_link.send(EngineCommand::RenderWallpaperNow);
    });

    // The tray menu item and the checkbox in the settings window are two views
    // of the same setting, so the tray hands the change to the window and lets
    // the existing auto-refresh callback do the work.
    let window_weak = window.as_weak();
    tray.on_auto_refresh_toggled(move |checked| {
        debug!(checked, "tray: auto-refresh toggled");
        if let Some(win) = window_weak.upgrade() {
            win.set_auto_refresh_enabled(checked);
            win.invoke_auto_refresh_changed();
        }
    });

    let about = about.clone();
    tray.on_show_about(move || {
        let _ = about.show();
    });

    tray.on_exit_app(|| {
        debug!("tray: exit requested, executing quit_event_loop");
        slint::quit_event_loop().ok();
    });

    tray.show()
        .inspect_err(|e| warn!("the tray icon could not be shown: {e}"))
        .ok()?;
    Some(tray)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_is_32x32() {
        let icon = create_icon();
        assert_eq!(icon.size().width, ICON_SIZE);
        assert_eq!(icon.size().height, ICON_SIZE);
    }

    /// The name comes from `--ipc-socket`, and Windows reads the backslash in
    /// it as a namespace separator and refuses the mutex outright.
    #[cfg(windows)]
    #[test]
    fn a_mutex_name_windows_refuses_leaves_the_app_running_alone() {
        assert!(matches!(
            acquire_single_instance(r"sunlit-earth-bad\name"),
            InstanceCheck::Alone(None)
        ));
    }

    #[test]
    fn the_tray_icon_is_a_disk_on_a_transparent_field() {
        // The length assertion above catches a bake at the wrong size; this
        // catches the other way a swap goes wrong, an all-zero or an
        // edge-to-edge buffer, which is a tray icon nobody can see either way.
        let alpha_at = |x: u32, y: u32| TRAY_RGBA[((y * ICON_SIZE + x) * 4 + 3) as usize];
        assert_eq!(alpha_at(ICON_SIZE / 2, ICON_SIZE / 2), 255);
        assert_eq!(alpha_at(0, ICON_SIZE - 1), 0);
    }
}
