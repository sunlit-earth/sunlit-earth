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

impl OrbitalCamera {
    pub fn new(longitude_deg: f32, latitude_deg: f32, distance: f32) -> Self {
        Self {
            longitude_deg,
            // Clamp to avoid gimbal lock: at ±90° the eye aligns with the
            // up vector, making look_at_rh produce a NaN view matrix.
            latitude_deg: latitude_deg.clamp(-89.9, 89.9),
            distance,
            fov_deg: 20.0,
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
    fn eye_at_90_latitude_is_clamped() {
        let cam = OrbitalCamera::new(0.0, 90.0, 5.0);
        let eye = cam.eye_position();
        // Latitude is clamped to 89.9° to avoid gimbal lock, so the
        // camera is near (but not exactly on) the +Y axis.
        assert!(eye.x.abs() < 0.01);
        assert!((eye.y - 5.0).abs() < 0.01);
        assert!(eye.z.abs() < 0.02);
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
}
