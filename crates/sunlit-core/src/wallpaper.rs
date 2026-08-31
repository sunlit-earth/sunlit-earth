//! Wallpaper export: where the file goes, how it is encoded, and on Windows the
//! monitor query and `SystemParametersInfoW` that make it the desktop.
//!
//! The first two halves are the same everywhere, which is why this module is no
//! longer Windows-only: every platform with a setter writes the same PNG into
//! the same directory under its own data directory, and only the act of handing
//! it to the desktop differs. The Linux side of that act is a table of per-desktop
//! commands in [`crate::desktop`], because there is no system call to make
//! there; the Win32 half is here, behind a `cfg`, because it is one.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing::debug;
#[cfg(windows)]
use tracing::info;
#[cfg(windows)]
use windows_sys::Win32::Foundation::GetLastError;
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SPI_SETDESKWALLPAPER, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SystemParametersInfoW,
};

/// Return the wallpaper output directory, creating it if it does not exist.
///
/// `%LOCALAPPDATA%\SunlitEarth` on Windows and `~/.local/share/SunlitEarth` on
/// Linux, which is what `dirs::data_local_dir` answers on each and the same
/// directory the config file and the texture cache already live in. Asked through
/// `dirs` rather than through `LOCALAPPDATA` directly so that the three agree
/// wherever the app runs.
pub fn wallpaper_dir() -> Result<PathBuf, String> {
    let dir = dirs::data_local_dir()
        .ok_or_else(|| "no local data directory on this system".to_owned())?
        .join("SunlitEarth");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create wallpaper directory: {e}"))?;
    Ok(dir)
}

/// The two slots a published wallpaper alternates between.
///
/// Two rather than one, because a desktop shell keys the wallpaper it is showing
/// on the path it was handed: a new image written to the path already in that
/// setting is a file the desktop has no reason to read again. plasmashell
/// ignores a second `plasma-apply-wallpaperimage` of the same file, and a
/// `gsettings` or `xfconf-query` write of the value already stored is a change
/// with nothing to notify. Only a path the desktop is not already showing makes
/// it load a file.
///
/// Two is also the smallest number that keeps what one name gave: a publish
/// overwrites the slot the desktop is not showing, so a desktop whose setting
/// still names the previous frame is looking at a stale image rather than at one
/// being rewritten underneath it.
///
/// A slot holds a whole layout rather than one file, since a publish is now one
/// image per monitor: `wallpaper-<slot>-<index>.png` per screen, plus
/// `wallpaper-<slot>-canvas.png` where the mode spans them.
///
/// Windows needs none of this, since `SystemParametersInfoW` reads whatever it
/// is handed, and shares it rather than making the output path depend on the
/// platform.
const SLOTS: [u32; 2] = [1, 2];

/// The single-image names published before a wallpaper was a layout.
///
/// Swept on the next publish, because they are full-resolution PNGs that
/// nothing will ever name again.
const LEGACY_NAMES: [&str; 2] = ["wallpaper-1.png", "wallpaper-2.png"];

/// The slot and the files the last publish in this process wrote.
///
/// Remembered rather than asked of the filesystem every time, because two
/// publishes can land inside one tick of the clock that stamps their
/// modification times, and two publishes to one slot are the thing the
/// alternation exists to prevent. Empty until this process has published, where
/// the modification times are all there is to go on.
static PUBLISHED: Mutex<Option<(u32, Vec<PathBuf>)>> = Mutex::new(None);

/// The file name one image of a publish takes.
fn slot_name(slot: u32, suffix: &str) -> String {
    format!("wallpaper-{slot}-{suffix}.png")
}

/// Every file of one slot, in name order.
fn slot_files(dir: &Path, slot: u32) -> Vec<PathBuf> {
    let prefix = format!("wallpaper-{slot}-");
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
        })
        .collect();
    files.sort();
    files
}

/// The files the most recent publish wrote, empty where nothing has published.
///
/// This is what a desktop's own store holds once the setter has run, which is
/// what lets a test read the setting back and recognize it.
///
/// What this process wrote, where it has written anything, and otherwise the
/// slot holding the more recently modified file. That second answer is what
/// carries the alternation across a restart, since the settings name the newest
/// files and the next publish is therefore the other slot, and it is also how a
/// process that did not do the publishing gets the same answer.
pub fn published_wallpaper_files() -> Result<Vec<PathBuf>, String> {
    if let Some((_, files)) = PUBLISHED
        .lock()
        .expect("the published name is poisoned")
        .clone()
    {
        return Ok(files);
    }
    let dir = wallpaper_dir()?;
    Ok(newest_slot(&dir)
        .map(|slot| slot_files(&dir, slot))
        .unwrap_or_default())
}

