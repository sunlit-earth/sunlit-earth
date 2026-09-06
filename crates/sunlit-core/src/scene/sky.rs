//! Per-frame astronomical state shared by sky renderers.

use astronomy_engine_bindings::{
    Astronomy_GeoMoon, Astronomy_GeoVector, Astronomy_Illumination, Astronomy_Rotation_EQJ_EQD,
    Astronomy_RotationAxis, Astronomy_SiderealTime, astro_aberration_t_ABERRATION, astro_body_t,
    astro_body_t_BODY_JUPITER, astro_body_t_BODY_MARS, astro_body_t_BODY_MERCURY,
    astro_body_t_BODY_MOON, astro_body_t_BODY_SATURN, astro_body_t_BODY_SUN,
    astro_body_t_BODY_VENUS, astro_status_t, astro_status_t_ASTRO_SUCCESS, astro_time_t,
    astro_vector_t,
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
    /// Geocentric moon position in renderer world space, in Earth radii.
    ///
    /// A position rather than a direction, because the Moon is the one thing in
    /// the sky near enough for the camera's own displacement to matter: every
    /// frame measures its direction and its apparent size from the eye, which
    /// is what makes parallax and apparent size exact for free.
    pub moon_position: Vec3,
    /// Rotation from the Moon's IAU body-fixed frame into renderer world space.
    pub moon_rotation: Mat3,
}

const PLANETS: [(PlanetKind, astro_body_t); 5] = [
    (PlanetKind::Mercury, astro_body_t_BODY_MERCURY),
    (PlanetKind::Venus, astro_body_t_BODY_VENUS),
    (PlanetKind::Mars, astro_body_t_BODY_MARS),
    (PlanetKind::Jupiter, astro_body_t_BODY_JUPITER),
    (PlanetKind::Saturn, astro_body_t_BODY_SATURN),
];

/// Compute all astronomy inputs for a scene at an injected UTC time.
pub(crate) fn compute_sky_state_at(dt: &DateTimeInput, now_utc: time::OffsetDateTime) -> SkyState {
    compute_sky_state_from_time(time_for_input(dt, now_utc))
}

/// Compute all astronomy inputs for a scene using the current UTC time.
pub fn compute_sky_state(dt: &DateTimeInput) -> SkyState {
    compute_sky_state_at(dt, time::OffsetDateTime::now_utc())
}

/// Compute the sky state from an Astronomy Engine time value.
pub(crate) fn compute_sky_state_from_time(mut time: astro_time_t) -> SkyState {
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
        moon_position: moon_position(time, world_from_eqj),
        moon_rotation: moon_rotation(&mut time, world_from_eqj),
    }
}

/// Earth radii per astronomical unit.
///
/// The scene's unit of length is the Earth's equatorial radius (6378.137 km),
/// which is what the camera distances and the atmosphere shells are in, so this
/// is 1 AU expressed in those.
const EARTH_RADII_PER_AU: f32 = 23_454.8;

/// Fail with the entry point's own name when a call reports one.
///
/// Every function below can only report a failure for a body it was not given
/// one of, and every body here is a compile-time constant, so this is
/// unreachable in practice. Naming the call is what would make it findable.
///
/// `#[track_caller]` so the panic points at the entry point rather than at
/// this line, which is where it pointed when each of them carried its own
/// assert.
#[track_caller]
fn checked(status: astro_status_t, what: &str) {
    assert_eq!(status, astro_status_t_ASTRO_SUCCESS, "{what} failed");
}

/// The library computes in f64 and the renderer draws in f32. Every narrowing
/// of a returned value goes through here, which is what keeps the suppression
/// to one site instead of one per entry point.
#[allow(clippy::cast_possible_truncation)]
fn f32_of(value: f64) -> f32 {
    value as f32
}

/// A returned vector in the renderer's precision.
fn vec3_of(v: astro_vector_t) -> Vec3 {
    Vec3::new(f32_of(v.x), f32_of(v.y), f32_of(v.z))
}

