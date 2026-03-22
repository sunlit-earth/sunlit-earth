//! Process-level memory tracking.
//!
//! Provides memory measurement on Windows via `GetProcessMemoryInfo`,
//! and a convenience function that logs values as structured events.

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

#[cfg(test)]
mod tests {
    use super::*;

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
