//! The provider trait and its two implementations.
//!
//! Plan decision 7. QEMU is a process plus a QMP socket; `Hyper-V` is a set of
//! `PowerShell` cmdlets. What they have in common is the guest contract, so
//! everything that speaks to the guest rather than to the hypervisor is a
//! default method here, implemented once over SSH.

pub mod qemu;

use std::path::Path;
use std::time::Duration;

use crate::runner::{CommandOutput, Runner};
use crate::ssh::{self, SshTarget};
use crate::state::{RunState, StartReason};
use crate::store::Store;
use crate::target::{HostOs, ProviderKind, Target};
use crate::util;

/// Where the guest keeps everything, on both operating systems.
pub const GUEST_ROOT_LINUX: &str = "sunlit-e2e";
pub const GUEST_ROOT_WINDOWS: &str = r"C:\sunlit-e2e";

/// How long to wait for the desktop session after SSH answers.
pub const SESSION_TIMEOUT: Duration = Duration::from_secs(300);

/// One hypervisor, driven.
pub trait Provider {
    fn kind(&self) -> ProviderKind;

    /// Make a throwaway child of the read-only golden image and record it.
    /// Does not boot anything.
    fn create_from_golden(&self, target: Target, reason: StartReason) -> Result<RunState, String>;

    /// Boot it, filling in how to reach it.
    fn start(&self, state: &mut RunState) -> Result<(), String>;

    /// Stop it and delete the overlay and the state file.
    fn destroy(&self, state: &RunState) -> Result<(), String>;

    /// Whether the VM is alive right now.
    fn is_running(&self, state: &RunState) -> bool;

    /// Open the guest's console, or explain how to.
    fn view(&self, state: &RunState) -> Result<String, String>;

    /// How to reach the guest over SSH.
    fn ssh_target(&self, state: &RunState) -> SshTarget;

    /// The runner, so the default methods can reach a process.
    fn runner(&self) -> &dyn Runner;

    /// Whether the host this is running on is Windows, which changes the null
    /// device the SSH client is pointed at.
    fn windows_host(&self) -> bool;

    /// Block until the guest's SSH server answers.
    fn wait_ssh(&self, state: &RunState, timeout: Duration) -> Result<Duration, String> {
        ssh::wait_ready(
            self.runner(),
            &self.ssh_target(state),
            timeout,
            ssh::POLL_INTERVAL,
            self.windows_host(),
        )
    }

    /// Run one command in the guest.
    fn exec(&self, state: &RunState, command: &str) -> Result<CommandOutput, String> {
        ssh::exec(
            self.runner(),
            &self.ssh_target(state),
            command,
            self.windows_host(),
        )
    }

    /// Copy a file or directory into the guest.
    fn copy_in(&self, state: &RunState, local: &Path, remote: &str) -> Result<(), String> {
        let cmd = ssh::scp_to_command(&self.ssh_target(state), local, remote, self.windows_host());
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
        let cmd =
            ssh::scp_from_command(&self.ssh_target(state), remote, local, self.windows_host());
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
    let kind =
        crate::target::resolve_provider(host, target, util::env_var(PROVIDER_ENV).as_deref())?;
    match kind {
        ProviderKind::Qemu => Ok(Box::new(qemu::QemuProvider::new(runner, store, host))),
        ProviderKind::HyperV => Err(HYPERV_PENDING.to_owned()),
    }
}

/// Override for the provider matrix, mostly so a Windows host can be pushed
/// onto QEMU for the Windows guest.
pub const PROVIDER_ENV: &str = "SUNLIT_EARTH_VM_PROVIDER";

/// Until the `Hyper-V` provider lands, a Windows host can still drive the
/// Windows guest by taking the Linux host's route through QEMU.
const HYPERV_PENDING: &str = "the Hyper-V provider is not implemented yet; set \
     SUNLIT_EARTH_VM_PROVIDER=qemu to drive the Windows guest through QEMU instead";

/// The provider that matches an existing state file, so `vm destroy` tears down
/// what actually exists rather than what the matrix would create today.
pub fn for_state<'a>(
    runner: &'a dyn Runner,
    store: &'a Store,
    state: &RunState,
) -> Result<Box<dyn Provider + 'a>, String> {
    let host = HostOs::current();
    match state.provider_kind() {
        Some(ProviderKind::Qemu) => Ok(Box::new(qemu::QemuProvider::new(runner, store, host))),
        Some(ProviderKind::HyperV) => Err(HYPERV_PENDING.to_owned()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_paths_use_each_operating_systems_separator() {
        assert_eq!(guest_root(Target::Linux), "sunlit-e2e");
        assert_eq!(guest_root(Target::Windows), r"C:\sunlit-e2e");
        assert_eq!(guest_results(Target::Linux), "sunlit-e2e/results");
        assert_eq!(guest_results(Target::Windows), r"C:\sunlit-e2e\results");
        assert_eq!(guest_bin(Target::Linux), "sunlit-e2e/bin");
        assert_eq!(guest_bin(Target::Windows), r"C:\sunlit-e2e\bin");
    }

    #[test]
    fn the_linux_guest_paths_are_relative_to_the_home_directory() {
        // scp with a relative remote path lands in the login directory, which
        // is where the session marker and the job runner both look.
        assert!(!guest_root(Target::Linux).starts_with('/'));
    }
}
