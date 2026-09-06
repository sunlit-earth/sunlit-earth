//! The Windows arm of `snapshot`. The counter table the three arms share is
//! in the parent module's doc.

use super::MemorySnapshot;

/// Return a snapshot of the current process's memory counters.
///
/// Windows calls `GetProcessMemoryInfo` with `PROCESS_MEMORY_COUNTERS_EX`,
/// Linux reads `/proc/self`, macOS asks the Mach kernel. Returns `None` on any
/// other platform, and on the three supported ones only if the OS refuses to
/// answer.
#[cfg(windows)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "the counters struct is a few dozen bytes, which is what its cb field carries"
)]
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
