//! Pure math functions for mouse interaction with the globe.
//!
//! They are the arithmetic behind the callbacks `ui_callbacks` registers, kept
//! apart from the Slint window so they can be tested without one.

use sunlit_core::scene::camera::zoom_to_distance;
use sunlit_core::scene::sky_lens;

/// Wrap a longitude value into the `[-180, 180)` range.
#[must_use]
fn wrap_longitude(lon: f32) -> f32 {
    ((lon + 180.0) % 360.0 + 360.0) % 360.0 - 180.0
}

/// Wrap a tilt value into the `[-180, 180)` range.
#[must_use]
fn wrap_angle_180(angle: f32) -> f32 {
    ((angle + 180.0) % 360.0 + 360.0) % 360.0 - 180.0
}

/// Apply tilt-corrected globe rotation from a mouse drag, at the gain the
/// zoom implies.
///
/// A test convenience: the callbacks pass a gain of their own through
/// [`apply_globe_drag_at`], which is what the drag actually uses.
///
/// :param lon: current longitude in degrees
/// :param lat: current latitude in degrees
/// :param `tilt_deg`: current tilt in degrees
/// :param zoom: normalized zoom slider value (0.0 to 1.0)
/// :param dx: horizontal mouse delta in pixels
/// :param dy: vertical mouse delta in pixels
/// :returns: new `(longitude, latitude)` tuple
#[cfg(test)]
#[must_use]
fn apply_globe_drag(lon: f32, lat: f32, tilt_deg: f32, zoom: f32, dx: f32, dy: f32) -> (f32, f32) {
    apply_globe_drag_at(lon, lat, tilt_deg, dx, dy, coarse_drag_gain(zoom))
}

/// The same, at a gain the caller has chosen.
///
/// :param lon: current longitude in degrees
/// :param lat: current latitude in degrees
/// :param `tilt_deg`: current tilt in degrees
/// :param dx: horizontal mouse delta in pixels
/// :param dy: vertical mouse delta in pixels
/// :param `degrees_per_px`: how far the globe turns per pixel of cursor
/// :returns: new `(longitude, latitude)` tuple
#[must_use]
pub fn apply_globe_drag_at(
    lon: f32,
    lat: f32,
    tilt_deg: f32,
    dx: f32,
    dy: f32,
    degrees_per_px: f32,
) -> (f32, f32) {
    let tilt_rad = tilt_deg.to_radians();
    let cos_t = tilt_rad.cos();
    let sin_t = tilt_rad.sin();

    // Rotate the (dx, dy) vector by -tilt to undo the screen-space rotation
    let delta = [dx * cos_t + dy * sin_t, -dx * sin_t + dy * cos_t];

    let new_lon = lon - delta[0] * degrees_per_px;
    let new_lat = lat + delta[1] * degrees_per_px;

    (wrap_longitude(new_lon), new_lat.clamp(-89.0, 89.0))
}

/// Cursor speed at and below which the drag turns the globe at the fine gain,
/// in logical pixels per second: a pixel every 16 milliseconds, a hand placing
/// the cursor rather than moving it.
const FINE_DRAG_SPEED: f32 = 60.0;

/// Cursor speed at and above which it turns at the coarse gain: an ordinary
/// sweep across the globe.
const COARSE_DRAG_SPEED: f32 = 600.0;

/// How long the speed estimate takes to follow the hand, in seconds.
///
/// Deltas arrive as whole pixels at whatever rate the mouse reports, so a raw
/// per-event speed is noisy by a factor of two and the gain would flicker
/// between the two ends over one slow drag.
const DRAG_SPEED_TIME_CONSTANT: f32 = 0.06;

/// Longest interval between two moves that still counts as one drag, in
/// seconds. A pause reads as a slow start rather than as a jump.
pub const DRAG_SPEED_MAX_INTERVAL: f32 = 0.5;

/// Shortest interval an estimate divides by, in seconds. Two moves delivered
/// in the same clock tick would otherwise read as infinite speed.
const DRAG_SPEED_MIN_INTERVAL: f32 = 0.001;

