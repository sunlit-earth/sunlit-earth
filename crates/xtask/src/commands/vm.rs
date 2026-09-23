//! The VM lifecycle commands: `up`, `ssh`, `view`, `status`, `down`, `purge`,
//! guest-contract smoke test.
//!
//! `vm up` and `e2e --target <t>` share the whole boot path, which is what
//! makes an interactive guest and a test guest the same guest.

use std::fmt::Write as _;
use std::time::Duration;

use crate::commands::status;
use crate::commands::teardown::{self, Selection};
use crate::guest::job;
use crate::provider::desktop::{Desktop, Login, SessionType};
use crate::provider::target::{HostOs, Image, ProviderKind, Target};
use crate::provider::{self, Provider};
use crate::runner::Runner;
use crate::store::inventory::{self, ImageCondition};
use crate::store::manifest::EvalState;
use crate::store::state::{RunState, StartReason};
use crate::store::{self, Store};
use crate::util;

/// How long a cold boot may take before the SSH server answers.
pub const BOOT_TIMEOUT: Duration = Duration::from_mins(10);

/// How long the desktop session may take after that.
pub const SESSION_TIMEOUT: Duration = Duration::from_mins(5);

/// A booted guest and the provider that owns it.
pub struct Session<'a> {
    pub provider: Box<dyn Provider + 'a>,
    pub state: RunState,
    pub image: Image,
}

impl Session<'_> {
    /// The operating system in the guest, which is what the guest contract and
    /// the job scripts are written against.
    pub fn target(&self) -> Target {
        self.image.target()
    }

    /// Stop the VM and remove the run state it left behind.
    ///
    /// The record goes only after the teardown succeeded, and a file that
    /// could not be removed is reported rather than swallowed: a leaked
    /// overlay is not cosmetic, because the next boot creates its child with
    /// `New-VHD -Path <existing>` or `qemu-img create <existing>` and both
    /// refuse.
    pub fn tear_down(&self, store: &Store) -> Result<(), String> {
        self.provider.destroy(&self.state)?;

        let problems = remove_run_state(&run_state_paths(store, self.image, &self.state));
        if problems.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "the VM was stopped, but {} could not be removed. The next boot \
                 will refuse to create its overlay until they are gone.",
                problems.join("; ")
            ))
        }
    }

    /// Record the VM, start it, and wait until it can be used.
    ///
    /// Split out of `boot` so that every step from "the VM exists" onwards is
    /// behind one `?`-free boundary: the caller turns any error here into a
    /// teardown or a message, and none of these steps can return quietly.
    ///
    /// The record is written before the VM is started and again afterwards.
    /// Before, because a running VM with no state file is invisible to status
    /// and destroy, and `teardown::plan` would then see an overlay with no VM
    /// behind it and unlink the disk of a live guest. Afterwards, because that
    /// is when the process id and the address exist to record.
    fn bring_up(&mut self, store: &Store) -> Result<(), String> {
        write_state(store, self.image, &self.state)?;

        println!(
            "starting {} on {}",
            self.state.vm_name,
            self.provider.kind().name()
        );
        self.provider.start(&mut self.state)?;
        write_state(store, self.image, &self.state)?;

        println!("waiting for the guest to answer on SSH");
        let elapsed = self.provider.wait_ssh(&self.state, BOOT_TIMEOUT)?;
        println!("  SSH answered after {:.0}s", elapsed.as_secs_f64());

        println!("{}", readiness_wait_line(self.image));
        let elapsed = job::wait_for_session(
            self.provider.as_ref(),
            &self.state,
            self.target(),
            SESSION_TIMEOUT,
        )?;
        println!("{}", readiness_ready_line(self.image, elapsed));
        if self.image == Image::Linux {
            println!("{}", self.check_login()?);
        }
        // Only a Linux guest can have a second screen, and only a Linux guest
        // has the xrandr the placement is written in, so the target is asked
        // rather than inferred from the count.
        let screens = self.state.screen_count();
        if screens > 1 && self.target() == Target::Linux {
            println!(
                "{}",
                place_screens(self.provider.as_ref(), &self.state, screens)
            );
        }
        Ok(())
    }

    /// Whether the session that came up is the one this guest was asked for.
    ///
    /// Read from the `session.env` the session wrote before its ready marker,
    /// which carries `XDG_SESSION_TYPE` and `XDG_CURRENT_DESKTOP` as the session
    /// itself set them. A boot that named nothing is held to the image default,
    /// which is as true there and costs the same.
    fn check_login(&self) -> Result<String, String> {
        let expected = self.state.login().unwrap_or(Login::IMAGE_DEFAULT);
        let session_env = self
            .provider
            .exec(
                &self.state,
                &format!("cat {}/session.env", provider::GUEST_ROOT_LINUX),
            )
            .map(|out| out.stdout)
            .unwrap_or_default();
        expected.check_session_env(&session_env)?;
        Ok(format!("  the session is {}", expected.label()))
    }

    /// How to reach and get rid of this guest, for when something has gone
    /// wrong and it is still running.
    pub fn reach_hint(&self) -> String {
        let image = self.image;
        format!(
            "  ssh:     cargo xtask vm ssh {image}\n  \
             {console}: cargo xtask vm view {image}\n  \
             down:    cargo xtask vm down {image}",
            console = image.console_label()
        )
    }
}

/// Everything in the store that belongs to the guest a session owns, which is
/// what a teardown removes and the whole of what it removes.
///
/// The overlay comes from the record rather than from the image, because the two
/// providers give it different names and the record is what says which one this
/// guest got. The other three are all the same case: a boot writes them, nothing
/// reads them once the guest that took a copy is gone, and a green run that left
/// one behind left run state under an image with nothing running. The job
/// scratch is the script a command copied into the guest, the hand-over scratch
/// is the Windows launcher staging writes for every guest it stages, and the
/// firmware variables are the per-VM copy a QEMU boot makes, which is every
/// Linux guest and a Windows one under the QEMU cell of the provider matrix.
/// `vm.log` is deliberately not in the list: a failed boot's message quotes its
/// tail and names its path, and deleting it here would make that path a lie.
pub fn run_state_paths(store: &Store, image: Image, state: &RunState) -> [std::path::PathBuf; 5] {
    [
        store.state_file(image),
        state.overlay.clone(),
        store.job_scratch(image),
        store.handover_scratch(image),
        store.firmware_vars(image),
    ]
}

/// Remove them, whether each is a file or a directory, and report what would not
/// go. A path that is not there is not a problem: a teardown of a guest that ran
/// no job is the ordinary case.
fn remove_run_state(paths: &[std::path::PathBuf]) -> Vec<String> {
    let mut problems = Vec::new();
    for path in paths {
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        if let Err(e) = removed
            && e.kind() != std::io::ErrorKind::NotFound
        {
            problems.push(format!("{}: {e}", path.display()));
        }
    }
    problems
}

/// What the wait after SSH is waiting for, per image.
///
/// One marker, and what writes it differs: an image with a session writes it
/// from the session that logs on, and the Linux builder from a oneshot unit at
/// boot, because it has no session to write it from and no X server to have one
/// in. Naming a desktop there describes an image nobody built, and a wait that
/// returns in no time at all then reads as a broken guest rather than as the
/// marker being in place before SSH was. The Windows builder is on the other
/// side of that line and says so: it is a differencing child of the desktop
/// image, the marker comes from the logon it inherited, and the six seconds a
/// boot of it spends here are six seconds of a session starting.
pub fn readiness_wait_line(image: Image) -> &'static str {
    if image.has_desktop() {
        "waiting for the desktop session"
    } else {
        "waiting for the guest to be ready for a job"
    }
}

/// The line that closes that wait, with what it cost.
pub fn readiness_ready_line(image: Image, elapsed: Duration) -> String {
    format!(
        "  {} was ready after {:.0}s",
        if image.has_desktop() {
            "the desktop"
        } else {
            "the guest"
        },
        elapsed.as_secs_f64()
    )
}

/// Deal with a guest after something went wrong with it.
///
/// A failure after the boot used to leave a VM running with no message and no
/// hint, which is the worst of both: it holds its memory, it blocks the next
/// run's ports, and nothing said it was there. Either it goes, or it is named
/// along with the command that removes it.
///
/// A guest kept here is kept for the same reason a guest kept after a green run
/// is, so its record goes through [`record_kept`] too: the work it was booted
/// for is over, and a record that still names a run or a build in progress makes
/// `vm down` warn about ending something that stopped when the failure did.
pub fn after_failure(session: &mut Session, store: &Store, keep: bool) -> String {
    if keep {
        record_kept(session, store);
        return format!(
            "{} is still running, because --keep was given.\n{}",
            session.state.vm_name,
            session.reach_hint()
        );
    }
    match session.tear_down(store) {
        Ok(()) => format!("{} was destroyed.", session.state.vm_name),
        Err(e) => format!(
            "{} could not be destroyed ({e}), and is still running.\n{}",
            session.state.vm_name,
            session.reach_hint()
        ),
    }
}

/// The help text an expired evaluation image gets (plan decision 5).
///
/// Expiry is not a hard refusal anywhere else, and it is not one here either:
/// it stops before booting and says how to proceed anyway, because the failure
/// it prevents is flakiness rather than an error.
pub fn expired_help(image: Image, state: EvalState) -> String {
    format!(
        "The {image} golden image's evaluation has expired.\n\n{}\n\n\
         An expired evaluation does not refuse to boot. Windows starts shutting \
         itself down about once an hour, so a run inside it fails in the middle \
         of whatever it was doing rather than failing cleanly. That is why this \
         is checked before booting instead of diagnosed afterwards.\n\n\
         Rebuild it:  cargo xtask vm build-image {image}\n\
         Boot anyway: add --allow-expired-image",
        state.summary()
    )
}

/// The help text a layer whose parent moved gets (plan decision 3).
///
/// Named separately from the other refusals because this is the one that has to
/// say what a layer is: the disk is intact, its own checksum matches, and it is
/// still unreadable, which reads as a bug in the check unless the text explains
/// that a differencing child holds only what its own build changed.
pub fn detached_help(image: Image, detail: &str) -> String {
    format!(
        "the {image} layer is detached from its parent: {detail}.\n\n\
         A layer holds only what its own build changed, so it cannot be read \
         without the disk it was built over. Hyper-V refuses to attach a child \
         whose parent's identifier moved, and qcow2 reads garbage without \
         saying so, which is why this is checked here rather than left to the \
         hypervisor. Nothing can be repaired in place: the blocks the parent \
         used to hold are the ones that are gone.\n\n\
         Rebuild it over the parent that is there now: \
         cargo xtask vm build-image {image}"
    )
}

/// The command that leaves a guest of this image behind to look at.
///
/// Both of them take a target rather than an image, because each picks the image
/// it needs itself, and which of the two it is follows from what the image is
/// for: the e2e suite runs in a desktop image and a release build in a builder.
/// `e2e --target windows-builder` is not something anyone can type.
///
/// `dist` needs `--no-verify` on top of the flag, because
/// [`dist::keeps_guest`](crate::commands::dist::keeps_guest) keeps the last
/// guest of a run and one guest only: a run that verifies boots the desktop
/// image after the builder, so `dist --target linux --keep` hands back a Debian
/// desktop with no source tree in it. Dropping the verification makes the
/// builder the last guest, which is the one this text is offering.
pub fn keep_command(image: Image) -> String {
    if image.is_builder() {
        format!(
            "cargo xtask dist --target {} --keep --no-verify",
            image.target()
        )
    } else {
        format!("cargo xtask e2e --target {} --keep", image.target())
    }
}

/// Refuse to boot from an image that cannot produce a trustworthy run.
pub fn check_image(store: &Store, image: Image, allow_expired: bool) -> Result<(), String> {
    let inventory = inventory::scan(store);
    let Some(entry) = inventory.for_image(image) else {
        return Err(format!("nothing is known about the {image} image"));
    };
    let condition = entry.condition(util::now_unix());
    if !condition.blocks_boot() {
        // Every non-blocking condition that is not simply "fine" says so.
        // Passing in silence is what let an image whose age nobody could read
        // boot as though it had been checked.
        match &condition {
            ImageCondition::Ok => {}
            ImageCondition::Stale { .. } => {
                println!(
                    "warning: the {image} image is stale: {}",
                    condition.detail()
                );
                println!(
                    "it still runs; `cargo xtask vm build-image {image}` brings it up to date"
                );
            }
            other => println!(
                "warning: the {image} image is {}: {}",
                other.label(),
                other.detail()
            ),
        }
        return Ok(());
    }
    match condition {
        ImageCondition::Expired { state } if allow_expired => {
            println!("warning: {}", state.summary());
            println!("proceeding because --allow-expired-image was given");
            Ok(())
        }
        ImageCondition::Expired { state } => Err(expired_help(image, state)),
        ImageCondition::Missing => Err(format!(
            "no {image} golden image yet. `cargo xtask vm build-image {image}` builds one."
        )),
        ImageCondition::Unmanifested { detail, .. } => Err(format!(
            "the {image} image has no usable manifest: {detail}.\n\n\
             The manifest is where the build timestamp lives, and that is the \
             only record of when the evaluation licence started running. \
             Without it there is no telling an image with two months left from \
             one that will start shutting itself down mid-run, which is the \
             failure this check exists to prevent.\n\n\
             Rebuild it: cargo xtask vm build-image {image}"
        )),
        ImageCondition::Corrupt { detail } => Err(format!(
            "the {image} golden image does not match its manifest: {detail}. \
             `cargo xtask vm build-image {image}` rebuilds it."
        )),
        ImageCondition::Detached { detail } => Err(detached_help(image, &detail)),
        // Everything else returned above, where `blocks_boot` said so.
        other => Err(format!(
            "the {image} image is not usable: {}",
            other.detail()
        )),
    }
}

/// Refuse to start a second VM (plan decision 7: one at a time), unless one of
/// the two is a builder.
///
/// What the rule protects is a host oversubscribed by two guests each sized for
/// the whole of it, which is a number rather than a principle, so the exemption
/// comes with the number: a boot that proceeds beside another guest names it and
/// says what the two hold together. Two desktop guests are still refused,
/// because that is the pair the rule was written about.
pub fn check_no_other_vm(runner: &dyn Runner, store: &Store, image: Image) -> Result<(), String> {
    let inventory = inventory::scan(store);
    for entry in &inventory.images {
        let Some(state) = &entry.state else { continue };
        let Some(other) = entry.image else { continue };
        if other == image {
            continue;
        }
        let running = provider::for_state(runner, store, state)
            .is_ok_and(|provider| provider.is_running(state));
        if !running {
            continue;
        }
        if may_run_beside(image, other) {
            println!("{}", beside_line(image, other));
            continue;
        }
        let cost = state
            .cost_of_ending()
            .map_or_else(String::new, |cost| format!(", which {cost}"));
        return Err(format!(
            "{} is already running, and this phase runs one VM at a time \
             (plan decision 7: every guest assumes the whole host's memory \
             and cores).\n\
             `cargo xtask vm down {other}` frees it{cost}.",
            state.vm_name
        ));
    }
    Ok(())
}

/// Whether these two images may hold guests at the same time.
///
/// A builder compiles for a guest under test and is not one, so the pair is a
/// compiler beside the thing it compiles for: that is the arrangement a Windows
/// host has had since WSL, where the compiler is a persistent environment beside
/// a guest that is not. Two desktop guests are the pair that oversubscribes a
/// host, and neither of them is holding anything the other needs.
pub fn may_run_beside(booting: Image, other: Image) -> bool {
    booting.is_builder() || other.is_builder()
}

