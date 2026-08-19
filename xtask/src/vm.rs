//! The VM lifecycle commands: `up`, `ssh`, `view`, `status`, `destroy`, and the
//! guest-contract smoke test.
//!
//! `vm up` and `e2e --target <t>` share the whole boot path, which is what
//! makes an interactive guest and a test guest the same guest.

use std::time::Duration;

use crate::destroy::{self, Selection};
use crate::inventory::{self, ImageCondition};
use crate::job;
use crate::manifest::EvalState;
use crate::provider::{self, Provider};
use crate::runner::Runner;
use crate::state::{RunState, StartReason};
use crate::status;
use crate::store::{self, Store};
use crate::target::Target;
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
        if let ImageCondition::Stale { .. } = condition {
            println!(
                "warning: the {target} image is stale: {}",
                condition.detail()
            );
            println!("it still runs; `cargo xtask vm build-image {target}` brings it up to date");
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

    // A VM already recorded for this target is stale run state; take it down
    // rather than booting a second one onto the same ports.
    if let Some(existing) = load_state(store, target) {
        println!("clearing the {target} VM left behind by an earlier run");
        if let Ok(existing_provider) = provider::for_state(runner, store, &existing) {
            let _ = existing_provider.destroy(&existing);
        }
        let _ = std::fs::remove_file(store.state_file(target));
    }

    let provider = provider::for_target(runner, store, target)?;
    println!("creating a throwaway overlay of the {target} golden image");
    let mut state = provider.create_from_golden(target, reason)?;

    println!("starting {} on {}", state.vm_name, provider.kind().name());
    provider.start(&mut state)?;
    write_state(store, target, &state)?;

    println!("waiting for the guest to answer on SSH");
    let elapsed = provider.wait_ssh(&state, BOOT_TIMEOUT)?;
    println!("  SSH answered after {:.0}s", elapsed.as_secs_f64());

    println!("waiting for the desktop session");
    let elapsed = job::wait_for_session(provider.as_ref(), &state, target, SESSION_TIMEOUT)?;
    println!(
        "  the desktop was ready after {:.0}s",
        elapsed.as_secs_f64()
    );

    Ok(Session {
        provider,
        state,
        target,
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
    let session = boot(runner, &store, target, StartReason::Up, allow_expired)?;
    // Decision 14: an interactive guest carries the current binaries, exactly
    // as a test run would, so `vm up` and `e2e --keep` land in the same place.
    crate::artifacts::stage(runner, &store, &session)?;
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
    let cmd = crate::ssh::ssh_command(
        &provider.ssh_target(&state),
        remote.as_deref(),
        provider.windows_host(),
    );
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
    println!("{}", provider.view(&state)?);
    println!(
        "Use the basic session, not an enhanced one: enhanced session mode is RDP \
         underneath and logs into a session of its own, which locks the console \
         session out from under a running job."
    );
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
    let code = job::run(
        session.provider.as_ref(),
        &session.state,
        target,
        script,
        &scratch,
        Duration::from_secs(300),
    )?;

    let results = store.results_dir(target);
    session
        .provider
        .collect_results(&session.state, &provider::guest_results(target), &results)?;

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
    } else {
        session.provider.destroy(&session.state)?;
        let _ = std::fs::remove_file(store.state_file(target));
        let _ = std::fs::remove_file(&session.state.overlay);
        println!("the VM is destroyed and the overlay is gone");
    }
    Ok(u8::from(code != 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::eval_state;
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
