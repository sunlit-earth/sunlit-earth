//! Process-level memory tracking.
//!
//! Provides memory measurement on Windows via `GetProcessMemoryInfo`,
//! a convenience function that logs values as structured events, and a CSV
//! metrics file written by the watchdog timer.
//!
//! The metrics file exists because release builds compile out `debug!` and
//! `info!` (`release_max_level_warn`), so the tray-mode memory leak produced no
//! telemetry at all across ten days of uptime. The CSV and the budget `warn!`
//! both survive that filter.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use tracing::warn;

/// Soft budget for committed private memory. Crossing it emits a `warn!`.
///
/// 3 GiB, not 2: decoding the two 8K JXL textures at the High tier pushes
/// private bytes to about 2.43 GiB transiently at startup (observed in a
/// release build), so a 2 GiB budget warned about normal operation. A warning
/// that fires every launch is a warning nobody reads.
///
/// This is a single number for every tier, which is the crude version. A
/// budget derived from the tier (the Low tier never goes near this) belongs
/// with the texture-tier work in retrospective section 9.
pub const PRIVATE_BYTES_BUDGET: u64 = 3 * 1024 * 1024 * 1024;

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
    /// Committed private memory in bytes (what Task Manager shows as "Private Bytes").
    pub private_bytes: u64,
}

/// Return a snapshot of the current process's memory counters.
///
/// On Windows this calls `GetProcessMemoryInfo` with `PROCESS_MEMORY_COUNTERS_EX`.
/// On other platforms, returns `None`.
#[cfg(windows)]
#[allow(clippy::cast_possible_truncation)]
pub fn snapshot() -> Option<MemorySnapshot> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: 0,
        PageFaultCount: 0,
        PeakWorkingSetSize: 0,
        WorkingSetSize: 0,
        QuotaPeakPagedPoolUsage: 0,
        QuotaPagedPoolUsage: 0,
        QuotaPeakNonPagedPoolUsage: 0,
        QuotaNonPagedPoolUsage: 0,
        PagefileUsage: 0,
        PeakPagefileUsage: 0,
        PrivateUsage: 0,
    };
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;

    // SAFETY: GetCurrentProcess returns a pseudo-handle (-1) that is always
    // valid for the calling process and does not need to be closed.
    // GetProcessMemoryInfo reads process memory counters into the provided
    // struct. PROCESS_MEMORY_COUNTERS_EX is a superset of PROCESS_MEMORY_COUNTERS
    // with the same initial layout, so casting the pointer is safe when cb is
    // set to the EX size.
    #[allow(unsafe_code)]
    let success = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            std::ptr::addr_of_mut!(counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        )
    };

    if success != 0 {
        Some(MemorySnapshot {
            rss_bytes: counters.WorkingSetSize as u64,
            peak_rss_bytes: counters.PeakWorkingSetSize as u64,
            private_bytes: counters.PrivateUsage as u64,
        })
    } else {
        None
    }
}

#[cfg(not(windows))]
pub fn snapshot() -> Option<MemorySnapshot> {
    None
}

