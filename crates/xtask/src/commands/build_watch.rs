//! What a build says about itself while Packer is silent.
//!
//! Packer prints `Waiting for SSH to become available...` and then nothing for
//! the rest of an hour-long Windows install, and that is honest: between the
//! boot prompt and the guest's first SSH answer, nothing on the host is driving
//! the install. Nothing is watching it either, which is the part worth fixing.
//! The signals exist, they were just never read: the disk Packer is installing
//! into, and, for a target whose template opens one, QEMU's monitor, which will
//! say whether the machine is running, where it stopped if it is not, and what
//! is on its screen.
//!
//! So this reads them every few seconds and prints a line a minute. The line is
//! the small half of the value. The large half is that a guest QEMU has stopped
//! no longer looks like a slow install: on 2026-08-20 two Windows builds sat at
//! `Waiting for SSH` for an hour and a quarter each with a machine that had
//! been stopped since minute four, and the only way to tell was to attach to
//! the monitor by hand. That is now the loudest thing the build says, and it
//! ends the build rather than letting it run out its two-hour SSH timeout.
//!
//! One QMP connection does all of it, the boot key included, because QEMU's
//! monitor serves one client at a time.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::provider::qmp::{self, Session};
use crate::provider::target::Target;
use crate::util::{format_bytes, format_duration};

/// How often the signals are read.
///
/// Fast enough that a stopped machine is noticed while it is still worth
/// saying, cheap enough to be free: one JSON exchange and one `stat`.
pub const POLL: Duration = Duration::from_secs(5);

/// How often a line is printed when nothing is happening.
///
/// An hour of build is 60 lines, which is a heartbeat rather than a log.
pub const HEARTBEAT: Duration = Duration::from_mins(1);

/// How finely the poll sleep is cut, so a build that ends does not have to wait
/// out a poll before its watcher notices.
const TICK: Duration = Duration::from_millis(250);

/// The key that answers "Press any key to boot from CD or DVD".
///
/// The prompt appears about ten seconds into the boot and lasts about five, and
/// neither number is under anyone's control, so it is pressed repeatedly rather
/// than timed.
pub const BOOT_KEY: &str = "spc";
pub const BOOT_KEY_GAP: Duration = Duration::from_secs(1);
pub const BOOT_KEY_HOLD_MS: u32 = 100;

/// The upper bound, for a host so slow that the prompt is a minute away.
pub const BOOT_KEY_MAX_PRESSES: u32 = 60;

/// The size at which the disk says the installer is running.
///
/// Pressing has to stop then, and this is the signal to stop on: everything
/// before that boot is the firmware, which writes nothing to the disk, and
/// setup's first writes are tens of megabytes. Measured on 2026-08-20: 197 KB
/// while the firmware is deciding, 96 MB within seconds of the installer
/// starting.
///
/// Stopping matters as much as pressing. A spacebar arriving after setup is up
/// presses whatever control has focus, and on the "Installing Windows" screen
/// that is Cancel, which puts an "Are you sure you want to quit?" dialog over
/// the install. That happened on a real build, so the window is not guessed at
/// any more: it closes when the installer is observably running.
pub const INSTALLER_WRITING_BYTES: u64 = 64 * 1024 * 1024;

/// How long to keep trying to reach the monitor before giving up on it.
const CONNECT_WINDOW: Duration = Duration::from_secs(30);

/// Whether the file at `path` has grown past `threshold`.
///
/// A missing file is not started: Packer creates the disk before QEMU runs, so
/// absence here means something is wrong elsewhere and pressing on is harmless.
pub fn installer_started(path: &Path, threshold: u64) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.len() > threshold)
}

/// How the guest looks from the monitor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Guest {
    /// This target's template opens no monitor, so there is nothing to ask. The
    /// Linux build is here, and its disk is still worth watching.
    Unmonitored,
    Running,
    /// QEMU has the machine stopped. `status` is QEMU's own word for it, and
    /// `at` is where the first vCPU sits, which is how a machine that paused
    /// for a moment is told from one that cannot run at all.
    Stopped {
        status: String,
        at: Option<String>,
    },
    /// The machine is not running, and on this hypervisor that is where it
    /// ends rather than something to resume.
    ///
    /// A `Hyper-V` guest stays running through the reboots an install performs,
    /// so anything else mid-install means the install is over. There is nothing
    /// to start again and nothing wedged: `Stopped` above exists because a WHPX
    /// guest can be paused at an instruction it will never leave, which is a
    /// state this hypervisor does not produce (amendment decision 18).
    Off {
        state: String,
    },
    /// The monitor did not answer, and will not be asked again.
    Silent(String),
}

