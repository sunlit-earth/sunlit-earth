//! The subsolar-point sun direction, kept as the reference implementation.
//!
//! Production reads the sun off [`crate::scene::sky::SkyState`], which rotates
//! one geocentric Astronomy Engine vector through the same frame the stars and
//! planets use. This module computes the same direction the other way, from
//! right ascension and declination of date plus sidereal time, and
//! `sky::tests::production_sun_agrees_with_reference_path` is what holds the
//! two together. Two independent derivations agreeing is worth more than one
//! of them being called twice, which is why this one was not deleted when the
//! sky took over the frame.
//!
//! [`DateTimeInput`] and [`make_time`] are production plumbing and live here
//! because this is where they started; `sky` takes both.

use astronomy_engine_bindings::astro_time_t;
#[cfg(test)]
use astronomy_engine_bindings::{
    Astronomy_Equator, Astronomy_MakeObserver, Astronomy_SiderealTime,
    astro_aberration_t_ABERRATION, astro_body_t_BODY_SUN, astro_equator_date_t_EQUATOR_OF_DATE,
    astro_status_t_ASTRO_SUCCESS,
};
#[cfg(test)]
use glam::Vec3;

/// Compute the sun's direction as a unit vector in the renderer's world-space
/// coordinate frame (Y-up, +Z = prime meridian at the equator, +X = 90 degrees
/// East) for a specific `astro_time_t`.
#[cfg(test)]
pub(crate) fn sun_direction_from_time(mut time: astro_time_t) -> Vec3 {
    // The observer is a surface point at 0N 0E, not the geocenter: that is a
    // topocentric answer, and for the sun it differs from the geocentric one
    // by at most 8.8 arcseconds. Production takes the geocentric vector
    // instead, which is one of the two reasons this path is a reference rather
    // than the source; the other is that the sky needs a rotation and not just
    // a direction. The tolerance in the equivalence test is set well above
    // this difference on purpose.
    //
    // SAFETY: Astronomy_MakeObserver is a pure C function that constructs a
    // value type from three doubles.
    #[allow(unsafe_code)]
    let observer = unsafe { Astronomy_MakeObserver(0.0, 0.0, 0.0) };

    // SAFETY: Astronomy_Equator reads the time struct (passed as a mutable
    // pointer so the C library can cache sidereal time internally) and the
    // observer value. All inputs are valid.
    #[allow(unsafe_code)]
    let equ = unsafe {
        Astronomy_Equator(
            astro_body_t_BODY_SUN,
            &raw mut time,
            observer,
            astro_equator_date_t_EQUATOR_OF_DATE,
            astro_aberration_t_ABERRATION,
        )
    };
    assert_eq!(
        equ.status, astro_status_t_ASTRO_SUCCESS,
        "Astronomy_Equator failed"
    );

    // SAFETY: Astronomy_SiderealTime reads the time struct (mutable pointer
    // for internal caching). The time value is valid.
    #[allow(unsafe_code)]
    let gast_hours = unsafe { Astronomy_SiderealTime(&raw mut time) };

    // Subsolar latitude = sun's declination
    let subsolar_lat_deg = equ.dec;

    // Subsolar longitude: the Greenwich Hour Angle of the sun is
    // GAST - RA, but hour angle is measured westward (positive = west).
    // Geographic longitude is positive east, so we need to negate:
    //   subsolar_lon = -(GAST - RA) * 15 = (RA - GAST) * 15
    // Normalize to [-180, 180] because RA and GAST can wrap independently.
    let mut subsolar_lon_deg = (equ.ra - gast_hours) * 15.0;
    subsolar_lon_deg = ((subsolar_lon_deg % 360.0) + 540.0) % 360.0 - 180.0;

    // Convert to renderer's Cartesian coordinate frame.
    // The camera uses: x = sin(lon), y = sin(lat), z = cos(lon)
    // so longitude=0 looks down the +Z axis. We must match that
    // convention for the sun direction.
    //   +Z = prime meridian at equator (0N, 0E)
    //   +Y = north pole
    //   +X = 90 degrees East (0N, 90E)
    let phi = subsolar_lat_deg.to_radians();
    let lambda = subsolar_lon_deg.to_radians();

    #[allow(clippy::cast_possible_truncation)]
    let dir = Vec3::new(
        (phi.cos() * lambda.sin()) as f32,
        phi.sin() as f32,
        (phi.cos() * lambda.cos()) as f32,
    );

    dir.normalize()
}

