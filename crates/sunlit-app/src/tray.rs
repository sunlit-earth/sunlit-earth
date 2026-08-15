//! System tray wiring and single-instance enforcement.
//!
//! The tray icon itself is a `SystemTrayIcon` declared in `ui/main.slint`, so
//! Slint owns the platform integration (Shell notification area on Windows,
//! `NSStatusItem` on macOS, `StatusNotifierItem` on Linux). What is left here
//! is the procedurally generated icon, the callback wiring, and the
//! single-instance mutex.

use slint::ComponentHandle;
use tracing::{debug, info};

use sunlit_core::engine::EngineCommand;

use crate::engine_client::EngineLink;
use crate::{MainWindow, TrayIcon};

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
pub fn create_icon() -> slint::Image {
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

    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &rgba, ICON_SIZE, ICON_SIZE,
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
    let instance =
        single_instance::SingleInstance::new(mutex_name).expect("failed to create single-instance mutex");
    instance.is_single().then_some(instance)
}

/// Create the tray icon and wire its callbacks to the window and the engine.
///
/// The returned handle must be kept alive: dropping it removes the icon from
/// the tray.
pub fn create_tray(window: &MainWindow, engine: &EngineLink) -> TrayIcon {
    let tray = TrayIcon::new().expect("failed to create tray icon");
    tray.set_tray_image(create_icon());
    tray.set_auto_refresh_enabled(window.get_auto_refresh_enabled());

    let window_weak = window.as_weak();
    tray.on_open_window(move || {
        if let Some(win) = window_weak.upgrade() {
            debug!("tray: showing window");
            sunlit_core::memory::log_memory_usage("after window shown");
            win.show().ok();
        }
    });

    let window_weak = window.as_weak();
    tray.on_toggle_window(move || {
        if let Some(win) = window_weak.upgrade() {
            if win.window().is_visible() {
                debug!("tray: hiding window (icon clicked)");
                win.hide().ok();
            } else {
                debug!("tray: showing window (icon clicked)");
                sunlit_core::memory::log_memory_usage("after window shown");
                win.show().ok();
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
}
