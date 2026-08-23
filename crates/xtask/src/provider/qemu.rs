//! The QEMU provider: a process, a QMP socket, and a qcow2 overlay.
//!
//! Used for the Linux guest everywhere, and for the Windows guest on a Linux
//! host. Acceleration is WHPX on a Windows host and KVM on a Linux one; both
//! run on top of a hypervisor the doctor has already checked for.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::guest::ssh::SshTarget;
use crate::provider::Stopped;
use crate::provider::console;
use crate::provider::desktop::Desktop;
use crate::provider::qmp;
use crate::provider::target::{HostOs, ProviderKind, Target};
use crate::runner::{Cmd, Runner};
use crate::store::Store;
use crate::store::state::{RunState, StartReason};
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

/// The process a QEMU guest of ours is.
pub const QEMU_IMAGE: &str = "qemu-system-x86_64";

/// How long to wait for QEMU to be gone after being asked to stop.
pub const QUIT_GRACE: Duration = Duration::from_secs(20);

/// Whether the process a state file points at is still the one it recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ownership {
    /// The recorded process is running and is ours.
    Ours,
    /// Nothing is running under that id.
    Gone,
    /// Something is running under that id, and it is not ours. Process ids are
    /// reused, so this is the ordinary consequence of a stale state file on a
    /// busy machine, not an exotic case.
    Foreign(String),
}

/// Decide whether an identity is the QEMU this VM started.
///
/// The image name is the cheap half and is all Windows offers. Where the
/// command line is available it is checked too, because two QEMU processes on
/// one host are far more likely than two processes sharing a pid, and the VM
/// name is on the command line precisely so it can be recognized.
pub fn classify(identity: Option<&crate::runner::ProcessIdentity>, vm_name: &str) -> Ownership {
    let Some(identity) = identity else {
        return Ownership::Gone;
    };
    if !crate::runner::image_matches(&identity.image, QEMU_IMAGE) {
        return Ownership::Foreign(identity.image.clone());
    }
    match &identity.command_line {
        Some(line) if !line.contains(vm_name) => Ownership::Foreign(format!(
            "{} running something else ({line})",
            identity.image
        )),
        _ => Ownership::Ours,
    }
}

/// Memory and processor count per guest.
pub fn resources_for(target: Target) -> (u32, u32) {
    match target {
        // Windows needs the headroom, and the e2e suite renders an 8K-capable
        // pipeline on a CPU rasterizer inside it.
        Target::Windows => (6144, 4),
        Target::Linux => (4096, 4),
    }
}

/// The default console resolution for a QEMU guest that can be told one.
///
/// A VNC viewer scales, unlike a basic `vmconnect` session, so there is nothing
/// here to fit to the host's screen: what this has to be is large enough for the
/// app's settings window and small enough to be a window on a laptop, and
/// 1920x1080 is both. [`console::RESOLUTION_ENV`] overrides it, the same
/// variable and the same spelling the `Hyper-V` guest reads.
pub const DEFAULT_CONSOLE: (u32, u32) = (1920, 1080);

/// The display device model.
///
/// The Linux guest has the virtio DRM driver in-kernel; the Windows guest is a
/// stock install with no virtio drivers, so it gets the emulated VGA that
/// Windows has a built-in driver for.
pub fn vga_for(target: Target) -> &'static str {
    match target {
        Target::Windows => "std",
        Target::Linux => "virtio-vga",
    }
}

/// How the guest's display is attached, and at what size.
///
/// `-device virtio-vga` rather than `-vga virtio` for the Linux guest: the same
/// device either way, but only the `-device` form takes properties, and
/// `xres`/`yres` are what set virtio-gpu's preferred mode. Modern Xorg takes that
/// mode, so this is what decides the console's size before the guest has booted
/// far enough to have an opinion. No resolution is passed to the Windows guest,
/// whose emulated VGA has no such property; its console size is a `Hyper-V`
/// matter, and this cell of the matrix exists to reproduce a Linux host rather
/// than to be looked at.
pub fn display_args(target: Target, console: (u32, u32)) -> Vec<String> {
    let device = vga_for(target);
    match target {
        Target::Windows => vec!["-vga".to_owned(), device.to_owned()],
        Target::Linux => vec![
            "-device".to_owned(),
            format!("{device},xres={},yres={}", console.0, console.1),
        ],
    }
}

