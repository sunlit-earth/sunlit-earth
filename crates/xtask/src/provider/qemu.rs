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
use crate::provider::desktop::Login;
use crate::provider::qmp;
use crate::provider::target::{HostOs, Image, ProviderKind, Target};
use crate::runner::{Cmd, Runner};
use crate::store::Store;
use crate::store::state::{RunState, StartReason};
use crate::util;

/// One VM at a time (plan decision 7), so every guest asks for the same,
/// recognizable ports first. Preferred rather than fixed: on a Windows host
/// `WinNAT` reserves 100-port blocks for `Hyper-V` and WSL at moments of its own
/// choosing (they move on reboot and whenever those services restart), a
/// reserved port refuses to bind, and QEMU exits over that before it has built
/// the machine. So [`QemuProvider::create_from_golden`] records the first
/// bindable port at or after each of these, `start` builds the command line
/// from the record, and a crashed orchestrator's guest is still reachable
/// because the ports are in its state file.
pub const SSH_PORT: u16 = 2222;
pub const QMP_PORT: u16 = 4444;
pub const VNC_DISPLAY: u16 = 0;

/// VNC display 0 is TCP port 5900, and so on.
pub const VNC_BASE_PORT: u16 = 5900;

pub fn vnc_port(display: u16) -> u16 {
    VNC_BASE_PORT + display
}

/// How far past its preferred port a pick may walk before giving up.
///
/// Windows reserves ports in blocks of 100, so a walk has to clear one block
/// that covers the preferred port plus a second that starts right after it.
/// 250 does, and it keeps the three walks disjoint: each preferred port is
/// more than a whole walk away from the next, which a test pins, so the picks
/// can never hand out the same port however far they move.
pub const PORT_WALK: u16 = 250;

/// The first port at or after `preferred` that can be bound on loopback now.
///
/// Found by doing what QEMU is about to do, because the two ways a port can be
/// refused look identical from anywhere else: something is listening on it, or
/// Windows has reserved it (`netsh interface ipv4 show excludedportrange
/// protocol=tcp` lists those blocks). The listener is dropped again, so
/// something else can still take the port before QEMU does; that race is
/// accepted, and losing it is one of the failures the SSH wait reports by
/// reading the exited process's log.
pub fn free_port_from(preferred: u16) -> Result<u16, String> {
    let end = preferred.saturating_add(PORT_WALK);
    for port in preferred..end {
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Ok(port);
        }
    }
    Err(format!(
        "no bindable TCP port on 127.0.0.1 between {preferred} and {end}: every one \
         is in use or reserved. On a Windows host, `netsh interface ipv4 show \
         excludedportrange protocol=tcp` lists the reserved blocks."
    ))
}

/// Pick a port, saying so when the preferred one could not be had: the answer
/// changes what a person points a VNC viewer or an ssh client at, so a silent
/// move would read as the fixed port everyone remembers.
fn pick_port(label: &str, preferred: u16) -> Result<u16, String> {
    let port = free_port_from(preferred)?;
    if port != preferred {
        println!("  the usual {label} port {preferred} is in use or reserved; using {port}");
    }
    Ok(port)
}

/// The `-vnc` display number behind a record's viewer address.
///
/// The record stores what a viewer connects to (`127.0.0.1:5903`); QEMU wants
/// the display number, which is the port minus 5900. A record with no address,
/// or an unreadable one, is display 0, which is what the address said before
/// it could vary.
pub fn vnc_display_of(state: &RunState) -> u16 {
    state.vnc.as_deref().map_or(VNC_DISPLAY, display_of_address)
}

/// The display number behind one recorded address.
fn display_of_address(address: &str) -> u16 {
    address
        .rsplit(':')
        .next()
        .and_then(|port| port.parse::<u16>().ok())
        .map_or(VNC_DISPLAY, |port| port.saturating_sub(VNC_BASE_PORT))
}

/// The display number of every screen a record names, first screen first.
///
/// Never empty: a record with no console at all still describes a machine with
/// one screen, and the display it gets is the one [`vnc_display_of`] answers
/// with, which is what a record written before this existed already meant.
pub fn vnc_displays_of(state: &RunState) -> Vec<u16> {
    let displays: Vec<u16> = state
        .consoles()
        .into_iter()
        .map(display_of_address)
        .collect();
    if displays.is_empty() {
        return vec![vnc_display_of(state)];
    }
    displays
}

/// How much of a dead QEMU's log its post-mortem quotes.
pub const LOG_TAIL_LINES: usize = 5;

/// The last words in a VM log: up to [`LOG_TAIL_LINES`] non-empty lines,
/// without the first line, which is the command line the runner wrote there
/// rather than anything QEMU said.
pub fn log_tail(text: &str) -> Vec<&str> {
    let lines: Vec<&str> = text
        .lines()
        .skip(1)
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(LOG_TAIL_LINES)..].to_vec()
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

/// The id the display device of a multi-screen guest carries.
///
/// What `screendump` needs to be told to capture a screen other than the first:
/// its `device` argument is a device id, and its `head` argument is the screen's
/// position on that device.
pub const DISPLAY_DEVICE_ID: &str = "gpu";

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
///
/// A guest with more than one screen is given the whole device as JSON, which is
/// the only form `-device` takes a list property in: `outputs.0.name=` is not a
/// property name QEMU resolves, and the `key=value` form has no other way to
/// write a list. Three things are set there, and all three are needed:
///
/// - `max_outputs` is how many scanouts the device offers, which is how many
///   connectors the guest's DRM driver creates. On its own it produces a second
///   connector that reports itself disconnected, because the device enables only
///   output 0 when it is realized and nothing enables the rest until a UI tells
///   QEMU what size it is. A VNC server is not that: a client can connect to the
///   second head and the guest still sees nothing plugged into it, which is
///   exactly what the first version of this did.
/// - `outputs`, one entry per screen with its own `xres`/`yres`, is what gives
///   each output a size up front, and an output with a size is one the guest
///   sees connected. Verified in the Debian guest on 2026-08-30: with
///   `max_outputs` alone, `card0-Virtual-2` reads `disconnected` and the session
///   has one screen; with the `outputs` list, both connectors read `connected`
///   and the Plasma session comes up 3840x1080 with the second screen already to
///   the right of the first.
/// - The device-level `xres`/`yres` stay, so the first screen is the size it
///   would have been either way.
///
/// The names are for QEMU's own display plumbing and nothing here reads them
/// back; they exist because the property requires one. The device itself gets an
/// id, [`DISPLAY_DEVICE_ID`], which is what makes a screen other than the first
/// addressable at all: QMP's `screendump` takes a device id and a head, and with
/// no id there is no way to name the second screen. A single-screen guest needs
/// none, because its console is the default one `screendump` takes without
/// being told anything.
pub fn display_args(target: Target, console: (u32, u32), screens: u16) -> Vec<String> {
    let device = vga_for(target);
    match target {
        Target::Windows => vec!["-vga".to_owned(), device.to_owned()],
        Target::Linux if screens > 1 => {
            let outputs: Vec<serde_json::Value> = (0..screens)
                .map(|screen| {
                    serde_json::json!({
                        "name": format!("screen-{screen}"),
                        "xres": console.0,
                        "yres": console.1,
                    })
                })
                .collect();
            let spec = serde_json::json!({
                "driver": device,
                "id": DISPLAY_DEVICE_ID,
                "xres": console.0,
                "yres": console.1,
                "max_outputs": screens,
                "outputs": outputs,
            });
            vec!["-device".to_owned(), spec.to_string()]
        }
        Target::Linux => vec![
            "-device".to_owned(),
            format!("{device},xres={},yres={}", console.0, console.1),
        ],
    }
}

