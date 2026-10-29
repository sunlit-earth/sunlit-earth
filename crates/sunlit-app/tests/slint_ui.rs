//! Headless Slint UI integration tests.
//!
//! These tests exercise property bindings, callback wiring, conditional
//! visibility, preset application, and default restoration using the
//! `i-slint-backend-testing` crate. No GPU or display is required.

use std::cell::RefCell;
use std::rc::Rc;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use slint::ComponentHandle;
use sunlit_core::color::Rgb8;
use sunlit_core::config::AppConfig;
use sunlit_core::params::SceneParams;
use sunlit_earth::ui_callbacks;
use sunlit_earth::{
    AtmosphereSettings, CameraPositionSettings, CelestialSettings, CloudSettings, DateTimeSettings,
    FramingSettings, LightingSettings, MainWindow, MoonSettings, SkySettings, StarSettings,
    SunSettings, SunriseBandSettings,
};

/// The sample counts the tests' adapter offers, in the AA combo box's order.
const AA_COUNTS: [u32; 4] = [1, 2, 4, 8];

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

/// Create a fresh `MainWindow` instance backed by the testing backend, holding
/// the default config the way startup leaves it before the window is shown.
///
/// `main.slint` states no defaults, so a bare window is all zeros with the
/// Clouds and Atmosphere rows hidden, which is a window the app never shows.
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
    ui_callbacks::register_color_text(&window);
    let config = AppConfig::default();
    ui_callbacks::apply_config_to_window(&window, &config);
    ui_callbacks::set_combobox_indices(
        &window.as_weak(),
        ui_callbacks::ComboIndices::of(&config, &AA_COUNTS, config.texture_resolution, &[]),
    );
    window
}

fn set_longitude(window: &MainWindow, longitude: f32) {
    window.set_camera_position(CameraPositionSettings {
        longitude,
        ..window.get_camera_position()
    });
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
    window.set_framing(FramingSettings {
        fov: 96.0,
        ..window.get_framing()
    });
    let params = ui_callbacks::read_params_from_window(&window, &AA_COUNTS);
    approx::assert_relative_eq!(params.camera.fov_deg, 96.0);

    let restored = SceneParams {
        camera: sunlit_core::scene::camera::CameraParams {
            fov_deg: 42.0,
            ..params.camera
        },
        ..params
    };
    ui_callbacks::apply_params_to_window(&window, &restored);
    approx::assert_relative_eq!(window.get_framing().fov, 42.0);
}

#[test]
fn test_celestial_properties_roundtrip_through_scene_params() {
    let window = create_window();
    window.set_celestial(CelestialSettings {
        sky: SkySettings {
            fov: 165.0,
            milky_way: 1.15,
        },
        sun: SunSettings {
            glow: 2.4,
            rays: 1.3,
            size: 4.5,
            halo_radius: 5.25,
            flare: 0.7,
            horizon_boost: 6.5,
            horizon_reach: 8.5,
            horizon_depth: 3.25,
            reddening: 1.6,
            refraction: 0.35,
        },
        moon: MoonSettings {
            brightness: 1.7,
            size: 5.5,
            earthshine: 0.21,
            shadow: 0.62,
        },
        stars: StarSettings {
            intensity: 0.73,
            size: 1.6,
            glow_strength: 2.4,
            glow_radius: 26.0,
            contrast: -0.67,
            mag_limit: 5.4,
        },
    });
    window.set_atmosphere(AtmosphereSettings {
        sunrise: SunriseBandSettings {
            glow: 2.4,
            width: 72.0,
        },
        ..window.get_atmosphere()
    });
    let params = ui_callbacks::read_params_from_window(&window, &AA_COUNTS);
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
    approx::assert_relative_eq!(params.moon_shadow, 0.62);
    approx::assert_relative_eq!(params.milky_way_intensity, 1.15);
}

#[test]
fn test_celestial_properties_apply_from_scene_params() {
    let window = create_window();
    let params = SceneParams {
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
        moon_shadow: 0.44,
        milky_way_intensity: 0.35,
        ..SceneParams::default()
    };
    ui_callbacks::apply_params_to_window(&window, &params);
    let CelestialSettings {
        sky,
        sun,
        moon,
        stars,
    } = window.get_celestial();
    let sunrise = window.get_atmosphere().sunrise;
    approx::assert_relative_eq!(sky.fov, 75.0);
    approx::assert_relative_eq!(stars.intensity, 3.2);
    approx::assert_relative_eq!(stars.size, 2.1);
    approx::assert_relative_eq!(stars.glow_strength, 2.8);
    approx::assert_relative_eq!(stars.glow_radius, 29.0);
    approx::assert_relative_eq!(stars.contrast, -0.76);
    approx::assert_relative_eq!(stars.mag_limit, 5.9);
    approx::assert_relative_eq!(sun.glow, 0.4);
    approx::assert_relative_eq!(sun.rays, 1.8);
    approx::assert_relative_eq!(sun.flare, 0.9);
    approx::assert_relative_eq!(sun.size, 2.25);
    approx::assert_relative_eq!(sun.halo_radius, 7.5);
    approx::assert_relative_eq!(sun.horizon_boost, 4.5);
    approx::assert_relative_eq!(sun.horizon_reach, 1.25);
    approx::assert_relative_eq!(sun.horizon_depth, 0.75);
    approx::assert_relative_eq!(sun.reddening, 0.4);
    approx::assert_relative_eq!(sun.refraction, 1.85);
    approx::assert_relative_eq!(sunrise.glow, 0.6);
    approx::assert_relative_eq!(sunrise.width, 18.0);
    approx::assert_relative_eq!(moon.brightness, 0.8);
    approx::assert_relative_eq!(moon.size, 3.5);
    approx::assert_relative_eq!(moon.earthshine, 0.17);
    approx::assert_relative_eq!(moon.shadow, 0.44);
    approx::assert_relative_eq!(sky.milky_way, 0.35);
}

// ---------------------------------------------------------------------------
// Resetting settings to their defaults
// ---------------------------------------------------------------------------

fn is_reset_button(element: &ElementHandle) -> bool {
    element
        .accessible_label()
        .is_some_and(|label| label.starts_with("Reset "))
}

/// The accessible label of every reset button in the tree, which is every
/// reset the Advanced section shows: a button is in the tree only while its
/// value, its subsection or its group differs from the default.
fn resets_offered(window: &MainWindow) -> Vec<String> {
    let mut labels: Vec<String> = ElementQuery::from_root(window)
        .match_predicate(is_reset_button)
        .find_all()
        .iter()
        .filter_map(ElementHandle::accessible_label)
        .map(|label| label.to_string())
        .collect();
    labels.sort();
    labels
}

/// The one element in the tree with this accessible label.
fn the_button(window: &MainWindow, label: &str) -> ElementHandle {
    let found: Vec<_> = ElementHandle::find_by_accessible_label(window, label).collect();
    assert_eq!(found.len(), 1, "expected exactly one '{label}'");
    found.into_iter().next().expect("one element")
}

/// Every element that is, or inherits, one of the panel's own components.
fn all_of(window: &MainWindow, component: &str) -> Vec<ElementHandle> {
    ElementQuery::from_root(window)
        .match_inherits(component)
        .find_all()
}

/// The Camera Position rows, which offer no reset however far they move.
const FIXED_ROWS: [&str; 3] = [
    "MainWindow::longitude-slider",
    "MainWindow::latitude-slider",
    "MainWindow::zoom-slider",
];

fn is_fixed(row: &ElementHandle) -> bool {
    row.id().is_some_and(|id| FIXED_ROWS.contains(&id.as_str()))
}

/// Every row of the Advanced section, the sliders' and the colors', in tree
/// order.
fn all_rows(window: &MainWindow) -> Vec<ElementHandle> {
    ElementQuery::from_root(window)
        .match_predicate(|element| part_of(element) == Some(Part::Row))
        .find_all()
}

/// Every row of the Advanced section that offers a reset once it moves.
fn resettable_rows(window: &MainWindow) -> Vec<ElementHandle> {
    all_rows(window)
        .into_iter()
        .filter(|row| !is_fixed(row))
        .collect()
}

/// Whether a row is a color's rather than a slider's.
fn is_color_row(row: &ElementHandle) -> bool {
    row.type_name()
        .into_iter()
        .chain(row.bases().into_iter().flatten())
        .any(|name| name == "SettingColor")
}

/// The swatch inside one `SettingColor`, which opens its picker and carries
/// its color as its accessible value.
fn swatch_of(row: &ElementHandle) -> ElementHandle {
    row.query_descendants()
        .match_accessible_role(AccessibleRole::Button)
        .match_predicate(|element| !is_reset_button(element))
        .find_first()
        .expect("a SettingColor holds a swatch")
}

/// The color a swatch reports.
fn swatch_color(swatch: &ElementHandle) -> Rgb8 {
    swatch
        .accessible_value()
        .and_then(|value| Rgb8::parse_hex(&value))
        .expect("a swatch reports its color as hex")
}

/// Move a color row the way a screen reader does, through the swatch's
/// accessible value, to a color far from the one it holds.
fn move_color_off_its_value(swatch: &ElementHandle) {
    let before = swatch_color(swatch);
    let target = Rgb8::new(255 - before.r, 255 - before.g, before.b.wrapping_add(128));
    swatch.set_accessible_value(target.to_hex());
    assert_eq!(
        swatch_color(swatch),
        target,
        "the swatch did not take {target}"
    );
}

/// Move one row of either kind off its value.
fn move_row_off_its_value(row: &ElementHandle) {
    if is_color_row(row) {
        move_color_off_its_value(&swatch_of(row));
    } else {
        move_off_its_value(&slider_of(row));
    }
}

/// The slider inside one `SettingRow`.
fn slider_of(row: &ElementHandle) -> ElementHandle {
    row.query_descendants()
        .match_accessible_role(AccessibleRole::Slider)
        .find_first()
        .expect("a SettingRow holds a slider")
}

fn accessible_number(element: &ElementHandle) -> f32 {
    element
        .accessible_value()
        .and_then(|value| value.parse().ok())
        .expect("a slider reports its value")
}

/// Move a slider the way a user does, through its accessibility action, to a
/// point well inside its own range and away from where it was.
///
/// The target is worked out from the range the slider reports rather than
/// from the field behind it, so this names no setting.
fn move_off_its_value(slider: &ElementHandle) {
    let minimum = slider
        .accessible_value_minimum()
        .expect("a slider has a minimum");
    let maximum = slider
        .accessible_value_maximum()
        .expect("a slider has a maximum");
    let before = accessible_number(slider);
    let target = [0.37, 0.63]
        .into_iter()
        .map(|fraction| minimum + (maximum - minimum) * fraction)
        .find(|target| (target - before).abs() > (maximum - minimum) * 0.05)
        .expect("one of two points a quarter of the range apart is away from the value");
    slider.set_accessible_value(target.to_string());
    let after = accessible_number(slider);
    assert!(
        (after - before).abs() > (maximum - minimum) * 0.05,
        "a slider from {minimum} to {maximum} stayed at {before}, asked for {target}"
    );
}

/// Every row of the Advanced section that offers a reset moved off its
/// default, a slider's or a color's.
fn move_every_row(window: &MainWindow) -> usize {
    let rows = resettable_rows(window);
    for row in &rows {
        move_row_off_its_value(row);
    }
    rows.len()
}

/// Press a reset and check that it went away, which it does only once what it
/// covers is back at its default.
///
/// An `if` drops its instance when the tree is next visited rather than when
/// its condition changes, so the visit comes before the check.
fn press(window: &MainWindow, button: &ElementHandle) {
    button.invoke_accessible_default_action();
    let _ = resets_offered(window);
    assert!(
        !button.is_valid(),
        "'{}' is still offered after it was pressed",
        button.accessible_label().unwrap_or_default()
    );
}

/// With the default config applied, the Advanced section offers nothing to
/// reset. A row that forgot its `default-value` reads zero and fails here,
/// unless zero is its default.
#[test]
fn test_the_defaults_offer_no_reset() {
    let window = create_window();
    window.set_advanced_open(true);
    materialize(&window);
    assert!(
        !all_rows(&window).is_empty(),
        "the Advanced section is open"
    );
    assert_eq!(resets_offered(&window), Vec::<String>::new());

    let indices = ui_callbacks::ComboIndices::of_window(&window);
    window.set_texture_index((indices.texture + 1) % 4);
    window.set_aa_index(1 - indices.aa.clamp(0, 1));
    window.set_texture_resolution_index((indices.texture_resolution + 1) % 3);
    assert_eq!(
        resets_offered(&window),
        Vec::<String>::new(),
        "the Rendering group offers no reset"
    );
}

/// A slider inside a subsection offers three resets once it moves: its row's,
/// its subsection's and its group's.
#[test]
fn test_moving_one_slider_offers_three_resets() {
    let window = create_window();
    window.set_advanced_open(true);

    let rows: Vec<_> =
        ElementHandle::find_by_element_id(&window, "MainWindow::sun-glow-slider").collect();
    assert_eq!(rows.len(), 1, "expected one Sun glare row");
    move_off_its_value(&slider_of(&rows[0]));

    assert_eq!(
        resets_offered(&window),
        ["Reset Celestial", "Reset Glare", "Reset Sun"]
    );
}

/// Every row's reset puts back its own default and nothing else.
///
/// Every slider is moved first and every row's reset pressed after, so a row
/// bound to one field and defaulting from another leaves that field off its
/// default, and the window then disagrees with the defaults.
#[test]
fn test_every_row_reset_returns_its_default() {
    let window = create_window();
    window.set_advanced_open(true);
    let moved = move_every_row(&window);
    assert!(moved > 0, "the Advanced section has sliders");
    assert_ne!(
        ui_callbacks::read_params_from_window(&window, &AA_COUNTS),
        SceneParams::default()
    );

    for row in resettable_rows(&window) {
        let reset = row
            .query_descendants()
            .match_predicate(is_reset_button)
            .find_first()
            .expect("a moved row offers its reset");
        press(&window, &reset);
    }

    assert_eq!(
        ui_callbacks::read_params_from_window(&window, &AA_COUNTS),
        SceneParams::default()
    );
    assert_eq!(resets_offered(&window), Vec::<String>::new());
}

/// Every subsection's reset and every group's puts back what it covers.
///
/// A heading or a group that compares one struct and assigns another still
/// differs from its default after the press, so its button stays and `press`
/// fails. The sliders move again before the groups, since a group whose rows
/// all sit under headings has nothing left to reset once the headings have
/// been pressed.
#[test]
fn test_every_heading_and_group_reset_clears_what_it_covers() {
    let window = create_window();
    window.set_advanced_open(true);
    move_every_row(&window);

    for heading in all_of(&window, "SettingHeading") {
        let reset = heading
            .query_descendants()
            .match_predicate(is_reset_button)
            .find_first()
            .expect("every subsection has a moved slider in it");
        press(&window, &reset);
    }

    move_every_row(&window);
    for group in all_of(&window, "SettingGroup") {
        let has_sliders = group
            .query_descendants()
            .match_predicate(|element| part_of(element) == Some(Part::Row) && !is_fixed(element))
            .find_first()
            .is_some();
        let title = group.accessible_label().expect("a group has a title");
        let own = group
            .query_descendants()
            .match_predicate(move |element| {
                element
                    .accessible_label()
                    .is_some_and(|label| label == format!("Reset {title}"))
            })
            .find_first();
        assert_eq!(
            own.is_some(),
            has_sliders,
            "a group offers its reset exactly when a slider in it has moved"
        );
        if let Some(reset) = own {
            press(&window, &reset);
        }
    }

    assert_eq!(
        ui_callbacks::read_params_from_window(&window, &AA_COUNTS),
        SceneParams::default()
    );
    assert_eq!(resets_offered(&window), Vec::<String>::new());
}

/// What an element of the Advanced section is, read off the component it is
/// an instance of.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Part {
    Group,
    Heading,
    Row,
}

fn part_of(element: &ElementHandle) -> Option<Part> {
    let names: Vec<_> = element
        .type_name()
        .into_iter()
        .chain(element.bases().into_iter().flatten())
        .collect();
    [
        ("SettingGroup", Part::Group),
        ("SettingHeading", Part::Heading),
        ("SettingRow", Part::Row),
        ("SettingColor", Part::Row),
    ]
    .into_iter()
    .find(|(name, _)| names.iter().any(|candidate| candidate == name))
    .map(|(_, part)| part)
}

/// The section's groups, headings and resettable rows in tree order, which is
/// the order the panel draws them in.
fn panel_parts(window: &MainWindow) -> Vec<(Part, ElementHandle)> {
    ElementQuery::from_root(window)
        .match_predicate(|element| part_of(element).is_some() && !is_fixed(element))
        .find_all()
        .into_iter()
        .filter_map(|element| part_of(&element).map(|part| (part, element)))
        .collect()
}

/// Whether each row, in tree order, is one the heading or group at `target`
/// covers: a group's rows are the ones up to the next group, and a heading's
/// the ones up to the next heading or the end of its group.
fn rows_covered_by(parts: &[(Part, ElementHandle)], target: usize) -> Vec<bool> {
    let kind = parts[target].0;
    let mut inside = false;
    let mut covered = Vec::new();
    for (index, (part, _)) in parts.iter().enumerate() {
        match part {
            Part::Row => covered.push(inside),
            Part::Group => inside = index == target,
            Part::Heading if index == target => inside = true,
            Part::Heading if kind == Part::Heading => inside = false,
            Part::Heading => {}
        }
    }
    covered
}

/// The reset a heading or a group offers for itself, rather than one of its
/// rows'.
fn own_reset(part: Part, element: &ElementHandle) -> Option<ElementHandle> {
    match part {
        Part::Heading => element
            .query_descendants()
            .match_predicate(is_reset_button)
            .find_first(),
        Part::Group => {
            let wanted = format!("Reset {}", element.accessible_label()?);
            element
                .query_descendants()
                .match_predicate(move |found| {
                    found
                        .accessible_label()
                        .is_some_and(|label| label == wanted.as_str())
                })
                .find_first()
        }
        Part::Row => None,
    }
}

/// Each heading's and each group's reset, pressed alone with every slider
/// moved, takes away the reset of exactly the rows it covers, which are read
/// off the tree rather than named.
///
/// Every press starts from a freshly moved window, so a reset that also puts
/// back something it does not cover takes that row's reset away wherever in
/// the panel the row is.
#[test]
fn test_each_heading_and_group_reset_changes_exactly_what_it_covers() {
    let targets = {
        let window = create_window();
        window.set_advanced_open(true);
        panel_parts(&window)
            .iter()
            .filter(|(part, _)| *part != Part::Row)
            .count()
    };
    assert!(targets > 0, "the Advanced section has headings and groups");

    let mut pressed = 0;
    for nth in 0..targets {
        let window = create_window();
        window.set_advanced_open(true);
        move_every_row(&window);
        let parts = panel_parts(&window);
        let (target, (kind, element)) = parts
            .iter()
            .enumerate()
            .filter(|(_, (part, _))| *part != Part::Row)
            .nth(nth)
            .expect("the same panel as the first window");
        let covered = rows_covered_by(&parts, target);
        let Some(reset) = own_reset(*kind, element) else {
            assert!(
                !covered.contains(&true),
                "a {kind:?} with a moved slider in it offers no reset"
            );
            continue;
        };
        assert!(
            covered.contains(&true),
            "a {kind:?} with no slider in it offers a reset"
        );
        let label = reset.accessible_label().unwrap_or_default();
        press(&window, &reset);
        pressed += 1;

        let rows = parts.iter().filter(|(part, _)| *part == Part::Row);
        for ((_, row), covered) in rows.zip(&covered) {
            let offered = row
                .query_descendants()
                .match_predicate(is_reset_button)
                .find_first()
                .is_some();
            let name = row
                .query_descendants()
                .match_type_name("Text")
                .find_first()
                .and_then(|text| text.accessible_label())
                .unwrap_or_default();
            assert_eq!(
                offered,
                !covered,
                "after '{label}', the '{name}' row's reset is {}",
                if offered { "still offered" } else { "gone" }
            );
        }
    }
    assert!(pressed > 0, "some heading or group offered its reset");
}

/// A group's reset puts its switch back on, and the rows the switch hides come
/// back with it.
#[test]
fn test_a_group_reset_turns_its_switch_back_on() {
    let window = create_window();
    window.set_advanced_open(true);
    window.set_clouds(CloudSettings {
        enabled: false,
        ..window.get_clouds()
    });
    window.set_atmosphere(AtmosphereSettings {
        enabled: false,
        ..window.get_atmosphere()
    });
    window.set_lighting(LightingSettings {
        diffuse_shading: false,
        ..window.get_lighting()
    });
    assert_eq!(
        resets_offered(&window),
        ["Reset Atmosphere", "Reset Clouds", "Reset Lighting"]
    );

    for group in ["Clouds", "Atmosphere", "Lighting"] {
        press(&window, &the_button(&window, &format!("Reset {group}")));
    }

    assert!(window.get_clouds().enabled);
    assert!(window.get_atmosphere().enabled);
    assert!(window.get_lighting().diffuse_shading);
    for id in [
        "MainWindow::cloud-opacity-slider",
        "MainWindow::rayleigh-intensity-slider",
    ] {
        let found: Vec<_> = ElementHandle::find_by_element_id(&window, id).collect();
        assert_eq!(found.len(), 1, "{id} is back with its switch");
    }
    assert_eq!(resets_offered(&window), Vec::<String>::new());
}

/// The override is the whole of what Date / Time offers to reset: the instant
/// it holds is written afresh each time it is turned on, so the hour and the
/// day have no default to go back to.
#[test]
fn test_date_time_offers_a_reset_while_overridden() {
    let window = create_window();
    window.set_advanced_open(true);
    window.set_datetime(DateTimeSettings {
        use_custom: true,
        ..window.get_datetime()
    });
    for id in [
        "MainWindow::custom-hour-slider",
        "MainWindow::custom-day-slider",
    ] {
        let rows: Vec<_> = ElementHandle::find_by_element_id(&window, id).collect();
        assert_eq!(rows.len(), 1, "expected one {id}");
        move_off_its_value(&slider_of(&rows[0]));
    }

    assert_eq!(resets_offered(&window), ["Reset Date / Time"]);
    press(&window, &the_button(&window, "Reset Date / Time"));
    assert!(!window.get_datetime().use_custom, "the time is live again");
    assert_eq!(resets_offered(&window), Vec::<String>::new());
}

/// Every reset carries a hover text of its own, which is also its accessible
/// description, and a heading's or a group's names what it covers.
#[test]
fn test_every_reset_says_what_it_does() {
    let window = create_window();
    window.set_advanced_open(true);
    move_every_row(&window);
    let resets = ElementQuery::from_root(&window)
        .match_predicate(is_reset_button)
        .find_all();
    assert!(!resets.is_empty(), "the moved sliders offer resets");

    let rows: Vec<String> = all_rows(&window)
        .iter()
        .filter_map(|row| {
            row.query_descendants()
                .match_type_name("Text")
                .find_first()
                .and_then(|text| text.accessible_label())
                .map(|label| label.to_string())
        })
        .collect();
    for reset in resets {
        let label = reset.accessible_label().unwrap_or_default();
        let target = label.trim_start_matches("Reset ");
        let description = reset.accessible_description().unwrap_or_default();
        assert!(!description.is_empty(), "'{label}' has no hover text");
        if !rows.iter().any(|row| row == target) {
            assert!(
                description.contains(target),
                "'{label}' says '{description}', which does not name {target}"
            );
        }
    }
}

/// The camera's position offers nothing to reset, neither its rows nor its
/// group, however far it moves.
#[test]
fn test_the_camera_position_offers_no_reset() {
    let window = create_window();
    window.set_advanced_open(true);
    for id in FIXED_ROWS {
        let rows: Vec<_> = ElementHandle::find_by_element_id(&window, id).collect();
        assert_eq!(rows.len(), 1, "expected one {id}");
        assert!(is_fixed(&rows[0]), "{id} is recognized as fixed");
        move_off_its_value(&slider_of(&rows[0]));
    }

    assert_ne!(
        window.get_camera_position(),
        window.get_defaults().camera_position
    );
    assert_eq!(resets_offered(&window), Vec::<String>::new());
}

/// Every kind of reset reaches the engine the way a drag does, through
/// `sliders-changed`, exactly once per press.
#[test]
fn test_a_reset_pushes_the_scene() {
    let window = create_window();
    window.set_advanced_open(true);
    let pushes = Rc::new(std::cell::Cell::new(0));
    let counter = Rc::clone(&pushes);
    window.on_sliders_changed(move || counter.set(counter.get() + 1));

    let rows: Vec<_> =
        ElementHandle::find_by_element_id(&window, "MainWindow::sun-glow-slider").collect();
    assert_eq!(rows.len(), 1, "expected one Sun glare row");
    let slider = slider_of(&rows[0]);
    for label in ["Reset Glare", "Reset Sun", "Reset Celestial"] {
        move_off_its_value(&slider);
        let before = pushes.get();
        press(&window, &the_button(&window, label));
        assert_eq!(pushes.get() - before, 1, "'{label}' pushed the scene once");
    }
}

// ---------------------------------------------------------------------------
// The ocean colors
// ---------------------------------------------------------------------------

