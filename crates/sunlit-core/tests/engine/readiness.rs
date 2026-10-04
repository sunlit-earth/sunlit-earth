//! Readiness over the tiles (plan decision 10): the wallpaper publish that
//! waits for the tiles its screens want and goes out after `TILE_WAIT` without
//! them, the exports a caller waits on, the order the wanted set takes a
//! waiting publish's tiles in, and a drag, whose threshold holds the set until
//! it stops and whose set converges after it (plan success criteria 5 and 6).
//!
//! Every case has an engine of its own over the Earth fixture, cut as the
//! golden suite cuts it, a mock clock that moves only when the case moves it,
//! and a tile gate the case shuts to keep the tiles it wants on their way.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sunlit_core::assets::tiles::{self, Pack, PackKind};
use sunlit_core::engine::clock::{Clock, MockClock};
use sunlit_core::engine::{EngineCommand, EngineEvent, TILE_WAIT, TileGate, TileReport};
use sunlit_core::params::SceneParams;
use sunlit_core::renderer::residency::{Output, Request, Residency, Surfaces};
use sunlit_core::renderer::surface_sampler_descriptor;
use sunlit_core::renderer::tiles::TileId;

use crate::harness::{Harness, TIMEOUT, gpu};
use crate::sinks::{RecordingSink, screen};
use crate::test_support::{self, ScratchDir};
use crate::textures::blend_params;
use crate::tiles::{EARTH, damage_the_day_tiles, day_over, settled, start, tile_report};

/// The screen every publish here is made for.
const SCREEN: (u32, u32) = (1920, 1080);

/// One of the engine's ticks on the mock clock.
const TICK: Duration = Duration::from_millis(50);

/// Over Africa, at the zoom where the harness's 512 x 288 preview wants the
/// coarser tiled level alone and a 1920 x 1080 screen the finer one nearly
/// everywhere: 30 tiles against 20 and 142.
fn view() -> SceneParams {
    day_over(20.0, 0.0, 0.55)
}

fn finest() -> u8 {
    tiles::Geometry::level_of(EARTH.face)
}

struct Rig {
    dir: ScratchDir,
    harness: Harness,
    sink: Arc<RecordingSink>,
    clock: Arc<MockClock>,
    gate: TileGate,
}

/// An engine over the Earth fixture at `params`, publishing to one screen of
/// `SCREEN`'s size, with everything it wants resident or failed, and the
/// night's pack landed: a day mode is ready without it, and its landing starts
/// a waiting publish's tile wait over at whatever the mock clock reads then.
/// `prepare` runs on the scratch directory before the engine starts.
fn rig(name: &str, params: SceneParams, prepare: impl FnOnce(&ScratchDir)) -> Rig {
    rig_at(name, params, time::OffsetDateTime::UNIX_EPOCH, prepare)
}

/// The same, with the mock clock starting at `at`.
fn rig_at(
    name: &str,
    params: SceneParams,
    at: time::OffsetDateTime,
    prepare: impl FnOnce(&ScratchDir),
) -> Rig {
    rig_configured(name, params, at, prepare, |_| {})
}

/// The same, with `configure` run on the engine's config last.
fn rig_configured(
    name: &str,
    params: SceneParams,
    at: time::OffsetDateTime,
    prepare: impl FnOnce(&ScratchDir),
    configure: impl FnOnce(&mut sunlit_core::engine::EngineConfig),
) -> Rig {
    let dir = ScratchDir::new(name);
    test_support::write_earth_fixture(&dir.join("textures"));
    prepare(&dir);
    let sink = Arc::new(RecordingSink::new(vec![screen(
        "only", 0, SCREEN.0, SCREEN.1, true,
    )]));
    let clock = Arc::new(MockClock::new(at));
    let gate = TileGate::default();
    let (sink_for_config, clock_for_config, gate_for_config) =
        (Arc::clone(&sink), Arc::clone(&clock), gate.clone());
    let harness = start(&dir, |config| {
        config.params = params;
        config.wallpaper = sink_for_config;
        config.clock = clock_for_config;
        config.tile_gate = gate_for_config;
        configure(config);
    });
    harness.wait_for_textures("the floors and the preview's tiles");
    harness.wait_for_slot_texture("night_floor");
    Rig {
        dir,
        harness,
        sink,
        clock,
        gate,
    }
}

