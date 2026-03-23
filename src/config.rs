use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::scene::camera::CameraParams;

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
/// Fields use `#[serde(default)]` at the struct level so that missing
/// fields in the TOML file are filled from `Default::default()`, and
/// unknown fields are silently ignored (forward compatibility).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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

    // Rendering
    pub texture_index: i32,
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

    // Color correction
    pub day_gamma: f32,
    pub day_saturation: f32,
    pub night_gamma: f32,
    pub night_saturation: f32,

    // Custom date/time override
    pub use_custom_datetime: bool,
    pub custom_hour: f32,
    pub custom_day_of_year: f32,
    #[serde(default = "default_custom_year")]
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
            texture_index: 3,
            sample_count: 8,
            terminator_width: 0.1,
            diffuse_shading: true,
            diffuse_floor: 0.70,
            diffuse_ramp: 0.20,
            spec_shininess: 100.0,
            spec_intensity: 0.17,
            fresnel_mix: 0.75,
            fresnel_exp: 4.0,
            cloud_opacity: 0.85,
            cloud_floor: 0.25,
            cloud_gamma: 0.65,
            atmo_enabled: true,
            rayleigh_intensity: 0.5,
            rayleigh_sharpness: 50.0,
            rayleigh_haze: 0.55,
            nightglow_intensity: 0.25,
            nightglow_falloff: 15.0,
            nightglow_balance: 0.37,
            day_gamma: 1.0,
            day_saturation: 1.0,
            night_gamma: 1.0,
            night_saturation: 1.0,
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

/// Returns the path to the config file.
///
/// On Windows this resolves to `%LOCALAPPDATA%\SunlitEarth\config.toml`.
/// Returns `None` if the platform's local data directory cannot be determined.
pub fn config_path() -> Option<PathBuf> {
    Some(dirs::data_local_dir()?.join("SunlitEarth").join("config.toml"))
}

/// Load the app configuration from disk.
///
/// Returns `AppConfig::default()` if the file does not exist, cannot be
/// read, or contains invalid TOML. Parse errors are logged to stderr.
pub fn load_config() -> AppConfig {
    let Some(path) = config_path() else {
        warn!("could not determine config directory");
        return AppConfig::default();
    };
    load_config_from(&path)
}

/// Load config from a specific path (used by both the public API and tests).
fn load_config_from(path: &std::path::Path) -> AppConfig {
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
        Ok(file) => file.sunlit.earth,
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
/// if it does not exist. Errors are logged to stderr but never propagated.
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

/// Save only the window position and size to disk, preserving all other
/// config values. This is called on window close so geometry is always
/// persisted, even when the user hasn't clicked "Set as Wallpaper".
pub fn save_window_geometry(x: i32, y: i32, width: u32, height: u32) {
    let mut config = load_config();
    config.window_x = Some(x);
    config.window_y = Some(y);
    config.window_width = Some(width);
    config.window_height = Some(height);
    save_config(&config);
}

/// Check whether the saved window position is visible on at least one
/// connected monitor by testing if the title bar region overlaps any display.
///
/// Returns `true` if the position is on-screen, `false` if off-screen or
/// if validation cannot be performed.
#[cfg(windows)]
#[allow(clippy::cast_possible_truncation)]
fn is_position_on_screen(x: i32, y: i32, width: u32, height: u32) -> bool {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromRect};

    let title_bar_height = 30i32.min(height.cast_signed());
    let rect = RECT {
        left: x,
        top: y,
        right: x.saturating_add(width.cast_signed()),
        bottom: y.saturating_add(title_bar_height),
    };

    // SAFETY: MonitorFromRect reads a RECT struct and queries the display
    // configuration. The rect is a local stack variable with valid values.
    // MONITOR_DEFAULTTONULL returns null if no monitor contains the rect.
    #[allow(unsafe_code)]
    let monitor = unsafe { MonitorFromRect(&raw const rect, MONITOR_DEFAULTTONULL) };
    !monitor.is_null()
}

#[cfg(not(windows))]
fn is_position_on_screen(_x: i32, _y: i32, _width: u32, _height: u32) -> bool {
    // No validation on non-Windows platforms — accept any saved position
    true
}