/// The slot holding the most recently written file, where there is one.
fn newest_slot(dir: &Path) -> Option<u32> {
    SLOTS
        .into_iter()
        .filter_map(|slot| {
            slot_files(dir, slot)
                .iter()
                .filter_map(|path| modified(path))
                .max()
                .map(|time| (time, slot))
        })
        .max_by_key(|(time, _)| *time)
        .map(|(_, slot)| slot)
}

/// When a file was last written, or `None` where there is no file to ask.
fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

/// One publish in progress: the slot it took, and what it has written so far.
///
/// A slot is emptied when it is taken rather than when it is left, so a layout
/// that lost a monitor does not leave a full-resolution PNG behind for the one
/// that went away. Emptying the slot the desktop is *not* showing is what makes
/// that safe.
pub struct Publication {
    slot: u32,
    dir: PathBuf,
    files: Vec<PathBuf>,
}

/// Take the slot the desktop is not showing, and clear it.
pub fn begin_publication() -> Result<Publication, String> {
    let dir = wallpaper_dir()?;
    let last = PUBLISHED
        .lock()
        .expect("the published name is poisoned")
        .as_ref()
        .map(|(slot, _)| *slot);
    let slot = if last.or_else(|| newest_slot(&dir)) == Some(SLOTS[0]) {
        SLOTS[1]
    } else {
        SLOTS[0]
    };
    for stale in slot_files(&dir, slot) {
        let _ = std::fs::remove_file(stale);
    }
    for legacy in LEGACY_NAMES {
        let _ = std::fs::remove_file(dir.join(legacy));
    }
    Ok(Publication {
        slot,
        dir,
        files: Vec::new(),
    })
}

impl Publication {
    /// Encode one RGBA8 image into this slot and answer with its path.
    ///
    /// Fast compression (`CompressionType::Fast`), because the user waits for
    /// the "Set as Wallpaper" operation to complete and a larger file is the
    /// cheaper half of that trade. Windows preserves PNG wallpapers losslessly
    /// (no JPEG transcode), which avoids the banding artifacts TIFF produced.
    pub fn write(
        &mut self,
        suffix: &str,
        pixels: &[u8],
        width: u32,
        height: u32,
    ) -> Result<PathBuf, String> {
        use image::ImageEncoder;
        use image::codecs::png::{CompressionType, FilterType, PngEncoder};

        let path = self.dir.join(slot_name(self.slot, suffix));
        debug!(path = %path.display(), width, height, "saving wallpaper PNG");

        let file =
            std::fs::File::create(&path).map_err(|e| format!("Failed to create PNG file: {e}"))?;
        let writer = std::io::BufWriter::new(file);
        let encoder = PngEncoder::new_with_quality(writer, CompressionType::Fast, FilterType::Sub);
        encoder
            .write_image(pixels, width, height, image::ColorType::Rgba8.into())
            .map_err(|e| format!("Failed to encode PNG: {e}"))?;

        self.files.push(path.clone());
        Ok(path)
    }