/// The tiles of `report`'s set in view that are neither resident nor failed.
fn missing(report: &TileReport) -> usize {
    report.wanted[..report.in_view]
        .iter()
        .filter(|id| !report.resident.contains(id) && !report.failed.contains(id))
        .count()
}

/// The report once `until` holds of it.
fn report_when(harness: &Harness, what: &str, until: impl Fn(&TileReport) -> bool) -> TileReport {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let report = tile_report(harness);
        if until(&report) {
            return report;
        }
        assert!(Instant::now() < deadline, "{what}: {report:#?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Ask for a wallpaper with the gate shut, and hand back the report once the
/// screen's tiles have joined the set.
fn ask_with_the_gate_shut(rig: &Rig, before: &TileReport) -> TileReport {
    rig.gate.shut();
    rig.harness.engine.send(EngineCommand::RenderWallpaperNow);
    let owed = report_when(&rig.harness, "the screen's tiles join the set", |r| {
        r.wanted.len() > before.wanted.len()
    });
    assert!(
        owed.wanted[..owed.in_view]
            .iter()
            .any(|id| id.key.level == finest()),
        "the premise: the screen wants the finer level where the preview does not"
    );
    owed
}

/// The pixels of the `index`th publication's one screen.
fn published(rig: &Rig, index: usize) -> Vec<u8> {
    let publications = rig.sink.publications();
    publications[index].frames[0]
        .as_ref()
        .expect("the one screen is painted")
        .pixels
        .clone()
}

/// A publish waits while a tile its screen wants is on its way, and goes out
/// on the tick the last one lands, drawn from all of them: the same picture a
/// publish with every tile long resident makes, and not the one the coarser
/// level alone makes. While it waits, the preview is drawn with its own cap,
/// not the screen's, so no frame of it reads the screen's finer tiles.
#[test]
fn a_publish_waits_for_the_tiles_its_screen_wants_and_goes_out_when_they_land() {
    let _gpu = gpu();
    let rig = rig("engine_ready_publish", view(), |_| {});
    let before = settled(&rig.harness, "the preview's tiles", |r| {
        !r.wanted.is_empty()
    });
    assert!(
        before.wanted.iter().all(|id| id.key.level < finest()),
        "the premise: the preview wants the coarser level alone"
    );
    let scene = SceneParams {
        star_intensity: 0.3,
        ..view()
    };
    rig.harness.settle();
    let (preview, _, _) = rig.harness.frame_after_change(&scene);

    let owed = ask_with_the_gate_shut(&rig, &before);
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        rig.sink.publications().is_empty(),
        "a wallpaper went out while its tiles were on their way"
    );
    while rig.harness.events.try_recv().is_ok() {}
    rig.gate.open();
    let deadline = Instant::now() + TIMEOUT;
    let mut frames = 0;
    loop {
        match rig.harness.events.recv_deadline(deadline) {
            Ok(EngineEvent::PreviewFrame { rgba, .. }) => {
                frames += 1;
                assert!(
                    rgba == preview,
                    "a preview frame drawn while the screen's tiles landed reads them past its \
                     own cap"
                );
            }
            Ok(EngineEvent::WallpaperSet(result)) => {
                assert!(result.is_ok(), "{result:?}");
                break;
            }
            Ok(_) => {}
            Err(e) => panic!("no publish within {TIMEOUT:?} of the gate opening: {e}"),
        }
    }
    assert!(frames > 0, "the tiles that landed drew no preview frame");
    let report = tile_report(&rig.harness);
    assert!(
        owed.wanted[..owed.in_view]
            .iter()
            .all(|id| report.resident.contains(id) || report.failed.contains(id)),
        "the publish went out before the screen's tiles were resident"
    );

    let first = published(&rig, 0);
    assert!(rig.harness.publish().is_ok());
    assert!(
        published(&rig, 1) == first,
        "a publish with every tile resident draws another picture"
    );
    rig.harness
        .engine
        .send(EngineCommand::SetTextureResolution(4096));
    settled(&rig.harness, "the coarser level", |r| {
        r.epoch == 1 && !r.wanted.is_empty()
    });
    assert!(rig.harness.publish().is_ok());
    let (_, detail) = crate::tiles::difference(&first, &published(&rig, 2), 8);
    assert!(
        detail > first.len() / 4 / 1000,
        "the screen was drawn as the coarser level alone draws it ({detail} pixels differ \
         by more than 8), so not with its own cap"
    );
}

/// A publish whose tiles do not all land within `TILE_WAIT` of the injected
/// clock goes out with what is resident, and is made again, from every tile,
/// when the rest land; not before, however long they take. The preview drawn
/// as they land keeps its own cap, not the one the screen's render held the
/// page table to.
#[test]
fn a_publish_that_waits_out_the_tile_wait_goes_out_and_is_made_again_when_the_tiles_land() {
    let _gpu = gpu();
    let rig = rig("engine_ready_timeout", view(), |_| {});
    let before = settled(&rig.harness, "the preview's tiles", |r| {
        !r.wanted.is_empty()
    });
    let scene = SceneParams {
        star_intensity: 0.3,
        ..view()
    };
    rig.harness.settle();
    let (preview, _, _) = rig.harness.frame_after_change(&scene);
    ask_with_the_gate_shut(&rig, &before);

    rig.harness.advance(
        &rig.clock,
        TILE_WAIT.saturating_sub(Duration::from_millis(1)),
    );
    rig.harness.settle();
    assert!(
        rig.sink.publications().is_empty(),
        "the wallpaper went out before the tile wait was over"
    );
    rig.harness.advance(&rig.clock, Duration::from_millis(1));
    assert!(rig.harness.wait_for_publish().is_ok());
    rig.harness.advance(&rig.clock, TICK);
    rig.harness.settle();
    rig.harness.advance(&rig.clock, TILE_WAIT);
    rig.harness.settle();
    assert_eq!(
        rig.sink.publications().len(),
        1,
        "made again while its tiles were still on their way"
    );
    assert!(
        missing(&tile_report(&rig.harness)) > 0,
        "the screen's tiles left the set with the first publish"
    );

    while rig.harness.events.try_recv().is_ok() {}
    rig.gate.open();
    let deadline = Instant::now() + TIMEOUT;
    let mut frames = 0;
    loop {
        match rig.harness.events.recv_deadline(deadline) {
            Ok(EngineEvent::PreviewFrame { rgba, .. }) => {
                frames += 1;
                assert!(
                    rgba == preview,
                    "a preview frame drawn after the screen's render reads the screen's tiles \
                     past its own cap"
                );
            }
            Ok(EngineEvent::WallpaperSet(result)) => {
                assert!(result.is_ok(), "{result:?}");
                break;
            }
            Ok(_) => {}
            Err(e) => panic!("not made again within {TIMEOUT:?} of the gate opening: {e}"),
        }
    }
    assert!(frames > 0, "the tiles that landed drew no preview frame");
    let (fallback, again) = (published(&rig, 0), published(&rig, 1));
    assert!(
        fallback != again,
        "the wallpaper made again is the one made without the tiles"
    );
    assert!(rig.harness.publish().is_ok());
    assert!(
        published(&rig, 2) == again,
        "the wallpaper made again is not the one every resident tile makes"
    );
}

/// Move the custom date into another month, and hand back that month.
fn to_another_month(rig: &Rig) -> usize {
    to_another_month_from(rig, view())
}

/// The same, with `params` the scene to move.
fn to_another_month_from(rig: &Rig, params: SceneParams) -> usize {
    let mut other = params;
    other.datetime.custom_day_of_year = 260;
    let month = sunlit_core::scene::month::month_in_force(&other.datetime, rig.clock.now_utc());
    assert_ne!(
        month,
        sunlit_core::scene::month::month_in_force(&params.datetime, rig.clock.now_utc()),
        "the premise: another month"
    );
    rig.harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(other)));
    month
}

