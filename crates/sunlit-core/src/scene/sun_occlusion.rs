//! Where the Sun lands on screen, and how much of it the viewer can see.
//!
//! The composite draws the globe through the camera's 20 degree perspective
//! lens and everything celestial through the stereographic sky lens, which
//! puts the painted globe about four and a half times larger on screen than
//! the sky lens's own image of the same silhouette. Occlusion therefore has to
//! be measured against the silhouette the viewer can see rather than against
//! the true angular limb: the second would fade the glare out while the Sun
//! still sits in visibly empty sky in one framing, and leave it burning on top
//! of the painted globe in another. The cost is that the answer no longer
//! agrees with an ephemeris, which is the price of a coherent picture under a
//! mixed lens.
//!
//! Screen space is where the comparison is possible, because every shape in it
//! is a circle. The sky lens is conformal, so a cone about a direction images
//! as a disc; the projected plane reaches pixels by one uniform scale, so that
//! disc is a circle in pixels; and the globe's silhouette is a circle there
//! too. Pixels are also the one space both lenses agree is isotropic.
//!
//! [`angular_visible_fraction`] is the ephemeris answer, computed from the
//! true angles and read by nothing in the renderer. It is the second
//! implementation the tests compare the screen-space one against, the same
//! role `scene::sun` plays for the sun direction.

use std::f32::consts::PI;

use glam::{Mat4, Vec2, Vec3, Vec4};

/// Angular radius of the Sun's disk seen from Earth, in degrees. The seasonal
/// variation (0.262 to 0.271) is below a pixel at any output this renders.
pub(crate) const SUN_ANGULAR_RADIUS_DEGREES: f32 = 0.267;

/// Smallest radius either half-degree body is drawn at, in pixels at 1080p.
///
/// At the default sky field of view the true disk is about 13 pixels across on
/// a 4K render and under one pixel on a small preview, so without a floor the
/// Sun collapses to a speck exactly where a user is tuning it. The star
/// sprites carry the same clause for the same reason. The Moon subtends the
/// same half degree and takes the same floor, or the two bodies that are the
/// same size in the sky would be different sizes on screen wherever it is
/// active.
pub(crate) const MIN_BODY_DISK_RADIUS_PIXELS: f32 = 1.6;

/// Output-density ramp shared with the star sprites: 1.0 at 1080p and below,
/// 2.0 at 4K and above.
pub fn pixel_scale(viewport_height: f32) -> f32 {
    (viewport_height / 1080.0).clamp(1.0, 2.0)
}

/// Earth's radius in kilometers, which is what turns a shell radius in Earth
/// radii into the height of the band it stands for.
pub(crate) const EARTH_RADIUS_KM: f32 = 6371.0;

/// Scale height of the density the light path runs through, in kilometers.
///
/// One number serves the extinction and the refraction alike: `70 exp(-h / 7)`
/// reproduces Mallama's cumulative air masses to about a fifth over the whole
/// band, and Kipping's ray tracing fits 6.911 km to the lensing.
const ATMOSPHERE_SCALE_HEIGHT_KM: f32 = 7.0;

/// Air masses along a ray that grazes the surface, from Mallama's table.
const HORIZON_AIR_MASS: f32 = 70.0;

/// Transmission per air mass for red, green and blue.
///
/// Mallama states 90 percent for red and 73 percent for blue; green is fitted
/// to his own altitude and transmission pairs, and Kipping's lowtran7 figures
/// for a sea-level path give the same chromatic ratio from another direction.
const CHANNEL_TRANSMISSION: [f32; 3] = [0.90, 0.836, 0.73];

/// Cumulative air mass along a light path whose lowest point is `height_km`.
///
/// Below the surface the path is the surface-grazing one: nothing carries more
/// atmosphere than the whole of it, and the exponential would otherwise run
/// away where the disk sits behind the painted limb.
#[must_use]
pub(crate) fn limb_air_mass(height_km: f32, reddening: f32) -> f32 {
    HORIZON_AIR_MASS * (-height_km.max(0.0) / ATMOSPHERE_SCALE_HEIGHT_KM).exp() * reddening
}

/// What that path leaves of the light, per channel.
///
/// Mirrored by `limb_transmission_km` in `sphere.wgsl`, which is what colors
/// the Sun's disk per fragment and the Rayleigh shell's forward lobe; this
/// spelling is what the CPU integrates over the visible disk. `reddening` zero
/// is the white Sun that knows nothing about the band.
#[must_use]
pub fn limb_transmission(height_km: f32, reddening: f32) -> Vec3 {
    let air_mass = limb_air_mass(height_km, reddening);
    Vec3::new(
        CHANNEL_TRANSMISSION[0].powf(air_mass),
        CHANNEL_TRANSMISSION[1].powf(air_mass),
        CHANNEL_TRANSMISSION[2].powf(air_mass),
    )
}

/// The same light as a hue: divided by its largest channel, which red always
/// is. What the disk and the glare are tinted with, since both are clipped
/// bright long before the path stops carrying anything.
#[must_use]
#[cfg(test)]
pub(crate) fn limb_hue(height_km: f32, reddening: f32) -> Vec3 {
    let transmitted = limb_transmission(height_km, reddening);
    transmitted / transmitted.max_element().max(1e-30)
}

/// How far the disk is drawn where the path carries almost nothing.
///
/// A tenth of the Sun is still a blinding source, so the disk clips at full
/// brightness wherever the green channel carries more than 2.5 percent, about
/// nine kilometers, and fades out below that instead of being cut where the
/// painted globe begins. Mirrored by `limb_disk_amplitude` in `sphere.wgsl`.
#[must_use]
pub fn limb_disk_amplitude(green_transmission: f32) -> f32 {
    (green_transmission * DISK_FADE_GAIN).clamp(0.0, 1.0).sqrt()
}

/// Where the disk stops being clipped white, as a gain on the green channel.
const DISK_FADE_GAIN: f32 = 40.0;

/// A circle on the framebuffer, in pixels, with y running down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenCircle {
    pub center: Vec2,
    pub radius: f32,
}

/// How much of the Sun's disk the viewer can see.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunVisibility {
    /// Fraction of the disk's area outside the globe's painted silhouette.
    pub visible_fraction: f32,
}

/// The frame geometry [`place_sun`] needs, bundled because it is nine values
/// and every one of them is already sitting in the uniform encoder.
#[derive(Clone, Copy, Debug)]
pub struct SunPlacementInputs {
    /// Geocentric sun direction in world space.
    pub sun_world_direction: Vec3,
    /// The camera's view matrix, shared by both lenses.
    pub view: Mat4,
    /// The Earth camera's full transform, used only to project the origin.
    pub mvp: Mat4,
    pub eye_distance: f32,
    pub sky_fov_deg: f32,
    /// The Earth camera's vertical field of view.
    pub camera_fov_deg: f32,
    /// Radius of the atmosphere shell, which sets the height of the band the
    /// light path runs through and the width of the horizon zone.
    pub atmosphere_radius: f32,
    /// Post-projection pan, in NDC, as the sky lens applies it.
    pub screen_offset: Vec2,
    pub viewport: Vec2,
    /// The Moon's screen circle, when the Moon is drawn.
    ///
    /// `None` covers a Moon switched off and a Moon with no texture behind it:
    /// a glare fading behind something invisible is as incoherent as one
    /// burning around a Moon that hides the disk.
    pub moon_disc: Option<ScreenCircle>,
    pub horizon: SunHorizonParams,
}

