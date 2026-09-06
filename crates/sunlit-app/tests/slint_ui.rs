//! Headless Slint UI integration tests.
//!
//! These tests exercise property bindings, callback wiring, conditional
//! visibility, preset application, and default restoration using the
//! `i-slint-backend-testing` crate. No GPU or display is required.

use std::cell::RefCell;
use std::rc::Rc;

use i_slint_backend_testing::ElementHandle;
use slint::ComponentHandle;
use sunlit_earth::MainWindow;

// ---------------------------------------------------------------------------
// Initialization boilerplate
// ---------------------------------------------------------------------------

/// Initialize the Slint testing backend for the current thread.
///
/// The Slint platform context is thread-local, so each test thread needs
/// its own initialization. With `threading: false`, no global event-loop
/// proxy is set, making per-thread init safe.
///
/// A thread-local flag prevents double-initialization on the same thread
/// (which would panic inside `set_platform`).
fn init() {
    thread_local! {
        static INITIALIZED: RefCell<bool> = const { RefCell::new(false) };
    }
    INITIALIZED.with(|flag| {
        if !*flag.borrow() {
            i_slint_backend_testing::init_no_event_loop();
            *flag.borrow_mut() = true;
        }
    });
}

/// Create a fresh `MainWindow` instance backed by the testing backend.
///
/// The window is resized to a large height so that all controls in the
/// `ScrollView` are within the visible viewport. Without this, the testing
/// backend does not materialize elements below the fold.
fn create_window() -> MainWindow {
    init();
    let window = MainWindow::new().unwrap();
    window
        .window()
        .set_size(slint::PhysicalSize::new(1200, 5000));
    window
}

#[test]
fn test_window_creates_successfully() {
    let _window = create_window();
}

// ---------------------------------------------------------------------------
// Property round-trip tests
// ---------------------------------------------------------------------------

/// The Earth lens travels with the rest of the camera, so a round trip is the
/// whole of its bridge: nothing derives it and nothing else writes it.
#[test]
fn test_camera_fov_roundtrips_through_scene_params() {
    let window = create_window();
    window.set_camera_fov(96.0);
    let params = sunlit_earth::ui_callbacks::read_params_from_window(&window, &[1, 2, 4, 8]);
    approx::assert_relative_eq!(params.camera.fov_deg, 96.0);

    let restored = sunlit_core::params::SceneParams {
        camera: sunlit_core::scene::camera::CameraParams {
            fov_deg: 42.0,
            ..params.camera
        },
        ..params
    };
    sunlit_earth::ui_callbacks::apply_params_to_window(&window, &restored);
    approx::assert_relative_eq!(window.get_camera_fov(), 42.0);
}

#[test]
fn test_celestial_properties_roundtrip_through_scene_params() {
    let window = create_window();
    window.set_sky_fov(165.0);
    window.set_star_intensity(0.73);
    window.set_star_size(1.6);
    window.set_star_glow_strength(2.4);
    window.set_star_glow_radius(26.0);
    window.set_star_contrast(-0.67);
    window.set_star_mag_limit(5.4);
    window.set_sun_glow(2.4);
    window.set_sun_rays(1.3);
    window.set_sun_flare(0.7);
    window.set_sun_size(4.5);
    window.set_sun_halo_radius(5.25);
    window.set_sun_horizon_boost(6.5);
    window.set_sun_horizon_reach(8.5);
    window.set_sun_horizon_depth(3.25);
    window.set_sun_reddening(1.6);
    window.set_sun_refraction(0.35);
    window.set_atmo_sunrise_glow(2.4);
    window.set_atmo_sunrise_width(72.0);
    window.set_moon_brightness(1.7);
    window.set_moon_size(5.5);
    window.set_moon_earthshine(0.21);
    window.set_milky_way_intensity(1.15);
    let params = sunlit_earth::ui_callbacks::read_params_from_window(&window, &[1, 2, 4, 8]);
    approx::assert_relative_eq!(params.sky_fov, 165.0);
    approx::assert_relative_eq!(params.star_intensity, 0.73);
    approx::assert_relative_eq!(params.star_size, 1.6);
    approx::assert_relative_eq!(params.star_glow_strength, 2.4);
    approx::assert_relative_eq!(params.star_glow_radius, 26.0);
    approx::assert_relative_eq!(params.star_contrast, -0.67);
    approx::assert_relative_eq!(params.star_mag_limit, 5.4);
    approx::assert_relative_eq!(params.sun_glow, 2.4);
    approx::assert_relative_eq!(params.sun_rays, 1.3);
    approx::assert_relative_eq!(params.sun_flare, 0.7);
    approx::assert_relative_eq!(params.sun_size, 4.5);
    approx::assert_relative_eq!(params.sun_halo_radius, 5.25);
    approx::assert_relative_eq!(params.sun_horizon_boost, 6.5);
    approx::assert_relative_eq!(params.sun_horizon_reach, 8.5);
    approx::assert_relative_eq!(params.sun_horizon_depth, 3.25);
    approx::assert_relative_eq!(params.sun_reddening, 1.6);
    approx::assert_relative_eq!(params.sun_refraction, 0.35);
    approx::assert_relative_eq!(params.atmo_sunrise_glow, 2.4);
    approx::assert_relative_eq!(params.atmo_sunrise_width, 72.0);
    approx::assert_relative_eq!(params.moon_brightness, 1.7);
    approx::assert_relative_eq!(params.moon_size, 5.5);
    approx::assert_relative_eq!(params.moon_earthshine, 0.21);
    approx::assert_relative_eq!(params.milky_way_intensity, 1.15);
}

