//! The single scene parameter struct.
//!
//! `SceneParams` is the one description of "what to draw" that flows through
//! every layer, so there are exactly two translation points for it:
//! `ui_callbacks::read_params_from_window` and `apply_params_to_window` in
//! `sunlit-app`, and `renderer::render_pass::write_uniforms` here.
//!
//! `docs/rendering.md` lists every file a new parameter touches.

use crate::color::Rgb8;
use crate::config::AppConfig;
use crate::scene::camera::{CameraParams, held_longitude, longitude_of};
use crate::scene::scene_time::SceneTime;
use crate::scene::sky::SkyState;

/// Radius of the cloud shell, just above the surface.
pub(crate) const CLOUD_SPHERE_RADIUS: f32 = 1.0015;
/// Half-width of the cloud layer's own terminator ramp, in units of n dot l.
///
/// Wider than the globe's 0.1: the shell's tangent condition already shifts the
/// ramp 3.14 degrees nightward, and the cloud tops that have lost the direct
/// beam are still lit by a twilight sky for several degrees after that. A
/// constant rather than a slider, because nobody wants to tune two terminators.
pub const CLOUD_TERMINATOR_WIDTH: f32 = 0.18;
/// Radius of the Rayleigh scattering shell.
pub(crate) const RAYLEIGH_RADIUS: f32 = 1.015;
/// Radius of the orange (sodium D + iron oxide) nightglow shell.
pub(crate) const NIGHTGLOW_ORANGE_RADIUS: f32 = 1.014;
/// Radius of the green (OI 557.7nm) nightglow shell.
pub(crate) const NIGHTGLOW_GREEN_RADIUS: f32 = 1.015;

const GAMMA_MIN: f32 = 0.2;
const GAMMA_MAX: f32 = 3.0;

/// Everything that determines the rendered image, except the frame's
/// resolution and sky state (both derived: resolution from the target, sky
/// state from the clock plus `time`).
#[derive(Clone, Copy, Debug, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the switches of the settings window, not a parameter list"
)]
pub struct SceneParams {
    /// The camera as the settings hold it. In the Sun-fixed mode its longitude
    /// is not read; [`Self::camera_at`] is the camera a frame is drawn with.
    pub camera: CameraParams,
    /// Whether the camera keeps its place relative to the Sun rather than over
    /// one place on the Earth, so the globe turns beneath it once a day.
    pub sun_fixed: bool,
    /// The local apparent solar time beneath the camera in the Sun-fixed mode,
    /// in hours from 0 to 24, which stands in for the longitude there.
    pub solar_time: f32,

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
    /// Whether the cloud layer is shown. Off, it is not drawn and no new
    /// download or decode starts: a transfer already under way when it goes off
    /// is not interrupted, and what it brings back is dropped undecoded.
    pub clouds_enabled: bool,
    /// Opacity of a cloud on the day hemisphere, a straight multiplier on the
    /// source's own coverage.
    pub cloud_opacity: f32,
    /// Opacity of a cloud on the night hemisphere, read as an optical depth
    /// rather than as a multiplier, which is what lets the top of its range
    /// cover the ground completely. See `fs_cloud` for why the two hemispheres
    /// cannot share one number.
    pub cloud_opacity_night: f32,
    /// How much of the map's clear-sky background is taken off as a local
    /// floor, 0 to 2, equatorward and poleward of `cloud_haze_latitude`, in
    /// degrees, across a band `cloud_haze_band` degrees wide. Up to 1 it takes
    /// that share of the estimate up to its cap; above 1 it lifts the cap
    /// toward full brightness, which reaches 1 at 2. See `fs_cloud`.
    pub cloud_haze_equator: f32,
    pub cloud_haze_pole: f32,
    pub cloud_haze_latitude: f32,
    pub cloud_haze_band: f32,
    /// The floor and the gamma applied after the haze removal over land.
    /// `fs_cloud` blends each with its ocean twin by the water mask.
    pub cloud_floor_land: f32,
    pub cloud_gamma_land: f32,
    /// The same two over open water.
    pub cloud_floor_ocean: f32,
    pub cloud_gamma_ocean: f32,
    /// Brightness of a cloud on the night hemisphere, as a fraction of display
    /// white. Not an irradiance: the night side is about 18.6 stops under the
    /// day side and this frame has one exposure for both, so the value is a
    /// choice about how much of that gap to compress into 8 bits.
    pub cloud_night: f32,

