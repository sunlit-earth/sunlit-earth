//! The time bar's side of the window.
//!
//! The bar shows the instant on screen, which is the last frame's while the
//! engine runs an excursion and the time setting's otherwise. Its transport
//! acts on the excursion alone, through `EngineCommand::Excursion`; its
//! tracks, exact entry and Now write the time setting, which a push carries
//! to the engine and which ends any excursion there. Every text the bar shows
//! is formatted here.

use std::cell::RefCell;

use slint::{ComponentHandle, Model};
use time::OffsetDateTime;

use sunlit_core::engine::EngineCommand;
use sunlit_core::engine::excursion::{ExcursionCommand, ExcursionKind, ExcursionState, TimeStep};
use sunlit_core::scene::camera;
use sunlit_core::scene::datetime;
use sunlit_core::scene::eclipse::{self, Eclipse, EclipseKind};
use sunlit_core::scene::scene_time::SceneTime;
use sunlit_core::scene::sky;

use crate::engine_client::EngineLink;
use crate::ui_callbacks::{self, display_offset, instant_of_sliders, sliders_of};
use crate::{
    EclipseBand, EclipseMark, EclipsePanelView, EclipsePhase, MainWindow, OverlayHints, RateChoice,
    SeasonMark, TimeBarView,
};

const MINUS: &str = "\u{2212}";

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

const SEASON_NAMES: [&str; 4] = [
    "March equinox",
    "June solstice",
    "September equinox",
    "December solstice",
];

/// How near a season's day, in days, the date track's handle snaps to it.
const SEASON_SNAP: i64 = 3;

/// The samples of the time track's shading, one every half hour.
pub const SHADING_SAMPLES: usize = 48;

/// The colors of the time track by the Sun's altitude in degrees, from
/// night through the twilights and sunrise to the day.
const ALTITUDE_COLORS: [(f32, [u8; 3]); 8] = [
    (-18.0, [10, 18, 38]),
    (-12.0, [18, 29, 63]),
    (-6.0, [59, 47, 99]),
    (-2.0, [150, 74, 92]),
    (1.0, [224, 135, 74]),
    (6.0, [226, 178, 102]),
    (15.0, [93, 154, 214]),
    (40.0, [63, 134, 208]),
];

/// What play runs, at which rate, and whether the engine says it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transport {
    pub kind: ExcursionKind,
    /// Signed: multiples of real time, or days a second.
    pub rate: i32,
    pub running: bool,
}

/// One use of the transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// The play button and Space.
    Toggle,
    /// K.
    Pause,
    /// A ruler's dot or a row of the speed list.
    Pick(i32),
    /// J, -1, and L, 1: from a pause start in their direction, then faster
    /// along it; against it slower, then stop.
    Shuttle(i32),
    /// R: the same speed the other way.
    Reverse,
    /// A radio label.
    Kind(ExcursionKind),
}

fn default_rate(kind: ExcursionKind) -> i32 {
    i32::try_from(kind.default_rate()).unwrap_or(1)
}

impl Transport {
    fn direction(self) -> i32 {
        if self.rate < 0 { -1 } else { 1 }
    }

    /// Where the rate stands on its kind's ladder; a rate off the ladder
    /// counts as the slowest.
    fn index(self) -> usize {
        self.kind
            .ladder()
            .iter()
            .position(|&rate| rate == self.rate.unsigned_abs())
            .unwrap_or(0)
    }

    /// The rate at `index` of the ladder, held to its ends, in this
    /// direction.
    fn at(self, index: usize) -> i32 {
        let ladder = self.kind.ladder();
        let magnitude = ladder[index.min(ladder.len() - 1)];
        self.direction() * i32::try_from(magnitude).unwrap_or(i32::MAX)
    }

    fn with_rate(self, rate: i32) -> Self {
        Self { rate, ..self }
    }

    fn play(self) -> ExcursionCommand {
        ExcursionCommand::Play {
            rate: self.rate,
            kind: self.kind,
        }
    }

    /// The transport after `action`, and what to ask of the engine. Whether
    /// it runs stays the engine's to say.
    #[must_use]
    pub fn act(self, action: Action) -> (Self, Option<ExcursionCommand>) {
        match action {
            Action::Toggle if self.running => (self, Some(ExcursionCommand::Pause)),
            Action::Toggle => (self, Some(self.play())),
            Action::Pause => (self, self.running.then_some(ExcursionCommand::Pause)),
            Action::Pick(rate) => {
                let next = self.with_rate(rate);
                (next, Some(next.play()))
            }
            Action::Shuttle(direction) => {
                let direction = if direction < 0 { -1 } else { 1 };
                let next = if !self.running {
                    self.with_rate(direction * self.rate.abs())
                } else if direction == self.direction() {
                    self.with_rate(self.at(self.index() + 1))
                } else if self.index() == 0 {
                    return (self, Some(ExcursionCommand::Pause));
                } else {
                    self.with_rate(self.at(self.index() - 1))
                };
                (next, Some(next.play()))
            }
            Action::Reverse => {
                let next = self.with_rate(-self.rate);
                (
                    next,
                    if self.running {
                        Some(next.play())
                    } else {
                        None
                    },
                )
            }
            Action::Kind(kind) if kind == self.kind => (self, None),
            Action::Kind(kind) => {
                let next = Self {
                    kind,
                    rate: self.direction() * default_rate(kind),
                    ..self
                };
                (
                    next,
                    if self.running {
                        Some(next.play())
                    } else {
                        None
                    },
                )
            }
        }
    }

    /// Whether `action` changes what play runs, its speed, or whether it
    /// runs.
    #[must_use]
    pub fn changes(self, action: Action) -> bool {
        let (next, command) = self.act(action);
        next != self
            || match command {
                Some(ExcursionCommand::Pause) => self.running,
                Some(ExcursionCommand::Play { .. }) => !self.running,
                _ => false,
            }
    }
}

/// `count` of `unit` a second, the unit in the plural from two: "1 week/s".
fn per_second(count: u32, unit: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural}/s")
}

/// A rate as the speed list names it: "300×", "1 week/s", with a minus sign
/// going backward.
#[must_use]
pub fn rate_label(rate: i32, kind: ExcursionKind) -> String {
    let sign = if rate < 0 { MINUS } else { "" };
    let magnitude = rate.unsigned_abs();
    let text = match kind {
        ExcursionKind::Clock => format!("{magnitude}\u{d7}"),
        ExcursionKind::Date if magnitude.is_multiple_of(30) => per_second(magnitude / 30, "month"),
        ExcursionKind::Date if magnitude.is_multiple_of(7) => per_second(magnitude / 7, "week"),
        ExcursionKind::Date => per_second(magnitude, "day"),
    };
    format!("{sign}{text}")
}

/// What a rate means: how much time a second of it covers, "5 min/s", or
/// that the date keeps the time of day.
#[must_use]
pub fn rate_note(rate: i32, kind: ExcursionKind) -> String {
    let magnitude = rate.unsigned_abs();
    match kind {
        ExcursionKind::Date => "same time each day".to_owned(),
        ExcursionKind::Clock if magnitude == 1 => "real time".to_owned(),
        ExcursionKind::Clock if magnitude.is_multiple_of(3600) => {
            format!("{} h/s", magnitude / 3600)
        }
        ExcursionKind::Clock if magnitude.is_multiple_of(60) => format!("{} min/s", magnitude / 60),
        ExcursionKind::Clock => format!("{magnitude} s/s"),
    }
}

