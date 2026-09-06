//! Clouds on the night side.

use sunlit_core::params::SceneParams;

use crate::clouds_variant::wait_for_cloud_size;
use crate::groups::surface;
use crate::harness::gpu;
use crate::support;

/// The size the cloud cases export at. Small on purpose: what they measure is
/// the mean of one window, not a picture.
const CLOUD_CASE_SIZE: (u32, u32) = (256, 128);

/// Half-width of that window, in pixels. The frame's center is the point the
/// camera sits over, and the night map's city is thirty degrees away from it,
/// which is well outside this at every zoom that fills the frame.
const CLOUD_WINDOW: u32 = 6;

/// Mean channel value over a square window at the center of an exported frame.
#[allow(clippy::cast_precision_loss)]
fn center_window_mean(pixels: &[u8], size: (u32, u32)) -> f64 {
    let (width, height) = size;
    let mut total = 0u64;
    let mut count = 0u64;
    for y in height / 2 - CLOUD_WINDOW..height / 2 + CLOUD_WINDOW {
        for x in width / 2 - CLOUD_WINDOW..width / 2 + CLOUD_WINDOW {
            let px = ((y * width + x) * 4) as usize;
            total += u64::from(pixels[px]) + u64::from(pixels[px + 1]) + u64::from(pixels[px + 2]);
            count += 3;
        }
    }
    total as f64 / count as f64
}

/// Parameters the cloud cases share: the fixture surface, the camera over the
/// point the case is about, and nothing else in the window.
///
/// The atmosphere, the stars and the Sun are all off so that the window holds
/// the globe and the layer over it and nothing else, and `hour` is what moves
/// the sun: the camera stays where it is, so the surface under the window is the
/// same texels whichever side of the terminator the case asks for.
fn cloud_case_params(texture_index: i32, longitude: f32, hour: f32) -> SceneParams {
    let mut params = SceneParams {
        texture_index,
        sample_count: 1,
        atmo_enabled: false,
        star_intensity: 0.0,
        sun_glow: 0.0,
        camera: sunlit_core::scene::camera::CameraParams {
            longitude,
            latitude: 0.0,
            zoom: 0.26,
            ..sunlit_core::scene::camera::CameraParams::default()
        },
        ..SceneParams::default()
    };
    params.datetime.use_custom = true;
    params.datetime.custom_hour = hour;
    params.datetime.custom_day_of_year = 80;
    params.datetime.custom_year = 2026;
    params
}

/// Noon UTC, where the frame center of a camera at longitude 180 is as deep into
/// the night as the globe goes, and midnight, where the same pixels are lit.
const NIGHT_HOUR: f32 = 12.0;
const DAY_HOUR: f32 = 0.0;

/// A cloud on the night side has to be brighter than the ground it covers.
///
/// The fixture's unlit base is what `BlackMarble_2016.jxl` reads over unlit
/// land, which is 42.0 in the units this prints. The deck reads its own value
/// almost exactly, because the fixture's cloud is 255 or nothing and the night
/// opacity covers the ground completely at any density of one.
#[test]
fn a_night_side_cloud_is_brighter_than_the_land_under_it() {
    let gpu = gpu();
    let harness = surface(&gpu);
    let params = cloud_case_params(3, 180.0, NIGHT_HOUR);

    let covered = harness.picture(&params, CLOUD_CASE_SIZE);
    let bare = harness.picture(
        &SceneParams {
            cloud_opacity: 0.0,
            cloud_opacity_night: 0.0,
            ..params
        },
        CLOUD_CASE_SIZE,
    );

    let (covered, bare) = (
        center_window_mean(&covered, CLOUD_CASE_SIZE),
        center_window_mean(&bare, CLOUD_CASE_SIZE),
    );
    println!("night side: {covered:.1} under the deck, {bare:.1} with the land bare");
    assert!(
        covered > bare + 8.0,
        "a night-side cloud reads {covered:.1} over ground that reads {bare:.1}: \
         the layer is darkening the night side instead of lighting it"
    );
}

/// The cloud layer is shaded by the sun in every texture mode, not only in the
/// one whose terminator uniform is real.
///
/// `write_uniforms` puts -1.0 in `terminator_width` outside blend mode, as the
/// sentinel that tells `fs_sphere` to ignore the sun. `fs_cloud` must not read
/// that same uniform: its ramp would become `smoothstep(1.0, -1.0, n_dot_l)`,
/// which the specification calls indeterminate and which the standard formula
/// inverts. The three single-texture modes are the ones that carry it.
///
/// The camera does not move between the two readings and the mode ignores the
/// sun, so the ground under the window is the same texels in both: the whole
/// difference is the layer's own shading.
#[test]
fn a_dayside_cloud_is_brighter_than_a_night_side_one_in_every_mode() {
    const MODES: [(i32, &str); 3] = [(0, "grid"), (1, "day"), (2, "night")];

    let gpu = gpu();
    let harness = surface(&gpu);

    for (mode, name) in MODES {
        let mut means = Vec::new();
        for hour in [DAY_HOUR, NIGHT_HOUR] {
            let pixels = harness.picture(&cloud_case_params(mode, 180.0, hour), CLOUD_CASE_SIZE);
            means.push(center_window_mean(&pixels, CLOUD_CASE_SIZE));
        }
        let (lit, unlit) = (means[0], means[1]);
        println!("{name} mode: {lit:.1} at noon, {unlit:.1} at midnight");
        assert!(
            lit > unlit + 40.0,
            "in {name} mode the deck reads {lit:.1} at noon and {unlit:.1} at midnight: \
             the cloud ramp is reading the sentinel rather than its own width"
        );
    }
}

