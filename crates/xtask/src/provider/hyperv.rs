//! The `Hyper-V` provider: `PowerShell` cmdlets and a differencing VHDX.
//!
//! Used for the Windows guest on a Windows host, which is the configuration the
//! product ships on. Everything here goes through `powershell.exe`; there is no
//! management API worth linking for eight commands.
//!
//! Unlike QEMU there is no process of ours to watch and no port to forward: the
//! hypervisor owns the VM, the guest gets an address from the Default Switch,
//! and both liveness and reachability are questions to ask the hypervisor.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::guest::ssh::SshTarget;
use crate::provider::Stopped;
use crate::provider::target::{HostOs, ProviderKind, Target};
use crate::runner::{Cmd, Runner, powershell, ps_quote};
use crate::store::Store;
use crate::store::state::{RunState, StartReason};
use crate::util;

/// The virtual switch every client Windows has out of the box. It NATs the
/// guest onto the host, which is all this needs and is why it is not worth
/// creating one.
pub const SWITCH: &str = "Default Switch";

/// How long to wait for the guest to report an address.
pub const ADDRESS_TIMEOUT: Duration = Duration::from_secs(300);

/// The account the golden image creates.
pub const GUEST_USER: &str = "tester";

/// Memory and processors for the guest.
pub const MEMORY_BYTES: u64 = 6 * 1024 * 1024 * 1024;
pub const CPUS: u32 = 4;

/// The script that creates the VM.
///
/// A differencing child of the read-only golden VHDX, exactly as the QEMU
/// provider makes a qcow2 overlay, so every run starts pristine and nothing
/// ever writes to the image (retrospective 8.3).
///
/// Generation 2 because Windows 11 needs UEFI. Secure Boot is off: the image
/// was installed with the requirement bypassed, and turning it on here would
/// be asserting something about the disk that was never true.
pub fn create_script(name: &str, golden: &str, overlay: &str) -> String {
    format!(
        "New-VHD -Path {overlay} -ParentPath {golden} -Differencing | Out-Null\n\
         New-VM -Name {name} -Generation 2 -MemoryStartupBytes {memory} \
         -VHDPath {overlay} -SwitchName {switch} | Out-Null\n\
         Set-VM -Name {name} -ProcessorCount {cpus} -AutomaticCheckpointsEnabled $false \
         -AutomaticStartAction Nothing -AutomaticStopAction TurnOff\n\
         Set-VMFirmware -VMName {name} -EnableSecureBoot Off\n\
         $drive = Get-VMHardDiskDrive -VMName {name}\n\
         Set-VMFirmware -VMName {name} -FirstBootDevice $drive\n\
         Set-VMMemory -VMName {name} -DynamicMemoryEnabled $false\n",
        name = ps_quote(name),
        golden = ps_quote(golden),
        overlay = ps_quote(overlay),
        switch = ps_quote(SWITCH),
        memory = MEMORY_BYTES,
        cpus = CPUS,
    )
}

/// The script that reports a VM's state, or nothing if it does not exist.
pub fn state_script(name: &str) -> String {
    format!(
        "$vm = Get-VM -Name {name} -ErrorAction SilentlyContinue\n\
         if ($vm) {{ Write-Output \"STATE=$($vm.State)\" }}\n",
        name = ps_quote(name)
    )
}

/// The script that reports the guest's addresses.
///
/// `Get-VMNetworkAdapter` reads them out of the integration services, which
/// every Windows guest has built in, so nothing has to be installed in the
/// guest to make this work.
pub fn address_script(name: &str) -> String {
    format!(
        "$adapter = Get-VMNetworkAdapter -VMName {name} -ErrorAction SilentlyContinue\n\
         if ($adapter) {{ $adapter.IPAddresses | ForEach-Object {{ Write-Output \"IP=$_\" }} }}\n",
        name = ps_quote(name)
    )
}

