//! The provider trait and its two implementations.
//!
//! Plan decision 7. QEMU is a process plus a QMP socket; `Hyper-V` is a set of
//! `PowerShell` cmdlets. What they have in common is the guest contract, so
//! everything that speaks to the guest rather than to the hypervisor is a
//! default method here, implemented once over SSH.

pub mod firmware;
pub mod hyperv;
pub mod qemu;
pub mod qmp;
pub mod target;

use std::path::Path;
use std::time::Duration;

use crate::guest::ssh::{self, SshTarget};
use crate::provider::target::{HostOs, ProviderKind, Target};
use crate::runner::{CommandOutput, Runner};
use crate::store::Store;
use crate::store::state::{RunState, StartReason};
use crate::util;

/// Where the guest keeps everything, on both operating systems.
///
/// Both are absolute, and both are absolute for the same reason: every path
/// that crosses into the guest is used twice, once as an `scp` destination and
/// once inside a script the guest runs, and those two have different working
/// directories. A relative path resolves differently in each, which is not a
/// bug that announces itself: it surfaces as the test harness exiting 127.
///
/// The Linux root deliberately sits outside the home directory as well, so
/// that it does not depend on what the account is called or where its home
/// ended up. `the_guest_roots_match_the_shipped_runners` pins each constant against
/// the script in `vm/` that has to agree with it.
pub const GUEST_ROOT_LINUX: &str = "/var/lib/sunlit-e2e";
pub const GUEST_ROOT_WINDOWS: &str = r"C:\sunlit-e2e";

/// What a destroy actually had to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// A running VM was stopped.
    Stopped,
    /// There was nothing running, though there may have been something
    /// registered to remove.
    WasNotRunning,
}

/// One hypervisor, driven.
pub trait Provider {
    fn kind(&self) -> ProviderKind;

    /// Make a throwaway child of the read-only golden image and record it.
    /// Does not boot anything.
    fn create_from_golden(&self, target: Target, reason: StartReason) -> Result<RunState, String>;

    /// Boot it, filling in how to reach it.
    fn start(&self, state: &mut RunState) -> Result<(), String>;

    /// Stop it and unregister it, if there is anything to stop.
    ///
    /// Returning `Ok` is the caller's licence to delete the overlay, so an
    /// error here has to mean "the VM may still be running" and nothing else.
    fn destroy(&self, state: &RunState) -> Result<Stopped, String>;

    /// Whether the VM is alive right now.
    fn is_running(&self, state: &RunState) -> bool;

    /// Open the guest's console, or explain how to.
    fn view(&self, state: &RunState) -> Result<String, String>;

    /// How to reach the guest over SSH.
    fn ssh_target(&self, state: &RunState) -> SshTarget;

    /// The runner, so the default methods can reach a process.
    fn runner(&self) -> &dyn Runner;

    /// Block until the guest's SSH server answers.
    fn wait_ssh(&self, state: &RunState, timeout: Duration) -> Result<Duration, String> {
        ssh::wait_ready(
            self.runner(),
            &self.ssh_target(state),
            timeout,
            ssh::POLL_INTERVAL,
        )
    }

    /// Run one command in the guest.
    fn exec(&self, state: &RunState, command: &str) -> Result<CommandOutput, String> {
        ssh::exec(self.runner(), &self.ssh_target(state), command)
    }

    /// Copy a file or directory into the guest.
    fn copy_in(&self, state: &RunState, local: &Path, remote: &str) -> Result<(), String> {
        let cmd = ssh::scp_to_command(&self.ssh_target(state), local, remote);
        let out = self
            .runner()
            .capture(&cmd)
            .map_err(|e| format!("cannot run scp: {e}"))?;
        if out.success() {
            Ok(())
        } else {
            Err(format!(
                "copying {} to {remote} failed: {}",
                local.display(),
                out.stderr.trim()
            ))
        }
    }

