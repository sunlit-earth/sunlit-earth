use glam::Mat4;

/// Groups all camera-related parameters that flow from the UI to the renderer.
#[derive(Clone, Copy, Debug)]
pub struct CameraParams {
    pub longitude: f32,
    pub latitude: f32,
    pub zoom: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub tilt_deg: f32,
    pub yaw_deg: f32,
    pub pitch_deg: f32,
}

impl Default for CameraParams {
    fn default() -> Self {
        Self {
            longitude: 0.0,
            latitude: 30.0,
            zoom: distance_to_zoom(8.0),
            offset_x: 0.0,
            offset_y: 0.0,
            tilt_deg: 0.0,
            yaw_deg: 0.0,
            pitch_deg: 0.0,
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
    /// Camera roll (tilt) in degrees
    pub tilt_deg: f32,
    /// Camera yaw (horizontal look redirection) in degrees
    pub yaw_deg: f32,
    /// Camera pitch (vertical look redirection) in degrees
    pub pitch_deg: f32,
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
            tilt_deg: 0.0,
            yaw_deg: 0.0,
            pitch_deg: 0.0,
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
    ///
    /// Applies post-view rotations: `Rz(tilt) * Rx(pitch) * Ry(yaw) * base_view`.
    /// Yaw rotates the view direction left/right, pitch rotates up/down,
    /// and tilt rolls the camera.
    pub fn view_matrix(&self) -> Mat4 {
        let eye = self.eye_position();
        let center = glam::Vec3::ZERO;
        let up = glam::Vec3::Y;
        let base_view = Mat4::look_at_rh(eye, center, up);

        let tilt = Mat4::from_rotation_z(self.tilt_deg.to_radians());
        let pitch = Mat4::from_rotation_x(self.pitch_deg.to_radians());
        let yaw = Mat4::from_rotation_y(self.yaw_deg.to_radians());

        tilt * pitch * yaw * base_view
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
    fn view_with_zero_rotations_unchanged() {
        let cam_a = OrbitalCamera::new(10.0, 20.0, 5.0);
        let mut cam_b = OrbitalCamera::new(10.0, 20.0, 5.0);
        cam_b.tilt_deg = 0.0;
        cam_b.yaw_deg = 0.0;
        cam_b.pitch_deg = 0.0;
        let view_a = cam_a.view_matrix();
        let view_b = cam_b.view_matrix();
        assert_relative_eq!(view_a.to_cols_array().as_slice(), view_b.to_cols_array().as_slice(), epsilon = 1e-6);
    }

    #[test]
    fn view_with_180_tilt_flips_vertical() {
        let cam_no_tilt = OrbitalCamera::new(0.0, 0.0, 5.0);
        let mut cam_tilt = OrbitalCamera::new(0.0, 0.0, 5.0);
        cam_tilt.tilt_deg = 180.0;

        let point = glam::Vec4::new(0.0, 1.0, 0.0, 1.0);
        let v_no = cam_no_tilt.view_matrix() * point;
        let v_yes = cam_tilt.view_matrix() * point;

        // Y components should have opposite signs (flipped)
        assert!(
            v_no.y * v_yes.y < 0.0,
            "Y should flip: no_tilt.y={}, tilt_180.y={}",
            v_no.y, v_yes.y
        );
    }

    #[test]
    fn view_with_tilt_preserves_determinant() {
        for tilt in [0.0, 45.0, 90.0, 135.0, 180.0] {
            let mut cam = OrbitalCamera::new(10.0, 20.0, 5.0);
            cam.tilt_deg = tilt;
            let det = cam.view_matrix().determinant();
            assert!(det.abs() > 0.5, "Determinant should be non-zero for tilt={tilt}");
        }
    }

    #[test]
    fn tilt_360_equals_zero() {
        let cam_0 = OrbitalCamera::new(10.0, 20.0, 5.0);
        let mut cam_360 = OrbitalCamera::new(10.0, 20.0, 5.0);
        cam_360.tilt_deg = 360.0;
        let v_0 = cam_0.view_matrix();
        let v_360 = cam_360.view_matrix();
        assert_relative_eq!(v_0.to_cols_array().as_slice(), v_360.to_cols_array().as_slice(), epsilon = 1e-4);
    }

    #[test]
    fn view_with_yaw_rotates_horizontally() {
        let cam_no_yaw = OrbitalCamera::new(0.0, 0.0, 5.0);
        let mut cam_yaw = OrbitalCamera::new(0.0, 0.0, 5.0);
        cam_yaw.yaw_deg = 45.0;

        // Point directly ahead in world space (at the origin)
        let point = glam::Vec4::new(0.0, 0.0, -1.0, 1.0);
        let v_no = cam_no_yaw.view_matrix() * point;
        let v_yes = cam_yaw.view_matrix() * point;

        assert!(
            (v_no.x - v_yes.x).abs() > 1e-3,
            "Yaw should change X: no_yaw.x={}, yaw_45.x={}",
            v_no.x, v_yes.x
        );
    }

    #[test]
    fn view_with_negative_yaw_mirrors_positive() {
        let base = OrbitalCamera::new(0.0, 0.0, 5.0);
        let mut cam_pos = OrbitalCamera::new(0.0, 0.0, 5.0);
        cam_pos.yaw_deg = 30.0;
        let mut cam_neg = OrbitalCamera::new(0.0, 0.0, 5.0);
        cam_neg.yaw_deg = -30.0;

        let point = glam::Vec4::new(1.0, 0.0, 0.0, 1.0);
        let v_base = base.view_matrix() * point;
        let v_pos = cam_pos.view_matrix() * point;
        let v_neg = cam_neg.view_matrix() * point;

        // Positive and negative yaw should shift X in opposite directions
        let delta_pos = v_pos.x - v_base.x;
        let delta_neg = v_neg.x - v_base.x;
        assert!(
            delta_pos * delta_neg < 0.0,
            "Opposite yaw should produce opposite X shifts: +yaw delta={delta_pos}, -yaw delta={delta_neg}"
        );
    }

    #[test]
    fn view_with_yaw_preserves_determinant() {
        for yaw in [-90.0, -45.0, 0.0, 45.0, 90.0] {
            let mut cam = OrbitalCamera::new(10.0, 20.0, 5.0);
            cam.yaw_deg = yaw;
            let det = cam.view_matrix().determinant();
            assert!(det.abs() > 0.5, "Determinant should be non-zero for yaw={yaw}");
        }
    }

    #[test]
    fn view_with_pitch_rotates_vertically() {
        let cam_no_pitch = OrbitalCamera::new(0.0, 0.0, 5.0);
        let mut cam_pitch = OrbitalCamera::new(0.0, 0.0, 5.0);
        cam_pitch.pitch_deg = 30.0;

        let point = glam::Vec4::new(0.0, 0.0, -1.0, 1.0);
        let v_no = cam_no_pitch.view_matrix() * point;
        let v_yes = cam_pitch.view_matrix() * point;

        assert!(
            (v_no.y - v_yes.y).abs() > 1e-3,
            "Pitch should change Y: no_pitch.y={}, pitch_30.y={}",
            v_no.y, v_yes.y
        );
    }

    #[test]
    fn view_with_negative_pitch_mirrors_positive() {
        let base = OrbitalCamera::new(0.0, 0.0, 5.0);
        let mut cam_pos = OrbitalCamera::new(0.0, 0.0, 5.0);
        cam_pos.pitch_deg = 30.0;
        let mut cam_neg = OrbitalCamera::new(0.0, 0.0, 5.0);
        cam_neg.pitch_deg = -30.0;

        let point = glam::Vec4::new(0.0, 1.0, 0.0, 1.0);
        let v_base = base.view_matrix() * point;
        let v_pos = cam_pos.view_matrix() * point;
        let v_neg = cam_neg.view_matrix() * point;

        // Positive and negative pitch should shift Y in opposite directions
        let delta_pos = v_pos.y - v_base.y;
        let delta_neg = v_neg.y - v_base.y;
        assert!(
            delta_pos * delta_neg < 0.0,
            "Opposite pitch should produce opposite Y shifts: +pitch delta={delta_pos}, -pitch delta={delta_neg}"
        );
    }

    #[test]
    fn view_with_pitch_preserves_determinant() {
        for pitch in [-90.0, -45.0, 0.0, 45.0, 90.0] {
            let mut cam = OrbitalCamera::new(10.0, 20.0, 5.0);
            cam.pitch_deg = pitch;
            let det = cam.view_matrix().determinant();
            assert!(det.abs() > 0.5, "Determinant should be non-zero for pitch={pitch}");
        }
    }

    #[test]
    fn all_rotations_zero_equals_no_rotation() {
        let cam_base = OrbitalCamera::new(15.0, 25.0, 6.0);
        let mut cam_zero = OrbitalCamera::new(15.0, 25.0, 6.0);
        cam_zero.tilt_deg = 0.0;
        cam_zero.yaw_deg = 0.0;
        cam_zero.pitch_deg = 0.0;
        assert_relative_eq!(
            cam_base.view_matrix().to_cols_array().as_slice(),
            cam_zero.view_matrix().to_cols_array().as_slice(),
            epsilon = 1e-6
        );
    }

    #[test]
    fn rotations_compose_correctly() {
        let mut cam = OrbitalCamera::new(10.0, 20.0, 5.0);
        cam.tilt_deg = 30.0;
        cam.yaw_deg = 20.0;
        cam.pitch_deg = 10.0;
        let view_combined = cam.view_matrix();

        // Manually compose: tilt * pitch * yaw * base_view
        let base_cam = OrbitalCamera::new(10.0, 20.0, 5.0);
        let base_view = Mat4::look_at_rh(
            base_cam.eye_position(),
            glam::Vec3::ZERO,
            glam::Vec3::Y,
        );
        let tilt = Mat4::from_rotation_z(30.0_f32.to_radians());
        let pitch = Mat4::from_rotation_x(10.0_f32.to_radians());
        let yaw = Mat4::from_rotation_y(20.0_f32.to_radians());
        let manual = tilt * pitch * yaw * base_view;

        assert_relative_eq!(
            view_combined.to_cols_array().as_slice(),
            manual.to_cols_array().as_slice(),
            epsilon = 1e-5
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