#[test]
fn test_celestial_properties_apply_from_scene_params() {
    let window = create_window();
    let params = sunlit_core::params::SceneParams {
        sky_fov: 75.0,
        star_intensity: 3.2,
        star_size: 2.1,
        star_glow_strength: 2.8,
        star_glow_radius: 29.0,
        star_contrast: -0.76,
        star_mag_limit: 5.9,
        sun_glow: 0.4,
        sun_rays: 1.8,
        sun_flare: 0.9,
        sun_size: 2.25,
        sun_halo_radius: 7.5,
        sun_horizon_boost: 4.5,
        sun_horizon_reach: 1.25,
        sun_horizon_depth: 0.75,
        sun_reddening: 0.4,
        sun_refraction: 1.85,
        atmo_sunrise_glow: 0.6,
        atmo_sunrise_width: 18.0,
        moon_brightness: 0.8,
        moon_size: 3.5,
        moon_earthshine: 0.17,
        milky_way_intensity: 0.35,
        ..sunlit_core::params::SceneParams::default()
    };
    sunlit_earth::ui_callbacks::apply_params_to_window(&window, &params);
    approx::assert_relative_eq!(window.get_sky_fov(), 75.0);
    approx::assert_relative_eq!(window.get_star_intensity(), 3.2);
    approx::assert_relative_eq!(window.get_star_size(), 2.1);
    approx::assert_relative_eq!(window.get_star_glow_strength(), 2.8);
    approx::assert_relative_eq!(window.get_star_glow_radius(), 29.0);
    approx::assert_relative_eq!(window.get_star_contrast(), -0.76);
    approx::assert_relative_eq!(window.get_star_mag_limit(), 5.9);
    approx::assert_relative_eq!(window.get_sun_glow(), 0.4);
    approx::assert_relative_eq!(window.get_sun_rays(), 1.8);
    approx::assert_relative_eq!(window.get_sun_flare(), 0.9);
    approx::assert_relative_eq!(window.get_sun_size(), 2.25);
    approx::assert_relative_eq!(window.get_sun_halo_radius(), 7.5);
    approx::assert_relative_eq!(window.get_sun_horizon_boost(), 4.5);
    approx::assert_relative_eq!(window.get_sun_horizon_reach(), 1.25);
    approx::assert_relative_eq!(window.get_sun_horizon_depth(), 0.75);
    approx::assert_relative_eq!(window.get_sun_reddening(), 0.4);
    approx::assert_relative_eq!(window.get_sun_refraction(), 1.85);
    approx::assert_relative_eq!(window.get_atmo_sunrise_glow(), 0.6);
    approx::assert_relative_eq!(window.get_atmo_sunrise_width(), 18.0);
    approx::assert_relative_eq!(window.get_moon_brightness(), 0.8);
    approx::assert_relative_eq!(window.get_moon_size(), 3.5);
    approx::assert_relative_eq!(window.get_moon_earthshine(), 0.17);
    approx::assert_relative_eq!(window.get_milky_way_intensity(), 0.35);
}

/// How close a hand-written literal in `main.slint` has to be to the config
/// default it mirrors, for the one property that needs any slack at all.
///
/// `zoom` reads 0.421 in the `.slint` file where the config computes it as
/// 0.42096078 through `distance_to_zoom`, a relative difference of 9.3e-5.
/// Every other property agrees exactly and is compared at `f32::EPSILON`, so a
/// literal that drifted in the fourth decimal is still a failure.
const SLINT_LITERAL_PRECISION: f32 = 1.0e-3;

/// The window and the config have to start from the same numbers, or the first
/// slider a user touches pushes all the others to whatever the `.slint` file
/// happened to say.
#[test]
fn test_the_window_and_the_config_start_from_the_same_defaults() {
    /// One row per property: the `AppConfig` field, the window getter that
    /// mirrors it, and the tolerance the pair is compared at.
    macro_rules! rows {
        ($w:ident, $c:ident, $($field:ident from $getter:ident),* $(,)?) => {
            vec![$((stringify!($field), $w.$getter(), $c.$field, f32::EPSILON)),*]
        };
    }

    let window = create_window();
    let config = sunlit_core::config::AppConfig::default();
    let mut rows = rows![
        window, config,
        longitude from get_camera_longitude,
        latitude from get_camera_latitude,
        offset_x from get_camera_offset_x,
        offset_y from get_camera_offset_y,
        tilt from get_camera_tilt,
        yaw from get_camera_yaw,
        pitch from get_camera_pitch,
        camera_fov from get_camera_fov,
        sky_fov from get_sky_fov,
        star_intensity from get_star_intensity,
        star_size from get_star_size,
        star_glow_strength from get_star_glow_strength,
        star_glow_radius from get_star_glow_radius,
        star_contrast from get_star_contrast,
        star_mag_limit from get_star_mag_limit,
        sun_glow from get_sun_glow,
        sun_rays from get_sun_rays,
        sun_flare from get_sun_flare,
        sun_size from get_sun_size,
        sun_halo_radius from get_sun_halo_radius,
        sun_horizon_boost from get_sun_horizon_boost,
        sun_horizon_reach from get_sun_horizon_reach,
        sun_horizon_depth from get_sun_horizon_depth,
        sun_reddening from get_sun_reddening,
        sun_refraction from get_sun_refraction,
        atmo_sunrise_glow from get_atmo_sunrise_glow,
        atmo_sunrise_width from get_atmo_sunrise_width,
        moon_brightness from get_moon_brightness,
        moon_size from get_moon_size,
        moon_earthshine from get_moon_earthshine,
        milky_way_intensity from get_milky_way_intensity,
        terminator_width from get_terminator_width,
        diffuse_floor from get_diffuse_floor,
        diffuse_ramp from get_diffuse_ramp,
        spec_shininess from get_spec_shininess,
        spec_intensity from get_spec_intensity,
        fresnel_mix from get_fresnel_mix,
        fresnel_exp from get_fresnel_exp,
        cloud_opacity from get_cloud_opacity,
        cloud_opacity_night from get_cloud_opacity_night,
        cloud_floor from get_cloud_floor,
        cloud_gamma from get_cloud_gamma,
        cloud_night from get_cloud_night,
        rayleigh_intensity from get_rayleigh_intensity,
        rayleigh_sharpness from get_rayleigh_sharpness,
        rayleigh_haze from get_rayleigh_haze,
        nightglow_intensity from get_nightglow_intensity,
        nightglow_falloff from get_nightglow_falloff,
        nightglow_balance from get_nightglow_balance,
        day_saturation from get_day_saturation,
        night_saturation from get_night_saturation,
    ];

    // The two gamma sliders carry a position on the curve rather than the value
    // itself, so what has to agree is the mapped default.
    for (name, from_window, gamma) in [
        ("day_gamma", window.get_day_gamma(), config.day_gamma),
        ("night_gamma", window.get_night_gamma(), config.night_gamma),
    ] {
        rows.push((
            name,
            from_window,
            sunlit_core::params::gamma_value_to_slider(gamma),
            f32::EPSILON,
        ));
    }
    // The one property the `.slint` file rounds; see SLINT_LITERAL_PRECISION.
    rows.push((
        "zoom",
        window.get_camera_zoom(),
        config.zoom,
        SLINT_LITERAL_PRECISION,
    ));

    for (name, from_window, from_config, tolerance) in rows {
        assert!(
            approx::relative_eq!(from_window, from_config, max_relative = tolerance),
            "{name}: the window starts at {from_window} and the config at {from_config}"
        );
    }
    assert_eq!(
        window.get_diffuse_shading(),
        config.diffuse_shading,
        "diffuse_shading"
    );
    assert_eq!(
        window.get_atmo_enabled(),
        config.atmo_enabled,
        "atmo_enabled"
    );
}