    /// Record this publication as the one the desktop is being handed.
    ///
    /// Called once the images are written and before the setter runs, which is
    /// the same moment the single-file publish recorded its name: a failed
    /// encode must not spend the slot the next publish is going to need.
    pub fn commit(self) -> Vec<PathBuf> {
        *PUBLISHED.lock().expect("the published name is poisoned") =
            Some((self.slot, self.files.clone()));
        self.files
    }
}

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
#[cfg(windows)]
pub fn ensure_dpi_awareness() {
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
/// The `id` is `szDevice` (`\\.\DISPLAY1`), which names the monitor for as long
/// as this session lasts. Off Windows the same question is answered by
/// [`crate::display`], which parses `xrandr --query`: there is no API in this
/// crate to ask, so it asks a program.
#[cfg(windows)]
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub fn enumerate_monitors() -> Result<Vec<crate::display::Monitor>, String> {
    use std::ptr;

    use windows_sys::Win32::Foundation::{BOOL, LPARAM, RECT, TRUE};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

    // Callback invoked by EnumDisplayMonitors for each monitor.
    // Pushes each HMONITOR handle into the Vec pointed to by lparam.
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
    debug!(count = monitors.len(), "enumerated the monitors");
    Ok(monitors)
}

/// A null-terminated fixed-width UTF-16 field as a `String`.
#[cfg(windows)]
fn wide_to_string(field: &[u16]) -> String {
    let end = field.iter().position(|&c| c == 0).unwrap_or(field.len());
    String::from_utf16_lossy(&field[..end])
}

/// What to call a Windows monitor in the settings window.
///
/// `\\.\DISPLAY2` is what Windows answers with and is not what its own display
/// settings show anybody, so the digit is lifted out of it and the position in
/// the enumeration stands in where there is no digit to lift.
#[cfg(windows)]
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

/// Detect the primary monitor's physical resolution in pixels.
///
/// One enumeration, not two: this is [`enumerate_monitors`] narrowed to the
/// monitor a single-screen wallpaper is sized for, with the same fallback to
/// the first that [`crate::display::primary_of`] makes for xrandr.
#[cfg(windows)]
pub fn get_primary_monitor_resolution() -> Result<(u32, u32), String> {
    let monitors = enumerate_monitors()?;
    let monitor = crate::display::primary_monitor_of(&monitors)
        .ok_or_else(|| "No primary monitor found".to_owned())?;
    debug!(
        width = monitor.width,
        height = monitor.height,
        "detected primary monitor resolution"
    );
    Ok((monitor.width, monitor.height))
}

/// Set the wallpaper display style to "Fill" (style 10, tile 0) via the
/// registry keys `HKCU\Control Panel\Desktop\WallpaperStyle` and
/// `HKCU\Control Panel\Desktop\TileWallpaper`.
#[cfg(windows)]
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
#[cfg(windows)]
pub fn set_wallpaper(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    // Verify the file exists and is non-empty
    let metadata = std::fs::metadata(path).map_err(|e| format!("Wallpaper file not found: {e}"))?;
    if metadata.len() == 0 {
        return Err("Wallpaper file is empty".to_owned());
    }

    // Canonicalize to absolute path
    let abs_path =
        std::fs::canonicalize(path).map_err(|e| format!("Failed to canonicalize path: {e}"))?;

    // Set "Fill" wallpaper style before applying
    ensure_fill_style()?;

    info!(path = %abs_path.display(), "applying wallpaper via SystemParametersInfoW");

    // Encode path as null-terminated UTF-16
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

    #[test]
    fn the_wallpaper_lives_under_this_systems_local_data_directory() {
        let result = wallpaper_dir();
        assert!(result.is_ok(), "wallpaper_dir should succeed");
        let dir = result.unwrap();
        assert!(
            dir.ends_with("SunlitEarth"),
            "path should end with SunlitEarth, got: {dir:?}"
        );
        assert!(dir.exists(), "directory should be created");
    }

    #[test]
    fn a_slots_names_carry_the_slot_and_the_image() {
        assert_eq!(slot_name(1, "0"), "wallpaper-1-0.png");
        assert_eq!(slot_name(2, "canvas"), "wallpaper-2-canvas.png");
        assert_ne!(
            slot_name(SLOTS[0], "0"),
            slot_name(SLOTS[1], "0"),
            "the two slots are what makes a publish a path the desktop has not seen"
        );
    }

    #[test]
    #[cfg(windows)]
    fn primary_resolution_is_nonzero() {
        let (w, h) = get_primary_monitor_resolution().expect("should detect primary monitor");
        assert!(w > 0, "width should be > 0");
        assert!(h > 0, "height should be > 0");
    }

    /// The Windows half of the platform seam, asserted against whatever this
    /// machine has: shape rather than values, because the values are the
    /// machine's.
    #[test]
    #[cfg(windows)]
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
        // And the narrowed query answers out of the same list rather than
        // enumerating a second time with its own rules.
        let primary = monitors.iter().find(|m| m.primary).unwrap();
        assert_eq!(
            get_primary_monitor_resolution().unwrap(),
            (primary.width, primary.height)
        );
    }

    #[test]
    #[cfg(windows)]
    fn a_display_device_is_labelled_by_its_own_number() {
        assert_eq!(display_label(r"\\.\DISPLAY2", 0), "Display 2");
        // A device path with no number to lift falls back to where it came in
        // the enumeration rather than to a name that addresses nothing.
        assert_eq!(display_label("", 3), "Display 4");
        assert_eq!(display_label(r"\\.\WEIRD", 0), "Display 1");
    }

    #[test]
    #[cfg(windows)]
    fn a_fixed_width_device_field_stops_at_its_terminator() {
        let mut field = [0u16; 32];
        for (slot, c) in field.iter_mut().zip("ok".encode_utf16()) {
            *slot = c;
        }
        assert_eq!(wide_to_string(&field), "ok");
        assert_eq!(wide_to_string(&[]), "");
    }

    #[test]
    #[cfg(windows)]
    fn primary_resolution_is_reasonable() {
        let (w, h) = get_primary_monitor_resolution().expect("should detect primary monitor");
        assert!(w >= 640, "width should be >= 640, got {w}");
        assert!(h >= 480, "height should be >= 480, got {h}");
    }

    #[test]
    #[cfg(windows)]
    fn set_wallpaper_rejects_missing_file() {
        let result = set_wallpaper(Path::new(r"C:\nonexistent\fake_wallpaper.png"));
        assert!(result.is_err(), "should reject missing file");
    }

    #[test]
    #[cfg(windows)]
    fn set_wallpaper_rejects_empty_file() {
        let dir = std::env::temp_dir().join("sunlit_earth_test");
        std::fs::create_dir_all(&dir).unwrap();
        let empty_file = dir.join("empty.png");
        std::fs::write(&empty_file, b"").unwrap();
        let result = set_wallpaper(&empty_file);
        assert!(result.is_err(), "should reject empty file");
        // Cleanup
        let _ = std::fs::remove_file(&empty_file);
    }

    /// The whole publish protocol in one test, because it is one behaviour and
    /// because these all share the one wallpaper directory: two tests writing
    /// into it at once would each see the other's slot.
    ///
    /// A second publish must not land in the slot the desktop is showing, a
    /// third has to come back to the first slot rather than growing a third one,
    /// and taking a slot has to empty it of the layout that was there.
    #[test]
    fn publishing_alternates_slots_and_empties_the_one_it_takes() {
        let red: Vec<u8> = (0..4u32 * 4).flat_map(|_| [255u8, 0, 0, 255]).collect();
        let blue: Vec<u8> = (0..2u32 * 2).flat_map(|_| [0u8, 0, 255, 255]).collect();

        // The one-file era's names are swept, because nothing will ever name
        // them again and each is as large as a screen.
        let dir = wallpaper_dir().unwrap();
        for legacy in LEGACY_NAMES {
            std::fs::write(dir.join(legacy), b"not really a png").unwrap();
        }

        let mut publication = begin_publication().expect("a slot to publish into");
        let first = publication.write("0", &red, 4, 4).expect("should save PNG");
        let second_screen = publication.write("1", &red, 4, 4).expect("a second screen");
        let canvas = publication.write("canvas", &red, 4, 4).expect("a canvas");
        assert!(first.exists(), "PNG file should exist");
        assert!(
            std::fs::metadata(&first).unwrap().len() > 0,
            "PNG file should be non-empty"
        );
        assert_eq!(
            publication.commit(),
            vec![first.clone(), second_screen.clone(), canvas.clone()]
        );
        for legacy in LEGACY_NAMES {
            assert!(!dir.join(legacy).exists(), "{legacy} survived a publish");
        }

        let mut publication = begin_publication().expect("the other slot");
        let second = publication.write("0", &blue, 2, 2).expect("second publish");
        assert_ne!(
            first, second,
            "a publish the desktop can see is one at a path it is not showing"
        );
        publication.commit();
        assert_eq!(
            published_wallpaper_files().unwrap(),
            vec![second.clone()],
            "the setter is handed the files that were just written"
        );

        // Each slot holds its own frame, so the one a desktop is still showing
        // is not the one being rewritten.
        let earlier = image::open(&first).expect("the earlier frame is still readable");
        assert_eq!((earlier.width(), earlier.height()), (4, 4));
        let later = image::open(&second).expect("should be readable by image crate");
        assert_eq!((later.width(), later.height()), (2, 2));
        let pixel = later.as_rgba8().expect("should be RGBA8").get_pixel(0, 0);
        assert_eq!(pixel[0], 0, "red channel should be 0 (blue image)");
        assert_eq!(pixel[2], 255, "blue channel should be 255");

        // Back to the first slot with a layout of one screen: the two images the
        // larger layout left there are gone rather than lingering at full size.
        let mut publication = begin_publication().expect("back to the first slot");
        assert!(
            !second_screen.exists() && !canvas.exists(),
            "the previous layout's extra images are still there"
        );
        let third = publication.write("0", &red, 4, 4).expect("third publish");
        assert_eq!(third, first, "two slots, taken in turn");
        assert_eq!(publication.commit(), vec![first]);
    }
}
