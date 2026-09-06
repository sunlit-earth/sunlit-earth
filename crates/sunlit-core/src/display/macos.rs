//! What the displays are on macOS, asked of CoreGraphics.
//!
//! CoreGraphics rather than AppKit, and that is the whole design of this file.
//! `NSScreen` is main-thread-only, and both callers of [`super::monitors`] are
//! off it: the engine thread publishes a wallpaper, and the `displays`
//! subcommand runs before any event loop exists. `CGGetActiveDisplayList` and
//! everything derived from it need no `MainThreadMarker`, so the query is the
//! same call from anywhere. The one place AppKit is unavoidable is handing a
//! finished image to the desktop, and [`crate::wallpaper`] hops to the main
//! thread for exactly that.
//!
//! Two coordinate spaces meet here. `CGDisplayBounds` answers in points with a
//! top-left origin in the global display space, which is already the sense
//! [`Monitor`] is in and is also the space winit reports a window position in,
//! so [`outputs`] hands the points straight through. [`monitors`] wants
//! physical pixels, so each display's own scale, its mode's pixel width over
//! its bounds' point width, multiplies its rectangle. That is exact for one
//! screen and for several at one scale; a layout that mixes scales puts the
//! span canvas's geometry in the state Windows already has open on the roadmap,
//! and per-monitor mode is exact either way.

use objc2_core_graphics::{
    CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayIsBuiltin, CGDisplayIsMain,
    CGDisplayModeGetPixelHeight, CGDisplayModeGetPixelWidth, CGError, CGGetActiveDisplayList,
};

use super::{Monitor, Output};

/// How many displays one query asks about.
///
/// macOS supports far fewer than this on any machine that exists, and the call
/// takes a fixed buffer, so the array is sized once rather than grown: a
/// session with more displays than this loses the ones past it, which is a
/// better failure than two calls that can disagree with each other.
const MAX_DISPLAYS: usize = 16;

/// One display, as the four CoreGraphics questions answer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Display {
    pub(crate) id: u32,
    /// The bounds in points, top-left origin, in the global display space.
    pub(crate) point_x: f64,
    pub(crate) point_y: f64,
    pub(crate) point_width: f64,
    pub(crate) point_height: f64,
    /// The current mode's size in pixels, which is what a Retina screen has
    /// more of than it has points.
    pub(crate) pixel_width: u32,
    pub(crate) pixel_height: u32,
    pub(crate) main: bool,
    pub(crate) builtin: bool,
}

/// The active displays of this session, or `None` where CoreGraphics refused.
///
/// An empty list is possible and is not a refusal: a login over SSH has no
/// display, and that is what the wallpaper setter's own check reads.
pub(crate) fn active_displays() -> Option<Vec<u32>> {
    let mut ids = [0u32; MAX_DISPLAYS];
    let mut count: u32 = 0;
    // SAFETY: `ids` is an array of `MAX_DISPLAYS` display ids and the count
    // passed is its length, so the call cannot write past it; `count` is a live
    // local for the duration of the call. Both pointers are non-null and
    // aligned because they come from locals of the right types.
    #[allow(unsafe_code)]
    let status = unsafe {
        CGGetActiveDisplayList(
            u32::try_from(MAX_DISPLAYS).unwrap_or(u32::MAX),
            ids.as_mut_ptr(),
            &raw mut count,
        )
    };
    if status != CGError::Success {
        tracing::debug!(code = status.0, "CGGetActiveDisplayList refused");
        return None;
    }
    let count = (count as usize).min(MAX_DISPLAYS);
    Some(ids[..count].to_vec())
}

/// Everything one display says about itself.
pub(crate) fn describe(id: u32) -> Display {
    let bounds = CGDisplayBounds(id);
    let mode = CGDisplayCopyDisplayMode(id);
    let (pixel_width, pixel_height) = mode.as_deref().map_or((0, 0), |mode| {
        (
            CGDisplayModeGetPixelWidth(Some(mode)),
            CGDisplayModeGetPixelHeight(Some(mode)),
        )
    });
    Display {
        id,
        point_x: bounds.origin.x,
        point_y: bounds.origin.y,
        point_width: bounds.size.width,
        point_height: bounds.size.height,
        pixel_width: u32::try_from(pixel_width).unwrap_or(0),
        pixel_height: u32::try_from(pixel_height).unwrap_or(0),
        main: CGDisplayIsMain(id),
        builtin: CGDisplayIsBuiltin(id),
    }
}

