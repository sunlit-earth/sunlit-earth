//! UI callback registration and config-to-window bridge functions.

use slint::ComponentHandle;

use crate::MainWindow;
use crate::displays;
use crate::engine_client::EngineLink;
use crate::mouse_math;
use sunlit_core::config::{self, AppConfig};
use sunlit_core::engine::EngineCommand;
use sunlit_core::params::{SceneParams, gamma_slider_to_value, gamma_value_to_slider};
use sunlit_core::scene::camera::{CameraParams, PRESETS};
use sunlit_core::scene::datetime;
use sunlit_core::scene::sun::DateTimeInput;

/// Register the left-drag: rotate the globe, tilt-corrected, at a gain that
/// follows the hand.
///
/// The estimator and the moment of the previous move are this callback's
/// alone: no other drag reads the cursor's speed, and the interval between two
/// moves is the only thing the toolkit does not hand over. They are shared with
/// the press, which clears both, because they describe one gesture: without
/// that, a deliberate drag begun a few tens of milliseconds after a sweep
/// inherits the sweep's speed and takes its first steps at the coarse gain.
fn register_globe_drag(window: &MainWindow, link: &EngineLink) {
    let window_weak = window.as_weak();
    let engine = link.clone();
    let speed = std::rc::Rc::new(std::cell::Cell::new(mouse_math::DragSpeed::default()));
    let previous_move = std::rc::Rc::new(std::cell::Cell::new(None::<std::time::Instant>));

    let pressed_speed = std::rc::Rc::clone(&speed);
    let pressed_move = std::rc::Rc::clone(&previous_move);
    window.on_mouse_drag_globe_begin(move || {
        pressed_speed.set(mouse_math::DragSpeed::default());
        pressed_move.set(None);
    });

    window.on_mouse_drag_globe(move |dx, dy| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let now = std::time::Instant::now();
        let seconds = previous_move
            .replace(Some(now))
            .map_or(mouse_math::DRAG_SPEED_MAX_INTERVAL, |previous| {
                now.duration_since(previous).as_secs_f32()
            });
        let mut estimator = speed.get();
        let cursor_speed = estimator.observe(dx.hypot(dy), seconds);
        speed.set(estimator);
        let gain = mouse_math::drag_gain(
            cursor_speed,
            mouse_math::coarse_drag_gain(win.get_camera_zoom()),
            mouse_math::fine_drag_gain(win.get_sky_fov(), win.get_viewport_width()),
        );
        let (lon, lat) = mouse_math::apply_globe_drag_at(
            win.get_camera_longitude(),
            win.get_camera_latitude(),
            win.get_camera_tilt(),
            dx,
            dy,
            gain,
        );
        win.set_camera_longitude(lon);
        win.set_camera_latitude(lat);
        engine.push_params(&win);
    });
}

