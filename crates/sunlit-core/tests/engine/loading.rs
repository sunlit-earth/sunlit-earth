//! The loading line: the packs a first run prepares, named one by one with
//! the months counted, and on a warm start nothing prepared and the tiles in
//! view named once they have been on their way for a moment.

use std::sync::Arc;
use std::time::{Duration, Instant};

use sunlit_core::assets::cube_layout::CubeTextures;
use sunlit_core::assets::tiles::{self, BuildGate};
use sunlit_core::engine::clock::MockClock;
use sunlit_core::engine::{EngineCommand, EngineEvent, TILES_NAMED_AFTER, TileGate};
use sunlit_core::params::SceneParams;

use crate::harness::{Harness, TIMEOUT, gpu, test_params};
use crate::readiness::build_every_pack;
use crate::test_support::{self, ScratchDir};
use crate::tiles::{day_over, start, tile_report};

/// Longer than the engine stays busy after a frame, so a tick after the mock
/// clock moves this far finds the transcoder's pause gate open.
const IDLE: Duration = Duration::from_secs(3);

/// Every status the engine sent, in order, until it sent `last`.
fn statuses_until(harness: &Harness, last: &str) -> Vec<String> {
    let deadline = Instant::now() + TIMEOUT;
    let mut seen = Vec::new();
    loop {
        let event = harness
            .events
            .recv_deadline(deadline)
            .unwrap_or_else(|_| panic!("no status {last:?} after {seen:?}"));
        if let EngineEvent::Status(text) = event {
            let done = text == last;
            seen.push(text);
            if done {
                return seen;
            }
        }
    }
}

/// The status texts in `seen` that are not `last`, which is at its end.
fn before(seen: &[String]) -> &[String] {
    &seen[..seen.len() - 1]
}

/// Every status the engine sent once it has finished a whole tick, in order.
fn statuses_so_far(harness: &Harness) -> Vec<String> {
    for _ in 0..2 {
        let _ = harness.engine.memory_report();
    }
    harness
        .events
        .try_iter()
        .filter_map(|event| match event {
            EngineEvent::Status(text) => Some(text),
            _ => None,
        })
        .collect()
}

/// A first run: each pack is named while it builds, the first frame's three
/// in their order with March, the month in force, counted first, then
/// nothing once their cubes are resident and the engine is busy, and the
/// rest of the year, counted on from March, once it is idle. The build gate
/// lets one pack through at a time, which a fixture pack built in
/// milliseconds would not leave the engine time to see.
#[test]
fn a_first_run_names_each_pack_it_prepares_and_counts_the_months() {
    let _gpu = gpu();
    let dir = ScratchDir::new("engine_loading_first_run");
    test_support::write_cube_fixture(&dir.join("textures"));
    let gate = BuildGate::default();
    gate.hold();
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let harness = Harness::start(|config| {
        config.cube_textures = CubeTextures::resolve(&dir.join("textures"));
        config.tile_geometry = tiles::FIXTURE;
        config.cache_dir = Some(dir.join("cache"));
        config.texture_resolution = 2048;
        config.clock = clock.clone();
        config.build_gate = gate.clone();
        let mut params = SceneParams {
            texture_index: 3,
            ..test_params()
        };
        params.datetime.custom_day_of_year = 69;
        config.params = params;
    });

    let first = statuses_until(&harness, "Preparing Oceans");
    assert!(
        before(&first)
            .iter()
            .all(|text| text == "Loading Day and Night..."),
        "before the mask: {first:?}"
    );
    for next in ["Preparing March, 1 of 12", "Preparing Night", ""] {
        gate.allow(1);
        let seen = statuses_until(&harness, next);
        assert!(before(&seen).is_empty(), "on the way to {next:?}: {seen:?}");
    }

    harness.advance(&clock, IDLE);
    let seen = statuses_until(&harness, "Preparing April, 2 of 12");
    assert!(before(&seen).is_empty(), "{seen:?}");

    gate.open();
    let rest = statuses_until(&harness, "");
    let places: Vec<usize> = before(&rest)
        .iter()
        .map(|text| {
            let (_, count) = text
                .strip_prefix("Preparing ")
                .and_then(|text| text.split_once(", "))
                .unwrap_or_else(|| panic!("{text:?} names no month"));
            let place = count
                .strip_suffix(" of 12")
                .unwrap_or_else(|| panic!("{text:?} counts no year"));
            place.parse().expect("a place in the year")
        })
        .collect();
    assert!(
        places.windows(2).all(|pair| pair[0] < pair[1]) && places.iter().all(|&p| p > 2),
        "the rest of the year counts on from April: {rest:?}"
    );
}

