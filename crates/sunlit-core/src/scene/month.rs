//! Which month's surface the globe is drawn with.
//!
//! Each of the twelve day surfaces stands for its whole month. The month in
//! force is the one whose first day is nearest the date, the convention of the
//! Blue Marble layer in NASA's Web World Wind, so the surface changes at the
//! middle of a month rather than at its start: from the 16th of a 31 day month
//! at noon, the 16th of a 30 day month at midnight, and the 15th of February, at
//! noon in a leap year. The date is the one the Sun is computed for, the custom
//! date or the live clock, through [`CivilTime`].

use super::datetime::{self, hour_float_to_hms};
use super::sun::DateTimeInput;

const SECONDS_PER_DAY: f64 = 86_400.0;

/// How near a hand-over the date has to be for the month across it to be read
/// ahead, in seconds: a day.
///
/// The custom date moves only when a slider does, by a day a step of the day
/// slider and by less than a day along the hour slider, so within a day of a
/// hand-over the next step of either can cross it, in either direction. The
/// live clock moves at a second a second and only forward, and a day is far
/// more than it needs: the engine draws at least every two minutes while the
/// date is live, and the tiles of a view take seconds at the most to be read.
pub const PREFETCH_WINDOW: f64 = SECONDS_PER_DAY;

/// A calendar date and time of day in UTC, as the Sun and the month both read
/// the scene's date.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CivilTime {
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// 1 to 31.
    pub day: u8,
    pub hour: i32,
    pub minute: i32,
    pub second: f64,
}

impl CivilTime {
    /// The instant `dt` names: the custom date when it is in use, the live
    /// clock's reading otherwise.
    pub(crate) fn of(dt: &DateTimeInput, now_utc: time::OffsetDateTime) -> Self {
        if dt.use_custom {
            let doy = dt.custom_day_of_year.max(1);
            let (month, day) = datetime::day_of_year_to_month_day(doy, dt.custom_year);
            let (hour, minute, second) = hour_float_to_hms(dt.custom_hour);
            Self {
                year: dt.custom_year,
                month,
                day,
                hour,
                minute,
                second,
            }
        } else {
            Self {
                year: now_utc.year(),
                month: u8::from(now_utc.month()),
                day: now_utc.day(),
                hour: i32::from(now_utc.hour()),
                minute: i32::from(now_utc.minute()),
                second: f64::from(now_utc.second()),
            }
        }
    }
}

/// The days in `month` (1 to 12) of `year`.
fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        2 if datetime::is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Half the length of calendar month `month` (1 to 12) of `year`, in seconds,
/// which is where its hand-over falls.
fn half_of(year: i32, month: u8) -> f64 {
    f64::from(days_in_month(year, month)) * SECONDS_PER_DAY / 2.0
}

/// The hand-overs either side of `at`: how long ago the last was and the month
/// in force before it, and how long until the next and the month in force
/// after it, each January 0.
struct HandOvers {
    since: f64,
    before: usize,
    until: f64,
    after: usize,
}

impl HandOvers {
    fn around(at: &CivilTime) -> Self {
        let month = at.month.clamp(1, 12);
        let into_month = f64::from(at.day.max(1) - 1) * SECONDS_PER_DAY
            + f64::from(at.hour) * 3600.0
            + f64::from(at.minute) * 60.0
            + at.second;
        let length = f64::from(days_in_month(at.year, month)) * SECONDS_PER_DAY;
        let half = length / 2.0;
        let (previous_year, previous) = if month == 1 {
            (at.year - 1, 12)
        } else {
            (at.year, month - 1)
        };
        let (next_year, next) = if month == 12 {
            (at.year + 1, 1)
        } else {
            (at.year, month + 1)
        };
        let this = usize::from(month - 1);
        if into_month < half {
            Self {
                since: into_month + half_of(previous_year, previous),
                before: (this + 11) % 12,
                until: half - into_month,
                after: (this + 1) % 12,
            }
        } else {
            Self {
                since: into_month - half,
                before: this,
                until: length - into_month + half_of(next_year, next),
                after: (this + 2) % 12,
            }
        }
    }

    /// The month in force between the two.
    fn in_force(&self) -> usize {
        (self.before + 1) % 12
    }
}