/// The key a wallpaper job addresses this display by.
///
/// The ColorSync UUID, which is the closest macOS has to Windows' device path:
/// a `CGDirectDisplayID` is reassigned across a reboot and even across a wake
/// where external screens come back in a different order, and a serial number
/// collides between two monitors of the same model. `display-<id>` is the
/// fallback where the UUID cannot be had, and it is the anchor that then does
/// not survive a reboot; `docs/platforms.md` says so.
///
/// The panic guard is the binding's, not this call's: `objc2-color-sync`
/// declares the function non-null and asserts on NULL, and Apple documents NULL
/// for a display id that is no longer valid. The ids here came from
/// `CGGetActiveDisplayList` microseconds earlier, so that is a race with a
/// screen being unplugged, and losing it must give this monitor a duller name
/// rather than take the process down.
pub(crate) fn display_key(id: u32) -> String {
    // SAFETY: `id` came from `CGGetActiveDisplayList`, which is what the
    // function documents as its argument, and the returned UUID is owned by the
    // `CFRetained` the binding wraps it in.
    #[allow(unsafe_code)]
    let uuid = std::panic::catch_unwind(|| unsafe {
        objc2_color_sync::CGDisplayCreateUUIDFromDisplayID(id)
    });
    match uuid {
        Ok(uuid) => objc2_core_foundation::CFUUID::new_string(None, Some(&uuid))
            .map(|text| text.to_string())
            .unwrap_or_else(|| format!("display-{id}")),
        Err(_) => {
            tracing::debug!(id, "this display has no ColorSync UUID any more");
            format!("display-{id}")
        }
    }
}

/// What to call a display in the settings window.
///
/// Numbered the way the Windows labels are, so the two platforms read alike,
/// and the built-in screen says so because "Display 1" and "Display 2" tell a
/// laptop user nothing about which is the lid.
fn display_label(display: &Display, index: usize) -> String {
    if display.builtin {
        format!("Display {} (built-in)", index + 1)
    } else {
        format!("Display {}", index + 1)
    }
}

/// This display's own scale: pixels per point, or 1.0 where it cannot be read.
///
/// Not `NSScreen::backingScaleFactor`, which needs the main thread. A mode that
/// answered with no pixel size, which is what a display being reconfigured
/// under the query looks like, leaves the rectangle in points rather than
/// scaling it by zero.
fn scale(display: &Display) -> f64 {
    if display.point_width <= 0.0 || display.pixel_width == 0 {
        return 1.0;
    }
    f64::from(display.pixel_width) / display.point_width
}

/// A point coordinate in physical pixels.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a display coordinate in pixels is inside i32 on every machine that exists, and saturating_cast is what f64 as i32 already does"
)]
fn to_pixels_i32(points: f64, scale: f64) -> i32 {
    (points * scale).round() as i32
}

/// A point extent in physical pixels, never zero.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "an extent is positive and inside u32 on every machine that exists"
)]
fn to_pixels_u32(points: f64, scale: f64) -> u32 {
    let value = (points * scale).round();
    if value < 1.0 { 1 } else { value as u32 }
}

/// One display as a [`Monitor`]: its rectangle in physical pixels.
fn monitor(display: &Display, index: usize) -> Monitor {
    let scale = scale(display);
    Monitor {
        id: display_key(display.id),
        label: display_label(display, index),
        x: to_pixels_i32(display.point_x, scale),
        y: to_pixels_i32(display.point_y, scale),
        width: to_pixels_u32(display.point_width, scale),
        height: to_pixels_u32(display.point_height, scale),
        primary: display.main,
    }
}

/// One display as an [`Output`]: its rectangle in points.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a window position in points is inside i32, and an extent is positive and inside u32"
)]
fn output(display: &Display, index: usize) -> Output {
    Output {
        name: display_label(display, index),
        primary: display.main,
        width: (display.point_width.round().max(1.0)) as u32,
        height: (display.point_height.round().max(1.0)) as u32,
        x: display.point_x.round() as i32,
        y: display.point_y.round() as i32,
    }
}

/// Every monitor this session has, in physical pixels.
pub(crate) fn monitors() -> Option<Vec<Monitor>> {
    let displays = active_displays()?;
    Some(
        displays
            .iter()
            .map(|id| describe(*id))
            .enumerate()
            .map(|(index, display)| monitor(&display, index))
            .collect(),
    )
}

