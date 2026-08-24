use crate::params::{ParamsDigest, SceneParams, quantize_direction};
use crate::scene::sky::SkyState;

/// Snapshot of everything that affects the rendered image.
///
/// The scene parameters contribute their quantized digest; the render target
/// size and the sun direction are derived per frame and therefore live here
/// rather than in `SceneParams`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FrameState {
    pub params: ParamsDigest,
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
}