/// One row of a returned rotation matrix, likewise.
fn vec3_of_row(row: [f64; 3]) -> Vec3 {
    Vec3::new(f32_of(row[0]), f32_of(row[1]), f32_of(row[2]))
}

/// The Moon's geocentric position in world space, in Earth radii.
fn moon_position(time: astro_time_t, rotation: Mat3) -> Vec3 {
    // SAFETY: Astronomy_GeoMoon is a pure C function. The time value is valid
    // and the function returns a value type with no retained references.
    #[allow(unsafe_code)]
    let vector = unsafe { Astronomy_GeoMoon(time) };
    checked(vector.status, "Astronomy_GeoMoon");
    rotation * (vec3_of(vector) * EARTH_RADII_PER_AU)
}

/// The rotation that takes the Moon's body-fixed frame into world space.
///
/// The IAU model gives a north pole direction and a spin angle, and the spin is
/// measured east along the body's equator from the ascending node of that
/// equator on the J2000 equator. So the node is the zero of longitude before
/// the spin is applied, the spin carries the prime meridian to where it
/// actually points, and the body's own axes follow from the two.
fn moon_rotation(time: &mut astro_time_t, world_from_eqj: Mat3) -> Mat3 {
    // SAFETY: Astronomy_RotationAxis only reads and updates the valid time
    // value passed by pointer, and returns a value type.
    #[allow(unsafe_code)]
    let axis = unsafe { Astronomy_RotationAxis(astro_body_t_BODY_MOON, std::ptr::from_mut(time)) };
    checked(axis.status, "Astronomy_RotationAxis");
    let pole = vec3_of(axis.north).normalize();
    let node = Vec3::Z.cross(pole);
    // A pole on the J2000 pole itself leaves the node undefined; the Moon's is
    // 66 degrees away from it and the fallback is never taken.
    let node = if node.length() > 1e-6 {
        node.normalize()
    } else {
        Vec3::X
    };
    let spin = f32_of(axis.spin).to_radians();
    let prime_meridian = node * spin.cos() + pole.cross(node) * spin.sin();
    let eqj_from_body = Mat3::from_cols(prime_meridian, pole.cross(prime_meridian), pole);
    world_from_eqj * eqj_from_body
}

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
    checked(rotation.status, "Astronomy_Rotation_EQJ_EQD");

    let eqd_from_eqj = Mat3::from_cols(
        vec3_of_row(rotation.rot[0]),
        vec3_of_row(rotation.rot[1]),
        vec3_of_row(rotation.rot[2]),
    );
    let earth_fixed_from_eqd = Mat3::from_rotation_z(-(f32_of(gast_hours) * 15.0).to_radians());
    let world_from_earth_fixed = Mat3::from_cols(Vec3::Z, Vec3::X, Vec3::Y);

    world_from_earth_fixed * earth_fixed_from_eqd * eqd_from_eqj
}

fn body_direction(body: astro_body_t, time: astro_time_t, rotation: Mat3) -> Vec3 {
    // SAFETY: Astronomy_GeoVector is a pure C function. The body constants and
    // time value are valid, and the function returns a value type.
    #[allow(unsafe_code)]
    let vector = unsafe { Astronomy_GeoVector(body, time, astro_aberration_t_ABERRATION) };
    checked(vector.status, "Astronomy_GeoVector");
    (rotation * vec3_of(vector).normalize()).normalize()
}

