//! Whether another application covers the desktop right now.
//!
//! The engine asks before an automatic wallpaper publish: a full screen video
//! or game hides the desktop, and the publish's render, readback and the
//! desktop's reload of the picture are work that disturbs it for a wallpaper
//! nobody can see. A window of another process covers the desktop when it
//! covers a whole monitor, which is what catches the borderless kind, or when
//! the platform says it is full screen. Where a rule is in doubt it leans
//! towards covered: a false positive holds back a wallpaper nobody sees, a
//! false negative is the stutter.
//!
//! Each platform gathers facts about its windows, and the decisions over those
//! facts are pure functions here, compiled and tested on every platform.

use std::fmt;
use std::sync::Arc;

pub mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// Asks the platform whether another application covers the desktop.
pub trait CoverProbe: Send + Sync {
    fn probe(&self) -> Coverage;
}

/// What a probe found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Coverage {
    Covered(Cover),
    Clear,
    /// The probe could not tell, and why.
    Unknown(String),
}

impl fmt::Display for Coverage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Covered(cover) => cover.fmt(f),
            Self::Clear => f.write_str("none"),
            Self::Unknown(reason) => write!(f, "unknown; {reason}"),
        }
    }
}

/// The window, or the state, that covers the desktop, named as well as the
/// platform can.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cover {
    /// The application, its class or its app ID.
    pub application: Option<String>,
    pub pid: Option<u32>,
    /// The monitor it covers, in the platform's own naming.
    pub monitor: Option<String>,
    pub signal: Signal,
}

impl Cover {
    fn new(signal: Signal) -> Self {
        Self {
            application: None,
            pid: None,
            monitor: None,
            signal,
        }
    }
}

impl fmt::Display for Cover {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.application, self.pid) {
            (Some(application), Some(pid)) => write!(f, "{application} (pid {pid})")?,
            (Some(application), None) => f.write_str(application)?,
            (None, Some(pid)) => write!(f, "pid {pid}")?,
            (None, None) => f.write_str("an application")?,
        }
        if let Some(monitor) = &self.monitor {
            write!(f, " on {monitor}")?;
        }
        write!(f, ", {}", self.signal)
    }
}

/// How a cover was recognized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// The platform reports the window as full screen.
    FullScreenState,
    /// The window's rectangle contains a whole monitor.
    Geometry,
    /// Windows reports a Direct3D application in exclusive full screen.
    ExclusiveDirect3D,
    /// Windows' presentation settings are on.
    PresentationMode,
    /// An application keeps the screen awake, which stands in for full screen
    /// where no window state can be read.
    IdleInhibited,
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::FullScreenState => "full screen state",
            Self::Geometry => "covers the screen",
            Self::ExclusiveDirect3D => "exclusive Direct3D",
            Self::PresentationMode => "presentation mode",
            Self::IdleInhibited => "keeps the screen awake",
        })
    }
}

/// This platform's probe, or `None` where there is none.
pub fn system_probe() -> Option<Arc<dyn CoverProbe>> {
    #[cfg(windows)]
    return Some(Arc::new(windows::WindowsProbe::new()));
    #[cfg(target_os = "macos")]
    return Some(Arc::new(macos::MacProbe));
    #[cfg(target_os = "linux")]
    return Some(Arc::new(linux::LinuxProbe::new()));
    #[allow(unreachable_code)]
    None
}

/// An axis-aligned rectangle by its edges, right and bottom exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i64,
    pub top: i64,
    pub right: i64,
    pub bottom: i64,
}

impl Rect {
    pub fn from_origin(x: i64, y: i64, width: i64, height: i64) -> Self {
        Self {
            left: x,
            top: y,
            right: x + width,
            bottom: y + height,
        }
    }

    /// Whether this rectangle contains `other`, edges included.
    pub fn contains(&self, other: &Self) -> bool {
        self.left <= other.left
            && self.top <= other.top
            && self.right >= other.right
            && self.bottom >= other.bottom
    }

    fn overlap(&self, other: &Self) -> i64 {
        let w = self.right.min(other.right) - self.left.max(other.left);
        let h = self.bottom.min(other.bottom) - self.top.max(other.top);
        if w > 0 && h > 0 { w * h } else { 0 }
    }

    fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }
}