/// Return the saved window geometry if it passes on-screen validation.
///
/// Returns `None` if any of the four geometry fields is missing or if
/// the saved position is no longer visible on any connected monitor.
pub fn validated_window_geometry(config: &AppConfig) -> Option<(i32, i32, u32, u32)> {
    let (Some(x), Some(y), Some(w), Some(h)) = (config.window_x, config.window_y, config.window_width, config.window_height) else {
        return None;
    };
    if w == 0 || h == 0 {
        return None;
    }
    if is_position_on_screen(x, y, w, h) {
        Some((x, y, w, h))
    } else {
        warn!(x, y, "saved window position is off-screen, using OS default");
        None
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

    // --- Step 2.1: AppConfig defaults and serde ---

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

    #[test]
    fn default_values_lighting() {
        let config = AppConfig::default();
        assert_relative_eq!(config.terminator_width, 0.1);
        assert!(config.diffuse_shading);
        assert_relative_eq!(config.diffuse_floor, 0.70);
        assert_relative_eq!(config.diffuse_ramp, 0.20);
    }

    #[test]
    fn default_values_rendering() {
        let config = AppConfig::default();
        assert_eq!(config.texture_index, 3);
        assert_eq!(config.sample_count, 8);
    }

    #[test]
    fn default_cloud_floor() {
        let config = AppConfig::default();
        assert_relative_eq!(config.cloud_floor, 0.25);
    }

    #[test]
    fn default_cloud_gamma() {
        let config = AppConfig::default();
        assert_relative_eq!(config.cloud_gamma, 0.65);
    }

    #[test]
    fn default_atmo_enabled() {
        let config = AppConfig::default();
        assert!(config.atmo_enabled);
    }

    #[test]
    fn default_rayleigh_intensity() {
        let config = AppConfig::default();
        assert_relative_eq!(config.rayleigh_intensity, 0.5);
    }

    #[test]
    fn default_rayleigh_sharpness() {
        let config = AppConfig::default();
        assert_relative_eq!(config.rayleigh_sharpness, 50.0);
    }

    #[test]
    fn default_nightglow_intensity() {
        let config = AppConfig::default();
        assert_relative_eq!(config.nightglow_intensity, 0.25);
    }

    #[test]
    fn default_nightglow_falloff() {
        let config = AppConfig::default();
        assert_relative_eq!(config.nightglow_falloff, 15.0);
    }

    #[test]
    fn default_nightglow_balance() {
        let config = AppConfig::default();
        assert_relative_eq!(config.nightglow_balance, 0.37);
    }

    #[test]
    fn deserialize_missing_atmo_fields_fills_defaults() {
        let config: AppConfig = toml::from_str("cloud_opacity = 0.5").unwrap();
        assert!(config.atmo_enabled);
        assert_relative_eq!(config.rayleigh_intensity, 0.5);
        assert_relative_eq!(config.rayleigh_sharpness, 50.0);
        assert_relative_eq!(config.nightglow_intensity, 0.25);
        assert_relative_eq!(config.nightglow_falloff, 15.0);
        assert_relative_eq!(config.nightglow_balance, 0.37);
    }

    #[test]
    fn deserialize_missing_cloud_fields_fills_defaults() {
        let config: AppConfig = toml::from_str("cloud_opacity = 0.5").unwrap();
        assert_relative_eq!(config.cloud_floor, 0.25);
        assert_relative_eq!(config.cloud_gamma, 0.65);
    }

    #[test]
    fn default_values_color_correction() {
        let config = AppConfig::default();
        assert_relative_eq!(config.day_gamma, 1.0);
        assert_relative_eq!(config.day_saturation, 1.0);
        assert_relative_eq!(config.night_gamma, 1.0);
        assert_relative_eq!(config.night_saturation, 1.0);
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
            texture_index: 1,
            sample_count: 4,
            terminator_width: 0.2,
            diffuse_shading: false,
            diffuse_floor: 0.75,
            diffuse_ramp: 0.5,
            spec_shininess: 200.0,
            spec_intensity: 0.6,
            fresnel_mix: 0.5,
            fresnel_exp: 3.0,
            cloud_opacity: 0.6,
            cloud_floor: 0.2,
            cloud_gamma: 0.3,
            atmo_enabled: false,
            rayleigh_intensity: 0.7,
            rayleigh_sharpness: 8.0,
            rayleigh_haze: 0.4,
            nightglow_intensity: 0.5,
            nightglow_falloff: 6.0,
            nightglow_balance: 0.3,
            day_gamma: 1.5,
            day_saturation: 0.8,
            night_gamma: 2.0,
            night_saturation: 0.5,
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
    fn deserialize_missing_fields() {
        let config: AppConfig = toml::from_str("longitude = 42.0").unwrap();
        assert_relative_eq!(config.longitude, 42.0);
        // All other fields should be at their defaults
        let defaults = AppConfig::default();
        assert_relative_eq!(config.latitude, defaults.latitude);
        assert_relative_eq!(config.zoom, defaults.zoom);
        assert_eq!(config.texture_index, defaults.texture_index);
    }

    // --- Custom datetime defaults ---

    #[test]
    fn default_use_custom_datetime_is_false() {
        let config = AppConfig::default();
        assert!(!config.use_custom_datetime);
    }

    #[test]
    fn default_custom_hour_is_12() {
        let config = AppConfig::default();
        assert_relative_eq!(config.custom_hour, 12.0);
    }

    #[test]
    fn default_custom_day_of_year_is_1() {
        let config = AppConfig::default();
        assert_relative_eq!(config.custom_day_of_year, 1.0);
    }

    #[test]
    fn default_custom_year_is_current() {
        let config = AppConfig::default();
        let current_year = time::OffsetDateTime::now_utc().year();
        assert_eq!(config.custom_year, current_year);
    }

    #[test]
    fn deserialize_missing_datetime_fields_fills_defaults() {
        let config: AppConfig = toml::from_str("longitude = 10.0").unwrap();
        let defaults = AppConfig::default();
        assert!(!config.use_custom_datetime);
        assert_relative_eq!(config.custom_hour, defaults.custom_hour);
        assert_relative_eq!(config.custom_day_of_year, defaults.custom_day_of_year);
        // custom_year uses its own serde default function
        let current_year = time::OffsetDateTime::now_utc().year();
        assert_eq!(config.custom_year, current_year);
    }

    #[test]
    fn deserialize_explicit_zero_year_stays_zero() {
        let config: AppConfig = toml::from_str("custom_year = 0").unwrap();
        assert_eq!(config.custom_year, 0);
    }

    #[test]
    fn deserialize_unknown_fields_ignored() {
        let config: AppConfig = toml::from_str("future_field = true").unwrap();
        assert_eq!(config, AppConfig::default());
    }

    #[test]
    fn deserialize_invalid_toml() {
        let result: Result<AppConfig, _> = toml::from_str("{{{invalid");
        assert!(result.is_err());
    }

    // --- Step 2.3: Config file I/O ---

    #[test]
    fn load_from_nonexistent_returns_default() {
        let path = std::env::temp_dir()
            .join("sunlit_earth_test_nonexistent")
            .join("config.toml");
        // Ensure the file does not exist
        let _ = fs::remove_file(&path);
        let config = load_config_from(&path);
        assert_eq!(config, AppConfig::default());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir().join("sunlit_earth_test_roundtrip");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("config.toml");

        let config = AppConfig {
            longitude: 99.0,
            latitude: -45.0,
            zoom: 0.6,
            tilt: 15.0,
            yaw: 20.0,
            pitch: -5.0,
            offset_x: 0.1,
            offset_y: -0.3,
            texture_index: 2,
            sample_count: 4,
            terminator_width: 0.15,
            diffuse_shading: false,
            diffuse_floor: 0.8,
            diffuse_ramp: 0.4,
            spec_shininess: 300.0,
            spec_intensity: 0.8,
            fresnel_mix: 0.7,
            fresnel_exp: 4.0,
            cloud_opacity: 0.6,
            cloud_floor: 0.15,
            cloud_gamma: 0.5,
            atmo_enabled: false,
            rayleigh_intensity: 0.5,
            rayleigh_sharpness: 7.0,
            rayleigh_haze: 0.5,
            nightglow_intensity: 0.4,
            nightglow_falloff: 5.0,
            nightglow_balance: 0.6,
            day_gamma: 1.8,
            day_saturation: 0.6,
            night_gamma: 2.2,
            night_saturation: 1.5,
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

        // Cleanup
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_creates_parent_directory() {
        let dir = std::env::temp_dir()
            .join("sunlit_earth_test_mkdir")
            .join("nested");
        let _ = fs::remove_dir_all(
            std::env::temp_dir().join("sunlit_earth_test_mkdir"),
        );
        let path = dir.join("config.toml");

        save_config_to(&AppConfig::default(), &path);
        assert!(path.exists());

        // Cleanup
        let _ = fs::remove_dir_all(
            std::env::temp_dir().join("sunlit_earth_test_mkdir"),
        );
    }

    #[test]
    fn save_atomic_write_uses_tilde() {
        let dir = std::env::temp_dir().join("sunlit_earth_test_atomic");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("config.toml");

        save_config_to(&AppConfig::default(), &path);

        // The final file should exist
        assert!(path.exists());
        // The temporary tilde file should NOT exist (rename completed)
        let tilde_path = path.with_extension("toml~");
        assert!(!tilde_path.exists());

        // Cleanup
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_corrupt_file_returns_default() {
        let dir = std::env::temp_dir().join("sunlit_earth_test_corrupt");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        fs::write(&path, "{{{invalid toml content").unwrap();
        let config = load_config_from(&path);
        assert_eq!(config, AppConfig::default());

        // Cleanup
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_partial_file_fills_defaults() {
        let dir = std::env::temp_dir().join("sunlit_earth_test_partial");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        fs::write(&path, "[sunlit.earth]\nlongitude = 99.0\n").unwrap();
        let config = load_config_from(&path);
        assert_relative_eq!(config.longitude, 99.0);
        // All other fields at defaults
        let defaults = AppConfig::default();
        assert_relative_eq!(config.latitude, defaults.latitude);
        assert_eq!(config.sample_count, defaults.sample_count);
        assert!(config.diffuse_shading);

        // Cleanup
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Window geometry validation ---

    #[test]
    fn validated_geometry_none_when_missing() {
        let config = AppConfig::default();
        assert!(validated_window_geometry(&config).is_none());
    }

    #[test]
    fn validated_geometry_none_when_partial() {
        let mut config = AppConfig::default();
        config.window_x = Some(100);
        config.window_y = Some(200);
        // width and height still None
        assert!(validated_window_geometry(&config).is_none());
    }

    #[test]
    fn validated_geometry_none_when_zero_size() {
        let mut config = AppConfig::default();
        config.window_x = Some(100);
        config.window_y = Some(200);
        config.window_width = Some(0);
        config.window_height = Some(600);
        assert!(validated_window_geometry(&config).is_none());
    }

    #[test]
    fn validated_geometry_accepts_on_screen() {
        let mut config = AppConfig::default();
        config.window_x = Some(100);
        config.window_y = Some(100);
        config.window_width = Some(800);
        config.window_height = Some(600);
        // On a machine with at least one monitor, (100, 100) should be on-screen
        let result = validated_window_geometry(&config);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), (100, 100, 800, 600));
    }

    #[test]
    fn validated_geometry_rejects_off_screen() {
        let mut config = AppConfig::default();
        config.window_x = Some(-50000);
        config.window_y = Some(-50000);
        config.window_width = Some(800);
        config.window_height = Some(600);
        assert!(validated_window_geometry(&config).is_none());
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
        let mut config = AppConfig::default();
        config.window_x = Some(100);
        config.window_y = Some(200);
        config.window_width = Some(1920);
        config.window_height = Some(1080);
        let file = ConfigFile { sunlit: SunlitSection { earth: config } };
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

    // --- Step 2.5: find_sample_count_index ---

    #[test]
    fn find_sample_count_exact_match() {
        assert_eq!(find_sample_count_index(&[1, 2, 4, 8], 4), 2);
    }

    #[test]
    fn find_sample_count_missing_falls_back() {
        assert_eq!(find_sample_count_index(&[1, 2, 4], 8), 2);
    }

    #[test]
    fn find_sample_count_one_returns_zero() {
        assert_eq!(find_sample_count_index(&[1], 8), 0);
    }

    #[test]
    fn find_sample_count_exact_match_8x() {
        assert_eq!(find_sample_count_index(&[1, 2, 4, 8], 8), 3);
    }
}
