//! Where the settings window comes back, and whether that place still exists.
//!
//! Per-OS work that has nothing to do with TOML: the geometry is written back
//! through the config module, but deciding that a saved position is still
//! reachable is a question for the display, and on Windows it is a Win32 call.
//! A config carried from a multi-monitor desk to a laptop is what this is for.

use tracing::warn;

use super::{AppConfig, load_config, save_config};

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

/// Check whether the saved window position is visible on at least one
/// connected monitor by testing if the title bar region overlaps any display.
///
/// Returns `true` if the position is on-screen, `false` if off-screen or
/// if validation cannot be performed.
#[cfg(windows)]
fn is_position_on_screen(x: i32, y: i32, width: u32, height: u32) -> bool {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromRect};

    let title_bar_height = 30i32.min(height.cast_signed());
    let rect = RECT {
        left: x,
        top: y,
        right: x.saturating_add(width.cast_signed()),
        bottom: y.saturating_add(title_bar_height),
    };

    // SAFETY: MonitorFromRect reads a RECT struct and queries the display
    // configuration. The rect is a local stack variable with valid values.
    // MONITOR_DEFAULTTONULL returns null if no monitor contains the rect.
    #[allow(unsafe_code)]
    let monitor = unsafe { MonitorFromRect(&raw const rect, MONITOR_DEFAULTTONULL) };
    !monitor.is_null()
}

/// Non-Windows: the real outputs where there are any, and a coordinate-range
/// check where there is nothing to ask.
///
/// On Linux `display::outputs` parses `xrandr --query`, which gives the same
/// shape of answer Win32 gives: a set of rectangles in one coordinate space, so
/// the title bar can be tested against each.
///
/// The coarse half remains for the platforms with no query, and for a run with
/// no display at all. X11's core protocol carries window coordinates as
/// `INT16`, so on that display server -32768..=32767 is the whole of what a
/// position can express; Wayland and macOS impose no such limit, but a
/// coordinate outside it is far outside any desktop either way. The bound is
/// chosen for being the one platform-defined number in the neighbourhood, not
/// because every platform enforces it.
#[cfg(not(windows))]
fn is_position_on_screen(x: i32, y: i32, width: u32, height: u32) -> bool {
    let Ok(width) = i32::try_from(width) else {
        return false;
    };
    let Ok(height) = i32::try_from(height) else {
        return false;
    };
    if let Some(outputs) = crate::display::outputs()
        && !outputs.is_empty()
    {
        // The same rectangle the Win32 branch tests: a window is reachable if
        // its title bar is, and a window whose body hangs off the bottom of a
        // screen can still be dragged back.
        let title_bar_height = 30.min(height);
        return outputs
            .iter()
            .any(|output| output.overlaps(x, y, width, title_bar_height));
    }
    plausible_coordinates(x, y, width, height)
}

/// Whether a saved geometry is inside the range a window position can express.
///
/// Much coarser than a monitor query: a position inside the range but on a
/// monitor that has since been unplugged passes here. It is the part that matters
/// most, though, which is stopping a config carried from a large multi-monitor
/// desk to a laptop from restoring a window into nowhere with no way to get it
/// back.
#[cfg(not(windows))]
fn plausible_coordinates(x: i32, y: i32, width: i32, height: i32) -> bool {
    const MIN: i32 = i16::MIN as i32;
    const MAX: i32 = i16::MAX as i32;

    (MIN..=MAX).contains(&x)
        && (MIN..=MAX).contains(&y)
        && (MIN..=MAX).contains(&x.saturating_add(width))
        && (MIN..=MAX).contains(&y.saturating_add(height))
}

/// Return the saved window geometry if it passes on-screen validation.
///
/// Returns `None` if any of the four geometry fields is missing or if
/// the saved position is no longer visible on any connected monitor.
pub fn validated_window_geometry(config: &AppConfig) -> Option<(i32, i32, u32, u32)> {
    let (Some(x), Some(y), Some(w), Some(h)) = (
        config.window_x,
        config.window_y,
        config.window_width,
        config.window_height,
    ) else {
        return None;
    };
    if w == 0 || h == 0 {
        return None;
    }
    if is_position_on_screen(x, y, w, h) {
        Some((x, y, w, h))
    } else {
        warn!(
            x,
            y, "saved window position is off-screen, using OS default"
        );
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    /// no monitor has nothing that contains the rectangle. Everywhere else the
    /// coarse fallback answers whether or not a display server is reachable.
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
        assert_eq!(
            validated_window_geometry(&config),
            Some((100, 100, 800, 600))
        );
    }

    /// Both implementations must reject this: Windows because no monitor
    /// contains the rect, everywhere else because the coordinates are outside
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
        assert!(plausible_coordinates(0, 0, 800, 600));
        assert!(plausible_coordinates(-1000, -1000, 800, 600));
        // X11 carries these as INT16, so a coordinate outside that is not a desk
        // anybody has: it is a config that came from somewhere else.
        assert!(!plausible_coordinates(-50_000, 0, 800, 600));
        assert!(!plausible_coordinates(0, 40_000, 800, 600));
        // And a window whose far edge leaves the range goes with it.
        assert!(!plausible_coordinates(32_000, 0, 800, 600));
    }
}
