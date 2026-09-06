use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::display::layout::DisplayMode;
use tracing::warn;

use crate::scene::camera::{CAMERA_FOV_MAX, CAMERA_FOV_MIN, CameraParams};

mod window_geometry;

pub use window_geometry::{save_window_geometry, validated_window_geometry};

/// How much the app is allowed to spend on looking good.
///
/// The point of the tiers is that the cheap one is the default while
/// developing and testing: an 8x MSAA 4K preview is not what anyone wants on
/// every `cargo run`, and it used to be exactly what they got. Release builds
/// still default to the full-quality path.
///
/// The tier does not choose the cloud image variant; [`TEXTURE_RESOLUTIONS`]
/// does, along with the rest of the texture memory. The cloud overlay is the
/// largest texture the app holds, so it belongs with the setting that says how
/// much texture memory to spend rather than with the one that says how much
/// work a frame is allowed to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QualityTier {
    Low,
    Medium,
    High,
}

impl QualityTier {
    /// Low while developing, high in a shipped binary.
    pub(crate) fn default_for_build() -> Self {
        if cfg!(debug_assertions) {
            Self::Low
        } else {
            Self::High
        }
    }

    /// Upper bound on the MSAA sample count. The UI's anti-aliasing options are
    /// filtered by this, so the combo box never offers something the tier will
    /// silently ignore.
    pub fn max_sample_count(self) -> u32 {
        match self {
            Self::Low => 1,
            Self::Medium => 4,
            Self::High => u32::MAX,
        }
    }

    /// Upper bound on the preview width in physical pixels. The height follows
    /// from the aspect ratio.
    pub(crate) fn max_preview_width(self) -> u32 {
        match self {
            Self::Low => 1280,
            Self::Medium => 1920,
            Self::High => u32::MAX,
        }
    }
}

impl Default for QualityTier {
    fn default() -> Self {
        Self::default_for_build()
    }
}

/// The strongest camera mode the settings window offers.
///
/// The spikes and the ghosts both reach their whole effect here, so the slider
/// stops at one and [`AppConfig::sanitize`] clamps a file to it, the way the
/// camera's own lens is clamped: a value nothing on screen can bring back is
/// not a setting.
pub(crate) const SUN_FLARE_MAX: f32 = 1.0;

/// The surface texture widths the user can choose between, widest first.
///
/// The two local assets are 8192 wide; the other two entries are exact halvings
/// of it, which is what lets the loader reach them with the box filter it
/// already uses for mip levels. This array is also the combo box model, so the
/// order here is the order on screen.
pub const TEXTURE_RESOLUTIONS: [u32; 3] = [8192, 4096, 2048];

/// The width a config without a `texture_resolution` key lands on.
///
/// Half of what the assets hold. The full 8192 costs about 400 MiB of GPU
/// memory across the two textures and their mip chains for detail that is
/// invisible at any sane zoom, so the default is the middle entry and the
/// widest is opt-in.
pub const DEFAULT_TEXTURE_RESOLUTION: u32 = 4096;

/// Replace a texture resolution that is not one of [`TEXTURE_RESOLUTIONS`] with
/// the default.
///
/// A config file is a text file: a hand-edited or foreign value arrives here as
/// a bare number, and the loader is the one place that has to reject it, since
/// everything downstream treats the value as an exact halving of the source.
pub(crate) fn resolve_texture_resolution(requested: u32) -> u32 {
    if TEXTURE_RESOLUTIONS.contains(&requested) {
        return requested;
    }
    warn!(
        requested,
        using = DEFAULT_TEXTURE_RESOLUTION,
        allowed = ?TEXTURE_RESOLUTIONS,
        "texture resolution is not one of the offered widths, falling back"
    );
    DEFAULT_TEXTURE_RESOLUTION
}

/// The combo box index for `width`, falling back to the default's index.
#[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
pub fn find_texture_resolution_index(width: u32) -> i32 {
    TEXTURE_RESOLUTIONS
        .iter()
        .position(|&w| w == width)
        .or_else(|| {
            TEXTURE_RESOLUTIONS
                .iter()
                .position(|&w| w == DEFAULT_TEXTURE_RESOLUTION)
        })
        .unwrap_or(0) as i32
}

/// The texture width a combo box index selects, falling back to the default.
#[allow(clippy::cast_sign_loss)]
pub fn texture_resolution_at(index: i32) -> u32 {
    if index < 0 {
        return DEFAULT_TEXTURE_RESOLUTION;
    }
    TEXTURE_RESOLUTIONS
        .get(index as usize)
        .copied()
        .unwrap_or(DEFAULT_TEXTURE_RESOLUTION)
}

/// Top-level config file structure, producing a `[sunlit.earth]` table in TOML.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct ConfigFile {
    sunlit: SunlitSection,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct SunlitSection {
    earth: AppConfig,
}

