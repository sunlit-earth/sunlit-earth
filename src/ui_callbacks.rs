//! UI callback registration and config-to-window bridge functions.
//!
//! Groups all Slint callback registrations by category and provides helper
//! functions for applying/reading config to/from the window.

use slint::ComponentHandle;
use tracing::info;

use crate::config::{self, AppConfig};
use crate::mouse_math;
use crate::renderer::{self, gamma_slider_to_value, gamma_value_to_slider};
use crate::scene::camera::PRESETS;
use crate::scene::datetime;
#[cfg(windows)]
use crate::wallpaper;
use crate::MainWindow;

/// Register mouse interaction callbacks: globe drag, frame drag, orient drag,
/// tilt drag, scroll zoom, and preset application.
pub fn register_mouse_callbacks(window: &MainWindow) {
    // Left-drag callback: rotate the globe (tilt-corrected)
    let window_weak = window.as_weak();
    window.on_mouse_drag_globe(move |dx, dy| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let (lon, lat) = mouse_math::apply_globe_drag(
            win.get_camera_longitude(),
            win.get_camera_latitude(),
            win.get_camera_tilt(),
            win.get_camera_zoom(),
            dx,
            dy,
        );
        win.set_camera_longitude(lon);
        win.set_camera_latitude(lat);
        win.window().request_redraw();
    });

    // Right-drag callback: adjust framing (offset X/Y)
    let window_weak = window.as_weak();
    window.on_mouse_drag_frame(move |dx, dy| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let (new_x, new_y) = mouse_math::apply_frame_drag(
            win.get_camera_offset_x(),
            win.get_camera_offset_y(),
            win.get_camera_zoom(),
            dx,
            dy,
        );
        win.set_camera_offset_x(new_x);
        win.set_camera_offset_y(new_y);
        win.window().request_redraw();
    });

    // Middle-drag callback: adjust pitch and yaw
    let window_weak = window.as_weak();
    window.on_mouse_drag_orient(move |dx, dy| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let (new_yaw, new_pitch) = mouse_math::apply_orient_drag(
            win.get_camera_yaw(),
            win.get_camera_pitch(),
            dx,
            dy,
        );
        win.set_camera_yaw(new_yaw);
        win.set_camera_pitch(new_pitch);
        win.window().request_redraw();
    });

    // Left+right drag callback: adjust tilt (horizontal only)
    let window_weak = window.as_weak();
    window.on_mouse_drag_tilt(move |dx, _dy| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        win.set_camera_tilt(mouse_math::apply_tilt_drag(win.get_camera_tilt(), dx));
        win.window().request_redraw();
    });

    // Mouse scroll callback: zoom in/out
    let window_weak = window.as_weak();
    window.on_mouse_scroll(move |delta| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        win.set_camera_zoom(mouse_math::apply_zoom_scroll(win.get_camera_zoom(), delta));
        win.window().request_redraw();
    });

    // Apply-preset callback: set camera parameters from the PRESETS array
    let window_weak = window.as_weak();
    #[allow(clippy::cast_sign_loss)]
    window.on_apply_preset(move |index| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let Some(preset) = PRESETS.get(index as usize) else {
            return;
        };
        win.set_camera_longitude(preset.longitude);
        win.set_camera_latitude(preset.latitude);
        win.set_camera_zoom(preset.zoom);
        win.set_camera_offset_x(preset.offset_x);
        win.set_camera_offset_y(preset.offset_y);
        win.set_camera_tilt(preset.tilt_deg);
        win.set_camera_yaw(preset.yaw_deg);
        win.set_camera_pitch(preset.pitch_deg);
        win.window().request_redraw();
    });
}

/// Register slider/combobox change callbacks that trigger redraws.
pub fn register_change_callbacks(window: &MainWindow, base_year: i32) {
    // Request a redraw whenever sliders change
    let window_weak = window.as_weak();
    window.on_sliders_changed(move || {
        if let Some(win) = window_weak.upgrade() {
            update_datetime_labels(&win, base_year);
            win.window().request_redraw();
        }
    });

    // When "Override date/time" is toggled on, initialize sliders to current UTC time
    let window_weak = window.as_weak();
    window.on_datetime_override_toggled(move || {
        if let Some(win) = window_weak.upgrade() {
            if win.get_use_custom_datetime() {
                let now = time::OffsetDateTime::now_utc();
                let hour =
                    f32::from(now.hour()) + f32::from(now.minute()) / 60.0 + f32::from(now.second()) / 3600.0;
                win.set_custom_hour(hour);
                win.set_custom_day_of_year(f32::from(now.ordinal()));
                win.set_custom_year_index(now.year() - base_year);
            }
            update_datetime_labels(&win, base_year);
            win.window().request_redraw();
        }
    });

    let window_weak = window.as_weak();
    window.on_msaa_changed(move || {
        if let Some(win) = window_weak.upgrade() {
            win.window().request_redraw();
        }
    });

    let window_weak = window.as_weak();
    window.on_texture_changed(move || {
        if let Some(win) = window_weak.upgrade() {
            win.window().request_redraw();
        }
    });
}

