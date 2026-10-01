//! Months (plan Step 5): the day floor of every month resident, the page
//! table's day half naming the month in force's tiles and no other month's,
//! the month across a near hand-over read ahead, and the custom date and the
//! live clock moving the month.
//!
//! Every case has an engine of its own over a cache built before it starts, so
//! every pack is there from the first tick and the floors of the months not in
//! force follow one a tick once the engine is idle. The Earth fixture has
//! July's faces in every month, so the cases over it look at what the page
//! table names; the fixture bake's two halves of the year are what a picture
//! of the floor tells apart.

use std::sync::Arc;
use std::time::{Duration, Instant};

use sunlit_core::assets::cube_layout::CubeTextures;
use sunlit_core::assets::tiles::{self, PackKind};
use sunlit_core::engine::clock::{Clock, MockClock};
use sunlit_core::engine::{EngineCommand, TileGate, TileReport};
use sunlit_core::params::SceneParams;
use sunlit_core::renderer::tiles::TileId;
use sunlit_core::scene::month::{month_ahead, month_in_force};

use crate::harness::{Harness, TIMEOUT, gpu, test_params};
use crate::readiness::build_every_pack;
use crate::sinks::{RecordingSink, screen};
use crate::test_support::{self, ScratchDir};
use crate::tiles::{day_over, difference, settled, start, tile_report};

/// Longer than the engine stays busy after a frame, so a tick after the mock
/// clock moves this far finds it idle.
const IDLE: Duration = Duration::from_secs(3);

/// Over Africa, at the zoom where the harness's preview wants the coarser
/// tiled level.
fn view() -> SceneParams {
    day_over(20.0, 0.0, 0.55)
}

/// How many day floors are resident.
fn day_floors(harness: &Harness) -> usize {
    harness
        .engine
        .memory_report()
        .expect("a report")
        .expected
        .iter()
        .filter(|texture| texture.label == "day_floor")
        .count()
}

