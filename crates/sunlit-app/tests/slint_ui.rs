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
    window.set_datetime(DateTimeSettings {
        use_custom: true,
        hour: 9.25,
        day_of_year: 120.0,
        year_index: 1,
    });
    let (sender, sent) = crossbeam_channel::unbounded();
    let link =
        sunlit_earth::engine_client::EngineLink::new(sender, vec!["None".to_owned()], vec![1]);
    let screens = sunlit_earth::displays::shared_monitors();
    ui_callbacks::register_mouse_callbacks(&window, &link);
    ui_callbacks::register_change_callbacks(
        &window,
        sunlit_core::scene::datetime::base_year(),
        &link,
    );
    ui_callbacks::register_action_callbacks(&window, &link, &screens);
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
