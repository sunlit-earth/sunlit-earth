//! The VM lifecycle commands: `up`, `ssh`, `view`, `status`, `destroy`, and the
//! guest-contract smoke test.
//!
//! `vm up` and `e2e --target <t>` share the whole boot path, which is what
//! makes an interactive guest and a test guest the same guest.

use std::time::Duration;

use crate::commands::destroy::{self, Selection};
use crate::commands::status;
use crate::guest::job;
use crate::provider::target::Target;
use crate::provider::{self, Provider};
use crate::runner::Runner;
use crate::store::inventory::{self, ImageCondition};
use crate::store::manifest::EvalState;
use crate::store::state::{RunState, StartReason};
use crate::store::{self, Store};
use crate::util;

/// How long a cold boot may take before the SSH server answers.
pub const BOOT_TIMEOUT: Duration = Duration::from_secs(600);

/// How long the desktop session may take after that.
pub const SESSION_TIMEOUT: Duration = Duration::from_secs(300);

/// A booted guest and the provider that owns it.
pub struct Session<'a> {
    pub provider: Box<dyn Provider + 'a>,
    pub state: RunState,
    pub target: Target,
}

impl Session<'_> {
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
        for path in [&store.state_file(self.target), &self.state.overlay] {
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
    /// and destroy, and `destroy::plan` would then see an overlay with no VM
    /// behind it and unlink the disk of a live guest. Afterwards, because that
    /// is when the process id and the address exist to record.
    fn bring_up(&mut self, store: &Store) -> Result<(), String> {
        write_state(store, self.target, &self.state)?;

        println!(
            "starting {} on {}",
            self.state.vm_name,
            self.provider.kind().name()
        );
        self.provider.start(&mut self.state)?;
        write_state(store, self.target, &self.state)?;

        println!("waiting for the guest to answer on SSH");
        let elapsed = self.provider.wait_ssh(&self.state, BOOT_TIMEOUT)?;
        println!("  SSH answered after {:.0}s", elapsed.as_secs_f64());

        println!("waiting for the desktop session");
        let elapsed = job::wait_for_session(
            self.provider.as_ref(),
            &self.state,
            self.target,
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
        let target = self.target;
        format!(
            "  ssh:     cargo xtask vm ssh {target}\n  \
             desktop: cargo xtask vm view {target}\n  \
             destroy: cargo xtask vm destroy {target}"
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
pub fn expired_help(target: Target, state: EvalState) -> String {
    format!(
        "The {target} golden image's evaluation has expired.\n\n{}\n\n\
         An expired evaluation does not refuse to boot. Windows starts shutting \
         itself down about once an hour, so a run inside it fails in the middle \
         of whatever it was doing rather than failing cleanly. That is why this \
         is checked before booting instead of diagnosed afterwards.\n\n\
         Rebuild it:  cargo xtask vm build-image {target}\n\
         Boot anyway: add --allow-expired-image",
        state.summary()
    )
}

/// Refuse to boot from an image that cannot produce a trustworthy run.
pub fn check_image(store: &Store, target: Target, allow_expired: bool) -> Result<(), String> {
    let inventory = inventory::scan(store);
    let Some(entry) = inventory.for_target(target) else {
        return Err(format!("nothing is known about the {target} image"));
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
                    "warning: the {target} image is stale: {}",
                    condition.detail()
                );
                println!(
                    "it still runs; `cargo xtask vm build-image {target}` brings it up to date"
                );
            }
            other => println!(
                "warning: the {target} image is {}: {}",
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
        ImageCondition::Expired { state } => Err(expired_help(target, state)),
        ImageCondition::Missing => Err(format!(
            "no {target} golden image yet. `cargo xtask vm build-image {target}` builds one."
        )),
        ImageCondition::Unmanifested { detail, .. } => Err(format!(
            "the {target} image has no usable manifest: {detail}.\n\n\
             The manifest is where the build timestamp lives, and that is the \
             only record of when the evaluation licence started running. \
             Without it there is no telling an image with two months left from \
             one that will start shutting itself down mid-run, which is the \
             failure this check exists to prevent.\n\n\
             Rebuild it: cargo xtask vm build-image {target}"
        )),
        ImageCondition::Corrupt { detail } => Err(format!(
            "the {target} golden image does not match its manifest: {detail}. \
             `cargo xtask vm build-image {target}` rebuilds it."
        )),
        // Everything else returned above, where `blocks_boot` said so.
        other => Err(format!(
            "the {target} image is not usable: {}",
            other.detail()
        )),
    }
}

/// Refuse to start a second VM (plan decision 7: one at a time).
pub fn check_no_other_vm(runner: &dyn Runner, store: &Store, target: Target) -> Result<(), String> {
    let inventory = inventory::scan(store);
    for entry in &inventory.targets {
        let Some(state) = &entry.state else { continue };
        let Some(other) = entry.target else { continue };
        if other == target {
            continue;
        }
        let running = provider::for_state(runner, store, state)
            .map(|p| p.is_running(state))
            .unwrap_or(false);
        if running {
            return Err(format!(
                "{} is already running, and this phase runs one VM at a time \
                 (they share the same forwarded ports).\n\
                 `cargo xtask vm destroy {other}` frees it.",
                state.vm_name
            ));
        }
    }
    Ok(())
}

/// Save the state file. Called as soon as the VM exists, so that a crash from
/// here on still leaves something `vm status` can see and `vm destroy` can
/// clean up.
pub fn write_state(store: &Store, target: Target, state: &RunState) -> Result<(), String> {
    let path = store.state_file(target);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, state.to_json())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Read the state file, if there is one for a VM of ours.
pub fn load_state(store: &Store, target: Target) -> Option<RunState> {
    let text = std::fs::read_to_string(store.state_file(target)).ok()?;
    RunState::from_json(&text).ok().filter(RunState::is_ours)
}

/// Boot a pristine overlay and wait for it to be usable.
pub fn boot<'a>(
    runner: &'a dyn Runner,
    store: &'a Store,
    target: Target,
    reason: StartReason,
    allow_expired: bool,
) -> Result<Session<'a>, String> {
    check_image(store, target, allow_expired)?;
    check_no_other_vm(runner, store, target)?;
    clear_stale_state(runner, store, target)?;

    let provider = provider::for_target(runner, store, target)?;
    println!("creating a throwaway overlay of the {target} golden image");
    let state = provider.create_from_golden(target, reason)?;

    // From here the VM exists: for Hyper-V it is registered, for QEMU its
    // overlay is on disk. Everything after this point goes through
    // `after_failure`, so no failure can return while leaving one running.
    let mut session = Session {
        provider,
        state,
        target,
    };
    match session.bring_up(store) {
        Ok(()) => Ok(session),
        Err(e) => {
            println!("{}", after_failure(&session, store, false));
            Err(e)
        }
    }
}

/// Take down whatever an earlier run left recorded for this target.
///
/// The record is removed only once the teardown has succeeded, which is the
/// same rule `destroy::execute` follows and for the same reason: deleting the
/// record of a VM that is still registered makes it invisible to `vm status`
/// and `vm destroy` for good. That is reachable on a host where the `Hyper-V`
/// cmdlets fail, which is a plain missing group membership away.
fn clear_stale_state(runner: &dyn Runner, store: &Store, target: Target) -> Result<(), String> {
    let Some(existing) = load_state(store, target) else {
        return Ok(());
    };
    println!("clearing the {target} VM left behind by an earlier run");

    let provider = provider::for_state(runner, store, &existing).map_err(|e| {
        format!(
            "{} is recorded for {target}, but {e}. The record is left in place \
             rather than deleted; `cargo xtask vm destroy {target}` clears it.",
            existing.vm_name
        )
    })?;

    let session = Session {
        provider,
        state: existing,
        target,
    };
    session.tear_down(store).map_err(|e| {
        format!(
            "the {target} VM left behind by an earlier run could not be taken \
             down: {e}\nIts record is kept rather than deleted, because a VM \
             that is still registered and no longer recorded cannot be found \
             again. `cargo xtask vm destroy {target}` retries this."
        )
    })
}

/// What `vm up` prints when it is done (plan decision 14).
pub fn lifecycle_explainer(target: Target) -> String {
    format!(
        "\n\
         {vm} is up.\n  \
         ssh:     cargo xtask vm ssh {target}\n  \
         desktop: cargo xtask vm view {target}\n  \
         destroy: cargo xtask vm destroy {target}\n\n\
         There is no stop or pause. This guest holds no state worth keeping, so \
         ending it and discarding it are the same act: destroying it frees the \
         memory and the overlay, leaves the golden image untouched, and the next \
         `vm up` boots something pristine. Until then it holds its RAM.\n\n\
         Watching a run is harmless; clicking during one perturbs it. The \
         hypervisor console has no clipboard integration, so text and files go \
         in over `vm ssh` and scp.",
        vm = target.vm_name()
    )
}

/// `vm up`.
pub fn up(runner: &dyn Runner, target: Target, allow_expired: bool) -> Result<u8, String> {
    let store = store::store()?;
    // Asked before anything is created: a guest with no binaries to put in it
    // is worse than a refusal.
    crate::guest::artifacts::check_can_build(crate::provider::target::HostOs::current(), target)?;

    let session = boot(runner, &store, target, StartReason::Up, allow_expired)?;
    // Decision 14: an interactive guest carries the current binaries, exactly
    // as a test run would, so `vm up` and `e2e --keep` land in the same place.
    if let Err(e) = crate::guest::artifacts::stage(runner, &store, &session) {
        // Keep the guest: `vm up` is for looking at one, and a guest that
        // booted is still worth having even if the binaries did not arrive.
        println!("{}", after_failure(&session, &store, true));
        return Err(e);
    }
    println!("{}", lifecycle_explainer(session.target));
    Ok(0)
}

/// `vm ssh`.
pub fn ssh(runner: &dyn Runner, target: Target, extra: &[String]) -> Result<u8, String> {
    let store = store::store()?;
    let state = load_state(&store, target).ok_or_else(|| {
        format!("no {target} VM is recorded. `cargo xtask vm up {target}` starts one.")
    })?;
    let provider = provider::for_state(runner, &store, &state)?;
    if !provider.is_running(&state) {
        return Err(format!(
            "{} is recorded but not running. `cargo xtask vm destroy {target}` \
             clears it and `cargo xtask vm up {target}` starts a fresh one.",
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
    let cmd =
        crate::guest::ssh::ssh_command(&ssh_target, remote.as_deref(), provider.windows_host())
            .interactive();
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
pub fn view(runner: &dyn Runner, target: Target) -> Result<u8, String> {
    let store = store::store()?;
    let state = load_state(&store, target).ok_or_else(|| {
        format!(
            "no {target} VM is running. `cargo xtask vm up {target}` starts one, \
             and `cargo xtask e2e --target {target} --keep` leaves the aftermath \
             of a test run to look at."
        )
    })?;
    let provider = provider::for_state(runner, &store, &state)?;
    if !provider.is_running(&state) {
        return Err(format!(
            "{} is recorded but not running; there is no console to attach to. \
             `cargo xtask vm up {target}` starts a fresh one.",
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
    for entry in &mut inventory.targets {
        if let Some(state) = &entry.state {
            entry.running = provider::for_state(runner, &store, state)
                .ok()
                .map(|p| p.is_running(state));
        }
    }
    print!("{}", status::render(&inventory, util::now_unix()));
    Ok(0)
}

/// `vm destroy`.
pub fn destroy_command(
    runner: &dyn Runner,
    selection: Selection,
    purge: bool,
) -> Result<u8, String> {
    let store = store::store()?;
    let inventory = inventory::scan(&store);
    let plan = destroy::plan(&store, &inventory, selection, purge);
    print!("{}", plan.render());
    if plan.is_empty() {
        return Ok(0);
    }
    // Asked unconditionally. Whether there is anything to stop is the
    // provider's question to answer, and answering it here by consulting
    // `is_running` first is what let a registered but powered-off Hyper-V VM
    // keep its registration while its disk was deleted out from under it.
    let outcome = destroy::execute(&plan, &|state| {
        provider::for_state(runner, &store, state)?.destroy(state)
    });
    print!("{}", destroy::render_outcome(&outcome, purge));
    Ok(u8::from(!outcome.problems.is_empty()))
}

/// The guest-contract smoke test: boot, run a trivial job, collect it, destroy.
///
/// This is where the contract is proved and where boot and poll timings get
/// calibrated, deliberately separate from the e2e suite so that a failure here
/// means the plumbing and a failure there means the product.
pub fn smoke(runner: &dyn Runner, target: Target, keep: bool) -> Result<u8, String> {
    let store = store::store()?;
    let started = std::time::Instant::now();
    let session = boot(runner, &store, target, StartReason::Run, false)?;

    let script = match target {
        Target::Windows => concat!(
            "@echo off\r\n",
            "echo sunlit-e2e smoke\r\n",
            "hostname\r\n",
            "whoami\r\n",
            "echo %SUNLIT_E2E_ARTIFACTS%\r\n",
            "echo smoke > %SUNLIT_E2E_ARTIFACTS%\\smoke.txt\r\n",
        ),
        Target::Linux => concat!(
            "#!/usr/bin/env bash\n",
            "set -eux\n",
            "echo 'sunlit-e2e smoke'\n",
            "hostname\n",
            "id\n",
            "echo \"DISPLAY=$DISPLAY\"\n",
            "xdpyinfo -display \"$DISPLAY\" | head -3\n",
            "echo smoke > \"$SUNLIT_E2E_ARTIFACTS/smoke.txt\"\n",
        ),
    };

    println!("running a trivial job through the guest contract");
    let scratch = store.run_dir(target).join("job");
    // From here on the VM exists, so `?` would leave it running unannounced.
    let code = match job::run(
        session.provider.as_ref(),
        &session.state,
        target,
        script,
        &scratch,
        Duration::from_secs(300),
    ) {
        Ok(code) => code,
        Err(e) => {
            println!("{}", after_failure(&session, &store, keep));
            return Err(e);
        }
    };

    let results = store.results_dir(target);
    if let Err(e) =
        session
            .provider
            .collect_results(&session.state, &provider::guest_results(target), &results)
    {
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
        println!("{}", lifecycle_explainer(target));
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

#[cfg(test)]
mod tests {
    #[test]
    fn an_interactive_shell_inherits_stdin_and_a_remote_command_does_not_change_that() {
        // `stream` gives a child a closed stdin unless the command says
        // otherwise, which is right for every orchestration step and wrong for
        // the one command whose purpose is to hand over the terminal.
        let target = crate::guest::ssh::SshTarget {
            user: "tester".to_owned(),
            host: "127.0.0.1".to_owned(),
            port: 2222,
            key: std::path::PathBuf::from("/srv/vm/ssh/id_ed25519"),
        };
        let shell = crate::guest::ssh::ssh_command(&target, None, true).interactive();
        assert!(
            shell.interactive,
            "a shell nobody can type into is not a shell"
        );
        // Every other ssh invocation stays non-interactive on purpose.
        let probe = crate::guest::ssh::ssh_command(&target, Some("echo hi"), true);
        assert!(!probe.interactive);
    }

    use super::*;
    use crate::store::manifest::eval_state;
    use crate::util::SECS_PER_DAY;

    #[test]
    fn the_expiry_help_explains_the_symptom_and_both_ways_out() {
        let text = expired_help(Target::Windows, eval_state(0, 95 * SECS_PER_DAY));
        assert!(text.contains("expired"), "{text}");
        assert!(text.contains("once an hour"), "{text}");
        assert!(
            text.contains("cargo xtask vm build-image windows"),
            "{text}"
        );
        assert!(text.contains("--allow-expired-image"), "{text}");
    }

    #[test]
    fn the_lifecycle_explainer_says_there_is_no_stop() {
        let text = lifecycle_explainer(Target::Linux);
        assert!(text.contains("no stop or pause"), "{text}");
        assert!(text.contains("golden image untouched"), "{text}");
        assert!(text.contains("holds its RAM"), "{text}");
        assert!(text.contains("cargo xtask vm destroy linux"), "{text}");
        assert!(text.contains("clicking during one perturbs it"), "{text}");
        assert!(text.contains("clipboard"), "{text}");
    }
}
