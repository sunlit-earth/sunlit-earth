//! The `Hyper-V` provider: `PowerShell` cmdlets and a differencing VHDX.
//!
//! Used for the Windows guest on a Windows host, which is the configuration the
//! product ships on. Everything here goes through `powershell.exe`; there is no
//! management API worth linking for eight commands.
//!
//! Unlike QEMU there is no process of ours to watch and no port to forward: the
//! hypervisor owns the VM, the guest gets an address from the Default Switch,
//! and both liveness and reachability are questions to ask the hypervisor.

use std::path::{Path, PathBuf};
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
/// whether this guest was handed over.
///
/// A guest handed over to a person has been given a passwordless account and a
/// running Remote Desktop service, so the enhanced session vmconnect prefers is
/// there for the taking, and it is the only session that can be resized. Any
/// other guest has neither, so vmconnect opens the basic session that is safe to
/// watch and asks for nothing. The two cases have opposite advice, and printing
/// both would leave the reader to work out which applies.
///
/// Keyed on the hand-over rather than on the reason the guest was started for,
/// because the reason is decided before the boot and the hand-over is a thing
/// the guest confirmed afterwards. The advice below is about what a dialog will
/// ask for, and a hand-over that did not happen or did not work leaves nothing
/// to type.
pub fn view_note(handed_over: bool) -> String {
    if handed_over {
        "It will offer an enhanced session, which is the one that can be \
         resized: the guest's desktop follows the window. The dialog wants the \
         guest's account, `tester`, and no password at all, so leave that field \
         empty and connect. A basic session needs nothing typed but is fixed at \
         the console resolution.\n\
         Enhanced is RDP, and RDP takes the console session over. That is \
         harmless here, because nothing of ours is running in this guest."
            .to_owned()
    } else {
        // The second paragraph is the stale-image case, and it is not
        // hypothetical: the image ships with Remote Desktop Services disabled,
        // but staleness only warns at boot, so a guest built before that change
        // still offers the enhanced session this text says it does not.
        "This guest was not handed over, so it offers no enhanced session and \
         asks for nothing: what opens is a basic session showing the console \
         desktop as it is. That is deliberate for a guest with a run in it. An \
         enhanced session is RDP, and connecting would take the console session \
         out from under the job, which is where its windows are.\n\
         If a credential dialog appears anyway, this guest's golden image \
         predates the one that disables Remote Desktop Services: cancel it \
         rather than signing in, for that same reason, and \
         `cargo xtask vm build-image windows` brings the image up to date.\n\
         Watching is harmless; clicking during a run perturbs it."
            .to_owned()
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

/// The script that reports the VM's own identifier.
///
/// Which is what `vmconnect` files its per-VM settings under, and every `vm up`
/// creates a VM with a new one.
pub fn id_script(name: &str) -> String {
    query_script(name, "if ($vm) { Write-Output \"ID=$($vm.Id)\" }\n")
}

/// Read the `ID=` line out of that script's output.
pub fn parse_id(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("ID="))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// The host `vmconnect` is pointed at, in both places that name one.
///
/// It is the argument `vmconnect` is started with and the `VmServerName` in the
/// settings it reads, from here so that the two agree. They did not to begin
/// with: the file said `%COMPUTERNAME%`, which is what the dialog's own checkbox
/// writes, while the process was started against `localhost`. The dialog was
/// suppressed anyway, so `vmconnect` keys the settings on the file name, but a
/// version that ever compared or used the name in the file would have connected
/// somewhere the caller never asked for. Both spellings reach this same machine,
/// and this one is the connection actually being made.
pub const VMCONNECT_SERVER: &str = "localhost";

/// The directory `vmconnect` keeps its per-VM settings in.
///
/// In the roaming profile beside `vmconnect.config`. Found by watching what the
/// connection dialog's "save my settings for future connections to this virtual
/// machine" checkbox wrote.
pub fn vmconnect_settings_dir(app_data: &Path) -> PathBuf {
    app_data
        .join("Microsoft")
        .join("Windows")
        .join("Hyper-V")
        .join("Client")
        .join("1.0")
}

/// The prefix and suffix of one of those settings files.
const SETTINGS_PREFIX: &str = "vmconnect.rdp.";
const SETTINGS_SUFFIX: &str = ".config";

/// Where `vmconnect` keeps its settings for one VM.
///
/// A .NET settings document per VM identifier. The name is upper case as the
/// checkbox writes it, and the identifier in it is the VM's own, so a guest
/// recreated under the same name gets a new one and is asked again.
pub fn vmconnect_settings_path(app_data: &Path, vm_id: &str) -> PathBuf {
    vmconnect_settings_dir(app_data).join(format!(
        "{SETTINGS_PREFIX}{}{SETTINGS_SUFFIX}",
        vm_id.to_uppercase()
    ))
}

/// Whether a file name in that directory is a per-VM settings document at all.
///
/// The name filter comes first because it decides what is worth opening:
/// everything else in there belongs to the user rather than to us.
pub fn looks_like_settings(file_name: &str) -> bool {
    file_name.starts_with(SETTINGS_PREFIX) && file_name.ends_with(SETTINGS_SUFFIX)
}

/// Whether one such file is a settings document of ours, naming this VM.
///
/// Both halves are needed and neither is enough. The name says it is a per-VM
/// settings document rather than `vmconnect.config` or anything else somebody
/// keeps in their roaming profile; the VM name inside says the document is one
/// of ours, since the identifier in the file name is a number nothing on the
/// host can be matched against once the VM is gone.
pub fn is_our_settings(file_name: &str, text: &str, vm_name: &str) -> bool {
    looks_like_settings(file_name) && text.contains(&format!("<value>{vm_name}</value>"))
}

/// Delete the settings files that name this VM, except one to keep.
///
/// One file per boot, two kilobytes each, in a directory that is the user's
/// rather than ours: small, but litter of a kind nothing else would ever clean
/// up, since the VM it was named after no longer exists. Called from two places
/// for the two ways one becomes stale: before a console opens, where the file
/// about to be used is the one to keep, and when the VM is destroyed, where
/// there is nothing left to keep it for.
///
/// Best effort throughout. A file that cannot be read is not one to delete, and
/// a directory that cannot be listed holds nothing this is willing to guess at.
pub fn forget_settings(app_data: &Path, vm_name: &str, keep: Option<&Path>) {
    let dir = vmconnect_settings_dir(app_data);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    // Kept by name rather than by comparing paths: everything here is in the
    // one directory, and the file system these names came off is
    // case-insensitive while `PathBuf` is not. A write to a name that differs
    // only in case lands in the existing file, which would then not match the
    // path that wrote it and would be swept as an earlier guest's.
    let keep = keep
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !looks_like_settings(name) || keep.is_some_and(|keep| keep.eq_ignore_ascii_case(name)) {
            continue;
        }
        // The file says which VM it belongs to, and that is the only thing that
        // makes it ours to delete.
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if is_our_settings(name, &text, vm_name) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Those settings, with the dialog already answered.
///
/// `SavedConfigExists` and `SaveButtonChecked` are the two that matter: with
/// them the dialog does not open, and `vmconnect` goes straight to the session.
/// Everything else is what the checkbox itself saves, kept as it writes it,
/// except the desktop size, which is the same size the basic session's console
/// gets. It is only the starting size in an enhanced session, since dragging the
/// window changes it, and starting at the size that fits this screen is better
/// than starting at whatever was last on the dialog's slider.
///
/// A whole file rather than an edit: it is written before every connection, for
/// an identifier that did not exist until this guest booted, so there is never
/// an existing one of ours to preserve.
pub fn vmconnect_settings(vm_name: &str, (width, height): (u32, u32)) -> String {
    let setting = |name: &str, kind: &str, value: &str| {
        format!(
            "        <setting name=\"{name}\" type=\"{kind}\">\n            \
             <value>{value}</value>\n        </setting>\n"
        )
    };
    let bool_setting = |name: &str, value: bool| {
        setting(name, "System.Boolean", if value { "True" } else { "False" })
    };
    let string_setting = |name: &str, value: &str| setting(name, "System.String", value);
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <configuration>\n    \
         <Microsoft.Virtualization.Client.RdpOptions>\n\
         {saved}{save_button}{size}{name}{host}\
         {clipboard}{printer}{enable_printer}{smartcards}{webauthn}\
         {audio_capture}{audio_playback}{full_screen}{all_monitors}\
         {drives}{usb}{pnp}    \
         </Microsoft.Virtualization.Client.RdpOptions>\n\
         </configuration>",
        saved = bool_setting("SavedConfigExists", true),
        save_button = bool_setting("SaveButtonChecked", true),
        size = setting(
            "DesktopSize",
            "System.Drawing.Size",
            &format!("{width}, {height}")
        ),
        name = string_setting("VmName", vm_name),
        host = string_setting("VmServerName", VMCONNECT_SERVER),
        clipboard = bool_setting("ClipboardRedirection", true),
        printer = bool_setting("PrinterRedirection", true),
        enable_printer = bool_setting("EnablePrinterRedirection", true),
        smartcards = bool_setting("SmartCardsRedirection", true),
        webauthn = bool_setting("WebAuthnRedirection", true),
        audio_capture = bool_setting("AudioCaptureRedirectionMode", false),
        audio_playback = setting(
            "AudioPlaybackRedirectionMode",
            "Microsoft.Virtualization.Client.RdpOptions+AudioPlaybackRedirectionType",
            "AUDIO_MODE_REDIRECT"
        ),
        full_screen = bool_setting("FullScreen", false),
        all_monitors = bool_setting("UseAllMonitors", false),
        // Empty, as the dialog leaves them: nothing of the host's is shared
        // with a guest that exists to be thrown away.
        drives = string_setting("RedirectedDrives", ""),
        usb = string_setting("RedirectedUsbDevices", ""),
        pnp = string_setting("RedirectedPnpDevices", ""),
    )
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

    /// Write `vmconnect`'s settings for this guest, so its connection dialog
    /// does not open.
    ///
    /// The dialog asks which resolution to use and whether to share local
    /// devices. Neither question has an interesting answer for a guest that will
    /// be discarded, and the checkbox that remembers them is filed under the
    /// VM's identifier, which is new every time one boots: answering it by hand
    /// lasts exactly until the next `vm up`. So the file is written before
    /// `vmconnect` starts.
    ///
    /// Best effort throughout. Every failure in here costs one dialog, so a
    /// missing `%APPDATA%`, a VM that cannot be found, or an unwritable
    /// directory are all reasons to carry on quietly rather than to refuse to
    /// open a console.
    fn answer_connection_dialog(&self, state: &RunState) {
        let Some(app_data) = std::env::var_os("APPDATA").map(PathBuf::from) else {
            return;
        };
        let Ok(out) = self.run_script(&id_script(&state.vm_name)) else {
            return;
        };
        let Some(id) = parse_id(&out) else {
            return;
        };
        let path = vmconnect_settings_path(&app_data, &id);
        if let Some(dir) = path.parent()
            && std::fs::create_dir_all(dir).is_err()
        {
            return;
        }
        let settings = vmconnect_settings(&state.vm_name, self.console_size_quietly());
        let _ = std::fs::write(&path, settings);
        forget_settings(&app_data, &state.vm_name, Some(&path));
    }

    /// The console size, without repeating the line that explains it.
    ///
    /// `view` runs long after the guest was created, so the choice has already
    /// been made and printed once; saying it again would suggest something had
    /// changed.
    fn console_size_quietly(&self) -> (u32, u32) {
        let requested = crate::util::env_var(RESOLUTION_ENV).and_then(|raw| parse_resolution(&raw));
        let area = self
            .runner
            .capture(&powershell(&work_area_script()))
            .ok()
            .filter(crate::runner::CommandOutput::success)
            .and_then(|out| parse_work_area(&out.stdout));
        console_resolution(requested, area)
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

        // The console settings go with the VM, whether or not there was one left
        // to stop. They are filed under an identifier that means nothing once
        // the VM is gone, so this is the last moment anything can recognize
        // them; the next boot's guest has a new identifier and writes its own.
        if let Some(app_data) = std::env::var_os("APPDATA").map(PathBuf::from) {
            forget_settings(&app_data, &state.vm_name, None);
        }

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
        // Only where an enhanced session is on offer, because the dialog this
        // answers is the enhanced one: a guest that was not handed over has no
        // such session and is never asked.
        if state.handed_over {
            self.answer_connection_dialog(state);
        }
        self.runner
            .spawn(
                &Cmd::new(viewer.to_string_lossy())
                    .args([VMCONNECT_SERVER.to_owned(), state.vm_name.clone()]),
                None,
            )
            .map_err(|e| format!("cannot start vmconnect: {e}"))?;
        Ok(format!(
            "vmconnect is opening {name}.\n{}",
            view_note(state.handed_over),
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

    /// The two settings that close the connection dialog, and the shape the file
    /// has to have for `vmconnect` to read it at all.
    #[test]
    fn the_saved_connection_settings_answer_the_dialog() {
        let text = vmconnect_settings("sunlit-e2e-windows", (1920, 1080));
        assert!(text.starts_with("<?xml version="), "{text}");
        assert!(
            text.contains("<Microsoft.Virtualization.Client.RdpOptions>"),
            "{text}"
        );
        for name in ["SavedConfigExists", "SaveButtonChecked"] {
            let setting = format!("<setting name=\"{name}\" type=\"System.Boolean\">");
            let at = text.find(&setting).expect("the setting is there");
            assert!(
                text[at..].starts_with(&format!("{setting}\n            <value>True</value>")),
                "{name} is what stops the dialog opening: {text}"
            );
        }
        assert!(text.contains("<value>1920, 1080</value>"), "{text}");
        assert!(text.contains("<value>sunlit-e2e-windows</value>"), "{text}");
        // The server named in the file is the one vmconnect is started against,
        // rather than a second spelling of the same machine.
        assert!(
            text.contains(&format!("<value>{VMCONNECT_SERVER}</value>")),
            "{text}"
        );
        // Nothing of the host's is handed to a guest that will be discarded.
        for shared in [
            "RedirectedDrives",
            "RedirectedUsbDevices",
            "RedirectedPnpDevices",
        ] {
            let setting = format!("<setting name=\"{shared}\" type=\"System.String\">");
            let at = text.find(&setting).expect("the setting is there");
            assert!(
                text[at..].starts_with(&format!("{setting}\n            <value></value>")),
                "{shared} should be empty: {text}"
            );
        }
    }

    /// The name is the VM's identifier in upper case, in the roaming profile:
    /// all three are what `vmconnect` itself looks for.
    #[test]
    fn the_settings_file_is_named_after_the_vm_identifier() {
        let path = vmconnect_settings_path(
            Path::new(r"C:\Users\me\AppData\Roaming"),
            "2333c8fa-26e0-41a5-9023-f95fffcc1c53",
        );
        assert_eq!(
            path,
            Path::new(
                r"C:\Users\me\AppData\Roaming\Microsoft\Windows\Hyper-V\Client\1.0\vmconnect.rdp.2333C8FA-26E0-41A5-9023-F95FFFCC1C53.config"
            )
        );
    }

    #[test]
    fn the_vm_identifier_comes_out_of_the_query() {
        assert!(
            id_script("sunlit-e2e-windows").contains("$vm.Id"),
            "the query asks for it"
        );
        assert_eq!(
            parse_id(&format!(
                "ID=2333c8fa-26e0-41a5-9023-f95fffcc1c53\n{QUERY_OK}\n"
            )),
            Some("2333c8fa-26e0-41a5-9023-f95fffcc1c53".to_owned())
        );
        // A guest that has gone away prints the marker and nothing else.
        assert_eq!(parse_id(&format!("{QUERY_OK}\n")), None);
        assert_eq!(parse_id("ID=\n"), None);
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
    fn what_the_console_offers_depends_on_whether_the_guest_was_handed_over() {
        let handed_over = view_note(true);
        assert!(handed_over.contains("resized"), "{handed_over}");
        assert!(handed_over.contains("tester"), "{handed_over}");
        assert!(handed_over.contains("no password"), "{handed_over}");

        let not = view_note(false);
        assert!(not.contains("no enhanced session"), "{not}");
        assert!(not.contains("asks for nothing"), "{not}");
        // Not a word about leaving the password blank: there is none to leave
        // blank in a guest that was never handed over.
        assert!(!not.contains("leave that field empty"), "{not}");
        // And the one case where the sentence above is wrong is named, because
        // a stale image still offers the session this says it does not.
        assert!(not.contains("cancel it"), "{not}");
        assert!(not.contains("build-image windows"), "{not}");
    }

    /// The predicate that decides what gets deleted out of the user's roaming
    /// profile, against the neighbours it has to leave alone.
    #[test]
    fn only_a_settings_document_naming_our_vm_is_ours_to_delete() {
        let ours = vmconnect_settings("sunlit-e2e-windows", (1920, 1080));
        assert!(is_our_settings(
            "vmconnect.rdp.2333C8FA-26E0-41A5-9023-F95FFFCC1C53.config",
            &ours,
            "sunlit-e2e-windows"
        ));
        // The same file, asked about the other target's VM.
        assert!(!is_our_settings(
            "vmconnect.rdp.2333C8FA-26E0-41A5-9023-F95FFFCC1C53.config",
            &ours,
            "sunlit-e2e-linux"
        ));
        // Somebody else's guest, with its own settings in the same directory.
        let theirs = vmconnect_settings("dev-box", (1024, 768));
        assert!(!is_our_settings(
            "vmconnect.rdp.9E1B0A77-0000-0000-0000-000000000001.config",
            &theirs,
            "sunlit-e2e-windows"
        ));
        // vmconnect's own settings, which are not per VM and not ours.
        assert!(!looks_like_settings("vmconnect.config"));
        assert!(!is_our_settings(
            "vmconnect.config",
            &ours,
            "sunlit-e2e-windows"
        ));
        // A truncated file that never got as far as naming a VM.
        assert!(!is_our_settings(
            "vmconnect.rdp.2333C8FA-26E0-41A5-9023-F95FFFCC1C53.config",
            "<?xml version=\"1.0\"?>",
            "sunlit-e2e-windows"
        ));
    }

    /// What the sweep does to a real directory, on both of its call paths.
    #[test]
    fn the_sweep_takes_our_stale_files_and_leaves_everything_else() {
        let app_data = std::env::temp_dir().join("sunlit_xtask_vmconnect_settings");
        let _ = std::fs::remove_dir_all(&app_data);
        let dir = vmconnect_settings_dir(&app_data);
        std::fs::create_dir_all(&dir).expect("temp tree");

        let current = vmconnect_settings_path(&app_data, "2333c8fa-0000-0000-0000-000000000001");
        let stale = vmconnect_settings_path(&app_data, "2333c8fa-0000-0000-0000-000000000002");
        let ours = vmconnect_settings("sunlit-e2e-windows", (1920, 1080));
        std::fs::write(&current, &ours).expect("write");
        std::fs::write(&stale, &ours).expect("write");

        let foreign = vmconnect_settings_path(&app_data, "aaaaaaaa-0000-0000-0000-000000000003");
        std::fs::write(&foreign, vmconnect_settings("dev-box", (1024, 768))).expect("write");
        let unrelated = dir.join("vmconnect.config");
        std::fs::write(&unrelated, "<configuration/>").expect("write");

        // Before a console opens: the file about to be used stays.
        forget_settings(&app_data, "sunlit-e2e-windows", Some(&current));
        assert!(current.is_file(), "the file this boot wrote");
        assert!(!stale.exists(), "the file an earlier boot left");
        assert!(foreign.is_file(), "another VM's settings");
        assert!(unrelated.is_file(), "vmconnect's own settings");

        // The name is what identifies the file to keep, on a file system where
        // two spellings of it are one file.
        let lower = dir.join(
            current
                .file_name()
                .and_then(|name| name.to_str())
                .expect("a name")
                .to_lowercase(),
        );
        forget_settings(&app_data, "sunlit-e2e-windows", Some(&lower));
        assert!(current.is_file(), "the same file under another spelling");

        // And when the VM goes, there is nothing left to keep it for.
        forget_settings(&app_data, "sunlit-e2e-windows", None);
        assert!(
            !current.exists(),
            "the VM is gone, so its settings are litter"
        );
        assert!(foreign.is_file(), "another VM's settings");
        assert!(unrelated.is_file(), "vmconnect's own settings");

        let _ = std::fs::remove_dir_all(&app_data);
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
