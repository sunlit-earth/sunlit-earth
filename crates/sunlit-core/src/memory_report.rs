//! Where the process's memory actually is, expected next to measured.
//!
//! [`MemoryReport`] is four short sections: the process counters, wgpu's own
//! internal counters, the backend allocator's live allocations, and the table
//! of what the renderer believes it owns. The point is the last two side by
//! side: the day the columns disagree is the day there is a leak.
//!
//! The report is short on purpose. Everything below [`REPORT_FLOOR_BYTES`] is
//! rolled into one line, and only the [`TOP_N`] largest allocation groups are
//! listed, because a report nobody reads is telemetry rather than a tool.
//!
//! Only the section names are a contract. The numbers, their order within a
//! line, and the row layout are free to change; nothing parses this, unlike the
//! single `query-memory` line the e2e suite reads.
//!
//! Two of the four sections can be absent, and say so rather than vanishing.
//! `Device::generate_allocator_report` is implemented for D3D12 and Vulkan and
//! returns `None` everywhere else, so Metal has no allocation section; the
//! process snapshot is absent only on a platform `memory::snapshot` does not
//! cover. The counters read zero on a backend that does not maintain them,
//! which is a number rather than an absence, so that section is always printed.

use std::collections::BTreeMap;
use std::fmt;

/// Allocations and textures below this are rolled up rather than listed.
const REPORT_FLOOR_BYTES: u64 = 1024 * 1024;

/// How many allocation groups are listed before the rest are rolled up.
const TOP_N: usize = 10;

/// The three process counters, as [`crate::memory::snapshot`] reports them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessSection {
    pub rss_bytes: u64,
    pub peak_rss_bytes: u64,
    pub private_bytes: u64,
}

/// wgpu's own running totals, in bytes and objects.
///
/// Signed because [`wgpu::InternalCounter`] is: it counts up on create and down
/// on destroy, and a backend that only implements one half would go negative
/// rather than wrap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CounterSection {
    pub texture_bytes: i64,
    pub buffer_bytes: i64,
    pub allocations: i64,
}

/// Live allocations sharing one label, which is how the report names a texture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllocationGroup {
    pub label: String,
    pub count: usize,
    pub bytes: u64,
}

/// What the backend allocator holds, reduced to two totals and the top groups.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AllocatorSection {
    pub total_allocated_bytes: u64,
    pub total_reserved_bytes: u64,
    pub blocks: usize,
    /// The [`TOP_N`] largest groups of at least [`REPORT_FLOOR_BYTES`].
    pub top: Vec<AllocationGroup>,
    /// Allocations the two rules above left out.
    pub rolled_up_count: usize,
    pub rolled_up_bytes: u64,
}

/// One texture the renderer knows it owns, and what it should cost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedTexture {
    pub label: String,
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
    pub mip_levels: u32,
    pub sample_count: u32,
}

impl ExpectedTexture {
    /// Bytes this texture occupies: every mip level, every sample.
    pub fn bytes(&self) -> u64 {
        texture_bytes(
            self.width,
            self.height,
            self.format,
            self.mip_levels,
            self.sample_count,
        )
    }
}

/// Bytes a texture of this shape occupies, mip chain and samples included.
///
/// A format with no fixed texel size contributes nothing rather than a guess;
/// the two this renderer uses (`Rgba8Unorm` and `Depth32Float`) both have one.
fn texture_bytes(
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    mip_levels: u32,
    sample_count: u32,
) -> u64 {
    let Some(bytes_per_texel) = format.block_copy_size(None).map(u64::from) else {
        return 0;
    };
    let mut total = 0u64;
    for level in 0..mip_levels {
        let w = u64::from(width >> level).max(1);
        let h = u64::from(height >> level).max(1);
        total += w * h * bytes_per_texel;
    }
    total * u64::from(sample_count.max(1))
}

