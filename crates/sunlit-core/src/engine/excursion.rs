//! Playback: the preview leaving the time setting for a while.
//!
//! An excursion starts at an instant and runs at a rate on the engine's own
//! clock, or rests paused. The preview draws at its instant while there is
//! one; every export, the wallpaper included, draws at the time setting, which
//! an excursion never changes. Nothing here is saved.

use std::time::Duration;

use time::OffsetDateTime;

use crate::scene::scene_time::{EARLIEST, LATEST, clamp_instant};

/// What play runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExcursionKind {
    /// The clock, at a multiple of real time.
    #[default]
    Clock,
    /// The date, at whole days a second, the time of day kept.
    Date,
}

impl ExcursionKind {
    /// Every rate of this kind, slowest first: multiples of real time for the
    /// clock, from real time to 6 hours a second, and days a second for the
    /// date, from a day to 3 months.
    #[must_use]
    pub fn ladder(self) -> &'static [u32] {
        match self {
            Self::Clock => &CLOCK_RATES,
            Self::Date => &DATE_RATES,
        }
    }

    /// The rate a session starts at: a minute a second, or a week a second.
    #[must_use]
    pub fn default_rate(self) -> u32 {
        match self {
            Self::Clock => 60,
            Self::Date => 7,
        }
    }

    /// How far a skip moves: 15 minutes or an hour, a day or a week.
    #[must_use]
    pub fn skip(self, big: bool) -> time::Duration {
        match (self, big) {
            (Self::Clock, false) => time::Duration::minutes(15),
            (Self::Clock, true) => time::Duration::hours(1),
            (Self::Date, false) => time::Duration::days(1),
            (Self::Date, true) => time::Duration::weeks(1),
        }
    }
}

/// The clock's rates in multiples of real time.
pub const CLOCK_RATES: [u32; 12] = [1, 5, 10, 30, 60, 120, 300, 600, 1800, 3600, 7200, 21_600];

/// The date's rates in days a second.
pub const DATE_RATES: [u32; 7] = [1, 2, 7, 14, 30, 60, 90];

/// One skip of the transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeStep {
    /// Whose sizes the skip takes.
    pub kind: ExcursionKind,
    pub forward: bool,
    /// The bigger of the two sizes, which Shift asks for.
    pub big: bool,
}

impl TimeStep {
    fn duration(self) -> time::Duration {
        let size = self.kind.skip(self.big);
        if self.forward { size } else { -size }
    }
}

/// What a client asks of the excursion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExcursionCommand {
    /// Run at `rate`, signed, of `kind`: from the instant the preview shows
    /// when there is an excursion, from the time setting otherwise.
    Play { rate: i32, kind: ExcursionKind },
    /// Stop at the instant reached.
    Pause,
    /// Move the excursion, running or paused, or start a paused one at the
    /// time setting moved by the step.
    Skip(TimeStep),
    /// Go back to the time setting.
    End,
}

/// What the engine says about the excursion, whenever it changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExcursionState {
    pub running: bool,
    /// Signed: multiples of real time for the clock, days a second for the
    /// date.
    pub rate: i32,
    pub kind: ExcursionKind,
}

/// The preview's own time while it is away from the setting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Excursion {
    /// The instant at `since`.
    anchor: OffsetDateTime,
    /// The engine clock's reading at `anchor`.
    since: Duration,
    rate: i32,
    kind: ExcursionKind,
    running: bool,
}