/// The month in force at `at`, January 0.
///
/// The first half of a month is its own and the second half the next one's;
/// the instant halfway through belongs to the next month, and the second half
/// of December is January's.
#[must_use]
pub(crate) fn month_index(at: &CivilTime) -> usize {
    HandOvers::around(at).in_force()
}

/// The month across the hand-over within [`PREFETCH_WINDOW`] of `at`, if one
/// is: the next hand-over's, and with `both_ways` the last one's as well,
/// which can be no nearer than a fortnight to the next.
#[must_use]
pub(crate) fn month_across(at: &CivilTime, both_ways: bool) -> Option<usize> {
    let hand_overs = HandOvers::around(at);
    if hand_overs.until <= PREFETCH_WINDOW {
        Some(hand_overs.after)
    } else if both_ways && hand_overs.since <= PREFETCH_WINDOW {
        Some(hand_overs.before)
    } else {
        None
    }
}

/// The month in force for the scene's date, January 0.
#[must_use]
pub fn month_in_force(dt: &DateTimeInput, now_utc: time::OffsetDateTime) -> usize {
    month_index(&CivilTime::of(dt, now_utc))
}

/// The month whose tiles are read ahead for the scene's date, January 0: the
/// one across a hand-over within [`PREFETCH_WINDOW`], ahead of the live clock,
/// which only goes forward, and either side of the custom date, which a slider
/// moves both ways.
#[must_use]
pub fn month_ahead(dt: &DateTimeInput, now_utc: time::OffsetDateTime) -> Option<usize> {
    month_across(&CivilTime::of(dt, now_utc), dt.use_custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(year: i32, month: u8, day: u8, hour: i32, minute: i32, second: f64) -> CivilTime {
        CivilTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
        }
    }

    /// The last second before the middle of every month is the month itself,
    /// and the middle is the next one, around the whole year.
    #[test]
    fn every_month_hands_over_at_its_middle() {
        for year in [2025, 2026] {
            for month in 1..=12_u8 {
                let length = days_in_month(year, month);
                // Halfway through, in whole hours: a 31 day month is halfway
                // at noon on the 16th, a 30 day one at midnight into the 16th.
                let half_hours = i32::from(length) * 12;
                let day = u8::try_from(half_hours / 24 + 1).expect("a day of the month");
                let hour = half_hours % 24;
                let this = usize::from(month - 1);
                let next = (this + 1) % 12;

                let before = if hour == 0 {
                    at(year, month, day - 1, 23, 59, 59.0)
                } else {
                    at(year, month, day, hour - 1, 59, 59.0)
                };
                assert_eq!(month_index(&before), this, "{year}-{month:02}: just before");
                assert_eq!(
                    month_index(&at(year, month, day, hour, 0, 0.0)),
                    next,
                    "{year}-{month:02}: at the middle"
                );
            }
        }
    }

    #[test]
    fn the_first_and_the_last_day_belong_to_different_surfaces() {
        assert_eq!(month_index(&at(2026, 3, 1, 0, 0, 0.0)), 2);
        assert_eq!(month_index(&at(2026, 3, 31, 23, 59, 59.0)), 3);
    }

    /// The second half of December is January's, which is the one hand-over
    /// that crosses a year.
    #[test]
    fn the_year_wraps_at_the_middle_of_december() {
        assert_eq!(month_index(&at(2026, 12, 16, 11, 59, 59.0)), 11);
        assert_eq!(month_index(&at(2026, 12, 16, 12, 0, 0.0)), 0);
        assert_eq!(month_index(&at(2026, 12, 31, 23, 59, 59.0)), 0);
        assert_eq!(month_index(&at(2027, 1, 1, 0, 0, 0.0)), 0);
        assert_eq!(month_index(&at(2027, 1, 16, 11, 59, 59.0)), 0);
        assert_eq!(month_index(&at(2027, 1, 16, 12, 0, 0.0)), 1);
    }

    /// February's middle moves by half a day in a leap year.
    #[test]
    fn a_leap_year_moves_the_middle_of_february() {
        assert_eq!(month_index(&at(2026, 2, 14, 23, 59, 59.0)), 1);
        assert_eq!(month_index(&at(2026, 2, 15, 0, 0, 0.0)), 2);
        assert_eq!(month_index(&at(2028, 2, 15, 11, 59, 59.0)), 1);
        assert_eq!(month_index(&at(2028, 2, 15, 12, 0, 0.0)), 2);
    }

    fn civil(t: time::OffsetDateTime) -> CivilTime {
        at(
            t.year(),
            u8::from(t.month()),
            t.day(),
            i32::from(t.hour()),
            i32::from(t.minute()),
            f64::from(t.second()),
        )
    }

    /// The instant calendar month `month` of `year` hands over to the next
    /// month in force.
    fn hand_over(year: i32, month: u8) -> time::OffsetDateTime {
        let half_hours = i32::from(days_in_month(year, month)) * 12;
        let day = u8::try_from(half_hours / 24 + 1).expect("a day of the month");
        let hour = u8::try_from(half_hours % 24).expect("an hour");
        time::Month::try_from(month)
            .ok()
            .and_then(|m| time::Date::from_calendar_date(year, m, day).ok())
            .and_then(|date| date.with_hms(hour, 0, 0).ok())
            .expect("a date")
            .assume_utc()
    }

    /// Within a day of a hand-over the month across it is read ahead: the
    /// next one's for either clock, the last one's for the custom date alone,
    /// around the whole year and across its end.
    #[test]
    fn the_month_across_a_hand_over_within_a_day_is_read_ahead() {
        let hours = time::Duration::hours;
        for year in [2026, 2028] {
            for month in 1..=12_u8 {
                let at_hand_over = hand_over(year, month);
                let this = usize::from(month - 1);
                let next = (this + 1) % 12;
                let label = format!("{year}-{month:02}");

                let before = civil(at_hand_over - hours(1));
                assert_eq!(month_index(&before), this, "{label}");
                assert_eq!(month_across(&before, false), Some(next), "{label}");
                assert_eq!(month_across(&before, true), Some(next), "{label}");
                let a_day_before = civil(at_hand_over - hours(24));
                assert_eq!(month_across(&a_day_before, true), Some(next), "{label}");
                let further = civil(at_hand_over - hours(25));
                assert_eq!(month_across(&further, true), None, "{label}");

                let after = civil(at_hand_over + hours(1));
                assert_eq!(month_index(&after), next, "{label}");
                assert_eq!(month_across(&after, false), None, "{label}: the live clock");
                assert_eq!(month_across(&after, true), Some(this), "{label}");
                let a_day_after = civil(at_hand_over + hours(24));
                assert_eq!(month_across(&a_day_after, true), Some(this), "{label}");
                let further = civil(at_hand_over + hours(25));
                assert_eq!(month_across(&further, true), None, "{label}");
            }
        }
    }

    /// The live clock reads ahead only forward, the custom date both ways.
    #[test]
    fn the_scene_date_names_the_month_read_ahead() {
        let after = hand_over(2026, 9) + time::Duration::hours(3);
        let live = DateTimeInput {
            use_custom: false,
            custom_hour: 12.0,
            custom_day_of_year: 1,
            custom_year: 2026,
        };
        assert_eq!(month_ahead(&live, after), None);
        assert_eq!(
            month_ahead(&live, hand_over(2026, 9) - time::Duration::hours(3)),
            Some(9)
        );

        // September 16th at 03:00, three hours past the hand-over at
        // midnight into the 16th of September's 30 days.
        let custom = DateTimeInput {
            use_custom: true,
            custom_hour: 3.0,
            custom_day_of_year: 259,
            custom_year: 2026,
        };
        assert_eq!(month_in_force(&custom, after), 9, "October's");
        assert_eq!(month_ahead(&custom, after), Some(8), "September across it");
    }

    /// The custom date and the live clock are read the way the Sun reads them.
    #[test]
    fn the_scene_date_names_the_month() {
        let custom = DateTimeInput {
            use_custom: true,
            custom_hour: 12.0,
            custom_day_of_year: 160,
            custom_year: 2026,
        };
        let never = time::OffsetDateTime::UNIX_EPOCH;
        assert_eq!(month_in_force(&custom, never), 5, "June 9 is June's");

        let solstice = DateTimeInput {
            custom_day_of_year: 172,
            ..custom
        };
        assert_eq!(month_in_force(&solstice, never), 6, "June 21 is July's");

        let live = DateTimeInput {
            use_custom: false,
            ..custom
        };
        let now = time::Date::from_calendar_date(2026, time::Month::September, 30)
            .and_then(|date| date.with_hms(10, 0, 0))
            .expect("a date")
            .assume_utc();
        assert_eq!(
            month_in_force(&live, now),
            9,
            "the end of September is October's"
        );
    }
}
