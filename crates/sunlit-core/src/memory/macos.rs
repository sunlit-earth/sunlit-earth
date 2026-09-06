//! The macOS arm of `snapshot`. The counter table the three arms share is in
//! the parent module's doc.

use super::MemorySnapshot;

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