/// Degrees the globe turns per pixel of an ordinary sweep.
///
/// Distance-proportional, so the painted surface moves at a fixed multiple of
/// the cursor whatever the zoom is. This is the gain the drag has always had.
///
/// :param zoom: normalized zoom slider value (0.0 to 1.0)
/// :returns: degrees per pixel
#[must_use]
pub fn coarse_drag_gain(zoom: f32) -> f32 {
    0.3 * zoom_to_distance(zoom) / 8.0
}

/// Degrees the globe turns per pixel of a deliberate hand.
///
/// Half the angle one pixel spans at the center of the sky lens, so the Sun's
/// image moves half a pixel per pixel of cursor near the frame's center and
/// about one at its edge, at every camera distance.
///
/// :param `sky_fov_deg`: the sky lens's field of view in degrees
/// :param `preview_width_px`: the preview's width in logical pixels
/// :returns: degrees per pixel
#[must_use]
pub fn fine_drag_gain(sky_fov_deg: f32, preview_width_px: f32) -> f32 {
    let edge = sky_lens::sky_lens_edge_radius(sky_fov_deg);
    (2.0 * edge / preview_width_px.max(1.0)).to_degrees()
}

/// Blend the two gains by how fast the cursor is moving.
///
/// Smoothstep in the logarithm of the speed, because what separates a placing
/// hand from a sweeping one is a ratio rather than a difference. The fine gain
/// never exceeds the coarse one: at the nearest zoom the two cross, and a
/// slow hand there should not turn the globe faster than a fast one.
///
/// The sweeping end returns the coarse gain itself rather than the interpolation
/// evaluated at 1, because `fine + (coarse - fine)` is a rounding away from
/// `coarse` at some of the zooms the slider reaches, and a sweep has to turn the
/// globe by exactly what it always turned it by.
///
/// :param `speed_px_per_sec`: the cursor's smoothed speed
/// :param coarse: the gain for a moving hand
/// :param fine: the gain for a slow one
/// :returns: degrees per pixel
#[must_use]
pub fn drag_gain(speed_px_per_sec: f32, coarse: f32, fine: f32) -> f32 {
    let fine = fine.min(coarse);
    let speed = speed_px_per_sec.max(FINE_DRAG_SPEED);
    let t = ((speed / FINE_DRAG_SPEED).ln() / (COARSE_DRAG_SPEED / FINE_DRAG_SPEED).ln())
        .clamp(0.0, 1.0);
    if t >= 1.0 {
        return coarse;
    }
    let eased = t * t * (3.0 - 2.0 * t);
    fine + (coarse - fine) * eased
}

/// The cursor's speed, smoothed, in logical pixels per second.
///
/// One `moved` callback carries a delta and the time since the previous one
/// carries the interval, and speed is the ratio: whether the toolkit delivers
/// one event per mouse report or one per frame changes both together and
/// leaves the speed alone.
#[derive(Clone, Copy, Debug, Default)]
pub struct DragSpeed {
    speed: f32,
}

impl DragSpeed {
    /// Fold one move into the estimate and return it.
    ///
    /// :param `delta_px`: how far the cursor moved, in logical pixels
    /// :param `seconds`: how long since the previous move
    /// :returns: the smoothed speed in logical pixels per second
    pub fn observe(&mut self, delta_px: f32, seconds: f32) -> f32 {
        let interval = seconds.clamp(DRAG_SPEED_MIN_INTERVAL, DRAG_SPEED_MAX_INTERVAL);
        let instant = delta_px.abs() / interval;
        let alpha = 1.0 - (-interval / DRAG_SPEED_TIME_CONSTANT).exp();
        self.speed += alpha * (instant - self.speed);
        self.speed
    }
}

