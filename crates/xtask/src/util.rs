//! Small helpers shared by the rest of the xtask: clock, byte formatting, and
//! the environment-variable convention the product crates already follow.

use std::time::{SystemTime, UNIX_EPOCH};

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Seconds in a day, the unit every age and expiry number in here is measured
/// in.
pub const SECS_PER_DAY: u64 = 24 * 60 * 60;

/// The current wall-clock time as whole seconds since the Unix epoch.
///
/// Every function that reasons about age takes `now` as an argument instead of
/// calling this, so the expiry and currency logic is testable without a clock.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Format a Unix timestamp as an RFC 3339 UTC string, or `"unknown"` if it is
/// outside the representable range.
pub fn format_unix_utc(secs: u64) -> String {
    i64::try_from(secs)
        .ok()
        .and_then(|s| OffsetDateTime::from_unix_timestamp(s).ok())
        .and_then(|dt| dt.format(&Rfc3339).ok())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Whole days between two Unix timestamps, floored, saturating at zero when
/// `later` precedes `earlier`.
pub fn days_between(earlier: u64, later: u64) -> u64 {
    later.saturating_sub(earlier) / SECS_PER_DAY
}

/// Render a byte count with a binary unit and one decimal.
///
/// Sizes here run from a few kilobytes (a manifest) to tens of gigabytes (a
/// Windows image), so a fixed unit would be unreadable at one end or the other.
#[allow(clippy::cast_precision_loss)]
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{:.1} {}", value, UNITS[unit])
}

/// An elapsed time, for the lines a long-running command prints about itself.
///
/// Whole seconds and no fractions: the readings are minutes apart, and the
/// question they answer is "how far into an hour-long build is this".
pub fn format_duration(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    let (hours, minutes, seconds) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m{seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

/// Treat an unset and a blank environment variable the same, matching
/// `sunlit_core::env_override`.
pub fn non_blank(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

/// Read an environment variable under the blank-is-unset rule.
pub fn env_var(name: &str) -> Option<String> {
    non_blank(std::env::var(name).ok())
}

/// `"1 thing"` / `"2 things"`, for report lines that count what they found.
pub fn count(n: usize, singular: &str) -> String {
    if n == 1 {
        format!("1 {singular}")
    } else {
        format!("{n} {singular}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_uses_the_largest_unit_that_keeps_the_value_above_one() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
        assert_eq!(format_bytes(1024_u64.pow(4)), "1.0 TiB");
        // Beyond the last unit the value keeps growing rather than wrapping.
        assert_eq!(format_bytes(2 * 1024_u64.pow(5)), "2048.0 TiB");
    }

    #[test]
    fn days_between_floors_and_never_goes_negative() {
        assert_eq!(days_between(0, 0), 0);
        assert_eq!(days_between(0, SECS_PER_DAY - 1), 0);
        assert_eq!(days_between(0, SECS_PER_DAY), 1);
        assert_eq!(days_between(0, 90 * SECS_PER_DAY + 5), 90);
        // A manifest stamped in the future (clock skew, or a restored backup)
        // reads as zero days old rather than as an enormous one.
        assert_eq!(days_between(100 * SECS_PER_DAY, 0), 0);
    }

    #[test]
    fn format_unix_utc_renders_rfc3339() {
        assert_eq!(format_unix_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_unix_utc(1_755_600_000), "2025-08-19T10:40:00Z");
    }

    #[test]
    fn blank_environment_values_are_treated_as_unset() {
        assert_eq!(non_blank(None), None);
        assert_eq!(non_blank(Some(String::new())), None);
        assert_eq!(non_blank(Some("   ".to_owned())), None);
        assert_eq!(non_blank(Some(" x ".to_owned())), Some(" x ".to_owned()));
    }

    #[test]
    fn durations_read_as_a_position_in_a_long_build() {
        use std::time::Duration;
        assert_eq!(format_duration(Duration::from_secs(0)), "0s");
        assert_eq!(format_duration(Duration::from_secs(45)), "45s");
        assert_eq!(format_duration(Duration::from_secs(60)), "1m00s");
        assert_eq!(format_duration(Duration::from_secs(250)), "4m10s");
        assert_eq!(format_duration(Duration::from_secs(3600)), "1h00m");
        assert_eq!(format_duration(Duration::from_secs(4500)), "1h15m");
    }

    #[test]
    fn count_pluralizes() {
        assert_eq!(count(0, "overlay"), "0 overlays");
        assert_eq!(count(1, "overlay"), "1 overlay");
        assert_eq!(count(2, "overlay"), "2 overlays");
    }
}