fn body_magnitude(body: astro_body_t, time: astro_time_t) -> f32 {
    // SAFETY: Astronomy_Illumination is a pure C function. The body constants
    // and time value are valid, and the function returns a value type.
    #[allow(unsafe_code)]
    let illumination = unsafe { Astronomy_Illumination(body, time) };
    checked(illumination.status, "Astronomy_Illumination");
    f32_of(illumination.mag)
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

    /// A custom date and time selects the instant the widgets name.
    #[test]
    fn a_custom_datetime_selects_the_instant_it_names() {
        let dt = DateTimeInput {
            use_custom: true,
            custom_hour: 12.0,
            // June 21 in a non-leap year.
            custom_day_of_year: 172,
            custom_year: 2025,
        };
        let selected = compute_sky_state_at(&dt, time::OffsetDateTime::UNIX_EPOCH);
        let expected = compute_sky_state_from_time(make_time(2025, 6, 21, 12, 0, 0.0));
        assert_eq!(selected, expected);
    }

    #[test]
    fn a_custom_day_of_year_of_zero_is_clamped_to_january_first() {
        let dt = DateTimeInput {
            use_custom: true,
            custom_hour: 12.0,
            custom_day_of_year: 0,
            custom_year: 2025,
        };
        let selected = compute_sky_state_at(&dt, time::OffsetDateTime::UNIX_EPOCH);
        let expected = compute_sky_state_from_time(make_time(2025, 1, 1, 12, 0, 0.0));
        assert_eq!(selected, expected);
    }

    /// Live time reads the injected `now`, which is what lets the mock clock in
    /// the soak test rotate the Earth for fourteen simulated days.
    #[test]
    fn live_time_follows_the_injected_now() {
        let dt = DateTimeInput {
            use_custom: false,
            custom_hour: 0.0,
            custom_day_of_year: 1,
            custom_year: 2025,
        };
        let now = time::OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        let selected = compute_sky_state_at(&dt, now);
        let expected = compute_sky_state_from_time(make_time(
            now.year(),
            i32::from(u8::from(now.month())),
            i32::from(now.day()),
            i32::from(now.hour()),
            i32::from(now.minute()),
            f64::from(now.second()),
        ));
        assert_eq!(selected, expected);
    }

    /// Selenographic coordinates of the sub-Earth point, in degrees.
    ///
    /// The direction from the Moon to the Earth, expressed in the Moon's own
    /// body-fixed frame. Tidal lock puts it near the prime meridian at the
    /// equator, and the libration is how far it wanders.
    fn sub_earth_point(state: &MoonState) -> (f32, f32) {
        let eqj_from_body = state.world_from_eqj.transpose() * state.rotation;
        let toward_earth = eqj_from_body.transpose()
            * (state.world_from_eqj.transpose() * -state.position).normalize();
        (
            toward_earth.y.atan2(toward_earth.x).to_degrees(),
            toward_earth.z.asin().to_degrees(),
        )
    }

    /// The library's own libration model, which the tests compare against.
    fn libration(time: astro_time_t) -> astronomy_engine_bindings::astro_libration_t {
        // SAFETY: Astronomy_Libration is a pure C function. The time value is
        // valid and the function returns a value type.
        #[allow(unsafe_code)]
        unsafe {
            astronomy_engine_bindings::Astronomy_Libration(time)
        }
    }

    /// The three quantities the Moon cases read, and none of the eleven
    /// ephemeris calls per sample `compute_sky_state_from_time` spends on the
    /// Sun and the five planets, which no case below looks at.
    struct MoonState {
        world_from_eqj: Mat3,
        position: Vec3,
        rotation: Mat3,
    }

    fn moon_state_from_time(mut time: astro_time_t) -> MoonState {
        let world_from_eqj = rotation_world_from_eqj(&mut time);
        MoonState {
            world_from_eqj,
            position: moon_position(time, world_from_eqj),
            rotation: moon_rotation(&mut time, world_from_eqj),
        }
    }

    /// Twice a day over four years, which is about fifty lunations sampled at
    /// every phase. Libration and lunar distance run on that cycle, so a finer
    /// grid buys none of the cases below anything.
    fn four_years_of_times() -> impl Iterator<Item = astro_time_t> {
        (2026..2030).flat_map(|year| {
            (1..=365).flat_map(move |doy| {
                let (month, day) = super::super::datetime::day_of_year_to_month_day(doy, year);
                [0, 12].into_iter().map(move |hour| {
                    make_time(year, i32::from(month), i32::from(day), hour, 0, 0.0)
                })
            })
        })
    }

    /// Perigee and apogee bound the orbit, so a scale error in the AU
    /// conversion or a direction taken for a position lands outside them.
    #[test]
    fn the_moon_orbits_between_fifty_five_and_sixty_four_earth_radii() {
        for time in four_years_of_times() {
            let distance = moon_state_from_time(time).position.length();
            assert!(
                (55.0..64.0).contains(&distance),
                "{distance} Earth radii at {}",
                time.ut
            );
        }
    }

    /// The scene's unit of length against the library's kilometers, which is
    /// what [`EARTH_RADII_PER_AU`] and nothing else decides.
    #[test]
    fn the_moon_distance_agrees_with_the_librarys_kilometers() {
        for time in four_years_of_times().step_by(37) {
            let distance = moon_state_from_time(time).position.length();
            let km = f64::from(distance) * 6378.137;
            assert_relative_eq!(km, libration(time).dist_km, max_relative = 1.0e-5);
        }
    }

    /// The Moon passed in front of the Sun over North America on 2024-04-08,
    /// with greatest eclipse at 18:17 UTC. A position wrong by a degree, in any
    /// component, does not reproduce that.
    #[test]
    fn the_moon_covers_the_sun_at_the_2024_total_eclipse() {
        let state = compute_sky_state_from_time(make_time(2024, 4, 8, 18, 17, 0.0));
        let separation = state
            .moon_position
            .normalize()
            .angle_between(state.sun_direction)
            .to_degrees();
        assert!(separation < 0.6, "separation was {separation} degrees");
    }

    /// Tidal lock: the Earth stays over the Moon's prime meridian, and how far
    /// it wanders is the optical libration, about eight degrees of longitude
    /// and seven of latitude. A transposed rotation or a flipped spin sign
    /// sends the sub-Earth point somewhere else entirely.
    #[test]
    fn the_sub_earth_point_stays_inside_the_libration_bounds() {
        for time in four_years_of_times() {
            let (longitude, latitude) = sub_earth_point(&moon_state_from_time(time));
            assert!(
                longitude.abs() < 8.5 && latitude.abs() < 7.5,
                "sub-Earth point at ({longitude}, {latitude}) at {}",
                time.ut
            );
        }
    }

    /// The same rotation against the library's own libration model, which is a
    /// second implementation of the same quantity rather than a bound on it.
    #[test]
    fn the_sub_earth_point_agrees_with_the_library_libration() {
        for time in four_years_of_times().step_by(37) {
            let (longitude, latitude) = sub_earth_point(&moon_state_from_time(time));
            let libration = libration(time);
            #[allow(clippy::cast_possible_truncation)]
            let (elon, elat) = (libration.elon as f32, libration.elat as f32);
            assert!(
                (longitude - elon).abs() < 0.1 && (latitude - elat).abs() < 0.1,
                "({longitude}, {latitude}) against the library's ({elon}, {elat}) at {}",
                time.ut
            );
        }
    }

    /// The Moon's rotation axis is inclined 1.54 degrees to the ecliptic pole,
    /// which is the Cassini state it has been in for most of its history. A
    /// pole taken with the wrong sign, or the body's axes read as rows rather
    /// than columns, misses that by tens of degrees.
    #[test]
    fn the_moon_pole_sits_at_the_cassini_tilt_from_the_ecliptic_pole() {
        let obliquity = 23.439_291_f32.to_radians();
        let ecliptic_north = Vec3::new(0.0, -obliquity.sin(), obliquity.cos());
        for time in four_years_of_times().step_by(37) {
            let state = moon_state_from_time(time);
            let eqj_from_body = state.world_from_eqj.transpose() * state.rotation;
            let tilt = eqj_from_body
                .z_axis
                .angle_between(ecliptic_north)
                .to_degrees();
            assert!((1.0..2.1).contains(&tilt), "tilt was {tilt} degrees");
        }
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
