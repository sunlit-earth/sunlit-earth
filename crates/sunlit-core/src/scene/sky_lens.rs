//! The stereographic sky lens, and where a body lands under it in pixels.
//!
//! Everything celestial is drawn through this lens while the globe is drawn
//! through the camera's perspective one, so the two images of the same
//! direction sit at different scales on the same frame. The lens is conformal,
//! which is what makes every shape here a circle: a cone about a direction
//! images as a disc, the projected plane reaches pixels by one uniform scale,
//! and the globe's painted silhouette is a circle there too.
//!
//! [`super::sun_occlusion`] measures the Sun against that silhouette and
//! [`super::moon`] places the Moon with the same functions, which is why the
//! lens lives here rather than inside either of them.

use std::f32::consts::PI;

use glam::{Mat4, Vec2, Vec3, Vec4};

use crate::display::layout::{SKY_FOV_MAX, SKY_FOV_MIN};

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

/// A circle on the framebuffer, in pixels, with y running down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenCircle {
    pub center: Vec2,
    pub radius: f32,
}

/// The horizontal half-extent of the sky lens in projected-plane units.
///
/// A direction `theta` from the view axis lands at `tan(theta / 2)`, and the
/// frame's horizontal edge is `sky_fov / 2` from the axis, so this is the
/// divisor that puts that edge at NDC 1. Mirrors `sky_lens_edge_radius` in
/// `sphere.wgsl`, clamp included. The upper end is the widest sky a spanned
/// canvas may derive, not the slider's 180, which is why the bounds are the
/// layout's own.
pub fn sky_lens_edge_radius(sky_fov_deg: f32) -> f32 {
    (sky_fov_deg.clamp(SKY_FOV_MIN, SKY_FOV_MAX) * PI / 720.0).tan()
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
}