impl Guest {
    /// The word for this kind of state, for noticing that it changed.
    fn kind(&self) -> &'static str {
        match self {
            Self::Unmonitored => "unmonitored",
            Self::Running => "running",
            Self::Stopped { .. } => "stopped",
            Self::Off { .. } => "off",
            Self::Silent(_) => "silent",
        }
    }
}

/// One reading of everything a build makes observable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sample {
    pub elapsed: Duration,
    /// Size of the disk Packer is installing into.
    pub disk: u64,
    pub guest: Guest,
    /// A checksum of the guest's screen, when one could be taken. The value
    /// says nothing; a change in it says the guest is doing something.
    pub screen: Option<String>,
}

/// What to do about a sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing worth saying.
    Quiet,
    /// Print this.
    Say(String),
    /// Print this, then start the machine again.
    Resume(String),
    /// Print this, then end the guest: the build cannot finish.
    GiveUp(String),
}

/// What the watcher remembers between samples.
///
/// Separate from the reading and the printing so the decisions can be tested
/// against fabricated samples, in the same spirit as the rest of the xtask.
#[derive(Debug, Default)]
pub struct Trend {
    /// When the last line was printed, and what the disk held then.
    said: Option<Duration>,
    disk_when_said: u64,
    disk: Option<u64>,
    disk_changed: Duration,
    screen: Option<String>,
    screen_changed: Duration,
    kind: Option<&'static str>,
    /// Where the machine was when it was last started again, and what the disk
    /// held then. A second stop in the same place with the same disk is a
    /// machine that is not going to run.
    resumed: Option<(Option<String>, u64)>,
}

impl Trend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one sample in and say what it is worth.
    pub fn observe(&mut self, sample: &Sample) -> Action {
        if self.disk != Some(sample.disk) {
            self.disk = Some(sample.disk);
            self.disk_changed = sample.elapsed;
        }
        if sample.screen.is_some() && self.screen != sample.screen {
            self.screen.clone_from(&sample.screen);
            self.screen_changed = sample.elapsed;
        }
        let changed = self.kind != Some(sample.guest.kind());
        self.kind = Some(sample.guest.kind());

        if let Guest::Stopped { status, at } = &sample.guest {
            return self.stopped(sample, status, at.as_deref());
        }
        let recovered = self.resumed.take().is_some();
        if recovered {
            return self.say(sample, "the guest is running again");
        }
        if changed || self.due(sample.elapsed) {
            let line = self.progress(sample);
            return self.say(sample, &line);
        }
        Action::Quiet
    }

    /// A stopped machine: worth starting again once, and worth giving up on if
    /// starting it again changed nothing.
    fn stopped(&mut self, sample: &Sample, status: &str, at: Option<&str>) -> Action {
        let place = at.map_or_else(String::new, |at| format!(" at {at}"));
        let stuck = self
            .resumed
            .as_ref()
            .is_some_and(|(before, disk)| before.as_deref() == at && *disk == sample.disk);
        self.said = Some(sample.elapsed);
        self.disk_when_said = sample.disk;
        if stuck {
            return Action::GiveUp(format!(
                "{}  the guest stopped again{place} with nothing written in between:\n\
                 it is not going to run. Starting it changed nothing, which is the host's\n\
                 virtualization giving up rather than anything the installer did. Packer\n\
                 would wait here until its SSH timeout, so the guest is being ended now\n\
                 and this build will fail.",
                format_duration(sample.elapsed)
            ));
        }
        self.resumed = Some((at.map(str::to_owned), sample.disk));
        Action::Resume(format!(
            "{}  the guest is stopped ({status}){place} after {}; starting it again",
            format_duration(sample.elapsed),
            format_bytes(sample.disk)
        ))
    }

    fn say(&mut self, sample: &Sample, line: &str) -> Action {
        self.said = Some(sample.elapsed);
        self.disk_when_said = sample.disk;
        Action::Say(format!("{}  {line}", format_duration(sample.elapsed)))
    }

    fn due(&self, elapsed: Duration) -> bool {
        self.said
            .is_none_or(|said| elapsed.saturating_sub(said) >= HEARTBEAT)
    }

    /// The line for a build that is getting on with it.
    fn progress(&self, sample: &Sample) -> String {
        let mut parts = vec![format!("{} on disk", format_bytes(sample.disk))];
        if let Some(rate) = self.disk_rate(sample) {
            parts.push(format!("+{}/min", format_bytes(rate)));
        } else {
            parts.push(format!(
                "unchanged for {}",
                format_duration(sample.elapsed.saturating_sub(self.disk_changed))
            ));
        }
        if sample.screen.is_some() {
            let still = sample.elapsed.saturating_sub(self.screen_changed);
            if still >= HEARTBEAT {
                parts.push(format!("screen unchanged for {}", format_duration(still)));
            } else {
                parts.push("screen changing".to_owned());
            }
        }
        match &sample.guest {
            Guest::Running => parts.push("guest running".to_owned()),
            Guest::Off { state } => parts.push(format!("the guest is {state}")),
            Guest::Silent(why) => parts.push(format!("the monitor stopped answering ({why})")),
            Guest::Unmonitored | Guest::Stopped { .. } => {}
        }
        parts.join(", ")
    }

    /// Bytes per minute since the last line, or `None` if nothing was written.
    fn disk_rate(&self, sample: &Sample) -> Option<u64> {
        let said = self.said?;
        let seconds = sample.elapsed.saturating_sub(said).as_secs();
        let grown = sample.disk.checked_sub(self.disk_when_said)?;
        if seconds == 0 || grown == 0 {
            return None;
        }
        u64::try_from(u128::from(grown) * 60 / u128::from(seconds)).ok()
    }
}

