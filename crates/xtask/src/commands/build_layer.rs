//! `cargo xtask vm build-image <layer>`: a differencing child of another image,
//! provisioned over SSH.
//!
//! Plan decision 2. The Windows builder is the Windows desktop image plus a
//! toolchain, and the cheapest way to have both is the copy-on-write mechanism
//! the runtime overlays already use, kept instead of thrown away: a differencing
//! child holds only what the provisioning wrote, which is gigabytes rather than
//! the fifteen a second Windows install would cost, and it takes minutes rather
//! than an hour because there is no install in it: four and six on this host, over
//! two builds.
//!
//! Nothing here creates media, answers a boot prompt, or watches an installer.
//! Everything below the toolchain is inherited from the parent, including the
//! job task, the session marker, the autologon, the firewall rule and the SSH
//! host keys, which is also why the guest contract needs no new code for a
//! builder: `job::run` cannot tell one of these guests from a desktop one.
//!
//! What a layer costs instead is an identity. It cannot be read without the
//! exact disk it was made from, so the manifest records the parent's checksum
//! and the inventory refuses to boot a layer whose parent moved
//! ([`ImageCondition::Detached`](crate::store::inventory::ImageCondition)).

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::commands::{build_hyperv, build_image, vm};
use crate::guest::toolchain::Toolchain;
use crate::provider::target::{HostOs, Image, ProviderKind, Target};
use crate::provider::{self, Provider, hyperv, qemu};
use crate::runner::{Cmd, Runner};
use crate::store::manifest::ParentRecord;
use crate::store::state::{RunState, StartReason};
use crate::store::{self, Store};
use crate::util;

/// The script that installs the toolchain, relative to the image's template
/// directory.
pub const TOOLCHAIN_SCRIPT: &str = "scripts/toolchain.ps1";

/// The script that proves it took.
pub const FINALIZE_SCRIPT: &str = "scripts/finalize.ps1";

/// How long to wait for the guest's SSH server after the boot.
pub const BOOT_TIMEOUT: Duration = Duration::from_mins(10);

/// Where `toolchain.ps1` puts libclang, and therefore what a build job in this
/// layer points `LIBCLANG_PATH` at.
///
/// A constant rather than a search, because the search is what bindgen would do
/// and what it does badly: with nothing to go on it looks for a clang
/// installation, and this layer deliberately has only the one DLL and the
/// headers beside it. `the_layer_puts_libclang_where_the_job_looks_for_it` reads
/// both scripts and this constant, since nothing else connects them.
pub const LIBCLANG_DIR: &str = r"C:\tools\llvm\bin";

/// Where a script is copied to inside a Windows guest.
///
/// Copied rather than encoded onto the command line, for the reason
/// `build_hyperv::finalize_destination` gives: `cmd.exe` truncates a command
/// line at eight kilobytes and these scripts are longer than that.
pub fn script_destination(name: &str) -> String {
    format!(r"{}\{name}", crate::provider::GUEST_ROOT_WINDOWS)
}

/// The command that runs one of them there, with the channel as its argument.
pub fn script_command(destination: &str, channel: &str) -> String {
    format!(
        "powershell.exe -NoProfile -ExecutionPolicy Bypass -File {destination} -Channel {channel}"
    )
}

/// Build one layer end to end.
pub fn run(runner: &dyn Runner, store: &Store, image: Image, host: HostOs) -> Result<u8, String> {
    let parent = image
        .parent()
        .ok_or_else(|| format!("{image} is not a layer over anything"))?;
    if image.target() != Target::Windows {
        return Err(format!(
            "{image} is a layer over a {} image, and only the Windows layer is \
             provisioned this way",
            image.target()
        ));
    }

    let pinned = crate::guest::toolchain::pinned()?;
    let template_dir = store::template_dir(image);
    for script in [TOOLCHAIN_SCRIPT, FINALIZE_SCRIPT] {
        let path = template_dir.join(script);
        if !path.is_file() {
            return Err(format!("no {} to provision the layer with", path.display()));
        }
    }

    let backing = check_parent(store, parent, image, host)?;
    build_image::ensure_ssh_key(runner, store)?;
    vm::check_no_other_vm(runner, store, image)?;
    vm::clear_stale_state(runner, store, image)?;

    let build_dir = store.build_dir(image);
    std::fs::create_dir_all(&build_dir)
        .map_err(|e| format!("cannot create {}: {e}", build_dir.display()))?;
    let staged = build_dir.join(backing.child_name);

    let (memory_mb, cpus) = provider::resources_for(image);
    println!("provisioning the {image} layer over the {parent} image");
    println!("  parent:   {}", backing.disk.display());
    println!("  layer:    {}", staged.display());
    println!(
        "  guest:    {cpus} vCPUs, {}",
        util::format_bytes(u64::from(memory_mb) * 1024 * 1024)
    );
    println!("  toolchain: {}", pinned.channel);
    println!("  this takes five minutes or so, most of it the Visual Studio installer");

    let outcome = provision(
        runner,
        store,
        image,
        &backing,
        &staged,
        &template_dir,
        &pinned,
    );
    build_image::forget_host_keys(store);
    outcome?;

    finish(runner, store, image, parent, &backing, &staged)
}

