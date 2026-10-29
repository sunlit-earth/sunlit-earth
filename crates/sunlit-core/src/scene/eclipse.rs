//! The Moon's shadow on the Earth: how much sunlight it leaves at a point,
//! where its center falls, when it touches the Earth, and the solar eclipses
//! Astronomy Engine finds.
//!
//! The drawn globe is a sphere, but everything here is on the WGS84
//! ellipsoid. The cube faces put each place's geodetic latitude at the
//! direction whose elevation is that latitude, so every direction of the
//! sphere is the true surface normal of the place drawn there, and
//! [`surface_point`] takes it to where that place really is.

use std::f32::consts::PI;

use astronomy_engine_bindings::{
    Astronomy_NextGlobalSolarEclipse, Astronomy_SearchGlobalSolarEclipse,
    Astronomy_SearchLocalSolarEclipse, Astronomy_TimeFromDays, astro_eclipse_kind_t,
    astro_eclipse_kind_t_ECLIPSE_ANNULAR, astro_eclipse_kind_t_ECLIPSE_PARTIAL,
    astro_eclipse_kind_t_ECLIPSE_TOTAL, astro_observer_t, astro_status_t_ASTRO_SUCCESS,
    astro_time_t,
};
use glam::{DVec2, DVec3, Vec3};
use time::{Duration, OffsetDateTime, UtcOffset};

use super::disk_occlusion::overlap_area;
use super::scene_time::{EARLIEST, LATEST};
use super::sky::{self, EARTH_RADII_PER_AU};

/// Kilometers in the scene's unit of length, the Earth's equatorial radius as
/// [`EARTH_RADII_PER_AU`] counts it.
const KM_PER_EARTH_RADIUS: f64 = 149_597_870.7 / EARTH_RADII_PER_AU as f64;

/// The Moon's mean radius and the Sun's, Astronomy Engine's, in Earth radii.
const MOON_RADIUS_F64: f64 = 1737.4 / KM_PER_EARTH_RADIUS;
const SUN_RADIUS_F64: f64 = 695_700.0 / KM_PER_EARTH_RADIUS;
/// WGS84's equatorial radius in Earth radii, and its flattening.
const EQUATORIAL_RADIUS_F64: f64 = 6378.137 / KM_PER_EARTH_RADIUS;
const FLATTENING: f64 = 1.0 / 298.257_223_563;
/// The polar radius over the equatorial one.
const POLAR_RATIO: f64 = 1.0 - FLATTENING;
const ECCENTRICITY_SQUARED_F64: f64 = FLATTENING * (2.0 - FLATTENING);

#[expect(
    clippy::cast_possible_truncation,
    reason = "the constants at the renderer's precision"
)]
const fn narrow(value: f64) -> f32 {
    value as f32
}

pub const MOON_RADIUS: f32 = narrow(MOON_RADIUS_F64);
pub const SUN_RADIUS: f32 = narrow(SUN_RADIUS_F64);
pub const EQUATORIAL_RADIUS: f32 = narrow(EQUATORIAL_RADIUS_F64);
pub const ECCENTRICITY_SQUARED: f32 = narrow(ECCENTRICITY_SQUARED_F64);

/// How far the point the camera follows hangs back from the axis's ground
/// point, as the softening of the square root that would otherwise race
/// where the axis enters or leaves the Earth.
const FOLLOW_SOFTENING: f64 = 0.2;

/// The Moon's shadow at one instant, in world space in Earth radii.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowAxis {
    /// The Moon's center.
    pub moon: Vec3,
    /// The unit direction from the Sun's center through the Moon's.
    pub axis: Vec3,
    /// The distance from the Moon's center to the Sun's.
    pub moon_to_sun: f32,
}

impl ShadowAxis {
    /// A full Moon's: the axis points from the Moon away from the Earth.
    #[cfg(test)]
    pub(crate) fn away_from_the_earth() -> Self {
        Self {
            moon: Vec3::new(0.0, 0.0, 60.0),
            axis: Vec3::Z,
            moon_to_sun: 23_400.0,
        }
    }
}

/// The place on the ellipsoid whose surface normal is `n`, raised `height`
/// along it: where a fragment of the sphere at `n`, or of a shell `height`
/// above it, stands for.
#[must_use]
pub fn surface_point(n: Vec3, height: f32) -> Vec3 {
    let normal_radius = EQUATORIAL_RADIUS / (1.0 - ECCENTRICITY_SQUARED * n.y * n.y).sqrt();
    normal_radius * Vec3::new(n.x, (1.0 - ECCENTRICITY_SQUARED) * n.y, n.z) + height * n
}

/// The share of sunlight that reaches `point`: the share of the Sun's disk
/// the Moon's leaves uncovered, seen from there, taken as a uniform disk.
///
/// Measured in the frame of the axis, from differences of numbers the size
/// of the Moon's distance, so it holds in f32 where the angle between the two
/// directions would not. `sphere.wgsl`'s `eclipse_light` is its twin.
#[must_use]
pub fn light_left(shadow: &ShadowAxis, point: Vec3) -> f32 {
    let q = point - shadow.moon;
    let z = q.dot(shadow.axis);
    if z <= 0.0 {
        return 1.0;
    }
    let r = (q - z * shadow.axis).length();
    let separation = (r / z).atan() - (r / (z + shadow.moon_to_sun)).atan();
    let moon_radius = (MOON_RADIUS / q.length()).asin();
    let sun_radius = (SUN_RADIUS / (q + shadow.moon_to_sun * shadow.axis).length()).asin();
    let covered = overlap_area(1.0, moon_radius / sun_radius, separation / sun_radius) / PI;
    (1.0 - covered).clamp(0.0, 1.0)
}

