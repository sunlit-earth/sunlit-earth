//! UI callback registration and config-to-window bridge functions.

use slint::ComponentHandle;

use crate::camera_controls::{self, EaseField};
use crate::displays;
use crate::engine_client::EngineLink;
use crate::mouse_math;
use crate::{
    AtmosphereSettings, CameraOrientationSettings, CameraPositionSettings, CameraReset,
    CameraStick, CelestialSettings, CloudCoverSettings, CloudSettings, ColorCorrectionSettings,
    ColorText, DateTimeSettings, FramingSettings, HazeSettings, LightingSettings, MainWindow,
    MoonSettings, NightglowSettings, OverlayHints, OverlayStyle, ParsedColor, RayleighSettings,
    SceneSettings, SkySettings, StarSettings, SunSettings, SunriseBandSettings,
    TextureColorSettings,
};
use sunlit_core::color::Rgb8;
use sunlit_core::config::{self, AppConfig};
use sunlit_core::engine::EngineCommand;
use sunlit_core::params::{SceneParams, gamma_slider_to_value, gamma_value_to_slider};
use sunlit_core::scene::camera::{self, CameraParams, PRESETS};
use sunlit_core::scene::datetime;
use sunlit_core::scene::scene_time::SceneTime;
use sunlit_core::scene::sky::{self, SkyState};

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
        // The first move of a drag takes the camera over from eclipse mode,
        // and the drag carries on from the camera shown.
        crate::time_bar::leave_eclipse(&win);
        let mut position = win.get_camera_position();
        let gain = mouse_math::drag_gain(
            cursor_speed,
            mouse_math::coarse_drag_gain(position.zoom),
            mouse_math::fine_drag_gain(win.get_celestial().sky.fov, win.get_displayed_short_edge()),
        );
        // The Sun-fixed mode turns the globe through the longitude its time of
        // day stands for, so the drag keeps its gain, its tilt correction and
        // its wrap, and a later time is a drag to the east.
        let (longitude, latitude) = mouse_math::apply_globe_drag_at(
            held_longitude_of(&position),
            position.latitude,
            win.get_camera_orientation().tilt,
            dx,
            dy,
            gain,
        );
        set_held_longitude(&mut position, longitude);
        position.latitude = latitude;
        win.set_camera_position(position);
        engine.push_params(&win);
    });
}