/// All user-configurable settings that are persisted to disk.
///
/// A key this build does not know is dropped on read, and `save_config_to`
/// then writes only the fields below, so running an older build once discards
/// every setting a newer one added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)]
pub struct AppConfig {
    // Camera position
    pub longitude: f32,
    pub latitude: f32,
    pub zoom: f32,

    // Camera orientation
    pub tilt: f32,
    pub yaw: f32,
    pub pitch: f32,

    // Framing
    pub offset_x: f32,
    pub offset_y: f32,
    /// Vertical field of view of the Earth lens, in degrees, between
    /// `CAMERA_FOV_MIN` and `CAMERA_FOV_MAX`. The sky has its own lens and its
    /// own `sky_fov`; this one frames the globe.
    pub camera_fov: f32,

    // Rendering
    pub texture_index: i32,
    /// Width the two local surface textures are loaded at, one of
    /// [`TEXTURE_RESOLUTIONS`]. Not part of `SceneParams`: it decides which
    /// pixels to load, not what to draw.
    pub texture_resolution: u32,
    pub sample_count: u32,
    /// How much work the renderer and the asset pipeline are allowed to do.
    pub quality_tier: QualityTier,

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
    pub cloud_opacity_night: f32,
    pub cloud_floor: f32,
    pub cloud_gamma: f32,
    pub cloud_night: f32,

    // Atmosphere
    pub atmo_enabled: bool,
    pub rayleigh_intensity: f32,
    pub rayleigh_sharpness: f32,
    pub rayleigh_haze: f32,
    pub nightglow_intensity: f32,
    pub nightglow_falloff: f32,
    pub nightglow_balance: f32,
    /// Brightness of the warm forward-scattering lobe the Rayleigh shell takes
    /// around a rising or setting Sun, over the blue it already scatters.
    pub atmo_sunrise_glow: f32,
    /// How far along the horizon that lobe reaches from the Sun: the scattering
    /// angle, in degrees, at which it falls to half.
    pub atmo_sunrise_width: f32,

    // Celestial background
    /// Horizontal field of view for the stereographic sky lens, in degrees.
    pub sky_fov: f32,
    /// Display brightness gain for stars and naked eye planets. A value of
    /// `1.0` is the renderer's neutral artistic exposure, while zero disables
    /// the sprite draw.
    pub star_intensity: f32,
    /// Multiplier for the compact star core. Glow size is controlled
    /// independently by `star_glow_radius`.
    pub star_size: f32,
    /// Strength of the soft halo around visually prominent stars. The UI
    /// offers zero through three times the nominal maximum.
    pub star_glow_strength: f32,
    /// Halo extent in pixels at 1080p. Output resolution scaling is automatic.
    pub star_glow_radius: f32,
    /// Separation between bright and faint stars, from minus one through one.
    /// Zero is neutral and negative values make magnitudes more uniform.
    pub star_contrast: f32,
    pub star_mag_limit: f32,
    /// Master strength of the Sun's glare, and the switch that puts the Sun in
    /// the scene at all: zero draws neither the disk nor the glare.
    pub sun_glow: f32,
    /// Strength of the ciliary corona, the fine radial needles the eye adds
    /// around a bright source.
    pub sun_rays: f32,
    /// Strength of camera mode: aperture spikes and lens ghosts, which belong
    /// to an imaging device rather than to an eye.
    pub sun_flare: f32,
    /// Multiplier on the Sun's radius, from its true angular size upward, the
    /// way `moon_size` works. The bloom's inner lobe keeps its thickness around
    /// the enlarged disk; everything else in the glare is a property of the eye
    /// and stays in absolute degrees.
    pub sun_size: f32,
    /// Angular radius of the lenticular halo ring around the glare, in degrees.
    /// The ring's width scales with it.
    pub sun_halo_radius: f32,
    /// How much brighter the glare peaks as the disk clears the horizon zone,
    /// before an eye exposed for the night side has adjusted. One is the
    /// physical answer and has no peak at all.
    pub sun_horizon_boost: f32,
    /// How far above the horizon zone, in zone widths, that peak decays back to
    /// the ordinary glare. Altitude stands in for the seconds an eye or a
    /// camera takes to settle, which at orbital rates is the same thing.
    pub sun_horizon_reach: f32,
    /// Thickness of the horizon zone in Sun diameters. Zero uses the painted
    /// annulus between the globe and the atmosphere shell alone, which is one
    /// or two pixels on a preview; the unit every other horizon effect is
    /// measured in.
    pub sun_horizon_depth: f32,
    /// How much the low atmosphere reddens the disk and the glare, as a scale
    /// on the air mass. One is the measured atmosphere and zero is the white
    /// Sun that knows nothing about the limb.
    pub sun_reddening: f32,
    /// How far the atmosphere lifts and flattens the disk near the horizon.
    /// Zero is the geometric Sun, one the coefficients derived from orbit.
    pub sun_refraction: f32,
    /// Brightness of the Moon's sunlit face, and the switch that puts the Moon
    /// in the scene at all: zero draws nothing.
    pub moon_brightness: f32,
    /// Multiplier on the Moon's radius, from its true angular size upward. The
    /// honest way to a larger Moon is a narrower `sky_fov`, which magnifies the
    /// sky around it too; this one magnifies the Moon alone.
    pub moon_size: f32,
    /// Floor under the Moon's unlit face: the earthshine that keeps a new moon
    /// from disappearing altogether.
    pub moon_earthshine: f32,
    /// Brightness of the diffuse Milky Way panorama, and the switch that puts
    /// it in the scene at all: zero skips the draw.
    pub milky_way_intensity: f32,