/// Whether the penumbra reaches the sphere of `radius` about the Earth's
/// center, which is what decides whether a frame draws the shadow at all.
#[must_use]
pub fn penumbra_reaches(shadow: &ShadowAxis, radius: f32) -> bool {
    let cone = Cone::penumbra(
        DVec3::from(shadow.moon),
        DVec3::from(shadow.axis).normalize(),
        f64::from(shadow.moon_to_sun),
    );
    cone.distance_from_center() < f64::from(radius)
}

/// One of the shadow's two cones, tangent to the Sun and the Moon.
struct Cone {
    /// The half angle.
    half_angle: f64,
    /// How far past the apex the Earth's center is along the axis; negative
    /// on the umbra's far nappe, the antumbra.
    depth: f64,
    /// The center's distance from the axis.
    off_axis: f64,
}

impl Cone {
    /// The penumbral cone, its apex between the Moon and the Sun.
    fn penumbra(moon: DVec3, axis: DVec3, moon_to_sun: f64) -> Self {
        let half_angle = ((SUN_RADIUS_F64 + MOON_RADIUS_F64) / moon_to_sun).asin();
        Self::with_apex(
            moon - axis * (MOON_RADIUS_F64 / half_angle.sin()),
            axis,
            half_angle,
        )
    }

    /// The umbral cone, its apex past the Moon.
    fn umbra(moon: DVec3, axis: DVec3, moon_to_sun: f64) -> Self {
        let half_angle = ((SUN_RADIUS_F64 - MOON_RADIUS_F64) / moon_to_sun).asin();
        Self::with_apex(
            moon + axis * (MOON_RADIUS_F64 / half_angle.sin()),
            axis,
            half_angle,
        )
    }

    fn with_apex(apex: DVec3, axis: DVec3, half_angle: f64) -> Self {
        let center = -apex;
        let depth = center.dot(axis);
        Self {
            half_angle,
            depth,
            off_axis: (center - depth * axis).length(),
        }
    }

    /// The Earth's center's distance from the cone's surface, outside it
    /// positive, which a sphere about the center touches at its radius.
    fn distance_from_center(&self) -> f64 {
        self.off_axis * self.half_angle.cos() - self.depth.abs() * self.half_angle.sin()
    }

    /// How far the ellipsoid stands clear of the cone, zero where they touch,
    /// given how far outside the ellipsoid's outline in the fundamental plane
    /// the axis passes. The sphere's test with the outline in place of the
    /// circle, its opening carried the same way.
    fn clearance(&self, outside_outline: f64) -> f64 {
        let (sin, cos) = self.half_angle.sin_cos();
        outside_outline * cos - self.depth.abs() * sin + EQUATORIAL_RADIUS_F64 * (cos - 1.0)
    }
}

/// The signed distance from `point` to the ellipse of semi-axes `a` along x
/// and `b` along y, outside positive: the nearest point's parameter by
/// Newton's method, which for an ellipse as round as the Earth's outline
/// settles in a few steps from the point's own angle.
fn distance_to_ellipse(point: DVec2, a: f64, b: f64) -> f64 {
    let p = point.abs();
    let mut t = (a * p.y).atan2(b * p.x);
    for _ in 0..8 {
        let (sin, cos) = t.sin_cos();
        let slope = (b * b - a * a) * sin * cos + a * p.x * sin - b * p.y * cos;
        let curve = (b * b - a * a) * (cos * cos - sin * sin) + a * p.x * cos + b * p.y * sin;
        if curve.abs() < f64::EPSILON {
            break;
        }
        t = (t - slope / curve).clamp(0.0, std::f64::consts::FRAC_PI_2);
    }
    let nearest = DVec2::new(a * t.cos(), b * t.sin());
    let distance = (p - nearest).length();
    if (p.x / a).powi(2) + (p.y / b).powi(2) > 1.0 {
        distance
    } else {
        -distance
    }
}

/// How far the ellipsoid stands clear of the penumbra and of the umbra at
/// `time`, each zero at a contact: the Besselian projection of the ellipsoid
/// along the axis onto the plane through the Earth's center, an ellipse whose
/// semi-axes are the equatorial radius and the radius the axis's declination
/// leaves of the polar flattening.
fn clearances(time: astro_time_t) -> (f64, f64) {
    let (moon, axis, moon_to_sun) = sky::shadow_f64(time);
    let across = DVec3::Y.cross(axis).normalize();
    let up = axis.cross(across);
    let nearest = moon - moon.dot(axis) * axis;
    let in_plane = DVec2::new(nearest.dot(across), nearest.dot(up));
    let sin_declination = axis.y;
    let cos_squared = 1.0 - sin_declination * sin_declination;
    let projected_polar = EQUATORIAL_RADIUS_F64
        * (sin_declination * sin_declination + POLAR_RATIO * POLAR_RATIO * cos_squared).sqrt();
    let outside = distance_to_ellipse(in_plane, EQUATORIAL_RADIUS_F64, projected_polar);
    (
        Cone::penumbra(moon, axis, moon_to_sun).clearance(outside),
        Cone::umbra(moon, axis, moon_to_sun).clearance(outside),
    )
}