/// The report once the screen's tiles of `month` are in the set, which is
/// once its floor is resident and its pack open.
fn screen_wanted_in(rig: &Rig, month: usize) -> TileReport {
    let report = report_when(
        &rig.harness,
        "the new month's screen tiles are wanted",
        |r| {
            r.wanted[..r.in_view]
                .iter()
                .any(|id| id.pack == PackKind::Day(month) && id.key.level == finest())
        },
    );
    assert!(missing(&report) > 0, "the premise: the gate holds them");
    report
}

/// A publish asked for while a cube is on its way plans at once, so its
/// screen's tiles join the set the moment the cube is resident, and waits
/// for them `TILE_WAIT` from then at the most. The other month's pack is
/// still to be built when the date moves to it: the transcoder leaves the
/// rest of the year until the engine has been idle for a while on the mock
/// clock, which does not move until the case moves it.
#[test]
fn a_publish_held_for_a_cube_waits_for_its_tiles_no_longer_than_the_tile_wait() {
    let _gpu = gpu();
    let rig = rig("engine_ready_cube_first", view(), |_| {});
    settled(&rig.harness, "the preview's tiles", |r| {
        !r.wanted.is_empty()
    });
    rig.gate.shut();
    let month = to_another_month(&rig);
    rig.harness.engine.send(EngineCommand::RenderWallpaperNow);
    screen_wanted_in(&rig, month);

    rig.harness.advance(
        &rig.clock,
        TILE_WAIT.saturating_sub(Duration::from_millis(1)),
    );
    rig.harness.settle();
    assert!(
        rig.sink.publications().is_empty(),
        "the wallpaper went out before the tile wait was over"
    );
    rig.harness.advance(&rig.clock, Duration::from_millis(1));
    assert!(
        rig.harness.wait_for_publish().is_ok(),
        "no publish once the tile wait was over"
    );
    rig.gate.open();
    assert!(rig.harness.wait_for_publish().is_ok());
}

