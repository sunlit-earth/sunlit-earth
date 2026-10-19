//! Pure math behind the camera panel on the preview and the keys that do the
//! same.
//!
//! The joysticks are rate controls: a deflection sets a speed and the camera
//! moves by that speed times the time since the previous tick, so a late tick
//! moves it further rather than slowing it down. All of them move the camera
//! the way they are pushed, which slides the picture the other way. The
//! callbacks in `ui_callbacks` read the window, call these and write it back.

use sunlit_core::scene::camera::zoom_to_distance;

use crate::mouse_math::{apply_globe_drag_at, wrap_angle_180};

/// How fast a full deflection of the orbit stick turns the globe at the
/// default distance of 8 Earth radii, in degrees a second.
const ORBIT_RATE_AT_DEFAULT: f32 = 70.0;

/// The fastest the orbit stick turns the globe, in degrees a second, which a
/// far camera would otherwise exceed.
const ORBIT_RATE_MAX: f32 = 120.0;

/// How fast a full deflection of the look stick swings yaw and pitch, in
/// degrees a second.
const LOOK_RATE: f32 = 45.0;

/// How fast a full deflection of the frame stick moves the offsets, in offset
/// units a second.
const FRAME_RATE: f32 = 1.4;

/// How far one orbit arrow steps the camera, in degrees.
const ORBIT_STEP: f32 = 10.0;

/// How far one click of a zoom button moves the zoom slider.
const ZOOM_STEP: f32 = 0.05;

/// How fast a held zoom button moves the zoom slider, a second.
const ZOOM_RATE: f32 = 0.3;

/// How long an ease takes from its start to its target, in seconds.
pub const EASE_SECONDS: f32 = 0.3;

/// The offsets' range on either side of zero, the sidebar's.
const OFFSET_LIMIT: f32 = 3.0;

/// Yaw's and pitch's range on either side of zero, the sidebar's.
const AIM_LIMIT: f32 = 90.0;

/// A stick's deflection as the rate it asks for.
///
/// The deflection is clamped to the unit disc, and its length squared sets the
/// speed, so the first part of the knob's travel is fine control and the rim
/// is full speed.
///
/// :param x: the deflection to the right, -1 to 1
/// :param y: the deflection upward, -1 to 1
/// :returns: the rate along each axis, each within -1 to 1
#[must_use]
pub fn stick_rate(x: f32, y: f32) -> (f32, f32) {
    let length = x.hypot(y);
    if length <= f32::EPSILON {
        return (0.0, 0.0);
    }
    let clamped = length.min(1.0);
    let scale = clamped * clamped / length;
    (x * scale, y * scale)
}

/// How fast the orbit stick turns the globe at full deflection, in degrees a
/// second: in proportion to the camera's distance, as the drag on the globe
/// is, so the surface passes at the same pace at every zoom, up to a cap.
///
/// :param zoom: the normalized zoom, 0 to 1
/// :returns: degrees a second
#[must_use]
pub fn orbit_rate(zoom: f32) -> f32 {
    (ORBIT_RATE_AT_DEFAULT * zoom_to_distance(zoom) / 8.0).min(ORBIT_RATE_MAX)
}

/// Move the camera over the globe by one tick of the orbit stick.
///
/// The longitude is the one the drag measures in: the camera's own, or in the
/// Sun-fixed mode the held longitude its time of day stands for.
///
/// :param lon: the held longitude in degrees
/// :param lat: the latitude in degrees
/// :param tilt: the roll in degrees, which turns the stick's axes with the picture
/// :param zoom: the normalized zoom
/// :param x: the deflection to the right
/// :param y: the deflection upward
/// :param seconds: the time since the previous tick
/// :returns: the new `(longitude, latitude)`
#[must_use]
pub fn orbit_by_stick(
    lon: f32,
    lat: f32,
    tilt: f32,
    zoom: f32,
    x: f32,
    y: f32,
    seconds: f32,
) -> (f32, f32) {
    let (rx, ry) = stick_rate(x, y);
    apply_globe_drag_at(lon, lat, tilt, -rx, ry, orbit_rate(zoom) * seconds)
}

/// Where one orbit arrow takes the camera.
///
/// :param lon: the held longitude in degrees
/// :param lat: the latitude in degrees
/// :param tilt: the roll in degrees
/// :param right: 1 for the right arrow, -1 for the left, 0 for neither
/// :param up: 1 for the up arrow, -1 for the down, 0 for neither
/// :returns: the target `(longitude, latitude)`
#[must_use]
pub fn orbit_step(lon: f32, lat: f32, tilt: f32, right: i32, up: i32) -> (f32, f32) {
    apply_globe_drag_at(lon, lat, tilt, -unit(right), unit(up), ORBIT_STEP)
}