/// Create an `astro_time_t` from calendar components (UTC).
pub fn make_time(
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: f64,
) -> astro_time_t {
    use astronomy_engine_bindings::Astronomy_MakeTime;
    // SAFETY: Astronomy_MakeTime is a pure C function that constructs a value
    // type from calendar components.
    #[allow(unsafe_code)]
    unsafe {
        Astronomy_MakeTime(year, month, day, hour, minute, second)
    }
}

/// Input parameters for astronomical computations, extracted from the UI.
///
/// This struct captures the datetime state needed to compute time-dependent
/// scene properties (sun direction, future moon/planet positions). It is
/// independent of Slint — use `ui_callbacks::read_datetime_input()` to
/// populate it from the window, or construct it directly for headless use.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DateTimeInput {
    /// Whether the user has selected a custom date/time (vs. live UTC).
    pub use_custom: bool,
    /// Custom hour as a float (0.0..24.0), only used when `use_custom` is true.
    pub custom_hour: f32,
    /// Custom day of year (1..366), only used when `use_custom` is true.
    pub custom_day_of_year: u16,
    /// Custom calendar year (e.g. 2025), only used when `use_custom` is true.
    pub custom_year: i32,
}

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;

    use super::*;

    /// Helper: compute sun direction for a specific UTC date/time.
    fn sun_dir_at(year: i32, month: i32, day: i32, hour: i32, minute: i32) -> Vec3 {
        let time = make_time(year, month, day, hour, minute, 0.0);
        sun_direction_from_time(time)
    }

    /// At the March equinox UTC noon, the subsolar point is near (0N, 0E).
    /// In the renderer's frame (+Z = prime meridian), expected: ~(0, 0, +1).
    #[test]
    fn march_equinox_noon() {
        let dir = sun_dir_at(2025, 3, 20, 12, 0);
        assert_relative_eq!(dir.z, 1.0, epsilon = 0.1);
        assert_relative_eq!(dir.y, 0.0, epsilon = 0.1);
        assert_relative_eq!(dir.x, 0.0, epsilon = 0.15);
    }

    /// At the June solstice UTC noon, the subsolar point is near (23.4N, 0E).
    /// Expected: Z ~ cos(23.4) ~ 0.92, Y ~ sin(23.4) ~ 0.40, X ~ 0.
    #[test]
    fn june_solstice_noon() {
        let dir = sun_dir_at(2025, 6, 21, 12, 0);
        assert_relative_eq!(dir.y, 0.40, epsilon = 0.1);
        assert_relative_eq!(dir.z, 0.92, epsilon = 0.1);
        assert_relative_eq!(dir.x, 0.0, epsilon = 0.15);
    }

    /// At the December solstice UTC noon, the subsolar point is near (23.4S, 0E).
    /// Expected: Y ~ -0.40.
    #[test]
    fn december_solstice_noon() {
        let dir = sun_dir_at(2025, 12, 21, 12, 0);
        assert_relative_eq!(dir.y, -0.40, epsilon = 0.1);
    }

    /// At UTC midnight on the March equinox, the subsolar point is near
    /// the international date line (180E). Expected: Z near -1, Y near 0, X near 0.
    #[test]
    fn march_equinox_midnight() {
        let dir = sun_dir_at(2025, 3, 20, 0, 0);
        assert_relative_eq!(dir.z, -1.0, epsilon = 0.1);
        assert_relative_eq!(dir.y, 0.0, epsilon = 0.1);
        assert_relative_eq!(dir.x, 0.0, epsilon = 0.15);
    }

    /// The returned vector should always have unit length.
    #[test]
    fn unit_vector() {
        let dir = sun_dir_at(2025, 3, 20, 12, 0);
        assert_relative_eq!(dir.length(), 1.0, epsilon = 1e-4);
    }
}
