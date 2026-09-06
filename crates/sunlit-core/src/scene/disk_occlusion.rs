//! What one circle hides of another, in the pixels both are drawn in.
//!
//! Circle against circle and nothing else: no sun, no band, no lens. The two
//! occluders `sun_occlusion` measures the Sun against are the globe's painted
//! silhouette and the Moon's disc, and both reach here as a
//! [`super::sky_lens::ScreenCircle`].

use std::f32::consts::PI;

use super::sky_lens::ScreenCircle;

/// How much of the Sun's disk the viewer can see.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunVisibility {
    /// Fraction of the disk's area outside the globe's painted silhouette.
    pub visible_fraction: f32,
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
pub(super) fn visibility(
    sun: ScreenCircle,
    globe: ScreenCircle,
    moon: Option<ScreenCircle>,
) -> SunVisibility {
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

/// Area shared by two circles whose centers are `distance` apart.
pub(super) fn overlap_area(r0: f32, r1: f32, distance: f32) -> f32 {
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

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;
    use glam::Vec2;

    use super::*;

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
}