/// A monitor as one route sees it, in that route's coordinate space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    pub name: Option<String>,
    pub rect: Rect,
}

/// A window that passed its platform's skip rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub application: Option<String>,
    pub pid: Option<u32>,
    pub rect: Rect,
    /// The platform reports it as full screen.
    pub full_screen: bool,
}

/// The first candidate that covers a screen, by its full screen state or by
/// its rectangle containing one. A full screen candidate is placed on the
/// screen its rectangle overlaps most.
pub fn first_cover(candidates: &[Candidate], screens: &[Screen]) -> Option<Cover> {
    candidates.iter().find_map(|candidate| {
        let (signal, screen) = if let Some(screen) = screens
            .iter()
            .find(|screen| !screen.rect.is_empty() && candidate.rect.contains(&screen.rect))
        {
            (Signal::Geometry, Some(screen))
        } else if candidate.full_screen {
            let screen = screens
                .iter()
                .map(|screen| (screen.rect.overlap(&candidate.rect), screen))
                .filter(|(overlap, _)| *overlap > 0)
                .max_by_key(|(overlap, _)| *overlap)
                .map(|(_, screen)| screen);
            (Signal::FullScreenState, screen)
        } else {
            return None;
        };
        let signal = if candidate.full_screen {
            Signal::FullScreenState
        } else {
            signal
        };
        Some(Cover {
            application: candidate.application.clone(),
            pid: candidate.pid,
            monitor: screen.and_then(|screen| screen.name.clone()),
            signal,
        })
    })
}

/// [`first_cover`] as a [`Coverage`].
pub fn coverage_of(candidates: &[Candidate], screens: &[Screen]) -> Coverage {
    first_cover(candidates, screens).map_or(Coverage::Clear, Coverage::Covered)
}

/// Windows: the shell's own windows, which cover the desktop by design.
pub const WINDOWS_SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "NotifyIconOverflowWindow",
    "Windows.UI.Core.CoreWindow",
    "MultitaskingViewFrame",
    "XamlExplorerHostIslandWindow",
    "WindowsDashboard",
];

/// Windows: what one top-level window is.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the facts the skip rule reads, one per window style or state"
)]
pub struct WindowsWindow {
    pub pid: u32,
    pub class: String,
    pub application: Option<String>,
    pub rect: Rect,
    pub visible: bool,
    pub minimized: bool,
    pub cloaked: bool,
    pub tool_window: bool,
    pub layered: bool,
    pub transparent: bool,
}

impl WindowsWindow {
    /// Whether this window can cover a monitor at all.
    pub fn counts(&self, own_pid: u32) -> bool {
        self.visible
            && !self.minimized
            && !self.cloaked
            && !self.tool_window
            && (!self.layered || !self.transparent)
            && self.pid != own_pid
            && !WINDOWS_SHELL_CLASSES.contains(&self.class.as_str())
    }
}

/// Windows: what `SHQueryUserNotificationState` answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationState {
    ExclusiveDirect3D,
    PresentationMode,
    /// `QUNS_BUSY`, which is not trusted alone.
    Busy,
    Other,
}

/// Windows: the decision over every top-level window, the monitors, and the
/// shell's notification state.
pub fn windows_coverage(
    windows: &[WindowsWindow],
    monitors: &[Screen],
    state: NotificationState,
    own_pid: u32,
) -> Coverage {
    let candidates: Vec<Candidate> = windows
        .iter()
        .filter(|window| window.counts(own_pid))
        .map(|window| Candidate {
            application: window.application.clone(),
            pid: Some(window.pid),
            rect: window.rect,
            full_screen: false,
        })
        .collect();
    if let Some(cover) = first_cover(&candidates, monitors) {
        return Coverage::Covered(cover);
    }
    match state {
        NotificationState::ExclusiveDirect3D => {
            Coverage::Covered(Cover::new(Signal::ExclusiveDirect3D))
        }
        NotificationState::PresentationMode => {
            Coverage::Covered(Cover::new(Signal::PresentationMode))
        }
        NotificationState::Busy | NotificationState::Other => Coverage::Clear,
    }
}

/// X11: `_NET_WM_DESKTOP`'s value for a window on every desktop.
pub const X11_ALL_DESKTOPS: u32 = 0xFFFF_FFFF;

/// X11: the window types the skip rule reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum X11Type {
    Desktop,
    Dock,
    Other,
}