/// How far an instant is from now: "now" within a minute, then the two
/// largest of years, days, hours and minutes, "+5 min", "−2 d 3 h".
#[must_use]
pub fn offset_text(from_now: time::Duration) -> String {
    const YEAR: u64 = 525_960;
    const DAY: u64 = 1440;
    let minutes = from_now.whole_minutes().unsigned_abs();
    if minutes == 0 {
        return "now".to_owned();
    }
    let sign = if from_now.is_negative() { MINUS } else { "+" };
    let (years, rest) = (minutes / YEAR, minutes % YEAR);
    let (days, rest) = (rest / DAY, rest % DAY);
    let (hours, minutes) = (rest / 60, rest % 60);
    if years > 0 {
        format!("{sign}{years} y {days} d")
    } else if days > 0 {
        format!("{sign}{days} d {hours} h")
    } else if hours > 0 {
        format!("{sign}{hours} h {minutes} min")
    } else {
        format!("{sign}{minutes} min")
    }
}

/// The folded bar's state: "LIVE", the offset of a fixed time from now, the
/// direction and the speed of a running excursion, or "Preview" for a
/// paused one.
#[must_use]
pub fn state_text(live: bool, excursion: Option<ExcursionState>, offset: &str) -> String {
    match excursion {
        Some(state) if state.running => {
            let arrow = if state.rate < 0 {
                "\u{25c0}"
            } else {
                "\u{25b6}"
            };
            format!("{arrow} {}", rate_label(state.rate.abs(), state.kind))
        }
        Some(_) => "Preview".to_owned(),
        None if live => "LIVE".to_owned(),
        None => offset.to_owned(),
    }
}

/// The time track's color for the Sun at `altitude` degrees.
#[must_use]
pub fn altitude_color(altitude: f32) -> [u8; 3] {
    let (first, last) = (
        ALTITUDE_COLORS[0],
        ALTITUDE_COLORS[ALTITUDE_COLORS.len() - 1],
    );
    if altitude <= first.0 {
        return first.1;
    }
    for pair in ALTITUDE_COLORS.windows(2) {
        let ((low, from), (high, to)) = (pair[0], pair[1]);
        if altitude <= high {
            let along = (altitude - low) / (high - low);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a blend of two bytes, rounded"
            )]
            return std::array::from_fn(|channel| {
                let (a, b) = (f32::from(from[channel]), f32::from(to[channel]));
                (a + (b - a) * along).round() as u8
            });
        }
    }
    last.1
}

/// The time track's shading for the day that starts at `day_start`, from the
/// Sun's altitude over the point at `latitude` and `longitude`, in the
/// middle of each half hour.
#[must_use]
pub fn shading(
    day_start: OffsetDateTime,
    latitude: f32,
    longitude: f32,
) -> [[u8; 3]; SHADING_SAMPLES] {
    let (lat, lon) = (latitude.to_radians(), longitude.to_radians());
    let up = [lat.cos() * lon.sin(), lat.sin(), lat.cos() * lon.cos()];
    std::array::from_fn(|sample| {
        #[expect(clippy::cast_possible_wrap, reason = "fewer than 48 samples")]
        let at = day_start + time::Duration::minutes(15 + 30 * sample as i64);
        let sun = sky::sun_direction_at(at);
        let sine = up[0] * sun.x + up[1] * sun.y + up[2] * sun.z;
        altitude_color(sine.clamp(-1.0, 1.0).asin().to_degrees())
    })
}

/// The day of the year, from 0, that the date track's handle stands on for
/// `day`, snapped to a season's day within [`SEASON_SNAP`] days.
#[must_use]
pub fn snap_to_season(day: i64, seasons: &[i64]) -> i64 {
    seasons
        .iter()
        .copied()
        .find(|season| (season - day).abs() <= SEASON_SNAP)
        .unwrap_or(day)
}

/// The kind the window's radio labels choose.
fn kind_of(view: &TimeBarView) -> ExcursionKind {
    if view.date_kind {
        ExcursionKind::Date
    } else {
        ExcursionKind::Clock
    }
}

/// The transport as the window holds it. A rate never chosen is the kind's
/// default, which is where a session starts.
fn transport(window: &MainWindow) -> Transport {
    let view = window.get_time_bar();
    let kind = kind_of(&view);
    Transport {
        kind,
        rate: if view.rate == 0 {
            default_rate(kind)
        } else {
            view.rate
        },
        running: view.running,
    }
}

/// The instant on screen: the last frame's during an excursion, the time
/// setting's otherwise.
fn on_screen(window: &MainWindow, now: OffsetDateTime) -> OffsetDateTime {
    if window.get_time_bar().excursion
        && let Some(SceneTime::Fixed(at)) =
            SceneTime::from_config_string(&window.get_time_frame_instant())
    {
        return at;
    }
    ui_callbacks::scene_time_of(&window.get_datetime()).instant(now)
}

/// The instant on screen in the displayed zone.
fn local_on_screen(window: &MainWindow, now: OffsetDateTime) -> OffsetDateTime {
    let at = on_screen(window, now);
    at.to_offset(display_offset(window.get_time_zone_utc(), at))
}

/// The fraction of its day a wall time has passed.
fn day_fraction(local: OffsetDateTime) -> f32 {
    let seconds =
        u32::from(local.hour()) * 3600 + u32::from(local.minute()) * 60 + u32::from(local.second());
    #[expect(clippy::cast_precision_loss, reason = "seconds of one day")]
    let fraction = seconds as f32 / 86_400.0;
    fraction
}

/// The fraction of its year a wall time has passed.
fn year_fraction(local: OffsetDateTime) -> f32 {
    (f32::from(local.ordinal() - 1) + day_fraction(local))
        / f32::from(datetime::days_in_year(local.year()))
}

/// Act on the transport and tell the engine. Playback of the date swings the
/// camera of eclipse mode around the globe, so it leaves the mode first.
fn act(window: &MainWindow, link: &EngineLink, action: Action) {
    let (next, command) = transport(window).act(action);
    if matches!(
        command,
        Some(ExcursionCommand::Play {
            kind: ExcursionKind::Date,
            ..
        })
    ) && window.get_camera_position().eclipse_follow
    {
        leave_eclipse(window);
        link.push_params(window);
    }
    let mut view = window.get_time_bar();
    view.rate = next.rate;
    view.date_kind = next.kind == ExcursionKind::Date;
    window.set_time_bar(view);
    if let Some(command) = command {
        link.send(EngineCommand::Excursion(command));
    }
    refresh(window);
}

/// Make `at` the time setting, from a track or exact entry. A new day on
/// screen leaves eclipse mode, whose eclipse is on the day it was.
fn set_time(window: &MainWindow, link: &EngineLink, at: OffsetDateTime) {
    leave_eclipse_for_a_new_day(window, at);
    write_time(window, link, at);
}

