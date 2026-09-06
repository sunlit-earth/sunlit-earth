//! The Win32 and COM half of the wallpaper path: the monitor query, the device
//! paths [`IDesktopWallpaper`] addresses a screen by, and the
//! `SystemParametersInfoW` call that makes a file the desktop.
//!
//! Every item here was gated on `cfg(windows)` one by one while it lived beside
//! the portable half; the gate is now on the module declaration in
//! [`super`], which is the pattern `display::watch` and `display::session_end`
//! already follow.

use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SPI_SETDESKWALLPAPER, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SystemParametersInfoW,
};

use super::begin_publication;

/// Declare this process per-monitor DPI aware, once, before anything asks Win32
/// where a monitor is.
///
/// `rcMonitor` is in virtual-screen coordinates, and those are physical pixels
/// only for a per-monitor aware process; a DPI-unaware one is handed the
/// virtualized rectangle instead, so under mixed scaling every monitor's size
/// and position would be wrong. winit does set
/// `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2`, but only from `EventLoop::new`,
/// which is reached when the settings window is built and never at all in a
/// headless run: `sunlit-earth displays` and `render` create no window, and
/// `displays` is precisely the command that exists to report these rectangles.
/// So the declaration is made here rather than left to whoever creates a window
/// first.
///
/// Idempotent by construction. A process whose awareness is already set refuses
/// the call with `ERROR_ACCESS_DENIED`, which is the case where winit got here
/// first and is the outcome this wants either way, so the return value is not an
/// error to report.
pub(crate) fn ensure_dpi_awareness() {
    use std::sync::Once;

    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };

    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: SetProcessDpiAwarenessContext takes one of the predefined
        // pseudo-handles by value and touches nothing of ours. It is safe to
        // call on any thread and at any time; it merely fails where the
        // awareness is already set.
        #[allow(unsafe_code)]
        let set =
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        debug!(
            already_aware = set == 0,
            "declared this process per-monitor DPI aware"
        );
    });
}

/// Every monitor Windows has, with the rectangle each occupies.
///
/// `EnumDisplayMonitors` + `GetMonitorInfoW`, keeping every monitor rather than
/// only the one flagged primary, because the position of each is what a
/// multi-monitor plan is built out of. `rcMonitor` is in virtual-screen
/// coordinates, which are physical pixels for a per-monitor DPI aware process;
/// see `docs/platforms.md` for what makes this one aware.
///
/// The `id` is `szDevice` (`\\.\DISPLAY1`) until [`desktop_wallpaper`] can
/// improve on it: `IDesktopWallpaper` addresses a monitor by a device path that
/// survives a reboot, and `szDevice` does not. Off Windows the same question is
/// answered by [`crate::display`], which parses `xrandr --query`: there is no
/// API in this crate to ask, so it asks a program.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub(crate) fn enumerate_monitors() -> Result<Vec<crate::display::Monitor>, String> {
    use std::ptr;

    use windows_sys::Win32::Foundation::{BOOL, LPARAM, RECT, TRUE};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

    #[allow(unsafe_code)]
    unsafe extern "system" fn enum_callback(
        hmonitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        // SAFETY: lparam was set to a valid &mut Vec<HMONITOR> pointer
        // by the caller below, and this callback runs synchronously on
        // the same thread within EnumDisplayMonitors.
        #[allow(unsafe_code)]
        let monitors = unsafe { &mut *(lparam as *mut Vec<HMONITOR>) };
        monitors.push(hmonitor);
        TRUE
    }

    ensure_dpi_awareness();
    let mut handles: Vec<HMONITOR> = Vec::new();

    // SAFETY: EnumDisplayMonitors with null HDC/RECT enumerates all monitors.
    // The callback receives a valid lparam pointing to our Vec. The call is
    // synchronous -- the callback runs on this thread before the function returns.
    #[allow(unsafe_code)]
    let success = unsafe {
        EnumDisplayMonitors(
            ptr::null_mut(),
            ptr::null(),
            Some(enum_callback),
            (&raw mut handles) as LPARAM,
        )
    };
    if success == 0 {
        return Err("EnumDisplayMonitors failed".to_owned());
    }

    let mut monitors = Vec::with_capacity(handles.len());
    for &hmon in &handles {
        let mut info: MONITORINFOEXW = {
            // SAFETY: MONITORINFOEXW is a plain-old-data C struct.
            // Zeroing it is safe; we set cbSize immediately after.
            #[allow(unsafe_code)]
            let zeroed: MONITORINFOEXW = unsafe { std::mem::zeroed() };
            zeroed
        };
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;

        // SAFETY: hmon is a valid HMONITOR from EnumDisplayMonitors.
        // info is a properly sized MONITORINFOEXW with cbSize set.
        // GetMonitorInfoW writes into info and returns TRUE on success.
        #[allow(unsafe_code)]
        let ok = unsafe { GetMonitorInfoW(hmon, (&raw mut info).cast::<MONITORINFO>()) };
        if ok == 0 {
            continue;
        }

        let rc = info.monitorInfo.rcMonitor;
        let device = wide_to_string(&info.szDevice);
        monitors.push(crate::display::Monitor {
            label: display_label(&device, monitors.len()),
            id: device,
            x: rc.left,
            y: rc.top,
            width: (rc.right - rc.left) as u32,
            height: (rc.bottom - rc.top) as u32,
            primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        });
    }
    adopt_device_paths(&mut monitors);
    debug!(count = monitors.len(), "enumerated the monitors");
    Ok(monitors)
}

