//! Reacting to a display layout that changed.

use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

use sunlit_core::display::Monitor;
use sunlit_core::engine::DISPLAY_SETTLE;

use crate::groups::{Watching, plain, watching};
use crate::harness::gpu;
use crate::sinks::{RecordingSink, screen};

/// How long a case waits for something it expects to happen.
pub(crate) const CHANGE_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a case waits before concluding that nothing is going to happen.
///
/// The engine loop wakes every 50 ms whatever else is going on, so this is
/// several of its ticks and not a guess at how fast the machine is.
pub(crate) const NOTHING_HAPPENS_IN: Duration = Duration::from_millis(400);

/// Block until the sink has been asked for its monitors `target` times.
fn wait_for_queries(sink: &RecordingSink, target: usize) {
    let deadline = std::time::Instant::now() + CHANGE_TIMEOUT;
    while sink.queries() < target {
        assert!(
            std::time::Instant::now() < deadline,
            "the engine asked for the monitors {} times, not {target}",
            sink.queries()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A one-screen layout no case has used before, and none will use again.
///
/// The engine remembers the layout it last settled on for as long as it runs,
/// and every case here is about the difference between a layout that changed
/// and one that did not. A shared engine outlives the case that gave it its
/// last layout, so a case that reused a size would be asking about a change
/// that had already happened.
fn an_unfamiliar_layout(id: &str) -> Vec<Monitor> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let width = 256 + NEXT.fetch_add(1, Ordering::Relaxed) * 8;
    vec![screen(id, 0, width, 160, true)]
}

/// Give the engine the settle and a hint's worth of a baseline to compare
/// against, so the cases below start from a layout the engine has seen.
fn establish_baseline(group: &Watching) {
    let layout = an_unfamiliar_layout("baseline");
    group.sink.set_monitors(layout.clone());
    group.hint();
    group.advance(&group.clock, DISPLAY_SETTLE + Duration::from_secs(1));
    let announced = group
        .next_layout(CHANGE_TIMEOUT)
        .expect("a layout the engine has not seen before is one it announces");
    assert_eq!(announced, layout);
}

/// A settled burst is one query and, where nothing moved, nothing else.
///
/// The hints this design pays for and discards are exactly this case: a resume
/// from sleep, a scaling change, a color depth change. Each costs one
/// enumeration and an equal comparison, and nothing on the desk moves. A burst
/// of them costs the same one enumeration, because a change is a burst on both
/// platforms.
#[test]
fn a_settled_burst_of_hints_is_one_query_whatever_it_was_made_of() {
    let gpu = gpu();
    let group = watching(&gpu);
    establish_baseline(group);

    let before = group.sink.queries();
    group.hint();
    group.advance(&group.clock, DISPLAY_SETTLE + Duration::from_secs(1));
    wait_for_queries(&group.sink, before + 1);
    assert!(
        group.next_layout(NOTHING_HAPPENS_IN).is_none(),
        "the layout is the one the engine already knows, so there is nothing to announce"
    );
    assert_eq!(
        group.sink.queries(),
        before + 1,
        "one settled burst is one query"
    );
    assert!(group.sink.publications().is_empty());

    for _ in 0..5 {
        group.hint();
    }
    group.advance(&group.clock, DISPLAY_SETTLE + Duration::from_secs(1));
    wait_for_queries(&group.sink, before + 2);
    std::thread::sleep(NOTHING_HAPPENS_IN);
    assert_eq!(
        group.sink.queries(),
        before + 2,
        "five hints inside the settle window are one layout change"
    );
}

/// A hint that lands while one is pending pushes the deadline out again.
///
/// Trailing rather than leading: a docking station brings its screens up one at
/// a time, and settling on the first of them costs a full render and a visible
/// swap that the second one immediately invalidates.
#[test]
fn a_hint_during_the_settle_moves_the_deadline_it_found() {
    let gpu = gpu();
    let group = watching(&gpu);
    establish_baseline(group);

    let before = group.sink.queries();
    group.hint();
    group.advance(&group.clock, Duration::from_millis(1500));
    group.hint();

    // The first hint's deadline has passed; the second one's has not.
    group.advance(&group.clock, Duration::from_secs(1));
    std::thread::sleep(NOTHING_HAPPENS_IN);
    assert_eq!(
        group.sink.queries(),
        before,
        "the second hint should have moved the deadline the first one set"
    );

    group.advance(&group.clock, Duration::from_secs(1));
    wait_for_queries(&group.sink, before + 1);
}

/// The rule, in the case it was written for: the desk holds a picture this
/// process made for a layout that is gone, so it gets one for the layout that
/// is here, at the sizes that layout has.
#[test]
fn a_layout_that_changed_under_a_published_wallpaper_is_published_again() {
    let gpu = gpu();
    let group = plain(&gpu);
    group.sink.set_monitors(an_unfamiliar_layout("docked"));

    group
        .publish()
        .expect("publishing to a recording sink cannot fail");
    assert_eq!(group.sink.publications().len(), 1);

    // The notebook was undocked: one screen left, and a different one.
    let alone = an_unfamiliar_layout("internal");
    let size = (alone[0].width, alone[0].height);
    group.sink.set_monitors(alone.clone());
    group.hint();
    group.advance(&group.clock, DISPLAY_SETTLE + Duration::from_secs(1));

    let announced = group
        .next_layout(CHANGE_TIMEOUT)
        .expect("a layout that changed is announced");
    assert_eq!(announced, alone);

    group
        .wait_for_publish()
        .expect("publishing to a recording sink cannot fail");
    let published = group.sink.publications();
    assert_eq!(published.len(), 2, "the change published once more");
    assert_eq!(
        published[1].images,
        vec![Some(size)],
        "the second publish is for the screen that is there now, at its own size"
    );
}

/// Somebody who opened the settings window to look and never asked for a
/// wallpaper does not get one because they moved a screen.
#[test]
fn a_layout_that_changed_with_nothing_on_the_desk_is_announced_and_not_published() {
    let gpu = gpu();
    let group = watching(&gpu);
    establish_baseline(group);

    let alone = an_unfamiliar_layout("internal");
    group.sink.set_monitors(alone.clone());
    group.hint();
    group.advance(&group.clock, DISPLAY_SETTLE + Duration::from_secs(1));

    assert_eq!(
        group.next_layout(CHANGE_TIMEOUT),
        Some(alone),
        "the window still has to be told, so its diagram is not a layout from ten minutes ago"
    );
    std::thread::sleep(NOTHING_HAPPENS_IN);
    assert!(
        group.sink.publications().is_empty(),
        "no wallpaper was ever asked for, so a moved screen does not produce one"
    );
}

/// A sink that refused is never asked unprompted, so a layout change cannot put
/// an error in the status line out of nowhere.
#[test]
fn a_layout_that_changed_after_a_refusal_is_announced_and_not_published() {
    let gpu = gpu();
    let group = watching(&gpu);
    group.sink.refuse();

    let refusal = group.publish().expect_err("a refusing sink cannot publish");
    assert_eq!(refusal, RecordingSink::REFUSED);

    establish_baseline(group);
    let alone = an_unfamiliar_layout("internal");
    group.sink.set_monitors(alone.clone());
    group.hint();
    group.advance(&group.clock, DISPLAY_SETTLE + Duration::from_secs(1));

    assert_eq!(group.next_layout(CHANGE_TIMEOUT), Some(alone));
    std::thread::sleep(NOTHING_HAPPENS_IN);
    assert!(group.sink.publications().is_empty());
    assert!(
        group.next_layout(NOTHING_HAPPENS_IN).is_none(),
        "one layout change is one announcement"
    );
}