/// Make `at` the time setting. The push ends the excursion in the engine when
/// the setting changed, and `End` when it did not; the bar goes back to the
/// setting at once rather than when the engine says so, so the next move of a
/// drag starts from what it set.
fn write_time(window: &MainWindow, link: &EngineLink, at: OffsetDateTime) {
    ui_callbacks::set_fixed_time(window, at);
    ui_callbacks::update_datetime_labels(window);
    link.push_params(window);
    let mut view = window.get_time_bar();
    if view.excursion {
        link.send(EngineCommand::Excursion(ExcursionCommand::End));
        view.excursion = false;
        view.running = false;
        window.set_time_bar(view);
    }
    refresh(window);
}

/// The instant at `fraction` along the track of the displayed day or year,
/// the rest of the instant on screen kept.
fn track_instant(window: &MainWindow, year_track: bool, fraction: f32) -> OffsetDateTime {
    let utc = window.get_time_zone_utc();
    let offset_at = |at| display_offset(utc, at);
    let local = local_on_screen(window, OffsetDateTime::now_utc());
    let fraction = fraction.clamp(0.0, 0.99999);
    if year_track {
        let days = datetime::days_in_year(local.year());
        #[expect(clippy::cast_possible_truncation, reason = "a day of one year")]
        let day = (fraction * f32::from(days)).floor() as i64;
        let day = snap_to_season(day, &season_days(local.year(), utc));
        let (hour, _, _) = sliders_of(local, offset_at);
        #[expect(clippy::cast_precision_loss, reason = "a day of one year")]
        instant_of_sliders(hour, (day + 1) as f32, local.year(), offset_at)
    } else {
        let minutes = (fraction * 1440.0).round().min(1439.0);
        instant_of_sliders(
            minutes / 60.0,
            f32::from(local.ordinal()),
            local.year(),
            offset_at,
        )
    }
}

/// The seasons of `year` in the displayed zone, `None` where the ephemeris
/// has no answer.
fn seasons_shown(year: i32, utc: bool) -> Vec<OffsetDateTime> {
    sky::seasons(year)
        .map(|seasons| {
            seasons
                .iter()
                .map(|&at| at.to_offset(display_offset(utc, at)))
                .collect()
        })
        .unwrap_or_default()
}

/// The days of the year, from 0, the seasons of `year` fall on.
fn season_days(year: i32, utc: bool) -> Vec<i64> {
    seasons_shown(year, utc)
        .iter()
        .map(|at| i64::from(at.ordinal()) - 1)
        .collect()
}

/// "18 Oct".
fn day_and_month(date: time::Date) -> String {
    format!(
        "{} {}",
        date.day(),
        MONTHS[usize::from(u8::from(date.month()) - 1)]
    )
}

/// "16:23", with ":07" while live.
fn clock_text(local: OffsetDateTime) -> String {
    format!("{:02}:{:02}", local.hour(), local.minute())
}

/// What the time setting, which the wallpaper keeps, reads in the displayed
/// zone.
fn wallpaper_text(window: &MainWindow) -> String {
    match ui_callbacks::scene_time_of(&window.get_datetime()) {
        SceneTime::Live => "The wallpaper keeps live time.".to_owned(),
        SceneTime::Fixed(at) => {
            let local = at.to_offset(display_offset(window.get_time_zone_utc(), at));
            format!(
                "The wallpaper keeps {} {}, {} {}.",
                clock_text(local),
                datetime::zone_label(local.offset()),
                datetime::day_label(local.date()),
                local.year()
            )
        }
    }
}

/// Write what the bar shows from the time setting, the excursion, the last
/// frame and the camera.
pub fn refresh(window: &MainWindow) {
    let now = OffsetDateTime::now_utc();
    let transport = transport(window);
    let mut view = window.get_time_bar();
    let live = !view.excursion && ui_callbacks::scene_time_of(&window.get_datetime()).is_live();
    let shown = on_screen(window, now);
    let local = local_on_screen(window, now);
    let offset = if live {
        String::new()
    } else {
        offset_text(shown - now)
    };
    let excursion = view.excursion.then_some(ExcursionState {
        running: view.running,
        rate: transport.rate,
        kind: transport.kind,
    });
    let clock = clock_text(local);
    let seconds = if live {
        format!(":{:02}", local.second())
    } else {
        String::new()
    };
    let day = datetime::day_label(local.date());
    view.live = live;
    view.rate = transport.rate;
    view.state_text = state_text(live, excursion, &offset).into();
    view.pill_text = if live {
        format!("{day} \u{b7} {clock}{seconds}")
    } else {
        format!("{day} {} \u{b7} {clock}", local.year())
    }
    .into();
    view.pill_zone = if window.get_time_zone_utc() {
        "UTC"
    } else {
        ""
    }
    .into();
    view.zone_text = datetime::zone_label(local.offset()).into();
    view.time_text = clock.into();
    view.seconds_text = seconds.into();
    view.date_text = format!("{day} {}", local.year()).into();
    view.offset_text = offset.into();
    view.speed_text = rate_label(transport.rate.abs(), transport.kind).into();
    view.speed_note = rate_note(transport.rate, transport.kind).into();
    view.day_fraction = day_fraction(local);
    view.year_fraction = year_fraction(local);
    let position = window.get_camera_position();
    view.sun_fixed = position.sun_fixed;
    refresh_eclipse(window, &mut view, position.eclipse_follow, now);
    view.year = local.year();
    view.month = i32::from(u8::from(local.month()));
    view.day = i32::from(local.day());
    view.hour = i32::from(local.hour());
    view.minute = i32::from(local.minute());
    let hints = window.global::<OverlayHints>();
    hints.set_speed(format!("{} · {}", view.speed_text, view.speed_note).into());
    window.set_time_bar(view);
    hints.set_wallpaper_time(wallpaper_text(window).into());

    refresh_ladder(window, transport);
    refresh_year(window, local);
    if !position.sun_fixed && !position.eclipse_follow {
        refresh_shading(window, local, position.latitude, position.longitude);
    }
}

/// The rulers' dots and the speed list, when the kind changed.
fn refresh_ladder(window: &MainWindow, transport: Transport) {
    let ladder: Vec<i32> = transport
        .kind
        .ladder()
        .iter()
        .map(|&rate| i32::try_from(rate).unwrap_or(i32::MAX))
        .collect();
    if window.get_time_ladder().iter().eq(ladder.iter().copied()) {
        return;
    }
    let choices: Vec<RateChoice> = ladder
        .iter()
        .flat_map(|&rate| [-rate, rate])
        .map(|rate| RateChoice {
            rate,
            label: rate_label(rate, transport.kind).into(),
            note: match transport.kind {
                ExcursionKind::Clock => rate_note(rate, transport.kind).into(),
                ExcursionKind::Date => slint::SharedString::new(),
            },
        })
        .collect();
    window.set_time_ladder(slint::ModelRc::new(slint::VecModel::from(ladder)));
    window.set_time_rates(slint::ModelRc::new(slint::VecModel::from(choices)));
}