// ---------------------------------------------------------------------------
// Load-defaults callback tests
// ---------------------------------------------------------------------------

/// The button exists, is unique, and invokes its callback; what the callback
/// does is `ui_callbacks::on_load_defaults`, which needs an `EngineLink` and is
/// covered by the e2e suite.
#[test]
fn test_load_defaults_fires_a_callback_that_can_reset_the_window() {
    let window = create_window();

    let window_weak = window.as_weak();
    window.on_load_defaults(move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let defaults = sunlit_core::config::AppConfig::default();
        win.set_camera_longitude(defaults.longitude);
        win.set_camera_latitude(defaults.latitude);
        win.set_camera_zoom(defaults.zoom);
        win.set_camera_offset_x(defaults.offset_x);
        win.set_camera_offset_y(defaults.offset_y);
        win.set_camera_tilt(defaults.tilt);
        win.set_camera_yaw(defaults.yaw);
        win.set_camera_pitch(defaults.pitch);
        win.set_diffuse_shading(defaults.diffuse_shading);
    });

    window.set_camera_longitude(123.0);
    window.set_camera_zoom(0.99);
    window.set_diffuse_shading(false);

    let buttons: Vec<_> =
        ElementHandle::find_by_accessible_label(&window, "Load Defaults").collect();
    assert_eq!(buttons.len(), 1);
    buttons[0].invoke_accessible_default_action();

    let defaults = sunlit_core::config::AppConfig::default();
    approx::assert_relative_eq!(window.get_camera_longitude(), defaults.longitude);
    approx::assert_relative_eq!(window.get_camera_zoom(), defaults.zoom);
    assert_eq!(window.get_diffuse_shading(), defaults.diffuse_shading);
}

// ---------------------------------------------------------------------------
// Advanced section visibility toggle tests
// ---------------------------------------------------------------------------

/// The advanced section's controls are in the element tree only while it is
/// open, which is what an `if`-gated subtree means. It starts closed.
#[test]
fn test_the_advanced_controls_are_in_the_tree_only_while_it_is_open() {
    let window = create_window();
    assert!(
        !window.get_advanced_open(),
        "the advanced section starts closed"
    );

    for open in [None, Some(true), Some(false)] {
        if let Some(open) = open {
            window.set_advanced_open(open);
        }
        let sliders: Vec<_> =
            ElementHandle::find_by_element_id(&window, "MainWindow::longitude-slider").collect();
        assert_eq!(
            !sliders.is_empty(),
            window.get_advanced_open(),
            "advanced-open is {}",
            window.get_advanced_open()
        );
    }
}

// ---------------------------------------------------------------------------
// Conditional visibility for the atmosphere
// ---------------------------------------------------------------------------