/// The pointer device, where the guest has a driver for one.
///
/// An absolute pointer, which is the fix for clicks landing away from the
/// cursor in a VNC viewer: VNC's `PointerEvent` carries absolute coordinates,
/// QEMU's implicit PS/2 mouse is a relative device, and the translation between
/// the two is what puts a click somewhere else on the screen. virtio because the
/// Linux guest drives it in-kernel and the rest of its devices are virtio
/// anyway; the Windows guest has no virtio driver at all and keeps the PS/2
/// mouse it does have one for.
pub fn pointer_device_for(target: Target) -> Option<&'static str> {
    match target {
        Target::Windows => None,
        Target::Linux => Some("virtio-tablet-pci"),
    }
}

/// The interfaces `-drive if=` accepts. Anything else is rejected at startup,
/// and QEMU exits before it has a console to say so on.
pub const DRIVE_INTERFACES: [&str; 9] = [
    "none", "ide", "scsi", "sd", "mtd", "floppy", "pflash", "virtio", "xen",
];

/// The device model the guest's disk is attached through.
///
/// The disk is attached in two parts, a backing `-drive if=none` and a
/// `-device` that references it by id. That is the explicit form, it works the
/// same for both guests, and it avoids the trap that `if=` looks like a
/// free-form field and is not: `if=ahci` is not one of the nine values QEMU
/// accepts, and the machine exits at startup rather than booting.
///
/// q35 has no legacy IDE controller. Its only IDE-family controller is the
/// built-in ICH9 AHCI one, which is where an `ide-hd` device lands and which a
/// stock Windows install has an in-box driver for. The bus is left to QEMU:
/// there is exactly one to choose from, and naming it adds a way to be wrong.
pub fn disk_device_for(target: Target) -> &'static str {
    match target {
        Target::Windows => "ide-hd",
        Target::Linux => "virtio-blk-pci",
    }
}

/// The network device model, for the same reason as the disk and the display.
///
/// This has to match what the image was installed with, or the guest comes up
/// with a device it has no driver for and no network. The symptom is the worst
/// kind: nothing is wrong on the host, the VM runs, and the SSH wait times out
/// ten minutes later with nothing to point at. `the_runtime_devices_match_the_templates`
/// pins each choice against the template that installed it.
pub fn nic_device_for(target: Target) -> &'static str {
    match target {
        Target::Windows => "e1000",
        Target::Linux => "virtio-net-pci",
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
    /// image does not need.
    ///
    /// The variables half is a per-VM copy of the host's pristine store, and
    /// it is worth being clear that it does not carry Windows Setup's boot
    /// entry: the store Setup wrote lives in Packer's output directory and is
    /// deleted with it once the image has been moved out. So a guest starts
    /// with blank NVRAM every time, and what makes it boot is the fallback
    /// loader at `\EFI\Boot\bootx64.efi` that the image's finalize step puts
    /// on the EFI system partition. That fallback is load-bearing here exactly
    /// as it is on `Hyper-V`, which is convenient: one mechanism covers both
    /// providers rather than each having its own.
    ///
    /// The copy is still per-VM, because the firmware writes to it during
    /// boot and two VMs sharing one store is a corruption waiting to happen.
    pub firmware: Option<crate::provider::firmware::Firmware>,
    /// The console's size, which the Linux guest is told and the Windows guest
    /// is not. See [`display_args`].
    pub console: (u32, u32),
    /// Which desktop the Linux guest logs into, when the host asked for one.
    ///
    /// `None` leaves the image's own default, so a guest booted by anything that
    /// does not know about this behaves as it always did.
    pub desktop: Option<Desktop>,
}