    // Atmosphere
    pub atmo_enabled: bool,
    pub rayleigh_intensity: f32,
    pub rayleigh_sharpness: f32,
    pub rayleigh_haze: f32,
    pub nightglow_intensity: f32,
    pub nightglow_falloff: f32,
    pub nightglow_balance: f32,
    pub atmo_sunrise_glow: f32,
    pub atmo_sunrise_width: f32,

    // Celestial background
    pub sky_fov: f32,
    pub star_intensity: f32,
    pub star_size: f32,
    pub star_glow_strength: f32,
    pub star_glow_radius: f32,
    pub star_contrast: f32,
    pub star_mag_limit: f32,

    // The Sun as a visible object
    pub sun_glow: f32,
    pub sun_rays: f32,
    pub sun_flare: f32,
    pub sun_size: f32,
    pub sun_halo_radius: f32,
    pub sun_horizon_boost: f32,
    pub sun_horizon_reach: f32,
    pub sun_horizon_depth: f32,
    pub sun_reddening: f32,
    pub sun_refraction: f32,

    // The Moon
    pub moon_brightness: f32,
    pub moon_size: f32,
    pub moon_earthshine: f32,

    // The Milky Way
    pub milky_way_intensity: f32,

    // Color correction (gamma values, not slider positions)
    /// The color open water takes on the day map and on the night map, before
    /// gamma and saturation. The renderer moves the water's share of each
    /// texel from the color its pack carries to this one, by the water mask.
    pub day_ocean_color: Rgb8,
    pub night_ocean_color: Rgb8,
    pub day_gamma: f32,
    pub day_saturation: f32,
    pub night_gamma: f32,
    pub night_saturation: f32,

