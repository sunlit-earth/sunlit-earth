//! The time setting: live time, or one instant.
//!
//! It is what the wallpaper and every export draw at, what the config saves,
//! and what the sidebar's Date / Time group and the time bar's tracks edit.
//! The preview can leave it for a while during an excursion, which the engine
//! owns and which never changes it.

use time::{Duration, OffsetDateTime, UtcOffset, macros::datetime};

/// The earliest instant the scene accepts.
pub const EARLIEST: OffsetDateTime = datetime!(1900-01-01 00:00 UTC);

/// The latest instant the scene accepts, the last second of 2100.
pub const LATEST: OffsetDateTime = datetime!(2100-12-31 23:59:59 UTC);

/// How the config file spells live time.
const LIVE: &str = "live";

/// Live time or one instant, in UTC.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SceneTime {
    /// The system clock's reading, whenever a frame is drawn.
    #[default]
    Live,
    /// One instant, within [`EARLIEST`] and [`LATEST`].
    Fixed(OffsetDateTime),
}

impl SceneTime {
    /// One instant, moved to UTC and into the range the scene accepts.
    #[must_use]
    pub fn fixed(at: OffsetDateTime) -> Self {
        Self::Fixed(clamp_instant(at))
    }

    /// The instant this names, with `now_utc` the clock's reading.
    #[must_use]
    pub fn instant(self, now_utc: OffsetDateTime) -> OffsetDateTime {
        match self {
            Self::Live => now_utc,
            Self::Fixed(at) => clamp_instant(at),
        }
    }

    #[must_use]
    pub fn is_live(self) -> bool {
        self == Self::Live
    }

    /// How the config file spells this: `"live"`, or the instant in RFC 3339
    /// in UTC.
    #[must_use]
    pub fn to_config_string(self) -> String {
        match self {
            Self::Live => LIVE.to_owned(),
            Self::Fixed(at) => format_instant(clamp_instant(at)),
        }
    }

    /// Read the config file's spelling back. `None` for anything that is
    /// neither.
    #[must_use]
    pub fn from_config_string(text: &str) -> Option<Self> {
        if text.trim().eq_ignore_ascii_case(LIVE) {
            return Some(Self::Live);
        }
        OffsetDateTime::parse(text.trim(), &time::format_description::well_known::Rfc3339)
            .ok()
            .map(Self::fixed)
    }
}

/// An instant in UTC and within [`EARLIEST`] and [`LATEST`].
#[must_use]
pub fn clamp_instant(at: OffsetDateTime) -> OffsetDateTime {
    at.to_offset(UtcOffset::UTC).clamp(EARLIEST, LATEST)
}

/// An instant in RFC 3339, in UTC.
fn format_instant(at: OffsetDateTime) -> String {
    at.to_offset(UtcOffset::UTC)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| LIVE.to_owned())
}

/// The instant the date and time override of version 2 of the config named,
/// computed as it was drawn: the day of the year through the year's calendar,
/// clamped to it, and the hour as a fraction of a day in UTC.
#[must_use]
pub fn instant_of_override(hour: f32, day_of_year: u16, year: i32) -> OffsetDateTime {
    let (month, day) = super::datetime::day_of_year_to_month_day(day_of_year.max(1), year);
    let (hour, minute, second) = super::datetime::hour_float_to_hms(hour);
    let date = time::Month::try_from(month)
        .ok()
        .and_then(|month| time::Date::from_calendar_date(year, month, day).ok())
        .unwrap_or(EARLIEST.date());
    let whole = second.trunc();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the nanoseconds of a fraction of a second"
    )]
    let nanos = ((second - whole) * 1e9).round() as i64;
    let midnight = date.midnight().assume_utc();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a second of a minute, below 60"
    )]
    let seconds = whole as i64;
    clamp_instant(
        midnight
            + Duration::hours(i64::from(hour))
            + Duration::minutes(i64::from(minute))
            + Duration::seconds(seconds)
            + Duration::nanoseconds(nanos),
    )
}

