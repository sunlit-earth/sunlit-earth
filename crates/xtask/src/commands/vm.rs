//! The VM lifecycle commands: `up`, `ssh`, `view`, `status`, `down`, `purge`,
//! guest-contract smoke test.
//!
//! `vm up` and `e2e --image <t>` share the whole boot path, which is what
//! makes an interactive guest and a test guest the same guest.

use std::time::Duration;

use crate::commands::status;
use crate::commands::teardown::{self, Selection};
use crate::guest::job;
use crate::provider::desktop::Desktop;
use crate::provider::target::{Image, Target};
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

        let mut problems = Vec::new();
        for path in [&store.state_file(self.image), &self.state.overlay] {
            if let Err(e) = std::fs::remove_file(path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                problems.push(format!("{}: {e}", path.display()));
            }
        }
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

        println!("waiting for the desktop session");
        let elapsed = job::wait_for_session(
            self.provider.as_ref(),
            &self.state,
            self.target(),
            SESSION_TIMEOUT,
        )?;
        println!(
            "  the desktop was ready after {:.0}s",
            elapsed.as_secs_f64()
        );
        Ok(())
    }

    /// How to reach and get rid of this guest, for when something has gone
    /// wrong and it is still running.
    pub fn reach_hint(&self) -> String {
        let image = self.image;
        format!(
            "  ssh:     cargo xtask vm ssh {image}\n  \
             desktop: cargo xtask vm view {image}\n  \
             down:    cargo xtask vm down {image}"
        )
    }
}

/// Deal with a guest after something went wrong with it.
///
/// A failure after the boot used to leave a VM running with no message and no
/// hint, which is the worst of both: it holds its memory, it blocks the next
/// run's ports, and nothing said it was there. Either it goes, or it is named
/// along with the command that removes it.
pub fn after_failure(session: &Session, store: &Store, keep: bool) -> String {
    if keep {
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

/// Refuse to start a second VM (plan decision 7: one at a time).
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
        if running {
            let cost = state
                .reason
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
    }
    Ok(())
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
/// a run whose results are about a desktop nobody chose. A builder is the
/// sharper case, because it has no desktop at all.
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
             a builder carries no desktop at all, which is what keeps it small and \
             what keeps a compiler out of the images the suite runs in"
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
    desktop: Option<Desktop>,
) -> Result<Session<'a>, String> {
    let desktop = desktop_for(image, desktop)?;
    check_image(store, image, allow_expired)?;
    check_no_other_vm(runner, store, image)?;
    clear_stale_state(runner, store, image)?;

    let provider = provider::for_image(runner, store, image)?;
    println!("creating a throwaway overlay of the {image} golden image");
    let mut state = provider.create_from_golden(image, reason)?;
    // Recorded before the VM is started, because the provider builds the guest's
    // fw_cfg argument out of the record rather than out of a parameter.
    state.desktop = desktop.map(|d| d.flag().to_owned());

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
            println!("{}", after_failure(&session, store, false));
            Err(e)
        }
    }
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
    println!("clearing the {image} VM left behind by an earlier run");

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

/// What was done to the guest this text is about.
///
/// Two of the paragraphs below describe what happens at the end of a command
/// rather than at the boot: the binaries with their launcher and desktop
/// shortcuts, and the enhanced session. `vm up` and `e2e --keep` do both,
/// `vm smoke --keep` does neither, and a hand-over that failed did only the
/// first. So the text is printed from what happened rather than from the image,
/// which is what it was doing when it told the owner of an empty desktop which
/// shortcut to double-click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prepared {
    /// Staging ran, so the binaries, the launcher and the desktop shortcuts
    /// went into the guest. Not that every part of it arrived: the shortcuts are
    /// convenience, so `artifacts::stage` warns about them and carries on, and
    /// that warning is on the line above this text rather than a command away.
    pub staged: bool,
    /// The guest confirmed it can offer an enhanced `vmconnect` session.
    pub enhanced_session: bool,
}