/// Wait until the floor of every month is resident, moving `clock` on so the
/// engine is idle, which is when the floors of the months not in force are
/// made resident.
pub(crate) fn wait_for_every_floor(harness: &Harness, clock: Option<&MockClock>) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let floors = day_floors(harness);
        if floors == 12 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{floors} day floors resident after {TIMEOUT:?}"
        );
        if let Some(clock) = clock {
            harness.advance(clock, IDLE);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The instant calendar month `month` of `year` hands over to the next month
/// in force: halfway through it.
fn hand_over(year: i32, month: u8) -> time::OffsetDateTime {
    let month = time::Month::try_from(month).expect("a month");
    let first = time::Date::from_calendar_date(year, month, 1)
        .expect("a date")
        .midnight()
        .assume_utc();
    first + time::Duration::hours(i64::from(time::util::days_in_month(month, year)) * 12)
}

/// `params` with the custom date at `at`.
fn on(params: &SceneParams, at: time::OffsetDateTime) -> SceneParams {
    let mut params = *params;
    params.datetime.use_custom = true;
    params.datetime.custom_year = at.year();
    params.datetime.custom_day_of_year = at.ordinal();
    params.datetime.custom_hour = f32::from(at.hour()) + f32::from(at.minute()) / 60.0;
    params
}

/// The tiles of `report`'s set in view that are neither resident nor failed.
fn missing(report: &TileReport) -> usize {
    report.wanted[..report.in_view]
        .iter()
        .filter(|id| !report.resident.contains(id) && !report.failed.contains(id))
        .count()
}

/// What the set and the page table say while `month` is in force and `ahead`
/// is read ahead: the day tiles in view and those the table names are that
/// month's alone, and those of another month in the set are the month
/// ahead's, after everything in view.
fn assert_month(report: &TileReport, month: usize, ahead: Option<usize>, at: &str) {
    let day = |id: &&TileId| matches!(id.pack, PackKind::Day(_));
    assert!(
        report.wanted[..report.in_view]
            .iter()
            .filter(day)
            .all(|id| id.pack == PackKind::Day(month)),
        "{at}: a tile of another month is wanted in view: {report:#?}"
    );
    assert!(
        report
            .named
            .iter()
            .filter(day)
            .all(|id| id.pack == PackKind::Day(month)),
        "{at}: the page table names another month's tile: {report:#?}"
    );
    assert!(
        report.wanted[report.in_view..]
            .iter()
            .filter(day)
            .all(|id| id.pack == PackKind::Day(month) || Some(id.pack) == ahead.map(PackKind::Day)),
        "{at}: a tile of a month neither in force nor ahead is wanted: {report:#?}"
    );
}

/// The day surface on the custom date `at` with nothing the Sun moves drawn
/// over it or into it, so a picture changes with the floor alone.
fn unlit_day(at: time::OffsetDateTime) -> SceneParams {
    on(
        &SceneParams {
            texture_index: 1,
            diffuse_shading: false,
            spec_intensity: 0.0,
            atmo_enabled: false,
            star_intensity: 0.0,
            sun_glow: 0.0,
            sun_rays: 0.0,
            sun_size: 0.0,
            ..test_params()
        },
        at,
    )
}

/// An engine over the fixture bake, every pack built before it starts, at the
/// resolution that allows no tile, so a picture is the floors' alone. The
/// bake draws January to June from January's faces and July to December from
/// July's.
fn fixture_floors(
    name: &str,
    every_floor: Option<bool>,
    params: SceneParams,
    clock: &Arc<MockClock>,
) -> (ScratchDir, Harness) {
    let (dir, textures) = fixture_cache(name);
    let harness = fixture_engine(&dir, textures, every_floor, params, clock);
    harness.wait_for_textures("the first floors");
    (dir, harness)
}

/// The fixture bake under `name`, every pack built.
fn fixture_cache(name: &str) -> (ScratchDir, CubeTextures) {
    let dir = ScratchDir::new(name);
    test_support::write_cube_fixture(&dir.join("textures"));
    let textures = CubeTextures::resolve(&dir.join("textures"));
    let cancel = std::sync::atomic::AtomicBool::new(false);
    for kind in PackKind::all() {
        tiles::ensure_pack(
            &dir.join("cache"),
            kind,
            &textures,
            &tiles::FIXTURE,
            &cancel,
        )
        .expect("build the pack");
    }
    (dir, textures)
}

/// An engine over the cache `fixture_cache` built, started and not waited on.
fn fixture_engine(
    dir: &ScratchDir,
    textures: CubeTextures,
    every_floor: Option<bool>,
    params: SceneParams,
    clock: &Arc<MockClock>,
) -> Harness {
    let clock_for_config = Arc::clone(clock);
    Harness::start(|config| {
        config.every_floor = every_floor;
        config.cube_textures = textures;
        config.tile_geometry = tiles::FIXTURE;
        config.cache_dir = Some(dir.join("cache"));
        config.texture_resolution = 2048;
        config.clock = clock_for_config;
        config.params = params;
    })
}

/// The day floors resident once the engine has been idle for as many ticks
/// as it would take to make every month's resident.
fn floors_when_idle(harness: &Harness, clock: &MockClock) -> usize {
    for _ in 0..15 {
        harness.advance(clock, IDLE);
        harness.settle();
    }
    day_floors(harness)
}

/// Every month's floor stays resident once its pack has landed, and the date
/// draws its own month's floor at once, with no pack built or landed in
/// between and no floor let go of. With nothing the Sun moves in the picture,
/// a day within June changes nothing, while a day across its hand-over
/// changes the floor from January's faces to July's; going back to March
/// draws March's picture again.
#[test]
fn every_months_floor_stays_resident_and_the_date_draws_its_own_at_once() {
    let _gpu = gpu();
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let march = unlit_day(hand_over(2026, 3) - time::Duration::days(6));
    // June hands over to July at midnight into the 16th.
    let june = hand_over(2026, 6) + time::Duration::hours(12);
    let [june_14, june_15, june_16] =
        [2, 1, 0].map(|days| unlit_day(june - time::Duration::days(days)));
    let (_dir, harness) = fixture_floors("engine_months_floors", Some(true), march, &clock);
    wait_for_every_floor(&harness, Some(&clock));

    let frame = crate::groups::FRAME;
    let first = harness.picture(&march, frame);
    let [a, b, c] = [june_14, june_15, june_16].map(|params| harness.picture(&params, frame));
    let again = harness.picture(&march, frame);
    assert!(a == b, "a day within June changes the picture");
    let (faces, _) = difference(&b, &c, 0);
    assert!(
        faces > 0.5,
        "July is not drawn from its own floor: a day across the hand-over changes the \
         picture by a mean of {faces:.3}"
    );
    assert_eq!(again, first, "March is drawn from its own floor again");
    assert_eq!(day_floors(&harness), 12, "a floor was let go of");
}

/// A CPU adapter keeps the month in force's floor and, within a day of a
/// hand-over, the month ahead's, and lets go of the rest however long the
/// engine is idle (plan departure 35). A month whose floor was let go of, or
/// never made resident, is drawn from its own floor in the frame that names
/// it all the same.
#[test]
fn a_cpu_adapter_keeps_the_month_in_forces_floor_and_the_month_aheads() {
    let _gpu = gpu();
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let june = hand_over(2026, 6) + time::Duration::hours(12);
    let june_10 = unlit_day(june - time::Duration::days(6));
    let (_dir, harness) = fixture_floors("engine_months_cpu_floors", Some(false), june_10, &clock);
    assert_eq!(floors_when_idle(&harness, &clock), 1, "June's alone");

    let frame = crate::groups::FRAME;
    let june_15 = harness.picture(&unlit_day(june - time::Duration::days(1)), frame);
    assert_eq!(
        floors_when_idle(&harness, &clock),
        2,
        "June's and July's ahead"
    );
    let july_16 = harness.picture(&unlit_day(june), frame);
    let (faces, _) = difference(&june_15, &july_16, 0);
    assert!(
        faces > 0.5,
        "July is not drawn from its own floor: {faces:.3}"
    );
    assert_eq!(
        floors_when_idle(&harness, &clock),
        2,
        "July's and June's behind"
    );

    let march = harness.picture(
        &unlit_day(hand_over(2026, 3) - time::Duration::days(6)),
        frame,
    );
    assert!(
        march == june_15,
        "March is not drawn from its own floor, which was never resident before"
    );
    assert_eq!(floors_when_idle(&harness, &clock), 1, "March's alone");
}

/// How many times a day floor was made resident, one let go of and made
/// resident again counting twice.
fn floors_installed(harness: &Harness) -> u64 {
    tile_report(harness).floors_installed
}

/// A date in June, ten days before July's hand-over.
fn june_10() -> SceneParams {
    unlit_day(hand_over(2026, 6) + time::Duration::hours(12) - time::Duration::days(6))
}

/// A CPU adapter makes the month in force's floor resident once and no other,
/// however long the engine is idle: a floor it lets go of is never made
/// resident first, which would cost a floor's upload on every tick.
#[test]
fn a_cpu_adapter_makes_no_floor_resident_that_it_lets_go_of_again() {
    let _gpu = gpu();
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let (_dir, harness) =
        fixture_floors("engine_months_cpu_installs", Some(false), june_10(), &clock);
    assert_eq!(
        floors_installed(&harness),
        1,
        "every pack landed and only June's floor was made resident"
    );
    assert_eq!(floors_when_idle(&harness, &clock), 1);
    assert_eq!(
        floors_installed(&harness),
        1,
        "a floor was made resident and let go of again while the engine was idle"
    );
}

/// With every floor kept, the floors of the months not in force are made
/// resident while the engine is idle and not before: the first frame waits for
/// the month in force's alone, and a busy engine uploads nothing else. Each is
/// made resident once.
#[test]
fn the_other_floors_wait_for_the_engine_to_be_idle() {
    let _gpu = gpu();
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let (_dir, harness) = fixture_floors("engine_months_busy", Some(true), june_10(), &clock);
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(25));
        let _ = harness.engine.memory_report();
    }
    assert_eq!(day_floors(&harness), 1, "June's alone while busy");
    assert_eq!(floors_installed(&harness), 1);

    wait_for_every_floor(&harness, Some(&clock));
    assert_eq!(floors_when_idle(&harness, &clock), 12);
    assert_eq!(floors_installed(&harness), 12, "a floor was made twice");
}

