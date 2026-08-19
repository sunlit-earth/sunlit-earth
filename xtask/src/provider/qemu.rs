//! The QEMU provider: a process, a QMP socket, and a qcow2 overlay.
//!
//! Used for the Linux guest everywhere, and for the Windows guest on a Linux
//! host. Acceleration is WHPX on a Windows host and KVM on a Linux one; both
//! run on top of a hypervisor the doctor has already checked for.

use std::path::{Path, PathBuf};

use crate::qmp;
use crate::runner::{Cmd, Runner};
use crate::ssh::SshTarget;
use crate::state::{RunState, StartReason};
use crate::store::Store;
use crate::target::{HostOs, ProviderKind, Target};
use crate::util;

/// One VM at a time (plan decision 7), so the ports are fixed rather than
/// allocated. Fixed ports also mean a crashed orchestrator leaves a guest
/// somebody can still reach.
pub const SSH_PORT: u16 = 2222;
pub const QMP_PORT: u16 = 4444;
pub const VNC_DISPLAY: u16 = 0;

/// VNC display 0 is TCP port 5900, and so on.
pub fn vnc_port(display: u16) -> u16 {
    5900 + display
}

/// The account the golden images create.
pub const GUEST_USER: &str = "tester";

/// Memory and processor count per guest.
pub fn resources_for(target: Target) -> (u32, u32) {
    match target {
        // Windows needs the headroom, and the e2e suite renders an 8K-capable
        // pipeline on a CPU rasterizer inside it.
        Target::Windows => (6144, 4),
        Target::Linux => (4096, 4),
    }
}

/// The display device.
///
/// The Linux guest has the virtio DRM driver in-kernel; the Windows guest is a
/// stock install with no virtio drivers, so it gets the emulated VGA that
/// Windows has a built-in driver for.
pub fn vga_for(target: Target) -> &'static str {
    match target {
        Target::Windows => "std",
        Target::Linux => "virtio",
    }
}

/// The disk interface, for the same reason as the display.
pub fn disk_interface_for(target: Target) -> &'static str {
    match target {
        Target::Windows => "ahci",
        Target::Linux => "virtio",
    }
}

/// Everything a `qemu-system-x86_64` command line needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub name: String,
    pub overlay: PathBuf,
    pub target: Target,
    pub memory_mb: u32,
    pub cpus: u32,
    pub accelerator: String,
    pub ssh_port: u16,
    pub qmp_port: u16,
    pub vnc_display: u16,
    /// UEFI firmware, which the Windows guest requires and the Linux cloud
    /// image does not need. The variables half must be a per-VM copy: the
    /// firmware writes the boot entry Windows Setup created into it.
    pub firmware: Option<crate::firmware::Firmware>,
}

impl Launch {
    /// The command line.
    pub fn args(&self) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "-name".into(),
            self.name.clone(),
            "-machine".into(),
            format!("q35,accel={}", self.accelerator),
            "-m".into(),
            self.memory_mb.to_string(),
            "-smp".into(),
            self.cpus.to_string(),
            "-drive".into(),
            format!(
                "file={},if={},format=qcow2",
                self.overlay.display(),
                disk_interface_for(self.target)
            ),
            "-netdev".into(),
            format!("user,id=net0,hostfwd=tcp:127.0.0.1:{}-:22", self.ssh_port),
            "-device".into(),
            "virtio-net-pci,netdev=net0".into(),
            "-vga".into(),
            vga_for(self.target).to_owned(),
            // No local window, and a VNC server on loopback that costs nothing
            // until somebody attaches (plan decision 7). This is what makes the
            // console available at any moment without a decision up front.
            "-display".into(),
            "none".into(),
            "-vnc".into(),
            format!("127.0.0.1:{}", self.vnc_display),
            "-qmp".into(),
            format!("tcp:127.0.0.1:{},server=on,wait=off", self.qmp_port),
            "-rtc".into(),
            "base=utc".into(),
        ];
        if let Some(firmware) = &self.firmware {
            // pflash rather than -bios: -bios gives the firmware nowhere to
            // keep its variables, and an installed Windows then has no boot
            // entry to find on the next start.
            args.push("-drive".into());
            args.push(format!(
                "if=pflash,format=raw,unit=0,readonly=on,file={}",
                firmware.code.display()
            ));
            args.push("-drive".into());
            args.push(format!(
                "if=pflash,format=raw,unit=1,file={}",
                firmware.vars.display()
            ));
        }
        args
    }
}

