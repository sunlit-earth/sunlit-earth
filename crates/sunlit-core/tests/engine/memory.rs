//! The memory report.

use crate::groups::{FRAME, SURFACE_WIDTH, plain, surface};
use crate::harness::{gpu, test_params};
use crate::textures::{blend_params, mib};

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

    let report = harness
        .engine
        .memory_report()
        .expect("the engine should answer with a report");
    println!("{report}");

    assert_eq!(expected_widths(&report, "day_texture"), [SURFACE_WIDTH]);
    assert_eq!(expected_widths(&report, "night_texture"), [SURFACE_WIDTH]);
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

/// After a switch down, nothing of the old width is left in the report and the
/// computed total has fallen.
///
/// The widths are the fixtures' rather than the 8192 and 2048 a user picks
/// between, for the reason every other resolution test uses small fixtures: the
/// property is about the purge and the reload, and real 8K assets would make
/// this a minute long. `lowering_the_resolution_lowers_the_process_footprint`
/// is the one that measures the real pair, where they are present.
#[test]
fn a_switch_down_leaves_no_texture_at_the_old_width() {
    const NARROW: u32 = SURFACE_WIDTH / 8;

    let gpu = gpu();
    let harness = surface(&gpu);
    harness.settle_at(&blend_params());

    let before = harness.engine.memory_report().expect("a report");
    assert_eq!(expected_widths(&before, "day_texture"), [SURFACE_WIDTH]);

    harness.set_texture_resolution(NARROW);
    harness.wait_for_textures("after switching down");
    harness.settle();

    let after = harness.engine.memory_report().expect("a report");
    println!("{after}");
    assert_eq!(expected_widths(&after, "day_texture"), [NARROW]);
    assert_eq!(expected_widths(&after, "night_texture"), [NARROW]);
    assert!(
        !after
            .expected
            .iter()
            .any(|texture| texture.label.ends_with("_texture")
                && texture.width == SURFACE_WIDTH
                && texture.mip_levels > 1),
        "a texture at the old width survived the switch:\n{after}"
    );
    assert!(
        after.expected_bytes() < before.expected_bytes(),
        "the computed total should fall with the width"
    );
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