/// The whole report.
pub struct MemoryReport {
    /// The adapter slug from `wgpu_init::adapter_key`, which is what says
    /// whether the GPU bytes below overlap the process's private bytes: they do
    /// on a software rasterizer, and mostly do not on a real GPU.
    pub adapter: String,
    pub process: Option<ProcessSection>,
    pub counters: CounterSection,
    pub allocator: Option<AllocatorSection>,
    /// Every texture the renderer owns, including ones under the floor. The
    /// total is over all of them; only the ones at or above the floor are
    /// listed.
    pub expected: Vec<ExpectedTexture>,
}

impl MemoryReport {
    /// Sum of the expected table, including rows too small to be listed.
    pub fn expected_bytes(&self) -> u64 {
        self.expected.iter().map(ExpectedTexture::bytes).sum()
    }
}

/// Assemble a report from a device, the adapter that opened it, and what the
/// caller believes it owns.
///
/// Both wgpu queries are cheap but not free: the allocator report walks every
/// live allocation behind the allocator's own lock. This runs on demand, on the
/// thread that owns the device, between frames.
pub(crate) fn collect(
    device: &wgpu::Device,
    adapter: &str,
    expected: Vec<ExpectedTexture>,
) -> MemoryReport {
    let counters = device.get_internal_counters();
    let counter = |value: isize| i64::try_from(value).unwrap_or(i64::MAX);
    MemoryReport {
        adapter: adapter.to_owned(),
        process: crate::memory::snapshot().map(|snap| ProcessSection {
            rss_bytes: snap.rss_bytes,
            peak_rss_bytes: snap.peak_rss_bytes,
            private_bytes: snap.private_bytes,
        }),
        counters: CounterSection {
            texture_bytes: counter(counters.hal.texture_memory.read()),
            buffer_bytes: counter(counters.hal.buffer_memory.read()),
            allocations: counter(counters.hal.memory_allocations.read()),
        },
        allocator: device
            .generate_allocator_report()
            .as_ref()
            .map(summarize_allocator),
        expected,
    }
}

/// Reduce a wgpu allocator report to two totals, the top groups, and a rollup.
///
/// The grouping is [`group_allocations`]; the three figures replaced here are
/// the allocator's own answer rather than a sum of what it happened to list,
/// and the difference between the two totals is the pool slack.
fn summarize_allocator(report: &wgpu::AllocatorReport) -> AllocatorSection {
    let mut section = group_allocations(
        report
            .allocations
            .iter()
            .map(|allocation| (allocation.name.as_str(), allocation.size)),
    );
    section.total_allocated_bytes = report.total_allocated_bytes;
    section.total_reserved_bytes = report.total_reserved_bytes;
    section.blocks = report.blocks.len();
    section
}

/// Group live allocations by label, keep the top few, roll the rest up.
///
/// Aggregation is by label, so the several allocations one mipmapped texture
/// might be split into read as one row naming that texture. An allocation wgpu
/// did not name is grouped under a single placeholder rather than being
/// dropped: unnamed bytes are still bytes, and a report that hid them would
/// understate the total it prints one line above.
///
/// Takes pairs rather than wgpu's own `AllocationReport`, which wgpu does not
/// re-export and a test therefore cannot build. Both totals come out as the sum
/// of what was passed in and `blocks` as zero, since nothing here knows about
/// the pool behind the allocations; [`summarize_allocator`] replaces all three.
fn group_allocations<'a>(
    allocations: impl IntoIterator<Item = (&'a str, u64)>,
) -> AllocatorSection {
    let mut groups: BTreeMap<&str, AllocationGroup> = BTreeMap::new();
    for (name, size) in allocations {
        let label = if name.is_empty() { "(unnamed)" } else { name };
        let entry = groups.entry(label).or_insert_with(|| AllocationGroup {
            label: label.to_owned(),
            count: 0,
            bytes: 0,
        });
        entry.count += 1;
        entry.bytes += size;
    }

    let mut groups: Vec<AllocationGroup> = groups.into_values().collect();
    // Largest first, and by label where two groups tie, so the same set of
    // allocations always produces the same report.
    groups.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.label.cmp(&b.label)));

    let listed = groups
        .iter()
        .take(TOP_N)
        .take_while(|group| group.bytes >= REPORT_FLOOR_BYTES)
        .count();
    let rolled_up = &groups[listed..];
    let total: u64 = groups.iter().map(|group| group.bytes).sum();

    AllocatorSection {
        total_allocated_bytes: total,
        total_reserved_bytes: total,
        blocks: 0,
        rolled_up_count: rolled_up.iter().map(|group| group.count).sum(),
        rolled_up_bytes: rolled_up.iter().map(|group| group.bytes).sum(),
        top: groups.iter().take(listed).cloned().collect(),
    }
}