/// The date track's month ticks, season marks and eclipse marks, when the
/// displayed year or zone changed.
fn refresh_year(window: &MainWindow, local: OffsetDateTime) {
    let utc = window.get_time_zone_utc();
    let key = format!("{}|{}", local.year(), if utc { "utc" } else { "local" });
    if window.get_time_year_key() == key.as_str() {
        return;
    }
    let days = f32::from(datetime::days_in_year(local.year()));
    let ticks: Vec<f32> = (1..=12_u8)
        .filter_map(|month| time::Month::try_from(month).ok())
        .filter_map(|month| time::Date::from_calendar_date(local.year(), month, 1).ok())
        .map(|first| f32::from(first.ordinal() - 1) / days)
        .collect();
    let shown_seasons = seasons_shown(local.year(), utc);
    let eclipses = eclipse::in_year(local.year(), |at| display_offset(utc, at));
    window.set_time_eclipses(slint::ModelRc::new(slint::VecModel::from(eclipse_marks(
        &eclipses,
        &shown_seasons,
        utc,
    ))));
    YEAR_ECLIPSES.with(|year| *year.borrow_mut() = (key.clone(), eclipses));
    let seasons: Vec<SeasonMark> = shown_seasons
        .iter()
        .zip(SEASON_NAMES)
        .map(|(&at, name)| SeasonMark {
            fraction: year_fraction(at),
            label: format!(
                "{name} \u{b7} {} {}",
                datetime::day_label(at.date()),
                clock_text(at)
            )
            .into(),
        })
        .collect();
    window.set_time_month_ticks(slint::ModelRc::new(slint::VecModel::from(ticks)));
    window.set_time_seasons(slint::ModelRc::new(slint::VecModel::from(seasons)));
    window.set_time_year_key(key.into());
}

/// The time track's shading, when the displayed day or the point under the
/// camera moved by a degree.
fn refresh_shading(window: &MainWindow, local: OffsetDateTime, latitude: f32, longitude: f32) {
    let utc = window.get_time_zone_utc();
    let day_start = instant_of_sliders(0.0, f32::from(local.ordinal()), local.year(), |at| {
        display_offset(utc, at)
    });
    let key = format!(
        "{}|{}|{}",
        day_start.unix_timestamp(),
        latitude.round(),
        longitude.round()
    );
    if window.get_time_shading_key() == key.as_str() {
        return;
    }
    let colors = shading(day_start, latitude, longitude);
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(
        u32::try_from(SHADING_SAMPLES).unwrap_or(1),
        1,
    );
    for (pixel, [r, g, b]) in buffer.make_mut_slice().iter_mut().zip(colors) {
        *pixel = slint::Rgb8Pixel { r, g, b };
    }
    window.set_time_shading(slint::Image::from_rgb8(buffer));
    window.set_time_shading_key(key.into());
}

/// The engine said what the excursion is now.
pub fn excursion_changed(window: &MainWindow, state: Option<ExcursionState>) {
    let mut view = window.get_time_bar();
    view.excursion = state.is_some();
    view.running = state.is_some_and(|state| state.running);
    if let Some(state) = state {
        view.rate = state.rate;
        view.date_kind = state.kind == ExcursionKind::Date;
    }
    window.set_time_bar(view);
    refresh(window);
}

/// A frame that shows `instant` reached the window.
pub fn frame_shown(window: &MainWindow, instant: OffsetDateTime) {
    window.set_time_frame_instant(SceneTime::fixed(instant).to_config_string().into());
    if window.get_time_bar().excursion {
        refresh(window);
    }
}

/// Wire the time bar's callbacks.
pub fn register(window: &MainWindow, link: &EngineLink) {
    let longest = ExcursionKind::Clock
        .ladder()
        .len()
        .max(ExcursionKind::Date.ladder().len());
    window.set_time_longest_ladder(i32::try_from(longest).unwrap_or(i32::MAX));
    register_transport(window, link);
    register_setting(window, link);
    register_eclipse(window, link);
    register_labels(window);
    refresh(window);
}

/// The transport's callbacks, which act on the excursion.
fn register_transport(window: &MainWindow, link: &EngineLink) {
    let weak = window.as_weak();
    let engine = link.clone();
    window.on_time_toggle_play(move || {
        if let Some(win) = weak.upgrade() {
            act(&win, &engine, Action::Toggle);
        }
    });

    let weak = window.as_weak();
    let engine = link.clone();
    window.on_time_pause(move || {
        if let Some(win) = weak.upgrade() {
            act(&win, &engine, Action::Pause);
        }
    });

    let weak = window.as_weak();
    let engine = link.clone();
    window.on_time_rate(move |rate| {
        if let Some(win) = weak.upgrade() {
            act(&win, &engine, Action::Pick(rate));
        }
    });

    let weak = window.as_weak();
    let engine = link.clone();
    window.on_time_shuttle(move |direction| {
        let Some(win) = weak.upgrade() else {
            return false;
        };
        let action = Action::Shuttle(direction);
        let changes = transport(&win).changes(action);
        act(&win, &engine, action);
        changes
    });

    let weak = window.as_weak();
    let engine = link.clone();
    window.on_time_reverse(move || {
        if let Some(win) = weak.upgrade() {
            act(&win, &engine, Action::Reverse);
        }
    });

    let weak = window.as_weak();
    let engine = link.clone();
    window.on_time_kind(move |date| {
        if let Some(win) = weak.upgrade() {
            let kind = if date {
                ExcursionKind::Date
            } else {
                ExcursionKind::Clock
            };
            act(&win, &engine, Action::Kind(kind));
        }
    });

    let weak = window.as_weak();
    let engine = link.clone();
    window.on_time_skip(move |forward, big| {
        if let Some(win) = weak.upgrade() {
            let step = TimeStep {
                kind: transport(&win).kind,
                forward,
                big,
            };
            if step.kind == ExcursionKind::Date && win.get_camera_position().eclipse_follow {
                leave_eclipse(&win);
                engine.push_params(&win);
            }
            engine.send(EngineCommand::Excursion(ExcursionCommand::Skip(step)));
        }
    });
}

/// The callbacks that write the time setting: Now, the tracks and exact
/// entry.
fn register_setting(window: &MainWindow, link: &EngineLink) {
    let weak = window.as_weak();
    let engine = link.clone();
    window.on_time_now(move || {
        if let Some(win) = weak.upgrade() {
            leave_eclipse(&win);
            let mut settings = win.get_datetime();
            settings.use_custom = false;
            settings.instant = slint::SharedString::new();
            win.set_datetime(settings);
            ui_callbacks::update_datetime_labels(&win);
            engine.push_params(&win);
            engine.send(EngineCommand::Excursion(ExcursionCommand::End));
            refresh(&win);
        }
    });

    let engine = link.clone();
    window.on_time_end(move || {
        engine.send(EngineCommand::Excursion(ExcursionCommand::End));
    });

    let weak = window.as_weak();
    let engine = link.clone();
    window.on_time_track(move |year_track, fraction| {
        if let Some(win) = weak.upgrade() {
            let at = track_instant(&win, year_track, fraction);
            set_time(&win, &engine, at);
        }
    });

    let weak = window.as_weak();
    let engine = link.clone();
    window.on_time_exact_date(move |year, month, day| {
        let Some(win) = weak.upgrade() else {
            return;
        };
        let date = u8::try_from(month)
            .ok()
            .and_then(|month| time::Month::try_from(month).ok())
            .zip(u8::try_from(day).ok())
            .and_then(|(month, day)| time::Date::from_calendar_date(year, month, day).ok());
        let Some(date) = date else {
            return;
        };
        let utc = win.get_time_zone_utc();
        let offset_at = |at| display_offset(utc, at);
        let (hour, _, _) = sliders_of(local_on_screen(&win, OffsetDateTime::now_utc()), offset_at);
        let at = instant_of_sliders(hour, f32::from(date.ordinal()), date.year(), offset_at);
        set_time(&win, &engine, at);
    });

    let weak = window.as_weak();
    let engine = link.clone();
    window.on_time_exact_time(move |hour, minute| {
        let Some(win) = weak.upgrade() else {
            return;
        };
        let utc = win.get_time_zone_utc();
        let local = local_on_screen(&win, OffsetDateTime::now_utc());
        #[expect(clippy::cast_precision_loss, reason = "a time of day")]
        let hour = (hour.clamp(0, 23) * 60 + minute.clamp(0, 59)) as f32 / 60.0;
        let at = instant_of_sliders(hour, f32::from(local.ordinal()), local.year(), |at| {
            display_offset(utc, at)
        });
        set_time(&win, &engine, at);
    });
}

