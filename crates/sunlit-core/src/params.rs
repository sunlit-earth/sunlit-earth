//! The single scene parameter struct.
//!
//! `SceneParams` is the one description of "what to draw" that flows through
//! every layer. Before it existed, the same thirty values were spelled out in
//! the Slint properties, in `AppConfig`, in `FrameState`, in `ShadingParams`,
//! and again when building `Uniforms`, so adding one shader knob meant touching
//! eight files. Now there are exactly two translation points: the Slint bridge
//! in `sunlit-app`, and the uniform encoder in `renderer::uniforms`.

use crate::config::AppConfig;
use crate::scene::camera::CameraParams;
use crate::scene::sun::DateTimeInput;

/// Radius of the cloud shell, just above the surface.
pub const CLOUD_SPHERE_RADIUS: f32 = 1.0015;
/// Radius of the Rayleigh scattering shell.
pub const RAYLEIGH_RADIUS: f32 = 1.015;
/// Radius of the orange (sodium D + iron oxide) nightglow shell.
pub const NIGHTGLOW_ORANGE_RADIUS: f32 = 1.014;
/// Radius of the green (OI 557.7nm) nightglow shell.
pub const NIGHTGLOW_GREEN_RADIUS: f32 = 1.015;

const GAMMA_MIN: f32 = 0.2;
const GAMMA_MAX: f32 = 3.0;

/// Everything that determines the rendered image, except the frame's
/// resolution and sky state (both derived: resolution from the target, sky
/// state from the clock plus `datetime`).
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct SceneParams {
    pub camera: CameraParams,

    /// Index of the selected texture mode (0 grid, 1 day, 2 night, 3 blend).
    pub texture_index: i32,
    /// MSAA sample count (1 disables multisampling).
    pub sample_count: u32,

    // Lighting
    pub terminator_width: f32,
    pub diffuse_shading: bool,
    pub diffuse_floor: f32,
    pub diffuse_ramp: f32,
    pub spec_shininess: f32,
    pub spec_intensity: f32,
    pub fresnel_mix: f32,
    pub fresnel_exp: f32,

    // Clouds
    pub cloud_opacity: f32,
    pub cloud_floor: f32,
    pub cloud_gamma: f32,

    // Atmosphere
    pub atmo_enabled: bool,
    pub rayleigh_intensity: f32,
    pub rayleigh_sharpness: f32,
    pub rayleigh_haze: f32,
    pub nightglow_intensity: f32,
    pub nightglow_falloff: f32,
    pub nightglow_balance: f32,

    // Celestial background
    pub star_intensity: f32,
    pub star_size: f32,
    pub star_glow_strength: f32,
    pub star_glow_radius: f32,
    pub star_contrast: f32,
    pub star_mag_limit: f32,

    // Color correction (gamma values, not slider positions)
    pub day_gamma: f32,
    pub day_saturation: f32,
    pub night_gamma: f32,
    pub night_saturation: f32,

    /// Which instant to render for: live UTC, or a user-chosen date and time.
    pub datetime: DateTimeInput,
}

impl Default for SceneParams {
    fn default() -> Self {
        Self::from_config(&AppConfig::default())
    }
}

