//! Installing Windows natively on `Hyper-V`, with no builder in the loop.
//!
//! What a Windows host does for `cargo xtask vm build-image windows`, per the
//! amendment `docs/plans/2026-08-20-phase3-amendment-hyperv-windows-build.md`.
//! A Linux host still builds this image with Packer and QEMU, and that path is
//! untouched: QEMU on a Windows host runs on WHPX, and a WHPX guest does not
//! survive the reset Windows Setup performs after copying its files, which is
//! deviation 25 of the phase 3 plan.
//!
//! Everything Packer contributed on this target had shrunk to CD creation, an
//! SSH wait, one provisioner, and a shutdown command, each of which the xtask
//! already performs elsewhere. So it does them here in order: repack the media
//! so the installer needs no keypress, build the unattend CD, create the VM,
//! start it, watch it install, run `finalize.ps1` over SSH, shut it down, keep
//! the disk and drop the VM, and turn the disk into the golden image.
//!
//! Two rules run through all of it. The build VM carries the same name the
//! runtime guest does, so `vm view`, `vm ssh`, `vm down`, and the
//! one-VM-at-a-time check apply to a build without new plumbing. And no failure
//! after the VM exists may leave a guest running silently: every one of them
//! either removes it or prints what is still there and how to reach it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::commands::build_image;
use crate::commands::build_watch::{Action, Guest, Sample, Trend, report};
use crate::commands::vm;
use crate::guest::ssh::{self, SshTarget};
use crate::provider::hyperv;
use crate::provider::target::{HostOs, ProviderKind, Target};
use crate::runner::{Runner, powershell, ps_quote};
use crate::store::state::{RunState, StartReason};
use crate::store::{self, Store, windows_media};
use crate::util::{self, format_bytes, format_duration};

/// The files the unattend CD carries out of the repo, relative to `vm/windows/`.
///
/// The same list the HCL template's `cd_files` block names, and
/// `the_cd_carries_what_the_template_carries` reads both and compares them:
/// where two artifacts have to agree and only a convention connects them, a test
/// has to read both. Windows Setup searches the root of every removable drive
/// for `Autounattend.xml`, so the CD is flat and the directories here are only
/// where the files live in the repo.
pub const CD_FILES: [&str; 4] = [
    "autounattend/Autounattend.xml",
    "scripts/bootstrap.ps1",
    "scripts/run-job.cmd",
    "scripts/session-ready.cmd",
];

/// The public half of the key pair both guests trust, under the name
/// `bootstrap.ps1` copies into `administrators_authorized_keys`.
pub const CD_AUTHORIZED_KEYS: &str = "authorized_keys";

/// How the first-logon command finds this CD without knowing its drive letter.
pub const CD_MARKER: &str = "sunlit-e2e-media.marker";
pub const CD_MARKER_CONTENT: &str = "sunlit-e2e";

/// The volume label of the unattend CD. Nothing depends on it; it is what makes
/// the disc recognizable in the guest's own explorer.
pub const CD_LABEL: &str = "SUNLIT_E2E";

/// Where the CD is staged inside the build directory, and what it is packed
/// into. Both are consumed by the install and removed once it has worked.
pub const UNATTEND_STAGING: &str = "unattend";
pub const UNATTEND_ISO: &str = "unattend.iso";

/// The script the install's last step runs, relative to `vm/windows/`.
pub const FINALIZE_SCRIPT: &str = "scripts/finalize.ps1";

/// The last scheduled task `bootstrap.ps1` registers.
///
/// Which is why it answers "has the first-logon bootstrap finished": SSH exists
/// from the middle of that script, so a guest that answers is not yet a guest
/// whose image is complete. `the_bootstrap_probe_waits_for_the_last_thing_bootstrap_does`
/// pins that this task really is the last one.
pub const SESSION_READY_TASK: &str = "sunlit-e2e-session-ready";

/// What the bootstrap probe prints when the task exists.
pub const BOOTSTRAP_DONE_MARKER: &str = "SUNLIT_BOOTSTRAP_DONE";

/// The disk the install writes into. The same 64 GB the Packer template asks
/// for, dynamic, so the file is the size of what Windows actually wrote.
pub const BUILD_DISK_BYTES: u64 = 64 * 1024 * 1024 * 1024;

/// How long the whole install may take before the build gives up on it.
///
/// Two hours, which is the `ssh_timeout` the Packer template used, and a guess
/// until a live run calibrates it: an install that reaches SSH in half an hour
/// makes this a bound on a build that has gone wrong rather than a deadline a
/// healthy one approaches.
pub const INSTALL_DEADLINE: Duration = Duration::from_secs(2 * 60 * 60);

/// How long the first-logon bootstrap may take after SSH answers.
///
/// It answers from the middle of that script, so what is left is a handful of
/// registry writes and two scheduled tasks.
pub const BOOTSTRAP_DEADLINE: Duration = Duration::from_secs(20 * 60);

/// How long the guest may take to shut itself down.
pub const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(30 * 60);

/// How often the install's signals are read.
pub const POLL: Duration = Duration::from_secs(10);

/// How many readings in a row may say the guest is not `Running` before the
/// build treats the install as over.
///
/// `Start-VM` returns before the guest is `Running`, and `Get-VM` reports
/// `Starting` in between, so one such reading is not an install that ended. Two
/// of them a poll apart is, because a `Hyper-V` guest stays `Running` through
/// the reboots an install performs. The same shape as `build_watch`'s
/// stopped-twice rule and for the same reason: one sample of a state a machine
/// passes through is not a verdict, and ending a healthy build on it would cost
/// the whole install.
pub const NOT_RUNNING_SAMPLES: u32 = 2;

/// How many probes in a row may fail before the build stops believing it can
/// see the VM at all.
///
/// One failed `Get-VM` is a hiccup and the install is unaffected by it. A minute
/// of them is the hypervisor or the group membership going away underneath, and
/// waiting out the install deadline to report that would be the hour-and-a-quarter
/// silence deviation 24 exists to prevent.
pub const PROBE_FAILURES_ALLOWED: u32 = 6;

/// The command that shuts the guest down, the same one the template used.
///
/// `p:4:1` is "planned, application, maintenance", which stops Windows asking
/// the next boot why it went down.
pub const SHUTDOWN_COMMAND: &str = "shutdown /s /t 5 /f /d p:4:1";

/// What the manifest records as having built the image.
///
/// `packer <version>` does not apply on this path, and "the mechanism" is the
/// useful thing to know when an image behaves oddly: the two builders produce
/// the same Windows through very different installs.
pub fn builder_label() -> String {
    format!(
        "hyper-v native install, xtask {}",
        env!("CARGO_PKG_VERSION")
    )
}

/// The script that creates the build VM.
///
/// Generation 2 because Windows 11 needs UEFI, and because a generation 2 VM
/// keeps its UEFI variable store: Setup's first phase writes a Windows Boot
/// Manager entry and puts it first, so the reboots partway through the install
/// go to the disk rather than back into Setup. That is the one assumption in
/// this path without a measurement behind it, which is why `probe_script`
/// reports the first boot entry and the build prints it when it changes.
///
/// Secure Boot is off, matching the runtime VMs and the image's own history: it
/// was installed with the requirement bypassed, and turning it on here would
/// assert something about the disk that was never true.
///
/// Both DVDs are attached and the install one boots first. The second carries
/// the unattend file and everything the first-logon script needs; Setup finds it
/// by searching every removable drive, and the bootstrap finds it by its marker
/// file.
pub fn create_script(name: &str, disk: &Path, install_iso: &Path, unattend_iso: &Path) -> String {
    format!(
        "New-VHD -Path {disk} -SizeBytes {size} -Dynamic | Out-Null\n\
         New-VM -Name {name} -Generation 2 -MemoryStartupBytes {memory} \
         -VHDPath {disk} -SwitchName {switch} | Out-Null\n\
         Set-VM -Name {name} -ProcessorCount {cpus} -AutomaticCheckpointsEnabled $false \
         -AutomaticStartAction Nothing -AutomaticStopAction TurnOff\n\
         Set-VMMemory -VMName {name} -DynamicMemoryEnabled $false\n\
         Set-VMFirmware -VMName {name} -EnableSecureBoot Off\n\
         Add-VMDvdDrive -VMName {name} -Path {install}\n\
         Add-VMDvdDrive -VMName {name} -Path {unattend}\n\
         $dvd = Get-VMDvdDrive -VMName {name} | \
         Where-Object {{ $_.Path -eq {install} }} | Select-Object -First 1\n\
         if (-not $dvd) {{ throw 'the installation media is not attached to the VM' }}\n\
         Set-VMFirmware -VMName {name} -FirstBootDevice $dvd\n",
        name = ps_quote(name),
        disk = ps_quote(disk),
        install = ps_quote(install_iso),
        unattend = ps_quote(unattend_iso),
        switch = ps_quote(hyperv::SWITCH),
        size = BUILD_DISK_BYTES,
        memory = hyperv::MEMORY_BYTES,
        cpus = hyperv::CPUS,
    )
}

