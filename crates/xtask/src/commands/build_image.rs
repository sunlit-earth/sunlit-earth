//! `cargo xtask vm build-image <target>`: Packer, then a manifest.
//!
//! The build itself is Packer's job. What this adds is everything around it:
//! the SSH key both guests trust, the accelerator the host can offer, a build
//! directory that Packer's "output must not exist" rule is happy with, and the
//! manifest that makes the result auditable afterwards (plan decision 4).

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::provider::target::{HostOs, Target};
use crate::runner::{Cmd, Runner};
use crate::store::hash;
use crate::store::manifest::{ImageRecord, Manifest};
use crate::store::{self, Store};
use crate::util;

/// The QEMU accelerator to ask Packer for.
///
/// WHPX on Windows is the same "Windows Hypervisor Platform" feature the doctor
/// checks for, and it coexists with `Hyper-V` because it runs on top of it. A
/// host with neither still builds, just slowly, which is better than refusing.
pub fn accelerator_for(host: HostOs) -> &'static str {
    match host {
        HostOs::Windows => "whpx",
        HostOs::Linux => "kvm",
        HostOs::Other => "none",
    }
}

/// What performs an install, which is not the same question as which hypervisor
/// runs the finished guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builder {
    /// The xtask installs Windows on `Hyper-V` itself, no builder in the loop.
    HyperV,
    /// Packer's QEMU builder, which is what both images were always built with.
    PackerQemu,
}

/// The builder matrix, which mirrors the runtime provider matrix.
///
/// | | Windows host | Linux host |
/// |---|---|---|
/// | Windows image | native `Hyper-V` | Packer and QEMU on KVM |
/// | Linux image | Packer and QEMU on WHPX | Packer and QEMU on KVM |
///
/// The one cell that changed is the Windows image on a Windows host, because
/// QEMU there runs on WHPX and a WHPX guest does not survive the reset Windows
/// Setup performs after copying its files (deviation 25 of the phase 3 plan,
/// and `docs/vm-setup.md` has the measurements). On this host the primary
/// runtime provider is `Hyper-V` anyway, so the install happens where the guest
/// will run and `qemu-img` derives the qcow2 for the QEMU override cell.
///
/// Deliberately not overridable. `SUNLIT_EARTH_VM_PROVIDER` moves a *guest*
/// between hypervisors, which is a cheap thing to try; pointing a build at a
/// path measured not to work would be a two-hour way to learn nothing. The
/// recheck recipe in `docs/vm-setup.md` re-measures it in twenty seconds
/// instead.
pub fn builder_for(host: HostOs, target: Target) -> Builder {
    match (host, target) {
        (HostOs::Windows, Target::Windows) => Builder::HyperV,
        _ => Builder::PackerQemu,
    }
}

/// What each path needs on `PATH`, and nothing it does not.
///
/// The `Hyper-V` path asks for less than the other: the install runs on the
/// hypervisor this host already has, so there is no `packer` and no
/// `qemu-system-x86_64` in it, and nothing after the ISO download touches the
/// network, so `packer init` and its firewall failure class (deviation 20) are
/// gone from this target. `qemu-img` stays, because the qcow2 the QEMU override
/// cell boots is derived from the VHDX.
pub fn required_tools(builder: Builder) -> &'static [&'static str] {
    match builder {
        Builder::HyperV => &[
            crate::store::windows_media::ISO_BUILDER,
            "qemu-img",
            "ssh",
            "scp",
            "ssh-keygen",
        ],
        Builder::PackerQemu => &["packer", "qemu-system-x86_64", "qemu-img", "ssh-keygen"],
    }
}

/// Everything a build needs, decided before anything runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub target: Target,
    pub template_dir: PathBuf,
    /// Packer's output directory. Deleted first, because Packer refuses to
    /// write into one that exists.
    pub output_dir: PathBuf,
    pub image_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub vars: Vec<(String, String)>,
}

impl BuildPlan {
    /// The `packer build` command line.
    pub fn build_args(&self) -> Vec<String> {
        let mut args = vec!["build".to_owned(), "-color=false".to_owned()];
        for (key, value) in &self.vars {
            args.push("-var".to_owned());
            args.push(format!("{key}={value}"));
        }
        args.push(self.template_dir.to_string_lossy().into_owned());
        args
    }
}