/// The two color rows, the day's and the night's.
fn ocean_rows(window: &MainWindow) -> [ElementHandle; 2] {
    [
        "MainWindow::day-ocean-color",
        "MainWindow::night-ocean-color",
    ]
    .map(|id| {
        let found: Vec<_> = ElementHandle::find_by_element_id(window, id).collect();
        assert_eq!(found.len(), 1, "expected one {id}");
        found.into_iter().next().expect("one row")
    })
}

/// The window's two ocean colors, as the scene reads them.
fn ocean_colors(window: &MainWindow) -> [Rgb8; 2] {
    let params = ui_callbacks::read_params_from_window(window, &AA_COUNTS);
    [params.day_ocean_color, params.night_ocean_color]
}

/// The one element of an open picker that carries this accessible label.
fn in_the_picker(window: &MainWindow, label: &'static str) -> Option<ElementHandle> {
    ElementQuery::from_root(window)
        .match_predicate(move |element| {
            element
                .accessible_label()
                .is_some_and(|found| found == label)
        })
        .find_first()
}

/// Where a point of an open picker is in the window, from where the testing
/// backend places it: inside a popup it reports a position from the popup's
/// own origin, which `SettingColor` puts 2 px under the swatch of `row`.
fn picker_in_window(row: &ElementHandle, inside: slint::LogicalPosition) -> slint::LogicalPosition {
    let swatch = swatch_of(row);
    let at = swatch.absolute_position();
    slint::LogicalPosition::new(
        at.x + inside.x,
        at.y + swatch.size().height + 2.0 + inside.y,
    )
}

/// The middle of an element of the picker `row` opened, in the window.
fn middle_of(row: &ElementHandle, element: &ElementHandle) -> slint::LogicalPosition {
    let at = element.absolute_position();
    let size = element.size();
    picker_in_window(
        row,
        slint::LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0),
    )
}

/// Press and release the left button at `position`.
fn press_at(window: &MainWindow, position: slint::LogicalPosition) {
    use slint::platform::{PointerEventButton, WindowEvent};
    let window = window.window();
    window.dispatch_event(WindowEvent::PointerMoved { position });
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

/// Open a color row's picker the way a click on its swatch does.
fn open_picker(window: &MainWindow, row: &ElementHandle) {
    swatch_of(row).invoke_accessible_default_action();
    assert!(
        in_the_picker(window, "Hue").is_some(),
        "the picker did not open"
    );
}

/// A click well away from the panel, where no picker reaches.
fn click_outside(window: &MainWindow) {
    press_at(window, slint::LogicalPosition::new(1100.0, 4800.0));
    assert!(
        in_the_picker(window, "Hue").is_none(),
        "a click outside the picker left it open"
    );
}

/// The colors reach the window from `SceneParams` and come back exactly,
/// every channel of each.
#[test]
fn test_the_ocean_colors_round_trip_through_scene_params() {
    let window = create_window();
    let params = SceneParams {
        day_ocean_color: Rgb8::new(1, 254, 128),
        night_ocean_color: Rgb8::new(255, 0, 7),
        ..SceneParams::default()
    };
    ui_callbacks::apply_params_to_window(&window, &params);
    assert_eq!(
        ocean_colors(&window),
        [params.day_ocean_color, params.night_ocean_color]
    );
    assert_eq!(
        ui_callbacks::read_params_from_window(&window, &AA_COUNTS),
        params
    );
}

/// Each color row stands first under its heading in Color Correction.
#[test]
fn test_the_ocean_rows_lead_the_day_and_night_texture_headings() {
    let window = create_window();
    window.set_advanced_open(true);
    let parts = panel_parts(&window);
    for (heading, row) in ["Day Texture", "Night Texture"]
        .into_iter()
        .zip(ocean_rows(&window))
    {
        let at = parts
            .iter()
            .position(|(part, element)| {
                *part == Part::Heading
                    && element
                        .query_descendants()
                        .match_type_name("Text")
                        .find_first()
                        .and_then(|text| text.accessible_label())
                        .is_some_and(|label| label == heading)
            })
            .unwrap_or_else(|| panic!("no '{heading}' heading"));
        let next = &parts[at + 1].1;
        assert_eq!(next.id(), row.id(), "the row after '{heading}'");
    }
}

/// Opening a picker and closing it with a click outside changes neither
/// color and pushes nothing to the engine: the picker writes the color from
/// its handlers alone.
#[test]
fn test_opening_and_closing_the_picker_changes_no_color() {
    let window = create_window();
    window.set_advanced_open(true);
    let pushes = Rc::new(std::cell::Cell::new(0));
    let counter = Rc::clone(&pushes);
    window.on_sliders_changed(move || counter.set(counter.get() + 1));
    let before = ocean_colors(&window);
    for row in ocean_rows(&window) {
        open_picker(&window, &row);
        click_outside(&window);
    }
    assert_eq!(ocean_colors(&window), before);
    assert_eq!(pushes.get(), 0, "an untouched picker pushed the scene");
}

/// The picker's hex field sets the color on Enter, with or without the hash,
/// and leaves it where the text is not a color.
#[test]
fn test_a_hex_entered_in_the_picker_sets_the_color() {
    use slint::platform::{Key, WindowEvent};
    let window = create_window();
    window.set_advanced_open(true);
    let [day, _] = ocean_rows(&window);
    open_picker(&window, &day);
    let field = ElementQuery::from_root(&window)
        .match_accessible_role(AccessibleRole::TextInput)
        .match_predicate(|element| {
            element
                .accessible_label()
                .is_some_and(|label| label == "Hex")
        })
        .find_first()
        .expect("the picker has a hex field");
    press_at(&window, middle_of(&day, &field));
    let enter = |text: &str| {
        field.set_accessible_value(text);
        window.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Return.into(),
        });
        window.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Return.into(),
        });
    };

    enter("#123456");
    assert_eq!(ocean_colors(&window)[0], Rgb8::new(0x12, 0x34, 0x56));
    enter("A0B0C0");
    assert_eq!(ocean_colors(&window)[0], Rgb8::new(0xa0, 0xb0, 0xc0));
    for invalid in ["#12345g", "#1234", "blue", ""] {
        enter(invalid);
        assert_eq!(
            ocean_colors(&window)[0],
            Rgb8::new(0xa0, 0xb0, 0xc0),
            "'{invalid}' moved the color"
        );
    }
    assert_eq!(
        ocean_colors(&window)[1],
        window_defaults_night(&window),
        "the night's color moved with the day's"
    );
}

/// The night's default as the window's resets hold it.
fn window_defaults_night(window: &MainWindow) -> Rgb8 {
    ui_callbacks::rgb8(window.get_defaults().color_correction.night.ocean)
}

/// The hue strip sets the hue through its accessible value, and a press in
/// the square sets the color of that place: saturation across, and value
/// down the square root axis.
#[test]
fn test_the_hue_strip_and_a_press_in_the_square_set_that_color() {
    let window = create_window();
    window.set_advanced_open(true);
    let [_, night] = ocean_rows(&window);
    open_picker(&window, &night);
    let hue = in_the_picker(&window, "Hue").expect("the picker has a hue strip");
    hue.set_accessible_value("120");
    approx::assert_relative_eq!(accessible_number(&hue), 120.0);

    let squares: Vec<_> =
        ElementHandle::find_by_element_id(&window, "ColorPicker::square").collect();
    assert_eq!(squares.len(), 1, "one open picker, one square");
    let at = squares[0].absolute_position();
    let size = squares[0].size();
    for (across, down) in [(0.25_f32, 0.25_f32), (0.75, 0.5), (0.5, 0.875)] {
        let position = picker_in_window(
            &night,
            slint::LogicalPosition::new(at.x + size.width * across, at.y + size.height * down),
        );
        press_at(&window, position);
        let saturation = across;
        let up = 1.0 - down;
        let expected = ui_callbacks::rgb8(slint::Color::from_hsva(120.0, saturation, up * up, 1.0));
        let drawn = ocean_colors(&window)[1];
        assert!(
            drawn
                .channels()
                .iter()
                .zip(expected.channels())
                .all(|(&d, e)| d.abs_diff(e) <= 1),
            "a press at ({across}, {down}) set {drawn}, the square's color there is {expected}"
        );
    }
}

// ---------------------------------------------------------------------------
// Load-defaults callback tests
// ---------------------------------------------------------------------------

/// The button exists, is unique, and invokes its callback. What the app's own
/// callback then does is the test below this one.
#[test]
fn test_load_defaults_fires_a_callback_that_can_reset_the_window() {
    let window = create_window();

    let window_weak = window.as_weak();
    window.on_load_defaults(move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        ui_callbacks::apply_params_to_window(&win, &SceneParams::default());
    });

    window.set_camera_position(CameraPositionSettings {
        longitude: 123.0,
        zoom: 0.99,
        ..window.get_camera_position()
    });
    window.set_lighting(LightingSettings {
        diffuse_shading: false,
        ..window.get_lighting()
    });

    let buttons: Vec<_> =
        ElementHandle::find_by_accessible_label(&window, "Load Defaults").collect();
    assert_eq!(buttons.len(), 1);
    buttons[0].invoke_accessible_default_action();

    let defaults = AppConfig::default();
    approx::assert_relative_eq!(window.get_camera_position().longitude, defaults.longitude);
    approx::assert_relative_eq!(window.get_camera_position().zoom, defaults.zoom);
    assert_eq!(
        window.get_lighting().diffuse_shading,
        defaults.diffuse_shading
    );
}

/// Load-defaults through the callback the app registers, rather than one this
/// test writes.
///
/// `apply_whole_config` is what load-defaults and reset both run, and it is
/// four things at once: the window, the diagram, the two settings the engine
/// holds itself because they are not in `SceneParams`, and the push. The combo
/// indices' second write lands on an event loop this backend does not run; the
/// first, which the push reads, is the next case's.
#[test]
fn test_load_defaults_applies_the_whole_config_the_app_would() {
    use slint::Model;
    use sunlit_core::engine::EngineCommand;

    let window = create_window();
    let (sender, sent) = crossbeam_channel::unbounded();
    let link = sunlit_earth::engine_client::EngineLink::new(
        sender,
        vec!["None".to_owned(), "MSAA".to_owned()],
        vec![1, 8],
    );
    let screens = sunlit_earth::displays::shared_monitors();
    sunlit_earth::displays::set_monitors(&screens, fabricated_monitors());
    sunlit_earth::ui_callbacks::register_action_callbacks(&window, &link, &screens);

    let defaults = sunlit_core::config::AppConfig::default();
    let wanted = sunlit_core::params::SceneParams::from_config(&defaults);
    set_longitude(&window, wanted.camera.longitude + 47.0);
    let mut celestial = window.get_celestial();
    celestial.sky.fov = wanted.sky_fov - 11.0;
    window.set_celestial(celestial);
    window.set_auto_refresh_enabled(!defaults.auto_refresh_enabled);
    link.set_resolution_is_one_run_only(true);

    window.invoke_load_defaults();

    approx::assert_relative_eq!(
        window.get_camera_position().longitude,
        wanted.camera.longitude
    );
    approx::assert_relative_eq!(window.get_celestial().sky.fov, wanted.sky_fov);
    assert_eq!(
        window.get_auto_refresh_enabled(),
        defaults.auto_refresh_enabled,
        "a setting outside SceneParams is put back too"
    );
    assert_eq!(
        window.get_display_tiles().iter().count(),
        fabricated_monitors().len(),
        "the diagram is rebuilt from the shared monitor list"
    );
    assert!(
        !link.resolution_is_one_run_only(),
        "the texture resolution is the user's again once they ask for defaults"
    );

    let sent: Vec<EngineCommand> = sent.try_iter().collect();
    let resolution = sent.iter().find_map(|command| match command {
        EngineCommand::SetTextureResolution(width) => Some(*width),
        _ => None,
    });
    assert_eq!(resolution, Some(defaults.texture_resolution));

    let plan = sent.iter().find_map(|command| match command {
        EngineCommand::SetDisplayPlan { mode, anchor } => Some((*mode, anchor.clone())),
        _ => None,
    });
    assert_eq!(plan, Some((defaults.display_mode, defaults.anchor())));

    let pushed = sent
        .iter()
        .find_map(|command| match command {
            EngineCommand::UpdateParams(params) => Some(params),
            _ => None,
        })
        .expect("the scene the window now holds reaches the engine");
    approx::assert_relative_eq!(pushed.camera.longitude, wanted.camera.longitude);
}