impl Excursion {
    /// The instant at `elapsed` on the engine's clock, within the range the
    /// scene accepts.
    #[must_use]
    pub fn instant_at(&self, elapsed: Duration) -> OffsetDateTime {
        if !self.running {
            return self.anchor;
        }
        let seconds = elapsed.saturating_sub(self.since).as_secs_f64();
        let moved = f64::from(self.rate) * seconds;
        let target = match self.kind {
            ExcursionKind::Clock => self.anchor.checked_add(time::Duration::seconds_f64(moved)),
            #[expect(
                clippy::cast_possible_truncation,
                reason = "whole days, far inside an i64 once clamped"
            )]
            ExcursionKind::Date => self
                .anchor
                .checked_add(time::Duration::days(moved.trunc().clamp(-1e7, 1e7) as i64)),
        };
        target.map_or(if moved < 0.0 { EARLIEST } else { LATEST }, clamp_instant)
    }

    #[must_use]
    pub fn state(&self) -> ExcursionState {
        ExcursionState {
            running: self.running,
            rate: self.rate,
            kind: self.kind,
        }
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Pause a running excursion that reached the first or the last instant
    /// the scene accepts. Returns whether it paused.
    pub fn stop_at_the_edge(&mut self, elapsed: Duration) -> bool {
        if !self.running {
            return false;
        }
        let at = self.instant_at(elapsed);
        let ahead = self.rate > 0;
        if (ahead && at >= LATEST) || (!ahead && at <= EARLIEST) {
            self.anchor = at;
            self.since = elapsed;
            self.running = false;
            return true;
        }
        false
    }

    /// Stop at the instant reached.
    pub fn pause(&mut self, elapsed: Duration) {
        self.anchor = self.instant_at(elapsed);
        self.since = elapsed;
        self.running = false;
    }
}

