//! Clouds on the night side, the switch for the layer, and what it says when
//! it has settled.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use sunlit_core::assets::cloud_source::{CloudImage, CloudSource};
use sunlit_core::assets::cube_layout::CubeTextures;
use sunlit_core::assets::tiles::{self, BuildGate, PackKind};
use sunlit_core::engine::clock::MockClock;
use sunlit_core::engine::{EngineCommand, EngineEvent};
use sunlit_core::params::SceneParams;

use crate::clouds_variant::wait_for_cloud_size;
use crate::cube::CubeFixture;
use crate::groups::surface;
use crate::harness::{Harness, TIMEOUT, gpu, test_params};
use crate::sinks::{RecordingSink, screen};
use crate::support;

/// The size the cloud cases export at. Small on purpose: what they measure is
/// the mean of one window, not a picture.
const CLOUD_CASE_SIZE: (u32, u32) = (256, 128);

/// Half-width of that window, in pixels. The frame's center is the point the
/// camera sits over, and at the cases' zoom this is about three degrees of
/// the globe either way.
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

/// Parameters the cloud cases share: the Earth fixture's surface, the camera
/// over the point the case is about, and nothing else in the window.
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

/// The Congo basin, unlit land on the equator with no city within the window.
const UNLIT_LAND: f32 = 20.0;

/// The hour at which the frame center of a camera at `longitude` is as deep
/// into the night as the globe goes.
fn midnight_at(longitude: f32) -> f32 {
    (NIGHT_HOUR + (180.0 - longitude) / 15.0).rem_euclid(24.0)
}

/// The night side's ground lifted to near display white by its gamma, so a
/// case has a bright ground under a night deck: the Earth fixture's night is
/// dark wherever the equatorial band lies.
fn with_a_lit_night_ground(params: &SceneParams) -> SceneParams {
    SceneParams {
        night_gamma: 16.0,
        ..*params
    }
}

