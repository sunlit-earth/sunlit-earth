//! Where the main window comes back, and whether it fits there.
//!
//! Per-OS work that has nothing to do with TOML: the geometry is written back
//! through the config module, but deciding where a saved window still fits is
//! a question for the display, and on Windows it is a Win32 call. A config
//! carried from a multi-monitor desk to a laptop is what this is for, and so is
//! a size no screen of this desk can hold.

use tracing::{debug, warn};

use super::{AppConfig, load_config, save_config};
use crate::display::layout::Rect;

/// Save only the window position and size to disk, preserving all other
/// config values. This is called on window close so geometry is always
/// persisted, even when the user hasn't clicked "Set as Wallpaper".
pub fn save_window_geometry(x: i32, y: i32, width: u32, height: u32) {
    let mut config = load_config();
    config.window_x = Some(x);
    config.window_y = Some(y);
    config.window_width = Some(width);
    config.window_height = Some(height);
    save_config(&config);
}

/// How far a window's outer rectangle reaches past its content on each side,
/// in the same pixels as the position.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Frame {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

impl Frame {
    fn horizontal(self) -> u64 {
        u64::from(self.left) + u64::from(self.right)
    }

    fn vertical(self) -> u64 {
        u64::from(self.top) + u64::from(self.bottom)
    }
}

/// How much of the window's top edge has to be on a screen for a person to
/// drag it.
const TITLE_BAR_HEIGHT: u32 = 30;

/// One monitor: the whole of it, and the part a window may occupy.
///
/// On Windows `work` leaves out the taskbar and docked bars. Elsewhere nothing
/// reports one, and the two are the same rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Screen {
    monitor: Rect,
    work: Rect,
}

impl Screen {
    #[cfg(any(test, not(windows)))]
    fn whole(rect: Rect) -> Self {
        Self {
            monitor: rect,
            work: rect,
        }
    }
}

/// Fit a saved window into the work area of the monitor its title bar is on.
///
/// `saved` is a position and a size the way Slint takes them: `set_position`
/// places the outer rectangle, frame included, and `set_size` sizes the
/// content, frame excluded. The monitor with the largest share of the title
/// bar strip is the one the window belongs to, a taskbar on it included, and
/// none at all means the window would come back where nobody can reach it. The
/// outer rectangle shrinks to that monitor's work area where it is larger, and
/// moves into it, keeping the top left corner where it can.
///
/// :param saved: the outer position and the content size
/// :param frame: what the system adds around the content
/// :param screens: the monitors of this desk
/// :returns: the outer position and the content size that fit, or `None` when
///     the title bar is on no monitor
fn fit(saved: Rect, frame: Frame, screens: &[Screen]) -> Option<Rect> {
    let outer_width = u64::from(saved.width) + frame.horizontal();
    let outer_height = u64::from(saved.height) + frame.vertical();
    let strip = Rect {
        x: saved.x,
        y: saved.y,
        width: u32::try_from(outer_width).unwrap_or(u32::MAX),
        height: TITLE_BAR_HEIGHT.min(u32::try_from(outer_height).unwrap_or(u32::MAX)),
    };
    let area = screens
        .iter()
        .map(|screen| (screen.monitor.overlap_area(&strip), screen))
        .filter(|(overlap, _)| *overlap > 0)
        .max_by_key(|(overlap, _)| *overlap)
        .map(|(_, screen)| screen.work)?;

    let outer_width = outer_width.min(u64::from(area.width));
    let outer_height = outer_height.min(u64::from(area.height));
    Some(Rect {
        x: place(saved.x, area.x, area.width, outer_width),
        y: place(saved.y, area.y, area.height, outer_height),
        width: content(outer_width, frame.horizontal()),
        height: content(outer_height, frame.vertical()),
    })
}

/// Move one edge so that `outer` pixels from it lie inside the area's span.
///
/// `outer` is at most `extent`, so the span always has room.
fn place(start: i32, area_start: i32, extent: u32, outer: u64) -> i32 {
    let first = i64::from(area_start);
    let last = first + i64::from(extent) - i64::try_from(outer).unwrap_or(0);
    i32::try_from(i64::from(start).clamp(first, last)).unwrap_or(area_start)
}

/// The content's extent inside an outer one, never below a pixel.
fn content(outer: u64, frame: u64) -> u32 {
    u32::try_from(outer.saturating_sub(frame))
        .unwrap_or(u32::MAX)
        .max(1)
}

