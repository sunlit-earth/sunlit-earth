//! System tray wiring and single-instance enforcement.
//!
//! The tray icon itself is a `SystemTrayIcon` declared in `ui/main.slint`, so
//! Slint owns the platform integration (Shell notification area on Windows,
//! `NSStatusItem` on macOS, `StatusNotifierItem` on Linux). What is left here
//! is the icon bytes, the callback wiring, and the single-instance mutex.

use slint::ComponentHandle;
use tracing::{debug, info};

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
/// single 4 KiB asset. `cargo xtask bake-icon` writes the file from
/// `assets/icon/sunlit-earth-32.svg`, the variant authored for this size.
const TRAY_RGBA: &[u8] = include_bytes!("../../../assets/icon/baked/tray-32.rgba");

// A bake at another size would otherwise fail inside Slint at startup, in tray
// mode only, which is the one path a headless test never reaches.
const _: () = assert!(TRAY_RGBA.len() == (ICON_SIZE * ICON_SIZE * 4) as usize);

/// The tray icon: the baked mark wrapped in the buffer Slint wants.
pub fn create_icon() -> slint::Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        TRAY_RGBA, ICON_SIZE, ICON_SIZE,
    );
    slint::Image::from_rgba8(buffer)
}

/// Check that no other instance of Sunlit Earth is running.
///
/// Returns the `SingleInstance` guard, which must be kept alive for the entire
/// process lifetime; dropping it releases the OS mutex and allows another
/// instance to start. `None` means an instance is already running, and the
/// caller is expected to exit.
pub fn acquire_single_instance(mutex_name: &str) -> Option<single_instance::SingleInstance> {
    let instance = single_instance::SingleInstance::new(mutex_name)
        .expect("failed to create single-instance mutex");
    instance.is_single().then_some(instance)
}

/// Create the tray icon and wire its callbacks to the window and the engine.
///
/// The returned handle must be kept alive: dropping it removes the icon from
/// the tray.
pub fn create_tray(window: &MainWindow, engine: &EngineLink, about: &AboutController) -> TrayIcon {
    let tray = TrayIcon::new().expect("failed to create tray icon");
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

    tray.show().expect("failed to show tray icon");
    tray
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