impl SceneParams {
    /// Read the scene half of a persisted config.
    pub fn from_config(config: &AppConfig) -> Self {
        Self {
            camera: CameraParams {
                longitude: config.longitude,
                latitude: config.latitude,
                zoom: config.zoom,
                offset_x: config.offset_x,
                offset_y: config.offset_y,
                tilt_deg: config.tilt,
                yaw_deg: config.yaw,
                pitch_deg: config.pitch,
            },
            texture_index: config.texture_index,
            sample_count: config.sample_count,
            terminator_width: config.terminator_width,
            diffuse_shading: config.diffuse_shading,
            diffuse_floor: config.diffuse_floor,
            diffuse_ramp: config.diffuse_ramp,
            spec_shininess: config.spec_shininess,
            spec_intensity: config.spec_intensity,
            fresnel_mix: config.fresnel_mix,
            fresnel_exp: config.fresnel_exp,
            cloud_opacity: config.cloud_opacity,
            cloud_floor: config.cloud_floor,
            cloud_gamma: config.cloud_gamma,
            atmo_enabled: config.atmo_enabled,
            rayleigh_intensity: config.rayleigh_intensity,
            rayleigh_sharpness: config.rayleigh_sharpness,
            rayleigh_haze: config.rayleigh_haze,
            nightglow_intensity: config.nightglow_intensity,
            nightglow_falloff: config.nightglow_falloff,
            nightglow_balance: config.nightglow_balance,
            star_intensity: config.star_intensity,
            star_size: config.star_size,
            star_glow_strength: config.star_glow_strength,
            star_glow_radius: config.star_glow_radius,
            star_contrast: config.star_contrast,
            star_mag_limit: config.star_mag_limit,
            day_gamma: config.day_gamma,
            day_saturation: config.day_saturation,
            night_gamma: config.night_gamma,
            night_saturation: config.night_saturation,
            datetime: DateTimeInput {
                use_custom: config.use_custom_datetime,
                custom_hour: config.custom_hour,
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                custom_day_of_year: config.custom_day_of_year as u16,
                custom_year: config.custom_year,
            },
        }
    }

    /// Write the scene half back into a config, leaving window geometry and the
    /// auto-refresh settings (which are not scene parameters) untouched.
    #[allow(clippy::cast_precision_loss)]
    pub fn write_to_config(&self, config: &mut AppConfig) {
        config.longitude = self.camera.longitude;
        config.latitude = self.camera.latitude;
        config.zoom = self.camera.zoom;
        config.offset_x = self.camera.offset_x;
        config.offset_y = self.camera.offset_y;
        config.tilt = self.camera.tilt_deg;
        config.yaw = self.camera.yaw_deg;
        config.pitch = self.camera.pitch_deg;
        config.texture_index = self.texture_index;
        config.sample_count = self.sample_count;
        config.terminator_width = self.terminator_width;
        config.diffuse_shading = self.diffuse_shading;
        config.diffuse_floor = self.diffuse_floor;
        config.diffuse_ramp = self.diffuse_ramp;
        config.spec_shininess = self.spec_shininess;
        config.spec_intensity = self.spec_intensity;
        config.fresnel_mix = self.fresnel_mix;
        config.fresnel_exp = self.fresnel_exp;
        config.cloud_opacity = self.cloud_opacity;
        config.cloud_floor = self.cloud_floor;
        config.cloud_gamma = self.cloud_gamma;
        config.atmo_enabled = self.atmo_enabled;
        config.rayleigh_intensity = self.rayleigh_intensity;
        config.rayleigh_sharpness = self.rayleigh_sharpness;
        config.rayleigh_haze = self.rayleigh_haze;
        config.nightglow_intensity = self.nightglow_intensity;
        config.nightglow_falloff = self.nightglow_falloff;
        config.nightglow_balance = self.nightglow_balance;
        config.star_intensity = self.star_intensity;
        config.star_size = self.star_size;
        config.star_glow_strength = self.star_glow_strength;
        config.star_glow_radius = self.star_glow_radius;
        config.star_contrast = self.star_contrast;
        config.star_mag_limit = self.star_mag_limit;
        config.day_gamma = self.day_gamma;
        config.day_saturation = self.day_saturation;
        config.night_gamma = self.night_gamma;
        config.night_saturation = self.night_saturation;
        config.use_custom_datetime = self.datetime.use_custom;
        config.custom_hour = self.datetime.custom_hour;
        config.custom_day_of_year = f32::from(self.datetime.custom_day_of_year);
        config.custom_year = self.datetime.custom_year;
    }

    /// Rayleigh intensity after the atmosphere master switch. Zero suppresses
    /// the draw call entirely.
    pub fn effective_rayleigh_intensity(&self) -> f32 {
        if self.atmo_enabled {
            self.rayleigh_intensity
        } else {
            0.0
        }
    }

    /// Nightglow intensity after the atmosphere master switch.
    pub fn effective_nightglow_intensity(&self) -> f32 {
        if self.atmo_enabled {
            self.nightglow_intensity
        } else {
            0.0
        }
    }

