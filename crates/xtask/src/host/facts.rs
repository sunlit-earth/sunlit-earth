//! What the host looks like, gathered read-only and unelevated.
//!
//! Plan decision 2: the doctor changes nothing about the system. On Windows
//! that rules out `Get-WindowsOptionalFeature -Online`, which requires admin,
//! and rules in the CIM classes, which do not. Collection is one process
//! invocation per host that emits a flat JSON document; parsing and judging
//! happen here and in `doctor.rs`, over text a test can fabricate.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::provider::target::HostOs;
use crate::runner::{Cmd, Runner, powershell, ps_quote};

/// Tools every flow needs, whichever guest is being driven: both golden images
/// are built with Packer's QEMU builder, and every guest is reached over SSH.
pub const REQUIRED_TOOLS: [&str; 6] = [
    "qemu-system-x86_64",
    "qemu-img",
    "packer",
    "ssh",
    "scp",
    "ssh-keygen",
];

/// The ISO-building tools Packer will look for, in the order it tries them.
///
/// `cd_files` and `cd_content` are not built in Go: Packer's `StepCreateCD`
/// resolves one of these on `PATH` and shells out to it, and if none is there
/// the build fails immediately with "could not find a supported CD ISO
/// creation command". Both templates use `cd_content`, the Linux one for its
/// cloud-init seed and the Windows one for the unattend file, so this is a
/// hard requirement for building either image.
///
/// The list and its order are from `multistep/commonsteps/step_create_cdrom.go`
/// in `packer-plugin-sdk`, read on 2026-08-20. `floppy_files` needs none of
/// them, being pure Go, which is why this dependency arrived with the move to
/// a CD that q35's missing floppy controller forced.
pub const PACKER_ISO_TOOLS: [&str; 4] = ["xorriso", "mkisofs", "hdiutil", "oscdimg"];

/// VNC clients tried in order for `vm view` of a QEMU guest. Absence is a
/// warning, never a failure: the address is printed instead.
///
/// Executable names, which is what makes them findable. `TightVNC.Viewer` used
/// to be in this list and could never have matched anything: that is a package
/// identifier, and the program `TightVNC` installs is `tvnviewer`. It takes the
/// same `host:port` argument the others do, verified against a listening socket
/// rather than assumed, and accepts the `host::port` spelling too.
///
/// KRDC comes after the three that open straight onto the framebuffer, because
/// it puts a Host Configuration dialog in front of a host it has not seen
/// before. One click, and it remembers.
pub const VNC_VIEWERS: [&str; 6] = [
    "vncviewer",
    "tigervnc",
    "tvnviewer",
    "krdc",
    "remmina",
    "vinagre",
];

/// The argument a viewer takes for a console at `host:port`.
///
/// KRDC is a multi-protocol client and takes a URL, `krdc [options] url`: a
/// bare `127.0.0.1:5919` is refused with a "Malformed URL" dialog, and
/// `vnc://127.0.0.1:5919` connects. Measured against a QEMU VNC server on
/// 2026-08-31, which also confirmed the port is a port to it and not a display
/// number. Everything else in the list takes the address as it stands.
pub fn vnc_viewer_argument(viewer: &str, address: &str) -> String {
    match viewer {
        "krdc" => format!("vnc://{address}"),
        _ => address.to_owned(),
    }
}

/// Where `TightVNC`'s own installer puts the viewer.
///
/// Its MSI does not touch `PATH`, so a normal `TightVNC` install is invisible to
/// a `PATH` lookup. The directory is the one the MSI's own layout names, which
/// is also what the scoop manifest extracts from (`PFiles\TightVNC`).
pub const WINDOWS_TIGHTVNC_DIRS: [&str; 2] = [
    r"C:\Program Files\TightVNC",
    r"C:\Program Files (x86)\TightVNC",
];

/// Where the winget QEMU package puts its binaries.
///
/// `SoftwareFreedomConservancy.QEMU` is an NSIS installer whose manifest
/// declares `DefaultInstallLocation: %ProgramFiles%\qemu` and which does not
/// touch `PATH` (the upstream `qemu.nsi` writes no environment key at all), so
/// a perfectly good QEMU install is invisible to a plain `PATH` lookup.
pub const WINDOWS_QEMU_DIRS: [&str; 2] = [r"C:\Program Files\qemu", r"C:\Program Files (x86)\qemu"];

/// Where winget links the commands of the packages it installs.
///
/// A winget package that declares a command, as both `Hashicorp.Packer` and
/// `Microsoft.OSCDIMG` do, gets a link here, and winget appends this directory
/// to the user `PATH` when it creates the first one. The shell that ran
/// `vm setup` inherited its `PATH` before that happened, so the tool is
/// installed and invisible to a plain `PATH` lookup until a new shell starts.
/// Detection has to look here as well, or a second `vm setup` in the same
/// shell plans an install that winget then refuses as redundant.
pub const WINGET_LINKS_SUBPATH: &str = r"Microsoft\WinGet\Links";

/// The winget link for a tool, given `%LOCALAPPDATA%`.
///
/// Pure, so that the location is testable without an environment.
pub fn winget_link_candidate(tool: &str, local_app_data: Option<&Path>) -> Option<PathBuf> {
    Some(
        local_app_data?
            .join(WINGET_LINKS_SUBPATH)
            .join(format!("{tool}.exe")),
    )
}