    /// The time setting: live time, or one instant. The wallpaper and every
    /// export draw at it; the preview leaves it only during an excursion.
    pub time: SceneTime,
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
                fov_deg: config.camera_fov,
            },
            sun_fixed: config.sun_fixed,
            solar_time: config.solar_time,
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
            clouds_enabled: config.clouds_enabled,
            cloud_opacity: config.cloud_opacity,
            cloud_opacity_night: config.cloud_opacity_night,
            cloud_haze_equator: config.cloud_haze_equator,
            cloud_haze_pole: config.cloud_haze_pole,
            cloud_haze_latitude: config.cloud_haze_latitude,
            cloud_haze_band: config.cloud_haze_band,
            cloud_floor_land: config.cloud_floor_land,
            cloud_gamma_land: config.cloud_gamma_land,
            cloud_floor_ocean: config.cloud_floor_ocean,
            cloud_gamma_ocean: config.cloud_gamma_ocean,
            cloud_night: config.cloud_night,
            atmo_enabled: config.atmo_enabled,
            rayleigh_intensity: config.rayleigh_intensity,
            rayleigh_sharpness: config.rayleigh_sharpness,
            rayleigh_haze: config.rayleigh_haze,
            nightglow_intensity: config.nightglow_intensity,
            nightglow_falloff: config.nightglow_falloff,
            nightglow_balance: config.nightglow_balance,
            atmo_sunrise_glow: config.atmo_sunrise_glow,
            atmo_sunrise_width: config.atmo_sunrise_width,
            sky_fov: config.sky_fov,
            star_intensity: config.star_intensity,
            star_size: config.star_size,
            star_glow_strength: config.star_glow_strength,
            star_glow_radius: config.star_glow_radius,
            star_contrast: config.star_contrast,
            star_mag_limit: config.star_mag_limit,
            sun_glow: config.sun_glow,
            sun_rays: config.sun_rays,
            sun_flare: config.sun_flare,
            sun_size: config.sun_size,
            sun_halo_radius: config.sun_halo_radius,
            sun_horizon_boost: config.sun_horizon_boost,
            sun_horizon_reach: config.sun_horizon_reach,
            sun_horizon_depth: config.sun_horizon_depth,
            sun_reddening: config.sun_reddening,
            sun_refraction: config.sun_refraction,
            moon_brightness: config.moon_brightness,
            moon_size: config.moon_size,
            moon_earthshine: config.moon_earthshine,
            milky_way_intensity: config.milky_way_intensity,
            day_ocean_color: config.day_ocean_color,
            night_ocean_color: config.night_ocean_color,
            day_gamma: config.day_gamma,
            day_saturation: config.day_saturation,
            night_gamma: config.night_gamma,
            night_saturation: config.night_saturation,
            time: config.scene_time,
        }
    }

    /// Write the scene half back into a config, leaving window geometry and the
    /// auto-refresh settings (which are not scene parameters) untouched.
    pub fn write_to_config(&self, config: &mut AppConfig) {
        config.longitude = self.camera.longitude;
        config.latitude = self.camera.latitude;
        config.zoom = self.camera.zoom;
        config.offset_x = self.camera.offset_x;
        config.offset_y = self.camera.offset_y;
        config.tilt = self.camera.tilt_deg;
        config.yaw = self.camera.yaw_deg;
        config.pitch = self.camera.pitch_deg;
        config.camera_fov = self.camera.fov_deg;
        config.sun_fixed = self.sun_fixed;
        config.solar_time = self.solar_time;
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
        config.clouds_enabled = self.clouds_enabled;
        config.cloud_opacity = self.cloud_opacity;
        config.cloud_opacity_night = self.cloud_opacity_night;
        config.cloud_haze_equator = self.cloud_haze_equator;
        config.cloud_haze_pole = self.cloud_haze_pole;
        config.cloud_haze_latitude = self.cloud_haze_latitude;
        config.cloud_haze_band = self.cloud_haze_band;
        config.cloud_floor_land = self.cloud_floor_land;
        config.cloud_gamma_land = self.cloud_gamma_land;
        config.cloud_floor_ocean = self.cloud_floor_ocean;
        config.cloud_gamma_ocean = self.cloud_gamma_ocean;
        config.cloud_night = self.cloud_night;
        config.atmo_enabled = self.atmo_enabled;
        config.rayleigh_intensity = self.rayleigh_intensity;
        config.rayleigh_sharpness = self.rayleigh_sharpness;
        config.rayleigh_haze = self.rayleigh_haze;
        config.nightglow_intensity = self.nightglow_intensity;
        config.nightglow_falloff = self.nightglow_falloff;
        config.nightglow_balance = self.nightglow_balance;
        config.atmo_sunrise_glow = self.atmo_sunrise_glow;
        config.atmo_sunrise_width = self.atmo_sunrise_width;
        config.sky_fov = self.sky_fov;
        config.star_intensity = self.star_intensity;
        config.star_size = self.star_size;
        config.star_glow_strength = self.star_glow_strength;
        config.star_glow_radius = self.star_glow_radius;
        config.star_contrast = self.star_contrast;
        config.star_mag_limit = self.star_mag_limit;
        config.sun_glow = self.sun_glow;
        config.sun_rays = self.sun_rays;
        config.sun_flare = self.sun_flare;
        config.sun_size = self.sun_size;
        config.sun_halo_radius = self.sun_halo_radius;
        config.sun_horizon_boost = self.sun_horizon_boost;
        config.sun_horizon_reach = self.sun_horizon_reach;
        config.sun_horizon_depth = self.sun_horizon_depth;
        config.sun_reddening = self.sun_reddening;
        config.sun_refraction = self.sun_refraction;
        config.moon_brightness = self.moon_brightness;
        config.moon_size = self.moon_size;
        config.moon_earthshine = self.moon_earthshine;
        config.milky_way_intensity = self.milky_way_intensity;
        config.day_ocean_color = self.day_ocean_color;
        config.night_ocean_color = self.night_ocean_color;
        config.day_gamma = self.day_gamma;
        config.day_saturation = self.day_saturation;
        config.night_gamma = self.night_gamma;
        config.night_saturation = self.night_saturation;
        config.scene_time = self.time;
    }

    /// The camera in the Earth-fixed world frame, at the instant of `sky`.
    ///
    /// The settings' camera itself in the Earth-fixed mode. In the Sun-fixed
    /// mode the longitude is the one the time of day puts beneath the camera
    /// with the Sun where `sky` has it, so every path that draws or measures a
    /// frame resolves its camera here, under the sky that frame is drawn with.
    pub fn camera_at(&self, sky: &SkyState) -> CameraParams {
        if self.sun_fixed {
            CameraParams {
                longitude: longitude_of(self.solar_time, sky.sun_direction),
                ..self.camera
            }
        } else {
            self.camera
        }
    }

    /// The camera as the settings hold it: in the Sun-fixed mode, with the
    /// held longitude the time of day stands for, so that what moves it is a
    /// hand on the settings and never the Earth's turn.
    pub(crate) fn held_camera(&self) -> CameraParams {
        if self.sun_fixed {
            CameraParams {
                longitude: held_longitude(self.solar_time),
                ..self.camera
            }
        } else {
            self.camera
        }
    }

    /// Whether the cloud layer draws at all: switched on, with either
    /// hemisphere's opacity up. Each hemisphere has its own opacity, so one of
    /// them at zero is not the layer switched off.
    pub fn draws_clouds(&self) -> bool {
        self.clouds_enabled && (self.cloud_opacity > 0.0 || self.cloud_opacity_night > 0.0)
    }

    /// Rayleigh intensity after the atmosphere master switch. Zero suppresses
    /// the draw call entirely.
    pub(crate) fn effective_rayleigh_intensity(&self) -> f32 {
        if self.atmo_enabled {
            self.rayleigh_intensity
        } else {
            0.0
        }
    }

    /// Nightglow intensity after the atmosphere master switch.
    pub(crate) fn effective_nightglow_intensity(&self) -> f32 {
        if self.atmo_enabled {
            self.nightglow_intensity
        } else {
            0.0
        }
    }
}