/// A cloud on the night side has to be brighter than the ground it covers.
///
/// The camera is over unlit land, which a night gamma of 1.5 lifts to about 36
/// in the units this prints rather than black, so the ordering is not true for
/// free.
/// The deck reads its own value almost exactly, because the fixture's cloud is
/// 255 or nothing and the night opacity covers the ground completely at any
/// density of one.
#[test]
fn a_night_side_cloud_is_brighter_than_the_land_under_it() {
    let gpu = gpu();
    let harness = surface(&gpu);
    let params = SceneParams {
        night_gamma: 1.5,
        ..cloud_case_params(3, UNLIT_LAND, midnight_at(UNLIT_LAND))
    };

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

/// The cloud layer over the grid is shaded by the Sun, although the grid is a
/// mode whose terminator uniform is not real.
///
/// `write_uniforms` puts -1.0 in `terminator_width` outside blend mode, as the
/// sentinel that tells `fs_sphere` to ignore the sun. `fs_cloud` must not read
/// that same uniform: its ramp would become `smoothstep(1.0, -1.0, n_dot_l)`,
/// which the specification calls indeterminate and which the standard formula
/// inverts. The grid carries the sentinel and keeps the shells under the Sun,
/// so it is the mode that shows this.
///
/// The camera does not move between the two readings and the mode ignores the
/// sun, so the ground under the window is the same texels in both: the whole
/// difference is the layer's own shading.
#[test]
fn a_dayside_cloud_is_brighter_than_a_night_side_one_over_the_grid() {
    let gpu = gpu();
    let harness = surface(&gpu);

    let mut means = Vec::new();
    for hour in [DAY_HOUR, NIGHT_HOUR] {
        let pixels = harness.picture(&cloud_case_params(0, 180.0, hour), CLOUD_CASE_SIZE);
        means.push(center_window_mean(&pixels, CLOUD_CASE_SIZE));
    }
    let (lit, unlit) = (means[0], means[1]);
    println!("grid mode: {lit:.1} at noon, {unlit:.1} at midnight");
    assert!(
        lit > unlit + 40.0,
        "in grid mode the deck reads {lit:.1} at noon and {unlit:.1} at midnight: \
         the cloud ramp is reading the sentinel rather than its own width"
    );
}

/// Day and Night show one map lit evenly, and the cloud layer over it is lit
/// the same way: Day draws day clouds over the night side, Night draws night
/// clouds over the day side, and neither reading moves with the Sun.
///
/// The surface in either mode ignores the Sun too, so the two hours of one
/// mode put the same ground under the window, and any difference between them
/// would be the layer's own shading.
#[test]
fn day_and_night_light_the_clouds_evenly() {
    let gpu = gpu();
    let harness = surface(&gpu);
    let read = |mode: i32, hour: f32| {
        let pixels = harness.picture(&cloud_case_params(mode, 180.0, hour), CLOUD_CASE_SIZE);
        center_window_mean(&pixels, CLOUD_CASE_SIZE)
    };

    let (day_at_noon, day_at_midnight) = (read(1, DAY_HOUR), read(1, NIGHT_HOUR));
    let (night_at_noon, night_at_midnight) = (read(2, DAY_HOUR), read(2, NIGHT_HOUR));
    println!(
        "day mode: {day_at_noon:.1} at noon, {day_at_midnight:.1} at midnight; \
         night mode: {night_at_noon:.1} at noon, {night_at_midnight:.1} at midnight"
    );

    assert!(
        (day_at_noon - day_at_midnight).abs() < 2.0,
        "in day mode the deck reads {day_at_noon:.1} at noon and {day_at_midnight:.1} at \
         midnight, so the night side still darkens it"
    );
    assert!(
        (night_at_noon - night_at_midnight).abs() < 2.0,
        "in night mode the deck reads {night_at_noon:.1} at noon and {night_at_midnight:.1} \
         at midnight, so the day side still lights it"
    );
    assert!(
        day_at_midnight > night_at_noon + 40.0,
        "day clouds on the night side read {day_at_midnight:.1} and night clouds on the day \
         side {night_at_noon:.1}, so the two modes do not light the layer as day and as night"
    );
}

/// Either opacity at zero switches off its own hemisphere and not the layer.
///
/// The banded fixture is 255 or nothing, so the deck over the frame center has a
/// density of one, and the ground under it is lit: it reads near display white
/// and the deck reads `cloud_night`, so which of the two the frame holds is one
/// number. At a night opacity of zero the night side has to show the ground
/// even though the day slider is up, and with only the night slider up the
/// layer still has to draw, which is what `draws_clouds` is for.
///
/// A density of one at a night opacity of zero is also `pow(0, 0)` before
/// `fs_cloud` holds the base off zero, so this is the shape that reaches it.
#[test]
fn an_opacity_at_zero_switches_off_only_its_own_hemisphere() {
    let gpu = gpu();
    let harness = surface(&gpu);
    let base = with_a_lit_night_ground(&cloud_case_params(3, 180.0, NIGHT_HOUR));
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
/// The camera sits over a lit night ground with the deck at one mid density
/// over all of it: the ground under the deck is near display white and the
/// deck itself is `cloud_night`, so what the blend does with the two is visible
/// in one number.
///
/// The three readings also have to be ordered, or a mapping that covered the
/// ground by ignoring the slider would pass the first assertion alone.
#[test]
fn the_night_opacity_reaches_full_cover() {
    let gpu = gpu();
    let harness = surface(&gpu);
    // A uniform map is all background, so haze removal would take the deck
    // this case measures off entirely.
    let base = SceneParams {
        cloud_haze_equator: 0.0,
        cloud_haze_pole: 0.0,
        ..with_a_lit_night_ground(&cloud_case_params(3, 180.0, NIGHT_HOUR))
    };

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

/// The banded fixture map, counting what it is asked for, or a source whose
/// every fetch fails as an outage would.
struct CountingClouds {
    inner: support::FixtureClouds,
    fetches: AtomicU64,
    offline: AtomicBool,
}

impl CountingClouds {
    fn new(offline: bool) -> Self {
        Self {
            inner: support::FixtureClouds::bands(),
            fetches: AtomicU64::new(0),
            offline: AtomicBool::new(offline),
        }
    }

    fn fetches(&self) -> u64 {
        self.fetches.load(Ordering::SeqCst)
    }

    fn set_offline(&self, offline: bool) {
        self.offline.store(offline, Ordering::SeqCst);
    }

    /// Watch the fetch count for [`QUIET`] and answer the highest it reached,
    /// so a poll the engine asked for in the ticks before has had the time to
    /// reach the source.
    fn fetches_after_quiet(&self) -> u64 {
        let until = std::time::Instant::now() + QUIET;
        while std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        self.fetches()
    }
}

/// How long a case watches for a fetch that must not come. The worker makes
/// one within milliseconds of the tick that asks for it.
const QUIET: Duration = Duration::from_secs(1);

impl CountingClouds {
    fn wait_for_fetches(&self, count: u64, what: &str) -> std::time::Instant {
        let deadline = std::time::Instant::now() + TIMEOUT;
        while self.fetches() < count {
            assert!(
                std::time::Instant::now() < deadline,
                "{what}: no fetch {count}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        std::time::Instant::now()
    }
}

impl CloudSource for CountingClouds {
    fn fetch_if_changed(&self, known_etag: Option<&str>) -> Result<Option<CloudImage>, String> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        if self.offline.load(Ordering::SeqCst) {
            return Err("the fixture is offline".to_owned());
        }
        self.inner.fetch_if_changed(known_etag)
    }

    fn describe(&self) -> String {
        "counting cloud fixture".to_owned()
    }
}

/// Wait out one whole engine iteration and hand back the `drawn` of every
/// `CloudsSettled` among the events so far, dropping the rest.
fn clouds_settled(harness: &Harness) -> Vec<bool> {
    for _ in 0..2 {
        let _ = harness.engine.memory_report();
    }
    harness
        .events
        .try_iter()
        .filter_map(|event| match event {
            EngineEvent::CloudsSettled { drawn } => Some(drawn),
            _ => None,
        })
        .collect()
}

fn has_cloud_texture(harness: &Harness) -> bool {
    let report = harness.engine.memory_report().expect("a report");
    report
        .expected
        .iter()
        .any(|texture| texture.label == "cloud_texture")
}

/// The scene of the switch cases: the grid, with a cloud layer that draws when
/// it is switched on.
fn switched(clouds_enabled: bool) -> SceneParams {
    SceneParams {
        clouds_enabled,
        cloud_opacity: 0.85,
        cloud_opacity_night: 0.55,
        ..test_params()
    }
}

/// Switched off, the cloud layer asks its source for nothing over several
/// polls that fall due, decodes nothing and says nothing. Switched on, one
/// fetch follows, the map and its background land and are said to, and the
/// layer is drawn; switched off again, the next poll that falls due is not
/// made.
#[test]
fn a_switched_off_cloud_layer_fetches_nothing_until_it_is_switched_on() {
    let _gpu = gpu();
    let source = Arc::new(CountingClouds::new(false));
    let bands = source.inner.serve_bands();
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let harness = Harness::start(|config| {
        config.params = switched(false);
        config.cloud = Some(Arc::clone(&source) as Arc<dyn CloudSource>);
        config.clock = Arc::clone(&clock) as Arc<dyn sunlit_core::engine::clock::Clock>;
        config.cloud_poll_interval = POLL;
    });

    for hour in 0..3 {
        harness.advance(&clock, POLL);
        assert!(clouds_settled(&harness).is_empty(), "hour {hour}");
    }
    assert_eq!(
        source.fetches_after_quiet(),
        0,
        "a fetch while the layer is off"
    );
    assert!(
        !has_cloud_texture(&harness),
        "a map decoded while the layer is off"
    );

    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(switched(true))));
    wait_for_cloud_size(&harness, bands, "the layer switched on");
    harness.wait_for_slot_texture("cloud_background_texture");
    assert_eq!(
        source.fetches(),
        1,
        "switching on asks for the poll it owed"
    );
    assert_eq!(clouds_settled(&harness), [true], "said once, as drawn");

    let size = (128, 64);
    let drawn = harness.picture(&switched(true), size);
    let bare = harness.picture(
        &SceneParams {
            cloud_opacity: 0.0,
            cloud_opacity_night: 0.0,
            ..switched(true)
        },
        size,
    );
    assert_ne!(drawn, bare, "the bands are not in the picture");
    assert_eq!(
        harness.picture(&switched(false), size),
        bare,
        "switched off, the layer is still drawn"
    );

    harness.advance(&clock, POLL);
    assert!(clouds_settled(&harness).is_empty());
    assert_eq!(
        source.fetches_after_quiet(),
        1,
        "a fetch while the layer is off again"
    );
}

/// Switching the layer off ends a poll that is backing off after a failure:
/// no further attempt is made while it is off, and switching it back on sends
/// the poll at once rather than when the backoff would have run out.
///
/// Without the switch reaching the worker its retry comes 15 s after the
/// failure, while the layer is off, which is what the time bound holds.
#[test]
fn switching_the_layer_off_ends_a_failing_retry() {
    let _gpu = gpu();
    let source = Arc::new(CountingClouds::new(true));
    let bands = source.inner.serve_bands();
    let harness = Harness::start(|config| {
        config.params = switched(true);
        config.cloud = Some(Arc::clone(&source) as Arc<dyn CloudSource>);
    });
    let failed_at = source.wait_for_fetches(1, "the first poll");

    harness.settle_at(&switched(false));
    source.set_offline(false);
    assert_eq!(
        source.fetches_after_quiet(),
        1,
        "an attempt after the layer was switched off"
    );
    assert!(
        !has_cloud_texture(&harness),
        "a map decoded while the layer is off"
    );

    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(switched(true))));
    source.wait_for_fetches(2, "switched back on");
    wait_for_cloud_size(&harness, bands, "switched back on");
    assert!(
        failed_at.elapsed() < FIRST_RETRY,
        "the map arrived {:?} after the failure, which is the backoff's retry rather than the poll the switch owed",
        failed_at.elapsed()
    );
    assert_eq!(source.fetches(), 2);
}