/// Where scoop puts the shims that stand in for the programs it installs.
///
/// Scoop installs into a versioned directory and puts a shim of the same name
/// as the program in `<root>\shims`, which is the only name worth looking for:
/// the versioned path changes under an update and the `current` junction beside
/// it is an implementation detail. It appends the shims directory to the user
/// `PATH` when it is first set up, so this has the same shape as the winget
/// problem it sits next to: a shell that started before then, or one where the
/// user profile was not loaded, sees an installed program and an unchanged
/// `PATH`.
///
/// A shim is a stand-in for running the program and not for its directory,
/// which matters in exactly one place: QEMU carries the UEFI firmware in a
/// `share` directory beside the binary, and a shim has no such neighbour. That
/// case reports where it looked (`provider::firmware::missing_message`), and
/// `vm setup` installs QEMU through winget into a real directory, so it is a
/// path nothing here takes.
pub const SCOOP_SHIMS_SUBPATH: &str = "shims";

/// Both scoop roots for a tool: the per-user one and the global one.
///
/// Pure, so the locations are testable without an environment. Each root is
/// taken from its documented variable and falls back to its documented default
/// (`~\scoop` for `SCOOP`, `%ProgramData%\scoop` for `SCOOP_GLOBAL`), because a
/// scoop install that never set either variable is the ordinary case rather
/// than an exotic one. The per-user root comes first: `scoop install` without
/// `--global` is what a developer runs, and a machine with both should prefer
/// the one that user's own commands resolve to.
pub fn scoop_shim_candidates(
    tool: &str,
    scoop: Option<&Path>,
    user_profile: Option<&Path>,
    scoop_global: Option<&Path>,
    program_data: Option<&Path>,
) -> Vec<PathBuf> {
    let user_root = scoop
        .map(Path::to_path_buf)
        .or_else(|| user_profile.map(|home| home.join("scoop")));
    let global_root = scoop_global
        .map(Path::to_path_buf)
        .or_else(|| program_data.map(|data| data.join("scoop")));
    [user_root, global_root]
        .into_iter()
        .flatten()
        .map(|root| root.join(SCOOP_SHIMS_SUBPATH).join(format!("{tool}.exe")))
        .collect()
}

/// The SID of the local `Hyper-V Administrators` group. Membership in it is
/// what lets `vm status`, `e2e`, and `vm down` drive the hypervisor without
/// elevation.
pub const HYPERV_ADMINS_SID: &str = "S-1-5-32-578";

/// The optional-feature names the doctor asks about.
pub const FEATURE_HYPERV: &str = "Microsoft-Hyper-V-All";
pub const FEATURE_WHPX: &str = "HypervisorPlatform";

/// The WSL distribution the Linux guest's binaries are built in.
///
/// Deliberately older than the guest, not the same as it: the guest is Debian 13
/// with glibc 2.41 and this is Ubuntu 22.04 with glibc 2.35, and old-builds-run-
/// on-new is the direction glibc's compatibility works in. The reverse is what
/// breaks, so the thing to keep an eye on is the guest moving above this rather
/// than this drifting below the guest. It used to be true that the two matched,
/// and the claim outlived the guest's move off Ubuntu by a phase.
pub const WSL_DISTRO: &str = "Ubuntu-22.04";

/// Which of the ISO builders Packer would pick from those present.
///
/// Packer tries its own order rather than a host-preferred one, so the answer
/// to "will the build work" is whether any is present, and the answer to
/// "which one will run" is the first of Packer's list that is.
pub fn packer_iso_tool<'a>(available: &[&'a str]) -> Option<&'a str> {
    PACKER_ISO_TOOLS
        .iter()
        .find_map(|wanted| available.iter().find(|found| *found == wanted).copied())
}

/// `Win32_OptionalFeature.InstallState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureState {
    Enabled,
    Disabled,
    Absent,
    Unknown,
}

impl FeatureState {
    pub fn from_code(code: i32) -> Self {
        match code {
            1 => Self::Enabled,
            2 => Self::Disabled,
            3 => Self::Absent,
            _ => Self::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Absent => "not available on this edition",
            Self::Unknown => "could not be determined",
        }
    }
}

/// One WSL distribution as `wsl --list --verbose` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslDistro {
    pub name: String,
    pub state: String,
    pub version: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowsFacts {
    pub caption: String,
    pub version: String,
    pub sku: u32,
    pub hypervisor_present: bool,
    /// `Win32_Processor.VirtualizationFirmwareEnabled`. Consulted only when no
    /// hypervisor is running: it can read false while `Hyper-V` owns VT-x.
    pub virtualization_firmware_enabled: Option<bool>,
    pub features: BTreeMap<String, FeatureState>,
    pub vmms_state: Option<String>,
    pub in_hyperv_admins: bool,
    pub elevated: bool,
    pub wsl_distros: Vec<WslDistro>,
    /// The machine `PATH`, needed because the winget QEMU package installs a
    /// working QEMU and leaves it off `PATH` entirely.
    pub machine_path: String,
    /// The user `PATH`, which is where winget appends its links directory.
    pub user_path: String,
}

impl WindowsFacts {
    pub fn feature(&self, name: &str) -> FeatureState {
        self.features
            .get(name)
            .copied()
            .unwrap_or(FeatureState::Unknown)
    }

    /// Whether the edition can run `Hyper-V` at all. Home cannot.
    pub fn edition_supports_hyperv(&self) -> bool {
        // SKU 101 is Windows Home, 100 Home N, 98 Home Single Language,
        // 99 Home China. The caption check is the readable backstop.
        !matches!(self.sku, 98..=101) && !self.caption.to_ascii_lowercase().contains(" home")
    }

