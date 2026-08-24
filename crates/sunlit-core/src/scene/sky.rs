//! Per-frame astronomical state shared by sky renderers.

use astronomy_engine_bindings::{
    Astronomy_GeoVector, Astronomy_Illumination, Astronomy_Rotation_EQJ_EQD,
    Astronomy_SiderealTime, astro_aberration_t_ABERRATION, astro_body_t, astro_body_t_BODY_JUPITER,
    astro_body_t_BODY_MARS, astro_body_t_BODY_MERCURY, astro_body_t_BODY_SATURN,
    astro_body_t_BODY_SUN, astro_body_t_BODY_VENUS, astro_status_t_ASTRO_SUCCESS, astro_time_t,
};
use glam::{Mat3, Vec3};

use super::sun::{DateTimeInput, make_time};

/// A naked eye planet rendered by the sky sprite pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanetKind {
    Mercury,
    Venus,
    Mars,
    Jupiter,
    Saturn,
}

/// The apparent state of a planet at one observation time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanetState {
    pub kind: PlanetKind,
    /// Geocentric direction in the renderer's world frame.
    pub direction: Vec3,
    /// Apparent visual magnitude as seen from Earth.
    pub magnitude: f32,
}

/// Astronomy values that are computed together for one rendered frame.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyState {
    /// Rotation from the J2000 mean equatorial frame into renderer world space.
    pub world_from_eqj: Mat3,
    /// Geocentric sun direction in renderer world space.
    pub sun_direction: Vec3,
    /// The five naked eye planets in stable display order.
    pub planets: [PlanetState; 5],
}

const PLANETS: [(PlanetKind, astro_body_t); 5] = [
    (PlanetKind::Mercury, astro_body_t_BODY_MERCURY),
    (PlanetKind::Venus, astro_body_t_BODY_VENUS),
    (PlanetKind::Mars, astro_body_t_BODY_MARS),
    (PlanetKind::Jupiter, astro_body_t_BODY_JUPITER),
    (PlanetKind::Saturn, astro_body_t_BODY_SATURN),
];

/// Compute all astronomy inputs for a scene at an injected UTC time.
pub fn compute_sky_state_at(dt: &DateTimeInput, now_utc: time::OffsetDateTime) -> SkyState {
    compute_sky_state_from_time(time_for_input(dt, now_utc))
}

/// Compute all astronomy inputs for a scene using the current UTC time.
pub fn compute_sky_state(dt: &DateTimeInput) -> SkyState {
    compute_sky_state_at(dt, time::OffsetDateTime::now_utc())
}

/// Compute the sky state from an Astronomy Engine time value.
pub fn compute_sky_state_from_time(mut time: astro_time_t) -> SkyState {
    let world_from_eqj = rotation_world_from_eqj(&mut time);
    let sun_direction = body_direction(astro_body_t_BODY_SUN, time, world_from_eqj);
    let planets = PLANETS.map(|(kind, body)| PlanetState {
        kind,
        direction: body_direction(body, time, world_from_eqj),
        magnitude: body_magnitude(body, time),
    });

    SkyState {
        world_from_eqj,
        sun_direction,
        planets,
    }
}

#[allow(clippy::cast_possible_truncation)]
fn rotation_world_from_eqj(time: &mut astro_time_t) -> Mat3 {
    // SAFETY: Both functions only read or update the valid time value passed
    // by pointer and return value types with no retained references.
    #[allow(unsafe_code)]
    let (rotation, gast_hours) = unsafe {
        (
            Astronomy_Rotation_EQJ_EQD(std::ptr::from_mut(time)),
            Astronomy_SiderealTime(std::ptr::from_mut(time)),
        )
    };
    assert_eq!(
        rotation.status, astro_status_t_ASTRO_SUCCESS,
        "Astronomy_Rotation_EQJ_EQD failed"
    );

    let eqd_from_eqj = Mat3::from_cols(
        Vec3::new(
            rotation.rot[0][0] as f32,
            rotation.rot[0][1] as f32,
            rotation.rot[0][2] as f32,
        ),
        Vec3::new(
            rotation.rot[1][0] as f32,
            rotation.rot[1][1] as f32,
            rotation.rot[1][2] as f32,
        ),
        Vec3::new(
            rotation.rot[2][0] as f32,
            rotation.rot[2][1] as f32,
            rotation.rot[2][2] as f32,
        ),
    );
    let earth_fixed_from_eqd = Mat3::from_rotation_z(-(gast_hours as f32 * 15.0).to_radians());
    let world_from_earth_fixed = Mat3::from_cols(Vec3::Z, Vec3::X, Vec3::Y);

    world_from_earth_fixed * earth_fixed_from_eqd * eqd_from_eqj
}