/// The QEMU provider.
pub struct QemuProvider<'a> {
    runner: &'a dyn Runner,
    store: &'a Store,
    host: HostOs,
}

impl<'a> QemuProvider<'a> {
    pub fn new(runner: &'a dyn Runner, store: &'a Store, host: HostOs) -> Self {
        Self {
            runner,
            store,
            host,
        }
    }

    fn qemu_binary(&self) -> Result<PathBuf, String> {
        crate::facts::resolve_tool(self.runner, "qemu-system-x86_64", self.host).ok_or_else(|| {
            "qemu-system-x86_64 is not available; run `cargo xtask vm doctor`".to_owned()
        })
    }

    fn qemu_img(&self) -> Result<PathBuf, String> {
        crate::facts::resolve_tool(self.runner, "qemu-img", self.host)
            .ok_or_else(|| "qemu-img is not available; run `cargo xtask vm doctor`".to_owned())
    }

    /// The launch parameters for a target.
    pub fn launch_for(&self, target: Target, overlay: PathBuf) -> Launch {
        let (memory_mb, cpus) = resources_for(target);
        let firmware = if target == Target::Windows {
            self.per_vm_firmware(target)
        } else {
            None
        };
        Launch {
            name: target.vm_name(),
            overlay,
            target,
            memory_mb,
            cpus,
            accelerator: crate::build_image::accelerator_for(self.host).to_owned(),
            ssh_port: SSH_PORT,
            qmp_port: QMP_PORT,
            vnc_display: VNC_DISPLAY,
            firmware,
        }
    }

    /// Copy the firmware's variables store into the run directory, so each VM
    /// writes its boot entries into its own throwaway copy rather than into the
    /// shared one the host installed.
    fn per_vm_firmware(&self, target: Target) -> Option<crate::firmware::Firmware> {
        let binary = crate::facts::resolve_tool(self.runner, "qemu-system-x86_64", self.host);
        let found = crate::firmware::locate(self.host, binary.as_deref())?;
        let copy = self.store.run_dir(target).join("efi-vars.fd");
        if std::fs::create_dir_all(self.store.run_dir(target)).is_err()
            || std::fs::copy(&found.vars, &copy).is_err()
        {
            return Some(found);
        }
        Some(crate::firmware::Firmware {
            code: found.code,
            vars: copy,
        })
    }
}

/// `qemu-img create` for a throwaway child of the golden image.
pub fn overlay_args(golden: &Path, overlay: &Path) -> Vec<String> {
    vec![
        "create".to_owned(),
        "-f".to_owned(),
        "qcow2".to_owned(),
        // The backing format has to be stated explicitly; qemu-img refuses to
        // guess it, and a guess is how an image gets misread as raw.
        "-F".to_owned(),
        "qcow2".to_owned(),
        "-b".to_owned(),
        golden.display().to_string(),
        overlay.display().to_string(),
    ]
}