/// The scene load-defaults pushes carries the config's sample count and
/// texture mode, which are rows of two combo boxes, rather than the rows the
/// window was on. Startup sets the rows through the same
/// `set_combobox_indices` before its own push.
#[test]
fn test_load_defaults_pushes_the_configs_combo_rows() {
    use sunlit_core::engine::EngineCommand;

    let window = create_window();
    let (sender, sent) = crossbeam_channel::unbounded();
    let counts = vec![1, 2, 4, 8];
    let link = sunlit_earth::engine_client::EngineLink::new(
        sender,
        counts.iter().map(|count| format!("{count}x")).collect(),
        counts.clone(),
    );
    let screens = sunlit_earth::displays::shared_monitors();
    sunlit_earth::ui_callbacks::register_action_callbacks(&window, &link, &screens);

    let defaults = sunlit_core::config::AppConfig::default();
    let other_count = counts
        .iter()
        .position(|&count| count != defaults.sample_count)
        .expect("a count the defaults do not ask for");
    window.set_aa_index(i32::try_from(other_count).expect("a small index"));
    window.set_texture_index((defaults.texture_index + 1) % 4);

    window.invoke_load_defaults();

    let pushed = sent
        .try_iter()
        .find_map(|command| match command {
            EngineCommand::UpdateParams(params) => Some(params),
            _ => None,
        })
        .expect("load-defaults pushes the window's scene");
    assert_eq!(pushed.sample_count, defaults.sample_count);
    assert_eq!(pushed.texture_index, defaults.texture_index);
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
        window.set_atmosphere(AtmosphereSettings {
            enabled,
            ..window.get_atmosphere()
        });
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

#[test]
fn test_the_cloud_sliders_follow_the_cloud_switch() {
    let window = create_window();
    window.set_advanced_open(true);

    for enabled in [true, false, true] {
        window.set_clouds(CloudSettings {
            enabled,
            ..window.get_clouds()
        });
        for id in [
            "MainWindow::cloud-opacity-slider",
            "MainWindow::cloud-haze-equator-slider",
            "MainWindow::cloud-gamma-ocean-slider",
        ] {
            let sliders: Vec<_> = ElementHandle::find_by_element_id(&window, id).collect();
            assert_eq!(
                !sliders.is_empty(),
                enabled,
                "{id} with the clouds {}",
                if enabled { "shown" } else { "hidden" }
            );
        }
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
    set_longitude(&window, 42.0);

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
    set_longitude(&window, 42.0);
    window.set_clouds(CloudSettings {
        opacity: 0.25,
        ..window.get_clouds()
    });

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

/// The sky slider spans the short edge, and its range covers every sky the
/// width-measured slider offered on the 16:9 screen most configs were tuned on,
/// 60 to 180 degrees across, which is what the version 4 step converts them to.
///
/// Its top is not the shader's ceiling either: `display::layout::SKY_FOV_MAX`
/// is 330, the widest sky a wide output or a span may derive, and the slider
/// means the short edge every output extends outward from.
#[test]
fn test_the_sky_slider_covers_the_range_it_had_on_a_16_9_screen() {
    use sunlit_core::display::layout::{Framing, screen_framing};

    let window = create_window();
    let across_16_9 = |sky_fov: f32| {
        let settings = Framing {
            camera_fov: 20.0,
            sky_fov,
            offset_x: 0.0,
            offset_y: 0.0,
        };
        screen_framing(settings, 1920, 1080).sky_fov
    };
    assert!(across_16_9(window.get_sky_fov_minimum()) <= 60.0);
    assert!(across_16_9(window.get_sky_fov_maximum()) >= 180.0);
    assert!(across_16_9(window.get_sky_fov_maximum()) < 200.0);
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
/// the write that sticks is the second of `set_combobox_indices`, which needs
/// an event loop this backend deliberately does not have.
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

/// The Sun-fixed check and the Time of day row it brings in ask for no more
/// than the floor either.
#[test]
fn test_the_sun_fixed_rows_ask_for_no_more_than_the_sidebar_floor() {
    let window = create_window();
    widest_panel_state(&window);
    window.set_camera_position(CameraPositionSettings {
        sun_fixed: true,
        ..window.get_camera_position()
    });
    assert_eq!(rows_shown(&window), (0, 1));

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

// ---------------------------------------------------------------------------
// The Sun-fixed camera
// ---------------------------------------------------------------------------

const LONGITUDE_ROW: &str = "MainWindow::longitude-slider";
const SOLAR_TIME_ROW: &str = "MainWindow::solar-time-slider";

/// A window with the app's own callbacks behind it and the override on at a
/// pinned instant, so that every conversion "now" makes is the same one the
/// test makes. The receiver holds what the window pushed to the engine.
fn wired_window() -> (
    MainWindow,
    crossbeam_channel::Receiver<sunlit_core::engine::EngineCommand>,
) {
    let window = create_window();
    window.set_advanced_open(true);
    window.set_time_zone_utc(true);
    window.set_datetime(DateTimeSettings {
        use_custom: true,
        instant: "2026-04-30T09:15:00Z".into(),
        hour: 9.25,
        day_of_year: 120.0,
        year: 2026,
    });
    let (sender, sent) = crossbeam_channel::unbounded();
    let link =
        sunlit_earth::engine_client::EngineLink::new(sender, vec!["None".to_owned()], vec![1]);
    let screens = sunlit_earth::displays::shared_monitors();
    ui_callbacks::register_mouse_callbacks(&window, &link);
    ui_callbacks::register_change_callbacks(&window, &link);
    ui_callbacks::register_action_callbacks(&window, &link, &screens);
    sunlit_earth::time_bar::register(&window, &link);
    release_every_modifier(&window);
    (window, sent)
}

/// The camera the window's scene is drawn with at the instant it names.
fn resolved(window: &MainWindow) -> sunlit_core::scene::camera::CameraParams {
    let params = ui_callbacks::read_params_from_window(window, &AA_COUNTS);
    params.camera_at(&sunlit_core::scene::sky::compute_sky_state(&params.time))
}

/// `a` minus `b` in degrees of longitude, the short way round.
fn east_of(a: f32, b: f32) -> f32 {
    (a - b + 180.0).rem_euclid(360.0) - 180.0
}

fn rows_shown(window: &MainWindow) -> (usize, usize) {
    (
        ElementHandle::find_by_element_id(window, LONGITUDE_ROW).count(),
        ElementHandle::find_by_element_id(window, SOLAR_TIME_ROW).count(),
    )
}

/// The last scene the window pushed.
fn last_pushed(
    sent: &crossbeam_channel::Receiver<sunlit_core::engine::EngineCommand>,
) -> SceneParams {
    sent.try_iter()
        .filter_map(|command| match command {
            sunlit_core::engine::EngineCommand::UpdateParams(params) => Some(*params),
            _ => None,
        })
        .last()
        .expect("the window pushed its scene")
}

/// The check swaps the Longitude row for the Time of day row and back, and
/// the camera drawn before and after each switch is the same one.
#[test]
fn test_the_sun_fixed_check_swaps_the_rows_and_keeps_the_view() {
    let (window, sent) = wired_window();
    set_longitude(&window, 140.0);
    assert_eq!(rows_shown(&window), (1, 0));
    let check = the_button(&window, "Fixed toward the Sun");

    for sun_fixed in [true, false] {
        let before = resolved(&window);
        check.invoke_accessible_default_action();
        assert_eq!(window.get_camera_position().sun_fixed, sun_fixed);
        let expected_rows = if sun_fixed { (0, 1) } else { (1, 0) };
        assert_eq!(rows_shown(&window), expected_rows);
        let after = resolved(&window);
        assert!(
            east_of(after.longitude, before.longitude).abs() < 0.01,
            "switching to sun_fixed = {sun_fixed} moved the camera from {} to {}",
            before.longitude,
            after.longitude
        );
        approx::assert_relative_eq!(after.latitude, before.latitude);
        approx::assert_relative_eq!(after.zoom, before.zoom);
        assert_eq!(last_pushed(&sent).sun_fixed, sun_fixed);
    }
    approx::assert_relative_eq!(
        window.get_camera_position().longitude,
        140.0,
        epsilon = 0.01
    );
}

/// A preset clicked in the Sun-fixed mode keeps the mode and puts its place
/// beneath the camera at the instant it was clicked.
#[test]
fn test_a_preset_in_the_sun_fixed_mode_puts_its_place_beneath_the_camera_now() {
    use sunlit_core::scene::camera::PRESETS;

    let (window, sent) = wired_window();
    window.set_camera_position(CameraPositionSettings {
        longitude: 77.0,
        sun_fixed: true,
        solar_time: 3.0,
        ..window.get_camera_position()
    });
    let europe = PRESETS[0];
    window.invoke_apply_preset(0);

    let position = window.get_camera_position();
    assert!(position.sun_fixed, "the preset left the Sun-fixed mode");
    approx::assert_relative_eq!(position.longitude, 77.0);
    let camera = resolved(&window);
    assert!(
        east_of(camera.longitude, europe.longitude).abs() < 0.01,
        "the camera hangs over {} rather than the preset's {}",
        camera.longitude,
        europe.longitude
    );
    approx::assert_relative_eq!(camera.latitude, europe.latitude);
    approx::assert_relative_eq!(camera.zoom, europe.zoom);
    approx::assert_relative_eq!(camera.pitch_deg, europe.pitch_deg);
    let pushed = last_pushed(&sent);
    assert!(pushed.sun_fixed);
    approx::assert_relative_eq!(pushed.solar_time, position.solar_time);
}

/// The horizontal part of a globe drag moves the time of day by the longitude
/// the same drag moves an Earth-fixed camera by, over 15 degrees an hour,
/// leaves the longitude alone, and wraps across midnight.
#[test]
fn test_a_globe_drag_in_the_sun_fixed_mode_moves_the_time_of_day() {
    use sunlit_core::scene::camera::{held_longitude, solar_time_of_held};

    let start = 23.8;
    let (window, _sent) = wired_window();
    set_longitude(&window, held_longitude(start));
    window.invoke_mouse_drag_globe_begin();
    window.invoke_mouse_drag_globe(-120.0, 0.0);
    let turned = window.get_camera_position().longitude;
    assert!(
        east_of(turned, held_longitude(start)) > 3.0,
        "the premise: the drag turns the globe east past midnight's meridian, to {turned}"
    );

    let (window, _sent) = wired_window();
    window.set_camera_position(CameraPositionSettings {
        longitude: -33.0,
        sun_fixed: true,
        solar_time: start,
        ..window.get_camera_position()
    });
    window.invoke_mouse_drag_globe_begin();
    window.invoke_mouse_drag_globe(-120.0, 0.0);
    let position = window.get_camera_position();
    approx::assert_relative_eq!(position.longitude, -33.0);
    assert!(
        position.solar_time < start - 12.0,
        "the time of day did not wrap across midnight: {}",
        position.solar_time
    );
    approx::assert_relative_eq!(
        position.solar_time,
        solar_time_of_held(turned),
        epsilon = 1e-3
    );
}

/// Load Defaults puts the camera's mode and its time of day back with the
/// rest of Camera Position, and the Longitude row with them.
#[test]
fn test_load_defaults_leaves_the_sun_fixed_mode() {
    let (window, _sent) = wired_window();
    window.set_camera_position(CameraPositionSettings {
        sun_fixed: true,
        solar_time: 19.0,
        ..window.get_camera_position()
    });
    let defaults = ui_callbacks::scene_settings(&SceneParams::default()).camera_position;
    assert_ne!(window.get_camera_position(), defaults);

    window.invoke_load_defaults();

    assert_eq!(window.get_camera_position(), defaults);
    let expected_rows = if defaults.sun_fixed { (0, 1) } else { (1, 0) };
    assert_eq!(rows_shown(&window), expected_rows);
}

/// The Time of day row offers no reset, like the rest of Camera Position.
#[test]
fn test_the_time_of_day_offers_no_reset() {
    let window = create_window();
    window.set_advanced_open(true);
    window.set_camera_position(CameraPositionSettings {
        sun_fixed: true,
        ..window.get_camera_position()
    });
    let rows: Vec<_> = ElementHandle::find_by_element_id(&window, SOLAR_TIME_ROW).collect();
    assert_eq!(rows.len(), 1, "expected one {SOLAR_TIME_ROW}");
    move_off_its_value(&slider_of(&rows[0]));
    assert_eq!(resets_offered(&window), Vec::<String>::new());
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
        win.set_camera_position(CameraPositionSettings {
            longitude: preset.longitude,
            latitude: preset.latitude,
            zoom: preset.zoom,
            ..win.get_camera_position()
        });
        let mut orientation = win.get_camera_orientation();
        orientation.pitch = preset.pitch_deg;
        win.set_camera_orientation(orientation);
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
        let position = window.get_camera_position();
        approx::assert_relative_eq!(position.longitude, expected.longitude);
        approx::assert_relative_eq!(position.latitude, expected.latitude);
        approx::assert_relative_eq!(position.zoom, expected.zoom);
        approx::assert_relative_eq!(window.get_camera_orientation().pitch, expected.pitch_deg);
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

/// Scrolled as far as it goes, a tab shows the end of its document: the scroll
/// area is as tall as what the tab draws, wrapped blocks included.
#[test]
fn test_the_attributions_tab_scrolls_to_the_end_of_its_document() {
    let window = about_window();
    window.set_attributions(sunlit_earth::about::document_model(&wrapping_document()));
    materialize_about(&window);

    assert_the_last_block_is_reachable(&window, "AboutWindow::attributions-heading");
}

#[test]
fn test_the_third_party_tab_scrolls_to_the_end_of_its_document() {
    let window = about_window();
    window.set_third_party(sunlit_earth::about::document_model(&wrapping_document()));
    open_tab(&window, 2);

    assert_the_last_block_is_reachable(&window, "AboutWindow::third-party-heading");
}

/// Paragraphs and list items that each wrap over several lines, then one
/// heading, which is the only one and so marks the end.
fn wrapping_document() -> String {
    let long = "a block that runs on far enough to wrap over more than one line in a \
                five hundred and sixty pixel window, and then keeps going for a while";
    let mut document = format!("{long}\n\n- {long}\n  - {long}\n\n").repeat(20);
    document.push_str("# The end\n");
    document
}

fn assert_the_last_block_is_reachable(window: &sunlit_earth::AboutWindow, heading_id: &str) {
    for _ in 0..50 {
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(280.0, 350.0),
                delta_x: 0.0,
                delta_y: -1000.0,
            });
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(50));
    }

    let Some(heading) = all_elements(window, heading_id).pop() else {
        panic!("scrolled to the end, the last block is still out of reach");
    };
    let bottom = heading.absolute_position().y + heading.size().height;
    // The window is 480 tall and its layout keeps 16 below the tab.
    assert!(
        bottom <= 464.0,
        "scrolled to the end, the last block ends at {bottom}, below the tab"
    );
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

// ---------------------------------------------------------------------------
// The loading line
// ---------------------------------------------------------------------------

/// The line over the preview shows what the engine last said is loading, and
/// is gone from the tree while it says nothing.
#[test]
fn test_the_loading_line_shows_the_engines_status_and_only_while_there_is_one() {
    let window = create_window();
    let line = |window: &MainWindow| -> Vec<String> {
        ElementHandle::find_by_element_id(window, "MainWindow::loading-label")
            .filter_map(|label| label.accessible_label())
            .map(|text| text.to_string())
            .collect()
    };
    assert!(line(&window).is_empty(), "nothing is said at first");

    for status in ["Preparing March, 4 of 12", "Loading Day and Night...", ""] {
        window.set_loading_text(status.into());
        let shown = line(&window);
        if status.is_empty() {
            assert!(shown.is_empty(), "{shown:?}");
        } else {
            assert_eq!(shown, [status]);
        }
    }
}

// ---------------------------------------------------------------------------
// The camera panel over the preview
// ---------------------------------------------------------------------------

/// The one element with this id, which the panel always shows.
fn the_element(window: &MainWindow, id: &str) -> ElementHandle {
    let found: Vec<_> = ElementHandle::find_by_element_id(window, id).collect();
    assert_eq!(found.len(), 1, "expected exactly one '{id}'");
    found.into_iter().next().expect("one element")
}

/// A point inside an element, as fractions of its size from its top left.
fn point_in(element: &ElementHandle, fx: f32, fy: f32) -> slint::LogicalPosition {
    let at = element.absolute_position();
    let size = element.size();
    slint::LogicalPosition::new(at.x + size.width * fx, at.y + size.height * fy)
}

/// Press the left button at `position` and keep it down.
fn hold_at(window: &MainWindow, position: slint::LogicalPosition) {
    use slint::platform::{PointerEventButton, WindowEvent};
    let window = window.window();
    window.dispatch_event(WindowEvent::PointerMoved { position });
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
}

/// Let the left button go at `position`.
fn release_at(window: &MainWindow, position: slint::LogicalPosition) {
    use slint::platform::{PointerEventButton, WindowEvent};
    window
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
}

/// Let mock time pass in frame-sized steps, so every timer fires along the
/// way rather than once at the end.
fn let_pass(milliseconds: u64) {
    for _ in 0..milliseconds.div_ceil(16) {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

/// N eases the roll back to north up rather than jumping, and gets there.
#[test]
fn test_north_puts_the_roll_back_to_zero_once_the_ease_has_passed() {
    let (window, sent) = wired_window();
    let mut orientation = window.get_camera_orientation();
    orientation.tilt = 120.0;
    window.set_camera_orientation(orientation);

    press_at(
        &window,
        point_in(&the_element(&window, "RollRing::north-touch"), 0.5, 0.5),
    );
    let_pass(48);
    let midway = window.get_camera_orientation().tilt;
    assert!(
        midway > 0.0 && midway < 120.0,
        "after a moment the roll is on its way, at {midway}"
    );
    let_pass(400);
    approx::assert_relative_eq!(window.get_camera_orientation().tilt, 0.0);
    assert!(
        !window.get_camera_easing(),
        "the ease timer stops once it is there"
    );
    approx::assert_relative_eq!(last_pushed(&sent).camera.tilt_deg, 0.0);
}

/// Move the held pointer to `position`.
fn drag_to(window: &MainWindow, position: slint::LogicalPosition) {
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
}

/// A press on the N that wanders less than a drag before it is let go is a
/// click: it rolls nothing on the way and puts north up.
#[test]
fn test_a_click_on_the_n_puts_north_up_even_with_a_wobble() {
    for wobble in [0.0, 2.0] {
        let (window, _sent) = wired_window();
        let mut orientation = window.get_camera_orientation();
        orientation.tilt = 120.0;
        window.set_camera_orientation(orientation);
        let at = point_in(&the_element(&window, "RollRing::north-touch"), 0.5, 0.5);
        let to = slint::LogicalPosition::new(at.x + wobble, at.y + wobble);

        hold_at(&window, at);
        drag_to(&window, to);
        approx::assert_relative_eq!(window.get_camera_orientation().tilt, 120.0);
        release_at(&window, to);
        let_pass(400);
        approx::assert_relative_eq!(window.get_camera_orientation().tilt, 0.0);
    }
}

/// The ring's middle and the N's, in the window.
fn ring_and_north(window: &MainWindow) -> (slint::LogicalPosition, slint::LogicalPosition) {
    (
        point_in(&the_element(window, "CameraPanel::ring"), 0.5, 0.5),
        point_in(&the_element(window, "RollRing::north-touch"), 0.5, 0.5),
    )
}

/// The roll a drag from `from` 60 degrees clockwise around `middle` makes,
/// with the button let go where it ends.
fn roll_of_a_drag(
    window: &MainWindow,
    middle: slint::LogicalPosition,
    from: slint::LogicalPosition,
) -> f32 {
    let before = window.get_camera_orientation().tilt;
    let (dx, dy) = (from.x - middle.x, from.y - middle.y);
    let radius = dx.hypot(dy);
    let start = dx.atan2(-dy);
    let along = |degrees: f32| {
        let angle = start + degrees.to_radians();
        slint::LogicalPosition::new(
            middle.x + radius * angle.sin(),
            middle.y - radius * angle.cos(),
        )
    };
    hold_at(window, from);
    for step in 1..=12_u8 {
        drag_to(window, along(f32::from(step) * 5.0));
    }
    release_at(window, along(60.0));
    let_pass(400);
    window.get_camera_orientation().tilt - before
}

/// A drag that starts on the N rolls as a drag on the ring from the same
/// point does, and its release does not put north up.
#[test]
fn test_a_drag_from_the_n_rolls_like_one_on_the_ring() {
    let (window, _sent) = wired_window();
    let (middle, north) = ring_and_north(&window);
    let from_north = roll_of_a_drag(&window, middle, north);
    approx::assert_abs_diff_eq!(from_north.abs(), 60.0, epsilon = 0.5);
    approx::assert_abs_diff_eq!(
        window.get_camera_orientation().tilt,
        from_north,
        epsilon = 0.01
    );

    let (window, _sent) = wired_window();
    let mut orientation = window.get_camera_orientation();
    orientation.tilt = 90.0;
    window.set_camera_orientation(orientation);
    let (_, moved_north) = ring_and_north(&window);
    assert!(
        (moved_north.x - north.x).hypot(moved_north.y - north.y) > 20.0,
        "the N is out of the way"
    );
    let from_ring = roll_of_a_drag(&window, middle, north);
    approx::assert_abs_diff_eq!(from_north, from_ring, epsilon = 0.01);
}

/// The cursor the window shows, as Slint names it.
fn cursor(window: &MainWindow) -> String {
    let adapter = i_slint_core::window::WindowInner::from_pub(window.window()).window_adapter();
    let testing = adapter
        .internal(i_slint_core::InternalToken)
        .and_then(|internal| {
            (internal as &dyn std::any::Any)
                .downcast_ref::<i_slint_backend_testing::testing_backend::TestingWindow>()
        })
        .expect("a window of the testing backend");
    format!("{:?}", testing.mouse_cursor())
}

/// The ring, its N and the three sticks show the open hand under the pointer
/// and the closed one while held where the platform draws hands for them,
/// and the pointing hand where it does not.
#[test]
fn test_the_draggable_camera_controls_show_a_hand() {
    for hands in [true, false] {
        let (window, _sent) = wired_window();
        window
            .global::<sunlit_earth::OverlayStyle>()
            .set_grab_cursors(hands);
        let (hover, held) = if hands {
            ("BuiltIn(Grab)", "BuiltIn(Grabbing)")
        } else {
            ("BuiltIn(Pointer)", "BuiltIn(Pointer)")
        };
        let ring = the_element(&window, "CameraPanel::ring");
        let places = [
            ("the ring", point_in(&ring, 0.06, 0.5)),
            ("the N", ring_and_north(&window).1),
            (
                "the orbit stick",
                point_in(&the_element(&window, "CameraPanel::orbit"), 0.5, 0.5),
            ),
            (
                "the look stick",
                point_in(&the_element(&window, "CameraPanel::look"), 0.5, 0.5),
            ),
            (
                "the frame stick",
                point_in(&the_element(&window, "CameraPanel::frame"), 0.5, 0.5),
            ),
        ];
        for (what, at) in places {
            window
                .window()
                .dispatch_event(slint::platform::WindowEvent::PointerMoved { position: at });
            assert_eq!(cursor(&window), hover, "{what} under the pointer");
            hold_at(&window, at);
            let nudged = slint::LogicalPosition::new(at.x, at.y + 1.0);
            window
                .window()
                .dispatch_event(slint::platform::WindowEvent::PointerMoved { position: nudged });
            assert_eq!(cursor(&window), held, "{what} held");
            release_at(&window, nudged);
            let_pass(400);
        }
    }
}

/// Holding the orbit joystick up and to the right, clear of the arrows, keeps
/// the camera traveling east, and in the Sun-fixed mode it moves the time of
/// day instead and leaves the longitude alone.
#[test]
fn test_a_held_orbit_joystick_travels_east_in_either_mode() {
    for sun_fixed in [false, true] {
        let (window, _sent) = wired_window();
        window.set_camera_position(CameraPositionSettings {
            longitude: 10.0,
            sun_fixed,
            solar_time: 6.0,
            ..window.get_camera_position()
        });
        let stick = the_element(&window, "CameraPanel::orbit");
        let push = point_in(&stick, 0.72, 0.3);
        hold_at(&window, push);
        let mut readings = Vec::new();
        for _ in 0..5 {
            let_pass(48);
            let position = window.get_camera_position();
            readings.push(if sun_fixed {
                position.solar_time
            } else {
                position.longitude
            });
            if sun_fixed {
                approx::assert_relative_eq!(position.longitude, 10.0);
            }
        }
        release_at(&window, push);
        assert!(
            readings.windows(2).all(|pair| pair[1] > pair[0]),
            "sun_fixed = {sun_fixed}: the camera did not keep traveling east: {readings:?}"
        );
        let settled = window.get_camera_position();
        let_pass(100);
        assert_eq!(
            window.get_camera_position(),
            settled,
            "the camera stops when the knob is let go"
        );
    }
}

/// The look and frame ↺ badges are in the tree exactly while their values
/// differ from the defaults, and a click on one brings them back.
#[test]
fn test_the_look_and_frame_resets_show_while_modified_and_clear_it() {
    let (window, _sent) = wired_window();
    let badges = |window: &MainWindow| {
        (
            ElementHandle::find_by_element_id(window, "CameraPanel::look-reset").count(),
            ElementHandle::find_by_element_id(window, "CameraPanel::frame-reset").count(),
        )
    };
    assert_eq!(badges(&window), (0, 0), "nothing to reset at the defaults");

    let defaults = window.get_defaults();
    let mut orientation = window.get_camera_orientation();
    orientation.pitch = defaults.camera_orientation.pitch + 12.0;
    window.set_camera_orientation(orientation);
    assert_eq!(badges(&window), (1, 0));
    let mut framing = window.get_framing();
    framing.offset_x = defaults.framing.offset_x - 0.4;
    window.set_framing(framing);
    assert_eq!(badges(&window), (1, 1));

    for id in ["CameraPanel::look-reset", "CameraPanel::frame-reset"] {
        press_at(&window, point_in(&the_element(&window, id), 0.5, 0.5));
        let_pass(400);
    }
    assert_eq!(window.get_camera_orientation(), defaults.camera_orientation);
    assert_eq!(window.get_framing(), defaults.framing);
    assert_eq!(badges(&window), (0, 0));
}

/// The zoom slider takes the value under the pointer, except near the mark,
/// where it snaps to the default.
#[test]
fn test_the_zoom_slider_snaps_to_the_mark() {
    let (window, sent) = wired_window();
    let default = window.get_defaults().camera_position.zoom;
    let slider = the_element(&window, "CameraPanel::zoom-slider");
    let height = slider.size().height;
    let at = |zoom: f32| point_in(&slider, 0.5, (4.0 + zoom * (height - 8.0)) / height);

    press_at(&window, at(0.9));
    let far = window.get_camera_position().zoom;
    approx::assert_relative_eq!(far, 0.9, epsilon = 0.02);
    press_at(&window, at(default + 0.02));
    approx::assert_relative_eq!(window.get_camera_position().zoom, default);
    approx::assert_relative_eq!(last_pushed(&sent).camera.zoom, default);
}

/// A click on the globe, which is what gives the preview's shortcuts the
/// keyboard.
fn click_the_globe(window: &MainWindow) {
    let size = window.window().size().to_logical(1.0);
    press_at(
        window,
        slint::LogicalPosition::new(size.width - 300.0, size.height / 2.0),
    );
}

fn key_down(window: &MainWindow, text: impl Into<slint::SharedString>) {
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: text.into() });
}

fn key_up(window: &MainWindow, text: impl Into<slint::SharedString>) {
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::KeyReleased { text: text.into() });
}

/// Let go of every modifier key. Slint keeps their state for the thread, so
/// one a case leaves down would be down in the next case's window.
fn release_every_modifier(window: &MainWindow) {
    use slint::platform::Key;

    for key in [
        Key::Shift,
        Key::ShiftR,
        Key::Control,
        Key::ControlR,
        Key::Alt,
        Key::AltGr,
        Key::Meta,
        Key::MetaR,
    ] {
        key_up(window, key);
    }
}

fn tap(window: &MainWindow, text: impl Into<slint::SharedString>) {
    let text = text.into();
    key_down(window, text.clone());
    key_up(window, text);
}

/// F1 opens the sheet and Esc closes it, and so does a click outside its
/// card.
#[test]
fn test_f1_opens_the_shortcut_sheet_and_escape_closes_it() {
    use slint::platform::Key;

    let (window, _sent) = wired_window();
    let sheets = |window: &MainWindow| {
        ElementHandle::find_by_element_id(window, "MainWindow::shortcut-sheet").count()
    };
    click_the_globe(&window);
    assert_eq!(sheets(&window), 0);
    tap(&window, Key::F1);
    assert_eq!(sheets(&window), 1, "F1 opens the sheet");
    tap(&window, Key::Escape);
    assert_eq!(sheets(&window), 0, "Esc closes it");

    tap(&window, Key::F1);
    assert_eq!(sheets(&window), 1);
    press_at(
        &window,
        slint::LogicalPosition::new(window.window().size().to_logical(1.0).width - 20.0, 20.0),
    );
    assert_eq!(sheets(&window), 0, "a click outside the card closes it");
}

/// The sheet opened with the panel's ? button closes on Esc though a sidebar
/// control had the keyboard.
#[test]
fn test_escape_closes_the_sheet_the_help_button_opened() {
    use slint::platform::Key;

    let (window, _sent) = wired_window();
    let sheets = |window: &MainWindow| {
        ElementHandle::find_by_element_id(window, "MainWindow::shortcut-sheet").count()
    };
    let slider = slider_of(&the_element(&window, "MainWindow::latitude-slider"));
    press_at(&window, point_in(&slider, 0.5, 0.5));
    press_at(
        &window,
        point_in(&the_element(&window, "CameraPanel::help-button"), 0.5, 0.5),
    );
    assert_eq!(sheets(&window), 1, "the ? button opens the sheet");
    tap(&window, Key::Escape);
    assert_eq!(sheets(&window), 0, "Esc closes it");
}

/// A bare click on the globe and the keys teach nothing about the mouse, so
/// the first use of the orbit control still names its gesture; a drag of the
/// globe that moved does, and then the control shows no tip.
#[test]
fn test_only_a_mouse_gesture_that_moved_counts_as_used() {
    use slint::platform::{Key, PointerEventButton, WindowEvent};

    let tips = |window: &MainWindow| all_of(window, "GestureTip").len();
    let step = |window: &MainWindow| {
        press_at(
            window,
            point_in(&the_element(window, "CameraPanel::right-arrow"), 0.5, 0.5),
        );
    };

    let (window, _sent) = wired_window();
    click_the_globe(&window);
    tap(&window, Key::RightArrow);
    step(&window);
    assert_eq!(tips(&window), 1, "the tip names the left drag");

    let (window, _sent) = wired_window();
    let size = window.window().size().to_logical(1.0);
    let from = slint::LogicalPosition::new(size.width - 300.0, size.height / 2.0);
    let to = slint::LogicalPosition::new(from.x + 40.0, from.y);
    hold_at(&window, from);
    window
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: to });
    window
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position: to,
            button: PointerEventButton::Left,
        });
    step(&window);
    assert_eq!(tips(&window), 0, "the left drag was used already");
}

/// The first-use tip stays inside the preview, centered on it, and keeps its
/// text on itself: on one line in the default window, wrapped in the
/// narrowest one with the tip grown around the lines.
#[test]
fn test_the_gesture_tip_fits_the_preview() {
    let mut line_height = None;
    for width in [1000, 520] {
        let (window, _sent) = wired_window();
        window
            .window()
            .set_size(slint::PhysicalSize::new(width, 600));
        press_at(
            &window,
            point_in(&the_element(&window, "CameraPanel::right-arrow"), 0.5, 0.5),
        );
        let tips = all_of(&window, "GestureTip");
        assert_eq!(tips.len(), 1, "{width} px: the orbit control shows its tip");
        let tip = edges(&tips[0]);
        let preview = edges(&the_element(&window, "MainWindow::image-container"));
        assert!(
            tip[0] >= preview[0] && tip[2] <= preview[2],
            "{width} px: the tip {}..{} leaves the preview {}..{}",
            tip[0],
            tip[2],
            preview[0],
            preview[2]
        );
        approx::assert_abs_diff_eq!(
            f32::midpoint(tip[0], tip[2]),
            f32::midpoint(preview[0], preview[2]),
            epsilon = 0.5
        );
        for id in [
            "GestureTip::label",
            "GestureTip::glyph",
            "GestureTip::message",
        ] {
            let part = edges(&the_element(&window, id));
            assert!(
                part[0] >= tip[0] && part[1] >= tip[1] && part[2] <= tip[2] && part[3] <= tip[3],
                "{width} px: {id} at {part:?} is not on the tip at {tip:?}"
            );
        }
        let message = edges(&the_element(&window, "GestureTip::message"));
        let height = message[3] - message[1];
        match line_height {
            None => line_height = Some(height),
            Some(line) => assert!(
                height > 1.5 * line,
                "{width} px: the text keeps one line's height, {height} px"
            ),
        }
    }
}

/// H hides the overlays and shows them again, the corner button brings them
/// back too, and the choice is saved with the window's settings.
#[test]
fn test_h_hides_and_shows_the_overlays() {
    let (window, _sent) = wired_window();
    click_the_globe(&window);
    tap(&window, "h");
    assert!(window.get_overlays_hidden(), "H hides the overlays");
    let saved = ui_callbacks::read_config_from_window_onto(
        &window,
        &AA_COUNTS,
        &AppConfig::default(),
        false,
    );
    assert!(saved.overlays_hidden, "the hidden overlays are saved");
    tap(&window, "h");
    assert!(!window.get_overlays_hidden(), "H again shows them");

    tap(&window, "h");
    press_at(
        &window,
        point_in(&the_element(&window, "MainWindow::show-button"), 0.5, 0.5),
    );
    assert!(
        !window.get_overlays_hidden(),
        "the corner button shows them"
    );
}

/// The frame guides are off at every start, the panel's button and F toggle
/// them, H hides them with the other overlays, and nothing of them is saved.
#[test]
fn test_the_frame_guides_toggle_and_hide_with_the_overlays() {
    let (window, _sent) = wired_window();
    assert!(!window.get_guides_shown(), "the guides are off at start");
    assert!(!window.get_guides_visible());

    press_at(
        &window,
        point_in(
            &the_element(&window, "CameraPanel::guides-button"),
            0.5,
            0.5,
        ),
    );
    assert!(window.get_guides_shown(), "the button shows the guides");
    assert!(window.get_guides_visible());
    press_at(
        &window,
        point_in(
            &the_element(&window, "CameraPanel::guides-button"),
            0.5,
            0.5,
        ),
    );
    assert!(!window.get_guides_shown(), "the button hides them again");

    click_the_globe(&window);
    tap(&window, "f");
    assert!(window.get_guides_shown(), "F shows the guides");
    tap(&window, "h");
    assert!(
        window.get_guides_shown() && !window.get_guides_visible(),
        "H hides the guides with the overlays and leaves them on"
    );
    tap(&window, "h");
    assert!(window.get_guides_visible(), "H again brings them back");
    tap(&window, "f");
    assert!(!window.get_guides_shown(), "F again hides them");

    let read = |window: &MainWindow| {
        ui_callbacks::read_config_from_window_onto(window, &AA_COUNTS, &AppConfig::default(), false)
    };
    let off = read(&window);
    tap(&window, "f");
    assert_eq!(read(&window), off, "the guides are not a setting");
}

/// Show `frame` in the window as the engine would hand it over: its size,
/// the short edge it was framed on, and the screen it was framed for.
fn show_frame(window: &MainWindow, frame: (i32, i32), short_edge: f32, screen: Option<(i32, i32)>) {
    let (screen_width, screen_height) = screen.unwrap_or((0, 0));
    window.set_frame_width(frame.0);
    window.set_frame_height(frame.1);
    window.set_frame_short_edge(short_edge);
    window.set_frame_screen_width(screen_width);
    window.set_frame_screen_height(screen_height);
}

/// A guide's rectangle, `[x, y, width, height]`.
fn guide_rect(guide: &sunlit_earth::FrameGuide) -> [f32; 4] {
    [guide.x, guide.y, guide.width, guide.height]
}

/// The preview panel's size, as the guides are laid out in it.
fn container(window: &MainWindow) -> (f32, f32) {
    (window.get_viewport_width(), window.get_viewport_height())
}

/// The rectangle of `aspect` the guides should draw for a displayed short
/// edge `short_edge`, centered on the container.
fn expected_rect(window: &MainWindow, aspect: f32, short_edge: f32) -> [f32; 4] {
    let (width, height) = container(window);
    let (w, h) = if aspect >= 1.0 {
        (short_edge * aspect, short_edge)
    } else {
        (short_edge, short_edge / aspect)
    };
    [(width - w) / 2.0, (height - h) / 2.0, w, h]
}

fn assert_rect(actual: [f32; 4], expected: [f32; 4], what: &str) {
    for (a, e) in actual.iter().zip(expected) {
        assert!(
            (a - e).abs() < 0.5,
            "{what}: {actual:?}, expected {expected:?}"
        );
    }
}

/// A frame of the container's own shape, framed for `screen` the way the
/// engine frames the preview: the screen's rectangle as tall as the frame.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn frame_filling_the_container(window: &MainWindow, screen: Option<(i32, i32)>) -> f32 {
    let (width, height) = container(window);
    let frame = (width.round() as i32, height.round() as i32);
    let short_edge = match screen {
        Some((w, h)) => frame.1 as f32 * (w as f32 / h as f32).min(1.0),
        None => frame.1 as f32,
    };
    show_frame(window, frame, short_edge, screen);
    short_edge * height / frame.1 as f32
}

