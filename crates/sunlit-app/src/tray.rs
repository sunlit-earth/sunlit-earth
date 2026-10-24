//! System tray wiring and single-instance enforcement.
//!
//! The tray icon itself is a `SystemTrayIcon` declared in `ui/tray.slint`, so
//! Slint owns the platform integration (Shell notification area on Windows,
//! `NSStatusItem` on macOS, `StatusNotifierItem` on Linux). What is left here
//! is the icon bytes, the callback wiring, and the single-instance mutex.

use std::time::Duration;

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

/// The single-instance names this run holds. It must be kept alive for the
/// entire process lifetime: dropping it releases them and lets another
/// instance start. It holds nothing when no mutex could be created at all,
/// where carrying on alone is the friendlier of the two wrong answers.
pub struct InstanceGuard {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "held so that dropping it releases the names")
    )]
    held: Vec<single_instance::SingleInstance>,
}

/// What the single-instance check found.
pub enum InstanceCheck {
    /// Nobody else holds the name.
    Alone(InstanceGuard),
    /// Another instance holds the name, and this one is expected to hand over
    /// to it and exit.
    AlreadyRunning,
    /// A version from before the guard's name carried the user's part holds
    /// the old name. It has no activation socket to hand over to, so this one
    /// exits quietly, as such a version did.
    OlderVersionRunning,
}

/// What asking for one name found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Claim {
    Taken,
    HeldElsewhere,
    Failed,
}

/// What the claims on this run's name and on the legacy one come to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Decision {
    Alone,
    AlreadyRunning,
    OlderVersionRunning,
}

fn decide(current: Claim, legacy: Option<Claim>) -> Decision {
    if current == Claim::HeldElsewhere {
        Decision::AlreadyRunning
    } else if legacy == Some(Claim::HeldElsewhere) {
        Decision::OlderVersionRunning
    } else {
        Decision::Alone
    }
}

fn claim(name: &str, held: &mut Vec<single_instance::SingleInstance>) -> Claim {
    match single_instance::SingleInstance::new(name) {
        Ok(instance) if instance.is_single() => {
            held.push(instance);
            Claim::Taken
        }
        Ok(_) => Claim::HeldElsewhere,
        Err(e) => {
            warn!("the single-instance mutex `{name}` could not be created: {e}");
            Claim::Failed
        }
    }
}

/// Ask for a name and let it go again at once.
#[cfg(all(unix, not(target_os = "macos")))]
fn probe(name: &str) -> Claim {
    claim(name, &mut Vec::new())
}

/// Check that no other instance of Sunlit Earth is running.
///
/// The name is derived from `--ipc-socket`, so it is as much the user's to get
/// wrong as the socket name is: on Windows a backslash in it is a namespace
/// separator and the mutex is refused outright.
///
/// On Linux the name used to be the same for every user and now carries the
/// user's part, so the old name is asked for too: an instance of an older
/// version holds it. It is let go again at once, because the abstract name is
/// shared by every user on the host and keeping it would turn another user's
/// new instance away as an older version.
///
/// :param `mutex_name`: the process-wide name this instance claims
/// :returns: what the check found
pub fn acquire_single_instance(mutex_name: &str) -> InstanceCheck {
    let mut held = Vec::new();
    let current = claim(&instance_name(mutex_name), &mut held);
    // Older versions took the bare name on Linux; elsewhere the name is
    // unchanged and there is nothing older to look for.
    #[cfg(all(unix, not(target_os = "macos")))]
    let legacy = (current != Claim::HeldElsewhere).then(|| probe(mutex_name));
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    let legacy = None;
    match decide(current, legacy) {
        Decision::Alone => InstanceCheck::Alone(InstanceGuard { held }),
        Decision::AlreadyRunning => InstanceCheck::AlreadyRunning,
        Decision::OlderVersionRunning => InstanceCheck::OlderVersionRunning,
    }
}

/// The name `single-instance` is handed, which on macOS is a path it calls
/// `flock` on rather than a name in a namespace of its own. A relative one
/// lands in the working directory, which for an app launched from Finder is
/// `/`, and the guard is then silently absent. A system with no data directory
/// keeps the bare name, which is what this did before.
#[cfg(target_os = "macos")]
fn instance_name(name: &str) -> String {
    let Some(dir) = sunlit_core::app_data_dir() else {
        warn!("this system has no data directory to keep the instance lock in");
        return name.to_owned();
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn!("the instance lock's directory could not be created: {e}");
        return name.to_owned();
    }
    dir.join(format!("instance-{name}.lock"))
        .to_string_lossy()
        .into_owned()
}