impl crate::provider::Provider for QemuProvider<'_> {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Qemu
    }

    fn create_from_golden(&self, target: Target, reason: StartReason) -> Result<RunState, String> {
        let golden = self.store.qcow2(target);
        if !golden.is_file() {
            return Err(format!(
                "no golden image at {}; `cargo xtask vm build-image {target}` builds one",
                golden.display()
            ));
        }
        let overlay = self.store.qemu_overlay(target);
        let run_dir = self.store.run_dir(target);
        std::fs::create_dir_all(&run_dir)
            .map_err(|e| format!("cannot create {}: {e}", run_dir.display()))?;
        let _ = std::fs::remove_file(&overlay);

        let qemu_img = self.qemu_img()?;
        let out = self
            .runner
            .capture(&Cmd::new(qemu_img.to_string_lossy()).args(overlay_args(&golden, &overlay)))
            .map_err(|e| format!("cannot run qemu-img: {e}"))?;
        if !out.success() {
            return Err(format!("qemu-img create failed: {}", out.stderr.trim()));
        }

        let mut state = RunState::new(
            target,
            ProviderKind::Qemu,
            overlay,
            reason,
            util::now_unix(),
        );
        "127.0.0.1".clone_into(&mut state.ssh_host);
        state.ssh_port = SSH_PORT;
        GUEST_USER.clone_into(&mut state.ssh_user);
        state.qmp_port = Some(QMP_PORT);
        state.vnc = Some(format!("127.0.0.1:{}", vnc_port(VNC_DISPLAY)));
        Ok(state)
    }

    fn start(&self, state: &mut RunState) -> Result<(), String> {
        let target = Target::ALL
            .into_iter()
            .find(|t| t.slug() == state.target)
            .ok_or_else(|| format!("unknown target '{}'", state.target))?;
        let binary = self.qemu_binary()?;
        let launch = self.launch_for(target, state.overlay.clone());
        let log = self.store.vm_log(target);
        let pid = self
            .runner
            .spawn(
                &Cmd::new(binary.to_string_lossy()).args(launch.args()),
                Some(&log),
            )
            .map_err(|e| format!("cannot start qemu: {e}"))?;
        state.pid = Some(pid);
        state.started_unix = util::now_unix();
        Ok(())
    }

    fn destroy(&self, state: &RunState) -> Result<(), String> {
        // QMP first, so QEMU closes the overlay before it is deleted. Killing
        // the process works too, but leaves the qcow2 needing a repair pass
        // that nobody will ever run on a file about to be removed.
        let mut problems = Vec::new();
        if let Some(port) = state.qmp_port
            && self.is_running(state)
            && let Err(e) = qmp::execute(port, "quit")
        {
            problems.push(e);
        }
        if let Some(pid) = state.pid
            && self.runner.process_alive(pid)
            && let Err(e) = self.runner.terminate(pid)
        {
            problems.push(format!("cannot terminate pid {pid}: {e}"));
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    fn is_running(&self, state: &RunState) -> bool {
        state.pid.is_some_and(|pid| self.runner.process_alive(pid))
    }

    fn view(&self, state: &RunState) -> Result<String, String> {
        let address = state
            .vnc
            .clone()
            .unwrap_or_else(|| format!("127.0.0.1:{}", vnc_port(VNC_DISPLAY)));
        for viewer in crate::facts::VNC_VIEWERS {
            if let Some(path) = self.runner.which(viewer) {
                self.runner
                    .spawn(&Cmd::new(path.to_string_lossy()).arg(address.clone()), None)
                    .map_err(|e| format!("cannot start {viewer}: {e}"))?;
                return Ok(format!("{viewer} is connecting to {address}"));
            }
        }
        Ok(format!(
            "no VNC viewer on PATH. The console is at {address}, with no password; \
             point any VNC client at it."
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

    fn launch(target: Target) -> Launch {
        let (memory_mb, cpus) = resources_for(target);
        Launch {
            name: target.vm_name(),
            overlay: PathBuf::from("/srv/vm/run/linux/overlay.qcow2"),
            target,
            memory_mb,
            cpus,
            accelerator: "kvm".to_owned(),
            ssh_port: SSH_PORT,
            qmp_port: QMP_PORT,
            vnc_display: VNC_DISPLAY,
            firmware: None,
        }
    }

    fn joined(target: Target) -> String {
        launch(target).args().join(" ")
    }

    #[test]
    fn vnc_display_zero_is_port_5900() {
        assert_eq!(vnc_port(0), 5900);
        assert_eq!(vnc_port(3), 5903);
    }

    #[test]
    fn the_command_line_forwards_ssh_to_loopback_only() {
        let text = joined(Target::Linux);
        assert!(text.contains("hostfwd=tcp:127.0.0.1:2222-:22"), "{text}");
        // Binding the forward to 0.0.0.0 would put a passwordless guest on the
        // network, which is the one thing this must not do.
        assert!(!text.contains("hostfwd=tcp::"), "{text}");
    }

    #[test]
    fn the_console_is_always_available_on_loopback_vnc() {
        let text = joined(Target::Linux);
        assert!(text.contains("-display none"), "{text}");
        assert!(text.contains("-vnc 127.0.0.1:0"), "{text}");
    }

    #[test]
    fn qmp_listens_without_blocking_the_boot() {
        let text = joined(Target::Linux);
        // `wait=off` matters: with the default, QEMU would not start until
        // something connected to the monitor.
        assert!(
            text.contains("-qmp tcp:127.0.0.1:4444,server=on,wait=off"),
            "{text}"
        );
    }

    #[test]
    fn the_windows_guest_gets_devices_it_has_drivers_for() {
        assert_eq!(vga_for(Target::Windows), "std");
        assert_eq!(disk_interface_for(Target::Windows), "ahci");
        assert_eq!(vga_for(Target::Linux), "virtio");
        assert_eq!(disk_interface_for(Target::Linux), "virtio");
        assert!(joined(Target::Windows).contains("if=ahci"));
        assert!(joined(Target::Linux).contains("if=virtio"));
    }

    #[test]
    fn the_windows_guest_gets_more_memory_than_the_linux_one() {
        let (windows, _) = resources_for(Target::Windows);
        let (linux, _) = resources_for(Target::Linux);
        assert!(windows > linux);
    }

    #[test]
    fn firmware_is_passed_only_when_there_is_some() {
        assert!(!joined(Target::Linux).contains("pflash"));
        let mut with_firmware = launch(Target::Windows);
        with_firmware.firmware = Some(crate::firmware::Firmware {
            code: PathBuf::from("/usr/share/OVMF/OVMF_CODE.fd"),
            vars: PathBuf::from("/srv/vm/run/windows/efi-vars.fd"),
        });
        let text = with_firmware.args().join(" ");
        assert!(
            text.contains(
                "if=pflash,format=raw,unit=0,readonly=on,file=/usr/share/OVMF/OVMF_CODE.fd"
            ),
            "{text}"
        );
        // The writable half is the per-VM copy, never the host's own: the
        // firmware writes the boot entry Windows Setup created into it.
        assert!(
            text.contains("if=pflash,format=raw,unit=1,file=/srv/vm/run/windows/efi-vars.fd"),
            "{text}"
        );
    }

    #[test]
    fn the_guest_clock_starts_from_utc() {
        // The renderer's whole output is a function of the date, so a guest
        // whose clock drifts to local time renders a different Earth.
        assert!(joined(Target::Linux).contains("-rtc base=utc"));
    }

    #[test]
    fn the_vm_carries_the_ownership_prefix_into_qemu() {
        assert!(joined(Target::Linux).contains("-name sunlit-e2e-linux"));
    }

    #[test]
    fn the_overlay_is_a_child_with_the_backing_format_stated() {
        let args = overlay_args(
            Path::new("/srv/vm/images/linux/golden.qcow2"),
            Path::new("/srv/vm/run/linux/overlay.qcow2"),
        );
        assert_eq!(args[0], "create");
        assert!(args.contains(&"-F".to_owned()), "{args:?}");
        assert!(args.contains(&"qcow2".to_owned()));
        assert_eq!(
            args.last().map(String::as_str),
            Some("/srv/vm/run/linux/overlay.qcow2")
        );
        let backing = args.iter().position(|a| a == "-b").expect("backing flag");
        assert_eq!(args[backing + 1], "/srv/vm/images/linux/golden.qcow2");
    }
}