/// Where the guides sit: each aspect's rectangle has its short side the
/// displayed short edge and is centered, a landscape screen's as tall as the
/// frame in a wide panel and a narrow one, a portrait screen's too, a screen
/// within 1% of 16:9 sharing that rectangle and its labels, and no screen
/// leaving only 16:9 and 4:3.
#[test]
#[allow(clippy::cast_precision_loss)]
fn test_the_frame_guides_outline_each_aspect_on_the_displayed_short_edge() {
    let window = create_window();
    window.set_guides_shown(true);

    for (size, what) in [
        ((3000, 900), "a wide panel"),
        ((1200, 1400), "a narrow panel"),
    ] {
        window
            .window()
            .set_size(slint::PhysicalSize::new(size.0, size.1));
        let short_edge = frame_filling_the_container(&window, Some((2560, 1440)));
        let (width, height) = container(&window);
        approx::assert_relative_eq!(window.get_displayed_short_edge(), height, epsilon = 0.5);
        let screen = window.get_guide_screen();
        assert!(screen.shown, "{what}");
        assert_rect(
            guide_rect(&screen),
            expected_rect(&window, 2560.0 / 1440.0, short_edge),
            what,
        );
        approx::assert_relative_eq!(screen.height, height, epsilon = 0.5);
        assert_eq!(screen.label, "Screen 2560 × 1440");
        if what == "a narrow panel" {
            assert!(
                screen.width > width,
                "the screen's sides are clipped in {what}"
            );
        }
        // 2560x1440 is 16:9, so the screen's rectangle carries both labels,
        // a row apart.
        let wide = window.get_guide_wide();
        assert!(
            !wide.shown,
            "{what}: 16:9 is drawn as the screen's rectangle"
        );
        approx::assert_relative_eq!(wide.label_x, screen.label_x);
        assert!(wide.label_y > screen.label_y + 10.0, "{what}");
        let classic = window.get_guide_classic();
        assert!(classic.shown);
        assert_rect(
            guide_rect(&classic),
            expected_rect(&window, 4.0 / 3.0, short_edge),
            what,
        );
        assert!(classic.label_y > wide.label_y, "{what}: the labels stack");
    }

    // A portrait screen: its rectangle as tall as the frame, its width the
    // short edge.
    window
        .window()
        .set_size(slint::PhysicalSize::new(2400, 1000));
    let short_edge = frame_filling_the_container(&window, Some((1080, 1920)));
    let (_, height) = container(&window);
    let screen = window.get_guide_screen();
    assert_rect(
        guide_rect(&screen),
        expected_rect(&window, 1080.0 / 1920.0, short_edge),
        "a portrait screen",
    );
    approx::assert_relative_eq!(screen.height, height, epsilon = 0.5);
    assert!(window.get_guide_wide().shown && window.get_guide_classic().shown);

    // Within 1% of 16:9 is 16:9.
    frame_filling_the_container(&window, Some((1366, 768)));
    assert!(
        !window.get_guide_wide().shown,
        "1366x768 shares 16:9's rectangle"
    );
    frame_filling_the_container(&window, Some((1920, 1200)));
    assert!(window.get_guide_wide().shown, "16:10 is not 16:9");

    // No screen: only 16:9 and 4:3, labelled from the top row.
    let short_edge = frame_filling_the_container(&window, None);
    assert!(!window.get_guide_screen().shown);
    let (wide, classic) = (window.get_guide_wide(), window.get_guide_classic());
    assert!(wide.shown && classic.shown);
    assert_rect(
        guide_rect(&wide),
        expected_rect(&window, 16.0 / 9.0, short_edge),
        "no screen",
    );
    assert!(classic.label_y > wide.label_y);
}

/// The guides follow the frame shown rather than the panel: a frame drawn
/// for another panel shape, shown with `cover` while a resize settles, puts
/// the displayed short edge where `cover` puts that frame's.
#[test]
fn test_the_frame_guides_follow_the_frame_shown_while_a_resize_settles() {
    let window = create_window();
    window
        .window()
        .set_size(slint::PhysicalSize::new(2000, 1000));
    let (width, height) = container(&window);
    // A square frame from before the panel was widened: `cover` scales it to
    // the panel's width and crops its top and bottom.
    show_frame(&window, (500, 500), 500.0, Some((1920, 1080)));
    let scale = (width / 500.0).max(height / 500.0);
    approx::assert_relative_eq!(
        window.get_displayed_short_edge(),
        500.0 * scale,
        epsilon = 0.5
    );
    assert_rect(
        guide_rect(&window.get_guide_screen()),
        expected_rect(&window, 16.0 / 9.0, 500.0 * scale),
        "the old frame",
    );
}

/// A held arrow orbits the camera for as long as it is held, and with Shift
/// held it swings the look instead.
#[test]
fn test_held_arrows_orbit_and_with_shift_look() {
    use slint::platform::Key;

    let (window, _sent) = wired_window();
    click_the_globe(&window);
    let start = window.get_camera_position();
    key_down(&window, Key::RightArrow);
    let_pass(200);
    key_up(&window, Key::RightArrow);
    let turned = window.get_camera_position();
    assert!(
        turned.longitude > start.longitude + 1.0,
        "the right arrow travels east: {} to {}",
        start.longitude,
        turned.longitude
    );
    let_pass(100);
    assert_eq!(window.get_camera_position(), turned, "and stops on release");

    let pitch = window.get_camera_orientation().pitch;
    key_down(&window, Key::Shift);
    key_down(&window, Key::UpArrow);
    let_pass(200);
    key_up(&window, Key::UpArrow);
    key_up(&window, Key::Shift);
    assert!(window.get_camera_orientation().pitch > pitch + 1.0);
    assert_eq!(
        window.get_camera_position(),
        turned,
        "Shift leaves the orbit alone"
    );
}

/// Every number of the camera the keys move.
fn camera_numbers(window: &MainWindow) -> [f32; 7] {
    let position = window.get_camera_position();
    let orientation = window.get_camera_orientation();
    let framing = window.get_framing();
    [
        position.longitude,
        position.latitude,
        orientation.tilt,
        orientation.yaw,
        orientation.pitch,
        framing.offset_x,
        framing.offset_y,
    ]
}

/// The camera after `key` is held for a fifth of a second with `modifier`
/// held around it, in a window of its own.
fn camera_after_holding(
    modifier: Option<slint::platform::Key>,
    key: impl Into<slint::SharedString>,
) -> [f32; 7] {
    let (window, _sent) = wired_window();
    click_the_globe(&window);
    let key = key.into();
    if let Some(modifier) = modifier {
        key_down(&window, modifier);
    }
    key_down(&window, key.clone());
    let_pass(200);
    key_up(&window, key);
    if let Some(modifier) = modifier {
        key_up(&window, modifier);
    }
    let_pass(100);
    camera_numbers(&window)
}

/// W, A, S and D move the camera exactly as the up, left, down and right
/// arrows do, alone, with Shift (which gives the capital) and with Ctrl.
#[test]
fn test_wasd_moves_the_camera_as_the_arrows_do() {
    use slint::platform::Key;

    let still = {
        let (window, _sent) = wired_window();
        camera_numbers(&window)
    };
    for (modifier, what) in [
        (None, "alone"),
        (Some(Key::Shift), "with Shift"),
        (Some(Key::Control), "with Ctrl"),
    ] {
        for (arrow, letter) in [
            (Key::UpArrow, "w"),
            (Key::LeftArrow, "a"),
            (Key::DownArrow, "s"),
            (Key::RightArrow, "d"),
        ] {
            let letter = if modifier == Some(Key::Shift) {
                letter.to_uppercase()
            } else {
                letter.to_owned()
            };
            let by_arrow = camera_after_holding(modifier, arrow);
            let by_letter = camera_after_holding(modifier, letter.as_str());
            assert!(
                by_arrow
                    .iter()
                    .zip(still)
                    .any(|(a, b)| (a - b).abs() > 1e-3),
                "{arrow:?} {what} moves the camera"
            );
            for (a, b) in by_arrow.iter().zip(by_letter) {
                approx::assert_relative_eq!(*a, b, epsilon = 1e-4);
            }
        }
    }
}

/// Leaving the Sun-fixed mode while an orbit step eases the time of day ends
/// the ease: the longitude the switch converted stays, rather than taking
/// the rest of the eased time of day as degrees.
#[test]
fn test_the_sun_fixed_switch_ends_an_orbit_ease() {
    let (window, _sent) = wired_window();
    window.set_camera_position(CameraPositionSettings {
        longitude: 10.0,
        sun_fixed: true,
        solar_time: 6.0,
        ..window.get_camera_position()
    });
    window.invoke_camera_orbit_step(1, 0);
    let_pass(48);
    assert!(
        window.get_camera_position().solar_time > 6.0,
        "the premise: the step is on its way"
    );
    the_button(&window, "Fixed toward the Sun").invoke_accessible_default_action();
    let switched = window.get_camera_position();
    assert!(!switched.sun_fixed);
    let_pass(400);
    assert_eq!(window.get_camera_position(), switched);
    assert!(!window.get_camera_easing());
}

/// A drag of the globe while an orbit step eases ends the ease, and the
/// camera stays where the drag put it.
#[test]
fn test_a_globe_drag_ends_an_orbit_ease() {
    let (window, _sent) = wired_window();
    set_longitude(&window, 10.0);
    window.invoke_camera_orbit_step(1, 0);
    let_pass(48);
    assert!(
        window.get_camera_position().longitude > 10.0,
        "the premise: the step is on its way"
    );
    window.invoke_mouse_drag_globe_begin();
    window.invoke_mouse_drag_globe(120.0, 0.0);
    let dragged = window.get_camera_position();
    let_pass(400);
    assert_eq!(window.get_camera_position(), dragged);
    assert!(!window.get_camera_easing());
}

// ---------------------------------------------------------------------------
// The Date / Time group on the time setting
// ---------------------------------------------------------------------------

/// A slider of the group moved: the instant is read from the three in the
/// displayed zone, pushed as the time setting, and the labels name the zone.
#[test]
fn test_the_date_time_sliders_set_the_instant_in_the_displayed_zone() {
    use sunlit_core::scene::scene_time::SceneTime;
    use time::macros::datetime;

    let (window, sent) = wired_window();
    let mut settings = window.get_datetime();
    settings.hour = 10.5;
    settings.year = 2024;
    settings.day_of_year = 60.0;
    window.set_datetime(settings);
    window.invoke_datetime_edited();

    let pushed = last_pushed(&sent).time;
    assert_eq!(pushed, SceneTime::fixed(datetime!(2024-02-29 10:30 UTC)));
    assert_eq!(
        window.get_datetime().instant.as_str(),
        "2024-02-29T10:30:00Z"
    );
    assert_eq!(window.get_hour_label().as_str(), "10:30 UTC");
    assert_eq!(window.get_day_label().as_str(), "Thu 29 Feb");
    assert_eq!(window.get_max_day_of_year(), 366);

    let saved = ui_callbacks::read_config_from_window_onto(
        &window,
        &AA_COUNTS,
        &AppConfig::default(),
        false,
    );
    assert_eq!(saved.scene_time, pushed);
    assert_eq!(saved.time_zone, sunlit_core::config::TimeZoneChoice::Utc);
}

/// The override off is live time, and turning it on fixes the instant it was
/// turned on at.
#[test]
fn test_the_override_fixes_now_and_off_is_live() {
    use sunlit_core::scene::scene_time::SceneTime;

    let (window, sent) = wired_window();
    let mut settings = window.get_datetime();
    settings.use_custom = false;
    window.set_datetime(settings);
    window.invoke_sliders_changed();
    assert_eq!(last_pushed(&sent).time, SceneTime::Live);

    let before = time::OffsetDateTime::now_utc();
    let mut settings = window.get_datetime();
    settings.use_custom = true;
    window.set_datetime(settings);
    window.invoke_datetime_override_toggled();
    let SceneTime::Fixed(at) = last_pushed(&sent).time else {
        panic!("the override fixes a time");
    };
    let after = time::OffsetDateTime::now_utc();
    assert!(
        before - time::Duration::seconds(1) <= at && at <= after,
        "{at} is not the moment the override went on"
    );
}

// ---------------------------------------------------------------------------
// The time bar
// ---------------------------------------------------------------------------

use sunlit_core::engine::excursion::{ExcursionCommand, ExcursionKind, ExcursionState};

/// The excursion commands the window sent, in order, and the last scene it
/// pushed among them.
fn excursion_commands(
    sent: &crossbeam_channel::Receiver<sunlit_core::engine::EngineCommand>,
) -> (Vec<ExcursionCommand>, Option<SceneParams>) {
    let mut commands = Vec::new();
    let mut pushed = None;
    for command in sent.try_iter() {
        match command {
            sunlit_core::engine::EngineCommand::Excursion(command) => commands.push(command),
            sunlit_core::engine::EngineCommand::UpdateParams(params) => pushed = Some(*params),
            _ => {}
        }
    }
    (commands, pushed)
}

/// Rest the pointer on the folded bar, which opens it.
fn open_the_bar(window: &MainWindow) {
    let bar = the_element(window, "MainWindow::time-bar");
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: point_in(&bar, 0.5, 0.5),
        });
    assert!(bar.size().height > 100.0, "hovering the pill opens the bar");
}

/// The one button labeled `label`.
fn the_labeled(window: &MainWindow, label: &str) -> ElementHandle {
    let found: Vec<_> = ElementHandle::find_by_accessible_label(window, label)
        .filter(|element| element.accessible_role() == Some(AccessibleRole::Button))
        .collect();
    assert_eq!(found.len(), 1, "expected exactly one '{label}'");
    found.into_iter().next().expect("one element")
}

/// A ruler's dots from its left end.
fn ruler_dot_elements(ruler: &ElementHandle) -> Vec<ElementHandle> {
    let mut dots = ruler
        .query_descendants()
        .match_id("SpeedRuler::dot")
        .find_all();
    dots.sort_by(|a, b| a.absolute_position().x.total_cmp(&b.absolute_position().x));
    dots
}

/// The middle of dot `slot` of a ruler, counted from its left end.
fn ruler_dot(ruler: &ElementHandle, slot: usize) -> slint::LogicalPosition {
    point_in(&ruler_dot_elements(ruler)[slot], 0.5, 0.5)
}

fn ruler_dots(ruler: &ElementHandle) -> usize {
    ruler_dot_elements(ruler).len()
}

fn paused(rate: i32) -> ExcursionState {
    ExcursionState {
        running: false,
        rate,
        kind: ExcursionKind::Clock,
    }
}

/// A dot of the forward ruler runs the excursion at its rate, and the time
/// setting stays where it was.
#[test]
fn test_a_forward_ruler_dot_runs_at_its_rate_and_leaves_the_setting() {
    let (window, sent) = wired_window();
    let instant = window.get_datetime().instant;
    open_the_bar(&window);
    let _ = excursion_commands(&sent);

    let forward = the_element(&window, "TimeBar::forward");
    press_at(&window, ruler_dot(&forward, 4));
    let rate = i32::try_from(ExcursionKind::Clock.ladder()[4]).expect("a rate");
    let (commands, pushed) = excursion_commands(&sent);
    assert_eq!(
        commands,
        [ExcursionCommand::Play {
            rate,
            kind: ExcursionKind::Clock
        }]
    );
    assert!(pushed.is_none(), "a speed pushed the scene");
    assert_eq!(window.get_datetime().instant, instant);
}

/// The radio labels choose what play runs, and the rulers carry that kind's
/// speeds; going over to the date while paused asks nothing of the engine.
#[test]
fn test_the_radio_labels_switch_the_rulers_ladder() {
    let (window, sent) = wired_window();
    open_the_bar(&window);
    let forward = |window: &MainWindow| the_element(window, "TimeBar::forward");
    assert_eq!(
        ruler_dots(&forward(&window)),
        ExcursionKind::Clock.ladder().len()
    );

    let _ = excursion_commands(&sent);
    press_at(&window, point_in(&the_labeled(&window, "DATE"), 0.5, 0.5));
    assert_eq!(
        ruler_dots(&forward(&window)),
        ExcursionKind::Date.ladder().len()
    );
    assert!(window.get_time_bar().date_kind);
    assert_eq!(excursion_commands(&sent).0, []);

    press_at(&window, point_in(&the_labeled(&window, "Play"), 0.5, 0.5));
    let rate = i32::try_from(ExcursionKind::Date.default_rate()).expect("a rate");
    assert_eq!(
        excursion_commands(&sent).0,
        [ExcursionCommand::Play {
            rate,
            kind: ExcursionKind::Date
        }]
    );
}

/// A press on the time track sets the time setting to that time of the day
/// on screen, which during an excursion is the excursion's day, and ends the
/// excursion; a drag carries on from there.
#[test]
fn test_a_track_drag_sets_the_time_setting() {
    use sunlit_core::scene::scene_time::SceneTime;
    use time::macros::datetime;

    let (window, sent) = wired_window();
    sunlit_earth::time_bar::excursion_changed(&window, Some(paused(60)));
    sunlit_earth::time_bar::frame_shown(&window, datetime!(2026-05-10 03:00 UTC));
    open_the_bar(&window);
    let _ = excursion_commands(&sent);

    let track = the_element(&window, "TimeBar::day-track");
    hold_at(&window, point_in(&track, 0.5, 0.5));
    let (commands, pushed) = excursion_commands(&sent);
    assert_eq!(
        pushed.expect("the press pushed the scene").time,
        SceneTime::fixed(datetime!(2026-05-10 12:00 UTC))
    );
    assert_eq!(commands, [ExcursionCommand::End]);

    let to = point_in(&track, 0.75, 0.5);
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved { position: to });
    release_at(&window, to);
    let (commands, pushed) = excursion_commands(&sent);
    assert_eq!(
        pushed.expect("the drag pushed the scene").time,
        SceneTime::fixed(datetime!(2026-05-10 18:00 UTC))
    );
    assert_eq!(commands, [], "the excursion had ended already");
}

/// Now puts the time setting back to live and ends any excursion, and so
/// does 0.
#[test]
fn test_now_sends_live_and_end() {
    use sunlit_core::scene::scene_time::SceneTime;

    let (window, sent) = wired_window();
    open_the_bar(&window);
    let _ = excursion_commands(&sent);
    press_at(&window, point_in(&the_labeled(&window, "Now"), 0.5, 0.5));
    let (commands, pushed) = excursion_commands(&sent);
    assert_eq!(pushed.expect("Now pushed the scene").time, SceneTime::Live);
    assert_eq!(commands, [ExcursionCommand::End]);
    assert!(window.get_time_bar().live);

    let (window, sent) = wired_window();
    click_the_globe(&window);
    let _ = excursion_commands(&sent);
    tap(&window, "0");
    let (commands, pushed) = excursion_commands(&sent);
    assert_eq!(pushed.expect("0 pushed the scene").time, SceneTime::Live);
    assert_eq!(commands, [ExcursionCommand::End]);
}

/// During an excursion the clock block says Preview, and a click on it ends
/// the excursion and leaves the time setting alone.
#[test]
fn test_the_preview_pill_ends_the_excursion() {
    let (window, sent) = wired_window();
    sunlit_earth::time_bar::excursion_changed(&window, Some(paused(60)));
    assert_eq!(window.get_time_bar().state_text.as_str(), "Preview");
    open_the_bar(&window);
    let _ = excursion_commands(&sent);
    press_at(
        &window,
        point_in(&the_labeled(&window, "Preview"), 0.5, 0.5),
    );
    let (commands, pushed) = excursion_commands(&sent);
    assert_eq!(commands, [ExcursionCommand::End]);
    assert!(pushed.is_none());
}

/// The bar folds 0.7 s after the pointer leaves it, unless it is pinned.
#[test]
fn test_the_pin_keeps_the_bar_open_with_the_pointer_outside() {
    let (window, _sent) = wired_window();
    let height = |window: &MainWindow| the_element(window, "MainWindow::time-bar").size().height;
    let away = || slint::platform::WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(900.0, 300.0),
    };

    open_the_bar(&window);
    window.window().dispatch_event(away());
    let_pass(1000);
    assert!(
        height(&window) < 40.0,
        "the bar folds once the pointer left"
    );

    open_the_bar(&window);
    press_at(
        &window,
        point_in(&the_labeled(&window, "Keep open"), 0.5, 0.5),
    );
    assert!(window.get_time_bar_pinned());
    window.window().dispatch_event(away());
    let_pass(1000);
    assert!(height(&window) > 100.0, "the pinned bar folded");
}

/// The sidebar's Date / Time group and the bar show the same instant in the
/// same zone, before and after the zone chip switches it.
#[test]
fn test_the_sidebar_group_and_the_bar_show_the_same_instant() {
    let (window, _sent) = wired_window();
    let agree = |window: &MainWindow| {
        let view = window.get_time_bar();
        assert_eq!(
            window.get_hour_label().as_str(),
            format!("{} {}", view.time_text, view.zone_text)
        );
        assert!(
            view.date_text.starts_with(window.get_day_label().as_str()),
            "{} against {}",
            view.date_text,
            window.get_day_label()
        );
    };
    window.invoke_sliders_changed();
    assert_eq!(window.get_time_bar().time_text.as_str(), "09:15");
    agree(&window);
    window.invoke_time_zone_toggled();
    assert!(!window.get_time_zone_utc());
    agree(&window);
}

/// Space plays and pauses through the engine, J and L shuttle, K pauses, the
/// comma and the period skip, with Shift by the bigger step.
#[test]
fn test_the_time_keys_drive_the_transport() {
    use slint::platform::Key;
    use sunlit_core::engine::excursion::TimeStep;

    let play = |rate| ExcursionCommand::Play {
        rate,
        kind: ExcursionKind::Clock,
    };
    let step = |forward, big| {
        ExcursionCommand::Skip(TimeStep {
            kind: ExcursionKind::Clock,
            forward,
            big,
        })
    };
    let (window, sent) = wired_window();
    click_the_globe(&window);
    let _ = excursion_commands(&sent);
    tap(&window, " ");
    tap(&window, "l");
    tap(&window, ",");
    key_down(&window, Key::Shift);
    tap(&window, ">");
    key_up(&window, Key::Shift);
    assert_eq!(
        excursion_commands(&sent).0,
        [play(60), play(60), step(false, false), step(true, true)]
    );

    sunlit_earth::time_bar::excursion_changed(
        &window,
        Some(ExcursionState {
            running: true,
            rate: 60,
            kind: ExcursionKind::Clock,
        }),
    );
    assert_eq!(
        window.get_time_bar().state_text.as_str(),
        "\u{25b6} 60\u{d7}"
    );
    tap(&window, "l");
    tap(&window, "k");
    assert_eq!(
        excursion_commands(&sent).0,
        [play(120), ExcursionCommand::Pause]
    );
}

/// The rates of the commands to play, in the order they were sent.
fn rates_played(
    sent: &crossbeam_channel::Receiver<sunlit_core::engine::EngineCommand>,
) -> Vec<i32> {
    excursion_commands(sent)
        .0
        .into_iter()
        .filter_map(|command| match command {
            ExcursionCommand::Play { rate, .. } => Some(rate),
            _ => None,
        })
        .collect()
}

/// The engine reporting play running on the clock at `rate`.
fn running_at(window: &MainWindow, rate: i32) {
    sunlit_earth::time_bar::excursion_changed(
        window,
        Some(ExcursionState {
            running: true,
            rate,
            kind: ExcursionKind::Clock,
        }),
    );
}

/// The clock's speeds, slowest first.
fn clock_ladder() -> Vec<i32> {
    ExcursionKind::Clock
        .ladder()
        .iter()
        .map(|&rate| i32::try_from(rate).expect("a rate"))
        .collect()
}

/// A held L steps once at once, then again after a pause and at a steady
/// pace after that, as further presses would, and stops when let go; held
/// on, it stops by itself at the fastest speed, and when the window loses
/// the keyboard.
#[test]
fn test_holding_l_keeps_shuttling_until_let_go_or_the_fastest() {
    let ladder = clock_ladder();
    let (window, sent) = wired_window();
    click_the_globe(&window);
    running_at(&window, ladder[1]);
    let _ = excursion_commands(&sent);

    key_down(&window, "l");
    assert_eq!(rates_played(&sent), [ladder[2]], "the press steps at once");
    let_pass(300);
    assert_eq!(rates_played(&sent), [], "and waits before it repeats");
    let_pass(200);
    assert_eq!(rates_played(&sent), [ladder[3]], "then steps again");
    let_pass(200);
    assert_eq!(rates_played(&sent), [ladder[4]], "and again");
    key_up(&window, "l");
    let_pass(1000);
    assert_eq!(rates_played(&sent), [], "until it is let go");

    running_at(&window, ladder[1]);
    key_down(&window, "l");
    let_pass(400 + 200 * 20);
    let mut expected = ladder[2..].to_vec();
    expected.push(ladder[ladder.len() - 1]);
    assert_eq!(
        rates_played(&sent),
        expected,
        "held on, it stops once a step changes nothing"
    );
    key_up(&window, "l");

    running_at(&window, ladder[1]);
    key_down(&window, "l");
    let_pass(300);
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::WindowActiveChanged(false));
    let_pass(1000);
    assert_eq!(
        rates_played(&sent),
        [ladder[2]],
        "and it stops when the window loses the keyboard"
    );
}

/// The camera's position, orientation and framing.
fn camera(
    window: &MainWindow,
) -> (
    CameraPositionSettings,
    sunlit_earth::CameraOrientationSettings,
    FramingSettings,
) {
    (
        window.get_camera_position(),
        window.get_camera_orientation(),
        window.get_framing(),
    )
}