/// A floor made resident while the engine is idle is uploaded then, and not
/// held as a staging buffer until a frame that may be minutes away.
#[test]
fn a_floor_made_resident_while_idle_is_uploaded_at_once() {
    let _gpu = gpu();
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let (_dir, harness) = fixture_floors("engine_months_upload", Some(true), june_10(), &clock);
    let buffers = |harness: &Harness| harness.engine.memory_report().expect("a report").counters;
    let before = buffers(&harness).buffer_bytes;
    wait_for_every_floor(&harness, Some(&clock));
    let after = buffers(&harness).buffer_bytes;
    assert!(
        after <= before + 1024 * 1024,
        "{before} bytes of buffers before the other floors were made resident, {after} after"
    );
}

/// Without a setting, the engine keeps every floor on a GPU and the month in
/// force's alone on a CPU adapter, which the harness's software adapter is.
#[test]
fn the_floors_kept_follow_the_adapter_unless_the_config_says_otherwise() {
    let _gpu = gpu();
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let (_dir, harness) = fixture_floors("engine_months_default", None, june_10(), &clock);
    let adapter = harness.engine.memory_report().expect("a report").adapter;
    if !["warp", "lavapipe"].contains(&adapter.as_str()) {
        eprintln!("skipped: {adapter} is not a software adapter");
        return;
    }
    assert_eq!(floors_when_idle(&harness, &clock), 1, "June's alone");
}