/// The one line a boot prints when it is not the only guest on this host.
///
/// The figures rather than a reassurance, because the host this was measured on
/// is not every host: what makes two guests fine here is 60 GiB, and a smaller
/// machine is the case that breaks. Both come from
/// [`provider::resources_for`](crate::provider::resources_for), so the line
/// cannot drift from what the two guests are given.
pub fn beside_line(booting: Image, other: Image) -> String {
    let total =
        u64::from(provider::resources_for(booting).0) + u64::from(provider::resources_for(other).0);
    format!(
        "{} is up as well, which a builder may be: the two hold {} of this host's \
         memory between them",
        other.vm_name(),
        util::format_bytes(total * 1024 * 1024)
    )
}

/// Save the state file. Called as soon as the VM exists, so that a crash from
/// here on still leaves something `vm status` can see and `vm down` can
/// clean up.
pub fn write_state(store: &Store, image: Image, state: &RunState) -> Result<(), String> {
    let path = store.state_file(image);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, state.to_json())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Read the state file, if there is one for a VM of ours.
pub fn load_state(store: &Store, image: Image) -> Option<RunState> {
    let text = std::fs::read_to_string(store.state_file(image)).ok()?;
    RunState::from_json(&text).ok().filter(RunState::is_ours)
}

/// Which desktop a guest may be asked to boot into, and whether it may be
/// asked at all.
///
/// Only the Debian 13 image carries more than one, so the flag is refused for
/// every other image rather than ignored: a run whose `--desktop` did nothing is
/// a run whose results are about a desktop nobody chose. The Linux builder is
/// the sharper case, because it has no session at all to choose one for.
pub fn desktop_for(image: Image, requested: Option<Desktop>) -> Result<Option<Desktop>, String> {
    match (image, requested) {
        (Image::Linux, chosen) => Ok(chosen),
        (_, None) => Ok(None),
        (_, Some(desktop)) if image.has_desktop() => Err(format!(
            "--desktop {desktop} is a Debian 13 guest option; the {image} image has \
             one desktop and no way to choose another"
        )),
        (_, Some(desktop)) => Err(format!(
            "--desktop {desktop} asks for a session the {image} image does not have: \
             it carries no desktop at all, which is what keeps it small and what \
             keeps a compiler out of the images the suite runs in"
        )),
    }
}

/// Which session a guest may be asked to log into, and whether it may be asked
/// at all.
///
/// A session type with no desktop is the image default's desktop, KDE, on that
/// type. The refusals off the Linux image mirror [`desktop_for`]'s, for the
/// same reason.
pub fn login_for(
    image: Image,
    desktop: Option<Desktop>,
    session_type: Option<SessionType>,
) -> Result<Option<Login>, String> {
    let desktop = desktop_for(image, desktop)?;
    match (image, session_type) {
        (Image::Linux, None) => desktop.map(|d| Login::new(d, SessionType::X11)).transpose(),
        (Image::Linux, Some(session_type)) => Login::new(
            desktop.unwrap_or(Login::IMAGE_DEFAULT.desktop()),
            session_type,
        )
        .map(Some),
        (_, None) => Ok(None),
        (_, Some(session_type)) if image.has_desktop() => Err(format!(
            "--session-type {session_type} is a Debian 13 guest option; the {image}              image has one session and no way to choose another"
        )),
        (_, Some(session_type)) => Err(format!(
            "--session-type {session_type} asks for a session the {image} image does              not have: it carries no desktop at all"
        )),
    }
}

/// What a boot is asked for beyond the image itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootRequest {
    pub desktop: Option<Desktop>,
    pub session_type: Option<SessionType>,
    pub screens: u16,
}

impl BootRequest {
    /// The image as it was built: its own default session on one screen.
    pub const PLAIN: Self = Self {
        desktop: None,
        session_type: None,
        screens: 1,
    };

    /// The request checked against the image: the session to name, if any, and
    /// how many screens.
    ///
    /// More than one screen is refused under Wayland because both halves of
    /// placing them are X11 commands: `xrandr` moves the outputs and `xinput`
    /// maps the pointer, and neither reaches a Wayland compositor's outputs.
    pub fn check(self, image: Image) -> Result<(Option<Login>, u16), String> {
        let login = login_for(image, self.desktop, self.session_type)?;
        let screens = screens_for(image, self.screens)?;
        if screens > 1
            && let Some(login) = login.filter(|l| l.session_type() == SessionType::Wayland)
        {
            return Err(format!(
                "--screens {screens} with --session-type wayland asks for a layout                  nothing here can make: the screens are placed with xrandr and the                  pointer mapped with xinput, and neither can move {}'s outputs.                  --session-type x11 gives {screens} screens, and a Wayland session                  runs on one",
                login.label()
            ));
        }
        Ok((login, screens))
    }
}

/// The most screens a guest may be asked for.
///
/// Not virtio-gpu's limit, which is sixteen scanouts: four is more than any
/// layout worth testing, and every screen past the first costs a VNC server, a
/// loopback port, and a share of a software renderer that is already the
/// slowest thing in the guest.
pub const MAX_SCREENS: u16 = 4;

/// How many screens a guest may be asked for, and whether it may be asked at
/// all.
///
/// One is never refused, because one is what every guest already has and a flag
/// that changes nothing is not worth a refusal. More than one is the Debian 13
/// guest's alone. The Windows guest cannot have it: its adapter has a single
/// head under either hypervisor, `Set-VMVideo` has no monitor count, and
/// `Hyper-V`'s multi-monitor path is an enhanced session that takes its monitors
/// from the host's own, so a second screen there is a display driver inside the
/// guest rather than a flag out here. The builders have no session to put a
/// second screen in front of.
pub fn screens_for(image: Image, requested: u16) -> Result<u16, String> {
    if requested == 0 {
        return Err("--screens 0 asks for a guest with no console at all; the \
                    fewest screens a guest can have is 1"
            .to_owned());
    }
    if requested == 1 {
        return Ok(1);
    }
    match image {
        Image::Linux if requested <= MAX_SCREENS => Ok(requested),
        Image::Linux => Err(format!(
            "--screens {requested} is more than the {MAX_SCREENS} a guest may be \
             asked for; every screen past the first is another VNC server and \
             another share of a software renderer"
        )),
        Image::Windows | Image::WindowsBuilder => Err(format!(
            "--screens {requested} asks the {image} image for something its video \
             adapter does not have: one head, whichever hypervisor is holding it. \
             A second screen in a Windows guest is an indirect display driver \
             installed inside it, not a flag out here"
        )),
        Image::LinuxBuilder => Err(format!(
            "--screens {requested} asks for screens the {image} image has no \
             session to put in front of: it carries no desktop at all, which is \
             what keeps it small"
        )),
    }
}

/// Boot a pristine overlay and wait for it to be usable.
pub fn boot<'a>(
    runner: &'a dyn Runner,
    store: &'a Store,
    image: Image,
    reason: StartReason,
    allow_expired: bool,
    request: BootRequest,
) -> Result<Session<'a>, String> {
    let (login, screens) = request.check(image)?;
    check_image(store, image, allow_expired)?;
    check_no_other_vm(runner, store, image)?;
    clear_stale_state(runner, store, image)?;
    // Whatever the last guest at this address was, its host key is not this
    // guest's: the Linux image generates a fresh set on every boot, and under
    // QEMU both guests answer on the same loopback port. An entry left behind is
    // a remote-host-identification-changed banner on every connection of this
    // run, which is noise that reads like a break-in.
    crate::guest::ssh::forget_host_keys(store);

    let provider = provider::for_image(runner, store, image)?;
    println!("creating a throwaway overlay of the {image} golden image");
    let mut state = provider.create_from_golden(image, reason)?;
    // Recorded before the VM is started, because the provider builds the guest's
    // fw_cfg argument, and its consoles, out of the record rather than out of a
    // parameter. One screen is recorded as no answer, which is what every record
    // written before a guest could have two already carries.
    state.desktop = login.map(|l| l.desktop().flag().to_owned());
    state.session_type = login
        .map(Login::session_type)
        .filter(|t| *t != SessionType::X11)
        .map(|t| t.flag().to_owned());
    state.screens = (screens > 1).then_some(screens);

    // From here the VM exists: for Hyper-V it is registered, for QEMU its
    // overlay is on disk. Everything after this point goes through
    // `after_failure`, so no failure can return while leaving one running.
    let mut session = Session {
        provider,
        state,
        image,
    };
    match session.bring_up(store) {
        Ok(()) => Ok(session),
        Err(e) => {
            println!("{}", after_failure(&mut session, store, false));
            Err(e)
        }
    }
}

/// Take a session over a guest that is already up, without touching it.
///
/// What a build reusing a running builder needs: the record is on disk, the
/// guest is answering, and the only thing missing is the provider that goes with
/// it. Nothing here checks that SSH answers, because the first thing every
/// caller does is ask the guest a question and that answer is the check.
pub fn adopt<'a>(
    runner: &'a dyn Runner,
    store: &'a Store,
    image: Image,
    state: RunState,
) -> Result<Session<'a>, String> {
    let provider = provider::for_state(runner, store, &state)?;
    Ok(Session {
        provider,
        state,
        image,
    })
}

/// Resume a stopped guest: the same overlay, a fresh address, and the wait a
/// boot performs.
///
/// The overlay is deliberately not recreated, which is the whole difference from
/// [`boot`]: what a stopped builder holds is a cargo build directory, and it is
/// the reason the guest was kept rather than destroyed. The address is picked
/// again because a builder may have been stopped for hours, and decision 1 lets
/// other guests take its ports while it was down.
pub fn resume<'a>(
    runner: &'a dyn Runner,
    store: &'a Store,
    image: Image,
    mut state: RunState,
) -> Result<Session<'a>, String> {
    check_no_other_vm(runner, store, image)?;
    // The host key at the port this guest is about to take belongs to whatever
    // was there last, which is the same reason `boot` forgets them.
    crate::guest::ssh::forget_host_keys(store);

    let provider = provider::for_state(runner, store, &state)?;
    provider.readdress(&mut state)?;
    state.stopped = false;
    let mut session = Session {
        provider,
        state,
        image,
    };
    match session.bring_up(store) {
        Ok(()) => Ok(session),
        Err(e) => {
            println!("{}", after_failed_resume(&mut session, store));
            Err(e)
        }
    }
}

/// What a failed resume does, which is nothing destructive.
///
/// [`boot`] tears its guest down on the way out, which is right for an overlay
/// it created three lines earlier and wrong for one it was called to preserve.
/// The failure this exists for is the one the risk section names: decision 2's
/// kill left the guest's filesystem unclean, the resumed guest is running a
/// repair pass, and it has not answered inside [`BOOT_TIMEOUT`]. Powering that
/// off and unlinking the disk throws away the build directory the guest was kept
/// for and starts the repair over, so the guest is left as the failure found it
/// and the message says how to look at it, put it back, or give it up. Deleting
/// a builder's overlay takes the person asking for it.
///
/// The record is the one thing this writes, and only for a guest that is not
/// running: a resume can also fail before the guest exists, at a port the
/// provider could not bind, and a record left saying `stopped: false` would
/// have `vm status` call a resumable builder a crashed one and point at
/// `vm down`, which is the deletion this whole path is avoiding.
fn after_failed_resume(session: &mut Session, store: &Store) -> String {
    let image = session.image;
    let vm_name = session.state.vm_name.clone();
    if session.provider.is_running(&session.state) {
        return resume_left_running(image, &vm_name);
    }
    session.state.stopped = true;
    match write_state(store, image, &session.state) {
        Ok(()) => resume_left_stopped(image, &vm_name),
        Err(e) => format!(
            "{vm_name} did not come back and is not running. Its overlay is \
             untouched, but the record could not be put back to stopped ({e}), so \
             `cargo xtask vm status` will call it crashed. Mind that before \
             running `cargo xtask vm down {image}`, which is what deletes the \
             build directory."
        ),
    }
}

/// A resume whose guest is up and did not answer.
fn resume_left_running(image: Image, vm_name: &str) -> String {
    format!(
        "{vm_name} did not answer, and nothing in it was deleted: it is still \
         running and its overlay is untouched, build directory and all. A resume \
         that takes longer than a cold boot is usually a guest still doing \
         something to itself.\n  \
         ssh:     cargo xtask vm ssh {image}\n  \
         {console}: cargo xtask vm view {image}\n  \
         stop:    cargo xtask vm stop {image} puts it back, and `cargo xtask vm \
         start {image}` tries again\n  \
         down:    cargo xtask vm down {image} gives it up, and the build directory with it",
        console = image.console_label()
    )
}

/// A resume whose guest never came up, put back the way it was found.
fn resume_left_stopped(image: Image, vm_name: &str) -> String {
    format!(
        "{vm_name} did not come up, and it is recorded as stopped again with its \
         overlay untouched. `cargo xtask vm start {image}` tries again, \
         `cargo xtask vm down {image}` gives it up and frees the build directory \
         with it."
    )
}

/// Why `vm start` and `vm stop` are for the builder images only (decision 5).
///
/// The refusal quotes the reason rather than hiding behind "unsupported": every
/// boot of a desktop guest is a pristine overlay, and that is what makes a
/// result from one worth having. A resumed overlay is not pristine.
pub fn persistence_refusal(image: Image) -> String {
    format!(
        "the {image} image is one the e2e suite runs in, and every guest of one is \
         a pristine overlay: that is what makes a result from it worth having, and \
         a guest resumed from where it was left is not that. It is the same \
         argument that keeps a compiler out of this image.\n\
         `cargo xtask vm down {image}` ends it and the next `cargo xtask vm up \
         {image}` boots a clean one. The builders are what `vm stop` and \
         `vm start` are for, because what a builder keeps is a build directory."
    )
}

/// `vm stop`: end a builder guest and keep everything in it.
pub fn stop(runner: &dyn Runner, image: Image) -> Result<u8, String> {
    if !image.is_builder() {
        return Err(persistence_refusal(image));
    }
    let store = store::store()?;
    let mut state = load_state(&store, image).ok_or_else(|| no_record(image))?;
    let provider = provider::for_state(runner, &store, &state)?;

    let stopped = provider.stop(&state)?;
    state.stopped = true;
    write_state(&store, image, &state)?;
    println!(
        "{}",
        stopped_line(
            image,
            &state.vm_name,
            stopped,
            std::fs::metadata(&state.overlay).ok().map(|m| m.len())
        )
    );
    Ok(0)
}

/// `vm start`: resume a stopped builder guest.
pub fn start(runner: &dyn Runner, image: Image) -> Result<u8, String> {
    if !image.is_builder() {
        return Err(persistence_refusal(image));
    }
    let store = store::store()?;
    let state = load_state(&store, image).ok_or_else(|| no_record(image))?;
    let provider = provider::for_state(runner, &store, &state)?;
    if provider.is_running(&state) {
        println!(
            "{} is already running; `cargo xtask vm ssh {image}` is a shell in it",
            state.vm_name
        );
        return Ok(0);
    }
    let session = resume(runner, &store, image, state)?;
    println!(
        "{} is up again, with what it was holding; `cargo xtask vm stop {image}` \
         puts it back",
        session.state.vm_name
    );
    Ok(0)
}

/// What `vm stop` and `vm start` say when there is nothing recorded to act on.
fn no_record(image: Image) -> String {
    format!(
        "no {image} VM is recorded, so there is nothing to stop or resume. \
         `cargo xtask vm up {image}` boots one."
    )
}

