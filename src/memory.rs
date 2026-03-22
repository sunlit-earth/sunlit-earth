//! Process-level memory tracking.
//!
//! Provides RSS (working set) measurement on Windows via `GetProcessMemoryInfo`,
//! and a convenience function that logs the current value as a structured event.

/// Return the current process's resident set size (working set) in bytes.
///
/// On Windows this calls `GetProcessMemoryInfo` to read
/// `PROCESS_MEMORY_COUNTERS.WorkingSetSize`. On other platforms, returns `None`.
#[cfg(windows)]
#[allow(clippy::cast_possible_truncation)]
pub fn current_rss_bytes() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut counters: PROCESS_MEMORY_COUNTERS = PROCESS_MEMORY_COUNTERS {
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
    };
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;

    // SAFETY: GetCurrentProcess returns a pseudo-handle (-1) that is always
    // valid for the calling process and does not need to be closed.
    // GetProcessMemoryInfo reads process memory counters into the provided
    // struct. The struct is stack-allocated with cb set to its size.
    #[allow(unsafe_code)]
    let success =
        unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &raw mut counters, counters.cb) };

    if success != 0 {
        Some(counters.WorkingSetSize as u64)
    } else {
        None
    }
}

#[cfg(not(windows))]
pub fn current_rss_bytes() -> Option<u64> {
    None
}

/// Log the current process RSS at `debug` level with structured fields.
///
/// The `context` parameter describes the checkpoint (e.g. "after wgpu init").
#[allow(clippy::cast_precision_loss)]
pub fn log_memory_usage(context: &str) {
    if let Some(bytes) = current_rss_bytes() {
        let mb = bytes as f64 / (1024.0 * 1024.0);
        tracing::debug!(context, rss_mb = format_args!("{mb:.1}"), "memory usage");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_rss_returns_some_on_windows() {
        let rss = current_rss_bytes();
        if cfg!(windows) {
            assert!(rss.is_some(), "expected Some on Windows");
            assert!(rss.unwrap() > 0, "RSS should be > 0");
        }
    }

    #[test]
    fn current_rss_is_reasonable() {
        if let Some(rss) = current_rss_bytes() {
            let ten_gb = 10 * 1024 * 1024 * 1024u64;
            assert!(rss < ten_gb, "RSS {rss} exceeds 10 GB sanity limit");
        }
    }
}
