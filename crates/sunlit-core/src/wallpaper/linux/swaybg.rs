//! A `swaybg` this app starts for every publish, and the one it replaces.
//!
//! swaybg has no IPC: a new image is a new process. So each publish notes the
//! ones already running whose command line names a file under the app's
//! wallpaper directory, starts its own, gives it a second to put its surface
//! up, and then ends the ones it noted. The last one is left running when the
//! app exits, which keeps the wallpaper on screen the way every desktop's own
//! setter does, and the next start finds it by its command line rather than
//! through a state file.

use std::path::Path;

use crate::desktop::names_a_file_under;

/// One process of this user's.
pub(super) struct Running<'a> {
    pub(super) pid: u32,
    pub(super) comm: &'a str,
    pub(super) cmdline: &'a [String],
}

/// The `swaybg` processes this app started.
///
/// A `swaybg` showing a file of the user's own is theirs, and stays.
pub(super) fn ours(processes: &[Running<'_>], dir: &Path) -> Vec<u32> {
    processes
        .iter()
        .filter(|p| p.comm == "swaybg" && names_a_file_under(p.cmdline, dir))
        .map(|p| p.pid)
        .collect()
}

/// Which of the `swaybg`s a publish noted before starting its own are still
/// ours to end once the new one is up.
///
/// Only those noted: one a later publish started meanwhile is that publish's
/// to keep. And only while the pid is still one of ours, since a pid that
/// ended may have been given to something else.
pub(super) fn to_end(noted: &[u32], processes: &[Running<'_>], dir: &Path) -> Vec<u32> {
    ours(processes, dir)
        .into_iter()
        .filter(|pid| noted.contains(pid))
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
    use std::time::{Duration, Instant};

    use super::{Running, load_failure, ours, to_end};
    use crate::desktop::Invocation;
    use crate::desktop::probe::{Process, own_processes};

    /// How long a new `swaybg` has to fail before it counts as running.
    const STARTUP: Duration = Duration::from_millis(500);

    /// How long the old surface stays up under the new one, so the screen
    /// never shows the compositor's grey between them.
    const HANDOVER: Duration = Duration::from_secs(1);

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
        let noted = ours(&running(&own_processes()), wallpaper_dir);
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

        let previous = OWNED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(child);
        let dir = wallpaper_dir.to_owned();
        std::thread::Builder::new()
            .name("swaybg-handover".to_owned())
            .spawn(move || hand_over(&dir, &noted, previous))
            .map_err(|e| format!("cannot start the swaybg handover: {e}"))?;
        Ok(())
    }

    fn running(processes: &[Process]) -> Vec<Running<'_>> {
        processes
            .iter()
            .map(|p| Running {
                pid: p.pid,
                comm: &p.comm,
                cmdline: &p.cmdline,
            })
            .collect()
    }

    fn hand_over(dir: &Path, noted: &[u32], previous: Option<Child>) {
        std::thread::sleep(HANDOVER);
        for pid in to_end(noted, &running(&own_processes()), dir) {
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
            let _ = child.kill();
            let _ = child.wait();
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
        assert_eq!(ours(&processes, dir), vec![10, 11]);
        // Noted before 11 started, so 11 is kept whenever the handover runs.
        assert_eq!(to_end(&[10], &processes, dir), vec![10]);
        // A pid noted as ours that now belongs to something else is left.
        assert_eq!(to_end(&[10, 13], &processes, dir), vec![10]);
        assert!(to_end(&[], &processes, dir).is_empty());
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