/// Assemble the plan for one target.
pub fn plan(
    store: &Store,
    target: Target,
    accelerator: &str,
    firmware: Option<&crate::provider::firmware::Firmware>,
) -> BuildPlan {
    let build_dir = store.build_dir(target);
    let mut vars = vec![
        (
            "output_dir".to_owned(),
            build_dir.join("output").to_string_lossy().into_owned(),
        ),
        ("accelerator".to_owned(), accelerator.to_owned()),
        (
            "ssh_private_key_file".to_owned(),
            store.ssh_key().to_string_lossy().into_owned(),
        ),
    ];
    if target == Target::Windows {
        vars.push((
            "iso_path".to_owned(),
            store.windows_iso().to_string_lossy().into_owned(),
        ));
        if let Some(firmware) = firmware {
            vars.push((
                "efi_firmware_code".to_owned(),
                firmware.code.to_string_lossy().into_owned(),
            ));
            vars.push((
                "efi_firmware_vars".to_owned(),
                firmware.vars.to_string_lossy().into_owned(),
            ));
        }
    }
    BuildPlan {
        target,
        template_dir: store::template_dir(target),
        output_dir: build_dir.join("output"),
        image_dir: store.image_dir(target),
        cache_dir: build_dir.join("cache"),
        vars,
    }
}

/// Read `packer version` for the manifest's `builder` field.
pub fn parse_packer_version(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|line| {
            let line = line.trim();
            line.strip_prefix("Packer v")
                .or_else(|| line.strip_prefix('v'))
                .map(|v| format!("packer {}", v.trim()))
        })
        .unwrap_or_else(|| "packer (version unknown)".to_owned())
}

/// Generate the key pair both guests trust, if it is not there yet.
///
/// `vm setup` normally does this. A build does it too, because a build is the
/// first thing that needs it and failing here would send the user back to a
/// command they have probably already run.
pub fn ensure_ssh_key(runner: &dyn Runner, store: &Store) -> Result<String, String> {
    let key = store.ssh_key();
    let public = store.ssh_pubkey();
    if !public.is_file() {
        if let Some(parent) = key.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        let _ = std::fs::remove_file(&key);
        let cmd = Cmd::new("ssh-keygen").args([
            "-t".to_owned(),
            "ed25519".to_owned(),
            "-N".to_owned(),
            String::new(),
            "-C".to_owned(),
            "sunlit-e2e".to_owned(),
            "-f".to_owned(),
            key.to_string_lossy().into_owned(),
        ]);
        let out = runner
            .capture(&cmd)
            .map_err(|e| format!("cannot run ssh-keygen: {e}"))?;
        if !out.success() {
            return Err(format!("ssh-keygen failed: {}", out.stderr.trim()));
        }
    }
    std::fs::read_to_string(&public)
        .map(|text| text.trim().to_owned())
        .map_err(|e| format!("cannot read {}: {e}", public.display()))
}

/// Build the manifest for a finished build.
pub fn manifest_for(
    target: Target,
    template_hash: String,
    images: &[(String, u64, String)],
    built_unix: u64,
    source: String,
    builder: String,
) -> Manifest {
    Manifest::new(
        target,
        template_hash,
        built_unix,
        source,
        builder,
        images
            .iter()
            .map(|(file, bytes, checksum)| ImageRecord {
                file: file.clone(),
                bytes: *bytes,
                checksum: checksum.clone(),
            })
            .collect(),
    )
}

/// How many times `packer init` is worth trying.
///
/// It is one HTTP conversation with a plugin registry, so a failure is usually
/// the network rather than anything about the template, and the cost of another
/// try is seconds against an hour-long build.
pub const INIT_ATTEMPTS: u32 = 3;

/// How long to wait between those attempts.
pub const INIT_RETRY_DELAY: Duration = Duration::from_secs(5);

/// What to say when every attempt failed.
///
/// The bare exit code sent one person looking for a Packer problem when what
/// they had was a local firewall that had not come back up after a reboot and
/// was refusing sockets to anything it did not recognise. Packer reports that
/// as a Go dial error, which reads like an outage.
pub fn init_failure_message(code: i32, attempts: u32) -> String {
    format!(
        "packer init failed with exit code {code} after {attempts} \
         {}.\n\nIt installs the plugins the template requires over HTTPS from \
         api.github.com, so the cause is usually outside Packer:\n  \
         - a local firewall or security product that blocks packer.exe. A socket \
         error such as \"An attempt was made to access a socket in a way \
         forbidden by its access permissions\" is that, not an outage.\n  \
         - a proxy, which packer reads from HTTP_PROXY and HTTPS_PROXY\n  \
         - a genuine outage at the registry\n\n\
         Nothing has been built, so running the command again is safe.",
        crate::util::count(attempts as usize, "attempt")
    )
}