    /// Pull the results directory back out.
    fn collect_results(&self, state: &RunState, remote: &str, local: &Path) -> Result<(), String> {
        if let Some(parent) = local.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        let _ = std::fs::remove_dir_all(local);
        let cmd = ssh::scp_from_command(&self.ssh_target(state), remote, local);
        let out = self
            .runner()
            .capture(&cmd)
            .map_err(|e| format!("cannot run scp: {e}"))?;
        if out.success() {
            Ok(())
        } else {
            Err(format!("collecting {remote} failed: {}", out.stderr.trim()))
        }
    }
}

/// The provider for one target on this host.
pub fn for_target<'a>(
    runner: &'a dyn Runner,
    store: &'a Store,
    target: Target,
) -> Result<Box<dyn Provider + 'a>, String> {
    let host = HostOs::current();
    let kind = crate::provider::target::resolve_provider(
        host,
        target,
        util::env_var(PROVIDER_ENV).as_deref(),
    )?;
    Ok(match kind {
        ProviderKind::Qemu => Box::new(qemu::QemuProvider::new(runner, store, host)),
        ProviderKind::HyperV => Box::new(hyperv::HypervProvider::new(runner, store, host)),
    })
}

/// Override for the provider matrix, mostly so a Windows host can be pushed
/// onto QEMU for the Windows guest.
pub const PROVIDER_ENV: &str = "SUNLIT_EARTH_VM_PROVIDER";

/// The provider that matches an existing state file, so `vm down` tears down
/// what actually exists rather than what the matrix would create today.
pub fn for_state<'a>(
    runner: &'a dyn Runner,
    store: &'a Store,
    state: &RunState,
) -> Result<Box<dyn Provider + 'a>, String> {
    let host = HostOs::current();
    match state.provider_kind() {
        Some(ProviderKind::Qemu) => Ok(Box::new(qemu::QemuProvider::new(runner, store, host))),
        Some(ProviderKind::HyperV) => {
            Ok(Box::new(hyperv::HypervProvider::new(runner, store, host)))
        }
        None => Err(format!(
            "the state file names an unknown provider '{}'",
            state.provider
        )),
    }
}

/// The guest's root directory, per target.
pub fn guest_root(target: Target) -> &'static str {
    match target {
        Target::Windows => GUEST_ROOT_WINDOWS,
        Target::Linux => GUEST_ROOT_LINUX,
    }
}

/// Where results land inside the guest.
pub fn guest_results(target: Target) -> String {
    match target {
        Target::Windows => format!(r"{GUEST_ROOT_WINDOWS}\results"),
        Target::Linux => format!("{GUEST_ROOT_LINUX}/results"),
    }
}

/// Where binaries are copied to inside the guest.
pub fn guest_bin(target: Target) -> String {
    match target {
        Target::Windows => format!(r"{GUEST_ROOT_WINDOWS}\bin"),
        Target::Linux => format!("{GUEST_ROOT_LINUX}/bin"),
    }
}