/// Replace each monitor's id with the device path the shell addresses it by.
///
/// The two enumerations have no key in common, so the rectangle is the join:
/// `GetMonitorRECT` and `rcMonitor` describe the same screen in the same
/// coordinates. Mirrored monitors are two entries with one rectangle, and each
/// path takes the first monitor not already claimed, which keeps the mapping a
/// bijection where a rectangle alone would be ambiguous.
///
/// A failure anywhere leaves `szDevice` in place. That is a name the shell will
/// not take, so a publish that needs to address a monitor says so rather than
/// setting the wrong screen's wallpaper.
fn adopt_device_paths(monitors: &mut [crate::display::Monitor]) {
    let api = match shell::DesktopWallpaperApi::open() {
        Ok(api) => api,
        Err(e) => {
            debug!(error = %e, "no shell wallpaper interface; keeping the display device names");
            return;
        }
    };
    let paths = match api.monitor_paths() {
        Ok(paths) => paths,
        Err(e) => {
            debug!(error = %e, "the shell listed no monitor device paths");
            return;
        }
    };
    let mut claimed = vec![false; monitors.len()];
    for path in paths {
        let Ok((left, top, right, bottom)) = api.monitor_rect(&path) else {
            continue;
        };
        let matched = monitors.iter().enumerate().position(|(index, monitor)| {
            !claimed[index]
                && monitor.x == left
                && monitor.y == top
                && i64::from(monitor.width) == i64::from(right) - i64::from(left)
                && i64::from(monitor.height) == i64::from(bottom) - i64::from(top)
        });
        if let Some(index) = matched {
            claimed[index] = true;
            monitors[index].id = path;
        }
    }
}

/// A null-terminated fixed-width UTF-16 field as a `String`.
fn wide_to_string(field: &[u16]) -> String {
    let end = field.iter().position(|&c| c == 0).unwrap_or(field.len());
    String::from_utf16_lossy(&field[..end])
}

/// What to call a Windows monitor in the settings window.
///
/// `\\.\DISPLAY2` is what Windows answers with and is not what its own display
/// settings show anybody, so the digit is lifted out of it and the position in
/// the enumeration stands in where there is no digit to lift.
fn display_label(device: &str, index: usize) -> String {
    let number = device
        .rsplit('\\')
        .next()
        .and_then(|name| name.strip_prefix("DISPLAY"))
        .and_then(|digits| digits.parse::<u32>().ok());
    match number {
        Some(number) => format!("Display {number}"),
        None => format!("Display {}", index + 1),
    }
}

