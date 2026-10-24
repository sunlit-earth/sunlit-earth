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
}

/// Show the window when the run starts.
///
/// The event loop is not running yet, so on macOS the activation waits for
/// its first turn, and a start that hides the window on that turn keeps the
/// accessory policy it launched with and never shows a Dock icon.
///
/// :param window: the settings window
/// :param `hides_at_once`: whether `--tray-start hidden` hides it again on the
///     first turn of the event loop
/// :returns: the platform's error when the window cannot be shown
pub fn show_at_startup(
    window: &MainWindow,
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
    platform::hidden(keeps_dock_icon(OTHER_WINDOWS.with(|windows| {
        windows
            .borrow()
            .iter()
            .map(|visible| visible())
            .collect::<Vec<_>>()
    })));
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
        let posted = slint::invoke_from_event_loop(|| {
            ON_REOPEN.with(|slot| {
                if let Some(on_reopen) = slot.borrow().as_ref() {
                    on_reopen();
                }
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
    fn another_window_on_screen_keeps_the_dock_icon() {
        assert!(keeps_dock_icon([true]));
        assert!(keeps_dock_icon([false, true]));
    }
}