/// The latitude and the longitude of the shadow's center: the axis's
/// intersection with the ellipsoid, or the point of its outline nearest the
/// axis when the axis misses, in degrees, the latitude geodetic.
#[must_use]
pub fn shadow_center(shadow: &ShadowAxis) -> (f32, f32) {
    let point = axis_point(shadow, 0.0);
    (point.latitude, point.longitude)
}

/// The latitude and the longitude the camera follows in eclipse mode: the
/// shadow's center with the square root softened, so the point moves at a
/// bounded speed where the axis enters and leaves the Earth.
#[must_use]
pub fn followed_point(shadow: &ShadowAxis) -> (f32, f32) {
    let point = axis_point(shadow, FOLLOW_SOFTENING);
    (point.latitude, point.longitude)
}

/// The share of the Sun covered at the shadow's center.
#[must_use]
pub fn covered_at_center(shadow: &ShadowAxis) -> f32 {
    let (latitude, longitude) = shadow_center(shadow);
    let (lat, lon) = (latitude.to_radians(), longitude.to_radians());
    let normal = Vec3::new(lat.cos() * lon.sin(), lat.sin(), lat.cos() * lon.cos());
    1.0 - light_left(shadow, surface_point(normal, 0.0))
}

struct AxisPoint {
    latitude: f32,
    longitude: f32,
    /// Whether the axis meets the ellipsoid.
    #[cfg(test)]
    meets: bool,
}

/// The point of the axis nearest the ellipsoid's sunward side, worked in the
/// frame where the ellipsoid is a sphere of the equatorial radius: there the
/// axis stays a line, and the near intersection with that sphere, or the
/// point of the line nearest the center when it misses, comes back exactly.
fn axis_point(shadow: &ShadowAxis, softening: f64) -> AxisPoint {
    let stretch = |v: DVec3| DVec3::new(v.x, v.y / POLAR_RATIO, v.z);
    let moon = stretch(DVec3::from(shadow.moon));
    let axis = stretch(DVec3::from(shadow.axis)).normalize();
    let nearest = moon - moon.dot(axis) * axis;
    let inside = EQUATORIAL_RADIUS_F64 * EQUATORIAL_RADIUS_F64 - nearest.length_squared();
    let back = (inside.max(0.0) + softening * softening).sqrt();
    let stretched = nearest - back * axis;
    let point = DVec3::new(stretched.x, stretched.y * POLAR_RATIO, stretched.z);
    let latitude = point
        .y
        .atan2(POLAR_RATIO * POLAR_RATIO * point.x.hypot(point.z))
        .to_degrees();
    let longitude = point.x.atan2(point.z).to_degrees();
    AxisPoint {
        latitude: narrow(latitude),
        longitude: narrow(longitude),
        #[cfg(test)]
        meets: inside > 0.0,
    }
}

/// The kind of a solar eclipse, as seen at its greatest point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EclipseKind {
    Partial,
    Annular,
    Total,
}

/// When the shadow first and last touches the Earth: the penumbra's first and
/// last contacts, and the umbra's or the antumbra's where it touches at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contacts {
    pub p1: OffsetDateTime,
    pub u1: Option<OffsetDateTime>,
    pub u4: Option<OffsetDateTime>,
    pub p4: OffsetDateTime,
}

/// Where the eclipse is greatest and how much of the Sun is covered there:
/// the library's point for a central eclipse, and for a partial one the
/// shadow's center at the peak, on the limb.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Greatest {
    pub latitude: f32,
    pub longitude: f32,
    /// The share of the Sun's disk covered, by area.
    pub covered: f32,
}

/// One solar eclipse.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Eclipse {
    pub kind: EclipseKind,
    /// The instant the axis passes nearest the Earth's center.
    pub peak: OffsetDateTime,
    pub contacts: Contacts,
    /// How long the total or the annular phase lasts at the greatest point.
    pub central_duration: Option<Duration>,
    pub greatest: Greatest,
}

impl Eclipse {
    /// Whether the penumbra is on the Earth at `at`.
    #[must_use]
    pub fn on_earth_at(&self, at: OffsetDateTime) -> bool {
        self.contacts.p1 <= at && at <= self.contacts.p4
    }
}

/// The first eclipse whose peak is after `at`, within the years the scene
/// accepts.
#[must_use]
pub fn next_after(at: OffsetDateTime) -> Option<Eclipse> {
    found_from(at - Duration::days(1))
        .find(|found| found.instant() > at)
        .filter(Found::in_range)
        .map(Found::eclipse)
}