/// A publish asked for while the new month's floor is on its way puts its
/// screen's tiles in the set at once, the night's included, which are
/// readable already: no frame is drawn before the floor lands, and nothing
/// else would ask for them.
#[test]
fn a_publish_held_for_a_cube_has_its_tiles_read_ahead_of_it() {
    let _gpu = gpu();
    let mut blend = day_over(90.0, 0.0, 0.55);
    blend.texture_index = 3;
    blend.terminator_width = 1.0;
    let rig = rig("engine_ready_ahead", blend, |_| {});
    settled(&rig.harness, "the preview's tiles", |r| {
        !r.wanted.is_empty()
    });
    rig.gate.shut();
    let month = to_another_month_from(&rig, blend);
    rig.harness.engine.send(EngineCommand::RenderWallpaperNow);
    let report = report_when(&rig.harness, "the screen's night tiles are wanted", |r| {
        r.wanted[..r.in_view]
            .iter()
            .any(|id| id.pack == PackKind::Night && id.key.level == finest())
    });
    assert!(
        report
            .wanted
            .iter()
            .all(|id| id.pack != PackKind::Day(month)),
        "the premise: the new month's floor has not landed"
    );
    assert!(rig.sink.publications().is_empty());
}

/// The tile wait starts over only for what could newly make the tiles
/// resident: the pack of the month in force or of the night landing, or the
/// month changing. Packs of other months landing while a publish waits leave
/// it counting.
#[test]
fn the_tile_wait_is_not_renewed_by_the_packs_of_other_months() {
    let _gpu = gpu();
    let rig = rig_configured(
        "engine_ready_other_packs",
        view(),
        time::OffsetDateTime::UNIX_EPOCH,
        |dir| {
            let textures =
                sunlit_core::assets::cube_layout::CubeTextures::resolve(&dir.join("textures"));
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let month = sunlit_core::scene::month::month_in_force(
                &view().datetime,
                time::OffsetDateTime::UNIX_EPOCH,
            );
            for kind in [PackKind::Mask, PackKind::Day(month), PackKind::Night] {
                tiles::ensure_pack(&dir.join("cache"), kind, &textures, &EARTH, &cancel)
                    .expect("build the pack");
            }
        },
        |config| config.every_floor = Some(true),
    );
    let before = settled(&rig.harness, "the preview's tiles", |r| {
        !r.wanted.is_empty()
    });
    ask_with_the_gate_shut(&rig, &before);

    rig.harness
        .advance(&rig.clock, TILE_WAIT.saturating_sub(Duration::from_secs(1)));
    let deadline = Instant::now() + TIMEOUT;
    while rig
        .harness
        .engine
        .memory_report()
        .expect("a report")
        .expected
        .iter()
        .filter(|texture| texture.label == "day_floor")
        .count()
        < 12
    {
        assert!(Instant::now() < deadline, "the other months' packs landed");
        std::thread::sleep(Duration::from_millis(20));
    }
    rig.harness.settle();
    assert!(rig.sink.publications().is_empty());

    rig.harness.advance(&rig.clock, Duration::from_secs(1));
    rig.harness.settle();
    assert!(
        !rig.sink.publications().is_empty(),
        "the wait started over when a pack of another month landed"
    );
}

