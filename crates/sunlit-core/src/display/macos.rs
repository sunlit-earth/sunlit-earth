//! What the displays are on macOS, asked of CoreGraphics.
//!
//! CoreGraphics rather than AppKit, and that is the whole design of this file:
//! `NSScreen` is main-thread-only and both callers of [`super::monitors`] are
//! off it, while `CGGetActiveDisplayList` and everything derived from it are
//! the same call from anywhere.
//!
//! Two coordinate spaces meet here. `CGDisplayBounds` answers in points, which
//! [`outputs`] hands straight through because it is the space winit reports a
//! window position in; [`monitors`] wants pixels. `docs/platforms.md` has what
//! that conversion is exact for.

use objc2_core_graphics::{
    CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayIsBuiltin, CGDisplayIsMain, CGDisplayMode,
    CGError, CGGetActiveDisplayList,
};

use super::{Monitor, Output};

/// How many displays one query asks about.
///
/// The call takes a fixed buffer, and one that is sized once loses a display
/// past this rather than risking two calls that disagree with each other.
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
/// An empty list is not a refusal: a login over SSH has no display, which is
/// what the wallpaper setter's own check reads.
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
            CGDisplayMode::pixel_width(Some(mode)),
            CGDisplayMode::pixel_height(Some(mode)),
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
/// The ColorSync UUID, the closest macOS has to Windows' device path, since a
/// `CGDirectDisplayID` is reassigned across a reboot. `display-<id>` is the
/// fallback, and it is the anchor that then does not survive one.
///
/// The panic guard is the binding's, not this call's: `objc2-color-sync`
/// asserts on the NULL that Apple documents for a display id that is no longer
/// valid, which here is a screen unplugged mid-query, and losing that must cost
/// a name rather than the process.
pub(crate) fn display_key(id: u32) -> String {
    // SAFETY: `id` came from `CGGetActiveDisplayList`, which is what the
    // function documents as its argument, and the returned UUID is owned by the
    // `CFRetained` the binding wraps it in.
    #[allow(unsafe_code)]
    let uuid = std::panic::catch_unwind(|| unsafe {
        objc2_color_sync::CGDisplayCreateUUIDFromDisplayID(id)
    });
    match uuid {
        Ok(uuid) => objc2_core_foundation::CFUUID::new_string(None, Some(&*uuid))
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
/// Numbered the way the Windows labels are, and the built-in screen says so,
/// because "Display 1" and "Display 2" do not say which one is the lid.
fn display_label(display: &Display, index: usize) -> String {
    if display.builtin {
        format!("Display {} (built-in)", index + 1)
    } else {
        format!("Display {}", index + 1)
    }
}

/// This display's own scale: pixels per point, or 1.0 where it cannot be read.
///
/// Not `NSScreen::backingScaleFactor`, which needs the main thread. A display
/// being reconfigured under the query answers with no mode, and its rectangle
/// stays in points rather than being scaled by zero.
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
///
/// The extent is the mode's own pixel count. The origin has no such shortcut:
/// the global display space is in points, so where a screen sits in pixels is
/// its point origin times its own scale.
fn monitor(display: &Display, index: usize) -> Monitor {
    let scale = scale(display);
    let (width, height) = if display.pixel_width > 0 && display.pixel_height > 0 {
        (display.pixel_width, display.pixel_height)
    } else {
        // A display being reconfigured under the query answers with no mode.
        (
            to_pixels_u32(display.point_width, scale),
            to_pixels_u32(display.point_height, scale),
        )
    };
    Monitor {
        id: display_key(display.id),
        label: display_label(display, index),
        x: to_pixels_i32(display.point_x, scale),
        y: to_pixels_i32(display.point_y, scale),
        width,
        height,
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
/// Points, because winit reports a window's position in them and the
/// window-position check is the only reader.
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

    /// A 2x laptop screen at the origin and a 1x external screen to the right
    /// of it, which is the layout no test here can answer for.
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

    /// A wallpaper is rendered at the pixel count, so a monitor described in
    /// points would be painted at half resolution on a Retina screen.
    #[test]
    fn a_monitor_is_its_display_in_physical_pixels() {
        let laptop = monitor(&retina(), 0);
        assert_eq!((laptop.width, laptop.height), (3024, 1964));
        assert_eq!((laptop.x, laptop.y), (0, 0));
        assert!(laptop.primary);

        // And a 1x screen is the same number twice.
        let beside_it = monitor(&external(), 1);
        assert_eq!((beside_it.width, beside_it.height), (1920, 1080));
        assert_eq!((beside_it.x, beside_it.y), (1512, 0));
        assert!(!beside_it.primary);
    }

    /// The window-position check reads points, which is winit's space.
    #[test]
    fn an_output_is_its_display_in_points() {
        let laptop = output(&retina(), 0);
        assert_eq!((laptop.width, laptop.height), (1512, 982));
        assert_eq!((laptop.x, laptop.y), (0, 0));
        assert!(laptop.overlaps(100, 100, 800, 30));
        assert!(!laptop.overlaps(1512, 100, 800, 30));
    }

    /// A display with no mode keeps its rectangle in points rather than
    /// collapsing to nothing.
    #[test]
    fn a_display_with_no_mode_keeps_its_points_rather_than_scaling_by_zero() {
        let mut display = retina();
        display.pixel_width = 0;
        display.pixel_height = 0;
        assert!((scale(&display) - 1.0).abs() < f64::EPSILON);
        let in_points = monitor(&display, 0);
        assert_eq!((in_points.width, in_points.height), (1512, 982));
        // And one with no points either still has an area, because a
        // zero-sized render target is not something to hand a GPU.
        display.point_width = 0.0;
        display.point_height = 0.0;
        let empty = monitor(&display, 0);
        assert_eq!((empty.width, empty.height), (1, 1));
    }

    /// The label is the Windows one, plus which of the numbers is the lid.
    #[test]
    fn the_labels_are_numbered_and_the_lid_says_so() {
        assert_eq!(display_label(&retina(), 0), "Display 1 (built-in)");
        assert_eq!(display_label(&external(), 1), "Display 2");
    }

    /// The id is the UUID where there is one and `display-<id>` where there is
    /// not, and never empty: a monitor nothing can address is not one to plan
    /// around.
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

    /// The whole query: it answers with monitors that have an area, or it says
    /// it cannot ask.
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
