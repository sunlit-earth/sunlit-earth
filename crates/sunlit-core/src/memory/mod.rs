//! Process-level memory tracking.
//!
//! Provides memory measurement on Windows, Linux, and macOS, a convenience
//! function that logs values as structured events, and a CSV metrics file
//! written by the watchdog timer.
//!
//! The three implementations answer the same three questions with whatever the
//! platform calls them, so `MemorySnapshot` and the CSV format are identical
//! everywhere and the soak test's assertions hold on all three:
//!
//! | | Windows | Linux | macOS |
//! |---|---|---|---|
//! | `rss_bytes` | `WorkingSetSize` | `VmRSS` | `resident_size` |
//! | `peak_rss_bytes` | `PeakWorkingSetSize` | `VmHWM` | `resident_size_peak` |
//! | `private_bytes` | `PrivateUsage` | `Private_Clean` + `Private_Dirty` | `phys_footprint` |
//!
//! The numbers are close cousins rather than the same quantity, so compare
//! them within one OS and not across. What every column does share is the
//! property the tests depend on: parking a decoded frame makes it go up.
//!
//! The metrics file exists because release builds compile out `debug!` and
//! `info!` (`release_max_level_warn`); the CSV and the budget `warn!` both
//! survive that filter.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use tracing::warn;

#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
pub use self::linux::snapshot;
#[cfg(target_os = "macos")]
pub use self::macos::snapshot;
#[cfg(windows)]
pub use self::windows::snapshot;

/// What a cold-cache launch costs in private bytes before any surface texture
/// or cloud image is resident.
///
/// The measured startup peak at the widest resolution is about 2.43 GiB in a
/// release build, of which 512 MiB is the three resident textures. The rest is
/// the two 8K JXL decodes, wgpu, the driver, and the process itself. The
/// decodes happen at every resolution, because a cold downscale cache reads
/// the full-width source whatever width it is asked for, so this part of the
/// budget does not shrink with the setting. Rounded up from about 1.93 GiB.
const COLD_START_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Slack above a cold start before the budget is crossed.
///
/// A warning that fires during normal operation is a warning nobody reads, so
/// this has to clear the measurement above on a machine that is not the one it
/// was taken on. It is also what stops the budget from being unreachable: at
/// every resolution the budget stays under twice the cold start, which
/// `the_budget_is_low_enough_to_catch_a_runaway` pins.
const BUDGET_HEADROOM_BYTES: u64 = 512 * 1024 * 1024;

/// Bytes the resident textures cost at `texture_resolution`.
///
/// Three textures at `width` by `width / 2` RGBA8: the day and night surfaces
/// and the cloud overlay, which follows the same setting. Each carries a full
/// mip chain, which is four thirds of its base level. Then the two overlays
/// with a width of their own, one of which the setting moves and one of which
/// it does not. Saturating, because the renderer takes any width as a cap and a
/// nonsense one must produce a large budget rather than a panic.
fn resident_texture_bytes(texture_resolution: u32) -> u64 {
    /// The day surface, the night surface, and the cloud overlay.
    const RESIDENT_TEXTURES: u64 = 3;

    let width = u64::from(texture_resolution);
    let base_level = width.saturating_mul(width / 2).saturating_mul(4);
    base_level
        .saturating_mul(RESIDENT_TEXTURES * 4 / 3)
        .saturating_add(MOON_TEXTURE_BYTES)
        .saturating_add(milky_way_texture_bytes(texture_resolution))
}

/// Bytes the Milky Way panorama costs at `texture_resolution`.
///
/// Its source is 4096 wide, which is wider than the narrowest cap the setting
/// offers and no wider than the other two, so the setting moves this term at
/// the narrow end and not at the wide one: 42.7 MiB with its mip chain at 8192
/// and 4096, 10.7 MiB at 2048 where the halving cache serves the downscale.
/// `PANORAMA_WIDTH` is what stops the term growing above the source's own
/// width, which is the same clause `halvings_to` applies to the pixels.
fn milky_way_texture_bytes(texture_resolution: u32) -> u64 {
    /// The width of the panorama in `textures/`.
    const PANORAMA_WIDTH: u32 = 4096;

    let width = u64::from(texture_resolution.min(PANORAMA_WIDTH));
    let base_level = width.saturating_mul(width / 2).saturating_mul(4);
    base_level.saturating_mul(4) / 3
}

