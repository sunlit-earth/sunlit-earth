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
    // After Feb 28 in a leap year, the cumulative offset is one less
    // than in a non-leap year, so we adjust the day-of-year down.
    for month_idx in (0..12).rev() {
        let mut cum = CUMULATIVE_DAYS[month_idx];
        if leap && month_idx >= 2 {
            cum += 1; // Feb has 29 days in a leap year
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

    // --- base_year / year_range ---

    #[test]
    fn base_year_is_current_minus_10() {
        let current = time::OffsetDateTime::now_utc().year();
        assert_eq!(base_year(), current - 10);
    }

    #[test]
    fn year_range_spans_21_years() {
        let (start, end) = year_range();
        assert_eq!(end - start + 1, 21);
    }

    #[test]
    fn year_range_centered_on_current() {
        let current = time::OffsetDateTime::now_utc().year();
        let (start, end) = year_range();
        assert_eq!(start, current - 10);
        assert_eq!(end, current + 10);
    }

    // --- is_leap_year ---

    #[test]
    fn leap_year_divisible_by_4() {
        assert!(is_leap_year(2024));
    }

    #[test]
    fn non_leap_year_not_divisible_by_4() {
        assert!(!is_leap_year(2025));
    }

    #[test]
    fn non_leap_century() {
        assert!(!is_leap_year(1900));
    }

    #[test]
    fn leap_400_year_century() {
        assert!(is_leap_year(2000));
    }

    #[test]
    fn non_leap_century_2100() {
        assert!(!is_leap_year(2100));
    }

    // --- days_in_year ---

    #[test]
    fn days_in_leap_year() {
        assert_eq!(days_in_year(2024), 366);
    }

    #[test]
    fn days_in_non_leap_year() {
        assert_eq!(days_in_year(2025), 365);
    }

    #[test]
    fn days_in_year_2000() {
        assert_eq!(days_in_year(2000), 366);
    }

    // --- day_of_year_to_month_day ---

    #[test]
    fn doy_jan_1() {
        assert_eq!(day_of_year_to_month_day(1, 2025), (1, 1));
    }

    #[test]
    fn doy_jan_31() {
        assert_eq!(day_of_year_to_month_day(31, 2025), (1, 31));
    }

    #[test]
    fn doy_feb_1() {
        assert_eq!(day_of_year_to_month_day(32, 2025), (2, 1));
    }

    #[test]
    fn doy_feb_28_non_leap() {
        assert_eq!(day_of_year_to_month_day(59, 2025), (2, 28));
    }

    #[test]
    fn doy_mar_1_non_leap() {
        assert_eq!(day_of_year_to_month_day(60, 2025), (3, 1));
    }

    #[test]
    fn doy_feb_29_leap() {
        assert_eq!(day_of_year_to_month_day(60, 2024), (2, 29));
    }

    #[test]
    fn doy_mar_1_leap() {
        assert_eq!(day_of_year_to_month_day(61, 2024), (3, 1));
    }

    #[test]
    fn doy_dec_31_non_leap() {
        assert_eq!(day_of_year_to_month_day(365, 2025), (12, 31));
    }

    #[test]
    fn doy_dec_31_leap() {
        assert_eq!(day_of_year_to_month_day(366, 2024), (12, 31));
    }

    #[test]
    fn doy_jul_1() {
        assert_eq!(day_of_year_to_month_day(182, 2025), (7, 1));
    }

    // --- month_day_label ---

    #[test]
    fn label_jan_1() {
        assert_eq!(month_day_label(1, 2025), "Jan 1");
    }

    #[test]
    fn label_mar_17() {
        assert_eq!(month_day_label(76, 2025), "Mar 17");
    }

    #[test]
    fn label_dec_31_non_leap() {
        assert_eq!(month_day_label(365, 2025), "Dec 31");
    }

    #[test]
    fn label_dec_31_leap() {
        assert_eq!(month_day_label(366, 2024), "Dec 31");
    }

    #[test]
    fn label_feb_29_leap() {
        assert_eq!(month_day_label(60, 2024), "Feb 29");
    }

    #[test]
    fn label_mar_1_non_leap() {
        assert_eq!(month_day_label(60, 2025), "Mar 1");
    }

    // --- hour_float_to_hm ---

    #[test]
    fn hm_zero() {
        assert_eq!(hour_float_to_hm(0.0), (0, 0));
    }

    #[test]
    fn hm_noon() {
        assert_eq!(hour_float_to_hm(12.0), (12, 0));
    }

    #[test]
    fn hm_half_past_two() {
        assert_eq!(hour_float_to_hm(14.5), (14, 30));
    }

    #[test]
    fn hm_quarter_to_three() {
        assert_eq!(hour_float_to_hm(14.75), (14, 45));
    }

    #[test]
    fn hm_almost_midnight() {
        // 23.99 -> 23 hours + 0.99*60 = 59.4 -> minute 59
        assert_eq!(hour_float_to_hm(23.99), (23, 59));
    }

    #[test]
    fn hm_clamped_at_24() {
        assert_eq!(hour_float_to_hm(24.0), (23, 59));
    }

    #[test]
    fn hm_one_minute() {
        // 1/60 = 0.01667
        let (h, m) = hour_float_to_hm(1.0 / 60.0);
        assert_eq!(h, 0);
        assert_eq!(m, 1);
    }

    // --- hour_label ---

    #[test]
    fn hour_label_midnight() {
        assert_eq!(hour_label(0.0), "00:00");
    }

    #[test]
    fn hour_label_morning() {
        assert_eq!(hour_label(9.5), "09:30");
    }

    #[test]
    fn hour_label_afternoon() {
        assert_eq!(hour_label(14.75), "14:45");
    }

    #[test]
    fn hour_label_late_night() {
        assert_eq!(hour_label(23.99), "23:59");
    }

    // --- hour_float_to_hms ---

    #[test]
    fn hms_zero() {
        let (h, m, s) = hour_float_to_hms(0.0);
        assert_eq!(h, 0);
        assert_eq!(m, 0);
        assert_relative_eq!(s, 0.0, epsilon = 0.1);
    }

    #[test]
    fn hms_half_past_two() {
        let (h, m, s) = hour_float_to_hms(14.5);
        assert_eq!(h, 14);
        assert_eq!(m, 30);
        assert_relative_eq!(s, 0.0, epsilon = 0.1);
    }

    #[test]
    fn hms_quarter_to_three() {
        let (h, m, s) = hour_float_to_hms(14.75);
        assert_eq!(h, 14);
        assert_eq!(m, 45);
        assert_relative_eq!(s, 0.0, epsilon = 0.1);
    }

    #[test]
    fn hms_fractional_seconds() {
        // 12.5025 = 12h 30m 9s
        let (h, m, s) = hour_float_to_hms(12.5025);
        assert_eq!(h, 12);
        assert_eq!(m, 30);
        assert_relative_eq!(s, 9.0, epsilon = 1.0);
    }

    #[test]
    fn hms_clamped_at_24() {
        let (h, m, s) = hour_float_to_hms(24.0);
        assert_eq!(h, 23);
        assert_eq!(m, 59);
        assert_relative_eq!(s, 59.0, epsilon = 1.0);
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
            fn doy_month_in_range(y in 1i32..=9999, doy in 1u16..=366) {
                let max = days_in_year(y);
                let doy_clamped = doy.min(max);
                let (month, day) = day_of_year_to_month_day(doy_clamped, y);
                prop_assert!((1..=12).contains(&month), "month out of range: {month}");
                prop_assert!((1..=31).contains(&day), "day out of range: {day}");
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
                let days_in_month: [u8; 12] = if is_leap_year(y) {
                    [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
                } else {
                    [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
                };
                let max_day = days_in_month[(month - 1) as usize];
                prop_assert!(day <= max_day, "day {day} exceeds max {max_day} for month {month}");
            }
        }
    }
}
