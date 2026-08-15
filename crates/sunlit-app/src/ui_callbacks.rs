//! UI callback registration and config-to-window bridge functions.
//!
//! Groups all Slint callback registrations by category and provides helper
//! functions for applying/reading config to/from the window.

use slint::ComponentHandle;

use crate::MainWindow;
use crate::engine_client::EngineLink;
use crate::mouse_math;
use sunlit_core::config::{self, AppConfig};
use sunlit_core::engine::EngineCommand;
use sunlit_core::params::{SceneParams, gamma_slider_to_value, gamma_value_to_slider};
use sunlit_core::scene::camera::{CameraParams, PRESETS};
use sunlit_core::scene::datetime;
use sunlit_core::scene::sun::DateTimeInput;

/// Register mouse interaction callbacks: globe drag, frame drag, orient drag,
/// tilt drag, scroll zoom, and preset application.
pub fn register_mouse_callbacks(window: &MainWindow, link: &EngineLink) {
    // Left-drag callback: rotate the globe (tilt-corrected)
    let window_weak = window.as_weak();
    let engine = link.clone();
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
        engine.push_params(&win);
    });

    // Right-drag callback: adjust framing (offset X/Y)
    let window_weak = window.as_weak();
    let engine = link.clone();
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
        engine.push_params(&win);
    });

    // Middle-drag callback: adjust pitch and yaw
    let window_weak = window.as_weak();
    let engine = link.clone();
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
        engine.push_params(&win);
    });

    // Left+right drag callback: adjust tilt (horizontal only)
    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_mouse_drag_tilt(move |dx, _dy| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        win.set_camera_tilt(mouse_math::apply_tilt_drag(win.get_camera_tilt(), dx));
        engine.push_params(&win);
    });

    // Mouse scroll callback: zoom in/out
    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_mouse_scroll(move |delta| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        win.set_camera_zoom(mouse_math::apply_zoom_scroll(win.get_camera_zoom(), delta));
        engine.push_params(&win);
    });

    // Apply-preset callback: set camera parameters from the PRESETS array
    let window_weak = window.as_weak();
    let engine = link.clone();
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
        engine.push_params(&win);
    });
}