/// The `-device` arguments that give a guest an absolute pointer.
///
/// An absolute pointer is the fix for a cursor that sits somewhere other than
/// the host's in a VNC viewer, and for the clicks that then land where it is:
/// VNC's `PointerEvent` carries absolute coordinates, QEMU's implicit PS/2 mouse
/// is a relative device, and the input core bridges the two with deltas that the
/// guest's own pointer acceleration scales again. Measured on a running guest
/// rather than reasoned about: `query-mice` over QMP answered one device,
/// `QEMU PS/2 Mouse` with `"absolute": false`, and an `input-send-event` with an
/// `abs` axis was refused with "Input handler not found for event type abs".
///
/// Which device depends on what the guest can drive. The Linux guest takes
/// virtio-input in-kernel and the rest of its devices are virtio anyway. The
/// Windows guest has no virtio driver at all, so it gets a USB tablet, which
/// Windows binds to its inbox HID driver, and a controller to put it on, because
/// a q35 machine starts with no USB at all. Both have to be on the command line
/// from the start: `pcie.0` does not support hot-plug, so neither can be added
/// to a guest that is already running.
///
/// One tablet whatever the screen count, and one screen it is exact on.
///
/// An absolute position means nothing without the screen it is on, and QEMU
/// hands over neither half of that: it scales a head's coordinates onto the
/// whole absolute range using that head's own width and adds no offset for where
/// the head sits in the guest's desktop. So on a two-screen guest one tablet
/// speaks for a desktop twice its width and every x comes out doubled, measured
/// in the Debian guest on 2026-08-30: the middle of the first screen's window
/// put the cursor at the right edge of that screen, and the second screen's
/// window had no offset at all.
///
/// A tablet per head, bound with `display=`/`head=`, is what that asks for and is
/// not what those properties do. Both are accepted on an input device and
/// neither is resolved: `-device virtio-tablet-pci,display=nosuch` starts a guest
/// where `-vnc <addr>,display=nosuch` is refused outright, and with a tablet
/// bound to each head, pointer events from *both* VNC servers arrived at the same
/// tablet, measured by reading the guest's `/dev/input/event*` while each server
/// was sent a move. So the extra tablets bought a second device that nothing
/// spoke to, and there is one again.
///
/// What makes the remaining one exact is in the guest: `vm::map_pointer_command`
/// maps it to the primary output, so the first screen's window clicks where it
/// points and the others are for looking at.
pub fn pointer_args(target: Target) -> Vec<String> {
    match target {
        Target::Windows => vec![
            "-device".to_owned(),
            format!("{USB_CONTROLLER},id=xhci"),
            "-device".to_owned(),
            "usb-tablet,bus=xhci.0".to_owned(),
        ],
        Target::Linux => vec!["-device".to_owned(), "virtio-tablet-pci".to_owned()],
    }
}

/// The USB controller a Windows guest's tablet hangs off.
///
/// `qemu-xhci` rather than what `-usb` would pick, because that depends on the
/// machine type: q35 gets an ICH9 EHCI, which is USB 2 and one more emulated
/// device between the tablet and the guest. xHCI is inbox in Windows 10 and
/// later.
pub const USB_CONTROLLER: &str = "qemu-xhci";

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
    /// Which image this guest is a child of. The devices below are chosen from
    /// its operating system, the memory and cores from the image itself.
    pub image: Image,
    pub memory_mb: u32,
    pub cpus: u32,
    pub accelerator: String,
    pub ssh_port: u16,
    pub qmp_port: u16,
    /// One VNC display number per screen, first screen first, never empty.
    ///
    /// A list rather than a count and a base: the numbers are the ports that
    /// were free when they were picked, and a second screen whose port had to
    /// move is not one past the first.
    pub vnc_displays: Vec<u16>,
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
    /// Which session the Linux guest logs into, when the host asked for one.
    ///
    /// `None` leaves the image's own default, so a guest booted by anything that
    /// does not know about this behaves as it always did.
    pub login: Option<Login>,
}

