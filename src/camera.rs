use glam::Mat4;

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
}

/// Maximum absolute latitude to avoid gimbal lock with `look_at`.
const MAX_LATITUDE: f32 = 89.5;

impl OrbitalCamera {
    pub fn new(longitude_deg: f32, latitude_deg: f32, distance: f32) -> Self {
        Self {
            longitude_deg,
            latitude_deg: latitude_deg.clamp(-MAX_LATITUDE, MAX_LATITUDE),
            distance,
            fov_deg: 30.0,
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

        // Compute a stable up vector that stays perpendicular to the view direction.
        // The "right" vector is always horizontal (in the XZ plane), so we derive
        // up from cross(forward, right) to avoid gimbal lock near the poles.
        let lon = self.longitude_deg.to_radians();
        let right = glam::Vec3::new(lon.cos(), 0.0, -lon.sin());
        let forward = (center - eye).normalize();
        let up = right.cross(forward).normalize();

        Mat4::look_at_rh(eye, center, up)
    }

    /// Compute the projection matrix for the given aspect ratio.
    pub fn projection_matrix(&self, aspect_ratio: f32) -> Mat4 {
        Mat4::perspective_rh(self.fov_deg.to_radians(), aspect_ratio, 0.1, 100.0)
    }

    /// Compute the combined model-view-projection matrix.
    /// The model matrix is identity (sphere at origin).
    pub fn mvp_matrix(&self, aspect_ratio: f32) -> Mat4 {
        self.projection_matrix(aspect_ratio) * self.view_matrix()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eye_at_zero_longitude_zero_latitude() {
        let cam = OrbitalCamera::new(0.0, 0.0, 5.0);
        let eye = cam.eye_position();
        // At lon=0, lat=0, camera should be on the +Z axis
        assert!(eye.x.abs() < 1e-5);
        assert!(eye.y.abs() < 1e-5);
        assert!((eye.z - 5.0).abs() < 1e-5);
    }

    #[test]
    fn eye_at_90_longitude() {
        let cam = OrbitalCamera::new(90.0, 0.0, 5.0);
        let eye = cam.eye_position();
        // At lon=90, lat=0, camera should be on the +X axis
        assert!((eye.x - 5.0).abs() < 1e-4);
        assert!(eye.y.abs() < 1e-5);
        assert!(eye.z.abs() < 1e-4);
    }

    #[test]
    fn eye_at_high_latitude_is_clamped() {
        let cam = OrbitalCamera::new(0.0, 90.0, 5.0);
        // Latitude should be clamped to MAX_LATITUDE
        assert!((cam.latitude_deg - MAX_LATITUDE).abs() < 1e-5);
        let eye = cam.eye_position();
        // Should be near the +Y axis but not exactly on it
        assert!(eye.y > 4.9);
        assert!(eye.z.abs() > 0.01); // Not exactly zero — slightly off-axis
    }

    #[test]
    fn negative_latitude_is_clamped() {
        let cam = OrbitalCamera::new(0.0, -90.0, 5.0);
        assert!((cam.latitude_deg - (-MAX_LATITUDE)).abs() < 1e-5);
    }

    #[test]
    fn mvp_is_not_identity() {
        let cam = OrbitalCamera::new(10.0, 20.0, 3.5);
        let mvp = cam.mvp_matrix(16.0 / 9.0);
        assert_ne!(mvp, Mat4::IDENTITY);
    }

    #[test]
    fn view_matrix_determinant_is_nonzero() {
        let cam = OrbitalCamera::new(45.0, 30.0, 4.0);
        let det = cam.view_matrix().determinant();
        assert!(det.abs() > 0.5, "View matrix should be invertible");
    }

    #[test]
    fn view_matrix_stable_at_extreme_latitude() {
        // Even at ±89.5°, the view matrix should still be well-behaved
        for lat in [-89.5, 89.5] {
            let cam = OrbitalCamera::new(0.0, lat, 5.0);
            let det = cam.view_matrix().determinant();
            assert!(
                det.abs() > 0.5,
                "View matrix degenerate at lat={lat}, det={det}"
            );
        }
    }

    #[test]
    fn up_vector_stays_consistent_across_latitudes() {
        // When orbiting from lat=0 to lat=80, the "up" direction on screen
        // should stay consistent (no sudden flips)
        let cam_low = OrbitalCamera::new(0.0, 10.0, 5.0);
        let cam_high = OrbitalCamera::new(0.0, 80.0, 5.0);
        let view_low = cam_low.view_matrix();
        let view_high = cam_high.view_matrix();

        // The Y component of the "up" in view space (column 1, row 1)
        // should have the same sign for both — no flip
        let up_y_low = view_low.col(1).y;
        let up_y_high = view_high.col(1).y;
        assert!(
            up_y_low * up_y_high > 0.0,
            "Up vector flipped: {up_y_low} vs {up_y_high}"
        );
    }
}