/// X11: what one client of the window manager, or one override-redirect child
/// of the root, is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct X11Window {
    pub application: Option<String>,
    pub pid: Option<u32>,
    /// In root coordinates.
    pub rect: Rect,
    /// Mapped, with its ancestors, and drawn: an input-only window, such as
    /// Mutter's guard window over the whole screen, is never viewable here.
    pub viewable: bool,
    pub hidden: bool,
    pub full_screen: bool,
    pub desktop: Option<u32>,
    pub kind: X11Type,
}

impl X11Window {
    /// Whether this client can cover a monitor at all.
    pub fn counts(&self, current_desktop: Option<u32>, own_pid: u32) -> bool {
        let on_this_desktop = match (self.desktop, current_desktop) {
            (Some(X11_ALL_DESKTOPS) | None, _) | (_, None) => true,
            (Some(desktop), Some(current)) => desktop == current,
        };
        self.viewable
            && !self.hidden
            && on_this_desktop
            && self.kind == X11Type::Other
            && self.pid != Some(own_pid)
    }
}

/// X11: the decision over the window manager's clients, top of the stack
/// first, and the root's override-redirect children, which count by geometry
/// alone.
pub fn x11_coverage(
    clients: &[X11Window],
    override_redirect: &[X11Window],
    monitors: &[Screen],
    current_desktop: Option<u32>,
    own_pid: u32,
) -> Coverage {
    let mut candidates: Vec<Candidate> = clients
        .iter()
        .filter(|window| window.counts(current_desktop, own_pid))
        .map(|window| Candidate {
            application: window.application.clone(),
            pid: window.pid,
            rect: window.rect,
            full_screen: window.full_screen,
        })
        .collect();
    candidates.extend(
        override_redirect
            .iter()
            .filter(|window| window.viewable && window.pid != Some(own_pid))
            .map(|window| Candidate {
                application: window.application.clone(),
                pid: window.pid,
                rect: window.rect,
                full_screen: false,
            }),
    );
    coverage_of(&candidates, monitors)
}

/// wlroots: what the foreign toplevel manager says of one toplevel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WlrToplevel {
    pub app_id: Option<String>,
    pub full_screen: bool,
    /// The names of the outputs it has entered.
    pub outputs: Vec<String>,
}

/// wlroots: a toplevel covers when it is full screen and on an output. A full
/// screen toplevel on a hidden workspace has no output.
pub fn wlr_coverage(toplevels: &[WlrToplevel]) -> Coverage {
    toplevels
        .iter()
        .find(|toplevel| toplevel.full_screen && !toplevel.outputs.is_empty())
        .map_or(Coverage::Clear, |toplevel| {
            Coverage::Covered(Cover {
                application: toplevel.app_id.clone(),
                pid: None,
                monitor: toplevel.outputs.first().cloned(),
                signal: Signal::FullScreenState,
            })
        })
}

/// macOS: how far below a display's top a window may start and still cover
/// it, in points. A full screen window on a display with a notch starts below
/// the notch.
pub const NOTCH_TOLERANCE: i64 = 50;

/// macOS: what `CGWindowListCopyWindowInfo` says of one on-screen window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacWindow {
    pub pid: u32,
    pub layer: i64,
    /// In global display coordinates, points, the origin at the top left.
    pub bounds: Rect,
}

/// macOS: a layer 0 window of another process covers a display when it spans
/// it horizontally and to the bottom, and starts at most [`NOTCH_TOLERANCE`]
/// below its top.
pub fn mac_coverage(windows: &[MacWindow], displays: &[Screen], own_pid: u32) -> Coverage {
    for window in windows
        .iter()
        .filter(|window| window.layer == 0 && window.pid != own_pid)
    {
        let b = &window.bounds;
        if let Some(display) = displays.iter().find(|display| {
            let d = &display.rect;
            !d.is_empty()
                && b.left <= d.left
                && b.right >= d.right
                && b.bottom >= d.bottom
                && b.top <= d.top + NOTCH_TOLERANCE
        }) {
            return Coverage::Covered(Cover {
                application: None,
                pid: Some(window.pid),
                monitor: display.name.clone(),
                signal: Signal::Geometry,
            });
        }
    }
    Coverage::Clear
}

#[cfg(test)]
mod tests;
