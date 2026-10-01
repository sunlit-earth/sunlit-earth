//! The memory report.

use crate::groups::{FRAME, plain, surface};
use crate::harness::{gpu, test_params};
use crate::textures::{blend_params, mib};
use crate::tiles::{EARTH, settled};

/// The width of every texture the report says the renderer owns under `label`.
pub(crate) fn expected_widths(
    report: &sunlit_core::memory_report::MemoryReport,
    label: &str,
) -> Vec<u32> {
    report
        .expected
        .iter()
        .filter(|texture| texture.label == label)
        .map(|texture| texture.width)
        .collect()
}

#[test]
fn the_report_names_the_textures_the_renderer_owns() {
    let gpu = gpu();
    let harness = surface(&gpu);
    harness.settle_at(&blend_params());
    settled(harness, "the view's tiles", |r| !r.wanted.is_empty());

    let report = harness
        .engine
        .memory_report()
        .expect("the engine should answer with a report");
    println!("{report}");

    for (label, width) in [
        ("night_floor", EARTH.floor),
        ("water_mask", EARTH.mask),
        ("page_table", EARTH.face / EARTH.tile),
    ] {
        assert_eq!(expected_widths(&report, label), [width], "{label}");
    }
    // A day floor a month whose pack has landed, every one at the floor's size.
    let days = expected_widths(&report, "day_floor");
    assert!(
        (1..=12).contains(&days.len()) && days.iter().all(|&width| width == EARTH.floor),
        "day_floor: {days:?}"
    );
    assert_eq!(
        expected_widths(&report, "tile_array").len(),
        1,
        "the widest setting allows tiles, and the view wants some"
    );
    assert_eq!(
        expected_widths(&report, "grid_texture").len(),
        1,
        "the procedural grid is always resident"
    );
    // The preview target, at the size the harness asked for.
    assert_eq!(expected_widths(&report, "render_texture"), [FRAME.0]);
    assert!(report.expected_bytes() > 0);
}

/// The measured and computed columns are the point of the report, so they have
/// to agree.
///
/// The tolerance is loose in both directions on purpose. wgpu's counter is the
/// backend allocator's figure, which rounds every texture up to an alignment
/// and may cover objects the renderer does not know it owns, so it sits above
/// the computed total; a backend that attributes some of its textures
/// elsewhere would sit below it. What the band is tight enough to catch is the
/// thing worth catching: a surface texture that was never freed, which is an
/// order of magnitude, not a factor of two.
#[test]
fn the_measured_and_computed_texture_totals_agree() {
    let gpu = gpu();
    let harness = surface(&gpu);
    harness.settle_at(&blend_params());

    let report = harness.engine.memory_report().expect("a report");
    let expected = report.expected_bytes();
    let Ok(measured) = u64::try_from(report.counters.texture_bytes) else {
        panic!("wgpu reported negative texture memory: {report}");
    };
    println!(
        "expected {:.1} MiB, wgpu counter {:.1} MiB",
        mib(expected),
        mib(measured)
    );
    if measured == 0 {
        println!("skipping the comparison: this backend maintains no texture counter");
        return;
    }
    assert!(
        measured >= expected / 2 && measured <= expected * 2 + 16 * 1024 * 1024,
        "wgpu says {:.1} MiB of textures, the renderer expects {:.1} MiB:\n{report}",
        mib(measured),
        mib(expected)
    );
}

/// A switch to the narrowest setting, which allows no tile, frees the tile
/// array, and the computed total falls by at least what it held; the floors
/// and the mask stay.
#[test]
fn a_switch_down_leaves_no_tile_array_behind() {
    let gpu = gpu();
    let harness = surface(&gpu);
    harness.settle_at(&blend_params());
    settled(harness, "the widest setting's tiles", |r| {
        !r.wanted.is_empty()
    });

    let before = harness.engine.memory_report().expect("a report");
    let array: u64 = before
        .expected
        .iter()
        .filter(|texture| texture.label == "tile_array")
        .map(sunlit_core::memory_report::ExpectedTexture::bytes)
        .sum();
    assert!(array > 0, "the array is resident at first:\n{before}");

    harness.set_texture_resolution(2048);
    settled(harness, "no tile allowed", |r| r.wanted.is_empty());
    harness.settle();

    let after = harness.engine.memory_report().expect("a report");
    println!("{after}");
    assert!(
        expected_widths(&after, "tile_array").is_empty(),
        "the tile array survived the switch:\n{after}"
    );
    for label in ["night_floor", "water_mask"] {
        assert_eq!(expected_widths(&after, label).len(), 1, "{label}:\n{after}");
    }
    assert!(
        !expected_widths(&after, "day_floor").is_empty(),
        "the day floors stay:\n{after}"
    );
    assert!(
        but_the_day_floors(&after) + array <= but_the_day_floors(&before),
        "the computed total should fall by the array's {} MiB",
        mib(array)
    );
}

/// The computed total without the day floors, which the other months' packs
/// add to whenever they land.
pub(crate) fn but_the_day_floors(report: &sunlit_core::memory_report::MemoryReport) -> u64 {
    report
        .expected
        .iter()
        .filter(|texture| texture.label != "day_floor")
        .map(sunlit_core::memory_report::ExpectedTexture::bytes)
        .sum()
}

/// Pool slack is what the allocator holds but nothing is using, and the report
/// shows it as the difference between the two totals it prints.
#[test]
fn the_allocator_section_reports_reserved_at_least_as_large_as_allocated() {
    let gpu = gpu();
    let harness = plain(&gpu);
    harness.settle_at(&test_params());
    let report = harness.engine.memory_report().expect("a report");

    let Some(allocator) = &report.allocator else {
        println!(
            "skipping: {} has no allocator report, and the section says so",
            report.adapter
        );
        assert!(
            report.to_string().contains("no allocator report"),
            "a backend without a report must still print the section:\n{report}"
        );
        return;
    };
    assert!(
        allocator.total_reserved_bytes >= allocator.total_allocated_bytes,
        "reserved {} is below allocated {}",
        allocator.total_reserved_bytes,
        allocator.total_allocated_bytes
    );
    assert!(
        allocator.total_allocated_bytes > 0,
        "a live device holds something:\n{report}"
    );
}