/// On Linux the name is an abstract socket, which every user on the host
/// shares, so it carries the user's part.
#[cfg(all(unix, not(target_os = "macos")))]
fn instance_name(name: &str) -> String {
    format!("{name}-{}", crate::activation::user_part())
}

/// On Windows a mutex without a namespace prefix is the session's own.
#[cfg(windows)]
fn instance_name(name: &str) -> String {
    name.to_owned()
}

/// How long a hidden start on Linux waits for a tray host to appear before it
/// decides the session has none.
pub const TRAY_HOST_WAIT: Duration = Duration::from_secs(10);

/// How often that wait asks the session bus.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const TRAY_HOST_POLL: Duration = Duration::from_millis(100);

/// Ask `present` until it answers yes or `timeout` has been slept away.
///
/// :returns: how long it slept before the yes, or `None` for no yes at all
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn wait_for(
    timeout: Duration,
    poll: Duration,
    mut present: impl FnMut() -> bool,
    mut sleep: impl FnMut(Duration),
) -> Option<Duration> {
    let mut waited = Duration::ZERO;
    loop {
        if present() {
            return Some(waited);
        }
        if waited >= timeout {
            return None;
        }
        let step = poll.min(timeout.saturating_sub(waited));
        sleep(step);
        waited += step;
    }
}