#[test]
fn test_the_atmosphere_sliders_follow_the_atmosphere_switch() {
    let window = create_window();
    window.set_advanced_open(true);

    for enabled in [true, false, true] {
        window.set_atmo_enabled(enabled);
        let sliders: Vec<_> =
            ElementHandle::find_by_element_id(&window, "MainWindow::rayleigh-intensity-slider")
                .collect();
        assert_eq!(
            !sliders.is_empty(),
            enabled,
            "the rayleigh slider with the atmosphere {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }
}

// ---------------------------------------------------------------------------
// Config save round-trip
// ---------------------------------------------------------------------------

/// The resolution combo box is the one persisted setting whose widget carries
/// an index rather than a value, so the mapping back to a width is what a save
/// depends on.
#[test]
fn test_save_reads_the_texture_resolution_from_its_combo_box() {
    use sunlit_core::config::{self, AppConfig, TEXTURE_RESOLUTIONS};

    let window = create_window();
    let stored = AppConfig::default();

    for (index, width) in TEXTURE_RESOLUTIONS.iter().enumerate() {
        window.set_texture_resolution_index(i32::try_from(index).unwrap());
        let saved = sunlit_earth::ui_callbacks::read_config_from_window_onto(
            &window,
            &[1, 2, 4, 8],
            &stored,
            false,
        );
        assert_eq!(
            saved.texture_resolution, *width,
            "the combo box index must decide the saved width"
        );
    }

    // An index the model does not have must not write a width nothing offers.
    window.set_texture_resolution_index(99);
    let saved = sunlit_earth::ui_callbacks::read_config_from_window_onto(
        &window,
        &[1, 2, 4, 8],
        &stored,
        false,
    );
    assert_eq!(saved.texture_resolution, config::DEFAULT_TEXTURE_RESOLUTION);
}

/// The one-run resolution flag is cleared by the combo box's own callback, so
/// it matters that setting the index from Rust is not a selection: startup,
/// reset, and load-defaults all set it that way, and any of them clearing the
/// flag would put a `--texture-resolution` override back into the config file.
#[test]
fn test_setting_a_combo_index_is_not_a_selection() {
    let window = create_window();

    let fired = Rc::new(RefCell::new(0));
    let counter = Rc::clone(&fired);
    window.on_texture_resolution_changed(move || *counter.borrow_mut() += 1);
    let counter = Rc::clone(&fired);
    window.on_texture_changed(move || *counter.borrow_mut() += 1);
    let counter = Rc::clone(&fired);
    window.on_msaa_changed(move || *counter.borrow_mut() += 1);

    window.set_texture_resolution_index(2);
    window.set_texture_index(1);
    window.set_aa_index(0);
    // Force the bindings to be evaluated rather than left pending.
    let _ = ElementHandle::find_by_accessible_label(&window, "Europe").count();

    assert_eq!(window.get_texture_resolution_index(), 2);
    assert_eq!(
        *fired.borrow(),
        0,
        "a combo box index set from Rust must not report a user selection"
    );
}

/// `--texture-resolution` is shown in the window, so that the window is not
/// claiming a width the engine is not using, but it is a one-run flag and a
/// save has to leave the stored width alone.
#[test]
fn test_save_keeps_the_stored_resolution_while_the_cli_owns_the_combo_box() {
    use sunlit_core::config::{AppConfig, TEXTURE_RESOLUTIONS};

    let window = create_window();
    let stored = AppConfig {
        texture_resolution: TEXTURE_RESOLUTIONS[0],
        ..AppConfig::default()
    };
    // The window shows something else, as an override does.
    window.set_texture_resolution_index(2);
    window.set_camera_longitude(42.0);

    let saved = sunlit_earth::ui_callbacks::read_config_from_window_onto(
        &window,
        &[1, 2, 4, 8],
        &stored,
        true,
    );
    assert_eq!(
        saved.texture_resolution, TEXTURE_RESOLUTIONS[0],
        "a one-run override must not be written to the config"
    );
    // Everything else still comes from the window.
    approx::assert_relative_eq!(saved.longitude, 42.0);

    let chosen = sunlit_earth::ui_callbacks::read_config_from_window_onto(
        &window,
        &[1, 2, 4, 8],
        &stored,
        false,
    );
    assert_eq!(
        chosen.texture_resolution, TEXTURE_RESOLUTIONS[2],
        "once the width is the user's, the window decides again"
    );
}

/// Settings without a widget must survive a save.
///
/// `quality_tier` is the current example: it is persisted but has no control in
/// the window, and building the saved config from `AppConfig::default()` meant
/// every "Set as Wallpaper" in a debug build overwrote a release install's
/// `high` with `low`.
#[test]
fn test_save_preserves_settings_without_a_widget() {
    use sunlit_core::config::{AppConfig, QualityTier};

    let window = create_window();
    window.set_camera_longitude(42.0);
    window.set_cloud_opacity(0.25);

    for tier in [QualityTier::Low, QualityTier::Medium, QualityTier::High] {
        let stored = AppConfig {
            quality_tier: tier,
            ..AppConfig::default()
        };
        let saved = sunlit_earth::ui_callbacks::read_config_from_window_onto(
            &window,
            &[1, 2, 4, 8],
            &stored,
            false,
        );
        assert_eq!(
            saved.quality_tier, tier,
            "a save must not rewrite the stored quality tier"
        );
        // The UI-managed fields must still be taken from the window.
        approx::assert_relative_eq!(saved.longitude, 42.0);
        approx::assert_relative_eq!(saved.cloud_opacity, 0.25);
    }
}

// ---------------------------------------------------------------------------
// The sky slider's range
// ---------------------------------------------------------------------------

/// The sky slider stops at 180 degrees, and that is not the shader's ceiling.
///
/// `display::layout::SKY_FOV_MAX` is 330, which is the widest sky a spanned
/// canvas may derive. The slider means the anchor screen's own field of view and
/// every other screen extends outward from it, so widening the slider to the
/// shader's clamp would offer a setting that means nothing on one screen and
/// double-counts on several.
#[test]
fn test_the_sky_slider_stops_where_one_screen_stops() {
    let window = create_window();

    approx::assert_relative_eq!(window.get_sky_fov_minimum(), 60.0);
    approx::assert_relative_eq!(window.get_sky_fov_maximum(), 180.0);
}

// ---------------------------------------------------------------------------
// The Displays group
// ---------------------------------------------------------------------------

fn fabricated_monitors() -> Vec<sunlit_core::display::Monitor> {
    use sunlit_core::display::Monitor;
    vec![
        Monitor {
            id: "DP-1".to_owned(),
            label: "DP-1".to_owned(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            primary: false,
        },
        Monitor {
            id: "DP-2".to_owned(),
            label: "DP-2".to_owned(),
            x: 1920,
            y: 0,
            width: 2560,
            height: 1440,
            primary: true,
        },
    ]
}

fn saved_with(
    window: &MainWindow,
    stored: &sunlit_core::config::AppConfig,
) -> sunlit_core::config::AppConfig {
    sunlit_earth::ui_callbacks::read_config_from_window_onto(window, &[1, 2, 4, 8], stored, false)
}

/// The screens the session has have to reach the combo, or the anchor cannot be
/// chosen at all.
#[test]
fn test_the_screen_combo_offers_the_automatic_row_and_every_monitor() {
    use slint::Model;

    let window = create_window();
    sunlit_earth::displays::apply_models_to_window(&window, &fabricated_monitors());

    let rows: Vec<String> = window
        .get_display_screen_options()
        .iter()
        .map(|row| row.to_string())
        .collect();
    assert_eq!(
        rows,
        vec![
            sunlit_earth::displays::AUTOMATIC_SCREEN.to_owned(),
            "DP-1  1920x1080".to_owned(),
            "DP-2  2560x1440  primary".to_owned(),
        ]
    );

    let modes: Vec<String> = window
        .get_display_mode_options()
        .iter()
        .map(|row| row.to_string())
        .collect();
    assert_eq!(modes, sunlit_earth::displays::mode_options());
}

/// The anchor is a combo index in the window and an id in the config file, so
/// the mapping between them is what a save depends on.
#[test]
fn test_the_anchor_and_the_mode_round_trip_through_a_save() {
    use sunlit_core::config::AppConfig;
    use sunlit_core::display::layout::DisplayMode;

    let window = create_window();
    sunlit_earth::displays::apply_models_to_window(&window, &fabricated_monitors());
    let stored = AppConfig::default();

    for (index, expected) in [(0, ""), (1, "DP-1"), (2, "DP-2")] {
        window.set_display_anchor_index(index);
        assert_eq!(saved_with(&window, &stored).anchor_monitor, expected);
    }

    for mode in DisplayMode::ALL {
        window.set_display_mode_index(i32::try_from(mode.index()).unwrap());
        assert_eq!(saved_with(&window, &stored).display_mode, mode);
    }
}

/// An unplugged screen must not cost the setting that named it.
///
/// The combo shows the automatic row for a stored id the session does not have,
/// because that is the screen the plan will actually be built around. Writing
/// that row back would turn one unplugged cable into a lost preference.
#[test]
fn test_a_stored_anchor_the_session_lost_survives_a_save() {
    use sunlit_core::config::AppConfig;

    let window = create_window();
    sunlit_earth::displays::apply_models_to_window(&window, &fabricated_monitors());
    let stored = AppConfig {
        anchor_monitor: "HDMI-9".to_owned(),
        ..AppConfig::default()
    };

    let indices = sunlit_earth::ui_callbacks::ComboIndices::of(
        &stored,
        &[1, 2, 4, 8],
        stored.texture_resolution,
        &sunlit_earth::displays::screen_ids(&fabricated_monitors()),
    );
    assert_eq!(
        indices.display_anchor, 0,
        "a screen this session does not have shows as the automatic row"
    );

    window.set_display_anchor_index(indices.display_anchor);
    assert_eq!(saved_with(&window, &stored).anchor_monitor, "HDMI-9");

    // Moving to a screen this session does have is a deliberate change, and so
    // is moving back to automatic from one.
    window.set_display_anchor_index(1);
    let chosen = saved_with(&window, &stored);
    assert_eq!(chosen.anchor_monitor, "DP-1");
    window.set_display_anchor_index(0);
    assert_eq!(saved_with(&window, &chosen).anchor_monitor, "");
}

/// A window that was never told what screens the session has must not overwrite
/// the stored anchor: that is macOS, where nothing can enumerate them.
#[test]
fn test_a_window_with_no_screen_model_leaves_the_stored_anchor_alone() {
    use sunlit_core::config::AppConfig;

    let window = create_window();
    let stored = AppConfig {
        anchor_monitor: "DP-2".to_owned(),
        ..AppConfig::default()
    };
    assert_eq!(saved_with(&window, &stored).anchor_monitor, "DP-2");
}

/// The diagram highlights the screen the plan will be built around, which for
/// the automatic row is whichever screen the system calls primary.
#[test]
fn test_the_diagram_marks_the_screen_the_plan_will_use() {
    use slint::Model;

    let window = create_window();
    let monitors = fabricated_monitors();
    sunlit_earth::displays::apply_diagram_to_window(&window, &monitors, None);

    let tiles: Vec<_> = window.get_display_tiles().iter().collect();
    assert_eq!(tiles.len(), 2);
    assert!(
        !tiles[0].anchor && tiles[1].anchor,
        "the second monitor is the primary one"
    );
    approx::assert_relative_eq!(window.get_display_aspect(), 4480.0 / 1440.0);

    sunlit_earth::displays::apply_diagram_to_window(&window, &monitors, Some("DP-1"));
    let tiles: Vec<_> = window.get_display_tiles().iter().collect();
    assert!(tiles[0].anchor && !tiles[1].anchor);
}

/// A layout that moved while the window was open.
///
/// The anchor row is the return value rather than a property read, because
/// setting it goes through `defer_combobox_indices`, which needs an event loop
/// this backend deliberately does not have.
#[test]
fn test_a_layout_that_changed_rebuilds_the_displays_group() {
    use slint::Model;

    let window = create_window();
    let screens = sunlit_earth::displays::shared_monitors();
    let both = fabricated_monitors();

    let row = sunlit_earth::displays::replace_monitors(&window, &screens, both.clone(), "DP-2");
    assert_eq!(
        row, 2,
        "the stored screen is the second row after automatic"
    );
    let rows: Vec<String> = window
        .get_display_screen_options()
        .iter()
        .map(|row| row.to_string())
        .collect();
    assert_eq!(rows.len(), 3);
    assert_eq!(window.get_display_tiles().iter().count(), 2);
    assert_eq!(sunlit_earth::displays::monitors_of(&screens), both);

    // Unplugged: one screen, and the group has nothing left to say.
    let alone = vec![both[0].clone()];
    let row = sunlit_earth::displays::replace_monitors(&window, &screens, alone.clone(), "DP-2");
    assert_eq!(
        row, 0,
        "a screen this session no longer has shows as the automatic row"
    );
    assert_eq!(window.get_display_tiles().iter().count(), 1);
    for id in [
        "MainWindow::display-mode-combo",
        "MainWindow::display-screen-combo",
    ] {
        let found: Vec<_> = ElementHandle::find_by_element_id(&window, id).collect();
        assert!(found.is_empty(), "one screen hides {id}");
    }
    assert_eq!(sunlit_earth::displays::monitors_of(&screens), alone);

    // Plugged back in: the setting that named it was never overwritten, so the
    // anchor follows its screen back.
    let row = sunlit_earth::displays::replace_monitors(&window, &screens, both.clone(), "DP-2");
    assert_eq!(row, 2, "the anchor follows its screen back");
    assert_eq!(window.get_display_tiles().iter().count(), 2);
    for id in [
        "MainWindow::display-mode-combo",
        "MainWindow::display-screen-combo",
    ] {
        let found: Vec<_> = ElementHandle::find_by_element_id(&window, id).collect();
        assert!(!found.is_empty(), "a second screen brings {id} back");
    }
}

/// The diagram highlights the screen the plan will use, which for a stored id
/// the session still has is that screen and not the primary.
#[test]
fn test_a_replaced_layout_highlights_the_stored_anchor() {
    use slint::Model;

    let window = create_window();
    let screens = sunlit_earth::displays::shared_monitors();
    sunlit_earth::displays::replace_monitors(&window, &screens, fabricated_monitors(), "DP-1");

    let tiles: Vec<_> = window.get_display_tiles().iter().collect();
    assert!(
        tiles[0].anchor && !tiles[1].anchor,
        "the stored screen is the anchor, even though the other one is primary"
    );
}
// ---------------------------------------------------------------------------
// Sidebar layout
// ---------------------------------------------------------------------------

/// The widest the settings panel is allowed to insist on being, set by a
/// `SettingCombo` row: an 88px label, 4px of spacing, the 140px combo, and the
/// panel's 8px + 22px of padding.
const PANEL_FLOOR: f32 = 262.0;

/// A layout counts an `if`-gated subtree towards its minimum width only once
/// that repeater has been walked. A running app's layout pass does it; a test
/// has to ask for the elements, or `panel-min-width` reads back the minimum of
/// a panel that is missing half its rows and the assertion passes for the
/// wrong reason.
fn materialize(window: &MainWindow) {
    for id in [
        "MainWindow::longitude-slider",
        "MainWindow::display-mode-combo",
        "MainWindow::about-button",
    ] {
        let _ = ElementHandle::find_by_element_id(window, id).count();
    }
}

/// Everything that can widen the panel, at once: both display combos, the
/// Advanced section, and an adapter name as long as the Mesa one that started
/// this.
fn widest_panel_state(window: &MainWindow) {
    sunlit_earth::displays::apply_models_to_window(window, &fabricated_monitors());
    sunlit_earth::displays::apply_diagram_to_window(window, &fabricated_monitors(), None);
    window.set_advanced_open(true);
    window.set_renderer_info(
        "AMD Radeon 780M Graphics (RADV PHOENIX) (Vulkan, IntegratedGpu)".into(),
    );
    materialize(window);
}

/// The sidebar's width is derived from this number, so a widget that reports an
/// unbounded minimum widens the panel for everybody rather than being clipped.
/// This is what fails when somebody adds one.
#[test]
fn test_nothing_in_the_panel_asks_for_more_than_the_sidebar_floor() {
    let window = create_window();
    widest_panel_state(&window);

    assert!(
        window.get_panel_min_width() <= PANEL_FLOOR,
        "the panel asks for {}px, past the {PANEL_FLOOR}px floor the sidebar is built on",
        window.get_panel_min_width()
    );
}

/// The panel is laid out at the left edge of its scroll area, not centred in a
/// viewport that grew past it. A derived floor keeps this true on its own, so
/// what this catches is a hardcoded sidebar width coming back.
#[test]
fn test_the_panel_starts_at_the_left_edge_in_both_advanced_states() {
    let window = create_window();
    widest_panel_state(&window);
    approx::assert_relative_eq!(window.get_panel_x(), 0.0);

    window.set_advanced_open(false);
    materialize(&window);
    approx::assert_relative_eq!(window.get_panel_x(), 0.0);
}

/// The window's minimum has to hold the panel's floor and still leave a globe
/// worth looking at. Both halves are read off the real layout, so raising the
/// floor without raising `min-width` fails here.
#[test]
fn test_the_narrowest_window_holds_the_panel_and_the_globe() {
    let window = create_window();
    widest_panel_state(&window);
    window.window().set_size(slint::LogicalSize::new(
        window.get_window_min_width(),
        900.0,
    ));
    materialize(&window);

    approx::assert_relative_eq!(window.get_panel_x(), 0.0);
    assert!(
        window.get_viewport_width() >= 230.0,
        "only {}px left for the image at the window's minimum width",
        window.get_viewport_width()
    );
}

#[test]
fn test_about_is_reachable_without_opening_advanced() {
    let window = create_window();
    assert!(!window.get_advanced_open(), "precondition: advanced closed");

    let asked = Rc::new(RefCell::new(false));
    let captured = Rc::clone(&asked);
    window.on_show_about(move || *captured.borrow_mut() = true);

    let buttons: Vec<_> =
        ElementHandle::find_by_element_id(&window, "MainWindow::about-button").collect();
    assert_eq!(buttons.len(), 1, "expected one About button");
    buttons[0].invoke_accessible_default_action();

    assert!(
        *asked.borrow(),
        "the About button did not ask for the window"
    );
}

/// The grid's order and `PRESETS`' order are deliberately different, so every
/// cell's index is checked by hand.
#[test]
fn test_every_preset_button_fires_the_index_it_is_named_for() {
    use sunlit_core::scene::camera::PRESETS;

    let window = create_window();

    let received: Rc<RefCell<Option<i32>>> = Rc::new(RefCell::new(None));
    let captured = Rc::clone(&received);
    let window_weak = window.as_weak();
    window.on_apply_preset(move |index| {
        *captured.borrow_mut() = Some(index);
        // The body `ui_callbacks::register_action_callbacks` installs, minus
        // the engine push it cannot do without an `EngineLink`.
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let Some(preset) = usize::try_from(index).ok().and_then(|i| PRESETS.get(i)) else {
            return;
        };
        win.set_camera_longitude(preset.longitude);
        win.set_camera_latitude(preset.latitude);
        win.set_camera_zoom(preset.zoom);
        win.set_camera_pitch(preset.pitch_deg);
    });

    for (label, index) in [
        ("Africa", 3),
        ("N. America", 1),
        ("S. America", 2),
        ("Asia", 4),
        ("Europe", 0),
        ("Oceania", 5),
        ("Pacific", 6),
        ("Blue Marble", 7),
        ("Earthrise", 8),
    ] {
        let buttons: Vec<_> = ElementHandle::find_by_accessible_label(&window, label).collect();
        assert_eq!(buttons.len(), 1, "expected exactly one '{label}' button");
        buttons[0].invoke_accessible_default_action();
        assert_eq!(
            *received.borrow(),
            Some(index),
            "'{label}' fired the wrong preset"
        );

        // And the index reached that entry of PRESETS rather than some other
        // one: a button wired to the wrong index, or a handler that ignored the
        // index, would leave the window on a different continent.
        let expected = &PRESETS[usize::try_from(index).expect("a preset index")];
        approx::assert_relative_eq!(window.get_camera_longitude(), expected.longitude);
        approx::assert_relative_eq!(window.get_camera_latitude(), expected.latitude);
        approx::assert_relative_eq!(window.get_camera_zoom(), expected.zoom);
        approx::assert_relative_eq!(window.get_camera_pitch(), expected.pitch_deg);
    }
}

/// The header is outside the tab area, so scrolling a tab cannot take the
/// version line with it.
#[test]
fn test_scrolling_a_tab_leaves_the_header_where_it_was() {
    let window = about_window();
    window.set_attributions(sunlit_earth::about::document_model(&"line\n\n".repeat(200)));
    materialize_about(&window);

    let body = i_slint_backend_testing::ElementQuery::from_root(&window)
        .match_type_name("StyledText")
        .find_first()
        .expect("the attributions tab has no StyledText");
    let header_before = window.get_version_y();
    let body_before = body.absolute_position().y;

    // Not `ElementHandle::scroll`, which aims at the element's centre: a
    // document taller than the window has its centre outside the window, and
    // an event there reaches nothing. This aims at a point in the tab's own
    // viewport, low enough to be under the header and the tab bar.
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(280.0, 350.0),
            delta_x: 0.0,
            delta_y: -400.0,
        });
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(50));

    // The body having moved is what stops this from passing because nothing
    // scrolled at all.
    assert!(
        (body.absolute_position().y - body_before).abs() > 1.0,
        "the tab did not scroll, so the header standing still proves nothing"
    );
    approx::assert_relative_eq!(window.get_version_y(), header_before);
}