impl Launch {
    /// The command line.
    pub fn args(&self) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "-name".into(),
            self.name.clone(),
            "-machine".into(),
            format!("q35,accel={}", self.accelerator),
            // The guest CPU is asked for explicitly because QEMU's default,
            // `qemu64`, is what makes WHPX abort a Windows guest: the vCPU dies
            // with "Unexpected VP exit code 4" as soon as the Windows boot
            // manager runs, and the VM then sits at the firmware logo until
            // something times out. The same flag is in both Packer templates,
            // for the same reason, and it costs nothing under KVM.
            "-cpu".into(),
            "max".into(),
            "-m".into(),
            self.memory_mb.to_string(),
            "-smp".into(),
            self.cpus.to_string(),
            "-drive".into(),
            format!(
                "file={},if=none,id=hd0,format=qcow2",
                self.overlay.display()
            ),
            "-device".into(),
            format!("{},drive=hd0", disk_device_for(self.target)),
            "-netdev".into(),
            format!("user,id=net0,hostfwd=tcp:127.0.0.1:{}-:22", self.ssh_port),
            "-device".into(),
            format!("{},netdev=net0", nic_device_for(self.target)),
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
        args.extend(display_args(self.target, self.console));
        if let Some(pointer) = pointer_device_for(self.target) {
            args.push("-device".into());
            args.push(pointer.to_owned());
        }
        if let Some(desktop) = self.desktop {
            args.extend(crate::provider::desktop::fw_cfg_args(desktop));
        }
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