/// The five horizon controls and the size, bundled because they travel
/// together from `SceneParams` into every function below.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunHorizonParams {
    /// Multiplier on the disk's angular radius, before the pixel floor.
    pub size: f32,
    /// Thickness of the horizon zone in Sun diameters. Zero leaves the zone at
    /// the painted annulus, which is the physically thin band.
    pub depth: f32,
    /// Scale on the air mass. Zero is a Sun the band does not touch.
    pub reddening: f32,
    /// Scale on the lift and the flattening. Zero is the geometric Sun.
    pub refraction: f32,
    /// How much brighter the glare peaks as the disk clears the zone.
    pub boost: f32,
    /// How far above the zone, in zone widths, that peak decays away.
    pub reach: f32,
}

impl SunHorizonParams {
    /// The settings that leave the Sun exactly where geometry puts it, white,
    /// unmagnified and unboosted: what everything here answered before the
    /// band existed.
    #[cfg(test)]
    pub(crate) const GEOMETRIC: Self = Self {
        size: 1.0,
        depth: 0.0,
        reddening: 0.0,
        refraction: 0.0,
        boost: 1.0,
        reach: 1.0,
    };
}

/// Everything the uniform encoder needs to place, color and fade the Sun.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunPlacement {
    /// Sun direction in view space, unit length, after refraction has lifted
    /// it. The shader rebuilds the screen position from this, so there is one
    /// formula and not two.
    pub view_direction: Vec3,
    /// The disk's radius in pixels, never below
    /// [`MIN_BODY_DISK_RADIUS_PIXELS`] times the density ramp. This is the
    /// unsquashed radius, which is what the quad has to span.
    pub disk_radius_pixels: f32,
    pub visibility: SunVisibility,
    /// The painted globe's silhouette, which every horizon effect is measured
    /// against.
    pub globe: ScreenCircle,
    /// Width of the horizon zone in pixels.
    pub zone_width_pixels: f32,
    /// Vertical magnification of the refracted disk: one where nothing bends.
    pub squash: f32,
    /// Mean transmitted hue of the visible disk, weighted by what of it is
    /// drawn. What the glare is made of.
    pub glare_tint: Vec3,
    /// Visible area times what the path through the band transmits: the
    /// illuminance the source delivers, which is what veiling glare scales
    /// with. One for a disk clear of the band.
    pub flux: f32,
    /// The exposure gain, an eye that has not caught up with the disk.
    pub horizon_gain: f32,
}

/// The horizontal half-extent of the sky lens in projected-plane units.
///
/// A direction `theta` from the view axis lands at `tan(theta / 2)`, and the
/// frame's horizontal edge is `sky_fov / 2` from the axis, so this is the
/// divisor that puts that edge at NDC 1. Mirrors `sky_lens_edge_radius` in
/// `sphere.wgsl`, clamp included. The upper end is the widest sky a spanned
/// canvas may derive, not the slider's 180; `display::layout::SKY_FOV_MAX` is
/// the same number.
pub fn sky_lens_edge_radius(sky_fov_deg: f32) -> f32 {
    (sky_fov_deg.clamp(60.0, 330.0) * PI / 720.0).tan()
}

/// The image of a cone of half-angle `half_angle` about `view_dir`, in pixels.
///
/// Stereographic projection maps circles on the sphere to circles in the
/// plane, so this is exact rather than a small-angle approximation. The cone's
/// extremes along the radial direction are the images of `theta - half_angle`
/// and `theta + half_angle`; keeping the first signed is what makes a cone
/// that contains the view axis straddle the origin with no special case.
///
/// `None` when the cone reaches the antipode, where the image is the exterior
/// of a circle and no finite radius describes it.
pub fn sky_lens_disc(
    view_dir: Vec3,
    half_angle: f32,
    sky_fov_deg: f32,
    screen_offset: Vec2,
    viewport: Vec2,
) -> Option<ScreenCircle> {
    let dir = view_dir.normalize_or_zero();
    let theta = (-dir.z).clamp(-1.0, 1.0).acos();
    if theta + half_angle >= PI {
        return None;
    }
    let transverse = dir.truncate();
    let radial = if transverse.length() > 1e-6 {
        transverse.normalize()
    } else {
        Vec2::X
    };
    let edge = sky_lens_edge_radius(sky_fov_deg);
    let near = ((theta - half_angle) * 0.5).tan() / edge;
    let far = ((theta + half_angle) * 0.5).tan() / edge;
    let aspect = viewport.x / viewport.y;
    let ndc = radial * ((near + far) * 0.5) * Vec2::new(1.0, aspect) + screen_offset;
    Some(ScreenCircle {
        center: ndc_to_pixels(ndc, viewport),
        // The projected plane reaches pixels by one uniform scale, so the disc
        // stays a circle whatever the aspect ratio is.
        radius: (far - near) * 0.5 * viewport.x * 0.5,
    })
}

/// The painted silhouette of a sphere of radius `body_radius` at the origin.
///
/// Its tangent-plane radius is `r / sqrt(d^2 - r^2)`, which the vertical field
/// of view normalizes to NDC. The center is the projected origin, exact on the
/// view axis; off it the true silhouette drifts outward and this stays put, an
/// approximation that is best where the globe is the subject of the frame,
/// which is every framing this renderer is for.
pub fn globe_screen_circle(
    mvp: Mat4,
    eye_distance: f32,
    body_radius: f32,
    camera_fov_deg: f32,
    viewport: Vec2,
) -> ScreenCircle {
    let clip = mvp * Vec4::new(0.0, 0.0, 0.0, 1.0);
    let ndc = if clip.w.abs() > 1e-6 {
        Vec2::new(clip.x / clip.w, clip.y / clip.w)
    } else {
        Vec2::ZERO
    };
    let horizon = (eye_distance * eye_distance - body_radius * body_radius).max(1e-6);
    let radius_ndc = body_radius / horizon.sqrt() / (camera_fov_deg * 0.5).to_radians().tan();
    ScreenCircle {
        center: ndc_to_pixels(ndc, viewport),
        radius: radius_ndc * viewport.y * 0.5,
    }
}

/// How far the atmosphere lifts a ray that grazes the surface, in zone widths.
///
/// 65.5 arcminutes at the station's horizon is 44 km of the ray's own lowest
/// altitude, and the band from the surface to the shell is 96 km of it.
const REFRACTION_LIFT_ZONES: f32 = 0.46;

/// How fast that lift falls off with height, in reciprocal zone widths: the
/// 96 km band over the 7 km scale height.
const REFRACTION_EXPONENT: f32 = 13.7;