/// A held key let go while Ctrl, Alt or the Windows key (⌘ on macOS) is
/// down stops what it moved, though those modifiers send a press of most of
/// these keys to the window; and so does one let go after Shift or Alt
/// changed the text it gives, as Shift turns - into _. A key held by its
/// text has stopped at Alt's press already; an arrow stops at its release.
#[test]
fn test_a_held_key_let_go_under_a_modifier_stops_the_camera() {
    use slint::SharedString;
    use slint::platform::Key;

    let held: Vec<SharedString> = vec![
        Key::LeftArrow.into(),
        Key::RightArrow.into(),
        Key::UpArrow.into(),
        Key::DownArrow.into(),
        "w".into(),
        "a".into(),
        "s".into(),
        "d".into(),
        "q".into(),
        "e".into(),
        "+".into(),
        "=".into(),
        "-".into(),
    ];
    let mut cases: Vec<(SharedString, Key, SharedString)> = Vec::new();
    for modifier in [Key::Control, Key::Alt, Key::Meta] {
        cases.extend(held.iter().map(|key| (key.clone(), modifier, key.clone())));
    }
    cases.extend([
        ("-".into(), Key::Shift, "_".into()),
        ("+".into(), Key::Shift, "*".into()),
        ("q".into(), Key::Alt, "œ".into()),
    ]);
    for (pressed, modifier, released) in cases {
        let what = format!("{pressed:?} let go as {released:?} with {modifier:?} down");
        let (window, _sent) = wired_window();
        click_the_globe(&window);
        let still = camera(&window);
        key_down(&window, pressed.clone());
        let_pass(50);
        assert_ne!(camera(&window), still, "{what}: the press moves the camera");
        key_down(&window, modifier);
        key_up(&window, released);
        key_up(&window, modifier);
        let stopped = camera(&window);
        let_pass(300);
        assert_eq!(camera(&window), stopped, "{what}: the camera keeps moving");
    }
}

/// A held J or L let go while a modifier is down stops stepping; under Alt,
/// Alt's press has stopped it already.
#[test]
fn test_a_shuttle_key_let_go_under_a_modifier_stops_stepping() {
    use slint::platform::Key;

    let ladder = clock_ladder();
    for key in ["j", "l"] {
        for modifier in [Key::Control, Key::Alt, Key::Meta, Key::Shift] {
            let what = format!("{key} let go with {modifier:?} down");
            let (window, sent) = wired_window();
            click_the_globe(&window);
            running_at(&window, ladder[ladder.len() / 2]);
            let _ = excursion_commands(&sent);
            key_down(&window, key);
            assert_eq!(
                excursion_commands(&sent).0.len(),
                1,
                "{what}: the press steps"
            );
            key_down(&window, modifier);
            if modifier == Key::Shift {
                key_up(&window, key.to_uppercase());
            } else {
                key_up(&window, key);
            }
            key_up(&window, modifier);
            let_pass(1500);
            assert_eq!(excursion_commands(&sent).0, [], "{what}: it keeps stepping");
        }
    }
}

/// A held key let go after `AltGr` changed the text it gives, as on a German
/// layout `AltGr` turns q into @, e into € and + into ~, stops what it moved,
/// with `AltGr` alone and with Ctrl held as well: `AltGr`'s press stops it
/// before the release comes.
#[test]
fn test_a_held_key_let_go_under_altgr_stops_the_camera() {
    use slint::platform::Key;

    let cases: [(&str, &[Key], &str); 3] = [
        ("q", &[Key::AltGr], "@"),
        ("e", &[Key::Control, Key::AltGr], "€"),
        ("+", &[Key::AltGr], "~"),
    ];
    for (pressed, modifiers, released) in cases {
        let what = format!("{pressed:?} let go as {released:?} with {modifiers:?} down");
        let (window, _sent) = wired_window();
        click_the_globe(&window);
        let still = camera(&window);
        key_down(&window, pressed);
        let_pass(50);
        assert_ne!(camera(&window), still, "{what}: the press moves the camera");
        for &modifier in modifiers {
            key_down(&window, modifier);
        }
        key_up(&window, released);
        for &modifier in modifiers.iter().rev() {
            key_up(&window, modifier);
        }
        let stopped = camera(&window);
        let_pass(300);
        assert_eq!(camera(&window), stopped, "{what}: the camera keeps moving");
    }
}

/// A held key let go after Ctrl and Alt went down and changed the text it
/// gives as `AltGr` would, as Windows lets left Ctrl + left Alt do on a
/// German layout or a US International one, where they turn w into å, stops
/// what it moved, though on Windows Slint then reports neither Ctrl nor Alt
/// in the release's modifiers; and so does one pressed under Alt alone. Alt's
/// press, or Ctrl's joining it, stops the key before the release comes.
#[test]
fn test_a_held_key_let_go_under_ctrl_and_alt_as_altgr_stops_the_camera() {
    use slint::platform::Key;

    let cases: [(&[Key], &str, &[Key], &str); 4] = [
        (&[], "q", &[Key::Control, Key::Alt], "@"),
        (&[], "e", &[Key::Control, Key::Alt], "€"),
        (&[], "+", &[Key::Control, Key::Alt], "~"),
        (&[Key::Alt], "w", &[Key::Control], "å"),
    ];
    for (before, pressed, after, released) in cases {
        let what = format!(
            "{pressed:?} pressed with {before:?} down, let go as {released:?} after {after:?}"
        );
        let (window, _sent) = wired_window();
        click_the_globe(&window);
        let still = camera(&window);
        for &modifier in before {
            key_down(&window, modifier);
        }
        key_down(&window, pressed);
        let_pass(50);
        assert_ne!(camera(&window), still, "{what}: the press moves the camera");
        for &modifier in after {
            key_down(&window, modifier);
        }
        key_up(&window, released);
        let stopped = camera(&window);
        let_pass(300);
        assert_eq!(camera(&window), stopped, "{what}: the camera keeps moving");
        for &modifier in before.iter().chain(after) {
            key_up(&window, modifier);
        }
    }
}

/// W pressed under Ctrl and Alt moves the frame on while another key comes
/// up under the same Ctrl and Alt, and stops when W is let go.
#[test]
fn test_a_release_under_the_same_ctrl_and_alt_leaves_a_held_key_at_work() {
    use slint::platform::Key;

    let (window, _sent) = wired_window();
    click_the_globe(&window);
    key_down(&window, Key::Control);
    key_down(&window, Key::Alt);
    key_down(&window, "w");
    let_pass(50);
    tap(&window, "x");
    let tapped = camera(&window);
    let_pass(100);
    assert_ne!(camera(&window), tapped, "the frame stops while W is held");
    key_up(&window, "w");
    let stopped = camera(&window);
    let_pass(300);
    assert_eq!(
        camera(&window),
        stopped,
        "the frame goes on after W is let go"
    );
    key_up(&window, Key::Alt);
    key_up(&window, Key::Control);
}

/// A held J or L let go after Option, `AltGr` or Ctrl + Alt changed the text
/// it gives, as Option turns j into ∆ on macOS and `AltGr` turns l into ł on
/// a Polish layout, stops stepping: the modifier's press ends the hold before
/// the release comes.
#[test]
fn test_a_shuttle_key_let_go_as_other_text_stops_stepping() {
    use slint::platform::Key;

    let ladder = clock_ladder();
    let cases: [(&str, &[Key], &str); 3] = [
        ("j", &[Key::Alt], "∆"),
        ("l", &[Key::AltGr], "ł"),
        ("l", &[Key::Control, Key::Alt], "ł"),
    ];
    for (pressed, modifiers, released) in cases {
        let what = format!("{pressed:?} let go as {released:?} with {modifiers:?} down");
        let (window, sent) = wired_window();
        click_the_globe(&window);
        running_at(&window, ladder[ladder.len() / 2]);
        let _ = excursion_commands(&sent);
        key_down(&window, pressed);
        assert_eq!(
            excursion_commands(&sent).0.len(),
            1,
            "{what}: the press steps"
        );
        for &modifier in modifiers {
            key_down(&window, modifier);
        }
        key_up(&window, released);
        let_pass(1500);
        assert_eq!(excursion_commands(&sent).0, [], "{what}: it keeps stepping");
        for &modifier in modifiers.iter().rev() {
            key_up(&window, modifier);
        }
    }
}

/// A key pressed with Shift and let go after it, as + is Shift + 1 on a
/// Swiss German layout and comes up as 1, stops what it moved.
#[test]
fn test_a_held_key_let_go_after_its_shift_stops_the_camera() {
    use slint::platform::Key;

    let (window, _sent) = wired_window();
    click_the_globe(&window);
    let still = camera(&window);
    key_down(&window, Key::Shift);
    key_down(&window, "+");
    let_pass(50);
    assert_ne!(camera(&window), still, "the press zooms");
    key_up(&window, Key::Shift);
    key_up(&window, "1");
    let stopped = camera(&window);
    let_pass(300);
    assert_eq!(camera(&window), stopped, "the zoom goes on");
}

/// Another key tapped under the Shift a held key went down with leaves the
/// held key at work.
#[test]
fn test_a_tap_under_the_same_shift_leaves_a_held_key_at_work() {
    use slint::platform::Key;

    let (window, _sent) = wired_window();
    click_the_globe(&window);
    key_down(&window, Key::Shift);
    key_down(&window, "W");
    let_pass(50);
    tap(&window, "!");
    let tapped = camera(&window);
    let_pass(100);
    assert_ne!(camera(&window), tapped, "the look stops while W is held");
    key_up(&window, "W");
    let stopped = camera(&window);
    let_pass(300);
    assert_eq!(
        camera(&window),
        stopped,
        "the look goes on after W is let go"
    );
    key_up(&window, Key::Shift);
}

/// The modifier presses that end a hold, each after the modifiers before it:
/// `AltGr`, Alt, and Ctrl joined by Alt or joining it, as Windows takes Ctrl +
/// Alt for `AltGr`.
const HOLD_ENDING_PRESSES: [(&[slint::platform::Key], &[slint::platform::Key]); 4] = {
    use slint::platform::Key;
    [
        (&[], &[Key::AltGr]),
        (&[], &[Key::Alt]),
        (&[], &[Key::Control, Key::Alt]),
        (&[Key::Alt], &[Key::Control]),
    ]
};

/// A key held by its text stops when Alt or `AltGr` goes down, or Ctrl joins
/// Alt, though its release never reaches the preview: under them many layouts
/// make that release a dead key, as `AltGr` does with − on a Swiss German one,
/// and Slint drops a key event that gives no text.
#[test]
fn test_alt_or_altgr_pressed_during_a_hold_stops_the_camera() {
    for (before, pressed) in HOLD_ENDING_PRESSES {
        let held: &[&str] = if before.is_empty() {
            &["w", "a", "s", "d", "+", "=", "-", "q", "e"]
        } else {
            &["w", "a", "s", "d"]
        };
        for &key in held {
            let what = format!("{key:?} held with {before:?} down, then {pressed:?}");
            let (window, _sent) = wired_window();
            click_the_globe(&window);
            let still = camera(&window);
            for &modifier in before {
                key_down(&window, modifier);
            }
            key_down(&window, key);
            let_pass(50);
            assert_ne!(camera(&window), still, "{what}: the press moves the camera");
            for &modifier in pressed {
                key_down(&window, modifier);
            }
            for &modifier in before.iter().chain(pressed).rev() {
                key_up(&window, modifier);
            }
            let stopped = camera(&window);
            let_pass(300);
            assert_eq!(camera(&window), stopped, "{what}: the camera keeps moving");
        }
    }
}

/// A J or L hold ends when Alt or `AltGr` goes down, or Ctrl joins Alt,
/// though the key's release never reaches the preview, as `AltGr` makes J's
/// a dead key on a German layout under Linux.
#[test]
fn test_alt_or_altgr_pressed_during_a_shuttle_hold_stops_stepping() {
    let ladder = clock_ladder();
    for (_, pressed) in HOLD_ENDING_PRESSES
        .iter()
        .filter(|(before, _)| before.is_empty())
    {
        for key in ["j", "l"] {
            let what = format!("{key} held, then {pressed:?}");
            let (window, sent) = wired_window();
            click_the_globe(&window);
            running_at(&window, ladder[ladder.len() / 2]);
            let _ = excursion_commands(&sent);
            key_down(&window, key);
            assert_eq!(
                excursion_commands(&sent).0.len(),
                1,
                "{what}: the press steps"
            );
            for &modifier in *pressed {
                key_down(&window, modifier);
            }
            for &modifier in pressed.iter().rev() {
                key_up(&window, modifier);
            }
            let_pass(1500);
            assert_eq!(excursion_commands(&sent).0, [], "{what}: it keeps stepping");
        }
    }
}

/// A held key let go after Shift changed its text into another key of the
/// preview's stops what it moved: where = is Shift + 0, as on German, Spanish
/// and Nordic layouts, = comes up as 0, and where - and = share a key, as on
/// Japanese and Dutch ones, each comes up as the other.
#[test]
fn test_a_held_key_let_go_as_another_key_of_the_preview_stops_the_camera() {
    use slint::platform::Key;

    for (shift_at_press, pressed, released) in
        [(true, "=", "0"), (false, "-", "="), (true, "=", "-")]
    {
        let what = if shift_at_press {
            format!("{pressed:?} pressed under Shift, let go as {released:?} after it")
        } else {
            format!("{pressed:?} held, let go as {released:?} under Shift")
        };
        let (window, _sent) = wired_window();
        click_the_globe(&window);
        let still = camera(&window);
        if shift_at_press {
            key_down(&window, Key::Shift);
        }
        key_down(&window, pressed);
        let_pass(50);
        assert_ne!(camera(&window), still, "{what}: the press zooms");
        if shift_at_press {
            key_up(&window, Key::Shift);
            key_up(&window, released);
        } else {
            key_down(&window, Key::Shift);
            key_up(&window, released);
            key_up(&window, Key::Shift);
        }
        let stopped = camera(&window);
        let_pass(300);
        assert_eq!(camera(&window), stopped, "{what}: the zoom goes on");
        release_every_modifier(&window);
    }
}

/// K and 0 pause and go back to the time setting, nothing held or with W
/// held under the same modifiers, which keeps moving.
#[test]
fn test_keys_that_hold_nothing_do_their_job_and_leave_a_held_key_at_work() {
    for hold in [None, Some("w")] {
        let what = format!("with {hold:?} held");
        let (window, sent) = wired_window();
        click_the_globe(&window);
        running_at(&window, 60);
        let _ = excursion_commands(&sent);
        if let Some(key) = hold {
            key_down(&window, key);
            let_pass(50);
        }
        tap(&window, "k");
        tap(&window, "0");
        assert_eq!(
            excursion_commands(&sent).0,
            [ExcursionCommand::Pause, ExcursionCommand::End],
            "{what}: K pauses and 0 goes back to the setting"
        );
        if let Some(key) = hold {
            let moving = camera(&window);
            let_pass(100);
            assert_ne!(camera(&window), moving, "{what}: the orbit stops");
            key_up(&window, key);
        }
    }
}

/// A held key goes on through a tap of another held key under Shift, let go
/// before or after Shift.
#[test]
fn test_a_tap_under_shift_leaves_a_key_held_without_it_at_work() {
    use slint::platform::Key;

    for (released, shift_first) in [("A", false), ("a", true)] {
        let what = format!("Shift + A let go as {released:?}, Shift first: {shift_first}");
        let (window, _sent) = wired_window();
        click_the_globe(&window);
        key_down(&window, "w");
        let_pass(50);
        key_down(&window, Key::Shift);
        key_down(&window, "A");
        let_pass(20);
        if shift_first {
            key_up(&window, Key::Shift);
            key_up(&window, released);
        } else {
            key_up(&window, released);
            key_up(&window, Key::Shift);
        }
        let moving = camera(&window);
        let_pass(100);
        assert_ne!(camera(&window), moving, "{what}: W stops");
        key_up(&window, "w");
        release_every_modifier(&window);
    }
}

/// A held key goes on through a tap of an arrow or of Q under Shift, whose
/// release ends the key it pressed and judges nothing else.
#[test]
fn test_a_tap_of_an_arrow_or_q_under_shift_leaves_a_key_held_without_it_at_work() {
    use slint::platform::Key;

    let taps: [(&str, slint::SharedString, slint::SharedString); 6] = [
        ("left", Key::LeftArrow.into(), Key::LeftArrow.into()),
        ("right", Key::RightArrow.into(), Key::RightArrow.into()),
        ("up", Key::UpArrow.into(), Key::UpArrow.into()),
        ("down", Key::DownArrow.into(), Key::DownArrow.into()),
        ("Q", "Q".into(), "Q".into()),
        ("Q let go as q", "Q".into(), "q".into()),
    ];
    for (name, pressed, released) in taps {
        let what = format!("Shift + {name} tapped");
        let (window, _sent) = wired_window();
        click_the_globe(&window);
        key_down(&window, "w");
        let_pass(50);
        key_down(&window, Key::Shift);
        key_down(&window, pressed);
        let_pass(20);
        key_up(&window, released);
        let moving = camera(&window);
        let_pass(100);
        assert_ne!(camera(&window), moving, "{what}: W stops");
        key_up(&window, "w");
        release_every_modifier(&window);
    }
}

/// A held key goes on through a + typed under the Shift it went down with
/// and let go as = after Shift, as the US layout gives them.
#[test]
fn test_a_release_that_ends_its_own_key_leaves_another_held_key_at_work() {
    use slint::platform::Key;

    let (window, _sent) = wired_window();
    click_the_globe(&window);
    key_down(&window, Key::Shift);
    key_down(&window, "W");
    key_down(&window, "+");
    let_pass(50);
    key_up(&window, Key::Shift);
    key_up(&window, "=");
    let moving = camera(&window);
    let_pass(100);
    assert_ne!(camera(&window), moving, "W stops");
    key_up(&window, "w");
    release_every_modifier(&window);
}

/// A key's repeated presses keep it held while it holds, and do not take up
/// again a hold that Alt or `AltGr` ended while the key stayed down.
#[test]
fn test_a_repeated_press_keeps_a_hold_but_does_not_take_up_an_ended_one() {
    use slint::platform::WindowEvent;

    for (before, pressed) in HOLD_ENDING_PRESSES {
        let held: &[&str] = if before.is_empty() {
            &["w", "a", "s", "d", "+", "=", "-", "q", "e"]
        } else {
            &["w", "a", "s", "d"]
        };
        for &key in held {
            let what = format!("{key:?} held with {before:?} down, then {pressed:?}");
            let (window, _sent) = wired_window();
            click_the_globe(&window);
            for &modifier in before {
                key_down(&window, modifier);
            }
            key_down(&window, key);
            let_pass(50);
            window
                .window()
                .dispatch_event(WindowEvent::KeyPressRepeated { text: key.into() });
            let moving = camera(&window);
            let_pass(50);
            assert_ne!(camera(&window), moving, "{what}: a repeat stops the camera");
            for &modifier in pressed {
                key_down(&window, modifier);
            }
            window
                .window()
                .dispatch_event(WindowEvent::KeyPressRepeated { text: key.into() });
            let stopped = camera(&window);
            let_pass(300);
            assert_eq!(camera(&window), stopped, "{what}: a repeat moves it again");
            key_up(&window, key);
            release_every_modifier(&window);
        }
    }
}

/// A repeated press of J or L does not take up again a hold that Alt or
/// `AltGr` ended while the key stayed down.
#[test]
fn test_a_repeated_shuttle_press_does_not_take_up_an_ended_hold() {
    use slint::platform::WindowEvent;

    let ladder = clock_ladder();
    for (_, pressed) in HOLD_ENDING_PRESSES
        .iter()
        .filter(|(before, _)| before.is_empty())
    {
        for key in ["j", "l"] {
            let what = format!("{key} held, then {pressed:?}");
            let (window, sent) = wired_window();
            click_the_globe(&window);
            running_at(&window, ladder[ladder.len() / 2]);
            key_down(&window, key);
            for &modifier in *pressed {
                key_down(&window, modifier);
            }
            let _ = excursion_commands(&sent);
            window
                .window()
                .dispatch_event(WindowEvent::KeyPressRepeated { text: key.into() });
            let_pass(1500);
            assert_eq!(
                excursion_commands(&sent).0,
                [],
                "{what}: a repeat steps again"
            );
            key_up(&window, key);
            release_every_modifier(&window);
        }
    }
}

/// An arrow goes on through Alt, `AltGr` and Ctrl with Alt, whose presses
/// end the keys held by their text, and stops when let go.
#[test]
fn test_an_arrow_held_through_alt_or_altgr_moves_on_until_let_go() {
    use slint::platform::Key;

    for (before, pressed) in HOLD_ENDING_PRESSES {
        for arrow in [
            Key::LeftArrow,
            Key::RightArrow,
            Key::UpArrow,
            Key::DownArrow,
        ] {
            let what = format!("{arrow:?} held with {before:?} down, then {pressed:?}");
            let (window, _sent) = wired_window();
            click_the_globe(&window);
            for &modifier in before {
                key_down(&window, modifier);
            }
            key_down(&window, arrow);
            let_pass(50);
            for &modifier in pressed {
                key_down(&window, modifier);
            }
            let moving = camera(&window);
            let_pass(100);
            assert_ne!(camera(&window), moving, "{what}: the arrow stops");
            key_up(&window, arrow);
            let stopped = camera(&window);
            let_pass(300);
            assert_eq!(
                camera(&window),
                stopped,
                "{what}: it goes on after the arrow is let go"
            );
            release_every_modifier(&window);
        }
    }
}

/// Shift or Ctrl pressed during a J or L hold leaves it stepping.
#[test]
fn test_shift_or_ctrl_pressed_during_a_shuttle_hold_leaves_it_stepping() {
    use slint::platform::Key;

    let ladder = clock_ladder();
    for key in ["j", "l"] {
        for modifier in [Key::Shift, Key::ShiftR, Key::Control, Key::ControlR] {
            let what = format!("{key} held, then {modifier:?}");
            let (window, sent) = wired_window();
            click_the_globe(&window);
            running_at(&window, ladder[ladder.len() / 2]);
            key_down(&window, key);
            let _ = excursion_commands(&sent);
            key_down(&window, modifier);
            let_pass(700);
            assert_ne!(
                excursion_commands(&sent).0,
                [],
                "{what}: the stepping stops"
            );
            key_up(&window, key);
            release_every_modifier(&window);
        }
    }
}

/// Shift or Ctrl pressed while W is held turns the orbit into the look or
/// the frame and stops nothing.
#[test]
fn test_shift_or_ctrl_pressed_during_a_hold_turns_it_to_look_or_frame() {
    use slint::platform::Key;

    for (modifier, look) in [(Key::Shift, true), (Key::Control, false)] {
        let what = format!("W held, then {modifier:?}");
        let (window, _sent) = wired_window();
        click_the_globe(&window);
        key_down(&window, "w");
        let_pass(50);
        key_down(&window, modifier);
        let_pass(20);
        let orientation = window.get_camera_orientation();
        let framing = window.get_framing();
        let_pass(100);
        if look {
            assert_ne!(
                window.get_camera_orientation(),
                orientation,
                "{what}: the look stops"
            );
        } else {
            assert_ne!(window.get_framing(), framing, "{what}: the frame stops");
        }
        key_up(&window, if look { "W" } else { "w" });
        key_up(&window, modifier);
    }
}

/// A key held under Alt, `AltGr`, or Ctrl and Alt, and let go after one of
/// them came up as other text than its press gave, stops what it moved; on
/// Windows so does a + that Ctrl and Alt gave as `AltGr` would, which Slint
/// reports under neither.
#[test]
fn test_a_key_held_under_alt_or_altgr_let_go_after_it_stops_the_camera() {
    use slint::platform::Key;

    let mut cases: Vec<(&[Key], &str, &[Key])> = vec![
        (&[Key::AltGr], "-", &[Key::AltGr]),
        (&[Key::Alt], "w", &[Key::Alt]),
        (&[Key::Control, Key::Alt], "w", &[Key::Control]),
        (&[Key::ControlR, Key::Alt], "w", &[Key::ControlR]),
    ];
    // Elsewhere Slint keeps Ctrl and Alt on that +, so its press goes to the
    // window.
    if cfg!(windows) {
        cases.push((&[Key::Control, Key::Alt], "+", &[Key::Control]));
    }
    for (down, pressed, let_go) in cases {
        let what = format!("{pressed:?} pressed under {down:?}, let go after {let_go:?}");
        let (window, _sent) = wired_window();
        click_the_globe(&window);
        held_under_and_let_go_after(&window, down, pressed, let_go, &what);
        release_every_modifier(&window);
    }
}

/// Hold `pressed` under `down`, let go of `let_go`, let `pressed` go as text
/// no key of the preview gives, and check that the camera stops.
fn held_under_and_let_go_after(
    window: &MainWindow,
    down: &[slint::platform::Key],
    pressed: &str,
    let_go: &[slint::platform::Key],
    what: &str,
) {
    for &modifier in down {
        key_down(window, modifier);
    }
    let still = camera(window);
    key_down(window, pressed);
    let_pass(50);
    assert_ne!(camera(window), still, "{what}: the press moves the camera");
    for &modifier in let_go {
        key_up(window, modifier);
    }
    key_up(window, "x");
    let stopped = camera(window);
    let_pass(300);
    assert_eq!(camera(window), stopped, "{what}: the camera keeps moving");
}

