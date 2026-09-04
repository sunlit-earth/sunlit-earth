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
    approx::assert_relative_eq!(window.get_sun_glow(), 1.2);
    approx::assert_relative_eq!(window.get_sun_rays(), 0.75);
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
fn test_default_moon_is_enlarged_two_and_a_half_times() {
    let window = create_window();
    approx::assert_relative_eq!(window.get_moon_brightness(), 1.0);
    approx::assert_relative_eq!(window.get_moon_size(), 2.5);
    approx::assert_relative_eq!(window.get_moon_earthshine(), 0.15);
}

#[test]
fn test_default_milky_way_is_on_at_a_fifth_of_full_strength() {
    let window = create_window();
    approx::assert_relative_eq!(window.get_milky_way_intensity(), 0.2);
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
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/main.slint"),
    )
    .expect("read ui/main.slint");
    let row = source
        .split_once("sky-fov-slider := SettingRow {")
        .expect("the sky field-of-view row")
        .1
        .split_once('}')
        .expect("the end of that row")
        .0;
    assert!(
        row.contains("minimum: 60.0;") && row.contains("maximum: 180.0;"),
        "the sky field-of-view row reads:\n{row}"
    );
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

/// Three modes that all mean the same thing are noise on one screen, so the
/// group is not there at all. The setting still persists.
#[test]
fn test_the_displays_group_is_absent_on_a_single_screen() {
    let window = create_window();
    sunlit_earth::displays::apply_diagram_to_window(&window, &fabricated_monitors()[..1], None);

    let combos: Vec<_> =
        ElementHandle::find_by_element_id(&window, "MainWindow::display-mode-combo").collect();
    assert!(
        combos.is_empty(),
        "the Displays group has nothing to say about one screen"
    );
}

#[test]
fn test_the_displays_group_is_there_on_two_screens() {
    let window = create_window();
    sunlit_earth::displays::apply_models_to_window(&window, &fabricated_monitors());
    sunlit_earth::displays::apply_diagram_to_window(&window, &fabricated_monitors(), None);

    for id in [
        "MainWindow::display-mode-combo",
        "MainWindow::display-screen-combo",
    ] {
        let found: Vec<_> = ElementHandle::find_by_element_id(&window, id).collect();
        assert!(!found.is_empty(), "{id} should be in the tree");
    }
}

/// A layout that moved while the window was open.
///
/// The whole reaction, in the order a person sees it: the combo rows become the
/// screens that are there, the group disappears when one is left and comes back
/// when the second returns, and the anchor lands on its own row again rather
/// than on the automatic one, because the stored id was kept while its screen
/// was gone.
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
    let combos: Vec<_> =
        ElementHandle::find_by_element_id(&window, "MainWindow::display-mode-combo").collect();
    assert!(combos.is_empty(), "one screen hides the group");
    assert_eq!(sunlit_earth::displays::monitors_of(&screens), alone);

    // Plugged back in: the setting that named it was never overwritten, so the
    // anchor follows its screen back.
    let row = sunlit_earth::displays::replace_monitors(&window, &screens, both.clone(), "DP-2");
    assert_eq!(row, 2, "the anchor follows its screen back");
    assert_eq!(window.get_display_tiles().iter().count(), 2);
    let combos: Vec<_> =
        ElementHandle::find_by_element_id(&window, "MainWindow::display-mode-combo").collect();
    assert!(!combos.is_empty(), "a second screen brings the group back");
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

/// `MainWindow`'s own `min-width` in `main.slint`.
const WINDOW_MIN_WIDTH: u32 = 520;

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
    window
        .window()
        .set_size(slint::PhysicalSize::new(WINDOW_MIN_WIDTH, 900));
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
    let window = create_window();

    let received: Rc<RefCell<Option<i32>>> = Rc::new(RefCell::new(None));
    let captured = Rc::clone(&received);
    window.on_apply_preset(move |index| {
        *captured.borrow_mut() = Some(index);
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
    }
}

/// The header does not move when the version line outgrows the window.
///
/// This is what the old scroll-area version of the window got wrong: a
/// non-wrapping line wider than the viewport took the content with it. The
/// header is a fixed layout now, so the mark and the title stay at the left
/// edge whatever the version string is.
#[test]
fn test_the_about_window_header_holds_its_place_under_a_long_version() {
    let window = about_window();

    window.set_version("0.0.0".into());
    let short = window.get_header_x();
    window.set_version("0.0.0-".repeat(60).into());
    let long = window.get_header_x();

    approx::assert_relative_eq!(short, long);
}

/// The header is outside the tab area, so scrolling a tab cannot take the
/// version line with it. That is the whole point of decision 3.
#[test]
fn test_scrolling_a_tab_leaves_the_header_where_it_was() {
    let window = about_window();
    window.set_attributions(slint::StyledText::from_markdown(&"line\n\n".repeat(200)).unwrap());
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

/// Each tab is reachable and carries the document it is for.
///
/// The tab bar is a `TabWidget`, so a tab's content only exists while that tab
/// is current; the assertion is on the property each one renders, which is
/// what an empty tab would fail.
#[test]
fn test_all_three_tabs_are_reachable_and_carry_their_document() {
    let window = about_window();
    let labels: Vec<String> = i_slint_backend_testing::ElementQuery::from_root(&window)
        .match_accessible_role(i_slint_backend_testing::AccessibleRole::Tab)
        .find_all()
        .iter()
        .filter_map(|tab| tab.accessible_label().map(|label| label.to_string()))
        .collect();

    assert_eq!(labels, ["Attributions", "License", "Third-party"]);
    assert_ne!(window.get_attributions(), slint::StyledText::default());
    assert_ne!(window.get_third_party(), slint::StyledText::default());
    assert!(!window.get_license_text().is_empty());
}

/// The licence tab does not wrap, so its scroll area has to be wider than the
/// window rather than clipping the text.
///
/// This is the trap decision 6 walks past: giving that layout `width: 100%`,
/// the way the two markdown tabs have it, would take the viewport down to the
/// visible width and cut a 76-column document off at whatever the window is,
/// with no way to scroll to the rest and nothing failing anywhere.
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

/// A link in the attributions tab reaches `open-url` with the URL the document
/// carries, which is the whole of the wiring between the markdown and the
/// platform's handler.
#[test]
fn test_a_link_in_the_attributions_tab_reaches_open_url() {
    let window = about_window();
    let opened: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let captured = opened.clone();
    window.on_open_url(move |url| captured.borrow_mut().push(url.to_string()));
    window.set_attributions(
        slint::StyledText::from_markdown("[the link](https://example.invalid/target)").unwrap(),
    );
    materialize_about(&window);

    // Not `mock_single_click`, which clicks the element's centre: the element
    // is as wide as the tab and the link is a few characters at its left edge,
    // so the centre is past the end of the text. The link's own glyphs are
    // what the hit test is about.
    let body = i_slint_backend_testing::ElementQuery::from_root(&window)
        .match_type_name("StyledText")
        .find_first()
        .expect("the attributions tab has no StyledText");
    click_at(&window, body.absolute_position(), 4.0, 6.0);

    assert_eq!(
        *opened.borrow(),
        vec!["https://example.invalid/target".to_owned()],
        "clicking the link did not reach open-url"
    );
}

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
    window.set_attributions(slint::StyledText::from_markdown("credits").unwrap());
    window.set_third_party(slint::StyledText::from_markdown("- `a 1.0.0`: MIT").unwrap());
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