#[allow(clippy::cast_possible_truncation)]
fn body_direction(body: astro_body_t, time: astro_time_t, rotation: Mat3) -> Vec3 {
    // SAFETY: Astronomy_GeoVector is a pure C function. The body constants and
    // time value are valid, and the function returns a value type.
    #[allow(unsafe_code)]
    let vector = unsafe { Astronomy_GeoVector(body, time, astro_aberration_t_ABERRATION) };
    assert_eq!(
        vector.status, astro_status_t_ASTRO_SUCCESS,
        "Astronomy_GeoVector failed"
    );
    let eqj = Vec3::new(vector.x as f32, vector.y as f32, vector.z as f32).normalize();
    (rotation * eqj).normalize()
}

#[allow(clippy::cast_possible_truncation)]
fn body_magnitude(body: astro_body_t, time: astro_time_t) -> f32 {
    // SAFETY: Astronomy_Illumination is a pure C function. The body constants
    // and time value are valid, and the function returns a value type.
    #[allow(unsafe_code)]
    let illumination = unsafe { Astronomy_Illumination(body, time) };
    assert_eq!(
        illumination.status, astro_status_t_ASTRO_SUCCESS,
        "Astronomy_Illumination failed"
    );
    illumination.mag as f32
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn time_for_input(dt: &DateTimeInput, now_utc: time::OffsetDateTime) -> astro_time_t {
    if dt.use_custom {
        let doy = dt.custom_day_of_year.max(1);
        let (month, day) = super::datetime::day_of_year_to_month_day(doy, dt.custom_year);
        let (hour, minute, second) = super::datetime::hour_float_to_hms(dt.custom_hour);
        make_time(
            dt.custom_year,
            i32::from(month),
            i32::from(day),
            hour,
            minute,
            second,
        )
    } else {
        make_time(
            now_utc.year(),
            i32::from(u8::from(now_utc.month())),
            i32::from(now_utc.day()),
            i32::from(now_utc.hour()),
            i32::from(now_utc.minute()),
            f64::from(now_utc.second()),
        )
    }
}

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;

    use crate::scene::sun::sun_direction_from_time;

    use super::*;

    fn pinned_time() -> astro_time_t {
        make_time(2025, 1, 1, 0, 0, 0.0)
    }

    #[test]
    fn world_rotation_is_orthonormal() {
        let rotation = compute_sky_state_from_time(pinned_time()).world_from_eqj;
        let identity = rotation.transpose() * rotation;
        let largest_error = identity
            .to_cols_array()
            .into_iter()
            .zip(Mat3::IDENTITY.to_cols_array())
            .map(|(actual, expected)| (actual - expected).abs())
            .fold(0.0_f32, f32::max);
        assert!(largest_error < 2.0e-6, "largest error was {largest_error}");
    }

    #[test]
    fn world_rotation_has_positive_unit_determinant() {
        let rotation = compute_sky_state_from_time(pinned_time()).world_from_eqj;
        assert_relative_eq!(rotation.determinant(), 1.0, epsilon = 2.0e-6);
    }

    #[test]
    fn production_sun_agrees_with_reference_path() {
        let time = pinned_time();
        let actual = compute_sky_state_from_time(time).sun_direction;
        let reference = sun_direction_from_time(time);
        let separation = actual.angle_between(reference).to_degrees();
        assert!(
            separation < 0.05,
            "separation was {separation} degrees: actual={actual:?}, reference={reference:?}"
        );
    }

    #[test]
    fn polaris_lands_near_world_north() {
        let rotation = compute_sky_state_from_time(pinned_time()).world_from_eqj;
        let ra = (2.0_f32 + 31.0 / 60.0) * 15.0_f32.to_radians();
        let dec = 89.26_f32.to_radians();
        let polaris_eqj = Vec3::new(dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin());
        assert!((rotation * polaris_eqj).angle_between(Vec3::Y).to_degrees() < 1.0);
    }

    #[test]
    fn jupiter_direction_matches_jpl_horizons() {
        let state = compute_sky_state_from_time(pinned_time());
        let jupiter = state
            .planets
            .iter()
            .find(|planet| planet.kind == PlanetKind::Jupiter)
            .expect("Jupiter is present");
        let ra = 71.503_92_f32.to_radians();
        let dec = 21.740_803_f32.to_radians();
        let expected_eqj = Vec3::new(dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin());
        let expected_world = state.world_from_eqj * expected_eqj;
        assert!(jupiter.direction.angle_between(expected_world).to_degrees() < 0.05);
    }

    #[test]
    fn venus_is_brighter_than_magnitude_negative_three() {
        let state = compute_sky_state_from_time(pinned_time());
        let venus = state
            .planets
            .iter()
            .find(|planet| planet.kind == PlanetKind::Venus)
            .expect("Venus is present");
        assert!(venus.magnitude < -3.0, "magnitude was {}", venus.magnitude);
    }
}