/// The tab bar offers all three tabs, in order.
///
/// What each tab renders is a separate question, and one this cannot answer by
/// reading back a property it set itself: the three tests below drive each
/// tab's own element instead.
#[test]
fn test_the_tab_bar_offers_all_three_tabs() {
    let window = about_window();
    let labels: Vec<String> = i_slint_backend_testing::ElementQuery::from_root(&window)
        .match_accessible_role(i_slint_backend_testing::AccessibleRole::Tab)
        .find_all()
        .iter()
        .filter_map(|tab| tab.accessible_label().map(|label| label.to_string()))
        .collect();

    assert_eq!(labels, ["Attributions", "License", "Third-party"]);
}

/// The licence tab does not wrap, so its scroll area has to be wider than the
/// window rather than clipping the text.
#[test]
fn test_the_license_tab_is_wider_than_a_narrow_window() {
    let window = about_window();
    window.window().set_size(slint::PhysicalSize::new(360, 400));
    window.set_license_text("x".repeat(200).into());
    open_tab(&window, 1);

    let body = i_slint_backend_testing::ElementQuery::from_root(&window)
        .match_type_name("Text")
        .find_all()
        .into_iter()
        .max_by(|a, b| a.size().width.total_cmp(&b.size().width))
        .expect("the licence tab has no Text");

    assert!(
        body.size().width > 360.0,
        "the licence body is {} wide in a 360 px window, so it was wrapped or clipped",
        body.size().width
    );
}

