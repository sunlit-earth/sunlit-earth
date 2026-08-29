//! `cargo xtask vm down` and `vm purge`: the cleanup pair to `vm status`.
//!
//! Plan decision 13, under the names the developer using it asked for. `down`
//! ends a guest and deletes its run state, which is cheap and never costs an
//! image rebuild. `purge` deletes what is on disk: the golden image, the
//! converted VHDX, the manifest, the build leftovers, and the cached
//! installation media, which is the disk-space recovery path. Flags narrow a
//! purge to one of those; on its own it means all of them, and it asks first.
//!
//! Both commands share one planner, because they differ only in [`Scope`].
//!
//! Selecting what to delete is a pure function, because the cost of getting it
//! wrong is somebody else's virtual machine. Nothing outside the image store is
//! ever deleted, and nothing without the `sunlit-e2e-` prefix is ever stopped.

use std::fmt::Write as _;
use std::io::{BufRead, Write as _WriteIo};
use std::path::PathBuf;

use crate::provider::target::Image;
use crate::store::Store;
use crate::store::inventory::{FileInfo, Inventory};
use crate::store::state::{RunState, StartReason};
use crate::util::format_bytes;

/// What a teardown deletes.
///
/// The three parts have very different costs to lose: run state is recreated by
/// the next boot, an image is a build of tens of minutes, and the installation
/// media is a 6.6 GB download. So they are separable, and `down` is exactly the
/// cheapest one.
///
/// Four independent answers to four independent questions rather than a state
/// machine, which is why `struct_excessive_bools` and `fn_params_excessive_bools`
/// are allowed here: naming them together is the point, and the flags being
/// additive is what makes "none of them" mean all of it.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scope {
    /// The guest itself, its throwaway overlay, and its run state.
    pub vm: bool,
    /// The golden image, its manifest, and Packer's build leftovers.
    pub image: bool,
    /// Cached installation media, which is the Windows evaluation ISO.
    pub iso: bool,
    /// The build cache an earlier `dist` left on this host, which is the one
    /// part `vm down` never takes (decision 28).
    pub cache: bool,
}

impl Scope {
    /// What `vm down` does.
    ///
    /// The build cache is deliberately not in it: run state is what the next
    /// boot recreates, and a cache is minutes of compiling that nothing
    /// recreates without doing the compiling again.
    pub const RUN_STATE: Self = Self {
        vm: true,
        image: false,
        iso: false,
        cache: false,
    };

    /// What `vm purge` does when nothing narrows it.
    pub const EVERYTHING: Self = Self {
        vm: true,
        image: true,
        iso: true,
        cache: true,
    };

    /// A purge's flags. None of them means all of it, which is the documented
    /// default and the reason the flags are additive rather than exclusive.
    #[allow(clippy::fn_params_excessive_bools)]
    pub fn from_flags(vm: bool, image: bool, iso: bool, cache: bool) -> Self {
        if vm || image || iso || cache {
            Self {
                vm,
                image,
                iso,
                cache,
            }
        } else {
            Self::EVERYTHING
        }
    }

    /// Whether a running guest has to be stopped for this scope.
    ///
    /// Its overlay is obviously held open by it, and so, for `Hyper-V`, is the
    /// golden image its differencing child was made from. Media is the one that
    /// depends on what the guest is: a booted test guest has long finished with
    /// the ISO, and a build VM has both DVDs attached for the whole install, so
    /// deleting the media under one means deleting a file Windows has open.
    /// Windows refuses that and the refusal is reported, but a purge that has
    /// to be run twice for a reason nothing explained is not the answer either,
    /// so a media purge ends a build first.
    pub fn needs_the_vm_stopped(self, reason: Option<StartReason>) -> bool {
        self.vm || self.image || (self.iso && reason == Some(StartReason::Build))
    }

    /// The parts, in words, for a prompt or a report.
    pub fn label(self) -> String {
        let parts: Vec<&str> = [
            self.vm.then_some("the VM and its run state"),
            self.image.then_some("the golden image"),
            self.iso.then_some("the installation media"),
            self.cache.then_some("the build cache"),
        ]
        .into_iter()
        .flatten()
        .collect();
        match parts.as_slice() {
            [] => "nothing".to_owned(),
            [one] => (*one).to_owned(),
            [first, second] => format!("{first} and {second}"),
            rest => format!(
                "{}, and {}",
                rest[..rest.len() - 1].join(", "),
                rest[rest.len() - 1]
            ),
        }
    }
}

/// Which images a teardown applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    One(Image),
    All,
}

impl Selection {
    pub fn includes(self, image: Image) -> bool {
        match self {
            Self::One(one) => one == image,
            Self::All => true,
        }
    }

    /// Whether this image is reached because a base it is a layer over was
    /// named, and the disks themselves are going.
    ///
    /// A layer is a differencing child: without the disk it was made from it is
    /// not a smaller image, it is an unreadable file. So a purge that takes a
    /// base takes its layers with it, and says so before it asks.
    pub fn reaches_as_a_layer(self, image: Image, scope: Scope) -> bool {
        scope.image
            && !self.includes(image)
            && image.parent().is_some_and(|parent| self.includes(parent))
    }

    pub fn label(self) -> String {
        match self {
            Self::One(image) => image.to_string(),
            Self::All => "all".to_owned(),
        }
    }
}

/// One file the plan intends to delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteItem {
    pub path: PathBuf,
    pub bytes: u64,
    /// The image whose VM holds this file, when one does. A file belonging to
    /// a VM that could not be stopped is not deleted, so this is what connects
    /// the two halves of a teardown.
    pub image: Option<Image>,
}