/// Every pack of the year, built into `dir`'s cache before the engine starts.
pub(crate) fn build_every_pack(dir: &ScratchDir) {
    let textures = sunlit_core::assets::cube_layout::CubeTextures::resolve(&dir.join("textures"));
    let cancel = std::sync::atomic::AtomicBool::new(false);
    for kind in PackKind::all() {
        tiles::ensure_pack(&dir.join("cache"), kind, &textures, &EARTH, &cancel)
            .expect("build the pack");
    }
}

/// The tile wait counts from when the cubes are resident, and starts over
/// when the month in force changes: none of the new month's tiles could be
/// read before. Every pack is built beforehand, so the new floor is resident
/// in the frame that names it and no cube is ever on its way, and the change
/// of month is what starts the wait over, since it opens no pack.
#[test]
fn the_tile_wait_starts_over_when_the_month_in_force_changes() {
    let _gpu = gpu();
    let rig = rig("engine_ready_new_month", view(), build_every_pack);
    let before = settled(&rig.harness, "the preview's tiles", |r| {
        !r.wanted.is_empty()
    });
    ask_with_the_gate_shut(&rig, &before);
    rig.harness
        .advance(&rig.clock, TILE_WAIT.saturating_sub(Duration::from_secs(1)));
    rig.harness.settle();
    assert!(rig.sink.publications().is_empty());

    let month = to_another_month(&rig);
    screen_wanted_in(&rig, month);
    rig.harness.advance(
        &rig.clock,
        TILE_WAIT.saturating_sub(Duration::from_millis(1)),
    );
    rig.harness.settle();
    assert!(
        rig.sink.publications().is_empty(),
        "the wallpaper went out before the wait from the new month's floor was over"
    );
    rig.harness.advance(&rig.clock, Duration::from_millis(1));
    assert!(rig.harness.wait_for_publish().is_ok());
    rig.gate.open();
    assert!(rig.harness.wait_for_publish().is_ok());
}

/// The live clock crossing into the next month while a publish waits for its
/// tiles asks for the new month's tiles at once, though no draw follows: the
/// publish is what sees the change, and it returns without drawing while it
/// waits. The wallpaper then goes out when they land, drawn from them, and is
/// not made again. The clock starts two seconds before January hands over to
/// February, so February's tiles in view of the preview are read ahead as
/// well; the screen's finer ones are not, and those are what it waits for.
#[test]
fn a_live_month_change_during_a_tile_wait_asks_for_the_new_months_tiles() {
    let _gpu = gpu();
    let before_noon = time::OffsetDateTime::UNIX_EPOCH
        + time::Duration::seconds(15 * 86_400 + 11 * 3600 + 59 * 60 + 58);
    let mut params = view();
    params.datetime.use_custom = false;
    let rig = rig_at(
        "engine_ready_live_month",
        params,
        before_noon,
        build_every_pack,
    );
    let before = settled(&rig.harness, "the preview's tiles", |r| {
        !r.wanted.is_empty()
    });
    assert!(
        before.wanted[..before.in_view]
            .iter()
            .all(|id| id.pack == PackKind::Day(0)),
        "the premise: January in force"
    );
    ask_with_the_gate_shut(&rig, &before);

    rig.harness.advance(&rig.clock, Duration::from_secs(3));
    rig.harness.settle();
    let report = tile_report(&rig.harness);
    assert!(
        report.wanted[..report.in_view]
            .iter()
            .any(|id| id.pack == PackKind::Day(1) && id.key.level == finest()),
        "the screen's February tiles are not wanted after the crossing: {report:#?}"
    );
    assert!(rig.sink.publications().is_empty());

    rig.gate.open();
    assert!(rig.harness.wait_for_publish().is_ok());
    rig.harness.advance(&rig.clock, TILE_WAIT);
    rig.harness.settle();
    assert_eq!(
        rig.sink.publications().len(),
        1,
        "it went out without its tiles and was made again"
    );
}