/// The script that tears the VM down.
///
/// `-TurnOff` rather than a graceful shutdown: the guest holds nothing worth
/// flushing, its disk is a differencing child about to be deleted, and waiting
/// for Windows to shut down politely costs a minute per run.
pub fn destroy_script(name: &str) -> String {
    format!(
        "$vm = Get-VM -Name {name} -ErrorAction SilentlyContinue\n\
         if ($vm) {{\n  \
         if ($vm.State -ne 'Off') {{ Stop-VM -Name {name} -TurnOff -Force }}\n  \
         Remove-VM -Name {name} -Force\n\
         }}\n",
        name = ps_quote(name)
    )
}

/// Read `STATE=` out of the state script's output.
pub fn parse_state(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("STATE="))
        .map(|state| state.trim().to_owned())
}

/// Read the `IP=` lines out of the address script's output.
pub fn parse_addresses(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| line.trim().strip_prefix("IP="))
        .map(|ip| ip.trim().to_owned())
        .filter(|ip| !ip.is_empty())
        .collect()
}

/// The first IPv4 address, which is the one to connect to.
///
/// A guest reports link-local IPv6 addresses long before it has a usable IPv4
/// one, and 169.254 addresses while DHCP is still going, so neither counts as
/// an answer.
pub fn first_usable_ipv4(addresses: &[String]) -> Option<String> {
    addresses
        .iter()
        .find(|address| {
            !address.contains(':')
                && !address.starts_with("169.254.")
                && address.split('.').count() == 4
        })
        .cloned()
}

/// The `Hyper-V` provider.
pub struct HypervProvider<'a> {
    runner: &'a dyn Runner,
    store: &'a Store,
    host: HostOs,
}

impl<'a> HypervProvider<'a> {
    pub fn new(runner: &'a dyn Runner, store: &'a Store, host: HostOs) -> Self {
        Self {
            runner,
            store,
            host,
        }
    }

    fn run_script(&self, script: &str) -> Result<String, String> {
        let out = self
            .runner
            .capture(&powershell(script))
            .map_err(|e| format!("cannot run powershell.exe: {e}"))?;
        if out.success() {
            Ok(out.stdout)
        } else {
            Err(format!(
                "Hyper-V refused: exit {:?}: {}",
                out.code,
                out.stderr.trim()
            ))
        }
    }

    /// The VM's state, `None` if it is not registered.
    ///
    /// The error is kept rather than swallowed: "the cmdlets did not work"
    /// and "there is no such VM" are the same answer to `is_running` and very
    /// different answers to "may I delete this disk now".
    fn query_state(&self, name: &str) -> Result<Option<String>, String> {
        Ok(parse_state(&self.run_script(&state_script(name))?))
    }