/// A link in the attributions tab reaches `open-url` with the URL that
/// document carries, which is the whole of the wiring between the markdown and
/// the platform's handler.
///
/// There is no one body element any more, so this clicks the first glyphs of
/// every block the tab renders and asserts that one call arrived carrying the
/// attributions document's own URL. The two documents carry different URLs, so
/// a tab bound to the wrong one fails here rather than looking identical, and
/// the paragraph in the fixture carries no link, so a stray call would fail it
/// too.
#[test]
fn test_a_link_in_the_attributions_tab_reaches_open_url() {
    let window = about_window();
    let opened = capture_open_url(&window);
    two_distinguishable_documents(&window);
    materialize_about(&window);

    let bodies = attributions_bodies(&window);
    assert_eq!(bodies.len(), 2, "expected the paragraph and the bullet");
    for body in &bodies {
        click_at(&window, body.absolute_position(), 4.0, 6.0);
    }

    assert_eq!(
        *opened.borrow(),
        vec![ATTRIBUTIONS_LINK.to_owned()],
        "the attributions tab did not open its own document's link"
    );
}

/// The tab lays a document out rather than stacking one paragraph on the next:
/// a heading is bigger than body text, an item's text starts to the right of
/// its bullet, a nested item is one step further in, and consecutive blocks
/// are separated by the layout's spacing rather than by `StyledText`'s
/// hardcoded zero.
#[test]
fn test_the_attributions_tab_lays_its_blocks_out_as_a_document() {
    let window = about_window();
    window.set_attributions(sunlit_earth::about::document_model(
        "# Imagery\n\nbody text\n\n- a top level item\n  - a nested item\n",
    ));
    materialize_about(&window);

    let heading = one_element(&window, "AboutWindow::attributions-heading");
    let paragraph = one_element(&window, "AboutWindow::attributions-paragraph");
    let bullets = all_elements(&window, "AboutWindow::attributions-bullet");
    let items = all_elements(&window, "AboutWindow::attributions-item");
    assert_eq!(
        (bullets.len(), items.len()),
        (2, 2),
        "two bullets, two texts"
    );

    assert!(
        heading.size().height > paragraph.size().height,
        "the heading measured {} against the paragraph's {}",
        heading.size().height,
        paragraph.size().height
    );
    assert!(
        items[0].absolute_position().x > bullets[0].absolute_position().x,
        "the item's text does not start right of its bullet"
    );
    approx::assert_relative_eq!(
        items[1].absolute_position().x - items[0].absolute_position().x,
        14.0
    );
    let gap =
        paragraph.absolute_position().y - heading.absolute_position().y - heading.size().height;
    assert!(
        gap >= 8.0,
        "the heading and the paragraph below it are {gap} apart"
    );
}