/// The last eclipse whose peak is before `at`, within the years the scene
/// accepts. The library searches forward only, so this starts far enough
/// back to hold at least one: the longest gap between two solar eclipses of
/// 1900 to 2100 is 178.3 days.
#[must_use]
pub fn previous_before(at: OffsetDateTime) -> Option<Eclipse> {
    found_from(at - Duration::days(200))
        .take_while(|found| found.instant() < at)
        .last()
        .filter(Found::in_range)
        .map(Found::eclipse)
}

/// The eclipse whose peak is nearest `at`.
#[must_use]
pub fn nearest(at: OffsetDateTime) -> Option<Eclipse> {
    found_from(at - Duration::days(200))
        .take_while(|found| found.instant() < at + Duration::days(200))
        .filter(Found::in_range)
        .min_by_key(|found| (found.instant() - at).abs())
        .map(Found::eclipse)
}

/// The eclipse whose penumbra is on the Earth at `at`, if there is one.
#[must_use]
pub fn on_earth_at(at: OffsetDateTime) -> Option<Eclipse> {
    // The penumbra is on the Earth for at most 6.2 hours of 1900 to 2100.
    let reach = Duration::hours(7);
    found_from(at - Duration::days(1))
        .take_while(|found| found.instant() < at + reach)
        .filter(|found| found.instant() > at - reach && found.in_range())
        .map(Found::eclipse)
        .find(|eclipse| eclipse.on_earth_at(at))
}

/// The eclipses whose peak falls in `year` in the zone `offset_at` gives for
/// an instant, in order.
#[must_use]
pub fn in_year(year: i32, offset_at: impl Fn(OffsetDateTime) -> UtcOffset) -> Vec<Eclipse> {
    let Ok(first) = time::Date::from_ordinal_date(year, 1) else {
        return Vec::new();
    };
    let start = first.midnight().assume_utc() - Duration::days(2);
    let end = start + Duration::days(370);
    found_from(start)
        .take_while(|found| found.instant() < end)
        .filter(|found| {
            let peak = found.instant();
            found.in_range() && peak.to_offset(offset_at(peak)).year() == year
        })
        .map(Found::eclipse)
        .collect()
}

/// What the library's global search reports of one eclipse.
#[derive(Clone, Copy)]
struct Found {
    kind: EclipseKind,
    peak: astro_time_t,
    latitude: f64,
    longitude: f64,
    covered: f64,
}

impl Found {
    /// The peak to the whole second. Two searches from different starts
    /// refine the same peak to instants microseconds apart, and the second is
    /// what makes the two the same eclipse to everything that compares them.
    fn instant(&self) -> OffsetDateTime {
        let exact = sky::instant_of_time(self.peak);
        let second = exact.replace_nanosecond(0).unwrap_or(exact);
        if exact.nanosecond() >= 500_000_000 {
            second + Duration::SECOND
        } else {
            second
        }
    }

    fn in_range(&self) -> bool {
        (EARLIEST..=LATEST).contains(&self.instant())
    }

    /// The eclipse with its contacts and its greatest point, which cost a
    /// scan of the shadow over the twelve hours around the peak.
    fn eclipse(self) -> Eclipse {
        let peak = self.instant();
        let (greatest, central_duration) = if self.kind == EclipseKind::Partial {
            let (latitude, longitude) = shadow_center(&sky::shadow_at(peak));
            let local = local_eclipse(self.peak, f64::from(latitude), f64::from(longitude));
            let greatest = Greatest {
                latitude,
                longitude,
                covered: local.map_or(0.0, |local| narrow(local.obscuration)),
            };
            (greatest, None)
        } else {
            let greatest = Greatest {
                latitude: narrow(self.latitude),
                longitude: narrow(self.longitude),
                covered: narrow(self.covered),
            };
            let duration = local_eclipse(self.peak, self.latitude, self.longitude)
                .filter(|local| local.kind != astro_eclipse_kind_t_ECLIPSE_PARTIAL)
                .map(|local| {
                    Duration::seconds_f64(
                        (local.total_end.time.ut - local.total_begin.time.ut) * 86_400.0,
                    )
                });
            (greatest, duration)
        };
        Eclipse {
            kind: self.kind,
            peak,
            contacts: contacts_around(self.peak.ut),
            central_duration: central_duration.filter(|duration| duration.is_positive()),
            greatest,
        }
    }
}

/// Successive eclipses, the first the earliest the library finds after
/// `start`. It walks the new Moons from there, so an eclipse whose peak is
/// after `start` and whose new Moon is before it is not among them: callers
/// start a day early.
fn found_from(start: OffsetDateTime) -> impl Iterator<Item = Found> {
    // SAFETY: Astronomy_SearchGlobalSolarEclipse is a pure C function that
    // takes a time value and returns a value type.
    #[allow(unsafe_code)]
    let first = unsafe { Astronomy_SearchGlobalSolarEclipse(time_of(start)) };
    std::iter::successors(found(first), |previous| {
        // SAFETY: as above, from the previous eclipse's peak.
        #[allow(unsafe_code)]
        let next = unsafe { Astronomy_NextGlobalSolarEclipse(previous.peak) };
        found(next)
    })
}

fn found(result: astronomy_engine_bindings::astro_global_solar_eclipse_t) -> Option<Found> {
    if result.status != astro_status_t_ASTRO_SUCCESS {
        return None;
    }
    Some(Found {
        kind: kind_of(result.kind)?,
        peak: result.peak,
        latitude: result.latitude,
        longitude: result.longitude,
        covered: result.obscuration,
    })
}