/// The preview forgets the Ctrl and Alt keys it saw go down when it loses
/// the keyboard, which takes their releases with it: an Alt left down no
/// longer ends a W hold Ctrl joins, and a Ctrl left down no longer makes a
/// key held under the other Ctrl and Alt read as held under them both after
/// that Ctrl comes up.
#[test]
fn test_the_ctrl_and_alt_keys_are_forgotten_with_the_keyboard() {
    use slint::platform::Key;
    use slint::platform::WindowEvent;

    let lose_the_keyboard_with = |window: &MainWindow, key: Key| {
        key_down(window, key);
        window
            .window()
            .dispatch_event(WindowEvent::WindowActiveChanged(false));
        window
            .window()
            .dispatch_event(WindowEvent::WindowActiveChanged(true));
    };

    let (window, _sent) = wired_window();
    click_the_globe(&window);
    lose_the_keyboard_with(&window, Key::Alt);
    key_down(&window, "w");
    let_pass(50);
    key_down(&window, Key::Control);
    let_pass(20);
    let framing = window.get_framing();
    let_pass(100);
    assert_ne!(
        window.get_framing(),
        framing,
        "Ctrl after an Alt the preview forgot: the frame stops"
    );
    key_up(&window, "w");
    release_every_modifier(&window);

    for (forgotten, other) in [(Key::Control, Key::ControlR), (Key::ControlR, Key::Control)] {
        let what = format!("W held under {other:?} and Alt with {forgotten:?} forgotten");
        let (window, _sent) = wired_window();
        click_the_globe(&window);
        lose_the_keyboard_with(&window, forgotten);
        held_under_and_let_go_after(&window, &[other, Key::Alt], "w", &[other], &what);
        release_every_modifier(&window);
    }
}

/// J pressed while L is held, or L while J is, takes the hold over and waits
/// its own full delay before it repeats, however soon after the first key it
/// came.
#[test]
fn test_a_shuttle_key_pressed_during_the_other_waits_its_own_delay() {
    let ladder = clock_ladder();
    for (first, second) in [("l", "j"), ("j", "l")] {
        for after in [100, 300, 600] {
            let what = format!("{second} {after} ms into a held {first}");
            let (window, sent) = wired_window();
            click_the_globe(&window);
            running_at(&window, ladder[ladder.len() / 2]);
            key_down(&window, first);
            let_pass(after);
            let _ = excursion_commands(&sent);
            key_down(&window, second);
            assert_eq!(
                excursion_commands(&sent).0.len(),
                1,
                "{what}: the press steps at once"
            );
            let_pass(350);
            assert_eq!(
                excursion_commands(&sent).0,
                [],
                "{what}: it repeats before its own delay"
            );
            let_pass(100);
            assert_eq!(
                excursion_commands(&sent).0.len(),
                1,
                "{what}: it repeats after its delay"
            );
            key_up(&window, second);
            key_up(&window, first);
        }
    }
}

/// K, Space, 0 and R end a J or L hold before they act, so the held key
/// does not take play up again; a new press of J or L does.
#[test]
fn test_k_space_0_and_r_end_a_shuttle_hold() {
    let ladder = clock_ladder();
    for held in ["j", "l"] {
        for key in ["k", " ", "0", "r"] {
            let what = format!("{key:?} during a held {held}");
            let (window, sent) = wired_window();
            click_the_globe(&window);
            running_at(&window, ladder[ladder.len() / 2]);
            key_down(&window, held);
            let_pass(100);
            tap(&window, key);
            let _ = excursion_commands(&sent);
            let_pass(1500);
            assert_eq!(
                excursion_commands(&sent).0,
                [],
                "{what}: the held key took play up again"
            );
            key_up(&window, held);
            key_down(&window, held);
            assert_eq!(
                excursion_commands(&sent).0.len(),
                1,
                "{what}: a new press steps"
            );
            key_up(&window, held);
        }
    }
}

/// G makes play run the date and T the time, as the radio labels do: paused
/// the bar switches its ladder, and running play goes on in the new kind.
#[test]
fn test_t_and_g_pick_what_play_runs() {
    let (window, sent) = wired_window();
    click_the_globe(&window);
    tap(&window, "g");
    assert!(window.get_time_bar().date_kind, "G picks the date");
    tap(&window, "t");
    assert!(!window.get_time_bar().date_kind, "T picks the time");
    assert_eq!(excursion_commands(&sent).0, [], "paused, nothing plays");

    sunlit_earth::time_bar::excursion_changed(
        &window,
        Some(ExcursionState {
            running: true,
            rate: 60,
            kind: ExcursionKind::Clock,
        }),
    );
    tap(&window, "G");
    let rate = i32::try_from(ExcursionKind::Date.default_rate()).expect("a rate");
    assert_eq!(
        excursion_commands(&sent).0,
        [ExcursionCommand::Play {
            rate,
            kind: ExcursionKind::Date
        }],
        "running, play goes on through the dates"
    );
}

/// The left, top, right and bottom edges of an element in the window.
fn edges(element: &ElementHandle) -> [f32; 4] {
    let at = element.absolute_position();
    let size = element.size();
    [at.x, at.y, at.x + size.width, at.y + size.height]
}

/// The one element `id` under `parent`.
fn the_descendant(parent: &ElementHandle, id: &str) -> ElementHandle {
    let found = parent.query_descendants().match_id(id).find_all();
    assert_eq!(found.len(), 1, "expected exactly one '{id}'");
    found.into_iter().next().expect("one element")
}

/// Rest the pointer on `control` until its hover hint shows, and return the
/// hint.
fn hint_over(window: &MainWindow, control: &ElementHandle) -> ElementHandle {
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: point_in(control, 0.5, 0.5),
        });
    for _ in 0..12 {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(50));
    }
    the_element(window, "MainWindow::hint")
}

/// Every key cap under `parent`, row by row and left to right, each wide
/// enough for its label and clear of the next one in its row.
fn caps_holding_their_labels(parent: &ElementHandle, what: &str) -> Vec<ElementHandle> {
    let mut caps = parent
        .query_descendants()
        .match_inherits("KeyCap")
        .find_all();
    caps.sort_by(|a, b| {
        let (a, b) = (a.absolute_position(), b.absolute_position());
        a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x))
    });
    for cap in &caps {
        let label = the_descendant(cap, "KeyCap::label");
        let [cap_left, _, cap_right, _] = edges(cap);
        let [label_left, _, label_right, _] = edges(&label);
        assert!(
            label_left >= cap_left + 4.0 && label_right <= cap_right - 4.0,
            "{what}: a cap {} px wide holds a label {} px wide",
            cap_right - cap_left,
            label_right - label_left
        );
    }
    for pair in caps.windows(2) {
        let [_, top, right, _] = edges(&pair[0]);
        let [next_left, next_top, _, _] = edges(&pair[1]);
        assert!(
            (top - next_top).abs() > 0.5 || right <= next_left,
            "{what}: two caps overlap"
        );
    }
    caps
}

/// The overlay controls whose hints carry keys, most of them a mouse gesture
/// too.
const HINTED_CONTROLS: [&str; 9] = [
    "CameraPanel::orbit",
    "CameraPanel::zoom-in",
    "CameraPanel::zoom-slider",
    "CameraPanel::zoom-out",
    "CameraPanel::look",
    "CameraPanel::frame",
    "CameraPanel::guides-button",
    "CameraPanel::help-button",
    "CameraPanel::hide-button",
];

/// Every hint's gesture and keys stay inside its border in both platforms'
/// key names: the keys keep their width on one line at the right, and the
/// gesture text takes the room left beside them.
#[test]
fn test_a_hints_gesture_and_keys_stay_inside_it() {
    for macos in [false, true] {
        let (window, _sent) = wired_window();
        window
            .global::<sunlit_earth::OverlayHints>()
            .set_is_macos(macos);
        for id in HINTED_CONTROLS {
            let what = format!("{id} (macOS: {macos})");
            let hint = hint_over(&window, &the_element(&window, id));
            let [left, top, right, bottom] = edges(&hint);
            let row = the_descendant(&hint, "OverlayHint::gesture-row");
            let [row_left, row_top, row_right, row_bottom] = edges(&row);
            assert!(
                row_left >= left + 10.0 - 0.5
                    && row_right <= right - 10.0 + 0.5
                    && row_top >= top
                    && row_bottom <= bottom - 8.0 + 0.5,
                "{what}: the gesture row runs out of the hint"
            );
            let gesture = the_descendant(&hint, "OverlayHint::gesture");
            let [_, text_top, text_right, text_bottom] = edges(&gesture);
            assert!(
                text_top >= row_top - 0.5 && text_bottom <= row_bottom + 0.5,
                "{what}: the gesture text spills out of its row"
            );
            let caps = caps_holding_their_labels(&hint, &what);
            assert!(!caps.is_empty(), "{what}: the hint names its keys");
            let [caps_left, caps_top, _, _] = edges(&caps[0]);
            assert!(
                text_right <= caps_left || text_bottom <= caps_top + 0.5,
                "{what}: the gesture text runs under the keys"
            );
            assert!(
                edges(&caps[caps.len() - 1])[2] <= row_right + 0.5,
                "{what}: the keys run out of the row"
            );
        }
    }
}

/// The keys each hinted control's action has, in the hint's own spelling,
/// with `SHIFT` and `MOD` standing for the platform's names; "or" between
/// keys is left out.
const HINT_KEYS: [(&str, &[&str]); 22] = [
    ("orbit", &["←↑↓→", "WASD"]),
    ("look", &["SHIFT", "←↑↓→", "WASD"]),
    ("frame", &["MOD", "←↑↓→", "WASD"]),
    ("roll", &["Q", "E"]),
    ("north", &["N"]),
    ("zoom", &["+", "−"]),
    ("zoom-in", &["+"]),
    ("zoom-out", &["−"]),
    ("guides", &["F"]),
    ("hide", &["H"]),
    ("show", &["H"]),
    ("skip-back", &[","]),
    ("skip-forward", &["."]),
    ("play", &["Space"]),
    ("ruler", &["J", "K", "L", "R"]),
    ("rate", &["J", "K", "L", "R"]),
    ("drive-time", &["T"]),
    ("drive-date", &["G"]),
    ("now", &["0"]),
    ("eclipse-mode", &["PgUp", "PgDn"]),
    ("eclipse-previous", &["PgUp"]),
    ("eclipse-next", &["PgDn"]),
];

/// The hinted controls no key does the same as: the resets, exact entry, the
/// zone, the state pill while live (not clickable) or in Preview (whose click
/// ends the excursion, which no key does alone), the pin and the tracks.
const HINTS_WITHOUT_KEYS: [&str; 12] = [
    "look-reset",
    "frame-reset",
    "pick-date",
    "pick-time",
    "zone",
    "live",
    "preview",
    "pin",
    "day-track",
    "day-track-sun-fixed",
    "day-track-eclipse",
    "year-track",
];

