use glam::Mat4;

/// Groups all camera-related parameters that flow from the UI to the renderer.
#[derive(Clone, Copy, Debug)]
pub struct CameraParams {
    pub longitude: f32,
    pub latitude: f32,
    pub zoom: f32,
    pub offset_x: f32,
    pub offset_y: f32,
}

impl Default for CameraParams {
    fn default() -> Self {
        Self {
            longitude: 0.0,
            latitude: 30.0,
            zoom: distance_to_zoom(8.0),
            offset_x: 0.0,
            offset_y: 0.0,
        }
    }
}

/// Minimum camera distance (closest zoom).
pub const ZOOM_DISTANCE_MIN: f32 = 1.5;
/// Maximum camera distance (farthest zoom).
pub const ZOOM_DISTANCE_MAX: f32 = 80.0;

/// Map a normalized slider value (0.0 to 1.0) to a camera distance
/// using an exponential curve: `1.5 * (80.0 / 1.5)^t`.
pub fn zoom_to_distance(t: f32) -> f32 {
    ZOOM_DISTANCE_MIN * (ZOOM_DISTANCE_MAX / ZOOM_DISTANCE_MIN).powf(t)
}

/// Inverse of `zoom_to_distance`: convert a camera distance back to
/// a normalized slider value.
pub fn distance_to_zoom(distance: f32) -> f32 {
    (distance / ZOOM_DISTANCE_MIN).ln() / (ZOOM_DISTANCE_MAX / ZOOM_DISTANCE_MIN).ln()
}

/// Orbital camera that orbits around the origin.
///
/// Longitude rotates around the Y axis, latitude tilts up/down,
/// and distance controls how far the camera is from the origin.
pub struct OrbitalCamera {
    /// Camera longitude in degrees (-180 to 180)
    pub longitude_deg: f32,
    /// Camera latitude in degrees (-90 to 90)
    pub latitude_deg: f32,
    /// Distance from the origin
    pub distance: f32,
    /// Vertical field of view in degrees
    pub fov_deg: f32,
    /// Screen-space horizontal offset (-1.0 to 1.0)
    pub offset_x: f32,
    /// Screen-space vertical offset (-1.0 to 1.0)
    pub offset_y: f32,
}

impl OrbitalCamera {
    pub fn new(longitude_deg: f32, latitude_deg: f32, distance: f32) -> Self {
        Self {
            longitude_deg,
            // Clamp to avoid gimbal lock: at ±90° the eye aligns with the
            // up vector, making look_at_rh produce a NaN view matrix.
            latitude_deg: latitude_deg.clamp(-89.9, 89.9),
            distance,
            fov_deg: 20.0,
            offset_x: 0.0,
            offset_y: 0.0,
        }
    }

    /// Compute the camera's position in world space.
    pub fn eye_position(&self) -> glam::Vec3 {
        let lon = self.longitude_deg.to_radians();
        let lat = self.latitude_deg.to_radians();

        let x = self.distance * lat.cos() * lon.sin();
        let y = self.distance * lat.sin();
        let z = self.distance * lat.cos() * lon.cos();

        glam::Vec3::new(x, y, z)
    }

    /// Compute the view matrix (camera looking at the origin).
    pub fn view_matrix(&self) -> Mat4 {
        let eye = self.eye_position();
        let center = glam::Vec3::ZERO;
        let up = glam::Vec3::Y;
        Mat4::look_at_rh(eye, center, up)
    }

    /// Compute the projection matrix for the given aspect ratio.
    pub fn projection_matrix(&self, aspect_ratio: f32) -> Mat4 {
        Mat4::perspective_rh(self.fov_deg.to_radians(), aspect_ratio, 0.1, 100.0)
    }

