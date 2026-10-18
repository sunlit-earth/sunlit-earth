//! UI callback registration and config-to-window bridge functions.

use slint::ComponentHandle;

use crate::displays;
use crate::engine_client::EngineLink;
use crate::mouse_math;
use crate::{
    AtmosphereSettings, CameraOrientationSettings, CameraPositionSettings, CelestialSettings,
    CloudCoverSettings, CloudSettings, ColorCorrectionSettings, DateTimeSettings, FramingSettings,
    HazeSettings, LightingSettings, MainWindow, MoonSettings, NightglowSettings, RayleighSettings,
    SceneSettings, SkySettings, StarSettings, SunSettings, SunriseBandSettings,
    TextureColorSettings,
};
use sunlit_core::config::{self, AppConfig};
use sunlit_core::engine::EngineCommand;
use sunlit_core::params::{SceneParams, gamma_slider_to_value, gamma_value_to_slider};
use sunlit_core::scene::camera::{self, CameraParams, PRESETS};
use sunlit_core::scene::datetime;
use sunlit_core::scene::sky::{self, SkyState};
use sunlit_core::scene::sun::DateTimeInput;

/// A Slint index as a position in the list it selects from.
///
/// A negative index is one no list has, which is what `get` already answers
/// `None` to, so the conversion needs no separate guard.
pub(crate) fn slider_index(value: i32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// A slider's float as the whole number a setting stores.
///
/// Rust's float-to-integer casts saturate, so a value outside the range lands
/// on an end of it rather than wrapping.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the one place a slider value becomes a count"
)]
pub(crate) fn slider_u32(value: f32) -> u32 {
    value as u32
}

/// The same for the one setting that is stored narrower, the day of the year.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the one place a slider value becomes a day of the year"
)]
pub(crate) fn slider_u16(value: f32) -> u16 {
    value as u16
}

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
        let mut position = win.get_camera_position();
        let gain = mouse_math::drag_gain(
            cursor_speed,
            mouse_math::coarse_drag_gain(position.zoom),
            mouse_math::fine_drag_gain(win.get_celestial().sky.fov, win.get_viewport_width()),
        );
        // The Sun-fixed mode turns the globe through the longitude its time of
        // day stands for, so the drag keeps its gain, its tilt correction and
        // its wrap, and a later time is a drag to the east.
        let held = if position.sun_fixed {
            camera::held_longitude(position.solar_time)
        } else {
            position.longitude
        };
        let (longitude, latitude) = mouse_math::apply_globe_drag_at(
            held,
            position.latitude,
            win.get_camera_orientation().tilt,
            dx,
            dy,
            gain,
        );
        if position.sun_fixed {
            position.solar_time = camera::solar_time_of_held(longitude);
        } else {
            position.longitude = longitude;
        }
        position.latitude = latitude;
        win.set_camera_position(position);
        engine.push_params(&win);
    });
}

/// The sky at the instant the window's scene is drawn at: the override's when
/// it is on, the system clock's otherwise. The engine draws within a tick of
/// this, which is a fifth of a thousandth of a degree of the Earth's turn.
fn sky_now(window: &MainWindow) -> SkyState {
    sky::compute_sky_state(&datetime_input(&window.get_datetime()))
}

/// Carry the view across a switch of the camera mode: the value the mode now
/// reads is converted from the one it read before, at the instant the window
/// draws, so the frame before and after the switch is the same.
pub fn convert_camera_frame(window: &MainWindow) {
    let sun = sky_now(window).sun_direction;
    let mut position = window.get_camera_position();
    if position.sun_fixed {
        position.solar_time = camera::solar_time_of(position.longitude, sun);
    } else {
        position.longitude = camera::longitude_of(position.solar_time, sun);
    }
    window.set_camera_position(position);
}