/// The same for the third tab, which is where the crate list lives and which
/// is laid out block by block too, so it has no one body element either.
#[test]
fn test_a_link_in_the_third_party_tab_reaches_open_url() {
    let window = about_window();
    let opened = capture_open_url(&window);
    two_distinguishable_documents(&window);
    open_tab(&window, 2);

    let bodies = third_party_bodies(&window);
    assert_eq!(bodies.len(), 2, "expected the preamble and the one entry");
    for body in &bodies {
        click_at(&window, body.absolute_position(), 4.0, 6.0);
    }

    assert_eq!(
        *opened.borrow(),
        vec![THIRD_PARTY_LINK.to_owned()],
        "the third-party tab did not open its own document's link"
    );
}

/// A wrapped list item keeps the whole column: its text is one box as tall as
/// the lines it takes, so the block under it starts below the last of them.
///
/// This is the shape's load-bearing property. A `StyledText` reports the height
/// of a single line unless it is a layout's own child, so a bullet laid out
/// beside the text in a `HorizontalLayout` leaves every wrapped item one line
/// tall and draws the rest of it over the block below.
#[test]
fn test_a_wrapped_list_item_does_not_spill_over_the_block_below() {
    let window = about_window();
    window.set_attributions(sunlit_earth::about::document_model(
        "- short\n- a list item that runs on far enough to wrap over more than \
         one line in a five hundred and sixty pixel window, which is what makes \
         this measurement mean anything at all\n- the block below\n",
    ));
    materialize_about(&window);

    let items = all_elements(&window, "AboutWindow::attributions-item");
    assert_eq!(items.len(), 3, "three list items");
    let (short, wrapped, below) = (&items[0], &items[1], &items[2]);

    assert!(
        wrapped.size().height >= short.size().height * 1.5,
        "the long item measured {} against one line's {}, so it did not wrap",
        wrapped.size().height,
        short.size().height
    );
    assert!(
        below.absolute_position().y >= wrapped.absolute_position().y + wrapped.size().height,
        "the block below starts at {}, inside the wrapped item, which ends at {}",
        below.absolute_position().y,
        wrapped.absolute_position().y + wrapped.size().height
    );
    approx::assert_relative_eq!(wrapped.absolute_position().x, short.absolute_position().x);
}

/// One link per markdown tab, each naming the document it is in.
const ATTRIBUTIONS_LINK: &str = "https://example.invalid/attributions";
const THIRD_PARTY_LINK: &str = "https://example.invalid/third-party";

fn two_distinguishable_documents(window: &sunlit_earth::AboutWindow) {
    window.set_attributions(sunlit_earth::about::document_model(&format!(
        "# Sources\n\nthis paragraph carries no link\n\n- [the link]({ATTRIBUTIONS_LINK})\n"
    )));
    window.set_third_party(sunlit_earth::about::document_model(&format!(
        "this preamble carries no link\n\n- [the link]({THIRD_PARTY_LINK})\n"
    )));
}

/// Every element the attributions tab renders a block's inline markdown into,
/// the paragraphs first and the bullets after.
fn attributions_bodies(window: &sunlit_earth::AboutWindow) -> Vec<ElementHandle> {
    [
        "AboutWindow::attributions-paragraph",
        "AboutWindow::attributions-item",
    ]
    .into_iter()
    .flat_map(|id| ElementHandle::find_by_element_id(window, id))
    .collect()
}

