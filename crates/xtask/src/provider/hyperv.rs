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

/// Overrides the guest console's resolution, as `WxH`.
pub const RESOLUTION_ENV: &str = "SUNLIT_EARTH_VM_RESOLUTION";

/// The modes the automatic choice picks from, smallest first.
///
/// Every one of them is in the list the guest's synthetic adapter advertises
/// (`CIM_VideoControllerResolution`, read inside a running guest on
/// 2026-08-21), and the largest is where that list ends. `Set-VMVideo` accepts
/// more than that and the guest honours it: 2560x1440 was set and the guest
/// came up in it. Going past what the adapter offers is a thing to ask for
/// rather than to be given, so it is reachable through [`RESOLUTION_ENV`] and
/// not from here.
pub const CONSOLE_MODES: [(u32, u32); 6] = [
    (1024, 768),
    (1280, 800),
    (1440, 900),
    (1600, 900),
    (1680, 1050),
    (1920, 1080),
];

/// Room to leave between the guest's framebuffer and the edges of the host's
/// screen: the window's borders across, and its title bar plus `vmconnect`'s
/// own toolbar and status bar down. A guest larger than the screen it is shown
/// on is the same problem as one too small, arrived at from the other side.
///
/// The work area this is subtracted from already excludes the host's taskbar.
const WINDOW_MARGIN: (u32, u32) = (32, 120);

/// What a resolution may be, at both ends.
///
/// The floor is the smallest mode in [`CONSOLE_MODES`] and the ceiling is
/// generous: the point is to catch a transposed or mistyped value, not to have
/// an opinion about a host with a very large screen.
const RESOLUTION_BOUNDS: (u32, u32) = (640, 7680);

/// Parse a `WxH` resolution, rejecting anything that is not one.
pub fn parse_resolution(value: &str) -> Option<(u32, u32)> {
    let (width, height) = value.trim().split_once(['x', 'X'])?;
    let width: u32 = width.trim().parse().ok()?;
    let height: u32 = height.trim().parse().ok()?;
    let (min, max) = RESOLUTION_BOUNDS;
    let plausible = (min..=max).contains(&width) && (min..=max).contains(&height);
    plausible.then_some((width, height))
}

/// The resolution to give a guest's console, and why.
///
/// A Hyper-V guest is seen through a basic `vmconnect` session, which shows the
/// framebuffer as it is: the window is the resolution, and there is no dragging
/// it larger. That makes the resolution the only lever, and 1024x768, which is
/// what a guest picks when nothing tells it otherwise, is a small window on any
/// screen bought in the last decade.
///
/// So the largest mode that fits the host's own screen, which is what makes it
/// right on a laptop and on a 3440x1440 desktop without either being
/// configured. An explicit request wins outright, including one larger than
/// anything here. A host whose screen size could not be read keeps today's
/// behaviour rather than guessing a size that might not fit, and the caller
/// says which of the three happened.
pub fn console_resolution(
    requested: Option<(u32, u32)>,
    host_work_area: Option<(u32, u32)>,
) -> (u32, u32) {
    if let Some(size) = requested {
        return size;
    }
    let smallest = CONSOLE_MODES[0];
    let Some((area_width, area_height)) = host_work_area else {
        return smallest;
    };
    let (margin_width, margin_height) = WINDOW_MARGIN;
    let width_budget = area_width.saturating_sub(margin_width);
    let height_budget = area_height.saturating_sub(margin_height);
    CONSOLE_MODES
        .into_iter()
        .rfind(|&(width, height)| width <= width_budget && height <= height_budget)
        .unwrap_or(smallest)
}

/// The marker the work-area query prints, and the script that prints it.
///
/// `Screen.PrimaryScreen.WorkingArea` rather than the whole virtual desktop or
/// the monitor's raw mode: it is the one that already excludes the taskbar, and
/// on a multi-monitor host the console window opens on one screen rather than
/// across all of them.
pub const WORK_AREA_MARK: &str = "WORKAREA=";

pub fn work_area_script() -> String {
    format!(
        "Add-Type -AssemblyName System.Windows.Forms\n\
         $screen = [System.Windows.Forms.Screen]::PrimaryScreen\n\
         if ($screen) {{\n  \
         $area = $screen.WorkingArea\n  \
         Write-Output ('{WORK_AREA_MARK}{{0}}x{{1}}' -f $area.Width, $area.Height)\n\
         }}\n"
    )
}

/// The work area out of that script's output, if it printed one.
pub fn parse_work_area(stdout: &str) -> Option<(u32, u32)> {
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix(WORK_AREA_MARK))
        .and_then(parse_resolution)
}

