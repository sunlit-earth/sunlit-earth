use astronomy_engine_bindings::{
    Astronomy_CurrentTime, Astronomy_Equator, Astronomy_MakeObserver, Astronomy_SiderealTime,
    astro_aberration_t_ABERRATION, astro_body_t_BODY_SUN, astro_equator_date_t_EQUATOR_OF_DATE,
    astro_status_t_ASTRO_SUCCESS, astro_time_t,
};
use glam::Vec3;

/// Compute the sun's direction as a unit vector in the renderer's
/// world-space coordinate frame (Y-up, +X = prime meridian at equator,
/// -Z = 90 degrees East) for the current UTC time.
pub fn sun_direction_now() -> Vec3 {
    // SAFETY: Astronomy_CurrentTime is a pure C function with no
    // preconditions. It reads the system clock and returns a value type.
    #[allow(unsafe_code)]
    let time = unsafe { Astronomy_CurrentTime() };
    sun_direction_from_time(time)
}

/// Compute the sun direction for a specific `astro_time_t`.
///
/// This is the core implementation shared by `sun_direction_now`,
/// `sun_direction_at`, and tests.
pub fn sun_direction_from_time(mut time: astro_time_t) -> Vec3 {
    // Get the sun's equatorial coordinates (right ascension and declination)
    // referred to the equator of date, with aberration correction.
    // We use a geocentric observer (lat=0, lon=0, height=0) because
    // we want the direction from Earth's center, not a surface point.
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
        equ.status,
        astro_status_t_ASTRO_SUCCESS,
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

/// Compute the sun's direction as a unit vector for a specific UTC date/time.
///
/// :param year: Calendar year (e.g. 2025).
/// :param month: Month, 1-12.
/// :param day: Day of month, 1-31.
/// :param hour: Hour, 0-23.
/// :param minute: Minute, 0-59.
/// :param second: Fractional second, 0.0..60.0.
/// :returns: Unit vector in the renderer's world-space coordinate frame.
pub fn sun_direction_at(
    year: i32, month: i32, day: i32,
    hour: i32, minute: i32, second: f64,
) -> Vec3 {
    let time = make_time(year, month, day, hour, minute, second);
    sun_direction_from_time(time)
}

/// Create an `astro_time_t` from calendar components (UTC).
pub fn make_time(year: i32, month: i32, day: i32, hour: i32, minute: i32, second: f64) -> astro_time_t {
    use astronomy_engine_bindings::Astronomy_MakeTime;
    // SAFETY: Astronomy_MakeTime is a pure C function that constructs a value
    // type from calendar components.
    #[allow(unsafe_code)]
    unsafe {
        Astronomy_MakeTime(year, month, day, hour, minute, second)
    }
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
        let dir = sun_direction_now();
        assert_relative_eq!(dir.length(), 1.0, epsilon = 1e-4);
    }

    // --- sun_direction_at ---

    /// `sun_direction_at` matches the test helper for the March equinox.
    #[test]
    fn direction_at_matches_helper_equinox() {
        let via_at = sun_direction_at(2025, 3, 20, 12, 0, 0.0);
        let via_helper = sun_dir_at(2025, 3, 20, 12, 0);
        assert_relative_eq!(via_at.x, via_helper.x, epsilon = 1e-5);
        assert_relative_eq!(via_at.y, via_helper.y, epsilon = 1e-5);
        assert_relative_eq!(via_at.z, via_helper.z, epsilon = 1e-5);
    }

    /// `sun_direction_at` matches the test helper for the June solstice.
    #[test]
    fn direction_at_matches_helper_solstice() {
        let via_at = sun_direction_at(2025, 6, 21, 12, 0, 0.0);
        let via_helper = sun_dir_at(2025, 6, 21, 12, 0);
        assert_relative_eq!(via_at.x, via_helper.x, epsilon = 1e-5);
        assert_relative_eq!(via_at.y, via_helper.y, epsilon = 1e-5);
        assert_relative_eq!(via_at.z, via_helper.z, epsilon = 1e-5);
    }

    /// `sun_direction_at` always returns a unit vector.
    #[test]
    fn direction_at_unit_vector() {
        let dir = sun_direction_at(2025, 3, 20, 12, 0, 0.0);
        assert_relative_eq!(dir.length(), 1.0, epsilon = 1e-4);
    }

    /// March equinox noon expectations for `sun_direction_at`.
    #[test]
    fn direction_at_equinox_expectations() {
        let dir = sun_direction_at(2025, 3, 20, 12, 0, 0.0);
        assert_relative_eq!(dir.z, 1.0, epsilon = 0.1);
        assert_relative_eq!(dir.y, 0.0, epsilon = 0.1);
        assert_relative_eq!(dir.x, 0.0, epsilon = 0.15);
    }

    /// June solstice noon: y ~ 0.40 (northern declination).
    #[test]
    fn direction_at_june_solstice() {
        let dir = sun_direction_at(2025, 6, 21, 12, 0, 0.0);
        assert_relative_eq!(dir.y, 0.40, epsilon = 0.1);
    }
}