/// Where a disk at geometric height `height` in zone widths is seen, and how
/// far the same bending flattens it.
///
/// The forward map is `height = apparent - lift * exp(-k * apparent)`, whose
/// slope is what magnifies the image, so the vertical magnification is that
/// slope's inverse. The map is monotonic and reaches `-lift` at an apparent
/// height of zero; below that the lift saturates, because no ray bends by more
/// than the whole atmosphere is worth and the exponential would otherwise run
/// away behind the painted limb.
///
/// At `refraction` zero this is the identity with no magnification at all,
/// which is what leaves the geometric Sun exactly where it was.
#[must_use]
pub(crate) fn refract(height: f32, refraction: f32) -> (f32, f32) {
    let lift = REFRACTION_LIFT_ZONES * refraction;
    if lift <= 0.0 {
        return (height, 1.0);
    }
    let slope = REFRACTION_EXPONENT * lift;
    if height <= -lift {
        return (height + lift, 1.0 / (1.0 + slope));
    }
    let mut apparent = (height + lift * (-REFRACTION_EXPONENT * height.max(0.0)).exp()).max(0.0);
    for _ in 0..12 {
        let falloff = (-REFRACTION_EXPONENT * apparent).exp();
        let residual = apparent - lift * falloff - height;
        apparent = (apparent - residual / (1.0 + slope * falloff)).max(0.0);
    }
    let falloff = (-REFRACTION_EXPONENT * apparent).exp();
    (apparent, 1.0 / (1.0 + slope * falloff))
}

/// The view-space direction a framebuffer position looks along.
///
/// The analytic inverse of [`sky_lens_disc`] at a zero half-angle, and the same
/// inversion `sky_lens_direction` performs in `sphere.wgsl`. Refraction is the
/// one thing that needs it here: the lift is a distance on screen, and the
/// honest way to turn it back into a direction is the lens itself rather than a
/// second linearization of it.
#[must_use]
pub fn sky_lens_direction(
    position: Vec2,
    sky_fov_deg: f32,
    screen_offset: Vec2,
    viewport: Vec2,
) -> Vec3 {
    let ndc = Vec2::new(
        position.x / viewport.x * 2.0 - 1.0,
        1.0 - position.y / viewport.y * 2.0,
    );
    let aspect = viewport.x / viewport.y;
    let projected = (ndc - screen_offset) / Vec2::new(1.0, aspect);
    let length = projected.length();
    let radial = if length > 1e-6 {
        projected / length
    } else {
        Vec2::X
    };
    let theta = 2.0 * (length * sky_lens_edge_radius(sky_fov_deg)).atan();
    Vec3::new(radial.x * theta.sin(), radial.y * theta.sin(), -theta.cos())
}

/// Strips the disk's radial extent is integrated over for the glare's color and
/// flux. Fine enough that the sum reproduces the closed-form visible area to
/// well under a percent, and it runs once a frame.
const FLUX_STRIPS: usize = 64;

/// The asymmetry parameter that puts the forward lobe's half strength at
/// `half_width_deg` of scattering angle.
///
/// Henyey-Greenstein normalized to one at zero scattering angle is
/// `((1 - g)^2 / (1 + g^2 - 2 g cos t))^1.5`; setting that to a half at `t`
/// gives a quadratic in `g` whose two roots are reciprocals, so the forward one
/// is the smaller. Derived here rather than in the shader because it is one
/// solve a frame against one per rim fragment.
#[must_use]
pub(crate) fn henyey_greenstein_asymmetry(half_width_deg: f32) -> f32 {
    let cosine = half_width_deg.to_radians().cos();
    let half = 0.5_f32.powf(2.0 / 3.0);
    let sum = (2.0 - 2.0 * half * cosine) / (1.0 - half);
    let root = (sum * sum - 4.0).max(0.0).sqrt();
    (0.5 * (sum - root)).clamp(0.0, 0.995)
}