/// `PATH` with `dir` in front of it.
///
/// For handing Packer a tool that is installed but not on this process's
/// `PATH`, which is what winget's links directory looks like until a new shell
/// starts.
pub fn path_with(dir: &Path, current: Option<&str>) -> String {
    let separator = if cfg!(windows) { ';' } else { ':' };
    match current {
        Some(current) if !current.is_empty() => {
            format!("{}{separator}{current}", dir.display())
        }
        _ => dir.display().to_string(),
    }
}

/// Install the plugins the template requires, retrying a failed attempt.
pub fn init_plugins(
    runner: &dyn Runner,
    plan: &BuildPlan,
    extra_path: Option<&str>,
    attempts: u32,
    delay: Duration,
) -> Result<(), String> {
    let mut last = 0;
    for attempt in 1..=attempts.max(1) {
        let mut init = Cmd::new("packer")
            .args([
                "init".to_owned(),
                plan.template_dir.to_string_lossy().into_owned(),
            ])
            .env("PACKER_CACHE_DIR", plan.cache_dir.to_string_lossy());
        if let Some(path) = extra_path {
            init = init.env("PATH", path);
        }
        last = runner
            .stream(&init)
            .map_err(|e| format!("cannot run packer: {e}"))?;
        if last == 0 {
            return Ok(());
        }
        if attempt < attempts {
            println!(
                "packer init failed with exit code {last}; retrying in {} seconds \
                 ({attempt} of {attempts} done)",
                delay.as_secs()
            );
            std::thread::sleep(delay);
        }
    }
    Err(init_failure_message(last, attempts.max(1)))
}

/// The ISO builder Packer will use, and the directory to put on its `PATH` when
/// this process's own `PATH` cannot see the tool yet.
///
/// A tool winget installed during this shell's lifetime is real but invisible
/// to a `PATH` lookup, because winget appended its links directory to the user
/// `PATH` after this process inherited its copy. Packer resolves the tool on
/// `PATH` itself, so the answer is to hand it a `PATH` that has it.
fn resolve_iso_tool(
    runner: &dyn Runner,
    host: HostOs,
) -> Result<(&'static str, Option<PathBuf>), String> {
    let on_path: Vec<&'static str> = crate::host::facts::PACKER_ISO_TOOLS
        .into_iter()
        .filter(|tool| runner.which(tool).is_some())
        .collect();
    if let Some(tool) = crate::host::facts::packer_iso_tool(&on_path) {
        return Ok((tool, None));
    }
    let found = crate::host::facts::PACKER_ISO_TOOLS
        .into_iter()
        .find_map(|tool| crate::host::facts::resolve_tool(runner, tool, host).map(|at| (tool, at)));
    let Some((tool, at)) = found else {
        return Err(format!(
            "no ISO builder on PATH, and Packer needs one to make the CD this \
             template hands the guest. It looks for {}. \
             `cargo xtask vm setup` installs one.",
            crate::host::facts::PACKER_ISO_TOOLS.join(", ")
        ));
    };
    Ok((tool, at.parent().map(Path::to_path_buf)))
}

/// The QMP port the Windows template opens for the boot key, and the one this
/// presses on. Not the 4444 a running guest uses, so a build and a guest cannot
/// collide.
pub const BUILD_QMP_PORT: u16 = 4445;

/// What Packer names the disk it builds, which is the template's `vm_name`.
pub const BUILT_DISK: &str = "golden.qcow2";

/// Run a build end to end, on whichever path this host and target choose.
pub fn run(runner: &dyn Runner, target: Target) -> Result<u8, String> {
    let store = store::store()?;
    let host = HostOs::current();
    if host == HostOs::Other {
        return Err("images are built on a Windows or Linux host".to_owned());
    }
    match builder_for(host, target) {
        Builder::HyperV => crate::commands::build_hyperv::run(runner, &store, target),
        Builder::PackerQemu => run_packer_build(runner, &store, target, host),
    }
}

