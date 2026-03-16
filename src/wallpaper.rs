//! Windows-only wallpaper export: monitor resolution detection, TIFF save,
//! and `SystemParametersInfoW` to set the desktop wallpaper.

use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SPI_SETDESKWALLPAPER, SystemParametersInfoW,
};

/// Return the wallpaper output directory (`%LOCALAPPDATA%\SunlitEarth\`),
/// creating it if it does not exist.
pub fn wallpaper_dir() -> Result<PathBuf, String> {
    let local_app_data =
        std::env::var("LOCALAPPDATA").map_err(|_| "LOCALAPPDATA not set".to_owned())?;
    let dir = PathBuf::from(local_app_data).join("SunlitEarth");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create wallpaper directory: {e}"))?;
    Ok(dir)
}

/// Detect the primary monitor's physical resolution in pixels.
///
/// Uses `EnumDisplayMonitors` + `GetMonitorInfoW` to find the primary
/// monitor and read its pixel dimensions from `rcMonitor`.
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
        let ok = unsafe {
            GetMonitorInfoW(hmon, (&raw mut info).cast::<MONITORINFO>())
        };
        if ok == 0 {
            continue;
        }

        if info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0 {
            let rc = info.monitorInfo.rcMonitor;
            let width = (rc.right - rc.left) as u32;
            let height = (rc.bottom - rc.top) as u32;
            return Ok((width, height));
        }
    }

    Err("No primary monitor found".to_owned())
}

/// Set the wallpaper display style to "Fill" (style 10, tile 0) via the
/// registry keys `HKCU\Control Panel\Desktop\WallpaperStyle` and
/// `HKCU\Control Panel\Desktop\TileWallpaper`.
///
/// Uses `reg.exe` to avoid pulling in additional windows-sys registry features.
fn ensure_fill_style() -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    let status = std::process::Command::new("reg")
        .args([
            "add",
            r"HKCU\Control Panel\Desktop",
            "/v",
            "WallpaperStyle",
            "/t",
            "REG_SZ",
            "/d",
            "10",
            "/f",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Failed to run reg.exe for WallpaperStyle: {e}"))?;
    if !status.status.success() {
        return Err(format!(
            "reg.exe WallpaperStyle failed: {}",
            String::from_utf8_lossy(&status.stderr)
        ));
    }

    let status = std::process::Command::new("reg")
        .args([
            "add",
            r"HKCU\Control Panel\Desktop",
            "/v",
            "TileWallpaper",
            "/t",
            "REG_SZ",
            "/d",
            "0",
            "/f",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Failed to run reg.exe for TileWallpaper: {e}"))?;
    if !status.status.success() {
        return Err(format!(
            "reg.exe TileWallpaper failed: {}",
            String::from_utf8_lossy(&status.stderr)
        ));
    }

    Ok(())
}

/// Windows `CREATE_NO_WINDOW` flag for `Command::creation_flags`.
/// Prevents a console window from flashing when spawning `reg.exe`.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Set the given image file as the Windows desktop wallpaper.
///
/// Verifies the file exists and is non-empty, sets "Fill" display style,
/// then calls `SystemParametersInfoW` with `SPI_SETDESKWALLPAPER`.
pub fn set_wallpaper(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    // Verify the file exists and is non-empty
    let metadata =
        std::fs::metadata(path).map_err(|e| format!("Wallpaper file not found: {e}"))?;
    if metadata.len() == 0 {
        return Err("Wallpaper file is empty".to_owned());
    }

    // Canonicalize to absolute path
    let abs_path =
        std::fs::canonicalize(path).map_err(|e| format!("Failed to canonicalize path: {e}"))?;

    // Set "Fill" wallpaper style before applying
    ensure_fill_style()?;

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
        return Err(format!(
            "SystemParametersInfoW failed (error code {err})"
        ));
    }

    Ok(())
}

/// Encode RGBA8 pixel data as a TIFF file and save it to the wallpaper directory.
///
/// Returns the path to the saved file on success.
pub fn save_wallpaper_image(pixels: &[u8], width: u32, height: u32) -> Result<PathBuf, String> {
    let dir = wallpaper_dir()?;
    let path = dir.join("wallpaper.tif");

    let img = image::RgbaImage::from_raw(width, height, pixels.to_vec())
        .ok_or_else(|| "Failed to create image from pixel data".to_owned())?;
    img.save(&path)
        .map_err(|e| format!("Failed to save TIFF: {e}"))?;

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallpaper_dir_uses_localappdata() {
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
    fn wallpaper_path_is_tif() {
        let dir = wallpaper_dir().unwrap();
        let path = dir.join("wallpaper.tif");
        assert!(
            path.to_string_lossy().ends_with(".tif"),
            "wallpaper path should end with .tif"
        );
    }

    #[test]
    fn primary_resolution_is_nonzero() {
        let (w, h) = get_primary_monitor_resolution().expect("should detect primary monitor");
        assert!(w > 0, "width should be > 0");
        assert!(h > 0, "height should be > 0");
    }

    #[test]
    fn primary_resolution_is_reasonable() {
        let (w, h) = get_primary_monitor_resolution().expect("should detect primary monitor");
        assert!(w >= 640, "width should be >= 640, got {w}");
        assert!(h >= 480, "height should be >= 480, got {h}");
    }

    #[test]
    fn set_wallpaper_rejects_missing_file() {
        let result = set_wallpaper(Path::new(r"C:\nonexistent\fake_wallpaper.tif"));
        assert!(result.is_err(), "should reject missing file");
    }

    #[test]
    fn set_wallpaper_rejects_empty_file() {
        let dir = std::env::temp_dir().join("sunlit_earth_test");
        std::fs::create_dir_all(&dir).unwrap();
        let empty_file = dir.join("empty.tif");
        std::fs::write(&empty_file, b"").unwrap();
        let result = set_wallpaper(&empty_file);
        assert!(result.is_err(), "should reject empty file");
        // Cleanup
        let _ = std::fs::remove_file(&empty_file);
    }

    /// Tests both creation and overwrite in a single test to avoid races
    /// (both operations write to the same fixed wallpaper path).
    #[test]
    fn save_wallpaper_creates_valid_tiff_and_overwrites() {
        // First save: 4x4 solid red
        let width = 4u32;
        let height = 4u32;
        let red_pixels: Vec<u8> = (0..width * height)
            .flat_map(|_| [255u8, 0, 0, 255])
            .collect();

        let path = save_wallpaper_image(&red_pixels, width, height).expect("should save TIFF");
        assert!(path.exists(), "TIFF file should exist");
        assert!(
            std::fs::metadata(&path).unwrap().len() > 0,
            "TIFF file should be non-empty"
        );

        // Verify it can be re-read with correct dimensions
        let img = image::open(&path).expect("should be readable by image crate");
        assert_eq!(img.width(), width);
        assert_eq!(img.height(), height);

        // Second save: 2x2 solid blue (overwrite)
        let w2 = 2u32;
        let h2 = 2u32;
        let blue_pixels: Vec<u8> = (0..w2 * h2)
            .flat_map(|_| [0u8, 0, 255, 255])
            .collect();
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