/// Every publish waits for its tiles from its own request: a second one, made
/// for a new view after the first went out and the clock moved on past
/// `TILE_WAIT`, waits for the tiles it wants as the first did.
#[test]
fn a_second_publish_waits_for_its_tiles_as_the_first_did() {
    let _gpu = gpu();
    let rig = rig("engine_ready_second", view(), |_| {});
    let before = settled(&rig.harness, "the preview's tiles", |r| {
        !r.wanted.is_empty()
    });
    ask_with_the_gate_shut(&rig, &before);
    rig.gate.open();
    assert!(rig.harness.wait_for_publish().is_ok());
    rig.harness.advance(&rig.clock, TILE_WAIT * 2);
    rig.harness.settle();

    rig.harness.settle_at(&day_over(-100.0, 0.0, 0.55));
    let moved = settled(&rig.harness, "the preview's tiles over the new view", |r| {
        !r.wanted.is_empty() && r.wanted != before.wanted
    });
    ask_with_the_gate_shut(&rig, &moved);
    rig.harness.advance(
        &rig.clock,
        TILE_WAIT.saturating_sub(Duration::from_millis(1)),
    );
    rig.harness.settle();
    assert_eq!(
        rig.sink.publications().len(),
        1,
        "the second publish went out before its tiles or its tile wait were done"
    );
    rig.gate.open();
    assert!(rig.harness.wait_for_publish().is_ok());
}

/// An export waits for its own tiles and not for the rest of the set: one
/// whose tiles are resident is answered at once while a publish waits for
/// the tiles of its screen.
#[test]
fn an_export_waits_for_its_own_tiles_and_not_for_a_publishs() {
    let _gpu = gpu();
    let rig = rig("engine_ready_own_tiles", view(), |_| {});
    let before = settled(&rig.harness, "the preview's tiles", |r| {
        !r.wanted.is_empty()
    });
    rig.harness.settle();
    let (_, width, height) = rig.harness.frame_after_change(&SceneParams {
        star_intensity: 0.3,
        ..view()
    });
    ask_with_the_gate_shut(&rig, &before);

    let engine = &rig.harness.engine;
    let answered = std::thread::scope(|scope| {
        let (done, answer) = crossbeam_channel::bounded(1);
        scope.spawn(move || {
            let _ = done.send(engine.export_pixels(width, height));
        });
        let answered = answer.recv_timeout(Duration::from_secs(10));
        if answered.is_err() {
            rig.harness.advance(&rig.clock, TILE_WAIT);
            let _ = answer.recv_timeout(TIMEOUT);
        }
        answered
    });
    assert!(
        answered.is_ok_and(|pixels| pixels.is_ok()),
        "an export at the preview's size, whose tiles are resident, waited for the screen's"
    );
    assert!(rig.sink.publications().is_empty());
    rig.gate.open();
    assert!(rig.harness.wait_for_publish().is_ok());
}

/// A publish asked for as the engine stops goes out with what is resident,
/// as an export waiting then is answered: there is no tick left to wait in.
#[test]
fn a_publish_asked_for_as_the_engine_stops_goes_out_with_what_is_resident() {
    let _gpu = gpu();
    let rig = rig("engine_ready_stop", view(), |_| {});
    let before = settled(&rig.harness, "the preview's tiles", |r| {
        !r.wanted.is_empty()
    });
    assert!(
        before.resident.iter().all(|id| id.key.level < finest()),
        "the premise: none of the screen's finer tiles is resident"
    );
    rig.gate.shut();
    let Rig {
        dir: _dir,
        harness,
        sink,
        ..
    } = rig;
    harness.engine.send(EngineCommand::RenderWallpaperNow);
    harness.engine.shutdown();
    let mut results = Vec::new();
    while let Ok(event) = harness.events.try_recv() {
        if let EngineEvent::WallpaperSet(result) = event {
            results.push(result);
        }
    }
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(results[0].is_ok(), "{results:?}");
    assert_eq!(sink.publications().len(), 1);
}