/// The script that reads everything the build watches, in one process.
///
/// Three signals rather than three invocations: at one poll every ten seconds
/// for two hours, a `powershell.exe` per signal is a cost worth not paying.
///
/// The boot entry is here because of the assumption `create_script` describes.
/// A `Drive` entry naming the DVD is the state the VM was created in; a `File`
/// entry is Setup's own boot manager, and its appearance is the assumption
/// holding.
pub fn probe_script(name: &str) -> String {
    hyperv::query_script(
        name,
        "if ($vm) {\n  \
         Write-Output \"STATE=$($vm.State)\"\n  \
         $order = @($vm | Get-VMFirmware | ForEach-Object { $_.BootOrder })\n  \
         if ($order.Count -gt 0) {\n    \
         $first = $order[0]\n    \
         $what = [string]$first.BootType\n    \
         if ($first.Device) { $what = \"$what $($first.Device.Name)\" }\n    \
         Write-Output \"BOOT=$what\"\n  \
         }\n  \
         $vm | Get-VMNetworkAdapter | ForEach-Object { $_.IPAddresses } | \
         ForEach-Object { Write-Output \"IP=$_\" }\n\
         }\n",
    )
}

/// The script that drops the VM and keeps its disk.
///
/// `Remove-VM` never deletes a VHDX, which is the whole point here: the disk is
/// the build's product and is about to become the golden image. It refuses to
/// act on a VM that is not off, because removing a running one would leave the
/// disk mid-write.
pub fn remove_keeping_disk_script(name: &str) -> String {
    hyperv::query_script(
        name,
        &format!(
            "if ($vm) {{\n  \
             if ($vm.State -ne 'Off') {{ throw \"$($vm.Name) is $($vm.State), not Off\" }}\n  \
             Remove-VM -Name {name} -Force\n\
             }}\n",
            name = ps_quote(name)
        ),
    )
}

/// The command that answers whether the first-logon bootstrap has finished.
///
/// `cmd.exe` is the guest's SSH shell and the golden image leaves it that way,
/// so this is batch rather than `PowerShell`, and `schtasks` reports a missing
/// task with an exit code rather than with an exception.
pub fn bootstrap_done_command() -> String {
    format!("schtasks /query /tn {SESSION_READY_TASK} >nul 2>&1 && echo {BOOTSTRAP_DONE_MARKER}")
}

/// Where `finalize.ps1` is copied to inside the guest, and the command that
/// runs it there.
///
/// Copied rather than encoded onto the command line: encoded, the script is
/// about twelve kilobytes, and `cmd.exe` truncates a command line at eight.
pub fn finalize_destination() -> String {
    format!(r"{}\finalize.ps1", crate::provider::GUEST_ROOT_WINDOWS)
}

pub fn finalize_command(destination: &str) -> String {
    format!("powershell.exe -NoProfile -ExecutionPolicy Bypass -File {destination}")
}

/// One reading of the build VM.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    /// Whether the script ran to the end. Without this, a VM that is gone and a
    /// hypervisor that stopped answering look the same, which is the
    /// distinction [`hyperv::QUERY_OK`] exists for.
    pub answered: bool,
    /// `Get-VM`'s own word for the state, or `None` if there is no such VM.
    pub state: Option<String>,
    /// The first entry in the firmware's boot order, as a short description.
    pub boot: Option<String>,
    pub addresses: Vec<String>,
}

impl Probe {
    pub fn parse(stdout: &str) -> Self {
        Self {
            answered: hyperv::answered(stdout),
            state: hyperv::parse_state(stdout),
            boot: util::marker(stdout, "BOOT="),
            addresses: hyperv::parse_addresses(stdout),
        }
    }

    pub fn running(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|state| state.eq_ignore_ascii_case("Running"))
    }

    /// How the guest looks to the heartbeat.
    ///
    /// A `Hyper-V` guest stays `Running` through the reboots an install
    /// performs, so anything else mid-install means the install ended. That is
    /// reported rather than resumed: the resume and give-up arms of the
    /// heartbeat answer a stopped QEMU machine, which this path never has
    /// (amendment decision 18).
    pub fn guest(&self) -> Guest {
        match &self.state {
            Some(state) if state.eq_ignore_ascii_case("Running") => Guest::Running,
            Some(state) => Guest::Off {
                state: state.clone(),
            },
            None => Guest::Off {
                state: "not registered".to_owned(),
            },
        }
    }
}

/// Build the unattend CD from the shipped files plus the two generated ones.
pub fn stage_unattend_cd(
    template_dir: &Path,
    staging: &Path,
    public_key: &str,
) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(staging);
    std::fs::create_dir_all(staging)
        .map_err(|e| format!("cannot create {}: {e}", staging.display()))?;
    for relative in CD_FILES {
        let from = template_dir.join(relative);
        let name = Path::new(relative)
            .file_name()
            .ok_or_else(|| format!("{relative} names no file"))?;
        std::fs::copy(&from, staging.join(name))
            .map_err(|e| format!("cannot copy {} onto the unattend CD: {e}", from.display()))?;
    }
    std::fs::write(
        staging.join(CD_AUTHORIZED_KEYS),
        format!("{}\n", public_key.trim()),
    )
    .map_err(|e| format!("cannot write the authorized key: {e}"))?;
    std::fs::write(staging.join(CD_MARKER), CD_MARKER_CONTENT)
        .map_err(|e| format!("cannot write the media marker: {e}"))
}

/// Stage the CD and pack it into an ISO.
fn build_unattend_cd(
    runner: &dyn Runner,
    tool: &Path,
    template_dir: &Path,
    build_dir: &Path,
    public_key: &str,
) -> Result<PathBuf, String> {
    let staging = build_dir.join(UNATTEND_STAGING);
    let iso = build_dir.join(UNATTEND_ISO);
    stage_unattend_cd(template_dir, &staging, public_key)?;
    let _ = std::fs::remove_file(&iso);
    let command = windows_media::oscdimg_command(tool, &staging, &iso, Some(CD_LABEL), None);
    let out = runner
        .capture(&command)
        .map_err(|e| format!("cannot run {}: {e}", tool.display()))?;
    if !out.success() {
        return Err(format!(
            "{} exited {:?} building the unattend CD: {}",
            tool.display(),
            out.code,
            out.stderr.trim()
        ));
    }
    if !iso.is_file() {
        return Err(format!(
            "{} reported success and produced no {}",
            tool.display(),
            iso.display()
        ));
    }
    println!(
        "unattend CD at {} ({})",
        iso.display(),
        format_bytes(std::fs::metadata(&iso).map(|m| m.len()).unwrap_or(0))
    );
    Ok(iso)
}

/// The record of the build VM, written before the VM exists.
///
/// Earlier than the runtime path writes its own, and deliberately: between
/// `New-VM` and the record there is a window in which a VM exists that nothing
/// can find again, and the record costs nothing to write early because
/// everything in it is decided in advance.
fn build_state(store: &Store, target: Target) -> RunState {
    let mut state = RunState::new(
        target,
        ProviderKind::HyperV,
        store.build_disk(target),
        StartReason::Build,
        util::now_unix(),
    );
    state.ssh_port = 22;
    hyperv::GUEST_USER.clone_into(&mut state.ssh_user);
    state
}