/// The one line a stop prints.
///
/// It says what the stop kept as well as what it freed, because a kept overlay
/// is gigabytes and the command that reclaims it is the one thing a person who
/// never runs it will wish they had been told.
fn stopped_line(
    image: Image,
    vm_name: &str,
    stopped: provider::Stopped,
    overlay_bytes: Option<u64>,
) -> String {
    let overlay = overlay_bytes.map_or_else(
        || "its overlay stays".to_owned(),
        |bytes| format!("its overlay stays, {} of it", util::format_bytes(bytes)),
    );
    let what = match stopped {
        provider::Stopped::ShutDown => "is stopped: the memory is back and",
        // Worth its own sentence rather than a shrug: the overlay this kept is
        // the reason the command exists, and the next start of it opens with a
        // repair pass that nobody asked for and nothing else would explain.
        provider::Stopped::Killed => "would not shut down and was killed: the memory is back and",
        provider::Stopped::WasNotRunning => "was not running; it is recorded as stopped and",
    };
    let repair = if stopped == provider::Stopped::Killed {
        " Its filesystem was not closed, so the next start begins with a repair pass."
    } else {
        ""
    };
    format!(
        "{vm_name} {what} {overlay}.{repair} \
         `cargo xtask vm start {image}` resumes it, `cargo xtask vm down {image}` \
         frees it."
    )
}

/// What to say about a guest that is recorded and not running.
///
/// The two cases want opposite advice, which is why the record says which is
/// which: a stopped builder is resumed, and a crashed guest is cleared away.
fn not_running_hint(image: Image, state: &RunState) -> String {
    if state.stopped {
        format!(
            "{} is stopped. `cargo xtask vm start {image}` resumes it with what \
             it was holding.",
            state.vm_name
        )
    } else {
        format!(
            "{} is recorded but not running. `cargo xtask vm down {image}` clears \
             it and `cargo xtask vm up {image}` starts a fresh one.",
            state.vm_name
        )
    }
}

/// One connected output of a Linux guest, as `xrandr --query` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub name: String,
    /// `1920x1080+1920+0`, or empty for a connected output with no mode
    /// assigned, which is a screen the session has not put anything on.
    pub geometry: String,
}

impl Screen {
    /// Where this screen's top left corner is, as xrandr wrote it.
    fn origin(&self) -> Option<&str> {
        let (_, offsets) = self.geometry.split_once('+')?;
        Some(offsets)
    }
}

/// The command that lays a session's outputs out in a row and prints the result.
///
/// X's own configuration leaves every connected output at the origin, which is
/// one screen shown twice rather than two screens side by side, so the session
/// has to be told. Told from here rather than from the image, because a line in
/// the image costs a rebuild and because a guest with one screen must not be
/// touched at all.
///
/// The session's environment comes from the file the guest contract writes at
/// login, which is the same file every job sources: an SSH command starts with
/// no `DISPLAY` and no X credentials of its own, and `xrandr` without those is
/// a command that fails for a reason that has nothing to do with the screens.
///
/// No `set -e`: an output that refuses its mode must not stop the ones after it,
/// and the answer to what happened is the `xrandr --query` at the end rather
/// than any exit code. `/ connected/` matches the connected outputs and not the
/// disconnected ones, whose word has no space in front of `connected`.
pub fn place_screens_command() -> String {
    format!(
        "set -a; . {root}/session.env 2>/dev/null; set +a; \
         export DISPLAY=\"${{DISPLAY:-:0}}\"; \
         prev=; \
         for out in $(xrandr --query | awk '/ connected/ {{print $1}}'); do \
         if [ -z \"$prev\" ]; then xrandr --output \"$out\" --auto --primary; \
         else xrandr --output \"$out\" --auto --right-of \"$prev\"; fi; \
         prev=\"$out\"; \
         done; \
         xrandr --query",
        root = crate::provider::GUEST_ROOT_LINUX,
    )
}

/// The marker the pointer mapping prints, followed by the output it mapped to
/// or `none`.
pub const POINTER_MARK: &str = "SUNLIT_POINTER=";

/// The command that confines the guest's tablet to one screen, and says which.
///
/// A guest with two screens has one absolute pointer for a desktop twice its
/// width, so a click lands at twice the x it was aimed at. X gives an absolute
/// device the whole screen until something maps it to one output, and this is
/// that something: the first screen becomes exact, and the others become
/// something to look at. Both halves of a per-screen pointer are missing from
/// QEMU rather than from here, which `qemu::pointer_args` records.
///
/// The device is found by name out of `xinput list` rather than by
/// `xinput list --id-only`, which prints nothing at all when two devices share a
/// name, and the mapping goes to the output xrandr calls primary.
///
/// `xinput` is not in every image: a guest without it prints `none` and the boot
/// says what that costs rather than failing, so an older image still boots and
/// still shows two screens.
pub fn map_pointer_command() -> String {
    format!(
        "set -a; . {root}/session.env 2>/dev/null; set +a; \
         export DISPLAY=\"${{DISPLAY:-:0}}\"; \
         out=$(xrandr --query | awk '/ connected primary/ {{print $1; exit}}'); \
         [ -n \"$out\" ] || out=$(xrandr --query | awk '/ connected/ {{print $1; exit}}'); \
         id=$(xinput list 2>/dev/null | \
         sed -n 's/.*QEMU Virtio Tablet.*id=\\([0-9][0-9]*\\).*/\\1/p' | head -1); \
         if [ -n \"$id\" ] && [ -n \"$out\" ] && xinput --map-to-output \"$id\" \"$out\"; then \
         echo \"{POINTER_MARK}$out\"; else echo \"{POINTER_MARK}none\"; fi",
        root = crate::provider::GUEST_ROOT_LINUX,
    )
}

/// What the mapping did, as a line to print.
pub fn pointer_report(stdout: &str) -> String {
    let mapped = stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix(POINTER_MARK))
        .unwrap_or("none");
    if mapped == "none" {
        return "warning: the pointer covers the whole desktop, so a click lands \
                at twice the x it was aimed at. Mapping it to one screen needs \
                `xinput`, which this image does not carry."
            .to_owned();
    }
    format!(
        "pointer: mapped to {mapped}, so that screen's window clicks where it \
         points. The other screens are for looking at: QEMU sends every screen's \
         pointer to the same device, so their windows drive {mapped} too."
    )
}

/// The connected outputs in `xrandr --query` output, in the order it listed
/// them.
///
/// `connected` as its own word, so `disconnected` is not one of them, and the
/// geometry is the first token shaped like one, so the word `primary` between
/// the two changes nothing and the free text after it is not mistaken for more
/// of it.
pub fn parse_screens(text: &str) -> Vec<Screen> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let name = parts.next()?;
            if parts.next()? != "connected" {
                return None;
            }
            let geometry = parts.find(|token| is_geometry(token)).unwrap_or_default();
            Some(Screen {
                name: name.to_owned(),
                geometry: geometry.to_owned(),
            })
        })
        .collect()
}

/// `1920x1080+0+0`, and not the mode list's `1920x1080` or anything with words
/// in it.
fn is_geometry(token: &str) -> bool {
    let Some((size, offsets)) = token.split_once('+') else {
        return false;
    };
    let sized = size.split_once('x').is_some_and(|(w, h)| {
        !w.is_empty()
            && w.chars().all(|c| c.is_ascii_digit())
            && !h.is_empty()
            && h.chars().all(|c| c.is_ascii_digit())
    });
    sized && offsets.contains('+')
}

/// What the session has, and what is wrong with it if anything is.
///
/// A count is not evidence: two screens stacked on one origin are what X does
/// on its own, and they read as two everywhere except on the console, so the
/// origins are checked as well as the number. Neither failure ends the boot. The
/// guest is up either way and looking at it is how the reason gets found.
pub fn screen_report(screens: &[Screen], asked: u16) -> String {
    let mut report = if screens.is_empty() {
        "screens: the session reports no connected output".to_owned()
    } else {
        let listed: Vec<String> = screens
            .iter()
            .map(|screen| {
                let geometry = if screen.geometry.is_empty() {
                    "no mode"
                } else {
                    &screen.geometry
                };
                format!("{} {geometry}", screen.name)
            })
            .collect();
        format!("screens: {}", listed.join(", "))
    };
    if screens.len() < usize::from(asked) {
        let _ = write!(
            report,
            "\nwarning: {asked} screens were asked for and the session has {}. \
             The guest is up; `cargo xtask vm ssh linux \"xrandr --query\"` is \
             what it sees.",
            screens.len()
        );
        return report;
    }
    let origins: Vec<&str> = screens.iter().filter_map(Screen::origin).collect();
    let stacked = origins
        .iter()
        .enumerate()
        .any(|(index, origin)| origins[index + 1..].contains(origin));
    if stacked {
        report.push_str(
            "\nwarning: two screens share an origin, so they are one picture \
             shown twice rather than a desktop across both. Placing them again \
             by hand is `cargo xtask vm ssh linux \"xrandr --output <b> \
             --right-of <a>\"`.",
        );
    }
    report
}

/// Place the guest's screens and report what came of it.
fn place_screens(provider: &dyn Provider, state: &RunState, asked: u16) -> String {
    let mut report = match provider.exec(state, &place_screens_command()) {
        Ok(output) => screen_report(&parse_screens(&output.stdout), asked),
        Err(e) => return format!("warning: the guest's screens could not be placed: {e}"),
    };
    // After the placement and not before: the mapping is computed from where the
    // outputs are, so a pointer mapped to a screen that then moves is mapped to
    // where that screen used to be.
    let pointer = match provider.exec(state, &map_pointer_command()) {
        Ok(output) => pointer_report(&output.stdout),
        Err(e) => format!("warning: the guest's pointer could not be mapped: {e}"),
    };
    report.push('\n');
    report.push_str(&pointer);
    report
}

/// Whether a recorded VM may be taken down to make room for a new one.
///
/// Everything else the xtask boots holds nothing worth keeping, which is the
/// whole of decision 14's lifecycle. An image build is the exception: it is tens
/// of minutes of install whose disk is not an image yet, so taking it down means
/// starting again from the media. A build that is recorded and no longer running
/// is a crashed one, and clearing that away is exactly what clearing is for.
pub fn may_clear(image: Image, reason: StartReason, running: bool) -> Result<(), String> {
    if reason == StartReason::Build && running {
        return Err(format!(
            "an image build is running in the {image} VM, and clearing it away \
             would throw away the install it is partway through.\n\
             Wait for it, or `cargo xtask vm down {image}` to end it deliberately."
        ));
    }
    Ok(())
}

/// What a boot says about the guest it is clearing away to make room.
///
/// A stopped builder is not a leftover: it is a build directory somebody kept,
/// and a boot that discards one in silence is how ten minutes of compile goes
/// missing. This boot wants a pristine overlay and takes it, because that is
/// what `vm up` and `dist` are for, so the line names what is going and the
/// command that would have kept it instead.
fn clearing_line(image: Image, state: &RunState) -> String {
    if state.stopped {
        format!(
            "discarding the stopped {image} guest with the build directory in it; \
             this boot wants a pristine overlay, and `cargo xtask vm start {image}` \
             is what resumes one instead"
        )
    } else {
        format!("clearing the {image} VM left behind by an earlier run")
    }
}

/// Take down whatever an earlier run left recorded for this image.
///
/// The record is removed only once the teardown has succeeded, which is the
/// same rule `teardown::execute` follows and for the same reason: deleting the
/// record of a VM that is still registered makes it invisible to `vm status`
/// and `vm down` for good. That is reachable on a host where the `Hyper-V`
/// cmdlets fail, which is a plain missing group membership away.
pub fn clear_stale_state(runner: &dyn Runner, store: &Store, image: Image) -> Result<(), String> {
    let Some(existing) = load_state(store, image) else {
        return Ok(());
    };

    let provider = provider::for_state(runner, store, &existing).map_err(|e| {
        format!(
            "{} is recorded for {image}, but {e}. The record is left in place \
             rather than deleted; `cargo xtask vm down {image}` clears it.",
            existing.vm_name
        )
    })?;

    may_clear(image, existing.reason, provider.is_running(&existing))?;

    // Said after the decision, not before it: a running build is refused here,
    // and announcing a clearing that is then declined describes something that
    // never happens.
    println!("{}", clearing_line(image, &existing));

    let session = Session {
        provider,
        state: existing,
        image,
    };
    session.tear_down(store).map_err(|e| {
        format!(
            "the {image} VM left behind by an earlier run could not be taken \
             down: {e}\nIts record is kept rather than deleted, because a VM \
             that is still registered and no longer recorded cannot be found \
             again. `cargo xtask vm down {image}` retries this."
        )
    })
}

/// Whether the current binaries went into the guest, and if not, whose answer
/// that is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Staging {
    /// Staging ran, so the binaries, the launcher and the desktop shortcuts
    /// went into the guest. Not that every part of it arrived: the shortcuts are
    /// convenience, so `artifacts::stage` warns about them and carries on, and
    /// that warning is on the line above this text rather than a command away.
    Done,
    /// Nothing was staged, and nothing about this host stopped it: the command
    /// that booted this guest had no binaries to put in it.
    Skipped,
    /// Nothing was staged and nothing could be, because what would compile the
    /// binaries is not there: a Linux host builds the Windows guest's in a
    /// builder guest, and that image may not have been built. The boot is still
    /// worth having: what is being looked at is the image.
    Impossible,
}

/// What was done to the guest this text is about.
///
/// Two of the three describe what happens at the end of a command rather than
/// at the boot: the binaries with their launcher and desktop shortcuts, and the
/// enhanced session. `vm up` and `e2e --keep` do both, `vm smoke --keep` does
/// neither, and a hand-over that failed did only the first. So the text is
/// printed from what happened rather than from the image, which is what it was
/// doing when it told the owner of an empty desktop which shortcut to
/// double-click. The third is the console, which is the hypervisor's and not
/// the guest's: it was reading that off the image too, and telling a Linux host
/// looking at a VNC framebuffer to cancel a `vmconnect` credential dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prepared {
    pub staged: Staging,
    /// Which hypervisor is showing this guest's console. It decides what the
    /// console is: `vmconnect` under Hyper-V, a VNC framebuffer under QEMU, and
    /// the same Windows image can be either.
    pub console: ProviderKind,
    /// The guest confirmed it can offer an enhanced `vmconnect` session.
    pub enhanced_session: bool,
}

impl Staging {
    /// What a boot that staged nothing should say about it.
    ///
    /// "Nothing was staged" reads as a property of the command on a host that
    /// could have staged something, and as a property of the host on one that
    /// could not: pointing a Linux host at `vm up windows` for the binaries is
    /// pointing it at a command that will not produce them either.
    pub fn skipped_for(store: &Store, host: HostOs, target: Target) -> Self {
        match crate::guest::artifacts::usable_builder(store, host, target) {
            Ok(_) => Self::Skipped,
            Err(_) => Self::Impossible,
        }
    }
}

impl Prepared {
    /// A guest left as it stood: nothing staged in it and nothing handed over.
    pub fn bare(store: &Store, console: ProviderKind, target: Target) -> Self {
        Self {
            staged: Staging::skipped_for(store, HostOs::current(), target),
            console,
            enhanced_session: false,
        }
    }
}

