//! Windows: every top-level window and the shell's notification state.

use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, TRUE};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::Shell::{
    QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetClassNameW, GetWindowLongPtrW, GetWindowRect,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT,
};
use windows_sys::core::BOOL;

use super::{CoverProbe, Coverage, NotificationState, Rect, Screen, WindowsWindow};

pub(super) struct WindowsProbe;

impl WindowsProbe {
    pub(super) fn new() -> Self {
        crate::wallpaper::ensure_dpi_awareness();
        Self
    }
}

impl CoverProbe for WindowsProbe {
    fn probe(&self) -> Coverage {
        let monitors = monitors();
        if monitors.is_empty() {
            return Coverage::Unknown("the monitors could not be listed".to_owned());
        }
        let mut coverage = super::windows_coverage(
            &windows(),
            &monitors,
            notification_state(),
            std::process::id(),
        );
        if let Coverage::Covered(cover) = &mut coverage
            && let Some(pid) = cover.pid
        {
            cover.application = process_name(pid);
        }
        coverage
    }
}

fn rect_of(rect: &RECT) -> Rect {
    Rect {
        left: i64::from(rect.left),
        top: i64::from(rect.top),
        right: i64::from(rect.right),
        bottom: i64::from(rect.bottom),
    }
}

const EMPTY_RECT: RECT = RECT {
    left: 0,
    top: 0,
    right: 0,
    bottom: 0,
};

/// Every monitor's rectangle in physical pixels, named by its device.
fn monitors() -> Vec<Screen> {
    #[allow(unsafe_code)]
    unsafe extern "system" fn collect(
        hmonitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        // SAFETY: lparam is the &mut Vec<Screen> `monitors` passes, and this
        // callback runs synchronously on that thread inside
        // EnumDisplayMonitors, while the Vec is borrowed for nothing else.
        #[allow(unsafe_code)]
        let screens = unsafe { &mut *(lparam as *mut Vec<Screen>) };
        let mut info = MONITORINFOEXW {
            monitorInfo: MONITORINFO {
                cbSize: u32::try_from(std::mem::size_of::<MONITORINFOEXW>()).unwrap_or(0),
                rcMonitor: EMPTY_RECT,
                rcWork: EMPTY_RECT,
                dwFlags: 0,
            },
            szDevice: [0; 32],
        };
        // SAFETY: hmonitor comes from EnumDisplayMonitors, and info is a
        // MONITORINFOEXW with cbSize set to its size, which GetMonitorInfoW
        // fills as the extended structure.
        #[allow(unsafe_code)]
        let ok = unsafe { GetMonitorInfoW(hmonitor, (&raw mut info).cast::<MONITORINFO>()) };
        if ok != 0 {
            screens.push(Screen {
                name: Some(wide_to_string(&info.szDevice)),
                rect: rect_of(&info.monitorInfo.rcMonitor),
            });
        }
        TRUE
    }

    let mut screens: Vec<Screen> = Vec::new();
    // SAFETY: a null HDC and clip rectangle enumerate every monitor. The
    // callback gets a pointer to `screens`, which outlives the call, and runs
    // only before EnumDisplayMonitors returns.
    #[allow(unsafe_code)]
    let ok = unsafe {
        EnumDisplayMonitors(
            ptr::null_mut(),
            ptr::null(),
            Some(collect),
            (&raw mut screens) as LPARAM,
        )
    };
    if ok == 0 {
        return Vec::new();
    }
    screens
}

fn wide_to_string(wide: &[u16]) -> String {
    let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}

/// Every top-level window, top of the z-order first.
fn windows() -> Vec<WindowsWindow> {
    #[allow(unsafe_code)]
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam is the &mut Vec<HWND> `windows` passes, and this
        // callback runs synchronously on that thread inside EnumWindows,
        // while the Vec is borrowed for nothing else.
        #[allow(unsafe_code)]
        let handles = unsafe { &mut *(lparam as *mut Vec<HWND>) };
        handles.push(hwnd);
        TRUE
    }

    let mut handles: Vec<HWND> = Vec::new();
    // SAFETY: the callback gets a pointer to `handles`, which outlives the
    // call, and runs only before EnumWindows returns.
    #[allow(unsafe_code)]
    let ok = unsafe { EnumWindows(Some(collect), (&raw mut handles) as LPARAM) };
    if ok == 0 {
        return Vec::new();
    }
    handles.into_iter().filter_map(describe).collect()
}

/// What `hwnd` is, or `None` for a window that is not visible, which is most
/// of them and needs nothing else asked.
#[allow(unsafe_code)]
fn describe(hwnd: HWND) -> Option<WindowsWindow> {
    // SAFETY: each call below takes a window handle EnumWindows produced and,
    // where it writes, a pointer to a local of the size it is told. A window
    // destroyed since the enumeration makes the calls fail, which each result
    // is checked for, rather than touch anything of ours.
    unsafe {
        if IsWindowVisible(hwnd) == 0 {
            return None;
        }
        let mut rect = EMPTY_RECT;
        if GetWindowRect(hwnd, &raw mut rect) == 0 {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &raw mut pid);
        let mut cloaked = 0u32;
        let cloaked_ok = DwmGetWindowAttribute(
            hwnd,
            u32::try_from(DWMWA_CLOAKED).unwrap_or(0),
            (&raw mut cloaked).cast(),
            u32::try_from(std::mem::size_of::<u32>()).unwrap_or(0),
        ) >= 0;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the extended style is a 32-bit flag set in a pointer-sized long"
        )]
        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let mut class = [0u16; 256];
        let len = GetClassNameW(hwnd, class.as_mut_ptr(), 256);
        let class = String::from_utf16_lossy(&class[..usize::try_from(len).unwrap_or(0)]);
        Some(WindowsWindow {
            pid,
            class,
            application: None,
            rect: rect_of(&rect),
            visible: true,
            minimized: IsIconic(hwnd) != 0,
            cloaked: cloaked_ok && cloaked != 0,
            tool_window: ex_style & WS_EX_TOOLWINDOW != 0,
            layered: ex_style & WS_EX_LAYERED != 0,
            transparent: ex_style & WS_EX_TRANSPARENT != 0,
        })
    }
}

fn notification_state() -> NotificationState {
    let mut state = 0;
    // SAFETY: SHQueryUserNotificationState writes one enum value to the
    // pointer it is given, a local that outlives the call.
    #[allow(unsafe_code)]
    let result = unsafe { SHQueryUserNotificationState(&raw mut state) };
    if result < 0 {
        return NotificationState::Other;
    }
    match state {
        QUNS_RUNNING_D3D_FULL_SCREEN => NotificationState::ExclusiveDirect3D,
        QUNS_PRESENTATION_MODE => NotificationState::PresentationMode,
        QUNS_BUSY => NotificationState::Busy,
        _ => NotificationState::Other,
    }
}

/// The executable's file name of process `pid`, where it may be asked.
fn process_name(pid: u32) -> Option<String> {
    // SAFETY: OpenProcess with the limited query right returns a handle or
    // null, which is checked. QueryFullProcessImageNameW writes at most
    // `len` UTF-16 units into the local buffer and the length back to `len`.
    // The handle is closed once, here.
    #[allow(unsafe_code)]
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buffer = [0u16; 1024];
        let mut len = u32::try_from(buffer.len()).unwrap_or(0);
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            buffer.as_mut_ptr(),
            &raw mut len,
        );
        CloseHandle(process);
        if ok == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buffer[..usize::try_from(len).unwrap_or(0)]);
        std::path::Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    }
}
