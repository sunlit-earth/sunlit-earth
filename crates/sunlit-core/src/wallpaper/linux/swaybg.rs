//! A `swaybg` this app starts for every publish, and the one it replaces.
//!
//! swaybg has no IPC: a new image is a new process. So each publish starts one,
//! gives it a second to put its surface up, and then ends every earlier one
//! whose command line names a file under the app's wallpaper directory. The
//! last one is left running when the app exits, which keeps the wallpaper on
//! screen the way every desktop's own setter does, and the next start finds it
//! by its command line rather than through a state file.

use std::path::Path;

use crate::desktop::names_a_file_under;

/// One process of this user's.
pub(super) struct Running<'a> {
    pub(super) pid: u32,
    pub(super) comm: &'a str,
    pub(super) cmdline: &'a [String],
}

/// The `swaybg` processes this app started, other than `keep`.
///
/// A `swaybg` showing a file of the user's own is theirs, and stays.
pub(super) fn to_end(processes: &[Running<'_>], dir: &Path, keep: u32) -> Vec<u32> {
    processes
        .iter()
        .filter(|p| p.pid != keep && p.comm == "swaybg" && names_a_file_under(p.cmdline, dir))
        .map(|p| p.pid)
        .collect()
}

/// swaybg's own report that it could not read the image, from its log.
///
/// swaybg does not exit when an image fails to load: it keeps running and
/// draws nothing, so its exit status cannot tell that publish from one that
/// worked, and its log is the only place that says so.
pub(super) fn load_failure(log: &str) -> Option<&str> {
    log.lines()
        .find(|line| line.contains("Failed to load"))
        .map(str::trim)
}

#[cfg(target_os = "linux")]
pub(super) use live::publish;

#[cfg(target_os = "linux")]
mod live {
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    use super::{Running, load_failure, to_end};
    use crate::desktop::Invocation;
    use crate::desktop::probe::own_processes;

    /// How long a new `swaybg` has to fail before it counts as running.
    const STARTUP: Duration = Duration::from_millis(500);

    /// How long the old surface stays up under the new one, so the screen
    /// never shows the compositor's grey between them.
    const HANDOVER: Duration = Duration::from_secs(1);

    /// The `swaybg` the latest publish started, which is never ended.
    static LATEST: AtomicU32 = AtomicU32::new(0);

    /// The `swaybg` this process started last, kept so that ending it can also
    /// reap it. One at a time: each publish takes the previous one out.
    static OWNED: Mutex<Option<Child>> = Mutex::new(None);

    /// Start `command`, and once it is up, end the ones it replaces.
    ///
    /// The handover runs on a one-shot thread so the engine does not wait out
    /// the second; the thread sleeps, signals, reaps, and ends.
    pub(in crate::wallpaper::linux) fn publish(
        command: &Invocation,
        image_dir: &Path,
        wallpaper_dir: &Path,
    ) -> Result<(), String> {
        let log_path = image_dir.join("swaybg.log");
        let log = std::fs::File::create(&log_path)
            .map_err(|e| format!("cannot write {}: {e}", log_path.display()))?;
        let mut child = {
            use std::os::unix::process::CommandExt as _;
            Command::new(command.program)
                .args(&command.args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(log)
                .process_group(0)
                .spawn()
                .map_err(|e| format!("cannot start {}: {e}", command.program))?
        };
        let started = Instant::now();
        while started.elapsed() < STARTUP {
            if let Some(status) = child
                .try_wait()
                .map_err(|e| format!("cannot watch swaybg: {e}"))?
            {
                let said = std::fs::read_to_string(&log_path).unwrap_or_default();
                return Err(format!(
                    "swaybg {} exited straight away ({status}): {}",
                    command.args.join(" "),
                    said.trim()
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let said = std::fs::read_to_string(&log_path).unwrap_or_default();
        if let Some(failure) = load_failure(&said) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("swaybg could not show the image: {failure}"));
        }

        LATEST.store(child.id(), Ordering::SeqCst);
        let previous = OWNED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(child);
        let dir = wallpaper_dir.to_owned();
        std::thread::Builder::new()
            .name("swaybg-handover".to_owned())
            .spawn(move || hand_over(&dir, previous))
            .map_err(|e| format!("cannot start the swaybg handover: {e}"))?;
        Ok(())
    }

    fn hand_over(dir: &Path, previous: Option<Child>) {
        std::thread::sleep(HANDOVER);
        let processes = own_processes();
        let running: Vec<Running<'_>> = processes
            .iter()
            .map(|p| Running {
                pid: p.pid,
                comm: &p.comm,
                cmdline: &p.cmdline,
            })
            .collect();
        for pid in to_end(&running, dir, LATEST.load(Ordering::SeqCst)) {
            let signalled = i32::try_from(pid)
                .ok()
                .and_then(rustix::process::Pid::from_raw)
                .map(|pid| rustix::process::kill_process(pid, rustix::process::Signal::TERM));
            if let Some(Err(e)) = signalled {
                tracing::warn!(pid, "an earlier swaybg could not be ended: {e}");
            }
        }
        if let Some(mut child) = previous {
            let deadline = Instant::now() + HANDOVER;
            while Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if child.id() != LATEST.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    #[test]
    fn only_our_earlier_swaybgs_are_ended() {
        let dir = Path::new("/home/t/.local/share/SunlitEarth/wallpaper");
        let old = line(&[
            "swaybg",
            "-o",
            "*",
            "-i",
            "/home/t/.local/share/SunlitEarth/wallpaper/gen-1/0.png",
            "-m",
            "fill",
        ]);
        let new = line(&[
            "swaybg",
            "-o",
            "*",
            "-i",
            "/home/t/.local/share/SunlitEarth/wallpaper/gen-2/0.png",
            "-m",
            "fill",
        ]);
        let users = line(&["swaybg", "-i", "/home/t/Pictures/beach.png"]);
        let viewer = line(&[
            "imv",
            "/home/t/.local/share/SunlitEarth/wallpaper/gen-1/0.png",
        ]);
        let processes = [
            Running {
                pid: 10,
                comm: "swaybg",
                cmdline: &old,
            },
            Running {
                pid: 11,
                comm: "swaybg",
                cmdline: &new,
            },
            Running {
                pid: 12,
                comm: "swaybg",
                cmdline: &users,
            },
            Running {
                pid: 13,
                comm: "imv",
                cmdline: &viewer,
            },
        ];
        assert_eq!(to_end(&processes, dir, 11), vec![10]);
        // With nothing kept, every one of ours goes and still nothing else.
        assert_eq!(to_end(&processes, dir, 0), vec![10, 11]);
    }

    #[test]
    fn a_swaybg_that_could_not_read_the_image_is_a_failure() {
        let failed = "2026-09-23 19:25:33 - [main.c:282] Found config * for output Virtual-1
            2026-09-23 19:25:33 - [background-image.c:30] Failed to load background image             (Couldn't recognize the image file format for file \"/tmp/0.png\").
            2026-09-23 19:25:33 - [main.c:610] Failed to load image: /tmp/0.png
";
        let failure = load_failure(failed).expect("a load failure");
        assert!(failure.contains("Couldn't recognize"), "{failure}");
        let shown = "2026-09-23 19:27:24 - [main.c:282] Found config * for output Virtual-1
";
        assert_eq!(load_failure(shown), None);
        assert_eq!(load_failure(""), None);
    }
}