/// What the parent is, and in which format the child has to be made.
struct Backing {
    /// The parent's disk this layer becomes a child of.
    disk: PathBuf,
    /// Its name inside the parent's image directory, which is what the
    /// manifest's parent record names and the inventory looks up.
    file: String,
    /// What the child is called while it is being built.
    child_name: &'static str,
    /// What the parent's manifest records for that file.
    checksum: String,
    /// The parent's build time, which is when the evaluation clock started.
    built_unix: u64,
    provider: ProviderKind,
}

/// Refuse before anything boots if the parent cannot be layered over.
///
/// Three things have to hold, and each of them fails differently later: the
/// parent has to be in a condition a guest may boot from, its manifest has to
/// record the disk this child will name, and the disk has to be in the format of
/// the hypervisor this host runs the guest on. The last is decision 3's "one
/// format per host": converting a differencing child between formats means
/// flattening it through a full copy of its parent.
fn check_parent(
    store: &Store,
    parent: Image,
    image: Image,
    host: HostOs,
) -> Result<Backing, String> {
    let provider = provider::target::provider_for(host, parent.target()).ok_or_else(|| {
        format!(
            "no hypervisor for a {} guest on a {} host",
            parent.target(),
            host.name()
        )
    })?;
    let (disk, file, child_name) = match provider {
        ProviderKind::HyperV => (store.vhdx(parent), "golden.vhdx", "layer.vhdx"),
        ProviderKind::Qemu => (store.qcow2(parent), "golden.qcow2", "layer.qcow2"),
    };

    let inventory = crate::store::inventory::scan(store);
    let entry = inventory
        .for_image(parent)
        .ok_or_else(|| format!("nothing is known about the {parent} image"))?;
    let condition = entry.condition(util::now_unix());
    if condition.blocks_boot() {
        return Err(format!(
            "the {parent} image is {}: {}.\n\n\
             A layer is a differencing child of it, so it cannot be built over an \
             image nothing may boot. `cargo xtask vm build-image {parent}` rebuilds it.",
            condition.label(),
            condition.detail()
        ));
    }
    if !disk.is_file() {
        return Err(format!(
            "no {} to layer over; this host runs a {} guest on {}, so that is the \
             format the layer has to be a child of",
            disk.display(),
            parent.target(),
            provider.name()
        ));
    }
    let manifest = entry.manifest.as_ref().ok_or_else(|| {
        format!(
            "the {parent} image has no readable manifest, and a layer records its \
             parent's checksum out of that manifest. `cargo xtask vm build-image \
             {parent}` writes one."
        )
    })?;
    let record = manifest.record_for(file).ok_or_else(|| {
        format!(
            "the {parent} manifest does not record {file}, so there would be nothing \
             for the {image} layer to check itself against later"
        )
    })?;

    Ok(Backing {
        disk,
        file: file.to_owned(),
        child_name,
        checksum: record.checksum.clone(),
        built_unix: manifest.built_unix,
        provider,
    })
}