/// Each monitor with its work area, which excludes the taskbar, in physical
/// pixels.
#[cfg(windows)]
fn screens() -> Option<Vec<Screen>> {
    use std::ptr;

    use windows_sys::Win32::Foundation::{LPARAM, RECT, TRUE};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    use windows_sys::core::BOOL;

    #[allow(unsafe_code)]
    unsafe extern "system" fn collect(
        hmonitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        // SAFETY: lparam is the &mut Vec<Screen> screens passes, and this
        // callback runs synchronously on that thread inside
        // EnumDisplayMonitors, while the Vec is borrowed for nothing else.
        #[allow(unsafe_code)]
        let screens = unsafe { &mut *(lparam as *mut Vec<Screen>) };
        let mut info = MONITORINFO {
            cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or(0),
            rcMonitor: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            rcWork: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            dwFlags: 0,
        };
        // SAFETY: hmonitor comes from EnumDisplayMonitors, and info is a
        // MONITORINFO with cbSize set, which GetMonitorInfoW fills.
        #[allow(unsafe_code)]
        let ok = unsafe { GetMonitorInfoW(hmonitor, &raw mut info) };
        if ok != 0 {
            screens.push(Screen {
                monitor: rect_of(info.rcMonitor),
                work: rect_of(info.rcWork),
            });
        }
        TRUE
    }

    fn rect_of(rect: RECT) -> Rect {
        Rect {
            x: rect.left,
            y: rect.top,
            width: rect.right.saturating_sub(rect.left).unsigned_abs(),
            height: rect.bottom.saturating_sub(rect.top).unsigned_abs(),
        }
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
        warn!("cannot enumerate the monitors to fit the window into");
        return None;
    }
    Some(screens)
}

/// The frame Windows puts around a window of this app's style, at the DPI of
/// the monitor the saved title bar is nearest to.
///
/// `WS_OVERLAPPEDWINDOW` with a menu, since the menu bar is native and outside
/// the content. The outer rectangle includes the resize borders the DWM draws
/// invisibly, so a fitted window looks a few pixels inside its area at the
/// sides; a 1200 by 5000 window restored on a 1920 by 1032 work area in the
/// Windows guest measured `[0, 0, 1216, 1012]` from `GetWindowRect`, 20 px short of the
/// bottom.
#[cfg(windows)]
fn frame_at(saved: Rect) -> Frame {
    use windows_sys::Win32::Foundation::{RECT, TRUE};
    use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromRect};
    use windows_sys::Win32::UI::HiDpi::{
        AdjustWindowRectExForDpi, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        USER_DEFAULT_SCREEN_DPI, WS_OVERLAPPEDWINDOW,
    };

    let strip = RECT {
        left: saved.x,
        top: saved.y,
        right: saved
            .x
            .saturating_add(i32::try_from(saved.width).unwrap_or(i32::MAX)),
        bottom: saved.y.saturating_add(TITLE_BAR_HEIGHT.cast_signed()),
    };
    // SAFETY: MonitorFromRect reads the local RECT and returns a monitor
    // handle, the nearest one with MONITOR_DEFAULTTONEAREST.
    #[allow(unsafe_code)]
    let monitor = unsafe { MonitorFromRect(&raw const strip, MONITOR_DEFAULTTONEAREST) };
    let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
    // SAFETY: monitor is a handle MonitorFromRect returned, and the two
    // out-parameters are locals GetDpiForMonitor writes.
    #[allow(unsafe_code)]
    let result =
        unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &raw mut dpi_x, &raw mut dpi_y) };
    let dpi = if result >= 0 && dpi_y > 0 {
        dpi_y
    } else {
        USER_DEFAULT_SCREEN_DPI
    };
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: AdjustWindowRectExForDpi grows the local RECT by the frame of
    // the given style at the given DPI and touches nothing else.
    #[allow(unsafe_code)]
    let ok = unsafe { AdjustWindowRectExForDpi(&raw mut rect, WS_OVERLAPPEDWINDOW, TRUE, 0, dpi) };
    if ok == 0 {
        warn!("cannot measure the window frame; fitting the content alone");
        return Frame::default();
    }
    let frame = Frame {
        left: rect.left.unsigned_abs(),
        top: rect.top.unsigned_abs(),
        right: rect.right.unsigned_abs(),
        bottom: rect.bottom.unsigned_abs(),
    };
    debug!(dpi, ?frame, "the window frame at the saved position");
    frame
}