/// A floor made resident while the engine is idle changes nothing drawn, so
/// no frame is drawn for it and the pause gate the frame would close stays
/// open for the next.
#[test]
fn a_floor_made_resident_while_idle_draws_no_frame() {
    let _gpu = gpu();
    let dir = ScratchDir::new("engine_months_no_frame");
    test_support::write_earth_fixture(&dir.join("textures"));
    build_every_pack(&dir);
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let clock_for_config = Arc::clone(&clock);
    let harness = start(&dir, |config| {
        config.every_floor = Some(true);
        config.clock = clock_for_config;
        config.params = view();
    });
    harness.wait_for_textures("the floors and the preview's tiles");
    let _ = settled(&harness, "the preview's tiles", |r| !r.named.is_empty());
    harness.settle();

    wait_for_every_floor(&harness, Some(&clock));
    let _ = harness.engine.memory_report();
    assert_eq!(
        harness.drained_frame(Duration::ZERO),
        None,
        "a frame was drawn while floors were made resident"
    );
}

/// The first floor to be resident is drawn, the month in force's or not: with
/// the month in force's pack unreadable, the picture is the floor of the first
/// month that comes up, which is a truer one than the grid.
#[test]
fn the_first_floor_is_drawn_when_the_month_in_forces_cannot_be() {
    let _gpu = gpu();
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let (dir, textures) = fixture_cache("engine_months_first_floor");
    let june = tiles::pack_path(&dir.join("cache"), PackKind::Day(5));
    let pack = tiles::Pack::open(&june).expect("a built pack");
    let mut bytes = std::fs::read(&june).expect("read the pack");
    for entry in pack.entries() {
        if entry.whole_face {
            bytes[usize::try_from(entry.offset).expect("an offset")] ^= 0xFF;
        }
    }
    drop(pack);
    std::fs::write(&june, bytes).expect("write the damaged pack");
    let params = june_10();
    let harness = fixture_engine(&dir, textures, Some(true), params, &clock);

    let deadline = Instant::now() + TIMEOUT;
    while day_floors(&harness) < 11 {
        assert!(Instant::now() < deadline, "{} floors", day_floors(&harness));
        harness.advance(&clock, IDLE);
        std::thread::sleep(Duration::from_millis(100));
    }
    let frame = crate::groups::FRAME;
    let day = harness.picture(&params, frame);
    let grid = harness.picture(
        &SceneParams {
            texture_index: 0,
            ..params
        },
        frame,
    );
    let (faces, _) = difference(&day, &grid, 0);
    assert!(
        faces > 0.5,
        "the grid is drawn though a floor is resident: {faces:.3}"
    );
}