/// A publish already waiting for its tiles when the engine stops goes out
/// with what is resident, though its request came in an earlier batch.
#[test]
fn a_publish_waiting_for_its_tiles_as_the_engine_stops_goes_out_with_what_is_resident() {
    let _gpu = gpu();
    let rig = rig("engine_ready_stop_owed", view(), |_| {});
    let before = settled(&rig.harness, "the preview's tiles", |r| {
        !r.wanted.is_empty()
    });
    ask_with_the_gate_shut(&rig, &before);
    rig.harness.advance(&rig.clock, TICK);
    rig.harness.settle();
    assert!(
        rig.sink.publications().is_empty(),
        "the premise: the publish is still waiting for its tiles"
    );
    let Rig {
        dir: _dir,
        harness,
        sink,
        ..
    } = rig;
    harness.engine.shutdown();
    let mut results = Vec::new();
    while let Ok(event) = harness.events.try_recv() {
        if let EngineEvent::WallpaperSet(result) = event {
            results.push(result);
        }
    }
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(results[0].is_ok(), "{results:?}");
    assert_eq!(sink.publications().len(), 1);
}

/// A tile that fails its read counts as resident: the textures are ready and
/// a publish goes out without the clock moving, drawn from the floor there.
#[test]
fn failed_tiles_hold_neither_readiness_nor_a_publish() {
    let _gpu = gpu();
    let rig = rig("engine_ready_failed", view(), |dir| {
        build_every_pack(dir);
        damage_the_day_tiles(&dir.join("cache"));
    });
    assert!(rig.harness.publish().is_ok());
    let report = tile_report(&rig.harness);
    assert!(report.resident.is_empty(), "{report:#?}");
    assert!(
        report.wanted[..report.in_view]
            .iter()
            .any(|id| id.key.level == finest() && report.failed.contains(id)),
        "the screen's tiles were never wanted: {report:#?}"
    );
}

/// The tiles of a waiting publish come first in the set: the head of it is the
/// set its screen wants alone, as the residency rule computes it for the
/// screen's camera.
#[test]
fn the_tiles_of_a_waiting_publish_come_first() {
    let _gpu = gpu();
    let rig = rig("engine_ready_first", view(), |_| {});
    let before = settled(&rig.harness, "the preview's tiles", |r| {
        !r.wanted.is_empty()
    });
    let owed = ask_with_the_gate_shut(&rig, &before);

    let params = view();
    let month = sunlit_core::scene::month::month_in_force(&params.datetime, rig.clock.now_utc());
    let pack = Pack::open(&tiles::pack_path(
        &rig.dir.join("cache"),
        PackKind::Day(month),
    ))
    .expect("the month's pack");
    let stored = |id: TileId| {
        pack.find(id.key)
            .is_some_and(|entry| !entry.ocean && !entry.whole_face)
    };
    let cpu_adapter = rig.harness.engine.adapter_info().contains("Cpu");
    let alone = Residency::new(EARTH).wanted(&Request {
        outputs: &[Output {
            camera: params.camera,
            width: SCREEN.0,
            height: SCREEN.1,
            export: true,
        }],
        month,
        ahead: None,
        surfaces: Surfaces::Day,
        finest: finest(),
        drag: None,
        anisotropy: surface_sampler_descriptor(cpu_adapter).anisotropy_clamp,
        stored: &stored,
    });
    let screen: HashSet<TileId> = alone.in_view().collect();
    let head: HashSet<TileId> = owed.wanted[..screen.len()].iter().copied().collect();
    assert_eq!(head, screen);
    rig.gate.open();
    assert!(rig.harness.wait_for_publish().is_ok());
}

/// An export a caller waits on waits for its tiles like a publish, and after
/// `TILE_WAIT` is answered with what is resident.
#[test]
fn an_export_a_caller_waits_on_waits_for_its_tiles() {
    let _gpu = gpu();
    let rig = rig("engine_ready_export", view(), |_| {});
    settled(&rig.harness, "the preview's tiles", |r| {
        !r.wanted.is_empty()
    });
    let engine = &rig.harness.engine;
    let answered_within = |wait: Duration, before: &dyn Fn()| {
        std::thread::scope(|scope| {
            let (done, answer) = crossbeam_channel::bounded(1);
            scope.spawn(move || {
                let _ = done.send(engine.export_pixels(SCREEN.0, SCREEN.1));
            });
            assert!(
                answer.recv_timeout(wait).is_err(),
                "answered while its tiles were on their way"
            );
            before();
            answer
                .recv_timeout(TIMEOUT)
                .expect("an answer")
                .expect("pixels")
        })
    };

    rig.gate.shut();
    let waited = answered_within(Duration::from_millis(200), &|| rig.gate.open());
    assert!(
        waited == rig.harness.export(SCREEN.0, SCREEN.1),
        "the export did not wait for every tile"
    );

    rig.gate.shut();
    rig.harness.settle_at(&day_over(-100.0, 0.0, 0.55));
    answered_within(Duration::from_millis(200), &|| {
        rig.harness.advance(&rig.clock, TILE_WAIT);
    });
    rig.gate.open();
}

