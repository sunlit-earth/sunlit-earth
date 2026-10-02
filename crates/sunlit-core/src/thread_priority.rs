//! Lowering a background thread's scheduling priority.
//!
//! Each OS has its own notion of "below normal", and each applies it to the
//! calling thread alone, so a worker lowers itself when it starts. The level is
//! absolute rather than a step down from wherever the thread stands, since a
//! thread created by a lowered one starts out lowered on Linux and macOS, and
//! lowering it again must not take it further:
//!
//! - Windows: `THREAD_PRIORITY_BELOW_NORMAL`, one step under the normal
//!   threads of the process.
//! - Linux: the nice value of the process's main thread plus 10, the
//!   adjustment `nice` makes by default. Under NPTL the nice value belongs to
//!   each thread, not to the process.
//! - macOS: the utility quality of service, Apple's class for long work whose
//!   progress the user sees but does not wait on.
//!
//! Elsewhere nothing is done and the thread keeps its priority.

use std::io;

/// Lower the calling thread's priority, logging rather than failing when the
/// OS refuses: a worker at the normal priority is slower for the foreground
/// but no less correct.
pub(crate) fn lower_current_thread(what: &str) {
    if let Err(e) = lower() {
        tracing::warn!(thread = what, error = %e, "could not lower the thread's priority");
    }
}

#[cfg(windows)]
fn lower() -> io::Result<()> {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
    };
    // SAFETY: GetCurrentThread returns a pseudo handle for the calling thread
    // that needs no closing, and SetThreadPriority reads only its two
    // arguments.
    #[allow(unsafe_code)]
    let ok = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn lower() -> io::Result<()> {
    use rustix::process::{getpid, getpriority_process, setpriority_process};
    let base = getpriority_process(Some(getpid()))?;
    setpriority_process(None, (base + 10).min(19))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn lower() -> io::Result<()> {
    // SAFETY: pthread_set_qos_class_self_np changes the calling thread's own
    // class and reads nothing but its two arguments.
    #[allow(unsafe_code)]
    let code =
        unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0) };
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code))
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn lower() -> io::Result<()> {
    Ok(())
}

/// How the calling thread's priority compares with the level
/// [`lower_current_thread`] sets: `Greater` while it runs above it, `None` on
/// an OS where it cannot be read back.
#[cfg(test)]
#[cfg_attr(
    any(windows, target_os = "linux"),
    expect(
        clippy::unnecessary_wraps,
        reason = "the other OSes have no reader and answer None"
    )
)]
pub(crate) fn lowered() -> Option<std::cmp::Ordering> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, GetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
        };
        // SAFETY: as in `lower`, a pseudo handle for the calling thread.
        #[allow(unsafe_code)]
        let priority = unsafe { GetThreadPriority(GetCurrentThread()) };
        Some(priority.cmp(&THREAD_PRIORITY_BELOW_NORMAL))
    }
    #[cfg(target_os = "linux")]
    {
        use rustix::process::{getpid, getpriority_process};
        let base = getpriority_process(Some(getpid())).expect("the process's nice value");
        let nice = getpriority_process(None).expect("the thread's nice value");
        Some((base + 10).min(19).cmp(&nice))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::*;

    #[test]
    fn a_lowered_thread_and_the_threads_it_makes_stand_at_one_level() {
        let (before, first, second) = std::thread::spawn(|| {
            let before = lowered();
            lower().expect("lower the priority");
            let first = lowered();
            let second = std::thread::spawn(|| {
                lower().expect("lower the priority again");
                lowered()
            })
            .join()
            .expect("the inner thread");
            (before, first, second)
        })
        .join()
        .expect("the thread");

        match before {
            None => println!("skipped the read back: this OS has no reader for it"),
            Some(Ordering::Greater) => {
                assert_eq!(first, Some(Ordering::Equal));
                assert_eq!(second, Some(Ordering::Equal), "lowered once, not twice");
                assert_eq!(
                    lowered(),
                    Some(Ordering::Greater),
                    "the test's thread is untouched"
                );
            }
            Some(_) => println!("skipped: the test process already runs at the lowest level"),
        }
    }
}