/// Register action callbacks: set wallpaper, load defaults, reset.
pub fn register_action_callbacks(window: &MainWindow, aa_counts: &[u32]) {
    // "Set as Wallpaper" button: save config, then apply wallpaper.
    {
        let window_weak = window.as_weak();
        let aa_counts_for_save = aa_counts.to_vec();
        window.on_set_wallpaper(move || {
            let Some(win) = window_weak.upgrade() else {
                return;
            };
            config::save_config(&read_config_from_window(&win, &aa_counts_for_save));
            #[cfg(windows)]
            if let Err(e) = do_set_wallpaper() {
                tracing::error!("Failed to set wallpaper: {e}");
            }
            #[cfg(not(windows))]
            tracing::warn!("Wallpaper export is not supported on this platform");
        });
    }

    // Load-defaults callback: restore all settings to AppConfig::default() without saving
    let window_weak = window.as_weak();
    let aa_counts_for_defaults = aa_counts.to_vec();
    window.on_load_defaults(move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let defaults = AppConfig::default();
        apply_config_to_window(&win, &defaults);

        let default_aa_index =
            config::find_sample_count_index(&aa_counts_for_defaults, defaults.sample_count);
        defer_combobox_indices(&win.as_weak(), default_aa_index, defaults.texture_index);
        win.window().request_redraw();
    });

    // Reset callback: reload config from disk and restore UI to last-saved state
    let window_weak = window.as_weak();
    let aa_counts_for_reset = aa_counts.to_vec();
    window.on_reset(move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let loaded = config::load_config();
        apply_config_to_window(&win, &loaded);

        let loaded_aa_index =
            config::find_sample_count_index(&aa_counts_for_reset, loaded.sample_count);
        defer_combobox_indices(&win.as_weak(), loaded_aa_index, loaded.texture_index);
        win.window().request_redraw();
    });
}

/// Defer setting `ComboBox` indices so they apply after Slint processes model
/// changes. This replaces three identical copies of the same pattern.
pub fn defer_combobox_indices(
    window_weak: &slint::Weak<MainWindow>,
    aa_index: i32,
    texture_index: i32,
) {
    let weak = window_weak.clone();
    slint::invoke_from_event_loop(move || {
        if let Some(win) = weak.upgrade() {
            win.set_aa_index(aa_index);
            win.set_texture_index(texture_index);
            win.window().request_redraw();
        }
    })
    .ok();
}

/// Apply a loaded config to all window properties except window geometry.
///
/// Called at startup, on reset, and on load-defaults. Window geometry is
/// applied only at startup so that reset/load-defaults don't move or resize
/// the window. The `texture_index` and `aa_index` are set via a deferred
/// `invoke_from_event_loop` instead, so they are not set here.
pub fn apply_config_to_window(window: &MainWindow, config: &AppConfig) {
    window.set_camera_longitude(config.longitude);
    window.set_camera_latitude(config.latitude);
    window.set_camera_zoom(config.zoom);
    window.set_camera_offset_x(config.offset_x);
    window.set_camera_offset_y(config.offset_y);
    window.set_camera_tilt(config.tilt);
    window.set_camera_yaw(config.yaw);
    window.set_camera_pitch(config.pitch);
    window.set_terminator_width(config.terminator_width);
    window.set_diffuse_shading(config.diffuse_shading);
    window.set_diffuse_floor(config.diffuse_floor);
    window.set_diffuse_ramp(config.diffuse_ramp);
    window.set_spec_shininess(config.spec_shininess);
    window.set_spec_intensity(config.spec_intensity);
    window.set_fresnel_mix(config.fresnel_mix);
    window.set_fresnel_exp(config.fresnel_exp);
    window.set_cloud_opacity(config.cloud_opacity);
    window.set_cloud_floor(config.cloud_floor);
    window.set_cloud_gamma(config.cloud_gamma);
    window.set_atmo_enabled(config.atmo_enabled);
    window.set_rayleigh_intensity(config.rayleigh_intensity);
    window.set_rayleigh_sharpness(config.rayleigh_sharpness);
    window.set_rayleigh_haze(config.rayleigh_haze);
    window.set_nightglow_intensity(config.nightglow_intensity);
    window.set_nightglow_falloff(config.nightglow_falloff);
    window.set_nightglow_balance(config.nightglow_balance);
    window.set_day_gamma(gamma_value_to_slider(config.day_gamma));
    window.set_day_saturation(config.day_saturation);
    window.set_night_gamma(gamma_value_to_slider(config.night_gamma));
    window.set_night_saturation(config.night_saturation);

    // Auto-refresh
    window.set_auto_refresh_enabled(config.auto_refresh_enabled);
    #[allow(clippy::cast_precision_loss)] // interval_minutes fits in f32 mantissa
    window.set_auto_refresh_interval(config.auto_refresh_interval_minutes as f32);

    // Custom datetime
    window.set_use_custom_datetime(config.use_custom_datetime);
    window.set_custom_hour(config.custom_hour);
    window.set_custom_day_of_year(config.custom_day_of_year);
    let base_year = datetime::base_year();
    let year_index = (config.custom_year - base_year).clamp(0, 20);
    window.set_custom_year_index(year_index);

    // Update display labels
    update_datetime_labels(window, base_year);
}