/// A sweep of the custom date across the year, an hour either side of every
/// hand-over, forward through all twelve and back: at every step the set in
/// view and the page table hold the month in force's tiles and no other
/// month's, the month across the hand-over is read ahead, and each crossing
/// finds the new month's tiles in view resident already, so it waits for
/// none. Midway in each direction the resolution setting goes down to 2048,
/// where the table names no tile and every floor stays, and back up. The
/// page table's rewrite checks in a debug build that every cell it names is
/// resident, and the engine would stop answering if one did not.
#[test]
fn a_sweep_of_the_custom_date_across_the_year_shows_each_month_its_own_tiles() {
    let _gpu = gpu();
    let dir = ScratchDir::new("engine_months_sweep");
    test_support::write_earth_fixture(&dir.join("textures"));
    build_every_pack(&dir);
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let clock_for_config = Arc::clone(&clock);
    let first = hand_over(2026, 1) - time::Duration::hours(1);
    let harness = start(&dir, |config| {
        config.every_floor = Some(true);
        config.clock = clock_for_config;
        config.params = on(&view(), first);
    });
    harness.wait_for_textures("the floors and the preview's tiles");
    wait_for_every_floor(&harness, Some(&clock));

    let step = |at: time::OffsetDateTime, crossing: bool| {
        let params = on(&view(), at);
        let month = month_in_force(&params.datetime, clock.now_utc());
        let ahead = month_ahead(&params.datetime, clock.now_utc());
        let label = format!("{at}");
        harness.settle_at(&params);
        let report = tile_report(&harness);
        assert_month(&report, month, ahead, &label);
        if crossing {
            assert_eq!(
                missing(&report),
                0,
                "{label}: the crossing waits for tiles of the month it enters"
            );
        }
        let report = settled(&harness, &label, |r| {
            r.named.iter().any(|id| id.pack == PackKind::Day(month))
        });
        assert_month(&report, month, ahead, &label);
        assert!(
            ahead.is_some_and(|ahead| report.wanted[report.in_view..]
                .iter()
                .any(|id| id.pack == PackKind::Day(ahead))),
            "{label}: the month across the hand-over is not read ahead: {report:#?}"
        );
    };
    let down_and_up = |at: time::OffsetDateTime| {
        let before = tile_report(&harness);
        harness
            .engine
            .send(EngineCommand::SetTextureResolution(2048));
        let none = settled(&harness, "at 2048", |r| r.epoch > before.epoch);
        assert!(none.wanted.is_empty() && none.named.is_empty(), "{none:#?}");
        assert_eq!(day_floors(&harness), 12, "a floor went with the tiles");
        harness
            .engine
            .send(EngineCommand::SetTextureResolution(8192));
        let datetime = on(&view(), at).datetime;
        let month = month_in_force(&datetime, clock.now_utc());
        let back = settled(&harness, "at 8192 again", |r| {
            r.epoch > none.epoch && r.named.iter().any(|id| id.pack == PackKind::Day(month))
        });
        assert_month(
            &back,
            month,
            month_ahead(&datetime, clock.now_utc()),
            "at 8192 again",
        );
    };

    for month in 1..=12_u8 {
        let at = hand_over(2026, month);
        step(at - time::Duration::hours(1), false);
        step(at + time::Duration::hours(1), true);
        if month == 6 {
            down_and_up(at + time::Duration::hours(1));
        }
    }
    for month in (1..=12_u8).rev() {
        let at = hand_over(2026, month);
        step(at + time::Duration::hours(1), false);
        step(at - time::Duration::hours(1), true);
        if month == 6 {
            down_and_up(at - time::Duration::hours(1));
        }
    }
}