/// The same for the third-party tab, whose rows carry their own ids so a test
/// can say which tab it found.
fn third_party_bodies(window: &sunlit_earth::AboutWindow) -> Vec<ElementHandle> {
    [
        "AboutWindow::third-party-paragraph",
        "AboutWindow::third-party-body",
    ]
    .into_iter()
    .flat_map(|id| ElementHandle::find_by_element_id(window, id))
    .collect()
}

fn one_element(window: &sunlit_earth::AboutWindow, id: &str) -> ElementHandle {
    let found = all_elements(window, id);
    assert_eq!(found.len(), 1, "expected exactly one {id}");
    found.into_iter().next().expect("one element")
}

fn all_elements(window: &sunlit_earth::AboutWindow, id: &str) -> Vec<ElementHandle> {
    ElementHandle::find_by_element_id(window, id).collect()
}

fn capture_open_url(window: &sunlit_earth::AboutWindow) -> Rc<RefCell<Vec<String>>> {
    let opened: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let captured = opened.clone();
    window.on_open_url(move |url| captured.borrow_mut().push(url.to_string()));
    opened
}

/// Click the first glyphs of a `StyledText`, where its first link starts.
///
/// Not `mock_single_click`, which clicks the element's centre: the element is
/// as wide as the tab and the link is a few characters at its left edge, so
/// the centre is past the end of the text. The link's own glyphs are what the
/// hit test is about.
/// The header link goes through the same callback as the documents' links.
#[test]
fn test_the_repository_link_reaches_open_url() {
    let window = about_window();
    let opened: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let captured = opened.clone();
    window.on_open_url(move |url| captured.borrow_mut().push(url.to_string()));
    materialize_about(&window);

    let link: Vec<_> =
        ElementHandle::find_by_element_id(&window, "AboutWindow::repository-link").collect();
    assert_eq!(link.len(), 1, "expected one repository link");
    link[0].mock_single_click(slint::platform::PointerEventButton::Left);

    let opened = opened.borrow();
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert!(
        opened[0].starts_with("https://github.com/"),
        "the header link opened {:?}",
        opened[0]
    );
}

/// An About window with all three documents in it, at a size a person would
/// see, which is what the tab tests need before they can find anything.
fn about_window() -> sunlit_earth::AboutWindow {
    init();
    let window = sunlit_earth::AboutWindow::new().unwrap();
    window.window().set_size(slint::PhysicalSize::new(560, 480));
    window.set_version("1.2.3".into());
    window.set_attributions(sunlit_earth::about::document_model("credits"));
    window.set_third_party(sunlit_earth::about::document_model("- `a 1.0.0`: MIT"));
    window.set_license_text("a licence\nsecond line\n".into());
    materialize_about(&window);
    window
}

/// Make one tab current, since a `TabWidget` only lays the current one out.
fn open_tab(window: &sunlit_earth::AboutWindow, index: usize) {
    let tabs = i_slint_backend_testing::ElementQuery::from_root(window)
        .match_accessible_role(i_slint_backend_testing::AccessibleRole::Tab)
        .find_all();
    tabs[index].invoke_accessible_default_action();
    materialize_about(window);
}

/// Force a layout, so geometry and the element tree are there to look at.
fn materialize_about(window: &sunlit_earth::AboutWindow) {
    window.show().unwrap();
    window.window().request_redraw();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(50));
}

/// Click a point offset from an element's own origin.
///
/// `ElementHandle` can only click an element's centre, and a link inside a
/// paragraph is not at the centre of the paragraph.
fn click_at(window: &sunlit_earth::AboutWindow, origin: slint::LogicalPosition, dx: f32, dy: f32) {
    use slint::platform::{PointerEventButton, WindowEvent};

    let position = slint::LogicalPosition::new(origin.x + dx, origin.y + dy);
    for event in [
        WindowEvent::PointerMoved { position },
        WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        },
        WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        },
    ] {
        window.window().dispatch_event(event);
    }
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
}

/// The adapter name stays on one line however long it is. A word-wrapped
/// `Text` inside the Advanced section reports the height of a single line
/// whatever it wraps to, so a scroll area sized from that cuts the rest off
/// with no way to scroll to it.
#[test]
fn test_the_adapter_line_never_wraps() {
    let window = create_window();
    widest_panel_state(&window);

    let short = adapter_line_height(&window, "Dx12");
    let long = adapter_line_height(
        &window,
        "AMD Radeon 780M Graphics (RADV PHOENIX) (Vulkan, IntegratedGpu)",
    );

    approx::assert_relative_eq!(short, long);
}

fn adapter_line_height(window: &MainWindow, renderer_info: &str) -> f32 {
    window.set_renderer_info(renderer_info.into());
    materialize(window);
    let found: Vec<_> =
        ElementHandle::find_by_element_id(window, "MainWindow::adapter-line").collect();
    assert_eq!(found.len(), 1, "expected exactly one adapter line");
    found[0].size().height
}

// A `Text` whose family and content the test drives, so the width it reports
// is the width the font gives that string.
slint::slint! {
    export component FamilyProbe inherits Window {
        in property <string> family;
        in property <string> body;
        out property <length> text-width: probe.preferred-width;
        probe := Text {
            font-family: root.family;
            text: root.body;
            wrap: no-wrap;
        }
    }
}

/// `about::MONO_FAMILY` has to name a fixed-width family that this platform
/// actually has. Slint resolves no generic keyword: `font-family` reaches
/// parley through `FontFamilyName::named`, so a name nobody has falls back to
/// the proportional default and the licence tab silently loses its layout with
/// no error anywhere. Four narrow glyphs and four wide ones come to the same
/// width in a fixed-width font and nowhere else, which is the difference this
/// measures.
///
/// The second half is what stops it passing for the wrong reason: the same
/// measurement under the default family must disagree, or an equal result
/// above would prove only that the probe cannot tell fonts apart.
#[test]
fn test_the_monospace_family_resolves_on_this_platform() {
    init();
    let probe = FamilyProbe::new().unwrap();

    let width_of = |family: &str, body: &str| {
        probe.set_family(family.into());
        probe.set_body(body.into());
        probe.get_text_width()
    };

    let narrow = width_of(sunlit_earth::about::MONO_FAMILY, "iiii");
    let wide = width_of(sunlit_earth::about::MONO_FAMILY, "mmmm");
    approx::assert_relative_eq!(narrow, wide);

    let proportional_narrow = width_of("", "iiii");
    let proportional_wide = width_of("", "mmmm");
    assert!(
        proportional_narrow < proportional_wide,
        "the default family measured as fixed-width, so this test cannot tell \
         a resolved monospace family from an unresolved one"
    );
}
