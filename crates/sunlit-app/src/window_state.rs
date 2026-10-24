//! Showing and hiding the settings window.
//!
//! Every place that does either goes through this module: the tray's "Open"
//! and its icon click, the close button in tray mode, the deferred hide of
//! `--tray-start hidden`, the IPC commands, a second launch, and on macOS the
//! Dock's reopen. Showing the window also turns the preview on and hiding it
//! turns the preview off, since nobody looks at a preview nobody can see.
//!
//! On macOS the Dock icon follows the window. The bundle declares
//! `LSUIElement`, so it starts as an accessory app with no Dock icon; showing
//! the window makes it a regular app and activates it, in the order that keeps
//! the menu bar from showing the previous app's menus (window, policy,
//! activation), and hiding it makes it an accessory again unless another of
//! its windows, the About window, is still on screen.

use std::cell::RefCell;
use std::time::Duration;

use slint::ComponentHandle;
use tracing::warn;

use crate::MainWindow;
use crate::engine_client::EngineLink;

thread_local! {
    /// Whether each of the app's other windows is on screen, asked when the
    /// settings window hides.
    static OTHER_WINDOWS: RefCell<Vec<Box<dyn Fn() -> bool>>> = RefCell::new(Vec::new());

    /// The settings window, asked whether it is on screen when another window
    /// closes.
    static MAIN_WINDOW: RefCell<Option<slint::Weak<MainWindow>>> = const { RefCell::new(None) };
}

/// Show the window and turn the preview on.
///
/// :param window: the settings window
/// :param engine: the engine whose preview follows the window
pub fn show(window: &MainWindow, engine: &EngineLink) {
    if let Err(e) = window.show() {
        warn!("the window could not be shown: {e}");
    }
    platform::shown();
    engine.set_preview_enabled(true);
    crate::autostart::ui::refresh(window);
}

/// Show the window, bring it to the front and give it the keyboard, for a
/// second launch.
///
/// Windows lets the process take the foreground because the launch called
/// `AllowSetForegroundWindow` for it. On X11 the launch's startup id goes on
/// the window before it is mapped and its time goes with the request, so a
/// window manager that prevents focus stealing can tell the user asked for it.
/// On Wayland the token cannot reach the window Slint creates on show, so the
/// compositor decides, and most mark the window rather than raise it. On macOS
/// [`show`] already activates the app.
///
/// :param window: the settings window
/// :param engine: the engine whose preview follows the window
/// :param `startup_id`: the X11 startup notification id the launch carried
pub fn show_and_focus(window: &MainWindow, engine: &EngineLink, startup_id: Option<&str>) {
    if let (Some(handle), Some(id)) = (raw_handle(window), startup_id) {
        focus::before_show(handle, id);
    }
    show(window, engine);
    if let Some(handle) = raw_handle(window) {
        focus::after_show(handle, startup_id);
    } else {
        tracing::debug!("the window has no handle yet to bring to the front");
    }
}

fn raw_handle(window: &MainWindow) -> Option<raw_window_handle::RawWindowHandle> {
    use raw_window_handle::HasWindowHandle;
    window
        .window()
        .window_handle()
        .window_handle()
        .ok()
        .map(|handle| handle.as_raw())
}

/// Show the window when the run starts.
///
/// The event loop is not running yet, so on macOS the activation waits for
/// its first turn, and a start that hides the window on that turn keeps the
/// accessory policy it launched with and never shows a Dock icon.
///
/// :param window: the settings window, or the one window a launch that found
///     an older version running opens
/// :param `hides_at_once`: whether `--tray-start hidden` hides it again on the
///     first turn of the event loop
/// :returns: the platform's error when the window cannot be shown
pub fn show_at_startup(
    window: &impl slint::ComponentHandle,
    hides_at_once: bool,
) -> Result<(), slint::PlatformError> {
    window.show()?;
    if !hides_at_once {
        slint::Timer::single_shot(Duration::ZERO, platform::shown);
    }
    Ok(())
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
    platform::hidden(keeps_dock_icon(others_visible()));
}

fn others_visible() -> Vec<bool> {
    OTHER_WINDOWS.with(|windows| windows.borrow().iter().map(|visible| visible()).collect())
}

/// Remember the settings window, which [`other_window_closed`] asks about.
pub fn register_main_window(window: &MainWindow) {
    MAIN_WINDOW.with(|slot| *slot.borrow_mut() = Some(window.as_weak()));
}

/// What follows the close of another of the app's windows, the About window:
/// on macOS the Dock icon goes once no window of the app is left on screen.
///
/// Slint hides the closed window after its close handler answers, so the
/// windows are looked at on the next turn of the event loop.
pub fn other_window_closed() {
    slint::Timer::single_shot(Duration::ZERO, || {
        let main_visible = MAIN_WINDOW.with(|slot| {
            slot.borrow()
                .as_ref()
                .and_then(slint::Weak::upgrade)
                .is_some_and(|window| window.window().is_visible())
        });
        if drops_dock_icon_after_close(main_visible, others_visible()) {
            platform::hidden(false);
        }
    });
}