/// The Packer and QEMU path: both images on a Linux host, and the Linux image
/// on a Windows one.
fn run_packer_build(
    runner: &dyn Runner,
    store: &Store,
    target: Target,
    host: HostOs,
) -> Result<u8, String> {
    for tool in required_tools(Builder::PackerQemu) {
        if crate::host::facts::resolve_tool(runner, tool, host).is_none() {
            return Err(format!(
                "{tool} is not available; run `cargo xtask vm doctor` for the whole list"
            ));
        }
    }

    // Both templates hand their guest a small CD, and Packer builds it by
    // shelling out to one of these. Without one it fails immediately, several
    // seconds into a command that otherwise takes an hour, with an error about
    // a tool nothing here has ever mentioned.
    let (iso_tool, iso_tool_dir) = resolve_iso_tool(runner, host)?;

    let public_key = ensure_ssh_key(runner, store)?;
    if target == Target::Windows {
        crate::store::windows_media::ensure_iso(runner, store)?;
    }

    let accelerator = accelerator_for(host);
    // Windows 11 needs UEFI, and Packer's own defaults for it are Linux paths.
    let firmware = if target == Target::Windows {
        let binary = crate::host::facts::resolve_tool(runner, "qemu-system-x86_64", host);
        Some(
            crate::provider::firmware::locate(host, binary.as_deref())
                .ok_or_else(|| crate::provider::firmware::missing_message(host))?,
        )
    } else {
        None
    };
    let plan = plan(store, target, accelerator, firmware.as_ref());
    if !plan.template_dir.is_dir() {
        return Err(format!(
            "no templates at {}; the repo is where they live",
            plan.template_dir.display()
        ));
    }

    // Packer refuses to write into a directory that exists, and a previous
    // failed build leaves one behind.
    let _ = std::fs::remove_dir_all(&plan.output_dir);
    std::fs::create_dir_all(&plan.cache_dir)
        .map_err(|e| format!("cannot create {}: {e}", plan.cache_dir.display()))?;

    let version = runner
        .capture(&Cmd::new("packer").arg("version"))
        .map_or_else(
            |_| "packer (version unknown)".to_owned(),
            |out| parse_packer_version(&out.stdout),
        );

    println!("building the {target} golden image with {version}");
    match &iso_tool_dir {
        None => println!("  cd images: {iso_tool}"),
        Some(dir) => println!("  cd images: {iso_tool}, from {}", dir.display()),
    }
    let build_dir = store.build_dir(target);
    let log = build_dir.join("packer.log");
    let screen = build_dir.join("screen.png");
    println!("  templates: {}", plan.template_dir.display());
    println!("  output:    {}", plan.output_dir.display());
    println!("  log:       {}", log.display());
    println!("  this takes tens of minutes and downloads several gigabytes");

    let extra_path = iso_tool_dir
        .as_deref()
        .map(|dir| path_with(dir, std::env::var("PATH").ok().as_deref()));

    init_plugins(
        runner,
        &plan,
        extra_path.as_deref(),
        INIT_ATTEMPTS,
        INIT_RETRY_DELAY,
    )?;

    run_packer(
        runner,
        &plan,
        target,
        public_key,
        extra_path.as_deref(),
        &log,
        screen,
    )?;

    finish(runner, store, target, &plan, &version)
}