/// Bytes the Moon's surface costs, at every resolution.
///
/// 1024 by 512 RGBA8 with its mip chain is 2.67 MiB on the GPU, and the decode
/// that produces it holds about the same again on the CPU while it runs: 2.67
/// plus 2.67, rounded up. A resolution switch purges and reloads this slot like
/// the others, but the file is narrower than the narrowest cap the setting
/// offers, so it is always loaded at its own width and the term does not move
/// with the setting.
const MOON_TEXTURE_BYTES: u64 = 6 * 1024 * 1024;

/// Soft budget for committed private memory. Crossing it emits a `warn!`.
///
/// A cold start plus its headroom plus whatever the chosen resolution keeps
/// resident, so the Low end of the setting is not judged against the High end's
/// footprint. At the widest resolution this is 3 GiB, the Moon's 6 MiB and the
/// panorama's 42.7 MiB.
fn private_bytes_budget(texture_resolution: u32) -> u64 {
    COLD_START_BYTES
        .saturating_add(BUDGET_HEADROOM_BYTES)
        .saturating_add(resident_texture_bytes(texture_resolution))
}

/// Size at which the metrics file is rotated, roughly 145 days of samples at
/// the watchdog cadence.
const METRICS_MAX_BYTES: u64 = 1024 * 1024;

/// Column header written when a metrics file is created.
const METRICS_HEADER: &str = "unix_ts,rss_bytes,peak_rss_bytes,private_bytes\n";

/// File name of the metrics CSV inside the metrics directory.
const METRICS_FILE_NAME: &str = "memory-metrics.csv";

/// Environment variable overriding the directory holding the metrics CSV.
const ENV_METRICS_DIR: &str = "SUNLIT_EARTH_METRICS_DIR";

/// Snapshot of process memory counters.
pub struct MemorySnapshot {
    /// Current resident set size (working set) in bytes.
    pub rss_bytes: u64,
    /// Peak RSS observed since process start, in bytes.
    pub peak_rss_bytes: u64,
    /// Memory this process does not share with any other, in bytes.
    ///
    /// Windows "Private Bytes", the sum of the private mappings on Linux, and
    /// the phys-footprint ledger on macOS. See the module docs for the exact
    /// counter per platform.
    pub private_bytes: u64,
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn snapshot() -> Option<MemorySnapshot> {
    None
}

/// Bytes as mebibytes, which is the only unit this crate reports memory in.
#[expect(
    clippy::cast_precision_loss,
    reason = "the one place a byte count becomes a float"
)]
pub(crate) fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// The same for a counter that can be negative.
#[expect(
    clippy::cast_precision_loss,
    reason = "the one place a signed byte count becomes a float"
)]
pub(crate) fn mib_signed(bytes: i64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// Log the current process memory at `debug` level with structured fields.
///
/// The `context` parameter describes the checkpoint (e.g. "after wgpu init").
///
/// The level check comes first because `debug!` compiling out does not compile
/// out the measurement behind it, and on Linux each call walks the page tables
/// through `/proc/self/smaps_rollup`. `enabled!` folds to a constant when the
/// level is compiled out (`release_max_level_warn`), so release builds drop the
/// whole body.
pub fn log_memory_usage(context: &str) {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return;
    }
    if let Some(snap) = snapshot() {
        let rss_mb = mib(snap.rss_bytes);
        let peak_rss_mb = mib(snap.peak_rss_bytes);
        let private_mb = mib(snap.private_bytes);
        tracing::debug!(
            context,
            rss_mb = format_args!("{rss_mb:.1}"),
            peak_rss_mb = format_args!("{peak_rss_mb:.1}"),
            private_mb = format_args!("{private_mb:.1}"),
            "memory usage"
        );
    }
}