/// The script that creates the VM.
///
/// A differencing child of the read-only golden VHDX, exactly as the QEMU
/// provider makes a qcow2 overlay, so every run starts pristine and nothing
/// ever writes to the image (retrospective 8.3).
///
/// Generation 2 because Windows 11 needs UEFI. Secure Boot is off: the image
/// was installed with the requirement bypassed, and turning it on here would
/// be asserting something about the disk that was never true.
pub fn create_script(name: &str, golden: &str, overlay: &str, console: (u32, u32)) -> String {
    format!(
        "New-VHD -Path {overlay} -ParentPath {golden} -Differencing | Out-Null\n\
         New-VM -Name {name} -Generation 2 -MemoryStartupBytes {memory} \
         -VHDPath {overlay} -SwitchName {switch} | Out-Null\n\
         Set-VM -Name {name} -ProcessorCount {cpus} -AutomaticCheckpointsEnabled $false \
         -AutomaticStartAction Nothing -AutomaticStopAction TurnOff\n\
         Set-VMFirmware -VMName {name} -EnableSecureBoot Off\n\
         $drive = Get-VMHardDiskDrive -VMName {name}\n\
         Set-VMFirmware -VMName {name} -FirstBootDevice $drive\n\
         Set-VMMemory -VMName {name} -DynamicMemoryEnabled $false\n\
         {video}",
        name = ps_quote(name),
        golden = ps_quote(golden),
        overlay = ps_quote(overlay),
        switch = ps_quote(SWITCH),
        memory = MEMORY_BYTES,
        cpus = CPUS,
        video = video_script(name, console),
    )
}

/// What to say about the session `vmconnect` is about to open, which depends on
/// what the guest was started for.
///
/// A guest handed over to a person has been given a passwordless account and a
/// running Remote Desktop service, so the enhanced session vmconnect prefers is
/// there for the taking, and it is the only session that can be resized. A
/// guest with a run in it has neither, so vmconnect opens the basic session that
/// is safe to watch and asks for nothing. The two cases have opposite advice,
/// and printing both would leave the reader to work out which applies.
pub fn view_note(reason: StartReason) -> String {
    match reason {
        StartReason::Up | StartReason::Keep => "It will offer an enhanced session, which is the \
             one that can be resized: the guest's desktop follows the window. \
             The dialog wants the guest's account, `tester`, and no password at \
             all, so leave that field empty and connect. A basic session needs \
             nothing typed but is fixed at the console resolution.\n\
             Enhanced is RDP, and RDP takes the console session over. That is \
             harmless here, because nothing of ours is running in this guest."
            .to_owned(),
        StartReason::Run | StartReason::Build => "This guest has a job running in it, so it \
             offers no enhanced session and asks for nothing: what opens is a \
             basic session showing the console desktop as it is. That is \
             deliberate. An enhanced session is RDP, and connecting would take \
             the console session out from under the job, which is where its \
             windows are.\n\
             Watching is harmless; clicking during a run perturbs it."
            .to_owned(),
    }
}

/// The line that fixes the guest's console resolution.
///
/// Here rather than anywhere later because the cmdlet refuses to run against a
/// VM that is on: "the virtual machine must be turned off to set the resolution
/// type or the horizontal or vertical resolution". Between `New-VM` and
/// `Start-VM` is the only window either create script has, and it is enough:
/// the mode is in place before the firmware draws anything, so the console is
/// the right size from the first frame rather than after a logon.
///
/// `Single` and not `Maximum`: `Maximum` advertises a list up to the size given
/// and leaves the guest to pick, which it does exactly as it does today, at
/// 1024x768. `Single` advertises one mode, so the guest has nothing else to
/// choose. What that costs is changing the resolution from inside the guest,
/// which is a trade worth making: the resolution now comes from the host, where
/// the screen it has to fit on is.
pub fn video_script(name: &str, (width, height): (u32, u32)) -> String {
    format!(
        "Set-VMVideo -VMName {name} -ResolutionType Single \
         -HorizontalResolution {width} -VerticalResolution {height}\n",
        name = ps_quote(name),
    )
}