/// Run a `PowerShell` script, mapping a failure to something a reader can act
/// on.
///
/// Not "Hyper-V refused". `powershell` prepends `$ErrorActionPreference =
/// 'Stop'`, so a script ends at its first error record whether that came from a
/// cmdlet or from the script's own `throw`, and both arrive here as a nonzero
/// exit with the message on stderr. The text carries the message rather than
/// guessing which of the two produced it.
fn run_script(runner: &dyn Runner, script: &str) -> Result<String, String> {
    let out = runner
        .capture(&powershell(script))
        .map_err(|e| format!("cannot run powershell.exe: {e}"))?;
    if out.success() {
        Ok(out.stdout)
    } else {
        Err(format!(
            "the Hyper-V script exited {:?}: {}",
            out.code,
            out.stderr.trim()
        ))
    }
}

/// Whether the build VM exists, which is what a failure has to say something
/// about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presence {
    /// Registered, so it is holding memory and a disk whatever state it is in.
    Registered,
    /// No VM of that name is registered. Which of "never created" and "created
    /// and gone again" that is cannot be read from one query, and nothing that
    /// reports it needs to know: nothing of this build is running either way.
    Gone,
    /// `Hyper-V` would not answer, which is its own thing to report rather than
    /// either of the above.
    Unknown { why: String },
}

/// Read a state query's answer as a presence.
///
/// The marker is what separates the two absences: a query that ran and found no
/// such VM says the VM is gone, and one that never ran says nothing at all
/// (departure 35).
pub fn presence_of(answer: &Result<String, String>) -> Presence {
    match answer {
        Ok(stdout) if hyperv::answered(stdout) => {
            if hyperv::parse_state(stdout).is_some() {
                Presence::Registered
            } else {
                Presence::Gone
            }
        }
        Ok(stdout) => Presence::Unknown {
            why: format!("the query did not run to the end: {}", stdout.trim()),
        },
        Err(e) => Presence::Unknown { why: e.clone() },
    }
}

/// Ask the hypervisor whether the build VM is there.
fn presence(runner: &dyn Runner, name: &str) -> Presence {
    presence_of(&run_script(runner, &hyperv::state_script(name)))
}

/// What is still there and how to get at it, for every failure from the moment
/// the VM could have been created.
///
/// A VM that exists is kept rather than removed: a failed install is the one
/// thing worth looking inside, `vm view` reaches it while it runs, and the
/// record written before it existed means `vm status` shows it and `vm down`
/// removes it along with the unfinished disk.
///
/// The three cases are told apart rather than assumed, because the create script
/// stops at its first failure and both halves of it are reachable: `New-VHD`
/// failing leaves nothing, and anything after `New-VM` leaves a registered VM,
/// including the script's own `throw` for media that would not attach.
///
/// [`Presence::Gone`] is reached from both ends of a build: a create that never
/// registered anything, and a VM that disappeared out from under an install the
/// watcher or the shutdown was in the middle of. One query cannot say which, so
/// this says what the query answered instead of picking one.
fn aftermath(presence: &Presence, target: Target, state: &RunState) -> String {
    let vm = &state.vm_name;
    match presence {
        Presence::Registered => format!(
            "{vm} is still there, with the disk it was installing onto.\n  \
             desktop: cargo xtask vm view {target}\n  \
             ssh:     cargo xtask vm ssh {target}\n  \
             down:    cargo xtask vm down {target}  (removes the VM and the unfinished disk)"
        ),
        Presence::Gone => format!(
            "no VM called {vm} is registered with Hyper-V now, so nothing of this \
             build is running.\n  \
             down:    cargo xtask vm down {target}  (clears the record and any disk \
             that was made for it)"
        ),
        Presence::Unknown { why } => format!(
            "whether {vm} exists cannot be read from Hyper-V ({why}), so it may be \
             running with the disk it was installing onto.\n  \
             status:  cargo xtask vm status\n  \
             down:    cargo xtask vm down {target}  (removes the VM and the unfinished disk)"
        ),
    }
}

/// Everything this path needs, and what to say when it is missing.
fn preflight(runner: &dyn Runner, store: &Store) -> Result<(), String> {
    let host = HostOs::Windows;
    for tool in build_image::required_tools(build_image::Builder::HyperV) {
        if crate::host::facts::resolve_tool(runner, tool, host).is_none() {
            return Err(format!(
                "{tool} is not available, and this build needs it; \
                 run `cargo xtask vm doctor` for the whole list"
            ));
        }
    }

    // The membership is granted at logon, not when `vm setup` adds it, so a
    // session that has not signed out since is a session whose token does not
    // have it. Without this the first cmdlet fails with a bare access denial,
    // which names neither the group nor the relogin.
    let facts = crate::host::facts::collect(runner, host, store.root());
    let windows = facts.windows.unwrap_or_default();
    if !windows.in_hyperv_admins && !windows.elevated {
        return Err(
            "this session is not a member of the Hyper-V Administrators group, and \
             creating a VM needs it.\n\
             `cargo xtask vm setup` adds you, and the membership only reaches your \
             token after signing out and back in. `cargo xtask vm doctor` confirms it."
                .to_owned(),
        );
    }
    Ok(())
}

/// Build the Windows golden image on `Hyper-V`.
pub fn run(runner: &dyn Runner, store: &Store, target: Target) -> Result<u8, String> {
    preflight(runner, store)?;

    let template_dir = store::template_dir(target);
    if !template_dir.is_dir() {
        return Err(format!(
            "no templates at {}; the repo is where they live",
            template_dir.display()
        ));
    }
    let oscdimg =
        crate::host::facts::resolve_tool(runner, windows_media::ISO_BUILDER, HostOs::Windows)
            .ok_or_else(|| format!("{} is not available", windows_media::ISO_BUILDER))?;

    // One VM at a time, and nothing of ours left running: the build VM carries
    // the same name a runtime guest does, so both questions are the ones the
    // boot path already asks.
    vm::check_no_other_vm(runner, store, target)?;
    vm::clear_stale_state(runner, store, target)?;

    let build_dir = store.build_dir(target);
    std::fs::create_dir_all(&build_dir)
        .map_err(|e| format!("cannot create {}: {e}", build_dir.display()))?;

    println!("building the {target} golden image on Hyper-V, with no builder in the loop");
    println!(
        "  install:  a generation 2 VM, {} vCPUs, {}",
        hyperv::CPUS,
        format_bytes(hyperv::MEMORY_BYTES)
    );
    println!("  disk:     {}", store.build_disk(target).display());
    println!("  media:    {}", store.iso_dir().display());
    println!("  this takes tens of minutes and downloads several gigabytes the first time");

    let public_key = build_image::ensure_ssh_key(runner, store)?;
    let media = windows_media::ensure_install_media(runner, store, &build_dir)?;
    let unattend = build_unattend_cd(runner, &oscdimg, &template_dir, &build_dir, &public_key)?;

    let mut state = build_state(store, target);
    let disk = state.overlay.clone();
    let _ = std::fs::remove_file(&disk);
    vm::write_state(store, target, &state)?;

    // From here on a VM may exist, so from here on every failure says what is
    // there. Including the one that creates it: the create script is one
    // `powershell.exe` that stops at its first failure, so every statement
    // after `New-VM` leaves a registered VM behind.
    create_and_install(
        runner,
        store,
        target,
        &mut state,
        &template_dir,
        &media.boot,
        &unattend,
    )?;

    let source = media.source.to_string_lossy().into_owned();
    let code = finish(runner, store, target, &disk, &template_dir, &source)?;
    clean_up_build_dir(&build_dir);
    Ok(code)
}