/// The zone chip, the label tick and the date track's hover label.
fn register_labels(window: &MainWindow) {
    let weak = window.as_weak();
    window.on_time_zone_toggled(move || {
        if let Some(win) = weak.upgrade() {
            win.set_time_zone_utc(!win.get_time_zone_utc());
            ui_callbacks::refresh_datetime_sliders(&win);
            refresh(&win);
        }
    });

    let weak = window.as_weak();
    window.on_time_tick(move || {
        if let Some(win) = weak.upgrade() {
            refresh(&win);
        }
    });

    let weak = window.as_weak();
    window.on_time_date_at(move |fraction| {
        let Some(win) = weak.upgrade() else {
            return slint::SharedString::new();
        };
        let local = local_on_screen(&win, OffsetDateTime::now_utc());
        let days = datetime::days_in_year(local.year());
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a day of one year"
        )]
        let day = (fraction.clamp(0.0, 0.99999) * f32::from(days)).floor() as u16;
        time::Date::from_ordinal_date(local.year(), day + 1)
            .map(|date| day_and_month(date).into())
            .unwrap_or_default()
    });
}

// ---------------------------------------------------------------------------
// Eclipse mode
// ---------------------------------------------------------------------------

/// Fourteen days: under half the shortest gap between two solar eclipses of
/// 1900 to 2100, 29.3 days, so an eclipse whose peak is this near an instant
/// is the eclipse nearest it.
const NEAREST_REACH: time::Duration = time::Duration::days(14);

/// How near a season an eclipse's mark has to be, in days, for its label to
/// name the season too: a glyph's width on the date track of the widest bar.
const SEASON_NEIGHBOR_DAYS: f64 = 6.0;

/// The eclipse the mode is on and its neighbors.
#[derive(Clone)]
struct Current {
    eclipse: Eclipse,
    previous: Option<Eclipse>,
    next: Option<Eclipse>,
}

thread_local! {
    /// The eclipse nearest the time setting, which is the one the mode is on,
    /// kept while the setting stays within `NEAREST_REACH` of its peak, which
    /// nothing that keeps the mode can move it past.
    static CURRENT: RefCell<Option<Current>> = const { RefCell::new(None) };
    /// The displayed year's eclipses in the order of the date track's marks,
    /// under the key of the year they were found for.
    static YEAR_ECLIPSES: RefCell<(String, Vec<Eclipse>)> = const { RefCell::new((String::new(), Vec::new())) };
}

/// The eclipse the mode is on, the one nearest the time setting `setting`.
fn current_eclipse(setting: OffsetDateTime) -> Option<Current> {
    CURRENT.with(|cell| {
        let mut cell = cell.borrow_mut();
        let held = cell
            .as_ref()
            .is_some_and(|current| (current.eclipse.peak - setting).abs() <= NEAREST_REACH);
        if !held {
            *cell = eclipse::nearest(setting).map(|eclipse| Current {
                previous: eclipse::previous_before(eclipse.peak),
                next: eclipse::next_after(eclipse.peak),
                eclipse,
            });
        }
        cell.clone()
    })
}

fn kind_index(kind: EclipseKind) -> i32 {
    match kind {
        EclipseKind::Partial => 0,
        EclipseKind::Annular => 1,
        EclipseKind::Total => 2,
    }
}

fn kind_word(kind: EclipseKind) -> &'static str {
    match kind {
        EclipseKind::Partial => "Partial",
        EclipseKind::Annular => "Annular",
        EclipseKind::Total => "Total",
    }
}

/// "Total solar eclipse".
fn eclipse_title(kind: EclipseKind) -> String {
    format!("{} solar eclipse", kind_word(kind))
}

/// "65° N".
fn latitude_text(latitude: f32) -> String {
    let hemisphere = if latitude < 0.0 { "S" } else { "N" };
    format!("{:.0}° {hemisphere}", latitude.abs())
}

/// "93%".
fn share_text(share: f32) -> String {
    format!("{:.0}%", (share * 100.0).clamp(0.0, 100.0))
}

/// "1 h 12 min", "20 min", "3 days".
fn span_text(span: time::Duration) -> String {
    let minutes = (span.abs().as_seconds_f64() / 60.0).round();
    if minutes < 60.0 {
        format!("{minutes:.0} min")
    } else if minutes < 48.0 * 60.0 {
        let hours = (minutes / 60.0).floor();
        let rest = minutes - hours * 60.0;
        if rest == 0.0 {
            format!("{hours:.0} h")
        } else {
            format!("{hours:.0} h {rest:.0} min")
        }
    } else {
        format!("{:.0} days", (minutes / 1440.0).round())
    }
}

/// "Partial, Sat 29 Mar 2025", in the displayed zone.
fn neighbor_text(eclipse: Option<&Eclipse>, utc: bool) -> String {
    eclipse.map_or_else(String::new, |eclipse| {
        let local = eclipse.peak.to_offset(display_offset(utc, eclipse.peak));
        format!(
            "{}, {} {}",
            kind_word(eclipse.kind),
            datetime::day_label(local.date()),
            local.year()
        )
    })
}

/// The eclipse's contacts and peak in order, with what the panel calls them
/// and the mark of each: 0 the penumbra's, 1 the umbra's, 2 the peak.
fn phases_of(eclipse: &Eclipse) -> Vec<(OffsetDateTime, String, i32)> {
    let central = match eclipse.kind {
        EclipseKind::Annular => "Annular",
        _ => "Total",
    };
    let contacts = eclipse.contacts;
    let mut phases = vec![(contacts.p1, "Partial begins".to_owned(), 0)];
    if let Some(u1) = contacts.u1 {
        phases.push((u1, format!("{central} begins"), 1));
    }
    phases.push((eclipse.peak, "Peak".to_owned(), 2));
    if let Some(u4) = contacts.u4 {
        phases.push((u4, format!("{central} ends"), 1));
    }
    phases.push((contacts.p4, "Partial ends".to_owned(), 0));
    phases
}

