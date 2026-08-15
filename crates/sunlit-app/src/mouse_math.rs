//! Pure math functions for mouse interaction with the globe.
//!
//! These functions are extracted from the mouse callback closures in `main.rs`
//! so they can be unit-tested independently of the Slint UI.

use sunlit_core::scene::camera::zoom_to_distance;

/// Wrap a longitude value into the `[-180, 180)` range.
#[must_use]
pub fn wrap_longitude(lon: f32) -> f32 {
    ((lon + 180.0) % 360.0 + 360.0) % 360.0 - 180.0
}

/// Wrap a tilt value into the `[-180, 180)` range.
#[must_use]
pub fn wrap_angle_180(angle: f32) -> f32 {
    ((angle + 180.0) % 360.0 + 360.0) % 360.0 - 180.0
}

/// Apply tilt-corrected globe rotation from a mouse drag.
///
/// :param lon: current longitude in degrees
/// :param lat: current latitude in degrees
/// :param `tilt_deg`: current tilt in degrees
/// :param zoom: normalized zoom slider value (0.0 to 1.0)
/// :param dx: horizontal mouse delta in pixels
/// :param dy: vertical mouse delta in pixels
/// :returns: new `(longitude, latitude)` tuple
#[must_use]
pub fn apply_globe_drag(
    lon: f32,
    lat: f32,
    tilt_deg: f32,
    zoom: f32,
    dx: f32,
    dy: f32,
) -> (f32, f32) {
    let tilt_rad = tilt_deg.to_radians();
    let cos_t = tilt_rad.cos();
    let sin_t = tilt_rad.sin();

    // Rotate the (dx, dy) vector by -tilt to undo the screen-space rotation
    let delta = [
        dx * cos_t + dy * sin_t,
        -dx * sin_t + dy * cos_t,
    ];

    // Scale sensitivity proportionally to camera distance
    let degrees_per_px = 0.3 * zoom_to_distance(zoom) / 8.0;

    let new_lon = lon - delta[0] * degrees_per_px;
    let new_lat = lat + delta[1] * degrees_per_px;

    (wrap_longitude(new_lon), new_lat.clamp(-89.0, 89.0))
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
pub fn apply_frame_drag(
    offset_x: f32,
    offset_y: f32,
    zoom: f32,
    dx: f32,
    dy: f32,
) -> (f32, f32) {
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

    #[test]
    fn wrap_longitude_identity_at_zero() {
        assert_relative_eq!(wrap_longitude(0.0), 0.0);
    }

    #[test]
    fn wrap_longitude_identity_at_negative_90() {
        assert_relative_eq!(wrap_longitude(-90.0), -90.0);
    }

    #[test]
    fn wrap_longitude_wraps_positive_360() {
        assert_relative_eq!(wrap_longitude(360.0), 0.0);
    }

    #[test]
    fn wrap_longitude_wraps_negative_360() {
        assert_relative_eq!(wrap_longitude(-360.0), 0.0);
    }

    #[test]
    fn wrap_longitude_wraps_540() {
        assert_relative_eq!(wrap_longitude(540.0), -180.0);
    }

    #[test]
    fn wrap_longitude_wraps_minus_180() {
        // -180 is the boundary; the modular wrap puts it at -180
        assert_relative_eq!(wrap_longitude(-180.0), -180.0);
    }

    #[test]
    fn wrap_longitude_wraps_180() {
        // 180 wraps to -180
        assert_relative_eq!(wrap_longitude(180.0), -180.0);
    }

    // --- wrap_angle_180 ---

    #[test]
    fn wrap_angle_180_identity_at_zero() {
        assert_relative_eq!(wrap_angle_180(0.0), 0.0);
    }

    #[test]
    fn wrap_angle_180_wraps_360() {
        assert_relative_eq!(wrap_angle_180(360.0), 0.0);
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
        // Dragging right should decrease longitude (globe rotates left)
        let (lon, _lat) = apply_globe_drag(0.0, 0.0, 0.0, 0.5, 10.0, 0.0);
        assert!(lon < 0.0, "dragging right should decrease longitude");
    }

    #[test]
    fn globe_drag_vertical_without_tilt() {
        // Dragging up (negative dy) should decrease latitude
        let (_lon, lat) = apply_globe_drag(0.0, 0.0, 0.0, 0.5, 0.0, -10.0);
        assert!(lat < 0.0, "dragging up should decrease latitude");
    }

    #[test]
    fn globe_drag_latitude_clamped_at_89() {
        let (_lon, lat) = apply_globe_drag(0.0, 88.0, 0.0, 0.5, 0.0, 1000.0);
        assert_relative_eq!(lat, 89.0);
    }

    #[test]
    fn globe_drag_latitude_clamped_at_minus_89() {
        let (_lon, lat) = apply_globe_drag(0.0, -88.0, 0.0, 0.5, 0.0, -1000.0);
        assert_relative_eq!(lat, -89.0);
    }

    #[test]
    fn globe_drag_longitude_wraps() {
        // Start near 180, drag to push past it
        let (lon, _lat) = apply_globe_drag(179.0, 0.0, 0.0, 0.5, -100.0, 0.0);
        // Should wrap around to negative side
        assert!(lon >= -180.0 && lon < 180.0, "longitude should be in [-180, 180), got {lon}");
    }

    #[test]
    fn globe_drag_tilt_correction_rotates_deltas() {
        // With 90-degree tilt, horizontal drag should affect latitude
        // and vertical drag should affect longitude
        let (_lon_tilted, lat_tilted) = apply_globe_drag(0.0, 0.0, 90.0, 0.5, 10.0, 0.0);
        let (_lon_normal, lat_normal) = apply_globe_drag(0.0, 0.0, 0.0, 0.5, 10.0, 0.0);

        // With 90-degree tilt, the horizontal drag component should mostly affect latitude
        assert!(
            lat_tilted.abs() > lat_normal.abs(),
            "90-degree tilt should redirect horizontal drag to latitude"
        );
    }

    #[test]
    fn globe_drag_zoom_sensitivity() {
        // At zoom=0 (closest), movement should be smaller than at zoom=1 (farthest)
        let (lon_close, _) = apply_globe_drag(0.0, 0.0, 0.0, 0.0, 10.0, 0.0);
        let (lon_far, _) = apply_globe_drag(0.0, 0.0, 0.0, 1.0, 10.0, 0.0);
        assert!(
            lon_far.abs() > lon_close.abs(),
            "farther zoom should produce larger longitude change"
        );
    }

    // --- apply_frame_drag ---

    #[test]
    fn frame_drag_no_movement() {
        let (x, y) = apply_frame_drag(1.0, 1.0, 0.5, 0.0, 0.0);
        assert_relative_eq!(x, 1.0);
        assert_relative_eq!(y, 1.0);
    }

    #[test]
    fn frame_drag_clamped_at_positive_3() {
        let (x, y) = apply_frame_drag(2.9, 2.9, 0.5, -10000.0, 10000.0);
        assert_relative_eq!(x, 3.0);
        assert_relative_eq!(y, 3.0);
    }

    #[test]
    fn frame_drag_clamped_at_negative_3() {
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

    #[test]
    fn orient_drag_clamped_at_90() {
        let (yaw, pitch) = apply_orient_drag(89.0, 89.0, 100.0, -100.0);
        assert_relative_eq!(yaw, 90.0);
        assert_relative_eq!(pitch, 90.0);
    }

    #[test]
    fn orient_drag_clamped_at_minus_90() {
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

    #[test]
    fn zoom_scroll_clamped_at_zero() {
        assert_relative_eq!(apply_zoom_scroll(0.01, 10000.0), 0.0);
    }

    #[test]
    fn zoom_scroll_clamped_at_one() {
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
            fn frame_drag_always_in_range(
                ox in -3.0f32..=3.0,
                oy in -3.0f32..=3.0,
                zoom in 0.0f32..=1.0,
                dx in -500.0f32..500.0,
                dy in -500.0f32..500.0,
            ) {
                let (new_x, new_y) = apply_frame_drag(ox, oy, zoom, dx, dy);
                prop_assert!(new_x >= -3.0 && new_x <= 3.0, "offset_x {new_x} out of range");
                prop_assert!(new_y >= -3.0 && new_y <= 3.0, "offset_y {new_y} out of range");
            }

            #[test]
            fn orient_drag_always_in_range(
                yaw in -90.0f32..=90.0,
                pitch in -90.0f32..=90.0,
                dx in -500.0f32..500.0,
                dy in -500.0f32..500.0,
            ) {
                let (new_yaw, new_pitch) = apply_orient_drag(yaw, pitch, dx, dy);
                prop_assert!(new_yaw >= -90.0 && new_yaw <= 90.0, "yaw {new_yaw} out of range");
                prop_assert!(new_pitch >= -90.0 && new_pitch <= 90.0, "pitch {new_pitch} out of range");
            }
        }
    }
}