/// The line a query script prints to prove it ran.
///
/// Marker rather than exit code, and not for elegance. `Get-VM -Name X` on a
/// host with no such VM writes an error record, and `-ErrorAction
/// SilentlyContinue` hides the record without undoing what it did to the exit
/// code: `powershell.exe` still exits 1. Every caller then reads "Hyper-V
/// refused" where the truth is "there is no such VM", which is the one
/// distinction a teardown may not get wrong, and it is what ended the first
/// live Windows image build one step from success.
///
/// So no query asks for a VM by name any more. Listing every VM and filtering
/// in the script is a question that has an answer either way, and this marker
/// says the listing itself worked: a host where the cmdlets fail, for a stopped
/// service or a missing group membership, throws before printing it. The crate
/// already learned this lesson once, from winget (deviation 19): what a script
/// knows, a script has to say, because `powershell -EncodedCommand` will not
/// carry it in the exit code.
pub const QUERY_OK: &str = "QUERY=ok";

/// A query script: find our VM without its absence reading as a failure, report
/// on it, and prove the query ran.
pub fn query_script(name: &str, body: &str) -> String {
    format!(
        "$vm = @(Get-VM) | Where-Object {{ $_.Name -eq {name} }} | Select-Object -First 1\n\
         {body}\
         Write-Output '{QUERY_OK}'\n",
        name = ps_quote(name)
    )
}

/// Whether a query script's own answer arrived.
pub fn answered(stdout: &str) -> bool {
    stdout.lines().any(|line| line.trim() == QUERY_OK)
}

/// The script that reports a VM's state, or nothing if it does not exist.
pub fn state_script(name: &str) -> String {
    query_script(name, "if ($vm) { Write-Output \"STATE=$($vm.State)\" }\n")
}

/// The script that reports the guest's addresses.
///
/// `Get-VMNetworkAdapter` reads them out of the integration services, which
/// every Windows guest has built in, so nothing has to be installed in the
/// guest to make this work. Handed the VM object rather than its name, so a
/// guest that has gone away is an empty answer rather than an error.
pub fn address_script(name: &str) -> String {
    query_script(
        name,
        "if ($vm) {\n  \
         $vm | Get-VMNetworkAdapter | ForEach-Object { $_.IPAddresses } | \
         ForEach-Object { Write-Output \"IP=$_\" }\n\
         }\n",
    )
}

