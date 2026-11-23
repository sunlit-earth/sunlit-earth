//! macOS: the on-screen windows' bounds from the window server, against the
//! displays' bounds.
//!
//! There is no public interface for another application's full screen state,
//! so a layer 0 window of another process that spans a display counts. Only
//! the process ID, the layer and the bounds are read, never a name, since
//! names need the Screen Recording permission. CoreGraphics only, so the probe
//! runs on the engine thread.

use objc2_core_foundation::{
    CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGRect,
};
use objc2_core_graphics::{
    CGDisplayBounds, CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo,
    CGWindowListOption, kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer, kCGWindowNumber,
    kCGWindowOwnerPID,
};
use tracing::debug;

use super::{CoverProbe, Coverage, MacWindow, Rect, Screen, mac_coverage};

pub(super) struct MacProbe;

impl CoverProbe for MacProbe {
    fn probe(&self) -> Coverage {
        let Some(ids) = crate::display::macos::active_displays() else {
            return Coverage::Unknown("the displays could not be listed".to_owned());
        };
        let displays: Vec<Screen> = ids
            .into_iter()
            .map(|id| Screen {
                name: Some(format!("display {id}")),
                rect: rect_of(CGDisplayBounds(id)),
            })
            .collect();
        match windows() {
            Ok(windows) => mac_coverage(&windows, &displays, std::process::id()),
            Err(reason) => Coverage::Unknown(reason),
        }
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "display coordinates in points are far inside i64"
)]
fn rect_of(rect: CGRect) -> Rect {
    let left = rect.origin.x.round() as i64;
    let top = rect.origin.y.round() as i64;
    Rect {
        left,
        top,
        right: (rect.origin.x + rect.size.width).round() as i64,
        bottom: (rect.origin.y + rect.size.height).round() as i64,
    }
}

type WindowInfo = CFDictionary<CFString, CFType>;

/// Every on-screen window outside the desktop elements, front to back.
fn windows() -> Result<Vec<MacWindow>, String> {
    let list = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
    )
    .ok_or("the window server gave no window list")?;
    // SAFETY: CGWindowListCopyWindowInfo documents its result as an array of
    // dictionaries keyed by CFString.
    #[allow(unsafe_code)]
    let list: CFRetained<CFArray<WindowInfo>> = unsafe { CFRetained::cast_unchecked(list) };
    // SAFETY: the window list keys are immutable CFString constants that
    // CoreGraphics exports for the life of the process.
    #[allow(unsafe_code)]
    let (layer_key, pid_key, bounds_key, number_key, alpha_key) = unsafe {
        (
            kCGWindowLayer,
            kCGWindowOwnerPID,
            kCGWindowBounds,
            kCGWindowNumber,
            kCGWindowAlpha,
        )
    };

    let mut windows = Vec::new();
    let mut withheld = 0usize;
    for info in list.iter() {
        let number = |key: &CFString| {
            info.get(key)
                .and_then(|value| value.downcast_ref::<CFNumber>().and_then(CFNumber::as_i64))
        };
        let bounds = info.get(bounds_key).and_then(|value| {
            let dict = value.downcast_ref::<CFDictionary>()?;
            let mut rect = CGRect::default();
            // SAFETY: `dict` is the bounds dictionary the window server made
            // with CGRectCreateDictionaryRepresentation, and `rect` is a live
            // local the call writes one CGRect to.
            #[allow(unsafe_code)]
            let ok = unsafe { CGRectMakeWithDictionaryRepresentation(Some(dict), &raw mut rect) };
            ok.then_some(rect)
        });
        debug!(
            window = ?number(number_key),
            pid = ?number(pid_key),
            layer = ?number(layer_key),
            bounds = ?bounds.map(rect_of),
            alpha = ?info
                .get(alpha_key)
                .and_then(|value| value.downcast_ref::<CFNumber>().and_then(CFNumber::as_f64)),
            "an on-screen window"
        );
        match (number(layer_key), number(pid_key), bounds) {
            (Some(layer), Some(pid), Some(bounds)) => windows.push(MacWindow {
                pid: u32::try_from(pid).unwrap_or(0),
                layer,
                bounds: rect_of(bounds),
            }),
            _ => withheld += 1,
        }
    }
    if windows.is_empty() && withheld > 0 {
        return Err(format!(
            "the window server withheld the layer, process or bounds of all {withheld} windows, \
             which may need the Screen Recording permission"
        ));
    }
    Ok(windows)
}