/// Whether closing another window leaves the app with no window on screen.
fn drops_dock_icon_after_close(
    main_visible: bool,
    others_visible: impl IntoIterator<Item = bool>,
) -> bool {
    !main_visible && !keeps_dock_icon(others_visible)
}

/// Count another of the app's windows in when the settings window hides.
///
/// :param `is_visible`: whether that window is on screen at the moment it is
///     asked
pub fn register_other_window(is_visible: impl Fn() -> bool + 'static) {
    OTHER_WINDOWS.with(|windows| windows.borrow_mut().push(Box::new(is_visible)));
}

/// Whether the app keeps its Dock icon once the settings window is hidden.
fn keeps_dock_icon(others_visible: impl IntoIterator<Item = bool>) -> bool {
    others_visible.into_iter().any(|visible| visible)
}

/// Have the Dock's reopen, a click on the Dock icon or a launch of the app
/// that is already running, call `on_reopen` on the event loop.
///
/// After the window exists, because that is what brings up the winit event
/// loop whose application delegate the handler is added to, and before the
/// event loop runs. Anywhere but macOS there is no such event and this does
/// nothing.
///
/// :param `on_reopen`: what a reopen does
pub fn install_reopen_handler(on_reopen: impl Fn() + 'static) {
    platform::install_reopen_handler(on_reopen);
}

#[cfg(target_os = "macos")]
mod platform {
    use std::cell::RefCell;

    use objc2::MainThreadMarker;
    use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
    use objc2::sel;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use tracing::{debug, warn};

    thread_local! {
        static ON_REOPEN: RefCell<Option<Box<dyn Fn()>>> = RefCell::new(None);
    }