/// Register mouse interaction callbacks: globe drag (left), frame drag
/// (right), orient drag (middle), tilt drag (left and right together), scroll
/// zoom, and preset application.
pub fn register_mouse_callbacks(window: &MainWindow, link: &EngineLink) {
    register_globe_drag(window, link);

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

    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_mouse_drag_orient(move |dx, dy| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let (new_yaw, new_pitch) =
            mouse_math::apply_orient_drag(win.get_camera_yaw(), win.get_camera_pitch(), dx, dy);
        win.set_camera_yaw(new_yaw);
        win.set_camera_pitch(new_pitch);
        engine.push_params(&win);
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_mouse_drag_tilt(move |dx, _dy| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        win.set_camera_tilt(mouse_math::apply_tilt_drag(win.get_camera_tilt(), dx));
        engine.push_params(&win);
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_mouse_scroll(move |delta| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        win.set_camera_zoom(mouse_math::apply_zoom_scroll(win.get_camera_zoom(), delta));
        engine.push_params(&win);
    });

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
        win.set_camera_fov(preset.fov_deg);
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

    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_datetime_override_toggled(move || {
        if let Some(win) = window_weak.upgrade() {
            if win.get_use_custom_datetime() {
                let now = time::OffsetDateTime::now_utc();
                let hour = f32::from(now.hour())
                    + f32::from(now.minute()) / 60.0
                    + f32::from(now.second()) / 3600.0;
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

    // The resolution is the one control that does not go through `push_params`:
    // it decides which pixels to load rather than what to draw.
    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_texture_resolution_changed(move || {
        if let Some(win) = window_weak.upgrade() {
            engine.set_resolution_is_one_run_only(false);
            engine.send(EngineCommand::SetTextureResolution(
                config::texture_resolution_at(win.get_texture_resolution_index()),
            ));
        }
    });
}

/// Register action callbacks: set wallpaper, load defaults, reset.
///
/// `screens` is the shared monitor list the Displays group is built from, read
/// rather than copied so a layout change reaches these callbacks too.
/// Load-defaults and reset both move the display plan, and the engine has to be
/// told: it holds the mode and the anchor of its own, exactly as it holds the
/// texture resolution.
pub fn register_action_callbacks(
    window: &MainWindow,
    link: &EngineLink,
    screens: &displays::SharedMonitors,
) {
    {
        let window_weak = window.as_weak();
        let engine = link.clone();
        window.on_set_wallpaper(move || {
            let Some(win) = window_weak.upgrade() else {
                return;
            };
            config::save_config(&read_config_from_window(&win, &engine));
            engine.push_params(&win);
            engine.send(EngineCommand::RenderWallpaperNow);
        });
    }

    let window_weak = window.as_weak();
    let engine = link.clone();
    let for_defaults = std::sync::Arc::clone(screens);
    window.on_load_defaults(move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let defaults = AppConfig::default();
        apply_config_to_window(&win, &defaults);

        defer_combobox_indices(
            &win.as_weak(),
            ComboIndices::of(
                &defaults,
                engine.aa_counts(),
                defaults.texture_resolution,
                &displays::screen_ids_of_window(&win),
            ),
        );
        displays::apply_diagram_to_window(
            &win,
            &displays::monitors_of(&for_defaults),
            defaults.anchor().as_deref(),
        );
        engine.set_resolution_is_one_run_only(false);
        engine.send(EngineCommand::SetTextureResolution(
            defaults.texture_resolution,
        ));
        engine.send(EngineCommand::SetDisplayPlan {
            mode: defaults.display_mode,
            anchor: defaults.anchor(),
        });
        engine.push_params(&win);
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    let for_reset = std::sync::Arc::clone(screens);
    window.on_reset(move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let loaded = config::load_config();
        apply_config_to_window(&win, &loaded);

        defer_combobox_indices(
            &win.as_weak(),
            ComboIndices::of(
                &loaded,
                engine.aa_counts(),
                loaded.texture_resolution,
                &displays::screen_ids_of_window(&win),
            ),
        );
        displays::apply_diagram_to_window(
            &win,
            &displays::monitors_of(&for_reset),
            loaded.anchor().as_deref(),
        );
        engine.set_resolution_is_one_run_only(false);
        engine.send(EngineCommand::SetTextureResolution(
            loaded.texture_resolution,
        ));
        engine.send(EngineCommand::SetDisplayPlan {
            mode: loaded.display_mode,
            anchor: loaded.anchor(),
        });
        engine.push_params(&win);
    });
}

/// Wire the Displays group: the mode, the anchor screen, and the diagram.
///
/// The monitor list is the one the group's rows were built from rather than a
/// fresh query, so a row and a rectangle never name different screens. A change
/// is persisted at once, the way the auto-refresh checkbox is: it is a setting
/// somebody chose rather than a slider they are still moving.
pub fn register_display_callbacks(
    window: &MainWindow,
    link: &EngineLink,
    screens: &displays::SharedMonitors,
) {
    let window_weak = window.as_weak();
    let engine = link.clone();
    let screens = std::sync::Arc::clone(screens);
    window.on_display_plan_changed(move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let anchor = displays::anchor_from_window(&win).filter(|id| !id.is_empty());
        displays::apply_diagram_to_window(
            &win,
            &displays::monitors_of(&screens),
            anchor.as_deref(),
        );
        engine.send(EngineCommand::SetDisplayPlan {
            mode: displays::mode_from_window(&win),
            anchor,
        });
        config::save_config(&read_config_from_window(&win, &engine));
    });
}

/// Every `ComboBox` index a config decides.
#[derive(Clone, Copy)]
pub struct ComboIndices {
    pub aa: i32,
    pub texture: i32,
    pub texture_resolution: i32,
    pub display_mode: i32,
    pub display_anchor: i32,
}

impl ComboIndices {
    /// The rows a config asks for.
    ///
    /// `texture_resolution` is passed rather than read off the config because
    /// the engine may have started at a width `--texture-resolution` chose.
    /// `screen_ids` is what the window's screen combo offers, which is where a
    /// stored anchor id becomes a row number, and an id this session does not
    /// have lands on the automatic row.
    pub fn of(
        config: &AppConfig,
        aa_counts: &[u32],
        texture_resolution: u32,
        screen_ids: &[String],
    ) -> Self {
        Self {
            aa: config::find_sample_count_index(aa_counts, config.sample_count),
            texture: config.texture_index,
            texture_resolution: config::find_texture_resolution_index(texture_resolution),
            display_mode: i32::try_from(config.display_mode.index()).unwrap_or_default(),
            display_anchor: displays::anchor_index(screen_ids, &config.anchor_monitor),
        }
    }

    /// The rows the window is on right now.
    ///
    /// What a caller that is changing one combo needs, so the deferred write
    /// puts every other combo back where it already was instead of reading a
    /// config the window may be ahead of.
    pub fn of_window(window: &MainWindow) -> Self {
        Self {
            aa: window.get_aa_index(),
            texture: window.get_texture_index(),
            texture_resolution: window.get_texture_resolution_index(),
            display_mode: window.get_display_mode_index(),
            display_anchor: window.get_display_anchor_index(),
        }
    }
}

/// Defer setting `ComboBox` indices so they apply after Slint processes model
/// changes. This replaces four identical copies of the same pattern.
pub fn defer_combobox_indices(window_weak: &slint::Weak<MainWindow>, indices: ComboIndices) {
    let weak = window_weak.clone();
    slint::invoke_from_event_loop(move || {
        if let Some(win) = weak.upgrade() {
            win.set_aa_index(indices.aa);
            win.set_texture_index(indices.texture);
            win.set_texture_resolution_index(indices.texture_resolution);
            win.set_display_mode_index(indices.display_mode);
            win.set_display_anchor_index(indices.display_anchor);
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

    window.set_auto_refresh_enabled(config.auto_refresh_enabled);
    #[allow(clippy::cast_precision_loss)] // interval_minutes fits in f32 mantissa
    window.set_auto_refresh_interval(config.auto_refresh_interval_minutes as f32);

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
    window.set_camera_fov(cam.fov_deg);
    window.set_terminator_width(params.terminator_width);
    window.set_diffuse_shading(params.diffuse_shading);
    window.set_diffuse_floor(params.diffuse_floor);
    window.set_diffuse_ramp(params.diffuse_ramp);
    window.set_spec_shininess(params.spec_shininess);
    window.set_spec_intensity(params.spec_intensity);
    window.set_fresnel_mix(params.fresnel_mix);
    window.set_fresnel_exp(params.fresnel_exp);
    window.set_cloud_opacity(params.cloud_opacity);
    window.set_cloud_opacity_night(params.cloud_opacity_night);
    window.set_cloud_floor(params.cloud_floor);
    window.set_cloud_gamma(params.cloud_gamma);
    window.set_cloud_night(params.cloud_night);
    window.set_atmo_enabled(params.atmo_enabled);
    window.set_rayleigh_intensity(params.rayleigh_intensity);
    window.set_rayleigh_sharpness(params.rayleigh_sharpness);
    window.set_rayleigh_haze(params.rayleigh_haze);
    window.set_nightglow_intensity(params.nightglow_intensity);
    window.set_nightglow_falloff(params.nightglow_falloff);
    window.set_nightglow_balance(params.nightglow_balance);
    window.set_atmo_sunrise_glow(params.atmo_sunrise_glow);
    window.set_atmo_sunrise_width(params.atmo_sunrise_width);
    window.set_sky_fov(params.sky_fov);
    window.set_star_intensity(params.star_intensity);
    window.set_star_size(params.star_size);
    window.set_star_glow_strength(params.star_glow_strength);
    window.set_star_glow_radius(params.star_glow_radius);
    window.set_star_contrast(params.star_contrast);
    window.set_star_mag_limit(params.star_mag_limit);
    window.set_sun_glow(params.sun_glow);
    window.set_sun_rays(params.sun_rays);
    window.set_sun_flare(params.sun_flare);
    window.set_sun_size(params.sun_size);
    window.set_sun_halo_radius(params.sun_halo_radius);
    window.set_sun_horizon_boost(params.sun_horizon_boost);
    window.set_sun_horizon_reach(params.sun_horizon_reach);
    window.set_sun_horizon_depth(params.sun_horizon_depth);
    window.set_sun_reddening(params.sun_reddening);
    window.set_sun_refraction(params.sun_refraction);
    window.set_moon_brightness(params.moon_brightness);
    window.set_moon_size(params.moon_size);
    window.set_moon_earthshine(params.moon_earthshine);
    window.set_milky_way_intensity(params.milky_way_intensity);
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
            fov_deg: window.get_camera_fov(),
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
        cloud_opacity_night: window.get_cloud_opacity_night(),
        cloud_floor: window.get_cloud_floor(),
        cloud_gamma: window.get_cloud_gamma(),
        cloud_night: window.get_cloud_night(),
        atmo_enabled: window.get_atmo_enabled(),
        rayleigh_intensity: window.get_rayleigh_intensity(),
        rayleigh_sharpness: window.get_rayleigh_sharpness(),
        rayleigh_haze: window.get_rayleigh_haze(),
        nightglow_intensity: window.get_nightglow_intensity(),
        nightglow_falloff: window.get_nightglow_falloff(),
        nightglow_balance: window.get_nightglow_balance(),
        atmo_sunrise_glow: window.get_atmo_sunrise_glow(),
        atmo_sunrise_width: window.get_atmo_sunrise_width(),
        sky_fov: window.get_sky_fov(),
        star_intensity: window.get_star_intensity(),
        star_size: window.get_star_size(),
        star_glow_strength: window.get_star_glow_strength(),
        star_glow_radius: window.get_star_glow_radius(),
        star_contrast: window.get_star_contrast(),
        star_mag_limit: window.get_star_mag_limit(),
        sun_glow: window.get_sun_glow(),
        sun_rays: window.get_sun_rays(),
        sun_flare: window.get_sun_flare(),
        sun_size: window.get_sun_size(),
        sun_halo_radius: window.get_sun_halo_radius(),
        sun_horizon_boost: window.get_sun_horizon_boost(),
        sun_horizon_reach: window.get_sun_horizon_reach(),
        sun_horizon_depth: window.get_sun_horizon_depth(),
        sun_reddening: window.get_sun_reddening(),
        sun_refraction: window.get_sun_refraction(),
        moon_brightness: window.get_moon_brightness(),
        moon_size: window.get_moon_size(),
        moon_earthshine: window.get_moon_earthshine(),
        milky_way_intensity: window.get_milky_way_intensity(),
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
/// This is the thin UI-reading layer; the engine turns the result into a
/// `scene::sky::SkyState` once per frame, which is where the sun direction,
/// the sky rotation and the planets all come from.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn read_datetime_input(window: &MainWindow) -> DateTimeInput {
    DateTimeInput {
        use_custom: window.get_use_custom_datetime(),
        custom_hour: window.get_custom_hour(),
        custom_day_of_year: window.get_custom_day_of_year() as u16,
        custom_year: window.get_custom_year_index() + datetime::base_year(),
    }
}

/// Read all persisted settings from the window's current UI state.
///
/// This is a read-modify-write against what is on disk, not a fresh
/// `AppConfig::default()`. Not every persisted setting has a widget: the
/// quality tier does not, and starting from the defaults meant that clicking
/// "Set as Wallpaper" in a debug build wrote `quality_tier = "low"` over a
/// release install's `"high"`, permanently. Anything the UI does not manage
/// has to survive a save untouched, and the only way to guarantee that as
/// fields are added is to start from the stored config rather than enumerate
/// what to preserve.
///
/// Reading from disk also means a `--quality` override is not persisted, which
/// is the intended behavior for a per-run flag. `--texture-resolution` does
/// have a widget, so keeping it out of the file takes the flag `link` carries.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub fn read_config_from_window(window: &MainWindow, link: &EngineLink) -> AppConfig {
    read_config_from_window_onto(
        window,
        link.aa_counts(),
        &config::load_config(),
        link.resolution_is_one_run_only(),
    )
}

/// The testable half of [`read_config_from_window`]: overwrite the UI-managed
/// fields of `stored` and leave everything else alone.
///
/// `keep_stored_resolution` is the one exception to reading the window: with it
/// set, the width in the window is a one-run override and the stored one wins.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub fn read_config_from_window_onto(
    window: &MainWindow,
    aa_counts: &[u32],
    stored: &AppConfig,
    keep_stored_resolution: bool,
) -> AppConfig {
    let pos = window.window().position();
    let size = window.window().size();

    let texture_resolution = if keep_stored_resolution {
        stored.texture_resolution
    } else {
        config::texture_resolution_at(window.get_texture_resolution_index())
    };

    let mut config = AppConfig {
        auto_refresh_enabled: window.get_auto_refresh_enabled(),
        auto_refresh_interval_minutes: window.get_auto_refresh_interval() as u32,
        texture_resolution,
        display_mode: displays::mode_from_window(window),
        anchor_monitor: displays::anchor_to_store(window, &stored.anchor_monitor),
        window_x: Some(pos.x),
        window_y: Some(pos.y),
        window_width: Some(size.width),
        window_height: Some(size.height),
        ..stored.clone()
    };
    read_params_from_window(window, aa_counts).write_to_config(&mut config);
    config
}

/// Update the hour label, day label, and max-day-of-year on the window
/// based on the current datetime slider values.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn update_datetime_labels(window: &MainWindow, base_year: i32) {
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