/// Quantize a float to integer thousandths.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a float-to-integer cast saturates, so a parameter outside the range still compares stably"
)]
fn q(value: f32) -> i32 {
    (value * 1000.0) as i32
}

/// Quantize a direction vector to integer milliradians for stable comparison.
pub(crate) fn quantize_direction(dir: glam::Vec3) -> [i32; 3] {
    [q(dir.x), q(dir.y), q(dir.z)]
}

/// Declares the dirty-check half of `SceneParams` once.
///
/// `ParamsDigest`, `digest()` and the mutation table the tests walk are all
/// generated from the one list below, so a parameter cannot be in the digest
/// and missing from the table, or the other way round. The `const _` in the
/// expansion is what makes the list complete: it builds a `SceneParams` and a
/// `CameraParams` out of the groups, so a field in either struct and in no
/// group here is a missing field the compiler names.
///
/// The groups say how a parameter is compared, which is the only thing the
/// digest needs to know about it:
///
/// - `camera`: the sub-fields of `CameraParams`, which the digest copies whole,
///   as `held_camera` holds them, and compares exactly.
/// - `counted`, `switched`: integers and switches, compared exactly.
/// - `quantized`: floats rounded to integer thousandths.
/// - `colored`: colors of 8 bit channels, compared exactly.
/// - `gated`: floats the digest reads through an accessor rather than from the
///   field, because another parameter can zero them.
/// - `indirect`: not a digest field, but a change to it moves one, so it still
///   needs a row in the table.
/// - `absent`: neither, and the comment on the group says why.
macro_rules! scene_digest {
    (
        camera { $($cam:ident),* $(,)? }
        counted { $($cnt:ident: $cnt_ty:ty),* $(,)? }
        switched { $($sw:ident),* $(,)? }
        quantized { $($qn:ident),* $(,)? }
        colored { $($col:ident),* $(,)? }
        gated { $($gt:ident from $src:ident),* $(,)? }
        indirect { $($ind:ident),* $(,)? }
        absent { $($abs:ident),* $(,)? }
    ) => {
        /// Dirty-check snapshot of a `SceneParams`.
        ///
        /// `time` is deliberately absent: the derived sky state is compared
        /// separately, so a live render changes even though `time` does not.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub(crate) struct ParamsDigest {
            pub camera: CameraParams,
            $(pub $cnt: $cnt_ty,)*
            $(pub $sw: bool,)*
            $(pub $qn: i32,)*
            $(pub $col: Rgb8,)*
            $(pub $gt: i32,)*
        }

        impl SceneParams {
            /// Quantized snapshot used for dirty checking.
            ///
            /// Camera values compare exactly; every other float is rounded to
            /// integer thousandths so that sub-visible slider jitter does not
            /// force a redraw. The camera is the held one, so the longitude
            /// the Sun-fixed mode does not read cannot move the digest.
            pub(crate) fn digest(&self) -> ParamsDigest {
                ParamsDigest {
                    camera: self.held_camera(),
                    $($cnt: self.$cnt,)*
                    $($sw: self.$sw,)*
                    $($qn: q(self.$qn),)*
                    $($col: self.$col,)*
                    $($gt: q(self.$src()),)*
                }
            }
        }

        /// Compile-time proof that the list above classifies every field of
        /// `SceneParams` and of `CameraParams`: the initializers name them all,
        /// so a parameter that is in one of those structs and in no group is a
        /// missing field here, reported under its own name.
        const _: fn(&SceneParams) -> SceneParams = |p| SceneParams {
            camera: CameraParams { $($cam: p.camera.$cam,)* },
            $($cnt: p.$cnt,)*
            $($sw: p.$sw,)*
            $($qn: p.$qn,)*
            $($col: p.$col,)*
            $($gt: p.$gt,)*
            $($ind: p.$ind,)*
            $($abs: p.$abs,)*
        };

        /// One row per parameter: its name, and a `SceneParams` that differs
        /// from `base` in that parameter and nothing else.
        #[cfg(test)]
        fn digest_mutations(base: SceneParams) -> Vec<(&'static str, SceneParams)> {
            vec![
                $((
                    stringify!($cam),
                    SceneParams {
                        camera: CameraParams {
                            $cam: base.camera.$cam + 1.0,
                            ..base.camera
                        },
                        ..base
                    },
                ),)*
                $((
                    stringify!($cnt),
                    SceneParams {
                        $cnt: base.$cnt + 1,
                        ..base
                    },
                ),)*
                $((
                    stringify!($sw),
                    SceneParams {
                        $sw: !base.$sw,
                        ..base
                    },
                ),)*
                $((
                    stringify!($qn),
                    SceneParams {
                        $qn: base.$qn + 1.0,
                        ..base
                    },
                ),)*
                $((
                    stringify!($col),
                    SceneParams {
                        $col: Rgb8 {
                            r: base.$col.r.wrapping_add(1),
                            ..base.$col
                        },
                        ..base
                    },
                ),)*
                $((
                    stringify!($gt),
                    SceneParams {
                        $gt: base.$gt + 1.0,
                        ..base
                    },
                ),)*
                $((
                    stringify!($ind),
                    SceneParams {
                        $ind: !base.$ind,
                        ..base
                    },
                ),)*
            ]
        }
    };
}

