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
//! The Linux private row is what `/proc/self/smaps_rollup` reports, which
//! arrived in Linux 4.14 and can be absent under a hardened kernel. Where it
//! is, `VmRSS` stands in (an upper bound, since it also counts shared pages)
//! and the fallback says so in the log once per process.
//!
//! The numbers are close cousins rather than the same quantity, so compare
//! them within one OS and not across. What every column does share is the
//! property the tests depend on: parking a decoded frame makes it go up.
//!
//! The metrics file exists because release builds compile out `debug!` and
//! `info!` (`release_max_level_warn`), so the tray-mode memory leak produced no
//! telemetry at all across ten days of uptime. The CSV and the budget `warn!`
//! both survive that filter.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use tracing::warn;

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
/// mip chain, which is four thirds of its base level. Saturating, because the
/// renderer takes any width as a cap and a nonsense one must produce a large
/// budget rather than a panic.
fn resident_texture_bytes(texture_resolution: u32) -> u64 {
    /// The day surface, the night surface, and the cloud overlay.
    const RESIDENT_TEXTURES: u64 = 3;

    let width = u64::from(texture_resolution);
    let base_level = width.saturating_mul(width / 2).saturating_mul(4);
    base_level.saturating_mul(RESIDENT_TEXTURES * 4 / 3)
}

/// Soft budget for committed private memory. Crossing it emits a `warn!`.
///
/// A cold start plus its headroom plus whatever the chosen resolution keeps
/// resident, so the Low end of the setting is not judged against the High end's
/// footprint. At the widest resolution this is 3 GiB, which is the single
/// number it replaces.
pub fn private_bytes_budget(texture_resolution: u32) -> u64 {
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

/// Return a snapshot of the current process's memory counters.
///
/// Windows calls `GetProcessMemoryInfo` with `PROCESS_MEMORY_COUNTERS_EX`,
/// Linux reads `/proc/self`, macOS asks the Mach kernel. Returns `None` on any
/// other platform, and on the three supported ones only if the OS refuses to
/// answer.
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

/// Linux: `/proc/self/status` for the resident sizes, `/proc/self/smaps_rollup`
/// for the private total. Both are plain text, so this needs no `unsafe` and no
/// binding crate.
#[cfg(target_os = "linux")]
pub fn snapshot() -> Option<MemorySnapshot> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let rss_bytes = parse_status_bytes(&status, "VmRSS")?;
    // VmHWM is the field most likely to be the one a restricted or unusual
    // /proc omits while still reporting VmRSS, and taking the whole snapshot
    // down over it would silence every assertion that reads the other two
    // fields. The current RSS is a true lower bound on the peak, so degrading
    // to it keeps the peak column honest in the direction that matters.
    let peak_rss_bytes = parse_status_bytes(&status, "VmHWM").unwrap_or(rss_bytes);

    // smaps_rollup arrived in Linux 4.14. Where it is missing (or unreadable
    // under a hardened kernel) RSS is the honest stand-in: it is an upper bound
    // on the private total, so the soak test's growth assertion stays valid,
    // it just also counts shared pages. Say so once, because a private column
    // that is quietly measuring something else is worth knowing about when
    // reading the metrics CSV afterwards.
    let private_bytes = fs::read_to_string("/proc/self/smaps_rollup")
        .ok()
        .and_then(|rollup| parse_private_bytes(&rollup))
        .unwrap_or_else(|| {
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                warn!(
                    "/proc/self/smaps_rollup is unreadable; reporting VmRSS as private \
                     bytes, which also counts pages shared with other processes"
                );
            });
            rss_bytes
        });

    Some(MemorySnapshot {
        rss_bytes,
        peak_rss_bytes,
        private_bytes,
    })
}

