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
// Initialization boilerplate (Step 2.1)
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
// Property round-trip tests (Step 2.2)
// ---------------------------------------------------------------------------

#[test]
fn test_camera_longitude_roundtrip() {
    let window = create_window();
    window.set_camera_longitude(42.5);
    approx::assert_relative_eq!(window.get_camera_longitude(), 42.5);
}

#[test]
fn test_camera_latitude_roundtrip() {
    let window = create_window();
    window.set_camera_latitude(-30.0);
    approx::assert_relative_eq!(window.get_camera_latitude(), -30.0);
}

#[test]
fn test_camera_zoom_roundtrip() {
    let window = create_window();
    window.set_camera_zoom(0.75);
    approx::assert_relative_eq!(window.get_camera_zoom(), 0.75);
}

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
fn test_bool_property_roundtrip() {
    let window = create_window();

    // diffuse-shading defaults to true; toggle to false and back
    window.set_diffuse_shading(false);
    assert!(!window.get_diffuse_shading());

    window.set_diffuse_shading(true);
    assert!(window.get_diffuse_shading());
}

#[test]
fn test_int_property_roundtrip() {
    let window = create_window();
    window.set_texture_index(2);
    assert_eq!(window.get_texture_index(), 2);
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

#[test]
fn test_default_star_tuning_uses_balanced_profile() {
    let window = create_window();
    approx::assert_relative_eq!(window.get_sky_fov(), 140.0);
    approx::assert_relative_eq!(window.get_star_intensity(), 2.0);
    approx::assert_relative_eq!(window.get_star_size(), 1.0);
    approx::assert_relative_eq!(window.get_star_glow_strength(), 0.5);
    approx::assert_relative_eq!(window.get_star_glow_radius(), 8.0);
    approx::assert_relative_eq!(window.get_star_contrast(), 0.3);
    approx::assert_relative_eq!(window.get_star_mag_limit(), 6.5);
}

#[test]
fn test_default_sun_shows_the_glare_and_a_trace_of_the_camera() {
    let window = create_window();
    approx::assert_relative_eq!(window.get_sun_glow(), 1.0);
    approx::assert_relative_eq!(window.get_sun_rays(), 0.6);
    approx::assert_relative_eq!(window.get_sun_flare(), 0.15);
    approx::assert_relative_eq!(window.get_sun_size(), 1.0);
    approx::assert_relative_eq!(window.get_sun_halo_radius(), 3.0);
}

/// The window and the config have to start from the same horizon, or the first
/// slider a user touches pushes the other seven of them at whatever the
/// `.slint` file happened to say.
#[test]
fn test_default_horizon_matches_the_config() {
    let window = create_window();
    let config = sunlit_core::config::AppConfig::default();
    approx::assert_relative_eq!(window.get_sun_horizon_boost(), config.sun_horizon_boost);
    approx::assert_relative_eq!(window.get_sun_horizon_reach(), config.sun_horizon_reach);
    approx::assert_relative_eq!(window.get_sun_horizon_depth(), config.sun_horizon_depth);
    approx::assert_relative_eq!(window.get_sun_reddening(), config.sun_reddening);
    approx::assert_relative_eq!(window.get_sun_refraction(), config.sun_refraction);
    approx::assert_relative_eq!(window.get_atmo_sunrise_glow(), config.atmo_sunrise_glow);
    approx::assert_relative_eq!(window.get_atmo_sunrise_width(), config.atmo_sunrise_width);
}

#[test]
fn test_default_moon_is_visible_at_its_true_size() {
    let window = create_window();
    approx::assert_relative_eq!(window.get_moon_brightness(), 1.0);
    approx::assert_relative_eq!(window.get_moon_size(), 1.0);
    approx::assert_relative_eq!(window.get_moon_earthshine(), 0.05);
}

#[test]
fn test_default_milky_way_is_on_at_half_strength() {
    let window = create_window();
    approx::assert_relative_eq!(window.get_milky_way_intensity(), 0.5);
}

// ---------------------------------------------------------------------------
// Preset callback wiring tests (Step 2.3)
// ---------------------------------------------------------------------------

#[test]
fn test_preset_europe_fires_callback() {
    let window = create_window();

    let received_index: Rc<RefCell<Option<i32>>> = Rc::new(RefCell::new(None));
    let captured = Rc::clone(&received_index);
    window.on_apply_preset(move |index| {
        *captured.borrow_mut() = Some(index);
    });

    let buttons: Vec<_> = ElementHandle::find_by_accessible_label(&window, "Europe").collect();
    assert_eq!(buttons.len(), 1, "expected exactly one 'Europe' button");
    buttons[0].invoke_accessible_default_action();

    assert_eq!(*received_index.borrow(), Some(0));
}

#[test]
fn test_preset_earthrise_fires_callback() {
    let window = create_window();

    let received_index: Rc<RefCell<Option<i32>>> = Rc::new(RefCell::new(None));
    let captured = Rc::clone(&received_index);
    window.on_apply_preset(move |index| {
        *captured.borrow_mut() = Some(index);
    });

    let buttons: Vec<_> = ElementHandle::find_by_accessible_label(&window, "Earthrise").collect();
    assert_eq!(buttons.len(), 1, "expected exactly one 'Earthrise' button");
    buttons[0].invoke_accessible_default_action();

    assert_eq!(*received_index.borrow(), Some(8));
}

#[test]
fn test_preset_changes_camera_properties() {
    let window = create_window();

    // Register a callback that applies the Europe preset values (PRESETS[0]).
    // These values are read from scene/camera.rs — the test verifies that
    // clicking the button fires the callback which sets properties, not that
    // any particular constant has a specific value.
    let window_weak = window.as_weak();
    window.on_apply_preset(move |index| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        if index == 0 {
            // Europe preset values from PRESETS[0] in scene/camera.rs
            win.set_camera_longitude(11.0);
            win.set_camera_latitude(24.0);
            win.set_camera_zoom(0.19);
            win.set_camera_offset_x(0.0);
            win.set_camera_offset_y(0.0);
            win.set_camera_tilt(0.0);
            win.set_camera_yaw(0.0);
            win.set_camera_pitch(30.0);
        }
    });

    // Set initial values that differ from the Europe preset
    window.set_camera_longitude(0.0);
    window.set_camera_latitude(0.0);

    // Click the Europe preset button
    let buttons: Vec<_> = ElementHandle::find_by_accessible_label(&window, "Europe").collect();
    assert_eq!(buttons.len(), 1);
    buttons[0].invoke_accessible_default_action();

    // Verify the callback applied the preset values
    approx::assert_relative_eq!(window.get_camera_longitude(), 11.0);
    approx::assert_relative_eq!(window.get_camera_latitude(), 24.0);
    approx::assert_relative_eq!(window.get_camera_zoom(), 0.19);
    approx::assert_relative_eq!(window.get_camera_pitch(), 30.0);
}

