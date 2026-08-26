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
pub const SUN_ANGULAR_RADIUS_DEGREES: f32 = 0.267;

/// Smallest radius either half-degree body is drawn at, in pixels at 1080p.
///
/// At the default sky field of view the true disk is about 13 pixels across on
/// a 4K render and under one pixel on a small preview, so without a floor the
/// Sun collapses to a speck exactly where a user is tuning it. Phase A's stars
/// carry the same clause for the same reason. The Moon subtends the same half
/// degree and takes the same floor, or the two bodies that are the same size in
/// the sky would be different sizes on screen wherever it is active.
pub const MIN_BODY_DISK_RADIUS_PIXELS: f32 = 1.6;

/// Output-density ramp shared with the star sprites: 1.0 at 1080p and below,
/// 2.0 at 4K and above.
pub fn pixel_scale(viewport_height: f32) -> f32 {
    (viewport_height / 1080.0).clamp(1.0, 2.0)
}

/// A circle on the framebuffer, in pixels, with y running down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenCircle {
    pub center: Vec2,
    pub radius: f32,
}

/// How much of the Sun's disk the viewer can see, and how much of what is
/// visible looks through the atmosphere shell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunVisibility {
    /// Fraction of the disk's area outside the globe's painted silhouette.
    pub visible_fraction: f32,
    /// Fraction of the disk's area inside the annulus between the painted
    /// silhouette and the atmosphere shell's, which is the grazing band where
    /// the light path runs through the lower atmosphere.
    pub transit_fraction: f32,
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
    /// Radius of the atmosphere shell that bounds the transit band.
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
}

/// Everything the uniform encoder needs to place and fade the Sun.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunPlacement {
    /// Sun direction in view space, unit length. The shader rebuilds the
    /// screen position from this, so there is one formula and not two.
    pub view_direction: Vec3,
    /// The disk's radius in pixels, never below
    /// [`MIN_BODY_DISK_RADIUS_PIXELS`] times the density ramp.
    pub disk_radius_pixels: f32,
    pub visibility: SunVisibility,
}