    pub(super) fn shown() {
        let Some(mtm) = MainThreadMarker::new() else {
            warn!("the activation policy can only be changed on the main thread");
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        if !app.setActivationPolicy(NSApplicationActivationPolicy::Regular) {
            warn!("the app could not be given a Dock icon");
        }
        #[expect(
            deprecated,
            reason = "`activate` needs macOS 14 and the app runs on 11"
        )]
        app.activateIgnoringOtherApps(true);
    }

    pub(super) fn hidden(keeps_dock_icon: bool) {
        if keeps_dock_icon {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            warn!("the activation policy can only be changed on the main thread");
            return;
        };
        if !NSApplication::sharedApplication(mtm)
            .setActivationPolicy(NSApplicationActivationPolicy::Accessory)
        {
            warn!("the app's Dock icon could not be removed");
        }
    }

    extern "C-unwind" fn should_handle_reopen(
        _this: *mut AnyObject,
        _cmd: Sel,
        _app: *mut AnyObject,
        _has_visible_windows: Bool,
    ) -> Bool {
        debug!("the Dock asked the app to reopen");
        // Through a zero timer, as a second launch's activation goes, so a
        // reopen during a hidden start lands after the deferred hide.
        let posted = slint::invoke_from_event_loop(|| {
            slint::Timer::single_shot(std::time::Duration::ZERO, || {
                ON_REOPEN.with(|slot| {
                    if let Some(on_reopen) = slot.borrow().as_ref() {
                        on_reopen();
                    }
                });
            });
        });
        if let Err(e) = posted {
            warn!("the reopen could not reach the event loop: {e}");
        }
        Bool::NO
    }

    pub(super) fn install_reopen_handler(on_reopen: impl Fn() + 'static) {
        ON_REOPEN.with(|slot| *slot.borrow_mut() = Some(Box::new(on_reopen)));

        let sel = sel!(applicationShouldHandleReopen:hasVisibleWindows:);
        let Some(class) = AnyClass::get(c"WinitApplicationDelegate") else {
            warn!(
                "winit's application delegate is not registered, so the Dock cannot reopen the window"
            );
            return;
        };
        if class.instance_method(sel).is_some() {
            warn!(
                "winit's application delegate already handles reopen, so the app's handler is not added"
            );
            return;
        }
        let handler: extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, Bool) -> Bool =
            should_handle_reopen;
        // SAFETY: the class is a registered Objective-C class, the selector
        // has no implementation on it (checked above), and the function has
        // the signature `- (BOOL)applicationShouldHandleReopen:(NSApplication
        // *)sender hasVisibleWindows:(BOOL)flag` declares, which the type
        // string spells; `Imp` is the type-erased form `class_addMethod`
        // takes, the same transmute Slint does for its own delegate method.
        #[allow(unsafe_code)]
        let added = unsafe {
            objc2::ffi::class_addMethod(
                std::ptr::from_ref(class).cast_mut(),
                sel,
                std::mem::transmute::<
                    extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, Bool) -> Bool,
                    Imp,
                >(handler),
                c"B@:@B".as_ptr(),
            )
        };
        if added.as_bool() {
            debug!("the Dock's reopen is handled");
        } else {
            warn!("the reopen handler could not be added to winit's application delegate");
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub(super) fn shown() {}

    pub(super) fn hidden(_keeps_dock_icon: bool) {}

    pub(super) fn install_reopen_handler(_on_reopen: impl Fn() + 'static) {}
}

#[cfg(windows)]
mod focus {
    use raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };

    pub(super) fn before_show(_handle: RawWindowHandle, _startup_id: &str) {}

    pub(super) fn after_show(handle: RawWindowHandle, _startup_id: Option<&str>) {
        let RawWindowHandle::Win32(handle) = handle else {
            return;
        };
        let hwnd = handle.hwnd.get() as HWND;
        // SAFETY: `hwnd` is this process's own window, just shown; all three
        // calls take it by value, touch no memory of ours, and fail harmlessly
        // on a handle that is no longer valid.
        #[allow(unsafe_code)]
        let foreground = unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd)
        };
        if foreground == 0 {
            tracing::debug!(
                "Windows did not let the window take the foreground, so it flashes in the task bar"
            );
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod focus {
    use raw_window_handle::RawWindowHandle;
    use tracing::debug;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt as _, EventMask, PropMode};
    use x11rb::wrapper::ConnectionExt as _;

    pub(super) fn before_show(handle: RawWindowHandle, startup_id: &str) {
        let Some(window) = x11_window(handle) else {
            return;
        };
        if let Err(e) = set_startup_id(window, startup_id) {
            debug!("the startup id could not be put on the window: {e}");
        }
    }

    pub(super) fn after_show(handle: RawWindowHandle, startup_id: Option<&str>) {
        let Some(window) = x11_window(handle) else {
            return;
        };
        let time = startup_id
            .and_then(crate::activation::startup_time)
            .unwrap_or(x11rb::CURRENT_TIME);
        if let Err(e) = request_focus(window, time) {
            debug!("the window manager could not be asked to focus the window: {e}");
        }
    }

    fn x11_window(handle: RawWindowHandle) -> Option<u32> {
        match handle {
            RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).ok(),
            RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
            _ => None,
        }
    }

    fn atom(conn: &impl Connection, name: &[u8]) -> Result<u32, String> {
        Ok(conn
            .intern_atom(false, name)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?
            .atom)
    }

    fn set_startup_id(window: u32, startup_id: &str) -> Result<(), String> {
        let (conn, _) = x11rb::connect(None).map_err(|e| e.to_string())?;
        let property = atom(&conn, b"_NET_STARTUP_ID")?;
        let utf8 = atom(&conn, b"UTF8_STRING")?;
        conn.change_property8(
            PropMode::REPLACE,
            window,
            property,
            utf8,
            startup_id.as_bytes(),
        )
        .map_err(|e| e.to_string())?;
        conn.flush().map_err(|e| e.to_string())
    }

    /// The EWMH request a pager sends, with source indication 1, an
    /// application, so the window manager weighs it like a user's launch.
    fn request_focus(window: u32, time: u32) -> Result<(), String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
        let root = conn.setup().roots[screen].root;
        let active = atom(&conn, b"_NET_ACTIVE_WINDOW")?;
        let event = ClientMessageEvent::new(32, window, active, [1, time, 0, 0, 0]);
        conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )
        .map_err(|e| e.to_string())?;
        conn.flush().map_err(|e| e.to_string())
    }
}

#[cfg(target_os = "macos")]
mod focus {
    use raw_window_handle::RawWindowHandle;

    pub(super) fn before_show(_handle: RawWindowHandle, _startup_id: &str) {}

    pub(super) fn after_show(_handle: RawWindowHandle, _startup_id: Option<&str>) {}
}

#[cfg(test)]
mod tests {
    use super::keeps_dock_icon;

    #[test]
    fn the_dock_icon_goes_with_the_last_window() {
        assert!(!keeps_dock_icon([]));
        assert!(!keeps_dock_icon([false]));
        assert!(!keeps_dock_icon([false, false]));
    }

    #[test]
    fn closing_the_last_window_on_screen_drops_the_dock_icon() {
        use super::drops_dock_icon_after_close;
        assert!(drops_dock_icon_after_close(false, [false]));
        assert!(drops_dock_icon_after_close(false, []));
        assert!(
            !drops_dock_icon_after_close(true, [false]),
            "the settings window is still on screen"
        );
        assert!(
            !drops_dock_icon_after_close(false, [false, true]),
            "another window is still on screen"
        );
    }

    #[test]
    fn another_window_on_screen_keeps_the_dock_icon() {
        assert!(keeps_dock_icon([true]));
        assert!(keeps_dock_icon([false, true]));
    }
}
