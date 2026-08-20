//! Guest targets, host operating systems, and the provider matrix that pairs
//! them.

use std::fmt;

use clap::ValueEnum;

/// A guest the desktop e2e suite can run in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, ValueEnum)]
pub enum Target {
    /// The Windows 11 Enterprise evaluation guest: the full suite, including
    /// everything that needs a real Windows desktop session.
    Windows,
    /// The Ubuntu 22.04 GNOME guest: the windowed-mode subset.
    Linux,
}

impl Target {
    /// Both targets, in the order reports list them.
    pub const ALL: [Self; 2] = [Self::Windows, Self::Linux];

    /// The lowercase name used in paths, VM names, and manifests.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
        }
    }

    /// The VM name this target's guest runs under.
    ///
    /// The prefix is what makes every VM the xtask creates recognizable as its
    /// own, so `vm status` and `vm down` never touch anything else on the
    /// host.
    pub fn vm_name(self) -> String {
        format!("{}{}", VM_NAME_PREFIX, self.slug())
    }

    /// Whether this target's image carries an evaluation licence that expires.
    pub fn has_eval_expiry(self) -> bool {
        matches!(self, Self::Windows)
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

/// The prefix every VM, overlay directory, and guest artifact the xtask creates
/// carries. Nothing without it is ever stopped, deleted, or reported.
pub const VM_NAME_PREFIX: &str = "sunlit-e2e-";

/// The host this xtask is running on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOs {
    Windows,
    Linux,
    /// Anything else, macOS in practice. Supported for `e2e --target host`
    /// only: macOS coverage lives on hosted runners, per retrospective 8.3.
    Other,
}

impl HostOs {
    /// The host this binary was compiled for.
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::Other => "unsupported",
        }
    }
}

/// Which hypervisor drives a given guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// `PowerShell` cmdlets, a differencing VHDX child, the Default Switch.
    HyperV,
    /// A QEMU process plus QMP over a localhost socket, qcow2 overlay.
    Qemu,
}

impl ProviderKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::HyperV => "hyperv",
            Self::Qemu => "qemu",
        }
    }

    /// Parse the `SUNLIT_EARTH_VM_PROVIDER` override.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "hyperv" | "hyper-v" => Some(Self::HyperV),
            "qemu" => Some(Self::Qemu),
            _ => None,
        }
    }
}

/// The provider matrix from retrospective section 8.3: `Hyper-V` for the
/// Windows guest on a Windows host, QEMU everywhere else.
///
/// This answers which hypervisor would drive a guest, which is not the same as
/// whether this host can run the suite in one. A Linux host has a hypervisor
/// for the Windows guest and no way to build the binaries to put in it; that
/// question belongs to `artifacts::check_can_build`, which the commands that
/// need binaries ask first.
///
/// The two coexist on a Windows host by design, because QEMU's WHPX
/// acceleration runs on top of the `Hyper-V` hypervisor.
pub fn provider_for(host: HostOs, target: Target) -> Option<ProviderKind> {
    match (host, target) {
        (HostOs::Windows, Target::Windows) => Some(ProviderKind::HyperV),
        (HostOs::Windows, Target::Linux) | (HostOs::Linux, _) => Some(ProviderKind::Qemu),
        (HostOs::Other, _) => None,
    }
}

/// The provider to use, honoring an explicit override.
///
/// An override is how a Windows host can be pushed onto QEMU for the Windows
/// guest, which is the configuration a Linux host uses anyway and therefore the
/// one worth being able to reproduce.
pub fn resolve_provider(
    host: HostOs,
    target: Target,
    override_value: Option<&str>,
) -> Result<ProviderKind, String> {
    if let Some(raw) = override_value {
        return ProviderKind::parse(raw)
            .ok_or_else(|| format!("SUNLIT_EARTH_VM_PROVIDER={raw} is not one of: hyperv, qemu"));
    }
    provider_for(host, target).ok_or_else(|| {
        format!(
            "no VM provider for a {target} guest on a {} host; \
             macOS coverage runs on hosted CI runners, not in local VMs",
            host.name()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_names_carry_the_ownership_prefix() {
        assert_eq!(Target::Windows.vm_name(), "sunlit-e2e-windows");
        assert_eq!(Target::Linux.vm_name(), "sunlit-e2e-linux");
        for target in Target::ALL {
            assert!(target.vm_name().starts_with(VM_NAME_PREFIX));
        }
    }

    #[test]
    fn only_the_windows_guest_has_an_evaluation_clock() {
        assert!(Target::Windows.has_eval_expiry());
        assert!(!Target::Linux.has_eval_expiry());
    }

    #[test]
    fn the_provider_matrix_matches_the_retrospective() {
        assert_eq!(
            provider_for(HostOs::Windows, Target::Windows),
            Some(ProviderKind::HyperV)
        );
        assert_eq!(
            provider_for(HostOs::Windows, Target::Linux),
            Some(ProviderKind::Qemu)
        );
        assert_eq!(
            provider_for(HostOs::Linux, Target::Windows),
            Some(ProviderKind::Qemu)
        );
        assert_eq!(
            provider_for(HostOs::Linux, Target::Linux),
            Some(ProviderKind::Qemu)
        );
        assert_eq!(provider_for(HostOs::Other, Target::Linux), None);
    }

    #[test]
    fn an_unsupported_host_explains_itself_rather_than_defaulting() {
        let err = resolve_provider(HostOs::Other, Target::Windows, None).unwrap_err();
        assert!(err.contains("hosted CI runners"), "{err}");
    }

    #[test]
    fn the_provider_override_wins_and_rejects_nonsense() {
        assert_eq!(
            resolve_provider(HostOs::Windows, Target::Windows, Some("qemu")),
            Ok(ProviderKind::Qemu)
        );
        assert_eq!(
            resolve_provider(HostOs::Linux, Target::Windows, Some("Hyper-V")),
            Ok(ProviderKind::HyperV)
        );
        let err =
            resolve_provider(HostOs::Windows, Target::Windows, Some("virtualbox")).unwrap_err();
        assert!(err.contains("hyperv, qemu"), "{err}");
    }
}