/// Run the build itself: the command, the watcher over it, and what a nonzero
/// exit code means.
fn run_packer(
    runner: &dyn Runner,
    plan: &BuildPlan,
    target: Target,
    public_key: String,
    extra_path: Option<&str>,
    log: &Path,
    screen: PathBuf,
) -> Result<(), String> {
    let mut vars = plan.vars.clone();
    vars.push(("ssh_public_key".to_owned(), public_key));
    if target == Target::Windows {
        vars.push(("qmp_port".to_owned(), BUILD_QMP_PORT.to_string()));
    }
    let mut build = Cmd::new("packer")
        .args(
            BuildPlan {
                vars,
                ..plan.clone()
            }
            .build_args(),
        )
        .env("PACKER_CACHE_DIR", plan.cache_dir.to_string_lossy())
        // Packer's own log, kept because the interesting half of a failed build
        // is what QEMU said, and Packer swallows that unless it is logging. To
        // a file rather than to stderr, which is what setting the path does, so
        // the terminal still shows the build and not the log.
        .env("PACKER_LOG", "1")
        .env("PACKER_LOG_PATH", log.to_string_lossy());
    if let Some(path) = extra_path {
        build = build.env("PATH", path);
    }

    // Packer says nothing between the boot prompt and the guest's first SSH
    // answer, which is most of a Windows install. The watcher reads what is
    // observable in the meantime and ends a build whose guest has stopped.
    let watcher = crate::commands::build_watch::spawn(crate::commands::build_watch::Watch {
        target,
        qmp_port: (target == Target::Windows).then_some(BUILD_QMP_PORT),
        // The same file `finish` collects, which is also the signal that the
        // boot prompt has been answered.
        disk: plan.output_dir.join(BUILT_DISK),
        screen,
        poll: crate::commands::build_watch::POLL,
    });
    let outcome = runner.stream(&build);
    let verdict = watcher.stop();
    let code = outcome.map_err(|e| format!("cannot run packer: {e}"))?;
    if code != 0 {
        return Err(match verdict {
            Some(why) => format!(
                "packer build failed with exit code {code} because {why}. \
                 Nothing has been built; `{}` has the log.",
                log.display()
            ),
            None => format!(
                "packer build failed with exit code {code}; the output above says why, \
                 `{}` holds what it left behind, and `{}` has the log",
                plan.output_dir.display(),
                log.display()
            ),
        });
    }
    Ok(())
}

/// Move the built image into place, convert it where a second format is
/// needed, and write the manifest.
fn finish(
    runner: &dyn Runner,
    store: &Store,
    target: Target,
    plan: &BuildPlan,
    builder: &str,
) -> Result<u8, String> {
    std::fs::create_dir_all(&plan.image_dir)
        .map_err(|e| format!("cannot create {}: {e}", plan.image_dir.display()))?;

    let built = plan.output_dir.join(BUILT_DISK);
    let qcow2 = store.qcow2(target);
    move_file(&built, &qcow2)?;

    let mut images = vec![record(&qcow2)?];

    if target == Target::Windows {
        // One canonical install, two disk formats: Hyper-V boots the VHDX and
        // QEMU boots the qcow2, and they are the same Windows. A native
        // Hyper-V build derives them the other way round.
        let vhdx = store.vhdx(target);
        println!("converting to VHDX for the Hyper-V provider");
        convert(runner, &qcow2, &vhdx, "vhdx")?;
        images.push(record(&vhdx)?);
    }

    let source = match target {
        Target::Windows => store.windows_iso().to_string_lossy().into_owned(),
        Target::Linux => "ubuntu 22.04 cloud image".to_owned(),
    };
    write_manifest(
        store,
        target,
        &plan.template_dir,
        &images,
        source,
        builder.to_owned(),
    )?;

    let _ = std::fs::remove_dir_all(&plan.output_dir);

    forget_host_keys(store);
    announce(target, &images);
    Ok(0)
}

/// `qemu-img convert` between the two formats the two providers boot.
///
/// `subformat=dynamic` only when a VHDX is being written: it is a VHDX option,
/// and qcow2 refuses it.
pub fn convert_command(from: &Path, to: &Path, format: &str) -> Cmd {
    let mut args = vec![
        "convert".to_owned(),
        "-p".to_owned(),
        "-O".to_owned(),
        format.to_owned(),
    ];
    if format == "vhdx" {
        args.push("-o".to_owned());
        args.push("subformat=dynamic".to_owned());
    }
    args.push(from.to_string_lossy().into_owned());
    args.push(to.to_string_lossy().into_owned());
    Cmd::new("qemu-img").args(args)
}

/// Derive the second disk format, streaming `qemu-img`'s own progress.
pub fn convert(runner: &dyn Runner, from: &Path, to: &Path, format: &str) -> Result<(), String> {
    let _ = std::fs::remove_file(to);
    let code = runner
        .stream(&convert_command(from, to, format))
        .map_err(|e| format!("cannot run qemu-img: {e}"))?;
    if code != 0 {
        // Deleted rather than left behind: a partial image is the one thing
        // the size check in the currency model cannot tell from a whole one,
        // because the manifest that would contradict it is not written yet.
        let _ = std::fs::remove_file(to);
        return Err(format!(
            "qemu-img convert to {format} failed with exit code {code}"
        ));
    }
    Ok(())
}