/// What a teardown would do, decided before anything is touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeardownPlan {
    pub selection: Selection,
    pub scope: Scope,
    /// VMs to stop and unregister first.
    pub vms: Vec<RunState>,
    pub files: Vec<DeleteItem>,
    /// Directories to remove once they are empty. Best effort: a directory that
    /// still holds something unexpected is left alone.
    pub dirs: Vec<PathBuf>,
    /// Things deliberately not touched, and why.
    pub refused: Vec<String>,
    /// Layers reached because the base they are children of was named, which is
    /// the one thing in the listing that a reader would otherwise take for a
    /// command deleting more than it was asked to.
    pub layers: Vec<Image>,
}

impl TeardownPlan {
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.vms.is_empty() && self.files.is_empty()
    }

    /// What this plan ends a build over and then leaves lying there, in words.
    ///
    /// One case reaches this, and it is the reason `cost_of_ending` exists: a
    /// media purge during a build stops the build (departure 45) while its scope
    /// does not include run state, so the record and the disk the install had
    /// written stay where they are. `vm status` afterwards reads as a crashed
    /// build with nothing having said why. Widening `--iso` to delete them
    /// instead would make a flag that narrows a purge delete more than it names,
    /// so the plan says what it leaves and which command takes it.
    fn left_behind(&self) -> Vec<String> {
        if self.scope.vm {
            return Vec::new();
        }
        self.vms
            .iter()
            .filter(|vm| vm.reason == StartReason::Build)
            .map(|vm| {
                format!(
                    "{}'s record and the disk its install had written, because this \
                     purge does not take run state; `cargo xtask vm down {}` clears \
                     both",
                    vm.vm_name, vm.image
                )
            })
            .collect()
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        if self.is_empty() {
            let _ = writeln!(
                out,
                "nothing to remove for {} ({})",
                self.selection.label(),
                self.scope.label()
            );
        }
        for vm in &self.vms {
            let _ = write!(out, "stop {} ({})", vm.vm_name, vm.provider);
            // What ending it costs, on the line that says it is being ended.
            // This is printed whether or not there is a question afterwards,
            // which is what makes `--force` a way to skip the question rather
            // than a way to end an hour-long install in silence.
            if let Some(cost) = vm.reason.cost_of_ending() {
                let _ = write!(out, ", which {cost}");
            }
            let _ = writeln!(out);
        }
        for file in &self.files {
            let _ = writeln!(
                out,
                "delete {} ({})",
                file.path.display(),
                format_bytes(file.bytes)
            );
        }
        if !self.files.is_empty() {
            let _ = writeln!(
                out,
                "that frees {} across {}",
                format_bytes(self.bytes()),
                crate::util::count(self.files.len(), "file")
            );
        }
        for layer in &self.layers {
            let _ = writeln!(
                out,
                "the {layer} layer goes with it: it holds only what its own \
                 build changed, so without the disk it is a child of it is not \
                 a smaller image but an unreadable file"
            );
        }
        for kept in self.left_behind() {
            let _ = writeln!(out, "kept: {kept}");
        }
        for refusal in &self.refused {
            let _ = writeln!(out, "skipped: {refusal}");
        }
        out
    }
}

/// Decide what a teardown would touch.
#[allow(clippy::too_many_lines)]
pub fn plan(
    store: &Store,
    inventory: &Inventory,
    selection: Selection,
    scope: Scope,
) -> TeardownPlan {
    let mut vms = Vec::new();
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let mut refused = Vec::new();
    let mut layers = Vec::new();

    let take = |file: &FileInfo,
                owner: Option<Image>,
                files: &mut Vec<DeleteItem>,
                refused: &mut Vec<String>| {
        if store.contains(&file.path) {
            files.push(DeleteItem {
                path: file.path.clone(),
                bytes: file.bytes,
                image: owner,
            });
        } else {
            refused.push(format!(
                "{} is outside the image store and was not deleted",
                file.path.display()
            ));
        }
    };

    for image in Image::ALL {
        let named = selection.includes(image);
        let as_a_layer = selection.reaches_as_a_layer(image, scope);
        if !named && !as_a_layer {
            continue;
        }
        // A layer taken along with its parent goes whole: its own overlay and
        // run state are worth nothing once the disk they were children of is
        // gone. The media is not its business either way.
        let scope = if named {
            scope
        } else {
            Scope {
                vm: true,
                image: true,
                iso: false,
                // A cache keyed to an image that is going is dead weight the
                // next build would refuse to restore anyway.
                cache: true,
            }
        };
        let Some(entry) = inventory.for_image(image) else {
            continue;
        };
        if as_a_layer {
            layers.push(image);
        }

        if scope.needs_the_vm_stopped(entry.state.as_ref().map(|state| state.reason)) {
            if let Some(state) = &entry.state {
                if state.is_ours() {
                    vms.push(state.clone());
                } else {
                    refused.push(format!(
                        "{} is not one of ours and was left alone",
                        state.vm_name
                    ));
                }
            }
            if let Some(error) = &entry.state_error {
                refused.push(format!("{image}: {error}"));
            }
        }

        if scope.vm {
            for file in &entry.run_files {
                take(file, Some(image), &mut files, &mut refused);
            }
            dirs.push(store.run_dir(image));
        }

        if scope.cache {
            for file in &entry.cache_files {
                take(file, Some(image), &mut files, &mut refused);
            }
            dirs.push(store.cache_dir(image));
        }

        if scope.image {
            // Images and build leftovers are not held open by a running VM,
            // but they are still that image's, so a VM that would not stop
            // keeps them too: a half-purged image is a state nothing can
            // recover from, and `vm status` would report the image as missing
            // while the VM still ran from it.
            for file in entry.images.iter().chain(entry.build_files.iter()) {
                take(file, Some(image), &mut files, &mut refused);
            }
            // The manifest goes last, and `execute` stops at a image's first
            // failure, so the two orderings that a partial purge can leave
            // behind are "images gone, manifest still there", which reads as
            // missing and is correct, and "nothing deleted at all". The order
            // that must not happen is the manifest going first, which would
            // leave images nobody can date: that is the state a boot now
            // refuses on.
            let manifest = store.manifest(image);
            if let Some(bytes) = entry.manifest_bytes {
                files.push(DeleteItem {
                    path: manifest,
                    bytes,
                    image: Some(image),
                });
            }
            dirs.push(store.image_dir(image));
            dirs.push(store.build_dir(image));
        }
    }

    if scope.iso {
        // Installation media is shared, so it only goes when the image that
        // consumes it does. The Windows evaluation ISO is the only cached
        // download; the Debian cloud image is fetched into the build directory.
        // Attributed to Windows rather than to nothing. It is only ever
        // deleted when Windows is in scope, so a Windows teardown that failed
        // has to keep it too: otherwise a destroy that reported "nothing was
        // deleted for that image" would have quietly removed the 6.6 GB
        // download the rebuild needs, and the guide's promise that a failed
        // destroy is safe to repeat would be false.
        if selection.includes(Image::Windows) {
            for file in &inventory.iso {
                take(file, Some(Image::Windows), &mut files, &mut refused);
            }
            dirs.push(store.iso_dir());
        }
    }

    TeardownPlan {
        selection,
        scope,
        vms,
        files,
        dirs,
        refused,
        layers,
    }
}

