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

/// The two names a published wallpaper alternates between.
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
/// overwrites the file the desktop is not showing, so a desktop whose setting
/// still names the previous frame is looking at a stale image rather than at one
/// being rewritten underneath it.
///
/// Windows needs none of this, since `SystemParametersInfoW` reads whatever it
/// is handed, and shares it rather than making the output path depend on the
/// platform.
const WALLPAPER_NAMES: [&str; 2] = ["wallpaper-1.png", "wallpaper-2.png"];

/// The name the last publish in this process wrote.
///
/// Remembered rather than asked of the filesystem every time, because two
/// publishes can land inside one tick of the clock that stamps their
/// modification times, and two publishes to one name are the thing the
/// alternation exists to prevent. Empty until this process has published, where
/// the modification times are all there is to go on.
static PUBLISHED: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Both files a published wallpaper can be, in a fixed order.
pub fn wallpaper_files() -> Result<[PathBuf; 2], String> {
    let dir = wallpaper_dir()?;
    Ok(WALLPAPER_NAMES.map(|name| dir.join(name)))
}

/// The file the most recent publish wrote, or `None` where nothing has published.
///
/// This is what a desktop's own store holds once the setter has run, which is
/// what lets a test read the setting back and recognize it.
///
/// What this process wrote, where it has written anything, and otherwise the
/// more recently modified of the two. That second answer is what carries the
/// alternation across a restart, since the setting names the newest file and the
/// next publish is therefore the other one, and it is also how a process that
/// did not do the publishing gets the same answer.
pub fn published_wallpaper_file() -> Result<Option<PathBuf>, String> {
    if let Some(path) = PUBLISHED
        .lock()
        .expect("the published name is poisoned")
        .clone()
    {
        return Ok(Some(path));
    }
    Ok(wallpaper_files()?
        .into_iter()
        .filter_map(|path| modified(&path).map(|time| (time, path)))
        .max_by_key(|(time, _)| *time)
        .map(|(_, path)| path))
}

/// The file the next publish writes: whichever of the two is not on the desktop.
fn next_wallpaper_file() -> Result<PathBuf, String> {
    let [first, second] = wallpaper_files()?;
    if published_wallpaper_file()?.as_ref() == Some(&first) {
        Ok(second)
    } else {
        Ok(first)
    }
}

/// When a file was last written, or `None` where there is no file to ask.
fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

