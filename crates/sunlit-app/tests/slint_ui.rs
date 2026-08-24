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
    window.set_star_intensity(0.73);
    window.set_star_mag_limit(5.4);
    let params = sunlit_earth::ui_callbacks::read_params_from_window(&window, &[1, 2, 4, 8]);
    approx::assert_relative_eq!(params.star_intensity, 0.73);
    approx::assert_relative_eq!(params.star_mag_limit, 5.4);
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