/// Either opacity at zero switches off its own hemisphere and not the layer.
///
/// The banded fixture is 255 or nothing, so the deck over the frame center has a
/// density of one, and the night map's city is under it: the ground reads near
/// display white and the deck reads `cloud_night`, so which of the two the frame
/// holds is one number. At a night opacity of zero the night side has to show
/// the ground even though the day slider is up, and with only the night slider
/// up the layer still has to draw, which is what `draws_clouds` is for.
///
/// A density of one at a night opacity of zero is also `pow(0, 0)` before
/// `fs_cloud` holds the base off zero, so this is the shape that reaches it.
#[test]
fn an_opacity_at_zero_switches_off_only_its_own_hemisphere() {
    let gpu = gpu();
    let harness = surface(&gpu);
    let base = cloud_case_params(3, support::NIGHT_FIXTURE_CITY.0, NIGHT_HOUR);
    let read = |day: f32, night: f32| {
        let pixels = harness.picture(
            &SceneParams {
                cloud_opacity: day,
                cloud_opacity_night: night,
                ..base
            },
            CLOUD_CASE_SIZE,
        );
        center_window_mean(&pixels, CLOUD_CASE_SIZE)
    };

    let day_only = read(base.cloud_opacity, 0.0);
    let night_only = read(0.0, base.cloud_opacity_night);
    let deck = f64::from(base.cloud_night) * 255.0;
    println!("day slider alone reads {day_only:.1}, night slider alone reads {night_only:.1}");

    assert!(
        day_only > deck + 40.0,
        "with the night opacity at zero the night side reads {day_only:.1}, which is the deck          at {deck:.1} rather than the lit ground under it"
    );
    assert!(
        (night_only - deck).abs() < 2.0,
        "with only the night opacity up the night side reads {night_only:.1} rather than the          deck's {deck:.1}, so the layer is not drawing when the day slider is zero"
    );
}

/// The night opacity covers the ground at the top of its range, and lets it
/// through below.
///
/// The camera sits over the night map's city with the deck at one mid density
/// over all of it: the ground under the deck is display white and the deck
/// itself is `cloud_night`, so what the blend does with the two is visible in
/// one number.
///
/// The three readings also have to be ordered, or a mapping that covered the
/// ground by ignoring the slider would pass the first assertion alone.
#[test]
fn the_night_opacity_reaches_full_cover() {
    let gpu = gpu();
    let harness = surface(&gpu);
    let base = cloud_case_params(3, support::NIGHT_FIXTURE_CITY.0, NIGHT_HOUR);

    // The banded map is 255 or nothing, so any nonzero night opacity covers the
    // ground completely and there is no covering left to measure. This is the
    // one case that needs the partial map, and the size is how it knows the
    // swap has reached the slot.
    let uniform = harness.clouds.serve_uniform(support::CLOUD_FIXTURE_PARTIAL);
    wait_for_cloud_size(harness, uniform, "the partial cloud map");

    let read = |night_opacity: f32| {
        let pixels = harness.picture(
            &SceneParams {
                cloud_opacity_night: night_opacity,
                ..base
            },
            CLOUD_CASE_SIZE,
        );
        center_window_mean(&pixels, CLOUD_CASE_SIZE)
    };

    let uncovered = read(0.0);
    let partly = read(SceneParams::default().cloud_opacity_night);
    let covered = read(1.0);
    let deck = f64::from(base.cloud_night) * 255.0;
    println!(
        "night opacity 0 reads {uncovered:.1}, default reads {partly:.1}, 1 reads {covered:.1},          the deck alone would read {deck:.1}"
    );

    assert!(
        (covered - deck).abs() < 2.0,
        "at full night opacity the window reads {covered:.1} where the deck alone is {deck:.1},          so the ground under it is still showing through"
    );
    assert!(
        uncovered > covered + 40.0,
        "the window reads {uncovered:.1} with the deck switched off and {covered:.1} with it          covering, which is not the lit ground this case needs under the deck"
    );
    assert!(
        partly > covered + 5.0 && partly < uncovered - 5.0,
        "the default night opacity reads {partly:.1}, which is not between the {covered:.1} of          full cover and the {uncovered:.1} of none, so the slider is not doing the covering"
    );
}