impl Prepared {
    /// A guest left as it stood: nothing staged in it and nothing handed over.
    pub const BARE: Self = Self {
        staged: false,
        enhanced_session: false,
    };
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
    format!(
        "\n\
         {vm} is up.\n  \
         ssh:     cargo xtask vm ssh {image}\n  \
         desktop: cargo xtask vm view {image}\n  \
         down:    cargo xtask vm down {image}\n\n\
         `vm down` is the stop, and an idle guest is worth stopping: it holds \
         {memory} of this host's memory for as long as it is up. What there is no \
         way to do is save or pause one, and nothing in here is worth saving, so \
         ending a guest and discarding it are the same act: the teardown frees \
         the memory and the overlay, leaves the golden image untouched, and the \
         next `vm up` boots something pristine.\n\n\
         Watching a run is harmless; clicking during one perturbs it.{session}{extra}",
        vm = image.vm_name(),
        memory = guest_memory(image),
        session = view_note(image, prepared.enhanced_session),
        extra = guest_environment_note(image, prepared.staged)
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
fn view_note(image: Image, enhanced_session: bool) -> String {
    match (image.target(), enhanced_session) {
        (Target::Windows, true) => "\n\nIts desktop opens in an enhanced session, which is the \
             one that can be resized: drag the window and the guest's desktop \
             follows. The dialog asks for the guest's account, `tester`, with no \
             password at all, so leave that field empty and connect. A guest \
             with a test run in it offers none of this and opens a basic session \
             instead, because an enhanced one would take the console session out \
             from under the run.\n\n\
             An enhanced session is RDP, so it carries the clipboard: text can \
             be pasted straight in. Files go in over `vm ssh` and scp."
            .to_owned(),
        (Target::Windows, false) => format!(
            "\n\nIts desktop opens in a basic session: nothing to type, and fixed \
             at the console resolution, because only an enhanced session can be \
             resized and this guest is not offering one. `vm up` and `e2e --keep` \
             are what turn that on.\n\n\
             A basic session carries no clipboard, so text and files go in over \
             `vm ssh` and scp.\n\n{}",
            crate::provider::hyperv::CREDENTIAL_DIALOG_CAVEAT
        ),
        (Target::Linux, _) => "\n\nThe console carries no clipboard integration, so text and \
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
fn guest_environment_note(image: Image, staged: bool) -> String {
    match (image.target(), staged) {
        (Target::Windows, true) => format!(
            "\n\nTwo shortcuts are on its desktop. `{app}` starts the app through \
             a launcher that sets `SLINT_BACKEND={backend}` for it: this guest has \
             no OpenGL, and without that the app exits before a window appears. \
             `{folder}` opens the directory the binaries, fixtures and results \
             are in.",
            app = crate::guest::handover::APP_SHORTCUT.trim_end_matches(".lnk"),
            folder = crate::guest::handover::FOLDER_SHORTCUT.trim_end_matches(".lnk"),
            backend = crate::commands::e2e::WINDOWS_SLINT_BACKEND
        ),
        (Target::Windows, false) => format!(
            "\n\nNothing of ours was staged in it, so its desktop is empty and \
             there is no app in it to start. `cargo xtask vm up {image}` boots a \
             guest with the binaries, the launcher and the shortcuts, and \
             `cargo xtask e2e --image {image} --keep` leaves one behind after a \
             run."
        ),
        (Target::Linux, true) => format!(
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
        (Target::Linux, false) => format!(
            "\n\nNothing of ours was staged in it, so there is no app in \
             `{root}` to start and no entry for one. `cargo xtask vm up {image}` \
             boots a guest with the binaries, the launcher and the desktop \
             entries, and `cargo xtask e2e --image {image} --keep` leaves one \
             behind after a run.",
            root = crate::provider::GUEST_ROOT_LINUX,
        ),
    }
}

/// `vm up`.
pub fn up(
    runner: &dyn Runner,
    image: Image,
    allow_expired: bool,
    desktop: Option<Desktop>,
) -> Result<u8, String> {
    let store = store::store()?;
    // Asked before anything is created: a guest with no binaries to put in it
    // is worse than a refusal.
    crate::guest::artifacts::check_can_build(
        crate::provider::target::HostOs::current(),
        image.target(),
    )?;

    let mut session = boot(
        runner,
        &store,
        image,
        StartReason::Up,
        allow_expired,
        desktop,
    )?;
    // Decision 14: an interactive guest carries the current binaries, exactly
    // as a test run would, so `vm up` and `e2e --keep` land in the same place.
    if let Err(e) = crate::guest::artifacts::stage(runner, &store, &session) {
        // Keep the guest: `vm up` is for looking at one, and a guest that
        // booted is still worth having even if the binaries did not arrive.
        println!("{}", after_failure(&session, &store, true));
        return Err(e);
    }
    let enhanced_session = hand_over(&mut session, &store);
    println!(
        "{}",
        lifecycle_explainer(
            session.image,
            Prepared {
                staged: true,
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

/// Record what a guest is once the command that booted it is finished with it.
///
/// A guest booted for a run and then kept is no longer a run in progress, and
/// `vm status` reads the reason to say why something is still there. Nothing
/// used to write [`StartReason::Keep`] at all, so a kept guest reported itself
/// as a run for as long as it existed.
fn record_kept(session: &mut Session, store: &Store) {
    if session.state.reason == StartReason::Run {
        session.state.reason = StartReason::Keep;
    }
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
        return Err(format!(
            "{} is recorded but not running. `cargo xtask vm down {image}` \
             clears it and `cargo xtask vm up {image}` starts a fresh one.",
            state.vm_name
        ));
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
             and `cargo xtask e2e --image {image} --keep` leaves the aftermath \
             of a test run to look at."
        )
    })?;
    let provider = provider::for_state(runner, &store, &state)?;
    if !provider.is_running(&state) {
        return Err(format!(
            "{} is recorded but not running; there is no console to attach to. \
             `cargo xtask vm up {image}` starts a fresh one.",
            state.vm_name
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
    desktop: Option<Desktop>,
) -> Result<u8, String> {
    let store = store::store()?;
    let started = std::time::Instant::now();
    let mut session = boot(runner, &store, image, StartReason::Run, false, desktop)?;

    let script = smoke_script(image);

    println!("running a trivial job through the guest contract");
    let scratch = store.run_dir(image).join("job");
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
            println!("{}", after_failure(&session, &store, keep));
            return Err(e);
        }
    };

    let results = store.results_dir(image);
    if let Err(e) = session.provider.collect_results(
        &session.state,
        &provider::guest_results(image.target()),
        &results,
    ) {
        println!("{}", after_failure(&session, &store, keep));
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
        println!("{}", lifecycle_explainer(image, Prepared::BARE));
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
/// What it asks the guest differs by what the guest has. A desktop image is
/// asked to prove there is an X server the job can reach, which is the half of
/// the contract a builder cannot have; a builder is asked for the toolchain
/// instead, which is the thing that makes it one. Asking either question of the
/// wrong image prints a command-not-found line that reads as a defect.
pub fn smoke_script(image: Image) -> String {
    match image.target() {
        Target::Windows => concat!(
            "@echo off\r\n",
            "echo sunlit-e2e smoke\r\n",
            "hostname\r\n",
            "whoami\r\n",
            "echo %SUNLIT_E2E_ARTIFACTS%\r\n",
            "echo smoke > %SUNLIT_E2E_ARTIFACTS%\\smoke.txt\r\n",
        )
        .to_owned(),
        Target::Linux => format!(
            "#!/usr/bin/env bash\n\
             set -eux\n\
             echo 'sunlit-e2e smoke'\n\
             hostname\n\
             id\n\
             echo \"DISPLAY=$DISPLAY\"\n\
             {probe}\
             echo smoke > \"$SUNLIT_E2E_ARTIFACTS/smoke.txt\"\n",
            probe = if image.has_desktop() {
                "xdpyinfo -display \"$DISPLAY\" | head -3\n"
            } else {
                "\"$HOME/.cargo/bin/cargo\" -V\n"
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
            Prepared {
                staged: true,
                enhanced_session: false,
            },
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
                lifecycle_explainer(image, Prepared::BARE)
                    .contains(&format!("{expected} of this host's memory")),
                "{image} is described as holding something other than {expected}"
            );
        }
        assert!(
            lifecycle_explainer(Image::Windows, Prepared::BARE).contains("6.0 GiB"),
            "the Windows desktop guest's own figure"
        );
        assert!(
            lifecycle_explainer(Image::WindowsBuilder, Prepared::BARE).contains("8.0 GiB"),
            "a builder gets more of the host, and the text has to say so"
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
            Prepared {
                staged: true,
                enhanced_session: false,
            },
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

    /// `vm smoke --keep` leaves a Linux guest with nothing of ours in it either,
    /// and being told which entry to click is worse than being told there is
    /// none. The Windows arm has said so since it existed; this is the same rule.
    #[test]
    fn a_bare_linux_guest_promises_no_launcher_and_names_what_would_stage_one() {
        let text = lifecycle_explainer(Image::Linux, Prepared::BARE);
        assert!(text.contains("Nothing of ours was staged"), "{text}");
        assert!(!text.contains(crate::guest::handover::ENTRY_NAME), "{text}");
        assert!(
            !text.contains(&crate::guest::handover::linux_launcher_path()),
            "{text}"
        );
        assert!(text.contains("cargo xtask vm up linux"), "{text}");
        assert!(text.contains("--image linux --keep"), "{text}");
    }

    #[test]
    fn handing_over_a_windows_guest_says_what_the_app_needs_in_it() {
        // The job sets this; a shell does not, and the failure without it names
        // an OpenGL symbol rather than the guest. What sets it for a person is
        // the desktop launcher, so both shortcuts are named as well.
        let text = lifecycle_explainer(
            Image::Windows,
            Prepared {
                staged: true,
                enhanced_session: true,
            },
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
        let text = lifecycle_explainer(Image::Windows, Prepared::BARE);
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
            Prepared::BARE,
            Prepared {
                staged: true,
                enhanced_session: false,
            },
        ] {
            let text = lifecycle_explainer(Image::Windows, prepared);
            assert!(text.contains(caveat), "{text}");
        }
        assert!(crate::provider::hyperv::view_note(Target::Windows, false).contains(caveat));

        // A guest that was handed over expects the dialog and can answer it, so
        // the caveat would contradict the advice it is printed beside.
        let handed_over = lifecycle_explainer(
            Image::Windows,
            Prepared {
                staged: true,
                enhanced_session: true,
            },
        );
        assert!(!handed_over.contains(caveat), "{handed_over}");
        assert!(!crate::provider::hyperv::view_note(Target::Windows, true).contains(caveat));
        // And a Linux guest has no vmconnect dialog to be surprised by.
        assert!(
            !lifecycle_explainer(
                Image::Linux,
                Prepared {
                    staged: true,
                    enhanced_session: false,
                }
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

            // A builder is the sharper case: it has no desktop at all, and
            // saying "the other image has one" would send somebody looking for
            // a session that is not in there.
            for builder in [Image::WindowsBuilder, Image::LinuxBuilder] {
                let refusal =
                    desktop_for(builder, Some(desktop)).expect_err("a builder has no desktop");
                assert!(refusal.contains("no desktop at all"), "{refusal}");
            }
        }
        // Neither guest has to be asked. On Linux that is the image's own
        // default session, which is the whole point of the flag being optional.
        for image in Image::ALL {
            assert_eq!(desktop_for(image, None), Ok(None), "{image}");
        }
    }

    /// The same two facts for a guest that was staged but whose hand-over did
    /// not take: the shortcuts are there and the enhanced session is not.
    #[test]
    fn a_failed_hand_over_still_describes_the_console_the_guest_has() {
        let text = lifecycle_explainer(
            Image::Windows,
            Prepared {
                staged: true,
                enhanced_session: false,
            },
        );
        assert!(text.contains("Sunlit Earth"), "{text}");
        assert!(text.contains("basic session"), "{text}");
        assert!(!text.contains("leave that field empty"), "{text}");
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