/// Detect the primary monitor's physical resolution in pixels.
///
/// Uses `EnumDisplayMonitors` + `GetMonitorInfoW` to find the primary
/// monitor and read its pixel dimensions from `rcMonitor`.
///
/// Off Windows the same question is answered by [`crate::display`], which parses
/// `xrandr --query`: there is no API in this crate to ask, so it asks a program.
#[cfg(windows)]
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub fn get_primary_monitor_resolution() -> Result<(u32, u32), String> {
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

    let mut monitors: Vec<HMONITOR> = Vec::new();

    // SAFETY: EnumDisplayMonitors with null HDC/RECT enumerates all monitors.
    // The callback receives a valid lparam pointing to our Vec. The call is
    // synchronous -- the callback runs on this thread before the function returns.
    #[allow(unsafe_code)]
    let success = unsafe {
        EnumDisplayMonitors(
            ptr::null_mut(),
            ptr::null(),
            Some(enum_callback),
            (&raw mut monitors) as LPARAM,
        )
    };
    if success == 0 {
        return Err("EnumDisplayMonitors failed".to_owned());
    }

    for &hmon in &monitors {
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

        if info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0 {
            let rc = info.monitorInfo.rcMonitor;
            let width = (rc.right - rc.left) as u32;
            let height = (rc.bottom - rc.top) as u32;
            debug!(width, height, "detected primary monitor resolution");
            return Ok((width, height));
        }
    }

    Err("No primary monitor found".to_owned())
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

/// Encode RGBA8 pixel data as PNG and save it to the wallpaper directory.
///
/// Uses fast compression (`CompressionType::Fast`) because the user waits
/// for the "Set as Wallpaper" operation to complete. Larger file size is
/// acceptable. Windows preserves PNG wallpapers losslessly (no JPEG
/// transcode), which avoids the banding artifacts that occurred with TIFF.
///
/// Writes to whichever of [`wallpaper_files`] the desktop is not showing, so
/// that the path handed to the setter afterwards is one it has to load. Returns
/// that path on success.
pub fn save_wallpaper_image(pixels: &[u8], width: u32, height: u32) -> Result<PathBuf, String> {
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};

    let path = next_wallpaper_file()?;
    debug!(path = %path.display(), "saving wallpaper PNG");

    let file =
        std::fs::File::create(&path).map_err(|e| format!("Failed to create PNG file: {e}"))?;
    let writer = std::io::BufWriter::new(file);

    let encoder = PngEncoder::new_with_quality(writer, CompressionType::Fast, FilterType::Sub);
    encoder
        .write_image(pixels, width, height, image::ColorType::Rgba8.into())
        .map_err(|e| format!("Failed to encode PNG: {e}"))?;

    // Only once the image is written, so a failed encode does not spend the
    // name the next publish is going to need.
    *PUBLISHED.lock().expect("the published name is poisoned") = Some(path.clone());

    Ok(path)
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
    fn both_wallpaper_paths_are_distinct_pngs() {
        let [first, second] = wallpaper_files().unwrap();
        for path in [&first, &second] {
            assert!(
                path.to_string_lossy().ends_with(".png"),
                "wallpaper path should end with .png, got: {path:?}"
            );
        }
        assert_ne!(
            first, second,
            "the two names are what makes a publish a path the desktop has not seen"
        );
    }

    #[test]
    #[cfg(windows)]
    fn primary_resolution_is_nonzero() {
        let (w, h) = get_primary_monitor_resolution().expect("should detect primary monitor");
        assert!(w > 0, "width should be > 0");
        assert!(h > 0, "height should be > 0");
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

    /// Three saves in one test, because the alternation is one behaviour and the
    /// saves share the one wallpaper directory: a second publish must not land
    /// on the file the desktop is showing, and a third has to come back to the
    /// first name rather than growing a third file.
    #[test]
    fn consecutive_saves_alternate_between_the_two_files() {
        let red: Vec<u8> = (0..4u32 * 4).flat_map(|_| [255u8, 0, 0, 255]).collect();
        let blue: Vec<u8> = (0..2u32 * 2).flat_map(|_| [0u8, 0, 255, 255]).collect();

        let first = save_wallpaper_image(&red, 4, 4).expect("should save PNG");
        assert!(first.exists(), "PNG file should exist");
        assert!(
            std::fs::metadata(&first).unwrap().len() > 0,
            "PNG file should be non-empty"
        );

        let second = save_wallpaper_image(&blue, 2, 2).expect("second save");
        assert_ne!(
            first, second,
            "a publish the desktop can see is one at a path it is not showing"
        );
        assert_eq!(
            published_wallpaper_file().unwrap().as_ref(),
            Some(&second),
            "the setter is handed the file that was just written"
        );

        // Each file holds its own frame, so the one a desktop is still showing
        // is not the one being rewritten.
        let earlier = image::open(&first).expect("the earlier frame is still readable");
        assert_eq!((earlier.width(), earlier.height()), (4, 4));
        let later = image::open(&second).expect("should be readable by image crate");
        assert_eq!((later.width(), later.height()), (2, 2));
        let pixel = later.as_rgba8().expect("should be RGBA8").get_pixel(0, 0);
        assert_eq!(pixel[0], 0, "red channel should be 0 (blue image)");
        assert_eq!(pixel[2], 255, "blue channel should be 255");

        let third = save_wallpaper_image(&red, 4, 4).expect("third save");
        assert_eq!(third, first, "two names, taken in turn");
        // Which of the two the run started on depends on what the directory
        // already held, so what is asserted is that it stayed within them.
        let names = wallpaper_files().unwrap();
        assert!(
            names.contains(&first) && names.contains(&second),
            "the saves used the names this module publishes: {names:?}"
        );
    }
}