/// The exposure gain: how much brighter the glare is than its flux says.
///
/// Neither an eye exposed for the night side nor a camera has caught up while
/// an orbital sunrise happens, and a still frame has no time axis, so the lag
/// is mapped onto how far the disk's lower edge has climbed above the zone. It
/// holds at `boost` until the whole disk stands clear and settles to one over
/// `reach` zone widths above that, which is the sequence a rising Sun reads as.
#[must_use]
pub(crate) fn exposure_gain(lower_edge_height: f32, boost: f32, reach: f32) -> f32 {
    let settled = smoothstep(1.0, 1.0 + reach.max(1e-3), lower_edge_height);
    1.0 + (boost - 1.0) * (1.0 - settled)
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The mean hue of the visible disk and the mean of what its light path
/// transmits, both over the part of the disk the globe leaves showing.
///
/// Strips of constant distance from the globe's center, because that distance
/// is what the band's height is measured along: every point of one strip looks
/// through the same path. Each strip contributes the disk's own arc there, so
/// the weights alone integrate to the visible area; the visible fraction itself
/// comes from the closed form rather than from this sum, which is what keeps a
/// disk clear of the band at exactly one.
///
/// The hue is weighted by what of the disk is drawn: a part of it the band has
/// already taken to nothing contributes no light to a glare made of the light
/// that reached the eye.
#[allow(clippy::too_many_arguments, clippy::cast_precision_loss)]
fn integrate_disk(
    center_distance: f32,
    disk_radius: f32,
    limb_distance: f32,
    globe_radius: f32,
    zone_width: f32,
    band_height_km: f32,
    squash: f32,
    reddening: f32,
) -> (Vec3, f32) {
    if center_distance <= 1e-4 || disk_radius <= 0.0 {
        return (Vec3::ONE, 0.0);
    }
    let lowest = (center_distance - disk_radius).max(limb_distance);
    let highest = center_distance + disk_radius;
    if highest <= lowest {
        return (Vec3::ONE, 0.0);
    }
    let step = (highest - lowest) / FLUX_STRIPS as f32;
    let mut weight_total = 0.0;
    let mut transmitted_total = 0.0;
    let mut hue_total = Vec3::ZERO;
    let mut drawn_total = 0.0;
    let mut highest_hue = Vec3::ONE;
    for strip in 0..FLUX_STRIPS {
        let radius = lowest + (strip as f32 + 0.5) * step;
        let cosine = ((radius * radius + center_distance * center_distance
            - disk_radius * disk_radius)
            / (2.0 * radius * center_distance))
            .clamp(-1.0, 1.0);
        let weight = 2.0 * radius * cosine.acos() * step;
        // Where this strip is drawn once refraction has flattened the disk
        // about its own center.
        let drawn = center_distance + squash * (radius - center_distance);
        let height = (drawn - globe_radius) / zone_width * band_height_km;
        let transmitted = limb_transmission(height, reddening);
        let hue = transmitted / transmitted.max_element().max(1e-30);
        let amplitude = limb_disk_amplitude(transmitted.y);
        weight_total += weight;
        transmitted_total += weight * transmitted.y;
        hue_total += weight * amplitude * hue;
        drawn_total += weight * amplitude;
        highest_hue = hue;
    }
    let mean_transmission = if weight_total > 0.0 {
        transmitted_total / weight_total
    } else {
        0.0
    };
    let tint = if drawn_total > 1e-6 {
        hue_total / drawn_total
    } else {
        highest_hue
    };
    (tint, mean_transmission)
}

/// How thick the horizon zone is, in pixels.
///
/// The painted annulus is the physical reading, and it is one or two pixels on
/// a preview at the default zooms, which is not enough for a gradient across
/// the disk to exist at all. `depth` widens it in disk diameters, which is also
/// about what the station sees: the band is four and a half disks there and the
/// disk spans a fifth of it.
fn horizon_zone_width(
    globe_radius: f32,
    atmosphere_radius: f32,
    depth: f32,
    disk_radius: f32,
) -> f32 {
    let annulus = (atmosphere_radius - globe_radius).max(0.0);
    annulus.max(depth * 2.0 * disk_radius).max(1e-4)
}

/// Place the Sun, color it by the path its light took, and measure what the
/// globe hides of it.
#[allow(clippy::too_many_lines)]
pub fn place_sun(inputs: &SunPlacementInputs) -> SunPlacement {
    let horizon = inputs.horizon;
    let true_direction = (inputs.view * inputs.sun_world_direction.extend(0.0))
        .truncate()
        .normalize_or_zero();
    let floor = MIN_BODY_DISK_RADIUS_PIXELS * pixel_scale(inputs.viewport.y);
    let half_angle = (SUN_ANGULAR_RADIUS_DEGREES * horizon.size).to_radians();
    let globe = globe_screen_circle(
        inputs.mvp,
        inputs.eye_distance,
        1.0,
        inputs.camera_fov_deg,
        inputs.viewport,
    );
    let atmosphere = globe_screen_circle(
        inputs.mvp,
        inputs.eye_distance,
        inputs.atmosphere_radius,
        inputs.camera_fov_deg,
        inputs.viewport,
    );
    let band_height_km = (inputs.atmosphere_radius - 1.0) * EARTH_RADIUS_KM;
    let imaged = |direction: Vec3| {
        sky_lens_disc(
            direction,
            half_angle,
            inputs.sky_fov_deg,
            inputs.screen_offset,
            inputs.viewport,
        )
        .map(|mut disc| {
            disc.radius = disc.radius.max(floor);
            disc
        })
    };
    let Some(disc) = imaged(true_direction) else {
        // The Sun sits at the antipode of the view axis, off screen whatever
        // the globe does: nothing hides it, and nothing draws it either.
        return SunPlacement {
            view_direction: true_direction,
            disk_radius_pixels: floor,
            visibility: SunVisibility {
                visible_fraction: 1.0,
            },
            globe,
            zone_width_pixels: horizon_zone_width(
                globe.radius,
                atmosphere.radius,
                horizon.depth,
                floor,
            ),
            squash: 1.0,
            glare_tint: Vec3::ONE,
            flux: 1.0,
            horizon_gain: 1.0,
        };
    };

    let zone = horizon_zone_width(globe.radius, atmosphere.radius, horizon.depth, disc.radius);
    let geometric_height = (disc.center.distance(globe.center) - globe.radius) / zone;
    let (apparent_height, squash) = refract(geometric_height, horizon.refraction);
    let lift = (apparent_height - geometric_height) * zone;
    // A lift of nothing leaves the direction untouched rather than sending it
    // through the lens and back, so a Sun the band cannot reach does not move
    // by whatever that round trip costs in the last bits.
    let (view_direction, disc) = if lift > 0.0 {
        let radial = (disc.center - globe.center)
            .try_normalize()
            .unwrap_or(Vec2::X);
        let lifted = sky_lens_direction(
            disc.center + radial * lift,
            inputs.sky_fov_deg,
            inputs.screen_offset,
            inputs.viewport,
        );
        imaged(lifted).map_or((true_direction, disc), |moved| (lifted, moved))
    } else {
        (true_direction, disc)
    };

    // Flattening the disk about its own center is the same, in area fractions,
    // as leaving it round and moving the limb that cuts it: the scale runs
    // along the cut's own normal, so it takes every fraction with it.
    let center_distance = disc.center.distance(globe.center);
    let spread = 1.0 / squash - 1.0;
    let moved = |radius: f32| radius + spread * (radius - center_distance);
    let visibility = visibility(
        disc,
        ScreenCircle {
            center: globe.center,
            radius: moved(globe.radius),
        },
        inputs.moon_disc,
    );

    let (glare_tint, mean_transmission) = integrate_disk(
        center_distance,
        disc.radius,
        moved(globe.radius),
        globe.radius,
        zone,
        band_height_km,
        squash,
        horizon.reddening,
    );
    let lower_edge = (center_distance - squash * disc.radius - globe.radius) / zone;
    SunPlacement {
        view_direction,
        disk_radius_pixels: disc.radius,
        visibility,
        globe,
        zone_width_pixels: zone,
        squash,
        glare_tint,
        flux: visibility.visible_fraction * mean_transmission,
        horizon_gain: exposure_gain(lower_edge, horizon.boost, horizon.reach),
    }
}

/// Split the Sun's disk against the two concentric silhouettes and the Moon.
///
/// The globe and the Moon are both occluders, and where both cover the disk at
/// once the two hidden areas combine as the larger of the two rather than as
/// their exact union. That is exact wherever only one of them is over the disk,
/// which is every frame that is not an eclipse at the limb, and errs toward
/// more glare in the one that is.
///
/// The Moon comparison carries none of the mixed-lens caveat the globe one
/// does: the Sun and the Moon are both imaged by the sky lens, so their circles
/// are in the same lens and the answer agrees with an ephemeris.
fn visibility(sun: ScreenCircle, globe: ScreenCircle, moon: Option<ScreenCircle>) -> SunVisibility {
    let behind_globe = overlap_area(sun.radius, globe.radius, sun.center.distance(globe.center));
    let behind_moon = moon.map_or(0.0, |moon| {
        overlap_area(sun.radius, moon.radius, sun.center.distance(moon.center))
    });
    let hidden = behind_globe.max(behind_moon);
    let area = PI * sun.radius * sun.radius;
    if area <= 0.0 {
        let outside = |circle: ScreenCircle| sun.center.distance(circle.center) > circle.radius;
        let clear = outside(globe) && moon.is_none_or(outside);
        return SunVisibility {
            visible_fraction: f32::from(u8::from(clear)),
        };
    }
    SunVisibility {
        visible_fraction: (1.0 - hidden / area).clamp(0.0, 1.0),
    }
}

/// The ephemeris answer: what fraction of the Sun's disk clears the true
/// angular limb of a sphere of radius `body_radius` at the origin.
///
/// Read by nothing in the renderer. It exists so the screen-space function has
/// something independent to be checked against in the one configuration where
/// the two lenses agree about scale.
#[cfg(test)]
pub(crate) fn angular_visible_fraction(sun_direction: Vec3, eye: Vec3, body_radius: f32) -> f32 {
    let distance = eye.length();
    if distance <= body_radius {
        return 0.0;
    }
    let limb = (body_radius / distance).asin();
    let sun_radius = SUN_ANGULAR_RADIUS_DEGREES.to_radians();
    let separation = sun_direction
        .normalize_or_zero()
        .angle_between(-eye.normalize_or_zero());
    let hidden = overlap_area(sun_radius, limb, separation);
    (1.0 - hidden / (PI * sun_radius * sun_radius)).clamp(0.0, 1.0)
}

/// Area shared by two circles whose centers are `distance` apart.
fn overlap_area(r0: f32, r1: f32, distance: f32) -> f32 {
    if r0 <= 0.0 || r1 <= 0.0 || distance >= r0 + r1 {
        return 0.0;
    }
    if distance <= (r0 - r1).abs() {
        let contained = r0.min(r1);
        return PI * contained * contained;
    }
    let d = distance;
    let a0 = ((d * d + r0 * r0 - r1 * r1) / (2.0 * d * r0))
        .clamp(-1.0, 1.0)
        .acos();
    let a1 = ((d * d + r1 * r1 - r0 * r0) / (2.0 * d * r1))
        .clamp(-1.0, 1.0)
        .acos();
    let kite = 0.5
        * ((-d + r0 + r1) * (d + r0 - r1) * (d - r0 + r1) * (d + r0 + r1))
            .max(0.0)
            .sqrt();
    r0 * r0 * a0 + r1 * r1 * a1 - kite
}

fn ndc_to_pixels(ndc: Vec2, viewport: Vec2) -> Vec2 {
    Vec2::new(
        (ndc.x + 1.0) * 0.5 * viewport.x,
        (1.0 - ndc.y) * 0.5 * viewport.y,
    )
}

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;

    use super::*;
    use crate::scene::camera::OrbitalCamera;

    /// The silhouette to measure a sun disk against: 50 pixels around
    /// (100, 100).
    fn globe() -> ScreenCircle {
        ScreenCircle {
            center: Vec2::new(100.0, 100.0),
            radius: 50.0,
        }
    }

    fn sun_at(x: f32, radius: f32) -> ScreenCircle {
        ScreenCircle {
            center: Vec2::new(x, 100.0),
            radius,
        }
    }

    // --- the light path through the band ---

    /// Mallama's Table 3.2: the lowest altitude of the ray, the cumulative air
    /// mass along it, and what fraction of green light comes out the far side.
    const MALLAMA_TABLE: [(f32, f32, f32); 11] = [
        (32.0, 0.6, 0.90),
        (27.0, 1.3, 0.79),
        (22.0, 2.7, 0.62),
        (20.0, 3.8, 0.51),
        (18.0, 5.0, 0.41),
        (15.0, 8.5, 0.22),
        (13.0, 13.0, 0.10),
        (8.0, 24.0, 0.014),
        (5.0, 37.0, 0.0013),
        (2.6, 50.0, 0.0001),
        (0.8, 62.0, 0.00001),
    ];

    #[test]
    fn the_exponential_reproduces_the_measured_air_masses() {
        for (height, air_mass, _) in MALLAMA_TABLE {
            let modeled = limb_air_mass(height, 1.0);
            let ratio = modeled / air_mass;
            assert!(
                (0.79..=1.21).contains(&ratio),
                "at {height} km the table has {air_mass} air masses and the model {modeled}"
            );
        }
    }

    /// Transmission is exponential in air mass, so the fifth the fit is worth
    /// at the top of the band is a factor of one and a half at the bottom of
    /// it. Twenty percent holds where the air mass is small, and the 13 km row
    /// is where the fit gives up; `docs/rendering.md` has the measured
    /// disagreement there.
    #[test]
    fn the_transmitted_green_follows_the_table_where_the_air_mass_is_small() {
        for (height, _, transmitted) in MALLAMA_TABLE.iter().take(6) {
            let modeled = limb_transmission(*height, 1.0).y;
            let ratio = modeled / transmitted;
            assert!(
                (0.8..=1.2).contains(&ratio),
                "at {height} km the table transmits {transmitted} of the green and the model {modeled}"
            );
        }
        let deep = limb_transmission(13.0, 1.0).y;
        assert!(
            (deep / 0.10 - 1.412).abs() < 0.05,
            "the 13 km row is the one the exponential fit misses; got {deep}"
        );
    }

    #[test]
    fn the_band_reddens_downward_and_whitens_upward() {
        let mut previous = limb_hue(0.0, 1.0);
        assert!(
            previous.y < 0.02 && previous.z < 0.001,
            "a ray that grazes the surface is red, got {previous}"
        );
        for step in 1..=48 {
            #[allow(clippy::cast_precision_loss)]
            let hue = limb_hue(step as f32 * 2.0, 1.0);
            assert!(
                hue.y >= previous.y && hue.z >= previous.z,
                "the band has to warm downward and cool upward, but {hue} sits above {previous}"
            );
            previous = hue;
        }
        let high = limb_hue(50.0, 1.0);
        assert!(
            (high - Vec3::ONE).abs().max_element() < 0.02,
            "above the weather the path takes nothing out, got {high}"
        );
    }

    #[test]
    fn a_reddening_of_zero_is_the_sun_that_knows_nothing_about_the_band() {
        for height in [-40.0, 0.0, 9.0, 96.0] {
            assert_eq!(limb_transmission(height, 0.0), Vec3::ONE);
            assert_relative_eq!(limb_disk_amplitude(limb_transmission(height, 0.0).y), 1.0);
        }
    }

    /// A tenth of the Sun is still a blinding source, so the disk clips white
    /// down to about nine kilometers and only then starts to go.
    #[test]
    fn the_disk_clips_white_until_the_path_carries_almost_nothing() {
        assert_relative_eq!(limb_disk_amplitude(limb_transmission(12.0, 1.0).y), 1.0);
        assert_relative_eq!(limb_disk_amplitude(limb_transmission(9.0, 1.0).y), 1.0);
        assert!(limb_disk_amplitude(limb_transmission(7.0, 1.0).y) < 0.9);
        assert!(limb_disk_amplitude(limb_transmission(2.0, 1.0).y) < 0.1);
    }

    // --- the flux the visible disk delivers ---

    /// A disk clear of the band delivers all of its light and is white doing
    /// it, and both have to be exact: the goldens with a Sun well above the
    /// limb come back byte for byte only if nothing here rounds.
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_disk_clear_of_the_band_carries_all_of_its_light() {
        let (tint, transmission) = integrate_disk(400.0, 4.0, 100.0, 100.0, 8.0, 95.565, 1.0, 1.0);
        assert_eq!(transmission, 1.0);
        assert_eq!(tint, Vec3::ONE);
    }

    /// Half the disk behind the limb with the visible half in the lowest tenth
    /// of the band: the area says a half and the light says far less, which is
    /// the whole reason the glare is a function of flux and not of area.
    #[test]
    fn a_disk_half_behind_the_limb_in_the_red_carries_almost_nothing() {
        let (tint, transmission) = integrate_disk(100.0, 5.0, 100.0, 100.0, 50.0, 95.565, 1.0, 1.0);
        assert!(
            transmission < 0.05,
            "the visible half is in the red, so it cannot carry {transmission} of the light"
        );
        assert!(
            tint.z < tint.y && tint.y < tint.x,
            "and what it carries is red, got {tint}"
        );
    }

    // --- the exposure gain ---

    #[test]
    fn the_glare_peaks_where_the_disk_clears_the_zone_and_settles_above_it() {
        assert_relative_eq!(exposure_gain(-2.0, 3.0, 4.0), 3.0);
        assert_relative_eq!(exposure_gain(1.0, 3.0, 4.0), 3.0);
        assert!(exposure_gain(2.0, 3.0, 4.0) < 3.0);
        assert!(exposure_gain(2.0, 3.0, 4.0) > exposure_gain(4.0, 3.0, 4.0));
        assert_relative_eq!(exposure_gain(5.0, 3.0, 4.0), 1.0);
        assert_relative_eq!(exposure_gain(80.0, 3.0, 4.0), 1.0);
    }

    #[test]
    fn a_boost_of_one_is_the_physical_answer_at_every_height() {
        for height in [-3.0, 0.0, 1.0, 2.5, 40.0] {
            assert_relative_eq!(exposure_gain(height, 1.0, 4.0), 1.0);
        }
    }

    // --- refraction ---

    #[test]
    #[allow(clippy::float_cmp)]
    fn no_refraction_leaves_the_disk_exactly_where_it_is() {
        for height in [-9.0, -0.46, 0.0, 0.3, 7.0] {
            let (apparent, squash) = refract(height, 0.0);
            assert_eq!(apparent, height);
            assert_eq!(squash, 1.0);
        }
    }

    /// The derived figures: a seventh of the vertical size at the horizon, and
    /// most of it back by a third of the way up the band.
    #[test]
    fn the_flattening_matches_the_orbital_measurements() {
        let (apparent, squash) = refract(-0.46, 1.0);
        assert_relative_eq!(apparent, 0.0, epsilon = 1e-5);
        assert_relative_eq!(squash, 0.137, epsilon = 0.002);
        let one_third = 1.0 / 3.0 - 0.46 * (-REFRACTION_EXPONENT / 3.0).exp();
        let (apparent, squash) = refract(one_third, 1.0);
        assert_relative_eq!(apparent, 1.0 / 3.0, epsilon = 1e-4);
        assert_relative_eq!(squash, 0.94, epsilon = 0.002);
    }

    #[test]
    fn the_newton_solve_inverts_the_map_it_is_solving() {
        for step in 0..60 {
            #[allow(clippy::cast_precision_loss)]
            let apparent = step as f32 * 0.05;
            let geometric = apparent - 0.46 * (-REFRACTION_EXPONENT * apparent).exp();
            let (solved, _) = refract(geometric, 1.0);
            assert_relative_eq!(solved, apparent, epsilon = 1e-4);
        }
    }

    #[test]
    fn the_lift_is_monotonic_and_never_more_than_the_whole_atmosphere() {
        let mut previous = f32::NEG_INFINITY;
        for step in -40..60 {
            #[allow(clippy::cast_precision_loss)]
            let geometric = step as f32 * 0.05;
            let (apparent, squash) = refract(geometric, 1.0);
            assert!(
                apparent > previous,
                "the map has to be monotonic at {geometric}"
            );
            assert!(
                apparent - geometric <= REFRACTION_LIFT_ZONES + 1e-5,
                "a lift of {} at {geometric} is more than the atmosphere is worth",
                apparent - geometric
            );
            assert!((0.1..=1.0).contains(&squash));
            previous = apparent;
        }
    }

    /// Flattening the disk about its own center and moving the limb it is cut
    /// by are the same thing in area fractions, which is what lets `visibility`
    /// stay circle against circle. Checked against the ellipse itself, sampled.
    ///
    /// The identity is exact where the limb is a straight line, and the limb is
    /// a circle: scaling one axis takes a circle to an ellipse, so the pulled
    /// back limb agrees with the moved circle where the disk sits and curves
    /// away from it to either side. What that costs is a function of how large
    /// the disk is against the globe, and it is worst where the disk is deepest
    /// in the band, which is where its light is nearly gone, so a glare already
    /// down to a hundredth is what carries the error. `docs/rendering.md` has
    /// the error measured at each disk size; it leaves little room under this
    /// case's tolerance, so the sampling density below is not free to fall.
    #[test]
    fn a_squashed_disk_against_the_limb_is_a_round_one_against_a_moved_limb() {
        let globe_radius = 120.0_f32;
        let disk_radius = 5.0_f32;
        for squash in [0.15_f32, 0.4, 0.75, 1.0] {
            for offset in [-4.0_f32, -2.0, 0.0, 3.0, 4.5] {
                let center = globe_radius + offset;
                let spread = 1.0 / squash - 1.0;
                let moved = globe_radius + spread * (globe_radius - center);
                let reported = visibility(
                    ScreenCircle {
                        center: Vec2::new(center, 0.0),
                        radius: disk_radius,
                    },
                    ScreenCircle {
                        center: Vec2::ZERO,
                        radius: moved,
                    },
                    None,
                )
                .visible_fraction;

                // The ellipse the shader actually draws, sampled on a grid.
                let steps: i16 = 400;
                let mut inside = 0_u32;
                let mut clear = 0_u32;
                for row in 0..steps {
                    for column in 0..steps {
                        #[allow(clippy::cast_precision_loss)]
                        let u = (f32::from(row) + 0.5) / f32::from(steps) * 2.0 - 1.0;
                        #[allow(clippy::cast_precision_loss)]
                        let v = (f32::from(column) + 0.5) / f32::from(steps) * 2.0 - 1.0;
                        if u * u + v * v > 1.0 {
                            continue;
                        }
                        inside += 1;
                        let drawn = center + squash * u * disk_radius;
                        let across = v * disk_radius;
                        if (drawn * drawn + across * across).sqrt() > globe_radius {
                            clear += 1;
                        }
                    }
                }
                let sampled = f64::from(clear) / f64::from(inside);
                assert!(
                    (f64::from(reported) - sampled).abs() < 0.03,
                    "squash {squash} at offset {offset}: the identity says {reported} and the \
                     ellipse itself {sampled}"
                );
            }
        }
    }

    // --- the forward lobe's width ---

    #[test]
    fn the_asymmetry_puts_the_lobe_at_half_where_the_slider_says() {
        for width in [10.0_f32, 30.0, 60.0, 90.0] {
            let g = henyey_greenstein_asymmetry(width);
            let cosine = width.to_radians().cos();
            let lobe = ((1.0 - g) * (1.0 - g) / (1.0 + g * g - 2.0 * g * cosine)).powf(1.5);
            assert_relative_eq!(lobe, 0.5, epsilon = 1e-3);
        }
        assert!(
            henyey_greenstein_asymmetry(10.0) > henyey_greenstein_asymmetry(90.0),
            "a narrower band is a more forward-scattering one"
        );
    }

    #[test]
    fn a_sun_clear_of_the_silhouette_is_fully_visible() {
        let v = visibility(sun_at(300.0, 5.0), globe(), None);
        assert_relative_eq!(v.visible_fraction, 1.0);
    }

    #[test]
    fn a_sun_inside_the_painted_globe_is_fully_hidden() {
        let v = visibility(sun_at(100.0, 5.0), globe(), None);
        assert_relative_eq!(v.visible_fraction, 0.0);
    }

    #[test]
    fn a_sun_centered_on_the_limb_is_half_visible() {
        let v = visibility(sun_at(150.0, 5.0), globe(), None);
        // A shade over half, because the limb curves away from the disk's
        // center rather than cutting it along a diameter.
        assert_relative_eq!(v.visible_fraction, 0.51, epsilon = 0.02);
    }

    /// A moon circle centered on a sun disk of the same size, well clear of
    /// the globe: an eclipse, and the glare has to go with the disk.
    #[test]
    fn a_moon_over_the_sun_hides_it_completely() {
        let sun = sun_at(300.0, 5.0);
        let moon = ScreenCircle {
            center: sun.center,
            radius: 5.0,
        };
        let v = visibility(sun, globe(), Some(moon));
        assert_relative_eq!(v.visible_fraction, 0.0);
    }

    /// A moon somewhere else in the sky changes nothing, which is the case
    /// every frame that is not an eclipse.
    #[test]
    fn a_moon_beside_the_sun_hides_nothing() {
        let sun = sun_at(300.0, 5.0);
        let moon = ScreenCircle {
            center: Vec2::new(340.0, 100.0),
            radius: 5.0,
        };
        let v = visibility(sun, globe(), Some(moon));
        assert_relative_eq!(v.visible_fraction, 1.0);
        assert_relative_eq!(
            v.visible_fraction,
            visibility(sun, globe(), None).visible_fraction
        );
    }

    /// Half over it: the same circle-circle lens area the globe is measured
    /// with, which for two equal circles whose centers are one radius apart
    /// covers 39.1 percent of either.
    #[test]
    fn a_moon_half_over_the_sun_hides_the_lens_area() {
        let sun = sun_at(300.0, 5.0);
        let moon = ScreenCircle {
            center: Vec2::new(305.0, 100.0),
            radius: 5.0,
        };
        let lens = 2.0 * (0.5_f32.acos() - 0.5 * 0.75_f32.sqrt()) / PI;
        let v = visibility(sun, globe(), Some(moon));
        assert_relative_eq!(v.visible_fraction, 1.0 - lens, epsilon = 1e-5);
    }

    /// With the globe and the Moon both over the disk, the larger hidden area
    /// wins rather than the two adding up. Their union is somewhere between the
    /// two, so this errs toward more glare and never toward less.
    #[test]
    fn the_larger_occluder_decides_when_both_cover_the_sun() {
        // Centered on the painted limb, so the globe hides about half.
        let sun = sun_at(150.0, 5.0);
        let globe_alone = visibility(sun, globe(), None).visible_fraction;
        // A moon just clipping the other side of the same disk, hiding less.
        let small_bite = ScreenCircle {
            center: Vec2::new(158.0, 100.0),
            radius: 4.0,
        };
        let with_small = visibility(sun, globe(), Some(small_bite)).visible_fraction;
        assert_relative_eq!(with_small, globe_alone);
        // And one covering the disk outright, hiding more than the globe does.
        let full = ScreenCircle {
            center: sun.center,
            radius: 6.0,
        };
        let with_full = visibility(sun, globe(), Some(full)).visible_fraction;
        assert_relative_eq!(with_full, 0.0);
    }

    #[test]
    fn the_visible_fraction_rises_monotonically_across_the_limb() {
        let mut previous = -1.0_f32;
        let mut seen_partial = false;
        for step in 0..=200 {
            #[allow(clippy::cast_precision_loss)]
            let x = 100.0 + step as f32 * 0.4;
            let fraction = visibility(sun_at(x, 6.0), globe(), None).visible_fraction;
            assert!(
                fraction >= previous - 1e-6,
                "the visible fraction fell from {previous} to {fraction} at x = {x}"
            );
            if fraction > 0.01 && fraction < 0.99 {
                seen_partial = true;
            }
            previous = fraction;
        }
        assert!(seen_partial, "the sweep never crossed the limb");
        assert_relative_eq!(previous, 1.0);
    }

    /// The aspect ratio at which the two lenses agree about angular scale.
    ///
    /// The sky lens maps a small angle to `theta * W / (4 * edge)` pixels and
    /// the Earth lens maps one to `alpha * H / (2 * tan(fov / 2))`. Setting
    /// those equal fixes the aspect ratio once both fields of view are chosen,
    /// and 60 degrees is the sky slider's own minimum rather than a number from
    /// outside the product's range.
    const AGREEING_SKY_FOV: f32 = 60.0;
    const CAMERA_FOV: f32 = crate::scene::camera::DEFAULT_CAMERA_FOV;

    fn agreeing_viewport() -> Vec2 {
        let aspect =
            2.0 * sky_lens_edge_radius(AGREEING_SKY_FOV) / (CAMERA_FOV * 0.5).to_radians().tan();
        Vec2::new(500.0 * aspect, 500.0)
    }

    #[test]
    fn the_screen_space_fraction_matches_the_ephemeris_where_the_lenses_agree() {
        let viewport = agreeing_viewport();
        let camera = OrbitalCamera::new(0.0, 0.0, 80.0);
        let eye = camera.eye_position();
        let limb = (1.0_f32 / 80.0).asin();
        let mut worst = 0.0_f32;
        for step in 0..=40 {
            #[allow(clippy::cast_precision_loss)]
            let separation = limb * 2.0 * step as f32 / 40.0;
            // The eye sits on +Z looking at the origin, so a direction tilted
            // out of the view axis by `separation` is a rotation about X.
            let sun = Vec3::new(0.0, separation.sin(), -separation.cos());
            let placement = place_sun(&SunPlacementInputs {
                sun_world_direction: sun,
                view: camera.view_matrix(),
                mvp: camera.mvp_matrix(viewport.x / viewport.y),
                eye_distance: camera.distance,
                sky_fov_deg: AGREEING_SKY_FOV,
                camera_fov_deg: CAMERA_FOV,
                atmosphere_radius: crate::params::RAYLEIGH_RADIUS,
                screen_offset: Vec2::ZERO,
                viewport,
                moon_disc: None,
                horizon: SunHorizonParams::GEOMETRIC,
            });
            let reference = angular_visible_fraction(sun, eye, 1.0);
            worst = worst.max((placement.visibility.visible_fraction - reference).abs());
        }
        assert!(
            worst < 0.01,
            "the screen-space fraction and the ephemeris one differ by {worst} at worst"
        );
    }

    /// `place_sun` hands the Moon's circle on to [`visibility`].
    #[test]
    fn place_sun_measures_the_sun_against_the_moon_it_was_given() {
        let viewport = agreeing_viewport();
        let camera = OrbitalCamera::new(0.0, 0.0, 80.0);
        // Thirty degrees off the view axis, where the painted globe is a few
        // degrees wide: nothing but the Moon can hide the disk here.
        let off_axis = 30.0_f32.to_radians();
        let sun = Vec3::new(0.0, off_axis.sin(), -off_axis.cos());
        let inputs = |moon_disc| SunPlacementInputs {
            sun_world_direction: sun,
            view: camera.view_matrix(),
            mvp: camera.mvp_matrix(viewport.x / viewport.y),
            eye_distance: camera.distance,
            sky_fov_deg: AGREEING_SKY_FOV,
            camera_fov_deg: CAMERA_FOV,
            atmosphere_radius: crate::params::RAYLEIGH_RADIUS,
            screen_offset: Vec2::ZERO,
            viewport,
            moon_disc,
            horizon: SunHorizonParams::GEOMETRIC,
        };
        let clear = place_sun(&inputs(None));
        assert_relative_eq!(clear.visibility.visible_fraction, 1.0);

        let view_direction = (camera.view_matrix() * sun.extend(0.0))
            .truncate()
            .normalize();
        let disk = sky_lens_disc(
            view_direction,
            SUN_ANGULAR_RADIUS_DEGREES.to_radians(),
            AGREEING_SKY_FOV,
            Vec2::ZERO,
            viewport,
        )
        .expect("the sun has an image at this framing");
        let eclipsed = place_sun(&inputs(Some(ScreenCircle {
            center: disk.center,
            radius: clear.disk_radius_pixels * 2.0,
        })));
        assert_relative_eq!(eclipsed.visibility.visible_fraction, 0.0);
    }

    #[test]
    fn a_cone_around_the_view_axis_images_as_a_disc_on_the_center() {
        let viewport = Vec2::new(800.0, 400.0);
        let disc = sky_lens_disc(
            Vec3::NEG_Z,
            10.0_f32.to_radians(),
            140.0,
            Vec2::ZERO,
            viewport,
        )
        .expect("a cone about the view axis has a finite image");
        assert_relative_eq!(disc.center.x, 400.0, epsilon = 1e-3);
        assert_relative_eq!(disc.center.y, 200.0, epsilon = 1e-3);
        assert!(disc.radius > 0.0);
    }

    #[test]
    fn a_cone_reaching_the_antipode_has_no_finite_image() {
        assert!(
            sky_lens_disc(
                Vec3::Z,
                10.0_f32.to_radians(),
                140.0,
                Vec2::ZERO,
                Vec2::new(800.0, 400.0),
            )
            .is_none()
        );
    }

    #[test]
    fn the_imaged_disc_is_the_same_size_along_both_screen_axes() {
        let viewport = Vec2::new(1600.0, 400.0);
        let half_angle = 0.267_f32.to_radians();
        let tilt = 25.0_f32.to_radians();
        let horizontal = sky_lens_disc(
            Vec3::new(tilt.sin(), 0.0, -tilt.cos()),
            half_angle,
            140.0,
            Vec2::ZERO,
            viewport,
        )
        .expect("finite image");
        let vertical = sky_lens_disc(
            Vec3::new(0.0, tilt.sin(), -tilt.cos()),
            half_angle,
            140.0,
            Vec2::ZERO,
            viewport,
        )
        .expect("finite image");
        assert_relative_eq!(horizontal.radius, vertical.radius, epsilon = 1e-5);
    }

    #[test]
    fn a_pan_moves_the_imaged_disc_by_exactly_the_pan() {
        let viewport = Vec2::new(800.0, 400.0);
        let tilt = 25.0_f32.to_radians();
        let direction = Vec3::new(tilt.sin(), 0.0, -tilt.cos());
        let pan = Vec2::new(0.35, -0.2);
        let disc = |offset| {
            sky_lens_disc(direction, 0.267_f32.to_radians(), 140.0, offset, viewport)
                .expect("finite image")
        };
        let centered = disc(Vec2::ZERO);
        let panned = disc(pan);
        // NDC y runs up and pixel y runs down, which is the whole reason the
        // sign of this is worth a test.
        assert_relative_eq!(
            panned.center.x - centered.center.x,
            pan.x * 0.5 * viewport.x,
            epsilon = 1e-3
        );
        assert_relative_eq!(
            panned.center.y - centered.center.y,
            -pan.y * 0.5 * viewport.y,
            epsilon = 1e-3
        );
        assert_relative_eq!(panned.radius, centered.radius, epsilon = 1e-5);
    }

    #[test]
    fn panning_the_frame_moves_the_sun_and_the_globe_together() {
        let viewport = Vec2::new(512.0, 256.0);
        let aspect = viewport.x / viewport.y;
        // The pan the renderer applies to the globe and the one it hands the
        // sky lens are the same pan with opposite signs, so a sign error in
        // either one separates the Sun from the silhouette it is measured
        // against.
        let (offset_x, offset_y) = (0.3_f32, -0.15_f32);
        let fraction = |tilt: f32, panned: bool| {
            let mut camera = OrbitalCamera::new(0.0, 0.0, 8.0);
            if panned {
                camera.offset_x = offset_x;
                camera.offset_y = offset_y;
            }
            let screen_offset = if panned {
                Vec2::new(-offset_x, -offset_y)
            } else {
                Vec2::ZERO
            };
            place_sun(&SunPlacementInputs {
                sun_world_direction: Vec3::new(tilt.sin(), 0.0, -tilt.cos()),
                view: camera.view_matrix(),
                mvp: camera.mvp_matrix(aspect),
                eye_distance: camera.distance,
                sky_fov_deg: 60.0,
                camera_fov_deg: CAMERA_FOV,
                atmosphere_radius: crate::params::RAYLEIGH_RADIUS,
                screen_offset,
                viewport,
                moon_disc: None,
                horizon: SunHorizonParams::GEOMETRIC,
            })
            .visibility
            .visible_fraction
        };
        let mut seen_partial = false;
        for step in 0..=40 {
            #[allow(clippy::cast_precision_loss)]
            let tilt = (step as f32 / 40.0) * 40.0_f32.to_radians();
            let centered = fraction(tilt, false);
            let panned = fraction(tilt, true);
            assert_relative_eq!(centered, panned, epsilon = 1e-4);
            if centered > 0.01 && centered < 0.99 {
                seen_partial = true;
            }
        }
        assert!(seen_partial, "the sweep never crossed the painted limb");
    }

    #[test]
    fn a_sub_pixel_disk_is_floored_rather_than_lost() {
        let viewport = Vec2::new(512.0, 256.0);
        let camera = OrbitalCamera::new(0.0, 0.0, 8.0);
        let tilt = 20.0_f32.to_radians();
        let placement = place_sun(&SunPlacementInputs {
            // Well clear of the globe but near enough to the view axis that
            // the lens is not stretching the disk: at 140 degrees on a 512
            // pixel frame the true disk is under a pixel across.
            sun_world_direction: Vec3::new(tilt.sin(), 0.0, -tilt.cos()),
            view: camera.view_matrix(),
            mvp: camera.mvp_matrix(2.0),
            eye_distance: camera.distance,
            sky_fov_deg: 140.0,
            camera_fov_deg: CAMERA_FOV,
            atmosphere_radius: crate::params::RAYLEIGH_RADIUS,
            screen_offset: Vec2::ZERO,
            viewport,
            moon_disc: None,
            horizon: SunHorizonParams::GEOMETRIC,
        });
        assert_relative_eq!(placement.disk_radius_pixels, MIN_BODY_DISK_RADIUS_PIXELS);
    }

    #[test]
    fn a_sun_behind_the_globe_reports_nothing_visible() {
        let viewport = Vec2::new(1920.0, 1080.0);
        let camera = OrbitalCamera::new(0.0, 0.0, 8.0);
        let placement = place_sun(&SunPlacementInputs {
            // Straight through the globe, away from the eye.
            sun_world_direction: Vec3::NEG_Z,
            view: camera.view_matrix(),
            mvp: camera.mvp_matrix(viewport.x / viewport.y),
            eye_distance: camera.distance,
            sky_fov_deg: 140.0,
            camera_fov_deg: CAMERA_FOV,
            atmosphere_radius: crate::params::RAYLEIGH_RADIUS,
            screen_offset: Vec2::ZERO,
            viewport,
            moon_disc: None,
            horizon: SunHorizonParams::GEOMETRIC,
        });
        assert_relative_eq!(placement.visibility.visible_fraction, 0.0);
    }
}