/// Where the texture assets are copied to inside the guest.
///
/// The directory keeps its repository name, so that what the job points
/// `SUNLIT_EARTH_TEXTURES` at is the same shape the app finds beside a checkout.
pub fn guest_textures(target: Target) -> String {
    match target {
        Target::Windows => format!(r"{GUEST_ROOT_WINDOWS}\textures"),
        Target::Linux => format!("{GUEST_ROOT_LINUX}/textures"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_paths_use_each_operating_systems_separator() {
        assert_eq!(guest_root(Target::Linux), "/var/lib/sunlit-e2e");
        assert_eq!(guest_root(Target::Windows), r"C:\sunlit-e2e");
        assert_eq!(guest_results(Target::Linux), "/var/lib/sunlit-e2e/results");
        assert_eq!(guest_results(Target::Windows), r"C:\sunlit-e2e\results");
        assert_eq!(guest_bin(Target::Linux), "/var/lib/sunlit-e2e/bin");
        assert_eq!(guest_bin(Target::Windows), r"C:\sunlit-e2e\bin");
        assert_eq!(
            guest_textures(Target::Linux),
            "/var/lib/sunlit-e2e/textures"
        );
        assert_eq!(guest_textures(Target::Windows), r"C:\sunlit-e2e\textures");
    }

    #[test]
    fn every_guest_path_is_absolute() {
        // The same path is used twice: as an scp destination and inside a
        // script the guest runs. Those two do not share a working directory,
        // so a relative path resolves differently in each.
        assert!(guest_root(Target::Linux).starts_with('/'));
        assert!(guest_results(Target::Linux).starts_with('/'));
        assert!(guest_bin(Target::Linux).starts_with('/'));
        assert!(guest_textures(Target::Linux).starts_with('/'));
        assert!(guest_root(Target::Windows).starts_with("C:"));
        assert!(guest_results(Target::Windows).starts_with("C:"));
        assert!(guest_bin(Target::Windows).starts_with("C:"));
        assert!(guest_textures(Target::Windows).starts_with("C:"));
    }

    /// The orchestrator and the scripts baked into the images have to name the
    /// same directory. Nothing else connects them, so a rename on either side
    /// would otherwise be found by a guest that boots and then fails to run
    /// anything.
    ///
    /// Every spelling is pinned, not one per file: the Linux root appears in
    /// the `install -d` that creates the scp destinations, in the session
    /// marker that writes `session.env`, and in the job runner's default, and
    /// a rename that missed any one of them would still break a run.
    #[test]
    fn the_guest_roots_match_the_shipped_runners() {
        let repo = crate::store::repo_root();

        let linux = std::fs::read_to_string(repo.join("vm/linux/scripts/guest-contract.sh"))
            .expect("the Linux guest contract script");
        // The runner's default, the directory creation, and the session
        // marker's own copy of the path.
        for expected in [
            format!("SUNLIT_E2E_ROOT:-{GUEST_ROOT_LINUX}"),
            format!(
                "root={GUEST_ROOT_LINUX}
"
            ),
        ] {
            assert!(
                linux.contains(&expected),
                "the Linux guest contract does not contain {expected:?}"
            );
        }
        // Both scripts it writes set the root, and both must agree.
        assert_eq!(
            linux.matches(&format!("root={GUEST_ROOT_LINUX}")).count()
                + linux
                    .matches(&format!("SUNLIT_E2E_ROOT:-{GUEST_ROOT_LINUX}"))
                    .count(),
            3,
            "the Linux root is spelled a different number of times than expected; \
             every spelling has to be {GUEST_ROOT_LINUX}"
        );
        // Nothing may still reach for a home-relative path.
        assert!(
            !linux.contains("$HOME/sunlit-e2e"),
            "a home-relative path survives in the Linux guest contract"
        );

        let finalize = std::fs::read_to_string(repo.join("vm/linux/scripts/finalize.sh"))
            .expect("the Linux finalize script");
        assert!(
            finalize.contains(GUEST_ROOT_LINUX),
            "finalize.sh clears a different directory than the contract creates"
        );

        for name in ["run-job.cmd", "session-ready.cmd"] {
            let script = std::fs::read_to_string(repo.join("vm/windows/scripts").join(name))
                .unwrap_or_else(|e| panic!("cannot read {name}: {e}"));
            assert!(
                script.contains(&format!("set ROOT={GUEST_ROOT_WINDOWS}")),
                "{name} does not use {GUEST_ROOT_WINDOWS}"
            );
        }

        // The marker `job.rs` polls for is written by the session-ready
        // script, so the two have to agree on its name as well as its
        // directory.
        let ready = std::fs::read_to_string(repo.join("vm/windows/scripts/session-ready.cmd"))
            .expect("the Windows session marker");
        assert!(ready.contains(r"%ROOT%\ready"), "{ready}");
        assert!(
            crate::guest::job::session_ready_command(Target::Windows)
                .contains(&format!(r"{GUEST_ROOT_WINDOWS}\ready")),
            "the orchestrator polls a different marker path than the guest writes"
        );
    }
}