/// Create the VM and install Windows into it, naming what is left behind if
/// anything in there fails.
///
/// One boundary rather than two, because "the VM exists" is not a line the
/// caller can draw: the create script's own failures are on both sides of it.
fn create_and_install(
    runner: &dyn Runner,
    store: &Store,
    target: Target,
    state: &mut RunState,
    template_dir: &Path,
    install_iso: &Path,
    unattend_iso: &Path,
) -> Result<(), String> {
    let disk = state.overlay.clone();
    let create = create_script(&state.vm_name, &disk, install_iso, unattend_iso);
    let outcome = match run_script(runner, &create) {
        Ok(_) => {
            println!("{} is created", state.vm_name);
            install_windows(runner, store, target, state, template_dir)
        }
        Err(e) => Err(e),
    };
    outcome.map_err(|e| {
        let presence = presence(runner, &state.vm_name);
        format!("{e}\n{}", aftermath(&presence, target, state))
    })
}

/// Remove what the install consumed, once it has worked.
///
/// The staging tree and the unattend ISO exist to be read by Windows Setup and
/// are worth nothing afterwards, so a successful build leaves neither: `vm
/// status` would otherwise report build leftovers for a target whose build
/// finished. The cached media is not touched, because it lives in the media
/// directory and is worth its download.
fn clean_up_build_dir(build_dir: &Path) {
    let _ = std::fs::remove_dir_all(build_dir.join(UNATTEND_STAGING));
    let _ = std::fs::remove_file(build_dir.join(UNATTEND_ISO));
}

/// Start the VM, watch it install, and leave it shut down with its disk intact.
///
/// One function so that every step from "the VM is running" to "the VM is gone"
/// is behind one boundary the caller turns into a message.
fn install_windows(
    runner: &dyn Runner,
    store: &Store,
    target: Target,
    state: &mut RunState,
    template_dir: &Path,
) -> Result<(), String> {
    run_script(
        runner,
        &format!("Start-VM -Name {}", ps_quote(&state.vm_name)),
    )?;
    state.started_unix = util::now_unix();
    vm::write_state(store, target, state)?;
    println!(
        "{} is running; Windows Setup takes it from here",
        state.vm_name
    );

    watch_install(runner, store, target, state, POLL)?;
    wait_for_bootstrap(runner, store, state)?;
    finalize(runner, store, state, template_dir)?;
    shut_down(runner, store, state)?;

    run_script(runner, &remove_keeping_disk_script(&state.vm_name))?;
    // Confirmed rather than assumed: the script tolerates a VM that is already
    // gone, so its success alone does not prove this one is. The marker is what
    // makes "no such VM" an answer rather than a failed query, which is the
    // distinction that ended the first live build here.
    let after = run_script(runner, &hyperv::state_script(&state.vm_name))?;
    if !hyperv::answered(&after) || hyperv::parse_state(&after).is_some() {
        return Err(format!(
            "{} may still be registered after Remove-VM, so its disk cannot be \
             moved out from under it. Hyper-V said: {}",
            state.vm_name,
            after.trim()
        ));
    }
    println!("{} is removed and its disk is kept", state.vm_name);
    // The VM is gone and the disk is about to move: the record describes
    // neither any more. Deleted after the removal succeeded, never before, so a
    // failure above leaves something `vm status` can still see.
    let _ = std::fs::remove_file(store.state_file(target));
    Ok(())
}

/// Watch the install until the guest answers on SSH.
///
/// The heartbeat is the same `Trend` the Packer path uses, with the signals
/// `Hyper-V` has in place of QEMU's: `Get-VM`'s state instead of the monitor's,
/// and the growing VHDX instead of the growing qcow2. No keypress, because the
/// media has no prompt, and no screenshots in this pass.
///
/// `poll` is [`POLL`] in a build; a test turns it down so the loop can be
/// exercised without waiting out real seconds, the same way `build_watch::Watch`
/// carries its own.
fn watch_install(
    runner: &dyn Runner,
    store: &Store,
    target: Target,
    state: &mut RunState,
    poll: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    let mut trend = Trend::new();
    let mut boot: Option<String> = None;
    let mut failures = 0;
    let mut not_running = 0;
    let mut addressed = false;
    let disk = state.overlay.clone();

    loop {
        match run_script(runner, &probe_script(&state.vm_name)) {
            Ok(stdout) => {
                let probe = Probe::parse(&stdout);
                // A script that exited zero without finishing is a probe that
                // failed, not a VM that is gone: the two are the same absence
                // of a STATE line, and only one of them means the install is
                // over.
                if !probe.answered {
                    failures += 1;
                    report(
                        "progress",
                        &format!("the VM query did not run to the end: {}", stdout.trim()),
                    );
                    if failures >= PROBE_FAILURES_ALLOWED {
                        return Err(unreadable(&state.vm_name, failures, STILL_INSTALLING));
                    }
                    std::thread::sleep(poll);
                    continue;
                }
                failures = 0;
                if probe.state.is_none() {
                    return Err(format!(
                        "{} is no longer registered with Hyper-V, and this build did \
                         not remove it. Nothing of the install survives that.",
                        state.vm_name
                    ));
                }

                if probe.boot != boot && probe.boot.is_some() {
                    boot.clone_from(&probe.boot);
                    if let Some(entry) = &boot {
                        report("firmware", &format!("first boot entry is {entry}"));
                    }
                }

                if !addressed && let Some(address) = hyperv::first_usable_ipv4(&probe.addresses) {
                    addressed = true;
                    state.ssh_host.clone_from(&address);
                    vm::write_state(store, target, state)?;
                    report(
                        "guest",
                        &format!("{} is at {address}, so Windows is up", state.vm_name),
                    );
                }

                let sample = Sample {
                    elapsed: started.elapsed(),
                    disk: std::fs::metadata(&disk).map_or(0, |meta| meta.len()),
                    guest: probe.guest(),
                    screen: None,
                };
                match trend.observe(&sample) {
                    Action::Quiet => {}
                    // Only `Say` is reachable: the other two answer a stopped
                    // QEMU machine, and `Probe::guest` never reports one.
                    Action::Say(text) | Action::Resume(text) | Action::GiveUp(text) => {
                        report("progress", &text);
                    }
                }

                // Two readings rather than one. `Start-VM` returns before the
                // guest is `Running`, so a `Starting` seen on the first poll is
                // a machine on its way up rather than an install that ended,
                // and ending the build there would throw away a healthy one.
                if probe.running() {
                    not_running = 0;
                } else {
                    not_running += 1;
                    if not_running >= NOT_RUNNING_SAMPLES {
                        return Err(format!(
                            "{} is {} on {} in a row after {}, and a Hyper-V guest stays \
                             running through the reboots an install performs. The install \
                             ended without reaching SSH; {} is the disk it wrote.",
                            state.vm_name,
                            probe.state.unwrap_or_default(),
                            util::count(not_running as usize, "reading"),
                            format_duration(started.elapsed()),
                            disk.display()
                        ));
                    }
                }

                if addressed && ssh::probe_ready(runner, &ssh_target(store, state)).is_ok() {
                    report(
                        "guest",
                        &format!("SSH answered after {}", format_duration(started.elapsed())),
                    );
                    return Ok(());
                }
            }
            Err(e) => {
                failures += 1;
                report("progress", &format!("cannot read the VM ({e})"));
                if failures >= PROBE_FAILURES_ALLOWED {
                    return Err(unreadable(&state.vm_name, failures, STILL_INSTALLING));
                }
            }
        }

        if started.elapsed() >= INSTALL_DEADLINE {
            return Err(format!(
                "the install did not reach SSH within {}. `cargo xtask vm view windows` \
                 shows what the guest is doing.",
                format_duration(INSTALL_DEADLINE)
            ));
        }
        std::thread::sleep(poll);
    }
}

/// What to say when the hypervisor stops answering questions about the guest.
///
/// `doing` is what the guest was in the middle of, because that is what is now
/// unknown: the sentence is the same either way and the consequence is not.
fn unreadable(name: &str, failures: u32, doing: &str) -> String {
    format!(
        "Hyper-V stopped answering questions about {name} ({} in a row). {doing}",
        util::count(failures as usize, "failed query")
    )
}