    /// Poll until the guest reports a usable address.
    fn wait_for_address(&self, name: &str, timeout: Duration) -> Result<String, String> {
        let start = Instant::now();
        loop {
            if let Ok(out) = self.run_script(&address_script(name))
                && let Some(address) = first_usable_ipv4(&parse_addresses(&out))
            {
                return Ok(address);
            }
            if start.elapsed() >= timeout {
                return Err(format!(
                    "{name} reported no address within {:.0}s. The guest is booting \
                     but not reachable; `cargo xtask vm view windows` shows its console.",
                    timeout.as_secs_f64()
                ));
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    }
}

impl crate::provider::Provider for HypervProvider<'_> {
    fn kind(&self) -> ProviderKind {
        ProviderKind::HyperV
    }

    fn create_from_golden(&self, target: Target, reason: StartReason) -> Result<RunState, String> {
        let golden = self.store.vhdx(target);
        if !golden.is_file() {
            return Err(format!(
                "no golden VHDX at {}; `cargo xtask vm build-image {target}` builds one",
                golden.display()
            ));
        }
        let name = target.vm_name();
        let overlay = self.store.overlay(target);
        std::fs::create_dir_all(self.store.run_dir(target))
            .map_err(|e| format!("cannot create the run directory: {e}"))?;

        // A leftover VM of ours holds the differencing disk open.
        let _ = self.run_script(&destroy_script(&name));
        let _ = std::fs::remove_file(&overlay);

        self.run_script(&create_script(
            &name,
            &golden.to_string_lossy(),
            &overlay.to_string_lossy(),
        ))?;

        let mut state = RunState::new(
            target,
            ProviderKind::HyperV,
            overlay,
            reason,
            util::now_unix(),
        );
        state.ssh_port = 22;
        GUEST_USER.clone_into(&mut state.ssh_user);
        Ok(state)
    }

    fn start(&self, state: &mut RunState) -> Result<(), String> {
        self.run_script(&format!("Start-VM -Name {}", ps_quote(&state.vm_name)))?;
        state.started_unix = util::now_unix();
        println!("waiting for the guest to report an address");
        state.ssh_host = self.wait_for_address(&state.vm_name, ADDRESS_TIMEOUT)?;
        println!("  {} is at {}", state.vm_name, state.ssh_host);
        Ok(())
    }

    fn destroy(&self, state: &RunState) -> Result<Stopped, String> {
        // Asked unconditionally, not only when the VM is running. An Off,
        // Saved, or Paused guest is still registered and still holds its
        // differencing disk, and a host where these cmdlets fail at all (no
        // Hyper-V Administrators membership, say) reports every VM as not
        // running. Deleting the disk of a VM that was never unregistered
        // leaves a broken VM that neither status nor destroy can see again.
        let before = self.query_state(&state.vm_name)?;
        if before.is_none() {
            return Ok(Stopped::WasNotRunning);
        }
        self.run_script(&destroy_script(&state.vm_name))?;

        // Confirm rather than assume: the script tolerates a missing VM, so
        // its success alone does not prove this one is gone.
        if self.query_state(&state.vm_name)?.is_some() {
            return Err(format!(
                "{} is still registered after Remove-VM",
                state.vm_name
            ));
        }
        // `before` is known to be Some here, and the state names come back
        // from PowerShell in whatever case it feels like, two lines from
        // another comparison that already allows for that.
        Ok(match before {
            Some(ref state) if state.eq_ignore_ascii_case("Off") => Stopped::WasNotRunning,
            _ => Stopped::Stopped,
        })
    }

    /// Whether the VM is running right now.
    ///
    /// A failed query answers `false`, which is the only thing a boolean can
    /// say, and is why nothing that deletes anything is allowed to ask this:
    /// `destroy` uses `query_state` so that "the cmdlets did not work" stays
    /// distinguishable from "there is no such VM". The callers here are
    /// `vm ssh`, `vm view`, `vm status`, and the one-VM-at-a-time check, all
    /// of which either report it or refuse, so a false negative costs a
    /// message rather than a disk.
    fn is_running(&self, state: &RunState) -> bool {
        match self.query_state(&state.vm_name) {
            Ok(Some(state)) => state.eq_ignore_ascii_case("Running"),
            Ok(None) => false,
            Err(e) => {
                eprintln!(
                    "warning: cannot tell whether {} is running: {e}",
                    state.vm_name
                );
                false
            }
        }
    }

    fn view(&self, state: &RunState) -> Result<String, String> {
        let viewer = crate::host::facts::resolve_tool(self.runner, "vmconnect", self.host)
            .unwrap_or_else(|| PathBuf::from("vmconnect.exe"));
        self.runner
            .spawn(
                &Cmd::new(viewer.to_string_lossy())
                    .args(["localhost".to_owned(), state.vm_name.clone()]),
                None,
            )
            .map_err(|e| format!("cannot start vmconnect: {e}"))?;
        Ok(format!(
            "vmconnect is opening {}.\n\
             Use the basic session it opens with, not an enhanced one: enhanced \
             session mode is RDP underneath and logs into a session of its own, \
             which locks the console session out from under a running job.",
            state.vm_name
        ))
    }

    fn ssh_target(&self, state: &RunState) -> SshTarget {
        SshTarget::from_state(state, self.store.ssh_key())
    }

    fn runner(&self) -> &dyn Runner {
        self.runner
    }

    fn windows_host(&self) -> bool {
        self.host == HostOs::Windows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vm_is_a_differencing_child_of_the_golden_image() {
        let script = create_script(
            "sunlit-e2e-windows",
            r"C:\vm\images\windows\golden.vhdx",
            r"C:\vm\run\windows\overlay.vhdx",
        );
        assert!(script.contains("-Differencing"), "{script}");
        assert!(
            script.contains(r"-ParentPath 'C:\vm\images\windows\golden.vhdx'"),
            "{script}"
        );
        // Nothing in here may write to the golden image.
        assert!(!script.contains("Set-VHD"), "{script}");
    }

    #[test]
    fn the_vm_is_generation_two_because_windows_11_needs_uefi() {
        let script = create_script("sunlit-e2e-windows", "g.vhdx", "o.vhdx");
        assert!(script.contains("-Generation 2"), "{script}");
        assert!(script.contains("-EnableSecureBoot Off"), "{script}");
        assert!(script.contains("FirstBootDevice"), "{script}");
    }

    #[test]
    fn the_vm_uses_the_switch_every_client_windows_has() {
        let script = create_script("sunlit-e2e-windows", "g.vhdx", "o.vhdx");
        assert!(script.contains("-SwitchName 'Default Switch'"), "{script}");
        assert!(!script.contains("New-VMSwitch"), "{script}");
    }

    #[test]
    fn the_vm_never_starts_itself_or_takes_checkpoints() {
        let script = create_script("sunlit-e2e-windows", "g.vhdx", "o.vhdx");
        assert!(
            script.contains("-AutomaticCheckpointsEnabled $false"),
            "{script}"
        );
        assert!(script.contains("-AutomaticStartAction Nothing"), "{script}");
    }

    #[test]
    fn every_script_names_a_vm_of_ours() {
        let name = Target::Windows.vm_name();
        for script in [
            create_script(&name, "g", "o"),
            state_script(&name),
            address_script(&name),
            destroy_script(&name),
        ] {
            assert!(script.contains("sunlit-e2e-windows"), "{script}");
        }
    }

    #[test]
    fn a_name_with_an_apostrophe_cannot_break_out_of_the_script() {
        let script = destroy_script("sunlit-e2e-o'brien");
        assert!(script.contains("'sunlit-e2e-o''brien'"), "{script}");
    }

    #[test]
    fn destroying_stops_first_and_removes_only_what_exists() {
        let script = destroy_script("sunlit-e2e-windows");
        assert!(script.contains("Get-VM -Name 'sunlit-e2e-windows' -ErrorAction SilentlyContinue"));
        assert!(script.contains("-TurnOff -Force"), "{script}");
        assert!(script.contains("Remove-VM"), "{script}");
        // Never a blanket removal.
        assert!(!script.contains("Get-VM |"), "{script}");
    }

    #[test]
    fn the_scripts_interpolate_rather_than_format() {
        // `Write-Output "..." -f $x` is not PowerShell: -f is a string
        // operator, not a parameter, and Write-Output rejects it.
        for script in [state_script("vm"), address_script("vm")] {
            assert!(!script.contains("\" -f "), "{script}");
        }
        assert!(state_script("vm").contains("$($vm.State)"));
    }

    #[test]
    fn the_state_is_read_from_the_marker_line() {
        assert_eq!(parse_state("STATE=Running\n"), Some("Running".to_owned()));
        assert_eq!(parse_state("noise\nSTATE=Off"), Some("Off".to_owned()));
        assert_eq!(parse_state(""), None);
        assert_eq!(parse_state("Get-VM : not found"), None);
    }

    #[test]
    fn addresses_are_read_from_the_marker_lines() {
        let out = "IP=fe80::1%5\nIP=192.168.1.20\nIP=\n";
        assert_eq!(
            parse_addresses(out),
            vec!["fe80::1%5".to_owned(), "192.168.1.20".to_owned()]
        );
        assert!(parse_addresses("").is_empty());
    }

    #[test]
    fn only_a_real_ipv4_address_counts_as_reachable() {
        let boot = vec![
            "fe80::215:5dff:fe00:1%12".to_owned(),
            "169.254.10.2".to_owned(),
        ];
        assert_eq!(first_usable_ipv4(&boot), None);

        let ready = vec![
            "fe80::215:5dff:fe00:1%12".to_owned(),
            "169.254.10.2".to_owned(),
            "172.28.144.5".to_owned(),
        ];
        assert_eq!(first_usable_ipv4(&ready), Some("172.28.144.5".to_owned()));
        assert_eq!(first_usable_ipv4(&[]), None);
    }
}