/// The sky at the instant the window's scene is drawn at: the override's when
/// it is on, the system clock's otherwise. The engine draws within a tick of
/// this, which is a fifth of a thousandth of a degree of the Earth's turn.
fn sky_now(window: &MainWindow) -> SkyState {
    sky::compute_sky_state(&scene_time_of(&window.get_datetime()))
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
        eclipse_follow: false,
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

/// The longitude the globe drag and the orbit controls move: the camera's
/// own, or in the Sun-fixed mode the one its time of day stands for.
fn held_longitude_of(position: &CameraPositionSettings) -> f32 {
    if position.sun_fixed {
        camera::held_longitude(position.solar_time)
    } else {
        position.longitude
    }
}

/// Write a held longitude back to whichever field the mode reads.
fn set_held_longitude(position: &mut CameraPositionSettings, longitude: f32) {
    if position.sun_fixed {
        position.solar_time = camera::solar_time_of_held(longitude);
    } else {
        position.longitude = longitude;
    }
}

/// Where an eased camera value stands in the window now.
fn ease_value(window: &MainWindow, field: EaseField) -> f32 {
    let position = window.get_camera_position();
    let orientation = window.get_camera_orientation();
    let framing = window.get_framing();
    match field {
        EaseField::Longitude => held_longitude_of(&position),
        EaseField::Latitude => position.latitude,
        EaseField::Zoom => position.zoom,
        EaseField::Tilt => orientation.tilt,
        EaseField::Yaw => orientation.yaw,
        EaseField::Pitch => orientation.pitch,
        EaseField::OffsetX => framing.offset_x,
        EaseField::OffsetY => framing.offset_y,
    }
}

/// Put an eased camera value into the window.
fn set_ease_value(window: &MainWindow, field: EaseField, value: f32) {
    match field {
        EaseField::Longitude | EaseField::Latitude | EaseField::Zoom => {
            let mut position = window.get_camera_position();
            match field {
                EaseField::Longitude => set_held_longitude(&mut position, value),
                EaseField::Latitude => position.latitude = value,
                _ => position.zoom = value,
            }
            window.set_camera_position(position);
        }
        EaseField::Tilt | EaseField::Yaw | EaseField::Pitch => {
            let mut orientation = window.get_camera_orientation();
            match field {
                EaseField::Tilt => orientation.tilt = value,
                EaseField::Yaw => orientation.yaw = value,
                _ => orientation.pitch = value,
            }
            window.set_camera_orientation(orientation);
        }
        EaseField::OffsetX | EaseField::OffsetY => {
            let mut framing = window.get_framing();
            if field == EaseField::OffsetX {
                framing.offset_x = value;
            } else {
                framing.offset_y = value;
            }
            window.set_framing(framing);
        }
    }
}

/// The eases in progress, and which field the held longitude stood for when
/// its ease started.
#[derive(Default)]
struct CameraEases {
    runs: camera_controls::Eases,
    sun_fixed: bool,
}

type SharedEases = std::rc::Rc<std::cell::RefCell<CameraEases>>;

/// Start easing camera values toward their targets and let the ease timer run.
fn ease_to(window: &MainWindow, eases: &SharedEases, targets: &[(EaseField, f32)]) {
    let mut eases = eases.borrow_mut();
    for &(field, target) in targets {
        if field == EaseField::Longitude {
            eases.sun_fixed = window.get_camera_position().sun_fixed;
        }
        eases.runs.start(field, ease_value(window, field), target);
    }
    window.set_camera_easing(eases.runs.is_running());
}

/// Stop easing camera values that a control has taken over.
fn cancel_eases(eases: &SharedEases, fields: &[EaseField]) {
    let mut eases = eases.borrow_mut();
    for &field in fields {
        eases.runs.cancel(field);
    }
}

/// Move the eases on by a tick and write where they are, after letting go
/// of every field something else wrote since the last tick: a drag, the
/// wheel, a sidebar slider, a preset, or the Sun-fixed switch, which also
/// changes what the held longitude means.
fn advance_eases(window: &MainWindow, eases: &SharedEases, seconds: f32) {
    let values = {
        let mut eases = eases.borrow_mut();
        if window.get_camera_position().sun_fixed != eases.sun_fixed {
            eases.runs.cancel(EaseField::Longitude);
        }
        eases.runs.release_taken(|field| ease_value(window, field));
        eases.runs.advance(seconds)
    };
    for &(field, value) in &values {
        set_ease_value(window, field, value);
    }
    let mut eases = eases.borrow_mut();
    for &(field, _) in &values {
        eases.runs.left(field, ease_value(window, field));
    }
    window.set_camera_easing(eases.runs.is_running());
}

/// Move the camera by one report of a stick on the panel or of held keys.
fn apply_camera_stick(
    window: &MainWindow,
    eases: &SharedEases,
    stick: CameraStick,
    x: f32,
    y: f32,
    seconds: f32,
) {
    match stick {
        CameraStick::Orbit => {
            cancel_eases(eases, &[EaseField::Longitude, EaseField::Latitude]);
            crate::time_bar::leave_eclipse(window);
            let mut position = window.get_camera_position();
            let (longitude, latitude) = camera_controls::orbit_by_stick(
                held_longitude_of(&position),
                position.latitude,
                window.get_camera_orientation().tilt,
                position.zoom,
                x,
                y,
                seconds,
            );
            set_held_longitude(&mut position, longitude);
            position.latitude = latitude;
            window.set_camera_position(position);
        }
        CameraStick::Look => {
            cancel_eases(eases, &[EaseField::Yaw, EaseField::Pitch]);
            let mut orientation = window.get_camera_orientation();
            (orientation.yaw, orientation.pitch) =
                camera_controls::look_by_stick(orientation.yaw, orientation.pitch, x, y, seconds);
            window.set_camera_orientation(orientation);
        }
        CameraStick::Frame => {
            cancel_eases(eases, &[EaseField::OffsetX, EaseField::OffsetY]);
            let mut framing = window.get_framing();
            (framing.offset_x, framing.offset_y) =
                camera_controls::frame_by_stick(framing.offset_x, framing.offset_y, x, y, seconds);
            window.set_framing(framing);
        }
    }
}

/// Where a ↺ or the N on the camera panel takes the values it resets: north
/// up, or the defaults.
fn reset_targets(defaults: &SceneSettings, which: CameraReset) -> Vec<(EaseField, f32)> {
    match which {
        CameraReset::North => vec![(EaseField::Tilt, 0.0)],
        CameraReset::Look => vec![
            (EaseField::Yaw, defaults.camera_orientation.yaw),
            (EaseField::Pitch, defaults.camera_orientation.pitch),
        ],
        CameraReset::Frame => vec![
            (EaseField::OffsetX, defaults.framing.offset_x),
            (EaseField::OffsetY, defaults.framing.offset_y),
        ],
    }
}

/// Register the camera panel over the preview: the three sticks, the orbit
/// arrows, the roll ring and its N, the zoom buttons, the ↺ badges, and the
/// timer that carries the eases.
///
/// The sticks and the held keys report the seconds since their previous
/// report, so the camera moves by the time that passed rather than by the
/// number of ticks.
fn register_camera_panel(window: &MainWindow, link: &EngineLink) {
    window
        .global::<OverlayHints>()
        .set_is_macos(cfg!(target_os = "macos"));
    window
        .global::<OverlayStyle>()
        .set_grab_cursors(!cfg!(target_os = "windows"));
    let eases = SharedEases::default();

    let window_weak = window.as_weak();
    let engine = link.clone();
    let stick_eases = std::rc::Rc::clone(&eases);
    window.on_camera_stick(move |stick, x, y, seconds| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        apply_camera_stick(&win, &stick_eases, stick, x, y, seconds);
        engine.push_params(&win);
    });

    let window_weak = window.as_weak();
    let step_eases = std::rc::Rc::clone(&eases);
    window.on_camera_orbit_step(move |right, up| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        crate::time_bar::leave_eclipse(&win);
        let position = win.get_camera_position();
        let (longitude, latitude) = camera_controls::orbit_step(
            held_longitude_of(&position),
            position.latitude,
            win.get_camera_orientation().tilt,
            right,
            up,
        );
        ease_to(
            &win,
            &step_eases,
            &[
                (EaseField::Longitude, longitude),
                (EaseField::Latitude, latitude),
            ],
        );
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    let roll_eases = std::rc::Rc::clone(&eases);
    window.on_camera_roll(move |degrees| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        cancel_eases(&roll_eases, &[EaseField::Tilt]);
        let mut orientation = win.get_camera_orientation();
        orientation.tilt = camera_controls::roll_by(orientation.tilt, degrees);
        win.set_camera_orientation(orientation);
        engine.push_params(&win);
    });

    let window_weak = window.as_weak();
    let reset_eases = std::rc::Rc::clone(&eases);
    window.on_camera_reset(move |which| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let targets = reset_targets(&win.get_defaults(), which);
        ease_to(&win, &reset_eases, &targets);
    });

    let window_weak = window.as_weak();
    let zoom_step_eases = std::rc::Rc::clone(&eases);
    window.on_camera_zoom_step(move |direction| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let target = camera_controls::zoom_step(win.get_camera_position().zoom, direction);
        ease_to(&win, &zoom_step_eases, &[(EaseField::Zoom, target)]);
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    let zoom_rate_eases = std::rc::Rc::clone(&eases);
    window.on_camera_zoom_rate(move |direction, seconds| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        cancel_eases(&zoom_rate_eases, &[EaseField::Zoom]);
        let mut position = win.get_camera_position();
        position.zoom = camera_controls::zoom_by_rate(position.zoom, direction, seconds);
        win.set_camera_position(position);
        engine.push_params(&win);
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_camera_ease_tick(move |seconds| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        advance_eases(&win, &eases, seconds);
        engine.push_params(&win);
    });
}