    // Color correction
    pub day_gamma: f32,
    pub day_saturation: f32,
    pub night_gamma: f32,
    pub night_saturation: f32,

    // Auto-refresh (wallpaper scheduler)
    pub auto_refresh_enabled: bool,
    pub auto_refresh_interval_minutes: u32,

    // Displays
    /// How this session's monitors relate to each other.
    ///
    /// Not a shader parameter, so it is not in `SceneParams` or its digest: it
    /// decides how many images a publish makes and how each is framed, not what
    /// a frame draws. `texture_resolution` is the precedent.
    pub display_mode: DisplayMode,
    /// The monitor a plan is anchored to, by the id its platform addresses it
    /// with; empty follows whatever the system calls primary.
    ///
    /// A `String` rather than an `Option<String>` in the file, because an empty
    /// value and a missing one mean the same thing here and one spelling in the
    /// TOML is one thing to explain.
    pub anchor_monitor: String,

    // Custom date/time override
    pub use_custom_datetime: bool,
    pub custom_hour: f32,
    pub custom_day_of_year: f32,
    pub custom_year: i32,

    // Window geometry (None on first launch — let the OS place the window)
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
    pub window_width: Option<u32>,
    pub window_height: Option<u32>,
}

/// Default value for `custom_year` when the field is missing from config.
fn default_custom_year() -> i32 {
    time::OffsetDateTime::now_utc().year()
}

impl AppConfig {
    /// Bring values a file could hold but the app cannot use back into range.
    ///
    /// Every load goes through here, so the rest of the app may treat a loaded
    /// config as valid. Serde's own defaults cover a *missing* field; this
    /// covers a present one with a value nothing offers.
    fn sanitize(&mut self) {
        self.texture_resolution = resolve_texture_resolution(self.texture_resolution);
        // The one parameter whose out-of-range value is not an ugly picture but
        // no picture at all: the perspective projection divides by
        // `tan(fov / 2)`, which is zero at 0 degrees and infinite at 180.
        self.camera_fov = self.camera_fov.clamp(CAMERA_FOV_MIN, CAMERA_FOV_MAX);
        // Camera mode reaches its whole effect at one and the slider stops
        // there, so a file holding more would draw a flare nothing on screen
        // can bring back.
        self.sun_flare = self.sun_flare.clamp(0.0, SUN_FLARE_MAX);
    }

    /// The monitor the wallpaper plan is anchored to, or `None` for the primary.
    pub fn anchor(&self) -> Option<String> {
        (!self.anchor_monitor.trim().is_empty()).then(|| self.anchor_monitor.clone())
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        let cam = CameraParams::default();
        Self {
            longitude: cam.longitude,
            latitude: cam.latitude,
            zoom: cam.zoom,
            tilt: cam.tilt_deg,
            yaw: cam.yaw_deg,
            pitch: cam.pitch_deg,
            offset_x: cam.offset_x,
            offset_y: cam.offset_y,
            camera_fov: cam.fov_deg,
            texture_index: 3,
            texture_resolution: DEFAULT_TEXTURE_RESOLUTION,
            sample_count: 8,
            quality_tier: QualityTier::default_for_build(),
            terminator_width: 0.1,
            diffuse_shading: true,
            diffuse_floor: 0.70,
            diffuse_ramp: 0.20,
            spec_shininess: 100.0,
            spec_intensity: 0.17,
            fresnel_mix: 0.75,
            fresnel_exp: 4.0,
            cloud_opacity: 0.85,
            cloud_opacity_night: 0.55,
            cloud_floor: 0.25,
            cloud_gamma: 0.65,
            cloud_night: 0.20,
            atmo_enabled: true,
            rayleigh_intensity: 0.5,
            rayleigh_sharpness: 50.0,
            rayleigh_haze: 0.55,
            nightglow_intensity: 0.25,
            nightglow_falloff: 15.0,
            nightglow_balance: 0.37,
            atmo_sunrise_glow: 1.0,
            atmo_sunrise_width: 20.0,
            sky_fov: 140.0,
            star_intensity: 2.0,
            star_size: 1.0,
            star_glow_strength: 0.5,
            star_glow_radius: 8.0,
            star_contrast: 0.3,
            star_mag_limit: 6.5,
            sun_glow: 1.2,
            sun_rays: 0.75,
            sun_flare: 0.15,
            sun_size: 1.0,
            sun_halo_radius: 3.0,
            sun_horizon_boost: 3.0,
            sun_horizon_reach: 4.0,
            sun_horizon_depth: 1.0,
            sun_reddening: 1.0,
            sun_refraction: 1.0,
            moon_brightness: 1.0,
            moon_size: 2.5,
            moon_earthshine: 0.15,
            milky_way_intensity: 0.2,
            day_gamma: 1.0,
            day_saturation: 1.0,
            night_gamma: 1.0,
            night_saturation: 0.85,
            auto_refresh_enabled: false,
            auto_refresh_interval_minutes: 5,
            display_mode: DisplayMode::default(),
            anchor_monitor: String::new(),
            use_custom_datetime: false,
            custom_hour: 12.0,
            custom_day_of_year: 1.0,
            custom_year: default_custom_year(),
            window_x: None,
            window_y: None,
            window_width: None,
            window_height: None,
        }
    }
}

