//! Wallpaper export: where the file goes, how it is encoded, and on Windows the
//! monitor query and `SystemParametersInfoW` that make it the desktop.
//!
//! The first two halves are the same everywhere, which is why this module is no
//! longer Windows-only: every platform with a setter writes the same PNG to the
//! same place in its own data directory, and only the act of handing it to the
//! desktop differs. The Linux side of that act is a table of per-desktop
//! commands in [`crate::desktop`], because there is no system call to make
//! there; the Win32 half is here, behind a `cfg`, because it is one.

#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;

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
/// Returns the path to the saved file on success.
pub fn save_wallpaper_image(pixels: &[u8], width: u32, height: u32) -> Result<PathBuf, String> {
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};

    let dir = wallpaper_dir()?;
    let path = dir.join("wallpaper.png");
    debug!(path = %path.display(), "saving wallpaper PNG");

    let file =
        std::fs::File::create(&path).map_err(|e| format!("Failed to create PNG file: {e}"))?;
    let writer = std::io::BufWriter::new(file);

    let encoder = PngEncoder::new_with_quality(writer, CompressionType::Fast, FilterType::Sub);
    encoder
        .write_image(pixels, width, height, image::ColorType::Rgba8.into())
        .map_err(|e| format!("Failed to encode PNG: {e}"))?;

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
    fn wallpaper_path_is_png() {
        let dir = wallpaper_dir().unwrap();
        let path = dir.join("wallpaper.png");
        assert!(
            path.to_string_lossy().ends_with(".png"),
            "wallpaper path should end with .png"
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

    /// Tests both creation and overwrite in a single test to avoid races
    /// (both operations write to the same fixed wallpaper path).
    #[test]
    fn save_wallpaper_creates_valid_png_and_overwrites() {
        // First save: 4x4 solid red
        let width = 4u32;
        let height = 4u32;
        let red_pixels: Vec<u8> = (0..width * height)
            .flat_map(|_| [255u8, 0, 0, 255])
            .collect();

        let path = save_wallpaper_image(&red_pixels, width, height).expect("should save PNG");
        assert!(path.exists(), "PNG file should exist");
        assert!(
            std::fs::metadata(&path).unwrap().len() > 0,
            "PNG file should be non-empty"
        );

        // Verify it can be re-read with correct dimensions
        let img = image::open(&path).expect("should be readable by image crate");
        assert_eq!(img.width(), width);
        assert_eq!(img.height(), height);

        // Second save: 2x2 solid blue (overwrite)
        let w2 = 2u32;
        let h2 = 2u32;
        let blue_pixels: Vec<u8> = (0..w2 * h2).flat_map(|_| [0u8, 0, 255, 255]).collect();
        let path2 = save_wallpaper_image(&blue_pixels, w2, h2).expect("second save");
        assert_eq!(path, path2, "should write to same path");

        // Verify the file now contains the second image's data
        let img2 = image::open(&path2).expect("should be readable after overwrite");
        assert_eq!(img2.width(), w2);
        assert_eq!(img2.height(), h2);
        let pixel = img2.as_rgba8().expect("should be RGBA8").get_pixel(0, 0);
        assert_eq!(pixel[0], 0, "red channel should be 0 (blue image)");
        assert_eq!(pixel[2], 255, "blue channel should be 255");
    }
}