/// Everything from "the child disk exists" to "the guest is gone and the disk is
/// finished", behind one boundary the caller turns into a message.
fn provision(
    runner: &dyn Runner,
    store: &Store,
    image: Image,
    backing: &Backing,
    staged: &Path,
    template_dir: &Path,
    pinned: &Toolchain,
) -> Result<(), String> {
    let mut state = RunState::new(
        image,
        backing.provider,
        staged.to_path_buf(),
        StartReason::Build,
        util::now_unix(),
    );
    // Written before the disk exists, for the reason every other boot writes it
    // early: a guest with no record is invisible to `vm status` and `vm down`,
    // and a differencing child with nothing recording it is a file nothing knows
    // is a build.
    vm::write_state(store, image, &state)?;

    let host = HostOs::current();
    let provider: Box<dyn Provider + '_> = match backing.provider {
        ProviderKind::HyperV => Box::new(hyperv::HypervProvider::new(runner, store, host)),
        ProviderKind::Qemu => Box::new(qemu::QemuProvider::new(runner, store, host)),
    };

    create_child(runner, store, image, backing, staged, &state, host)?;
    fill_in_address(&mut state, backing.provider)?;
    provider.start(&mut state)?;
    vm::write_state(store, image, &state)?;
    println!(
        "{} is running; waiting for it to answer on SSH",
        state.vm_name
    );

    let outcome = provider
        .wait_ssh(&state, BOOT_TIMEOUT)
        .and_then(|elapsed| {
            println!("  SSH answered after {:.0}s", elapsed.as_secs_f64());
            provision_guest(provider.as_ref(), &state, template_dir, pinned)
        })
        .and_then(|()| shut_down(runner, store, provider.as_ref(), &state, backing.provider));
    if outcome.is_err() {
        // Kept, not removed: a failed provisioning is worth looking inside, and
        // the record written above is what makes it reachable.
        println!("{}", left_behind(image, &state));
    }
    outcome
}

/// Make the differencing child, and on `Hyper-V` the VM that boots it.
///
/// The two hypervisors divide this differently. `New-VHD -Differencing` and
/// `New-VM` are one script, because a VM is a registered object either way and
/// the create script is where every setting on it belongs; QEMU's disk is a file
/// and its machine is a command line built at start.
fn create_child(
    runner: &dyn Runner,
    store: &Store,
    image: Image,
    backing: &Backing,
    staged: &Path,
    state: &RunState,
    host: HostOs,
) -> Result<(), String> {
    let _ = std::fs::remove_file(staged);
    match backing.provider {
        ProviderKind::HyperV => {
            // A leftover VM of ours would hold the child open.
            let _ = build_hyperv::run_script(runner, &hyperv::destroy_script(&state.vm_name));
            let console = hyperv::HypervProvider::new(runner, store, host).console_size();
            build_hyperv::run_script(
                runner,
                &hyperv::create_script(
                    &state.vm_name,
                    &backing.disk.to_string_lossy(),
                    &staged.to_string_lossy(),
                    console,
                    provider::resources_for(image),
                ),
            )
            .map(|_| ())
        }
        ProviderKind::Qemu => {
            let qemu_img =
                crate::host::facts::resolve_tool(runner, "qemu-img", host).ok_or_else(|| {
                    "qemu-img is not available; run `cargo xtask vm doctor`".to_owned()
                })?;
            let out = runner
                .capture(
                    &Cmd::new(qemu_img.to_string_lossy())
                        .args(qemu::overlay_args(&backing.disk, staged)),
                )
                .map_err(|e| format!("cannot run qemu-img: {e}"))?;
            if out.success() {
                Ok(())
            } else {
                Err(format!("qemu-img create failed: {}", out.stderr.trim()))
            }
        }
    }
}

/// Record how the guest will be reached, which differs by hypervisor: a
/// `Hyper-V` guest is on the NAT switch at port 22 and its address is read after
/// it boots, and a QEMU guest is three forwarded loopback ports picked now.
fn fill_in_address(state: &mut RunState, kind: ProviderKind) -> Result<(), String> {
    match kind {
        ProviderKind::HyperV => {
            state.ssh_port = 22;
            hyperv::GUEST_USER.clone_into(&mut state.ssh_user);
            Ok(())
        }
        ProviderKind::Qemu => qemu::fill_in_address(state),
    }
}