/// `IDesktopWallpaper` behind a small safe wrapper.
///
/// The COM interface is the only way to address one monitor:
/// `SystemParametersInfoW` sets the wallpaper of a whole session and has no
/// parameter for which screen. It is Windows 8 and later, which is everything
/// this ships to.
///
/// Every call is `unsafe` in the generated bindings and every one of them is
/// wrapped here, so the rest of the module never writes `unsafe` and the
/// invariants are argued once each rather than at every call site.
mod shell;

/// Make a finished job the Windows desktop's wallpaper.
///
/// A session with one monitor keeps `SystemParametersInfoW` and the registry
/// style write exactly as they were, so the one configuration that is
/// regression-tested on every desktop and in the Hyper-V guest does not move.
/// Everything beyond one screen goes through `IDesktopWallpaper`, which is the
/// only interface that can address a monitor.
///
/// Addressing one needs the device path [`adopt_device_paths`] maps onto it, and
/// that mapping is a rectangle comparison nothing guarantees: `GetMonitorRECT`
/// and `rcMonitor` disagreeing under mixed DPI would leave every screen holding
/// the `szDevice` name it came in with, which the shell does not take. Three
/// answers follow, and none of them is failing the publish. A screen with no
/// device path keeps the wallpaper it has and the note says which. A screen the
/// shell refuses costs that screen alone, because an unplug between the
/// enumeration and the call looks exactly like that from here and the screens
/// still there should not pay for it. And where *no* screen could be addressed,
/// this falls back to the single-monitor path with the anchor's picture: that is
/// what every session did before this feature, and losing the primary's
/// wallpaper on a configuration nobody has verified is the worst outcome
/// available.
pub(crate) fn set_wallpaper_job(
    job: &crate::engine::wallpaper_sink::WallpaperJob,
) -> Result<String, String> {
    use std::sync::Arc;

    use crate::display::layout::DisplayMode;
    use crate::engine::wallpaper_sink::Frame;

    let spanning = job.mode == DisplayMode::AcrossScreens;
    let mut publication = begin_publication()?;

    if job.monitors.len() <= 1 {
        let frame = job.anchor_image()?;
        let path = publication.write("0", &frame.pixels, frame.width, frame.height)?;
        publication.commit();
        set_wallpaper(&path)?;
        return Ok(String::new());
    }

    if spanning {
        let canvas = job
            .canvas()
            .ok_or_else(|| "a view across the screens was asked for without a canvas".to_owned())?;
        let path = publication.write("canvas", &canvas.pixels, canvas.width, canvas.height)?;
        publication.commit();
        let api = shell::DesktopWallpaperApi::open()?;
        // Windows does the cutting: one file instead of one per screen, and the
        // path it keeps consistent by itself when a monitor is unplugged.
        api.set_position(shell::Position::Span)?;
        api.set(None, &path)?;
        info!(path = %path.display(), "spanned the wallpaper across every monitor");
        return Ok(String::new());
    }

    let mut written: Vec<(Arc<Frame>, PathBuf)> = Vec::new();
    let mut images: Vec<(&crate::display::Monitor, PathBuf)> = Vec::new();
    let mut anchor_path: Option<PathBuf> = None;
    for (index, monitor) in job.monitors.iter().enumerate() {
        // A screen with no picture is one this mode does not paint, and it is
        // left holding whatever it already had.
        let Some(frame) = job.image_for(index)? else {
            continue;
        };
        // Two screens showing the same picture cost one render, and this is
        // what carries that as far as the file: one encode and one path.
        let seen = written
            .iter()
            .find(|(seen, _)| Arc::ptr_eq(seen, &frame))
            .map(|(_, path)| path.clone());
        let path = if let Some(path) = seen {
            path
        } else {
            let path =
                publication.write(&index.to_string(), &frame.pixels, frame.width, frame.height)?;
            written.push((Arc::clone(&frame), path.clone()));
            path
        };
        if index == job.anchor {
            anchor_path = Some(path.clone());
        }
        images.push((monitor, path));
    }
    publication.commit();

    let api = shell::DesktopWallpaperApi::open()?;
    api.set_position(shell::Position::Fill)?;
    let mut unaddressed = Vec::new();
    let mut refused = Vec::new();
    let mut painted = 0usize;
    for (monitor, path) in &images {
        if !is_device_path(&monitor.id) {
            unaddressed.push(monitor.label.clone());
            continue;
        }
        match api.set(Some(&monitor.id), path) {
            Ok(()) => painted += 1,
            Err(e) => {
                warn!(
                    monitor = %monitor.label,
                    error = %e,
                    "the shell would not take this screen's wallpaper"
                );
                refused.push(monitor.label.clone());
            }
        }
    }

    if painted == 0 {
        let path =
            anchor_path.ok_or_else(|| "this publish has no image for its own anchor".to_owned())?;
        set_wallpaper(&path)?;
        let anchor = job
            .anchor_monitor()
            .map_or("the anchor", |m| m.label.as_str());
        return Ok(format!(
            "Windows would not take a wallpaper for any screen by name, so every \
             screen got {anchor}'s picture the way they all did before"
        ));
    }

    info!(screens = painted, "set a wallpaper per monitor");
    let mut notes = Vec::new();
    if !unaddressed.is_empty() {
        notes.push(format!(
            "Windows named no device for {}, so those screens kept the wallpaper they had",
            unaddressed.join(", ")
        ));
    }
    if !refused.is_empty() {
        notes.push(format!(
            "Windows would not take a wallpaper for {}, which is what a screen \
             unplugged since the layout was read looks like from here",
            refused.join(", ")
        ));
    }
    Ok(notes.join("; "))
}