impl Launch {
    /// How many screens this machine has, which is how many consoles it has.
    fn screens(&self) -> u16 {
        u16::try_from(self.vnc_displays.len())
            .unwrap_or(u16::MAX)
            .max(1)
    }

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
            format!("{},drive=hd0", disk_device_for(self.image.target())),
            "-netdev".into(),
            format!("user,id=net0,hostfwd=tcp:127.0.0.1:{}-:22", self.ssh_port),
            "-device".into(),
            format!("{},netdev=net0", nic_device_for(self.image.target())),
            // No local window, and a VNC server on loopback that costs nothing
            // until somebody attaches (plan decision 7). This is what makes the
            // console available at any moment without a decision up front.
            "-display".into(),
            "none".into(),
            "-qmp".into(),
            format!("tcp:127.0.0.1:{},server=on,wait=off", self.qmp_port),
            "-rtc".into(),
            "base=utc".into(),
        ];
        // One VNC server per screen, each bound to the head it shows, and QEMU
        // takes as many `-vnc` servers as it is given.
        //
        // `display=` is not optional next to `head=`, and leaving it out is the
        // trap this walked into: QEMU looks a console up only when a device is
        // named, so `-vnc <addr>,head=1` parses, starts, and serves the first
        // screen. Two servers, two windows, the same picture in both, and an
        // extended desktop behind them that neither window was showing. A device
        // id that is not there is refused outright, which is what makes this
        // worth writing rather than hoping: the wrong spelling fails at startup
        // instead of quietly showing the wrong screen.
        //
        // A single-screen guest names neither, because its display device has no
        // id and its console is the only one there is.
        for (head, display) in self.vnc_displays.iter().enumerate() {
            args.push("-vnc".into());
            if self.screens() > 1 {
                args.push(format!(
                    "127.0.0.1:{display},display={DISPLAY_DEVICE_ID},head={head}"
                ));
            } else {
                args.push(format!("127.0.0.1:{display}"));
            }
        }
        args.extend(display_args(
            self.image.target(),
            self.console,
            self.screens(),
        ));
        args.extend(pointer_args(self.image.target()));
        if let Some(login) = self.login {
            args.extend(crate::provider::desktop::fw_cfg_args(login));
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

    /// The image a state file names, defaulting to the Linux one only so the
    /// message-building path cannot panic on a corrupt file.
    fn image_of(state: &RunState) -> Image {
        state.image().unwrap_or(Image::Linux)
    }

    /// What there is to say about a QEMU that is no longer running: its last
    /// words. `start` points the process's output at the log, so a refused
    /// bind, a rejected option, and an accelerator failure all end up there,
    /// and this is the only place they can be read back from.
    fn post_mortem(&self, state: &RunState) -> String {
        let name = &state.vm_name;
        let log = self.store.vm_log(Self::image_of(state));
        match std::fs::read_to_string(&log) {
            Ok(text) => {
                let tail = log_tail(&text);
                if tail.is_empty() {
                    format!(
                        "{name}'s qemu process has exited without printing anything; \
                         its command line is the first line of {}",
                        log.display()
                    )
                } else {
                    format!(
                        "{name}'s qemu process has exited. Its log ends with:\n    {}\n  \
                         the full log is {}",
                        tail.join("\n    "),
                        log.display()
                    )
                }
            }
            Err(_) => format!(
                "{name}'s qemu process has exited, and there is no log at {}",
                log.display()
            ),
        }
    }

    /// Ask QEMU over QMP, and if the process is still there afterwards, insist.
    ///
    /// The two callers differ only in what they ask and how long they allow:
    /// `destroy` tells QEMU to quit, `stop` presses the guest's power button.
    /// Both end in the same terminate, because a QMP command that was accepted
    /// and changed nothing is indistinguishable from one that was never read,
    /// and a teardown or a stop that hangs is worse than either.
    fn end(&self, state: &RunState, command: &str, grace: Duration) -> Result<Stopped, String> {
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
                    self.store.state_file(Self::image_of(state)).display()
                ));
            }
            Ownership::Ours => {}
        }

        // QMP first, so QEMU closes the overlay itself. Killing the process
        // works too, at the cost of a qcow2 that needs a repair pass on the
        // next boot, which is a cost a teardown does not care about and a stop
        // does.
        let asked = state
            .qmp_port
            .is_some_and(|port| qmp::execute(port, command).is_ok());
        if asked && self.wait_for_exit(state, grace) {
            return Ok(Stopped::ShutDown);
        }

        // Said rather than done quietly: the caller keeping this overlay is
        // about to be told the guest is stopped, and the difference between a
        // guest that shut itself down and one that was killed is a repair pass
        // it would otherwise meet for the first time on the next boot.
        println!("{}", insist_line(&state.vm_name, asked, grace));
        self.runner
            .terminate(pid)
            .map_err(|e| format!("cannot terminate pid {pid}: {e}"))?;
        if self.wait_for_exit(state, QUIT_GRACE) {
            Ok(Stopped::Killed)
        } else {
            Err(format!(
                "pid {pid} is still running after being asked and then told to stop"
            ))
        }
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

    /// The launch parameters for a target, from its record.
    ///
    /// The ports come off the record rather than out of the constants, because
    /// `create_from_golden` may have had to move off a preferred port that
    /// would not bind. The fallbacks are for a record written before ports
    /// could move, whose serde defaults left `0` and `None` behind; no guest
    /// was ever forwarded on port 0.
    pub fn launch_for(&self, image: Image, state: &RunState) -> Launch {
        let (memory_mb, cpus) = crate::provider::resources_for(image);
        let firmware = if image.target() == Target::Windows {
            self.per_vm_firmware(image)
        } else {
            None
        };
        Launch {
            name: image.vm_name(),
            overlay: state.overlay.clone(),
            image,
            memory_mb,
            cpus,
            accelerator: crate::commands::build_image::accelerator_for(self.host).to_owned(),
            ssh_port: if state.ssh_port == 0 {
                SSH_PORT
            } else {
                state.ssh_port
            },
            qmp_port: state.qmp_port.unwrap_or(QMP_PORT),
            vnc_displays: vnc_displays_of(state),
            firmware,
            console: console::requested_resolution(true).unwrap_or(DEFAULT_CONSOLE),
            login: state.login(),
        }
    }

    /// Copy the firmware's variables store into the run directory, so each VM
    /// writes its boot entries into its own throwaway copy rather than into the
    /// shared one the host installed.
    fn per_vm_firmware(&self, image: Image) -> Option<crate::provider::firmware::Firmware> {
        let binary = crate::host::facts::resolve_tool(self.runner, "qemu-system-x86_64", self.host);
        let found = crate::provider::firmware::locate(self.host, binary.as_deref())?;
        let copy = self.store.firmware_vars(image);
        if std::fs::create_dir_all(self.store.run_dir(image)).is_err()
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

/// Record how the guest will be reached, which is three loopback ports.
///
/// Picked now and recorded rather than fixed: `start` builds the command line
/// from the record and everything later reads it too, and on a Windows host
/// `WinNAT` reserves hundred-port blocks at moments of its own choosing, so a
/// fixed port is a QEMU that exits before it has built the machine.
/// What a stop or a destroy says at the moment it stops asking.
///
/// Two ways to arrive here and they are not the same fault: a guest that was
/// asked and spent the whole grace is one that will not comply, and a guest that
/// could not be asked at all is a QMP socket that is not answering. The line
/// names which, because the first is the guest's doing and the second is this
/// host's.
pub fn insist_line(vm_name: &str, asked: bool, grace: Duration) -> String {
    let why = if asked {
        format!("has not shut down in {:.0}s", grace.as_secs_f64())
    } else {
        "could not be asked to shut down over QMP".to_owned()
    };
    format!("  {vm_name} {why}; killing it, so its disk will need a repair pass")
}

pub fn fill_in_address(state: &mut RunState) -> Result<(), String> {
    "127.0.0.1".clone_into(&mut state.ssh_host);
    GUEST_USER.clone_into(&mut state.ssh_user);
    state.ssh_port = pick_port("ssh", SSH_PORT)?;
    state.qmp_port = Some(pick_port("qmp", QMP_PORT)?);
    state.vnc = Some(format!(
        "127.0.0.1:{}",
        pick_port("vnc", vnc_port(VNC_DISPLAY))?
    ));
    Ok(())
}

/// Give a guest one console per screen, on top of the first one
/// [`fill_in_address`] already picked.
///
/// Nothing at all for a single-screen guest, which is every guest but the one
/// that asked for more, so the common boot picks the ports it always picked.
///
/// Each further port is walked from one past the last rather than from the base,
/// which is what keeps two screens off the same port; they come out consecutive
/// on a host with nothing in the way and do not have to be. No message when one
/// moves, unlike the three fixed ports: there is no remembered port for a second
/// screen to have moved off, and the boot prints every console address anyway.
pub fn fill_in_consoles(state: &mut RunState) -> Result<(), String> {
    let screens = usize::from(state.screen_count());
    if screens < 2 {
        return Ok(());
    }
    let first = state
        .vnc
        .clone()
        .unwrap_or_else(|| format!("127.0.0.1:{}", vnc_port(VNC_DISPLAY)));
    let mut last = display_of_address(&first);
    let mut heads = vec![first];
    while heads.len() < screens {
        let port = free_port_from(vnc_port(last).saturating_add(1))?;
        last = port.saturating_sub(VNC_BASE_PORT);
        heads.push(format!("127.0.0.1:{port}"));
    }
    state.vnc_heads = heads;
    Ok(())
}

/// `qemu-img create` for a differencing child of another disk: a throwaway
/// overlay of a golden image, or the layer a builder image is.
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

    fn create_from_golden(&self, image: Image, reason: StartReason) -> Result<RunState, String> {
        let golden = self.store.qcow2(image);
        if !golden.is_file() {
            return Err(format!(
                "no {image} image at {}; `cargo xtask vm build-image {image}` builds one",
                golden.display()
            ));
        }
        let overlay = self.store.qemu_overlay(image);
        let run_dir = self.store.run_dir(image);
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

        let mut state = RunState::new(image, ProviderKind::Qemu, overlay, reason, util::now_unix());
        fill_in_address(&mut state)?;
        Ok(state)
    }

    fn start(&self, state: &mut RunState) -> Result<(), String> {
        let image = state
            .image()
            .ok_or_else(|| format!("unknown image '{}'", state.image))?;
        let binary = self.qemu_binary()?;
        // The one thing that cannot be picked when the record is created: how
        // many screens the guest has is decided by the command that boots it,
        // and that command writes it to the record after the provider made one.
        fill_in_consoles(state)?;
        // Everything about how to reach the guest, the desktop and the ports,
        // comes off the record rather than out of parameters: the command that
        // chose them is finished by the time anything starts a process, and
        // `vm status` has to be able to say the same things about this guest.
        let launch = self.launch_for(image, state);
        launch.validate()?;
        if image.target() == Target::Linux {
            let (width, height) = launch.console;
            let session = if image.has_desktop() {
                launch.login.map_or_else(
                    || "the image's own default desktop".to_owned(),
                    |d| format!("the {} session", d.label()),
                )
            } else {
                "a text console, since this image has no desktop".to_owned()
            };
            let screens = match launch.screens() {
                1 => String::new(),
                n => format!(", {n} screens"),
            };
            println!("console: {width}x{height}{screens}, into {session}");
        }
        let log = self.store.vm_log(image);
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
        // `quit` rather than a shutdown request: the guest holds nothing worth
        // flushing, its overlay is about to be deleted, and waiting for Windows
        // to shut down politely costs a minute per run.
        self.end(state, "quit", QUIT_GRACE)
    }

    fn stop(&self, state: &RunState) -> Result<Stopped, String> {
        // The ACPI power button, which is a request the guest carries out
        // itself: what this keeps is a build directory in the guest's own
        // filesystem, so the guest is the only thing that can close it cleanly.
        self.end(state, "system_powerdown", crate::provider::SHUTDOWN_GRACE)
    }

    fn readdress(&self, state: &mut RunState) -> Result<(), String> {
        fill_in_address(state)
    }

    fn is_running(&self, state: &RunState) -> bool {
        matches!(self.ownership(state), Ownership::Ours)
    }

    fn defunct(&self, state: &RunState) -> Option<String> {
        // `Foreign` means the pid was reused, which is the same verdict as
        // `Gone`: our process has exited, and what it had to say is in the log.
        match self.ownership(state) {
            Ownership::Ours => None,
            Ownership::Gone | Ownership::Foreign(_) => Some(self.post_mortem(state)),
        }
    }

    /// One viewer per screen, because a guest's second screen is as much its
    /// console as its first and there is no order in which to show one of them.
    fn view(&self, state: &RunState) -> Result<String, String> {
        let addresses: Vec<String> = match state.consoles().as_slice() {
            [] => vec![format!("127.0.0.1:{}", vnc_port(VNC_DISPLAY))],
            found => found.iter().map(|&a| a.to_owned()).collect(),
        };
        // `resolve_tool` rather than a bare `PATH` lookup, so that the answer
        // is the same one `vm doctor` reports. A viewer installed by a package
        // manager that appends to the user `PATH`, which is both winget and
        // scoop, is invisible to every shell that started before it did.
        for viewer in crate::host::facts::VNC_VIEWERS {
            if let Some(path) = crate::host::facts::resolve_tool(self.runner, viewer, self.host) {
                for (opened, address) in addresses.iter().enumerate() {
                    let argument = crate::host::facts::vnc_viewer_argument(viewer, address);
                    self.runner
                        .spawn(&Cmd::new(path.to_string_lossy()).arg(argument), None)
                        // One viewer per screen means a failure partway through
                        // leaves windows open, and a message naming only what
                        // failed reads as though nothing started.
                        .map_err(|e| match opened {
                            0 => format!("cannot start {viewer}: {e}"),
                            1 => format!(
                                "cannot start {viewer} for the screen at {address}: {e}. \
                                 The viewer already open on the first screen stays open."
                            ),
                            open => format!(
                                "cannot start {viewer} for the screen at {address}: {e}. \
                                 The {open} viewers already open stay open."
                            ),
                        })?;
                }
                return Ok(format!(
                    "{viewer} is connecting to {}",
                    addresses.join(" and ")
                ));
            }
        }
        Ok(format!(
            "no VNC viewer found. The {console} at {}, with no password; \
             point any VNC client at {them}. `cargo xtask vm doctor` lists the \
             names looked for.",
            addresses.join(" and "),
            console = if addresses.len() == 1 {
                "console is"
            } else {
                "consoles are"
            },
            them = if addresses.len() == 1 { "it" } else { "them" },
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
    use crate::provider::desktop::{Desktop, SessionType};
    use crate::runner::fake::FakeRunner;

    fn launch(image: Image) -> Launch {
        let (memory_mb, cpus) = crate::provider::resources_for(image);
        Launch {
            name: image.vm_name(),
            overlay: PathBuf::from("/srv/vm/run/linux/overlay.qcow2"),
            image,
            memory_mb,
            cpus,
            accelerator: "kvm".to_owned(),
            ssh_port: SSH_PORT,
            qmp_port: QMP_PORT,
            vnc_displays: vec![VNC_DISPLAY],
            firmware: None,
            console: DEFAULT_CONSOLE,
            login: None,
        }
    }

    fn joined(image: Image) -> String {
        launch(image).args().join(" ")
    }

    /// A guest with two heads, which is what `--screens 2` records.
    fn two_screen_state() -> RunState {
        let mut state = RunState::new(
            Image::Linux,
            ProviderKind::Qemu,
            PathBuf::from("/srv/vm/run/linux/overlay.qcow2"),
            StartReason::Run,
            0,
        );
        state.vnc_heads = vec!["127.0.0.1:5919".to_owned(), "127.0.0.1:5920".to_owned()];
        state.screens = Some(2);
        state
    }

    /// Every screen gets its own window. A guest's second screen is as much its
    /// console as the first, and opening one of them would be a choice nothing
    /// here is entitled to make.
    #[test]
    fn a_two_screen_guest_opens_a_viewer_on_each_of_them() {
        let store = Store::new("/srv/vm");
        let runner = FakeRunner::new().with_tool("vncviewer", "/usr/bin/vncviewer");
        let provider = QemuProvider::new(&runner, &store, HostOs::Linux);

        let said = provider.view(&two_screen_state()).expect("both viewers");
        assert!(said.contains("127.0.0.1:5919 and 127.0.0.1:5920"), "{said}");
        let spawned = runner.spawned.borrow().clone();
        assert_eq!(spawned.len(), 2, "{spawned:?}");
        assert!(spawned[0].contains("127.0.0.1:5919"), "{spawned:?}");
        assert!(spawned[1].contains("127.0.0.1:5920"), "{spawned:?}");
    }

    /// A viewer that fails on the second screen has already put a window on the
    /// first, and a message naming only the failure reads as though nothing
    /// started at all.
    #[test]
    fn a_viewer_that_fails_on_a_later_screen_says_what_is_already_open() {
        let store = Store::new("/srv/vm");
        let runner = FakeRunner::new()
            .with_tool("vncviewer", "/usr/bin/vncviewer")
            .failing_to_spawn("5920");
        let provider = QemuProvider::new(&runner, &store, HostOs::Linux);

        let refusal = provider
            .view(&two_screen_state())
            .expect_err("the second viewer was refused");
        assert!(refusal.contains("127.0.0.1:5920"), "{refusal}");
        assert!(refusal.contains("already open"), "{refusal}");
        assert_eq!(runner.spawned.borrow().len(), 1, "the first one did start");
    }

    /// With no viewer on the host the addresses are all the answer there is, so
    /// both of them have to be in it, in the plural.
    #[test]
    fn two_consoles_with_no_viewer_are_both_named() {
        let store = Store::new("/srv/vm");
        let runner = FakeRunner::new();
        let provider = QemuProvider::new(&runner, &store, HostOs::Linux);

        let said = provider
            .view(&two_screen_state())
            .expect("advice, not a viewer");
        assert!(said.contains("127.0.0.1:5919 and 127.0.0.1:5920"), "{said}");
        assert!(said.contains("consoles are"), "{said}");
        assert!(said.contains("point any VNC client at them"), "{said}");
        assert!(runner.spawned.borrow().is_empty(), "nothing to start");
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
            Image::Linux,
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
            Image::Linux,
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
            Image::Linux,
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
        // `Killed` rather than `Stopped`, because nothing asked the guest
        // anything: a teardown counts the two alike, since the overlay whose
        // repair pass they differ over is deleted moments later.
        assert_eq!(provider.destroy(&state), Ok(Stopped::Killed));
        assert_eq!(*runner.terminated.borrow(), vec![4242]);
        // Gone afterwards, which is what `wait_for_exit` had to observe for
        // the destroy to report success at all.
        assert!(!provider.is_running(&state));
    }

    /// A stop that could not ask still insists, because everything a builder
    /// keeps is a cache and a stop that hangs is worse than a cold build. What
    /// separates it from a destroy is only the grace and the request; the
    /// process going away is the same observation.
    #[test]
    fn a_stop_that_cannot_ask_the_guest_still_ends_it() {
        let store = Store::new("/srv/vm");
        let runner = FakeRunner::new().with_process(
            4242,
            "qemu-system-x86_64",
            Some("qemu-system-x86_64 -name sunlit-e2e-windows-builder"),
        );
        let provider = QemuProvider::new(&runner, &store, HostOs::Linux);
        let mut state = RunState::new(
            Image::WindowsBuilder,
            ProviderKind::Qemu,
            PathBuf::from("/srv/vm/run/windows-builder/overlay.qcow2"),
            StartReason::Suite,
            0,
        );
        state.pid = Some(4242);
        // No QMP port, so nothing here opens a socket to whatever is on 4444.
        state.qmp_port = None;

        // And it says which it was: a stop keeps the overlay, so the caller has
        // to know it is keeping one that was never closed.
        assert_eq!(provider.stop(&state), Ok(Stopped::Killed));
        assert_eq!(*runner.terminated.borrow(), vec![4242]);
        assert!(!provider.is_running(&state));
        // And a guest that is already gone is nothing to stop, which is what
        // tells `vm stop` to say so rather than to report a stop it did not do.
        assert_eq!(provider.stop(&state), Ok(Stopped::WasNotRunning));
    }

    /// A resumed guest cannot trust the ports in its own record: while it was
    /// stopped, 2222 may have gone to a desktop guest, and QEMU exits over a
    /// port it cannot bind before it has built the machine.
    #[test]
    fn a_resumed_guest_picks_its_ports_again() {
        let store = Store::new("/srv/vm");
        let runner = FakeRunner::new();
        let provider = QemuProvider::new(&runner, &store, HostOs::Linux);
        let mut state = RunState::new(
            Image::WindowsBuilder,
            ProviderKind::Qemu,
            PathBuf::from("/srv/vm/run/windows-builder/overlay.qcow2"),
            StartReason::Suite,
            0,
        );
        state.ssh_port = 0;
        state.qmp_port = None;
        state.vnc = None;

        provider.readdress(&mut state).expect("free ports");
        assert!(state.ssh_port >= SSH_PORT, "{}", state.ssh_port);
        assert_eq!(state.ssh_host, "127.0.0.1");
        assert_eq!(state.ssh_user, GUEST_USER);
        assert!(state.qmp_port.is_some_and(|port| port >= QMP_PORT));
        assert!(
            state
                .vnc
                .as_deref()
                .is_some_and(|vnc| vnc.starts_with("127.0.0.1:")),
            "{:?}",
            state.vnc
        );
    }

    /// The two ways to reach a kill are not the same fault, and the line has to
    /// say which: one is a guest that will not comply, the other is a QMP socket
    /// on this host that is not answering. Both warn about the repair pass,
    /// because that is what the next boot of a kept overlay will do.
    #[test]
    fn the_line_before_a_kill_names_which_of_the_two_it_is() {
        let asked = insist_line("sunlit-e2e-windows-builder", true, Duration::from_secs(60));
        assert!(asked.contains("has not shut down in 60s"), "{asked}");
        assert!(asked.contains("repair pass"), "{asked}");

        let unasked = insist_line("sunlit-e2e-windows-builder", false, Duration::from_secs(60));
        assert!(unasked.contains("could not be asked"), "{unasked}");
        assert!(!unasked.contains("60s"), "{unasked}");
        assert!(unasked.contains("repair pass"), "{unasked}");
    }

    #[test]
    fn destroying_a_vm_that_is_already_gone_is_not_a_stop() {
        let store = Store::new("/srv/vm");
        let runner = FakeRunner::new();
        let provider = QemuProvider::new(&runner, &store, HostOs::Linux);
        let mut state = RunState::new(
            Image::Linux,
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
    fn a_free_port_is_taken_as_it_is() {
        // An OS-assigned port, freed the moment before it is asked about, so
        // the test never depends on what else this machine runs.
        let free = std::net::TcpListener::bind(("127.0.0.1", 0))
            .expect("bind")
            .local_addr()
            .expect("addr")
            .port();
        assert_eq!(free_port_from(free), Ok(free));
    }

    #[test]
    fn a_taken_port_is_walked_past() {
        let held = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let taken = held.local_addr().expect("addr").port();
        if taken > 65_000 {
            // At the very top of the range there may be no room left to walk
            // into; the OS hands ports out from the middle of it, so this is
            // one draw in tens of thousands.
            println!("skipping: the OS handed back {taken}, too near the top of the range");
            return;
        }
        let picked = free_port_from(taken).expect("a later port is free");
        assert!(picked > taken, "{picked} is not past {taken}");
        assert!(picked < taken + PORT_WALK, "{picked} left the walk");
        drop(held);
    }

    #[test]
    fn the_three_port_walks_cannot_meet() {
        // Each preferred port is more than a whole walk from the next, so the
        // three picks can never hand out the same port however far the ones
        // before them had to move.
        assert!(u32::from(SSH_PORT) + u32::from(PORT_WALK) <= u32::from(QMP_PORT));
        assert!(u32::from(QMP_PORT) + u32::from(PORT_WALK) <= u32::from(vnc_port(VNC_DISPLAY)));
    }

    fn recorded_state() -> RunState {
        RunState::new(
            Image::Linux,
            ProviderKind::Qemu,
            PathBuf::from("/srv/vm/run/linux/overlay.qcow2"),
            StartReason::Run,
            0,
        )
    }

    #[test]
    fn the_command_line_is_built_from_the_record() {
        // The record is where `create_from_golden` put the ports it could
        // actually bind, so a launch built from the constants would undo the
        // picking exactly where it matters.
        let store = Store::new("/srv/vm");
        let runner = FakeRunner::new();
        let provider = QemuProvider::new(&runner, &store, HostOs::Linux);
        let mut state = recorded_state();
        state.ssh_port = 2224;
        state.qmp_port = Some(4446);
        state.vnc = Some("127.0.0.1:5903".to_owned());
        let text = provider.launch_for(Image::Linux, &state).args().join(" ");
        assert!(text.contains("hostfwd=tcp:127.0.0.1:2224-:22"), "{text}");
        assert!(
            text.contains("-qmp tcp:127.0.0.1:4446,server=on,wait=off"),
            "{text}"
        );
        assert!(text.contains("-vnc 127.0.0.1:3"), "{text}");
    }

    #[test]
    fn the_viewer_is_handed_the_address_in_the_form_it_accepts() {
        let store = Store::new("/srv/vm");
        let mut state = recorded_state();
        state.vnc = Some("127.0.0.1:5903".to_owned());

        let runner = FakeRunner::new().with_tool("krdc", "/usr/bin/krdc");
        let provider = QemuProvider::new(&runner, &store, HostOs::Linux);
        let note = provider.view(&state).expect("a viewer was found");
        assert_eq!(runner.spawned.borrow().len(), 1);
        assert!(
            runner.spawned.borrow()[0].contains("vnc://127.0.0.1:5903"),
            "{:?}",
            runner.spawned.borrow()
        );
        // The note is about the console, not about the URL the client wanted.
        assert!(note.contains("127.0.0.1:5903"), "{note}");

        let runner = FakeRunner::new().with_tool("vncviewer", "/usr/bin/vncviewer");
        let provider = QemuProvider::new(&runner, &store, HostOs::Linux);
        provider.view(&state).expect("a viewer was found");
        assert!(
            !runner.spawned.borrow()[0].contains("vnc://"),
            "{:?}",
            runner.spawned.borrow()
        );
    }

    #[test]
    fn a_record_from_before_ports_could_move_launches_on_the_usual_ones() {
        // serde's defaults for an old vm.json leave 0 and None behind, and no
        // guest was ever forwarded on port 0.
        let store = Store::new("/srv/vm");
        let runner = FakeRunner::new();
        let provider = QemuProvider::new(&runner, &store, HostOs::Linux);
        let mut state = recorded_state();
        state.ssh_port = 0;
        state.qmp_port = None;
        state.vnc = None;
        let text = provider.launch_for(Image::Linux, &state).args().join(" ");
        assert!(text.contains("hostfwd=tcp:127.0.0.1:2222-:22"), "{text}");
        assert!(text.contains("tcp:127.0.0.1:4444,server=on"), "{text}");
        assert!(text.contains("-vnc 127.0.0.1:0"), "{text}");
    }

    #[test]
    fn the_recorded_vnc_address_names_the_display_qemu_is_given() {
        let mut state = recorded_state();
        state.vnc = Some("127.0.0.1:5903".to_owned());
        assert_eq!(vnc_display_of(&state), 3);
        state.vnc = Some("127.0.0.1:5900".to_owned());
        assert_eq!(vnc_display_of(&state), 0);
        state.vnc = None;
        assert_eq!(vnc_display_of(&state), 0);
        state.vnc = Some("not an address".to_owned());
        assert_eq!(vnc_display_of(&state), 0);
    }

    #[test]
    fn the_log_tail_skips_the_command_line_and_keeps_the_last_words() {
        use std::fmt::Write as _;

        let text = "qemu-system-x86_64 -name x\n\nline one\nline two\n";
        assert_eq!(log_tail(text), vec!["line one", "line two"]);

        let mut long = String::from("the command line\n");
        for i in 0..20 {
            let _ = writeln!(long, "line {i}");
        }
        let tail = log_tail(&long);
        assert_eq!(tail.len(), LOG_TAIL_LINES);
        assert_eq!(tail.last(), Some(&"line 19"));

        // A log holding only the command line has no last words.
        assert!(log_tail("the command line\n").is_empty());
        assert!(log_tail("").is_empty());
    }

    #[test]
    fn a_running_guest_is_not_defunct() {
        let store = Store::new("/srv/vm");
        let runner = FakeRunner::new().with_process(
            4242,
            "qemu-system-x86_64",
            Some("qemu-system-x86_64 -name sunlit-e2e-linux"),
        );
        let provider = QemuProvider::new(&runner, &store, HostOs::Linux);
        let mut state = recorded_state();
        state.pid = Some(4242);
        assert_eq!(provider.defunct(&state), None);
    }

    #[test]
    fn a_dead_qemu_reports_its_last_words_from_the_log() {
        // The message that ends the SSH wait: QEMU's own words, not the
        // command line the runner wrote as the log's first line.
        let dir = std::env::temp_dir().join("sunlit_xtask_post_mortem");
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::new(&dir);
        let log = store.vm_log(Image::Linux);
        std::fs::create_dir_all(log.parent().expect("run dir")).expect("create");
        std::fs::write(
            &log,
            "qemu-system-x86_64.exe -name sunlit-e2e-linux -qmp tcp:127.0.0.1:4444\n\
             qemu-system-x86_64.exe: -qmp tcp:127.0.0.1:4444,server=on,wait=off: \
             Failed to bind socket: Input/output error\n",
        )
        .expect("write");

        let runner = FakeRunner::new();
        let provider = QemuProvider::new(&runner, &store, HostOs::Windows);
        let mut state = recorded_state();
        state.pid = Some(37380);
        let reason = provider.defunct(&state).expect("the process is gone");
        assert!(reason.contains("Failed to bind socket"), "{reason}");
        assert!(reason.contains(&log.display().to_string()), "{reason}");
        assert!(!reason.contains("-name sunlit-e2e-linux"), "{reason}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dead_qemu_with_no_log_still_names_where_it_would_be() {
        let store = Store::new("/srv/vm/nowhere");
        let runner = FakeRunner::new();
        let provider = QemuProvider::new(&runner, &store, HostOs::Linux);
        let mut state = recorded_state();
        state.pid = Some(37380);
        let reason = provider.defunct(&state).expect("the process is gone");
        assert!(reason.contains("has exited"), "{reason}");
        assert!(reason.contains("vm.log"), "{reason}");
    }

    #[test]
    fn the_command_line_forwards_ssh_to_loopback_only() {
        let text = joined(Image::Linux);
        assert!(text.contains("hostfwd=tcp:127.0.0.1:2222-:22"), "{text}");
        // Binding the forward to 0.0.0.0 would put a passwordless guest on the
        // network, which is the one thing this must not do.
        assert!(!text.contains("hostfwd=tcp::"), "{text}");
    }

    #[test]
    fn the_console_is_always_available_on_loopback_vnc() {
        let text = joined(Image::Linux);
        assert!(text.contains("-display none"), "{text}");
        assert!(text.contains("-vnc 127.0.0.1:0"), "{text}");
    }

    /// A screen is a VNC server bound to a head, a scanout on the device to bind
    /// it to, and a size on that scanout, and none of the three is any use
    /// without the others: without `max_outputs` the guest has one connector
    /// however many servers are listening, without `head=` every server shows the
    /// first screen, and without a size in `outputs` the guest reports the second
    /// connector as disconnected and the session has one screen.
    #[test]
    fn a_second_screen_is_a_second_console_and_a_scanout_with_a_size() {
        let mut launch = launch(Image::Linux);
        launch.vnc_displays = vec![0, 1];
        let text = launch.args().join(" ");
        assert!(
            text.contains(&format!(
                "-vnc 127.0.0.1:0,display={DISPLAY_DEVICE_ID},head=0"
            )),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                "-vnc 127.0.0.1:1,display={DISPLAY_DEVICE_ID},head=1"
            )),
            "{text}"
        );

        let device = display_args(Target::Linux, (1920, 1080), 2);
        let spec: serde_json::Value =
            serde_json::from_str(&device[1]).expect("the device is written as JSON");
        assert_eq!(spec["driver"], "virtio-vga");
        assert_eq!(spec["max_outputs"], 2);
        assert_eq!(
            spec["id"], DISPLAY_DEVICE_ID,
            "without an id there is no way to name a screen but the first: {spec}"
        );
        let outputs = spec["outputs"].as_array().expect("one entry per screen");
        assert_eq!(outputs.len(), 2, "{spec}");
        for output in outputs {
            assert_eq!(output["xres"], 1920, "{spec}");
            assert_eq!(output["yres"], 1080, "{spec}");
            assert!(
                output["name"].as_str().is_some_and(|n| !n.is_empty()),
                "the property requires a name: {spec}"
            );
        }
    }

    /// The ports are whatever was free when they were picked, so the second
    /// screen's is not necessarily one past the first's, and the head numbers
    /// are positions rather than ports.
    #[test]
    fn a_screen_that_had_to_move_keeps_its_place_in_the_order() {
        let mut launch = launch(Image::Linux);
        launch.vnc_displays = vec![3, 9];
        let text = launch.args().join(" ");
        assert!(
            text.contains(&format!(
                "-vnc 127.0.0.1:3,display={DISPLAY_DEVICE_ID},head=0"
            )),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                "-vnc 127.0.0.1:9,display={DISPLAY_DEVICE_ID},head=1"
            )),
            "{text}"
        );
    }

    /// The guest everything else boots is told what it has always been told: one
    /// `-vnc`, and a display device with no `max_outputs` on it.
    #[test]
    fn a_one_screen_guest_gets_the_command_line_it_always_got() {
        let text = joined(Image::Linux);
        assert_eq!(text.matches("-vnc ").count(), 1, "{text}");
        assert!(!text.contains("max_outputs"), "{text}");
        assert!(
            text.contains(&format!(
                "-device virtio-vga,xres={},yres={}",
                DEFAULT_CONSOLE.0, DEFAULT_CONSOLE.1
            )),
            "{text}"
        );
    }

    /// The Windows guest's display has no such property, and the flag that would
    /// ask for one is refused a long way before this.
    #[test]
    fn the_windows_display_takes_no_outputs_whatever_it_is_asked() {
        let args = display_args(Target::Windows, (1920, 1080), 2).join(" ");
        assert_eq!(args, "-vga std");
    }

    #[test]
    fn a_record_with_one_console_describes_one_screen() {
        let mut state = recorded_state();
        state.vnc = Some("127.0.0.1:5903".to_owned());
        assert_eq!(vnc_displays_of(&state), vec![3]);
        // And a record with no console at all is still a machine with a screen.
        state.vnc = None;
        assert_eq!(vnc_displays_of(&state), vec![VNC_DISPLAY]);
    }

    #[test]
    fn a_record_with_two_consoles_describes_them_in_order() {
        let mut state = recorded_state();
        state.vnc = Some("127.0.0.1:5903".to_owned());
        state.vnc_heads = vec!["127.0.0.1:5903".to_owned(), "127.0.0.1:5907".to_owned()];
        assert_eq!(vnc_displays_of(&state), vec![3, 7]);
        // The single-console field stays the first screen's, so everything that
        // wants one console gets the one a person would look at first.
        assert_eq!(vnc_display_of(&state), 3);
    }

    /// The ports are real: the picker binds them to find out, so this asserts
    /// what came back rather than what was hoped for.
    #[test]
    fn the_extra_consoles_are_picked_past_the_one_that_came_first() {
        let mut state = recorded_state();
        state.vnc = Some("127.0.0.1:5900".to_owned());
        state.screens = Some(3);
        fill_in_consoles(&mut state).expect("three loopback ports");
        assert_eq!(state.vnc_heads.len(), 3, "{:?}", state.vnc_heads);
        assert_eq!(state.vnc_heads[0], "127.0.0.1:5900");
        let displays = vnc_displays_of(&state);
        assert!(
            displays.windows(2).all(|pair| pair[0] < pair[1]),
            "the screens share a port or went backwards: {displays:?}"
        );
    }

    #[test]
    fn a_one_screen_guest_is_left_with_the_console_it_was_given() {
        let mut state = recorded_state();
        state.vnc = Some("127.0.0.1:5900".to_owned());
        fill_in_consoles(&mut state).expect("nothing to pick");
        assert!(state.vnc_heads.is_empty(), "{:?}", state.vnc_heads);
        assert_eq!(state.consoles(), vec!["127.0.0.1:5900"]);
    }

    #[test]
    fn the_guest_cpu_is_asked_for_rather_than_left_to_qemu() {
        // With QEMU's default `qemu64`, WHPX kills the vCPU as soon as the
        // Windows boot manager runs ("Unexpected VP exit code 4"), and the VM
        // then sits at the firmware logo forever. Both Packer templates carry
        // the same flag, which their own test checks.
        for image in Image::ALL {
            assert!(joined(image).contains("-cpu max"), "{image}");
        }
    }

    #[test]
    fn qmp_listens_without_blocking_the_boot() {
        let text = joined(Image::Linux);
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
            joined(Image::Windows).contains("-device ide-hd,drive=hd0"),
            "{}",
            joined(Image::Windows)
        );
        assert!(
            joined(Image::Linux).contains("-device virtio-blk-pci,drive=hd0"),
            "{}",
            joined(Image::Linux)
        );
        for image in Image::ALL {
            assert!(joined(image).contains("if=none,id=hd0"), "{image}");
        }
    }

    #[test]
    fn a_bad_drive_interface_is_refused_before_anything_is_spawned() {
        let mut broken = launch(Image::Windows);
        broken.overlay = PathBuf::from("/srv/vm/o.qcow2,if=ahci");
        let err = broken.validate().unwrap_err();
        assert!(err.contains("'ahci' is not one QEMU accepts"), "{err}");
        assert!(launch(Image::Linux).validate().is_ok());
    }

    #[test]
    fn every_drive_uses_an_interface_qemu_accepts() {
        // `if=` is not free-form. An unaccepted value makes QEMU exit at
        // startup, and because the process is detached with its output in a
        // log, that surfaces ten minutes later as an SSH timeout.
        let mut with_firmware = launch(Image::Windows);
        with_firmware.firmware = Some(crate::provider::firmware::Firmware {
            code: PathBuf::from("/fw/code.fd"),
            vars: PathBuf::from("/fw/vars.fd"),
        });
        for launch in [launch(Image::Linux), launch(Image::Windows), with_firmware] {
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
        let (windows, _) = crate::provider::resources_for(Image::Windows);
        let (linux, _) = crate::provider::resources_for(Image::Linux);
        assert!(windows > linux);
    }

    #[test]
    fn firmware_is_passed_only_when_there_is_some() {
        assert!(!joined(Image::Linux).contains("pflash"));
        let mut with_firmware = launch(Image::Windows);
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
        assert!(joined(Image::Linux).contains("-rtc base=utc"));
    }

    #[test]
    fn the_vm_carries_the_ownership_prefix_into_qemu() {
        assert!(joined(Image::Linux).contains("-name sunlit-e2e-linux"));
    }

    #[test]
    fn every_guest_gets_an_absolute_pointer_so_the_cursor_is_where_the_host_put_it() {
        // VNC sends absolute coordinates and QEMU's implicit PS/2 mouse is a
        // relative device, so a guest without an absolute pointer of its own
        // gets deltas and its own acceleration on top of them, and its cursor
        // drifts away from the host's. Each guest takes the device it has a
        // driver for.
        assert!(
            joined(Image::Linux).contains("-device virtio-tablet-pci"),
            "{}",
            joined(Image::Linux)
        );
        for image in [Image::Windows, Image::WindowsBuilder] {
            let text = joined(image);
            assert!(text.contains("-device usb-tablet,bus=xhci.0"), "{text}");
            // The tablet needs a controller, and the controller has to be named
            // before the device that sits on it.
            let controller = text.find(USB_CONTROLLER).expect("a USB controller");
            assert!(
                controller < text.find("usb-tablet").expect("a tablet"),
                "{text}"
            );
            assert!(!text.contains("virtio-tablet"), "{text}");
        }
    }

    /// One tablet per head is what a two-screen guest wants and not what QEMU's
    /// `display=` gives on an input device: it is accepted, never resolved, and
    /// both screens' pointer events were measured arriving at the same tablet.
    /// A second device nothing speaks to is worse than none, so there is one.
    #[test]
    fn a_guest_with_two_screens_still_has_one_pointer() {
        let mut launch = launch(Image::Linux);
        launch.vnc_displays = vec![0, 1];
        let text = launch.args().join(" ");
        assert_eq!(
            text.matches("virtio-tablet-pci").count(),
            1,
            "a tablet per head is what this wants and not what QEMU's \
             display= does on an input device: {text}"
        );
        assert!(!text.contains("virtio-tablet-pci,display"), "{text}");
    }

    /// `head=` on a VNC server is silently ignored unless `display=` names the
    /// device the head is on: QEMU looks a console up only when it has a device,
    /// so without one both servers serve the first screen and the second window
    /// is a copy of the first. A device id that does not exist is refused at
    /// startup, which is why naming it is safe and leaving it out is not.
    #[test]
    fn a_screens_vnc_server_names_the_device_its_head_is_on() {
        let mut launch = launch(Image::Linux);
        launch.vnc_displays = vec![0, 1];
        let text = launch.args().join(" ");
        for head in 0..2 {
            assert!(
                text.contains(&format!(
                    "-vnc 127.0.0.1:{head},display={DISPLAY_DEVICE_ID},head={head}"
                )),
                "{text}"
            );
        }
        // And a one-screen guest names neither, because its display device has
        // no id and its console is the only one there is.
        let single = joined(Image::Linux);
        assert!(single.contains("-vnc 127.0.0.1:0 "), "{single}");
        assert!(!single.contains("display=gpu"), "{single}");
    }

    #[test]
    fn the_linux_console_is_a_size_the_host_chose_rather_than_one_the_guest_picked() {
        // `-device virtio-vga` rather than `-vga virtio`, because only the
        // device form carries the properties that set the preferred mode.
        let args = launch(Image::Linux).args();
        assert!(
            args.join(" ")
                .contains("-device virtio-vga,xres=1920,yres=1080"),
            "{args:?}"
        );
        assert!(!args.iter().any(|arg| arg == "-vga"), "{args:?}");

        let mut small = launch(Image::Linux);
        small.console = (1280, 800);
        assert!(
            small.args().join(" ").contains("xres=1280,yres=800"),
            "{:?}",
            small.args()
        );

        // The Windows guest's emulated VGA has no such property, and its console
        // is a Hyper-V matter anyway.
        let windows = joined(Image::Windows);
        assert!(windows.contains("-vga std"), "{windows}");
        assert!(!windows.contains("xres="), "{windows}");
    }

    #[test]
    fn the_desktop_reaches_the_guest_through_fw_cfg_and_only_when_one_was_asked_for() {
        // No flag means the image's own default, so a guest booted by anything
        // that does not know about this behaves as it always did.
        assert!(
            !joined(Image::Linux).contains("fw_cfg"),
            "{}",
            joined(Image::Linux)
        );

        let mut with_desktop = launch(Image::Linux);
        with_desktop.login = Login::new(Desktop::Gnome, SessionType::X11).ok();
        let text = with_desktop.args().join(" ");
        assert!(
            text.contains("-fw_cfg name=opt/sunlit/desktop,string=gnome-xorg"),
            "{text}"
        );
        with_desktop.login = Login::new(Desktop::Gnome, SessionType::Wayland).ok();
        let text = with_desktop.args().join(" ");
        assert!(
            text.contains("-fw_cfg name=opt/sunlit/desktop,string=gnome-wayland"),
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
    use crate::provider::target::{Image, Target};
    use crate::store::template_dir;

    /// The Packer template of one image. A layer has none: it is provisioned
    /// over its parent rather than installed, and `build_layer` is its builder.
    fn template(image: Image) -> String {
        let dir = template_dir(image);
        let name = match image {
            Image::Windows => "windows11.pkr.hcl",
            Image::Linux => "debian-13.pkr.hcl",
            Image::LinuxBuilder => "ubuntu-2204.pkr.hcl",
            Image::WindowsBuilder => unreachable!("a layer has no Packer template"),
        };
        std::fs::read_to_string(dir.join(name))
            .unwrap_or_else(|e| panic!("cannot read the {image} template: {e}"))
    }

    /// Every image Packer builds, which is every one but the layer.
    fn packer_images() -> impl Iterator<Item = Image> {
        Image::ALL.into_iter().filter(|image| !image.is_layer())
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
        let cores: u32 = variable_default(&template(Image::Windows), "cpus")
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
            template(Image::Windows).contains(r#"["-cpu", "max"]"#),
            "the Windows template leaves the guest CPU at QEMU's default"
        );
    }

    #[test]
    fn the_boot_key_is_pressed_over_qmp_rather_than_typed_by_packer() {
        // Packer logs every keystroke it sends over VNC and the guest receives
        // none of them on this host, so the template asks Packer to type
        // nothing and opens a monitor for the xtask to press the key on. The
        // port has to be the one the xtask presses.
        let text = template(Image::Windows);
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
        let text = template(Image::Windows);
        assert!(text.contains(r#"["-boot", "order=d"]"#), "{text}");
    }

    #[test]
    fn the_runtime_devices_match_the_templates() {
        for image in packer_images() {
            let text = template(image);

            // Packer spells the network device the way QEMU's -device does.
            assert_eq!(
                setting(&text, "net_device"),
                nic_device_for(image.target()),
                "{image}: the image is installed with a different NIC than it boots with"
            );

            // The disk is spelled as an interface in Packer and as a device at
            // runtime, so the pairing is stated rather than compared.
            let expected_interface = match disk_device_for(image.target()) {
                "ide-hd" => "ide",
                "virtio-blk-pci" => "virtio",
                other => panic!("unmapped disk device {other}"),
            };
            assert_eq!(
                setting(&text, "disk_interface"),
                expected_interface,
                "{image}: the image is installed on a different disk controller than it boots from"
            );

            // Both machines are q35, which is what makes an ide-hd device land
            // on a SATA controller rather than a legacy IDE one.
            assert_eq!(setting(&text, "machine_type"), "q35", "{image}");

            // The display is runtime-only: Packer never sees it, so this only
            // checks the device is one QEMU knows, in the form it takes it.
            let display = display_args(image.target(), (1920, 1080), 1).join(" ");
            assert!(
                ["-vga std", "-device virtio-vga,"]
                    .iter()
                    .any(|known| display.starts_with(known)),
                "{image}: unknown display arguments {display}"
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
