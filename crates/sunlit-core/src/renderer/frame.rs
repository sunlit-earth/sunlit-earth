use crate::params::{ParamsDigest, SceneParams, quantize_direction};
use crate::scene::camera::CameraParams;
use crate::scene::sky::SkyState;

/// Snapshot of everything that affects the rendered image.
///
/// The scene parameters contribute their quantized digest; the render target
/// size and the sun direction are derived per frame and therefore live here
/// rather than in `SceneParams`. So does the camera the frame is drawn with,
/// which the Sun-fixed mode resolves from the sky: the sun direction is
/// compared to a milliradian, and the Earth takes 14 seconds to turn that far,
/// which at the closest zoom is several pixels of the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FrameState {
    pub params: ParamsDigest,
    /// The camera resolved under the frame's sky, compared exactly like the
    /// digest's.
    pub camera: CameraParams,
    pub width: u32,
    pub height: u32,
    /// Sun direction quantized to integer milliradians for stable comparison.
    pub sun_direction: [i32; 3],
    /// First sky rotation basis vector, quantized to integer milliradians.
    pub sky_basis_x: [i32; 3],
    /// Second sky rotation basis vector, quantized to integer milliradians.
    pub sky_basis_y: [i32; 3],
}

/// Build a `FrameState` for dirty-check comparison.
pub(crate) fn build_frame_state(
    params: &SceneParams,
    render_width: u32,
    render_height: u32,
    sky: &SkyState,
) -> FrameState {
    FrameState {
        params: params.digest(),
        camera: params.camera_at(sky),
        width: render_width,
        height: render_height,
        sun_direction: quantize_direction(sky.sun_direction),
        sky_basis_x: quantize_direction(sky.world_from_eqj.x_axis),
        sky_basis_y: quantize_direction(sky.world_from_eqj.y_axis),
    }
}

#[cfg(test)]
mod tests {
    use crate::scene::sky::{PlanetKind, PlanetState};

    use super::*;

    fn sky(sun: glam::Vec3) -> SkyState {
        let planet = PlanetState {
            kind: PlanetKind::Mercury,
            direction: glam::Vec3::X,
            magnitude: 0.0,
        };
        SkyState {
            world_from_eqj: glam::Mat3::IDENTITY,
            sun_direction: sun,
            planets: [planet; 5],
            moon_position: glam::Vec3::new(0.0, 0.0, 60.0),
            moon_rotation: glam::Mat3::IDENTITY,
            shadow: crate::scene::eclipse::ShadowAxis::away_from_the_earth(),
        }
    }

    fn state(width: u32, height: u32, sun: glam::Vec3) -> FrameState {
        build_frame_state(&SceneParams::default(), width, height, &sky(sun))
    }

    const SUN: glam::Vec3 = glam::Vec3::new(0.1234, -0.5678, 0.9012);

    #[test]
    fn identical_inputs_compare_equal() {
        assert_eq!(state(1920, 1080, SUN), state(1920, 1080, SUN));
    }

    #[test]
    fn width_change_triggers_dirty() {
        assert_ne!(state(1920, 1080, SUN), state(1024, 1080, SUN));
    }

    #[test]
    fn height_change_triggers_dirty() {
        assert_ne!(state(1920, 1080, SUN), state(1920, 720, SUN));
    }

    #[test]
    fn sun_direction_change_triggers_dirty() {
        let moved = glam::Vec3::new(0.5, SUN.y, SUN.z);
        assert_ne!(state(1920, 1080, SUN), state(1920, 1080, moved));
    }

    #[test]
    fn sub_milliradian_sun_movement_compares_equal() {
        let jitter = glam::Vec3::new(0.123_45, SUN.y, SUN.z);
        assert_eq!(state(1920, 1080, SUN), state(1920, 1080, jitter));
    }

    #[test]
    fn scene_parameter_change_triggers_dirty() {
        let changed = SceneParams {
            cloud_opacity: 0.1,
            ..SceneParams::default()
        };
        assert_ne!(
            state(1920, 1080, SUN),
            build_frame_state(&changed, 1920, 1080, &sky(SUN))
        );
    }

    #[test]
    fn sky_rotation_change_triggers_dirty() {
        let mut moved = sky(SUN);
        moved.world_from_eqj = glam::Mat3::from_rotation_y(0.01);
        assert_ne!(
            state(1920, 1080, SUN),
            build_frame_state(&SceneParams::default(), 1920, 1080, &moved)
        );
    }

    fn key(params: &SceneParams, sun: glam::Vec3) -> FrameState {
        build_frame_state(params, 1920, 1080, &sky(sun))
    }

    /// Each mode reads one of the two keys, and the frame changes with that
    /// one alone.
    #[test]
    fn the_frame_key_follows_the_key_the_camera_mode_reads() {
        let earth = SceneParams::default();
        let sun_fixed = SceneParams {
            sun_fixed: true,
            ..earth
        };
        let later = |p: SceneParams| SceneParams {
            solar_time: p.solar_time + 0.25,
            ..p
        };
        let east = |p: SceneParams| SceneParams {
            camera: CameraParams {
                longitude: p.camera.longitude + 3.0,
                ..p.camera
            },
            ..p
        };
        assert_ne!(key(&sun_fixed, SUN), key(&later(sun_fixed), SUN));
        assert_eq!(key(&sun_fixed, SUN), key(&east(sun_fixed), SUN));
        assert_eq!(key(&earth, SUN), key(&later(earth), SUN));
        assert_ne!(key(&earth, SUN), key(&east(earth), SUN));
    }

    /// The Earth's turn moves a Sun-fixed camera, and a turn well short of the
    /// milliradian the sun direction is compared to is still a new frame. An
    /// Earth-fixed camera does not move with it, so the same turn draws
    /// nothing there.
    #[test]
    fn a_turn_under_a_milliradian_redraws_only_the_sun_fixed_camera() {
        let turned = glam::Mat3::from_rotation_y(-0.000_2) * SUN;
        assert_eq!(quantize_direction(turned), quantize_direction(SUN));
        let sun_fixed = SceneParams {
            sun_fixed: true,
            ..SceneParams::default()
        };
        assert_ne!(key(&sun_fixed, SUN), key(&sun_fixed, turned));
        assert_eq!(
            key(&SceneParams::default(), SUN),
            key(&SceneParams::default(), turned)
        );
    }
}