/// Copy both scripts in and run them.
///
/// The toolchain script first, then the one that proves it took, which is the
/// same order and the same division of labour the image builds use: a layer
/// without `rc.exe` in it would otherwise be found by a release build failing in
/// `embed-resource` twenty minutes into its compile.
fn provision_guest(
    provider: &dyn Provider,
    state: &RunState,
    template_dir: &Path,
    pinned: &Toolchain,
) -> Result<(), String> {
    for (script, what) in [
        (TOOLCHAIN_SCRIPT, "installing the toolchain"),
        (FINALIZE_SCRIPT, "checking what it installed"),
    ] {
        let local = template_dir.join(script);
        let name = local
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let destination = script_destination(&name);
        println!("{what} ({name})");
        provider.copy_in(state, &local, &destination)?;
        let out = provider.exec(state, &script_command(&destination, &pinned.channel))?;
        print!("{}", out.stdout);
        if !out.success() {
            return Err(format!(
                "{name} exited {:?} inside the guest, so the layer is not usable. \
                 Its output is above; {} has the rest.",
                out.code,
                out.stderr.trim()
            ));
        }
    }
    Ok(())
}

/// Shut the guest down and get rid of the machine, keeping the disk.
///
/// The disk is the product here, so nothing may touch it until the guest is off:
/// a differencing child that was still being written to is not an image, and
/// neither hypervisor will say so afterwards.
fn shut_down(
    runner: &dyn Runner,
    store: &Store,
    provider: &dyn Provider,
    state: &RunState,
    kind: ProviderKind,
) -> Result<(), String> {
    match kind {
        ProviderKind::HyperV => {
            build_hyperv::shut_down(runner, store, state)?;
            build_hyperv::run_script(
                runner,
                &build_hyperv::remove_keeping_disk_script(&state.vm_name),
            )?;
            // Confirmed rather than assumed: the script tolerates a VM that is
            // already gone, so its success alone does not prove this one is.
            let after = build_hyperv::run_script(runner, &hyperv::state_script(&state.vm_name))?;
            if !hyperv::answered(&after) || hyperv::parse_state(&after).is_some() {
                return Err(format!(
                    "{} may still be registered after Remove-VM, so its disk cannot \
                     be moved out from under it. Hyper-V said: {}",
                    state.vm_name,
                    after.trim()
                ));
            }
            println!("{} is removed and its disk is kept", state.vm_name);
        }
        ProviderKind::Qemu => {
            // The connection dies with the session this starts, so its result
            // says nothing; whether the process is gone is the only answer that
            // counts, and `destroy` is what waits for it.
            let _ = provider.exec(state, build_hyperv::SHUTDOWN_COMMAND);
            provider.destroy(state)?;
            println!("{} has stopped and its disk is kept", state.vm_name);
        }
    }
    // The machine is gone and the disk is about to move: the record describes
    // neither any more. Deleted after the teardown succeeded, never before, so a
    // failure above leaves something `vm status` can still see.
    if let Some(image) = state.image() {
        let _ = std::fs::remove_file(store.state_file(image));
    }
    Ok(())
}

/// Give back the blocks the guest freed, before the manifest measures the file.
///
/// The guest's own `finalize.ps1` deletes what a build does not read and then
/// retrims the volume, which is what tells the virtual disk those blocks are
/// free; this is the half that shrinks the file. It runs after the move and
/// before `record`, so the manifest's size and checksum describe the disk as it
/// will be read, and it is best effort: a layer that could not be compacted is a
/// larger layer and not a failed build.
///
/// Hyper-V only. A qcow2 layer exists only on a Linux host, where the guest's
/// discards reach a sparse file directly and nothing on the host has to be run;
/// compacting one would mean `qemu-img convert` into a copy and a rename over the
/// original, which is the step the Debian template already turns off.
fn compact(runner: &dyn Runner, disk: &Path, provider: ProviderKind) {
    if provider != ProviderKind::HyperV {
        return;
    }
    let before = std::fs::metadata(disk).map(|m| m.len()).unwrap_or_default();
    println!("compacting {}", disk.display());
    // Not through `run_script`, whose failure is a build failure. `Optimize-VHD`
    // needs the Hyper-V module and a disk nothing has attached, and both are
    // conditions this step can lose without costing anything but bytes.
    let outcome = runner
        .capture(&crate::runner::powershell(&compact_script(disk)))
        .map_err(|e| e.to_string())
        .and_then(|out| {
            if out.success() {
                Ok(())
            } else {
                Err(out.stderr.trim().to_owned())
            }
        });
    match outcome {
        Ok(()) => {
            let after = std::fs::metadata(disk).map(|m| m.len()).unwrap_or_default();
            println!(
                "  {} -> {}",
                util::format_bytes(before),
                util::format_bytes(after)
            );
        }
        Err(e) => println!("  could not compact it ({e}); the layer keeps its full size"),
    }
}

