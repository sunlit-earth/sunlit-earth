//! The state file that sits next to a VM's overlay.
//!
//! Plan decision 13: everything the xtask creates has to be findable again
//! after the orchestrator that created it is gone. A crashed run, a `--keep`
//! run, and a `vm up` all leave the same small file, so `vm status` and
//! `vm destroy` work from a record on disk rather than by scanning the host for
//! things that might be ours.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::target::{ProviderKind, Target, VM_NAME_PREFIX};

pub const STATE_VERSION: u32 = 1;

/// Why a VM exists, which is what `vm status` reports and what tells a reader
/// whether something was left behind deliberately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartReason {
    /// A test run currently in progress.
    Run,
    /// A test run that finished and was kept with `--keep`.
    Keep,
    /// An interactive guest from `vm up`.
    Up,
}

impl StartReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::Run => "test run",
            Self::Keep => "kept after a test run (--keep)",
            Self::Up => "interactive (vm up)",
        }
    }
}

/// What one running or left-behind VM is, and how to reach it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunState {
    #[serde(default)]
    pub format_version: u32,
    pub target: String,
    pub provider: String,
    pub vm_name: String,
    pub overlay: PathBuf,
    #[serde(default)]
    pub ssh_host: String,
    #[serde(default)]
    pub ssh_port: u16,
    #[serde(default)]
    pub ssh_user: String,
    /// The always-on localhost VNC address of a QEMU guest, if any.
    #[serde(default)]
    pub vnc: Option<String>,
    #[serde(default)]
    pub qmp_port: Option<u16>,
    /// The QEMU process id. `Hyper-V` guests have none: the VM is owned by the
    /// hypervisor, not by a process of ours.
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub started_unix: u64,
    pub reason: StartReason,
}

impl RunState {
    pub fn new(
        target: Target,
        provider: ProviderKind,
        overlay: PathBuf,
        reason: StartReason,
        started_unix: u64,
    ) -> Self {
        Self {
            format_version: STATE_VERSION,
            target: target.slug().to_owned(),
            provider: provider.name().to_owned(),
            vm_name: target.vm_name(),
            overlay,
            ssh_host: String::new(),
            ssh_port: 0,
            ssh_user: String::new(),
            vnc: None,
            qmp_port: None,
            pid: None,
            started_unix,
            reason,
        }
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("malformed VM state file: {e}"))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn provider_kind(&self) -> Option<ProviderKind> {
        ProviderKind::parse(&self.provider)
    }

    /// Whether this record describes something the xtask created.
    ///
    /// `vm destroy` refuses to act on anything that fails this, so a
    /// hand-edited or corrupted state file cannot aim the teardown at another
    /// VM on the host.
    pub fn is_ours(&self) -> bool {
        self.vm_name.starts_with(VM_NAME_PREFIX)
            && Target::ALL.iter().any(|t| t.slug() == self.target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RunState {
        let mut state = RunState::new(
            Target::Linux,
            ProviderKind::Qemu,
            PathBuf::from("/srv/vm/run/linux/overlay.qcow2"),
            StartReason::Keep,
            1_755_600_000,
        );
        state.ssh_host = "127.0.0.1".to_owned();
        state.ssh_port = 2222;
        state.ssh_user = "tester".to_owned();
        state.vnc = Some("127.0.0.1:5900".to_owned());
        state.qmp_port = Some(4444);
        state.pid = Some(1234);
        state
    }

    #[test]
    fn a_state_file_round_trips() {
        let state = sample();
        let parsed = RunState::from_json(&state.to_json()).expect("round trip");
        assert_eq!(parsed, state);
        assert_eq!(parsed.provider_kind(), Some(ProviderKind::Qemu));
    }

    #[test]
    fn the_reason_is_stored_in_lowercase() {
        let json = sample().to_json();
        assert!(json.contains(r#""reason": "keep""#), "{json}");
        let parsed = RunState::from_json(&json.replace(r#""keep""#, r#""up""#)).expect("parses");
        assert_eq!(parsed.reason, StartReason::Up);
    }

    #[test]
    fn a_minimal_state_file_parses() {
        let parsed = RunState::from_json(
            r#"{"target":"windows","provider":"hyperv","vm_name":"sunlit-e2e-windows",
                "overlay":"C:/vm/run/windows/overlay.vhdx","reason":"run"}"#,
        )
        .expect("defaults fill the rest");
        assert!(parsed.is_ours());
        assert_eq!(parsed.pid, None);
        assert_eq!(parsed.ssh_port, 0);
    }

    #[test]
    fn a_state_file_naming_a_foreign_vm_is_not_ours() {
        let mut state = sample();
        state.vm_name = "production-db".to_owned();
        assert!(!state.is_ours());

        let mut other = sample();
        other.target = "solaris".to_owned();
        assert!(!other.is_ours());
    }

    #[test]
    fn a_truncated_state_file_is_an_error_not_a_default() {
        assert!(RunState::from_json("{\"target\":").is_err());
        assert!(RunState::from_json("{}").is_err());
    }
}
