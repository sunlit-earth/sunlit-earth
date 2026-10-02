//! The loading line the settings window shows over the preview: the pack the
//! transcoder is preparing, and otherwise the surface the frame waits for, a
//! cube or the tiles in view.

use std::time::Duration;

use crate::assets::cube_layout::MONTHS;
use crate::assets::tiles::{PackKind, Phase, TranscodeStatus};

/// How long the tiles in view have to be on their way before the line names
/// them. A GPU reads a view's tiles in a tick or two (research section 24),
/// which a line that came and went with every move of the camera would only
/// flicker over; a software adapter, or a cold file cache, takes long enough
/// to say so.
pub const TILES_NAMED_AFTER: Duration = Duration::from_millis(500);

const MONTH_NAMES: [&str; MONTHS] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// A pack the transcoder is building, as the line names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Preparing {
    /// A month's day pack, January 0, and its place in the year's count: one
    /// more than the day packs ready or failed before it.
    Month {
        month: usize,
        place: usize,
    },
    Night,
    Mask,
}

impl Preparing {
    /// The pack `status` says is being built, if one is.
    pub(super) fn of(status: &TranscodeStatus) -> Option<Self> {
        let Phase::Building(kind) = status.phase else {
            return None;
        };
        Some(match kind {
            PackKind::Day(month) => {
                let done = status
                    .ready
                    .iter()
                    .chain(status.failed.iter().map(|failure| &failure.kind))
                    .filter(|kind| matches!(kind, PackKind::Day(_)))
                    .count();
                Self::Month {
                    month,
                    place: (done + 1).min(MONTHS),
                }
            }
            PackKind::Night => Self::Night,
            PackKind::Mask => Self::Mask,
        })
    }
}

/// The line for a transcoder preparing `preparing`, and the day and the night
/// surfaces the frame waits for; empty when nothing is.
pub(super) fn text(preparing: Option<Preparing>, day: bool, night: bool) -> String {
    match preparing {
        Some(Preparing::Month { month, place }) => {
            format!("Preparing {}, {place} of {MONTHS}", MONTH_NAMES[month])
        }
        Some(Preparing::Night) => "Preparing Night".to_owned(),
        Some(Preparing::Mask) => "Preparing Oceans".to_owned(),
        None => match (day, night) {
            (true, true) => "Loading Day and Night...".to_owned(),
            (true, false) => "Loading Day...".to_owned(),
            (false, true) => "Loading Night...".to_owned(),
            (false, false) => String::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::tiles::PackFailure;

    fn building(kind: PackKind, ready: &[PackKind], failed: &[PackKind]) -> TranscodeStatus {
        TranscodeStatus {
            phase: Phase::Building(kind),
            ready: ready.to_vec(),
            failed: failed
                .iter()
                .map(|&kind| PackFailure {
                    kind,
                    reason: "damaged".to_owned(),
                })
                .collect(),
        }
    }

    #[test]
    fn a_month_is_named_with_its_place_among_the_twelve() {
        let first = building(PackKind::Day(2), &[PackKind::Mask], &[]);
        assert_eq!(
            text(Preparing::of(&first), true, true),
            "Preparing March, 1 of 12"
        );
        let fourth = building(
            PackKind::Day(4),
            &[
                PackKind::Mask,
                PackKind::Day(2),
                PackKind::Night,
                PackKind::Day(3),
            ],
            &[PackKind::Day(1)],
        );
        assert_eq!(
            text(Preparing::of(&fourth), false, false),
            "Preparing May, 4 of 12",
            "a month that failed has had its turn"
        );
    }

    #[test]
    fn the_night_and_the_mask_are_named_without_a_count() {
        let night = building(PackKind::Night, &[PackKind::Mask, PackKind::Day(0)], &[]);
        assert_eq!(text(Preparing::of(&night), true, true), "Preparing Night");
        let mask = building(PackKind::Mask, &[], &[]);
        assert_eq!(text(Preparing::of(&mask), true, true), "Preparing Oceans");
    }

    #[test]
    fn without_a_build_the_line_names_what_the_frame_waits_for() {
        for phase in [Phase::Checking, Phase::Paused, Phase::Done] {
            let status = TranscodeStatus {
                phase,
                ready: Vec::new(),
                failed: Vec::new(),
            };
            assert_eq!(Preparing::of(&status), None);
        }
        assert_eq!(text(None, true, true), "Loading Day and Night...");
        assert_eq!(text(None, true, false), "Loading Day...");
        assert_eq!(text(None, false, true), "Loading Night...");
        assert_eq!(text(None, false, false), "");
    }
}