/// What the panel says about the eclipse at its greatest point.
fn fact_text(eclipse: &Eclipse) -> String {
    let greatest = eclipse.greatest;
    match (eclipse.kind, eclipse.central_duration) {
        (EclipseKind::Partial, _) | (_, None) => format!(
            "{} of the Sun covered at most, at {}",
            share_text(greatest.covered),
            latitude_text(greatest.latitude)
        ),
        (kind, Some(duration)) => {
            let seconds = duration.as_seconds_f64().round();
            format!(
                "{} {:.0} min {:.0} s at greatest eclipse",
                if kind == EclipseKind::Annular {
                    "Annularity"
                } else {
                    "Totality"
                },
                (seconds / 60.0).floor(),
                seconds % 60.0
            )
        }
    }
}

/// What the instant on screen shows of the shadow.
fn now_text(eclipse: &Eclipse, shown: OffsetDateTime) -> String {
    if shown < eclipse.contacts.p1 {
        format!(
            "Shadow reaches Earth in {}",
            span_text(eclipse.contacts.p1 - shown)
        )
    } else if shown > eclipse.contacts.p4 {
        format!(
            "Shadow left Earth {} ago",
            span_text(shown - eclipse.contacts.p4)
        )
    } else {
        let shadow = sky::shadow_at(shown);
        format!(
            "Now: Sun {} covered at {}",
            share_text(eclipse::covered_at_center(&shadow)),
            latitude_text(eclipse::shadow_center(&shadow).0)
        )
    }
}

/// `rows` in `held`, the model the panel shows, where it has as many rows,
/// each written only where it changed: a new model would make the panel's
/// repeater destroy every row on each frame of a run, and a press or a hover
/// on one with it.
fn phase_rows(
    held: slint::ModelRc<EclipsePhase>,
    rows: Vec<EclipsePhase>,
) -> slint::ModelRc<EclipsePhase> {
    if held.row_count() != rows.len() {
        return slint::ModelRc::new(slint::VecModel::from(rows));
    }
    for (index, row) in rows.into_iter().enumerate() {
        if held.row_data(index).as_ref() != Some(&row) {
            held.set_row_data(index, row);
        }
    }
    held
}

fn panel_view(
    current: &Current,
    shown: OffsetDateTime,
    utc: bool,
    held: slint::ModelRc<EclipsePhase>,
) -> EclipsePanelView {
    let eclipse = &current.eclipse;
    let local = |at: OffsetDateTime| at.to_offset(display_offset(utc, at));
    let peak = local(eclipse.peak);
    let phases = phases_of(eclipse);
    let on_earth = eclipse.on_earth_at(shown);
    let row = phases
        .iter()
        .rposition(|(at, _, _)| *at <= shown)
        .filter(|_| on_earth)
        .and_then(|row| i32::try_from(row).ok())
        .unwrap_or(-1);
    EclipsePanelView {
        kind: kind_index(eclipse.kind),
        title: eclipse_title(eclipse.kind).into(),
        date_text: format!(
            "{} {} \u{b7} {}",
            datetime::day_label(peak.date()),
            peak.year(),
            datetime::zone_label(peak.offset())
        )
        .into(),
        phases: phase_rows(
            held,
            phases
                .iter()
                .map(|(at, label, glyph)| EclipsePhase {
                    label: label.as_str().into(),
                    time: clock_text(local(*at)).into(),
                    glyph: *glyph,
                })
                .collect(),
        ),
        current: row,
        fact: fact_text(eclipse).into(),
        now_text: now_text(eclipse, shown).into(),
        has_previous: current.previous.is_some(),
        has_next: current.next.is_some(),
    }
}

