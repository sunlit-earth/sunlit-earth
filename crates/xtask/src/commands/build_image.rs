//! `cargo xtask vm build-image <target>`: Packer, then a manifest.
//!
//! The build itself is Packer's job. What this adds is everything around it:
//! the SSH key both guests trust, the accelerator the host can offer, a build
//! directory that Packer's "output must not exist" rule is happy with, and the
//! manifest that makes the result auditable afterwards (plan decision 4).

use std::path::{Path, PathBuf};

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

/// Run a build end to end.
pub fn run(runner: &dyn Runner, target: Target) -> Result<u8, String> {
    let store = store::store()?;
    let host = HostOs::current();
    if host == HostOs::Other {
        return Err("images are built on a Windows or Linux host".to_owned());
    }
    for tool in ["packer", "qemu-system-x86_64", "qemu-img", "ssh-keygen"] {
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
    let iso_tools: Vec<&str> = crate::host::facts::PACKER_ISO_TOOLS
        .into_iter()
        .filter(|tool| runner.which(tool).is_some())
        .collect();
    let Some(iso_tool) = crate::host::facts::packer_iso_tool(&iso_tools) else {
        return Err(format!(
            "no ISO builder on PATH, and Packer needs one to make the CD this \
             template hands the guest. It looks for {}. \
             `cargo xtask vm setup` installs one.",
            crate::host::facts::PACKER_ISO_TOOLS.join(", ")
        ));
    };

    let public_key = ensure_ssh_key(runner, &store)?;
    if target == Target::Windows {
        crate::store::windows_media::ensure_iso(runner, &store)?;
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
    let plan = plan(&store, target, accelerator, firmware.as_ref());
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
    println!("  cd images: {iso_tool}");
    println!("  templates: {}", plan.template_dir.display());
    println!("  output:    {}", plan.output_dir.display());
    println!("  this takes tens of minutes and downloads several gigabytes");

    let init = Cmd::new("packer")
        .args([
            "init".to_owned(),
            plan.template_dir.to_string_lossy().into_owned(),
        ])
        .env("PACKER_CACHE_DIR", plan.cache_dir.to_string_lossy());
    let code = runner
        .stream(&init)
        .map_err(|e| format!("cannot run packer: {e}"))?;
    if code != 0 {
        return Err(format!("packer init failed with exit code {code}"));
    }

    let mut vars = plan.vars.clone();
    vars.push(("ssh_public_key".to_owned(), public_key));
    let build = Cmd::new("packer")
        .args(
            BuildPlan {
                vars,
                ..plan.clone()
            }
            .build_args(),
        )
        .env("PACKER_CACHE_DIR", plan.cache_dir.to_string_lossy());
    let code = runner
        .stream(&build)
        .map_err(|e| format!("cannot run packer: {e}"))?;
    if code != 0 {
        return Err(format!(
            "packer build failed with exit code {code}; the output above says why, \
             and `{}` holds what it left behind",
            plan.output_dir.display()
        ));
    }

    finish(runner, &store, target, &plan, &version)
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

    let built = plan.output_dir.join("golden.qcow2");
    let qcow2 = store.qcow2(target);
    move_file(&built, &qcow2)?;

    let mut images = vec![record(&qcow2)?];

    if target == Target::Windows {
        // One canonical install, two disk formats: Hyper-V boots the VHDX and
        // QEMU boots the qcow2, and they are the same Windows.
        let vhdx = store.vhdx(target);
        println!("converting to VHDX for the Hyper-V provider");
        let convert = Cmd::new("qemu-img").args([
            "convert".to_owned(),
            "-p".to_owned(),
            "-O".to_owned(),
            "vhdx".to_owned(),
            "-o".to_owned(),
            "subformat=dynamic".to_owned(),
            qcow2.to_string_lossy().into_owned(),
            vhdx.to_string_lossy().into_owned(),
        ]);
        let code = runner
            .stream(&convert)
            .map_err(|e| format!("cannot run qemu-img: {e}"))?;
        if code != 0 {
            return Err(format!("qemu-img convert failed with exit code {code}"));
        }
        images.push(record(&vhdx)?);
    }

    let template_hash = hash::read_tree(&plan.template_dir)
        .map(|files| hash::template_hash(&files))
        .map_err(|e| format!("cannot hash the templates: {e}"))?;
    let source = match target {
        Target::Windows => store.windows_iso().to_string_lossy().into_owned(),
        Target::Linux => "ubuntu 22.04 cloud image".to_owned(),
    };
    let manifest = manifest_for(
        target,
        template_hash,
        &images,
        util::now_unix(),
        source,
        builder.to_owned(),
    );
    std::fs::write(store.manifest(target), manifest.to_json())
        .map_err(|e| format!("cannot write the manifest: {e}"))?;

    let _ = std::fs::remove_dir_all(&plan.output_dir);

    println!();
    println!("the {target} golden image is built:");
    for (file, bytes, _) in &images {
        println!("  {file}  {}", util::format_bytes(*bytes));
    }
    if target.has_eval_expiry() {
        println!(
            "  the evaluation clock started now and runs {} days",
            crate::store::manifest::EVAL_TOTAL_DAYS
        );
    }
    println!("`cargo xtask vm status` lists it; `cargo xtask e2e --target {target}` uses it.");
    Ok(0)
}

fn record(path: &Path) -> Result<(String, u64, String), String> {
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
fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    if !from.is_file() {
        return Err(format!(
            "packer reported success but produced no {}",
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