/// Bytes as mebibytes, which is the only unit the report speaks.
#[allow(clippy::cast_precision_loss)]
fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// The same for a counter that can be negative.
#[allow(clippy::cast_precision_loss)]
fn mib_signed(bytes: i64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

impl fmt::Display for MemoryReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "memory report (adapter {})", self.adapter)?;

        match &self.process {
            Some(p) => writeln!(
                f,
                "process: rss {:.1} MiB, peak rss {:.1} MiB, private {:.1} MiB",
                mib(p.rss_bytes),
                mib(p.peak_rss_bytes),
                mib(p.private_bytes)
            )?,
            None => writeln!(f, "process: not measurable on this platform")?,
        }

        writeln!(
            f,
            "wgpu counters: textures {:.1} MiB, buffers {:.1} MiB, {} allocations",
            mib_signed(self.counters.texture_bytes),
            mib_signed(self.counters.buffer_bytes),
            self.counters.allocations
        )?;

        match &self.allocator {
            Some(a) => {
                writeln!(
                    f,
                    "gpu allocations: {:.1} MiB in use, {:.1} MiB reserved, {} blocks",
                    mib(a.total_allocated_bytes),
                    mib(a.total_reserved_bytes),
                    a.blocks
                )?;
                for group in &a.top {
                    writeln!(
                        f,
                        "  {} x{}: {:.1} MiB",
                        group.label,
                        group.count,
                        mib(group.bytes)
                    )?;
                }
                writeln!(
                    f,
                    "  rest x{}: {:.1} MiB",
                    a.rolled_up_count,
                    mib(a.rolled_up_bytes)
                )?;
            }
            None => writeln!(
                f,
                "gpu allocations: no allocator report on this backend ({})",
                self.adapter
            )?,
        }

        // The measured half of the comparison is the point of the section, so
        // it goes on the same line; a backend with no texture counter reports
        // zero, which as a comparison would read as a disagreement rather than
        // an absence, so it is left off instead.
        let against = if self.counters.texture_bytes == 0 {
            String::new()
        } else {
            format!(
                ", against {:.1} MiB measured",
                mib_signed(self.counters.texture_bytes)
            )
        };
        writeln!(
            f,
            "expected: {:.1} MiB in {} textures{against}; \
             rows under {:.0} MiB are omitted",
            mib(self.expected_bytes()),
            self.expected.len(),
            mib(REPORT_FLOOR_BYTES)
        )?;
        for texture in &self.expected {
            let bytes = texture.bytes();
            if bytes < REPORT_FLOOR_BYTES {
                continue;
            }
            writeln!(
                f,
                "  {} {}x{} {:?} mips {} samples {}: {:.1} MiB",
                texture.label,
                texture.width,
                texture.height,
                texture.format,
                texture.mip_levels,
                texture.sample_count,
                mib(bytes)
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = REPORT_FLOOR_BYTES;

    /// `group_allocations` over owned pairs, which is what a test has.
    fn grouped(allocations: &[(String, u64)]) -> AllocatorSection {
        group_allocations(
            allocations
                .iter()
                .map(|(name, size)| (name.as_str(), *size)),
        )
    }

    fn pairs(allocations: &[(&str, u64)]) -> Vec<(String, u64)> {
        allocations
            .iter()
            .map(|(name, size)| ((*name).to_owned(), *size))
            .collect()
    }

    #[test]
    fn allocations_sharing_a_label_become_one_row() {
        let section = grouped(&pairs(&[
            ("day_texture", 4 * MIB),
            ("day_texture", 2 * MIB),
        ]));
        assert_eq!(section.top.len(), 1);
        assert_eq!(section.top[0].count, 2);
        assert_eq!(section.top[0].bytes, 6 * MIB);
        assert_eq!(section.rolled_up_count, 0);
    }

    #[test]
    fn rows_are_ordered_largest_first() {
        let section = grouped(&pairs(&[
            ("small", 2 * MIB),
            ("large", 9 * MIB),
            ("middle", 5 * MIB),
        ]));
        let labels: Vec<&str> = section.top.iter().map(|g| g.label.as_str()).collect();
        assert_eq!(labels, ["large", "middle", "small"]);
    }

    #[test]
    fn at_most_ten_rows_are_listed_and_the_rest_are_rolled_up() {
        let allocations: Vec<(String, u64)> = (0..40)
            .map(|i| (format!("texture_{i:02}"), (40 - i) * MIB))
            .collect();
        let section = grouped(&allocations);

        assert_eq!(section.top.len(), TOP_N);
        assert_eq!(section.rolled_up_count, 30);
        // The listed rows are the ten largest: 40 down to 31 MiB.
        let listed: u64 = (31..=40).map(|n| n * MIB).sum();
        assert_eq!(section.top.iter().map(|g| g.bytes).sum::<u64>(), listed);
        assert_eq!(
            section.rolled_up_bytes,
            section.total_allocated_bytes - listed
        );
    }

    #[test]
    fn groups_under_the_floor_are_rolled_up_even_with_room_to_spare() {
        let section = grouped(&pairs(&[
            ("big", 8 * MIB),
            ("tiny", 1024),
            ("also_tiny", 2048),
        ]));
        assert_eq!(
            section.top.len(),
            1,
            "only the row over the floor is listed"
        );
        assert_eq!(section.rolled_up_count, 2);
        assert_eq!(section.rolled_up_bytes, 3072);
    }

    #[test]
    fn a_group_exactly_at_the_floor_is_listed() {
        let section = grouped(&pairs(&[("edge", MIB)]));
        assert_eq!(section.top.len(), 1);
        assert_eq!(section.rolled_up_count, 0);
    }

    #[test]
    fn unnamed_allocations_are_counted_rather_than_dropped() {
        let section = grouped(&pairs(&[("", 4 * MIB)]));
        assert_eq!(section.top.len(), 1);
        assert_eq!(section.top[0].label, "(unnamed)");
        assert_eq!(section.top[0].bytes, 4 * MIB);
    }

    #[test]
    fn listed_plus_rolled_up_is_everything() {
        let allocations: Vec<(String, u64)> =
            (0..25u64).map(|i| (format!("t{i}"), i * 700_000)).collect();
        let section = grouped(&allocations);
        let listed: u64 = section.top.iter().map(|g| g.bytes).sum();
        assert_eq!(
            listed + section.rolled_up_bytes,
            allocations.iter().map(|(_, size)| size).sum::<u64>()
        );
    }

    // -----------------------------------------------------------------------
    // texture_bytes
    // -----------------------------------------------------------------------

    #[test]
    fn a_single_level_texture_is_its_pixels() {
        assert_eq!(
            texture_bytes(1920, 1080, wgpu::TextureFormat::Rgba8Unorm, 1, 1),
            1920 * 1080 * 4
        );
    }

    #[test]
    fn a_full_mip_chain_costs_four_thirds_of_the_base_level() {
        let base = 8192u64 * 4096 * 4;
        let mips = 8192u32.ilog2() + 1;
        let total = texture_bytes(8192, 4096, wgpu::TextureFormat::Rgba8Unorm, mips, 1);
        // Exact within the rounding of the last few 1x1 levels.
        assert!(
            total > base * 4 / 3 && total < base * 4 / 3 + 64,
            "{total} is not four thirds of {base}"
        );
    }

    #[test]
    fn depth_and_multisampling_are_counted() {
        assert_eq!(
            texture_bytes(640, 480, wgpu::TextureFormat::Depth32Float, 1, 1),
            640 * 480 * 4
        );
        assert_eq!(
            texture_bytes(640, 480, wgpu::TextureFormat::Rgba8Unorm, 1, 4),
            640 * 480 * 4 * 4
        );
    }

    #[test]
    fn mip_levels_past_a_dimension_clamp_to_one() {
        // 4x1 with three levels: 4x1, 2x1, 1x1.
        assert_eq!(
            texture_bytes(4, 1, wgpu::TextureFormat::Rgba8Unorm, 3, 1),
            (4 + 2 + 1) * 4
        );
    }

    // -----------------------------------------------------------------------
    // The rendered report
    // -----------------------------------------------------------------------

    fn expected(label: &str, width: u32, height: u32) -> ExpectedTexture {
        ExpectedTexture {
            label: label.to_owned(),
            width,
            height,
            format: wgpu::TextureFormat::Rgba8Unorm,
            mip_levels: 1,
            sample_count: 1,
        }
    }

    /// Mebibytes as a counter value.
    fn counter_mib(count: i64) -> i64 {
        count * 1024 * 1024
    }

    fn fabricated() -> MemoryReport {
        MemoryReport {
            adapter: "warp".to_owned(),
            process: Some(ProcessSection {
                rss_bytes: 100 * MIB,
                peak_rss_bytes: 200 * MIB,
                private_bytes: 300 * MIB,
            }),
            counters: CounterSection {
                texture_bytes: counter_mib(90),
                buffer_bytes: counter_mib(1),
                allocations: 7,
            },
            allocator: Some(grouped(&pairs(&[
                ("day_texture", 40 * MIB),
                ("night_texture", 40 * MIB),
                ("readback", 4096),
            ]))),
            expected: vec![
                expected("day_texture", 2048, 1024),
                expected("night_texture", 2048, 1024),
                expected("dummy_1x1", 1, 1),
            ],
        }
    }

    /// Four sections and nothing else: one header line each, plus the rows the
    /// allocation and expected sections are made of.
    #[test]
    fn the_report_has_exactly_four_sections() {
        let text = fabricated().to_string();
        let headers: Vec<&str> = text
            .lines()
            .filter(|line| !line.starts_with(' ') && !line.starts_with("memory report"))
            .collect();
        assert_eq!(headers.len(), 4, "unexpected sections in:\n{text}");
        assert!(headers[0].starts_with("process:"));
        assert!(headers[1].starts_with("wgpu counters:"));
        assert!(headers[2].starts_with("gpu allocations:"));
        assert!(headers[3].starts_with("expected:"));
    }

    #[test]
    fn the_rollup_line_is_always_there() {
        let text = fabricated().to_string();
        assert!(
            text.contains("  rest x1: 0.0 MiB"),
            "missing the rollup line in:\n{text}"
        );
    }

    #[test]
    fn a_texture_under_the_floor_is_counted_but_not_listed() {
        let report = fabricated();
        assert_eq!(report.expected.len(), 3);
        let text = report.to_string();
        assert!(!text.contains("dummy_1x1"), "listed a 1x1 texture:\n{text}");
        assert!(
            text.contains("in 3 textures"),
            "the total should count all three:\n{text}"
        );
    }

    #[test]
    fn a_backend_without_an_allocator_report_still_prints_the_section() {
        let mut report = fabricated();
        report.adapter = "metal".to_owned();
        report.allocator = None;
        let text = report.to_string();
        assert!(
            text.contains("gpu allocations: no allocator report on this backend (metal)"),
            "missing the degraded section in:\n{text}"
        );
        assert_eq!(
            text.lines()
                .filter(|line| !line.starts_with(' ') && !line.starts_with("memory report"))
                .count(),
            4
        );
    }

    #[test]
    fn a_platform_without_a_process_snapshot_still_prints_the_section() {
        let mut report = fabricated();
        report.process = None;
        assert!(
            report
                .to_string()
                .contains("process: not measurable on this platform")
        );
    }

    #[test]
    fn a_backend_without_a_texture_counter_makes_no_comparison() {
        let mut report = fabricated();
        assert!(report.to_string().contains("against 90.0 MiB measured"));
        report.counters.texture_bytes = 0;
        let text = report.to_string();
        assert!(
            !text.contains("measured"),
            "unexpected comparison in:\n{text}"
        );
        assert!(text.contains("expected: "), "the section is still there");
    }
}