#[cfg(windows)]
fn fit_to_this_desk(saved: Rect) -> Option<Rect> {
    crate::wallpaper::ensure_dpi_awareness();
    let screens = screens()?;
    fit(saved, frame_at(saved), &screens)
}

/// What a window manager adds, estimated: nothing at the sides and a title bar
/// on top. Nothing reports the work area or the frame here, and a window
/// manager constrains an oversized window itself as well.
#[cfg(not(windows))]
const ESTIMATED_FRAME: Frame = Frame {
    left: 0,
    top: TITLE_BAR_HEIGHT,
    right: 0,
    bottom: 0,
};

/// The real monitors where there are any, and a coordinate-range check where
/// there is nothing to ask.
///
/// On Linux `display::monitors` is the outputs `xrandr --query` lists, and on
/// macOS the CoreGraphics displays in pixels, which is the space winit reports
/// a window's position and size in, where the displays in points would shrink
/// a window on a Retina screen to half its size. Both give the shape of answer
/// Win32 gives: a set of rectangles in one coordinate space. Under Wayland the
/// compositor ignores the position, and only the size the fit comes to
/// matters.
#[cfg(not(windows))]
fn fit_to_this_desk(saved: Rect) -> Option<Rect> {
    if let Some(monitors) = crate::display::monitors()
        && !monitors.is_empty()
    {
        let screens: Vec<Screen> = monitors
            .iter()
            .map(|monitor| Screen::whole(monitor.rect()))
            .collect();
        return fit(saved, ESTIMATED_FRAME, &screens);
    }
    plausible_coordinates(saved).then_some(saved)
}

/// Whether a saved geometry is inside the range a window position can express.
///
/// Much coarser than a monitor query: a position inside the range but on a
/// monitor that has since been unplugged passes here. It is the part that matters
/// most, though, which is stopping a config carried from a large multi-monitor
/// desk to a laptop from restoring a window into nowhere with no way to get it
/// back.
///
/// X11's core protocol carries window coordinates as `INT16`, so on that
/// display server -32768..=32767 is the whole of what a position can express;
/// Wayland and macOS impose no such limit, but a coordinate outside it is far
/// outside any desktop either way. The bound is chosen for being the one
/// platform-defined number in the neighborhood, not because every platform
/// enforces it.
#[cfg(not(windows))]
fn plausible_coordinates(saved: Rect) -> bool {
    let range = i64::from(i16::MIN)..=i64::from(i16::MAX);
    let (x, y) = (i64::from(saved.x), i64::from(saved.y));
    range.contains(&x)
        && range.contains(&y)
        && range.contains(&(x + i64::from(saved.width)))
        && range.contains(&(y + i64::from(saved.height)))
}

