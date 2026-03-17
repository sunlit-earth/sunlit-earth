use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::scene::camera::CameraParams;

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
            diffuse_floor: 0.50,
            diffuse_ramp: 0.25,
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
        eprintln!("Warning: could not determine config directory");
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
            eprintln!("Warning: could not read config file {}: {e}", path.display());
            return AppConfig::default();
        }
    };
    match toml::from_str(&contents) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Warning: could not parse config file {}: {e}", path.display());
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
        eprintln!("Warning: could not determine config directory; config not saved");
        return;
    };
    save_config_to(config, &path);
}

/// Save config to a specific path (used by both the public API and tests).
fn save_config_to(config: &AppConfig, path: &std::path::Path) {
    let toml_str = match toml::to_string_pretty(config) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Warning: could not serialize config: {e}");
            return;
        }
    };

    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        eprintln!("Warning: could not create config directory {}: {e}", parent.display());
        return;
    }

    let tmp_path = path.with_extension("toml~");
    if let Err(e) = fs::write(&tmp_path, &toml_str) {
        eprintln!("Warning: could not write temporary config file {}: {e}", tmp_path.display());
        return;
    }

    if let Err(e) = fs::rename(&tmp_path, path) {
        eprintln!("Warning: could not rename config file to {}: {e}", path.display());
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
        assert_relative_eq!(config.diffuse_floor, 0.50);
        assert_relative_eq!(config.diffuse_ramp, 0.25);
    }

    #[test]
    fn default_values_rendering() {
        let config = AppConfig::default();
        assert_eq!(config.texture_index, 3);
        assert_eq!(config.sample_count, 8);
    }

    #[test]
    fn serde_round_trip() {
        let config = AppConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: AppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(config, parsed);
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

        fs::write(&path, "longitude = 99.0\n").unwrap();
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