// ---------------------------------------------------------------------------
// Load-defaults callback tests (Step 2.4)
// ---------------------------------------------------------------------------

#[test]
fn test_load_defaults_fires_callback() {
    let window = create_window();

    // Register a handler that applies default values (mimics main.rs behavior).
    // We read AppConfig::default() to get the canonical defaults so this test
    // does not hard-code constants.
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

    // Set a non-default value
    window.set_camera_longitude(123.0);
    assert!(
        (window.get_camera_longitude() - 123.0).abs() < 0.01,
        "precondition: longitude should be 123.0 before load-defaults"
    );

    // Click "Load Defaults"
    let buttons: Vec<_> =
        ElementHandle::find_by_accessible_label(&window, "Load Defaults").collect();
    assert_eq!(
        buttons.len(),
        1,
        "expected exactly one 'Load Defaults' button"
    );
    buttons[0].invoke_accessible_default_action();

    // Verify the callback restored the default longitude
    let defaults = sunlit_core::config::AppConfig::default();
    approx::assert_relative_eq!(window.get_camera_longitude(), defaults.longitude);
}

#[test]
fn test_load_defaults_resets_multiple_properties() {
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

    // Set several non-default values
    window.set_camera_longitude(123.0);
    window.set_camera_zoom(0.99);
    window.set_diffuse_shading(false);

    // Click "Load Defaults"
    let buttons: Vec<_> =
        ElementHandle::find_by_accessible_label(&window, "Load Defaults").collect();
    assert_eq!(buttons.len(), 1);
    buttons[0].invoke_accessible_default_action();

    // Verify all properties reset to defaults
    let defaults = sunlit_core::config::AppConfig::default();
    approx::assert_relative_eq!(window.get_camera_longitude(), defaults.longitude);
    approx::assert_relative_eq!(window.get_camera_zoom(), defaults.zoom);
    assert_eq!(window.get_diffuse_shading(), defaults.diffuse_shading);
}