/// Environment variable overriding the config file location.
const ENV_CONFIG: &str = "SUNLIT_EARTH_CONFIG";

/// Returns the path to the config file.
///
/// On Windows this resolves to `%LOCALAPPDATA%\SunlitEarth\config.toml`.
/// `SUNLIT_EARTH_CONFIG` overrides the location so tests do not read or write
/// the developer's real settings. Returns `None` if the platform's local data
/// directory cannot be determined.
pub(crate) fn config_path() -> Option<PathBuf> {
    config_path_from(crate::env_override(ENV_CONFIG).as_deref())
}

/// Resolve the config file path from an optional environment override.
fn config_path_from(env_path: Option<&str>) -> Option<PathBuf> {
    match env_path {
        Some(path) => Some(PathBuf::from(path)),
        None => Some(
            dirs::data_local_dir()?
                .join("SunlitEarth")
                .join("config.toml"),
        ),
    }
}

/// Load the app configuration from the standard path.
///
/// [`load_config_from`] does the work and states what a failure gives back.
pub fn load_config() -> AppConfig {
    let Some(path) = config_path() else {
        warn!("could not determine config directory");
        return AppConfig::default();
    };
    load_config_from(&path)
}

/// Load config from a specific path.
///
/// Returns `AppConfig::default()` if the file does not exist, cannot be read,
/// or contains invalid TOML. A read or parse failure is logged at `warn`.
pub fn load_config_from(path: &std::path::Path) -> AppConfig {
    let contents = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return AppConfig::default();
        }
        Err(e) => {
            warn!(path = %path.display(), error = %e, "could not read config file");
            return AppConfig::default();
        }
    };
    match toml::from_str::<ConfigFile>(&contents) {
        Ok(file) => {
            let mut config = file.sunlit.earth;
            config.sanitize();
            config
        }
        Err(e) => {
            warn!(path = %path.display(), error = %e, "could not parse config file");
            AppConfig::default()
        }
    }
}

/// Save the app configuration to disk.
///
/// Uses an atomic write strategy: writes to a temporary file with a `~`
/// suffix, then renames it to the final path. Creates the parent directory
/// if it does not exist. Errors are logged at `warn` and never propagated.
pub fn save_config(config: &AppConfig) {
    let Some(path) = config_path() else {
        warn!("could not determine config directory; config not saved");
        return;
    };
    save_config_to(config, &path);
}

/// Save config to a specific path (used by both the public API and tests).
fn save_config_to(config: &AppConfig, path: &std::path::Path) {
    let file = ConfigFile {
        sunlit: SunlitSection {
            earth: config.clone(),
        },
    };
    let toml_str = match toml::to_string_pretty(&file) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "could not serialize config");
            return;
        }
    };

    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        warn!(path = %parent.display(), error = %e, "could not create config directory");
        return;
    }

    let tmp_path = path.with_extension("toml~");
    if let Err(e) = fs::write(&tmp_path, &toml_str) {
        warn!(path = %tmp_path.display(), error = %e, "could not write temporary config file");
        return;
    }

    if let Err(e) = fs::rename(&tmp_path, path) {
        warn!(path = %path.display(), error = %e, "could not rename config file");
    }
}