fn kind_of(kind: astro_eclipse_kind_t) -> Option<EclipseKind> {
    [
        (astro_eclipse_kind_t_ECLIPSE_PARTIAL, EclipseKind::Partial),
        (astro_eclipse_kind_t_ECLIPSE_ANNULAR, EclipseKind::Annular),
        (astro_eclipse_kind_t_ECLIPSE_TOTAL, EclipseKind::Total),
    ]
    .into_iter()
    .find_map(|(library, ours)| (library == kind).then_some(ours))
}

/// The eclipse an observer at sea level at the given place sees nearest the
/// global peak.
fn local_eclipse(
    peak: astro_time_t,
    latitude: f64,
    longitude: f64,
) -> Option<astronomy_engine_bindings::astro_local_solar_eclipse_t> {
    let observer = astro_observer_t {
        latitude,
        longitude,
        height: 0.0,
    };
    // SAFETY: Astronomy_SearchLocalSolarEclipse is a pure C function that
    // takes value types and returns one.
    #[allow(unsafe_code)]
    let local = unsafe { Astronomy_SearchLocalSolarEclipse(time_of_ut(peak.ut - 0.2), observer) };
    (local.status == astro_status_t_ASTRO_SUCCESS && (local.peak.time.ut - peak.ut).abs() < 0.2)
        .then_some(local)
}

/// The scan's reach either side of the peak and its step, in days.
const SCAN_REACH: f64 = 0.25;
const SCAN_STEP: f64 = 2.0 / 1440.0;
/// How finely a contact is bisected, in days.
const CONTACT_PRECISION: f64 = 0.1 / 86_400.0;

/// The four contacts of the eclipse peaking at `peak_ut`: a scan at two minute
/// steps brackets each root of the two clearances, and bisection refines it.
fn contacts_around(peak_ut: f64) -> Contacts {
    let start = peak_ut - SCAN_REACH;
    let steps = (2.0 * SCAN_REACH / SCAN_STEP).round();
    let mut penumbra = Vec::new();
    let mut umbra = Vec::new();
    let mut before = (start, clearances(time_of_ut(start)));
    let mut step = 1.0;
    while step <= steps {
        let ut = start + step * SCAN_STEP;
        let after = (ut, clearances(time_of_ut(ut)));
        if (before.1.0 > 0.0) != (after.1.0 > 0.0) {
            penumbra.push(bisect(before.0, ut, |time| clearances(time).0));
        }
        if (before.1.1 > 0.0) != (after.1.1 > 0.0) {
            umbra.push(bisect(before.0, ut, |time| clearances(time).1));
        }
        before = after;
        step += 1.0;
    }
    let instant = |ut: f64| sky::instant_of_time(time_of_ut(ut));
    Contacts {
        p1: instant(penumbra.first().copied().unwrap_or(start)),
        u1: umbra.first().copied().map(instant),
        u4: umbra.last().copied().map(instant),
        p4: instant(penumbra.last().copied().unwrap_or(start + 2.0 * SCAN_REACH)),
    }
}

/// The root of `clearance` between two instants it differs in sign at.
fn bisect(mut low: f64, mut high: f64, clearance: impl Fn(astro_time_t) -> f64) -> f64 {
    let outside_low = clearance(time_of_ut(low)) > 0.0;
    while high - low > CONTACT_PRECISION {
        let middle = 0.5 * (low + high);
        if (clearance(time_of_ut(middle)) > 0.0) == outside_low {
            low = middle;
        } else {
            high = middle;
        }
    }
    0.5 * (low + high)
}

fn time_of(at: OffsetDateTime) -> astro_time_t {
    let noon_2000 = time::macros::datetime!(2000-01-01 12:00 UTC);
    time_of_ut((at - noon_2000).as_seconds_f64() / 86_400.0)
}