/// The install's half of that sentence.
const STILL_INSTALLING: &str = "The install may still be running; nothing here can tell.";

/// The shutdown's half.
const STILL_SHUTTING_DOWN: &str = "The guest may still be shutting down; nothing here can tell, \
                                   and its disk cannot be moved until it is off.";

/// Wait for the first-logon bootstrap to finish.
///
/// SSH exists from the middle of `bootstrap.ps1`, three steps in of nine, so a
/// guest that answers is not a guest whose scheduled tasks are registered yet.
/// Without this the next step is a race that `finalize.ps1` loses by reporting a
/// missing task, which reads like a broken image rather than an early question.
fn wait_for_bootstrap(runner: &dyn Runner, store: &Store, state: &RunState) -> Result<(), String> {
    let started = Instant::now();
    let command = bootstrap_done_command();
    let target = ssh_target(store, state);
    loop {
        if let Ok(out) = ssh::exec(runner, &target, &command)
            && out.stdout.contains(BOOTSTRAP_DONE_MARKER)
        {
            report(
                "guest",
                &format!(
                    "the first-logon bootstrap finished after {}",
                    format_duration(started.elapsed())
                ),
            );
            return Ok(());
        }
        if started.elapsed() >= BOOTSTRAP_DEADLINE {
            return Err(format!(
                "the guest answered on SSH but its first-logon bootstrap did not \
                 finish within {}. Its transcript is at {}\\bootstrap.log inside the \
                 guest, which `cargo xtask vm ssh windows` reaches.",
                format_duration(BOOTSTRAP_DEADLINE),
                crate::provider::GUEST_ROOT_WINDOWS
            ));
        }
        std::thread::sleep(POLL);
    }
}

/// Copy `finalize.ps1` in and run it.
fn finalize(
    runner: &dyn Runner,
    store: &Store,
    state: &RunState,
    template_dir: &Path,
) -> Result<(), String> {
    let script = template_dir.join(FINALIZE_SCRIPT);
    if !script.is_file() {
        return Err(format!("no {} to finish the image with", script.display()));
    }
    let target = ssh_target(store, state);
    let destination = finalize_destination();
    println!("finishing the image with {}", script.display());

    let copy = ssh::scp_to_command(&target, &script, &destination);
    let out = runner
        .capture(&copy)
        .map_err(|e| format!("cannot run scp: {e}"))?;
    if !out.success() {
        return Err(format!(
            "copying {} into the guest failed: {}",
            script.display(),
            out.stderr.trim()
        ));
    }

    let out = ssh::exec(runner, &target, &finalize_command(&destination))?;
    print!("{}", out.stdout);
    if !out.success() {
        return Err(format!(
            "finalize.ps1 exited {:?} inside the guest, so the image is not usable. \
             Its output is above; {} has the rest.",
            out.code,
            out.stderr.trim()
        ));
    }
    Ok(())
}

/// What one reading of the VM says about a shutdown in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shutting {
    /// It is off, so the disk is finished and safe to move.
    Off,
    /// Still on, in whatever state `Get-VM` gave.
    Waiting(String),
    /// The query ran and found no such VM, which nothing here did.
    Gone,
    /// The query itself did not run, so this reading says nothing.
    Unreadable,
}

/// Read a state query's answer as the state of a shutdown.
///
/// The marker check is what makes `Gone` mean it: a query that never ran has no
/// `STATE=` line either, and reading that as a VM that vanished would report a
/// build's disk as lost while the guest was shutting down normally. Departure 35
/// is this distinction, and this was the one query site not making it.
pub fn shutting(stdout: &str) -> Shutting {
    let probe = Probe::parse(stdout);
    if !probe.answered {
        return Shutting::Unreadable;
    }
    match probe.state {
        None => Shutting::Gone,
        Some(state) if state.eq_ignore_ascii_case("Off") => Shutting::Off,
        Some(state) => Shutting::Waiting(state),
    }
}

/// Shut the guest down and wait for the VM to be off.
fn shut_down(runner: &dyn Runner, store: &Store, state: &RunState) -> Result<(), String> {
    println!("shutting the guest down");
    // The connection dies with the session this starts, so a failure here says
    // nothing: the state of the VM is the only answer that counts.
    let _ = ssh::exec(runner, &ssh_target(store, state), SHUTDOWN_COMMAND);

    let started = Instant::now();
    let mut failures = 0;
    loop {
        let reading = match run_script(runner, &hyperv::state_script(&state.vm_name)) {
            Ok(stdout) => shutting(&stdout),
            Err(e) => {
                report("progress", &format!("cannot read the VM ({e})"));
                Shutting::Unreadable
            }
        };
        match reading {
            Shutting::Gone => {
                return Err(format!(
                    "{} vanished while shutting down; its disk is whatever the \
                     install left behind.",
                    state.vm_name
                ));
            }
            Shutting::Off => {
                report(
                    "guest",
                    &format!("off after {}", format_duration(started.elapsed())),
                );
                return Ok(());
            }
            // A guest that is shutting down is one the hypervisor is busy with,
            // so a failed or half-finished query is tolerated the same handful
            // of times the install watcher tolerates it, rather than ending a
            // build that is seconds from done.
            Shutting::Unreadable => {
                failures += 1;
                if failures >= PROBE_FAILURES_ALLOWED {
                    return Err(unreadable(&state.vm_name, failures, STILL_SHUTTING_DOWN));
                }
            }
            Shutting::Waiting(_) => failures = 0,
        }
        if started.elapsed() >= SHUTDOWN_DEADLINE {
            return Err(format!(
                "{} did not shut down within {}. Its disk is mid-write, so nothing \
                 here will move it.",
                state.vm_name,
                format_duration(SHUTDOWN_DEADLINE)
            ));
        }
        std::thread::sleep(POLL);
    }
}

/// Where the guest is and how to authenticate to it.
fn ssh_target(store: &Store, state: &RunState) -> SshTarget {
    SshTarget::from_state(state, store.ssh_key())
}