/// The horizontal half-extent of the sky lens in projected-plane units.
///
/// A direction `theta` from the view axis lands at `tan(theta / 2)`, and the
/// frame's horizontal edge is `sky_fov / 2` from the axis, so this is the
/// divisor that puts that edge at NDC 1. Mirrors `sky_lens_edge_radius` in
/// `sphere.wgsl`, clamp included.
pub fn sky_lens_edge_radius(sky_fov_deg: f32) -> f32 {
    (sky_fov_deg.clamp(60.0, 180.0) * PI / 720.0).tan()
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

/// Place the Sun and measure what the globe hides of it.
pub fn place_sun(inputs: &SunPlacementInputs) -> SunPlacement {
    let view_direction = (inputs.view * inputs.sun_world_direction.extend(0.0))
        .truncate()
        .normalize_or_zero();
    let floor = MIN_BODY_DISK_RADIUS_PIXELS * pixel_scale(inputs.viewport.y);
    let disc = sky_lens_disc(
        view_direction,
        SUN_ANGULAR_RADIUS_DEGREES.to_radians(),
        inputs.sky_fov_deg,
        inputs.screen_offset,
        inputs.viewport,
    );
    let Some(mut disc) = disc else {
        // The Sun sits at the antipode of the view axis, off screen whatever
        // the globe does: nothing hides it, and nothing draws it either.
        return SunPlacement {
            view_direction,
            disk_radius_pixels: floor,
            visibility: SunVisibility {
                visible_fraction: 1.0,
                transit_fraction: 0.0,
            },
        };
    };
    disc.radius = disc.radius.max(floor);
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
    SunPlacement {
        view_direction,
        disk_radius_pixels: disc.radius,
        visibility: visibility(disc, globe, atmosphere, inputs.moon_disc),
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
/// are in the same lens and the answer agrees with an ephemeris. The transit
/// band belongs to the globe alone, so the Moon does not enter it.
pub fn visibility(
    sun: ScreenCircle,
    globe: ScreenCircle,
    atmosphere: ScreenCircle,
    moon: Option<ScreenCircle>,
) -> SunVisibility {
    let behind_globe = overlap_area(sun.radius, globe.radius, sun.center.distance(globe.center));
    let behind_moon = moon.map_or(0.0, |moon| {
        overlap_area(sun.radius, moon.radius, sun.center.distance(moon.center))
    });
    let hidden = behind_globe.max(behind_moon);
    let within_atmosphere = overlap_area(
        sun.radius,
        atmosphere.radius,
        sun.center.distance(atmosphere.center),
    );
    let area = PI * sun.radius * sun.radius;
    if area <= 0.0 {
        let outside = |circle: ScreenCircle| sun.center.distance(circle.center) > circle.radius;
        let clear = outside(globe) && moon.is_none_or(outside);
        let banded = clear && sun.center.distance(atmosphere.center) <= atmosphere.radius;
        return SunVisibility {
            visible_fraction: f32::from(u8::from(clear)),
            transit_fraction: f32::from(u8::from(banded)),
        };
    }
    SunVisibility {
        visible_fraction: (1.0 - hidden / area).clamp(0.0, 1.0),
        transit_fraction: ((within_atmosphere - behind_globe) / area).clamp(0.0, 1.0),
    }
}

/// The ephemeris answer: what fraction of the Sun's disk clears the true
/// angular limb of a sphere of radius `body_radius` at the origin.
///
/// Read by nothing in the renderer. It exists so the screen-space function has
/// something independent to be checked against in the one configuration where
/// the two lenses agree about scale.
pub fn angular_visible_fraction(sun_direction: Vec3, eye: Vec3, body_radius: f32) -> f32 {
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

    /// Concentric silhouettes to measure a sun disk against: the globe at 50
    /// pixels and the atmosphere shell at 60, both around (100, 100).
    fn globe() -> ScreenCircle {
        ScreenCircle {
            center: Vec2::new(100.0, 100.0),
            radius: 50.0,
        }
    }

    fn atmosphere() -> ScreenCircle {
        ScreenCircle {
            center: Vec2::new(100.0, 100.0),
            radius: 60.0,
        }
    }

    fn sun_at(x: f32, radius: f32) -> ScreenCircle {
        ScreenCircle {
            center: Vec2::new(x, 100.0),
            radius,
        }
    }

    #[test]
    fn a_sun_clear_of_both_silhouettes_is_fully_visible() {
        let v = visibility(sun_at(300.0, 5.0), globe(), atmosphere(), None);
        assert_relative_eq!(v.visible_fraction, 1.0);
        assert_relative_eq!(v.transit_fraction, 0.0);
    }

    #[test]
    fn a_sun_inside_the_painted_globe_is_fully_hidden() {
        let v = visibility(sun_at(100.0, 5.0), globe(), atmosphere(), None);
        assert_relative_eq!(v.visible_fraction, 0.0);
        assert_relative_eq!(v.transit_fraction, 0.0);
    }

    #[test]
    fn a_sun_centered_on_the_limb_is_half_visible() {
        let v = visibility(sun_at(150.0, 5.0), globe(), atmosphere(), None);
        // A shade over half, because the limb curves away from the disk's
        // center rather than cutting it along a diameter.
        assert_relative_eq!(v.visible_fraction, 0.51, epsilon = 0.02);
        // What cleared the limb is entirely inside the atmosphere circle,
        // which is what the transit band means.
        assert_relative_eq!(v.transit_fraction, v.visible_fraction, epsilon = 1e-5);
    }

    #[test]
    fn a_sun_in_the_annulus_is_visible_and_fully_in_transit() {
        let v = visibility(sun_at(155.0, 2.0), globe(), atmosphere(), None);
        assert_relative_eq!(v.visible_fraction, 1.0);
        assert_relative_eq!(v.transit_fraction, 1.0);
    }

    #[test]
    fn a_sun_past_the_atmosphere_is_out_of_transit() {
        let v = visibility(sun_at(170.0, 2.0), globe(), atmosphere(), None);
        assert_relative_eq!(v.visible_fraction, 1.0);
        assert_relative_eq!(v.transit_fraction, 0.0);
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
        let v = visibility(sun, globe(), atmosphere(), Some(moon));
        assert_relative_eq!(v.visible_fraction, 0.0);
        assert_relative_eq!(v.transit_fraction, 0.0);
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
        let v = visibility(sun, globe(), atmosphere(), Some(moon));
        assert_relative_eq!(v.visible_fraction, 1.0);
        assert_relative_eq!(
            v.visible_fraction,
            visibility(sun, globe(), atmosphere(), None).visible_fraction
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
        let v = visibility(sun, globe(), atmosphere(), Some(moon));
        assert_relative_eq!(v.visible_fraction, 1.0 - lens, epsilon = 1e-5);
    }

    /// With the globe and the Moon both over the disk, the larger hidden area
    /// wins rather than the two adding up. Their union is somewhere between the
    /// two, so this errs toward more glare and never toward less.
    #[test]
    fn the_larger_occluder_decides_when_both_cover_the_sun() {
        // Centered on the painted limb, so the globe hides about half.
        let sun = sun_at(150.0, 5.0);
        let globe_alone = visibility(sun, globe(), atmosphere(), None).visible_fraction;
        // A moon just clipping the other side of the same disk, hiding less.
        let small_bite = ScreenCircle {
            center: Vec2::new(158.0, 100.0),
            radius: 4.0,
        };
        let with_small = visibility(sun, globe(), atmosphere(), Some(small_bite)).visible_fraction;
        assert_relative_eq!(with_small, globe_alone);
        // And one covering the disk outright, hiding more than the globe does.
        let full = ScreenCircle {
            center: sun.center,
            radius: 6.0,
        };
        let with_full = visibility(sun, globe(), atmosphere(), Some(full)).visible_fraction;
        assert_relative_eq!(with_full, 0.0);
    }

    #[test]
    fn the_visible_fraction_rises_monotonically_across_the_limb() {
        let mut previous = -1.0_f32;
        let mut seen_partial = false;
        for step in 0..=200 {
            #[allow(clippy::cast_precision_loss)]
            let x = 100.0 + step as f32 * 0.4;
            let fraction = visibility(sun_at(x, 6.0), globe(), atmosphere(), None).visible_fraction;
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
    const CAMERA_FOV: f32 = 20.0;

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
    ///
    /// Every other Moon case here calls `visibility` itself, so this is what
    /// says the argument survives the way in: without it the field could be
    /// dropped in the one function the renderer actually calls.
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
        });
        assert_relative_eq!(placement.visibility.visible_fraction, 0.0);
    }
}