    /// Check the command line before spawning it.
    ///
    /// QEMU rejects an unknown `-drive if=` at startup, and `start` detaches
    /// the process with its output going to a log file, so the rejection would
    /// otherwise surface ten minutes later as an SSH timeout with the reason
    /// sitting in a file nobody was told to read. This turns that into an
    /// error before anything is spawned.
    pub fn validate(&self) -> Result<(), String> {
        for arg in self.args() {
            for field in arg.split(',') {
                if let Some(value) = field.strip_prefix("if=")
                    && !DRIVE_INTERFACES.contains(&value)
                {
                    return Err(format!(
                        "the drive interface '{value}' is not one QEMU accepts \
                         ({}); this is a bug in the xtask, not in the host",
                        DRIVE_INTERFACES.join(", ")
                    ));
                }
            }
        }
        Ok(())
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

    /// What the recorded process is now.
    fn ownership(&self, state: &RunState) -> Ownership {
        let Some(pid) = state.pid else {
            return Ownership::Gone;
        };
        classify(self.runner.process_identity(pid).as_ref(), &state.vm_name)
    }

    /// The target a state file names, defaulting to the Linux one only so the
    /// message-building path cannot panic on a corrupt file.
    fn target_of(state: &RunState) -> Target {
        state.target().unwrap_or(Target::Linux)
    }

    /// Poll until the process is gone, up to `grace`.
    fn wait_for_exit(&self, state: &RunState, grace: Duration) -> bool {
        let start = std::time::Instant::now();
        loop {
            if matches!(
                self.ownership(state),
                Ownership::Gone | Ownership::Foreign(_)
            ) {
                return true;
            }
            if start.elapsed() >= grace {
                return false;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn qemu_binary(&self) -> Result<PathBuf, String> {
        crate::host::facts::resolve_tool(self.runner, "qemu-system-x86_64", self.host).ok_or_else(
            || "qemu-system-x86_64 is not available; run `cargo xtask vm doctor`".to_owned(),
        )
    }

    fn qemu_img(&self) -> Result<PathBuf, String> {
        crate::host::facts::resolve_tool(self.runner, "qemu-img", self.host)
            .ok_or_else(|| "qemu-img is not available; run `cargo xtask vm doctor`".to_owned())
    }

    /// The launch parameters for a target.
    pub fn launch_for(&self, target: Target, overlay: PathBuf, desktop: Option<Desktop>) -> Launch {
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
            accelerator: crate::commands::build_image::accelerator_for(self.host).to_owned(),
            ssh_port: SSH_PORT,
            qmp_port: QMP_PORT,
            vnc_display: VNC_DISPLAY,
            firmware,
            console: console::requested_resolution(true).unwrap_or(DEFAULT_CONSOLE),
            desktop,
        }
    }

    /// Copy the firmware's variables store into the run directory, so each VM
    /// writes its boot entries into its own throwaway copy rather than into the
    /// shared one the host installed.
    fn per_vm_firmware(&self, target: Target) -> Option<crate::provider::firmware::Firmware> {
        let binary = crate::host::facts::resolve_tool(self.runner, "qemu-system-x86_64", self.host);
        let found = crate::provider::firmware::locate(self.host, binary.as_deref())?;
        let copy = self.store.run_dir(target).join("efi-vars.fd");
        if std::fs::create_dir_all(self.store.run_dir(target)).is_err()
            || std::fs::copy(&found.vars, &copy).is_err()
        {
            return Some(found);
        }
        Some(crate::provider::firmware::Firmware {
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
        let target = state
            .target()
            .ok_or_else(|| format!("unknown target '{}'", state.target))?;
        let binary = self.qemu_binary()?;
        // The desktop comes off the record rather than out of a parameter: the
        // command that chose it is finished by the time anything starts a
        // process, and `vm status` has to be able to say which one this guest
        // was booted into.
        let desktop = state.desktop.as_deref().and_then(Desktop::parse);
        let launch = self.launch_for(target, state.overlay.clone(), desktop);
        launch.validate()?;
        if target == Target::Linux {
            let (width, height) = launch.console;
            let session = desktop.map_or_else(
                || "the image's own default desktop".to_owned(),
                |d| format!("the {} session", d.label()),
            );
            println!("console: {width}x{height}, into {session}");
        }
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

    fn destroy(&self, state: &RunState) -> Result<Stopped, String> {
        let Some(pid) = state.pid else {
            return Ok(Stopped::WasNotRunning);
        };
        match self.ownership(state) {
            Ownership::Gone => return Ok(Stopped::WasNotRunning),
            // Refusing here is the point of asking. The caller deletes the
            // overlay after a successful stop, and killing a stranger's
            // process tree because a pid was reused is the one failure this
            // command must not have.
            Ownership::Foreign(found) => {
                return Err(format!(
                    "pid {pid} is now {found}, not this VM's QEMU; the process id \
                     was reused, so nothing was stopped and nothing was deleted. \
                     Remove {} by hand once you are sure it is idle.",
                    self.store.state_file(Self::target_of(state)).display()
                ));
            }
            Ownership::Ours => {}
        }

        // QMP first, so QEMU closes the overlay before it is deleted. Killing
        // the process works too, but leaves the qcow2 needing a repair pass
        // that nobody will ever run on a file about to be removed.
        if let Some(port) = state.qmp_port
            && qmp::execute(port, "quit").is_ok()
            && self.wait_for_exit(state, QUIT_GRACE)
        {
            return Ok(Stopped::Stopped);
        }
        self.runner
            .terminate(pid)
            .map_err(|e| format!("cannot terminate pid {pid}: {e}"))?;
        if self.wait_for_exit(state, QUIT_GRACE) {
            Ok(Stopped::Stopped)
        } else {
            Err(format!(
                "pid {pid} is still running after being asked and then told to stop"
            ))
        }
    }

    fn is_running(&self, state: &RunState) -> bool {
        matches!(self.ownership(state), Ownership::Ours)
    }

    fn view(&self, state: &RunState) -> Result<String, String> {
        let address = state
            .vnc
            .clone()
            .unwrap_or_else(|| format!("127.0.0.1:{}", vnc_port(VNC_DISPLAY)));
        // `resolve_tool` rather than a bare `PATH` lookup, so that the answer
        // is the same one `vm doctor` reports. A viewer installed by a package
        // manager that appends to the user `PATH`, which is both winget and
        // scoop, is invisible to every shell that started before it did.
        for viewer in crate::host::facts::VNC_VIEWERS {
            if let Some(path) = crate::host::facts::resolve_tool(self.runner, viewer, self.host) {
                self.runner
                    .spawn(&Cmd::new(path.to_string_lossy()).arg(address.clone()), None)
                    .map_err(|e| format!("cannot start {viewer}: {e}"))?;
                return Ok(format!("{viewer} is connecting to {address}"));
            }
        }
        Ok(format!(
            "no VNC viewer found. The console is at {address}, with no password; \
             point any VNC client at it. `cargo xtask vm doctor` lists the names \
             looked for."
        ))
    }

    fn ssh_target(&self, state: &RunState) -> SshTarget {
        SshTarget::from_state(state, self.store.ssh_key())
    }

    fn runner(&self) -> &dyn Runner {
        self.runner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider as _;
    use crate::runner::fake::FakeRunner;

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
            console: DEFAULT_CONSOLE,
            desktop: None,
        }
    }

    fn joined(target: Target) -> String {
        launch(target).args().join(" ")
    }

    #[test]
    fn a_qemu_guest_is_running_only_while_its_own_process_is() {
        let store = Store::new("/srv/vm");
        let runner = FakeRunner::new().with_process(
            4242,
            "qemu-system-x86_64",
            Some("qemu-system-x86_64 -name sunlit-e2e-linux -m 4096"),
        );
        let provider = QemuProvider::new(&runner, &store, HostOs::Linux);
        let mut state = RunState::new(
            Target::Linux,
            ProviderKind::Qemu,
            PathBuf::from("/srv/vm/run/linux/overlay.qcow2"),
            StartReason::Run,
            0,
        );
        // No pid recorded: a state file written before the process started.
        assert!(!provider.is_running(&state));
        // A pid that is not ours any more.
        state.pid = Some(1);
        assert!(!provider.is_running(&state));
        state.pid = Some(4242);
        assert!(provider.is_running(&state));
    }

    #[test]
    fn a_reused_pid_is_not_mistaken_for_our_vm() {
        // Process ids are reused. Everything downstream of this check kills a
        // process tree, so being wrong here means killing a stranger's.
        assert_eq!(classify(None, "sunlit-e2e-linux"), Ownership::Gone);

        let editor = crate::runner::ProcessIdentity {
            image: "vim".to_owned(),
            command_line: Some("vim notes.txt".to_owned()),
        };
        assert_eq!(
            classify(Some(&editor), "sunlit-e2e-linux"),
            Ownership::Foreign("vim".to_owned())
        );

        // Another QEMU, but not this VM's: the name is on the command line so
        // that this case is distinguishable.
        let other_vm = crate::runner::ProcessIdentity {
            image: "qemu-system-x86".to_owned(),
            command_line: Some("qemu-system-x86_64 -name someone-elses-vm".to_owned()),
        };
        assert!(matches!(
            classify(Some(&other_vm), "sunlit-e2e-linux"),
            Ownership::Foreign(_)
        ));

        let ours = crate::runner::ProcessIdentity {
            image: "qemu-system-x86".to_owned(),
            command_line: Some("qemu-system-x86_64 -name sunlit-e2e-linux".to_owned()),
        };
        assert_eq!(classify(Some(&ours), "sunlit-e2e-linux"), Ownership::Ours);
    }

    #[test]
    fn without_a_command_line_the_image_name_is_the_whole_answer() {
        // Windows gives no command line cheaply, so a matching image name is
        // as far as the check goes there.
        let windows = crate::runner::ProcessIdentity {
            image: "qemu-system-x86_64.exe".to_owned(),
            command_line: None,
        };
        assert_eq!(
            classify(Some(&windows), "sunlit-e2e-windows"),
            Ownership::Ours
        );
    }

    #[test]
    fn destroying_a_reused_pid_refuses_rather_than_killing_it() {
        let store = Store::new("/srv/vm");
        let runner = FakeRunner::new().with_process(4242, "postgres", Some("postgres -D /data"));
        let provider = QemuProvider::new(&runner, &store, HostOs::Linux);
        let mut state = RunState::new(
            Target::Linux,
            ProviderKind::Qemu,
            PathBuf::from("/srv/vm/run/linux/overlay.qcow2"),
            StartReason::Run,
            0,
        );
        state.pid = Some(4242);
        let err = provider.destroy(&state).unwrap_err();
        assert!(err.contains("was reused"), "{err}");
        assert!(runner.terminated.borrow().is_empty(), "it killed something");
    }

    #[test]
    fn destroying_a_running_guest_stops_it_and_reports_that_it_did() {
        // The path that licenses the caller to delete the overlay. Without a
        // test it was covered only by a fake-runner field nothing read.
        let store = Store::new("/srv/vm");
        let runner = FakeRunner::new().with_process(
            4242,
            "qemu-system-x86_64",
            Some("qemu-system-x86_64 -name sunlit-e2e-linux"),
        );
        let provider = QemuProvider::new(&runner, &store, HostOs::Linux);
        let mut state = RunState::new(
            Target::Linux,
            ProviderKind::Qemu,
            PathBuf::from("/srv/vm/run/linux/overlay.qcow2"),
            StartReason::Run,
            0,
        );
        state.pid = Some(4242);
        // No QMP port, so the teardown goes straight to terminating rather
        // than opening a socket to whatever happens to be on 4444 here.
        state.qmp_port = None;

        assert!(provider.is_running(&state));
        assert_eq!(provider.destroy(&state), Ok(Stopped::Stopped));
        assert_eq!(*runner.terminated.borrow(), vec![4242]);
        // Gone afterwards, which is what `wait_for_exit` had to observe for
        // the destroy to report success at all.
        assert!(!provider.is_running(&state));
    }

    #[test]
    fn destroying_a_vm_that_is_already_gone_is_not_a_stop() {
        let store = Store::new("/srv/vm");
        let runner = FakeRunner::new();
        let provider = QemuProvider::new(&runner, &store, HostOs::Linux);
        let mut state = RunState::new(
            Target::Linux,
            ProviderKind::Qemu,
            PathBuf::from("/srv/vm/run/linux/overlay.qcow2"),
            StartReason::Run,
            0,
        );
        state.pid = Some(4242);
        assert_eq!(provider.destroy(&state), Ok(Stopped::WasNotRunning));
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
    fn the_guest_cpu_is_asked_for_rather_than_left_to_qemu() {
        // With QEMU's default `qemu64`, WHPX kills the vCPU as soon as the
        // Windows boot manager runs ("Unexpected VP exit code 4"), and the VM
        // then sits at the firmware logo forever. Both Packer templates carry
        // the same flag, which their own test checks.
        for target in Target::ALL {
            assert!(joined(target).contains("-cpu max"), "{target}");
        }
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
        assert_eq!(vga_for(Target::Linux), "virtio-vga");
        // ide-hd lands on q35's built-in AHCI controller, which Windows has an
        // in-box driver for; virtio-blk needs one the Linux guest has and
        // Windows does not.
        assert!(
            joined(Target::Windows).contains("-device ide-hd,drive=hd0"),
            "{}",
            joined(Target::Windows)
        );
        assert!(
            joined(Target::Linux).contains("-device virtio-blk-pci,drive=hd0"),
            "{}",
            joined(Target::Linux)
        );
        for target in Target::ALL {
            assert!(joined(target).contains("if=none,id=hd0"), "{target}");
        }
    }

    #[test]
    fn a_bad_drive_interface_is_refused_before_anything_is_spawned() {
        let mut broken = launch(Target::Windows);
        broken.overlay = PathBuf::from("/srv/vm/o.qcow2,if=ahci");
        let err = broken.validate().unwrap_err();
        assert!(err.contains("'ahci' is not one QEMU accepts"), "{err}");
        assert!(launch(Target::Linux).validate().is_ok());
    }

    #[test]
    fn every_drive_uses_an_interface_qemu_accepts() {
        // `if=` is not free-form. An unaccepted value makes QEMU exit at
        // startup, and because the process is detached with its output in a
        // log, that surfaces ten minutes later as an SSH timeout.
        let mut with_firmware = launch(Target::Windows);
        with_firmware.firmware = Some(crate::provider::firmware::Firmware {
            code: PathBuf::from("/fw/code.fd"),
            vars: PathBuf::from("/fw/vars.fd"),
        });
        for launch in [
            launch(Target::Linux),
            launch(Target::Windows),
            with_firmware,
        ] {
            for arg in launch.args() {
                for field in arg.split(',') {
                    let Some(value) = field.strip_prefix("if=") else {
                        continue;
                    };
                    assert!(
                        DRIVE_INTERFACES.contains(&value),
                        "if={value} is not one of {DRIVE_INTERFACES:?}"
                    );
                }
            }
        }
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
        with_firmware.firmware = Some(crate::provider::firmware::Firmware {
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
        // firmware writes to it during boot, and two VMs sharing one store
        // corrupt each other's.
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
    fn the_linux_guest_gets_an_absolute_pointer_so_clicks_land_where_the_cursor_is() {
        // VNC sends absolute coordinates and QEMU's implicit PS/2 mouse is a
        // relative device, so without this a click in the viewer lands somewhere
        // else on the guest's screen. The Windows guest has no virtio driver, so
        // it keeps the mouse it does have one for.
        assert_eq!(pointer_device_for(Target::Linux), Some("virtio-tablet-pci"));
        assert_eq!(pointer_device_for(Target::Windows), None);
        assert!(
            joined(Target::Linux).contains("-device virtio-tablet-pci"),
            "{}",
            joined(Target::Linux)
        );
        assert!(!joined(Target::Windows).contains("tablet"));
    }

    #[test]
    fn the_linux_console_is_a_size_the_host_chose_rather_than_one_the_guest_picked() {
        // `-device virtio-vga` rather than `-vga virtio`, because only the
        // device form carries the properties that set the preferred mode.
        let args = launch(Target::Linux).args();
        assert!(
            args.join(" ")
                .contains("-device virtio-vga,xres=1920,yres=1080"),
            "{args:?}"
        );
        assert!(!args.iter().any(|arg| arg == "-vga"), "{args:?}");

        let mut small = launch(Target::Linux);
        small.console = (1280, 800);
        assert!(
            small.args().join(" ").contains("xres=1280,yres=800"),
            "{:?}",
            small.args()
        );

        // The Windows guest's emulated VGA has no such property, and its console
        // is a Hyper-V matter anyway.
        let windows = joined(Target::Windows);
        assert!(windows.contains("-vga std"), "{windows}");
        assert!(!windows.contains("xres="), "{windows}");
    }

    #[test]
    fn the_desktop_reaches_the_guest_through_fw_cfg_and_only_when_one_was_asked_for() {
        // No flag means the image's own default, so a guest booted by anything
        // that does not know about this behaves as it always did.
        assert!(
            !joined(Target::Linux).contains("fw_cfg"),
            "{}",
            joined(Target::Linux)
        );

        let mut with_desktop = launch(Target::Linux);
        with_desktop.desktop = Some(Desktop::Gnome);
        let text = with_desktop.args().join(" ");
        assert!(
            text.contains("-fw_cfg name=opt/sunlit/desktop,string=gnome-xorg"),
            "{text}"
        );
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

#[cfg(test)]
mod template_agreement {
    //! The runtime command line and the image build have to choose the same
    //! virtual hardware.
    //!
    //! A guest installed with one network card and booted with another has no
    //! driver for what it finds, and the symptom is an SSH wait that times out
    //! ten minutes later with nothing on the host to point at. Nothing but a
    //! convention connected the two sides, so this reads the templates.

    use super::{disk_device_for, display_args, nic_device_for};
    use crate::provider::target::Target;
    use crate::store::template_dir;

    fn template(target: Target) -> String {
        let dir = template_dir(target);
        let name = match target {
            Target::Windows => "windows11.pkr.hcl",
            Target::Linux => "debian-13.pkr.hcl",
        };
        std::fs::read_to_string(dir.join(name))
            .unwrap_or_else(|e| panic!("cannot read the {target} template: {e}"))
    }

    /// The default of one `variable "name" {}` block, since a template has many
    /// `default =` lines and only one of them belongs to the variable in hand.
    fn variable_default(text: &str, name: &str) -> String {
        let after = text
            .split_once(&format!("variable \"{name}\" {{"))
            .unwrap_or_else(|| panic!("no variable {name} in the template"))
            .1;
        let block = after
            .split_once('}')
            .unwrap_or_else(|| panic!("variable {name} has no closing brace"))
            .0;
        setting(block, "default")
    }

    /// The value of a `key = "value"` line in an HCL template.
    fn setting(text: &str, key: &str) -> String {
        text.lines()
            .find_map(|line| {
                let (found, value) = line.split_once('=')?;
                (found.trim() == key).then(|| value.trim().trim_matches('"').to_owned())
            })
            .unwrap_or_else(|| panic!("no {key} in the template"))
    }

    #[test]
    fn the_windows_template_installs_on_at_least_two_cores() {
        // Windows 11 Setup refuses a single-core processor outright, and the
        // unattend file's BypassCPUCheck does not cover that check, so this is a
        // floor rather than a preference. It is pinned because one vCPU is the
        // workaround for the WHPX reset fault and would otherwise look like a
        // free choice to make here.
        let cores: u32 = variable_default(&template(Target::Windows), "cpus")
            .parse()
            .expect("the cpus default is a number");
        assert!(cores >= 2, "Windows 11 Setup refuses {cores} core(s)");
    }

    #[test]
    fn the_windows_template_does_not_leave_the_guest_cpu_at_qemus_default() {
        // QEMU's default `qemu64` is what makes WHPX abort a Windows guest the
        // moment its boot manager runs, and the symptom is an image build that
        // sits at the firmware logo until Packer's SSH timeout. The runtime
        // side of this is asserted next to the other launch arguments, and it
        // covers both guests; only the Windows *install* has been seen to need
        // it, and asking for it in the Linux template too would invalidate
        // every Linux image already built for no behaviour anyone has observed.
        assert!(
            template(Target::Windows).contains(r#"["-cpu", "max"]"#),
            "the Windows template leaves the guest CPU at QEMU's default"
        );
    }

    #[test]
    fn the_boot_key_is_pressed_over_qmp_rather_than_typed_by_packer() {
        // Packer logs every keystroke it sends over VNC and the guest receives
        // none of them on this host, so the template asks Packer to type
        // nothing and opens a monitor for the xtask to press the key on. The
        // port has to be the one the xtask presses.
        let text = template(Target::Windows);
        assert!(text.contains("boot_command = []"), "{text}");
        assert!(
            text.contains(r#"["-qmp", "tcp:127.0.0.1:${var.qmp_port}"#),
            "{text}"
        );
        let port = crate::commands::build_image::BUILD_QMP_PORT.to_string();
        assert!(
            text.contains(&format!("default = \"{port}\"")),
            "the template's qmp_port default is not {port}"
        );
        assert_ne!(
            crate::commands::build_image::BUILD_QMP_PORT,
            super::QMP_PORT,
            "a build and a running guest would fight over the monitor"
        );
    }

    #[test]
    fn the_windows_template_boots_its_installation_media_first() {
        // Without this the "Press any key to boot from CD or DVD" prompt never
        // appears, and no keypress can rescue the build.
        let text = template(Target::Windows);
        assert!(text.contains(r#"["-boot", "order=d"]"#), "{text}");
    }

    #[test]
    fn the_runtime_devices_match_the_templates() {
        for target in Target::ALL {
            let text = template(target);

            // Packer spells the network device the way QEMU's -device does.
            assert_eq!(
                setting(&text, "net_device"),
                nic_device_for(target),
                "{target}: the image is installed with a different NIC than it boots with"
            );

            // The disk is spelled as an interface in Packer and as a device at
            // runtime, so the pairing is stated rather than compared.
            let expected_interface = match disk_device_for(target) {
                "ide-hd" => "ide",
                "virtio-blk-pci" => "virtio",
                other => panic!("unmapped disk device {other}"),
            };
            assert_eq!(
                setting(&text, "disk_interface"),
                expected_interface,
                "{target}: the image is installed on a different disk controller than it boots from"
            );

            // Both machines are q35, which is what makes an ide-hd device land
            // on a SATA controller rather than a legacy IDE one.
            assert_eq!(setting(&text, "machine_type"), "q35", "{target}");

            // The display is runtime-only: Packer never sees it, so this only
            // checks the device is one QEMU knows, in the form it takes it.
            let display = display_args(target, (1920, 1080)).join(" ");
            assert!(
                ["-vga std", "-device virtio-vga,"]
                    .iter()
                    .any(|known| display.starts_with(known)),
                "{target}: unknown display arguments {display}"
            );
        }
    }

    #[test]
    fn the_windows_guest_boots_devices_a_stock_install_has_drivers_for() {
        // The whole reason for the split: no virtio drivers are injected, so
        // both of these have to be things Windows ships a driver for.
        assert_eq!(nic_device_for(Target::Windows), "e1000");
        assert_eq!(disk_device_for(Target::Windows), "ide-hd");
        assert_ne!(
            nic_device_for(Target::Windows),
            nic_device_for(Target::Linux)
        );
    }
}