/// Move the disk into the image store, derive the qcow2, and write the manifest.
///
/// The conversion direction is the flip decision 10's intent survives: on this
/// host the primary provider is `Hyper-V`, so the install produces the VHDX
/// natively and the qcow2 is derived from it for the QEMU override cell. One
/// canonical install, two formats, either way round.
fn finish(
    runner: &dyn Runner,
    store: &Store,
    target: Target,
    built: &Path,
    template_dir: &Path,
    source: &str,
) -> Result<u8, String> {
    let image_dir = store.image_dir(target);
    std::fs::create_dir_all(&image_dir)
        .map_err(|e| format!("cannot create {}: {e}", image_dir.display()))?;

    let vhdx = store.vhdx(target);
    if !built.is_file() {
        return Err(format!(
            "the install finished and produced no {}",
            built.display()
        ));
    }
    build_image::move_file(built, &vhdx)?;
    let mut images = vec![build_image::record(&vhdx)?];

    let qcow2 = store.qcow2(target);
    println!("converting to qcow2 for the QEMU provider");
    build_image::convert(runner, &vhdx, &qcow2, "qcow2")?;
    images.push(build_image::record(&qcow2)?);

    build_image::write_manifest(
        store,
        target,
        template_dir,
        &images,
        source.to_owned(),
        builder_label(),
    )?;
    // The host keys the new image answers with are forgotten by the caller,
    // which does it on the failure path too: a build that got as far as an SSH
    // session and then failed has already written an entry that is wrong.
    build_image::announce(target, &images);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::CommandOutput;
    use crate::runner::fake::FakeRunner;

    const NAME: &str = "sunlit-e2e-windows";

    fn create() -> String {
        create_script(
            NAME,
            Path::new(r"C:\vm\run\windows\build.vhdx"),
            Path::new(r"C:\vm\iso\noprompt.iso"),
            Path::new(r"C:\vm\build\windows\unattend.iso"),
        )
    }

    #[test]
    fn the_build_vm_is_the_shape_the_amendment_specifies() {
        let script = create();
        assert!(script.contains("-Generation 2"), "{script}");
        assert!(script.contains("-ProcessorCount 4"), "{script}");
        assert!(
            script.contains(&format!(
                "-MemoryStartupBytes {}",
                6_u64 * 1024 * 1024 * 1024
            )),
            "{script}"
        );
        assert!(script.contains("-DynamicMemoryEnabled $false"), "{script}");
        assert!(script.contains("-SwitchName 'Default Switch'"), "{script}");
        assert!(script.contains("-EnableSecureBoot Off"), "{script}");
        assert!(
            script.contains("-AutomaticCheckpointsEnabled $false"),
            "{script}"
        );
        assert!(script.contains("-AutomaticStartAction Nothing"), "{script}");
        // A fresh disk, not a differencing child of anything: there is no
        // golden image yet, and this is what becomes one.
        assert!(script.contains("-Dynamic"), "{script}");
        assert!(!script.contains("-Differencing"), "{script}");
        assert!(
            script.contains(&format!("-SizeBytes {BUILD_DISK_BYTES}")),
            "{script}"
        );
    }

    #[test]
    fn both_dvds_are_attached_and_the_installer_boots_first() {
        let script = create();
        assert_eq!(script.matches("Add-VMDvdDrive").count(), 2, "{script}");
        assert!(script.contains(r"'C:\vm\iso\noprompt.iso'"), "{script}");
        assert!(
            script.contains(r"'C:\vm\build\windows\unattend.iso'"),
            "{script}"
        );
        // The first boot device is the install medium, found by its path rather
        // than by position: two DVD drives come back in whatever order the
        // cmdlet feels like.
        assert!(script.contains("-FirstBootDevice $dvd"), "{script}");
        assert!(script.contains("Select-Object -First 1"), "{script}");
        // And a build that could not attach it fails now rather than booting
        // into an empty firmware and waiting out the install deadline.
        assert!(script.contains("throw"), "{script}");
    }

    #[test]
    fn every_build_script_names_a_vm_of_ours() {
        for script in [
            create(),
            probe_script(NAME),
            remove_keeping_disk_script(NAME),
        ] {
            assert!(script.contains(NAME), "{script}");
            // Never a blanket query or removal.
            assert!(!script.contains("Get-VM |"), "{script}");
        }
        assert_eq!(NAME, Target::Windows.vm_name());
    }

    #[test]
    fn a_name_with_an_apostrophe_cannot_break_out_of_a_build_script() {
        let script = remove_keeping_disk_script("sunlit-e2e-o'brien");
        assert!(script.contains("'sunlit-e2e-o''brien'"), "{script}");
    }

    #[test]
    fn removing_the_vm_keeps_the_disk_and_refuses_a_running_guest() {
        let script = remove_keeping_disk_script(NAME);
        assert!(script.contains("Remove-VM"), "{script}");
        // Remove-VM never deletes a VHDX, and nothing here may either: that
        // disk is the product.
        assert!(!script.contains("Remove-Item"), "{script}");
        assert!(!script.contains("Remove-VHD"), "{script}");
        assert!(script.contains("-ne 'Off'"), "{script}");
        assert!(script.contains("throw"), "{script}");
    }

    #[test]
    fn the_probe_reads_state_boot_entry_and_address_in_one_go() {
        let script = probe_script(NAME);
        for marker in ["STATE=", "BOOT=", "IP=", hyperv::QUERY_OK] {
            assert!(script.contains(marker), "{script}");
        }
        assert!(script.contains("Get-VMFirmware"), "{script}");
        assert!(script.contains("Get-VMNetworkAdapter"), "{script}");
        // `-f` is a string operator, not a parameter of Write-Output.
        assert!(!script.contains("\" -f "), "{script}");
        // Nothing is looked up by name, because a name that is not there is an
        // error record, and a suppressed error record still decides the exit
        // code.
        assert!(!script.contains("-VMName"), "{script}");
        assert!(
            !script.contains("-ErrorAction SilentlyContinue"),
            "{script}"
        );
    }

    #[test]
    fn a_probe_is_read_from_its_marker_lines() {
        let probe = Probe::parse(
            "STATE=Running\nBOOT=Drive DVD Drive\nIP=fe80::1%5\nIP=169.254.3.4\nIP=172.28.144.5\nQUERY=ok\n",
        );
        assert!(probe.answered);
        assert!(probe.running());
        assert_eq!(probe.boot.as_deref(), Some("Drive DVD Drive"));
        assert_eq!(
            hyperv::first_usable_ipv4(&probe.addresses),
            Some("172.28.144.5".to_owned())
        );
        assert_eq!(probe.guest(), Guest::Running);
    }

    #[test]
    fn a_guest_that_is_not_running_is_reported_rather_than_resumed() {
        // A Hyper-V guest stays Running through the reboots an install
        // performs, so Off mid-install is the install having ended. The
        // heartbeat's resume and give-up arms answer a stopped QEMU machine,
        // which this path never has.
        let off = Probe::parse("STATE=Off\nQUERY=ok\n");
        assert!(off.answered);
        assert!(!off.running());
        assert_eq!(
            off.guest(),
            Guest::Off {
                state: "Off".to_owned()
            }
        );

        // A VM that is gone: the query ran and found nothing.
        let gone = Probe::parse("QUERY=ok\n");
        assert!(gone.answered);
        assert_eq!(gone.state, None);
        assert!(!gone.running());

        // A query that never ran, which looks the same in every way except the
        // marker. The build treats this as a failed probe rather than as a VM
        // that vanished, because only one of the two ends the install.
        let unanswered = Probe::parse("");
        assert!(!unanswered.answered);
        assert_eq!(unanswered.state, None);

        let mut trend = Trend::new();
        let action = trend.observe(&Sample {
            elapsed: Duration::from_secs(5),
            disk: 12 * 1024 * 1024 * 1024,
            guest: off.guest(),
            screen: None,
        });
        assert!(matches!(action, Action::Say(_)), "{action:?}");
    }

    #[test]
    fn the_installer_needs_no_keypress_on_this_path() {
        // The whole of deviation 22's machinery is absent here, because the
        // media it boots has no prompt to answer.
        let script = create();
        assert!(!script.contains("send-key"), "{script}");
        assert!(!script.contains("qmp"), "{script}");
    }

    #[test]
    fn the_unattend_cd_carries_the_shipped_files_and_the_two_generated_ones() {
        let dir = std::env::temp_dir().join("sunlit_xtask_unattend_cd");
        let _ = std::fs::remove_dir_all(&dir);
        let template = dir.join("template");
        for relative in CD_FILES {
            let path = template.join(relative);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("temp tree");
            std::fs::write(&path, b"contents").expect("write");
        }
        let staging = dir.join("staging");
        stage_unattend_cd(&template, &staging, "ssh-ed25519 AAAA test\n").expect("staged");

        // Flat, because Windows Setup searches the root of the drive and the
        // bootstrap reads everything from beside itself.
        for name in [
            "Autounattend.xml",
            "bootstrap.ps1",
            "run-job.cmd",
            "session-ready.cmd",
            CD_AUTHORIZED_KEYS,
            CD_MARKER,
        ] {
            assert!(staging.join(name).is_file(), "{name} is not on the CD");
        }
        let key = std::fs::read_to_string(staging.join(CD_AUTHORIZED_KEYS)).expect("read");
        assert_eq!(key, "ssh-ed25519 AAAA test\n");
        assert_eq!(
            std::fs::read_to_string(staging.join(CD_MARKER)).expect("read"),
            CD_MARKER_CONTENT
        );

        // Staged again over the top: a rebuild must not leave a previous
        // build's key on the disc.
        stage_unattend_cd(&template, &staging, "ssh-ed25519 BBBB test").expect("staged again");
        let key = std::fs::read_to_string(staging.join(CD_AUTHORIZED_KEYS)).expect("read");
        assert_eq!(key, "ssh-ed25519 BBBB test\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Rust list and the HCL template's `cd_files` block have to name the
    /// same files: the Linux host builds this image from the template and the
    /// Windows host from the constant, and a guest missing one of these files
    /// fails an hour into an install for a reason nothing connects to a list.
    ///
    /// This is the technique that caught the e1000 and virtio-net divergence in
    /// the second validation round: where two artifacts must agree and only a
    /// convention connects them, a test reads both.
    #[test]
    fn the_cd_carries_what_the_template_carries() {
        let hcl =
            std::fs::read_to_string(crate::store::repo_root().join("vm/windows/windows11.pkr.hcl"))
                .expect("the Windows template");

        let mut from_template = hcl_list(&hcl, "cd_files")
            .into_iter()
            .map(|entry| entry.replace("${path.root}/", ""))
            .collect::<Vec<_>>();
        let mut from_constant: Vec<String> = CD_FILES.iter().map(|f| (*f).to_owned()).collect();
        from_template.sort();
        from_constant.sort();
        assert_eq!(
            from_template, from_constant,
            "the template and CD_FILES name different files"
        );

        // And the two generated entries, which are `cd_content` there.
        let mut keys = hcl_content_keys(&hcl);
        let mut expected = vec![CD_AUTHORIZED_KEYS.to_owned(), CD_MARKER.to_owned()];
        keys.sort();
        expected.sort();
        assert_eq!(
            keys, expected,
            "the template generates different CD entries"
        );
        assert!(
            hcl.contains(CD_MARKER_CONTENT),
            "the marker's content differs"
        );
    }

    /// The two generated CD entries have four copies of their names between
    /// them: this crate's constants, the HCL template's `cd_content` block, the
    /// unattend file's first-logon command, and the bootstrap script that reads
    /// them inside the guest. `the_cd_carries_what_the_template_carries` pins
    /// the first two against each other; these are the other two, and nothing
    /// but a convention connects any of them. A name that drifts fails an hour
    /// into an install, and the failure names a missing file rather than a list.
    #[test]
    fn the_guest_looks_for_the_cd_entries_this_crate_writes() {
        let repo = crate::store::repo_root();
        let unattend =
            std::fs::read_to_string(repo.join("vm/windows/autounattend/Autounattend.xml"))
                .expect("the unattend file");
        let bootstrap = std::fs::read_to_string(repo.join("vm/windows/scripts/bootstrap.ps1"))
            .expect("the bootstrap script");

        // The first-logon command finds the disc by the marker file, because it
        // cannot know which drive letter Windows gave it.
        assert!(
            unattend.contains(CD_MARKER),
            "the unattend file looks for a marker this build does not write"
        );
        // And it runs the bootstrap off that disc under the flat name the CD
        // carries it under, not the path it has in the repo.
        for relative in CD_FILES {
            let flat = Path::new(relative)
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .into_owned();
            if flat == "Autounattend.xml" {
                // Setup finds this one itself, by searching the drive.
                continue;
            }
            assert!(
                unattend.contains(&flat) || bootstrap.contains(&flat),
                "nothing in the guest reads {flat}, so it is either dead weight \
                 on the CD or read under another name"
            );
        }
        // The bootstrap copies the public key out of the disc by name.
        assert!(
            bootstrap.contains(CD_AUTHORIZED_KEYS),
            "the bootstrap reads a different key file than this build writes"
        );
    }

    /// The bootstrap probe means "the bootstrap has finished" only for as long
    /// as this task is the last thing the bootstrap registers.
    #[test]
    fn the_bootstrap_probe_waits_for_the_last_thing_bootstrap_does() {
        let bootstrap = std::fs::read_to_string(
            crate::store::repo_root().join("vm/windows/scripts/bootstrap.ps1"),
        )
        .expect("the bootstrap script");
        let last = bootstrap
            .rfind("Register-ScheduledTask")
            .expect("the bootstrap registers scheduled tasks");
        assert!(
            bootstrap[last..].contains(SESSION_READY_TASK),
            "the last task the bootstrap registers is not {SESSION_READY_TASK}, \
             so waiting for it no longer means the bootstrap has finished"
        );
        let command = bootstrap_done_command();
        assert!(command.contains(SESSION_READY_TASK), "{command}");
        assert!(command.contains(BOOTSTRAP_DONE_MARKER), "{command}");
        // cmd.exe is the guest's SSH shell, so this is batch and not
        // PowerShell.
        assert!(command.starts_with("schtasks "), "{command}");
    }

    #[test]
    fn the_finalize_step_copies_the_script_in_and_runs_it_as_a_file() {
        // Encoded onto the command line it is about twelve kilobytes, and
        // cmd.exe truncates a command line at eight.
        let destination = finalize_destination();
        assert_eq!(destination, r"C:\sunlit-e2e\finalize.ps1");
        let command = finalize_command(&destination);
        assert!(
            command.contains("-File C:\\sunlit-e2e\\finalize.ps1"),
            "{command}"
        );
        assert!(command.contains("-ExecutionPolicy Bypass"), "{command}");
        assert!(!command.contains("-EncodedCommand"), "{command}");
        // The script it runs is the one the repo ships and the Packer path
        // provisions.
        assert!(
            crate::store::repo_root()
                .join("vm/windows")
                .join(FINALIZE_SCRIPT)
                .is_file()
        );
    }

    #[test]
    fn the_shutdown_is_the_one_the_template_used() {
        let hcl =
            std::fs::read_to_string(crate::store::repo_root().join("vm/windows/windows11.pkr.hcl"))
                .expect("the Windows template");
        assert!(
            hcl.contains(SHUTDOWN_COMMAND),
            "the template shuts the guest down differently than this path does"
        );
    }

    #[test]
    fn the_build_record_marks_a_build_and_owns_the_disk_a_teardown_deletes() {
        let store = Store::new("/srv/vm");
        let state = build_state(&store, Target::Windows);
        assert_eq!(state.reason, StartReason::Build);
        assert!(
            state.reason.label().contains("build"),
            "{}",
            state.reason.label()
        );
        assert!(state.is_ours());
        assert_eq!(state.provider_kind(), Some(ProviderKind::HyperV));
        // The disk the install writes into is the one `vm down` deletes: a
        // half-built image is worth nothing.
        assert_eq!(state.overlay, store.build_disk(Target::Windows));
        assert_ne!(state.overlay, store.overlay(Target::Windows));
        assert_eq!(state.ssh_user, hyperv::GUEST_USER);
        assert_eq!(state.ssh_port, 22);
        // Nothing of a QEMU guest applies.
        assert_eq!(state.pid, None);
        assert_eq!(state.qmp_port, None);
    }

    #[test]
    fn the_manifest_records_the_mechanism_rather_than_a_packer_version() {
        let label = builder_label();
        assert!(label.contains("hyper-v"), "{label}");
        assert!(!label.contains("packer"), "{label}");
    }

    #[test]
    fn what_is_still_running_is_named_along_with_how_to_reach_it() {
        let state = build_state(&Store::new("/srv/vm"), Target::Windows);
        let text = aftermath(&Presence::Registered, Target::Windows, &state);
        assert!(text.contains("sunlit-e2e-windows is still there"), "{text}");
        for hint in [
            "cargo xtask vm view windows",
            "cargo xtask vm ssh windows",
            "cargo xtask vm down windows",
        ] {
            assert!(text.contains(hint), "{text}");
        }

        // A VM that is not registered is not described as running, because a
        // hint to go and look at one is worse than none. And it is not
        // described as never created either: a VM that vanished mid-install
        // answers the same query the same way, and this text is on both paths.
        let gone = aftermath(&Presence::Gone, Target::Windows, &state);
        assert!(
            gone.contains("no VM called sunlit-e2e-windows is registered"),
            "{gone}"
        );
        assert!(!gone.contains("was not created"), "{gone}");
        assert!(!gone.contains("still there"), "{gone}");
        assert!(!gone.contains("vm view windows"), "{gone}");
        assert!(gone.contains("cargo xtask vm down windows"), "{gone}");

        // And a hypervisor that would not say says so, rather than picking one
        // of the two answers it does not have.
        let unknown = aftermath(
            &Presence::Unknown {
                why: "the Hyper-V script exited Some(1): access denied".to_owned(),
            },
            Target::Windows,
            &state,
        );
        assert!(unknown.contains("cannot be read from Hyper-V"), "{unknown}");
        assert!(unknown.contains("access denied"), "{unknown}");
        assert!(unknown.contains("cargo xtask vm status"), "{unknown}");
    }

    #[test]
    fn a_state_query_says_which_of_the_three_answers_it_gave() {
        // The marker is the whole distinction: a query that found nothing and
        // one that never ran have the same missing STATE line.
        assert_eq!(
            presence_of(&Ok("STATE=Off\nQUERY=ok\n".to_owned())),
            Presence::Registered
        );
        assert_eq!(presence_of(&Ok("QUERY=ok\n".to_owned())), Presence::Gone);
        assert!(matches!(
            presence_of(&Ok(String::new())),
            Presence::Unknown { .. }
        ));
        assert!(matches!(
            presence_of(&Err("the Hyper-V script exited Some(1): nope".to_owned())),
            Presence::Unknown { .. }
        ));
    }

    /// The create script is one `powershell.exe` invocation with
    /// `$ErrorActionPreference = 'Stop'` in front of it, so it ends at its first
    /// failure and every statement after `New-VM` leaves a registered VM behind.
    /// Its own `throw` for media that would not attach is one of those, and a
    /// failure there used to return with the VM unnamed.
    #[test]
    fn a_create_that_failed_after_new_vm_names_the_vm_it_left_behind() {
        let runner = FakeRunner::new()
            .on(
                "New-VHD",
                CommandOutput::failed(1, "the installation media is not attached to the VM"),
            )
            .on("STATE=", CommandOutput::ok("STATE=Off\nQUERY=ok\n"));
        let store = Store::new("/srv/vm");
        let mut state = build_state(&store, Target::Windows);
        let error = create_and_install(
            &runner,
            &store,
            Target::Windows,
            &mut state,
            Path::new("vm/windows"),
            Path::new(r"C:\vm\iso\noprompt.iso"),
            Path::new(r"C:\vm\build\windows\unattend.iso"),
        )
        .expect_err("the create script failed");
        assert!(error.contains("is not attached to the VM"), "{error}");
        assert!(
            error.contains("sunlit-e2e-windows is still there"),
            "{error}"
        );
        assert!(error.contains("cargo xtask vm down windows"), "{error}");
        // The script's own `throw` is not Hyper-V refusing anything, and saying
        // so sent the reader looking at the hypervisor.
        assert!(!error.contains("Hyper-V refused"), "{error}");
    }

    #[test]
    fn a_create_that_failed_before_new_vm_says_there_is_nothing_running() {
        let runner = FakeRunner::new()
            .on(
                "New-VHD",
                CommandOutput::failed(1, "there is not enough space on the disk"),
            )
            .on("STATE=", CommandOutput::ok("QUERY=ok\n"));
        let store = Store::new("/srv/vm");
        let mut state = build_state(&store, Target::Windows);
        let error = create_and_install(
            &runner,
            &store,
            Target::Windows,
            &mut state,
            Path::new("vm/windows"),
            Path::new("i.iso"),
            Path::new("u.iso"),
        )
        .expect_err("the create script failed");
        assert!(error.contains("not enough space"), "{error}");
        assert!(
            error.contains("no VM called sunlit-e2e-windows is registered"),
            "{error}"
        );
        assert!(!error.contains("still there"), "{error}");
    }

    #[test]
    fn a_guest_that_is_starting_does_not_end_the_install_on_its_first_reading() {
        // `Start-VM` returns before the guest is Running, so one non-Running
        // reading is a machine on its way up. Two are an install that ended.
        let runner =
            FakeRunner::new().on("STATE=", CommandOutput::ok("STATE=Starting\nQUERY=ok\n"));
        let store = Store::new("/srv/vm");
        let mut state = build_state(&store, Target::Windows);
        let error = watch_install(&runner, &store, Target::Windows, &mut state, Duration::ZERO)
            .expect_err("a guest that never runs ends the install");
        assert!(error.contains("Starting"), "{error}");
        assert!(error.contains("2 readings in a row"), "{error}");
        assert!(
            runner.calls().len() >= NOT_RUNNING_SAMPLES as usize,
            "the first reading has to have been tolerated: {:?}",
            runner.calls()
        );
    }

    #[test]
    fn a_vm_that_vanished_mid_install_is_not_reported_as_one_that_was_never_created() {
        // Both ends of a build reach `Presence::Gone` through the one map_err
        // in `create_and_install`: a create that registered nothing, and an
        // install whose VM disappeared under it. The text is composed here the
        // way that map_err composes it, and it may not assert a history no
        // query can read.
        let runner = FakeRunner::new().on("STATE=", CommandOutput::ok("QUERY=ok\n"));
        let store = Store::new("/srv/vm");
        let mut state = build_state(&store, Target::Windows);
        let error = watch_install(&runner, &store, Target::Windows, &mut state, Duration::ZERO)
            .expect_err("a VM that is not registered ends the install");
        assert!(error.contains("is no longer registered"), "{error}");

        let whole = format!(
            "{error}\n{}",
            aftermath(&presence(&runner, &state.vm_name), Target::Windows, &state)
        );
        assert!(!whole.contains("was not created"), "{whole}");
        assert!(
            whole.contains("no VM called sunlit-e2e-windows is registered"),
            "{whole}"
        );
    }

    #[test]
    fn a_shutdown_reading_that_never_ran_is_not_a_vm_that_vanished() {
        // The one query site that was not making departure 35's distinction. A
        // VM that vanished mid-shutdown means the install's disk is gone; a
        // query that did not run means nothing at all, and treating the second
        // as the first would report a lost build for a hiccup.
        assert_eq!(shutting("STATE=Off\nQUERY=ok\n"), Shutting::Off);
        assert_eq!(
            shutting("STATE=Running\nQUERY=ok\n"),
            Shutting::Waiting("Running".to_owned())
        );
        assert_eq!(shutting("QUERY=ok\n"), Shutting::Gone);
        assert_eq!(shutting(""), Shutting::Unreadable);
        assert_eq!(shutting("STATE=Off\n"), Shutting::Unreadable);
    }

    #[test]
    fn a_build_that_worked_leaves_no_leftovers_behind_it() {
        let dir = std::env::temp_dir().join("sunlit_xtask_build_leftovers");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(UNATTEND_STAGING)).expect("temp tree");
        std::fs::write(dir.join(UNATTEND_STAGING).join("Autounattend.xml"), b"x").expect("write");
        std::fs::write(dir.join(UNATTEND_ISO), b"iso").expect("write");
        // Anything else in there is somebody's diagnostics and is left alone.
        std::fs::write(dir.join("packer.log"), b"log").expect("write");

        clean_up_build_dir(&dir);
        assert!(!dir.join(UNATTEND_STAGING).exists());
        assert!(!dir.join(UNATTEND_ISO).exists());
        assert!(dir.join("packer.log").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The entries of an HCL list assignment, as written.
    fn hcl_list(hcl: &str, key: &str) -> Vec<String> {
        let start = hcl
            .find(&format!("{key} = ["))
            .unwrap_or_else(|| panic!("the template has no {key} block"));
        let open = hcl[start..].find('[').expect("an opening bracket") + start;
        let close = hcl[open..].find(']').expect("a closing bracket") + open;
        hcl[open + 1..close]
            .split(',')
            .map(|entry| entry.trim().trim_matches('"').to_owned())
            .filter(|entry| !entry.is_empty())
            .collect()
    }

    /// The keys of an HCL map assignment.
    fn hcl_content_keys(hcl: &str) -> Vec<String> {
        let start = hcl
            .find("cd_content = {")
            .expect("the template has no cd_content block");
        let open = hcl[start..].find('{').expect("an opening brace") + start;
        let close = hcl[open..].find('}').expect("a closing brace") + open;
        hcl[open + 1..close]
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let (key, _) = line.split_once('=')?;
                Some(key.trim().trim_matches('"').to_owned())
            })
            .filter(|key| !key.is_empty())
            .collect()
    }
}
