//! System tray icon with context menu for background wallpaper management.
//!
//! Runs on a dedicated background thread with its own Win32 message pump.
//! Communication from the tray thread to the Slint event loop uses
//! `slint::invoke_from_event_loop()`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use muda::accelerator::Accelerator;
use muda::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use slint::ComponentHandle;
use tray_icon::{Icon, TrayIconBuilder};

use crate::MainWindow;

/// Generate a 32x32 RGBA icon programmatically (blue circle on transparent).
#[allow(clippy::cast_precision_loss)]
fn generate_tray_icon_rgba() -> Vec<u8> {
    let size: u32 = 32;
    let center = size as f32 / 2.0;
    let radius = center - 1.0;
    let mut buf = vec![0u8; (size * size * 4) as usize];

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= radius {
                let idx = ((y * size + x) * 4) as usize;
                // Blue-green earth-like color
                buf[idx] = 40;      // R
                buf[idx + 1] = 120; // G
                buf[idx + 2] = 200; // B
                buf[idx + 3] = 255; // A
            }
        }
    }

    buf
}

/// Spawn the tray icon thread.
///
/// Creates a system tray icon with a context menu and runs a Win32 message
/// pump to process tray events. Menu actions communicate with the Slint
/// event loop via `invoke_from_event_loop()`.
pub fn spawn_tray_thread(
    window_weak: slint::Weak<MainWindow>,
    config: crate::config::AppConfig,
) {
    std::thread::spawn(move || {
        run_tray(window_weak, &config);
    });
}

/// Menu item IDs as string constants for stable matching.
const ID_REFRESH: &str = "refresh";
const ID_AUTO_REFRESH: &str = "auto_refresh";
const ID_SETTINGS: &str = "settings";
const ID_QUIT: &str = "quit";

#[allow(clippy::needless_pass_by_value)]
fn run_tray(
    window_weak: slint::Weak<MainWindow>,
    config: &crate::config::AppConfig,
) {
    // Build menu
    let menu = Menu::new();

    let no_accel: Option<Accelerator> = None;

    let refresh_item = MenuItem::with_id(
        MenuId::new(ID_REFRESH),
        "Refresh",
        true,
        no_accel,
    );
    let auto_refresh_item = CheckMenuItem::with_id(
        MenuId::new(ID_AUTO_REFRESH),
        "Auto Refresh",
        true,
        config.auto_refresh_enabled,
        no_accel,
    );
    let settings_item = MenuItem::with_id(
        MenuId::new(ID_SETTINGS),
        "Settings",
        true,
        no_accel,
    );
    let quit_item = MenuItem::with_id(
        MenuId::new(ID_QUIT),
        "Quit",
        true,
        no_accel,
    );

    menu.append(&refresh_item).expect("Failed to add Refresh");
    menu.append(&auto_refresh_item).expect("Failed to add Auto Refresh");
    menu.append(&PredefinedMenuItem::separator()).expect("Failed to add separator");
    menu.append(&settings_item).expect("Failed to add Settings");
    menu.append(&PredefinedMenuItem::separator()).expect("Failed to add separator");
    menu.append(&quit_item).expect("Failed to add Quit");

    // Create tray icon
    let icon_rgba = generate_tray_icon_rgba();
    let icon = Icon::from_rgba(icon_rgba, 32, 32).expect("Failed to create tray icon");

    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Sunlit Earth")
        .with_icon(icon)
        .build()
        .expect("Failed to build tray icon");

    // Auto-refresh control: shared atomics between tray and refresh threads
    let auto_refresh_active = Arc::new(AtomicBool::new(config.auto_refresh_enabled));
    let auto_refresh_interval = Arc::new(AtomicU32::new(config.auto_refresh_interval_minutes));

    // Spawn the auto-refresh background thread
    {
        let active = Arc::clone(&auto_refresh_active);
        let interval = Arc::clone(&auto_refresh_interval);
        std::thread::spawn(move || {
            run_auto_refresh_loop(&active, &interval);
        });
    }

    // Win32 message pump (required for tray icon events on Windows)
    let menu_rx = MenuEvent::receiver();

    loop {
        // Pump Windows messages so the tray icon stays responsive
        pump_win32_messages();

        // Poll for menu events (non-blocking)
        if let Ok(event) = menu_rx.try_recv() {
            handle_menu_event(
                &event.id,
                &window_weak,
                &auto_refresh_active,
                &auto_refresh_interval,
            );
        }

        // Small sleep to avoid busy-waiting
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Process pending Win32 messages without blocking.
#[allow(unsafe_code)]
fn pump_win32_messages() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    };

    loop {
        // SAFETY: MSG is a plain-old-data C struct; zeroing is safe.
        let mut msg: MSG = unsafe { std::mem::zeroed() };

        // SAFETY: PeekMessageW reads pending messages from the thread's
        // message queue. The MSG struct is stack-allocated and valid.
        // PM_REMOVE removes messages after reading. Null hWnd means all
        // messages for this thread.
        let has_message = unsafe {
            PeekMessageW(
                &raw mut msg,
                std::ptr::null_mut(),
                0,
                0,
                PM_REMOVE,
            )
        };

        if has_message == 0 {
            break;
        }

        // SAFETY: TranslateMessage and DispatchMessageW process a valid
        // MSG struct retrieved from PeekMessageW above.
        unsafe {
            TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    }
}

fn handle_menu_event(
    id: &MenuId,
    window_weak: &slint::Weak<MainWindow>,
    auto_refresh_active: &Arc<AtomicBool>,
    _auto_refresh_interval: &Arc<AtomicU32>,
) {
    let id_str = id.as_ref();
    match id_str {
        ID_SETTINGS => {
            let ww = window_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = ww.upgrade() {
                    win.show().expect("Failed to show window");
                }
            });
        }
        ID_QUIT => {
            let _ = slint::invoke_from_event_loop(|| {
                slint::quit_event_loop().expect("Failed to quit event loop");
            });
        }
        ID_REFRESH => {
            do_headless_refresh();
        }
        ID_AUTO_REFRESH => {
            toggle_auto_refresh(auto_refresh_active);
        }
        _ => {}
    }
}