    /// Compute the combined model-view-projection matrix.
    /// The model matrix is identity (sphere at origin).
    /// Applies a post-projection translation for screen-space pan/offset.
    pub fn mvp_matrix(&self, aspect_ratio: f32) -> Mat4 {
        let base_mvp = self.projection_matrix(aspect_ratio) * self.view_matrix();
        let offset = Mat4::from_translation(glam::Vec3::new(self.offset_x, self.offset_y, 0.0));
        offset * base_mvp
    }
}

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;

    use super::*;

    #[test]
    fn camera_params_default_values() {
        let params = CameraParams::default();
        assert_relative_eq!(params.longitude, 0.0);
        assert_relative_eq!(params.latitude, 30.0);
        // Default zoom is the normalized value that produces distance 8.0
        assert_relative_eq!(zoom_to_distance(params.zoom), 8.0, epsilon = 1e-3);
    }

    #[test]
    fn zoom_to_distance_at_zero() {
        assert_relative_eq!(zoom_to_distance(0.0), 1.5, epsilon = 1e-5);
    }

    #[test]
    fn zoom_to_distance_at_one() {
        assert_relative_eq!(zoom_to_distance(1.0), 80.0, epsilon = 1e-3);
    }

    #[test]
    fn zoom_to_distance_at_half() {
        let expected = (1.5_f32 * 80.0).sqrt();
        assert_relative_eq!(zoom_to_distance(0.5), expected, epsilon = 0.1);
    }

    #[test]
    fn zoom_to_distance_monotonic() {
        let t_values = [0.0, 0.25, 0.5, 0.75, 1.0];
        let distances: Vec<f32> = t_values.iter().map(|&t| zoom_to_distance(t)).collect();
        for i in 1..distances.len() {
            assert!(
                distances[i] > distances[i - 1],
                "distance at t={} ({}) should be > distance at t={} ({})",
                t_values[i], distances[i], t_values[i - 1], distances[i - 1]
            );
        }
    }

    #[test]
    fn distance_to_zoom_roundtrip() {
        for &d in &[1.5, 3.0, 8.0, 20.0, 50.0, 80.0] {
            let t = distance_to_zoom(d);
            let roundtrip = zoom_to_distance(t);
            assert_relative_eq!(roundtrip, d, epsilon = 1e-3);
        }
    }

    #[test]
    fn distance_to_zoom_default() {
        let t = distance_to_zoom(8.0);
        assert!(t > 0.0 && t < 1.0, "default zoom t={t} should be in (0, 1)");
    }

    #[test]
    fn mvp_with_zero_offset_unchanged() {
        let cam_a = OrbitalCamera::new(10.0, 20.0, 5.0);
        let mut cam_b = OrbitalCamera::new(10.0, 20.0, 5.0);
        cam_b.offset_x = 0.0;
        cam_b.offset_y = 0.0;
        let mvp_a = cam_a.mvp_matrix(16.0 / 9.0);
        let mvp_b = cam_b.mvp_matrix(16.0 / 9.0);
        assert_relative_eq!(mvp_a.to_cols_array().as_slice(), mvp_b.to_cols_array().as_slice(), epsilon = 1e-6);
    }

    #[test]
    fn mvp_with_positive_x_offset_shifts_right() {
        let aspect = 16.0 / 9.0;
        let mut cam_no_offset = OrbitalCamera::new(0.0, 0.0, 5.0);
        cam_no_offset.offset_x = 0.0;
        let mut cam_offset = OrbitalCamera::new(0.0, 0.0, 5.0);
        cam_offset.offset_x = 0.5;

        // Transform the origin point
        let point = glam::Vec4::new(0.0, 0.0, 0.0, 1.0);
        let clip_no = cam_no_offset.mvp_matrix(aspect) * point;
        let clip_yes = cam_offset.mvp_matrix(aspect) * point;

        // The X in clip space should be larger with the positive offset
        assert!(
            clip_yes.x > clip_no.x,
            "clip_yes.x ({}) should be > clip_no.x ({})",
            clip_yes.x, clip_no.x
        );
    }

    #[test]
    fn mvp_with_offset_preserves_depth() {
        let aspect = 16.0 / 9.0;
        let cam_no_offset = OrbitalCamera::new(0.0, 0.0, 5.0);
        let mut cam_offset = OrbitalCamera::new(0.0, 0.0, 5.0);
        cam_offset.offset_x = 0.5;

        let point = glam::Vec4::new(0.0, 0.0, 0.0, 1.0);
        let clip_no = cam_no_offset.mvp_matrix(aspect) * point;
        let clip_yes = cam_offset.mvp_matrix(aspect) * point;

        assert_relative_eq!(clip_no.z, clip_yes.z, epsilon = 1e-6);
    }

    #[test]
    fn eye_at_zero_longitude_zero_latitude() {
        let cam = OrbitalCamera::new(0.0, 0.0, 5.0);
        let eye = cam.eye_position();
        // At lon=0, lat=0, camera should be on the +Z axis
        assert_relative_eq!(eye.x, 0.0, epsilon = 1e-5);
        assert_relative_eq!(eye.y, 0.0, epsilon = 1e-5);
        assert_relative_eq!(eye.z, 5.0, epsilon = 1e-5);
    }

    #[test]
    fn eye_at_90_longitude() {
        let cam = OrbitalCamera::new(90.0, 0.0, 5.0);
        let eye = cam.eye_position();
        // At lon=90, lat=0, camera should be on the +X axis
        assert_relative_eq!(eye.x, 5.0, epsilon = 1e-4);
        assert_relative_eq!(eye.y, 0.0, epsilon = 1e-5);
        assert_relative_eq!(eye.z, 0.0, epsilon = 1e-4);
    }

    #[test]
    fn eye_at_90_latitude_is_clamped() {
        let cam = OrbitalCamera::new(0.0, 90.0, 5.0);
        let eye = cam.eye_position();
        // Latitude is clamped to 89.9° to avoid gimbal lock, so the
        // camera is near (but not exactly on) the +Y axis.
        assert_relative_eq!(eye.x, 0.0, epsilon = 0.01);
        assert_relative_eq!(eye.y, 5.0, epsilon = 0.01);
        assert_relative_eq!(eye.z, 0.0, epsilon = 0.02);
    }

    #[test]
    fn mvp_is_not_identity() {
        let cam = OrbitalCamera::new(10.0, 20.0, 3.5);
        let mvp = cam.mvp_matrix(16.0 / 9.0);
        // MVP should not be identity since we have a non-trivial camera
        assert_ne!(mvp, Mat4::IDENTITY);
    }

    #[test]
    fn view_matrix_determinant_is_nonzero() {
        let cam = OrbitalCamera::new(45.0, 30.0, 4.0);
        let det = cam.view_matrix().determinant();
        assert!(det.abs() > 0.5, "View matrix should be invertible");
    }

    proptest::proptest! {
        #[test]
        fn zoom_range_proptest(t in 0.0_f32..=1.0) {
            let d = zoom_to_distance(t);
            proptest::prop_assert!(
                d >= ZOOM_DISTANCE_MIN && d <= ZOOM_DISTANCE_MAX,
                "zoom_to_distance({t}) = {d}, expected in [{}, {}]",
                ZOOM_DISTANCE_MIN, ZOOM_DISTANCE_MAX
            );
        }
    }
}