/// What `vm up` prints when it is done (plan decision 14).
///
/// `vm down` is the stop, and this text used to open by denying that a stop
/// existed at all, which argues with a reader whose mental model is right. What
/// is missing is a save or a pause, and what makes their absence cost nothing is
/// that a guest holds nothing worth saving. The memory figure is the guest's
/// own, because an idle guest holding gigabytes is the reason to take it down
/// rather than leave it up.
pub fn lifecycle_explainer(image: Image, prepared: Prepared) -> String {
    if image.is_builder() {
        return builder_explainer(image);
    }
    format!(
        "\n\
         {vm} is up.\n  \
         ssh:     cargo xtask vm ssh {image}\n  \
         {console}: cargo xtask vm view {image}\n  \
         down:    cargo xtask vm down {image}\n\n\
         `vm down` is the stop, and an idle guest is worth stopping: it holds \
         {memory} of this host's memory for as long as it is up. What there is no \
         way to do is save or pause one, and nothing in here is worth saving, so \
         ending a guest and discarding it are the same act: the teardown frees \
         the memory and the overlay, leaves the golden image untouched, and the \
         next `vm up` boots something pristine.\n\n\
         Watching a run is harmless; clicking during one perturbs it.{session}{extra}",
        vm = image.vm_name(),
        console = image.console_label(),
        memory = guest_memory(image),
        session = view_note(prepared.console, image, prepared.enhanced_session),
        extra = guest_environment_note(image, prepared.staged)
    )
}

/// What a builder guest's boot closes with (plan decision 9).
///
/// Shorter than a desktop guest's, because less of it is true here rather than
/// more. A builder has nothing staged in it, no shortcuts on a desktop, no
/// choice of console session to explain, nothing to paste into and no run in it
/// to perturb. What it has instead is a lifetime: it is the one kind of guest
/// that can be stopped and resumed, so its own two commands are in the list, and
/// the paragraph is about what a stop keeps rather than about a save that does
/// not exist.
fn builder_explainer(image: Image) -> String {
    format!(
        "\n\
         {vm} is up.\n  \
         ssh:     cargo xtask vm ssh {image}\n  \
         {console}: cargo xtask vm view {image}\n  \
         stop:    cargo xtask vm stop {image}\n  \
         start:   cargo xtask vm start {image}\n  \
         down:    cargo xtask vm down {image}\n\n\
         It holds {memory} of this host's memory while it runs, and `vm stop` \
         gives that back and keeps the overlay: what is in there is a cargo build \
         directory and a crate registry, which is what makes the next build in it \
         a link rather than a compile. `vm down` frees the overlay with it, and \
         the build after that starts from nothing.",
        vm = image.vm_name(),
        console = image.console_label(),
        memory = guest_memory(image),
    )
}

/// How much of the host a guest of this image holds while it is up.
///
/// Taken from the QEMU launch parameters, which both providers agree with:
/// `provider::hyperv::MEMORY_BYTES` is the same 6 GiB a Windows guest gets
/// there, and `the_memory_a_guest_is_said_to_hold_is_the_memory_it_gets` pins
/// that, because a figure printed to argue for a teardown has to be the real
/// one.
fn guest_memory(image: Image) -> String {
    let mib = u64::from(crate::provider::resources_for(image).0);
    util::format_bytes(mib * 1024 * 1024)
}

/// How to look at this guest, per hypervisor and per what it is offering.
///
/// The two consoles are not the same thing to sit in front of. `vmconnect` can
/// open a session that resizes, at the cost of a dialog to dismiss, and this is
/// the moment that dialog was made dismissable, but only for a guest that was
/// handed over: any other one gets the basic session, which is fixed at the
/// console resolution and asks for nothing. A VNC viewer on a QEMU guest has no
/// such choice to explain. The clipboard differs the same way, because an
/// enhanced session is RDP and carries one.
///
/// The not-handed-over case ends in the same
/// [`CREDENTIAL_DIALOG_CAVEAT`](crate::provider::hyperv::CREDENTIAL_DIALOG_CAVEAT)
/// `vm view` prints, from the same constant: the two texts describe the same
/// console for the same guests, so "nothing to type" needs its exception in both
/// places or in neither.
fn view_note(console: ProviderKind, image: Image, enhanced_session: bool) -> String {
    // Keyed on the hypervisor before the guest, because which console this is
    // belongs to the hypervisor: the Windows image under QEMU, which is how a
    // Linux host boots it, is a VNC framebuffer with no session to choose, no
    // clipboard either way, and no credential dialog to warn about.
    match (console, image.target(), enhanced_session) {
        (ProviderKind::HyperV, Target::Windows, true) => {
            "\n\nIts desktop opens in an enhanced session, which is the \
             one that can be resized: drag the window and the guest's desktop \
             follows. The dialog asks for the guest's account, `tester`, with no \
             password at all, so leave that field empty and connect. A guest \
             with a test run in it offers none of this and opens a basic session \
             instead, because an enhanced one would take the console session out \
             from under the run.\n\n\
             An enhanced session is RDP, so it carries the clipboard: text can \
             be pasted straight in. Files go in over `vm ssh` and scp."
                .to_owned()
        }
        (ProviderKind::HyperV, Target::Windows, false) => format!(
            "\n\nIts desktop opens in a basic session: nothing to type, and fixed \
             at the console resolution, because only an enhanced session can be \
             resized and this guest is not offering one. `vm up` and `e2e --keep` \
             are what turn that on.\n\n\
             A basic session carries no clipboard, so text and files go in over \
             `vm ssh` and scp.\n\n{}",
            crate::provider::hyperv::CREDENTIAL_DIALOG_CAVEAT
        ),
        _ => "\n\nThe console carries no clipboard integration, so text and \
             files go in over `vm ssh` and scp."
            .to_owned(),
    }
}

/// What is waiting on the desktop of a guest that has just been handed over.
///
/// A Windows guest has no OpenGL, and the failure that produces names
/// `glCreateShader` rather than the guest, so what the app needs is said where
/// the guest is handed over. The launcher behind the desktop shortcut is what
/// sets it; the value named here comes from the job's own constant, so the
/// three cannot drift apart.
///
/// A guest nothing was staged in has none of that, and is told what would put it
/// there instead: `vm smoke --keep` leaves an empty desktop, and being told
/// which shortcut to double-click is worse than being told there is none.
///
/// The Linux arm has the opposite problem to solve. Nothing there needs an
/// environment override, so what a person is missing is not a variable but the
/// guest's root: it is outside any home directory on purpose, and the first
/// person handed a KDE guest could not find the app. It also cannot promise a
/// desktop icon, because GNOME shows none at all, so it names the menu and the
/// desktop separately.
///
/// Only the two desktop images reach this. A builder has nothing staged in it
/// and closes with [`builder_explainer`] instead, which is decision 9: what a
/// builder needs said is what it is holding and how to stop it, and none of
/// what is here is true of one.
fn guest_environment_note(image: Image, staged: Staging) -> String {
    match (image.target(), staged) {
        (Target::Windows, Staging::Done) => format!(
            "\n\nTwo shortcuts are on its desktop. `{app}` starts the app through \
             a launcher that sets `SLINT_BACKEND={backend}` for it: this guest has \
             no OpenGL, and without that the app exits before a window appears. \
             `{folder}` opens the directory the binaries, fixtures and results \
             are in.",
            app = crate::guest::handover::APP_SHORTCUT.trim_end_matches(".lnk"),
            folder = crate::guest::handover::FOLDER_SHORTCUT.trim_end_matches(".lnk"),
            backend = crate::commands::e2e::WINDOWS_SLINT_BACKEND
        ),
        // What must not be here is `vm up` on its own, which would be the
        // command that just ran: nothing changes until the thing that would
        // compile the binaries exists, so that is what this names.
        (Target::Windows, Staging::Impossible) => format!(
            "\n\nNothing of ours is in it: nothing on this host can compile \
             Windows binaries as it stands, so its desktop is empty and there is \
             no app in it to start. `cargo xtask vm build-image {builder}` gives \
             this host a guest that can, and the boot after that stages them. \
             Meanwhile what this one is good for is the image itself: \
             `cargo xtask vm view {image}` shows its console and \
             `cargo xtask vm ssh {image}` is a shell in it.",
            builder = Image::builder(Target::Windows),
        ),
        (Target::Windows, Staging::Skipped) => format!(
            "\n\nNothing of ours was staged in it, so its desktop is empty and \
             there is no app in it to start. `cargo xtask vm up {image}` boots a \
             guest with the binaries, the launcher and the shortcuts, and \
             `{keep}` leaves one behind after a run.",
            keep = keep_command(image),
        ),
        (Target::Linux, Staging::Done) => format!(
            "\n\nWhat this boot staged is in `{root}`, which is outside any home \
             directory: the app and the test harness in `bin/`, the fixtures, and \
             a run's results. `{launcher}` starts the app from there, naming the \
             staged textures directory when there is one, and it is what the \
             `{app}` entry runs. Both entries are in the applications menu, and on \
             the desktop itself where the desktop shows icons, which Plasma, XFCE \
             and Cinnamon do and GNOME does not. From a shell: `{launcher}`.",
            root = crate::provider::GUEST_ROOT_LINUX,
            launcher = crate::guest::handover::linux_launcher_path(),
            app = crate::guest::handover::ENTRY_NAME,
        ),
        (Target::Linux, _) => format!(
            "\n\nNothing of ours was staged in it, so there is no app in \
             `{root}` to start and no entry for one. `cargo xtask vm up {image}` \
             boots a guest with the binaries, the launcher and the desktop \
             entries, and `{keep}` leaves one behind after a run.",
            root = crate::provider::GUEST_ROOT_LINUX,
            keep = keep_command(image),
        ),
    }
}

/// Whether a boot of this image puts the current binaries in the guest.
///
/// Every image the suite runs in, and no builder. A builder is not a guest the
/// suite runs in, and on a host that compiles the suite in this very image the
/// build would leave a stopped guest that the boot then discards to make its
/// pristine overlay: ten minutes spent putting a test harness where nothing
/// runs it, and the build directory thrown away with the guest.
pub fn stages_binaries(image: Image) -> bool {
    !image.is_builder()
}

/// `vm up`.
pub fn up(
    runner: &dyn Runner,
    image: Image,
    allow_expired: bool,
    request: BootRequest,
) -> Result<u8, String> {
    // A usage error is answered before the build it would otherwise waste.
    request.check(image)?;
    let store = store::store()?;
    // Built before anything is created, because it decides what this boot is: a
    // guest carrying the current binaries, or the golden image itself to look
    // at. A compile error therefore costs no boot at all.
    //
    // A host that cannot build them still boots: `vm up` is for looking at a
    // guest, and a Linux host that has just spent an hour building the Windows
    // image has every reason to boot it. `e2e` refuses that case instead,
    // because a suite with nothing to run is not a run.
    let built = if stages_binaries(image) {
        match crate::guest::artifacts::usable_builder(&store, HostOs::current(), image.target()) {
            Ok(_) => Some(crate::guest::artifacts::build(
                runner,
                &store,
                image.target(),
            )?),
            Err(reason) => {
                println!("nothing of ours goes into this guest: {reason}");
                println!("it boots as the image built it");
                None
            }
        }
    } else {
        None
    };

    let mut session = boot(
        runner,
        &store,
        image,
        StartReason::Up,
        allow_expired,
        request,
    )?;
    // Decision 14: an interactive guest carries the current binaries, exactly
    // as a test run would, so `vm up` and `e2e --keep` land in the same place.
    let staged = match &built {
        Some(built) => {
            if let Err(e) = crate::guest::artifacts::stage(&store, &session, built) {
                // Keep the guest: `vm up` is for looking at one, and a guest
                // that booted is still worth having even if the binaries did
                // not arrive.
                println!("{}", after_failure(&mut session, &store, true));
                return Err(e);
            }
            Staging::Done
        }
        None if !stages_binaries(image) => Staging::Skipped,
        None => Staging::Impossible,
    };
    let enhanced_session = hand_over(&mut session, &store);
    println!(
        "{}",
        lifecycle_explainer(
            session.image,
            Prepared {
                staged,
                console: session.provider.kind(),
                enhanced_session
            }
        )
    );
    Ok(0)
}

/// The last thing done to a guest that is being left to a person.
///
/// Turning an enhanced `vmconnect` session on belongs here rather than in the
/// boot or the staging, because it is the one thing a guest with a test suite
/// running in it must not offer: connecting takes over the console session,
/// which is where those tests keep their desktop. `vm up` and `e2e --keep` are
/// the two moments where nothing of ours is running in the guest and somebody
/// is about to look at it.
///
/// Answers whether the guest ended up offering it, and writes that into the
/// record, because `vm view` runs later as its own command and has no other way
/// to know: what it decides from it is whether to answer the connection dialog
/// and what to tell somebody the console will ask them for. So the answer is no
/// for a guest that has no such session to offer at all, and not only for one
/// where turning it on failed: a Linux guest is looked at through VNC, and a
/// record claiming otherwise would have `vm view` promise a blank password for
/// an account that is not in it.
pub fn hand_over(session: &mut Session, store: &Store) -> bool {
    let enhanced_session = match crate::guest::handover::enable_enhanced_session(
        session.provider.as_ref(),
        &session.state,
        session.target(),
    ) {
        Ok(offered) => offered,
        Err(e) => {
            println!("warning: {e}");
            println!("  the basic session still shows the desktop; it cannot be resized");
            false
        }
    };
    session.state.handed_over = enhanced_session;
    record_kept(session, store);
    enhanced_session
}

/// What a guest's record says once the work it was booted for is over.
///
/// The two reasons that describe work in progress become the one that describes
/// a guest waiting for somebody, and everything else is left alone.
pub fn reason_when_kept(reason: StartReason) -> StartReason {
    match reason {
        StartReason::Run | StartReason::Dist => StartReason::Keep,
        other => other,
    }
}

/// Record what a guest is once the command that booted it is finished with it.
///
/// A guest booted for a run and then kept is no longer a run in progress, and
/// `vm status` reads the reason to say why something is still there. Nothing
/// used to write [`StartReason::Keep`] at all, so a kept guest reported itself
/// as a run for as long as it existed.
///
/// A release build is the same case and reaches this at the same moment, once
/// its work is over: leaving it named as one costs more than a stale label,
/// because `StartReason::Dist` carries a `cost_of_ending`, and the teardown the
/// closing text just told a person to run would warn them that it ends a build
/// that finished minutes ago. An image build never reaches here, which is why it
/// is not in the list: nothing hands one over.
fn record_kept(session: &mut Session, store: &Store) {
    session.state.reason = reason_when_kept(session.state.reason);
    if let Err(e) = write_state(store, session.image, &session.state) {
        println!("warning: the guest is up, but its record could not be updated: {e}");
        println!(
            "  `cargo xtask vm view {}` may open the wrong console",
            session.image
        );
    }
}

/// `vm ssh`.
pub fn ssh(runner: &dyn Runner, image: Image, extra: &[String]) -> Result<u8, String> {
    let store = store::store()?;
    let state = load_state(&store, image).ok_or_else(|| {
        format!("no {image} VM is recorded. `cargo xtask vm up {image}` starts one.")
    })?;
    let provider = provider::for_state(runner, &store, &state)?;
    if !provider.is_running(&state) {
        return Err(not_running_hint(image, &state));
    }
    let remote = if extra.is_empty() {
        None
    } else {
        Some(extra.join(" "))
    };
    let ssh_target = provider.ssh_target(&state);
    // Without this the child gets a closed stdin, and `vm ssh` is then a shell
    // nobody can type into: the keystrokes sit unread in the terminal's own
    // buffer, unechoed, and only appear once the process finally exits. That is
    // what it looked like when this was reported, and the appearance of a
    // frozen terminal was the symptom.
    let cmd = crate::guest::ssh::ssh_command(&ssh_target, remote.as_deref()).interactive();
    if remote.is_none() {
        println!(
            "opening a shell on {} ({}:{}); `exit` or Ctrl-D leaves it",
            state.vm_name, ssh_target.host, ssh_target.port
        );
    }
    let code = runner
        .stream(&cmd)
        .map_err(|e| format!("cannot run ssh: {e}"))?;
    Ok(u8::try_from(code).unwrap_or(1))
}