/// Write the manifest that makes the result auditable (plan decision 4).
pub fn write_manifest(
    store: &Store,
    target: Target,
    template_dir: &Path,
    images: &[(String, u64, String)],
    source: String,
    builder: String,
) -> Result<(), String> {
    let template_hash = hash::read_tree(template_dir)
        .map(|files| hash::template_hash(&files))
        .map_err(|e| format!("cannot hash the templates: {e}"))?;
    let manifest = manifest_for(
        target,
        template_hash,
        images,
        util::now_unix(),
        source,
        builder,
    );
    std::fs::write(store.manifest(target), manifest.to_json())
        .map_err(|e| format!("cannot write the manifest: {e}"))
}

/// Forget the host keys of the image that has just been replaced.
///
/// A guest's SSH host keys are generated during the image build and live on the
/// golden disk, so every throwaway overlay of one image answers with the same
/// key and the known-hosts file beside the key pair is right about it. A rebuild
/// changes that key, and `ssh` then prints the remote-host-identification-changed
/// warning on every connection for the rest of that image's life:
/// `StrictHostKeyChecking=no` accepts a *new* key without a prompt and says
/// nothing about accepting a *changed* one. Public-key authentication keeps
/// working either way, so this is noise rather than a failure, which is exactly
/// why it would never be dealt with otherwise.
///
/// The file is the xtask's own and holds nothing but these guests, so a build
/// deletes it. Clearing both targets' entries rather than one is deliberate: it
/// is one file, and the entries come back on the next boot for free.
pub fn forget_host_keys(store: &Store) {
    let path = crate::guest::ssh::known_hosts(&store.ssh_key());
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        println!("warning: cannot clear {} ({e})", path.display());
    }
}

/// The closing report, the same whichever path built the image.
pub fn announce(target: Target, images: &[(String, u64, String)]) {
    println!();
    println!("the {target} golden image is built:");
    for (file, bytes, _) in images {
        println!("  {file}  {}", util::format_bytes(*bytes));
    }
    if target.has_eval_expiry() {
        println!(
            "  the evaluation clock started now and runs {} days",
            crate::store::manifest::EVAL_TOTAL_DAYS
        );
    }
    println!("`cargo xtask vm status` lists it; `cargo xtask e2e --target {target}` uses it.");
}

pub fn record(path: &Path) -> Result<(String, u64, String), String> {
    let bytes = std::fs::metadata(path)
        .map_err(|e| format!("cannot stat {}: {e}", path.display()))?
        .len();
    let checksum = hash::checksum_file(path).map_err(|e| format!("cannot checksum: {e}"))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok((name, bytes, checksum))
}