impl serde::Serialize for SceneTime {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_config_string())
    }
}

impl<'de> serde::Deserialize<'de> for SceneTime {
    /// Anything that is neither `"live"` nor an RFC 3339 instant loads as live
    /// time, so a damaged value costs the fixed time and not the whole file.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Ok(Self::from_config_string(&text).unwrap_or_else(|| {
            tracing::warn!(
                value = text,
                "the scene time is not \"live\" or an instant, using live time"
            );
            Self::Live
        }))
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    #[test]
    fn live_time_is_the_clocks_reading_and_a_fixed_one_is_itself() {
        let now = datetime!(2026-10-18 14:23:45.5 UTC);
        assert_eq!(SceneTime::Live.instant(now), now);
        let at = datetime!(2024-02-29 06:00 UTC);
        assert_eq!(SceneTime::fixed(at).instant(now), at);
    }

    #[test]
    fn a_fixed_instant_is_kept_in_utc_and_within_the_range() {
        let at = datetime!(2026-03-29 03:30 +2);
        let SceneTime::Fixed(fixed) = SceneTime::fixed(at) else {
            panic!("a fixed time");
        };
        assert_eq!(fixed, at);
        assert_eq!(fixed.offset(), UtcOffset::UTC);
        assert_eq!(
            SceneTime::fixed(datetime!(1850-06-01 00:00 UTC)),
            SceneTime::Fixed(EARLIEST)
        );
        assert_eq!(
            SceneTime::fixed(datetime!(2150-06-01 00:00 UTC)),
            SceneTime::Fixed(LATEST)
        );
    }

    #[test]
    fn the_config_spelling_round_trips() {
        for time in [
            SceneTime::Live,
            SceneTime::fixed(datetime!(2026-10-18 14:23:00 UTC)),
            SceneTime::fixed(datetime!(1999-12-31 23:59:59.25 UTC)),
        ] {
            assert_eq!(
                SceneTime::from_config_string(&time.to_config_string()),
                Some(time)
            );
        }
        assert_eq!(
            SceneTime::fixed(datetime!(2026-10-18 14:23:00 UTC)).to_config_string(),
            "2026-10-18T14:23:00Z"
        );
        assert_eq!(SceneTime::from_config_string("Live"), Some(SceneTime::Live));
        assert_eq!(SceneTime::from_config_string("noon"), None);
    }

    #[test]
    fn a_damaged_value_loads_as_live_time() {
        #[derive(serde::Deserialize)]
        struct File {
            scene_time: SceneTime,
        }
        let file: File = toml::from_str("scene_time = \"yesterday\"").unwrap();
        assert_eq!(file.scene_time, SceneTime::Live);
    }

    /// The day of the year goes through the year's own calendar, so day 60 is
    /// the leap day in a leap year and the first of March otherwise, day 366
    /// is the last day of either, and the hour keeps its fraction.
    #[test]
    fn the_old_override_becomes_the_instant_it_drew() {
        assert_eq!(
            instant_of_override(12.0, 60, 2024),
            datetime!(2024-02-29 12:00 UTC)
        );
        assert_eq!(
            instant_of_override(12.0, 60, 2025),
            datetime!(2025-03-01 12:00 UTC)
        );
        assert_eq!(
            instant_of_override(0.0, 366, 2024),
            datetime!(2024-12-31 00:00 UTC)
        );
        assert_eq!(
            instant_of_override(0.0, 366, 2025),
            datetime!(2025-12-31 00:00 UTC)
        );
        assert_eq!(
            instant_of_override(9.25, 120, 2026),
            datetime!(2026-04-30 09:15 UTC)
        );
        let fractional = instant_of_override(14.123, 1, 2026);
        assert_eq!(fractional.date(), datetime!(2026-01-01 00:00 UTC).date());
        assert_eq!((fractional.hour(), fractional.minute()), (14, 7));
    }
}