/// Swing yaw and pitch by one tick of the look stick.
///
/// :param yaw: the yaw in degrees
/// :param pitch: the pitch in degrees
/// :param x: the deflection to the right
/// :param y: the deflection upward
/// :param seconds: the time since the previous tick
/// :returns: the new `(yaw, pitch)`, each within the sidebar's range
#[must_use]
pub fn look_by_stick(yaw: f32, pitch: f32, x: f32, y: f32, seconds: f32) -> (f32, f32) {
    let (rx, ry) = stick_rate(x, y);
    (
        (yaw + rx * LOOK_RATE * seconds).clamp(-AIM_LIMIT, AIM_LIMIT),
        (pitch + ry * LOOK_RATE * seconds).clamp(-AIM_LIMIT, AIM_LIMIT),
    )
}

/// Move the frame by one tick of the frame stick.
///
/// :param `offset_x`: the horizontal offset
/// :param `offset_y`: the vertical offset
/// :param x: the deflection to the right
/// :param y: the deflection upward
/// :param seconds: the time since the previous tick
/// :returns: the new `(offset_x, offset_y)`, each within the sidebar's range
#[must_use]
pub fn frame_by_stick(offset_x: f32, offset_y: f32, x: f32, y: f32, seconds: f32) -> (f32, f32) {
    let (rx, ry) = stick_rate(x, y);
    (
        (offset_x + rx * FRAME_RATE * seconds).clamp(-OFFSET_LIMIT, OFFSET_LIMIT),
        (offset_y + ry * FRAME_RATE * seconds).clamp(-OFFSET_LIMIT, OFFSET_LIMIT),
    )
}

/// Where one click of a zoom button takes the zoom.
///
/// :param zoom: the normalized zoom
/// :param direction: 1 to zoom in, toward the Earth, -1 to zoom out
/// :returns: the target zoom, within 0 to 1
#[must_use]
pub fn zoom_step(zoom: f32, direction: i32) -> f32 {
    (zoom - unit(direction) * ZOOM_STEP).clamp(0.0, 1.0)
}

/// Move the zoom by one tick of a held zoom button or key.
///
/// :param zoom: the normalized zoom
/// :param direction: 1 to zoom in, -1 to zoom out
/// :param seconds: the time since the previous tick
/// :returns: the new zoom, within 0 to 1
#[must_use]
pub fn zoom_by_rate(zoom: f32, direction: i32, seconds: f32) -> f32 {
    (zoom - unit(direction) * ZOOM_RATE * seconds).clamp(0.0, 1.0)
}

/// Roll the picture.
///
/// :param tilt: the roll in degrees
/// :param degrees: how far to turn it, positive clockwise on screen
/// :returns: the new roll, wrapped into `[-180, 180)`
#[must_use]
pub fn roll_by(tilt: f32, degrees: f32) -> f32 {
    wrap_angle_180(tilt + degrees)
}

/// The sign of a direction as a float, so a caller passing 2 or -5 still means
/// one step.
#[expect(
    clippy::cast_precision_loss,
    reason = "the value is -1, 0 or 1 after signum"
)]
fn unit(direction: i32) -> f32 {
    direction.signum() as f32
}

/// A camera value an ease can move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EaseField {
    /// The held longitude: the camera's own, or what the Sun-fixed mode's
    /// time of day stands for.
    Longitude,
    Latitude,
    Zoom,
    Tilt,
    Yaw,
    Pitch,
    OffsetX,
    OffsetY,
}

impl EaseField {
    /// Whether the field is an angle that wraps at ±180°, so an ease takes
    /// the short way round.
    fn wraps(self) -> bool {
        matches!(self, Self::Longitude | Self::Tilt)
    }
}

#[derive(Clone, Copy, Debug)]
struct Run {
    field: EaseField,
    from: f32,
    to: f32,
    elapsed: f32,
}

/// The camera values on their way to a target: N, the ↺ badges, the orbit
/// arrows and the zoom buttons ease rather than jump, so the globe visibly
/// turns back.
///
/// One run per field: a new target for a field replaces the run it had.
#[derive(Clone, Debug, Default)]
pub struct Eases {
    runs: Vec<Run>,
}