/// Read one `Key:   1234 kB` line out of a `/proc` file and return it in bytes.
///
/// The unit is checked rather than assumed. Every field this reads is
/// documented in kibibytes, so the check should never fire; if a kernel ever
/// reported one of them in anything else, the alternative to failing here is
/// silently multiplying it by 1024.
///
/// Compiled on Linux and in every test build, so the parsing is covered by the
/// unit tests on the development machine rather than only on the Linux runner.
#[cfg(any(target_os = "linux", test))]
fn parse_status_bytes(text: &str, key: &str) -> Option<u64> {
    for line in text.lines() {
        let Some(rest) = line.strip_prefix(key) else {
            continue;
        };
        // `VmRSS` must not match a hypothetical `VmRSSFoo`.
        let Some(rest) = rest.strip_prefix(':') else {
            continue;
        };
        let mut fields = rest.split_whitespace();
        let value: u64 = fields.next()?.parse().ok()?;
        if fields.next()? != "kB" {
            return None;
        }
        return value.checked_mul(1024);
    }
    None
}

/// Sum the private mappings reported by `/proc/self/smaps_rollup`, in bytes.
///
/// Clean plus dirty, which is what the rollup offers as "not shared with
/// anyone else". Swapped-out private pages are not included; a runner that is
/// swapping has bigger problems than this counter.
#[cfg(any(target_os = "linux", test))]
fn parse_private_bytes(rollup: &str) -> Option<u64> {
    let clean = parse_status_bytes(rollup, "Private_Clean")?;
    let dirty = parse_status_bytes(rollup, "Private_Dirty")?;
    clean.checked_add(dirty)
}