/// Log the current process memory at `debug` level with structured fields.
///
/// The `context` parameter describes the checkpoint (e.g. "after wgpu init").
#[allow(clippy::cast_precision_loss)]
pub fn log_memory_usage(context: &str) {
    if let Some(snap) = snapshot() {
        let rss_mb = snap.rss_bytes as f64 / (1024.0 * 1024.0);
        let peak_rss_mb = snap.peak_rss_bytes as f64 / (1024.0 * 1024.0);
        let private_mb = snap.private_bytes as f64 / (1024.0 * 1024.0);
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
pub fn metrics_path() -> Option<PathBuf> {
    metrics_path_from(crate::env_override(ENV_METRICS_DIR).as_deref())
}

/// Resolve the metrics CSV path from an optional environment override.
fn metrics_path_from(env_dir: Option<&str>) -> Option<PathBuf> {
    match env_dir {
        Some(dir) => Some(PathBuf::from(dir).join(METRICS_FILE_NAME)),
        None => Some(
            dirs::data_local_dir()?
                .join("SunlitEarth")
                .join(METRICS_FILE_NAME),
        ),
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
pub fn append_metrics_sample(path: &Path, snap: &MemorySnapshot) {
    append_sample_to(path, snap, METRICS_MAX_BYTES);
}

/// Take a sample, write it to the metrics file, and warn when private memory
/// exceeds `PRIVATE_BYTES_BUDGET`.
///
/// Called from the engine's metrics schedule (once at startup, then every ten
/// minutes). Does nothing on platforms without a memory snapshot
/// implementation.
pub fn record_metrics_sample() {
    let Some(snap) = snapshot() else {
        return;
    };

    if let Some(path) = metrics_path() {
        append_metrics_sample(&path, &snap);
    }

    if snap.private_bytes > PRIVATE_BYTES_BUDGET {
        warn!(
            private_bytes = snap.private_bytes,
            budget_bytes = PRIVATE_BYTES_BUDGET,
            rss_bytes = snap.rss_bytes,
            "memory budget exceeded"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a snapshot with known values for format and rotation tests.
    fn sample_snapshot() -> MemorySnapshot {
        MemorySnapshot {
            rss_bytes: 111,
            peak_rss_bytes: 222,
            private_bytes: 333,
        }
    }

    /// A unique scratch directory for one metrics test.
    fn metrics_test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sunlit_earth_test_metrics_{name}"));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn metrics_path_with_override_uses_that_directory() {
        let path = metrics_path_from(Some("C:/tmp/sunlit")).expect("override should resolve");
        assert_eq!(path, PathBuf::from("C:/tmp/sunlit").join(METRICS_FILE_NAME));
    }

    #[test]
    fn metrics_path_without_override_uses_app_folder() {
        if let Some(path) = metrics_path_from(None) {
            assert!(path.ends_with(METRICS_FILE_NAME), "unexpected path: {}", path.display());
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
        assert_eq!(lines.len(), 3, "expected header + 2 samples, got {contents:?}");
        assert_eq!(lines[0], METRICS_HEADER.trim_end());
        for line in &lines[1..] {
            assert_eq!(line.split(',').count(), 4);
            assert!(line.ends_with(",111,222,333"), "unexpected data line: {line}");
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_creates_missing_parent_directory() {
        let dir = metrics_test_dir("mkdir");
        let path = dir.join("nested").join("memory-metrics.csv");

        append_sample_to(&path, &sample_snapshot(), METRICS_MAX_BYTES);
        assert!(path.exists());

        let _ = fs::remove_dir_all(&dir);
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
        assert_eq!(contents.lines().count(), 2, "fresh file should have header + 1 sample");
        assert_eq!(contents.lines().next(), Some(METRICS_HEADER.trim_end()));

        let _ = fs::remove_dir_all(&dir);
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
            fs::read_dir(&dir).expect("metrics dir should exist").count(),
            2,
            "rotation should keep exactly one .old file"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_returns_some_on_windows() {
        let snap = snapshot();
        if cfg!(windows) {
            let snap = snap.expect("expected Some on Windows");
            assert!(snap.rss_bytes > 0, "RSS should be > 0");
            assert!(
                snap.peak_rss_bytes >= snap.rss_bytes,
                "peak RSS should be >= current RSS"
            );
            assert!(snap.private_bytes > 0, "private bytes should be > 0");
        }
    }

    #[test]
    fn snapshot_values_are_reasonable() {
        if let Some(snap) = snapshot() {
            let ten_gb = 10 * 1024 * 1024 * 1024u64;
            assert!(snap.rss_bytes < ten_gb, "RSS {} exceeds 10 GB", snap.rss_bytes);
            assert!(snap.peak_rss_bytes < ten_gb, "peak RSS {} exceeds 10 GB", snap.peak_rss_bytes);
            assert!(snap.private_bytes < ten_gb, "private bytes {} exceeds 10 GB", snap.private_bytes);
        }
    }
}