/// The stretches of the day on screen the shadow is on the Earth, and where
/// the peak falls along it, -1 outside it.
fn eclipse_bands(eclipse: &Eclipse, local: OffsetDateTime, utc: bool) -> (Vec<EclipseBand>, f32) {
    let offset_at = |at| display_offset(utc, at);
    let day_start = instant_of_sliders(0.0, f32::from(local.ordinal()), local.year(), offset_at);
    let day_end = local
        .date()
        .next_day()
        .map_or(day_start + time::Duration::DAY, |next| {
            instant_of_sliders(0.0, f32::from(next.ordinal()), next.year(), offset_at)
        });
    let length = (day_end - day_start).as_seconds_f64();
    #[expect(clippy::cast_possible_truncation, reason = "a fraction of one day")]
    let along = |at: OffsetDateTime| ((at - day_start).as_seconds_f64() / length) as f32;
    let band = |from: OffsetDateTime, to: OffsetDateTime, umbra: bool| {
        let (start, end) = (along(from), along(to));
        (end > 0.0 && start < 1.0).then(|| EclipseBand {
            start: start.max(0.0),
            end: end.min(1.0),
            umbra,
            cut_start: start < 0.0,
            cut_end: end > 1.0,
        })
    };
    let contacts = eclipse.contacts;
    let bands = [
        band(contacts.p1, contacts.p4, false),
        contacts
            .u1
            .zip(contacts.u4)
            .and_then(|(u1, u4)| band(u1, u4, true)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let peak = along(eclipse.peak);
    (
        bands,
        if (0.0..=1.0).contains(&peak) {
            peak
        } else {
            -1.0
        },
    )
}

/// The date track's marks for `eclipses`, in the displayed zone, a mark near
/// a season naming it too.
fn eclipse_marks(eclipses: &[Eclipse], seasons: &[OffsetDateTime], utc: bool) -> Vec<EclipseMark> {
    eclipses
        .iter()
        .map(|eclipse| {
            let at = eclipse.peak.to_offset(display_offset(utc, eclipse.peak));
            let mut label = format!(
                "{} \u{b7} {} {}",
                eclipse_title(eclipse.kind),
                datetime::day_label(at.date()),
                clock_text(at)
            );
            if let Some((_, name)) = seasons.iter().zip(SEASON_NAMES).find(|(season, _)| {
                (**season - eclipse.peak).abs().as_seconds_f64() < SEASON_NEIGHBOR_DAYS * 86_400.0
            }) {
                label.push_str(" \u{b7} ");
                label.push_str(name);
            }
            EclipseMark {
                fraction: year_fraction(at),
                kind: kind_index(eclipse.kind),
                label: label.into(),
            }
        })
        .collect()
}

/// The time setting's instant, which is what the mode's eclipse is nearest.
fn setting_instant(window: &MainWindow, now: OffsetDateTime) -> OffsetDateTime {
    ui_callbacks::scene_time_of(&window.get_datetime()).instant(now)
}

/// The panel, the bands, the hints and the sidebar's line, for the eclipse
/// the mode is on; the bands and the view's peak cleared with the mode off.
fn refresh_eclipse(window: &MainWindow, view: &mut TimeBarView, follow: bool, now: OffsetDateTime) {
    let current = follow
        .then(|| current_eclipse(setting_instant(window, now)))
        .flatten();
    view.eclipse_follow = follow;
    let Some(current) = current else {
        view.eclipse_peak = -1.0;
        if window.get_time_eclipse_bands().row_count() > 0 {
            window.set_time_eclipse_bands(slint::ModelRc::default());
        }
        return;
    };
    let utc = window.get_time_zone_utc();
    let shown = on_screen(window, now);
    let local = shown.to_offset(display_offset(utc, shown));
    let (bands, peak) = eclipse_bands(&current.eclipse, local, utc);
    view.eclipse_kind = kind_index(current.eclipse.kind);
    view.eclipse_peak = peak;
    if !window
        .get_time_eclipse_bands()
        .iter()
        .eq(bands.iter().cloned())
    {
        window.set_time_eclipse_bands(slint::ModelRc::new(slint::VecModel::from(bands)));
    }
    let held = window.get_time_eclipse_panel().phases;
    window.set_time_eclipse_panel(panel_view(&current, shown, utc, held));
    let peak_local = current
        .eclipse
        .peak
        .to_offset(display_offset(utc, current.eclipse.peak));
    window.set_eclipse_follow_label(
        format!(
            "Following the solar eclipse of {} {}",
            datetime::day_label(peak_local.date()),
            peak_local.year()
        )
        .into(),
    );
    let hints = window.global::<OverlayHints>();
    hints.set_eclipse_previous(neighbor_text(current.previous.as_ref(), utc).into());
    hints.set_eclipse_next(neighbor_text(current.next.as_ref(), utc).into());
}

/// Turn eclipse mode on at `eclipse`: its peak becomes the time setting,
/// which ends an excursion, and the camera follows the shadow from there.
pub fn enter_eclipse(window: &MainWindow, link: &EngineLink, eclipse: &Eclipse) {
    let mut position = window.get_camera_position();
    position.eclipse_follow = true;
    window.set_camera_position(position);
    write_time(window, link, eclipse.peak);
}

/// Turn eclipse mode off, leaving the camera where it is: the position the
/// mode resolved at the instant on screen is written into the settings the
/// camera's mode reads, the latitude and the longitude, or the time of day
/// in the Sun-fixed mode. The caller pushes the scene.
pub fn leave_eclipse(window: &MainWindow) {
    leave_eclipse_as(window, window.get_camera_position().sun_fixed);
}

/// `leave_eclipse` writing the field the mode `sun_fixed` reads, for the
/// Sun-fixed switch, whose callback runs once the switch has already moved.
pub(crate) fn leave_eclipse_as(window: &MainWindow, sun_fixed: bool) {
    let mut position = window.get_camera_position();
    if !position.eclipse_follow {
        return;
    }
    let at = on_screen(window, OffsetDateTime::now_utc());
    let (latitude, longitude) = eclipse::followed_point(&sky::shadow_at(at));
    position.latitude = latitude;
    if sun_fixed {
        position.solar_time = camera::solar_time_of(longitude, sky::sun_direction_at(at));
    } else {
        position.longitude = longitude;
    }
    position.eclipse_follow = false;
    window.set_camera_position(position);
    window.set_time_shading_key(slint::SharedString::new());
    window.set_time_eclipse_bands(slint::ModelRc::default());
}

/// Whether `a` and `b` fall on different days in the displayed zone.
fn different_days(window: &MainWindow, a: OffsetDateTime, b: OffsetDateTime) -> bool {
    let utc = window.get_time_zone_utc();
    a.to_offset(display_offset(utc, a)).date() != b.to_offset(display_offset(utc, b)).date()
}

/// Leave eclipse mode when a write of the time setting to `at` moves the day
/// on screen, which is the day the mode's eclipse is on.
pub(crate) fn leave_eclipse_for_a_new_day(window: &MainWindow, at: OffsetDateTime) {
    if window.get_camera_position().eclipse_follow
        && different_days(window, on_screen(window, OffsetDateTime::now_utc()), at)
    {
        leave_eclipse(window);
    }
}

/// The callbacks of eclipse mode: the button, previous and next, the date
/// track's marks and the panel's phase rows.
fn register_eclipse(window: &MainWindow, link: &EngineLink) {
    let weak = window.as_weak();
    let engine = link.clone();
    window.on_time_eclipse_toggled(move || {
        let Some(win) = weak.upgrade() else {
            return;
        };
        if win.get_camera_position().eclipse_follow {
            leave_eclipse(&win);
            engine.push_params(&win);
            return;
        }
        let at = on_screen(&win, OffsetDateTime::now_utc());
        if let Some(found) = eclipse::on_earth_at(at).or_else(|| eclipse::next_after(at)) {
            enter_eclipse(&win, &engine, &found);
        }
    });

    let weak = window.as_weak();
    let engine = link.clone();
    window.on_time_eclipse_step(move |forward| {
        let Some(win) = weak.upgrade() else {
            return;
        };
        let now = OffsetDateTime::now_utc();
        let target = if win.get_camera_position().eclipse_follow {
            current_eclipse(setting_instant(&win, now)).and_then(|current| {
                if forward {
                    current.next
                } else {
                    current.previous
                }
            })
        } else {
            let at = on_screen(&win, now);
            if forward {
                eclipse::next_after(at)
            } else {
                eclipse::previous_before(at)
            }
        };
        if let Some(found) = target {
            enter_eclipse(&win, &engine, &found);
        }
    });

    let weak = window.as_weak();
    let engine = link.clone();
    window.on_time_eclipse_picked(move |index| {
        let Some(win) = weak.upgrade() else {
            return;
        };
        let picked = YEAR_ECLIPSES.with(|year| {
            usize::try_from(index)
                .ok()
                .and_then(|index| year.borrow().1.get(index).copied())
        });
        if let Some(found) = picked {
            enter_eclipse(&win, &engine, &found);
        }
    });

    let weak = window.as_weak();
    let engine = link.clone();
    window.on_time_eclipse_phase(move |index| {
        let Some(win) = weak.upgrade() else {
            return;
        };
        let at =
            current_eclipse(setting_instant(&win, OffsetDateTime::now_utc())).and_then(|current| {
                let phases = phases_of(&current.eclipse);
                usize::try_from(index)
                    .ok()
                    .and_then(|index| phases.get(index).map(|phase| phase.0))
            });
        if let Some(at) = at {
            write_time(&win, &engine, at);
        }
    });
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn clock(rate: i32, running: bool) -> Transport {
        Transport {
            kind: ExcursionKind::Clock,
            rate,
            running,
        }
    }

    #[expect(clippy::unnecessary_wraps, reason = "what `act` answers")]
    fn play(rate: i32, kind: ExcursionKind) -> Option<ExcursionCommand> {
        Some(ExcursionCommand::Play { rate, kind })
    }

    #[test]
    fn play_toggles_and_pause_only_pauses_a_run() {
        let paused = clock(60, false);
        assert_eq!(paused.act(Action::Toggle).1, play(60, ExcursionKind::Clock));
        assert_eq!(paused.act(Action::Pause).1, None);
        let running = clock(60, true);
        assert_eq!(running.act(Action::Toggle).1, Some(ExcursionCommand::Pause));
        assert_eq!(running.act(Action::Pause).1, Some(ExcursionCommand::Pause));
    }

    /// J and L start in their direction from a pause, go faster along it up
    /// to the ladder's end, and against it slower and then stop.
    #[test]
    fn the_shuttle_starts_speeds_up_slows_down_and_stops() {
        let (next, command) = clock(60, false).act(Action::Shuttle(-1));
        assert_eq!((next.rate, command), (-60, play(-60, ExcursionKind::Clock)));

        let ladder = ExcursionKind::Clock.ladder();
        let fastest = i32::try_from(ladder[ladder.len() - 1]).expect("a rate");
        let (next, _) = clock(60, true).act(Action::Shuttle(1));
        assert_eq!(next.rate, 120);
        let (next, _) = clock(fastest, true).act(Action::Shuttle(1));
        assert_eq!(next.rate, fastest, "the fastest stays the fastest");

        let (next, _) = clock(60, true).act(Action::Shuttle(-1));
        assert_eq!(next.rate, 30);
        let slowest = i32::try_from(ladder[0]).expect("a rate");
        let (next, command) = clock(slowest, true).act(Action::Shuttle(-1));
        assert_eq!(
            (next.rate, command),
            (slowest, Some(ExcursionCommand::Pause))
        );
    }

    /// A shuttle step changes something everywhere but at the end of the
    /// ladder it goes along: starting, a faster or slower speed, and the
    /// stop below the slowest all count.
    #[test]
    fn only_the_shuttle_at_the_ladders_end_changes_nothing() {
        let ladder = ExcursionKind::Clock.ladder();
        let fastest = i32::try_from(ladder[ladder.len() - 1]).expect("a rate");
        let slowest = i32::try_from(ladder[0]).expect("a rate");
        assert!(clock(60, false).changes(Action::Shuttle(1)));
        assert!(clock(fastest, false).changes(Action::Shuttle(1)));
        assert!(clock(60, true).changes(Action::Shuttle(1)));
        assert!(clock(60, true).changes(Action::Shuttle(-1)));
        assert!(clock(slowest, true).changes(Action::Shuttle(-1)));
        assert!(!clock(fastest, true).changes(Action::Shuttle(1)));
        assert!(!clock(-fastest, true).changes(Action::Shuttle(-1)));
    }

    #[test]
    fn reverse_keeps_the_speed_and_tells_the_engine_only_while_running() {
        assert_eq!(
            clock(300, false).act(Action::Reverse),
            (clock(-300, false), None)
        );
        assert_eq!(
            clock(300, true).act(Action::Reverse),
            (clock(-300, true), play(-300, ExcursionKind::Clock))
        );
    }

    /// A radio label switches to the other kind's default rate in the same
    /// direction, and a run goes on in it.
    #[test]
    fn a_kind_switch_keeps_the_direction_at_the_kinds_default() {
        let date = default_rate(ExcursionKind::Date);
        let (next, command) = clock(-300, true).act(Action::Kind(ExcursionKind::Date));
        assert_eq!((next.kind, next.rate), (ExcursionKind::Date, -date));
        assert_eq!(command, play(-date, ExcursionKind::Date));
        assert_eq!(
            clock(60, false).act(Action::Kind(ExcursionKind::Date)).1,
            None
        );
        assert_eq!(
            clock(60, true).act(Action::Kind(ExcursionKind::Clock)).1,
            None
        );
    }

    #[test]
    fn offsets_name_the_two_largest_units() {
        let text = |duration| offset_text(duration);
        assert_eq!(text(time::Duration::seconds(59)), "now");
        assert_eq!(text(time::Duration::seconds(-59)), "now");
        assert_eq!(text(time::Duration::minutes(5)), "+5 min");
        assert_eq!(text(time::Duration::minutes(-65)), "\u{2212}1 h 5 min");
        assert_eq!(
            text(-(time::Duration::days(2) + time::Duration::hours(3))),
            "\u{2212}2 d 3 h"
        );
        assert_eq!(text(time::Duration::days(365 * 3 + 40)), "+3 y 39 d");
    }

    #[test]
    fn zone_labels_name_whole_and_half_hour_offsets() {
        let label = |hours, minutes| {
            datetime::zone_label(time::UtcOffset::from_hms(hours, minutes, 0).expect("an offset"))
        };
        assert_eq!(label(0, 0), "UTC");
        assert_eq!(label(2, 0), "UTC+2");
        assert_eq!(label(5, 30), "UTC+5:30");
        assert_eq!(label(-3, -30), "UTC\u{2212}3:30");
    }

    /// Every rate of both ladders has a label and a meaning of its own, and
    /// going backward adds the sign and nothing else.
    #[test]
    fn every_rate_has_its_own_speed_text() {
        for kind in [ExcursionKind::Clock, ExcursionKind::Date] {
            let labels: Vec<String> = kind
                .ladder()
                .iter()
                .map(|&rate| rate_label(i32::try_from(rate).expect("a rate"), kind))
                .collect();
            let mut distinct = labels.clone();
            distinct.sort();
            distinct.dedup();
            assert_eq!(distinct.len(), labels.len(), "{labels:?}");
            for (label, &rate) in labels.iter().zip(kind.ladder()) {
                let rate = i32::try_from(rate).expect("a rate");
                assert_eq!(rate_label(-rate, kind), format!("{MINUS}{label}"));
                assert!(!rate_note(rate, kind).is_empty());
            }
        }
        let notes: Vec<String> = ExcursionKind::Clock
            .ladder()
            .iter()
            .map(|&rate| rate_note(i32::try_from(rate).expect("a rate"), ExcursionKind::Clock))
            .collect();
        let mut distinct = notes.clone();
        distinct.dedup();
        assert_eq!(distinct.len(), notes.len(), "{notes:?}");
        assert_eq!(rate_label(300, ExcursionKind::Clock), "300\u{d7}");
        assert_eq!(rate_note(300, ExcursionKind::Clock), "5 min/s");
        assert_eq!(rate_label(7, ExcursionKind::Date), "1 week/s");
        assert_eq!(rate_label(90, ExcursionKind::Date), "3 months/s");
        assert_eq!(rate_label(1, ExcursionKind::Date), "1 day/s");
    }

    #[test]
    fn the_state_is_live_an_offset_a_run_or_preview() {
        assert_eq!(state_text(true, None, ""), "LIVE");
        assert_eq!(state_text(false, None, "+5 min"), "+5 min");
        let run = ExcursionState {
            running: true,
            rate: -3600,
            kind: ExcursionKind::Clock,
        };
        assert_eq!(
            state_text(false, Some(run), "+5 min"),
            "\u{25c0} 3600\u{d7}"
        );
        let paused = ExcursionState {
            running: false,
            ..run
        };
        assert_eq!(state_text(false, Some(paused), "+5 min"), "Preview");
    }

    /// Noon at the equator on an equinox is the day's color, and midnight the
    /// night's; the colors move from one to the other.
    #[test]
    fn the_shading_is_day_at_noon_and_night_at_midnight() {
        let day = shading(datetime!(2026-03-20 00:00 UTC), 0.0, 0.0);
        assert_eq!(day[0], ALTITUDE_COLORS[0].1);
        assert_eq!(
            day[SHADING_SAMPLES / 2],
            ALTITUDE_COLORS[ALTITUDE_COLORS.len() - 1].1
        );
        assert_eq!(altitude_color(-90.0), ALTITUDE_COLORS[0].1);
    }

    #[test]
    fn the_date_track_snaps_to_a_season_within_three_days() {
        let seasons = [78, 171, 265, 354];
        assert_eq!(snap_to_season(81, &seasons), 78);
        assert_eq!(snap_to_season(82, &seasons), 82);
        assert_eq!(snap_to_season(352, &seasons), 354);
    }
}
