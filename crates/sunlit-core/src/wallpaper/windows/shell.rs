use std::path::Path;

use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::UI::Shell::{
    DESKTOP_WALLPAPER_POSITION, DWPOS_FILL, DWPOS_SPAN, DesktopWallpaper, IDesktopWallpaper,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

/// A string the shell allocated with the COM task allocator.
///
/// `GetMonitorDevicePathAt` hands back memory the caller owns, and the
/// wrapper exists so that every early return frees it.
struct TaskMem(PWSTR);

impl TaskMem {
    /// Its contents as a `String`, empty where the shell answered with
    /// nothing. Not `Display`, because a wrapper around a raw pointer that
    /// prints itself invites being printed.
    fn read(&self) -> String {
        if self.0.is_null() {
            return String::new();
        }
        // SAFETY: a non-null PWSTR from a COM out-parameter is a
        // null-terminated UTF-16 string the shell allocated and this owns.
        #[allow(unsafe_code)]
        unsafe {
            String::from_utf16_lossy(self.0.as_wide())
        }
    }
}

impl Drop for TaskMem {
    fn drop(&mut self) {
        if self.0.is_null() {
            return;
        }
        // SAFETY: the pointer came from a COM method that transfers
        // ownership to the caller, and this is the only release of it.
        #[allow(unsafe_code)]
        unsafe {
            CoTaskMemFree(Some(self.0.as_ptr().cast()));
        }
    }
}

/// Initialize COM on this thread, once, and never tear it down.
///
/// A single-threaded apartment, which is what a desktop shell object wants
/// and what the UI thread is already in. A thread that is already in the
/// multi-threaded apartment answers `RPC_E_CHANGED_MODE`, and that is not an
/// error to report: COM is initialized there, the apartment is simply
/// somebody else's, and an in-process shell object works either way. What
/// must not happen is calling `CoUninitialize` on a thread this did not
/// initialize, which is why nothing here ever does.
fn initialize() {
    thread_local! {
        static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    DONE.with(|done| {
        if done.replace(true) {
            return;
        }
        // SAFETY: CoInitializeEx takes no pointers of ours and is safe to
        // call on any thread. Its result is inspected rather than asserted,
        // because both "already initialized" answers are fine here.
        #[allow(unsafe_code)]
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        tracing::debug!(hresult = hr.0, "initialized COM on this thread");
    });
}

/// Where an image is fitted on the screens it is set on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    /// One screen's own picture, filled to it. What `WallpaperStyle=10` does.
    Fill,
    /// One image stretched over the whole virtual desktop.
    Span,
}

impl From<Position> for DESKTOP_WALLPAPER_POSITION {
    fn from(position: Position) -> Self {
        match position {
            Position::Fill => DWPOS_FILL,
            Position::Span => DWPOS_SPAN,
        }
    }
}

/// The shell's own wallpaper object.
pub struct DesktopWallpaperApi(IDesktopWallpaper);

impl DesktopWallpaperApi {
    /// Create the shell object, initializing COM on this thread first.
    pub fn open() -> Result<Self, String> {
        initialize();
        // SAFETY: DesktopWallpaper is an in-process shell class and the
        // requested interface is the one the type parameter names, so the
        // generated wrapper checks the QueryInterface itself.
        #[allow(unsafe_code)]
        let api: IDesktopWallpaper =
            unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL) }
                .map_err(|e| format!("cannot reach the desktop wallpaper interface: {e}"))?;
        Ok(Self(api))
    }

    /// Every monitor the shell will address, by the device path it takes.
    ///
    /// The path is what survives a reboot, which `\\.\DISPLAY1` does not, so
    /// it is what the anchor setting stores.
    pub fn monitor_paths(&self) -> Result<Vec<String>, String> {
        // SAFETY: no arguments, and the count is a plain out-parameter.
        #[allow(unsafe_code)]
        let count = unsafe { self.0.GetMonitorDevicePathCount() }
            .map_err(|e| format!("cannot count the monitors the shell addresses: {e}"))?;
        let mut paths = Vec::with_capacity(count as usize);
        for index in 0..count {
            // SAFETY: the index is below the count the shell just gave, and
            // the returned string is owned by TaskMem from here on.
            #[allow(unsafe_code)]
            let path = unsafe { self.0.GetMonitorDevicePathAt(index) }
                .map_err(|e| format!("cannot read monitor {index}'s device path: {e}"))?;
            paths.push(TaskMem(path).read());
        }
        Ok(paths)
    }

    /// The rectangle the shell says one device path occupies.
    ///
    /// This is what maps a device path onto an `HMONITOR`: the two
    /// enumerations have no key in common and this rectangle is the only
    /// thing both of them report.
    pub fn monitor_rect(&self, id: &str) -> Result<(i32, i32, i32, i32), String> {
        let id = HSTRING::from(id);
        // SAFETY: the HSTRING outlives the call, and PCWSTR borrows it.
        #[allow(unsafe_code)]
        let rect = unsafe { self.0.GetMonitorRECT(PCWSTR(id.as_ptr())) }
            .map_err(|e| format!("cannot read that monitor's rectangle: {e}"))?;
        Ok((rect.left, rect.top, rect.right, rect.bottom))
    }

    /// How the images this object sets are fitted.
    pub fn set_position(&self, position: Position) -> Result<(), String> {
        // SAFETY: the position is one of the enumeration's own values.
        #[allow(unsafe_code)]
        unsafe { self.0.SetPosition(position.into()) }
            .map_err(|e| format!("cannot set the wallpaper position: {e}"))
    }

    /// Put one image on one monitor, or on every monitor where `monitor` is
    /// `None`.
    pub fn set(&self, monitor: Option<&str>, image: &Path) -> Result<(), String> {
        let id = monitor.map(HSTRING::from);
        let image = HSTRING::from(image.as_os_str());
        let id = id.as_ref().map_or(PCWSTR::null(), |id| PCWSTR(id.as_ptr()));
        // SAFETY: both HSTRINGs outlive the call, and a null monitor id is
        // the interface's own way of naming every monitor.
        #[allow(unsafe_code)]
        unsafe { self.0.SetWallpaper(id, PCWSTR(image.as_ptr())) }.map_err(|e| {
            format!(
                "cannot set the wallpaper of {}: {e}",
                monitor.unwrap_or("every monitor")
            )
        })
    }
}