/// What a watcher needs to know about the build it is watching.
#[derive(Debug, Clone)]
pub struct Watch {
    pub target: Target,
    /// The monitor port, for a target whose template opens one.
    pub qmp_port: Option<u16>,
    /// The disk Packer is installing into.
    pub disk: PathBuf,
    /// Where to leave the guest's screen. Outside Packer's output directory,
    /// because Packer deletes that when a build fails and a failed build is
    /// exactly when the last screen is worth having.
    pub screen: PathBuf,
    /// How often to read the signals. `POLL` in a build; a test turns it down
    /// so the loop can be exercised without waiting out real seconds.
    pub poll: Duration,
}

/// The running watcher, and the handle that ends it.
pub struct Watcher {
    done: Arc<AtomicBool>,
    verdict: Arc<Mutex<Option<String>>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Watcher {
    /// Whether the watcher has already ended by itself, which it does when it
    /// gives up on the guest.
    #[cfg(test)]
    fn finished(&self) -> bool {
        self.handle
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
    }

    /// End the watcher, and report in one line why it gave up if it did.
    pub fn stop(mut self) -> Option<String> {
        self.done.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        self.verdict.lock().ok().and_then(|v| v.clone())
    }
}

/// Start watching, in the background, while Packer builds.
///
/// Nothing here can fail a build that would otherwise have worked: a monitor
/// that cannot be reached costs the boot key and the guest's half of the
/// reporting, and the disk is watched either way.
pub fn spawn(watch: Watch) -> Watcher {
    let done = Arc::new(AtomicBool::new(false));
    let verdict = Arc::new(Mutex::new(None));
    let handle = std::thread::Builder::new()
        .name("build-watch".to_owned())
        .spawn({
            let done = Arc::clone(&done);
            let verdict = Arc::clone(&verdict);
            move || watch_build(&watch, &done, &verdict)
        })
        .ok();
    Watcher {
        done,
        verdict,
        handle,
    }
}

fn watch_build(watch: &Watch, done: &AtomicBool, verdict: &Mutex<Option<String>>) {
    let mut session = watch.qmp_port.and_then(|port| connect(port, done));
    if let Some(session) = session.as_mut() {
        press_boot_key(watch, session, done);
    }
    let started = Instant::now();
    let mut trend = Trend::new();
    let mut screens = watch.qmp_port.is_some();
    while !wait(done, watch.poll) {
        let sample = read(watch, &mut session, &mut screens, started.elapsed());
        match trend.observe(&sample) {
            Action::Quiet => {}
            Action::Say(text) => say(&text),
            Action::Resume(text) => {
                say(&text);
                if let Some(session) = session.as_mut()
                    && let Err(e) = session.cont()
                {
                    say(&format!("the guest would not start again ({e})"));
                }
            }
            Action::GiveUp(text) => {
                let reason = gave_up(&watch.screen);
                say(&format!("{text}\n{reason}"));
                if let Some(session) = session.as_mut() {
                    let _ = session.quit();
                }
                if let Ok(mut verdict) = verdict.lock() {
                    *verdict = Some(reason);
                }
                return;
            }
        }
    }
}

/// The one line that says why a build was ended, and where to look.
///
/// The same sentence twice on purpose: once where it happened, and once in the
/// command's error at the end, which is the line a person still has on screen
/// after an hour of build output has scrolled past. It names the screenshot
/// because that is the artifact that says what the guest was doing, and it says
/// nothing about a screenshot that was never taken.
fn gave_up(screen: &Path) -> String {
    if screen.is_file() {
        format!(
            "the guest stopped and could not be started again; \
             its last screen is at {}",
            screen.display()
        )
    } else {
        "the guest stopped and could not be started again".to_owned()
    }
}

/// `"1 press"` / `"2 presses"`, which the generic pluralizer gets wrong.
fn presses(n: u32) -> String {
    if n == 1 {
        "1 press".to_owned()
    } else {
        format!("{n} presses")
    }
}

/// Print one reading under a label, continuation lines lined up under the first.
///
/// Shared with the native `Hyper-V` build, so a build reads the same whichever
/// mechanism produced it. Every label is nine characters wide including its
/// colon, which is what lines the two columns up.
pub fn report(label: &str, text: &str) {
    let column = format!("{label}:");
    for (index, line) in text.lines().enumerate() {
        if index == 0 {
            tell(&format!("  {column:<9}  {line}"));
        } else {
            tell(&format!("             {}", line.trim_start()));
        }
    }
}

/// Print one reading of a build's progress.
fn say(text: &str) {
    report("progress", text);
}

/// Write one line, and carry on if there is nothing listening.
///
/// `println!` panics when stdout is gone, which for a watcher thread means the
/// boot key stops being pressed and the build dies waiting for SSH. That is not
/// hypothetical: piping a build through `head` did exactly that. A reporter has
/// no business taking the build down, so a failed write is dropped.
fn tell(line: &str) {
    use std::io::Write as _;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// Reach the monitor, allowing for QEMU not being up yet.
fn connect(port: u16, done: &AtomicBool) -> Option<Session> {
    let deadline = Instant::now() + CONNECT_WINDOW;
    loop {
        match Session::connect(port) {
            Ok(session) => return Some(session),
            Err(e) => {
                if Instant::now() >= deadline || done.load(Ordering::Relaxed) {
                    tell(&format!(
                        "  progress:  no monitor on 127.0.0.1:{port} ({e}); \
                         watching the disk only"
                    ));
                    return None;
                }
                std::thread::sleep(TICK);
            }
        }
    }
}

/// Answer the installer's boot prompt.
///
/// Packer's own `boot_command` is empty because its VNC keystrokes do not
/// arrive on this host; QMP's `send-key` injects at the input device instead.
fn press_boot_key(watch: &Watch, session: &mut Session, done: &AtomicBool) {
    if watch.target != Target::Windows {
        return;
    }
    tell(&format!(
        "  boot key:  {BOOT_KEY} until the installer writes, to answer \
         \"Press any key to boot from CD or DVD\""
    ));
    let disk = watch.disk.clone();
    let stop = || done.load(Ordering::Relaxed) || installer_started(&disk, INSTALLER_WRITING_BYTES);
    match qmp::press_key_until(
        session,
        BOOT_KEY,
        BOOT_KEY_MAX_PRESSES,
        BOOT_KEY_GAP,
        BOOT_KEY_HOLD_MS,
        &stop,
    ) {
        Ok(sent) if installer_started(&watch.disk, INSTALLER_WRITING_BYTES) => {
            tell(&format!(
                "  boot key:  the installer started after {}",
                presses(sent)
            ));
        }
        Ok(sent) => tell(&format!(
            "  boot key:  {} sent and the installer has not started; if this build \
             fails waiting for SSH, the boot prompt was missed",
            presses(sent)
        )),
        Err(e) => tell(&format!("  boot key:  not sent ({e})")),
    }
}

/// Read every signal once.
///
/// A monitor that errors is dropped rather than retried: this is a stream
/// protocol, so a failed exchange leaves the connection out of step with the
/// server, and a build reporting its disk is better than one reporting nonsense
/// about its guest.
fn read(
    watch: &Watch,
    session: &mut Option<Session>,
    screens: &mut bool,
    elapsed: Duration,
) -> Sample {
    let disk = std::fs::metadata(&watch.disk).map_or(0, |meta| meta.len());
    let Some(live) = session.as_mut() else {
        return Sample {
            elapsed,
            disk,
            guest: Guest::Unmonitored,
            screen: None,
        };
    };
    let guest = match live.status() {
        Err(e) => {
            *session = None;
            *screens = false;
            return Sample {
                elapsed,
                disk,
                guest: Guest::Silent(e),
                screen: None,
            };
        }
        Ok(status) if status == "running" => Guest::Running,
        Ok(status) => Guest::Stopped {
            status,
            at: live.instruction_pointer(),
        },
    };
    let screen = screenshot(watch, session, screens);
    Sample {
        elapsed,
        disk,
        guest,
        screen,
    }
}

/// Take a screenshot and hash it, or stop taking them.
fn screenshot(watch: &Watch, session: &mut Option<Session>, screens: &mut bool) -> Option<String> {
    if !*screens {
        return None;
    }
    let live = session.as_mut()?;
    if let Err(e) = live.screendump(&watch.screen) {
        // An older QEMU has no PNG writer, and this is the only place that
        // would notice. Say it once and carry on with the rest.
        tell(&format!("  progress:  no screenshots from this QEMU ({e})"));
        *screens = false;
        return None;
    }
    crate::store::hash::checksum_file(&watch.screen).ok()
}

/// Sleep in slices, and report whether the build has ended.
fn wait(done: &AtomicBool, total: Duration) -> bool {
    let deadline = Instant::now() + total;
    loop {
        if done.load(Ordering::Relaxed) {
            return true;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        std::thread::sleep(left.min(TICK));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn running(elapsed: u64, disk: u64) -> Sample {
        Sample {
            elapsed: secs(elapsed),
            disk,
            guest: Guest::Running,
            screen: None,
        }
    }

    fn stopped(elapsed: u64, disk: u64, at: Option<&str>) -> Sample {
        Sample {
            elapsed: secs(elapsed),
            disk,
            guest: Guest::Stopped {
                status: "paused".to_owned(),
                at: at.map(str::to_owned),
            },
            screen: None,
        }
    }

    fn text(action: &Action) -> String {
        match action {
            Action::Quiet => "(quiet)".to_owned(),
            Action::Say(t) | Action::Resume(t) | Action::GiveUp(t) => t.clone(),
        }
    }

    #[test]
    fn the_first_reading_is_printed_and_the_next_few_are_not() {
        let mut trend = Trend::new();
        assert!(matches!(trend.observe(&running(5, 1024)), Action::Say(_)));
        assert_eq!(trend.observe(&running(10, 2048)), Action::Quiet);
        assert_eq!(trend.observe(&running(30, 4096)), Action::Quiet);
        let minute = trend.observe(&running(65, 8192));
        assert!(matches!(minute, Action::Say(_)), "{minute:?}");
    }

    #[test]
    fn a_line_carries_the_size_the_rate_and_the_guest() {
        let mut trend = Trend::new();
        let _ = trend.observe(&running(5, 1_000_000_000));
        let line = text(&trend.observe(&running(65, 2_000_000_000)));
        assert!(line.starts_with("1m05s  "), "{line}");
        assert!(line.contains("1.9 GiB on disk"), "{line}");
        // A gigabyte in the minute since the last line.
        assert!(line.contains("/min"), "{line}");
        assert!(line.contains("guest running"), "{line}");
    }

    #[test]
    fn a_disk_that_stopped_growing_says_how_long_for() {
        let mut trend = Trend::new();
        let _ = trend.observe(&running(5, 12_000_000_000));
        let line = text(&trend.observe(&running(605, 12_000_000_000)));
        assert!(line.contains("unchanged for 10m00s"), "{line}");
        assert!(!line.contains("/min"), "{line}");
    }

    #[test]
    fn a_still_screen_is_told_from_a_changing_one() {
        let mut trend = Trend::new();
        let mut sample = running(5, 1024);
        sample.screen = Some("crc32:aaaaaaaa".to_owned());
        let first = text(&trend.observe(&sample));
        assert!(first.contains("screen changing"), "{first}");

        let mut later = running(65, 1024);
        later.screen = Some("crc32:bbbbbbbb".to_owned());
        let changed = text(&trend.observe(&later));
        assert!(changed.contains("screen changing"), "{changed}");

        let mut same = running(600, 1024);
        same.screen = Some("crc32:bbbbbbbb".to_owned());
        let still = text(&trend.observe(&same));
        assert!(still.contains("screen unchanged for 8m55s"), "{still}");
    }

    #[test]
    fn a_stopped_guest_is_reported_and_started_again() {
        let mut trend = Trend::new();
        let _ = trend.observe(&running(5, 12_000_000_000));
        let action = trend.observe(&stopped(10, 12_000_000_000, Some("0xfffd3da2")));
        let line = text(&action);
        assert!(matches!(action, Action::Resume(_)), "{action:?}");
        assert!(line.contains("the guest is stopped (paused)"), "{line}");
        assert!(line.contains("at 0xfffd3da2"), "{line}");
        assert!(line.contains("starting it again"), "{line}");
    }

    #[test]
    fn a_guest_that_stops_twice_in_the_same_place_ends_the_build() {
        let mut trend = Trend::new();
        let _ = trend.observe(&stopped(10, 12_000_000_000, Some("0xfffd3da2")));
        let action = trend.observe(&stopped(15, 12_000_000_000, Some("0xfffd3da2")));
        let line = text(&action);
        assert!(matches!(action, Action::GiveUp(_)), "{action:?}");
        assert!(line.contains("stopped again"), "{line}");
        assert!(line.contains("not going to run"), "{line}");
        assert!(line.contains("SSH timeout"), "{line}");
    }

    #[test]
    fn a_stop_that_moved_on_is_started_again_rather_than_given_up_on() {
        let mut trend = Trend::new();
        let _ = trend.observe(&stopped(10, 12_000_000_000, Some("0xfffd3da2")));
        // A different instruction, and a disk that grew: the machine is doing
        // something between the stops, whatever it is.
        let moved = trend.observe(&stopped(15, 12_000_000_000, Some("0xdeadbeef")));
        assert!(matches!(moved, Action::Resume(_)), "{moved:?}");
        let mut trend = Trend::new();
        let _ = trend.observe(&stopped(10, 12_000_000_000, Some("0xfffd3da2")));
        let wrote = trend.observe(&stopped(15, 12_500_000_000, Some("0xfffd3da2")));
        assert!(matches!(wrote, Action::Resume(_)), "{wrote:?}");
    }

    #[test]
    fn a_guest_that_comes_back_says_so_instead_of_ending_the_build() {
        let mut trend = Trend::new();
        let _ = trend.observe(&stopped(10, 12_000_000_000, Some("0xfffd3da2")));
        let back = trend.observe(&running(15, 12_000_000_000));
        assert!(text(&back).contains("running again"), "{back:?}");
        // And the stop is forgotten, so a later one is resumed on its own
        // merits rather than being treated as the second of a pair.
        let again = trend.observe(&stopped(20, 12_000_000_000, Some("0xfffd3da2")));
        assert!(matches!(again, Action::Resume(_)), "{again:?}");
    }

    #[test]
    fn an_unmonitored_target_still_reports_its_disk() {
        let mut trend = Trend::new();
        let sample = Sample {
            elapsed: secs(5),
            disk: 3_000_000_000,
            guest: Guest::Unmonitored,
            screen: None,
        };
        let line = text(&trend.observe(&sample));
        assert!(line.contains("2.8 GiB on disk"), "{line}");
        assert!(!line.contains("guest"), "{line}");
    }

    #[test]
    fn a_guest_that_turned_itself_off_is_said_at_once_and_not_resumed() {
        // The Hyper-V build's signal for "the install ended". Reported the
        // moment it changes rather than at the next heartbeat, and never as
        // something to start again: there is nothing wedged to recover.
        let mut trend = Trend::new();
        let _ = trend.observe(&running(5, 12_000_000_000));
        let sample = Sample {
            elapsed: secs(10),
            disk: 12_000_000_000,
            guest: Guest::Off {
                state: "Off".to_owned(),
            },
            screen: None,
        };
        let action = trend.observe(&sample);
        assert!(matches!(action, Action::Say(_)), "{action:?}");
        let line = text(&action);
        assert!(line.contains("the guest is Off"), "{line}");
        assert!(!line.contains("starting it again"), "{line}");
    }

    #[test]
    fn a_monitor_that_goes_quiet_is_reported_when_it_happens() {
        let mut trend = Trend::new();
        let _ = trend.observe(&running(5, 1024));
        let sample = Sample {
            elapsed: secs(10),
            disk: 1024,
            guest: Guest::Silent("the QMP socket closed".to_owned()),
            screen: None,
        };
        // Not held back for the heartbeat: the state changed.
        let line = text(&trend.observe(&sample));
        assert!(line.contains("stopped answering"), "{line}");
        assert!(line.contains("the QMP socket closed"), "{line}");
    }

    #[test]
    fn the_verdict_names_the_screenshot_when_there_is_one() {
        let dir = std::env::temp_dir().join("sunlit_xtask_verdict");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let screen = dir.join("screen.png");
        assert!(
            !gave_up(&screen).contains("screen"),
            "a screenshot that was never taken is not offered"
        );
        std::fs::write(&screen, b"not really a png").expect("write");
        let named = gave_up(&screen);
        assert!(named.contains("could not be started again"), "{named}");
        assert!(
            named.contains(&screen.display().to_string()),
            "the path itself has to be in the line: {named}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn presses_are_counted_in_english() {
        assert_eq!(presses(1), "1 press");
        assert_eq!(presses(51), "51 presses");
    }

    #[test]
    fn a_watcher_over_a_guest_that_will_not_run_ends_the_build() {
        // The whole thread, against a monitor that answers "paused" forever:
        // the boot key, the polling, the resume, the verdict, and the `quit`
        // that makes Packer fail now rather than at its SSH timeout.
        let dir = std::env::temp_dir().join("sunlit_xtask_watch_giveup");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let disk = dir.join(crate::commands::build_image::BUILT_DISK);
        // Sparse rather than written: this only has to look big enough for the
        // boot key to see an installer at work and stop pressing.
        let file = std::fs::File::create(&disk).expect("create the test disk");
        file.set_len(INSTALLER_WRITING_BYTES + 1)
            .expect("size the test disk");
        drop(file);

        // A screenshot from an earlier poll, so the verdict has one to name.
        let screen = dir.join("screen.png");
        std::fs::write(&screen, b"not really a png").expect("write the screen");

        let server = qmp::fake::Server::start(&["paused"]);
        let watcher = spawn(Watch {
            target: Target::Windows,
            qmp_port: Some(server.port),
            disk,
            screen: screen.clone(),
            poll: Duration::from_millis(20),
        });
        for _ in 0..500 {
            if watcher.finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let verdict = watcher.stop().expect("a watcher that gave up says so");
        assert!(verdict.contains("could not be started again"), "{verdict}");
        assert!(
            verdict.contains(&screen.display().to_string()),
            "the verdict carries the screenshot path into the command's error: {verdict}"
        );
        let asked = server.received();
        assert!(
            asked.iter().any(|line| line.contains("send-key")),
            "the boot prompt is answered first: {asked:?}"
        );
        assert!(
            asked.iter().any(|line| line.contains("\"cont\"")),
            "a stopped guest is started again before it is given up on: {asked:?}"
        );
        assert!(
            asked.iter().any(|line| line.contains("\"quit\"")),
            "and then ended, so the build fails now: {asked:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_boot_key_stops_once_the_installer_is_writing() {
        // The signal has to distinguish "the firmware is still deciding", where
        // the disk is a couple of hundred kilobytes, from "setup is running",
        // where it is tens of megabytes within seconds. Pressing past that point
        // put an "are you sure you want to quit" dialog over a real install.
        let dir = std::env::temp_dir().join("sunlit_xtask_boot_key_disk");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let disk = dir.join(crate::commands::build_image::BUILT_DISK);
        assert!(
            !installer_started(&disk, INSTALLER_WRITING_BYTES),
            "a disk that does not exist yet has not started"
        );
        std::fs::write(&disk, vec![0u8; 197_632]).expect("write");
        assert!(
            !installer_started(&disk, INSTALLER_WRITING_BYTES),
            "the firmware writes nothing, and this is what that looks like"
        );
        std::fs::write(&disk, vec![0u8; 96 * 1024 * 1024]).expect("write");
        assert!(installer_started(&disk, INSTALLER_WRITING_BYTES));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
