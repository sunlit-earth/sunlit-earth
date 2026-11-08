//! The preview's size, from the window to the engine.
//!
//! The engine draws at the size it is sent, capped in width by the quality
//! tier. A window resize or a splitter drag sends one size once the movement
//! stops. A full screen change is one jump rather than a drag, so the window
//! asks for its new size to go at once through `viewport-jumped`. Every size
//! sent also tells the window which frame size to wait for, which is what
//! ends the cover after a full screen change.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use slint::ComponentHandle;
use sunlit_core::config::QualityTier;
use sunlit_core::engine::{EngineCommand, preview_target_size};
use tracing::debug;

use crate::MainWindow;
use crate::engine_client::EngineLink;
use crate::ui_callbacks;

/// How often the app checks whether the preview viewport changed size.
const VIEWPORT_POLL: Duration = Duration::from_millis(200);

/// Which viewport sizes reach the engine: a size once two polls in a row have
/// read it, and only when it is not the one sent last. A size still changing
/// costs the engine no target rebuild until it rests.
#[derive(Debug, Default)]
struct ViewportSettle {
    previous: Option<(u32, u32)>,
    sent: Option<(u32, u32)>,
}

impl ViewportSettle {
    /// Take one poll's reading and return the size to send, if any.
    fn observe(&mut self, size: (u32, u32)) -> Option<(u32, u32)> {
        let settled = self.previous.replace(size) == Some(size);
        if !settled {
            return None;
        }
        self.take(size)
    }

    /// Take a size that is known to have stopped changing, and return it if it
    /// is to be sent: no second reading is waited for.
    fn jump(&mut self, size: (u32, u32)) -> Option<(u32, u32)> {
        self.previous = Some(size);
        self.take(size)
    }

    fn take(&mut self, size: (u32, u32)) -> Option<(u32, u32)> {
        if size.0 == 0 || size.1 == 0 || self.sent == Some(size) {
            return None;
        }
        self.sent = Some(size);
        Some(size)
    }
}

/// Send the preview's size to the engine as it changes, and tell the window
/// the frame size each one comes back as.
///
/// Slint has no size-changed callback that Rust can bind, and polling a pair
/// of properties every 200 ms is far cheaper than any alternative. The poll runs
/// for as long as the timer returned is kept, and reads nothing while a
/// full screen switch has the window blank.
///
/// :param window: the main window
/// :param link: the engine the sizes go to
/// :param quality: the quality tier the engine runs at, whose width cap
///     decides the size of the frames that come back
pub fn wire(window: &MainWindow, link: &EngineLink, quality: QualityTier) -> slint::Timer {
    let settle = Rc::new(RefCell::new(ViewportSettle::default()));
    let report = {
        let engine = link.clone();
        move |win: &MainWindow, size: Option<(u32, u32)>, known: (u32, u32)| {
            if let Some((width, height)) = size {
                engine.send(EngineCommand::SetPreviewSize(width, height));
            }
            if known.0 > 0 && known.1 > 0 {
                let (width, height) = preview_target_size(known, quality);
                let int = |value: u32| i32::try_from(value).unwrap_or(i32::MAX);
                win.invoke_frame_awaited(int(width), int(height));
            }
        }
    };
    let report = Rc::new(report);

    let timer = slint::Timer::default();
    let window_weak = window.as_weak();
    let (poll_settle, poll_report) = (Rc::clone(&settle), Rc::clone(&report));
    timer.start(slint::TimerMode::Repeated, VIEWPORT_POLL, move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        // Under a switch's blank the preview passes through sizes that are
        // neither the old one nor the new; its size goes when the blank lifts.
        if win.get_switch_blank() {
            return;
        }
        let size = size_of(&win);
        if let Some(sent) = poll_settle.borrow_mut().observe(size) {
            poll_report(&win, Some(sent), sent);
        }
    });

    let window_weak = window.as_weak();
    window.on_viewport_jumped(move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let size = size_of(&win);
        debug!(?size, "the preview's size jumped");
        let sent = settle.borrow_mut().jump(size);
        // Also when the engine has the size already: the window has forgotten
        // which frame it waits for, and that frame is the one drawn for it.
        report(&win, sent, size);
    });
    timer
}

/// The preview's size in physical pixels.
fn size_of(window: &MainWindow) -> (u32, u32) {
    let scale = window.window().scale_factor();
    (
        ui_callbacks::slider_u32(window.get_viewport_width() * scale),
        ui_callbacks::slider_u32(window.get_viewport_height() * scale),
    )
}

#[cfg(test)]
mod tests {
    use super::ViewportSettle;

    fn sent(readings: &[(u32, u32)]) -> Vec<(u32, u32)> {
        let mut settle = ViewportSettle::default();
        readings
            .iter()
            .filter_map(|&size| settle.observe(size))
            .collect()
    }

    #[test]
    fn a_steady_size_is_sent_once() {
        assert_eq!(sent(&[(800, 600); 6]), [(800, 600)]);
    }

    #[test]
    fn a_size_that_changes_on_every_poll_is_never_sent() {
        let moving: Vec<_> = (0..20).map(|i| (800 + i * 7, 600)).collect();
        assert_eq!(sent(&moving), []);
    }

    #[test]
    fn a_size_that_changes_then_holds_for_one_poll_is_sent_once() {
        assert_eq!(
            sent(&[
                (800, 600),
                (800, 600),
                (811, 600),
                (823, 607),
                (840, 610),
                (840, 610),
                (840, 610),
            ]),
            [(800, 600), (840, 610)]
        );
    }

    #[test]
    fn returning_to_the_size_sent_last_sends_nothing() {
        assert_eq!(
            sent(&[(800, 600), (800, 600), (900, 600), (800, 600), (800, 600)]),
            [(800, 600)]
        );
    }

    #[test]
    fn a_zero_size_is_never_sent() {
        assert_eq!(sent(&[(0, 0); 3]), []);
        assert_eq!(sent(&[(0, 600); 3]), []);
        assert_eq!(sent(&[(800, 0); 3]), []);
        assert_eq!(
            sent(&[
                (800, 600),
                (800, 600),
                (0, 0),
                (0, 0),
                (800, 600),
                (800, 600)
            ]),
            [(800, 600)]
        );
        let mut settle = ViewportSettle::default();
        assert_eq!(settle.jump((0, 1080)), None);
    }

    #[test]
    fn a_jump_is_sent_without_waiting_for_a_second_poll() {
        let mut settle = ViewportSettle::default();
        assert_eq!(settle.observe((800, 600)), None);
        assert_eq!(settle.observe((800, 600)), Some((800, 600)));
        assert_eq!(settle.jump((1920, 1080)), Some((1920, 1080)), "at once");
        assert_eq!(
            settle.observe((1920, 1080)),
            None,
            "and the poll that reads it next does not send it again"
        );
        assert_eq!(
            settle.jump((1920, 1080)),
            None,
            "nor does a second jump to it"
        );
        assert_eq!(settle.jump((800, 600)), Some((800, 600)), "and back");
    }
}