/// Every hint id a `.slint` expression picks, read from the source between
/// `marker` and the `""` that ends the choice.
fn hint_ids_in(source: &str, marker: &str) -> Vec<String> {
    let start = source
        .find(marker)
        .expect("the hint choice is in the source");
    let rest = &source[start + marker.len()..];
    let choice = &rest[..rest.find("\"\";").expect("the choice ends in \"\"")];
    choice
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Every hint a control can show carries the keys that do what the control
/// does, in both platforms' names, and every id the overlays can pick is
/// classified here, so a new control states its keys or that it has none.
#[test]
fn test_every_hint_names_the_keys_of_its_control() {
    use slint::Model as _;

    let mut used = Vec::new();
    for (source, marker) in [
        (
            include_str!("../../sunlit-ui/ui/overlays/camera-panel.slint"),
            "out property <string> hovered-hint:",
        ),
        (
            include_str!("../../sunlit-ui/ui/overlays/time-bar.slint"),
            "out property <string> hovered-hint:",
        ),
        (
            include_str!("../../sunlit-ui/ui/overlays/eclipse-panel.slint"),
            "out property <string> hovered-hint:",
        ),
        (
            include_str!("../../sunlit-ui/ui/main.slint"),
            "property <string> hint-id:",
        ),
    ] {
        used.extend(hint_ids_in(source, marker));
    }
    let classified: Vec<&str> = HINT_KEYS
        .iter()
        .map(|(id, _)| *id)
        .chain(HINTS_WITHOUT_KEYS)
        .chain(["help"])
        .collect();
    for id in &used {
        assert!(
            classified.contains(&id.as_str()),
            "the hint {id} is in neither HINT_KEYS nor HINTS_WITHOUT_KEYS"
        );
    }
    for id in &classified {
        assert!(
            used.iter().any(|used| used == id),
            "no control picks the hint {id}"
        );
    }

    for macos in [false, true] {
        let (window, _sent) = wired_window();
        let hints = window.global::<sunlit_earth::OverlayHints>();
        hints.set_is_macos(macos);
        let (shift, modifier) = if macos {
            ("⇧", "⌘")
        } else {
            ("Shift", "Ctrl")
        };
        let keys_of = |id: &str| {
            let content = hints.invoke_content(id.into());
            assert!(!content.title.is_empty(), "the hint {id} has no title");
            content
                .keys
                .iter()
                .map(|key: slint::SharedString| key.to_string())
                .filter(|key| key != "or")
                .collect::<Vec<_>>()
        };
        for (id, keys) in HINT_KEYS {
            let wanted: Vec<&str> = keys
                .iter()
                .map(|key| match *key {
                    "SHIFT" => shift,
                    "MOD" => modifier,
                    key => key,
                })
                .collect();
            assert_eq!(keys_of(id), wanted, "the hint {id} (macOS: {macos})");
        }
        for id in HINTS_WITHOUT_KEYS {
            assert!(
                keys_of(id).is_empty(),
                "the hint {id} (macOS: {macos}) names keys; move it to HINT_KEYS"
            );
        }
        let help = if macos {
            vec!["⌘", "?"]
        } else {
            vec!["F1", "Ctrl", "?"]
        };
        assert_eq!(keys_of("help"), help, "the help hint (macOS: {macos})");
        for id in ["skip-back", "skip-forward"] {
            let body = hints.invoke_content(id.into()).body;
            assert!(
                body.contains(&format!("With {shift}, ")),
                "the hint {id} (macOS: {macos}) does not name {shift} for the bigger step"
            );
        }
    }
}

/// A gesture too long for the room beside the keys wraps, and the hint grows
/// to hold every line of it.
#[test]
fn test_a_wrapping_gesture_keeps_its_hint_around_it() {
    for macos in [false, true] {
        let (window, _sent) = wired_window();
        window
            .global::<sunlit_earth::OverlayHints>()
            .set_is_macos(macos);
        for id in ["CameraPanel::orbit", "CameraPanel::frame"] {
            let hint = hint_over(&window, &the_element(&window, id));
            let line = the_descendant(&hint, "OverlayHint::title").size().height;
            let gesture = the_descendant(&hint, "OverlayHint::gesture");
            assert!(
                gesture.size().height >= 2.0 * line - 0.5,
                "{id} (macOS: {macos}): the gesture is {} px high, one line {line} px",
                gesture.size().height
            );
            assert!(
                edges(&gesture)[3] <= edges(&hint)[3] - 8.0 + 0.5,
                "{id} (macOS: {macos}): the gesture runs over the hint's border"
            );
        }
    }
}

// A wrapping `Text` in the sheet's description font, whose minimum width is
// the width of the longest word of what it holds.
slint::slint! {
    export component WordProbe inherits Window {
        in property <string> body;
        out property <length> longest-word: probe.min-width;
        probe := Text {
            text: root.body;
            font-size: 12px;
            wrap: word-wrap;
        }
    }
}

/// Window sizes from the app's narrowest at the default height up to a wide
/// one, with the default 1000 by 600 among them.
const SHEET_WINDOWS: [(u32, u32); 6] = [
    (520, 600),
    (700, 600),
    (800, 700),
    (900, 600),
    (1000, 600),
    (1400, 900),
];

/// The shortcut sheet fits its window, two equal columns side by side where
/// there is room and one under the other where not, and every description
/// stays on the card without a word broken inside, every key cap holding its
/// label. In the default window and at 800 by 700 the whole card shows
/// without scrolling.
#[test]
fn test_the_shortcut_sheet_keeps_its_text_on_the_card() {
    init();
    let probe = WordProbe::new().expect("a probe");
    for macos in [false, true] {
        for (width, height) in SHEET_WINDOWS {
            let what = format!("{width} by {height} (macOS: {macos})");
            let window = create_window();
            window
                .window()
                .set_size(slint::PhysicalSize::new(width, height));
            window
                .global::<sunlit_earth::OverlayHints>()
                .set_is_macos(macos);
            window.set_shortcut_sheet_open(true);
            let card = the_element(&window, "ShortcutSheet::card");
            let [left, top, right, bottom] = edges(&card);
            #[expect(clippy::cast_precision_loss, reason = "a window size")]
            let (window_width, window_height) = (width as f32, height as f32);
            assert!(
                left >= window_width - window.get_viewport_width() - 0.5
                    && right <= window_width + 0.5
                    && top >= -0.5
                    && bottom <= window_height + 0.5,
                "{what}: the card runs out of the preview"
            );

            let mouse = edges(&the_element(&window, "ShortcutSheet::mouse-column"));
            let keys = edges(&the_element(&window, "ShortcutSheet::keys-column"));
            approx::assert_abs_diff_eq!(mouse[2] - mouse[0], keys[2] - keys[0], epsilon = 0.5);
            assert!(
                keys[0] >= mouse[2] + 26.0 - 0.5 || keys[1] >= mouse[3] - 0.5,
                "{what}: the columns overlap"
            );
            if width >= 1000 {
                assert!(
                    keys[0] > mouse[2],
                    "{what}: the columns stand one under the other"
                );
            }

            let content = edges(&the_element(&window, "ShortcutSheet::card-layout"));
            if (width, height) == (1000, 600) || (width, height) == (800, 700) {
                assert!(
                    content[3] <= bottom + 0.5,
                    "{what}: the card scrolls, {} over {}",
                    content[3],
                    bottom
                );
            }

            // The testing backend leaves out what is scrolled out of sight,
            // and the text's widths do not depend on the height.
            window
                .window()
                .set_size(slint::PhysicalSize::new(width, 5000));
            let card = the_element(&window, "ShortcutSheet::card");
            let [left, _, right, _] = edges(&card);
            let content = edges(&the_element(&window, "ShortcutSheet::card-layout"));
            let texts = card
                .query_descendants()
                .match_inherits("SheetText")
                .find_all();
            assert_eq!(
                texts.len(),
                22,
                "{what}: the sheet lists every gesture and key"
            );
            for text in &texts {
                let label = text.accessible_label().expect("a description");
                let [text_left, _, text_right, text_bottom] = edges(text);
                assert!(
                    text_left >= left + 22.0 - 0.5
                        && text_right <= right - 22.0 + 0.5
                        && text_bottom <= content[3],
                    "{what}: {label:?} runs off the card"
                );
                probe.set_body(label.clone());
                assert!(
                    text_right - text_left >= probe.get_longest_word() - 0.5,
                    "{what}: {label:?} has {} px for a word {} px wide",
                    text_right - text_left,
                    probe.get_longest_word()
                );
            }
            caps_holding_their_labels(&card, &what);
        }
    }
}

/// The folded pill names the zone only in UTC, where its time would
/// otherwise be taken for local time.
#[test]
fn test_the_folded_pill_names_utc_and_leaves_local_time_plain() {
    let (window, _sent) = wired_window();
    let texts = |window: &MainWindow| -> Vec<String> {
        the_element(window, "TimeBar::pill-row")
            .query_descendants()
            .match_inherits("Text")
            .find_all()
            .iter()
            .filter_map(ElementHandle::accessible_label)
            .map(|label| label.to_string())
            .collect()
    };
    if window.get_time_zone_utc() {
        window.invoke_time_zone_toggled();
    }
    let local = texts(&window);
    assert!(local.len() >= 2, "the pill's texts: {local:?}");
    assert!(
        local.iter().all(|text| !text.contains("UTC")),
        "local time: {local:?}"
    );
    window.invoke_time_zone_toggled();
    let utc = texts(&window);
    assert!(utc.iter().any(|text| text == "UTC"), "UTC: {utc:?}");
}

/// The left and right edges of each row's elements in `column`, top to
/// bottom, the rows told apart by their vertical middles.
fn row_spans(column: &ElementHandle, component: &str) -> Vec<(f32, f32)> {
    let mut rows: Vec<(f32, f32, f32)> = Vec::new();
    for element in column
        .query_descendants()
        .match_inherits(component)
        .find_all()
    {
        let [left, top, right, bottom] = edges(&element);
        let middle = f32::midpoint(top, bottom);
        match rows.iter_mut().find(|row| (row.0 - middle).abs() < 1.0) {
            Some(row) => {
                row.1 = row.1.min(left);
                row.2 = row.2.max(right);
            }
            None => rows.push((middle, left, right)),
        }
    }
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter()
        .map(|(_, left, right)| (left, right))
        .collect()
}

/// The sheet's descriptions start a small gap after the widest mouse glyph
/// and the widest row of keys, the keys stand flush right against that gap
/// and the descriptions flush left after it; the glyphs and the caps are
/// at least 22 px tall, and the sheet names no platform.
#[test]
fn test_the_shortcut_sheet_aligns_its_keys_against_their_descriptions() {
    for macos in [false, true] {
        for width in [1000, 520] {
            let what = format!("{width} px (macOS: {macos})");
            let window = create_window();
            window
                .window()
                .set_size(slint::PhysicalSize::new(width, 5000));
            window
                .global::<sunlit_earth::OverlayHints>()
                .set_is_macos(macos);
            window.set_shortcut_sheet_open(true);

            for platform in ["Windows and Linux", "macOS"] {
                assert_eq!(
                    ElementHandle::find_by_accessible_label(&window, platform).count(),
                    0,
                    "{what}: the sheet names {platform:?}"
                );
            }

            let mouse = the_element(&window, "ShortcutSheet::mouse-column");
            let glyphs = row_spans(&mouse, "MouseGlyph");
            let gestures = row_spans(&mouse, "SheetText");
            assert_eq!(glyphs.len(), gestures.len(), "{what}: a glyph per gesture");
            let widest_glyph = glyphs.iter().map(|g| g.1).fold(f32::MIN, f32::max);
            for (text_left, _) in &gestures {
                let gap = text_left - widest_glyph;
                assert!(
                    (4.0..=12.0).contains(&gap),
                    "{what}: a gesture's text stands {gap} px after the widest glyph"
                );
            }

            let keyboard = the_element(&window, "ShortcutSheet::keys-column");
            let keys = row_spans(&keyboard, "KeyCap");
            let texts = row_spans(&keyboard, "SheetText");
            assert_eq!(keys.len(), texts.len(), "{what}: keys for each description");
            let right = keys[0].1;
            let text_left = texts[0].0;
            for ((_, keys_right), (left, _)) in keys.iter().zip(&texts) {
                approx::assert_abs_diff_eq!(*keys_right, right, epsilon = 0.5);
                approx::assert_abs_diff_eq!(*left, text_left, epsilon = 0.5);
            }
            let gap = text_left - right;
            assert!(
                (4.0..=12.0).contains(&gap),
                "{what}: the descriptions stand {gap} px after the keys"
            );

            let card = the_element(&window, "ShortcutSheet::card");
            for (component, smallest) in [("MouseGlyph", 22.0), ("KeyCap", 22.0)] {
                for element in card
                    .query_descendants()
                    .match_inherits(component)
                    .find_all()
                {
                    assert!(
                        element.size().height >= smallest - 0.5,
                        "{what}: a {component} {} px tall",
                        element.size().height
                    );
                }
            }
        }
    }
}

/// The transport stands centered over the tracks whatever the bar's width.
#[test]
fn test_play_stands_centered_over_the_tracks() {
    for width in [1400, 1000] {
        let (window, _sent) = wired_window();
        window
            .window()
            .set_size(slint::PhysicalSize::new(width, 5000));
        open_the_bar(&window);
        let [play_left, _, play_right, _] = edges(&the_labeled(&window, "Play"));
        approx::assert_abs_diff_eq!(
            f32::midpoint(play_left, play_right),
            play_middle(&window),
            epsilon = 1.5
        );
    }
}

/// Where play's middle belongs: over the tracks' middle while the transport
/// centered there clears eclipse mode's button and its 6 px gap, and
/// otherwise in the middle of the room left of them.
fn play_middle(window: &MainWindow) -> f32 {
    let [track_left, _, track_right, _] = edges(&the_element(window, "TimeBar::day-track"));
    let row = edges(&the_element(window, "TimeBar::skip-forward"))[2]
        - edges(&the_element(window, "TimeBar::skip-back"))[0];
    let room_right = edges(&the_labeled(window, "Eclipse mode"))[0] - 6.0;
    let centered = f32::midpoint(track_left, track_right);
    if centered + row / 2.0 <= room_right + 0.5 {
        centered
    } else {
        f32::midpoint(track_left, room_right)
    }
}

/// Windows at the default panel from the narrowest whose bar holds the
/// compact transport with the time's twelve speeds up to one past the widest
/// bar.
const BAR_WINDOWS: [u32; 7] = [986, 1000, 1050, 1100, 1150, 1200, 1400];

fn overlap(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[2] - 0.5 && b[0] < a[2] - 0.5 && a[1] < b[3] - 0.5 && b[1] < a[3] - 0.5
}

/// At every bar width the transport stays inside the tracks' column, which
/// keeps it off the clock column, clear of the speed readout and the radio
/// labels and of eclipse mode's button at the column's right end, with play
/// where `play_middle` puts it, for both kinds of play.
#[test]
fn test_the_transport_stays_over_the_tracks_and_clear_of_the_readout() {
    for date in [false, true] {
        for width in BAR_WINDOWS {
            let (window, _sent) = wired_window();
            window
                .window()
                .set_size(slint::PhysicalSize::new(width, 5000));
            open_the_bar(&window);
            if date {
                press_at(&window, point_in(&the_labeled(&window, "DATE"), 0.5, 0.5));
            }
            let what = format!("{width} px window, date: {date}");
            let [track_left, _, track_right, _] =
                edges(&the_element(&window, "TimeBar::day-track"));
            let play = the_labeled(&window, "Play");
            let mut transport: Vec<ElementHandle> = [
                "TimeBar::skip-back",
                "TimeBar::backward",
                "TimeBar::forward",
                "TimeBar::skip-forward",
            ]
            .iter()
            .map(|id| the_element(&window, id))
            .collect();
            transport.push(play.clone());
            let others = [
                the_element(&window, "TimeBar::speed-readout"),
                the_labeled(&window, "TIME"),
                the_labeled(&window, "DATE"),
            ];
            let mode = edges(&the_labeled(&window, "Eclipse mode"));
            approx::assert_abs_diff_eq!(mode[2], track_right, epsilon = 0.5);
            for part in &transport {
                let part = edges(part);
                assert!(
                    part[0] >= track_left - 0.5 && part[2] <= track_right + 0.5,
                    "{what}: the transport runs out of the tracks' column"
                );
                assert!(
                    part[2] <= mode[0] - 6.0 + 0.5,
                    "{what}: the transport runs into eclipse mode's button"
                );
                for other in &others {
                    assert!(
                        !overlap(part, edges(other)),
                        "{what}: the transport runs over {:?}",
                        other.accessible_label()
                    );
                }
            }
            let [play_left, _, play_right, _] = edges(&play);
            approx::assert_abs_diff_eq!(
                f32::midpoint(play_left, play_right),
                play_middle(&window),
                epsilon = 1.5
            );
        }
    }
}

/// The open bar's parts by id or label, those the preview does not cut away
/// entirely, with their left and right edges measured from the bar's left.
fn bar_parts(window: &MainWindow) -> Vec<(String, f32, f32)> {
    let bar_left = edges(&the_element(window, "MainWindow::time-bar"))[0];
    let mut parts = Vec::new();
    for id in [
        "TimeBar::skip-back",
        "TimeBar::backward",
        "TimeBar::forward",
        "TimeBar::skip-forward",
        "TimeBar::speed-readout",
        "TimeBar::day-track",
        "TimeBar::year-track",
    ] {
        if let Some(part) = ElementHandle::find_by_element_id(window, id).next() {
            let [left, _, right, _] = edges(&part);
            parts.push((id.to_owned(), left - bar_left, right - bar_left));
        }
    }
    for label in ["Play", "TIME", "DATE", "Now", "Time", "Date"] {
        if let Some(part) = ElementHandle::find_by_accessible_label(window, label)
            .find(|element| element.accessible_role() == Some(AccessibleRole::Button))
        {
            let [left, _, right, _] = edges(&part);
            parts.push((label.to_owned(), left - bar_left, right - bar_left));
        }
    }
    parts
}

/// A window at the default panel whose bar holds the compact transport with
/// the time's twelve speeds, and narrower ones down to the app's minimum.
const NARROWEST_BAR_WINDOW: u32 = 986;
const NARROWER_WINDOWS: [u32; 6] = [520, 600, 700, 800, 900, 985];

/// The open bar is never narrower than the bar that holds the compact
/// transport, whichever kind of play it shows: in a narrower preview it keeps
/// that width and that layout, centered on the preview, and the preview cuts
/// its edges.
#[test]
fn test_a_narrow_preview_cuts_the_bar_rather_than_squeezing_it() {
    for date in [false, true] {
        let (window, _sent) = wired_window();
        window.set_time_bar_pinned(true);
        window.invoke_time_kind(date);
        window
            .window()
            .set_size(slint::PhysicalSize::new(NARROWEST_BAR_WINDOW, 5000));
        let bar_width = the_element(&window, "MainWindow::time-bar").size().width;
        let layout = bar_parts(&window);
        assert_eq!(layout.len(), 13, "the whole bar shows at the narrowest");
        for width in NARROWER_WINDOWS {
            window
                .window()
                .set_size(slint::PhysicalSize::new(width, 5000));
            let what = format!("{width} px window, date: {date}");
            let [bar_left, _, bar_right, _] = edges(&the_element(&window, "MainWindow::time-bar"));
            let [preview_left, _, preview_right, _] =
                edges(&the_element(&window, "MainWindow::image-container"));
            approx::assert_abs_diff_eq!(bar_right - bar_left, bar_width, epsilon = 0.5);
            approx::assert_abs_diff_eq!(
                f32::midpoint(bar_left, bar_right),
                f32::midpoint(preview_left, preview_right),
                epsilon = 0.5
            );
            let shown = bar_parts(&window);
            assert!(
                shown.iter().any(|(name, _, _)| name == "Play"),
                "{what}: play is cut away"
            );
            for (name, left, right) in shown {
                let (_, at_left, at_right) = layout
                    .iter()
                    .find(|(id, _, _)| *id == name)
                    .expect("a part of the bar");
                assert!(
                    (left - at_left).abs() < 0.5 && (right - at_right).abs() < 0.5,
                    "{what}: {name} stands at {left}..{right} in the bar, not {at_left}..{at_right}"
                );
            }
        }
    }
}

/// Where the preview cuts the bar, open or folded, a strip at each of its
/// edges over the bar fades the controls out; where the bar fits there is none.
#[test]
fn test_the_cut_edges_of_the_bar_fade_out() {
    let (window, _sent) = wired_window();
    let check = |what: &str| -> bool {
        let [bar_left, bar_top, bar_right, bar_bottom] =
            edges(&the_element(&window, "MainWindow::time-bar"));
        let [preview_left, _, preview_right, _] =
            edges(&the_element(&window, "MainWindow::image-container"));
        let cut = bar_right - bar_left > preview_right - preview_left + 0.5;
        let fades: Vec<_> = ["MainWindow::bar-fade-left", "MainWindow::bar-fade-right"]
            .iter()
            .flat_map(|id| ElementHandle::find_by_element_id(&window, id))
            .collect();
        if !cut {
            assert!(fades.is_empty(), "{what}: a bar that fits does not fade");
            return false;
        }
        assert_eq!(fades.len(), 2, "{what}: a cut bar fades at both edges");
        let [left, right] = [edges(&fades[0]), edges(&fades[1])];
        approx::assert_abs_diff_eq!(left[0], preview_left, epsilon = 0.5);
        approx::assert_abs_diff_eq!(right[2], preview_right, epsilon = 0.5);
        for fade in [left, right] {
            approx::assert_abs_diff_eq!(fade[1], bar_top, epsilon = 0.5);
            approx::assert_abs_diff_eq!(fade[3], bar_bottom, epsilon = 0.5);
            assert!(fade[2] - fade[0] < 0.25 * (preview_right - preview_left));
        }
        true
    };
    window
        .window()
        .set_size(slint::PhysicalSize::new(520, 5000));
    check("the folded pill at 520 px");
    window.set_time_bar_pinned(true);
    for width in [520, 700, 900] {
        window
            .window()
            .set_size(slint::PhysicalSize::new(width, 5000));
        assert!(check(&format!("{width} px")), "{width} px: the bar is cut");
    }
    for width in [NARROWEST_BAR_WINDOW, 1000, 1400] {
        window
            .window()
            .set_size(slint::PhysicalSize::new(width, 5000));
        assert!(!check(&format!("{width} px")), "{width} px: the bar fits");
    }
}

/// The part of the bar the preview cuts away takes no clicks: a press on the
/// sidebar where the day track runs under it leaves the time setting alone,
/// and a press on the track's visible part does not.
#[test]
fn test_the_cut_part_of_the_bar_takes_no_clicks() {
    let (window, sent) = wired_window();
    window.set_time_bar_pinned(true);
    window
        .window()
        .set_size(slint::PhysicalSize::new(520, 5000));
    let track = edges(&the_element(&window, "TimeBar::day-track"));
    let preview_left = edges(&the_element(&window, "MainWindow::image-container"))[0];
    assert!(
        track[0] < preview_left - 10.0,
        "the track runs under the sidebar"
    );
    let y = f32::midpoint(track[1], track[3]);
    let _ = excursion_commands(&sent);

    press_at(&window, slint::LogicalPosition::new(preview_left - 5.0, y));
    assert!(
        excursion_commands(&sent).1.is_none(),
        "a press beside the preview reached the track"
    );
    press_at(&window, slint::LogicalPosition::new(preview_left + 5.0, y));
    assert!(
        excursion_commands(&sent).1.is_some(),
        "a press on the track's visible part sets the time"
    );
}

/// Where the picker `label` opens from a click on the part of its button the
/// preview shows, as its left, top, right and bottom edges in the window, or
/// `None` where the preview cuts the whole button away. The testing backend
/// reports a popup's elements from the popup's own origin, so the origin comes
/// from the window, moved inside the window the way Slint moves a popup that
/// would leave it, and a click on the picker's Cancel button there, which
/// closes it, proves the picker stood there.
fn picker_edges(window: &MainWindow, label: &str) -> Option<[f32; 4]> {
    let [preview_left, _, preview_right, _] =
        edges(&the_element(window, "MainWindow::image-container"));
    // A preview narrower than the bar can cut the button out of sight.
    let button = ElementHandle::find_by_accessible_label(window, label)
        .find(|element| element.accessible_role() == Some(AccessibleRole::Button))?;
    let [left, top, right, bottom] = edges(&button);
    let (left, right) = (left.max(preview_left + 1.0), right.min(preview_right - 1.0));

    if left >= right {
        return None;
    }
    press_at(
        window,
        slint::LogicalPosition::new(f32::midpoint(left, right), f32::midpoint(top, bottom)),
    );
    let dialogs = all_of(window, "Dialog");
    assert_eq!(dialogs.len(), 1, "the {label} button opens its picker");
    let size = dialogs[0].size();
    let (x, y) = if label == "Date" {
        (window.get_date_picker_x(), window.get_date_picker_y())
    } else {
        (window.get_time_picker_x(), window.get_time_picker_y())
    };
    let room = window.window().size().to_logical(1.0);
    let (x, y) = (
        x.clamp(0.0, room.width - size.width),
        y.clamp(0.0, room.height - size.height),
    );
    let cancel = point_in(&the_labeled(window, "Cancel"), 0.5, 0.5);
    press_at(
        window,
        slint::LogicalPosition::new(x + cancel.x, y + cancel.y),
    );
    assert!(
        all_of(window, "Dialog").is_empty(),
        "the {label} picker was not at {x}, {y}"
    );
    Some([x, y, x + size.width, y + size.height])
}

/// The date and time pickers open inside the preview wherever they fit, above
/// the bar. Where one is wider than the preview it stands against the
/// window's right edge, so it covers no more of the side bar than its excess
/// width.
#[test]
fn test_the_pickers_open_inside_the_preview() {
    let (window, _sent) = wired_window();
    window.set_time_bar_pinned(true);
    let (mut opened, mut wider) = (0, 0);
    for width in [520, 600, 700, 800, 900, 1000] {
        window
            .window()
            .set_size(slint::PhysicalSize::new(width, 700));
        let preview = edges(&the_element(&window, "MainWindow::image-container"));
        let bar_top = edges(&the_element(&window, "MainWindow::time-bar"))[1];
        for label in ["Date", "Time"] {
            let what = format!("{width} px window, {label} picker");
            let Some([left, top, right, bottom]) = picker_edges(&window, label) else {
                continue;
            };
            opened += 1;
            assert!(
                top >= preview[1] && bottom <= bar_top,
                "{what}: {top}..{bottom} is not between the preview's top and the bar"
            );
            assert!(
                right <= preview[2] + 0.5,
                "{what}: its right edge {right} is past the window's {}",
                preview[2]
            );
            if right - left <= preview[2] - preview[0] {
                assert!(
                    left >= preview[0] - 0.5,
                    "{what}: {left}..{right} reaches over the side bar left of {}",
                    preview[0]
                );
            } else {
                wider += 1;
                approx::assert_abs_diff_eq!(right, preview[2], epsilon = 0.5);
            }
        }
    }
    assert!(opened >= 8, "only {opened} pickers opened");
    assert!(wider > 0, "no window was narrower than a picker");
}

/// In the app's default window the rulers are packed closer than at full
/// size, and a press on each dot still runs at that dot's speed.
#[test]
fn test_each_dot_of_a_compact_ruler_runs_at_its_own_rate() {
    let (window, sent) = wired_window();
    window
        .window()
        .set_size(slint::PhysicalSize::new(1000, 5000));
    open_the_bar(&window);
    let ladder = ExcursionKind::Clock.ladder();
    for (id, side) in [("TimeBar::forward", 1), ("TimeBar::backward", -1)] {
        let ruler = the_element(&window, id);
        let dots = ruler_dot_elements(&ruler);
        assert_eq!(dots.len(), ladder.len());
        let pitch = point_in(&dots[1], 0.5, 0.5).x - point_in(&dots[0], 0.5, 0.5).x;
        assert!(pitch < 10.5, "{id}: the dots stand {pitch} px apart");
        for slot in 0..dots.len() {
            let _ = excursion_commands(&sent);
            press_at(&window, ruler_dot(&ruler, slot));
            let index = if side < 0 {
                ladder.len() - 1 - slot
            } else {
                slot
            };
            let rate = side * i32::try_from(ladder[index]).expect("a rate");
            assert_eq!(
                excursion_commands(&sent).0,
                [ExcursionCommand::Play {
                    rate,
                    kind: ExcursionKind::Clock
                }],
                "{id}, dot {slot}"
            );
        }
    }
}

/// Every speed of `kind`'s ladder in both directions, running.
fn running_speeds(kind: ExcursionKind) -> Vec<ExcursionState> {
    kind.ladder()
        .iter()
        .map(|&rate| i32::try_from(rate).expect("a rate"))
        .flat_map(|rate| [rate, -rate])
        .map(|rate| ExcursionState {
            running: true,
            rate,
            kind,
        })
        .collect()
}

/// Whether the speed readout shows the speed and what it means in words
/// rather than its arrow alone.
fn the_readout_speaks(window: &MainWindow) -> bool {
    let view = window.get_time_bar();
    let labels: Vec<String> = the_element(window, "TimeBar::speed-readout")
        .query_descendants()
        .find_all()
        .iter()
        .filter_map(|element| element.accessible_label().map(|label| label.to_string()))
        .collect();
    let speaks = |text: &str| labels.iter().any(|label| label == text);
    speaks(view.speed_text.as_str()) && speaks(view.speed_note.as_str())
}

/// At the bar's full width the readout gives every speed of both kinds in
/// words, clear of the transport.
#[test]
fn test_a_full_bar_names_every_speed_in_words() {
    let (window, _sent) = wired_window();
    window
        .window()
        .set_size(slint::PhysicalSize::new(1400, 5000));
    open_the_bar(&window);
    let bar = edges(&the_element(&window, "MainWindow::time-bar"));
    approx::assert_abs_diff_eq!(bar[2] - bar[0], 800.0, epsilon = 0.5);
    for kind in [ExcursionKind::Clock, ExcursionKind::Date] {
        for state in running_speeds(kind) {
            sunlit_earth::time_bar::excursion_changed(&window, Some(state));
            let what = format!("{} ({kind:?})", window.get_time_bar().speed_text);
            assert!(the_readout_speaks(&window), "{what}: the readout is folded");
            let readout = edges(&the_element(&window, "TimeBar::speed-readout"));
            for id in [
                "TimeBar::skip-back",
                "TimeBar::backward",
                "TimeBar::forward",
                "TimeBar::skip-forward",
            ] {
                assert!(
                    !overlap(readout, edges(&the_element(&window, id))),
                    "{what}: the readout runs into {id}"
                );
            }
        }
    }
}

/// Whether the readout folds at each forward speed of `kind` in a window
/// `width` px wide.
fn folds_by_speed(window: &MainWindow, kind: ExcursionKind, width: u32) -> Vec<bool> {
    window
        .window()
        .set_size(slint::PhysicalSize::new(width, 5000));
    running_speeds(kind)
        .into_iter()
        .filter(|state| state.rate > 0)
        .map(|state| {
            sunlit_earth::time_bar::excursion_changed(window, Some(state));
            !the_readout_speaks(window)
        })
        .collect()
}

/// Whether the readout folds depends on the bar's width and the kind of
/// play, never on the speed: from the narrowest window that holds the compact
/// row to the widest bar, and a pixel either side of the fold.
#[test]
fn test_the_readout_folds_alike_at_every_speed() {
    let (window, _sent) = wired_window();
    open_the_bar(&window);
    for kind in [ExcursionKind::Clock, ExcursionKind::Date] {
        let alike = |width: u32| {
            let folds = folds_by_speed(&window, kind, width);
            assert!(
                folds.iter().all(|&folded| folded == folds[0]),
                "{width} px window, {kind:?}: the readout folds at some speeds only: {folds:?}"
            );
            folds[0]
        };
        let mut widths: Vec<u32> = (BAR_WINDOWS[0]..1400).step_by(32).collect();
        widths.push(1400);
        let folded: Vec<bool> = widths.iter().map(|&width| alike(width)).collect();
        assert!(
            folded[0] && !folded[folded.len() - 1],
            "{kind:?}: the readout folds in the narrowest window and speaks at the widest bar"
        );
        let flip = folded
            .iter()
            .position(|&f| !f)
            .expect("a width that speaks");
        let (mut narrow, mut wide) = (widths[flip - 1], widths[flip]);
        while wide - narrow > 1 {
            let middle = narrow.midpoint(wide);
            if alike(middle) {
                narrow = middle;
            } else {
                wide = middle;
            }
        }
        assert!(
            alike(narrow) && !alike(wide),
            "{kind:?}: the fold sits between {narrow} and {wide} px"
        );
    }
}

/// The speed's hint names the speed, which a narrow bar shows as an arrow
/// alone, and follows it while the pointer rests on the readout.
#[test]
fn test_the_speed_hint_names_the_speed() {
    for width in [1000, 1400] {
        let (window, _sent) = wired_window();
        window
            .window()
            .set_size(slint::PhysicalSize::new(width, 5000));
        sunlit_earth::time_bar::refresh(&window);
        open_the_bar(&window);
        let rest = point_in(&the_labeled(&window, "Speed"), 0.5, 0.5);
        hint_over(&window, &the_labeled(&window, "Speed"));
        let names_the_speed = |what: &str| {
            let hints: Vec<_> =
                ElementHandle::find_by_element_id(&window, "MainWindow::hint").collect();
            assert_eq!(
                hints.len(),
                1,
                "{width} px window, {what}: the hint went away"
            );
            let title = the_descendant(&hints[0], "OverlayHint::title")
                .accessible_label()
                .expect("a title");
            let view = window.get_time_bar();
            assert!(
                title.contains(view.speed_text.as_str())
                    && title.contains(view.speed_note.as_str()),
                "{width} px window, {what}: the hint says {title}"
            );
        };
        names_the_speed("at the start");
        for state in [
            ExcursionState {
                running: true,
                rate: 3600,
                kind: ExcursionKind::Clock,
            },
            ExcursionState {
                running: true,
                rate: -5,
                kind: ExcursionKind::Clock,
            },
            ExcursionState {
                running: true,
                rate: 90,
                kind: ExcursionKind::Date,
            },
        ] {
            sunlit_earth::time_bar::excursion_changed(&window, Some(state));
            window
                .window()
                .dispatch_event(slint::platform::WindowEvent::PointerMoved { position: rest });
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(50));
            names_the_speed(&format!("{state:?}"));
        }
    }
}

// A key cap in a row narrower than its label asks for, beside text that would
// take every pixel the cap gives up, and the label alone in the cap's font.
slint::slint! {
    import { KeyCap } from "../sunlit-ui/ui/overlays/common.slint";
    export component SqueezedCap inherits Window {
        out property <length> cap-width: cap.width;
        out property <length> label-width: label.preferred-width;
        HorizontalLayout {
            width: 24px;
            cap := KeyCap {
                text: "Space";
            }
            Text {
                text: "Play / pause";
                wrap: word-wrap;
                horizontal-stretch: 1;
            }
        }
        label := Text {
            text: "Space";
            font-size: 11px;
            visible: false;
        }
    }
}

/// A key cap squeezed by its row keeps the width its label needs.
#[test]
fn test_a_squeezed_key_cap_keeps_its_label_inside() {
    init();
    let window = SqueezedCap::new().expect("a window");
    window.show().expect("shown");
    assert!(
        window.get_cap_width() >= window.get_label_width() + 10.0 - 0.5,
        "a cap {} px wide holds a label {} px wide",
        window.get_cap_width(),
        window.get_label_width()
    );
}

/// In the list of every speed, each side's speeds stand flush right and
/// their meanings flush left, and each side's heading is centered over it,
/// for both kinds of play and at two window widths.
#[test]
fn test_the_speed_list_aligns_speeds_right_and_meanings_left() {
    for date in [false, true] {
        for width in [1000, 1400] {
            let what = format!("{width} px (date: {date})");
            let (window, _sent) = wired_window();
            window.invoke_time_kind(date);
            window
                .window()
                .set_size(slint::PhysicalSize::new(width, 5000));
            window.set_time_bar_pinned(true);
            let_pass(500);
            let readout = the_element(&window, "TimeBar::speed-readout");
            press_at(&window, point_in(&readout, 0.5, 0.5));
            let_pass(100);

            let rows: Vec<_> =
                ElementHandle::find_by_element_id(&window, "TimeBar::choice-row").collect();
            let labels: Vec<_> =
                ElementHandle::find_by_element_id(&window, "TimeBar::rate-label").collect();
            let notes: Vec<_> =
                ElementHandle::find_by_element_id(&window, "TimeBar::rate-note").collect();
            let headings: Vec<_> =
                ElementHandle::find_by_element_id(&window, "TimeBar::rates-heading").collect();
            assert!(rows.len() >= 4, "{what}: the list is open");
            assert_eq!(
                (labels.len(), notes.len()),
                (rows.len(), rows.len()),
                "{what}"
            );
            assert_eq!(headings.len(), 2, "{what}: a heading per side");

            for (side, heading) in headings.iter().enumerate() {
                let column: Vec<usize> = (side..rows.len()).step_by(2).collect();
                let label_rights: Vec<f32> = column.iter().map(|&i| edges(&labels[i])[2]).collect();
                let label_lefts: Vec<f32> = column.iter().map(|&i| edges(&labels[i])[0]).collect();
                for right in &label_rights {
                    approx::assert_abs_diff_eq!(*right, label_rights[0], epsilon = 0.5);
                }
                assert!(
                    label_lefts
                        .iter()
                        .any(|left| (left - label_lefts[0]).abs() > 2.0),
                    "{what}: the speeds of side {side} all start at one place: {label_lefts:?}"
                );
                for &i in &column {
                    let note = edges(&notes[i]);
                    approx::assert_abs_diff_eq!(
                        note[0],
                        edges(&notes[column[0]])[0],
                        epsilon = 0.5
                    );
                    assert!(
                        note[0] > label_rights[0],
                        "{what}: a meaning starts before its speed ends"
                    );
                }
                let row = edges(&rows[column[0]]);
                let heading = edges(heading);
                approx::assert_abs_diff_eq!(
                    f32::midpoint(heading[0], heading[2]),
                    f32::midpoint(row[0], row[2]),
                    epsilon = 0.5
                );
            }
        }
    }
}

/// Pick another day in the open date picker, or another hour in the open
/// time picker, and check that the picker shows it.
fn pick_another(window: &MainWindow, label: &str) {
    if label == "Date" {
        let day = ["15", "16"]
            .into_iter()
            .map(|day| the_labeled(window, day))
            .find(|day| day.accessible_checked() != Some(true))
            .expect("a day not picked");
        day.invoke_accessible_default_action();
        assert_eq!(day.accessible_checked(), Some(true), "the day is picked");
        return;
    }
    let field = || {
        ElementHandle::find_by_accessible_label(window, "hour")
            .next()
            .expect("an hour field")
            .accessible_value()
            .expect("an hour")
    };
    let hour = field();
    let other = (hour.parse::<u32>().expect("a number") + 5) % 24;
    the_labeled(window, &format!("{other} Hours or minutes of 24"))
        .invoke_accessible_default_action();
    assert_eq!(field(), other.to_string(), "the hour is picked");
}