/// What to ask before a purge.
///
/// The plan's own rendering has already listed every file above it, so this is
/// the question rather than the inventory: the point of asking at all is that a
/// purge deletes things that cost tens of minutes to build or a six-gigabyte
/// download to fetch again.
///
/// The one thing the scope and the byte count cannot say is what stopping a
/// guest costs, and for a build that is more than everything else in the
/// question put together, so it is asked about here as well as printed above.
pub fn confirmation_prompt(plan: &TeardownPlan) -> String {
    let mut costs = String::new();
    for vm in &plan.vms {
        if let Some(cost) = vm.reason.cost_of_ending() {
            let _ = write!(costs, " That also stops {}, which {cost}.", vm.vm_name);
        }
    }
    format!(
        "delete {} for {}, freeing {}?{costs} [y/N] ",
        plan.scope.label(),
        plan.selection.label(),
        format_bytes(plan.bytes())
    )
}

/// Whether an answer to that question is a yes.
///
/// Anything that is not plainly yes is a no, including an empty line, which is
/// what the capital N in the prompt promises.
pub fn is_affirmative(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Give the console back line input and echo before reading an answer.
///
/// The input mode belongs to the terminal session, not to this process, and
/// `wsl.exe` and `ssh.exe` are both known to exit without restoring it.
/// `PSReadLine` repairs the shell's own prompt every time it draws one, so the
/// session looks healthy while cooked reads stay broken, and the first program
/// to do a plain line read gets keystrokes without echo and an Enter that
/// yields a bare carriage return, never the newline the read is waiting for.
/// This is that program: the one cooked read in the xtask. Redirected stdin is
/// left alone, since `GetConsoleMode` fails on anything that is not a console,
/// and a pipe's semantics (an answer, or nothing meaning no) need no repair.
#[cfg(windows)]
fn restore_console_line_input() {
    use windows_sys::Win32::System::Console::{
        ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT, GetConsoleMode, GetStdHandle,
        STD_INPUT_HANDLE, SetConsoleMode,
    };
    let mut mode = 0;
    // SAFETY: GetStdHandle returns a handle this process may query without
    // taking ownership, and `mode` outlives both calls. GetConsoleMode failing
    // (a pipe, the null device, no console at all) leaves `mode` untouched and
    // skips the set.
    #[allow(unsafe_code)]
    unsafe {
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        if GetConsoleMode(handle, &raw mut mode) != 0 {
            let wanted = mode | ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT;
            if wanted != mode {
                let _ = SetConsoleMode(handle, wanted);
            }
        }
    }
}

/// On Unix the terminal's cooked mode is the shell's to restore, and the
/// corruption this guards against is a Windows console phenomenon.
#[cfg(not(windows))]
fn restore_console_line_input() {}

/// Ask, and read the answer.
///
/// Nothing to read means no. That is the case in a script or a CI job, and
/// deleting a golden image because nobody was there to object is the wrong
/// default; `--force` is how such a caller says yes in advance.
pub fn confirm(prompt: &str) -> bool {
    restore_console_line_input();
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    match std::io::stdin().lock().read_line(&mut answer) {
        Ok(0) | Err(_) => {
            println!("\nnot confirmed: nothing to read on stdin. `--force` answers it in advance");
            false
        }
        Ok(_) => is_affirmative(&answer),
    }
}

/// What a teardown actually managed to do.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TeardownOutcome {
    /// VMs that were actually running and were stopped.
    pub stopped: usize,
    pub deleted: usize,
    /// Files deliberately not deleted, because the VM holding them would not
    /// stop.
    pub skipped: usize,
    pub bytes_freed: u64,
    pub problems: Vec<String>,
}