/// Returns the path of the memory metrics CSV.
///
/// On Windows this resolves to `%LOCALAPPDATA%\SunlitEarth\memory-metrics.csv`.
/// `SUNLIT_EARTH_METRICS_DIR` overrides the directory so tests do not append to
/// the samples a real installation is accumulating; without it, every
/// test-spawned process pollutes the soak data. Returns `None` if the
/// platform's local data directory cannot be determined.
fn metrics_path() -> Option<PathBuf> {
    metrics_path_from(crate::env_override(ENV_METRICS_DIR).as_deref())
}

/// Resolve the metrics CSV path from an optional environment override.
fn metrics_path_from(env_dir: Option<&str>) -> Option<PathBuf> {
    match env_dir {
        Some(dir) => Some(PathBuf::from(dir).join(METRICS_FILE_NAME)),
        None => Some(crate::app_data_dir()?.join(METRICS_FILE_NAME)),
    }
}

/// Seconds since the Unix epoch, or 0 if the clock is before it.
fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Format one CSV data line for a snapshot, including the trailing newline.
fn format_metrics_line(unix_ts: u64, snap: &MemorySnapshot) -> String {
    format!(
        "{unix_ts},{},{},{}\n",
        snap.rss_bytes, snap.peak_rss_bytes, snap.private_bytes
    )
}

/// Append one sample to the metrics CSV at `path`, rotating to `.csv.old`
/// first if the file has grown past `max_bytes`.
///
/// Errors are logged and never propagated: telemetry must not take the app
/// down.
fn append_sample_to(path: &Path, snap: &MemorySnapshot, max_bytes: u64) {
    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        warn!(path = %parent.display(), error = %e, "could not create metrics directory");
        return;
    }

    if file_len(path) > max_bytes {
        let rotated = path.with_extension("csv.old");
        if let Err(e) = fs::rename(path, &rotated) {
            warn!(path = %path.display(), error = %e, "could not rotate metrics file");
        }
    }

    let needs_header = file_len(path) == 0;
    let mut file = match fs::OpenOptions::new().create(true).append(true).open(path) {
        Ok(f) => f,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "could not open metrics file");
            return;
        }
    };

    let mut contents = String::new();
    if needs_header {
        contents.push_str(METRICS_HEADER);
    }
    contents.push_str(&format_metrics_line(unix_timestamp(), snap));

    if let Err(e) = file.write_all(contents.as_bytes()) {
        warn!(path = %path.display(), error = %e, "could not write metrics sample");
    }
}

/// Size of `path` in bytes, or 0 if it cannot be read.
fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |m| m.len())
}

/// Append one sample to the metrics CSV at `path`.
fn append_metrics_sample(path: &Path, snap: &MemorySnapshot) {
    append_sample_to(path, snap, METRICS_MAX_BYTES);
}