impl Eases {
    /// Start easing a field from where it is to a target.
    ///
    /// :param field: the value to move
    /// :param from: where it is now
    /// :param to: where it should end
    pub fn start(&mut self, field: EaseField, from: f32, to: f32) {
        self.runs.retain(|run| run.field != field);
        let to = if field.wraps() {
            from + wrap_angle_180(to - from)
        } else {
            to
        };
        self.runs.push(Run {
            field,
            from,
            to,
            elapsed: 0.0,
        });
    }

    /// Stop easing a field, for when something else has taken it over.
    pub fn cancel(&mut self, field: EaseField) {
        self.runs.retain(|run| run.field != field);
    }

    /// Whether any field is still on its way.
    #[must_use]
    pub fn is_running(&self) -> bool {
        !self.runs.is_empty()
    }

    /// Move every run on by a tick and say where each field now is.
    ///
    /// A run that reaches its target gives the target exactly and ends.
    ///
    /// :param seconds: the time since the previous tick
    /// :returns: each moving field with its value, the angles wrapped
    pub fn advance(&mut self, seconds: f32) -> Vec<(EaseField, f32)> {
        let mut values = Vec::with_capacity(self.runs.len());
        for run in &mut self.runs {
            run.elapsed += seconds.max(0.0);
            let progress = (run.elapsed / EASE_SECONDS).min(1.0);
            let value = if progress >= 1.0 {
                run.to
            } else {
                let eased = 1.0 - (1.0 - progress).powi(3);
                run.from + (run.to - run.from) * eased
            };
            let value = if run.field.wraps() {
                wrap_angle_180(value)
            } else {
                value
            };
            values.push((run.field, value));
        }
        self.runs.retain(|run| run.elapsed < EASE_SECONDS);
        values
    }
}

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;
    use sunlit_core::scene::camera::{held_longitude, solar_time_of_held};

    use super::*;

    /// The zoom at which the camera is 8 Earth radii out.
    fn zoom_at_distance_8() -> f32 {
        (8.0_f32 / 1.5).ln() / (80.0_f32 / 1.5).ln()
    }

    #[test]
    fn a_full_deflection_for_one_second_turns_the_globe_by_the_stated_rate() {
        let zoom = zoom_at_distance_8();
        let mut lon = 0.0;
        let lat = 0.0;
        for _ in 0..100 {
            (lon, _) = orbit_by_stick(lon, lat, 0.0, zoom, 1.0, 0.0, 0.01);
        }
        assert_relative_eq!(lon, ORBIT_RATE_AT_DEFAULT, max_relative = 1e-3);
    }

    #[test]
    fn the_orbit_rate_follows_the_distance_up_to_its_cap() {
        let near = orbit_rate(0.1);
        let default = orbit_rate(zoom_at_distance_8());
        let far = orbit_rate(1.0);
        assert!(
            near < default,
            "a near camera turns slower: {near} vs {default}"
        );
        assert_relative_eq!(far, ORBIT_RATE_MAX);
    }

    #[test]
    fn the_rate_grows_with_the_square_of_the_deflection() {
        let (half, _) = stick_rate(0.5, 0.0);
        let (full, _) = stick_rate(1.0, 0.0);
        assert_relative_eq!(half * 4.0, full);
        let (beyond, _) = stick_rate(2.0, 0.0);
        assert_relative_eq!(beyond, full);
        assert_eq!(stick_rate(0.0, 0.0), (0.0, 0.0));
    }

    /// "Up" on the stick moves the camera toward the top of the picture
    /// whatever the roll: at no roll that is north, and with the picture
    /// turned half round it is south.
    #[test]
    fn up_on_the_stick_is_up_on_the_screen_at_any_roll() {
        let zoom = zoom_at_distance_8();
        let (_, upright) = orbit_by_stick(0.0, 0.0, 0.0, zoom, 0.0, 1.0, 0.1);
        assert!(upright > 0.0, "up at no roll goes north, got {upright}");
        let (_, upside_down) = orbit_by_stick(0.0, 0.0, 180.0, zoom, 0.0, 1.0, 0.1);
        assert!(
            upside_down < 0.0,
            "up with the picture upside down goes south, got {upside_down}"
        );
        let (sideways, level) = orbit_by_stick(0.0, 0.0, 90.0, zoom, 0.0, 1.0, 0.1);
        assert!(
            sideways.abs() > 1.0,
            "a quarter roll turns up into east or west"
        );
        assert!(level.abs() < 1e-3, "and leaves the latitude, got {level}");
    }

    #[test]
    fn the_orbit_keeps_the_latitude_off_the_poles_and_wraps_the_longitude() {
        let zoom = 1.0;
        let (_, lat) = orbit_by_stick(0.0, 88.0, 0.0, zoom, 0.0, 1.0, 10.0);
        assert_relative_eq!(lat, 89.0);
        let (lon, _) = orbit_by_stick(175.0, 0.0, 0.0, zoom, 1.0, 0.0, 0.1);
        assert!(
            (-180.0..0.0).contains(&lon),
            "past 180 east wraps to the west: {lon}"
        );
    }

    #[test]
    fn an_orbit_step_moves_ten_degrees_the_way_the_arrow_points() {
        let (east, lat) = orbit_step(0.0, 0.0, 0.0, 1, 0);
        assert_relative_eq!(east, ORBIT_STEP);
        assert_relative_eq!(lat, 0.0);
        let (_, north) = orbit_step(0.0, 0.0, 0.0, 0, 1);
        assert_relative_eq!(north, ORBIT_STEP);
    }

    /// In the Sun-fixed mode the step goes through the held longitude, as the
    /// globe drag does: east is a later time of day, by 40 minutes a step,
    /// and the time wraps across midnight.
    #[test]
    fn in_the_sun_fixed_mode_a_step_east_is_a_later_time_of_day() {
        let solar_time = 23.5;
        let (lon, _) = orbit_step(held_longitude(solar_time), 0.0, 0.0, 1, 0);
        let later = solar_time_of_held(lon);
        assert_relative_eq!(later, solar_time + 40.0 / 60.0 - 24.0, epsilon = 1e-4);
    }

    #[test]
    fn look_and_frame_move_the_way_the_stick_is_pushed_and_stop_at_the_sidebars_range() {
        let (yaw, pitch) = look_by_stick(0.0, 0.0, 1.0, 1.0, 0.1);
        assert!(yaw > 0.0 && pitch > 0.0);
        assert_eq!(look_by_stick(89.0, -89.0, 1.0, -1.0, 10.0), (90.0, -90.0));
        let (x, y) = frame_by_stick(0.0, 0.0, -1.0, 1.0, 0.1);
        assert!(x < 0.0 && y > 0.0);
        assert_eq!(frame_by_stick(2.9, -2.9, 1.0, -1.0, 10.0), (3.0, -3.0));
    }

    #[test]
    fn zoom_in_moves_toward_the_earth_and_both_ends_hold() {
        assert!(zoom_step(0.5, 1) < 0.5);
        assert!(zoom_step(0.5, -1) > 0.5);
        assert_relative_eq!(zoom_step(0.01, 1), 0.0);
        assert_relative_eq!(zoom_by_rate(0.99, -1, 1.0), 1.0);
        assert!(zoom_by_rate(0.5, 1, 0.1) < 0.5);
    }

    #[test]
    fn a_roll_wraps_into_the_half_open_range() {
        assert_relative_eq!(roll_by(170.0, 20.0), -170.0);
        assert_relative_eq!(roll_by(-170.0, -20.0), 170.0);
    }

    #[test]
    fn an_ease_reaches_its_target_exactly_and_ends() {
        let mut eases = Eases::default();
        eases.start(EaseField::Zoom, 0.2, 0.7);
        let mut last = 0.2;
        let mut ticks = 0;
        while eases.is_running() {
            let values = eases.advance(0.016);
            let (field, value) = values[0];
            assert_eq!(field, EaseField::Zoom);
            assert!(
                value >= last,
                "an ease never turns back: {value} after {last}"
            );
            last = value;
            ticks += 1;
            assert!(ticks < 100, "the ease ends");
        }
        assert_eq!(last.to_bits(), 0.7_f32.to_bits());
    }

    #[test]
    fn an_angle_eases_the_short_way_round() {
        let mut eases = Eases::default();
        eases.start(EaseField::Tilt, 170.0, -170.0);
        let values = eases.advance(EASE_SECONDS / 2.0);
        let (_, halfway) = values[0];
        assert!(
            halfway.abs() > 170.0,
            "from 170 to -170 passes through 180, got {halfway}"
        );
        let values = eases.advance(EASE_SECONDS);
        assert_relative_eq!(values[0].1, -170.0);
    }

    #[test]
    fn a_new_target_replaces_the_fields_run_and_cancel_drops_it() {
        let mut eases = Eases::default();
        eases.start(EaseField::Yaw, 10.0, 0.0);
        eases.start(EaseField::Yaw, 10.0, 20.0);
        let values = eases.advance(EASE_SECONDS);
        assert_eq!(values.len(), 1);
        assert_relative_eq!(values[0].1, 20.0);
        eases.start(EaseField::Pitch, 5.0, 0.0);
        eases.cancel(EaseField::Pitch);
        assert!(!eases.is_running());
    }
}