/// Whether a monitor id is one `IDesktopWallpaper` will take.
///
/// Every id starts life as `szDevice` (`\\.\DISPLAY1`) and
/// [`adopt_device_paths`] replaces the ones the shell could be matched to with
/// the device path it addresses that screen by. So an id still in the
/// `\\.\` shape is one the mapping did not reach, and giving it to
/// `SetWallpaper` addresses nothing at all: the interface takes the path, and a
/// display device name is not one.
fn is_device_path(id: &str) -> bool {
    !id.is_empty() && !id.starts_with(r"\\.\")
}

/// Set the wallpaper display style to "Fill" (style 10, tile 0) via the
/// registry keys `HKCU\Control Panel\Desktop\WallpaperStyle` and
/// `HKCU\Control Panel\Desktop\TileWallpaper`.
fn ensure_fill_style() -> Result<(), String> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};

    debug!("setting Fill wallpaper style");

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags("Control Panel\\Desktop", KEY_SET_VALUE)
        .map_err(|e| format!("Failed to open Desktop registry key: {e}"))?;

    key.set_value("WallpaperStyle", &"10")
        .map_err(|e| format!("Failed to set WallpaperStyle: {e}"))?;
    key.set_value("TileWallpaper", &"0")
        .map_err(|e| format!("Failed to set TileWallpaper: {e}"))?;

    Ok(())
}