/// `vm view`.
pub fn view(runner: &dyn Runner, image: Image) -> Result<u8, String> {
    let store = store::store()?;
    let state = load_state(&store, image).ok_or_else(|| {
        format!(
            "no {image} VM is running. `cargo xtask vm up {image}` starts one, \
             and `{keep}` leaves the aftermath of a run to look at.",
            keep = keep_command(image),
        )
    })?;
    let provider = provider::for_state(runner, &store, &state)?;
    if !provider.is_running(&state) {
        return Err(format!(
            "there is no console to attach to: {}",
            not_running_hint(image, &state)
        ));
    }
    // The advice about how to use a console belongs to whichever provider
    // opened it: none of the enhanced-session warning means anything to
    // somebody looking at a VNC framebuffer.
    println!("{}", provider.view(&state)?);
    Ok(0)
}

/// `vm status`, with liveness filled in from the providers.
pub fn status(runner: &dyn Runner) -> Result<u8, String> {
    let store = store::store()?;
    let mut inventory = inventory::scan(&store);
    for entry in &mut inventory.images {
        if let Some(state) = &entry.state {
            entry.running = provider::for_state(runner, &store, state)
                .ok()
                .map(|p| p.is_running(state));
        }
    }
    print!("{}", status::render(&inventory, util::now_unix()));
    Ok(0)
}

/// `vm down`: end the guest, keep everything that took time to build.
pub fn down(runner: &dyn Runner, selection: Selection) -> Result<u8, String> {
    tear_down(runner, selection, teardown::Scope::RUN_STATE, true)
}

/// `vm purge`: delete what is on disk, after asking.
pub fn purge(
    runner: &dyn Runner,
    selection: Selection,
    scope: teardown::Scope,
    force: bool,
) -> Result<u8, String> {
    tear_down(runner, selection, scope, force)
}

/// The shared half. `confirmed` is what `--force` sets and what `down` is
/// always allowed to assume: it deletes only state the next boot recreates.
fn tear_down(
    runner: &dyn Runner,
    selection: Selection,
    scope: teardown::Scope,
    confirmed: bool,
) -> Result<u8, String> {
    let store = store::store()?;
    let inventory = inventory::scan(&store);
    let plan = teardown::plan(&store, &inventory, selection, scope);
    print!("{}", plan.render());
    if plan.is_empty() {
        return Ok(0);
    }
    if !confirmed && !teardown::confirm(&teardown::confirmation_prompt(&plan)) {
        println!("nothing was deleted");
        return Ok(0);
    }
    // Asked unconditionally. Whether there is anything to stop is the
    // provider's question to answer, and answering it here by consulting
    // `is_running` first is what let a registered but powered-off Hyper-V VM
    // keep its registration while its disk was deleted out from under it.
    let outcome = teardown::execute(&plan, &|state| {
        provider::for_state(runner, &store, state)?.destroy(state)
    });
    print!("{}", teardown::render_outcome(&outcome, scope));
    Ok(u8::from(!outcome.problems.is_empty()))
}

/// The guest-contract smoke test: boot, run a trivial job, collect it, destroy.
///
/// This is where the contract is proved and where boot and poll timings get
/// calibrated, deliberately separate from the e2e suite so that a failure here
/// means the plumbing and a failure there means the product.
pub fn smoke(
    runner: &dyn Runner,
    image: Image,
    keep: bool,
    request: BootRequest,
) -> Result<u8, String> {
    let store = store::store()?;
    let started = std::time::Instant::now();
    let mut session = boot(runner, &store, image, StartReason::Run, false, request)?;

    let script = smoke_script(image);

    println!("running a trivial job through the guest contract");
    let scratch = store.job_scratch(image);
    // From here on the VM exists, so `?` would leave it running unannounced.
    let code = match job::run(
        session.provider.as_ref(),
        &session.state,
        image.target(),
        &script,
        &scratch,
        Duration::from_mins(5),
    ) {
        Ok(code) => code,
        Err(e) => {
            println!("{}", after_failure(&mut session, &store, keep));
            return Err(e);
        }
    };

    let results = store.results_dir(image);
    if let Err(e) = session.provider.collect_results(
        &session.state,
        &provider::guest_results(image.target()),
        &results,
    ) {
        println!("{}", after_failure(&mut session, &store, keep));
        return Err(e);
    }

    println!();
    println!(
        "the job exited {code} after {:.0}s total",
        started.elapsed().as_secs_f64()
    );
    println!("results are in {}", results.display());
    if let Ok(log) = std::fs::read_to_string(results.join("output.log")) {
        println!("--- output.log ---");
        print!("{log}");
        println!("--- end ---");
    }

    if keep {
        // Nothing was staged in this guest and nothing was handed over: the
        // smoke test proves the guest contract and leaves the guest as it found
        // it, so the text says what is actually in there.
        record_kept(&mut session, &store);
        println!(
            "{}",
            lifecycle_explainer(
                image,
                Prepared::bare(&store, session.provider.kind(), image.target())
            )
        );
    } else if let Err(e) = session.tear_down(&store) {
        // The same shape as the other three teardown sites: name the VM and
        // say how to reach it, because it is still there.
        println!(
            "warning: {} could not be destroyed: {e}",
            session.state.vm_name
        );
        println!("{}", session.reach_hint());
    } else {
        println!(
            "{} is destroyed and the overlay is gone",
            session.state.vm_name
        );
    }
    Ok(u8::from(code != 0))
}