/// Every monitor this session has, in points.
///
/// Points because this is what the window-position check reads, and winit
/// reports a window's position in points on macOS. Nothing else uses it.
pub(crate) fn outputs() -> Option<Vec<Output>> {
    let displays = active_displays()?;
    Some(
        displays
            .iter()
            .map(|id| describe(*id))
            .enumerate()
            .map(|(index, display)| output(&display, index))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x laptop screen at the origin, and a 1x external screen to the right
    /// of it, which is the layout question 4 in the research document asks a
    /// tester about.
    fn retina() -> Display {
        Display {
            id: 1,
            point_x: 0.0,
            point_y: 0.0,
            point_width: 1512.0,
            point_height: 982.0,
            pixel_width: 3024,
            pixel_height: 1964,
            main: true,
            builtin: true,
        }
    }

    fn external() -> Display {
        Display {
            id: 2,
            point_x: 1512.0,
            point_y: 0.0,
            point_width: 1920.0,
            point_height: 1080.0,
            pixel_width: 1920,
            pixel_height: 1080,
            main: false,
            builtin: false,
        }
    }

    /// A Retina screen has twice the pixels it has points, and that is what a
    /// wallpaper is rendered at: a monitor described in points would be painted
    /// at half resolution on the screen that most needs the other half.
    #[test]
    fn a_monitor_is_its_display_in_physical_pixels() {
        let monitor = monitor(&retina(), 0);
        assert_eq!((monitor.width, monitor.height), (3024, 1964));
        assert_eq!((monitor.x, monitor.y), (0, 0));
        assert!(monitor.primary);

        // And a 1x screen is the same number twice, so the scaling cannot be a
        // constant somebody has to remember to turn off.
        let monitor = monitor(&external(), 1);
        assert_eq!((monitor.width, monitor.height), (1920, 1080));
        assert_eq!((monitor.x, monitor.y), (1512, 0));
        assert!(!monitor.primary);
    }

    /// The window-position check reads points, because that is the space winit
    /// reports a window position in.
    #[test]
    fn an_output_is_its_display_in_points() {
        let output = output(&retina(), 0);
        assert_eq!((output.width, output.height), (1512, 982));
        assert_eq!((output.x, output.y), (0, 0));
        // Which is exactly what the shared overlap check then answers over.
        assert!(output.overlaps(100, 100, 800, 30));
        assert!(!output.overlaps(1512, 100, 800, 30));
    }

    /// A display being reconfigured under the query answers with no mode, and
    /// the rectangle then stays in points rather than collapsing to nothing.
    #[test]
    fn a_display_with_no_mode_keeps_its_points_rather_than_scaling_by_zero() {
        let mut display = retina();
        display.pixel_width = 0;
        display.pixel_height = 0;
        assert!((scale(&display) - 1.0).abs() < f64::EPSILON);
        let monitor = monitor(&display, 0);
        assert_eq!((monitor.width, monitor.height), (1512, 982));
        // And a display with no points either is still a monitor with an area,
        // because a zero-sized render target is not something to hand a GPU.
        display.point_width = 0.0;
        display.point_height = 0.0;
        let monitor = monitor(&display, 0);
        assert_eq!((monitor.width, monitor.height), (1, 1));
    }

    /// The label is the Windows one plus the one thing a laptop user needs: a
    /// built-in screen says which of two numbers is the lid.
    #[test]
    fn the_labels_are_numbered_and_the_lid_says_so() {
        assert_eq!(display_label(&retina(), 0), "Display 1 (built-in)");
        assert_eq!(display_label(&external(), 1), "Display 2");
    }

    /// The id is the UUID where there is one and `display-<id>` where there is
    /// not, and never empty: a monitor nothing can address is not one to plan
    /// around, which is what the shared display test asserts.
    #[test]
    fn every_display_key_is_something_a_setter_can_address() {
        let Some(displays) = active_displays() else {
            println!("no CoreGraphics display list in this session, so no key to read");
            return;
        };
        for id in displays {
            let key = display_key(id);
            assert!(!key.is_empty(), "display {id} has no key");
            assert!(!key.contains(' '), "a key is addressed, not shown: {key}");
        }
    }

    /// The whole query, where this session has one. It answers or it says it
    /// cannot, and what it answers is a list of monitors with an area.
    #[test]
    fn the_session_query_answers_with_monitors_that_have_an_area() {
        let Some(monitors) = monitors() else {
            println!("CoreGraphics refused the display list in this session");
            return;
        };
        for monitor in &monitors {
            assert!(monitor.width > 0 && monitor.height > 0, "{monitor:?}");
            assert!(!monitor.id.is_empty(), "{monitor:?}");
            assert!(monitor.label.starts_with("Display "), "{monitor:?}");
        }
        assert_eq!(monitors.len(), outputs().map_or(0, |list| list.len()));
    }
}