scene_digest! {
    camera {
        longitude,
        latitude,
        zoom,
        offset_x,
        offset_y,
        tilt_deg,
        yaw_deg,
        pitch_deg,
        fov_deg,
    }
    counted {
        texture_index: i32,
        sample_count: u32,
    }
    switched {
        sun_fixed,
        diffuse_shading,
        clouds_enabled,
    }
    quantized {
        terminator_width,
        diffuse_floor,
        diffuse_ramp,
        spec_shininess,
        spec_intensity,
        fresnel_mix,
        fresnel_exp,
        cloud_opacity,
        cloud_opacity_night,
        cloud_haze_equator,
        cloud_haze_pole,
        cloud_haze_latitude,
        cloud_haze_band,
        cloud_floor_land,
        cloud_gamma_land,
        cloud_floor_ocean,
        cloud_gamma_ocean,
        cloud_night,
        rayleigh_sharpness,
        rayleigh_haze,
        nightglow_falloff,
        nightglow_balance,
        atmo_sunrise_glow,
        atmo_sunrise_width,
        sky_fov,
        star_intensity,
        star_size,
        star_glow_strength,
        star_glow_radius,
        star_contrast,
        star_mag_limit,
        sun_glow,
        sun_rays,
        sun_flare,
        sun_size,
        sun_halo_radius,
        sun_horizon_boost,
        sun_horizon_reach,
        sun_horizon_depth,
        sun_reddening,
        sun_refraction,
        moon_brightness,
        moon_size,
        moon_earthshine,
        milky_way_intensity,
        day_gamma,
        day_saturation,
        night_gamma,
        night_saturation,
    }
    colored {
        day_ocean_color,
        night_ocean_color,
    }
    gated {
        rayleigh_intensity from effective_rayleigh_intensity,
        nightglow_intensity from effective_nightglow_intensity,
    }
    indirect {
        // Zeroes both gated intensities rather than reaching a digest field of
        // its own.
        atmo_enabled,
    }
    absent {
        // The instant to render for. The digest compares the sun direction it
        // derives instead, so a live render changes while `time` does not.
        time,
        // The time of day, which places the camera only together with the
        // instant, so the frame key compares the camera it resolves for the
        // reason `time` is absent. In the Sun-fixed mode it also reaches
        // the held camera the digest copies; the Earth-fixed mode does not read
        // it at all, which is why the table cannot have a row for it.
        solar_time,
    }
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
            cloud_floor_land: 0.196,
            cloud_gamma_land: 0.3,
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

    /// The display plan is a setting, not a shader parameter.
    #[test]
    fn the_display_plan_is_not_a_shader_parameter() {
        let mut config = crate::config::AppConfig::default();
        let before = SceneParams::from_config(&config);
        config.display_mode = crate::display::layout::DisplayMode::AcrossScreens;
        config.anchor_monitor = "DP-2".to_owned();
        let after = SceneParams::from_config(&config);
        assert_eq!(before.digest(), after.digest());
        assert_eq!(before, after, "the plan reached SceneParams");
    }

    #[test]
    fn digest_quantizes_to_thousandths() {
        let d = params().digest();
        assert_eq!(d.terminator_width, 150);
        assert_eq!(d.diffuse_floor, 100);
        assert_eq!(d.diffuse_ramp, 600);
        assert_eq!(d.fresnel_mix, 500);
        assert_eq!(d.cloud_opacity, 750);
        assert_eq!(d.cloud_floor_land, 196);
        assert_eq!(d.cloud_gamma_land, 300);
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

    /// The switch gates the draw whichever hemisphere's opacity is up, and with
    /// it on either one alone draws the layer.
    #[test]
    fn the_cloud_switch_gates_the_draw_whatever_the_opacities() {
        for (day, night) in [(0.5, 0.0), (0.0, 0.5), (0.5, 0.5)] {
            let on = SceneParams {
                clouds_enabled: true,
                cloud_opacity: day,
                cloud_opacity_night: night,
                ..params()
            };
            assert!(on.draws_clouds(), "day {day}, night {night}");
            let off = SceneParams {
                clouds_enabled: false,
                ..on
            };
            assert!(!off.draws_clouds(), "day {day}, night {night}");
        }
        let neither = SceneParams {
            clouds_enabled: true,
            cloud_opacity: 0.0,
            cloud_opacity_night: 0.0,
            ..params()
        };
        assert!(!neither.draws_clouds());
    }

    #[test]
    fn direction_quantization_uses_milliradians() {
        let q = quantize_direction(glam::Vec3::new(0.1234, -0.5678, 0.9012));
        assert_eq!(q, [123, -567, 901]);
    }

    /// Every field that reaches the shader must make the digest differ.
    ///
    /// The rows come from the `scene_digest!` list, so a parameter declared
    /// there is exercised here without a second edit, and a parameter added to
    /// `SceneParams` and to no group there does not compile.
    #[test]
    fn every_shader_parameter_triggers_dirty() {
        let base = params();
        assert!(
            base.atmo_enabled,
            "the gated rows only move the digest with the atmosphere switched on"
        );
        for (name, mutated) in digest_mutations(base) {
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
    fn the_time_is_not_part_of_the_digest() {
        let a = params();
        let b = SceneParams {
            time: SceneTime::fixed(time::macros::datetime!(2026-01-01 03:00 UTC)),
            ..a
        };
        assert_ne!(a, b, "the params themselves differ");
        assert_eq!(
            a.digest(),
            b.digest(),
            "the digest tracks the derived sun direction instead"
        );
    }

    // --- the Sun-fixed camera ---

    fn sky_at(hour: i32, minute: i32) -> SkyState {
        crate::scene::sky::compute_sky_state_from_time(crate::scene::sun::make_time(
            2026, 10, 16, hour, minute, 0.0,
        ))
    }

    fn sun_fixed_at(solar_time: f32) -> SceneParams {
        SceneParams {
            sun_fixed: true,
            solar_time,
            ..params()
        }
    }

    /// The signed hour angle of the Sun seen from the camera's meridian, in
    /// degrees: how far east of the eye the Sun stands about the pole.
    fn sun_east_of_the_eye(camera: &CameraParams, sky: &SkyState) -> f32 {
        let eye = crate::scene::camera::OrbitalCamera::from_params(camera).eye_position();
        let sun = sky.sun_direction;
        let turned = (sun.x.atan2(sun.z) - eye.x.atan2(eye.z)).to_degrees();
        (turned + 180.0).rem_euclid(360.0) - 180.0
    }

    /// Hours apart on one day, the Sun stands at one hour angle from the eye:
    /// the camera kept its place against the Sun while the Earth turned.
    #[test]
    fn the_sun_keeps_its_hour_angle_from_the_eye_through_a_day() {
        let params = sun_fixed_at(15.0);
        let angles: Vec<f32> = [(4, 0), (10, 0), (16, 0)]
            .into_iter()
            .map(|(hour, minute)| {
                let sky = sky_at(hour, minute);
                sun_east_of_the_eye(&params.camera_at(&sky), &sky)
            })
            .collect();
        for angle in &angles {
            assert_relative_eq!(*angle, angles[0], epsilon = 0.01);
        }
        // Three in the afternoon puts the Sun three hours west of the eye.
        assert_relative_eq!(angles[0], -45.0, epsilon = 0.01);
    }

    /// The Sun crosses the sky westward at 15 degrees an hour, and the camera
    /// follows it.
    #[test]
    fn an_hour_later_the_camera_hangs_fifteen_degrees_further_west() {
        let params = sun_fixed_at(9.5);
        let before = params.camera_at(&sky_at(7, 20)).longitude;
        let after = params.camera_at(&sky_at(8, 20)).longitude;
        let moved = (after - before + 180.0).rem_euclid(360.0) - 180.0;
        assert_relative_eq!(moved, -15.0, epsilon = 0.05);
    }

    /// The Earth-fixed mode draws exactly the camera the settings hold, under
    /// any sky, which is what keeps every existing frame bit for bit.
    #[test]
    fn the_earth_fixed_mode_resolves_to_the_camera_itself() {
        let params = SceneParams {
            sun_fixed: false,
            solar_time: 3.0,
            ..params()
        };
        for (hour, minute) in [(0, 0), (6, 45), (13, 10), (23, 59)] {
            assert_eq!(params.camera_at(&sky_at(hour, minute)), params.camera);
        }
        assert_eq!(params.held_camera(), params.camera);
    }

    /// The mode only ever reads one of the two keys, so the other one cannot
    /// move the digest.
    #[test]
    fn the_digest_follows_the_key_the_mode_reads() {
        let earth = params();
        let moved_longitude = |p: SceneParams| SceneParams {
            camera: CameraParams {
                longitude: p.camera.longitude + 20.0,
                ..p.camera
            },
            ..p
        };
        let moved_time = |p: SceneParams| SceneParams {
            solar_time: p.solar_time + 1.5,
            ..p
        };
        assert_ne!(earth.digest(), moved_longitude(earth).digest());
        assert_eq!(earth.digest(), moved_time(earth).digest());

        let sun = sun_fixed_at(10.0);
        assert_eq!(sun.digest(), moved_longitude(sun).digest());
        assert_ne!(sun.digest(), moved_time(sun).digest());
    }

    /// Both keys travel through a config and back.
    #[test]
    fn the_camera_mode_and_its_time_of_day_round_trip_through_a_config() {
        let mut config = AppConfig::default();
        let original = sun_fixed_at(20.75);
        original.write_to_config(&mut config);
        assert!(config.sun_fixed);
        assert_relative_eq!(config.solar_time, 20.75);
        assert_eq!(SceneParams::from_config(&config), original);
    }

    /// A config written before the mode existed draws what it always drew.
    #[test]
    fn a_config_without_the_new_keys_draws_the_earth_fixed_camera() {
        let config: AppConfig = toml::from_str("longitude = 42.0\nlatitude = -7.0").unwrap();
        let params = SceneParams::from_config(&config);
        for (hour, minute) in [(2, 0), (14, 30)] {
            let camera = params.camera_at(&sky_at(hour, minute));
            assert_relative_eq!(camera.longitude, 42.0);
            assert_relative_eq!(camera.latitude, -7.0);
        }
    }

    /// Switched on with the time of day a fresh config holds, the camera looks
    /// at the sunlit side.
    #[test]
    fn the_default_time_of_day_looks_at_the_day_side() {
        let params = SceneParams {
            sun_fixed: true,
            ..SceneParams::default()
        };
        for (hour, minute) in [(1, 0), (9, 0), (17, 0)] {
            let sky = sky_at(hour, minute);
            let eye = crate::scene::camera::OrbitalCamera::from_params(&params.camera_at(&sky))
                .eye_position();
            assert!(
                eye.normalize().dot(sky.sun_direction) > 0.5,
                "{hour}:{minute:02}"
            );
        }
    }

    // --- gamma slider mapping ---

    /// The slider's three anchors, against the range's own constants rather
    /// than the numbers they hold today.
    #[test]
    fn the_gamma_slider_spans_its_range_around_the_identity() {
        assert_relative_eq!(gamma_slider_to_value(0.0), GAMMA_MIN, epsilon = 1e-5);
        assert_relative_eq!(gamma_slider_to_value(0.5), 1.0, epsilon = 1e-5);
        assert_relative_eq!(gamma_slider_to_value(1.0), GAMMA_MAX, epsilon = 1e-5);
        assert_relative_eq!(gamma_value_to_slider(1.0), 0.5, epsilon = 1e-5);
    }

    #[test]
    fn gamma_slider_monotonic() {
        #[expect(clippy::cast_precision_loss, reason = "the loop counter is ten")]
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
        for gamma in [GAMMA_MIN, 0.5, 1.0, 2.0, GAMMA_MAX] {
            let t = gamma_value_to_slider(gamma);
            assert_relative_eq!(gamma_slider_to_value(t), gamma, epsilon = 1e-5);
        }
    }
}