/// The script that tears the VM down.
///
/// `-TurnOff` rather than a graceful shutdown: the guest holds nothing worth
/// flushing, its disk is a differencing child about to be deleted, and waiting
/// for Windows to shut down politely costs a minute per run.
///
/// The two cmdlets that change anything name the VM explicitly, even though the
/// object is already in hand, because a mutating cmdlet reading its subject from
/// a pipeline is one refactor away from acting on everything the pipeline holds.
pub fn destroy_script(name: &str) -> String {
    query_script(
        name,
        &format!(
            "if ($vm) {{\n  \
             if ($vm.State -ne 'Off') {{ Stop-VM -Name {name} -TurnOff -Force }}\n  \
             Remove-VM -Name {name} -Force\n\
             }}\n",
            name = ps_quote(name)
        ),
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
    /// different answers to "may I delete this disk now". Which is why the
    /// answer has to be the script's own word for it rather than its exit code;
    /// see [`QUERY_OK`].
    fn query_state(&self, name: &str) -> Result<Option<String>, String> {
        let out = self.run_script(&state_script(name))?;
        if !answered(&out) {
            return Err(format!(
                "the query about {name} did not run to the end, so whether it \
                 exists is unknown. Its output was: {}",
                out.trim()
            ));
        }
        Ok(parse_state(&out))
    }

    /// The console resolution for a guest about to be created, said out loud.
    ///
    /// Printed because all three answers are ones somebody would want to know
    /// about: a size asked for, a size derived from this screen, and the small
    /// default that means the screen could not be read. Without the line, the
    /// last of those is indistinguishable from nothing having changed.
    pub fn console_size(&self) -> (u32, u32) {
        let requested = crate::util::env_var(RESOLUTION_ENV).and_then(|raw| {
            let parsed = parse_resolution(&raw);
            if parsed.is_none() {
                println!(
                    "warning: {RESOLUTION_ENV} is {raw:?}, which is not a size like \
                     1920x1080; choosing one for this host instead"
                );
            }
            parsed
        });
        let area = self
            .runner
            .capture(&powershell(&work_area_script()))
            .ok()
            .filter(crate::runner::CommandOutput::success)
            .and_then(|out| parse_work_area(&out.stdout));
        let (width, height) = console_resolution(requested, area);
        match (requested, area) {
            (Some(_), _) => println!("console: {width}x{height}, from {RESOLUTION_ENV}"),
            (None, Some((aw, ah))) => {
                println!("console: {width}x{height}, the largest that fits this host's {aw}x{ah}");
            }
            (None, None) => println!(
                "console: {width}x{height}; this host's screen size could not be read, \
                 and {RESOLUTION_ENV} sets it explicitly"
            ),
        }
        (width, height)
    }

    /// Poll until the guest reports a usable address.
    fn wait_for_address(&self, name: &str, timeout: Duration) -> Result<String, String> {
        let start = Instant::now();
        loop {
            if let Ok(out) = self.run_script(&address_script(name))
                && answered(&out)
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
            self.console_size(),
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
    /// distinguishable from "there is no such VM". The callers are `vm ssh`,
    /// `vm view`, `vm status`, the one-VM-at-a-time check, and the answer
    /// `clear_stale_state` hands `vm::may_clear`. The first four either report
    /// it or refuse. The fifth decides whether a recorded image build is
    /// running, and so whether clearing it away is allowed at all. A query that
    /// failed answers no there, which would allow a live build to be cleared;
    /// what keeps that from costing the install is that the clearing itself goes
    /// through `destroy`, which asks `query_state` and refuses to delete
    /// anything it cannot account for. So even there a false negative costs a
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
            "vmconnect is opening {name}.\n{}",
            view_note(state.reason),
            name = state.vm_name,
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

    #[test]
    fn the_vm_is_a_differencing_child_of_the_golden_image() {
        let script = create_script(
            "sunlit-e2e-windows",
            r"C:\vm\images\windows\golden.vhdx",
            r"C:\vm\run\windows\overlay.vhdx",
            (1920, 1080),
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
        let script = create_script("sunlit-e2e-windows", "g.vhdx", "o.vhdx", (1920, 1080));
        assert!(script.contains("-Generation 2"), "{script}");
        assert!(script.contains("-EnableSecureBoot Off"), "{script}");
        assert!(script.contains("FirstBootDevice"), "{script}");
    }

    #[test]
    fn the_vm_uses_the_switch_every_client_windows_has() {
        let script = create_script("sunlit-e2e-windows", "g.vhdx", "o.vhdx", (1920, 1080));
        assert!(script.contains("-SwitchName 'Default Switch'"), "{script}");
        assert!(!script.contains("New-VMSwitch"), "{script}");
    }

    #[test]
    fn the_vm_never_starts_itself_or_takes_checkpoints() {
        let script = create_script("sunlit-e2e-windows", "g.vhdx", "o.vhdx", (1920, 1080));
        assert!(
            script.contains("-AutomaticCheckpointsEnabled $false"),
            "{script}"
        );
        assert!(script.contains("-AutomaticStartAction Nothing"), "{script}");
    }

    /// The two kinds of guest get opposite advice, and each has to get its own.
    #[test]
    fn what_the_console_offers_depends_on_what_the_guest_is_for() {
        for handed_over in [StartReason::Up, StartReason::Keep] {
            let note = view_note(handed_over);
            assert!(note.contains("resized"), "{note}");
            assert!(note.contains("tester"), "{note}");
            assert!(note.contains("no password"), "{note}");
        }
        for running in [StartReason::Run, StartReason::Build] {
            let note = view_note(running);
            assert!(note.contains("no enhanced session"), "{note}");
            assert!(note.contains("asks for nothing"), "{note}");
            // Not a word about leaving the password blank: there is none to
            // leave blank in a guest that was never handed over.
            assert!(!note.contains("leave that field empty"), "{note}");
        }
    }

    /// The console has to be sized before the VM starts, because the cmdlet
    /// refuses to run against one that is on.
    #[test]
    fn the_console_is_sized_in_the_create_script() {
        let script = create_script("sunlit-e2e-windows", "g.vhdx", "o.vhdx", (1920, 1080));
        assert!(
            script.contains(
                "Set-VMVideo -VMName 'sunlit-e2e-windows' -ResolutionType Single \
                 -HorizontalResolution 1920 -VerticalResolution 1080"
            ),
            "{script}"
        );
        let video = script.find("Set-VMVideo").expect("the video line is there");
        let start = script.find("New-VM").expect("the VM is created");
        assert!(video > start, "{script}");
        // `Maximum` leaves the guest to pick, and what it picks is 1024x768.
        assert!(!script.contains("-ResolutionType Maximum"), "{script}");
    }

    /// The largest mode that fits, and no larger than the screen it will be
    /// shown on.
    #[test]
    fn the_console_grows_to_the_host_screen_and_no_further() {
        // The 3440x1440 ultrawide this was written on, where the largest mode
        // fits with room to spare.
        assert_eq!(console_resolution(None, Some((3440, 1400))), (1920, 1080));
        // A 1080p screen, where it does not: 1080 plus the window's own
        // furniture is taller than the screen, and 1600x900 is the next one
        // down that fits.
        assert_eq!(console_resolution(None, Some((1920, 1040))), (1600, 900));
        // A 1366x768 panel has room for none of the larger modes, so it keeps
        // what a guest would have picked for itself.
        assert_eq!(console_resolution(None, Some((1366, 728))), (1024, 768));
        // A screen size nobody could read is not a licence to guess.
        assert_eq!(console_resolution(None, None), CONSOLE_MODES[0]);
        // An explicit request wins, including one bigger than any mode here.
        assert_eq!(
            console_resolution(Some((2560, 1440)), Some((1366, 728))),
            (2560, 1440)
        );
    }

    #[test]
    fn a_resolution_is_two_numbers_and_anything_else_is_not_one() {
        assert_eq!(parse_resolution("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_resolution(" 2560 X 1440 "), Some((2560, 1440)));
        for bad in [
            "",
            "1920",
            "1920x",
            "x1080",
            "1920*1080",
            "1920x1080x60",
            "huge",
            // Out of bounds at both ends: a typo rather than a screen.
            "320x240",
            "99999x1080",
        ] {
            assert_eq!(parse_resolution(bad), None, "{bad}");
        }
    }

    /// The query and its parser are two halves of one thing, and the half that
    /// can be checked here is that the parser reads what the script prints.
    #[test]
    fn the_work_area_query_and_its_parser_agree() {
        let script = work_area_script();
        assert!(script.contains(WORK_AREA_MARK), "{script}");
        assert_eq!(
            parse_work_area(&format!("noise\n{WORK_AREA_MARK}3440x1400\n")),
            Some((3440, 1400))
        );
        // A host with no screen prints no marker, which is not a failure.
        assert_eq!(parse_work_area("nothing here"), None);
        assert_eq!(parse_work_area(&format!("{WORK_AREA_MARK}wide")), None);
    }

    #[test]
    fn every_script_names_a_vm_of_ours() {
        let name = Target::Windows.vm_name();
        for script in [
            create_script(&name, "g", "o", (1920, 1080)),
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
        assert!(
            script.contains("$_.Name -eq 'sunlit-e2e-windows'"),
            "{script}"
        );
        assert!(script.contains("if ($vm)"), "{script}");
        assert!(script.contains("-TurnOff -Force"), "{script}");
        // Never a blanket removal: both cmdlets that change anything name the
        // VM, so no pipeline can widen what they act on.
        assert!(
            script.contains("Stop-VM -Name 'sunlit-e2e-windows'"),
            "{script}"
        );
        assert!(
            script.contains("Remove-VM -Name 'sunlit-e2e-windows'"),
            "{script}"
        );
    }

    /// `Get-VM -Name X` for a VM that does not exist writes an error record,
    /// and `-ErrorAction SilentlyContinue` hides the record while leaving
    /// `powershell.exe` exiting 1. Measured on 2026-08-20: it is what ended the
    /// first live Windows image build one step from success, after the guest had
    /// installed, finalized, and shut down. So no query names a VM to the
    /// cmdlet any more, and every one of them says whether it ran.
    #[test]
    fn no_query_asks_for_a_vm_by_name_or_hides_an_error_behind_an_exit_code() {
        for script in [
            state_script("sunlit-e2e-windows"),
            address_script("sunlit-e2e-windows"),
            destroy_script("sunlit-e2e-windows"),
        ] {
            assert!(
                !script.contains("Get-VM -Name"),
                "a by-name lookup makes a missing VM an error: {script}"
            );
            assert!(
                !script.contains("-ErrorAction SilentlyContinue"),
                "a suppressed error still decides the exit code: {script}"
            );
            assert!(
                script.contains(QUERY_OK),
                "a query has to say it ran: {script}"
            );
        }
        // And a by-name lookup of the network adapter would do the same, so the
        // VM object is handed over instead.
        assert!(!address_script("vm").contains("Get-VMNetworkAdapter -VMName"));
    }

    #[test]
    fn a_query_that_did_not_run_is_told_from_one_that_found_nothing() {
        // Both come back with no STATE line. Only the marker separates "there
        // is no such VM" from "the cmdlets never got that far", and a teardown
        // may not confuse the two: one means the disk is safe to delete and the
        // other means nothing is known about the guest holding it.
        assert!(answered("QUERY=ok\n"));
        assert!(answered("STATE=Running\nQUERY=ok"));
        assert!(!answered(""));
        assert!(!answered("STATE=Running\n"));
        assert!(!answered("Get-VM : access denied\n"));
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