/// Put a preset's position, orientation and framing into the window.
///
/// In the Sun-fixed mode the mode stays on and the preset's place is put
/// beneath the camera as of now, through the time of day; the longitude is the
/// other mode's and keeps what it held.
pub fn apply_preset(window: &MainWindow, preset: &CameraParams) {
    let current = window.get_camera_position();
    let (longitude, solar_time) = if current.sun_fixed {
        let sun = sky_now(window).sun_direction;
        (
            current.longitude,
            camera::solar_time_of(preset.longitude, sun),
        )
    } else {
        (preset.longitude, current.solar_time)
    };
    window.set_camera_position(CameraPositionSettings {
        longitude,
        latitude: preset.latitude,
        zoom: preset.zoom,
        sun_fixed: current.sun_fixed,
        solar_time,
    });
    window.set_camera_orientation(CameraOrientationSettings {
        tilt: preset.tilt_deg,
        yaw: preset.yaw_deg,
        pitch: preset.pitch_deg,
    });
    window.set_framing(FramingSettings {
        fov: preset.fov_deg,
        offset_x: preset.offset_x,
        offset_y: preset.offset_y,
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
        let mut framing = win.get_framing();
        (framing.offset_x, framing.offset_y) = mouse_math::apply_frame_drag(
            framing.offset_x,
            framing.offset_y,
            win.get_camera_position().zoom,
            dx,
            dy,
        );
        win.set_framing(framing);
        engine.push_params(&win);
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_mouse_drag_orient(move |dx, dy| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let mut orientation = win.get_camera_orientation();
        (orientation.yaw, orientation.pitch) =
            mouse_math::apply_orient_drag(orientation.yaw, orientation.pitch, dx, dy);
        win.set_camera_orientation(orientation);
        engine.push_params(&win);
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_mouse_drag_tilt(move |dx, _dy| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let mut orientation = win.get_camera_orientation();
        orientation.tilt = mouse_math::apply_tilt_drag(orientation.tilt, dx);
        win.set_camera_orientation(orientation);
        engine.push_params(&win);
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_mouse_scroll(move |delta| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let mut position = win.get_camera_position();
        position.zoom = mouse_math::apply_zoom_scroll(position.zoom, delta);
        win.set_camera_position(position);
        engine.push_params(&win);
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_apply_preset(move |index| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let Some(preset) = PRESETS.get(slider_index(index)) else {
            return;
        };
        apply_preset(&win, preset);
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
            let date_time = win.get_datetime();
            if date_time.use_custom {
                let now = time::OffsetDateTime::now_utc();
                win.set_datetime(DateTimeSettings {
                    hour: f32::from(now.hour())
                        + f32::from(now.minute()) / 60.0
                        + f32::from(now.second()) / 3600.0,
                    day_of_year: f32::from(now.ordinal()),
                    year_index: now.year() - base_year,
                    ..date_time
                });
            }
            update_datetime_labels(&win, base_year);
            engine.push_params(&win);
        }
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_camera_frame_toggled(move || {
        if let Some(win) = window_weak.upgrade() {
            convert_camera_frame(&win);
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
        apply_whole_config(&win, &engine, &for_defaults, &AppConfig::default());
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    let for_reset = std::sync::Arc::clone(screens);
    window.on_reset(move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        apply_whole_config(&win, &engine, &for_reset, &config::load_config());
    });
}

/// Put a whole config into the window, the engine and the diagram at once.
///
/// Load-defaults and reset differ only in where the config comes from. Both
/// replace every setting rather than one, which is why the texture resolution
/// and the display plan are sent explicitly: neither travels in `SceneParams`,
/// and the engine holds its own copy of each.
fn apply_whole_config(
    window: &MainWindow,
    engine: &EngineLink,
    screens: &displays::SharedMonitors,
    config: &AppConfig,
) {
    apply_config_to_window(window, config);

    set_combobox_indices(
        &window.as_weak(),
        ComboIndices::of(
            config,
            engine.aa_counts(),
            config.texture_resolution,
            &displays::screen_ids_of_window(window),
        ),
    );
    displays::apply_diagram_to_window(
        window,
        &displays::monitors_of(screens),
        config.anchor().as_deref(),
    );
    engine.set_resolution_is_one_run_only(false);
    engine.send(EngineCommand::SetTextureResolution(
        config.texture_resolution,
    ));
    engine.send(EngineCommand::SetDisplayPlan {
        mode: config.display_mode,
        anchor: config.anchor(),
    });
    engine.push_params(window);
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
    /// What a caller that is changing one combo needs, so the second write
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

    fn apply_to(self, window: &MainWindow) {
        window.set_aa_index(self.aa);
        window.set_texture_index(self.texture);
        window.set_texture_resolution_index(self.texture_resolution);
        window.set_display_mode_index(self.display_mode);
        window.set_display_anchor_index(self.display_anchor);
    }
}

/// Set the `ComboBox` indices now, and again once Slint has processed the
/// model changes made in this turn, which can move them.
///
/// The second write is the one that sticks. The first is what a push of the
/// window's scene in the same turn reads, as the startup's and load-defaults'
/// do: the sample count and the texture mode are combo rows.
pub fn set_combobox_indices(window_weak: &slint::Weak<MainWindow>, indices: ComboIndices) {
    if let Some(win) = window_weak.upgrade() {
        indices.apply_to(&win);
    }
    let weak = window_weak.clone();
    slint::invoke_from_event_loop(move || {
        if let Some(win) = weak.upgrade() {
            indices.apply_to(&win);
            win.window().request_redraw();
        }
    })
    .ok();
}

/// Apply a loaded config to all window properties except window geometry.
///
/// Called at startup, on reset, and on load-defaults. Window geometry is
/// applied only at startup so that reset/load-defaults don't move or resize
/// the window. The `texture_index` and `aa_index` are set through
/// `set_combobox_indices` instead, so they are not set here.
///
/// Every path that fills the window comes through here, so this is also where
/// the window learns what its resets go back to: `defaults` is written from
/// `AppConfig::default()`, whichever config the values come from, and no other
/// place in the app or in `main.slint` states a default.
pub fn apply_config_to_window(window: &MainWindow, config: &AppConfig) {
    window.set_defaults(scene_settings(&SceneParams::default()));
    apply_params_to_window(window, &SceneParams::from_config(config));
    update_solar_time_label(window);

    window.set_auto_refresh_enabled(config.auto_refresh_enabled);
    #[expect(
        clippy::cast_precision_loss,
        reason = "an interval in minutes is far inside the f32 mantissa"
    )]
    window.set_auto_refresh_interval(config.auto_refresh_interval_minutes as f32);

    update_datetime_labels(window, datetime::base_year());
}

/// The scene as the window holds it: one struct per group of the Advanced
/// section, nested the way its subsections are.
///
/// This is the one way a `SceneSettings` is built. Slint derives `Default` for
/// the structs it generates, so `SceneSettings::default()` is all zeros rather
/// than the app's defaults, which are `scene_settings(&SceneParams::default())`.
/// Each struct is written out whole, so a field added on the Slint side does
/// not compile until it is filled here.
#[expect(clippy::too_many_lines, reason = "the length is one setting per line")]
pub fn scene_settings(params: &SceneParams) -> SceneSettings {
    let cam = &params.camera;
    SceneSettings {
        camera_position: CameraPositionSettings {
            longitude: cam.longitude,
            latitude: cam.latitude,
            zoom: cam.zoom,
            sun_fixed: params.sun_fixed,
            solar_time: params.solar_time,
        },
        camera_orientation: CameraOrientationSettings {
            tilt: cam.tilt_deg,
            yaw: cam.yaw_deg,
            pitch: cam.pitch_deg,
        },
        framing: FramingSettings {
            fov: cam.fov_deg,
            offset_x: cam.offset_x,
            offset_y: cam.offset_y,
        },
        datetime: DateTimeSettings {
            use_custom: params.datetime.use_custom,
            hour: params.datetime.custom_hour,
            day_of_year: f32::from(params.datetime.custom_day_of_year),
            year_index: (params.datetime.custom_year - datetime::base_year()).clamp(0, 20),
        },
        clouds: CloudSettings {
            enabled: params.clouds_enabled,
            opacity: params.cloud_opacity,
            opacity_night: params.cloud_opacity_night,
            night_brightness: params.cloud_night,
            haze: HazeSettings {
                equator: params.cloud_haze_equator,
                pole: params.cloud_haze_pole,
                latitude: params.cloud_haze_latitude,
                band: params.cloud_haze_band,
            },
            land: CloudCoverSettings {
                floor: params.cloud_floor_land,
                gamma: params.cloud_gamma_land,
            },
            ocean: CloudCoverSettings {
                floor: params.cloud_floor_ocean,
                gamma: params.cloud_gamma_ocean,
            },
        },
        celestial: CelestialSettings {
            sky: SkySettings {
                fov: params.sky_fov,
                milky_way: params.milky_way_intensity,
            },
            sun: SunSettings {
                glow: params.sun_glow,
                rays: params.sun_rays,
                size: params.sun_size,
                halo_radius: params.sun_halo_radius,
                flare: params.sun_flare,
                horizon_boost: params.sun_horizon_boost,
                horizon_reach: params.sun_horizon_reach,
                horizon_depth: params.sun_horizon_depth,
                reddening: params.sun_reddening,
                refraction: params.sun_refraction,
            },
            moon: MoonSettings {
                brightness: params.moon_brightness,
                size: params.moon_size,
                earthshine: params.moon_earthshine,
            },
            stars: StarSettings {
                intensity: params.star_intensity,
                size: params.star_size,
                glow_strength: params.star_glow_strength,
                glow_radius: params.star_glow_radius,
                contrast: params.star_contrast,
                mag_limit: params.star_mag_limit,
            },
        },
        atmosphere: AtmosphereSettings {
            enabled: params.atmo_enabled,
            rayleigh: RayleighSettings {
                intensity: params.rayleigh_intensity,
                sharpness: params.rayleigh_sharpness,
                haze: params.rayleigh_haze,
            },
            sunrise: SunriseBandSettings {
                glow: params.atmo_sunrise_glow,
                width: params.atmo_sunrise_width,
            },
            nightglow: NightglowSettings {
                intensity: params.nightglow_intensity,
                balance: params.nightglow_balance,
                falloff: params.nightglow_falloff,
            },
        },
        lighting: LightingSettings {
            terminator_width: params.terminator_width,
            diffuse_shading: params.diffuse_shading,
            diffuse_floor: params.diffuse_floor,
            diffuse_ramp: params.diffuse_ramp,
            spec_shininess: params.spec_shininess,
            spec_intensity: params.spec_intensity,
            fresnel_mix: params.fresnel_mix,
            fresnel_exp: params.fresnel_exp,
        },
        color_correction: ColorCorrectionSettings {
            day: TextureColorSettings {
                gamma: gamma_value_to_slider(params.day_gamma),
                saturation: params.day_saturation,
            },
            night: TextureColorSettings {
                gamma: gamma_value_to_slider(params.night_gamma),
                saturation: params.night_saturation,
            },
        },
    }
}

/// Write scene parameters into the window's properties.
///
/// The texture and AA combo boxes are deliberately left alone: their indices
/// are applied through `set_combobox_indices`, whose second write lands after
/// Slint has processed the model changes. The destructuring names every group,
/// so a group added to `SceneSettings` does not compile until it is written.
pub fn apply_params_to_window(window: &MainWindow, params: &SceneParams) {
    let SceneSettings {
        camera_position,
        camera_orientation,
        framing,
        datetime: date_time,
        clouds,
        celestial,
        atmosphere,
        lighting,
        color_correction,
    } = scene_settings(params);
    window.set_camera_position(camera_position);
    window.set_camera_orientation(camera_orientation);
    window.set_framing(framing);
    window.set_datetime(date_time);
    window.set_clouds(clouds);
    window.set_celestial(celestial);
    window.set_atmosphere(atmosphere);
    window.set_lighting(lighting);
    window.set_color_correction(color_correction);
}

/// Every group the window holds, read in one literal so that a group added
/// to `SceneSettings` does not compile until it is read.
fn settings_from_window(window: &MainWindow) -> SceneSettings {
    SceneSettings {
        camera_position: window.get_camera_position(),
        camera_orientation: window.get_camera_orientation(),
        framing: window.get_framing(),
        datetime: window.get_datetime(),
        clouds: window.get_clouds(),
        celestial: window.get_celestial(),
        atmosphere: window.get_atmosphere(),
        lighting: window.get_lighting(),
        color_correction: window.get_color_correction(),
    }
}

/// Read the window's properties into the single scene parameter struct.
///
/// This is one of the two translation points for `SceneParams` (the other is
/// the uniform encoder in the renderer). `aa_counts` maps the AA combo box
/// index to an MSAA sample count.
pub fn read_params_from_window(window: &MainWindow, aa_counts: &[u32]) -> SceneParams {
    let aa_index = slider_index(window.get_aa_index().max(0));
    let SceneSettings {
        camera_position: position,
        camera_orientation: orientation,
        framing,
        datetime: date_time,
        clouds,
        celestial,
        atmosphere,
        lighting,
        color_correction: color,
    } = settings_from_window(window);
    SceneParams {
        camera: CameraParams {
            longitude: position.longitude,
            latitude: position.latitude,
            zoom: position.zoom,
            offset_x: framing.offset_x,
            offset_y: framing.offset_y,
            tilt_deg: orientation.tilt,
            yaw_deg: orientation.yaw,
            pitch_deg: orientation.pitch,
            fov_deg: framing.fov,
        },
        sun_fixed: position.sun_fixed,
        solar_time: position.solar_time,
        texture_index: window.get_texture_index(),
        sample_count: aa_counts.get(aa_index).copied().unwrap_or(1),
        terminator_width: lighting.terminator_width,
        diffuse_shading: lighting.diffuse_shading,
        diffuse_floor: lighting.diffuse_floor,
        diffuse_ramp: lighting.diffuse_ramp,
        spec_shininess: lighting.spec_shininess,
        spec_intensity: lighting.spec_intensity,
        fresnel_mix: lighting.fresnel_mix,
        fresnel_exp: lighting.fresnel_exp,
        clouds_enabled: clouds.enabled,
        cloud_opacity: clouds.opacity,
        cloud_opacity_night: clouds.opacity_night,
        cloud_haze_equator: clouds.haze.equator,
        cloud_haze_pole: clouds.haze.pole,
        cloud_haze_latitude: clouds.haze.latitude,
        cloud_haze_band: clouds.haze.band,
        cloud_floor_land: clouds.land.floor,
        cloud_gamma_land: clouds.land.gamma,
        cloud_floor_ocean: clouds.ocean.floor,
        cloud_gamma_ocean: clouds.ocean.gamma,
        cloud_night: clouds.night_brightness,
        atmo_enabled: atmosphere.enabled,
        rayleigh_intensity: atmosphere.rayleigh.intensity,
        rayleigh_sharpness: atmosphere.rayleigh.sharpness,
        rayleigh_haze: atmosphere.rayleigh.haze,
        nightglow_intensity: atmosphere.nightglow.intensity,
        nightglow_falloff: atmosphere.nightglow.falloff,
        nightglow_balance: atmosphere.nightglow.balance,
        atmo_sunrise_glow: atmosphere.sunrise.glow,
        atmo_sunrise_width: atmosphere.sunrise.width,
        sky_fov: celestial.sky.fov,
        star_intensity: celestial.stars.intensity,
        star_size: celestial.stars.size,
        star_glow_strength: celestial.stars.glow_strength,
        star_glow_radius: celestial.stars.glow_radius,
        star_contrast: celestial.stars.contrast,
        star_mag_limit: celestial.stars.mag_limit,
        sun_glow: celestial.sun.glow,
        sun_rays: celestial.sun.rays,
        sun_flare: celestial.sun.flare,
        sun_size: celestial.sun.size,
        sun_halo_radius: celestial.sun.halo_radius,
        sun_horizon_boost: celestial.sun.horizon_boost,
        sun_horizon_reach: celestial.sun.horizon_reach,
        sun_horizon_depth: celestial.sun.horizon_depth,
        sun_reddening: celestial.sun.reddening,
        sun_refraction: celestial.sun.refraction,
        moon_brightness: celestial.moon.brightness,
        moon_size: celestial.moon.size,
        moon_earthshine: celestial.moon.earthshine,
        milky_way_intensity: celestial.sky.milky_way,
        day_gamma: gamma_slider_to_value(color.day.gamma),
        day_saturation: color.day.saturation,
        night_gamma: gamma_slider_to_value(color.night.gamma),
        night_saturation: color.night.saturation,
        datetime: datetime_input(&date_time),
    }
}

/// The window's date and time as the input for astronomical computations.
///
/// This is the thin UI-reading layer; the engine turns the result into a
/// `scene::sky::SkyState` once per frame, which is where the sun direction,
/// the sky rotation and the planets all come from.
fn datetime_input(settings: &DateTimeSettings) -> DateTimeInput {
    DateTimeInput {
        use_custom: settings.use_custom,
        custom_hour: settings.hour,
        custom_day_of_year: slider_u16(settings.day_of_year),
        custom_year: settings.year_index + datetime::base_year(),
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
        auto_refresh_interval_minutes: slider_u32(window.get_auto_refresh_interval()),
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

/// Update the Time of day row's reading from the time it holds. The row's
/// right end is midnight again, and reads as it.
pub(crate) fn update_solar_time_label(window: &MainWindow) {
    let solar_time = window.get_camera_position().solar_time.rem_euclid(24.0);
    window.set_solar_time_label(datetime::hour_label(solar_time).into());
}

/// Update the hour label, day label, and max-day-of-year on the window
/// based on the current datetime slider values.
fn update_datetime_labels(window: &MainWindow, base_year: i32) {
    let date_time = window.get_datetime();
    window.set_hour_label(datetime::hour_label(date_time.hour).into());

    let year = date_time.year_index + base_year;
    let max_doy = datetime::days_in_year(year);
    window.set_max_day_of_year(i32::from(max_doy));

    // Clamp day-of-year if it exceeds the new max (e.g. leap -> non-leap)
    let day_of_year = date_time.day_of_year.min(f32::from(max_doy));
    let doy = slider_u16(day_of_year).max(1);
    window.set_day_label(datetime::month_day_label(doy, year).into());
    if day_of_year < date_time.day_of_year {
        window.set_datetime(DateTimeSettings {
            day_of_year,
            ..date_time
        });
    }
}