/// A drag of 5 degrees a tick at the near end of the zoom holds the set at the
/// drag's threshold, and readiness with it, until it stops; within 500 ms of
/// the mock clock after the last move the set is the 1 px one again and every
/// tile of it is resident (plan success criterion 5). Every rewrite of the page
/// table on the way is checked in a debug build: no frame draws a cell without
/// a resident ancestor.
#[test]
fn a_drag_converges_to_the_one_pixel_set_within_half_a_second_of_stopping() {
    let _gpu = gpu();
    let mut scene = day_over(0.0, 0.0, 0.0);
    let rig = rig("engine_ready_drag", scene, |_| {});
    settled(&rig.harness, "the tiles at the near end", |r| {
        !r.wanted.is_empty()
    });

    let mut ready_while_dragging = false;
    for step in 1..=12 {
        rig.clock.advance(TICK);
        scene.camera.longitude += 5.0;
        rig.harness
            .engine
            .send(EngineCommand::UpdateParams(Box::new(scene)));
        std::thread::sleep(TICK);
        let report = tile_report(&rig.harness);
        if step > 1 {
            assert!(report.dragging, "step {step} is not a drag: {report:#?}");
        }
        while let Ok(event) = rig.harness.events.try_recv() {
            ready_while_dragging |= step > 1 && matches!(event, EngineEvent::TexturesReady);
        }
    }
    assert!(
        !ready_while_dragging,
        "the textures were ready during a drag"
    );

    let mut since = Duration::ZERO;
    let mut ready = false;
    let converged = loop {
        let report = tile_report(&rig.harness);
        while let Ok(event) = rig.harness.events.try_recv() {
            ready |= matches!(event, EngineEvent::TexturesReady);
        }
        if !report.dragging && missing(&report) == 0 {
            break report;
        }
        assert!(
            since < Duration::from_millis(500),
            "not converged {since:?} after the drag stopped: {report:#?}"
        );
        rig.harness.advance(&rig.clock, TICK);
        std::thread::sleep(TICK);
        since += TICK;
    };
    println!(
        "a drag of 5 degrees a tick converged {since:?} after it stopped, {} tiles in view",
        converged.in_view
    );
    if !ready {
        rig.harness.wait_for_textures("readiness after the drag");
    }
}

/// With no cube to draw, which is a checkout without its LFS objects or an
/// engine with no cache directory, Blend mode has nothing to wait for: a
/// publish goes out at once and the textures are never reported ready. A
/// pending cube that never comes would otherwise hold every publish forever.
#[test]
fn without_a_cube_a_publish_in_blend_mode_goes_out_and_no_textures_are_ready() {
    let _gpu = gpu();
    let sink = Arc::new(RecordingSink::new(vec![screen(
        "only", 0, SCREEN.0, SCREEN.1, true,
    )]));
    let sink_for_config = Arc::clone(&sink);
    let harness = Harness::start(move |config| {
        config.params = blend_params();
        config.wallpaper = sink_for_config;
    });
    harness.engine.send(EngineCommand::RenderWallpaperNow);
    let mut ready = false;
    let deadline = Instant::now() + TIMEOUT;
    let published = loop {
        match harness.events.recv_deadline(deadline) {
            Ok(EngineEvent::WallpaperSet(result)) => break result,
            Ok(EngineEvent::TexturesReady) => ready = true,
            Ok(_) => {}
            Err(e) => panic!("no publish within {TIMEOUT:?}: {e}"),
        }
    };
    assert!(published.is_ok(), "the publish should have gone out");
    assert_eq!(sink.publications().len(), 1, "exactly one publish");
    let _ = harness.engine.memory_report();
    ready |= std::iter::from_fn(|| harness.events.try_recv().ok())
        .any(|event| matches!(event, EngineEvent::TexturesReady));
    assert!(!ready, "nothing is ready without a cube in Blend mode");
}