/// Apply framing offset adjustment from a right-drag.
///
/// :param `offset_x`: current horizontal offset
/// :param `offset_y`: current vertical offset
/// :param zoom: normalized zoom slider value (0.0 to 1.0)
/// :param dx: horizontal mouse delta in pixels
/// :param dy: vertical mouse delta in pixels
/// :returns: new `(offset_x, offset_y)` tuple, clamped to `[-3.0, 3.0]`
#[must_use]
pub fn apply_frame_drag(offset_x: f32, offset_y: f32, zoom: f32, dx: f32, dy: f32) -> (f32, f32) {
    let sensitivity = 0.002 * zoom_to_distance(zoom) / 8.0;
    let new_x = (offset_x - dx * sensitivity).clamp(-3.0, 3.0);
    let new_y = (offset_y + dy * sensitivity).clamp(-3.0, 3.0);
    (new_x, new_y)
}

/// Apply yaw/pitch adjustment from a middle-drag.
///
/// :param yaw: current yaw in degrees
/// :param pitch: current pitch in degrees
/// :param dx: horizontal mouse delta in pixels
/// :param dy: vertical mouse delta in pixels
/// :returns: new `(yaw, pitch)` tuple, clamped to `[-90.0, 90.0]`
#[must_use]
pub fn apply_orient_drag(yaw: f32, pitch: f32, dx: f32, dy: f32) -> (f32, f32) {
    let degrees_per_px = 0.2;
    let new_yaw = (yaw + dx * degrees_per_px).clamp(-90.0, 90.0);
    let new_pitch = (pitch - dy * degrees_per_px).clamp(-90.0, 90.0);
    (new_yaw, new_pitch)
}

/// Apply tilt adjustment from a left+right drag (horizontal only).
///
/// :param tilt: current tilt in degrees
/// :param dx: horizontal mouse delta in pixels
/// :returns: new tilt in degrees, wrapped to `[-180, 180)`
#[must_use]
pub fn apply_tilt_drag(tilt: f32, dx: f32) -> f32 {
    let degrees_per_px = 0.5;
    let new_tilt = tilt + dx * degrees_per_px;
    wrap_angle_180(new_tilt)
}