/// macOS: `task_info(TASK_VM_INFO)`.
///
/// `phys_footprint` is the counter Activity Monitor shows as "Memory" and the
/// one the jetsam limits are enforced against, which makes it the closest
/// analogue of Windows private bytes: it is what this process is charged for
/// and excludes pages shared with other processes.
#[cfg(target_os = "macos")]
pub fn snapshot() -> Option<MemorySnapshot> {
    use std::mem::offset_of;

    use mach2::kern_return::KERN_SUCCESS;
    use mach2::message::mach_msg_type_number_t;
    use mach2::task::task_info;
    use mach2::task_info::{TASK_VM_INFO, task_vm_info};
    use mach2::traps::mach_task_self;
    use mach2::vm_types::{integer_t, natural_t};

    let mut info = task_vm_info::default();
    // The kernel fills as many revisions of the struct as it knows and writes
    // back how many it filled; asking for the whole of the binding crate's
    // (possibly newer) struct is how the SDK's own TASK_VM_INFO_COUNT is
    // defined, and `phys_footprint` has been in revision 1 since 10.11.
    let mut count =
        mach_msg_type_number_t::try_from(size_of::<task_vm_info>() / size_of::<natural_t>())
            .ok()?;

    // SAFETY: `mach_task_self()` returns the send right to this process's own
    // task port, which is always valid and needs no deallocation. `task_info`
    // writes at most `count` `integer_t`s into the buffer it is given; the
    // buffer is a live local `task_vm_info` and `count` is derived from that
    // same type's size, so the kernel cannot write past it. `info` is fully
    // initialized before the call, so any field the kernel leaves alone reads
    // back as zero rather than as garbage.
    #[allow(unsafe_code)]
    let result = unsafe {
        task_info(
            mach_task_self(),
            TASK_VM_INFO,
            (&raw mut info).cast::<integer_t>(),
            &raw mut count,
        )
    };

    if result != KERN_SUCCESS {
        return None;
    }

    // The kernel writes back how much it filled, and a kernel that filled less
    // than the fields read below would leave them reading zero out of the
    // zeroed struct: a plausible-looking number rather than an error.
    // `phys_footprint` has been in revision 1 since 10.11, so this should never
    // fire; the point is what happens if it ever does. The offsets are taken
    // per field rather than assuming `phys_footprint` sits last of the three,
    // since that ordering is exactly the sort of assumption this guards.
    let filled_bytes = usize::try_from(count).ok()? * size_of::<natural_t>();
    let needed_bytes = offset_of!(task_vm_info, resident_size)
        .max(offset_of!(task_vm_info, resident_size_peak))
        .max(offset_of!(task_vm_info, phys_footprint))
        + size_of::<u64>();
    if filled_bytes < needed_bytes {
        return None;
    }

    // `task_vm_info` is `repr(packed(4))`, so these are field reads by value
    // rather than references into the struct.
    Some(MemorySnapshot {
        rss_bytes: info.resident_size,
        peak_rss_bytes: info.resident_size_peak,
        private_bytes: info.phys_footprint,
    })
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn snapshot() -> Option<MemorySnapshot> {
    None
}

/// Log the current process memory at `debug` level with structured fields.
///
/// The `context` parameter describes the checkpoint (e.g. "after wgpu init").
///
/// The level check comes first because `debug!` compiling out does not compile
/// out the measurement behind it. This is called from about seventeen places,
/// three of them per wallpaper export and one per cloud decode, and on Linux
/// each call walks the page tables through `/proc/self/smaps_rollup`.
/// `enabled!` folds to a constant when the level is compiled out
/// (`release_max_level_warn`), so release builds drop the whole body.
#[allow(clippy::cast_precision_loss)]
pub fn log_memory_usage(context: &str) {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return;
    }
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
/// exceeds the budget for `texture_resolution`.
///
/// Called from the engine's metrics schedule (once at startup, then every ten
/// minutes) with the width the renderer is currently loading at, which is what
/// most of the budget is spent on. Does nothing on platforms without a memory
/// snapshot implementation.
pub fn record_metrics_sample(texture_resolution: u32) {
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

    const MIB: u64 = 1024 * 1024;

    /// The one cold-cache startup peak anyone has measured: about 2.43 GiB of
    /// private bytes, at 8192, in a release build.
    ///
    /// Every resolution is held to this figure rather than to a smaller one
    /// derived from it, and that is the point. The peak is dominated by the two
    /// 8K JXL decodes, which a cold downscale cache performs whatever width it
    /// was asked for; how much of the resident saving at a lower width also
    /// shows up in the peak is exactly what nobody has measured. Deriving a
    /// per-resolution peak from the budget's own decomposition would make the
    /// two move together and assert nothing, and it would let the budget rest
    /// on a saving that may not be there.
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
        assert_eq!(
            contents.lines().count(),
            2,
            "fresh file should have header + 1 sample"
        );
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
            fs::read_dir(&dir)
                .expect("metrics dir should exist")
                .count(),
            2,
            "rotation should keep exactly one .old file"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// Every platform the project builds for must be able to measure itself.
    ///
    /// This used to be Windows-only, and everything that asserts on memory
    /// (the soak test above all) quietly became a no-op elsewhere. Making it
    /// an assertion on all three supported platforms is what stops a broken
    /// per-OS snapshot from looking like a passing test suite.
    #[test]
    fn snapshot_returns_some_on_every_supported_platform() {
        let snap = snapshot();
        if cfg!(any(windows, target_os = "linux", target_os = "macos")) {
            let snap = snap.expect("expected Some on Windows, Linux and macOS");
            assert!(snap.rss_bytes > 0, "RSS should be > 0");
            assert!(
                snap.peak_rss_bytes >= snap.rss_bytes,
                "peak RSS {} should be >= current RSS {}",
                snap.peak_rss_bytes,
                snap.rss_bytes
            );
            assert!(snap.private_bytes > 0, "private bytes should be > 0");
        }
    }

    #[test]
    fn status_parser_reads_kibibytes_as_bytes() {
        let status = "Name:\tsunlit-earth\nVmHWM:\t  204800 kB\nVmRSS:\t   102400 kB\n";
        assert_eq!(parse_status_bytes(status, "VmRSS"), Some(102_400 * 1024));
        assert_eq!(parse_status_bytes(status, "VmHWM"), Some(204_800 * 1024));
    }

    #[test]
    fn status_parser_returns_none_for_a_missing_key() {
        let status = "VmRSS:\t 100 kB\n";
        assert_eq!(parse_status_bytes(status, "VmSwap"), None);
    }

    /// `VmRSS` must not be satisfied by a longer key that starts the same way.
    #[test]
    fn status_parser_requires_the_whole_key() {
        let status = "VmRSSExtra:\t 999 kB\nVmRSS:\t 100 kB\n";
        assert_eq!(parse_status_bytes(status, "VmRSS"), Some(100 * 1024));
    }

    /// The scale factor is only correct if the value really is in kibibytes.
    #[test]
    fn status_parser_requires_the_kilobyte_unit() {
        assert_eq!(parse_status_bytes("Threads:\t 8\n", "Threads"), None);
        assert_eq!(parse_status_bytes("VmRSS:\t 100 MB\n", "VmRSS"), None);
    }

    #[test]
    fn rollup_parser_sums_clean_and_dirty() {
        let rollup = "Rss:\t 4096 kB\nPrivate_Clean:\t 256 kB\nPrivate_Dirty:\t 1024 kB\n";
        assert_eq!(parse_private_bytes(rollup), Some((256 + 1024) * 1024));
    }

    #[test]
    fn rollup_parser_returns_none_when_a_field_is_absent() {
        assert_eq!(parse_private_bytes("Private_Clean:\t 256 kB\n"), None);
    }

    // --- the private-bytes budget ---

    /// 3 GiB is the number that was measured against, and the widest
    /// resolution is where it was measured. The refactor must not have moved
    /// it.
    #[test]
    fn the_widest_resolution_keeps_the_budget_it_had() {
        assert_eq!(private_bytes_budget(8192), 3 * 1024 * MIB);
    }

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

    /// The rule from the original comment: a warning that fires during normal
    /// operation is a warning nobody reads. A cold cache at any resolution
    /// still decodes both 8K sources, so that launch is the worst normal
    /// operation gets and the budget has to clear it everywhere.
    ///
    /// The narrow end is the binding case, not a restatement of the wide one:
    /// it gets the smallest resident allowance and has the same decode to pay
    /// for. 2048 clears the measurement by 104 MiB where 8192 clears it by 584,
    /// so a cold-start figure set too low fails here at the two lower widths
    /// while the widest, which is where the 3 GiB total is anchored, still
    /// passes.
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

    /// The other half of the same rule: a budget nothing can reach reports
    /// nothing. At every resolution it stays under twice a cold start, so a
    /// process that has doubled its startup footprint is named.
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

    /// The resident half is what the setting actually buys, so it has to be
    /// the three textures the app holds and not a number someone typed.
    #[test]
    fn the_resident_half_is_three_mipped_textures() {
        for width in TEXTURE_RESOLUTIONS {
            let one_base_level = u64::from(width) * u64::from(width / 2) * 4;
            assert_eq!(resident_texture_bytes(width), 3 * one_base_level * 4 / 3);
        }
        assert_eq!(resident_texture_bytes(8192), 512 * MIB);
        assert_eq!(resident_texture_bytes(4096), 128 * MIB);
        assert_eq!(resident_texture_bytes(2048), 32 * MIB);
    }

    /// The renderer accepts any width as a cap, so the budget has to answer for
    /// one no config offers rather than overflow on it.
    #[test]
    fn an_absurd_resolution_still_yields_a_budget() {
        assert!(private_bytes_budget(0) >= COLD_START_BYTES);
        assert!(private_bytes_budget(u32::MAX) >= COLD_START_BYTES);
    }

    #[test]
    fn snapshot_values_are_reasonable() {
        if let Some(snap) = snapshot() {
            let ten_gb = 10 * 1024 * 1024 * 1024u64;
            assert!(
                snap.rss_bytes < ten_gb,
                "RSS {} exceeds 10 GB",
                snap.rss_bytes
            );
            assert!(
                snap.peak_rss_bytes < ten_gb,
                "peak RSS {} exceeds 10 GB",
                snap.peak_rss_bytes
            );
            assert!(
                snap.private_bytes < ten_gb,
                "private bytes {} exceeds 10 GB",
                snap.private_bytes
            );
        }
    }
}
