//! The app's main window as the desktop sees it: its rectangle, the rectangle
//! of the monitor it is on, and keys pressed into it.
//!
//! Win32 on Windows and `xdotool` with `xrandr` on X11, which the Linux guest
//! image installs. A Wayland session lets no client read another's window or
//! type into it, and macOS has no guest, so both answer `None`.

/// Left, top, right and bottom in desktop pixels.
pub(crate) type Rect = [i32; 4];

#[derive(Clone, Copy, Debug)]
pub(crate) enum Key {
    F11,
    Escape,
}

/// The main window of process `pid`, once it is on screen.
pub(crate) struct MainWindow(imp::Handle);

impl MainWindow {
    /// Find the window, or `None` where this session cannot look.
    ///
    /// :param pid: the app's process id
    pub(crate) fn find(pid: u32) -> Option<Self> {
        imp::find(pid).map(Self)
    }

    /// The window's rectangle and its monitor's.
    pub(crate) fn rects(&self) -> Option<(Rect, Rect)> {
        imp::rects(&self.0)
    }

    /// Press and release `key` in the window.
    pub(crate) fn press(&self, key: Key) -> Result<(), String> {
        imp::press(&self.0, key)
    }
}

fn title() -> String {
    format!("Sunlit Earth {}", env!("CARGO_PKG_VERSION"))
}

#[cfg(windows)]
mod imp {
    use super::{Key, Rect};
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GetWindowRect, GetWindowThreadProcessId, PostMessageW, WM_KEYDOWN, WM_KEYUP,
    };

    pub(super) struct Handle(HWND);

    pub(super) fn find(pid: u32) -> Option<Handle> {
        let title: Vec<u16> = super::title()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: a null class name, which means any class, and a
        // NUL-terminated title that outlives the call.
        #[allow(unsafe_code)]
        let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        if hwnd.is_null() {
            return None;
        }
        let mut owner = 0u32;
        // SAFETY: a window handle FindWindowW just returned and a writable
        // u32 for the process id.
        #[allow(unsafe_code)]
        unsafe {
            GetWindowThreadProcessId(hwnd, &raw mut owner);
        }
        (owner == pid).then_some(Handle(hwnd))
    }

    pub(super) fn rects(handle: &Handle) -> Option<(Rect, Rect)> {
        let mut window = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: a window handle of a live process and a writable RECT.
        #[allow(unsafe_code)]
        let read = unsafe { GetWindowRect(handle.0, &raw mut window) };
        if read == 0 {
            return None;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the structure's size, a few dozen bytes"
        )]
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            rcMonitor: window,
            rcWork: window,
            dwFlags: 0,
        };
        // SAFETY: the same window handle; MonitorFromWindow with
        // MONITOR_DEFAULTTONEAREST always names a monitor, and `info` is a
        // writable MONITORINFO whose size field is set.
        #[allow(unsafe_code)]
        let found = unsafe {
            let monitor = MonitorFromWindow(handle.0, MONITOR_DEFAULTTONEAREST);
            GetMonitorInfoW(monitor, &raw mut info)
        };
        if found == 0 {
            return None;
        }
        let rect = |r: RECT| [r.left, r.top, r.right, r.bottom];
        Some((rect(window), rect(info.rcMonitor)))
    }

    /// Posted to the window's queue rather than sent through `SendInput`,
    /// which only reaches the foreground window, and a process the harness
    /// started has no claim to be that.
    pub(super) fn press(handle: &Handle, key: Key) -> Result<(), String> {
        let (virtual_key, scan_code): (usize, isize) = match key {
            Key::F11 => (0x7A, 0x57),
            Key::Escape => (0x1B, 0x01),
        };
        let down = 1 | (scan_code << 16);
        #[expect(
            clippy::cast_possible_wrap,
            reason = "the previous-state and transition bits of a key's lParam"
        )]
        let up = down | 0xC000_0000_u32 as i32 as isize;
        for (message, l_param) in [(WM_KEYDOWN, down), (WM_KEYUP, up)] {
            // SAFETY: a window handle of a live process and a keyboard
            // message whose parameters carry no pointers.
            #[allow(unsafe_code)]
            let posted = unsafe { PostMessageW(handle.0, message, virtual_key, l_param) };
            if posted == 0 {
                return Err(format!(
                    "PostMessageW failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{Key, Rect};
    use std::process::Command;

    pub(super) struct Handle(String);

    fn run(program: &str, args: &[&str]) -> Option<String> {
        let output = Command::new(program).args(args).output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }

    pub(super) fn find(pid: u32) -> Option<Handle> {
        if std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t.trim() == "wayland") {
            return None;
        }
        let pid = pid.to_string();
        let title = format!("^{}$", super::title());
        let found = run(
            "xdotool",
            &["search", "--onlyvisible", "--pid", &pid, "--name", &title],
        )?;
        found.lines().last().map(|id| Handle(id.trim().to_owned()))
    }

    pub(super) fn rects(handle: &Handle) -> Option<(Rect, Rect)> {
        let shell = run("xdotool", &["getwindowgeometry", "--shell", &handle.0])?;
        let value = |name: &str| -> Option<i32> {
            shell
                .lines()
                .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))?
                .trim()
                .parse()
                .ok()
        };
        let (x, y) = (value("X")?, value("Y")?);
        let window = [x, y, x + value("WIDTH")?, y + value("HEIGHT")?];
        let monitors = run("xrandr", &["--listmonitors"])?;
        let (cx, cy) = (
            i32::midpoint(window[0], window[2]),
            i32::midpoint(window[1], window[3]),
        );
        let monitor = monitors
            .lines()
            .filter_map(monitor_of)
            .find(|m| (m[0]..m[2]).contains(&cx) && (m[1]..m[3]).contains(&cy))?;
        Some((window, monitor))
    }

    /// One line of `xrandr --listmonitors`, ` 0: +*Virtual-1 1280/339x800/212+0+0  Virtual-1`.
    fn monitor_of(line: &str) -> Option<Rect> {
        let geometry = line.split_whitespace().nth(2)?;
        let (width, rest) = geometry.split_once('/')?;
        let (_, rest) = rest.split_once('x')?;
        let (height, rest) = rest.split_once('/')?;
        let mut origin = rest.split('+').skip(1);
        let x: i32 = origin.next()?.parse().ok()?;
        let y: i32 = origin.next()?.parse().ok()?;
        let (width, height): (i32, i32) = (width.parse().ok()?, height.parse().ok()?);
        Some([x, y, x + width, y + height])
    }

    /// Through XTEST to the focused window, after making it that: a key sent
    /// to a window directly arrives marked as synthetic, which toolkits may
    /// ignore.
    pub(super) fn press(handle: &Handle, key: Key) -> Result<(), String> {
        let name = match key {
            Key::F11 => "F11",
            Key::Escape => "Escape",
        };
        run(
            "xdotool",
            &["windowactivate", "--sync", &handle.0, "key", name],
        )
        .map(drop)
        .ok_or_else(|| format!("xdotool could not press {name} in window {}", handle.0))
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use super::{Key, Rect};

    pub(super) struct Handle;

    pub(super) fn find(_pid: u32) -> Option<Handle> {
        None
    }

    pub(super) fn rects(_handle: &Handle) -> Option<(Rect, Rect)> {
        None
    }

    pub(super) fn press(_handle: &Handle, _key: Key) -> Result<(), String> {
        Err("no way to press a key here".to_owned())
    }
}