/// The saved window geometry, fitted to the screen it comes back on.
///
/// Returns `None` if any of the four geometry fields is missing, or if the
/// saved title bar is on no screen of this desk, and the system places the
/// window. Otherwise the position and size that keep the whole window on the
/// screen its title bar is on, which is the saved geometry itself whenever it
/// already fits.
pub fn validated_window_geometry(config: &AppConfig) -> Option<(i32, i32, u32, u32)> {
    let (Some(x), Some(y), Some(width), Some(height)) = (
        config.window_x,
        config.window_y,
        config.window_width,
        config.window_height,
    ) else {
        return None;
    };
    if width == 0 || height == 0 {
        return None;
    }
    let saved = Rect {
        x,
        y,
        width,
        height,
    };
    let Some(fitted) = fit_to_this_desk(saved) else {
        warn!(
            x,
            y, "saved window position is off-screen, using OS default"
        );
        return None;
    };
    if fitted != saved {
        debug!(?saved, ?fitted, "fitted the saved window to its screen");
    }
    Some((fitted.x, fitted.y, fitted.width, fitted.height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn whole(areas: &[Rect]) -> Vec<Screen> {
        areas.iter().copied().map(Screen::whole).collect()
    }

    /// A Windows frame at 100 %: resize borders and a caption and a menu bar.
    const FRAME: Frame = Frame {
        left: 8,
        top: 51,
        right: 8,
        bottom: 8,
    };

    #[test]
    fn validated_geometry_none_when_missing() {
        let config = AppConfig::default();
        assert!(validated_window_geometry(&config).is_none());
    }

    #[test]
    fn validated_geometry_none_when_partial() {
        let config = AppConfig {
            window_x: Some(100),
            window_y: Some(200),
            // width and height still None
            ..AppConfig::default()
        };
        assert!(validated_window_geometry(&config).is_none());
    }

    #[test]
    fn validated_geometry_none_when_zero_size() {
        let config = AppConfig {
            window_x: Some(100),
            window_y: Some(200),
            window_width: Some(0),
            window_height: Some(600),
            ..AppConfig::default()
        };
        assert!(validated_window_geometry(&config).is_none());
    }

    /// Windows answers this from the live desktop, so a session that enumerates
    /// no monitor has nothing to fit into. Everywhere else the coarse fallback
    /// answers whether or not a display server is reachable. The desk may be
    /// small enough to move or shrink the window, but never to grow it.
    #[test]
    fn validated_geometry_accepts_on_screen() {
        if cfg!(windows) && crate::display::monitors().is_none_or(|m| m.is_empty()) {
            println!("skipping: this session enumerates no monitor");
            return;
        }
        let config = AppConfig {
            window_x: Some(100),
            window_y: Some(100),
            window_width: Some(800),
            window_height: Some(600),
            ..AppConfig::default()
        };
        let (_, _, width, height) =
            validated_window_geometry(&config).expect("a window on the primary screen comes back");
        assert!(width <= 800 && height <= 600);
    }

    /// Both implementations must reject this: Windows because no monitor
    /// holds the title bar, everywhere else because the coordinates are outside
    /// the range a window position can take.
    #[test]
    fn validated_geometry_rejects_off_screen() {
        let config = AppConfig {
            window_x: Some(-50000),
            window_y: Some(-50000),
            window_width: Some(800),
            window_height: Some(600),
            ..AppConfig::default()
        };
        assert!(validated_window_geometry(&config).is_none());
    }

    /// The coarse half, which is what a run with no display to ask falls back
    /// to, and which is still the only answer on a platform with no query.
    #[test]
    #[cfg(not(windows))]
    fn the_coordinate_range_is_the_one_a_window_position_can_express() {
        assert!(plausible_coordinates(rect(0, 0, 800, 600)));
        assert!(plausible_coordinates(rect(-1000, -1000, 800, 600)));
        // X11 carries these as INT16, so a coordinate outside that is not a desk
        // anybody has: it is a config that came from somewhere else.
        assert!(!plausible_coordinates(rect(-50_000, 0, 800, 600)));
        assert!(!plausible_coordinates(rect(0, 40_000, 800, 600)));
        // And a window whose far edge leaves the range goes with it.
        assert!(!plausible_coordinates(rect(32_000, 0, 800, 600)));
    }

    /// The test window's geometry, which a test once saved into a real config:
    /// at the origin and taller than any screen.
    #[test]
    fn a_window_taller_than_the_screen_comes_back_as_tall_as_it() {
        let area = rect(0, 0, 3440, 1400);
        let fitted = fit(rect(0, 0, 1200, 5000), FRAME, &whole(&[area])).unwrap();
        assert_eq!(fitted, rect(0, 0, 1200, 1400 - 51 - 8));
    }

    #[test]
    fn a_window_larger_than_the_screen_both_ways_fills_it() {
        let area = rect(0, 0, 1920, 1040);
        let fitted = fit(rect(200, 100, 4000, 3000), FRAME, &whole(&[area])).unwrap();
        assert_eq!(fitted, rect(0, 0, 1920 - 16, 1040 - 59));
    }

    #[test]
    fn a_window_hanging_off_the_right_edge_moves_left() {
        let area = rect(0, 0, 1920, 1040);
        let fitted = fit(rect(1500, 100, 800, 600), FRAME, &whole(&[area])).unwrap();
        assert_eq!(fitted, rect(1920 - 816, 100, 800, 600));
    }

    #[test]
    fn a_window_below_the_bottom_edge_moves_up() {
        let area = rect(0, 0, 1920, 1040);
        let fitted = fit(rect(100, 900, 800, 600), FRAME, &whole(&[area])).unwrap();
        assert_eq!(fitted, rect(100, 1040 - 659, 800, 600));
    }

    #[test]
    fn a_title_bar_on_no_screen_is_left_to_the_system() {
        let area = rect(0, 0, 1920, 1040);
        assert_eq!(fit(rect(2500, 100, 800, 600), FRAME, &whole(&[area])), None);
        assert_eq!(fit(rect(100, -500, 800, 400), FRAME, &whole(&[area])), None);
        assert_eq!(fit(rect(100, 100, 800, 600), FRAME, &whole(&[])), None);
    }

    /// The body may hang off a screen; only the title bar decides which one the
    /// window is on.
    #[test]
    fn a_window_whose_title_bar_is_on_screen_but_body_is_not_moves_onto_it() {
        let area = rect(0, 0, 1920, 1040);
        let fitted = fit(rect(100, 1020, 800, 600), FRAME, &whole(&[area])).unwrap();
        assert_eq!(fitted, rect(100, 1040 - 659, 800, 600));
    }

    /// A monitor left of the primary has negative coordinates, and a window
    /// whose title bar is mostly on it belongs to it.
    #[test]
    fn with_two_screens_the_one_the_title_bar_is_on_wins() {
        let left = rect(-2560, 0, 2560, 1400);
        let primary = rect(0, 0, 1920, 1040);
        let areas = [primary, left];

        let mostly_left = fit(rect(-700, 100, 800, 2000), FRAME, &whole(&areas)).unwrap();
        assert_eq!(mostly_left, rect(-816, 0, 800, 1400 - 59));

        let mostly_right = fit(rect(-100, 100, 800, 2000), FRAME, &whole(&areas)).unwrap();
        assert_eq!(mostly_right, rect(0, 0, 800, 1040 - 59));
    }

    /// A taskbar docked at the top: a title bar saved above the work area is
    /// still on the monitor, and comes back below the taskbar.
    #[test]
    fn a_title_bar_in_the_taskbar_band_moves_into_the_work_area() {
        let screen = Screen {
            monitor: rect(0, 0, 1920, 1080),
            work: rect(0, 48, 1920, 1032),
        };
        for y in [0, 17] {
            let fitted = fit(rect(100, y, 800, 600), FRAME, &[screen]).unwrap();
            assert_eq!(fitted, rect(100, 48, 800, 600));
        }
    }

    /// The monitor holding most of the title bar wins even where its share is
    /// all taskbar and the neighbor's share is work area.
    #[test]
    fn the_monitor_counts_its_taskbar_when_choosing() {
        let primary = Screen {
            monitor: rect(0, 0, 1920, 1080),
            work: rect(0, 48, 1920, 1032),
        };
        let right = Screen::whole(rect(1920, 0, 1920, 1080));
        let fitted = fit(rect(1200, 0, 800, 600), FRAME, &[primary, right]).unwrap();
        assert_eq!(fitted, rect(1920 - 816, 48, 800, 600));
    }

    #[test]
    fn a_window_that_fits_comes_back_unchanged() {
        let left = rect(-2560, 0, 2560, 1400);
        let primary = rect(0, 0, 1920, 1040);
        for saved in [rect(100, 100, 800, 600), rect(-2000, 200, 1200, 900)] {
            assert_eq!(fit(saved, FRAME, &whole(&[primary, left])), Some(saved));
        }
        assert_eq!(
            fit(rect(0, 0, 1920 - 16, 1040 - 59), FRAME, &whole(&[primary])),
            Some(rect(0, 0, 1920 - 16, 1040 - 59))
        );
    }

    /// Asked of the live system, which answers at the nearest monitor's DPI
    /// whether or not the session has one.
    #[test]
    #[cfg(windows)]
    fn the_windows_frame_has_borders_and_a_caption_with_a_menu() {
        crate::wallpaper::ensure_dpi_awareness();
        let frame = frame_at(rect(100, 100, 800, 600));
        println!("frame: {frame:?}");
        assert!(frame.left > 0 && frame.left == frame.right, "{frame:?}");
        assert!(frame.top > frame.bottom, "{frame:?}");
    }

    #[test]
    fn the_content_never_shrinks_below_a_pixel() {
        let tiny = rect(0, 0, 10, 10);
        let fitted = fit(rect(0, 0, 800, 600), FRAME, &whole(&[tiny])).unwrap();
        assert_eq!((fitted.width, fitted.height), (1, 1));
    }
}