// ---------------------------------------------------------------------------
// Advanced section visibility toggle tests (Step 2.5)
// ---------------------------------------------------------------------------

#[test]
fn test_advanced_section_starts_closed() {
    let window = create_window();

    assert!(
        !window.get_advanced_open(),
        "advanced section should start closed"
    );

    // The longitude slider is inside `if root.advanced-open:` so it should
    // not appear in the element tree when advanced is closed.
    let sliders: Vec<_> =
        ElementHandle::find_by_element_id(&window, "MainWindow::longitude-slider").collect();
    assert!(
        sliders.is_empty(),
        "longitude slider should not be in the tree when advanced is closed"
    );
}

#[test]
fn test_advanced_section_opens() {
    let window = create_window();

    window.set_advanced_open(true);

    let sliders: Vec<_> =
        ElementHandle::find_by_element_id(&window, "MainWindow::longitude-slider").collect();
    assert!(
        !sliders.is_empty(),
        "longitude slider should be in the tree when advanced is open"
    );
}

#[test]
fn test_advanced_section_closes() {
    let window = create_window();

    // Open then close
    window.set_advanced_open(true);
    window.set_advanced_open(false);

    let sliders: Vec<_> =
        ElementHandle::find_by_element_id(&window, "MainWindow::longitude-slider").collect();
    assert!(
        sliders.is_empty(),
        "longitude slider should disappear when advanced is closed again"
    );
}

// ---------------------------------------------------------------------------
// Conditional visibility tests for atmosphere (Step 2.6)
// ---------------------------------------------------------------------------

#[test]
fn test_atmosphere_sliders_hidden_when_disabled() {
    let window = create_window();

    // Open advanced but disable atmosphere
    window.set_advanced_open(true);
    window.set_atmo_enabled(false);

    let sliders: Vec<_> =
        ElementHandle::find_by_element_id(&window, "MainWindow::rayleigh-intensity-slider")
            .collect();
    assert!(
        sliders.is_empty(),
        "rayleigh intensity slider should not be in the tree when atmosphere is disabled"
    );
}

#[test]
fn test_atmosphere_sliders_visible_when_enabled() {
    let window = create_window();

    // Open advanced section; atmo-enabled defaults to true.
    window.set_advanced_open(true);
    assert!(
        window.get_atmo_enabled(),
        "precondition: atmo-enabled should default to true"
    );

    let sliders: Vec<_> =
        ElementHandle::find_by_element_id(&window, "MainWindow::rayleigh-intensity-slider")
            .collect();
    assert!(
        !sliders.is_empty(),
        "rayleigh intensity slider should be in the tree when atmosphere is enabled"
    );
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
    assert!((saved.longitude - 42.0).abs() < f32::EPSILON);

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
        assert!((saved.longitude - 42.0).abs() < f32::EPSILON);
        assert!((saved.cloud_opacity - 0.25).abs() < f32::EPSILON);
    }
}