    /// Quantized snapshot used for dirty checking.
    ///
    /// Camera values compare exactly; every other float is rounded to integer
    /// thousandths so that sub-visible slider jitter does not force a redraw.
    #[allow(clippy::cast_possible_truncation)]
    pub fn digest(&self) -> ParamsDigest {
        ParamsDigest {
            camera: self.camera,
            texture_index: self.texture_index,
            sample_count: self.sample_count,
            terminator_width: q(self.terminator_width),
            diffuse_shading: self.diffuse_shading,
            diffuse_floor: q(self.diffuse_floor),
            diffuse_ramp: q(self.diffuse_ramp),
            spec_shininess: q(self.spec_shininess),
            spec_intensity: q(self.spec_intensity),
            fresnel_mix: q(self.fresnel_mix),
            fresnel_exp: q(self.fresnel_exp),
            cloud_opacity: q(self.cloud_opacity),
            cloud_floor: q(self.cloud_floor),
            cloud_gamma: q(self.cloud_gamma),
            rayleigh_intensity: q(self.effective_rayleigh_intensity()),
            rayleigh_sharpness: q(self.rayleigh_sharpness),
            rayleigh_haze: q(self.rayleigh_haze),
            nightglow_intensity: q(self.effective_nightglow_intensity()),
            nightglow_falloff: q(self.nightglow_falloff),
            nightglow_balance: q(self.nightglow_balance),
            star_intensity: q(self.star_intensity),
            star_size: q(self.star_size),
            star_glow_strength: q(self.star_glow_strength),
            star_glow_radius: q(self.star_glow_radius),
            star_contrast: q(self.star_contrast),
            star_mag_limit: q(self.star_mag_limit),
            day_gamma: q(self.day_gamma),
            day_saturation: q(self.day_saturation),
            night_gamma: q(self.night_gamma),
            night_saturation: q(self.night_saturation),
        }
    }
}

/// Quantize a float to integer thousandths.
#[allow(clippy::cast_possible_truncation)]
fn q(value: f32) -> i32 {
    (value * 1000.0) as i32
}

/// Quantize a direction vector to integer milliradians for stable comparison.
#[allow(clippy::cast_possible_truncation)]
pub fn quantize_direction(dir: glam::Vec3) -> [i32; 3] {
    [q(dir.x), q(dir.y), q(dir.z)]
}

/// Dirty-check snapshot of a `SceneParams`.
///
/// `datetime` is deliberately absent: the derived sky state is compared
/// separately, so a live UTC render changes even though `datetime` does not.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamsDigest {
    pub camera: CameraParams,
    pub texture_index: i32,
    pub sample_count: u32,
    pub terminator_width: i32,
    pub diffuse_shading: bool,
    pub diffuse_floor: i32,
    pub diffuse_ramp: i32,
    pub spec_shininess: i32,
    pub spec_intensity: i32,
    pub fresnel_mix: i32,
    pub fresnel_exp: i32,
    pub cloud_opacity: i32,
    pub cloud_floor: i32,
    pub cloud_gamma: i32,
    pub rayleigh_intensity: i32,
    pub rayleigh_sharpness: i32,
    pub rayleigh_haze: i32,
    pub nightglow_intensity: i32,
    pub nightglow_falloff: i32,
    pub nightglow_balance: i32,
    pub star_intensity: i32,
    pub star_size: i32,
    pub star_glow_strength: i32,
    pub star_glow_radius: i32,
    pub star_contrast: i32,
    pub star_mag_limit: i32,
    pub day_gamma: i32,
    pub day_saturation: i32,
    pub night_gamma: i32,
    pub night_saturation: i32,
}

/// Map a normalized slider position (0.0 to 1.0) to a gamma value (0.2 to 3.0).
///
/// The midpoint (0.5) maps to gamma 1.0 (identity) so the neutral value is
/// centered on the slider. Piecewise linear: lower half spans
/// `[GAMMA_MIN, 1.0]`, upper half spans `[1.0, GAMMA_MAX]`.
pub fn gamma_slider_to_value(t: f32) -> f32 {
    if t <= 0.5 {
        GAMMA_MIN + (1.0 - GAMMA_MIN) * (t / 0.5)
    } else {
        1.0 + (GAMMA_MAX - 1.0) * ((t - 0.5) / 0.5)
    }
}