/// The live clock reads the next month ahead within a day of its hand-over,
/// after everything of the month in force, and crosses into it without a
/// wait: with every read held, the month in force's set in view is complete
/// the moment the clock passes the hand-over, the table names the new month's
/// tiles, and a wallpaper of the preview's own size goes out at once. The old
/// month's tiles leave the set, since the live clock reads only forward.
#[test]
fn the_live_clock_reads_the_next_month_ahead_and_crosses_without_a_wait() {
    let _gpu = gpu();
    let dir = ScratchDir::new("engine_months_live");
    test_support::write_earth_fixture(&dir.join("textures"));
    build_every_pack(&dir);
    let april = hand_over(2026, 4);
    let clock = Arc::new(MockClock::new(april - time::Duration::hours(2)));
    let gate = TileGate::default();
    // The harness's preview is 512 x 256 once its size is quantized.
    let sink = Arc::new(RecordingSink::new(vec![screen("only", 0, 512, 256, true)]));
    let (clock_for_config, gate_for_config, sink_for_config) =
        (Arc::clone(&clock), gate.clone(), Arc::clone(&sink));
    let mut params = view();
    params.datetime.use_custom = false;
    let harness = start(&dir, |config| {
        config.every_floor = Some(true);
        config.clock = clock_for_config;
        config.tile_gate = gate_for_config;
        config.wallpaper = sink_for_config;
        config.params = params;
    });
    harness.wait_for_textures("the floors and the preview's tiles");
    wait_for_every_floor(&harness, Some(&clock));
    assert!(clock.now_utc() < april, "the premise: April is in force");

    let before = settled(&harness, "April and May ahead", |r| {
        r.wanted.iter().any(|id| id.pack == PackKind::Day(4))
    });
    assert_month(&before, 3, Some(4), "before the hand-over");
    let mut in_view: Vec<_> = before.wanted[..before.in_view]
        .iter()
        .map(|id| id.key)
        .collect();
    let mut ahead: Vec<_> = before.wanted[before.in_view..]
        .iter()
        .filter(|id| id.pack == PackKind::Day(4))
        .map(|id| id.key)
        .collect();
    in_view.sort_unstable();
    ahead.sort_unstable();
    assert_eq!(
        ahead, in_view,
        "May's tiles in view are read ahead, each once"
    );

    gate.shut();
    harness.advance(&clock, Duration::from_secs(2 * 3600));
    harness.settle();
    let after = tile_report(&harness);
    assert_month(&after, 4, None, "after the hand-over");
    assert!(after.in_view > 0 && missing(&after) == 0, "{after:#?}");
    assert!(
        after.named.iter().any(|id| id.pack == PackKind::Day(4)),
        "the table does not name May's tiles: {after:#?}"
    );
    assert!(
        !after.wanted.iter().any(|id| id.pack == PackKind::Day(3)),
        "April's tiles are still wanted: {after:#?}"
    );
    assert!(
        harness.publish().is_ok(),
        "the wallpaper waited for tiles that were read ahead"
    );
    gate.open();
}