/// Find the index of `desired` sample count in `aa_counts`, or fall back
/// to the last index (highest available count).
///
/// Returns the index as `i32` for direct use with Slint's `set_aa_index()`.
#[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
pub fn find_sample_count_index(aa_counts: &[u32], desired: u32) -> i32 {
    aa_counts
        .iter()
        .position(|&c| c == desired)
        .unwrap_or(aa_counts.len().saturating_sub(1)) as i32
}

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;

    use super::*;
    use crate::scene::camera::DEFAULT_CAMERA_FOV;
    use crate::test_support::ScratchDir;

    // --- config_path resolution ---

    #[test]
    fn config_path_with_override_uses_that_file() {
        let path =
            config_path_from(Some("C:/tmp/sunlit/custom.toml")).expect("override should resolve");
        assert_eq!(path, PathBuf::from("C:/tmp/sunlit/custom.toml"));
    }

    #[test]
    fn config_path_without_override_uses_app_folder() {
        if let Some(path) = config_path_from(None) {
            assert!(
                path.ends_with("config.toml"),
                "unexpected path: {}",
                path.display()
            );
            assert!(
                path.parent().is_some_and(|p| p.ends_with("SunlitEarth")),
                "expected the app folder, got {}",
                path.display()
            );
        }
    }

    // --- AppConfig defaults and serde ---

    #[test]
    fn default_values_match_camera_params() {
        let config = AppConfig::default();
        let cam = CameraParams::default();
        assert_relative_eq!(config.longitude, cam.longitude);
        assert_relative_eq!(config.latitude, cam.latitude);
        assert_relative_eq!(config.zoom, cam.zoom);
        assert_relative_eq!(config.offset_x, cam.offset_x);
        assert_relative_eq!(config.offset_y, cam.offset_y);
        assert_relative_eq!(config.tilt, cam.tilt_deg);
        assert_relative_eq!(config.yaw, cam.yaw_deg);
        assert_relative_eq!(config.pitch, cam.pitch_deg);
    }

    /// Every default is already inside the range the loader enforces, so a
    /// fresh config is a fixed point of the repair pass rather than something
    /// `sanitize` rewrites on the way in.
    #[test]
    fn the_defaults_survive_the_loaders_own_repairs() {
        let mut config = AppConfig::default();
        config.sanitize();
        assert_eq!(config, AppConfig::default());
    }

    /// Every field a file leaves out arrives at its default, including the ones
    /// nobody thought to list. `AppConfig` derives `PartialEq`, so one
    /// comparison against a default carrying the single present field covers
    /// the whole struct.
    #[test]
    fn a_field_a_file_leaves_out_arrives_at_its_default() {
        let config: AppConfig = toml::from_str("longitude = 42.0").unwrap();
        assert_eq!(
            config,
            AppConfig {
                longitude: 42.0,
                ..AppConfig::default()
            }
        );
    }

    #[test]
    fn a_lens_flare_past_the_sliders_end_loads_clamped() {
        let scratch = ScratchDir::new("config_clamp_sun_flare");
        let path = scratch.join("config.toml");

        fs::write(&path, "[sunlit.earth]\nsun_flare = 1.8\n").unwrap();
        assert_relative_eq!(load_config_from(&path).sun_flare, SUN_FLARE_MAX);
        fs::write(&path, "[sunlit.earth]\nsun_flare = -0.5\n").unwrap();
        assert_relative_eq!(load_config_from(&path).sun_flare, 0.0);
    }

    #[test]
    fn the_earth_lens_defaults_to_the_narrow_one_the_presets_were_framed_at() {
        assert_relative_eq!(AppConfig::default().camera_fov, DEFAULT_CAMERA_FOV);
    }

    #[test]
    fn loading_a_config_with_a_degenerate_lens_repairs_it() {
        let scratch = ScratchDir::new("config_bad_fov");
        let path = scratch.join("config.toml");

        fs::write(&path, "[sunlit.earth]\ncamera_fov = 180.0\n").unwrap();
        assert_relative_eq!(load_config_from(&path).camera_fov, CAMERA_FOV_MAX);

        fs::write(&path, "[sunlit.earth]\ncamera_fov = 0.0\n").unwrap();
        assert_relative_eq!(load_config_from(&path).camera_fov, CAMERA_FOV_MIN);
    }

    #[test]
    fn serde_round_trip() {
        let config = AppConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: AppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn default_window_geometry_is_none() {
        let config = AppConfig::default();
        assert!(config.window_x.is_none());
        assert!(config.window_y.is_none());
        assert!(config.window_width.is_none());
        assert!(config.window_height.is_none());
    }

    #[test]
    fn serde_round_trip_non_default() {
        let config = AppConfig {
            longitude: 42.5,
            latitude: -15.0,
            zoom: 0.8,
            tilt: 30.0,
            yaw: -10.0,
            pitch: 5.0,
            offset_x: 0.3,
            offset_y: -0.2,
            camera_fov: 35.0,
            texture_index: 1,
            texture_resolution: 8192,
            sample_count: 4,
            quality_tier: QualityTier::Medium,
            terminator_width: 0.2,
            diffuse_shading: false,
            diffuse_floor: 0.75,
            diffuse_ramp: 0.5,
            spec_shininess: 200.0,
            spec_intensity: 0.6,
            fresnel_mix: 0.5,
            fresnel_exp: 3.0,
            cloud_opacity: 0.6,
            cloud_opacity_night: 0.45,
            cloud_floor: 0.2,
            cloud_gamma: 0.3,
            cloud_night: 0.4,
            atmo_enabled: false,
            rayleigh_intensity: 0.7,
            rayleigh_sharpness: 8.0,
            rayleigh_haze: 0.4,
            nightglow_intensity: 0.5,
            nightglow_falloff: 6.0,
            nightglow_balance: 0.3,
            atmo_sunrise_glow: 1.6,
            atmo_sunrise_width: 55.0,
            sky_fov: 110.0,
            star_intensity: 0.7,
            star_size: 1.4,
            star_glow_strength: 0.6,
            star_glow_radius: 8.0,
            star_contrast: 0.7,
            star_mag_limit: 5.8,
            sun_glow: 1.4,
            sun_rays: 0.3,
            sun_flare: 0.9,
            sun_size: 2.5,
            sun_halo_radius: 4.5,
            sun_horizon_boost: 2.0,
            sun_horizon_reach: 6.0,
            sun_horizon_depth: 2.0,
            sun_reddening: 0.7,
            sun_refraction: 1.5,
            moon_brightness: 1.3,
            moon_size: 2.5,
            moon_earthshine: 0.12,
            milky_way_intensity: 0.8,
            day_gamma: 1.5,
            day_saturation: 0.8,
            night_gamma: 2.0,
            night_saturation: 0.5,
            display_mode: DisplayMode::AcrossScreens,
            anchor_monitor: "DP-2".to_owned(),
            auto_refresh_enabled: true,
            auto_refresh_interval_minutes: 15,
            use_custom_datetime: true,
            custom_hour: 14.5,
            custom_day_of_year: 76.0,
            custom_year: 2030,
            window_x: Some(100),
            window_y: Some(200),
            window_width: Some(1024),
            window_height: Some(768),
        };
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: AppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn deserialize_empty_string() {
        let config: AppConfig = toml::from_str("").unwrap();
        assert_eq!(config, AppConfig::default());
    }

    #[test]
    fn default_custom_year_is_current() {
        let config = AppConfig::default();
        let current_year = time::OffsetDateTime::now_utc().year();
        assert_eq!(config.custom_year, current_year);
    }

    #[test]
    fn deserialize_explicit_zero_year_stays_zero() {
        let config: AppConfig = toml::from_str("custom_year = 0").unwrap();
        assert_eq!(config.custom_year, 0);
    }

    #[test]
    fn a_display_mode_this_build_does_not_have_loads_as_the_default() {
        let config: AppConfig = toml::from_str(
            "display_mode = \"every-other-screen\"
anchor_monitor = \"DP-9\"
sky_fov = 111.0
",
        )
        .expect("an unknown mode must not fail the whole file");
        assert_eq!(config.display_mode, DisplayMode::default());
        assert_relative_eq!(config.sky_fov, 111.0);
        // The anchor is a name, not an enumeration, so a screen this session
        // does not have is kept: it is the screen somebody chose, and it comes
        // back when they plug it in again.
        assert_eq!(config.anchor(), Some("DP-9".to_owned()));
    }

    #[test]
    fn an_empty_anchor_is_the_system_primary() {
        let config = AppConfig::default();
        assert_eq!(config.display_mode, DisplayMode::EveryScreen);
        assert_eq!(config.anchor(), None);
        let blank = AppConfig {
            anchor_monitor: "   ".to_owned(),
            ..AppConfig::default()
        };
        assert_eq!(blank.anchor(), None);
    }

    #[test]
    fn deserialize_unknown_fields_ignored() {
        let config: AppConfig = toml::from_str("future_field = true").unwrap();
        assert_eq!(config, AppConfig::default());
    }

    // --- Config file I/O ---

    #[test]
    fn load_from_nonexistent_returns_default() {
        let scratch = ScratchDir::new("config_nonexistent");
        let config = load_config_from(&scratch.join("config.toml"));
        assert_eq!(config, AppConfig::default());
    }

    #[test]
    fn save_and_load_round_trip() {
        let scratch = ScratchDir::new("config_roundtrip");
        let path = scratch.join("config.toml");

        // A tier this build does not default to, so a repair that reset the
        // field could not pass by accident.
        let tier = if QualityTier::default_for_build() == QualityTier::High {
            QualityTier::Low
        } else {
            QualityTier::High
        };
        let config = AppConfig {
            longitude: 99.0,
            latitude: -45.0,
            zoom: 0.6,
            tilt: 15.0,
            yaw: 20.0,
            pitch: -5.0,
            offset_x: 0.1,
            offset_y: -0.3,
            camera_fov: 65.0,
            texture_index: 2,
            texture_resolution: 2048,
            sample_count: 4,
            quality_tier: tier,
            terminator_width: 0.15,
            diffuse_shading: false,
            diffuse_floor: 0.8,
            diffuse_ramp: 0.4,
            spec_shininess: 300.0,
            spec_intensity: 0.8,
            fresnel_mix: 0.7,
            fresnel_exp: 4.0,
            cloud_opacity: 0.6,
            cloud_opacity_night: 0.95,
            cloud_floor: 0.15,
            cloud_gamma: 0.5,
            cloud_night: 0.1,
            atmo_enabled: false,
            rayleigh_intensity: 0.5,
            rayleigh_sharpness: 7.0,
            rayleigh_haze: 0.5,
            nightglow_intensity: 0.4,
            nightglow_falloff: 5.0,
            nightglow_balance: 0.6,
            atmo_sunrise_glow: 0.4,
            atmo_sunrise_width: 20.0,
            sky_fov: 155.0,
            star_intensity: 0.8,
            star_size: 1.6,
            star_glow_strength: 0.5,
            star_glow_radius: 7.5,
            star_contrast: 0.65,
            star_mag_limit: 6.2,
            sun_glow: 0.8,
            sun_rays: 0.9,
            sun_flare: 0.4,
            sun_size: 5.0,
            sun_halo_radius: 7.0,
            sun_horizon_boost: 6.0,
            sun_horizon_reach: 1.5,
            sun_horizon_depth: 0.5,
            sun_reddening: 1.4,
            sun_refraction: 0.25,
            moon_brightness: 0.6,
            moon_size: 6.0,
            moon_earthshine: 0.3,
            milky_way_intensity: 1.6,
            day_gamma: 1.8,
            day_saturation: 0.6,
            night_gamma: 2.2,
            night_saturation: 1.5,
            display_mode: DisplayMode::AcrossScreens,
            anchor_monitor: "DP-2".to_owned(),
            auto_refresh_enabled: true,
            auto_refresh_interval_minutes: 10,
            use_custom_datetime: true,
            custom_hour: 8.25,
            custom_day_of_year: 200.0,
            custom_year: 2020,
            window_x: Some(50),
            window_y: Some(75),
            window_width: Some(800),
            window_height: Some(600),
        };

        save_config_to(&config, &path);
        let loaded = load_config_from(&path);
        assert_eq!(config, loaded);
    }

    #[test]
    fn save_creates_parent_directory() {
        let scratch = ScratchDir::new("config_mkdir");
        let path = scratch.join("nested").join("config.toml");

        save_config_to(&AppConfig::default(), &path);
        assert!(path.exists());
    }

    #[test]
    fn save_atomic_write_uses_tilde() {
        let scratch = ScratchDir::new("config_atomic");
        let path = scratch.join("config.toml");

        save_config_to(&AppConfig::default(), &path);

        assert!(path.exists());
        assert!(
            !path.with_extension("toml~").exists(),
            "the unfinished file must be gone once the rename completes"
        );
    }

    #[test]
    fn load_corrupt_file_returns_default() {
        let scratch = ScratchDir::new("config_corrupt");
        let path = scratch.join("config.toml");

        fs::write(&path, "{{{invalid toml content").unwrap();
        assert_eq!(load_config_from(&path), AppConfig::default());
    }

    /// The same field-by-field default filling as
    /// `a_field_a_file_leaves_out_arrives_at_its_default`, through the file
    /// path, which adds the `[sunlit.earth]` section and the repair pass.
    #[test]
    fn load_partial_file_fills_defaults() {
        let scratch = ScratchDir::new("config_partial");
        let path = scratch.join("config.toml");

        fs::write(&path, "[sunlit.earth]\nlongitude = 99.0\n").unwrap();
        assert_eq!(
            load_config_from(&path),
            AppConfig {
                longitude: 99.0,
                ..AppConfig::default()
            }
        );
    }

    #[test]
    fn serde_window_geometry_none_omitted() {
        let file = ConfigFile::default();
        let toml_str = toml::to_string_pretty(&file).unwrap();
        // None fields should not appear in the TOML output
        assert!(!toml_str.contains("window_x"));
        assert!(!toml_str.contains("window_y"));
        assert!(!toml_str.contains("window_width"));
        assert!(!toml_str.contains("window_height"));
    }

    #[test]
    fn serde_window_geometry_round_trip() {
        let config = AppConfig {
            window_x: Some(100),
            window_y: Some(200),
            window_width: Some(1920),
            window_height: Some(1080),
            ..AppConfig::default()
        };
        let file = ConfigFile {
            sunlit: SunlitSection { earth: config },
        };
        let toml_str = toml::to_string_pretty(&file).unwrap();
        let parsed: ConfigFile = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.sunlit.earth.window_x, Some(100));
        assert_eq!(parsed.sunlit.earth.window_y, Some(200));
        assert_eq!(parsed.sunlit.earth.window_width, Some(1920));
        assert_eq!(parsed.sunlit.earth.window_height, Some(1080));
    }

    #[test]
    fn config_file_contains_sunlit_earth_table() {
        let file = ConfigFile::default();
        let toml_str = toml::to_string_pretty(&file).unwrap();
        assert!(toml_str.contains("[sunlit.earth]"));
    }

    // --- quality tiers ---

    #[test]
    fn tier_caps_are_ordered() {
        let tiers = [QualityTier::Low, QualityTier::Medium, QualityTier::High];
        for pair in tiers.windows(2) {
            assert!(
                pair[0].max_sample_count() < pair[1].max_sample_count(),
                "sample cap should grow with the tier"
            );
            assert!(
                pair[0].max_preview_width() < pair[1].max_preview_width(),
                "preview cap should grow with the tier"
            );
        }
    }

    #[test]
    fn low_tier_disables_msaa() {
        assert_eq!(QualityTier::Low.max_sample_count(), 1);
    }

    #[test]
    fn the_default_config_takes_the_tier_this_build_asks_for() {
        assert_eq!(
            AppConfig::default().quality_tier,
            QualityTier::default_for_build()
        );
    }

    #[test]
    fn quality_tier_serializes_lowercase() {
        let config = AppConfig {
            quality_tier: QualityTier::Medium,
            ..AppConfig::default()
        };
        let toml_str = toml::to_string_pretty(&config).unwrap();
        assert!(
            toml_str.contains("quality_tier = \"medium\""),
            "unexpected serialization:
{toml_str}"
        );
    }

    #[test]
    fn quality_tier_round_trips_for_every_variant() {
        for tier in [QualityTier::Low, QualityTier::Medium, QualityTier::High] {
            let config = AppConfig {
                quality_tier: tier,
                ..AppConfig::default()
            };
            let parsed: AppConfig =
                toml::from_str(&toml::to_string_pretty(&config).unwrap()).unwrap();
            assert_eq!(parsed.quality_tier, tier);
        }
    }

    // --- texture resolution ---

    #[test]
    fn default_texture_resolution_is_one_of_the_offered_widths() {
        assert!(TEXTURE_RESOLUTIONS.contains(&DEFAULT_TEXTURE_RESOLUTION));
        assert_eq!(
            AppConfig::default().texture_resolution,
            DEFAULT_TEXTURE_RESOLUTION
        );
    }

    #[test]
    fn offered_widths_are_exact_halvings_of_the_widest() {
        for pair in TEXTURE_RESOLUTIONS.windows(2) {
            assert_eq!(
                pair[0],
                pair[1] * 2,
                "each width must be twice the next, so halving reaches it"
            );
        }
    }

    #[test]
    fn every_offered_width_is_accepted() {
        for width in TEXTURE_RESOLUTIONS {
            assert_eq!(resolve_texture_resolution(width), width);
        }
    }

    #[test]
    fn a_width_nothing_offers_falls_back_to_the_default() {
        for width in [0, 1, 1024, 3000, 4095, 16384, u32::MAX] {
            assert_eq!(
                resolve_texture_resolution(width),
                DEFAULT_TEXTURE_RESOLUTION
            );
        }
    }

    #[test]
    fn loading_a_config_with_an_impossible_resolution_repairs_it() {
        let scratch = ScratchDir::new("config_bad_resolution");
        let path = scratch.join("config.toml");

        fs::write(&path, "[sunlit.earth]\ntexture_resolution = 12345\n").unwrap();
        assert_eq!(
            load_config_from(&path).texture_resolution,
            DEFAULT_TEXTURE_RESOLUTION
        );
    }

    #[test]
    fn resolution_index_and_width_are_inverses() {
        for (index, width) in TEXTURE_RESOLUTIONS.iter().enumerate() {
            let index = i32::try_from(index).unwrap();
            assert_eq!(find_texture_resolution_index(*width), index);
            assert_eq!(texture_resolution_at(index), *width);
        }
    }

    #[test]
    fn an_index_outside_the_model_yields_the_default_width() {
        for index in [-5, -1, 3, 99] {
            assert_eq!(texture_resolution_at(index), DEFAULT_TEXTURE_RESOLUTION);
        }
    }

    #[test]
    fn a_width_nothing_offers_indexes_the_default() {
        let expected = find_texture_resolution_index(DEFAULT_TEXTURE_RESOLUTION);
        assert_eq!(find_texture_resolution_index(1234), expected);
    }

    // --- find_sample_count_index ---

    #[test]
    fn a_sample_count_indexes_itself_or_the_strongest_on_offer() {
        for (offered, requested, expected) in [
            (&[1, 2, 4, 8][..], 4, 2),
            (&[1, 2, 4, 8][..], 8, 3),
            (&[1, 2, 4][..], 8, 2),
            (&[1][..], 8, 0),
        ] {
            assert_eq!(
                find_sample_count_index(offered, requested),
                expected,
                "{requested}x among {offered:?}"
            );
        }
    }
}