/// The excursion after `command`, at `elapsed` on the engine's clock, with
/// the time setting at `setting`.
#[must_use]
pub fn apply(
    current: Option<Excursion>,
    command: ExcursionCommand,
    elapsed: Duration,
    setting: OffsetDateTime,
) -> Option<Excursion> {
    let here = current.map_or(setting, |excursion| excursion.instant_at(elapsed));
    match command {
        ExcursionCommand::Play { rate, kind } => Some(Excursion {
            anchor: clamp_instant(here),
            since: elapsed,
            rate,
            kind,
            running: rate != 0,
        }),
        ExcursionCommand::Pause => current.map(|mut excursion| {
            excursion.pause(elapsed);
            excursion
        }),
        ExcursionCommand::Skip(step) => {
            let anchor = here
                .checked_add(step.duration())
                .map_or(if step.forward { LATEST } else { EARLIEST }, clamp_instant);
            Some(match current {
                Some(excursion) => Excursion {
                    anchor,
                    since: elapsed,
                    ..excursion
                },
                None => Excursion {
                    anchor,
                    since: elapsed,
                    rate: i32::try_from(step.kind.default_rate()).unwrap_or(1),
                    kind: step.kind,
                    running: false,
                },
            })
        }
        ExcursionCommand::End => None,
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    const START: OffsetDateTime = datetime!(2026-10-18 14:00 UTC);

    fn play(rate: i32, kind: ExcursionKind) -> Excursion {
        apply(
            None,
            ExcursionCommand::Play { rate, kind },
            Duration::from_secs(10),
            START,
        )
        .expect("an excursion")
    }

    fn secs(seconds: f64) -> Duration {
        Duration::from_secs_f64(seconds)
    }

    #[test]
    fn a_clock_run_moves_by_its_rate_either_way() {
        let forward = play(3600, ExcursionKind::Clock);
        assert_eq!(
            forward.instant_at(secs(11.0)),
            datetime!(2026-10-18 15:00 UTC)
        );
        assert_eq!(
            forward.instant_at(secs(10.5)),
            datetime!(2026-10-18 14:30 UTC)
        );
        let backward = play(-60, ExcursionKind::Clock);
        assert_eq!(
            backward.instant_at(secs(12.0)),
            datetime!(2026-10-18 13:58 UTC)
        );
    }

    /// A date run moves in whole days, so the time of day of UTC stays.
    #[test]
    fn a_date_run_keeps_the_time_of_day() {
        let forward = play(7, ExcursionKind::Date);
        assert_eq!(forward.instant_at(secs(10.1)), START);
        assert_eq!(
            forward.instant_at(secs(11.0)),
            datetime!(2026-10-25 14:00 UTC)
        );
        assert_eq!(
            forward.instant_at(secs(10.5)),
            datetime!(2026-10-21 14:00 UTC)
        );
        let backward = play(-30, ExcursionKind::Date);
        assert_eq!(
            backward.instant_at(secs(11.0)),
            datetime!(2026-09-18 14:00 UTC)
        );
    }

    /// Retuning starts the new rate from the instant reached, so the time
    /// does not jump.
    #[test]
    fn a_retune_keeps_the_instant_continuous() {
        let first = play(3600, ExcursionKind::Clock);
        let at = secs(12.0);
        let reached = first.instant_at(at);
        let retuned = apply(
            Some(first),
            ExcursionCommand::Play {
                rate: -7200,
                kind: ExcursionKind::Clock,
            },
            at,
            START,
        )
        .expect("still an excursion");
        assert_eq!(retuned.instant_at(at), reached);
        assert_eq!(
            retuned.instant_at(secs(13.0)),
            reached - time::Duration::hours(2)
        );
    }

    #[test]
    fn pause_freezes_the_instant_reached() {
        let running = play(3600, ExcursionKind::Clock);
        let paused = apply(Some(running), ExcursionCommand::Pause, secs(11.0), START)
            .expect("still an excursion");
        assert!(!paused.is_running());
        assert_eq!(
            paused.instant_at(secs(500.0)),
            datetime!(2026-10-18 15:00 UTC)
        );
        assert_eq!(
            apply(None, ExcursionCommand::Pause, secs(11.0), START),
            None,
            "pause without an excursion starts none"
        );
    }

    #[test]
    fn a_skip_moves_an_excursion_or_starts_a_paused_one_at_the_setting() {
        let step = |kind, forward, big| ExcursionCommand::Skip(TimeStep { kind, forward, big });
        let started = apply(
            None,
            step(ExcursionKind::Clock, true, false),
            secs(1.0),
            START,
        )
        .expect("a skip starts an excursion");
        assert!(!started.is_running());
        assert_eq!(
            started.instant_at(secs(9.0)),
            datetime!(2026-10-18 14:15 UTC)
        );

        let running = play(60, ExcursionKind::Clock);
        let skipped = apply(
            Some(running),
            step(ExcursionKind::Date, false, true),
            secs(11.0),
            START,
        )
        .expect("still an excursion");
        assert!(
            skipped.is_running(),
            "a skip keeps a running excursion running"
        );
        assert_eq!(
            skipped.instant_at(secs(11.0)),
            datetime!(2026-10-11 14:01 UTC)
        );
        assert_eq!(
            apply(Some(skipped), ExcursionCommand::End, secs(12.0), START),
            None
        );
    }

    /// A run that reaches 1900 or 2100 stops there and pauses.
    #[test]
    fn a_run_pauses_at_the_edges_of_the_range() {
        let near_end = apply(
            None,
            ExcursionCommand::Play {
                rate: 90,
                kind: ExcursionKind::Date,
            },
            Duration::ZERO,
            datetime!(2100-12-01 00:00 UTC),
        )
        .expect("an excursion");
        let mut at_end = near_end;
        assert!(!at_end.stop_at_the_edge(secs(0.2)));
        assert!(at_end.stop_at_the_edge(secs(1.0)));
        assert!(!at_end.is_running());
        assert_eq!(at_end.instant_at(secs(5.0)), LATEST);

        let mut at_start = apply(
            None,
            ExcursionCommand::Play {
                rate: -21_600,
                kind: ExcursionKind::Clock,
            },
            Duration::ZERO,
            datetime!(1900-01-01 01:00 UTC),
        )
        .expect("an excursion");
        assert!(at_start.stop_at_the_edge(secs(1.0)));
        assert_eq!(at_start.instant_at(secs(9.0)), EARLIEST);
    }

    #[test]
    fn every_ladder_rate_is_distinct_and_sorted_and_holds_its_default() {
        for kind in [ExcursionKind::Clock, ExcursionKind::Date] {
            let ladder = kind.ladder();
            assert!(ladder.windows(2).all(|pair| pair[0] < pair[1]), "{kind:?}");
            assert!(ladder.contains(&kind.default_rate()), "{kind:?}");
        }
    }
}