/// Inverse of `gamma_slider_to_value`: convert a gamma value back to a
/// normalized slider position.
pub fn gamma_value_to_slider(gamma: f32) -> f32 {
    if gamma <= 1.0 {
        (gamma - GAMMA_MIN) / (1.0 - GAMMA_MIN) * 0.5
    } else {
        0.5 + (gamma - 1.0) / (GAMMA_MAX - 1.0) * 0.5
    }
}

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;

    use super::*;

    /// Params with distinctive values so quantization is observable.
    fn params() -> SceneParams {
        SceneParams {
            camera: CameraParams {
                longitude: 10.0,
                latitude: 20.0,
                zoom: 0.35,
                ..CameraParams::default()
            },
            terminator_width: 0.15,
            diffuse_floor: 0.1,
            diffuse_ramp: 0.6,
            spec_shininess: 150.0,
            spec_intensity: 0.4,
            fresnel_mix: 0.5,
            cloud_opacity: 0.75,
            cloud_floor: 0.196,
            cloud_gamma: 0.3,
            day_gamma: 1.5,
            day_saturation: 0.5,
            rayleigh_intensity: 0.5,
            rayleigh_sharpness: 50.0,
            nightglow_intensity: 0.25,
            nightglow_falloff: 15.0,
            nightglow_balance: 0.37,
            ..SceneParams::default()
        }
    }

    // --- config bridge ---

    #[test]
    fn default_params_match_default_config() {
        let config = AppConfig::default();
        let params = SceneParams::from_config(&config);
        assert_eq!(params, SceneParams::default());
        assert_relative_eq!(params.camera.longitude, config.longitude);
        assert_eq!(params.sample_count, config.sample_count);
    }

    #[test]
    fn config_round_trip_preserves_scene_values() {
        let mut config = AppConfig::default();
        let original = params();
        original.write_to_config(&mut config);
        assert_eq!(SceneParams::from_config(&config), original);
    }

    #[test]
    fn write_to_config_leaves_non_scene_fields_alone() {
        let mut config = AppConfig {
            auto_refresh_enabled: true,
            auto_refresh_interval_minutes: 42,
            window_x: Some(7),
            window_width: Some(1234),
            ..AppConfig::default()
        };
        params().write_to_config(&mut config);
        assert!(config.auto_refresh_enabled);
        assert_eq!(config.auto_refresh_interval_minutes, 42);
        assert_eq!(config.window_x, Some(7));
        assert_eq!(config.window_width, Some(1234));
    }

    // --- quantization ---

    #[test]
    fn digest_quantizes_to_thousandths() {
        let d = params().digest();
        assert_eq!(d.terminator_width, 150);
        assert_eq!(d.diffuse_floor, 100);
        assert_eq!(d.diffuse_ramp, 600);
        assert_eq!(d.fresnel_mix, 500);
        assert_eq!(d.cloud_opacity, 750);
        assert_eq!(d.cloud_floor, 196);
        assert_eq!(d.cloud_gamma, 300);
        assert_eq!(d.day_gamma, 1500);
        assert_eq!(d.day_saturation, 500);
        assert_eq!(d.rayleigh_intensity, 500);
        assert_eq!(d.rayleigh_sharpness, 50_000);
        assert_eq!(d.nightglow_intensity, 250);
        assert_eq!(d.nightglow_falloff, 15_000);
        assert_eq!(d.nightglow_balance, 370);
    }

    #[test]
    fn sub_threshold_change_compares_equal() {
        let a = SceneParams {
            terminator_width: 0.150_1,
            ..params()
        };
        let b = SceneParams {
            terminator_width: 0.150_4,
            ..params()
        };
        assert_eq!(a.digest(), b.digest());
    }

    #[test]
    fn at_threshold_change_compares_different() {
        let a = SceneParams {
            terminator_width: 0.150,
            ..params()
        };
        let b = SceneParams {
            terminator_width: 0.151,
            ..params()
        };
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn direction_quantization_uses_milliradians() {
        let q = quantize_direction(glam::Vec3::new(0.1234, -0.5678, 0.9012));
        assert_eq!(q, [123, -567, 901]);
    }

    /// Every field that reaches the shader must make the digest differ.
    /// Written as a table so adding a parameter without wiring the dirty check
    /// is a test failure rather than a stale-frame bug.
    /// The length is the table, one row per shader parameter.
    #[allow(clippy::too_many_lines)]
    #[test]
    fn every_shader_parameter_triggers_dirty() {
        let base = params();
        let mutations: Vec<(&str, SceneParams)> = vec![
            (
                "longitude",
                SceneParams {
                    camera: CameraParams {
                        longitude: 11.0,
                        ..base.camera
                    },
                    ..base
                },
            ),
            (
                "latitude",
                SceneParams {
                    camera: CameraParams {
                        latitude: 21.0,
                        ..base.camera
                    },
                    ..base
                },
            ),
            (
                "zoom",
                SceneParams {
                    camera: CameraParams {
                        zoom: 0.4,
                        ..base.camera
                    },
                    ..base
                },
            ),
            (
                "offset_x",
                SceneParams {
                    camera: CameraParams {
                        offset_x: 0.5,
                        ..base.camera
                    },
                    ..base
                },
            ),
            (
                "offset_y",
                SceneParams {
                    camera: CameraParams {
                        offset_y: 0.5,
                        ..base.camera
                    },
                    ..base
                },
            ),
            (
                "tilt",
                SceneParams {
                    camera: CameraParams {
                        tilt_deg: 45.0,
                        ..base.camera
                    },
                    ..base
                },
            ),
            (
                "yaw",
                SceneParams {
                    camera: CameraParams {
                        yaw_deg: 30.0,
                        ..base.camera
                    },
                    ..base
                },
            ),
            (
                "pitch",
                SceneParams {
                    camera: CameraParams {
                        pitch_deg: 30.0,
                        ..base.camera
                    },
                    ..base
                },
            ),
            (
                "texture_index",
                SceneParams {
                    texture_index: 1,
                    ..base
                },
            ),
            (
                "sample_count",
                SceneParams {
                    sample_count: 2,
                    ..base
                },
            ),
            (
                "terminator_width",
                SceneParams {
                    terminator_width: 0.25,
                    ..base
                },
            ),
            (
                "diffuse_shading",
                SceneParams {
                    diffuse_shading: !base.diffuse_shading,
                    ..base
                },
            ),
            (
                "diffuse_floor",
                SceneParams {
                    diffuse_floor: 0.2,
                    ..base
                },
            ),
            (
                "diffuse_ramp",
                SceneParams {
                    diffuse_ramp: 0.7,
                    ..base
                },
            ),
            (
                "spec_shininess",
                SceneParams {
                    spec_shininess: 200.0,
                    ..base
                },
            ),
            (
                "spec_intensity",
                SceneParams {
                    spec_intensity: 0.5,
                    ..base
                },
            ),
            (
                "fresnel_mix",
                SceneParams {
                    fresnel_mix: 0.6,
                    ..base
                },
            ),
            (
                "fresnel_exp",
                SceneParams {
                    fresnel_exp: 6.0,
                    ..base
                },
            ),
            (
                "cloud_opacity",
                SceneParams {
                    cloud_opacity: 0.5,
                    ..base
                },
            ),
            (
                "cloud_floor",
                SceneParams {
                    cloud_floor: 0.3,
                    ..base
                },
            ),
            (
                "cloud_gamma",
                SceneParams {
                    cloud_gamma: 0.5,
                    ..base
                },
            ),
            (
                "atmo_enabled",
                SceneParams {
                    atmo_enabled: !base.atmo_enabled,
                    ..base
                },
            ),
            (
                "rayleigh_intensity",
                SceneParams {
                    rayleigh_intensity: 0.9,
                    ..base
                },
            ),
            (
                "rayleigh_sharpness",
                SceneParams {
                    rayleigh_sharpness: 60.0,
                    ..base
                },
            ),
            (
                "rayleigh_haze",
                SceneParams {
                    rayleigh_haze: 0.9,
                    ..base
                },
            ),
            (
                "nightglow_intensity",
                SceneParams {
                    nightglow_intensity: 0.5,
                    ..base
                },
            ),
            (
                "nightglow_falloff",
                SceneParams {
                    nightglow_falloff: 6.0,
                    ..base
                },
            ),
            (
                "nightglow_balance",
                SceneParams {
                    nightglow_balance: 0.8,
                    ..base
                },
            ),
            (
                "star_intensity",
                SceneParams {
                    star_intensity: 0.8,
                    ..base
                },
            ),
            (
                "star_size",
                SceneParams {
                    star_size: 1.5,
                    ..base
                },
            ),
            (
                "star_glow_strength",
                SceneParams {
                    star_glow_strength: 0.7,
                    ..base
                },
            ),
            (
                "star_glow_radius",
                SceneParams {
                    star_glow_radius: 8.0,
                    ..base
                },
            ),
            (
                "star_contrast",
                SceneParams {
                    star_contrast: -0.8,
                    ..base
                },
            ),
            (
                "star_mag_limit",
                SceneParams {
                    star_mag_limit: 5.5,
                    ..base
                },
            ),
            (
                "day_gamma",
                SceneParams {
                    day_gamma: 1.8,
                    ..base
                },
            ),
            (
                "day_saturation",
                SceneParams {
                    day_saturation: 0.8,
                    ..base
                },
            ),
            (
                "night_gamma",
                SceneParams {
                    night_gamma: 1.5,
                    ..base
                },
            ),
            (
                "night_saturation",
                SceneParams {
                    night_saturation: 0.5,
                    ..base
                },
            ),
        ];
        for (name, mutated) in mutations {
            assert_ne!(
                base.digest(),
                mutated.digest(),
                "{name} must trigger a redraw"
            );
        }
    }

    #[test]
    fn disabling_the_atmosphere_zeroes_both_shells() {
        let off = SceneParams {
            atmo_enabled: false,
            ..params()
        };
        assert_relative_eq!(off.effective_rayleigh_intensity(), 0.0);
        assert_relative_eq!(off.effective_nightglow_intensity(), 0.0);
        assert_eq!(off.digest().rayleigh_intensity, 0);
        assert_eq!(off.digest().nightglow_intensity, 0);
    }

    #[test]
    fn datetime_is_not_part_of_the_digest() {
        let a = params();
        let b = SceneParams {
            datetime: DateTimeInput {
                custom_hour: 3.0,
                ..a.datetime
            },
            ..a
        };
        assert_ne!(a, b, "the params themselves differ");
        assert_eq!(
            a.digest(),
            b.digest(),
            "the digest tracks the derived sun direction instead"
        );
    }

    // --- gamma slider mapping ---

    #[test]
    fn gamma_slider_endpoints() {
        assert_relative_eq!(gamma_slider_to_value(0.0), 0.2, epsilon = 1e-5);
        assert_relative_eq!(gamma_slider_to_value(1.0), 3.0, epsilon = 1e-5);
    }

    #[test]
    fn gamma_slider_midpoint_is_identity() {
        assert_relative_eq!(gamma_slider_to_value(0.5), 1.0, epsilon = 1e-5);
    }

    #[test]
    fn gamma_slider_monotonic() {
        #[allow(clippy::cast_precision_loss)]
        let values: Vec<f32> = (0..=10)
            .map(|i| gamma_slider_to_value(i as f32 / 10.0))
            .collect();
        for pair in values.windows(2) {
            assert!(
                pair[1] > pair[0],
                "expected {:.3} > {:.3}",
                pair[1],
                pair[0]
            );
        }
    }

    #[test]
    fn gamma_roundtrip() {
        for gamma in [0.2, 0.5, 1.0, 2.0, 3.0] {
            let t = gamma_value_to_slider(gamma);
            assert_relative_eq!(gamma_slider_to_value(t), gamma, epsilon = 1e-5);
        }
    }

    #[test]
    fn gamma_inverse_midpoint() {
        assert_relative_eq!(gamma_value_to_slider(1.0), 0.5, epsilon = 1e-5);
    }
}