/// Rename, falling back to copy when the two paths are on different volumes.
pub fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    if !from.is_file() {
        return Err(format!(
            "the build reported success but produced no {}",
            from.display()
        ));
    }
    let _ = std::fs::remove_file(to);
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("cannot move {} to {}: {e}", from.display(), to.display()))?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::new("/srv/vm")
    }

    #[test]
    fn the_builder_matrix_mirrors_the_runtime_provider_matrix() {
        // The one cell that is not Packer is the one QEMU cannot install: a
        // WHPX guest does not survive the reset Windows Setup performs.
        assert_eq!(
            builder_for(HostOs::Windows, Target::Windows),
            Builder::HyperV
        );
        assert_eq!(
            builder_for(HostOs::Windows, Target::Linux),
            Builder::PackerQemu
        );
        assert_eq!(
            builder_for(HostOs::Linux, Target::Windows),
            Builder::PackerQemu
        );
        assert_eq!(
            builder_for(HostOs::Linux, Target::Linux),
            Builder::PackerQemu
        );
        // The same builder the runtime provider would be, wherever both exist.
        for target in Target::ALL {
            let native = crate::provider::target::provider_for(HostOs::Windows, target);
            let hyperv = native == Some(crate::provider::target::ProviderKind::HyperV);
            assert_eq!(
                builder_for(HostOs::Windows, target) == Builder::HyperV,
                hyperv,
                "{target} builds on a different hypervisor than it runs on"
            );
        }
    }

    #[test]
    fn the_native_path_needs_neither_packer_nor_a_qemu_to_run_a_guest_in() {
        let native = required_tools(Builder::HyperV);
        assert!(!native.contains(&"packer"), "{native:?}");
        assert!(!native.contains(&"qemu-system-x86_64"), "{native:?}");
        // It still needs the ISO builder, for both the media repack and the
        // unattend CD, and qemu-img for the qcow2 the override cell boots.
        assert!(native.contains(&"oscdimg"), "{native:?}");
        assert!(native.contains(&"qemu-img"), "{native:?}");
        // And the SSH client family, which is how the guest is reached.
        for tool in ["ssh", "scp", "ssh-keygen"] {
            assert!(native.contains(&tool), "{native:?}");
        }

        let packer = required_tools(Builder::PackerQemu);
        assert!(packer.contains(&"packer"), "{packer:?}");
        assert!(packer.contains(&"qemu-system-x86_64"), "{packer:?}");
        assert!(packer.contains(&"qemu-img"), "{packer:?}");
    }

    #[test]
    fn the_conversion_command_is_pinned_in_both_directions() {
        // One canonical install, two formats. Which one is derived depends on
        // where the install happened, and the command is otherwise the same.
        let to_vhdx = convert_command(
            Path::new("/srv/vm/images/windows/golden.qcow2"),
            Path::new("/srv/vm/images/windows/golden.vhdx"),
            "vhdx",
        );
        assert_eq!(to_vhdx.program, "qemu-img");
        assert_eq!(
            to_vhdx.args,
            [
                "convert",
                "-p",
                "-O",
                "vhdx",
                "-o",
                "subformat=dynamic",
                "/srv/vm/images/windows/golden.qcow2",
                "/srv/vm/images/windows/golden.vhdx",
            ]
        );

        let to_qcow2 = convert_command(
            Path::new("/srv/vm/images/windows/golden.vhdx"),
            Path::new("/srv/vm/images/windows/golden.qcow2"),
            "qcow2",
        );
        assert_eq!(
            to_qcow2.args,
            [
                "convert",
                "-p",
                "-O",
                "qcow2",
                "/srv/vm/images/windows/golden.vhdx",
                "/srv/vm/images/windows/golden.qcow2",
            ]
        );
        // `subformat=dynamic` is a VHDX option, and qcow2 rejects it.
        assert!(!to_qcow2.args.contains(&"-o".to_owned()));
    }

    #[test]
    fn a_failing_init_is_retried_and_then_explained() {
        use crate::runner::CommandOutput;
        use crate::runner::fake::FakeRunner;

        let plan = plan(&store(), Target::Linux, "kvm", None);
        let runner = FakeRunner::new().on("packer init", CommandOutput::failed(1, "dial tcp"));
        let error = init_plugins(&runner, &plan, None, 3, Duration::ZERO)
            .expect_err("every attempt failed");
        assert_eq!(
            runner
                .calls()
                .iter()
                .filter(|call| call.contains("packer init"))
                .count(),
            3,
            "{:?}",
            runner.calls()
        );
        // The bare exit code is what sent someone hunting for a Packer bug.
        assert!(error.contains("firewall"), "{error}");
        assert!(error.contains("HTTP_PROXY"), "{error}");
        assert!(error.contains("3 attempts"), "{error}");
        assert!(error.contains("safe"), "{error}");

        let good = FakeRunner::new().on("packer init", CommandOutput::ok(""));
        assert!(init_plugins(&good, &plan, None, 3, Duration::ZERO).is_ok());
        assert_eq!(
            good.calls()
                .iter()
                .filter(|call| call.contains("packer init"))
                .count(),
            1,
            "a working init is not retried"
        );
    }

    #[test]
    fn a_tool_off_path_is_handed_to_packer_through_path() {
        let dir = Path::new("/opt/oscdimg");
        let separator = if cfg!(windows) { ';' } else { ':' };
        assert_eq!(
            path_with(dir, Some("/usr/bin")),
            format!("{}{separator}/usr/bin", dir.display())
        );
        // An empty or absent PATH must not produce a leading separator, which
        // some shells read as "the current directory".
        assert_eq!(path_with(dir, Some("")), dir.display().to_string());
        assert_eq!(path_with(dir, None), dir.display().to_string());
    }

    #[test]
    fn the_accelerator_follows_the_host() {
        assert_eq!(accelerator_for(HostOs::Windows), "whpx");
        assert_eq!(accelerator_for(HostOs::Linux), "kvm");
        assert_eq!(accelerator_for(HostOs::Other), "none");
    }

    #[test]
    fn the_plan_points_packer_at_the_store_and_the_repo() {
        let plan = plan(&store(), Target::Linux, "whpx", None);
        assert!(
            plan.template_dir.ends_with("vm/linux") || plan.template_dir.ends_with(r"vm\linux")
        );
        assert_eq!(plan.image_dir, store().image_dir(Target::Linux));
        assert!(
            plan.output_dir
                .starts_with(store().build_dir(Target::Linux))
        );
        assert!(plan.cache_dir.starts_with(store().build_dir(Target::Linux)));
    }

    #[test]
    fn only_the_windows_plan_carries_an_iso_path() {
        let firmware = crate::provider::firmware::Firmware {
            code: PathBuf::from("/fw/code.fd"),
            vars: PathBuf::from("/fw/vars.fd"),
        };
        let linux = plan(&store(), Target::Linux, "kvm", Some(&firmware));
        let windows = plan(&store(), Target::Windows, "whpx", Some(&firmware));
        assert!(!linux.vars.iter().any(|(k, _)| k == "iso_path"));
        assert!(windows.vars.iter().any(|(k, _)| k == "iso_path"));
        // The Linux cloud image boots without UEFI, so it is not handed
        // firmware it does not need.
        assert!(!linux.vars.iter().any(|(k, _)| k == "efi_firmware_code"));
        assert!(windows.vars.iter().any(|(k, _)| k == "efi_firmware_code"));
        assert!(windows.vars.iter().any(|(k, _)| k == "efi_firmware_vars"));
    }

    #[test]
    fn the_build_command_passes_every_variable_and_the_template_last() {
        let plan = plan(&store(), Target::Linux, "kvm", None);
        let args = plan.build_args();
        assert_eq!(args[0], "build");
        assert!(args.contains(&"accelerator=kvm".to_owned()));
        assert!(
            args.iter().any(|a| a.starts_with("ssh_private_key_file=")),
            "{args:?}"
        );
        assert_eq!(
            args.last(),
            Some(&plan.template_dir.to_string_lossy().into_owned())
        );
        // One -var flag per variable, and nothing shell-quoted: the arguments
        // reach the process directly, so a path with a space needs no escaping.
        assert_eq!(
            args.iter().filter(|a| *a == "-var").count(),
            plan.vars.len()
        );
    }

    #[test]
    fn the_packer_version_is_read_for_the_manifest() {
        assert_eq!(parse_packer_version("Packer v1.16.0\n"), "packer 1.16.0");
        assert_eq!(parse_packer_version("v1.11.2"), "packer 1.11.2");
        assert_eq!(
            parse_packer_version("something else entirely"),
            "packer (version unknown)"
        );
    }

    #[test]
    fn a_rebuild_forgets_the_host_keys_of_the_image_it_replaced() {
        // The keys live on the golden disk, so they change with the image and
        // every connection afterwards would warn that the host identification
        // changed. Nothing else writes this file, and a missing one is not a
        // problem to report.
        let dir = std::env::temp_dir().join("sunlit_xtask_forget_host_keys");
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::new(&dir);
        let known = crate::guest::ssh::known_hosts(&store.ssh_key());
        std::fs::create_dir_all(known.parent().expect("a parent")).expect("temp tree");
        std::fs::write(&known, b"172.28.144.5 ssh-ed25519 AAAA\n").expect("write");
        // The key pair itself is not touched: it is what the guests trust.
        std::fs::write(store.ssh_key(), b"private").expect("write");

        forget_host_keys(&store);
        assert!(!known.exists());
        assert!(store.ssh_key().is_file());
        forget_host_keys(&store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_manifest_records_every_image_the_build_produced() {
        let manifest = manifest_for(
            Target::Windows,
            "crc32:aaaa".to_owned(),
            &[
                ("golden.qcow2".to_owned(), 10, "crc32:1".to_owned()),
                ("golden.vhdx".to_owned(), 20, "crc32:2".to_owned()),
            ],
            1_755_600_000,
            "eval.iso".to_owned(),
            "packer 1.16.0".to_owned(),
        );
        assert_eq!(manifest.images.len(), 2);
        assert_eq!(manifest.record("golden.vhdx").map(|r| r.bytes), Some(20));
        assert_eq!(manifest.built_utc, "2025-08-19T10:40:00Z");
        assert_eq!(manifest.target, "windows");
    }
}