fn time_of_ut(ut: f64) -> astro_time_t {
    // SAFETY: Astronomy_TimeFromDays is a pure C function that builds a value
    // type from a day count.
    #[allow(unsafe_code)]
    unsafe {
        Astronomy_TimeFromDays(ut)
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn sky_shadow(at: OffsetDateTime) -> ShadowAxis {
        sky::shadow_at(at)
    }

    fn eclipse_near(at: OffsetDateTime) -> Eclipse {
        nearest(at).expect("an eclipse near the date")
    }

    /// The point on the ellipsoid at a geodetic latitude and longitude.
    fn ground(latitude: f32, longitude: f32) -> Vec3 {
        let (lat, lon) = (latitude.to_radians(), longitude.to_radians());
        surface_point(
            Vec3::new(lat.cos() * lon.sin(), lat.sin(), lat.cos() * lon.cos()),
            0.0,
        )
    }

    /// Great circle distance in kilometers on a sphere of the mean radius.
    fn km_apart(a: (f32, f32), b: (f64, f64)) -> f64 {
        let (lat1, lon1) = (f64::from(a.0).to_radians(), f64::from(a.1).to_radians());
        let (lat2, lon2) = (b.0.to_radians(), b.1.to_radians());
        let cosine = lat1.sin() * lat2.sin() + lat1.cos() * lat2.cos() * (lon1 - lon2).cos();
        cosine.clamp(-1.0, 1.0).acos() * 6371.0
    }

    fn degrees_apart(a: (f32, f32), b: (f32, f32)) -> f64 {
        km_apart(a, (f64::from(b.0), f64::from(b.1))) / 6371.0 * 180.0 / std::f64::consts::PI
    }

    /// Points across the shadow at the depth of its ground point: from the
    /// axis outward past the penumbra's edge.
    fn across_the_shadow(shadow: &ShadowAxis, steps: u16, reach: f32) -> Vec<(f32, Vec3)> {
        let center = ground(shadow_center(shadow).0, shadow_center(shadow).1);
        let on_axis = shadow.moon + (center - shadow.moon).dot(shadow.axis) * shadow.axis;
        let sideways = shadow.axis.cross(Vec3::Y).normalize();
        (0..=steps)
            .map(|step| {
                let offset = reach * f32::from(step) / f32::from(steps);
                (offset, on_axis + offset * sideways)
            })
            .collect()
    }

    #[test]
    fn light_left_is_one_outside_the_penumbra_and_behind_the_moon() {
        let shadow = sky_shadow(eclipse_near(datetime!(2024-04-08 12:00 UTC)).peak);
        assert!((light_left(&shadow, shadow.moon - shadow.axis) - 1.0).abs() < f32::EPSILON);
        for (offset, point) in across_the_shadow(&shadow, 10, 3.0) {
            if offset > 0.7 {
                assert!(
                    (light_left(&shadow, point) - 1.0).abs() < f32::EPSILON,
                    "{offset} Earth radii off the axis"
                );
            }
        }
        let far = sky_shadow(datetime!(2024-04-22 18:00 UTC));
        assert!((light_left(&far, ground(0.0, 0.0)) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn the_light_falls_monotonically_to_nothing_on_a_total_eclipse_axis() {
        let shadow = sky_shadow(eclipse_near(datetime!(2024-04-08 12:00 UTC)).peak);
        let line = across_the_shadow(&shadow, 200, 0.7);
        assert!(light_left(&shadow, line[0].1) < 1e-6, "the axis is lit");
        for pair in line.windows(2) {
            assert!(
                light_left(&shadow, pair[1].1) >= light_left(&shadow, pair[0].1),
                "the light falls outward at {} Earth radii",
                pair[1].0
            );
        }
        assert!(light_left(&shadow, line[200].1) > 0.999);
    }

    #[test]
    fn the_antumbras_center_is_lit_and_darker_than_around_it() {
        let shadow = sky_shadow(eclipse_near(datetime!(2023-10-14 12:00 UTC)).peak);
        let line = across_the_shadow(&shadow, 100, 0.7);
        let center = light_left(&shadow, line[0].1);
        assert!(
            center > 0.0 && center < 0.2,
            "{center} at the antumbra's center"
        );
        for pair in line.windows(2) {
            // Inside the antumbra the light is flat up to rounding.
            assert!(light_left(&shadow, pair[1].1) >= light_left(&shadow, pair[0].1) - 1e-5);
        }
    }

    /// The same rule in f64 with the lens area worked from first principles.
    fn light_left_f64(shadow: &ShadowAxis, point: Vec3) -> f64 {
        let (moon, axis, moon_to_sun) = (
            DVec3::from(shadow.moon),
            DVec3::from(shadow.axis),
            f64::from(shadow.moon_to_sun),
        );
        let past = DVec3::from(point) - moon;
        let depth = past.dot(axis);
        if depth <= 0.0 {
            return 1.0;
        }
        let off_axis = (past - depth * axis).length();
        let sep = (off_axis / depth).atan() - (off_axis / (depth + moon_to_sun)).atan();
        let rm = (MOON_RADIUS_F64 / past.length()).asin();
        let rs = (SUN_RADIUS_F64 / (past + moon_to_sun * axis).length()).asin();
        let area = if sep >= rm + rs {
            0.0
        } else if sep <= (rm - rs).abs() {
            std::f64::consts::PI * rm.min(rs).powi(2)
        } else {
            let a0 = ((sep * sep + rs * rs - rm * rm) / (2.0 * sep * rs)).acos();
            let a1 = ((sep * sep + rm * rm - rs * rs) / (2.0 * sep * rm)).acos();
            let kite = 0.5
                * ((-sep + rs + rm) * (sep + rs - rm) * (sep - rs + rm) * (sep + rs + rm)).sqrt();
            rs * rs * a0 + rm * rm * a1 - kite
        };
        1.0 - area / (std::f64::consts::PI * rs * rs)
    }

    #[test]
    fn light_left_in_f32_agrees_with_an_f64_reference_through_the_umbra() {
        for at in [
            datetime!(2024-04-08 12:00 UTC),
            datetime!(2023-10-14 12:00 UTC),
        ] {
            let shadow = sky_shadow(eclipse_near(at).peak);
            for (offset, point) in across_the_shadow(&shadow, 400, 0.8) {
                let f32_value = f64::from(light_left(&shadow, point));
                let f64_value = light_left_f64(&shadow, point);
                assert!(
                    (f32_value - f64_value).abs() < 1e-3,
                    "{f32_value} against {f64_value} at {offset} Earth radii at {at}"
                );
            }
        }
    }

    #[test]
    fn a_surface_point_lies_on_the_ellipsoid_near_its_normal() {
        let pole = surface_point(Vec3::Y, 0.0);
        let equator = surface_point(Vec3::X, 0.0);
        approx::assert_relative_eq!(
            f64::from(pole.length()),
            EQUATORIAL_RADIUS_F64 * POLAR_RATIO,
            max_relative = 1e-6
        );
        approx::assert_relative_eq!(equator.length(), EQUATORIAL_RADIUS, max_relative = 1e-6);
        for step in 0..=90_u8 {
            let lat = f32::from(step).to_radians();
            let n = Vec3::new(lat.cos(), lat.sin(), 0.0);
            let point = surface_point(n, 0.0);
            assert!(
                point.angle_between(n).to_degrees() < 0.2,
                "{step} degrees: {}",
                point.angle_between(n).to_degrees()
            );
        }
    }

    /// The day, the map's Delta T in seconds, and P1, U1, U4 and P4 in the
    /// map's UT.
    type MapContacts = ((i32, u8, u8), f64, [Option<&'static str>; 4]);

    /// Espenak's maps, <https://eclipse.gsfc.nasa.gov/SEplot/SEplot2001/>,
    /// read 2026-10-29.
    const NASA_CONTACTS: [MapContacts; 6] = [
        (
            (2023, 10, 14),
            80.7,
            [
                Some("15:03:38.4"),
                Some("16:09:59.1"),
                Some("19:48:53.5"),
                Some("20:55:06.9"),
            ],
        ),
        (
            (2024, 4, 8),
            81.2,
            [
                Some("15:42:07.0"),
                Some("16:38:44.4"),
                Some("19:55:29.1"),
                Some("20:52:13.8"),
            ],
        ),
        (
            (2025, 3, 29),
            82.3,
            [Some("08:50:34.9"), None, None, Some("12:43:36.2")],
        ),
        (
            (2026, 2, 17),
            83.2,
            [
                Some("09:56:14.4"),
                Some("11:42:37.6"),
                Some("12:41:21.0"),
                Some("14:27:29.8"),
            ],
        ),
        (
            (2026, 8, 12),
            83.8,
            [
                Some("15:34:01.1"),
                Some("16:57:54.1"),
                Some("18:33:57.4"),
                Some("19:57:47.6"),
            ],
        ),
        (
            (2027, 8, 2),
            84.8,
            [
                Some("07:30:00.3"),
                Some("08:23:16.1"),
                Some("11:49:44.4"),
                Some("12:42:59.6"),
            ],
        ),
    ];

    fn seconds_of(hms: &str) -> f64 {
        let parts: Vec<f64> = hms.split(':').map(|part| part.parse().unwrap()).collect();
        parts[0] * 3600.0 + parts[1] * 60.0 + parts[2]
    }

    /// The contacts against NASA's, NASA's times moved onto Astronomy
    /// Engine's Delta T: the maps were drawn under a Delta T predicted then.
    #[test]
    fn the_contacts_agree_with_nasas_maps() {
        for ((year, month, day), map_delta_t, times) in NASA_CONTACTS {
            let date = time::Date::from_calendar_date(year, month.try_into().unwrap(), day)
                .unwrap()
                .midnight()
                .assume_utc();
            let found = found_from(date - Duration::days(1))
                .next()
                .expect("an eclipse");
            let eclipse = found.eclipse();
            let library_delta_t = (found.peak.tt - found.peak.ut) * 86_400.0;
            let ours = [
                Some(eclipse.contacts.p1),
                eclipse.contacts.u1,
                eclipse.contacts.u4,
                Some(eclipse.contacts.p4),
            ];
            for (index, (ours, nasa)) in ours.iter().zip(times).enumerate() {
                assert_eq!(ours.is_some(), nasa.is_some(), "contact {index} of {date}");
                let (Some(ours), Some(nasa)) = (ours, nasa) else {
                    continue;
                };
                let nasa = seconds_of(nasa) + map_delta_t - library_delta_t;
                let ours = (*ours - date).as_seconds_f64();
                assert!(
                    (ours - nasa).abs() < 15.0,
                    "contact {index} of {date}: {:+.1} s from NASA's",
                    ours - nasa
                );
            }
        }
    }

    fn eclipses_between(first_year: i32, last_year: i32) -> Vec<Eclipse> {
        (first_year..=last_year)
            .flat_map(|year| in_year(year, |_| UtcOffset::UTC))
            .collect()
    }

    #[test]
    fn the_contacts_are_in_order_around_the_peak() {
        for eclipse in eclipses_between(2020, 2030) {
            let c = eclipse.contacts;
            assert!(c.p1 < eclipse.peak && eclipse.peak < c.p4, "{eclipse:?}");
            assert_eq!(c.u1.is_some(), c.u4.is_some(), "{eclipse:?}");
            if let (Some(u1), Some(u4)) = (c.u1, c.u4) {
                assert!(c.p1 < u1 && u1 < u4 && u4 < c.p4, "{eclipse:?}");
            }
            if eclipse.kind != EclipseKind::Partial {
                assert!(
                    c.u1.is_some(),
                    "a central eclipse without an umbra: {eclipse:?}"
                );
            }
        }
    }

    #[test]
    fn next_and_previous_are_inverse() {
        let eclipses = eclipses_between(2020, 2030);
        assert!(eclipses.len() > 20);
        for pair in eclipses.windows(2) {
            assert_eq!(next_after(pair[0].peak).map(|e| e.peak), Some(pair[1].peak));
            assert_eq!(
                previous_before(pair[1].peak).map(|e| e.peak),
                Some(pair[0].peak)
            );
        }
    }

    #[test]
    fn previous_and_next_stop_at_the_range_ends() {
        let first = next_after(EARLIEST).expect("an eclipse in 1900");
        assert_eq!(first.peak.year(), 1900);
        assert_eq!(previous_before(first.peak), None);
        let last = previous_before(LATEST).expect("an eclipse in 2100");
        assert_eq!(last.peak.year(), 2100);
        assert_eq!(next_after(last.peak), None);
    }

    #[test]
    fn on_earth_is_between_the_penumbras_contacts() {
        let eclipse = eclipse_near(datetime!(2026-08-12 12:00 UTC));
        let c = eclipse.contacts;
        for at in [
            c.p1 + Duration::seconds(30),
            eclipse.peak,
            c.p4 - Duration::seconds(30),
        ] {
            assert_eq!(on_earth_at(at).map(|e| e.peak), Some(eclipse.peak), "{at}");
        }
        assert_eq!(on_earth_at(c.p1 - Duration::hours(1)), None);
        assert_eq!(on_earth_at(c.p4 + Duration::hours(1)), None);
        for shift in [Duration::days(-1), Duration::ZERO, Duration::days(1)] {
            assert_eq!(eclipse_near(eclipse.peak + shift).peak, eclipse.peak);
        }
    }

    #[test]
    fn the_central_durations_agree_with_nasas() {
        for (at, nasa) in [
            (datetime!(2024-04-08 12:00 UTC), 4.0 * 60.0 + 28.1),
            (datetime!(2027-08-02 12:00 UTC), 6.0 * 60.0 + 22.6),
        ] {
            let duration = eclipse_near(at)
                .central_duration
                .expect("a central eclipse")
                .as_seconds_f64();
            assert!(
                (duration - nasa).abs() < 10.0,
                "{duration} s against {nasa} s"
            );
        }
    }

    #[test]
    fn the_shadows_center_at_the_peak_is_the_librarys_peak_point() {
        let start = datetime!(2026-01-01 00:00 UTC);
        let end = datetime!(2031-01-01 00:00 UTC);
        let mut central = 0;
        for found in found_from(start).take_while(|found| found.instant() < end) {
            if found.kind == EclipseKind::Partial {
                continue;
            }
            central += 1;
            let center = shadow_center(&sky_shadow(found.instant()));
            let apart = km_apart(center, (found.latitude, found.longitude));
            assert!(apart < 2.0, "{apart} km at {}", found.instant());
        }
        assert!(central >= 8);
        let partial = eclipse_near(datetime!(2025-03-29 12:00 UTC));
        assert_eq!(partial.kind, EclipseKind::Partial);
        let center = shadow_center(&sky_shadow(partial.peak));
        assert!(
            (center.0 - 61.0).abs() < 1.0 && (center.1 + 77.0).abs() < 1.0,
            "{center:?}"
        );
    }

    #[test]
    fn the_followed_point_moves_slowly_and_stays_near_the_shadow() {
        let eclipse = eclipse_near(datetime!(2024-04-08 12:00 UTC));
        let mut at = eclipse.contacts.p1;
        let mut previous = followed_point(&sky_shadow(at));
        while at <= eclipse.contacts.p4 {
            at += Duration::seconds(10);
            let shadow = sky_shadow(at);
            let followed = followed_point(&shadow);
            let per_minute = degrees_apart(previous, followed) * 6.0;
            assert!(per_minute < 3.0, "{per_minute} degrees a minute at {at}");
            if axis_point(&shadow, 0.0).meets {
                let off = degrees_apart(followed, shadow_center(&shadow));
                assert!(off < 11.0, "{off} degrees off the shadow at {at}");
            }
            previous = followed;
        }
    }

    #[test]
    fn the_penumbra_reaches_the_shells_only_during_an_eclipse() {
        let eclipse = eclipse_near(datetime!(2024-04-08 12:00 UTC));
        let shell = crate::params::RAYLEIGH_RADIUS;
        assert!(penumbra_reaches(&sky_shadow(eclipse.peak), shell));
        assert!(penumbra_reaches(&sky_shadow(eclipse.contacts.p1), shell));
        assert!(!penumbra_reaches(
            &sky_shadow(eclipse.contacts.p1 - Duration::minutes(10)),
            shell
        ));
        assert!(!penumbra_reaches(
            &sky_shadow(eclipse.contacts.p4 + Duration::minutes(10)),
            shell
        ));
        assert!(!penumbra_reaches(
            &sky_shadow(datetime!(2024-04-22 18:00 UTC)),
            shell
        ));
    }
}