/// Wait up to [`TRAY_HOST_WAIT`] for `org.kde.StatusNotifierWatcher` to get
/// an owner on the session bus.
///
/// At login the panel that hosts the tray and the autostart entries start at
/// the same time, and Slint's tray gives up at once when the watcher is not
/// there yet, which would leave a hidden start running windowed. A session
/// that never gets a watcher, GNOME without the extension, shows the window
/// after the wait as before.
#[cfg(target_os = "linux")]
pub fn wait_for_tray_host() {
    const WATCHER: &str = "org.kde.StatusNotifierWatcher";
    let bus = match zbus::blocking::Connection::session() {
        Ok(bus) => bus,
        Err(e) => {
            warn!("the session bus could not be reached to ask for a tray host: {e}");
            return;
        }
    };
    let proxy = match zbus::blocking::fdo::DBusProxy::new(&bus) {
        Ok(proxy) => proxy,
        Err(e) => {
            warn!("the session bus could not be asked for a tray host: {e}");
            return;
        }
    };
    let Ok(name) = zbus::names::BusName::try_from(WATCHER) else {
        return;
    };
    let present = || proxy.name_has_owner(name.clone()).unwrap_or(false);
    match wait_for(TRAY_HOST_WAIT, TRAY_HOST_POLL, present, std::thread::sleep) {
        Some(Duration::ZERO) => debug!("the tray host is there"),
        Some(waited) => info!(
            waited_ms = waited.as_millis(),
            "waited {} ms for the tray host",
            waited.as_millis()
        ),
        None => info!(
            "waited {} s for a tray host and none came, so this run shows its window",
            TRAY_HOST_WAIT.as_secs()
        ),
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
    wire(&tray, window, engine, about);

    tray.show()
        .inspect_err(|e| warn!("the tray icon could not be shown: {e}"))
        .ok()?;
    Some(tray)
}

/// Show the settings window's auto-refresh state in the tray menu.
///
/// :param tray: the tray icon
/// :param enabled: whether auto-refresh is on
pub fn follow_auto_refresh(tray: &TrayIcon, enabled: bool) {
    tray.set_auto_refresh_enabled(enabled);
}

/// Wire the tray's callbacks to the window, the engine and the About window,
/// and have its two state items start from what the window shows.
pub fn wire(tray: &TrayIcon, window: &MainWindow, engine: &EngineLink, about: &AboutController) {
    follow_auto_refresh(tray, window.get_auto_refresh_enabled());
    tray.set_autostart_on(window.get_autostart_on());
    tray.set_autostart_available(window.get_autostart_available());
    crate::autostart::ui::register_tray(tray);

    let window_weak = window.as_weak();
    let engine_link = engine.clone();
    tray.on_open_window(move || {
        if let Some(win) = window_weak.upgrade() {
            debug!("tray: showing window");
            sunlit_core::memory::log_memory_usage("after window shown");
            crate::window_state::show(&win, &engine_link);
        }
    });

    let window_weak = window.as_weak();
    let engine_link = engine.clone();
    tray.on_toggle_window(move || {
        if let Some(win) = window_weak.upgrade() {
            if crate::window_state::toggle(&win, &engine_link) {
                debug!("tray: window shown (icon clicked)");
                sunlit_core::memory::log_memory_usage("after window shown");
            } else {
                debug!("tray: window hidden (icon clicked)");
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

    // The same write and read as the settings window's switch. A failed write
    // has no status line here, so it is a warning in the log, and the item
    // shows what the read after it found.
    let window_weak = window.as_weak();
    tray.on_autostart_toggled(move |on| {
        debug!(on, "tray: start on login toggled");
        if let Some(win) = window_weak.upgrade() {
            let _ = crate::autostart::ui::toggle(&win, on);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_claim_on_a_held_name_finds_the_first_running() {
        let name = format!("sunlit-earth-guard-test-{}", std::process::id());
        let InstanceCheck::Alone(guard) = acquire_single_instance(&name) else {
            panic!("nothing else holds a name made for this test");
        };
        assert!(!guard.held.is_empty());
        assert!(matches!(
            acquire_single_instance(&name),
            InstanceCheck::AlreadyRunning
        ));
    }

    #[test]
    fn a_name_held_elsewhere_is_a_running_instance_whatever_the_legacy_name_says() {
        for legacy in [
            None,
            Some(Claim::Taken),
            Some(Claim::HeldElsewhere),
            Some(Claim::Failed),
        ] {
            assert_eq!(
                decide(Claim::HeldElsewhere, legacy),
                Decision::AlreadyRunning,
                "{legacy:?}"
            );
        }
    }

    #[test]
    fn a_legacy_name_held_elsewhere_is_an_older_version() {
        for current in [Claim::Taken, Claim::Failed] {
            assert_eq!(
                decide(current, Some(Claim::HeldElsewhere)),
                Decision::OlderVersionRunning,
                "{current:?}"
            );
        }
    }

    #[test]
    fn with_neither_name_held_elsewhere_the_run_is_alone() {
        for current in [Claim::Taken, Claim::Failed] {
            for legacy in [None, Some(Claim::Taken), Some(Claim::Failed)] {
                assert_eq!(
                    decide(current, legacy),
                    Decision::Alone,
                    "{current:?} {legacy:?}"
                );
            }
        }
    }

    #[test]
    fn a_tray_host_that_is_there_is_not_waited_for() {
        let mut slept = Duration::ZERO;
        let waited = wait_for(TRAY_HOST_WAIT, TRAY_HOST_POLL, || true, |d| slept += d);
        assert_eq!(waited, Some(Duration::ZERO));
        assert_eq!(slept, Duration::ZERO);
    }

    #[test]
    fn a_tray_host_that_comes_ends_the_wait_when_it_comes() {
        let mut asked = 0;
        let mut slept = Duration::ZERO;
        let waited = wait_for(
            TRAY_HOST_WAIT,
            TRAY_HOST_POLL,
            || {
                asked += 1;
                asked > 3
            },
            |d| slept += d,
        );
        assert_eq!(waited, Some(slept));
        assert!(
            slept > Duration::ZERO && slept < TRAY_HOST_WAIT,
            "{slept:?}"
        );
    }

    #[test]
    fn a_tray_host_that_never_comes_is_waited_for_exactly_the_limit() {
        let mut slept = Duration::ZERO;
        let waited = wait_for(TRAY_HOST_WAIT, TRAY_HOST_POLL, || false, |d| slept += d);
        assert_eq!(waited, None);
        assert_eq!(slept, TRAY_HOST_WAIT);
    }

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
            InstanceCheck::Alone(guard) if guard.held.is_empty()
        ));
    }

    /// On macOS the name has to be an absolute path. Everywhere else it is a
    /// name in a namespace of its own and arrives unchanged.
    #[test]
    fn the_instance_lock_is_a_real_path_on_the_platform_that_makes_it_a_file() {
        let name = instance_name("sunlit-earth-app");
        if cfg!(target_os = "macos") {
            let path = std::path::Path::new(&name);
            assert!(path.is_absolute(), "{name}");
            assert!(name.ends_with("instance-sunlit-earth-app.lock"), "{name}");
            assert!(
                path.parent().is_some_and(std::path::Path::is_dir),
                "the lock's directory has to exist for the create to work: {name}"
            );
        } else if cfg!(windows) {
            assert_eq!(name, "sunlit-earth-app");
        } else {
            assert!(name.starts_with("sunlit-earth-app-"), "{name}");
            assert_eq!(
                name,
                instance_name("sunlit-earth-app"),
                "stable for one user"
            );
        }
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