/// Perform a one-shot headless wallpaper refresh in the background.
fn do_headless_refresh() {
    std::thread::spawn(|| {
        let config = crate::config::load_config();
        if let Err(e) = do_headless_wallpaper_export(&config) {
            eprintln!("Headless wallpaper export failed: {e}");
        }
    });
}

/// Toggle auto-refresh in the config, update the atomic flag, and save to disk.
fn toggle_auto_refresh(active: &Arc<AtomicBool>) {
    let was_active = active.load(Ordering::Relaxed);
    let now_active = !was_active;
    active.store(now_active, Ordering::Relaxed);

    let mut config = crate::config::load_config();
    config.auto_refresh_enabled = now_active;
    crate::config::save_config(&config);
    eprintln!(
        "Auto-refresh {}",
        if now_active { "enabled" } else { "disabled" }
    );
}

/// Background auto-refresh loop.
///
/// Sleeps for the configured interval, then checks whether auto-refresh is
/// enabled and `has_set_wallpaper` is true. If both conditions are met,
/// performs a headless wallpaper export. The loop runs indefinitely and
/// respects the `active` and `interval` atomics which can be updated from
/// the tray thread at any time.
fn run_auto_refresh_loop(active: &AtomicBool, interval_minutes: &AtomicU32) {
    loop {
        // Sleep in 1-second increments so we can respond to interval changes
        let target_secs = u64::from(interval_minutes.load(Ordering::Relaxed).max(1)) * 60;
        for _ in 0..target_secs {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }

        // Check if auto-refresh is enabled
        if !active.load(Ordering::Relaxed) {
            continue;
        }

        // Re-read config from disk to get current settings and check has_set_wallpaper
        let config = crate::config::load_config();
        if !config.has_set_wallpaper {
            continue;
        }

        eprintln!("Auto-refresh: updating wallpaper...");
        if let Err(e) = do_headless_wallpaper_export(&config) {
            eprintln!("Auto-refresh failed: {e}");
        }
    }
}

/// Shared headless wallpaper export logic used by both "Refresh" and
/// auto-refresh timer.
pub(crate) fn do_headless_wallpaper_export(
    config: &crate::config::AppConfig,
) -> Result<(), String> {
    // Refresh cloud cache before rendering
    if let Err(e) = crate::cloud_fetcher::check_and_refresh_cloud_cache() {
        eprintln!("Warning: cloud cache refresh failed: {e}");
    }

    let (width, height) = crate::wallpaper::get_primary_monitor_resolution()?;
    let pixels = crate::headless::render_wallpaper_headless(config, (width, height), false)?;
    let path = crate::wallpaper::save_wallpaper_image(&pixels, width, height)?;
    crate::wallpaper::set_wallpaper(&path)?;

    eprintln!("Wallpaper updated ({width}x{height})");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_icon_rgba_buffer_size() {
        let buf = generate_tray_icon_rgba();
        assert_eq!(buf.len(), 32 * 32 * 4, "RGBA buffer should be 32x32 pixels");
    }

    #[test]
    fn menu_item_ids_are_unique() {
        let ids = [ID_REFRESH, ID_AUTO_REFRESH, ID_SETTINGS, ID_QUIT];
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "all menu IDs should be unique");
    }
}
