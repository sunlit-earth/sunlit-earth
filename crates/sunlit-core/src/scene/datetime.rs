/// Return the base year for the year `ComboBox` (current year - 10).
pub fn base_year() -> i32 {
    time::OffsetDateTime::now_utc().year() - 10
}

/// Return the (start, end) year range for the year `ComboBox`.
///
/// The range is current year +/- 10, yielding 21 entries.
pub fn year_range() -> (i32, i32) {
    let current = time::OffsetDateTime::now_utc().year();
    (current - 10, current + 10)
}

/// Check whether a given year is a leap year.
///
/// A year is a leap year if it is divisible by 4, except for century
/// years which must also be divisible by 400.
pub(crate) fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Return the number of days in the given year (365 or 366).
pub fn days_in_year(year: i32) -> u16 {
    if is_leap_year(year) { 366 } else { 365 }
}

/// Cumulative days before each month (non-leap year).
/// Index 0 = before January (0 days), index 1 = before February (31), etc.
const CUMULATIVE_DAYS: [u16; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];

/// Convert a day-of-year (1-based) and year to a (month, day) pair.
///
/// :param doy: Day of the year, 1 through `days_in_year(year)`.
/// :param year: Calendar year (used for leap year detection).
/// :returns: `(month, day)` where month is 1-12 and day is 1-31.
///
/// Clamps `doy` to the valid range for the year.
#[allow(clippy::cast_possible_truncation)]
pub fn day_of_year_to_month_day(doy: u16, year: i32) -> (u8, u8) {
    let max_doy = days_in_year(year);
    let doy = doy.clamp(1, max_doy);

    let leap = is_leap_year(year);
    // The leap day pushes March and everything after it one day later in the
    // year, which is one more day before each of those months.
    for month_idx in (0..12).rev() {
        let mut cum = CUMULATIVE_DAYS[month_idx];
        if leap && month_idx >= 2 {
            cum += 1;
        }
        if doy > cum {
            return ((month_idx + 1) as u8, (doy - cum) as u8);
        }
    }
    // Fallback (should never happen with clamped doy >= 1)
    (1, 1)
}

/// Abbreviated month names for display labels.
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Format a day-of-year as a "Mon DD" label (e.g. "Mar 17").
///
/// :param doy: Day of the year, 1-based.
/// :param year: Calendar year (used for leap year detection).
/// :returns: Formatted string like "Jan 1", "Feb 29", "Dec 31".
pub fn month_day_label(doy: u16, year: i32) -> String {
    let (month, day) = day_of_year_to_month_day(doy, year);
    let name = MONTH_NAMES[(month - 1) as usize];
    format!("{name} {day}")
}

/// Decompose a fractional hour (0.0 .. 24.0) into (hour, minute).
///
/// :param h: Fractional hour, e.g. 14.5 means 14:30.
/// :returns: `(hour, minute)` where hour is 0-23 and minute is 0-59.
///
/// Clamps to \[0.0, 24.0). A value of exactly 24.0 maps to (23, 59).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn hour_float_to_hm(h: f32) -> (u8, u8) {
    let h = h.clamp(0.0, 24.0);
    if h >= 24.0 {
        return (23, 59);
    }
    let hour = h as u8;
    let minute = ((h - f32::from(hour)) * 60.0) as u8;
    (hour, minute.min(59))
}

/// Format a fractional hour as an "HH:MM" label (e.g. "14:30").
///
/// :param h: Fractional hour, 0.0 to 24.0.
/// :returns: Formatted string like "00:00", "09:30", "23:59".
pub fn hour_label(h: f32) -> String {
    let (hour, minute) = hour_float_to_hm(h);
    format!("{hour:02}:{minute:02}")
}