/// The poll interval of the switch cases, on the mock clock.
const POLL: Duration = Duration::from_hours(1);

/// With nothing cached and a source that fails, the layer says it has nothing
/// to show as soon as the first poll has failed, well before the worker's
/// first retry.
#[test]
fn a_cloud_layer_with_nothing_to_show_says_so_before_the_first_retry() {
    let _gpu = gpu();
    let source = Arc::new(CountingClouds::new(true));
    let start = std::time::Instant::now();
    let harness = Harness::start(|config| {
        config.params = switched(true);
        config.cloud = Some(Arc::clone(&source) as Arc<dyn CloudSource>);
    });
    let deadline = start + FIRST_RETRY;
    let mut answer = None;
    while let Ok(event) = harness.events.recv_deadline(deadline) {
        if let EngineEvent::CloudsSettled { drawn } = event {
            answer = Some(drawn);
            break;
        }
    }
    assert_eq!(
        answer,
        Some(false),
        "no answer within the {FIRST_RETRY:?} the first retry waits"
    );
    assert_eq!(source.fetches(), 1);
}

/// What the worker waits before its first retry, from `cloud_fetcher`.
const FIRST_RETRY: Duration = Duration::from_secs(15);

/// Day mode reads no mask of its own, but the cloud layer over it does: with
/// the layer drawn a publish waits for the mask, and with it switched off it
/// goes out without. Every pack but the mask's is built beforehand, and the
/// mask's build is held at the gate.
#[test]
fn a_drawn_cloud_layer_holds_a_day_publish_for_the_mask() {
    let _gpu = gpu();
    for clouds in [false, true] {
        let fixture = CubeFixture::new(&format!("engine_clouds_mask_{clouds}"));
        let textures = CubeTextures::resolve(&fixture.dir.join("textures"));
        let cancel = std::sync::atomic::AtomicBool::new(false);
        for kind in PackKind::all().filter(|&kind| kind != PackKind::Mask) {
            tiles::ensure_pack(&fixture.cache(), kind, &textures, &tiles::FIXTURE, &cancel)
                .expect("build the pack");
        }
        let gate = BuildGate::default();
        gate.hold();
        let sink = Arc::new(RecordingSink::new(vec![screen("only", 0, 64, 32, true)]));
        let sink_for_config = Arc::clone(&sink);
        let harness = Harness::start(|config| {
            fixture.configure(config);
            config.wallpaper = sink_for_config;
            config.params = SceneParams {
                texture_index: 1,
                ..switched(clouds)
            };
            config.build_gate = gate.clone();
        });

        harness.engine.send(EngineCommand::RenderWallpaperNow);
        let within = if clouds { HELD } else { TIMEOUT };
        let deadline = std::time::Instant::now() + within;
        let mut published = None;
        while let Ok(event) = harness.events.recv_deadline(deadline) {
            if let EngineEvent::WallpaperSet(result) = event {
                published = Some(result);
                break;
            }
        }
        if clouds {
            assert!(
                published.is_none(),
                "a publish under a drawn cloud layer went out without the mask"
            );
            gate.open();
            assert!(harness.wait_for_publish().is_ok());
        } else {
            assert!(
                published.is_some_and(|result| result.is_ok()),
                "a publish with the cloud layer off waited for the mask"
            );
            gate.open();
        }
        assert_eq!(sink.publications().len(), 1, "clouds {clouds}");
    }
}

