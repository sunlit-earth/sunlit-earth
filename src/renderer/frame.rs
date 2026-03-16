/// Snapshot of inputs that affect the rendered image.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FrameState {
    pub longitude: f32,
    pub latitude: f32,
    pub zoom: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub tilt: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub sample_count: u32,
    pub texture_index: i32,
    pub width: u32,
    pub height: u32,
    /// Sun direction quantized to integer milliradians for stable comparison.
    pub sun_direction: [i32; 3],
    /// Terminator width quantized to integer milliradians.
    pub terminator_width: i32,
    /// Whether diffuse shading is enabled.
    pub diffuse_shading: bool,
    /// Diffuse floor quantized to integer thousandths.
    pub diffuse_floor: i32,
    /// Diffuse ramp quantized to integer thousandths.
    pub diffuse_ramp: i32,
}

/// Build a `FrameState` from raw values, quantizing floats to integer
/// milliradians/thousandths for stable dirty-check comparison.
#[allow(clippy::cast_possible_truncation, clippy::too_many_arguments)]
pub(crate) fn build_frame_state(
    camera: &crate::scene::camera::CameraParams,
    sample_count: u32,
    texture_index: i32,
    render_width: u32,
    render_height: u32,
    sun_dir: glam::Vec3,
    terminator_width: f32,
    diffuse_shading: bool,
    diffuse_floor: f32,
    diffuse_ramp: f32,
) -> FrameState {
    FrameState {
        longitude: camera.longitude,
        latitude: camera.latitude,
        zoom: camera.zoom,
        offset_x: camera.offset_x,
        offset_y: camera.offset_y,
        tilt: camera.tilt_deg,
        yaw: camera.yaw_deg,
        pitch: camera.pitch_deg,
        sample_count,
        texture_index,
        width: render_width,
        height: render_height,
        sun_direction: [
            (sun_dir.x * 1000.0) as i32,
            (sun_dir.y * 1000.0) as i32,
            (sun_dir.z * 1000.0) as i32,
        ],
        terminator_width: (terminator_width * 1000.0) as i32,
        diffuse_shading,
        diffuse_floor: (diffuse_floor * 1000.0) as i32,
        diffuse_ramp: (diffuse_ramp * 1000.0) as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::camera::CameraParams;

    /// Helper: build a camera params with typical values.
    fn default_camera() -> CameraParams {
        CameraParams {
            longitude: 10.0,
            latitude: 20.0,
            zoom: 3.5,
            offset_x: 0.0,
            offset_y: 0.0,
            tilt_deg: 0.0,
            yaw_deg: 0.0,
            pitch_deg: 0.0,
        }
    }

    /// Helper: build a frame state with typical values, allowing overrides.
    fn default_frame_state() -> FrameState {
        build_frame_state(
            &default_camera(),
            4,             // sample_count
            0,             // texture_index
            1920,          // render_width
            1080,          // render_height
            glam::Vec3::new(0.1234, -0.5678, 0.9012), // sun_dir
            0.15,          // terminator_width
            true,          // diffuse_shading
            0.1,           // diffuse_floor
            0.6,           // diffuse_ramp
        )
    }

    #[test]
    fn frame_state_sun_direction_quantization() {
        let state = default_frame_state();
        assert_eq!(state.sun_direction[0], 123);
        assert_eq!(state.sun_direction[1], -567);
        assert_eq!(state.sun_direction[2], 901);
    }

    #[test]
    fn frame_state_terminator_width_quantization() {
        let state = default_frame_state();
        assert_eq!(state.terminator_width, 150);
    }

    #[test]
    fn frame_state_diffuse_params_quantization() {
        let state = default_frame_state();
        assert_eq!(state.diffuse_floor, 100);
        assert_eq!(state.diffuse_ramp, 600);
    }

    #[test]
    fn frame_state_sub_threshold_change_compares_equal() {
        let cam = default_camera();
        let state_a = build_frame_state(
            &cam, 4, 0, 1920, 1080,
            glam::Vec3::new(0.1230, -0.5670, 0.9010),
            0.15, true, 0.1, 0.6,
        );
        let state_b = build_frame_state(
            &cam, 4, 0, 1920, 1080,
            glam::Vec3::new(0.1235, -0.5670, 0.9010),
            0.15, true, 0.1, 0.6,
        );
        assert_eq!(state_a, state_b, "Sub-threshold changes should compare equal");
    }

    #[test]
    fn frame_state_at_threshold_change_compares_different() {
        let cam = default_camera();
        let state_a = build_frame_state(
            &cam, 4, 0, 1920, 1080,
            glam::Vec3::new(0.1230, -0.5670, 0.9010),
            0.15, true, 0.1, 0.6,
        );
        let state_b = build_frame_state(
            &cam, 4, 0, 1920, 1080,
            glam::Vec3::new(0.1240, -0.5670, 0.9010),
            0.15, true, 0.1, 0.6,
        );
        assert_ne!(state_a, state_b, "At-threshold changes should compare different");
    }

    #[test]
    fn frame_state_each_field_triggers_dirty() {
        let base = default_frame_state();
        let cam = default_camera();
        let sun = glam::Vec3::new(0.1234, -0.5678, 0.9012);

        let modified = build_frame_state(
            &CameraParams { longitude: 11.0, ..cam }, 4, 0, 1920, 1080,
            sun, 0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "longitude change should trigger dirty");

        let modified = build_frame_state(
            &CameraParams { latitude: 21.0, ..cam }, 4, 0, 1920, 1080,
            sun, 0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "latitude change should trigger dirty");

        let modified = build_frame_state(
            &CameraParams { zoom: 4.0, ..cam }, 4, 0, 1920, 1080,
            sun, 0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "zoom change should trigger dirty");

        let modified = build_frame_state(
            &cam, 8, 0, 1920, 1080,
            sun, 0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "sample_count change should trigger dirty");

        let modified = build_frame_state(
            &cam, 4, 1, 1920, 1080,
            sun, 0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "texture_index change should trigger dirty");

        let modified = build_frame_state(
            &cam, 4, 0, 1024, 1080,
            sun, 0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "width change should trigger dirty");

        let modified = build_frame_state(
            &cam, 4, 0, 1920, 720,
            sun, 0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "height change should trigger dirty");

        let modified = build_frame_state(
            &cam, 4, 0, 1920, 1080,
            glam::Vec3::new(0.5, -0.5678, 0.9012),
            0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "sun_direction change should trigger dirty");

        let modified = build_frame_state(
            &cam, 4, 0, 1920, 1080,
            sun, 0.25, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "terminator_width change should trigger dirty");

        let modified = build_frame_state(
            &cam, 4, 0, 1920, 1080,
            sun, 0.15, false, 0.1, 0.6,
        );
        assert_ne!(base, modified, "diffuse_shading change should trigger dirty");

        let modified = build_frame_state(
            &cam, 4, 0, 1920, 1080,
            sun, 0.15, true, 0.2, 0.6,
        );
        assert_ne!(base, modified, "diffuse_floor change should trigger dirty");

        let modified = build_frame_state(
            &cam, 4, 0, 1920, 1080,
            sun, 0.15, true, 0.1, 0.7,
        );
        assert_ne!(base, modified, "diffuse_ramp change should trigger dirty");
    }

    #[test]
    fn frame_state_tilt_triggers_dirty() {
        let base = default_frame_state();
        let cam = default_camera();
        let sun = glam::Vec3::new(0.1234, -0.5678, 0.9012);

        let modified = build_frame_state(
            &CameraParams { tilt_deg: 45.0, ..cam }, 4, 0, 1920, 1080,
            sun, 0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "tilt change should trigger dirty");
    }

    #[test]
    fn frame_state_yaw_triggers_dirty() {
        let base = default_frame_state();
        let cam = default_camera();
        let sun = glam::Vec3::new(0.1234, -0.5678, 0.9012);

        let modified = build_frame_state(
            &CameraParams { yaw_deg: 30.0, ..cam }, 4, 0, 1920, 1080,
            sun, 0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "yaw change should trigger dirty");
    }

    #[test]
    fn frame_state_pitch_triggers_dirty() {
        let base = default_frame_state();
        let cam = default_camera();
        let sun = glam::Vec3::new(0.1234, -0.5678, 0.9012);

        let modified = build_frame_state(
            &CameraParams { pitch_deg: 30.0, ..cam }, 4, 0, 1920, 1080,
            sun, 0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "pitch change should trigger dirty");
    }

    #[test]
    fn frame_state_offset_triggers_dirty() {
        let base = default_frame_state();
        let cam = default_camera();
        let sun = glam::Vec3::new(0.1234, -0.5678, 0.9012);

        let modified = build_frame_state(
            &CameraParams { offset_x: 0.5, ..cam }, 4, 0, 1920, 1080,
            sun, 0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "offset_x change should trigger dirty");

        let modified = build_frame_state(
            &CameraParams { offset_y: 0.5, ..cam }, 4, 0, 1920, 1080,
            sun, 0.15, true, 0.1, 0.6,
        );
        assert_ne!(base, modified, "offset_y change should trigger dirty");
    }
}