/// What compacting one asks Hyper-V for.
///
/// `Full` rather than the default `Quick`: quick mode gives back only the blocks
/// the disk's own allocation table already calls free, and what the guest just
/// did was mark blocks free in its file system. Full mode mounts the disk
/// read-only and reads that file system, which is the only way the two agree.
pub fn compact_script(disk: &Path) -> String {
    format!(
        "Optimize-VHD -Path {} -Mode Full -ErrorAction Stop",
        crate::runner::ps_quote(disk)
    )
}

/// Move the finished layer into the image store and write its manifest.
fn finish(
    runner: &dyn Runner,
    store: &Store,
    image: Image,
    parent: Image,
    backing: &Backing,
    staged: &Path,
) -> Result<u8, String> {
    let image_dir = store.image_dir(image);
    std::fs::create_dir_all(&image_dir)
        .map_err(|e| format!("cannot create {}: {e}", image_dir.display()))?;
    let final_disk = match backing.provider {
        ProviderKind::HyperV => store.vhdx(image),
        ProviderKind::Qemu => store.qcow2(image),
    };
    if !staged.is_file() {
        return Err(format!(
            "the provisioning finished and produced no {}",
            staged.display()
        ));
    }
    build_image::move_file(staged, &final_disk)?;
    compact(runner, &final_disk, backing.provider);

    let images = vec![build_image::record(&final_disk)?];
    let template_hash = crate::store::hash::read_tree(&store::template_dir(image))
        .map(|files| crate::store::hash::template_hash(&files))
        .map_err(|e| format!("cannot hash the templates: {e}"))?;
    let manifest = crate::store::manifest::Manifest::new(
        image,
        template_hash,
        util::now_unix(),
        format!("{}/{}", parent.slug(), backing.file),
        format!("xtask layer over {parent}"),
        images
            .iter()
            .map(
                |(file, bytes, checksum)| crate::store::manifest::ImageRecord {
                    file: file.clone(),
                    bytes: *bytes,
                    checksum: checksum.clone(),
                },
            )
            .collect(),
    )
    .with_parent(ParentRecord {
        image: parent.slug().to_owned(),
        file: backing.file.clone(),
        checksum: backing.checksum.clone(),
        built_unix: backing.built_unix,
    });
    std::fs::write(store.manifest(image), manifest.to_json())
        .map_err(|e| format!("cannot write the manifest: {e}"))?;

    build_image::announce(image, &images);
    println!(
        "it is a differencing child of {}, so neither file can be moved or rebuilt \
         without the other",
        backing.disk.display()
    );
    if let Some(state) = manifest.eval_state(image, util::now_unix()) {
        println!("  {}", state.summary());
    }
    Ok(0)
}