/// How long the held publish is watched for, on the wall clock.
const HELD: Duration = Duration::from_secs(3);

/// The banded fixture map, with every fetch held until the case lets it
/// through, as a slow download would be.
struct HeldClouds {
    inner: support::FixtureClouds,
    held: std::sync::Mutex<bool>,
    released: std::sync::Condvar,
    entered: AtomicU64,
}

impl HeldClouds {
    fn new() -> Self {
        Self {
            inner: support::FixtureClouds::bands(),
            held: std::sync::Mutex::new(true),
            released: std::sync::Condvar::new(),
            entered: AtomicU64::new(0),
        }
    }

    fn release(&self) {
        *self.held.lock().expect("the hold") = false;
        self.released.notify_all();
    }

    fn wait_for_entered(&self, count: u64, what: &str) {
        let deadline = std::time::Instant::now() + TIMEOUT;
        while self.entered.load(Ordering::SeqCst) < count {
            assert!(
                std::time::Instant::now() < deadline,
                "{what}: no fetch {count}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl CloudSource for HeldClouds {
    fn fetch_if_changed(&self, known_etag: Option<&str>) -> Result<Option<CloudImage>, String> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        let mut held = self.held.lock().expect("the hold");
        while *held {
            held = self.released.wait(held).expect("the hold");
        }
        drop(held);
        self.inner.fetch_if_changed(known_etag)
    }

    fn describe(&self) -> String {
        "held cloud fixture".to_owned()
    }
}

/// A download that comes back after the layer was switched off is dropped
/// undecoded: no map reaches the GPU while the layer is off, and switching it
/// back on downloads the map again and draws it.
#[test]
fn a_download_that_returns_after_the_switch_is_not_decoded() {
    let _gpu = gpu();
    let source = Arc::new(HeldClouds::new());
    let bands = source.inner.serve_bands();
    let harness = Harness::start(|config| {
        config.params = switched(true);
        config.cloud = Some(Arc::clone(&source) as Arc<dyn CloudSource>);
    });
    source.wait_for_entered(1, "the first poll");

    harness.settle_at(&switched(false));
    source.release();
    let until = std::time::Instant::now() + QUIET;
    while std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !has_cloud_texture(&harness),
        "a map downloaded while the layer was off was decoded and uploaded"
    );
    assert!(
        clouds_settled(&harness).is_empty(),
        "a dropped download is not the layer's first answer"
    );

    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(switched(true))));
    wait_for_cloud_size(&harness, bands, "switched back on");
    assert_eq!(
        source.entered.load(Ordering::SeqCst),
        2,
        "switching on downloads the map again"
    );
    assert_eq!(clouds_settled(&harness), [true]);
}