/// Carry out a plan.
///
/// `stop` is supplied by the caller so this half is testable and so the
/// provider layer stays where it belongs.
///
/// A VM is stopped before its files are deleted, because a running hypervisor
/// holds them open. If that stop fails, the image's files are left alone: the
/// alternative is unlinking the disk of a VM that is still running, which
/// destroys a guest mid-write and, in a purge, takes the golden image with it.
/// A teardown that reports a problem and leaves everything in place can be
/// retried; one that half-succeeded cannot.
pub fn execute(
    plan: &TeardownPlan,
    stop: &dyn Fn(&RunState) -> Result<crate::provider::Stopped, String>,
) -> TeardownOutcome {
    let mut outcome = TeardownOutcome {
        problems: plan.refused.clone(),
        ..TeardownOutcome::default()
    };
    // Image, and why it was held back: the two are reported together,
    // because "a file survived" without the reason is not actionable.
    let mut held_back: Vec<(Image, String)> = Vec::new();

    for vm in &plan.vms {
        match stop(vm) {
            Ok(crate::provider::Stopped::Stopped) => outcome.stopped += 1,
            Ok(crate::provider::Stopped::WasNotRunning) => {}
            Err(e) => {
                outcome
                    .problems
                    .push(format!("could not stop {}: {e}", vm.vm_name));
                if let Some(image) = vm.image() {
                    held_back.push((image, format!("{} would not stop", vm.vm_name)));
                }
            }
        }
    }

    for file in &plan.files {
        if let Some(image) = file.image
            && held_back.iter().any(|(held, _)| *held == image)
        {
            outcome.skipped += 1;
            continue;
        }
        match std::fs::remove_file(&file.path) {
            Ok(()) => {
                outcome.deleted += 1;
                outcome.bytes_freed += file.bytes;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                outcome
                    .problems
                    .push(format!("could not delete {}: {e}", file.path.display()));
                // Stop at the first failure for this image. The files are
                // ordered so that the manifest goes last, which only helps if
                // a failure before it stops it from going at all: an image
                // that outlives its manifest cannot be dated, and dating it is
                // how the evaluation clock is read.
                if let Some(image) = file.image {
                    held_back.push((
                        image,
                        format!("{} could not be deleted", file.path.display()),
                    ));
                }
            }
        }
    }

    // Reported per image and by the reason that image was held back. Under
    // `all` a single sentence naming neither is unusable: it says something
    // survived without saying what, or why, or which of two targets it was.
    for (image, reason) in &held_back {
        let count = plan
            .files
            .iter()
            .filter(|f| f.image == Some(*image))
            .count();
        if count == 0 {
            continue;
        }
        outcome.problems.push(format!(
            "{image}: {} left in place because {reason}",
            crate::util::count(count, "file")
        ));
    }

    for dir in &plan.dirs {
        remove_if_empty(dir);
    }

    outcome
}

/// Remove `dir` and any empty directory inside it, deepest first.
///
/// Only ever an empty directory, so anything unexpected inside survives and
/// shows up in the next `vm status`, which scans a run directory whole. The
/// nesting matters because the directories a teardown names have subdirectories
/// in them: without this, emptying one would leave the outer directory standing
/// for good, which reads as run state that could not be removed.
fn remove_if_empty(dir: &std::path::Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                remove_if_empty(&entry.path());
            }
        }
    }
    let _ = std::fs::remove_dir(dir);
}