/// Apply zoom from a mouse scroll event.
///
/// :param zoom: current normalized zoom value (0.0 to 1.0)
/// :param delta: scroll delta (positive = scroll up)
/// :returns: new zoom value, clamped to `[0.0, 1.0]`
#[must_use]
pub fn apply_zoom_scroll(zoom: f32, delta: f32) -> f32 {
    let scroll_sensitivity = 0.0003;
    (zoom - delta * scroll_sensitivity).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;

    use super::*;

    // --- wrap_longitude ---

    /// The half-open range is what makes the wrap a normal form: 180 comes back
    /// as -180 so that one angle has one spelling. `wrap_angle_180` answers the
    /// same question and has to agree everywhere.
    #[test]
    fn an_angle_wraps_into_the_half_open_range_around_zero() {
        for (angle, wrapped) in [
            (0.0, 0.0),
            (-90.0, -90.0),
            (360.0, 0.0),
            (-360.0, 0.0),
            (540.0, -180.0),
            (-180.0, -180.0),
            (180.0, -180.0),
        ] {
            assert_relative_eq!(wrap_longitude(angle), wrapped);
            assert_relative_eq!(wrap_angle_180(angle), wrapped);
        }
    }

    // --- apply_globe_drag ---

    #[test]
    fn globe_drag_no_movement() {
        let (lon, lat) = apply_globe_drag(10.0, 20.0, 0.0, 0.5, 0.0, 0.0);
        assert_relative_eq!(lon, 10.0);
        assert_relative_eq!(lat, 20.0);
    }

    #[test]
    fn globe_drag_horizontal_without_tilt() {
        let (lon, _lat) = apply_globe_drag(0.0, 0.0, 0.0, 0.5, 10.0, 0.0);
        assert!(lon < 0.0, "dragging right should decrease longitude");
    }

    #[test]
    fn globe_drag_vertical_without_tilt() {
        let (_lon, lat) = apply_globe_drag(0.0, 0.0, 0.0, 0.5, 0.0, -10.0);
        assert!(lat < 0.0, "dragging up should decrease latitude");
    }

    /// A drag that would take the camera over a pole stops just short of it,
    /// at either end. That the whole range is respected for any drag is
    /// `globe_drag_latitude_always_in_range`.
    #[test]
    fn globe_drag_latitude_stops_at_both_poles() {
        let (_lon, north) = apply_globe_drag(0.0, 88.0, 0.0, 0.5, 0.0, 1000.0);
        assert_relative_eq!(north, 89.0);
        let (_lon, south) = apply_globe_drag(0.0, -88.0, 0.0, 0.5, 0.0, -1000.0);
        assert_relative_eq!(south, -89.0);
    }

    #[test]
    fn globe_drag_tilt_correction_rotates_deltas() {
        let (_lon_tilted, lat_tilted) = apply_globe_drag(0.0, 0.0, 90.0, 0.5, 10.0, 0.0);
        let (_lon_normal, lat_normal) = apply_globe_drag(0.0, 0.0, 0.0, 0.5, 10.0, 0.0);

        assert!(
            lat_tilted.abs() > lat_normal.abs(),
            "90-degree tilt should redirect horizontal drag to latitude"
        );
    }

    #[test]
    fn globe_drag_zoom_sensitivity() {
        let (lon_close, _) = apply_globe_drag(0.0, 0.0, 0.0, 0.0, 10.0, 0.0);
        let (lon_far, _) = apply_globe_drag(0.0, 0.0, 0.0, 1.0, 10.0, 0.0);
        assert!(
            lon_far.abs() > lon_close.abs(),
            "farther zoom should produce larger longitude change"
        );
    }

    // --- the speed-dependent gain ---

    /// Where the sky lens puts a direction `theta` off the view axis, in
    /// pixels from the frame's center.
    ///
    /// Through `sky_lens_disc`, which is the projection the Sun's own image
    /// goes through, so what this measures is where the Sun would be drawn
    /// rather than a second spelling of the lens.
    fn image_pixels(theta_deg: f32, sky_fov_deg: f32, width: f32) -> f32 {
        let viewport = glam::Vec2::new(width, width * 9.0 / 16.0);
        let theta = theta_deg.to_radians();
        let direction = glam::Vec3::new(theta.sin(), 0.0, -theta.cos());
        let disc = sky_lens::sky_lens_disc(
            direction,
            0.001_f32.to_radians(),
            sky_fov_deg,
            glam::Vec2::ZERO,
            viewport,
        )
        .expect("a direction near the view axis has an image");
        disc.center.x - viewport.x * 0.5
    }

    #[test]
    fn the_fine_gain_moves_the_suns_image_half_a_pixel() {
        // Measured across four degrees of sky rather than across one gain,
        // because recovering an angle of a hundredth of a degree from a
        // direction costs an `acos` of a cosine that is 1.0 in f32. Over four
        // degrees the lens is still linear to a part in ten thousand, so the
        // span holds one pixel for every two gains inside it.
        let width = 1920.0;
        let half_span = 2.0_f32;
        for sky_fov in [60.0_f32, 140.0, 180.0] {
            let gain = fine_drag_gain(sky_fov, width);
            let span =
                image_pixels(half_span, sky_fov, width) - image_pixels(-half_span, sky_fov, width);
            assert_relative_eq!(span, half_span / gain, max_relative = 0.001);
        }
    }

    #[test]
    fn a_slow_hand_turns_the_globe_at_the_same_rate_at_every_zoom() {
        // The fine gain is a property of the sky lens rather than of the
        // camera, so the only thing that can make a slow drag differ between
        // two zooms is the cap against the coarse gain. Walking the whole
        // slider is what says where that cap is active.
        for (sky_fov, capped_somewhere) in [(140.0_f32, false), (180.0, true)] {
            let fine = fine_drag_gain(sky_fov, 1920.0);
            let mut capped = 0;
            for step in 0..=1000_u16 {
                let coarse = coarse_drag_gain(f32::from(step) / 1000.0);
                let gain = drag_gain(0.0, coarse, fine);
                if coarse < fine {
                    capped += 1;
                    assert_relative_eq!(gain, coarse);
                } else {
                    assert_relative_eq!(gain, fine);
                }
            }
            assert_eq!(
                capped > 0,
                capped_somewhere,
                "at {sky_fov} degrees of sky the coarse gain capped the fine one                  at {capped} of 1001 zooms"
            );
        }
    }

    #[test]
    fn the_fine_gain_never_exceeds_the_coarse_one() {
        // The nearest zoom against the widest sky is the one framing where a
        // deliberate hand would otherwise turn the globe faster than a sweeping
        // one.
        let coarse = coarse_drag_gain(0.0);
        let fine = fine_drag_gain(180.0, 1920.0);
        assert!(fine > coarse, "the two no longer cross at the nearest zoom");
        assert_relative_eq!(drag_gain(0.0, coarse, fine), coarse);
    }

    #[test]
    fn a_slow_hand_gets_the_fine_gain_and_a_sweep_the_coarse_one() {
        let coarse = coarse_drag_gain(0.5);
        let fine = fine_drag_gain(140.0, 1920.0);
        assert_relative_eq!(drag_gain(0.0, coarse, fine), fine);
        assert_relative_eq!(drag_gain(FINE_DRAG_SPEED, coarse, fine), fine);
        assert_relative_eq!(drag_gain(COARSE_DRAG_SPEED, coarse, fine), coarse);
        assert_relative_eq!(drag_gain(10_000.0, coarse, fine), coarse);
    }

    #[test]
    fn a_sweep_turns_the_globe_exactly_as_it_always_did() {
        // Every zoom the slider reaches, because the blend's own arithmetic is
        // where this could fail: `fine + (coarse - fine)` rounds away from
        // `coarse` at some of them, which is why the sweeping end returns the
        // coarse gain itself.
        let fine = fine_drag_gain(140.0, 1920.0);
        for step in 0..=1000_u16 {
            let zoom = f32::from(step) / 1000.0;
            let gain = drag_gain(COARSE_DRAG_SPEED, coarse_drag_gain(zoom), fine);
            let swept = apply_globe_drag_at(12.0, -5.0, 21.0, 9.0, -4.0, gain);
            let before = apply_globe_drag(12.0, -5.0, 21.0, zoom, 9.0, -4.0);
            assert_eq!(
                swept, before,
                "a sweep at zoom {zoom} turned the globe differently"
            );
        }
    }

    #[test]
    fn the_speed_estimate_follows_a_constant_hand() {
        let mut speed = DragSpeed::default();
        let step = DRAG_SPEED_TIME_CONSTANT / 4.0;
        let target = 240.0;
        let mut estimate = 0.0;
        for _ in 0..12 {
            estimate = speed.observe(target * step, step);
        }
        assert_relative_eq!(estimate, target, max_relative = 0.05);
    }

    #[test]
    fn a_pause_reads_as_a_slow_start() {
        // Ten pixels after two seconds of stillness is two pixels a second if
        // the interval is taken at face value, and twenty if it is capped.
        let mut capped = DragSpeed::default();
        let mut at_the_cap = DragSpeed::default();
        assert_relative_eq!(
            capped.observe(10.0, 2.0),
            at_the_cap.observe(10.0, DRAG_SPEED_MAX_INTERVAL)
        );
    }

    // --- apply_frame_drag ---

    #[test]
    fn frame_drag_no_movement() {
        let (x, y) = apply_frame_drag(1.0, 1.0, 0.5, 0.0, 0.0);
        assert_relative_eq!(x, 1.0);
        assert_relative_eq!(y, 1.0);
    }

    /// Both ends of the pan clamp. The range itself is `frame_drag_always_in_range`.
    #[test]
    fn frame_drag_clamps_at_both_ends() {
        let (x, y) = apply_frame_drag(2.9, 2.9, 0.5, -10000.0, 10000.0);
        assert_relative_eq!(x, 3.0);
        assert_relative_eq!(y, 3.0);
        let (x, y) = apply_frame_drag(-2.9, -2.9, 0.5, 10000.0, -10000.0);
        assert_relative_eq!(x, -3.0);
        assert_relative_eq!(y, -3.0);
    }

    #[test]
    fn frame_drag_zoom_sensitivity() {
        let (x_close, _) = apply_frame_drag(0.0, 0.0, 0.0, 100.0, 0.0);
        let (x_far, _) = apply_frame_drag(0.0, 0.0, 1.0, 100.0, 0.0);
        assert!(
            x_far.abs() > x_close.abs(),
            "farther zoom should produce larger offset change"
        );
    }

    // --- apply_orient_drag ---

    #[test]
    fn orient_drag_no_movement() {
        let (yaw, pitch) = apply_orient_drag(10.0, 20.0, 0.0, 0.0);
        assert_relative_eq!(yaw, 10.0);
        assert_relative_eq!(pitch, 20.0);
    }

    /// Both ends of the orientation clamp. The range itself is
    /// `orient_drag_always_in_range`.
    #[test]
    fn orient_drag_clamps_at_both_ends() {
        let (yaw, pitch) = apply_orient_drag(89.0, 89.0, 100.0, -100.0);
        assert_relative_eq!(yaw, 90.0);
        assert_relative_eq!(pitch, 90.0);
        let (yaw, pitch) = apply_orient_drag(-89.0, -89.0, -100.0, 100.0);
        assert_relative_eq!(yaw, -90.0);
        assert_relative_eq!(pitch, -90.0);
    }

    // --- apply_tilt_drag ---

    #[test]
    fn tilt_drag_no_movement() {
        assert_relative_eq!(apply_tilt_drag(45.0, 0.0), 45.0);
    }

    #[test]
    fn tilt_drag_wraps_positive() {
        // 170 + 30 * 0.5 = 185, wraps to -175
        let result = apply_tilt_drag(170.0, 30.0);
        assert_relative_eq!(result, -175.0);
    }

    #[test]
    fn tilt_drag_wraps_negative() {
        // -170 - 30 * 0.5 = -185, wraps to 175
        let result = apply_tilt_drag(-170.0, -30.0);
        assert_relative_eq!(result, 175.0);
    }

    // --- apply_zoom_scroll ---

    #[test]
    fn zoom_scroll_no_delta() {
        assert_relative_eq!(apply_zoom_scroll(0.5, 0.0), 0.5);
    }

    /// Both ends of the zoom clamp. The range itself is
    /// `zoom_scroll_always_in_unit_range`.
    #[test]
    fn zoom_scroll_clamps_at_both_ends() {
        assert_relative_eq!(apply_zoom_scroll(0.01, 10000.0), 0.0);
        assert_relative_eq!(apply_zoom_scroll(0.99, -10000.0), 1.0);
    }

    #[test]
    fn zoom_scroll_up_decreases() {
        // Positive delta (scroll up) should decrease zoom (zoom in)
        let result = apply_zoom_scroll(0.5, 100.0);
        assert!(result < 0.5, "scroll up should decrease zoom");
    }

    // --- proptests ---

    mod proptests {
        use proptest::prelude::*;

        use super::super::*;

        proptest! {
            #[test]
            fn wrap_longitude_always_in_range(lon in -1000.0f32..1000.0) {
                let wrapped = wrap_longitude(lon);
                prop_assert!(wrapped >= -180.0, "wrapped longitude {wrapped} < -180");
                prop_assert!(wrapped < 180.0, "wrapped longitude {wrapped} >= 180");
            }

            #[test]
            fn zoom_scroll_always_in_unit_range(
                zoom in 0.0f32..=1.0,
                delta in -10000.0f32..10000.0,
            ) {
                let result = apply_zoom_scroll(zoom, delta);
                prop_assert!(result >= 0.0, "zoom {result} < 0");
                prop_assert!(result <= 1.0, "zoom {result} > 1");
            }

            #[test]
            fn globe_drag_latitude_always_in_range(
                lat in -89.0f32..=89.0,
                tilt in -180.0f32..180.0,
                zoom in 0.0f32..=1.0,
                dx in -500.0f32..500.0,
                dy in -500.0f32..500.0,
            ) {
                let (_lon, new_lat) = apply_globe_drag(0.0, lat, tilt, zoom, dx, dy);
                prop_assert!(new_lat >= -89.0, "latitude {new_lat} < -89");
                prop_assert!(new_lat <= 89.0, "latitude {new_lat} > 89");
            }

            #[test]
            fn globe_drag_longitude_always_in_range(
                lon in -180.0f32..180.0,
                tilt in -180.0f32..180.0,
                zoom in 0.0f32..=1.0,
                dx in -500.0f32..500.0,
                dy in -500.0f32..500.0,
            ) {
                let (new_lon, _lat) = apply_globe_drag(lon, 0.0, tilt, zoom, dx, dy);
                prop_assert!(new_lon >= -180.0, "longitude {new_lon} < -180");
                prop_assert!(new_lon < 180.0, "longitude {new_lon} >= 180");
            }

            #[test]
            fn drag_gain_stays_between_its_two_ends(
                speed in 0.0f32..5000.0,
                coarse in 0.001f32..1.0,
                fine in 0.001f32..1.0,
            ) {
                let gain = drag_gain(speed, coarse, fine);
                let low = fine.min(coarse);
                prop_assert!(gain >= low - 1e-6, "gain {gain} below its fine end {low}");
                prop_assert!(gain <= coarse + 1e-6, "gain {gain} above its coarse end {coarse}");
            }

            #[test]
            fn drag_gain_rises_with_the_cursors_speed(
                slower in 0.0f32..5000.0,
                faster in 0.0f32..5000.0,
                coarse in 0.001f32..1.0,
                fine in 0.001f32..1.0,
            ) {
                let (slower, faster) = (slower.min(faster), slower.max(faster));
                let at_slower = drag_gain(slower, coarse, fine);
                let at_faster = drag_gain(faster, coarse, fine);
                prop_assert!(
                    at_faster >= at_slower - 1e-6,
                    "{faster} px/s gave {at_faster}, less than the {at_slower} of {slower}"
                );
            }

            #[test]
            fn drag_gain_is_continuous_in_the_speed(
                speed in 0.0f32..5000.0,
                coarse in 0.001f32..1.0,
                fine in 0.001f32..1.0,
            ) {
                // A percent of the speed either side of any point moves the
                // gain by a small part of the distance between its two ends,
                // which is what says there is no step anywhere in the blend.
                let span = (coarse - fine.min(coarse)).abs();
                let below = drag_gain(speed * 0.99, coarse, fine);
                let above = drag_gain(speed * 1.01, coarse, fine);
                prop_assert!(
                    (above - below).abs() <= span * 0.02 + 1e-6,
                    "the gain stepped from {below} to {above} over a percent of {speed} px/s"
                );
            }

            #[test]
            fn the_speed_estimate_is_never_negative_or_wild(
                delta in -2000.0f32..2000.0,
                seconds in -1.0f32..10.0,
            ) {
                let mut speed = DragSpeed::default();
                let estimate = speed.observe(delta, seconds);
                prop_assert!(estimate >= 0.0, "speed {estimate} is negative");
                prop_assert!(estimate.is_finite(), "speed {estimate} is not finite");
            }

            #[test]
            fn frame_drag_always_in_range(
                ox in -3.0f32..=3.0,
                oy in -3.0f32..=3.0,
                zoom in 0.0f32..=1.0,
                dx in -500.0f32..500.0,
                dy in -500.0f32..500.0,
            ) {
                let (new_x, new_y) = apply_frame_drag(ox, oy, zoom, dx, dy);
                prop_assert!((-3.0..=3.0).contains(&new_x), "offset_x {new_x} out of range");
                prop_assert!((-3.0..=3.0).contains(&new_y), "offset_y {new_y} out of range");
            }

            #[test]
            fn orient_drag_always_in_range(
                yaw in -90.0f32..=90.0,
                pitch in -90.0f32..=90.0,
                dx in -500.0f32..500.0,
                dy in -500.0f32..500.0,
            ) {
                let (new_yaw, new_pitch) = apply_orient_drag(yaw, pitch, dx, dy);
                prop_assert!((-90.0..=90.0).contains(&new_yaw), "yaw {new_yaw} out of range");
                prop_assert!((-90.0..=90.0).contains(&new_pitch), "pitch {new_pitch} out of range");
            }
        }
    }
}