/// Take a sample, write it to the metrics file, and warn when private memory
/// exceeds the budget for `texture_resolution`.
///
/// Called from the engine's metrics schedule (once at startup, then every ten
/// minutes) with the width the renderer is currently loading at, which is what
/// most of the budget is spent on. Does nothing on platforms without a memory
/// snapshot implementation.
pub(crate) fn record_metrics_sample(texture_resolution: u32) {
    let Some(snap) = snapshot() else {
        return;
    };

    if let Some(path) = metrics_path() {
        append_metrics_sample(&path, &snap);
    }

    let budget = private_bytes_budget(texture_resolution);
    if snap.private_bytes > budget {
        warn!(
            private_bytes = snap.private_bytes,
            budget_bytes = budget,
            texture_resolution,
            rss_bytes = snap.rss_bytes,
            "memory budget exceeded"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TEXTURE_RESOLUTIONS;
    use crate::test_support::ScratchDir;

    const MIB: u64 = 1024 * 1024;

    /// The one cold-cache startup peak anyone has measured: about 2.43 GiB of
    /// private bytes, at 8192, in a release build. Every resolution is held to
    /// this figure rather than to a smaller one derived from it; the reasoning
    /// is in `docs/testing.md`.
    const MEASURED_COLD_START_PEAK: u64 = 2488 * MIB;

    /// Build a snapshot with known values for format and rotation tests.
    fn sample_snapshot() -> MemorySnapshot {
        MemorySnapshot {
            rss_bytes: 111,
            peak_rss_bytes: 222,
            private_bytes: 333,
        }
    }

    /// A unique scratch directory for one metrics test.
    fn metrics_test_dir(name: &str) -> ScratchDir {
        ScratchDir::new(&format!("metrics_{name}"))
    }

    #[test]
    fn metrics_path_with_override_uses_that_directory() {
        let path = metrics_path_from(Some("C:/tmp/sunlit")).expect("override should resolve");
        assert_eq!(path, PathBuf::from("C:/tmp/sunlit").join(METRICS_FILE_NAME));
    }

    #[test]
    fn metrics_path_without_override_uses_app_folder() {
        if let Some(path) = metrics_path_from(None) {
            assert!(
                path.ends_with(METRICS_FILE_NAME),
                "unexpected path: {}",
                path.display()
            );
            assert!(
                path.parent().is_some_and(|p| p.ends_with("SunlitEarth")),
                "expected the app folder, got {}",
                path.display()
            );
        }
    }

    #[test]
    fn format_metrics_line_has_four_columns() {
        let line = format_metrics_line(1_700_000_000, &sample_snapshot());
        assert_eq!(line, "1700000000,111,222,333\n");
        assert_eq!(line.trim_end().split(',').count(), 4);
    }

    #[test]
    fn append_writes_header_once_then_data_lines() {
        let dir = metrics_test_dir("append");
        let path = dir.join("memory-metrics.csv");

        append_sample_to(&path, &sample_snapshot(), METRICS_MAX_BYTES);
        append_sample_to(&path, &sample_snapshot(), METRICS_MAX_BYTES);

        let contents = fs::read_to_string(&path).expect("metrics file should exist");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(
            lines.len(),
            3,
            "expected header + 2 samples, got {contents:?}"
        );
        assert_eq!(lines[0], METRICS_HEADER.trim_end());
        for line in &lines[1..] {
            assert_eq!(line.split(',').count(), 4);
            assert!(
                line.ends_with(",111,222,333"),
                "unexpected data line: {line}"
            );
        }
    }

    #[test]
    fn append_creates_missing_parent_directory() {
        let dir = metrics_test_dir("mkdir");
        let path = dir.join("nested").join("memory-metrics.csv");

        append_sample_to(&path, &sample_snapshot(), METRICS_MAX_BYTES);
        assert!(path.exists());
    }

    #[test]
    fn append_rotates_when_file_exceeds_limit() {
        let dir = metrics_test_dir("rotate");
        let path = dir.join("memory-metrics.csv");
        let rotated = dir.join("memory-metrics.csv.old");

        // Two samples with a tiny limit: the second rotates the first away.
        append_sample_to(&path, &sample_snapshot(), 8);
        let first = fs::read_to_string(&path).expect("first metrics file should exist");
        append_sample_to(&path, &sample_snapshot(), 8);

        assert!(rotated.exists(), "rotated file was not created");
        assert_eq!(
            fs::read_to_string(&rotated).expect("rotated file should be readable"),
            first
        );

        let contents = fs::read_to_string(&path).expect("fresh metrics file should exist");
        assert_eq!(
            contents.lines().count(),
            2,
            "fresh file should have header + 1 sample"
        );
        assert_eq!(contents.lines().next(), Some(METRICS_HEADER.trim_end()));
    }

    #[test]
    fn append_replaces_a_previous_rotation() {
        let dir = metrics_test_dir("rotate_twice");
        let path = dir.join("memory-metrics.csv");
        let rotated = dir.join("memory-metrics.csv.old");

        for _ in 0..3 {
            append_sample_to(&path, &sample_snapshot(), 8);
        }

        assert!(rotated.exists());
        assert_eq!(
            fs::read_dir(&dir)
                .expect("metrics dir should exist")
                .count(),
            2,
            "rotation should keep exactly one .old file"
        );
    }

    #[test]
    fn snapshot_returns_plausible_figures_on_every_supported_platform() {
        let snap = snapshot();
        if cfg!(any(windows, target_os = "linux", target_os = "macos")) {
            let snap = snap.expect("expected Some on Windows, Linux and macOS");
            let ten_gb = 10 * 1024 * MIB;
            assert!(snap.rss_bytes > 0, "RSS should be > 0");
            assert!(
                snap.rss_bytes < ten_gb,
                "RSS {} exceeds 10 GB",
                snap.rss_bytes
            );
            assert!(
                snap.peak_rss_bytes >= snap.rss_bytes,
                "peak RSS {} should be >= current RSS {}",
                snap.peak_rss_bytes,
                snap.rss_bytes
            );
            assert!(
                snap.peak_rss_bytes < ten_gb,
                "peak RSS {} exceeds 10 GB",
                snap.peak_rss_bytes
            );
            assert!(snap.private_bytes > 0, "private bytes should be > 0");
            assert!(
                snap.private_bytes < ten_gb,
                "private bytes {} exceeds 10 GB",
                snap.private_bytes
            );
        }
    }

    // --- the private-bytes budget ---

    #[test]
    fn the_budget_grows_with_the_resolution() {
        let mut widths = TEXTURE_RESOLUTIONS;
        widths.sort_unstable();
        for pair in widths.windows(2) {
            assert!(
                private_bytes_budget(pair[0]) < private_bytes_budget(pair[1]),
                "the budget at {} should be below the one at {}",
                pair[0],
                pair[1]
            );
        }
    }

    /// A cold cache at any resolution still decodes both 8K sources, so that
    /// launch is the worst normal operation gets and the budget has to clear it
    /// everywhere. The narrow end is the binding case rather than a restatement
    /// of the wide one: it gets the smallest resident allowance and has the same
    /// decode to pay for. The measured clearances are in `docs/testing.md`.
    #[test]
    fn the_budget_stays_above_a_cold_cache_first_run_at_every_resolution() {
        for width in TEXTURE_RESOLUTIONS {
            let budget = private_bytes_budget(width);
            assert!(
                budget > MEASURED_COLD_START_PEAK,
                "at {width} the budget is {} MiB and a cold start was measured at {} MiB",
                budget / MIB,
                MEASURED_COLD_START_PEAK / MIB
            );
        }
    }

    /// A budget nothing can reach reports nothing: at every resolution it stays
    /// under twice a cold start, so a process that has doubled its startup
    /// footprint is named.
    #[test]
    fn the_budget_is_low_enough_to_catch_a_runaway() {
        for width in TEXTURE_RESOLUTIONS {
            let budget = private_bytes_budget(width);
            assert!(
                budget < MEASURED_COLD_START_PEAK * 2,
                "at {width} the budget is {} MiB, twice a cold start is {} MiB",
                budget / MIB,
                MEASURED_COLD_START_PEAK * 2 / MIB
            );
        }
    }

    #[test]
    fn the_resident_half_is_the_textures_the_renderer_keeps() {
        let surfaces = |width| {
            resident_texture_bytes(width) - MOON_TEXTURE_BYTES - milky_way_texture_bytes(width)
        };
        assert_eq!(surfaces(8192), 512 * MIB);
        assert_eq!(surfaces(4096), 128 * MIB);
        assert_eq!(surfaces(2048), 32 * MIB);
    }

    #[test]
    fn the_panoramas_term_is_capped_at_the_width_of_the_file() {
        assert_eq!(milky_way_texture_bytes(4096), 4096 * 2048 * 4 * 4 / 3);
        assert_eq!(milky_way_texture_bytes(8192), milky_way_texture_bytes(4096));
        assert_eq!(
            milky_way_texture_bytes(2048),
            milky_way_texture_bytes(4096) / 4
        );
    }

    #[test]
    fn an_absurd_resolution_still_yields_a_budget() {
        assert!(private_bytes_budget(0) >= COLD_START_BYTES);
        assert!(private_bytes_budget(u32::MAX) >= COLD_START_BYTES);
    }
}