/// Read all persisted settings from the window's current UI state.
#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub fn read_config_from_window(window: &MainWindow, aa_counts: &[u32]) -> AppConfig {
    let aa_index = window.get_aa_index() as usize;
    let sample_count = aa_counts.get(aa_index).copied().unwrap_or(1);

    let pos = window.window().position();
    let size = window.window().size();

    AppConfig {
        longitude: window.get_camera_longitude(),
        latitude: window.get_camera_latitude(),
        zoom: window.get_camera_zoom(),
        tilt: window.get_camera_tilt(),
        yaw: window.get_camera_yaw(),
        pitch: window.get_camera_pitch(),
        offset_x: window.get_camera_offset_x(),
        offset_y: window.get_camera_offset_y(),
        texture_index: window.get_texture_index(),
        sample_count,
        terminator_width: window.get_terminator_width(),
        diffuse_shading: window.get_diffuse_shading(),
        diffuse_floor: window.get_diffuse_floor(),
        diffuse_ramp: window.get_diffuse_ramp(),
        spec_shininess: window.get_spec_shininess(),
        spec_intensity: window.get_spec_intensity(),
        fresnel_mix: window.get_fresnel_mix(),
        fresnel_exp: window.get_fresnel_exp(),
        cloud_opacity: window.get_cloud_opacity(),
        cloud_floor: window.get_cloud_floor(),
        cloud_gamma: window.get_cloud_gamma(),
        atmo_enabled: window.get_atmo_enabled(),
        rayleigh_intensity: window.get_rayleigh_intensity(),
        rayleigh_sharpness: window.get_rayleigh_sharpness(),
        rayleigh_haze: window.get_rayleigh_haze(),
        nightglow_intensity: window.get_nightglow_intensity(),
        nightglow_falloff: window.get_nightglow_falloff(),
        nightglow_balance: window.get_nightglow_balance(),
        day_gamma: gamma_slider_to_value(window.get_day_gamma()),
        day_saturation: window.get_day_saturation(),
        night_gamma: gamma_slider_to_value(window.get_night_gamma()),
        night_saturation: window.get_night_saturation(),
        auto_refresh_enabled: window.get_auto_refresh_enabled(),
        auto_refresh_interval_minutes: window.get_auto_refresh_interval() as u32,
        use_custom_datetime: window.get_use_custom_datetime(),
        custom_hour: window.get_custom_hour(),
        custom_day_of_year: window.get_custom_day_of_year(),
        custom_year: window.get_custom_year_index() + datetime::base_year(),
        window_x: Some(pos.x),
        window_y: Some(pos.y),
        window_width: Some(size.width),
        window_height: Some(size.height),
    }
}

/// Update the hour label, day label, and max-day-of-year on the window
/// based on the current datetime slider values.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn update_datetime_labels(window: &MainWindow, base_year: i32) {
    let h = window.get_custom_hour();
    window.set_hour_label(datetime::hour_label(h).into());

    let year = window.get_custom_year_index() + base_year;
    let max_doy = datetime::days_in_year(year);
    window.set_max_day_of_year(i32::from(max_doy));

    // Clamp day-of-year if it exceeds the new max (e.g. leap -> non-leap)
    let doy = window.get_custom_day_of_year();
    if doy > f32::from(max_doy) {
        window.set_custom_day_of_year(f32::from(max_doy));
    }

    let doy = window.get_custom_day_of_year() as u16;
    let doy = doy.max(1);
    window.set_day_label(datetime::month_day_label(doy, year).into());
}

/// Encode RGBA8 pixels as PNG and write to the given path.
pub fn save_render_png(
    path: &std::path::Path,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<(), String> {
    use image::{ImageBuffer, Rgba};
    let img: ImageBuffer<Rgba<u8>, _> =
        ImageBuffer::from_raw(width, height, pixels.to_vec())
            .ok_or_else(|| "pixel buffer size mismatch".to_owned())?;
    img.save(path).map_err(|e| format!("failed to save PNG: {e}"))
}

/// Render the current scene at the primary monitor's resolution, save as PNG,
/// and set it as the Windows desktop wallpaper.
#[cfg(windows)]
pub fn do_set_wallpaper() -> Result<(), String> {
    info!("starting wallpaper export");
    let (width, height) = wallpaper::get_primary_monitor_resolution()?;
    let pixels = renderer::export_wallpaper_image(width, height)?;
    let path = wallpaper::save_wallpaper_image(&pixels, width, height)?;
    wallpaper::set_wallpaper(&path)?;
    info!(path = %path.display(), "wallpaper set successfully");
    crate::memory::log_memory_usage("after wallpaper set");
    Ok(())
}