    pub fn distro(&self, name: &str) -> Option<&WslDistro> {
        self.wsl_distros
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case(name))
    }

    /// Whether a directory is already on the persisted `PATH`, machine or user,
    /// comparing the way Windows does: case-insensitively and ignoring a
    /// trailing separator.
    ///
    /// Both halves count, because the question every caller is really asking is
    /// "will a new shell find this", and winget answers it by appending to the
    /// user half.
    pub fn path_contains(&self, dir: &str) -> bool {
        let mut entries = self
            .machine_path
            .split(';')
            .chain(self.user_path.split(';'));
        entries.any(|entry| {
            entry
                .trim()
                .trim_end_matches(['\\', '/'])
                .eq_ignore_ascii_case(dir.trim_end_matches(['\\', '/']))
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinuxFacts {
    pub pretty_name: String,
    pub kvm_present: bool,
    pub kvm_writable: bool,
    pub groups: Vec<String>,
}

impl LinuxFacts {
    pub fn in_kvm_group(&self) -> bool {
        self.groups.iter().any(|g| g == "kvm")
    }
}

/// Everything the doctor and the setup command reason about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostFacts {
    pub os: Option<HostOs>,
    pub description: String,
    pub windows: Option<WindowsFacts>,
    pub linux: Option<LinuxFacts>,
    /// Tool name to resolved path, `None` when it was not found.
    pub tools: BTreeMap<String, Option<PathBuf>>,
    pub vnc_viewer: Option<PathBuf>,
    /// The ISO builders on `PATH`, of the ones Packer knows how to drive.
    pub iso_tools: Vec<String>,
    /// ISO builders that are installed but not on this process's `PATH`. That
    /// is the state a fresh `vm setup` leaves behind, and it is a different
    /// answer from "missing": a new shell finds them.
    pub iso_tools_off_path: Vec<String>,
    /// The UEFI firmware OVMF installed, when this host has it. Only the
    /// Windows guest under QEMU needs it, so its absence is a warning rather
    /// than a failure.
    pub uefi_firmware: Option<crate::provider::firmware::Firmware>,
    /// Free space where the image store lives, or the nearest existing parent.
    pub free_bytes: Option<u64>,
    /// Why the host probe failed, when it did. Every other field is then at its
    /// default, which the doctor reports rather than reading as fact.
    pub probe_error: Option<String>,
}

impl HostFacts {
    pub fn os(&self) -> HostOs {
        self.os.unwrap_or(HostOs::Other)
    }

    pub fn tool(&self, name: &str) -> Option<&Path> {
        self.tools.get(name).and_then(Option::as_deref)
    }

    /// The ISO builder that is installed, whether or not this process's `PATH`
    /// can see it yet. This is the question `vm setup` asks, because installing
    /// a second copy of a tool winget already has is not an improvement.
    pub fn iso_tool_installed(&self) -> Option<&str> {
        self.iso_tool().or_else(|| {
            let off_path: Vec<&str> = self.iso_tools_off_path.iter().map(String::as_str).collect();
            packer_iso_tool(&off_path)
        })
    }

    /// The ISO builder Packer would use, if any is present.
    pub fn iso_tool(&self) -> Option<&str> {
        let available: Vec<&str> = self.iso_tools.iter().map(String::as_str).collect();
        packer_iso_tool(&available).map(|found| {
            PACKER_ISO_TOOLS
                .iter()
                .find(|known| **known == found)
                .copied()
                .unwrap_or(found)
        })
    }
}

// ---------------------------------------------------------------------------
// Collection
// ---------------------------------------------------------------------------

/// The probe script. One invocation, one JSON document, no state changed.
///
/// Lists travel as strings rather than JSON arrays on purpose: Windows
/// `PowerShell` 5.1's `ConvertTo-Json` collapses a single-element array to a
/// scalar, so an array-shaped field would parse differently depending on how
/// many `Hyper-V` features happen to be installed.
const WINDOWS_PROBE: &str = r#"
$os = Get-CimInstance Win32_OperatingSystem
$cs = Get-CimInstance Win32_ComputerSystem
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1

$pairs = @()
foreach ($name in @('Microsoft-Hyper-V-All','HypervisorPlatform')) {
  $feature = Get-CimInstance -ClassName Win32_OptionalFeature -Filter "Name='$name'" -ErrorAction SilentlyContinue
  # 4 is Unknown. A query that returned nothing means the question could not
  # be answered, which is a different thing from the feature not existing on
  # this edition, and reporting it as absent sent people to buy Windows Pro.
  $state = if ($feature) { [int]$feature.InstallState } else { 4 }
  $pairs += "$name=$state"
}

# In a try rather than with -ErrorAction SilentlyContinue. On a host without
# Hyper-V there is no such service, and a suppressed error still leaves
# powershell.exe exiting 1, which would make the whole probe read as broken and
# hide every feature answer behind it. A caught one costs nothing.
$vmms = ''
try {
  $service = Get-Service -Name vmms -ErrorAction Stop
  $vmms = $service.Status.ToString()
} catch { }

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$inHyperV = @($identity.Groups | Where-Object { $_.Value -eq 'S-1-5-32-578' }).Count -gt 0
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
$elevated = $principal.IsInRole([Security.Principal.WindowsBuiltinRole]::Administrator)

$env:WSL_UTF8 = '1'
$wsl = ''
try { $wsl = (& wsl.exe --list --verbose 2>&1 | Out-String) } catch { $wsl = '' }

$free = $null
try {
  $qualifier = Split-Path -Qualifier __STORE__
  $free = (New-Object System.IO.DriveInfo($qualifier + '\')).AvailableFreeSpace
} catch { $free = $null }

[pscustomobject]@{
  caption = [string]$os.Caption
  version = [string]$os.Version
  sku = [int]$os.OperatingSystemSKU
  hypervisor_present = [bool]$cs.HypervisorPresent
  virtualization_firmware_enabled = $cpu.VirtualizationFirmwareEnabled
  features = ($pairs -join ';')
  vmms_state = $vmms
  in_hyperv_admins = $inHyperV
  elevated = $elevated
  wsl_raw = $wsl
  free_bytes = $free
  machine_path = [string][Environment]::GetEnvironmentVariable('Path','Machine')
  user_path = [string][Environment]::GetEnvironmentVariable('Path','User')
} | ConvertTo-Json -Compress
"#;

/// The probe script with the store path filled in.
pub fn windows_probe_script(store_root: &Path) -> String {
    WINDOWS_PROBE.replace("__STORE__", &ps_quote(store_root))
}

/// Gather everything about the host.
pub fn collect(runner: &dyn Runner, host: HostOs, store_root: &Path) -> HostFacts {
    let mut facts = HostFacts {
        os: Some(host),
        ..HostFacts::default()
    };

    for tool in REQUIRED_TOOLS {
        facts
            .tools
            .insert(tool.to_owned(), resolve_tool(runner, tool, host));
    }
    facts.vnc_viewer = VNC_VIEWERS
        .iter()
        .find_map(|viewer| resolve_tool(runner, viewer, host));
    // Where it lives on a Windows host depends on where QEMU is, so this comes
    // after the tools.
    facts.uefi_firmware = crate::provider::firmware::locate(host, facts.tool("qemu-system-x86_64"));

    // Packer resolves these on PATH itself, so a fallback location would not
    // help it: only a real PATH hit counts here.
    facts.iso_tools = PACKER_ISO_TOOLS
        .iter()
        .filter(|tool| runner.which(tool).is_some())
        .map(|tool| (*tool).to_owned())
        .collect();
    // Installed but not yet on this process's PATH, which is what a winget
    // install in this same shell looks like.
    facts.iso_tools_off_path = PACKER_ISO_TOOLS
        .iter()
        .filter(|tool| runner.which(tool).is_none())
        .filter(|tool| resolve_tool(runner, tool, host).is_some())
        .map(|tool| (*tool).to_owned())
        .collect();

    match host {
        HostOs::Windows => {
            facts
                .tools
                .insert("wsl".to_owned(), resolve_tool(runner, "wsl", host));
            facts.tools.insert(
                "vmconnect".to_owned(),
                resolve_tool(runner, "vmconnect", host),
            );
            let script = windows_probe_script(store_root);
            match runner.capture(&powershell(&script)) {
                Ok(output) if output.success() => match parse_windows_facts(output.trimmed()) {
                    Ok((windows, free)) => {
                        facts.description = format!("{} {}", windows.caption, windows.version);
                        facts.free_bytes = free;
                        facts.windows = Some(windows);
                    }
                    // A script read from stdin reports success even when it
                    // died on its first statement, so the stderr goes into the
                    // message: without it the report says only that the output
                    // was empty.
                    Err(e) => {
                        facts.probe_error = Some(match output.stderr.trim() {
                            "" => e,
                            stderr => format!("{e} (stderr: {stderr})"),
                        });
                    }
                },
                Ok(output) => {
                    facts.probe_error = Some(format!(
                        "the host probe failed (exit {:?}): {}",
                        output.code,
                        output.stderr.trim()
                    ));
                }
                Err(e) => facts.probe_error = Some(format!("cannot run powershell.exe: {e}")),
            }
        }
        HostOs::Linux => {
            let linux = collect_linux(runner);
            facts.description = if linux.pretty_name.is_empty() {
                "Linux".to_owned()
            } else {
                linux.pretty_name.clone()
            };
            facts.linux = Some(linux);
            facts.free_bytes = free_space_linux(runner, store_root);
        }
        HostOs::Other => {
            std::env::consts::OS.clone_into(&mut facts.description);
        }
    }

    facts
}

/// Find a tool, falling back to the places a package manager is known to put it
/// without putting it on `PATH`.
pub fn resolve_tool(runner: &dyn Runner, tool: &str, host: HostOs) -> Option<PathBuf> {
    if let Some(path) = runner.which(tool) {
        return Some(path);
    }
    fallback_candidates(tool, host)
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// Well-known absolute locations for a tool, by name and host.
pub fn fallback_candidates(tool: &str, host: HostOs) -> Vec<PathBuf> {
    if host != HostOs::Windows {
        return Vec::new();
    }
    let mut out = Vec::new();
    if tool.starts_with("qemu") {
        for dir in WINDOWS_QEMU_DIRS {
            out.push(Path::new(dir).join(format!("{tool}.exe")));
        }
    }
    match tool {
        "vmconnect" => out.push(PathBuf::from(r"C:\Windows\System32\vmconnect.exe")),
        "ssh" | "scp" | "ssh-keygen" => {
            out.push(Path::new(r"C:\Windows\System32\OpenSSH").join(format!("{tool}.exe")));
        }
        "wsl" => out.push(PathBuf::from(r"C:\Windows\System32\wsl.exe")),
        "tvnviewer" => {
            for dir in WINDOWS_TIGHTVNC_DIRS {
                out.push(Path::new(dir).join(format!("{tool}.exe")));
            }
        }
        _ => {}
    }
    // Last, because a real installation directory is the better answer when
    // there is one; these are what make a just-installed tool findable. Every
    // path here is a guess that is checked for existence by the only caller, so
    // naming a package manager the host has never had costs nothing.
    if let Some(link) = winget_link_candidate(
        tool,
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .as_deref(),
    ) {
        out.push(link);
    }
    let env = |name: &str| std::env::var_os(name).map(PathBuf::from);
    out.extend(scoop_shim_candidates(
        tool,
        env("SCOOP").as_deref(),
        env("USERPROFILE").as_deref(),
        env("SCOOP_GLOBAL").as_deref(),
        env("ProgramData").as_deref(),
    ));
    out
}

fn collect_linux(runner: &dyn Runner) -> LinuxFacts {
    let pretty_name = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|text| parse_os_release_pretty_name(&text))
        .unwrap_or_default();

    let kvm = Path::new("/dev/kvm");
    let kvm_present = kvm.exists();
    // Opening for write is the only honest test of "writable": the permission
    // bits alone do not account for group membership that a relogin has not
    // applied yet. The handle is dropped immediately and nothing is written.
    let kvm_writable = kvm_present
        && std::fs::OpenOptions::new()
            .write(true)
            .open(kvm)
            .map(drop)
            .is_ok();

    let groups = runner
        .capture(&Cmd::new("id").arg("-nG"))
        .ok()
        .filter(crate::runner::CommandOutput::success)
        .map(|out| {
            out.trimmed()
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    LinuxFacts {
        pretty_name,
        kvm_present,
        kvm_writable,
        groups,
    }
}

fn free_space_linux(runner: &dyn Runner, path: &Path) -> Option<u64> {
    let probe = existing_ancestor(path)?;
    let out = runner
        .capture(&Cmd::new("df").args(["-kP".to_owned(), probe.to_string_lossy().into_owned()]))
        .ok()?;
    out.success()
        .then(|| parse_df_available_bytes(&out.stdout))?
}

/// The deepest existing directory at or above `path`. A store that has never
/// been created still sits on a filesystem whose free space is the answer.
pub fn existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        if candidate.is_dir() {
            return Some(candidate.to_path_buf());
        }
        current = candidate.parent();
    }
    None
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawWindowsFacts {
    #[serde(default)]
    caption: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    sku: u32,
    #[serde(default)]
    hypervisor_present: bool,
    #[serde(default)]
    virtualization_firmware_enabled: Option<bool>,
    #[serde(default)]
    features: String,
    #[serde(default)]
    vmms_state: String,
    #[serde(default)]
    in_hyperv_admins: bool,
    #[serde(default)]
    elevated: bool,
    #[serde(default)]
    wsl_raw: String,
    #[serde(default)]
    free_bytes: Option<u64>,
    #[serde(default)]
    machine_path: String,
    #[serde(default)]
    user_path: String,
}

/// Parse the probe's JSON into facts plus the free-space figure.
pub fn parse_windows_facts(json: &str) -> Result<(WindowsFacts, Option<u64>), String> {
    let raw: RawWindowsFacts = serde_json::from_str(json)
        .map_err(|e| format!("cannot read the host probe output: {e}"))?;

    let features = parse_pairs(&raw.features)
        .into_iter()
        .map(|(name, value)| {
            let state = value
                .parse::<i32>()
                .map_or(FeatureState::Unknown, FeatureState::from_code);
            (name, state)
        })
        .collect();

    let facts = WindowsFacts {
        caption: raw.caption.trim().to_owned(),
        version: raw.version.trim().to_owned(),
        sku: raw.sku,
        hypervisor_present: raw.hypervisor_present,
        virtualization_firmware_enabled: raw.virtualization_firmware_enabled,
        features,
        vmms_state: Some(raw.vmms_state.trim().to_owned()).filter(|s| !s.is_empty()),
        in_hyperv_admins: raw.in_hyperv_admins,
        elevated: raw.elevated,
        wsl_distros: parse_wsl_list(&raw.wsl_raw),
        machine_path: raw.machine_path,
        user_path: raw.user_path,
    };
    Ok((facts, raw.free_bytes))
}

/// Split a `a=1;b=2` string, tolerating blanks and stray separators.
pub fn parse_pairs(text: &str) -> Vec<(String, String)> {
    text.split(';')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            let key = key.trim();
            (!key.is_empty()).then(|| (key.to_owned(), value.trim().to_owned()))
        })
        .collect()
}

/// Parse `wsl --list --verbose` output.
///
/// The command writes UTF-16 by default, which arrives as interleaved NUL bytes
/// through a pipe; the probe sets `WSL_UTF8=1` first, and this strips any NULs
/// that survive so a host on an older WSL still parses. The header row and the
/// `*` default marker are dropped.
pub fn parse_wsl_list(text: &str) -> Vec<WslDistro> {
    text.replace('\u{0}', "")
        .lines()
        .filter_map(|line| {
            let line = line.trim_start().trim_start_matches('*').trim();
            if line.is_empty() || line.starts_with("NAME") {
                return None;
            }
            let mut fields = line.split_whitespace();
            let name = fields.next()?.to_owned();
            let state = fields.next().unwrap_or_default().to_owned();
            let version = fields.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            // A distro list that failed prints a sentence, not a table.
            (!name.contains(':') && version > 0).then_some(WslDistro {
                name,
                state,
                version,
            })
        })
        .collect()
}

/// `PRETTY_NAME` from `/etc/os-release`.
pub fn parse_os_release_pretty_name(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|value| value.trim().trim_matches('"').to_owned())
}

/// The `Available` column of `df -kP`, in bytes.
pub fn parse_df_available_bytes(text: &str) -> Option<u64> {
    let line = text.lines().nth(1)?;
    let available: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
    Some(available * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROBE_JSON: &str = r#"{
        "caption": "Microsoft Windows 11 Pro",
        "version": "10.0.26200",
        "sku": 48,
        "hypervisor_present": true,
        "virtualization_firmware_enabled": null,
        "features": "Microsoft-Hyper-V-All=1;HypervisorPlatform=2",
        "vmms_state": "Running",
        "in_hyperv_admins": true,
        "elevated": false,
        "wsl_raw": "  NAME            STATE           VERSION\r\n* Ubuntu-22.04    Stopped         2\r\n  docker-desktop  Running         2\r\n",
        "free_bytes": 274877906944,
        "machine_path": "C:\\Windows\\system32;C:\\Program Files\\qemu\\"
    }"#;

    #[test]
    fn the_probe_output_parses_into_facts() {
        let (facts, free) = parse_windows_facts(PROBE_JSON).expect("valid probe output");
        assert_eq!(facts.caption, "Microsoft Windows 11 Pro");
        assert_eq!(facts.sku, 48);
        assert!(facts.hypervisor_present);
        assert_eq!(facts.virtualization_firmware_enabled, None);
        assert_eq!(facts.feature(FEATURE_HYPERV), FeatureState::Enabled);
        assert_eq!(facts.feature(FEATURE_WHPX), FeatureState::Disabled);
        assert_eq!(facts.vmms_state.as_deref(), Some("Running"));
        assert!(facts.in_hyperv_admins);
        assert!(!facts.elevated);
        assert_eq!(free, Some(274_877_906_944));
        assert_eq!(facts.distro(WSL_DISTRO).map(|d| d.version), Some(2));
    }

    #[test]
    fn path_membership_ignores_case_and_a_trailing_separator() {
        let (facts, _) = parse_windows_facts(PROBE_JSON).expect("valid probe output");
        assert!(facts.path_contains(r"C:\Program Files\qemu"));
        assert!(facts.path_contains(r"c:\program files\qemu\"));
        assert!(!facts.path_contains(r"C:\Program Files\packer"));
        assert!(!WindowsFacts::default().path_contains(r"C:\Program Files\qemu"));
    }

    #[test]
    fn an_unasked_feature_reads_as_unknown_rather_than_absent() {
        let (facts, _) = parse_windows_facts(PROBE_JSON).expect("valid");
        assert_eq!(facts.feature("Containers"), FeatureState::Unknown);
    }

    #[test]
    fn a_probe_that_returned_nothing_useful_is_an_error_not_a_default() {
        assert!(parse_windows_facts("").is_err());
        assert!(parse_windows_facts("Get-CimInstance : Access denied").is_err());
    }

    #[test]
    fn a_probe_missing_fields_still_parses_with_defaults() {
        let (facts, free) = parse_windows_facts(r#"{"caption":"Windows"}"#).expect("defaults");
        assert_eq!(facts.caption, "Windows");
        assert!(!facts.hypervisor_present);
        assert_eq!(free, None);
        assert!(facts.wsl_distros.is_empty());
    }

    #[test]
    fn a_feature_query_that_failed_is_unknown_rather_than_absent() {
        // "absent" reads as "this edition of Windows cannot do it", which
        // sends someone to buy a different one. A query that did not answer
        // says only that.
        let (facts, _) =
            parse_windows_facts(r#"{"features": "Microsoft-Hyper-V-All=4;HypervisorPlatform=1"}"#)
                .expect("valid");
        assert_eq!(facts.feature(FEATURE_HYPERV), FeatureState::Unknown);
        assert_eq!(FeatureState::Unknown.label(), "could not be determined");
        assert!(!FeatureState::Unknown.label().contains("edition"));
    }

    #[test]
    fn feature_codes_map_to_states() {
        assert_eq!(FeatureState::from_code(1), FeatureState::Enabled);
        assert_eq!(FeatureState::from_code(2), FeatureState::Disabled);
        assert_eq!(FeatureState::from_code(3), FeatureState::Absent);
        assert_eq!(FeatureState::from_code(4), FeatureState::Unknown);
        assert_eq!(FeatureState::from_code(-1), FeatureState::Unknown);
    }

    #[test]
    fn home_editions_are_recognized_by_sku_and_by_name() {
        let mut facts = WindowsFacts {
            sku: 48,
            caption: "Microsoft Windows 11 Pro".to_owned(),
            ..WindowsFacts::default()
        };
        assert!(facts.edition_supports_hyperv());

        facts.sku = 101;
        assert!(!facts.edition_supports_hyperv());

        facts.sku = 48;
        facts.caption = "Microsoft Windows 11 Home".to_owned();
        assert!(!facts.edition_supports_hyperv());
    }

    #[test]
    fn pairs_survive_blanks_and_stray_separators() {
        assert_eq!(parse_pairs(""), Vec::new());
        assert_eq!(
            parse_pairs("a=1;;b=2;"),
            vec![
                ("a".to_owned(), "1".to_owned()),
                ("b".to_owned(), "2".to_owned())
            ]
        );
        assert_eq!(parse_pairs("novalue"), Vec::new());
    }

    #[test]
    fn the_wsl_table_parses_and_drops_the_header_and_marker() {
        let distros = parse_wsl_list(
            "  NAME            STATE           VERSION\n\
             * Ubuntu-22.04    Running         2\n\
               legacy          Stopped         1\n",
        );
        assert_eq!(distros.len(), 2);
        assert_eq!(distros[0].name, "Ubuntu-22.04");
        assert_eq!(distros[0].state, "Running");
        assert_eq!(distros[0].version, 2);
        assert_eq!(distros[1].version, 1);
    }

    #[test]
    fn a_utf16_wsl_table_still_parses() {
        let raw = "\u{0}N\u{0}A\u{0}M\u{0}E\u{0}\n\u{0}*\u{0} \u{0}U\u{0}b\u{0}u\u{0}n\u{0}t\u{0}u\u{0}-\u{0}2\u{0}2\u{0}.\u{0}0\u{0}4\u{0} \u{0} \u{0}R\u{0}u\u{0}n\u{0}n\u{0}i\u{0}n\u{0}g\u{0} \u{0} \u{0}2\u{0}\n";
        let distros = parse_wsl_list(raw);
        assert_eq!(distros.len(), 1);
        assert_eq!(distros[0].name, "Ubuntu-22.04");
    }

    #[test]
    fn a_wsl_error_message_is_not_mistaken_for_a_distro() {
        let distros = parse_wsl_list(
            "Windows Subsystem for Linux has no installed distributions.\n\
             Error: 0x8004032d\n",
        );
        assert!(distros.is_empty(), "{distros:?}");
        assert!(parse_wsl_list("").is_empty());
    }

    #[test]
    fn os_release_yields_the_pretty_name() {
        let text = "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 22.04.4 LTS\"\nID=ubuntu\n";
        assert_eq!(
            parse_os_release_pretty_name(text).as_deref(),
            Some("Ubuntu 22.04.4 LTS")
        );
        assert_eq!(parse_os_release_pretty_name("ID=ubuntu"), None);
    }

    #[test]
    fn df_output_is_read_from_the_available_column() {
        let text = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n\
                    /dev/sda1        263174212 100000000 149000000      41% /\n";
        assert_eq!(parse_df_available_bytes(text), Some(149_000_000 * 1024));
        assert_eq!(parse_df_available_bytes("header only\n"), None);
        assert_eq!(parse_df_available_bytes(""), None);
    }

    #[test]
    fn kvm_group_membership_is_read_from_the_group_list() {
        let facts = LinuxFacts {
            groups: vec!["dev".to_owned(), "kvm".to_owned()],
            ..LinuxFacts::default()
        };
        assert!(facts.in_kvm_group());
        assert!(!LinuxFacts::default().in_kvm_group());
    }

    #[test]
    fn packer_picks_the_first_of_its_own_order_that_is_present() {
        // Packer tries its list in its order, not the host's preference, so
        // "which one will run" is not "which one was found first".
        assert_eq!(packer_iso_tool(&["oscdimg", "xorriso"]), Some("xorriso"));
        assert_eq!(packer_iso_tool(&["mkisofs", "oscdimg"]), Some("mkisofs"));
        assert_eq!(packer_iso_tool(&["oscdimg"]), Some("oscdimg"));
        assert_eq!(packer_iso_tool(&[]), None);
        assert_eq!(packer_iso_tool(&["genisoimage"]), None);
    }

    #[test]
    fn the_iso_tool_list_is_the_one_packer_looks_for() {
        // From packer-plugin-sdk's step_create_cdrom.go. Order matters: it is
        // what decides which tool runs on a host that has several.
        assert_eq!(
            PACKER_ISO_TOOLS,
            ["xorriso", "mkisofs", "hdiutil", "oscdimg"]
        );
    }

    // Windows path semantics: off Windows, `Path` treats a drive-qualified
    // path as a single component, and this code only ever runs on a
    // Windows host anyway.
    #[cfg(windows)]
    #[test]
    fn windows_tool_fallbacks_cover_the_winget_qemu_install_location() {
        let candidates = fallback_candidates("qemu-system-x86_64", HostOs::Windows);
        assert!(
            candidates
                .iter()
                .any(|p| p == Path::new(r"C:\Program Files\qemu\qemu-system-x86_64.exe")),
            "{candidates:?}"
        );
        assert!(fallback_candidates("qemu-img", HostOs::Linux).is_empty());
        assert!(
            fallback_candidates("vmconnect", HostOs::Windows)
                .iter()
                .any(|p| p.ends_with("vmconnect.exe"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_winget_installed_tool_is_found_through_its_links_directory() {
        let candidate =
            winget_link_candidate("packer", Some(Path::new(r"C:\Users\me\AppData\Local")))
                .expect("a local app data directory yields a candidate");
        assert_eq!(
            candidate,
            Path::new(r"C:\Users\me\AppData\Local\Microsoft\WinGet\Links\packer.exe")
        );
        assert!(winget_link_candidate("packer", None).is_none());
        assert!(
            fallback_candidates("packer", HostOs::Windows)
                .iter()
                .any(|p| p.ends_with(r"Microsoft\WinGet\Links\packer.exe")),
            "a winget package that declares a command lands there"
        );
    }

    /// Every name here has to be one a `PATH` lookup could return, which is the
    /// mistake `TightVNC.Viewer` was: a package identifier in a list of
    /// commands, so the one viewer it was meant to cover was never found.
    #[test]
    fn every_vnc_viewer_is_named_the_way_a_command_is() {
        for viewer in VNC_VIEWERS {
            assert!(
                viewer
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{viewer} is not a plain command name"
            );
        }
        assert!(VNC_VIEWERS.contains(&"tvnviewer"), "TightVNC's viewer");
    }

    /// A viewer handed an address it cannot parse is a viewer that opens an
    /// error dialog instead of the guest's console.
    #[test]
    fn krdc_is_handed_a_url_and_the_others_the_bare_address() {
        assert_eq!(
            vnc_viewer_argument("krdc", "127.0.0.1:5919"),
            "vnc://127.0.0.1:5919"
        );
        for viewer in VNC_VIEWERS.iter().filter(|name| **name != "krdc") {
            assert_eq!(
                vnc_viewer_argument(viewer, "127.0.0.1:5919"),
                "127.0.0.1:5919",
                "{viewer}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_scoop_installed_tool_is_found_through_its_shims_directory() {
        // Both roots named, which is the machine that has each kind of install.
        assert_eq!(
            scoop_shim_candidates(
                "tvnviewer",
                Some(Path::new(r"D:\scoop")),
                None,
                Some(Path::new(r"C:\scoop-global")),
                None
            ),
            vec![
                PathBuf::from(r"D:\scoop\shims\tvnviewer.exe"),
                PathBuf::from(r"C:\scoop-global\shims\tvnviewer.exe"),
            ]
        );
        // Neither named, which is the ordinary install: scoop sets no variable
        // and lives under the profile.
        assert_eq!(
            scoop_shim_candidates(
                "tvnviewer",
                None,
                Some(Path::new(r"C:\Users\me")),
                None,
                Some(Path::new(r"C:\ProgramData"))
            ),
            vec![
                PathBuf::from(r"C:\Users\me\scoop\shims\tvnviewer.exe"),
                PathBuf::from(r"C:\ProgramData\scoop\shims\tvnviewer.exe"),
            ]
        );
        // Nothing to go on is no candidates rather than a relative path, which
        // would be resolved against whatever directory the xtask was run from.
        assert!(scoop_shim_candidates("tvnviewer", None, None, None, None).is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn the_tightvnc_viewer_is_looked_for_where_both_of_its_installers_put_it() {
        let candidates = fallback_candidates("tvnviewer", HostOs::Windows);
        assert!(
            candidates
                .iter()
                .any(|p| p == Path::new(r"C:\Program Files\TightVNC\tvnviewer.exe")),
            "the MSI's own location: {candidates:?}"
        );
        assert!(
            candidates
                .iter()
                .any(|p| p.ends_with(r"scoop\shims\tvnviewer.exe")),
            "a scoop install: {candidates:?}"
        );
        // The real installation directory is the better answer, so it is tried
        // before the shim that only stands in for one.
        let program_files = candidates
            .iter()
            .position(|p| p.ends_with(r"TightVNC\tvnviewer.exe"));
        let shim = candidates
            .iter()
            .position(|p| p.ends_with(r"scoop\shims\tvnviewer.exe"));
        assert!(program_files < shim, "{candidates:?}");
    }

    #[test]
    fn the_user_half_of_the_path_counts_as_being_on_path() {
        // winget appends its links directory to the user PATH, and the question
        // every caller is asking is whether a new shell would find the tool.
        let facts = WindowsFacts {
            machine_path: r"C:\Windows\System32".to_owned(),
            user_path: r"C:\Users\me\AppData\Local\Microsoft\WinGet\Links;".to_owned(),
            ..WindowsFacts::default()
        };
        assert!(facts.path_contains(r"C:\Users\me\AppData\Local\Microsoft\WinGet\Links"));
        assert!(facts.path_contains(r"c:\windows\system32\"));
        assert!(!facts.path_contains(r"C:\Program Files\qemu"));
    }

    #[test]
    fn an_iso_builder_off_path_is_installed_but_not_what_packer_sees() {
        let facts = HostFacts {
            iso_tools_off_path: vec!["oscdimg".to_owned()],
            ..HostFacts::default()
        };
        assert_eq!(facts.iso_tool(), None, "Packer only looks on PATH");
        assert_eq!(facts.iso_tool_installed(), Some("oscdimg"));

        let both = HostFacts {
            iso_tools: vec!["xorriso".to_owned()],
            iso_tools_off_path: vec!["oscdimg".to_owned()],
            ..HostFacts::default()
        };
        assert_eq!(both.iso_tool(), Some("xorriso"));
        assert_eq!(both.iso_tool_installed(), Some("xorriso"));
    }

    #[test]
    fn the_probe_script_carries_the_store_path_and_the_group_sid() {
        let script = WINDOWS_PROBE.replace("__STORE__", &ps_quote(Path::new(r"C:\vm")));
        assert!(script.contains(r"'C:\vm'"));
        assert!(script.contains(HYPERV_ADMINS_SID));
        assert!(!script.contains("__STORE__"));
        // Nothing in the probe may change the machine.
        for forbidden in ["Enable-Windows", "Set-", "New-Item", "Add-LocalGroupMember"] {
            assert!(
                !script.contains(forbidden),
                "probe must be read-only: {forbidden}"
            );
        }
    }
}