/// A click outside the date or the time picker closes it as Cancel does: a
/// day or an hour picked in it is dropped, the time setting stays as it was,
/// nothing reaches the engine, and the bar folds again once the pointer has
/// left it. The same pick taken with OK changes the setting, which shows the
/// pick is one the setting would follow.
#[test]
fn test_a_click_outside_a_picker_closes_it_as_cancel_does() {
    let away = slint::LogicalPosition::new(900.0, 300.0);
    for (label, ok) in [
        ("Date", true),
        ("Date", false),
        ("Time", true),
        ("Time", false),
    ] {
        let (window, sent) = wired_window();
        let setting = window.get_datetime();
        let _ = excursion_commands(&sent);
        open_the_bar(&window);
        press_at(&window, point_in(&the_labeled(&window, label), 0.5, 0.5));
        assert_eq!(
            all_of(&window, "Dialog").len(),
            1,
            "{label}: the picker opens"
        );
        pick_another(&window, label);
        if ok {
            the_labeled(&window, "OK").invoke_accessible_default_action();
            assert_ne!(
                window.get_datetime(),
                setting,
                "{label}: OK left the setting as it was"
            );
            continue;
        }

        press_at(&window, away);
        let_pass(100);
        assert!(
            all_of(&window, "Dialog").is_empty(),
            "{label}: a click outside closes the picker"
        );
        assert_eq!(
            window.get_datetime(),
            setting,
            "{label}: the setting changed"
        );
        let (commands, pushed) = excursion_commands(&sent);
        assert!(
            commands.is_empty() && pushed.is_none(),
            "{label}: the close reached the engine: {commands:?}"
        );

        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved { position: away });
        let_pass(1000);
        assert!(
            the_element(&window, "MainWindow::time-bar").size().height < 40.0,
            "{label}: the bar stays open after its picker closed"
        );
    }
}

/// A link to an engine nobody runs, for callbacks that only send to it.
fn idle_link() -> sunlit_earth::engine_client::EngineLink {
    let (sender, _) = crossbeam_channel::unbounded();
    sunlit_earth::engine_client::EngineLink::new(sender, vec!["None".to_owned()], vec![1])
}

/// The elements with this accessible label, however many there are.
fn labelled(window: &MainWindow, label: &str) -> Vec<ElementHandle> {
    ElementHandle::find_by_accessible_label(window, label).collect()
}

/// A second launch of the version that runs shows the window and nothing
/// else; one of another version gets the notice, which names both and which
/// "Close" takes away.
#[test]
fn test_the_version_notice_is_for_another_version_and_close_dismisses_it() {
    use sunlit_earth::activation::{Launch, activate};

    let window = create_window();
    let link = idle_link();
    let running = env!("CARGO_PKG_VERSION");

    activate(
        &window,
        &link,
        &Launch {
            version: Some(running.to_owned()),
            ..Launch::default()
        },
    );
    assert!(window.window().is_visible());
    assert!(!window.get_version_notice_open());
    assert!(labelled(&window, "Quit Sunlit Earth").is_empty());

    let launched = "999.0.0-test";
    activate(
        &window,
        &link,
        &Launch {
            version: Some(launched.to_owned()),
            exe: Some("/elsewhere/sunlit-earth".to_owned()),
            ..Launch::default()
        },
    );
    assert!(window.get_version_notice_open());
    assert_eq!(labelled(&window, "Quit Sunlit Earth").len(), 1);
    let message: Vec<String> = ElementQuery::from_root(&window)
        .find_all()
        .into_iter()
        .filter_map(|element| element.accessible_label())
        .map(|label| label.to_string())
        .filter(|label| label.contains(launched))
        .collect();
    assert!(
        message.iter().any(|text| text.contains(running)),
        "the notice names both versions: {message:?}"
    );
    assert!(
        message.iter().all(|text| !text.contains("/elsewhere")),
        "the launched path belongs in the log, not the notice: {message:?}"
    );

    let close = labelled(&window, "Close");
    assert_eq!(close.len(), 1);
    close[0].invoke_accessible_default_action();
    assert!(!window.get_version_notice_open());
    assert!(labelled(&window, "Quit Sunlit Earth").is_empty());
}

/// A launch beside an older version says so in a window of its own, which
/// names this version, and "Close" hides it.
#[test]
fn test_the_older_version_window_names_this_version_and_close_hides_it() {
    init();
    let window = sunlit_earth::activation::older_version_window().unwrap();
    window.show().unwrap();
    assert!(window.window().is_visible());

    let labels: Vec<String> = ElementQuery::from_root(&window)
        .find_all()
        .into_iter()
        .filter_map(|element| element.accessible_label())
        .map(|label| label.to_string())
        .collect();
    assert!(
        labels
            .iter()
            .any(|text| text.contains("older version") && text.contains(env!("CARGO_PKG_VERSION"))),
        "the window says an older version runs and names this one: {labels:?}"
    );

    let close: Vec<ElementHandle> =
        ElementHandle::find_by_accessible_label(&window, "Close").collect();
    assert_eq!(close.len(), 1);
    close[0].invoke_accessible_default_action();
    assert!(!window.window().is_visible());
}

/// The "Start on login" check box, found by its label.
fn start_on_login(window: &MainWindow) -> ElementHandle {
    labelled(window, "Start on login")
        .into_iter()
        .find(|element| element.accessible_checkable() == Some(true))
        .expect("the Start on login switch")
}

/// The texts on screen, by their accessible labels.
fn texts_on_screen(window: &MainWindow) -> Vec<String> {
    ElementQuery::from_root(window)
        .find_all()
        .into_iter()
        .filter_map(|element| element.accessible_label())
        .map(|label| label.to_string())
        .collect()
}

/// The switch shows the entry it is given: on for this copy with nothing
/// under it, on with the other path named, off with the system's switch-off
/// named, and disabled with the line to add where the session has no
/// autostart.
#[test]
fn test_the_start_on_login_switch_shows_the_entry_it_is_given() {
    use std::path::PathBuf;
    use sunlit_earth::autostart::{self, Autostart};

    let window = create_window();

    let this = autostart::this_exe().expect("this test's own path");
    autostart::ui::show(&window, &Autostart::On { exe: this });
    let switch = start_on_login(&window);
    assert_eq!(switch.accessible_checked(), Some(true));
    assert_eq!(switch.accessible_enabled(), Some(true));
    assert!(window.get_autostart_note().is_empty());

    let other = PathBuf::from("/somewhere/else/sunlit-earth");
    autostart::ui::show(&window, &Autostart::On { exe: other });
    assert_eq!(start_on_login(&window).accessible_checked(), Some(true));
    assert!(
        texts_on_screen(&window)
            .iter()
            .any(|text| text.contains("/somewhere/else/sunlit-earth")),
        "the note names the other copy"
    );

    autostart::ui::show(
        &window,
        &Autostart::TurnedOffBySystem {
            exe: PathBuf::from("/x"),
        },
    );
    assert_eq!(start_on_login(&window).accessible_checked(), Some(false));
    assert!(
        texts_on_screen(&window)
            .iter()
            .any(|text| text.starts_with("Turned off in")),
        "the note says where it was switched off"
    );

    let reason = "This session does not start apps from the autostart folder. \
                  Add `exec /usr/bin/sunlit-earth --tray-start hidden` to its config.";
    autostart::ui::show(
        &window,
        &Autostart::Unsupported {
            reason: reason.to_owned(),
        },
    );
    let switch = start_on_login(&window);
    assert_eq!(switch.accessible_checked(), Some(false));
    assert_eq!(switch.accessible_enabled(), Some(false));
    assert!(
        texts_on_screen(&window).iter().any(|text| text == reason),
        "the exec line is on screen"
    );

    autostart::ui::show(&window, &Autostart::Off);
    assert_eq!(start_on_login(&window).accessible_checked(), Some(false));
    assert!(window.get_autostart_note().is_empty());
}

/// A tray icon wired to `window` the way the app wires it, with the settings
/// window's auto-refresh change following into the tray as `app.rs` does.
fn wired_tray(window: &MainWindow) -> sunlit_earth::TrayIcon {
    let tray = sunlit_earth::TrayIcon::new().expect("the testing backend creates a tray icon");
    sunlit_earth::tray::wire(
        &tray,
        window,
        &idle_link(),
        &sunlit_earth::about::AboutController::default(),
    );
    let window_weak = window.as_weak();
    let tray_weak = tray.as_weak();
    window.on_auto_refresh_changed(move || {
        if let (Some(win), Some(tray)) = (window_weak.upgrade(), tray_weak.upgrade()) {
            sunlit_earth::tray::follow_auto_refresh(&tray, win.get_auto_refresh_enabled());
        }
    });
    tray
}

/// Slint's tray backends draw no checkmark, so the state is in the titles:
/// the mark is there exactly when auto-refresh is on, follows the settings
/// window, and a toggle from the tray reaches the window.
#[test]
fn test_the_tray_auto_refresh_title_carries_the_mark() {
    let window = create_window();
    window.set_auto_refresh_enabled(false);
    let tray = wired_tray(&window);
    assert_eq!(tray.get_auto_refresh_title(), "Auto-refresh");

    let checkbox = labelled(&window, "Auto-refresh wallpaper")
        .into_iter()
        .find(|element| element.accessible_checkable() == Some(true))
        .expect("the auto-refresh check box");
    checkbox.invoke_accessible_default_action();
    assert!(window.get_auto_refresh_enabled());
    assert_eq!(tray.get_auto_refresh_title(), "✓ Auto-refresh");

    tray.set_auto_refresh_enabled(false);
    tray.invoke_auto_refresh_toggled(false);
    assert!(
        !window.get_auto_refresh_enabled(),
        "the tray's toggle reached the window"
    );
    assert_eq!(tray.get_auto_refresh_title(), "Auto-refresh");
}

/// "Start on login" in the tray follows each read of the entry and is
/// disabled where the session has no autostart.
#[test]
fn test_the_tray_start_on_login_item_follows_the_entry() {
    use std::path::PathBuf;
    use sunlit_earth::autostart::{self, Autostart};

    let window = create_window();
    let tray = wired_tray(&window);

    autostart::ui::show(
        &window,
        &Autostart::On {
            exe: PathBuf::from("/x/sunlit-earth"),
        },
    );
    assert!(tray.get_autostart_on());
    assert_eq!(tray.get_autostart_title(), "✓ Start on login");
    assert!(tray.get_autostart_available());

    autostart::ui::show(&window, &Autostart::Off);
    assert_eq!(tray.get_autostart_title(), "Start on login");

    autostart::ui::show(
        &window,
        &Autostart::Unsupported {
            reason: "no autostart here".to_owned(),
        },
    );
    assert!(!tray.get_autostart_available());
    assert_eq!(tray.get_autostart_title(), "Start on login");
}

// ---------------------------------------------------------------------------
// Eclipse mode
// ---------------------------------------------------------------------------

/// The peak of the solar eclipse nearest `at`.
fn peak_near(at: time::OffsetDateTime) -> time::OffsetDateTime {
    sunlit_core::scene::eclipse::nearest(at)
        .expect("an eclipse near the date")
        .peak
}

/// Make `at` the window's time setting, as the date and time group holds it.
fn set_the_time(window: &MainWindow, at: time::OffsetDateTime) {
    let at = at.to_offset(time::UtcOffset::UTC);
    window.set_datetime(DateTimeSettings {
        use_custom: true,
        instant: sunlit_core::scene::scene_time::SceneTime::fixed(at)
            .to_config_string()
            .into(),
        hour: f32::from(at.hour()) + f32::from(at.minute()) / 60.0,
        day_of_year: f32::from(at.ordinal()),
        year: at.year(),
    });
    sunlit_earth::time_bar::refresh(window);
}

/// The window's time setting, `None` while live.
fn the_time(window: &MainWindow) -> Option<time::OffsetDateTime> {
    match sunlit_core::scene::scene_time::SceneTime::from_config_string(
        &window.get_datetime().instant,
    ) {
        Some(sunlit_core::scene::scene_time::SceneTime::Fixed(at))
            if window.get_datetime().use_custom =>
        {
            Some(at)
        }
        _ => None,
    }
}

fn following(window: &MainWindow) -> bool {
    window.get_camera_position().eclipse_follow
}

fn press_the_labeled(window: &MainWindow, label: &str) {
    press_at(window, point_in(&the_labeled(window, label), 0.5, 0.5));
}

/// A window in eclipse mode at the peak of the eclipse nearest `near`, with
/// the bar open.
fn in_eclipse_mode(
    near: time::OffsetDateTime,
) -> (
    MainWindow,
    crossbeam_channel::Receiver<sunlit_core::engine::EngineCommand>,
    time::OffsetDateTime,
) {
    let (window, sent) = wired_window();
    let peak = peak_near(near);
    set_the_time(&window, peak - time::Duration::minutes(20));
    open_the_bar(&window);
    press_the_labeled(&window, "Eclipse mode");
    assert!(following(&window), "the button turned the mode on");
    assert_eq!(the_time(&window), Some(peak));
    (window, sent, peak)
}

/// With the time on no eclipse, the button writes the peak of the next one;
/// with the shadow on the Earth, the peak of that one.
#[test]
fn test_the_eclipse_button_enters_the_mode_at_the_eclipse_on_screen_or_the_next() {
    use time::macros::datetime;

    let august = peak_near(datetime!(2026-08-12 12:00 UTC));
    for start in [
        datetime!(2026-06-01 12:00 UTC),
        august - time::Duration::minutes(30),
    ] {
        let (window, sent) = wired_window();
        set_the_time(&window, start);
        open_the_bar(&window);
        press_the_labeled(&window, "Eclipse mode");
        assert!(following(&window), "from {start}");
        assert_eq!(the_time(&window), Some(august), "from {start}");
        let pushed = last_pushed(&sent);
        assert!(pushed.eclipse_follow);
        assert_eq!(
            pushed.time,
            sunlit_core::scene::scene_time::SceneTime::fixed(august)
        );
    }
}

/// Page Down enters the mode at the next eclipse, and the keys and the
/// panel's chevrons walk the eclipses with the mode kept on.
#[test]
fn test_previous_and_next_walk_the_eclipses_and_keep_the_mode() {
    use slint::platform::Key;
    use time::macros::datetime;

    let february = peak_near(datetime!(2026-02-17 12:00 UTC));
    let august = peak_near(datetime!(2026-08-12 12:00 UTC));
    let next_february = peak_near(datetime!(2027-02-06 12:00 UTC));
    let (window, _sent) = wired_window();
    set_the_time(&window, datetime!(2026-01-10 12:00 UTC));
    click_the_globe(&window);
    tap(&window, Key::PageDown);
    assert!(following(&window), "Page Down with the mode off enters it");
    assert_eq!(the_time(&window), Some(february));
    tap(&window, Key::PageDown);
    assert_eq!(the_time(&window), Some(august));
    tap(&window, Key::PageDown);
    assert_eq!(the_time(&window), Some(next_february));
    tap(&window, Key::PageUp);
    assert_eq!(the_time(&window), Some(august));
    assert!(following(&window));

    open_the_bar(&window);
    press_the_labeled(&window, "Next solar eclipse");
    assert_eq!(the_time(&window), Some(next_february));
    press_the_labeled(&window, "Previous solar eclipse");
    press_the_labeled(&window, "Previous solar eclipse");
    assert_eq!(the_time(&window), Some(february));
    assert!(following(&window), "the panel's chevrons keep the mode");
}

/// A click on an eclipse's mark on the date track enters the mode at that
/// eclipse, and the press is the mark's: it writes the peak, not the date
/// track's instant under the pointer.
#[test]
fn test_a_date_track_mark_enters_the_mode_at_its_eclipse() {
    use time::macros::datetime;

    let august = peak_near(datetime!(2026-08-12 12:00 UTC));
    let (window, sent) = wired_window();
    open_the_bar(&window);
    let marks: Vec<ElementHandle> = the_element(&window, "MainWindow::time-bar")
        .query_descendants()
        .match_accessible_role(AccessibleRole::Button)
        .match_predicate(|element| {
            element
                .accessible_label()
                .is_some_and(|label| label.starts_with("Total solar eclipse"))
        })
        .find_all();
    assert_eq!(marks.len(), 1, "2026 has one total solar eclipse");
    let _ = sent.try_iter().count();
    press_at(&window, point_in(&marks[0], 0.5, 0.5));
    assert!(following(&window));
    assert_eq!(the_time(&window), Some(august));
    assert!(last_pushed(&sent).eclipse_follow);
}

/// What the window resolves before and after an exit is the same camera, to
/// a hundredth of a degree.
fn assert_the_same_camera(
    before: &sunlit_core::scene::camera::CameraParams,
    after: &sunlit_core::scene::camera::CameraParams,
    what: &str,
) {
    assert!(
        east_of(after.longitude, before.longitude).abs() < 0.01
            && (after.latitude - before.latitude).abs() < 0.01,
        "{what}: the camera moved from {:.3}, {:.3} to {:.3}, {:.3}",
        before.latitude,
        before.longitude,
        after.latitude,
        after.longitude
    );
}

/// Every exit that keeps the instant on screen leaves the camera where the
/// mode had it, the Sun-fixed mode included, where the time of day is what
/// puts that longitude beneath the camera.
#[test]
fn test_each_exit_ends_the_mode_and_leaves_the_camera_where_it_was() {
    use sunlit_earth::CameraStick;
    use time::macros::datetime;

    type Exit = fn(&MainWindow);
    let exits: [(&str, Exit); 6] = [
        ("the button", |window| {
            press_the_labeled(window, "Eclipse mode");
        }),
        ("the globe drag", |window| {
            window.invoke_mouse_drag_globe_begin();
            window.invoke_mouse_drag_globe(0.0, 0.0);
        }),
        ("the orbit stick", |window| {
            window.invoke_camera_stick(CameraStick::Orbit, 0.0, 0.0, 0.0);
        }),
        ("the Sun-fixed switch", |window| {
            the_button(window, "Fixed toward the Sun").invoke_accessible_default_action();
        }),
        ("a date track pick", |window| {
            window.invoke_time_track(true, 0.95);
        }),
        ("date playback", |window| {
            press_at(window, point_in(&the_labeled(window, "DATE"), 0.5, 0.5));
            press_the_labeled(window, "Play");
        }),
    ];
    for sun_fixed in [false, true] {
        for (what, exit) in exits {
            let (window, _sent, _) = in_eclipse_mode(datetime!(2024-04-08 12:00 UTC));
            if sun_fixed {
                let mut position = window.get_camera_position();
                position.sun_fixed = true;
                window.set_camera_position(position);
            }
            let before = resolved(&window);
            let shown_before = the_time(&window);
            exit(&window);
            let what = format!("{what}, Sun-fixed: {sun_fixed}");
            assert!(!following(&window), "{what}: the mode is still on");
            if shown_before == the_time(&window) {
                assert_the_same_camera(&before, &resolved(&window), &what);
            } else {
                // A new day: the camera the settings now hold is the one
                // the mode resolved at the instant left.
                let position = window.get_camera_position();
                assert!((position.latitude - before.latitude).abs() < 0.01, "{what}");
            }
        }
    }

    for (what, exit) in [
        ("Now", (|window| window.invoke_time_now()) as Exit),
        ("the override switched off", |window| {
            let mut settings = window.get_datetime();
            settings.use_custom = false;
            window.set_datetime(settings);
            window.invoke_datetime_override_toggled();
        }),
        ("a preset", |window| window.invoke_apply_preset(0)),
        ("an orbit step", |window| {
            window.invoke_camera_orbit_step(1, 0);
        }),
    ] {
        let (window, _sent, _) = in_eclipse_mode(datetime!(2024-04-08 12:00 UTC));
        exit(&window);
        assert!(!following(&window), "{what}: the mode is still on");
    }
}

/// What moves the time within the eclipse's day, and what turns, zooms,
/// frames or rolls the picture, keeps the mode.
#[test]
fn test_what_keeps_the_day_or_only_frames_the_picture_keeps_the_mode() {
    use sunlit_earth::CameraStick;
    use time::macros::datetime;

    type Keep = fn(&MainWindow);
    let keeps: [(&str, Keep); 8] = [
        ("a day track drag", |window| {
            window.invoke_time_track(false, 0.6);
        }),
        ("exact time entry", |window| {
            window.invoke_time_exact_time(19, 30);
        }),
        ("the Hour slider", |window| {
            let mut settings = window.get_datetime();
            settings.hour = 20.5;
            window.set_datetime(settings);
            window.invoke_datetime_edited();
        }),
        ("zoom", |window| window.invoke_camera_zoom_step(1)),
        ("look", |window| {
            window.invoke_camera_stick(CameraStick::Look, 0.5, 0.0, 0.1);
        }),
        ("frame", |window| {
            window.invoke_camera_stick(CameraStick::Frame, 0.5, 0.0, 0.1);
        }),
        ("roll", |window| window.invoke_camera_roll(5.0)),
        ("a clock run", |window| press_the_labeled(window, "Play")),
    ];
    for (what, keep) in keeps {
        let (window, _sent, _) = in_eclipse_mode(datetime!(2024-04-08 12:00 UTC));
        keep(&window);
        assert!(following(&window), "{what} ended the mode");
    }
}

/// Date playback and a skip of the date end the mode; a clock run does not.
#[test]
fn test_date_playback_and_a_date_skip_end_the_mode() {
    use sunlit_core::engine::excursion::{ExcursionCommand, ExcursionKind};
    use time::macros::datetime;

    let (window, sent, _) = in_eclipse_mode(datetime!(2024-04-08 12:00 UTC));
    let _ = excursion_commands(&sent);
    press_the_labeled(&window, "Play");
    assert!(following(&window), "a clock run ended the mode");
    let (commands, _) = excursion_commands(&sent);
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ExcursionCommand::Play {
                kind: ExcursionKind::Clock,
                ..
            }
        )),
        "{commands:?}"
    );

    let (window, _sent, _) = in_eclipse_mode(datetime!(2024-04-08 12:00 UTC));
    press_at(&window, point_in(&the_labeled(&window, "DATE"), 0.5, 0.5));
    assert!(
        following(&window),
        "choosing the date's kind alone moved nothing"
    );
    window.invoke_time_skip(true, false);
    assert!(!following(&window), "a skip of the date kept the mode");
}

/// The panel is shown exactly while the bar is open and the mode is on, its
/// rows are the contacts in the displayed zone, and a click on a row writes
/// its instant and keeps the mode. A partial eclipse has three rows.
#[test]
fn test_the_panel_shows_the_phases_and_a_row_click_keeps_the_mode() {
    use slint::Model as _;
    use time::macros::datetime;

    let panels = |window: &MainWindow| {
        ElementHandle::find_by_element_id(window, "MainWindow::eclipse-panel").count()
    };
    let (window, _sent) = wired_window();
    open_the_bar(&window);
    assert_eq!(panels(&window), 0, "the panel shows with the mode off");

    let (window, _sent, peak) = in_eclipse_mode(datetime!(2024-04-08 12:00 UTC));
    assert_eq!(panels(&window), 1, "the panel is not shown in the mode");
    let eclipse = sunlit_core::scene::eclipse::nearest(peak).expect("the eclipse");
    let view = window.get_time_eclipse_panel();
    let times: Vec<String> = view
        .phases
        .iter()
        .map(|phase| phase.time.to_string())
        .collect();
    let hhmm = |at: time::OffsetDateTime| format!("{:02}:{:02}", at.hour(), at.minute());
    let contacts = eclipse.contacts;
    assert_eq!(
        times,
        [
            hhmm(contacts.p1),
            hhmm(contacts.u1.expect("a total eclipse")),
            hhmm(peak),
            hhmm(contacts.u4.expect("a total eclipse")),
            hhmm(contacts.p4),
        ]
    );
    assert_eq!(view.current, 2, "the peak's row is the current one");
    press_the_labeled(&window, "Partial begins");
    assert_eq!(the_time(&window), Some(contacts.p1));
    assert!(following(&window), "a phase row ended the mode");

    let (window, _sent, _) = in_eclipse_mode(datetime!(2025-03-29 12:00 UTC));
    assert_eq!(window.get_time_eclipse_panel().phases.row_count(), 3);
}

/// In the mode the day track shows the eclipse's bands instead of the Sun's
/// altitude, and the altitude comes back once the mode is left.
#[test]
fn test_the_day_track_shows_the_eclipse_in_the_mode() {
    use slint::Model as _;
    use time::macros::datetime;

    let (window, _sent, _) = in_eclipse_mode(datetime!(2024-04-08 12:00 UTC));
    let bands = window.get_time_eclipse_bands();
    assert_eq!(bands.row_count(), 2, "the penumbra's band and the umbra's");
    assert!(window.get_time_bar().eclipse_peak > 0.0);
    press_the_labeled(&window, "Eclipse mode");
    assert_eq!(window.get_time_eclipse_bands().row_count(), 0);
    assert!(
        !window.get_time_shading_key().is_empty(),
        "the altitude shading was not drawn again"
    );
}

/// Load Defaults turns the mode off.
#[test]
fn test_load_defaults_leaves_eclipse_mode() {
    use time::macros::datetime;

    let (window, sent, _) = in_eclipse_mode(datetime!(2024-04-08 12:00 UTC));
    window.invoke_load_defaults();
    assert!(!following(&window));
    assert!(!last_pushed(&sent).eclipse_follow);
}