/// Decompose a fractional hour into (hour, minute, second) with types
/// matching the Astronomy Engine FFI (`i32`, `i32`, `f64`).
///
/// Unlike `hour_float_to_hm`, this preserves fractional seconds for
/// maximum precision when computing sun positions.
///
/// :param h: Fractional hour, 0.0 to 24.0.
/// :returns: `(hour, minute, second)` where hour is 0-23, minute is 0-59,
///           and second is 0.0..60.0.
///
/// Clamps to \[0.0, 24.0). A value of exactly 24.0 maps to (23, 59, 59.0)
/// approximately.
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn hour_float_to_hms(h: f32) -> (i32, i32, f64) {
    let h = f64::from(h.clamp(0.0, 24.0));
    if h >= 24.0 {
        return (23, 59, 59.0);
    }
    let hour = h as i32;
    let rem = (h - f64::from(hour)) * 60.0;
    let minute = rem as i32;
    let second = (rem - f64::from(minute)) * 60.0;
    (hour, minute.min(59), second.min(59.999_999))
}

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;

    use super::*;

    #[test]
    fn the_year_combo_box_is_centred_on_the_current_year() {
        let current = time::OffsetDateTime::now_utc().year();
        let (start, end) = year_range();
        assert_eq!(start, current - 10);
        assert_eq!(end, current + 10);
        assert_eq!(base_year(), start);
    }

    #[test]
    fn leap_years_are_the_ones_the_calendar_says() {
        for (year, leap) in [
            (2024, true),
            (2025, false),
            (1900, false),
            (2000, true),
            (2100, false),
        ] {
            assert_eq!(is_leap_year(year), leap, "year {year}");
        }
    }

    #[test]
    fn the_day_of_year_lands_on_the_right_month_and_day() {
        for (doy, year, expected) in [
            (1, 2025, (1, 1)),
            (31, 2025, (1, 31)),
            (32, 2025, (2, 1)),
            (59, 2025, (2, 28)),
            // The leap day is where the two years part company.
            (60, 2025, (3, 1)),
            (60, 2024, (2, 29)),
            (61, 2024, (3, 1)),
            (182, 2025, (7, 1)),
            (365, 2025, (12, 31)),
            (366, 2024, (12, 31)),
        ] {
            assert_eq!(
                day_of_year_to_month_day(doy, year),
                expected,
                "day {doy} of {year}"
            );
        }
    }

    #[test]
    fn the_label_names_the_month_and_drops_the_leading_zero() {
        for (doy, year, expected) in [
            (1, 2025, "Jan 1"),
            (76, 2025, "Mar 17"),
            (60, 2024, "Feb 29"),
            (365, 2025, "Dec 31"),
        ] {
            assert_eq!(month_day_label(doy, year), expected, "day {doy} of {year}");
        }
    }

    #[test]
    fn a_fractional_hour_splits_into_hours_and_minutes() {
        for (hour, expected) in [
            (0.0, (0, 0)),
            (1.0 / 60.0, (0, 1)),
            (12.0, (12, 0)),
            (14.5, (14, 30)),
            (14.75, (14, 45)),
            (23.99, (23, 59)),
            // The top of the range is a minute short of the next day rather
            // than the next day's zero.
            (24.0, (23, 59)),
        ] {
            assert_eq!(hour_float_to_hm(hour), expected, "hour {hour}");
        }
    }

    #[test]
    fn the_hour_label_pads_both_halves_to_two_digits() {
        for (hour, expected) in [
            (0.0, "00:00"),
            (9.5, "09:30"),
            (14.75, "14:45"),
            (23.99, "23:59"),
        ] {
            assert_eq!(hour_label(hour), expected, "hour {hour}");
        }
    }

    #[test]
    fn a_fractional_hour_keeps_its_seconds_for_the_ephemeris() {
        for (hour, expected) in [
            (0.0, (0, 0, 0.0)),
            (14.5, (14, 30, 0.0)),
            (14.75, (14, 45, 0.0)),
            (12.5025, (12, 30, 9.0)),
            (24.0, (23, 59, 59.0)),
        ] {
            let (h, m, s) = hour_float_to_hms(hour);
            assert_eq!((h, m), (expected.0, expected.1), "hour {hour}");
            assert_relative_eq!(s, expected.2, epsilon = 1.0);
        }
    }

    // --- proptests ---

    mod proptests {
        use proptest::prelude::*;

        use super::super::*;

        proptest! {
            #[test]
            fn days_always_365_or_366(y in 1i32..=9999) {
                let d = days_in_year(y);
                prop_assert!(d == 365 || d == 366);
            }

            #[test]
            fn days_366_iff_leap(y in 1i32..=9999) {
                prop_assert_eq!(days_in_year(y) == 366, is_leap_year(y));
            }

            #[test]
            fn hms_in_range(h in 0.0f32..24.0) {
                let (hour, minute, second) = hour_float_to_hms(h);
                prop_assert!((0..24).contains(&hour), "hour out of range: {hour}");
                prop_assert!((0..60).contains(&minute), "minute out of range: {minute}");
                prop_assert!((0.0..60.0).contains(&second), "second out of range: {second}");
            }

            #[test]
            fn hm_hour_and_minute_in_range(h in 0.0f32..24.0) {
                let (hour, minute) = hour_float_to_hm(h);
                prop_assert!(hour < 24, "hour out of range: {hour}");
                prop_assert!(minute < 60, "minute out of range: {minute}");
            }

            #[test]
            fn doy_day_within_month(y in 1i32..=9999, doy in 1u16..=366) {
                let max = days_in_year(y);
                let doy_clamped = doy.min(max);
                let (month, day) = day_of_year_to_month_day(doy_clamped, y);
                prop_assert!((1..=12).contains(&month), "month out of range: {month}");
                let days_in_month: [u8; 12] = if is_leap_year(y) {
                    [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
                } else {
                    [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
                };
                let max_day = days_in_month[(month - 1) as usize];
                prop_assert!(day >= 1, "day out of range: {day}");
                prop_assert!(day <= max_day, "day {day} exceeds max {max_day} for month {month}");
            }
        }
    }
}