/// What is still there after a failure, and how to reach it.
fn left_behind(image: Image, state: &RunState) -> String {
    format!(
        "{vm} is still there with the unfinished layer attached.\n  \
         ssh:     cargo xtask vm ssh {image}\n  \
         {console}: cargo xtask vm view {image}\n  \
         down:    cargo xtask vm down {image}  (removes the VM and the unfinished disk)",
        vm = state.vm_name,
        console = image.console_label()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guest_runs_the_script_with_the_channel_as_its_argument() {
        let destination = script_destination("toolchain.ps1");
        assert_eq!(destination, r"C:\sunlit-e2e\toolchain.ps1");
        let command = script_command(&destination, "1.94.0");
        assert!(command.contains("-ExecutionPolicy Bypass"), "{command}");
        assert!(
            command.contains(r"-File C:\sunlit-e2e\toolchain.ps1"),
            "{command}"
        );
        assert!(command.contains("-Channel 1.94.0"), "{command}");
    }

    /// The scripts are copied rather than encoded onto the command line, for the
    /// reason the image build's finalize step is: `cmd.exe`, which is the
    /// guest's SSH shell, truncates a command line at eight kilobytes.
    #[test]
    fn both_scripts_land_in_the_guest_root() {
        for script in [TOOLCHAIN_SCRIPT, FINALIZE_SCRIPT] {
            let name = Path::new(script)
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .into_owned();
            assert!(
                script_destination(&name).starts_with(crate::provider::GUEST_ROOT_WINDOWS),
                "{script}"
            );
        }
    }

    /// Three places have to agree about where libclang is: the script that puts
    /// it there, the script that checks it, and the constant the build job sets
    /// `LIBCLANG_PATH` from. Nothing else connects them, and the cost of a
    /// disagreement is a build that fails in bindgen forty minutes in.
    #[test]
    fn the_layer_puts_libclang_where_the_job_looks_for_it() {
        let dir = crate::store::template_dir(Image::WindowsBuilder);
        for script in [TOOLCHAIN_SCRIPT, FINALIZE_SCRIPT] {
            let text = std::fs::read_to_string(dir.join(script))
                .unwrap_or_else(|e| panic!("cannot read {script}: {e}"));
            assert!(
                text.contains(LIBCLANG_DIR),
                "{script} does not name {LIBCLANG_DIR}"
            );
        }
    }

    /// Both scripts take the channel as a named parameter, because the command
    /// that runs them passes one. A script without the parameter would take the
    /// argument as positional and ignore it, which reads as a toolchain that
    /// installed the wrong version for no visible reason.
    #[test]
    fn both_scripts_declare_the_channel_parameter_the_command_passes() {
        let dir = crate::store::template_dir(Image::WindowsBuilder);
        for script in [TOOLCHAIN_SCRIPT, FINALIZE_SCRIPT] {
            let text = std::fs::read_to_string(dir.join(script))
                .unwrap_or_else(|e| panic!("cannot read {script}: {e}"));
            assert!(text.contains("[string]$Channel"), "{script}");
        }
        assert!(script_command("x.ps1", "1.94.0").contains("-Channel 1.94.0"));
    }

    #[test]
    fn a_failure_names_the_guest_and_the_way_out() {
        let state = RunState::new(
            Image::WindowsBuilder,
            ProviderKind::HyperV,
            PathBuf::from("C:/vm/build/windows-builder/layer.vhdx"),
            StartReason::Build,
            0,
        );
        let text = left_behind(Image::WindowsBuilder, &state);
        assert!(text.contains("sunlit-e2e-windows-builder"), "{text}");
        assert!(text.contains("vm ssh windows-builder"), "{text}");
        assert!(text.contains("vm down windows-builder"), "{text}");
        // The word in front of the view line is the image's, not this text's:
        // the guest here is a differencing child of the Windows desktop image
        // and opens the session its parent was built with.
        assert!(
            text.contains("desktop: cargo xtask vm view windows-builder"),
            "{text}"
        );
        for image in Image::ALL {
            let text = left_behind(image, &state);
            let expected = format!("{}: cargo xtask vm view {image}", image.console_label());
            assert!(text.contains(&expected), "{text}");
        }
    }

    #[test]
    fn only_a_vhdx_layer_is_compacted_on_the_host() {
        let disk = PathBuf::from("C:/vm/images/windows-builder/layer.vhdx");

        let script = compact_script(&disk);
        assert!(script.contains("Optimize-VHD"), "{script}");
        // Quick mode gives back nothing here: what the guest did was mark blocks
        // free in its own file system.
        assert!(script.contains("-Mode Full"), "{script}");
        assert!(
            script.contains("'C:/vm/images/windows-builder/layer.vhdx'"),
            "{script}"
        );

        let runner = crate::runner::fake::FakeRunner::new()
            .on("Optimize-VHD", crate::runner::CommandOutput::ok(""));
        compact(&runner, &disk, ProviderKind::HyperV);
        assert_eq!(runner.calls().len(), 1);

        // A qcow2 layer's discards reach a sparse file with nothing on the host
        // to run, so this must issue no command rather than a failing one.
        let runner = crate::runner::fake::FakeRunner::new();
        compact(&runner, &disk, ProviderKind::Qemu);
        assert!(runner.calls().is_empty(), "{:?}", runner.calls());
    }
}