/// The closing report.
pub fn render_outcome(outcome: &TeardownOutcome, scope: Scope) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "stopped {}, deleted {}, freed {}",
        crate::util::count(outcome.stopped, "VM"),
        crate::util::count(outcome.deleted, "file"),
        format_bytes(outcome.bytes_freed)
    );
    // What was asked for is not what happened: a purge that held everything
    // back still deleted nothing, and saying the images are gone when they are
    // sitting there is how someone ends up rebuilding an image they still have.
    if scope.image && outcome.skipped == 0 && outcome.deleted > 0 {
        let _ = writeln!(
            out,
            "the golden images are gone; `cargo xtask vm build-image <image>` rebuilds them"
        );
    } else if scope.image && outcome.skipped > 0 {
        let _ = writeln!(
            out,
            "some images were kept, listed below; nothing was half-deleted, so \
             running this again once the problem is fixed is safe"
        );
    } else if outcome.deleted > 0 || outcome.stopped > 0 {
        let _ = writeln!(
            out,
            "the golden images are untouched; the next run boots a pristine overlay"
        );
    }
    for problem in &outcome.problems {
        let _ = writeln!(out, "problem: {problem}");
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::provider::target::ProviderKind;
    use std::path::Path;

    use super::*;
    use crate::provider::target::Image;
    use crate::store::inventory::fixtures::{BUILT, empty, healthy, inventory};
    use crate::store::state::StartReason;

    fn store() -> Store {
        Store::new("/srv/vm")
    }

    fn with_run_state(image: Image) -> crate::store::inventory::ImageInventory {
        let mut entry = healthy(image);
        entry.run_files = vec![
            FileInfo::new(
                format!("/srv/vm/run/{image}/overlay.qcow2"),
                2 * 1024 * 1024 * 1024,
                BUILT,
            ),
            FileInfo::new(format!("/srv/vm/run/{image}/vm.json"), 512, BUILT),
        ];
        entry.state = Some(RunState::new(
            image,
            ProviderKind::Qemu,
            format!("/srv/vm/run/{image}/overlay.qcow2").into(),
            StartReason::Keep,
            BUILT,
        ));
        entry.manifest_bytes = Some(400);
        entry
    }

    #[test]
    fn a_plain_destroy_takes_run_state_and_leaves_the_image() {
        let inv = inventory(vec![with_run_state(Image::Linux)]);
        let plan = plan(
            &store(),
            &inv,
            Selection::One(Image::Linux),
            Scope::RUN_STATE,
        );
        assert_eq!(plan.vms.len(), 1);
        assert_eq!(plan.files.len(), 2);
        assert_eq!(plan.bytes(), 2 * 1024 * 1024 * 1024 + 512);
        assert!(
            !plan
                .files
                .iter()
                .any(|f| f.path.to_string_lossy().contains("images")),
            "{plan:?}"
        );
    }

    #[test]
    fn purging_a_base_takes_its_layer_and_says_why_the_layer_is_in_the_list() {
        let inv = inventory(vec![
            healthy(Image::Windows),
            healthy(Image::WindowsBuilder),
        ]);
        let both = plan(
            &store(),
            &inv,
            Selection::One(Image::Windows),
            Scope::EVERYTHING,
        );
        assert_eq!(both.layers, vec![Image::WindowsBuilder]);
        let rendered = both.render();
        assert!(rendered.contains("images/windows-builder"), "{rendered}");
        // Why a command asked for one image is deleting two.
        assert!(
            rendered.contains("the windows-builder layer goes with it"),
            "{rendered}"
        );
        assert!(rendered.contains("unreadable file"), "{rendered}");

        // The other direction: a layer named on its own is the only thing
        // taken, and there is nothing to explain.
        let layer = plan(
            &store(),
            &inv,
            Selection::One(Image::WindowsBuilder),
            Scope::EVERYTHING,
        );
        assert!(layer.layers.is_empty(), "{layer:?}");
        let rendered = layer.render();
        assert!(rendered.contains("images/windows-builder"), "{rendered}");
        assert!(!rendered.contains("images/windows/"), "{rendered}");
        assert!(!rendered.contains("goes with it"), "{rendered}");
    }

    #[test]
    fn a_purge_adds_the_image_the_manifest_and_the_media() {
        let mut inv = inventory(vec![with_run_state(Image::Windows)]);
        inv.iso = vec![FileInfo::new(
            "/srv/vm/iso/win.iso",
            7 * 1024 * 1024 * 1024,
            BUILT,
        )];
        let plan = plan(
            &store(),
            &inv,
            Selection::One(Image::Windows),
            Scope::EVERYTHING,
        );
        let names: Vec<String> = plan
            .files
            .iter()
            .map(|f| f.path.to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().any(|n| n.contains("golden.qcow2")),
            "{names:?}"
        );
        assert!(
            names.iter().any(|n| n.contains("manifest.json")),
            "{names:?}"
        );
        assert!(names.iter().any(|n| n.contains("win.iso")), "{names:?}");
        assert_eq!(
            plan.bytes(),
            20 * 1024 * 1024 * 1024 + 7 * 1024 * 1024 * 1024 + 2 * 1024 * 1024 * 1024 + 512 + 400
        );
    }

    #[test]
    fn purging_the_media_takes_both_windows_media_files() {
        // The download, its prompt-free repack, and the record tying the second
        // to the first. `--iso` is the command the status report offers for
        // reclaiming the media, and leaving any of it behind would make the
        // number it printed a lie; a record outliving its copy would claim a
        // provenance for a file that is gone.
        let mut inv = inventory(vec![healthy(Image::Windows)]);
        inv.iso = vec![
            FileInfo::new(
                "/srv/vm/iso/windows11-enterprise-eval.iso",
                7 * 1024 * 1024 * 1024,
                BUILT,
            ),
            FileInfo::new(
                "/srv/vm/iso/windows11-enterprise-eval-noprompt.iso",
                7 * 1024 * 1024 * 1024,
                BUILT,
            ),
            FileInfo::new(
                format!(
                    "/srv/vm/iso/{}",
                    crate::store::windows_media::SOURCE_MARK_FILE
                ),
                0,
                BUILT,
            ),
        ];
        let plan = plan(
            &store(),
            &inv,
            Selection::One(Image::Windows),
            Scope::from_flags(false, false, true, false),
        );
        assert_eq!(plan.files.len(), 3, "{plan:?}");
        assert_eq!(plan.bytes(), 14 * 1024 * 1024 * 1024);
        assert!(
            plan.files.iter().all(|f| f.image == Some(Image::Windows)),
            "{plan:?}"
        );
    }

    #[test]
    fn a_crashed_build_is_torn_down_with_its_unfinished_disk() {
        // The build disk lives in the run directory precisely so that this is
        // true without new plumbing: a half-built image is worth nothing, and
        // `vm down` is what gets rid of it.
        let mut entry = healthy(Image::Windows);
        entry.run_files = vec![
            FileInfo::new(
                "/srv/vm/run/windows/build.vhdx",
                11 * 1024 * 1024 * 1024,
                BUILT,
            ),
            FileInfo::new("/srv/vm/run/windows/vm.json", 512, BUILT),
        ];
        let mut state = RunState::new(
            Image::Windows,
            ProviderKind::HyperV,
            "/srv/vm/run/windows/build.vhdx".into(),
            StartReason::Build,
            BUILT,
        );
        state.ssh_user = "tester".to_owned();
        entry.state = Some(state);

        let plan = plan(
            &store(),
            &inventory(vec![entry]),
            Selection::One(Image::Windows),
            Scope::RUN_STATE,
        );
        assert_eq!(plan.vms.len(), 1);
        assert!(
            plan.files
                .iter()
                .any(|f| f.path.to_string_lossy().contains("build.vhdx")),
            "{plan:?}"
        );
        // And the golden image the image may already have is untouched.
        assert!(
            !plan
                .files
                .iter()
                .any(|f| f.path.to_string_lossy().contains("golden.")),
            "{plan:?}"
        );
    }

    #[test]
    fn purging_only_linux_leaves_the_windows_media_alone() {
        let mut inv = inventory(vec![healthy(Image::Windows), healthy(Image::Linux)]);
        inv.iso = vec![FileInfo::new("/srv/vm/iso/win.iso", 1024, BUILT)];
        let plan = plan(
            &store(),
            &inv,
            Selection::One(Image::Linux),
            Scope::EVERYTHING,
        );
        let names: Vec<String> = plan
            .files
            .iter()
            .map(|f| f.path.to_string_lossy().into_owned())
            .collect();
        assert!(!names.iter().any(|n| n.contains("win.iso")), "{names:?}");
        assert!(!names.iter().any(|n| n.contains("windows")), "{names:?}");
    }

    #[test]
    fn destroy_all_covers_both_targets() {
        let inv = inventory(vec![
            with_run_state(Image::Windows),
            with_run_state(Image::Linux),
        ]);
        let plan = plan(&store(), &inv, Selection::All, Scope::RUN_STATE);
        assert_eq!(plan.vms.len(), 2);
        assert_eq!(plan.files.len(), 4);
    }

    #[test]
    fn a_state_file_naming_a_foreign_vm_is_refused_rather_than_acted_on() {
        let mut entry = with_run_state(Image::Linux);
        entry.state.as_mut().unwrap().vm_name = "production-db".to_owned();
        let plan = plan(
            &store(),
            &inventory(vec![entry]),
            Selection::All,
            Scope::RUN_STATE,
        );
        assert!(plan.vms.is_empty());
        assert!(plan.refused[0].contains("production-db"), "{plan:?}");
    }

    #[test]
    fn a_path_outside_the_store_is_refused_rather_than_deleted() {
        let mut entry = with_run_state(Image::Linux);
        entry.run_files[0].path = PathBuf::from("/etc/passwd");
        let plan = plan(
            &store(),
            &inventory(vec![entry]),
            Selection::All,
            Scope::EVERYTHING,
        );
        assert!(
            !plan
                .files
                .iter()
                .any(|f| f.path == Path::new("/etc/passwd")),
            "{plan:?}"
        );
        assert!(
            plan.refused.iter().any(|r| r.contains("/etc/passwd")),
            "{plan:?}"
        );
    }

    #[test]
    fn a_purge_with_no_flags_means_all_of_it() {
        assert_eq!(
            Scope::from_flags(false, false, false, false),
            Scope::EVERYTHING
        );
        assert_eq!(
            Scope::from_flags(false, false, true, false),
            Scope {
                vm: false,
                image: false,
                iso: true,
                cache: false
            }
        );
        // Additive rather than exclusive: two flags select two things.
        assert_eq!(
            Scope::from_flags(false, true, true, false),
            Scope {
                vm: false,
                image: true,
                iso: true,
                cache: false
            }
        );
        // The build cache is the one part `vm down` never takes and a purge
        // reaches only when nothing narrows it or when `--cache` names it.
        const { assert!(!Scope::RUN_STATE.cache) };
        const { assert!(Scope::EVERYTHING.cache) };
        assert_eq!(
            Scope::from_flags(false, false, false, true),
            Scope {
                vm: false,
                image: false,
                iso: false,
                cache: true
            }
        );
        assert!(
            Scope::from_flags(false, false, false, true)
                .label()
                .contains("build cache")
        );
        // `down` is the cheap one, and purge's flags cannot produce it by
        // accident: it never touches an image.
        assert_ne!(
            Scope::RUN_STATE,
            Scope::from_flags(false, false, false, false)
        );
        assert_eq!(
            Scope::RUN_STATE,
            Scope::from_flags(true, false, false, false)
        );
    }

    #[test]
    fn only_a_scope_that_touches_the_guests_files_stops_the_guest() {
        let run = Some(StartReason::Run);
        assert!(Scope::RUN_STATE.needs_the_vm_stopped(run));
        assert!(Scope::EVERYTHING.needs_the_vm_stopped(run));
        // A Hyper-V child holds the golden image open, so an image purge stops
        // the VM as well.
        assert!(
            Scope::from_flags(false, true, false, false).needs_the_vm_stopped(run),
            "deleting an image under a running differencing child"
        );
        // Media is different: a booted test guest finished with the ISO long
        // ago, whether there is a guest recorded or not.
        let media = Scope::from_flags(false, false, true, false);
        assert!(!media.needs_the_vm_stopped(run));
        assert!(!media.needs_the_vm_stopped(None));
        // Except under a build, which has both DVDs attached for the whole
        // install: deleting the media under one is deleting a file Windows has
        // open.
        assert!(media.needs_the_vm_stopped(Some(StartReason::Build)));
    }

    #[test]
    fn purging_the_media_during_a_build_ends_the_build_first() {
        // The install DVD is attached to the build VM for the length of the
        // install, so the media cannot go while it runs.
        let mut entry = with_run_state(Image::Windows);
        if let Some(state) = entry.state.as_mut() {
            state.reason = StartReason::Build;
        }
        let inv = inventory(vec![entry]);
        let plan = plan(
            &store(),
            &inv,
            Selection::One(Image::Windows),
            Scope::from_flags(false, false, true, false),
        );
        assert_eq!(plan.vms.len(), 1, "{plan:?}");
        assert_eq!(plan.vms[0].reason, StartReason::Build);
    }

    #[test]
    fn a_purge_that_ends_a_build_says_so_whether_or_not_it_asks() {
        // `--force` skips the question, not the consequences, so the listing
        // carries what ending the guest costs. The question carries it too,
        // because a question about bytes does not describe an install.
        let mut entry = with_run_state(Image::Windows);
        if let Some(state) = entry.state.as_mut() {
            state.reason = StartReason::Build;
        }
        let plan = plan(
            &store(),
            &inventory(vec![entry]),
            Selection::One(Image::Windows),
            Scope::from_flags(false, false, true, false),
        );

        let text = plan.render();
        assert!(
            text.contains("stop sunlit-e2e-windows (qemu), which ends the image build"),
            "{text}"
        );
        assert!(text.contains("over from the media"), "{text}");
        // And what it stops without clearing up, with the command that does.
        assert!(
            text.contains("kept: sunlit-e2e-windows's record and the disk its install had written"),
            "{text}"
        );
        assert!(text.contains("does not take run state"), "{text}");
        assert!(text.contains("cargo xtask vm down windows"), "{text}");

        let prompt = confirmation_prompt(&plan);
        assert!(prompt.contains("ends the image build"), "{prompt}");
        assert!(prompt.ends_with("[y/N] "), "{prompt}");
    }

    #[test]
    fn a_teardown_that_takes_the_run_state_with_it_has_nothing_to_report_keeping() {
        // `vm down` on a build, and a purge with `--vm`, delete the record and
        // the unfinished disk themselves, so there is nothing left to name.
        let mut entry = with_run_state(Image::Windows);
        if let Some(state) = entry.state.as_mut() {
            state.reason = StartReason::Build;
        }
        let inv = inventory(vec![entry]);
        for scope in [Scope::RUN_STATE, Scope::EVERYTHING] {
            let text = plan(&store(), &inv, Selection::One(Image::Windows), scope).render();
            assert!(text.contains("which ends the image build"), "{text}");
            assert!(!text.contains("kept:"), "{text}");
        }
    }

    #[test]
    fn ending_a_guest_that_holds_nothing_is_reported_as_the_plain_thing_it_is() {
        let inv = inventory(vec![with_run_state(Image::Linux)]);
        let plan = plan(
            &store(),
            &inv,
            Selection::One(Image::Linux),
            Scope::EVERYTHING,
        );
        // The stop line ends where the VM's provider does: nothing to add.
        let text = plan.render();
        assert!(text.contains("stop sunlit-e2e-linux (qemu)\n"), "{text}");
        assert!(!text.contains("kept:"), "{text}");
        assert!(!confirmation_prompt(&plan).contains("That also stops"));
    }

    #[test]
    fn purging_only_the_media_leaves_the_guest_and_its_image_alone() {
        let inv = inventory(vec![with_run_state(Image::Windows)]);
        let plan = plan(
            &store(),
            &inv,
            Selection::One(Image::Windows),
            Scope::from_flags(false, false, true, false),
        );
        assert!(plan.vms.is_empty(), "{plan:?}");
        let paths: Vec<String> = plan
            .files
            .iter()
            .map(|f| f.path.display().to_string())
            .collect();
        assert!(paths.iter().all(|p| p.contains("iso")), "{paths:?}");
    }

    #[test]
    fn the_question_says_what_goes_and_what_it_frees() {
        let inv = inventory(vec![with_run_state(Image::Linux)]);
        let plan = plan(
            &store(),
            &inv,
            Selection::One(Image::Linux),
            Scope::EVERYTHING,
        );
        let prompt = confirmation_prompt(&plan);
        for part in [
            "the VM and its run state",
            "the golden image",
            "the installation media",
            "linux",
            "[y/N]",
        ] {
            assert!(prompt.contains(part), "{prompt} is missing {part}");
        }
        assert!(prompt.contains("GiB"), "{prompt}");
    }

    #[test]
    fn only_a_plain_yes_deletes_anything() {
        for yes in [
            "y", "Y", "yes", "YES", " yes 
",
        ] {
            assert!(is_affirmative(yes), "{yes:?}");
        }
        // An empty line is what someone pressing return gives, and the prompt
        // promises that means no.
        for no in [
            "",
            "
",
            "n",
            "no",
            "sure",
            "yes please",
            "yolo",
        ] {
            assert!(!is_affirmative(no), "{no:?}");
        }
    }

    #[test]
    fn an_empty_store_produces_an_empty_plan_that_says_so() {
        let inv = inventory(vec![empty(Image::Windows), empty(Image::Linux)]);
        let plan = plan(&store(), &inv, Selection::All, Scope::EVERYTHING);
        assert!(plan.is_empty());
        assert_eq!(plan.bytes(), 0);
        let text = plan.render();
        assert!(text.contains("nothing to remove for all ("), "{text}");
        // What was asked for is named, so "nothing to remove" cannot be read as
        // "there is nothing there" when only one part was in scope.
        assert!(text.contains("the golden image"), "{text}");
    }

    #[test]
    fn the_plan_reads_as_a_list_of_intentions() {
        let inv = inventory(vec![with_run_state(Image::Linux)]);
        let text = plan(
            &store(),
            &inv,
            Selection::One(Image::Linux),
            Scope::RUN_STATE,
        )
        .render();
        assert!(text.contains("stop sunlit-e2e-linux (qemu)"), "{text}");
        assert!(
            text.contains("delete /srv/vm/run/linux/overlay.qcow2 (2.0 GiB)"),
            "{text}"
        );
    }

    #[test]
    fn a_purge_deletes_the_manifest_after_everything_it_describes() {
        // An image that outlives its manifest cannot be dated, and dating it
        // is the only way to read the evaluation clock. A purge that got that
        // far and stopped would leave the store in exactly the state a boot
        // now has to refuse.
        let inv = inventory(vec![with_run_state(Image::Windows)]);
        let plan = plan(
            &store(),
            &inv,
            Selection::One(Image::Windows),
            Scope::EVERYTHING,
        );
        let manifest = plan
            .files
            .iter()
            .position(|f| f.path.to_string_lossy().contains("manifest.json"))
            .expect("the manifest is in the plan");
        let last_image = plan
            .files
            .iter()
            .rposition(|f| f.path.to_string_lossy().contains("golden."))
            .expect("an image is in the plan");
        assert!(
            manifest > last_image,
            "the manifest is deleted before the images it describes"
        );
    }

    #[test]
    fn a_vm_that_will_not_stop_keeps_its_files() {
        // The alternative is unlinking the disk of a running VM, and with
        // --purge the golden image with it. A destroy that changed nothing can
        // be retried; one that half-succeeded cannot.
        let inv = inventory(vec![with_run_state(Image::Linux)]);
        let plan = plan(
            &store(),
            &inv,
            Selection::One(Image::Linux),
            Scope::EVERYTHING,
        );
        assert!(!plan.files.is_empty());

        let outcome = execute(&plan, &|_| Err("the hypervisor said no".to_owned()));
        assert_eq!(outcome.deleted, 0);
        assert_eq!(outcome.skipped, plan.files.len());
        assert!(
            outcome.problems.iter().any(|p| p.contains("left in place")),
            "{outcome:?}"
        );
    }

    #[test]
    fn one_targets_failure_does_not_hold_back_another() {
        let inv = inventory(vec![
            with_run_state(Image::Windows),
            with_run_state(Image::Linux),
        ]);
        let plan = plan(&store(), &inv, Selection::All, Scope::RUN_STATE);
        let outcome = execute(&plan, &|state| {
            if state.image == "windows" {
                Err("stuck".to_owned())
            } else {
                Ok(crate::provider::Stopped::Stopped)
            }
        });
        assert_eq!(outcome.stopped, 1);
        // Two files per image; only the Windows ones are held back.
        assert_eq!(outcome.skipped, 2);
    }

    #[test]
    fn the_cached_iso_is_held_back_with_the_target_that_consumes_it() {
        // It is only ever deleted when Windows is in scope, so a Windows
        // teardown that failed has to keep it: deleting the 6.6 GB download a
        // rebuild needs, while reporting that nothing was deleted for that
        // image, is the opposite of the "safe to repeat" the guide promises.
        let mut inv = inventory(vec![with_run_state(Image::Windows)]);
        inv.iso = vec![FileInfo::new("/srv/vm/iso/win.iso", 1024, BUILT)];
        let plan = plan(
            &store(),
            &inv,
            Selection::One(Image::Windows),
            Scope::EVERYTHING,
        );

        let iso = plan
            .files
            .iter()
            .find(|f| f.path.to_string_lossy().contains("win.iso"))
            .expect("the iso is in the plan");
        assert_eq!(iso.image, Some(Image::Windows));

        let outcome = execute(&plan, &|_| Err("stuck".to_owned()));
        assert_eq!(outcome.deleted, 0);
        assert_eq!(outcome.skipped, plan.files.len());
    }

    #[test]
    fn a_vm_that_was_not_running_is_not_counted_as_stopped() {
        let inv = inventory(vec![with_run_state(Image::Linux)]);
        let plan = plan(
            &store(),
            &inv,
            Selection::One(Image::Linux),
            Scope::RUN_STATE,
        );
        let outcome = execute(&plan, &|_| Ok(crate::provider::Stopped::WasNotRunning));
        assert_eq!(outcome.stopped, 0);
        assert_eq!(outcome.skipped, 0);
        assert!(render_outcome(&outcome, Scope::RUN_STATE).contains("stopped 0 VMs"));
    }

    #[test]
    fn execution_reports_what_it_could_not_stop() {
        let inv = inventory(vec![with_run_state(Image::Linux)]);
        let plan = plan(
            &store(),
            &inv,
            Selection::One(Image::Linux),
            Scope::RUN_STATE,
        );
        let outcome = execute(&plan, &|_| Err("the hypervisor said no".to_owned()));
        assert_eq!(outcome.stopped, 0);
        assert_eq!(outcome.deleted, 0);
        assert_eq!(outcome.bytes_freed, 0);
        assert!(outcome.problems[0].contains("the hypervisor said no"));
    }

    #[test]
    fn the_outcome_says_whether_a_rebuild_is_now_needed() {
        let outcome = TeardownOutcome {
            stopped: 1,
            deleted: 3,
            skipped: 0,
            bytes_freed: 1024 * 1024,
            problems: Vec::new(),
        };
        let plain = render_outcome(&outcome, Scope::RUN_STATE);
        assert!(
            plain.contains("stopped 1 VM, deleted 3 files, freed 1.0 MiB"),
            "{plain}"
        );
        assert!(plain.contains("golden images are untouched"), "{plain}");
        let purged = render_outcome(&outcome, Scope::EVERYTHING);
        assert!(purged.contains("build-image"), "{purged}");
    }

    /// A run directory has directories in it, so the teardown has to reach
    /// through them; and the rule that it removes only empty ones is what keeps
    /// anything unexpected inside for the next `vm status` to report.
    #[test]
    fn a_teardown_reaches_through_a_directory_and_stops_at_anything_left_in_one() {
        let dir = std::env::temp_dir().join("sunlit_xtask_remove_if_empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("job")).expect("temp tree");
        std::fs::create_dir_all(dir.join("dist")).expect("temp tree");
        std::fs::write(dir.join("dist").join("src.tar"), b"x").expect("write");

        remove_if_empty(&dir);
        assert!(!dir.join("job").exists(), "an empty directory survived");
        assert!(dir.join("dist").join("src.tar").is_file());
        assert!(dir.is_dir(), "a directory with a file under it was removed");

        std::fs::remove_file(dir.join("dist").join("src.tar")).expect("remove");
        remove_if_empty(&dir);
        assert!(!dir.exists());
    }
}