/// A warm start finds every pack current and prepares none, draws its floors
/// without naming a build, and names the day tiles in view only once they
/// have been on their way for `TILES_NAMED_AFTER`, until they land.
#[test]
fn a_warm_start_prepares_nothing_and_names_the_tiles_after_a_moment() {
    let _gpu = gpu();
    let dir = ScratchDir::new("engine_loading_warm_start");
    test_support::write_earth_fixture(&dir.join("textures"));
    build_every_pack(&dir);
    let builds = BuildGate::default();
    builds.hold();
    let reads = TileGate::default();
    reads.shut();
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let harness = start(&dir, |config| {
        config.params = day_over(20.0, 0.0, 0.55);
        config.clock = clock.clone();
        config.build_gate = builds.clone();
        config.tile_gate = reads.clone();
    });

    let deadline = Instant::now() + TIMEOUT;
    while tile_report(&harness).wanted.is_empty() {
        assert!(
            Instant::now() < deadline,
            "the view's tiles were never wanted"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let early = statuses_so_far(&harness);
    assert!(
        early
            .iter()
            .all(|text| text.is_empty() || text == "Loading Day...")
            && early.last().is_none_or(String::is_empty),
        "a floor on its way is named, the tiles not yet: {early:?}"
    );

    named_after_a_moment(&harness, &clock);
    reads.open();
    landed(&harness);

    // The moment counts again from the next tiles on their way, not from the
    // first ones.
    reads.shut();
    let before_the_move = tile_report(&harness).reads;
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(day_over(
            110.0, 30.0, 0.55,
        ))));
    let deadline = Instant::now() + TIMEOUT;
    while missing(&tile_report(&harness)) == 0 {
        assert!(Instant::now() < deadline, "the move wanted no new tile");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(tile_report(&harness).reads, before_the_move);
    named_after_a_moment(&harness, &clock);
    reads.open();
    landed(&harness);
}

/// The tiles of `report`'s set in view that are neither resident nor failed.
fn missing(report: &sunlit_core::engine::TileReport) -> usize {
    report.wanted[..report.in_view]
        .iter()
        .filter(|id| !report.resident.contains(id) && !report.failed.contains(id))
        .count()
}

/// Tiles in view are on their way with the reads shut: the line says
/// nothing a millisecond short of `TILES_NAMED_AFTER` on the mock clock, and
/// names the day once the moment has passed.
fn named_after_a_moment(harness: &Harness, clock: &MockClock) {
    harness.advance(
        clock,
        TILES_NAMED_AFTER.saturating_sub(Duration::from_millis(1)),
    );
    let short = statuses_so_far(harness);
    assert!(
        short.is_empty(),
        "named before the moment ran out: {short:?}"
    );
    harness.advance(clock, Duration::from_millis(1));
    let waiting = statuses_until(harness, "Loading Day...");
    assert!(before(&waiting).is_empty(), "{waiting:?}");
}

/// With the reads open, the line goes blank once every tile in view is there.
fn landed(harness: &Harness) {
    let seen = statuses_until(harness, "");
    assert!(before(&seen).is_empty(), "{seen:?}");
    let report = tile_report(harness);
    assert_eq!(
        missing(&report),
        0,
        "the line went blank before the tiles were there: {report:#?}"
    );
}