/// Register mouse interaction callbacks: globe drag (left), frame drag
/// (right), orient drag (middle), tilt drag (left and right together), scroll
/// zoom, and preset application, and the camera panel's controls that do the
/// same.
pub fn register_mouse_callbacks(window: &MainWindow, link: &EngineLink) {
    register_globe_drag(window, link);
    register_camera_panel(window, link);

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
        crate::time_bar::leave_eclipse(&win);
        apply_preset(&win, preset);
        engine.push_params(&win);
    });
}

/// Register slider and combo box change callbacks. Each one pushes the whole
/// scene to the engine, which decides whether anything actually changed.
pub fn register_change_callbacks(window: &MainWindow, link: &EngineLink) {
    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_sliders_changed(move || {
        if let Some(win) = window_weak.upgrade() {
            update_datetime_labels(&win);
            engine.push_params(&win);
        }
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_datetime_override_toggled(move || {
        if let Some(win) = window_weak.upgrade() {
            if win.get_datetime().use_custom {
                let now = time::OffsetDateTime::now_utc();
                set_fixed_time(&win, now.replace_nanosecond(0).unwrap_or(now));
            } else {
                crate::time_bar::leave_eclipse(&win);
            }
            update_datetime_labels(&win);
            engine.push_params(&win);
        }
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_datetime_edited(move || {
        if let Some(win) = window_weak.upgrade() {
            let settings = win.get_datetime();
            let utc = win.get_time_zone_utc();
            let at = instant_of_sliders(settings.hour, settings.day_of_year, settings.year, |at| {
                display_offset(utc, at)
            });
            crate::time_bar::leave_eclipse_for_a_new_day(&win, at);
            set_fixed_time(&win, at);
            update_datetime_labels(&win);
            engine.push_params(&win);
        }
    });

    let window_weak = window.as_weak();
    let engine = link.clone();
    window.on_camera_frame_toggled(move || {
        if let Some(win) = window_weak.upgrade() {
            // The switch has moved by the time this runs, so eclipse mode is
            // left into the field the mode it moved from reads, which the
            // conversion then carries across.
            let switched_to = win.get_camera_position().sun_fixed;
            crate::time_bar::leave_eclipse_as(&win, !switched_to);
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

/// A color as the window holds it.
pub fn slint_color(color: Rgb8) -> slint::Color {
    slint::Color::from_rgb_u8(color.r, color.g, color.b)
}

/// A color the window holds, without its alpha, which no color row sets.
pub fn rgb8(color: slint::Color) -> Rgb8 {
    Rgb8::new(color.red(), color.green(), color.blue())
}

/// Give the window's `ColorText` its two functions, `Rgb8`'s own, so the
/// panel writes and reads a color's hex text as the config file does. Until
/// this runs, a color row shows no text and accepts none.
pub fn register_color_text(window: &MainWindow) {
    let text = window.global::<ColorText>();
    text.on_format(|color| rgb8(color).to_hex().into());
    text.on_parse(|text| {
        Rgb8::parse_hex(&text).map_or_else(ParsedColor::default, |color| ParsedColor {
            valid: true,
            color: slint_color(color),
        })
    });
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
    window.set_time_zone_utc(config.time_zone == config::TimeZoneChoice::Utc);
    apply_params_to_window(window, &SceneParams::from_config(config));
    update_solar_time_label(window);

    window.set_auto_refresh_enabled(config.auto_refresh_enabled);
    window.set_overlays_hidden(config.overlays_hidden);
    window.set_time_bar_pinned(config.time_bar_pinned);
    #[expect(
        clippy::cast_precision_loss,
        reason = "an interval in minutes is far inside the f32 mantissa"
    )]
    window.set_auto_refresh_interval(config.auto_refresh_interval_minutes as f32);

    update_datetime_labels(window);
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
            eclipse_follow: params.eclipse_follow,
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
        datetime: datetime_settings(params.time, |_| time::UtcOffset::UTC),
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
                shadow: params.moon_shadow,
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
                ocean: slint_color(params.day_ocean_color),
                gamma: gamma_value_to_slider(params.day_gamma),
                saturation: params.day_saturation,
            },
            night: TextureColorSettings {
                ocean: slint_color(params.night_ocean_color),
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
    refresh_datetime_sliders(window);
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
        eclipse_follow: position.eclipse_follow,
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
        moon_shadow: celestial.moon.shadow,
        milky_way_intensity: celestial.sky.milky_way,
        day_ocean_color: rgb8(color.day.ocean),
        night_ocean_color: rgb8(color.night.ocean),
        day_gamma: gamma_slider_to_value(color.day.gamma),
        day_saturation: color.day.saturation,
        night_gamma: gamma_slider_to_value(color.night.gamma),
        night_saturation: color.night.saturation,
        time: scene_time_of(&date_time),
    }
}

/// The window's date and time as the input for astronomical computations.
///
/// This is the thin UI-reading layer; the engine turns the result into a
/// `scene::sky::SkyState` once per frame, which is where the sun direction,
/// the sky rotation and the planets all come from.
///
/// The instant the group holds is the setting, so a push reads back exactly
/// what was written; the sliders only show it. A group whose instant is
/// missing, which nothing writes, falls back to its sliders in UTC.
pub(crate) fn scene_time_of(settings: &DateTimeSettings) -> SceneTime {
    if !settings.use_custom {
        return SceneTime::Live;
    }
    match SceneTime::from_config_string(&settings.instant) {
        Some(fixed @ SceneTime::Fixed(_)) => fixed,
        _ => SceneTime::fixed(instant_of_sliders(
            settings.hour,
            settings.day_of_year,
            settings.year,
            |_| time::UtcOffset::UTC,
        )),
    }
}

/// The offset the window shows `at` in: UTC, or the system's offset in force
/// at that instant, summer time included. A system that cannot say shows UTC.
#[must_use]
pub fn display_offset(utc: bool, at: time::OffsetDateTime) -> time::UtcOffset {
    if utc {
        time::UtcOffset::UTC
    } else {
        time::UtcOffset::local_offset_at(at).unwrap_or(time::UtcOffset::UTC)
    }
}

/// An instant as the Date / Time group's Time, Day and Year, in the zone
/// `offset_at` gives for it.
#[must_use]
pub fn sliders_of(
    at: time::OffsetDateTime,
    offset_at: impl Fn(time::OffsetDateTime) -> time::UtcOffset,
) -> (f32, f32, i32) {
    let local = at.to_offset(offset_at(at));
    #[expect(
        clippy::cast_precision_loss,
        reason = "nanoseconds of one second, far inside the mantissa"
    )]
    let second = f32::from(local.second()) + local.nanosecond() as f32 / 1e9;
    let hour = f32::from(local.hour()) + f32::from(local.minute()) / 60.0 + second / 3600.0;
    (hour, f32::from(local.ordinal()), local.year())
}

/// The instant the Date / Time group's Time, Day and Year name, read in the
/// zone `offset_at` gives, to the second.
///
/// The year is held to 1900 to 2100 and the day to the year's own. A wall
/// time that a summer time change repeats is its first occurrence, and one
/// the change skips is read with the offset before it, which lands after
/// the gap.
#[must_use]
pub fn instant_of_sliders(
    hour: f32,
    day_of_year: f32,
    year: i32,
    offset_at: impl Fn(time::OffsetDateTime) -> time::UtcOffset,
) -> time::OffsetDateTime {
    let year = year.clamp(1900, 2100);
    let day = slider_u16(day_of_year).clamp(1, datetime::days_in_year(year));
    let date = time::Date::from_ordinal_date(year, day).unwrap_or(time::Date::MIN);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "seconds of one day, rounded"
    )]
    let seconds = (f64::from(hour.clamp(0.0, 24.0)) * 3600.0)
        .round()
        .min(86_399.0) as i64;
    let wall = date.midnight() + time::Duration::seconds(seconds);
    let before = wall.assume_offset(offset_at(wall.assume_utc() - time::Duration::days(1)));
    let after = wall.assume_offset(offset_at(wall.assume_utc() + time::Duration::days(1)));
    let valid = |at: &time::OffsetDateTime| offset_at(*at) == at.offset();
    let at = [before, after]
        .into_iter()
        .filter(valid)
        .min()
        .unwrap_or(before);
    sunlit_core::scene::scene_time::clamp_instant(at)
}

/// The time setting as the Date / Time group holds it: the switch, the
/// instant, and the sliders in the zone `offset_at` gives. Live time leaves
/// the sliders at noon on the first of January of this year.
fn datetime_settings(
    scene_time: SceneTime,
    offset_at: impl Fn(time::OffsetDateTime) -> time::UtcOffset,
) -> DateTimeSettings {
    match scene_time {
        SceneTime::Live => DateTimeSettings {
            use_custom: false,
            instant: slint::SharedString::new(),
            hour: 12.0,
            day_of_year: 1.0,
            year: time::OffsetDateTime::now_utc().year(),
        },
        SceneTime::Fixed(at) => {
            let (hour, day_of_year, year) = sliders_of(at, offset_at);
            DateTimeSettings {
                use_custom: true,
                instant: scene_time.to_config_string().into(),
                hour,
                day_of_year,
                year,
            }
        }
    }
}

/// Fix the time setting at `at` and show it in the group's sliders.
pub(crate) fn set_fixed_time(window: &MainWindow, at: time::OffsetDateTime) {
    let utc = window.get_time_zone_utc();
    window.set_datetime(datetime_settings(SceneTime::fixed(at), |at| {
        display_offset(utc, at)
    }));
}

/// Show the group's instant in its sliders in the displayed zone, after the
/// instant or the zone changed.
pub(crate) fn refresh_datetime_sliders(window: &MainWindow) {
    if let SceneTime::Fixed(at) = scene_time_of(&window.get_datetime()) {
        set_fixed_time(window, at);
    }
    update_datetime_labels(window);
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
        overlays_hidden: window.get_overlays_hidden(),
        time_bar_pinned: window.get_time_bar_pinned(),
        time_zone: if window.get_time_zone_utc() {
            config::TimeZoneChoice::Utc
        } else {
            config::TimeZoneChoice::Local
        },
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

/// Update the Time and Day labels and the Day slider's range from the
/// group's instant, in the displayed zone: "16:23 UTC+2", "Sun 18 Oct".
pub(crate) fn update_datetime_labels(window: &MainWindow) {
    let settings = window.get_datetime();
    let SceneTime::Fixed(at) = scene_time_of(&settings) else {
        return;
    };
    let local = at.to_offset(display_offset(window.get_time_zone_utc(), at));
    window.set_hour_label(
        format!(
            "{} {}",
            datetime::hour_label(settings.hour),
            datetime::zone_label(local.offset())
        )
        .into(),
    );
    window.set_day_label(datetime::day_label(local.date()).into());
    window.set_max_day_of_year(i32::from(datetime::days_in_year(local.year())));
}

#[cfg(test)]
mod tests {
    use time::macros::{datetime, offset};
    use time::{OffsetDateTime, UtcOffset};

    use super::*;

    /// Central European time as the system would answer it: UTC+2 from 01:00
    /// UTC on the last Sunday of March to 01:00 UTC on the last Sunday of
    /// October, in 2024 and 2026, and UTC+1 otherwise.
    fn berlin(at: OffsetDateTime) -> UtcOffset {
        let summers = [
            (
                datetime!(2024-03-31 01:00 UTC),
                datetime!(2024-10-27 01:00 UTC),
            ),
            (
                datetime!(2026-03-29 01:00 UTC),
                datetime!(2026-10-25 01:00 UTC),
            ),
        ];
        if summers.iter().any(|(from, to)| *from <= at && at < *to) {
            offset!(+2)
        } else {
            offset!(+1)
        }
    }

    fn utc(_: OffsetDateTime) -> UtcOffset {
        UtcOffset::UTC
    }

    /// Instants across a leap day and on either side of both summer time
    /// changes come back from the sliders as they went in, in either zone.
    #[test]
    fn the_sliders_round_trip_an_instant_in_local_time_and_utc() {
        for at in [
            datetime!(2024-02-29 23:30 UTC),
            datetime!(2024-03-01 00:15 UTC),
            datetime!(2024-12-31 23:59 UTC),
            datetime!(2026-03-29 00:59 UTC),
            datetime!(2026-03-29 01:00 UTC),
            datetime!(2026-10-25 00:00 UTC),
            datetime!(2026-10-25 02:00 UTC),
            datetime!(2026-10-18 14:23:45 UTC),
        ] {
            for (name, zone) in [
                ("berlin", berlin as fn(OffsetDateTime) -> UtcOffset),
                ("utc", utc),
            ] {
                let (hour, day, year) = sliders_of(at, zone);
                assert_eq!(
                    instant_of_sliders(hour, day, year, zone),
                    at,
                    "{at} in {name}"
                );
            }
        }
    }

    /// The sliders read in the zone: the last half hour of 2024 in UTC is the
    /// first of 2025 in Berlin, and summer time moves the hour by two.
    #[test]
    fn the_sliders_read_the_instant_in_the_displayed_zone() {
        let (hour, day, year) = sliders_of(datetime!(2024-12-31 23:30 UTC), berlin);
        assert_eq!((day, year), (1.0, 2025));
        approx::assert_relative_eq!(hour, 0.5);
        let (hour, _, _) = sliders_of(datetime!(2026-07-01 12:00 UTC), berlin);
        approx::assert_relative_eq!(hour, 14.0);
        let (hour, day, year) = sliders_of(datetime!(2024-02-29 12:00 UTC), utc);
        assert_eq!((day, year), (60.0, 2024));
        approx::assert_relative_eq!(hour, 12.0);
    }

    /// The hour the autumn change repeats reads as its first occurrence, and
    /// the hour the spring change skips lands after the gap.
    #[test]
    fn a_wall_time_a_summer_time_change_repeats_or_skips_has_one_instant() {
        let day = |at: OffsetDateTime| f32::from(at.ordinal());
        assert_eq!(
            instant_of_sliders(2.5, day(datetime!(2026-10-25 00:00 UTC)), 2026, berlin),
            datetime!(2026-10-25 00:30 UTC)
        );
        assert_eq!(
            instant_of_sliders(2.5, day(datetime!(2026-03-29 00:00 UTC)), 2026, berlin),
            datetime!(2026-03-29 01:30 UTC)
        );
    }

    /// A day past the year's last and a year past 2100 are held to the range.
    #[test]
    fn the_sliders_hold_the_day_and_the_year_to_their_range() {
        assert_eq!(
            instant_of_sliders(0.0, 366.0, 2025, utc),
            datetime!(2025-12-31 00:00 UTC)
        );
        assert_eq!(
            instant_of_sliders(12.0, 1.0, 2300, utc),
            datetime!(2100-01-01 12:00 UTC)
        );
    }

    /// The group holds the instant itself, so what it pushes is exactly what
    /// was written, whatever the sliders' rounding.
    #[test]
    fn the_group_reads_back_exactly_the_instant_it_holds() {
        let at = datetime!(2026-10-18 14:23:45.123 UTC);
        let settings = datetime_settings(SceneTime::fixed(at), berlin);
        assert_eq!(scene_time_of(&settings), SceneTime::Fixed(at));
        let live = datetime_settings(SceneTime::Live, berlin);
        assert_eq!(scene_time_of(&live), SceneTime::Live);
    }
}