/// Set the given image file as the Windows desktop wallpaper.
///
/// Verifies the file exists and is non-empty, sets "Fill" display style,
/// then calls `SystemParametersInfoW` with `SPI_SETDESKWALLPAPER`.
pub(crate) fn set_wallpaper(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    let metadata = std::fs::metadata(path).map_err(|e| format!("Wallpaper file not found: {e}"))?;
    if metadata.len() == 0 {
        return Err("Wallpaper file is empty".to_owned());
    }

    let abs_path =
        std::fs::canonicalize(path).map_err(|e| format!("Failed to canonicalize path: {e}"))?;

    ensure_fill_style()?;

    info!(path = %abs_path.display(), "applying wallpaper via SystemParametersInfoW");

    let mut wide_path: Vec<u16> = abs_path.as_os_str().encode_wide().collect();
    // Strip the \\?\ prefix that canonicalize adds on Windows
    let unc_prefix: [u16; 4] = [
        u16::from(b'\\'),
        u16::from(b'\\'),
        u16::from(b'?'),
        u16::from(b'\\'),
    ];
    if wide_path.starts_with(&unc_prefix) {
        wide_path = wide_path[4..].to_vec();
    }
    wide_path.push(0); // null terminator

    // SAFETY: wide_path is a valid null-terminated UTF-16 string pointing to
    // an existing, non-empty file. SystemParametersInfoW with SPI_SETDESKWALLPAPER
    // reads the string and sets the wallpaper. SPIF_UPDATEINIFILE persists the
    // setting across reboots; SPIF_SENDCHANGE notifies other windows.
    #[allow(unsafe_code)]
    let result = unsafe {
        SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            wide_path.as_mut_ptr().cast(),
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        )
    };

    if result == 0 {
        // SAFETY: GetLastError is a trivial Win32 function that reads the
        // calling thread's last-error code. No preconditions.
        #[allow(unsafe_code)]
        let err = unsafe { GetLastError() };
        return Err(format!("SystemParametersInfoW failed (error code {err})"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Windows half of the platform seam, asserted against whatever this
    /// machine has: shape rather than values, because the values are the
    /// machine's.
    ///
    /// One enumeration, not three: on Windows an enumeration is
    /// `EnumDisplayMonitors`, a `GetMonitorInfoW` per monitor, and a COM object
    /// opened for the device paths.
    #[test]
    fn every_monitor_is_enumerated_with_a_rectangle_and_one_of_them_is_primary() {
        let monitors = enumerate_monitors().expect("Windows can enumerate its monitors");
        assert!(!monitors.is_empty(), "a desktop session has a monitor");
        for monitor in &monitors {
            assert!(
                monitor.width > 0 && monitor.height > 0,
                "a monitor with no pixels: {monitor:?}"
            );
            assert!(!monitor.id.is_empty(), "nothing to address: {monitor:?}");
            assert!(monitor.label.starts_with("Display "), "{monitor:?}");
        }
        assert_eq!(
            monitors.iter().filter(|m| m.primary).count(),
            1,
            "Windows marks exactly one monitor primary: {monitors:?}"
        );
        let primary = monitors.iter().find(|m| m.primary).unwrap();
        assert!(
            primary.width >= 640 && primary.height >= 480,
            "a desktop nobody could use: {primary:?}"
        );
    }

    #[test]
    fn a_display_device_is_labeled_by_its_own_number() {
        assert_eq!(display_label(r"\\.\DISPLAY2", 0), "Display 2");
        // A device path with no number to lift falls back to where it came in
        // the enumeration rather than to a name that addresses nothing.
        assert_eq!(display_label("", 3), "Display 4");
        assert_eq!(display_label(r"\\.\WEIRD", 0), "Display 1");
    }

    #[test]
    fn a_display_device_name_is_not_something_the_shell_can_be_given() {
        assert!(!is_device_path(r"\\.\DISPLAY1"));
        assert!(!is_device_path(""));
        assert!(is_device_path(
            r"\\?\DISPLAY#GSM5B09#5&d0e4e51&0&UID4353#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
        ));
    }

    #[test]
    fn a_fixed_width_device_field_stops_at_its_terminator() {
        let mut field = [0u16; 32];
        for (slot, c) in field.iter_mut().zip("ok".encode_utf16()) {
            *slot = c;
        }
        assert_eq!(wide_to_string(&field), "ok");
        assert_eq!(wide_to_string(&[]), "");
    }

    #[test]
    fn set_wallpaper_rejects_a_file_it_cannot_hand_over() {
        assert!(
            set_wallpaper(Path::new(r"C:\nonexistent\fake_wallpaper.png")).is_err(),
            "a file that is not there is not a wallpaper"
        );

        let scratch = crate::test_support::ScratchDir::new("wallpaper_empty_file");
        let empty_file = scratch.join("empty.png");
        std::fs::write(&empty_file, b"").unwrap();
        assert!(
            set_wallpaper(&empty_file).is_err(),
            "an empty file is not a wallpaper"
        );
    }
}