/// The trivial job the smoke test runs.
///
/// What it asks the guest differs by what the guest has. A builder of either
/// target is asked for its toolchain, which is the thing that makes it one and
/// the thing a `dist` run refuses without. The Debian desktop image is asked to
/// prove there is an X server the job can reach, which is the half of the
/// contract a builder cannot have; the Windows desktop image is asked neither,
/// because its job runs from the console session or it does not run at all, so
/// getting this far is the proof. Asking a question of the wrong image prints a
/// command-not-found line that reads as a defect in the image.
pub fn smoke_script(image: Image) -> String {
    match image.target() {
        Target::Windows => format!(
            "@echo off\r\n\
             echo sunlit-e2e smoke\r\n\
             hostname\r\n\
             whoami\r\n\
             echo %SUNLIT_E2E_ARTIFACTS%\r\n\
             {probe}\
             echo smoke > %SUNLIT_E2E_ARTIFACTS%\\smoke.txt\r\n",
            probe = if image.is_builder() {
                "\"%USERPROFILE%\\.cargo\\bin\\cargo.exe\" -V || exit /b 1\r\n"
            } else {
                ""
            },
        ),
        Target::Linux => format!(
            "#!/usr/bin/env bash\n\
             set -eux\n\
             echo 'sunlit-e2e smoke'\n\
             hostname\n\
             id\n\
             echo \"DISPLAY=$DISPLAY\"\n\
             {probe}\
             echo smoke > \"$SUNLIT_E2E_ARTIFACTS/smoke.txt\"\n",
            probe = if image.is_builder() {
                "\"$HOME/.cargo/bin/cargo\" -V\n"
            } else {
                "xdpyinfo -display \"$DISPLAY\" | head -3\n"
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_interactive_shell_inherits_stdin_and_a_remote_command_does_not_change_that() {
        // `stream` gives a child a closed stdin unless the command says
        // otherwise, which is right for every orchestration step and wrong for
        // the one command whose purpose is to hand over the terminal.
        let reachable = crate::guest::ssh::SshTarget {
            user: "tester".to_owned(),
            host: "127.0.0.1".to_owned(),
            port: 2222,
            key: std::path::PathBuf::from("/srv/vm/ssh/id_ed25519"),
        };
        let shell = crate::guest::ssh::ssh_command(&reachable, None).interactive();
        assert!(
            shell.interactive,
            "a shell nobody can type into is not a shell"
        );
        // Every other ssh invocation stays non-interactive on purpose.
        let probe = crate::guest::ssh::ssh_command(&reachable, Some("echo hi"));
        assert!(!probe.interactive);
    }

    /// A guest's closing-text facts, every one of them stated.
    ///
    /// `Prepared::bare` asks this host what it can build, and a session asks its
    /// provider which console it opened; a test that left either to the machine
    /// it runs on would assert something different on each one.
    fn prepared(console: ProviderKind, staged: Staging, enhanced_session: bool) -> Prepared {
        Prepared {
            staged,
            console,
            enhanced_session,
        }
    }

    /// The same, for a guest with nothing of ours in it.
    fn bare(console: ProviderKind, staged: Staging) -> Prepared {
        prepared(console, staged, false)
    }

    use super::*;
    use crate::store::manifest::eval_state;
    use crate::util::SECS_PER_DAY;

    #[test]
    fn the_expiry_help_explains_the_symptom_and_both_ways_out() {
        let text = expired_help(Image::Windows, eval_state(0, 95 * SECS_PER_DAY));
        assert!(text.contains("expired"), "{text}");
        assert!(text.contains("once an hour"), "{text}");
        assert!(
            text.contains("cargo xtask vm build-image windows"),
            "{text}"
        );
        assert!(text.contains("--allow-expired-image"), "{text}");
    }

    /// The smoke test proves the guest contract, and what there is to prove
    /// differs: a desktop image has a session for a job to reach, a builder has
    /// a compiler. Asking the wrong question prints a command that is not there,
    /// which reads as a broken image rather than a wrong script.
    #[test]
    fn the_smoke_job_asks_each_image_about_what_it_has() {
        let desktop = smoke_script(Image::Linux);
        assert!(desktop.contains("xdpyinfo"), "{desktop}");
        assert!(!desktop.contains("cargo"), "{desktop}");

        let builder = smoke_script(Image::LinuxBuilder);
        assert!(builder.contains(".cargo/bin/cargo"), "{builder}");
        assert!(!builder.contains("xdpyinfo"), "{builder}");

        // Both builders, not only the one whose script had a question in it
        // already: a Windows builder with no toolchain is the same broken image
        // as a Linux one, and the smoke test is where that shows.
        let windows = smoke_script(Image::WindowsBuilder);
        assert!(windows.contains(r"\.cargo\bin\cargo.exe"), "{windows}");
        assert!(windows.contains("exit /b 1"), "{windows}");
        assert!(!smoke_script(Image::Windows).contains("cargo"));

        for image in Image::ALL {
            assert_eq!(
                smoke_script(image).contains("cargo"),
                image.is_builder(),
                "{image}"
            );
        }

        for image in Image::ALL {
            let script = smoke_script(image);
            assert!(script.contains("smoke.txt"), "{image}");
            if image.target() == Target::Windows {
                assert!(script.contains("\r\n"), "{image}");
            } else {
                assert!(!script.contains('\r'), "{image}");
            }
        }
    }

    #[test]
    fn a_running_image_build_is_not_cleared_away_to_make_room() {
        let running = may_clear(Image::Windows, StartReason::Build, true).unwrap_err();
        assert!(running.contains("image build is running"), "{running}");
        assert!(running.contains("vm down windows"), "{running}");
        // The refusal is about the reason rather than about the image, so it
        // names the image it was asked about and not the one builds usually
        // happen on.
        let linux = may_clear(Image::Linux, StartReason::Build, true).unwrap_err();
        assert!(linux.contains("vm down linux"), "{linux}");
        assert!(!linux.contains("windows"), "{linux}");
        // A crashed build is exactly what clearing is for, and every other
        // guest holds nothing worth keeping.
        assert!(may_clear(Image::Windows, StartReason::Build, false).is_ok());
        for reason in [StartReason::Run, StartReason::Keep, StartReason::Up] {
            for image in Image::ALL {
                assert!(may_clear(image, reason, true).is_ok(), "{reason:?}");
                assert!(may_clear(image, reason, false).is_ok(), "{reason:?}");
            }
        }
    }

    #[test]
    fn being_told_to_end_a_build_says_what_that_ends() {
        // The one-VM-at-a-time refusal points at `vm down <other>`, and for a
        // build that command costs an install rather than a boot. The clause
        // itself lives on the reason, because the teardown that stops a guest
        // has to say the same thing.
        let build = StartReason::Build
            .cost_of_ending()
            .expect("ending a build costs something");
        assert!(build.contains("ends the image build"), "{build}");
        assert!(build.contains("over from the media"), "{build}");
        for reason in [StartReason::Run, StartReason::Keep, StartReason::Up] {
            assert_eq!(reason.cost_of_ending(), None, "{reason:?}");
        }
    }

    /// The text used to open by denying that a stop existed, which argues with
    /// a reader who is right: `vm down` is the stop, and an idle guest is worth
    /// stopping. What is missing is a save or a pause, and the rest of what the
    /// old wording said is true and stays.
    #[test]
    fn the_lifecycle_explainer_names_the_stop_and_what_it_frees() {
        let text = lifecycle_explainer(
            Image::Linux,
            prepared(ProviderKind::Qemu, Staging::Done, false),
        );
        assert!(text.contains("`vm down` is the stop"), "{text}");
        assert!(!text.contains("no stop or pause"), "{text}");
        assert!(text.contains("no way to do is save or pause"), "{text}");
        // The reason to take an idle one down, in the guest's own numbers.
        assert!(text.contains("4.0 GiB of this host's memory"), "{text}");
        assert!(text.contains("golden image untouched"), "{text}");
        assert!(text.contains("cargo xtask vm down linux"), "{text}");
        assert!(text.contains("clicking during one perturbs it"), "{text}");
        assert!(text.contains("clipboard"), "{text}");
        // Mesa answers in the Linux guest, so there is nothing to set there.
        assert!(!text.contains("SLINT_BACKEND"), "{text}");
    }

    /// A figure printed to argue for a teardown has to be the one the guest
    /// actually holds. Both hypervisors read it from one function now, so what
    /// this pins is that the text and that function agree, and that the two
    /// images a target has are not described by one number.
    #[test]
    fn the_memory_a_guest_is_said_to_hold_is_the_memory_it_gets() {
        for image in Image::ALL {
            let (mib, _) = crate::provider::resources_for(image);
            let expected = util::format_bytes(u64::from(mib) * 1024 * 1024);
            assert!(
                lifecycle_explainer(image, bare(ProviderKind::Qemu, Staging::Skipped))
                    .contains(&format!("{expected} of this host's memory")),
                "{image} is described as holding something other than {expected}"
            );
        }
        // And the two images of a target are not described by one number where
        // they do not hold one: the Linux builder gets twice the desktop
        // guest's memory, and both texts have to say their own figure.
        let desktop = lifecycle_explainer(Image::Linux, bare(ProviderKind::Qemu, Staging::Skipped));
        let builder = lifecycle_explainer(
            Image::LinuxBuilder,
            bare(ProviderKind::Qemu, Staging::Skipped),
        );
        let (desktop_mib, _) = crate::provider::resources_for(Image::Linux);
        let (builder_mib, _) = crate::provider::resources_for(Image::LinuxBuilder);
        assert!(builder_mib > desktop_mib);
        assert!(
            !desktop.contains(&util::format_bytes(u64::from(builder_mib) * 1024 * 1024)),
            "{desktop}"
        );
        assert!(
            !builder.contains(&util::format_bytes(u64::from(desktop_mib) * 1024 * 1024)),
            "{builder}"
        );
    }

    /// The Linux guest's root is outside any home directory on purpose, and
    /// until the hand-over existed nothing ever named it: the first person handed
    /// a KDE guest had nowhere to look for the app. So the closing text names the
    /// root, the launcher, and how to start it without an icon.
    #[test]
    fn handing_over_a_linux_guest_says_where_the_app_is_and_how_to_start_it() {
        let text = lifecycle_explainer(
            Image::Linux,
            prepared(ProviderKind::Qemu, Staging::Done, false),
        );
        assert!(text.contains(crate::provider::GUEST_ROOT_LINUX), "{text}");
        assert!(
            text.contains(&crate::guest::handover::linux_launcher_path()),
            "{text}"
        );
        assert!(text.contains(crate::guest::handover::ENTRY_NAME), "{text}");
        // GNOME draws no desktop icons at all, so the menu and the desktop are
        // named separately rather than a launcher promised on every desktop.
        assert!(text.contains("applications menu"), "{text}");
        assert!(text.contains("GNOME does not"), "{text}");
    }

    /// A guest with nothing in it because the builder image is missing has one
    /// actionable command, and `vm up` is not it: that is the command that just
    /// ran, and it will do the same thing again until the image exists.
    #[test]
    fn a_windows_guest_with_no_builder_image_names_the_image_and_not_vm_up() {
        let text = lifecycle_explainer(
            Image::Windows,
            bare(ProviderKind::Qemu, Staging::Impossible),
        );
        assert!(!text.contains("cargo xtask vm up windows"), "{text}");
        assert!(
            text.contains("cargo xtask vm build-image windows-builder"),
            "{text}"
        );
        assert!(text.contains("cargo xtask vm view windows"), "{text}");
        assert!(text.contains("cargo xtask vm ssh windows"), "{text}");
        // The other two texts about the same guest are about a guest that could
        // have been staged, and neither may claim this one's excuse.
        for staged in [Staging::Skipped, Staging::Done] {
            let text = lifecycle_explainer(Image::Windows, bare(ProviderKind::HyperV, staged));
            assert!(!text.contains("can compile"), "{text}");
        }
    }

    /// The same guest, and the console is the hypervisor's rather than the
    /// guest's: a Windows guest under QEMU is a VNC framebuffer, so none of
    /// what `vmconnect` offers, asks for, or warns about applies to it.
    #[test]
    fn a_windows_guest_under_qemu_is_described_as_the_vnc_console_it_has() {
        let text = lifecycle_explainer(
            Image::Windows,
            bare(ProviderKind::Qemu, Staging::Impossible),
        );
        assert!(!text.contains("basic session"), "{text}");
        assert!(!text.contains("enhanced session"), "{text}");
        assert!(
            !text.contains(crate::provider::hyperv::CREDENTIAL_DIALOG_CAVEAT),
            "{text}"
        );
        assert!(text.contains("no clipboard integration"), "{text}");
    }

    /// Nothing staged is the host's answer rather than the command's when what
    /// would compile the binaries is a builder guest that has not been built:
    /// a Linux host with the image gets Windows binaries, and without it gets
    /// none.
    #[test]
    fn staging_is_impossible_when_the_builder_it_would_use_is_not_there() {
        let empty = Store::new(std::env::temp_dir().join("sunlit_xtask_staging_no_images"));
        assert_eq!(
            Staging::skipped_for(&empty, HostOs::Linux, Target::Windows),
            Staging::Impossible
        );
        // The cells that compile on the host itself, or in WSL, need no image
        // and are the command's answer whatever the store holds.
        for (host, target) in [
            (HostOs::Linux, Target::Linux),
            (HostOs::Windows, Target::Windows),
            (HostOs::Windows, Target::Linux),
        ] {
            assert_eq!(
                Staging::skipped_for(&empty, host, target),
                Staging::Skipped,
                "{host:?} {target}"
            );
        }
    }

    /// `vm smoke --keep` leaves a Linux guest with nothing of ours in it either,
    /// and being told which entry to click is worse than being told there is
    /// none. The Windows arm has said so since it existed; this is the same rule.
    #[test]
    fn a_bare_linux_guest_promises_no_launcher_and_names_what_would_stage_one() {
        let text = lifecycle_explainer(Image::Linux, bare(ProviderKind::Qemu, Staging::Skipped));
        assert!(text.contains("Nothing of ours was staged"), "{text}");
        assert!(!text.contains(crate::guest::handover::ENTRY_NAME), "{text}");
        assert!(
            !text.contains(&crate::guest::handover::linux_launcher_path()),
            "{text}"
        );
        assert!(text.contains("cargo xtask vm up linux"), "{text}");
        assert!(text.contains(&keep_command(Image::Linux)), "{text}");
    }

    /// The Linux builder has no session for an entry to appear in, so neither
    /// of its texts may name one: a guest that was told to look in a menu it
    /// does not have reads as a guest that failed to stage.
    #[test]
    fn a_linux_builder_is_never_promised_a_menu_or_a_desktop_entry() {
        for prepared in [
            bare(ProviderKind::Qemu, Staging::Skipped),
            prepared(ProviderKind::Qemu, Staging::Done, false),
        ] {
            let text = lifecycle_explainer(Image::LinuxBuilder, prepared);
            assert!(!text.contains("desktop entries"), "{text}");
            assert!(!text.contains(crate::guest::handover::ENTRY_NAME), "{text}");
            assert!(!text.contains("applications menu"), "{text}");
            assert!(text.contains("vm ssh linux-builder"), "{text}");
        }
        // The desktop image keeps every one of those promises, which is what
        // makes the builder's silence about them a difference and not a loss.
        let desktop = lifecycle_explainer(
            Image::Linux,
            prepared(ProviderKind::Qemu, Staging::Done, false),
        );
        assert!(desktop.contains("applications menu"), "{desktop}");
    }

    /// A guest kept after a release build is not a release build in progress,
    /// and the difference is not cosmetic: `StartReason::Dist` carries a cost of
    /// ending, so the `vm down` the closing text has just recommended would warn
    /// about ending a build that finished minutes ago.
    #[test]
    fn a_guest_kept_after_its_work_is_no_longer_named_as_that_work() {
        assert_eq!(reason_when_kept(StartReason::Run), StartReason::Keep);
        assert_eq!(reason_when_kept(StartReason::Dist), StartReason::Keep);
        assert!(
            reason_when_kept(StartReason::Dist)
                .cost_of_ending()
                .is_none()
        );
        // An image build never reaches a hand-over, and an interactive guest is
        // already what it says it is.
        assert_eq!(reason_when_kept(StartReason::Build), StartReason::Build);
        assert_eq!(reason_when_kept(StartReason::Up), StartReason::Up);
        assert_eq!(reason_when_kept(StartReason::Keep), StartReason::Keep);
    }

    /// The line a boot prints between SSH and the job is about a session the
    /// Linux builder has none of: it writes its readiness marker from a oneshot
    /// unit at boot, so a wait that returns instantly there is the image working
    /// rather than a desktop that failed to appear. The Windows builder is the
    /// other way round, and the seconds it spends here are a real logon.
    #[test]
    fn only_an_image_with_a_desktop_is_said_to_be_waiting_for_one() {
        for image in Image::ALL {
            let waiting = readiness_wait_line(image);
            let ready = readiness_ready_line(image, Duration::from_secs(6));
            assert_eq!(waiting.contains("desktop"), image.has_desktop(), "{image}");
            assert_eq!(ready.contains("desktop"), image.has_desktop(), "{image}");
            assert!(ready.contains("after 6s"), "{image}: {ready}");
        }
        assert_eq!(
            readiness_wait_line(Image::WindowsBuilder),
            "waiting for the desktop session"
        );
        assert_eq!(
            readiness_wait_line(Image::LinuxBuilder),
            "waiting for the guest to be ready for a job"
        );
    }

    #[test]
    fn handing_over_a_windows_guest_says_what_the_app_needs_in_it() {
        // The job sets this; a shell does not, and the failure without it names
        // an OpenGL symbol rather than the guest. What sets it for a person is
        // the desktop launcher, so both shortcuts are named as well.
        let text = lifecycle_explainer(
            Image::Windows,
            prepared(ProviderKind::HyperV, Staging::Done, true),
        );
        assert!(
            text.contains(&format!(
                "SLINT_BACKEND={}",
                crate::commands::e2e::WINDOWS_SLINT_BACKEND
            )),
            "{text}"
        );
        assert!(text.contains("no OpenGL"), "{text}");
        assert!(text.contains("Sunlit Earth"), "{text}");
        assert!(text.contains("sunlit-e2e"), "{text}");
        // Named the way Explorer shows them, without the extension it hides.
        assert!(!text.contains(".lnk"), "{text}");
        // An enhanced session is RDP, and RDP brings the clipboard with it.
        assert!(text.contains("carries the clipboard"), "{text}");
    }

    /// `vm smoke --keep` stages nothing and hands nothing over, so it may not
    /// promise either. This is the text a person reads while looking at an empty
    /// desktop in a session that asks them for nothing.
    #[test]
    fn a_guest_that_was_left_as_it_stood_promises_neither_shortcuts_nor_a_session() {
        let text =
            lifecycle_explainer(Image::Windows, bare(ProviderKind::HyperV, Staging::Skipped));
        assert!(text.contains("`vm down` is the stop"), "{text}");
        // Neither shortcut, and no launcher behind one.
        assert!(!text.contains("Sunlit Earth"), "{text}");
        assert!(!text.contains("SLINT_BACKEND"), "{text}");
        assert!(text.contains("cargo xtask vm up windows"), "{text}");
        // And the basic session, which is what a guest nobody handed over has.
        assert!(text.contains("basic session"), "{text}");
        assert!(!text.contains("leave that field empty"), "{text}");
        assert!(text.contains("no clipboard"), "{text}");
    }

    /// A guest nobody handed over is the one that can ask for credentials
    /// anyway, and this text is read by the two commands that leave one: `vm
    /// smoke --keep`, and a `vm up` whose hand-over did not take. Both get the
    /// same sentence `vm view` gives, because the alternative is a console the
    /// two commands describe differently.
    #[test]
    fn every_text_about_a_guest_that_offers_no_enhanced_session_carries_the_same_caveat() {
        let caveat = crate::provider::hyperv::CREDENTIAL_DIALOG_CAVEAT;
        for prepared in [
            bare(ProviderKind::HyperV, Staging::Skipped),
            prepared(ProviderKind::HyperV, Staging::Done, false),
        ] {
            let text = lifecycle_explainer(Image::Windows, prepared);
            assert!(text.contains(caveat), "{text}");
        }
        assert!(crate::provider::hyperv::view_note(Target::Windows, false).contains(caveat));

        // A guest that was handed over expects the dialog and can answer it, so
        // the caveat would contradict the advice it is printed beside.
        let handed_over = lifecycle_explainer(
            Image::Windows,
            prepared(ProviderKind::HyperV, Staging::Done, true),
        );
        assert!(!handed_over.contains(caveat), "{handed_over}");
        assert!(!crate::provider::hyperv::view_note(Target::Windows, true).contains(caveat));
        // And a Linux guest has no vmconnect dialog to be surprised by.
        assert!(
            !lifecycle_explainer(
                Image::Linux,
                prepared(ProviderKind::Qemu, Staging::Done, false)
            )
            .contains(caveat)
        );
    }

    /// Only one image carries four desktops, so only one guest can be asked
    /// which to use, and the other refuses rather than ignoring the flag.
    ///
    /// Ignoring it is the failure this guards: a Windows run that recorded a
    /// desktop nobody could boot into would have `vm status` naming an XFCE
    /// session on a guest that has no such thing, and the run's results would be
    /// about a desktop nobody chose.
    #[test]
    fn only_the_linux_guest_can_be_asked_which_desktop_to_boot() {
        for desktop in Desktop::ALL {
            assert_eq!(
                desktop_for(Image::Linux, Some(desktop)),
                Ok(Some(desktop)),
                "{desktop}"
            );
            let refusal = desktop_for(Image::Windows, Some(desktop))
                .expect_err("the Windows image has one desktop");
            // What was asked for and why it cannot be, both in the message: the
            // flag is right for the other image rather than wrong everywhere.
            assert!(refusal.contains(desktop.flag()), "{refusal}");
            assert!(refusal.contains("Debian 13 guest option"), "{refusal}");

            // The Linux builder is the sharper case: it has no desktop at all,
            // and saying "the other image has one" would send somebody looking
            // for a session that is not in there. The Windows builder is not
            // that case, because it logs on the session its parent was built
            // with, so it gets the same refusal the desktop image gets.
            let refusal = desktop_for(Image::LinuxBuilder, Some(desktop))
                .expect_err("the Linux builder has no desktop");
            assert!(refusal.contains("no desktop at all"), "{refusal}");
            let refusal = desktop_for(Image::WindowsBuilder, Some(desktop))
                .expect_err("the Windows builder has one desktop");
            assert!(refusal.contains("Debian 13 guest option"), "{refusal}");
            assert!(!refusal.contains("no desktop at all"), "{refusal}");
        }
        // Neither guest has to be asked. On Linux that is the image's own
        // default session, which is the whole point of the flag being optional.
        for image in Image::ALL {
            assert_eq!(desktop_for(image, None), Ok(None), "{image}");
        }
    }

    #[test]
    fn a_session_type_is_asked_of_the_linux_guest_alone_and_defaults_to_kde() {
        let wayland = Some(SessionType::Wayland);
        assert_eq!(
            login_for(Image::Linux, None, wayland),
            Ok(Login::new(Desktop::Kde, SessionType::Wayland).ok())
        );
        assert_eq!(
            login_for(Image::Linux, Some(Desktop::Gnome), wayland),
            Ok(Login::new(Desktop::Gnome, SessionType::Wayland).ok())
        );
        assert_eq!(
            login_for(Image::Linux, Some(Desktop::Xfce), None),
            Ok(Login::new(Desktop::Xfce, SessionType::X11).ok()),
            "a desktop alone is its X11 session, as it always was"
        );
        assert_eq!(login_for(Image::Linux, None, None), Ok(None));
        let refusal = login_for(Image::Linux, Some(Desktop::Cinnamon), wayland)
            .expect_err("Cinnamon has no Wayland session here");
        assert!(refusal.contains("--desktop gnome"), "{refusal}");

        let refusal =
            login_for(Image::Windows, None, wayland).expect_err("the Windows image has one");
        assert!(refusal.contains("Debian 13 guest option"), "{refusal}");
        let refusal = login_for(Image::Windows, None, Some(SessionType::X11))
            .expect_err("even the one it would have anyway");
        assert!(refusal.contains("--session-type x11"), "{refusal}");
        let refusal = login_for(Image::LinuxBuilder, None, wayland)
            .expect_err("the Linux builder has no desktop");
        assert!(refusal.contains("no desktop at all"), "{refusal}");
    }

    #[test]
    fn a_wayland_guest_is_refused_a_second_screen_and_an_x11_one_is_not() {
        let request = |session_type, screens| BootRequest {
            desktop: Some(Desktop::Gnome),
            session_type,
            screens,
        };
        let refusal = request(Some(SessionType::Wayland), 2)
            .check(Image::Linux)
            .expect_err("xrandr cannot place a Wayland session's outputs");
        assert!(refusal.contains("--screens 2"), "{refusal}");
        assert!(refusal.contains("--session-type x11"), "{refusal}");
        assert!(refusal.contains("GNOME (Wayland)"), "{refusal}");
        assert!(
            request(Some(SessionType::Wayland), 1)
                .check(Image::Linux)
                .is_ok()
        );
        assert!(
            request(Some(SessionType::X11), 2)
                .check(Image::Linux)
                .is_ok()
        );
        assert!(request(None, 2).check(Image::Linux).is_ok());
        assert_eq!(BootRequest::PLAIN.check(Image::Windows), Ok((None, 1)));
    }

    #[test]
    fn only_the_linux_guest_can_be_asked_for_more_than_one_screen() {
        assert_eq!(screens_for(Image::Linux, 2), Ok(2));
        assert_eq!(screens_for(Image::Linux, MAX_SCREENS), Ok(MAX_SCREENS));

        // Windows is the refusal that has to explain itself, because the flag
        // exists, the guest is a real desktop, and the thing that cannot do it
        // is the video adapter rather than the xtask.
        let refusal = screens_for(Image::Windows, 2).expect_err("one head, one screen");
        assert!(refusal.contains("--screens 2"), "{refusal}");
        assert!(refusal.contains("display driver"), "{refusal}");
        let refusal =
            screens_for(Image::LinuxBuilder, 2).expect_err("no session to put a screen in");
        assert!(refusal.contains("no desktop at all"), "{refusal}");

        let refusal = screens_for(Image::Linux, MAX_SCREENS + 1).expect_err("past the cap");
        assert!(refusal.contains(&MAX_SCREENS.to_string()), "{refusal}");
        let refusal = screens_for(Image::Linux, 0).expect_err("a guest has at least one screen");
        assert!(refusal.contains("--screens 0"), "{refusal}");

        // One is what every guest already has, so no image refuses it: a flag
        // that changes nothing is not worth a refusal, and every boot that does
        // not mention screens passes exactly this.
        for image in Image::ALL {
            assert_eq!(screens_for(image, 1), Ok(1), "{image}");
        }
    }

    /// Real `xrandr --query` output from the Linux guest's shape: the connected
    /// output with its mode, the mode list indented under it, and the head with
    /// nothing on it.
    const XRANDR: &str = "\
Screen 0: minimum 320 x 200, current 3840 x 1080, maximum 16384 x 16384
Virtual-1 connected primary 1920x1080+0+0 (normal left inverted right x axis y axis) 0mm x 0mm
   1920x1080     59.96*+
   1280x800      59.81
Virtual-2 connected 1920x1080+1920+0 (normal left inverted right x axis y axis) 0mm x 0mm
   1920x1080     59.96*+
Virtual-3 disconnected (normal left inverted right x axis y axis)
";

    #[test]
    fn the_screens_are_the_connected_outputs_with_their_places() {
        let screens = parse_screens(XRANDR);
        assert_eq!(
            screens,
            vec![
                Screen {
                    name: "Virtual-1".to_owned(),
                    geometry: "1920x1080+0+0".to_owned(),
                },
                Screen {
                    name: "Virtual-2".to_owned(),
                    geometry: "1920x1080+1920+0".to_owned(),
                },
            ],
            "the disconnected head or the mode list got in"
        );
    }

    #[test]
    fn a_connected_output_with_no_mode_is_a_screen_with_nowhere_to_draw() {
        let screens = parse_screens(
            "Virtual-2 connected (normal left inverted right x axis y axis)\n   1920x1080 59.96\n",
        );
        assert_eq!(screens.len(), 1);
        assert!(screens[0].geometry.is_empty(), "{:?}", screens[0]);
        assert!(
            screen_report(&screens, 1).contains("no mode"),
            "a screen with nothing on it reads as one that has something"
        );
    }

    #[test]
    fn a_session_that_came_up_with_fewer_screens_than_were_asked_for_says_so() {
        let report = screen_report(&parse_screens(XRANDR), 3);
        assert!(report.contains("warning:"), "{report}");
        assert!(report.contains("3 screens were asked for"), "{report}");
        assert!(report.contains("has 2"), "{report}");
    }

    /// Two screens at one origin is what X does on its own, and it counts as
    /// two everywhere except on the console, where it is one picture twice.
    #[test]
    fn two_screens_stacked_on_one_origin_are_not_a_desktop_across_both() {
        let stacked = "\
Virtual-1 connected primary 1920x1080+0+0 (normal left inverted right x axis y axis) 0mm x 0mm
Virtual-2 connected 1920x1080+0+0 (normal left inverted right x axis y axis) 0mm x 0mm
";
        let report = screen_report(&parse_screens(stacked), 2);
        assert!(report.contains("share an origin"), "{report}");

        // And the layout that is right says nothing beyond what it is.
        let placed = screen_report(&parse_screens(XRANDR), 2);
        assert!(!placed.contains("warning:"), "{placed}");
        assert!(placed.contains("Virtual-1 1920x1080+0+0"), "{placed}");
        assert!(placed.contains("Virtual-2 1920x1080+1920+0"), "{placed}");
    }

    /// A pointer that covers a desktop wider than the window it is driven from
    /// clicks somewhere else, so a guest that could not be mapped has to say so
    /// rather than look like one that was.
    #[test]
    fn the_pointer_says_which_screen_it_clicks_on_and_which_it_does_not() {
        let mapped = pointer_report(&format!("something first\n{POINTER_MARK}Virtual-1\n"));
        assert!(mapped.contains("Virtual-1"), "{mapped}");
        assert!(!mapped.contains("warning"), "{mapped}");
        // The other screens are not the same as the mapped one, and a reader who
        // is not told that will click on them and wonder.
        assert!(mapped.contains("looking at"), "{mapped}");

        for answer in [format!("{POINTER_MARK}none"), String::new()] {
            let missing = pointer_report(&answer);
            assert!(missing.contains("warning"), "{missing}");
            assert!(missing.contains("xinput"), "{missing}");
        }
    }

    /// The command runs in a session it did not start, and everything it needs
    /// to reach that session is in the file the guest contract writes at login.
    #[test]
    fn the_screen_placement_takes_the_sessions_own_environment() {
        let command = place_screens_command();
        assert!(
            command.contains(&format!(
                ". {}/session.env",
                crate::provider::GUEST_ROOT_LINUX
            )),
            "{command}"
        );
        assert!(command.contains("DISPLAY"), "{command}");
        // The placement is the point: without --right-of the second output
        // lands on the first and the guest has one screen shown twice.
        assert!(command.contains("--right-of"), "{command}");
        // And it ends by asking what happened rather than by trusting that it
        // worked, which is what the report reads.
        assert!(command.trim_end().ends_with("xrandr --query"), "{command}");
    }

    /// The same two facts for a guest that was staged but whose hand-over did
    /// not take: the shortcuts are there and the enhanced session is not.
    #[test]
    fn a_failed_hand_over_still_describes_the_console_the_guest_has() {
        let text = lifecycle_explainer(
            Image::Windows,
            prepared(ProviderKind::HyperV, Staging::Done, false),
        );
        assert!(text.contains("Sunlit Earth"), "{text}");
        assert!(text.contains("basic session"), "{text}");
        assert!(!text.contains("leave that field empty"), "{text}");
    }

    #[test]
    fn the_command_that_keeps_a_guest_is_the_one_that_runs_in_that_image() {
        assert_eq!(
            keep_command(Image::Linux),
            "cargo xtask e2e --target linux --keep"
        );
        // Not `e2e`, which has no image to run its suite in here, and not the
        // image's own slug, which neither command takes. `--no-verify` is what
        // makes the builder the last guest of the run and so the one kept.
        assert_eq!(
            keep_command(Image::WindowsBuilder),
            "cargo xtask dist --target windows --keep --no-verify"
        );
        for image in Image::ALL {
            let text = keep_command(image);
            assert!(!text.contains("--image"), "{text}");
            assert!(!text.contains("builder"), "{text}");
        }
    }

    /// The flag on its own keeps the last guest of the run, which for a
    /// verifying `dist` is the desktop image: a text offering the builder has
    /// to name the command that actually leaves the builder up.
    #[test]
    fn the_command_that_keeps_a_builder_is_one_that_keeps_a_builder() {
        use crate::commands::dist::{Boot, Options, Which, keeps_guest};
        for image in Image::ALL {
            let text = keep_command(image);
            if !image.is_builder() {
                assert!(!text.contains("--no-verify"), "{text}");
                continue;
            }
            let options = Options {
                which: match image.target() {
                    Target::Windows => Which::Windows,
                    Target::Linux => Which::Linux,
                },
                keep: true,
                verify: !text.contains("--no-verify"),
                cache: true,
                allow_expired: false,
                allow_dirty: false,
            };
            let boot = Boot {
                target: image.target(),
                builder: true,
                failed: false,
            };
            assert!(
                keeps_guest(options, boot),
                "`{text}` does not keep the {image} guest"
            );
        }
    }

    /// The image with no session must not be offered a desktop, and the ones
    /// with a session must not be offered a console: a closing text is one
    /// screen about one guest, and the Windows builder is where it used to give
    /// both answers, offering a console four lines above two sentences about its
    /// desktop.
    #[test]
    fn the_line_that_offers_a_console_calls_it_what_the_image_has() {
        for image in Image::ALL {
            let expected = if image.has_desktop() {
                "desktop: cargo xtask vm view"
            } else {
                "console: cargo xtask vm view"
            };
            let text = lifecycle_explainer(image, bare(ProviderKind::Qemu, Staging::Skipped));
            assert!(text.contains(expected), "{text}");
            let store = Store::new("/srv/vm");
            let hint = Session {
                provider: Box::new(fake::Fake),
                state: fake::state(&store, image, StartReason::Run),
                image,
            }
            .reach_hint();
            assert!(hint.contains(expected), "{hint}");
        }
        // The image with no session is the one that must not be offered a
        // desktop, and the desktop images are the ones whose text explains what
        // is missing when nothing was staged into one.
        let linux = lifecycle_explainer(Image::Linux, bare(ProviderKind::Qemu, Staging::Skipped));
        assert!(linux.contains("no app in"), "{linux}");
        assert!(
            lifecycle_explainer(
                Image::WindowsBuilder,
                bare(ProviderKind::HyperV, Staging::Skipped)
            )
            .contains("desktop: cargo xtask vm view windows-builder"),
        );
    }

    /// The whole difference between a boot and a resume, on the path where it
    /// matters most. A boot's failure tears its guest down because it made the
    /// overlay; a resume was called to keep one, and the failure it is most
    /// likely to hit is a guest running a repair pass after decision 2's kill,
    /// which is the moment the build directory is worth the most and the moment
    /// this used to delete it.
    #[test]
    fn a_resume_that_fails_keeps_the_overlay_and_the_record() {
        let store = fake::store("failed_resume_keeps_the_overlay");
        let image = Image::WindowsBuilder;
        let mut state = fake::state(&store, image, StartReason::Up);
        // What a resume starts from: a record that said stopped, cleared on the
        // way in by `resume` itself, and an overlay with a build directory in it.
        state.stopped = false;
        std::fs::write(&state.overlay, b"a cargo build directory").expect("the overlay");
        write_state(&store, image, &state).expect("the record");

        let mut session = Session {
            // Not running: the resume failed before the guest existed.
            provider: Box::new(fake::Undestroyable(false)),
            state,
            image,
        };
        let text = after_failed_resume(&mut session, &store);

        assert!(session.state.overlay.is_file(), "{text}");
        let recorded = load_state(&store, image).expect("the record is still there");
        assert!(recorded.stopped, "{recorded:?}");
        assert!(
            text.contains("cargo xtask vm start windows-builder"),
            "{text}"
        );
        assert!(
            text.contains("cargo xtask vm down windows-builder"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(store.root());
    }

    /// The other half: a guest that is up and not answering is left up. Powering
    /// it off would start whatever it is doing over, and the record already says
    /// what is true, so the only thing this owes is a way to look at it and two
    /// ways out that differ in what they cost.
    #[test]
    fn a_resume_that_fails_with_the_guest_up_leaves_it_up_and_says_so() {
        let store = fake::store("failed_resume_leaves_it_up");
        let image = Image::WindowsBuilder;
        let mut state = fake::state(&store, image, StartReason::Up);
        state.stopped = false;
        std::fs::write(&state.overlay, b"a cargo build directory").expect("the overlay");
        write_state(&store, image, &state).expect("the record");

        let mut session = Session {
            provider: Box::new(fake::Undestroyable(true)),
            state,
            image,
        };
        let text = after_failed_resume(&mut session, &store);

        assert!(session.state.overlay.is_file(), "{text}");
        assert!(text.contains("it is still running"), "{text}");
        // The record is left alone, because a running guest is what it says.
        let recorded = load_state(&store, image).expect("the record is still there");
        assert!(!recorded.stopped, "{recorded:?}");
        assert!(
            text.contains("cargo xtask vm ssh windows-builder"),
            "{text}"
        );
        assert!(
            text.contains("cargo xtask vm stop windows-builder"),
            "{text}"
        );
        assert!(
            text.contains("cargo xtask vm down windows-builder"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(store.root());
    }

    /// Neither message may read like a teardown: what a person does after a
    /// failed resume depends entirely on believing the overlay is still there.
    #[test]
    fn neither_failed_resume_message_says_anything_was_destroyed() {
        for image in Image::ALL.into_iter().filter(|image| image.is_builder()) {
            for text in [
                resume_left_running(image, "sunlit-e2e-x"),
                resume_left_stopped(image, "sunlit-e2e-x"),
            ] {
                assert!(!text.contains("was destroyed"), "{text}");
                assert!(text.contains("untouched"), "{text}");
                assert!(
                    text.contains(&format!("cargo xtask vm down {image}")),
                    "{text}"
                );
            }
        }
    }

    /// A run kept after its work failed is as finished as one kept after its
    /// work passed, so its record has to say so: `StartReason::Dist` carries a
    /// cost of ending, and the `vm down` this very message recommends would
    /// warn about ending a build that stopped when the failure did.
    #[test]
    fn a_guest_kept_after_a_failure_is_no_longer_recorded_as_the_work_that_failed() {
        let store = fake::store("kept_after_failure");
        let mut session = Session {
            provider: Box::new(fake::Fake),
            state: fake::state(&store, Image::LinuxBuilder, StartReason::Dist),
            image: Image::LinuxBuilder,
        };
        write_state(&store, session.image, &session.state).expect("write the record");

        let text = after_failure(&mut session, &store, true);
        assert!(text.contains("still running"), "{text}");

        let recorded = load_state(&store, session.image).expect("the record is still there");
        assert_eq!(recorded.reason, StartReason::Keep);
        assert_eq!(recorded.reason.cost_of_ending(), None);
        let _ = std::fs::remove_dir_all(store.root());
    }

    /// What a teardown removes is the guest's own run state and nothing else:
    /// the record, the overlay, and the scratch the job script was written into.
    /// The last of those is why a green run used to leave run state behind under
    /// an image with nothing running.
    #[test]
    fn a_teardown_removes_the_run_state_it_wrote_and_leaves_the_rest() {
        let store = fake::store("tear_down_run_state");
        let image = Image::LinuxBuilder;
        let state = fake::state(&store, image, StartReason::Dist);
        let session = Session {
            provider: Box::new(fake::Fake),
            state: state.clone(),
            image,
        };

        assert_eq!(
            run_state_paths(&store, image, &state),
            [
                store.state_file(image),
                state.overlay.clone(),
                store.job_scratch(image),
                store.handover_scratch(image),
                store.firmware_vars(image),
            ]
        );

        write_state(&store, image, &state).expect("write the record");
        std::fs::write(&state.overlay, b"overlay").expect("write the overlay");
        std::fs::create_dir_all(store.job_scratch(image)).expect("the job scratch");
        std::fs::write(store.job_scratch(image).join("job.sh"), b"echo hi").expect("the script");
        // Written by staging in every Windows guest and by every QEMU boot
        // respectively, so a run that leaves either behind leaves run state
        // under an image with nothing running.
        std::fs::create_dir_all(store.handover_scratch(image)).expect("the handover scratch");
        std::fs::write(
            store.handover_scratch(image).join("run-app.cmd"),
            b"@echo off",
        )
        .expect("the launcher");
        std::fs::write(store.firmware_vars(image), b"vars").expect("the firmware variables");
        std::fs::write(store.vm_log(image), b"qemu").expect("the log");
        // The bundle a release build assembles is in the run directory too, and
        // it is the one thing there that outlives the guest it was staged into:
        // `dist` writes the final record into it and archives it after the
        // verification boot is over. It was on the list above once, and the
        // teardown then deleted the bundle that boot had just proved.
        std::fs::create_dir_all(store.bundle_scratch(image)).expect("the bundle scratch");
        std::fs::write(store.bundle_scratch(image).join("sunlit-earth"), b"elf")
            .expect("the bundled binary");

        session.tear_down(&store).expect("the teardown");

        assert!(!store.state_file(image).exists());
        assert!(!state.overlay.exists());
        assert!(!store.job_scratch(image).exists());
        assert!(!store.handover_scratch(image).exists());
        assert!(!store.firmware_vars(image).exists());
        // The log is what a failed boot's message quotes and names, so a
        // teardown is not what removes it.
        assert!(store.vm_log(image).is_file());
        assert!(store.bundle_scratch(image).is_dir());
        // What removes the bundle is `dist` itself, and what sweeps one a run
        // died in the middle of is `vm down`, which takes the run directory
        // whole rather than reading this list.
        assert!(
            store
                .bundle_scratch(image)
                .starts_with(store.run_dir(image))
        );
        let _ = std::fs::remove_dir_all(store.root());
    }

    /// A store in a directory of its own and a provider that answers the two
    /// questions a teardown asks, for the cases that write files and read them
    /// back.
    mod fake {
        use super::*;
        use crate::provider::target::ProviderKind;

        pub fn store(name: &str) -> Store {
            let root = std::env::temp_dir().join(format!("sunlit_xtask_vm_{name}"));
            let _ = std::fs::remove_dir_all(&root);
            let store = Store::new(&root);
            for image in Image::ALL {
                std::fs::create_dir_all(store.run_dir(image)).expect("the run directory");
            }
            store
        }

        pub fn state(store: &Store, image: Image, reason: StartReason) -> RunState {
            RunState::new(image, ProviderKind::Qemu, store.overlay(image), reason, 0)
        }

        /// Everything a teardown consults is real; everything else is
        /// unreachable, because nothing in these cases boots anything.
        pub struct Fake;

        impl crate::provider::Provider for Fake {
            fn kind(&self) -> ProviderKind {
                ProviderKind::Qemu
            }
            fn create_from_golden(&self, _: Image, _: StartReason) -> Result<RunState, String> {
                unreachable!("these cases create nothing")
            }
            fn start(&self, _: &mut RunState) -> Result<(), String> {
                unreachable!("these cases start nothing")
            }
            fn destroy(&self, _: &RunState) -> Result<crate::provider::Stopped, String> {
                Ok(crate::provider::Stopped::WasNotRunning)
            }
            fn stop(&self, _: &RunState) -> Result<crate::provider::Stopped, String> {
                unreachable!("these cases stop nothing")
            }
            fn is_running(&self, _: &RunState) -> bool {
                false
            }
            fn defunct(&self, _: &RunState) -> Option<String> {
                None
            }
            fn view(&self, _: &RunState) -> Result<String, String> {
                unreachable!("these cases open no console")
            }
            fn ssh_target(&self, state: &RunState) -> crate::guest::ssh::SshTarget {
                crate::guest::ssh::SshTarget::from_state(state, std::path::Path::new("id_ed25519"))
            }
            fn runner(&self) -> &dyn crate::runner::Runner {
                unreachable!("these cases run nothing")
            }
        }

        /// A guest that says whether it is up and refuses to be destroyed, for
        /// the cases whose whole point is that a failure kept the overlay: if
        /// the path under test ever tears one down, the case dies here rather
        /// than passing on a message that says otherwise.
        pub struct Undestroyable(pub bool);

        impl crate::provider::Provider for Undestroyable {
            fn kind(&self) -> ProviderKind {
                Fake.kind()
            }
            fn create_from_golden(&self, _: Image, _: StartReason) -> Result<RunState, String> {
                unreachable!("these cases create nothing")
            }
            fn start(&self, _: &mut RunState) -> Result<(), String> {
                unreachable!("these cases start nothing")
            }
            fn destroy(&self, _: &RunState) -> Result<crate::provider::Stopped, String> {
                unreachable!("a failed resume destroys nothing")
            }
            fn stop(&self, _: &RunState) -> Result<crate::provider::Stopped, String> {
                unreachable!("these cases stop nothing")
            }
            fn is_running(&self, _: &RunState) -> bool {
                self.0
            }
            fn defunct(&self, _: &RunState) -> Option<String> {
                None
            }
            fn view(&self, _: &RunState) -> Result<String, String> {
                unreachable!("these cases open no console")
            }
            fn ssh_target(&self, state: &RunState) -> crate::guest::ssh::SshTarget {
                Fake.ssh_target(state)
            }
            fn runner(&self) -> &dyn crate::runner::Runner {
                unreachable!("these cases run nothing")
            }
        }
    }

    /// Decision 1, in both directions. What the one-VM rule protects is a host
    /// oversubscribed by two guests each sized for the whole of it, and a
    /// builder beside a guest under test is not that pair: it is the compiler
    /// beside the thing it compiles for, which is what a Windows host has always
    /// had in WSL.
    #[test]
    fn a_builder_may_run_beside_a_guest_under_test_and_two_desktop_guests_may_not() {
        for target in Target::ALL {
            let desktop = Image::desktop(target);
            let builder = Image::builder(target);
            assert!(may_run_beside(desktop, builder), "{target}");
            assert!(may_run_beside(builder, desktop), "{target}");
        }
        // Across targets too: what the exemption is about is what the guest is
        // for, not which operating system is in it.
        assert!(may_run_beside(Image::Linux, Image::WindowsBuilder));
        assert!(may_run_beside(Image::WindowsBuilder, Image::LinuxBuilder));
        // And the pair the rule was written about stays refused.
        assert!(!may_run_beside(Image::Windows, Image::Linux));
        assert!(!may_run_beside(Image::Linux, Image::Windows));
    }

    /// The exemption comes with the number, because what makes two guests fine
    /// is this host's memory rather than a principle, and a smaller host is the
    /// case that breaks.
    #[test]
    fn a_boot_beside_another_guest_names_it_and_what_the_two_hold() {
        let line = beside_line(Image::Windows, Image::WindowsBuilder);
        assert!(line.contains("sunlit-e2e-windows-builder"), "{line}");
        let total = u64::from(crate::provider::resources_for(Image::Windows).0)
            + u64::from(crate::provider::resources_for(Image::WindowsBuilder).0);
        assert!(
            line.contains(&util::format_bytes(total * 1024 * 1024)),
            "{line}"
        );
        // One line, because that is the whole output budget decision 9 gives it.
        assert_eq!(line.lines().count(), 1, "{line}");
    }

    /// Decision 5. The refusal quotes the reason rather than hiding behind
    /// "unsupported", because the reason is the same one that keeps a compiler
    /// out of these images and it is worth reading twice.
    #[test]
    fn stopping_a_guest_the_suite_runs_in_is_refused_by_naming_the_pristine_overlay() {
        for image in Image::ALL.into_iter().filter(|image| !image.is_builder()) {
            let text = persistence_refusal(image);
            assert!(text.contains("pristine overlay"), "{text}");
            assert!(
                text.contains(&format!("cargo xtask vm down {image}")),
                "{text}"
            );
            // And it says where the two commands do apply, so the reader is not
            // left thinking they do not exist.
            assert!(text.contains("The builders are what"), "{text}");
            assert!(text.contains("build directory"), "{text}");
        }
    }

    /// A stopped builder is a build directory somebody kept, and `vm up` and
    /// `dist` both want a pristine overlay: they take it, and the line says what
    /// went with it rather than calling it a leftover.
    #[test]
    fn a_boot_that_discards_a_stopped_builder_says_so_rather_than_calling_it_a_leftover() {
        let store = Store::new("/srv/vm");
        let mut state = fake::state(&store, Image::WindowsBuilder, StartReason::Suite);
        let crashed = clearing_line(Image::WindowsBuilder, &state);
        assert!(
            crashed.contains("left behind by an earlier run"),
            "{crashed}"
        );

        state.stopped = true;
        let stopped = clearing_line(Image::WindowsBuilder, &state);
        assert!(stopped.contains("build directory"), "{stopped}");
        assert!(
            stopped.contains("cargo xtask vm start windows-builder"),
            "{stopped}"
        );
        assert_eq!(stopped.lines().count(), 1, "{stopped}");
    }

    /// What a stop keeps matters as much as what it frees: the overlay is
    /// gigabytes, and the command that reclaims it is the one thing a person who
    /// never runs it will wish they had been told.
    #[test]
    fn a_stop_says_what_it_kept_what_it_freed_and_what_reclaims_it() {
        let line = stopped_line(
            Image::WindowsBuilder,
            "sunlit-e2e-windows-builder",
            provider::Stopped::ShutDown,
            Some(6 * 1024 * 1024 * 1024),
        );
        assert!(line.contains("is stopped"), "{line}");
        assert!(line.contains("6.0 GiB"), "{line}");
        assert!(
            line.contains("cargo xtask vm start windows-builder"),
            "{line}"
        );
        assert!(
            line.contains("cargo xtask vm down windows-builder"),
            "{line}"
        );
        assert_eq!(line.lines().count(), 1, "{line}");

        // A guest that had already gone away is recorded as stopped anyway, so
        // the next command resumes rather than clears, and the line does not
        // claim a stop that did not happen.
        let gone = stopped_line(
            Image::LinuxBuilder,
            "sunlit-e2e-linux-builder",
            provider::Stopped::WasNotRunning,
            None,
        );
        assert!(gone.contains("was not running"), "{gone}");

        // A guest that had to be killed kept its overlay too, and the cost of
        // that overlay is the sentence a clean stop does not carry: whoever
        // resumes it meets a repair pass, and this is where they hear about it.
        let killed = stopped_line(
            Image::WindowsBuilder,
            "sunlit-e2e-windows-builder",
            provider::Stopped::Killed,
            Some(6 * 1024 * 1024 * 1024),
        );
        assert!(killed.contains("was killed"), "{killed}");
        assert!(killed.contains("repair pass"), "{killed}");
        assert!(killed.contains("6.0 GiB"), "{killed}");
        assert!(
            killed.contains("cargo xtask vm start windows-builder"),
            "{killed}"
        );
        assert_eq!(killed.lines().count(), 1, "{killed}");
    }

    /// The two guests `is_running` cannot tell apart want opposite advice, and
    /// the record is the only thing that says which is which.
    #[test]
    fn a_stopped_guest_is_offered_a_resume_and_a_crashed_one_a_teardown() {
        let store = Store::new("/srv/vm");
        let mut state = fake::state(&store, Image::WindowsBuilder, StartReason::Suite);
        let crashed = not_running_hint(Image::WindowsBuilder, &state);
        assert!(
            crashed.contains("cargo xtask vm down windows-builder"),
            "{crashed}"
        );
        assert!(!crashed.contains("vm start"), "{crashed}");

        state.stopped = true;
        let stopped = not_running_hint(Image::WindowsBuilder, &state);
        assert!(
            stopped.contains("cargo xtask vm start windows-builder"),
            "{stopped}"
        );
        assert!(!stopped.contains("vm down"), "{stopped}");
    }

    /// A builder is not a guest the suite runs in, and on this host the suite is
    /// compiled in that very image: staging into it would mean a build whose
    /// stopped guest the same command then discards.
    #[test]
    fn a_boot_stages_binaries_into_the_guests_the_suite_runs_in_and_no_others() {
        for image in Image::ALL {
            assert_eq!(stages_binaries(image), !image.is_builder(), "{image}");
        }
    }

    /// Criterion 7. A builder needs less said about it than a desktop guest,
    /// not more: the parts of that text about shortcuts, sessions and clipboards
    /// are about a guest somebody was handed, and a builder is a compiler.
    #[test]
    fn a_builders_closing_text_drops_every_part_of_a_desktop_guests_that_is_not_true_of_it() {
        for image in Image::ALL.into_iter().filter(|image| image.is_builder()) {
            let text = lifecycle_explainer(image, bare(ProviderKind::HyperV, Staging::Skipped));
            // Nothing is staged in a builder, so there is nothing to click.
            assert!(!text.contains(crate::guest::handover::ENTRY_NAME), "{text}");
            assert!(
                !text.contains(crate::guest::handover::APP_SHORTCUT.trim_end_matches(".lnk")),
                "{text}"
            );
            assert!(!text.contains("SLINT_BACKEND"), "{text}");
            // No choice of console session, and nothing to paste into one.
            assert!(!text.contains("enhanced session"), "{text}");
            assert!(!text.contains("basic session"), "{text}");
            assert!(!text.contains("clipboard"), "{text}");
            assert!(
                !text.contains(crate::provider::hyperv::CREDENTIAL_DIALOG_CAVEAT),
                "{text}"
            );
            // And no run in it to be told not to click during.
            assert!(!text.contains("Watching a run"), "{text}");
            // What it does have is a lifetime, which is the one thing the
            // desktop guests' text denies exists.
            assert!(!text.contains("no way to do is save or pause"), "{text}");
            assert!(
                text.contains(&format!("cargo xtask vm stop {image}")),
                "{text}"
            );
            assert!(
                text.contains(&format!("cargo xtask vm start {image}")),
                "{text}"
            );
            assert!(
                text.contains(&format!("cargo xtask vm down {image}")),
                "{text}"
            );

            // Shorter than the desktop guest's of the same target, which is the
            // direction decision 9 asks for.
            let desktop = lifecycle_explainer(
                Image::desktop(image.target()),
                prepared(ProviderKind::HyperV, Staging::Done, true),
            );
            assert!(
                text.len() < desktop.len(),
                "the {image} text is longer than the desktop guest's: {text}"
            );
        }
    }

    #[test]
    fn a_detached_layer_is_refused_by_naming_the_rebuild_of_the_layer() {
        let text = detached_help(
            Image::WindowsBuilder,
            "the windows image was rebuilt: golden.vhdx is crc32:1 now and this \
             layer was built over crc32:2",
        );
        // The layer, not its parent: rebuilding the parent is what caused this.
        assert!(
            text.contains("cargo xtask vm build-image windows-builder"),
            "{text}"
        );
        assert!(text.contains("was rebuilt"), "{text}");
        // The reason a disk whose own checksum matches is unusable anyway.
        assert!(text.contains("only what its own build changed"), "{text}");
    }
}