/// Register slider and combo box change callbacks. Each one pushes the whole
/// scene to the engine, which decides whether anything actually changed.
pub fn register_change_callbacks(window: &MainWindow, base_year: i32, link: &EngineLink) {
    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_sliders_changed(move || {
        if let Some(win) = window_weak.upgrade() {
            update_datetime_labels(&win, base_year);
            engine.push_params(&win);
        }
    });

    // When "Override date/time" is toggled on, initialize sliders to current UTC time
    let window_weak = window.as_weak();
    let engine = link.clone();
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
            engine.push_params(&win);
        }
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_msaa_changed(move || {
        if let Some(win) = window_weak.upgrade() {
            engine.push_params(&win);
        }
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_texture_changed(move || {
        if let Some(win) = window_weak.upgrade() {
            engine.push_params(&win);
        }
    });
}

/// Register action callbacks: set wallpaper, load defaults, reset.
pub fn register_action_callbacks(window: &MainWindow, link: &EngineLink) {
    // "Set as Wallpaper" button: save config, then ask the engine to export.
    {
        let window_weak = window.as_weak();
        let engine = link.clone();
        window.on_set_wallpaper(move || {
            let Some(win) = window_weak.upgrade() else {
                return;
            };
            config::save_config(&read_config_from_window(&win, engine.aa_counts()));
            engine.push_params(&win);
            engine.send(EngineCommand::RenderWallpaperNow);
        });
    }

    // Load-defaults callback: restore all settings to AppConfig::default() without saving
    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_load_defaults(move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let defaults = AppConfig::default();
        apply_config_to_window(&win, &defaults);

        let default_aa_index =
            config::find_sample_count_index(engine.aa_counts(), defaults.sample_count);
        defer_combobox_indices(&win.as_weak(), default_aa_index, defaults.texture_index);
        engine.push_params(&win);
    });

    // Reset callback: reload config from disk and restore UI to last-saved state
    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_reset(move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let loaded = config::load_config();
        apply_config_to_window(&win, &loaded);

        let loaded_aa_index =
            config::find_sample_count_index(engine.aa_counts(), loaded.sample_count);
        defer_combobox_indices(&win.as_weak(), loaded_aa_index, loaded.texture_index);
        engine.push_params(&win);
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
    apply_params_to_window(window, &SceneParams::from_config(config));

    // Auto-refresh
    window.set_auto_refresh_enabled(config.auto_refresh_enabled);
    #[allow(clippy::cast_precision_loss)] // interval_minutes fits in f32 mantissa
    window.set_auto_refresh_interval(config.auto_refresh_interval_minutes as f32);

    // Update display labels
    update_datetime_labels(window, datetime::base_year());
}

/// Write scene parameters into the window's properties.
///
/// The texture and AA combo boxes are deliberately left alone: their indices
/// are applied through `defer_combobox_indices` so they land after Slint has
/// processed the model changes.
pub fn apply_params_to_window(window: &MainWindow, params: &SceneParams) {
    let cam = &params.camera;
    window.set_camera_longitude(cam.longitude);
    window.set_camera_latitude(cam.latitude);
    window.set_camera_zoom(cam.zoom);
    window.set_camera_offset_x(cam.offset_x);
    window.set_camera_offset_y(cam.offset_y);
    window.set_camera_tilt(cam.tilt_deg);
    window.set_camera_yaw(cam.yaw_deg);
    window.set_camera_pitch(cam.pitch_deg);
    window.set_terminator_width(params.terminator_width);
    window.set_diffuse_shading(params.diffuse_shading);
    window.set_diffuse_floor(params.diffuse_floor);
    window.set_diffuse_ramp(params.diffuse_ramp);
    window.set_spec_shininess(params.spec_shininess);
    window.set_spec_intensity(params.spec_intensity);
    window.set_fresnel_mix(params.fresnel_mix);
    window.set_fresnel_exp(params.fresnel_exp);
    window.set_cloud_opacity(params.cloud_opacity);
    window.set_cloud_floor(params.cloud_floor);
    window.set_cloud_gamma(params.cloud_gamma);
    window.set_atmo_enabled(params.atmo_enabled);
    window.set_rayleigh_intensity(params.rayleigh_intensity);
    window.set_rayleigh_sharpness(params.rayleigh_sharpness);
    window.set_rayleigh_haze(params.rayleigh_haze);
    window.set_nightglow_intensity(params.nightglow_intensity);
    window.set_nightglow_falloff(params.nightglow_falloff);
    window.set_nightglow_balance(params.nightglow_balance);
    window.set_day_gamma(gamma_value_to_slider(params.day_gamma));
    window.set_day_saturation(params.day_saturation);
    window.set_night_gamma(gamma_value_to_slider(params.night_gamma));
    window.set_night_saturation(params.night_saturation);

    window.set_use_custom_datetime(params.datetime.use_custom);
    window.set_custom_hour(params.datetime.custom_hour);
    window.set_custom_day_of_year(f32::from(params.datetime.custom_day_of_year));
    let year_index = (params.datetime.custom_year - datetime::base_year()).clamp(0, 20);
    window.set_custom_year_index(year_index);
}

/// Read the window's properties into the single scene parameter struct.
///
/// This is one of the two translation points for `SceneParams` (the other is
/// the uniform encoder in the renderer). `aa_counts` maps the AA combo box
/// index to an MSAA sample count.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub fn read_params_from_window(window: &MainWindow, aa_counts: &[u32]) -> SceneParams {
    let aa_index = window.get_aa_index().max(0) as usize;
    SceneParams {
        camera: CameraParams {
            longitude: window.get_camera_longitude(),
            latitude: window.get_camera_latitude(),
            zoom: window.get_camera_zoom(),
            offset_x: window.get_camera_offset_x(),
            offset_y: window.get_camera_offset_y(),
            tilt_deg: window.get_camera_tilt(),
            yaw_deg: window.get_camera_yaw(),
            pitch_deg: window.get_camera_pitch(),
        },
        texture_index: window.get_texture_index(),
        sample_count: aa_counts.get(aa_index).copied().unwrap_or(1),
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
        datetime: read_datetime_input(window),
    }
}

/// Extract datetime parameters from the Slint window for astronomical
/// computations.
///
/// This is the thin UI-reading layer; the actual computation lives in
/// `scene::sun::compute_sun_direction()`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn read_datetime_input(window: &MainWindow) -> DateTimeInput {
    DateTimeInput {
        use_custom: window.get_use_custom_datetime(),
        custom_hour: window.get_custom_hour(),
        custom_day_of_year: window.get_custom_day_of_year() as u16,
        custom_year: window.get_custom_year_index() + datetime::base_year(),
    }
}

/// Read all persisted settings from the window's current UI state.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub fn read_config_from_window(window: &MainWindow, aa_counts: &[u32]) -> AppConfig {
    let pos = window.window().position();
    let size = window.window().size();

    let mut config = AppConfig {
        auto_refresh_enabled: window.get_auto_refresh_enabled(),
        auto_refresh_interval_minutes: window.get_auto_refresh_interval() as u32,
        window_x: Some(pos.x),
        window_y: Some(pos.y),
        window_width: Some(size.width),
        window_height: Some(size.height),
        ..AppConfig::default()
    };
    read_params_from_window(window, aa_counts).write_to_config(&mut config);
    config
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
