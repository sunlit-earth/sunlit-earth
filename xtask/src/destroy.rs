//! `cargo xtask vm destroy [--purge]`: the cleanup pair to `vm status`.
//!
//! Plan decision 13. A plain destroy tears down run state only, which is cheap
//! and never costs an image rebuild. `--purge` additionally deletes the golden
//! images, the converted VHDX, the cached installation media, and the manifest,
//! which is the disk-space recovery path.
//!
//! Selecting what to delete is a pure function, because the cost of getting it
//! wrong is somebody else's virtual machine. Nothing outside the image store is
//! ever deleted, and nothing without the `sunlit-e2e-` prefix is ever stopped.

use std::fmt::Write as _;
use std::path::PathBuf;

use crate::inventory::{FileInfo, Inventory};
use crate::state::RunState;
use crate::store::Store;
use crate::target::Target;
use crate::util::format_bytes;

/// Which targets a destroy applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    One(Target),
    All,
}

impl Selection {
    pub fn includes(self, target: Target) -> bool {
        match self {
            Self::One(one) => one == target,
            Self::All => true,
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::One(target) => target.to_string(),
            Self::All => "all".to_owned(),
        }
    }
}

/// One file the plan intends to delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteItem {
    pub path: PathBuf,
    pub bytes: u64,
    /// The target whose VM holds this file, when one does. A file belonging to
    /// a VM that could not be stopped is not deleted, so this is what connects
    /// the two halves of a destroy.
    pub target: Option<Target>,
}

/// What a destroy would do, decided before anything is touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestroyPlan {
    pub selection: Selection,
    pub purge: bool,
    /// VMs to stop and unregister first.
    pub vms: Vec<RunState>,
    pub files: Vec<DeleteItem>,
    /// Directories to remove once they are empty. Best effort: a directory that
    /// still holds something unexpected is left alone.
    pub dirs: Vec<PathBuf>,
    /// Things deliberately not touched, and why.
    pub refused: Vec<String>,
}

impl DestroyPlan {
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.vms.is_empty() && self.files.is_empty()
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        if self.is_empty() {
            let _ = writeln!(
                out,
                "nothing to destroy for {} ({})",
                self.selection.label(),
                if self.purge {
                    "no images, media, or run state"
                } else {
                    "no run state"
                }
            );
        }
        for vm in &self.vms {
            let _ = writeln!(out, "stop {} ({})", vm.vm_name, vm.provider);
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
        for refusal in &self.refused {
            let _ = writeln!(out, "skipped: {refusal}");
        }
        out
    }
}

/// Decide what a destroy would touch.
pub fn plan(
    store: &Store,
    inventory: &Inventory,
    selection: Selection,
    purge: bool,
) -> DestroyPlan {
    let mut vms = Vec::new();
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let mut refused = Vec::new();

    let take = |file: &FileInfo,
                owner: Option<Target>,
                files: &mut Vec<DeleteItem>,
                refused: &mut Vec<String>| {
        if store.contains(&file.path) {
            files.push(DeleteItem {
                path: file.path.clone(),
                bytes: file.bytes,
                target: owner,
            });
        } else {
            refused.push(format!(
                "{} is outside the image store and was not deleted",
                file.path.display()
            ));
        }
    };

    for target in Target::ALL {
        if !selection.includes(target) {
            continue;
        }
        let Some(entry) = inventory.for_target(target) else {
            continue;
        };

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
            refused.push(format!("{target}: {error}"));
        }

        for file in &entry.run_files {
            take(file, Some(target), &mut files, &mut refused);
        }
        dirs.push(store.run_dir(target));

        if purge {
            // Images and build leftovers are not held open by a running VM,
            // but they are still that target's, so a VM that would not stop
            // keeps them too: a half-purged target is a state nothing can
            // recover from, and `vm status` would report the image as missing
            // while the VM still ran from it.
            for file in entry.images.iter().chain(entry.build_files.iter()) {
                take(file, Some(target), &mut files, &mut refused);
            }
            let manifest = store.manifest(target);
            if let Some(bytes) = entry.manifest_bytes {
                files.push(DeleteItem {
                    path: manifest,
                    bytes,
                    target: Some(target),
                });
            }
            dirs.push(store.image_dir(target));
            dirs.push(store.build_dir(target));
        }
    }

    if purge {
        // Installation media is shared, so it only goes when the target that
        // consumes it does. The Windows evaluation ISO is the only cached
        // download; the Ubuntu cloud image is fetched into the build directory.
        let media_in_scope = selection.includes(Target::Windows);
        if media_in_scope {
            for file in &inventory.iso {
                take(file, None, &mut files, &mut refused);
            }
            dirs.push(store.iso_dir());
        }
    }

    DestroyPlan {
        selection,
        purge,
        vms,
        files,
        dirs,
        refused,
    }
}

/// What a destroy actually managed to do.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DestroyOutcome {
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
/// holds them open. If that stop fails, the target's files are left alone: the
/// alternative is unlinking the disk of a VM that is still running, which
/// destroys a guest mid-write and, with `--purge`, takes the golden image with
/// it. A destroy that reports a problem and leaves everything in place can be
/// retried; one that half-succeeded cannot.
pub fn execute(
    plan: &DestroyPlan,
    stop: &dyn Fn(&RunState) -> Result<crate::provider::Stopped, String>,
) -> DestroyOutcome {
    let mut outcome = DestroyOutcome {
        problems: plan.refused.clone(),
        ..DestroyOutcome::default()
    };
    let mut held_back: Vec<Target> = Vec::new();

    for vm in &plan.vms {
        match stop(vm) {
            Ok(crate::provider::Stopped::Stopped) => outcome.stopped += 1,
            Ok(crate::provider::Stopped::WasNotRunning) => {}
            Err(e) => {
                outcome
                    .problems
                    .push(format!("could not stop {}: {e}", vm.vm_name));
                if let Some(target) = Target::ALL.into_iter().find(|t| t.slug() == vm.target) {
                    held_back.push(target);
                }
            }
        }
    }

    for file in &plan.files {
        if let Some(target) = file.target
            && held_back.contains(&target)
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
            Err(e) => outcome
                .problems
                .push(format!("could not delete {}: {e}", file.path.display())),
        }
    }

    if outcome.skipped > 0 {
        outcome.problems.push(format!(
            "{} left in place because the VM holding them would not stop; \
             nothing was deleted for that target",
            crate::util::count(outcome.skipped, "file")
        ));
    }

    for dir in &plan.dirs {
        // Only ever removes an empty directory, so anything unexpected inside
        // survives and shows up in the next `vm status`.
        let _ = std::fs::remove_dir(dir);
    }

    outcome
}

/// The closing report.
pub fn render_outcome(outcome: &DestroyOutcome, purge: bool) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "stopped {}, deleted {}, freed {}",
        crate::util::count(outcome.stopped, "VM"),
        crate::util::count(outcome.deleted, "file"),
        format_bytes(outcome.bytes_freed)
    );
    if purge {
        let _ = writeln!(
            out,
            "the golden images are gone; `cargo xtask vm build-image <target>` rebuilds them"
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
    use std::path::Path;

    use super::*;
    use crate::inventory::fixtures::{BUILT, empty, healthy, inventory};
    use crate::state::StartReason;
    use crate::target::ProviderKind;

    fn store() -> Store {
        Store::new("/srv/vm")
    }

    fn with_run_state(target: Target) -> crate::inventory::TargetInventory {
        let mut entry = healthy(target);
        entry.run_files = vec![
            FileInfo::new(
                format!("/srv/vm/run/{target}/overlay.qcow2"),
                2 * 1024 * 1024 * 1024,
                BUILT,
            ),
            FileInfo::new(format!("/srv/vm/run/{target}/vm.json"), 512, BUILT),
        ];
        entry.state = Some(RunState::new(
            target,
            ProviderKind::Qemu,
            format!("/srv/vm/run/{target}/overlay.qcow2").into(),
            StartReason::Keep,
            BUILT,
        ));
        entry.manifest_bytes = Some(400);
        entry
    }

    #[test]
    fn a_plain_destroy_takes_run_state_and_leaves_the_image() {
        let inv = inventory(vec![with_run_state(Target::Linux)]);
        let plan = plan(&store(), &inv, Selection::One(Target::Linux), false);
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
    fn a_purge_adds_the_image_the_manifest_and_the_media() {
        let mut inv = inventory(vec![with_run_state(Target::Windows)]);
        inv.iso = vec![FileInfo::new(
            "/srv/vm/iso/win.iso",
            7 * 1024 * 1024 * 1024,
            BUILT,
        )];
        let plan = plan(&store(), &inv, Selection::One(Target::Windows), true);
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
    fn purging_only_linux_leaves_the_windows_media_alone() {
        let mut inv = inventory(vec![healthy(Target::Windows), healthy(Target::Linux)]);
        inv.iso = vec![FileInfo::new("/srv/vm/iso/win.iso", 1024, BUILT)];
        let plan = plan(&store(), &inv, Selection::One(Target::Linux), true);
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
            with_run_state(Target::Windows),
            with_run_state(Target::Linux),
        ]);
        let plan = plan(&store(), &inv, Selection::All, false);
        assert_eq!(plan.vms.len(), 2);
        assert_eq!(plan.files.len(), 4);
    }

    #[test]
    fn a_state_file_naming_a_foreign_vm_is_refused_rather_than_acted_on() {
        let mut entry = with_run_state(Target::Linux);
        entry.state.as_mut().unwrap().vm_name = "production-db".to_owned();
        let plan = plan(&store(), &inventory(vec![entry]), Selection::All, false);
        assert!(plan.vms.is_empty());
        assert!(plan.refused[0].contains("production-db"), "{plan:?}");
    }

    #[test]
    fn a_path_outside_the_store_is_refused_rather_than_deleted() {
        let mut entry = with_run_state(Target::Linux);
        entry.run_files[0].path = PathBuf::from("/etc/passwd");
        let plan = plan(&store(), &inventory(vec![entry]), Selection::All, true);
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
    fn an_empty_store_produces_an_empty_plan_that_says_so() {
        let inv = inventory(vec![empty(Target::Windows), empty(Target::Linux)]);
        let plan = plan(&store(), &inv, Selection::All, true);
        assert!(plan.is_empty());
        assert_eq!(plan.bytes(), 0);
        assert!(plan.render().contains("nothing to destroy for all"));
    }

    #[test]
    fn the_plan_reads_as_a_list_of_intentions() {
        let inv = inventory(vec![with_run_state(Target::Linux)]);
        let text = plan(&store(), &inv, Selection::One(Target::Linux), false).render();
        assert!(text.contains("stop sunlit-e2e-linux (qemu)"), "{text}");
        assert!(
            text.contains("delete /srv/vm/run/linux/overlay.qcow2 (2.0 GiB)"),
            "{text}"
        );
    }

    #[test]
    fn a_vm_that_will_not_stop_keeps_its_files() {
        // The alternative is unlinking the disk of a running VM, and with
        // --purge the golden image with it. A destroy that changed nothing can
        // be retried; one that half-succeeded cannot.
        let inv = inventory(vec![with_run_state(Target::Linux)]);
        let plan = plan(&store(), &inv, Selection::One(Target::Linux), true);
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
            with_run_state(Target::Windows),
            with_run_state(Target::Linux),
        ]);
        let plan = plan(&store(), &inv, Selection::All, false);
        let outcome = execute(&plan, &|state| {
            if state.target == "windows" {
                Err("stuck".to_owned())
            } else {
                Ok(crate::provider::Stopped::Stopped)
            }
        });
        assert_eq!(outcome.stopped, 1);
        // Two files per target; only the Windows ones are held back.
        assert_eq!(outcome.skipped, 2);
    }

    #[test]
    fn shared_media_is_not_held_back_by_a_targets_failure() {
        // The ISO belongs to no VM, so nothing is holding it open.
        let mut inv = inventory(vec![with_run_state(Target::Windows)]);
        inv.iso = vec![FileInfo::new("/srv/vm/iso/win.iso", 1024, BUILT)];
        let plan = plan(&store(), &inv, Selection::One(Target::Windows), true);
        let outcome = execute(&plan, &|_| Err("stuck".to_owned()));
        let iso = plan
            .files
            .iter()
            .find(|f| f.path.to_string_lossy().contains("win.iso"))
            .expect("the iso is in the plan");
        assert_eq!(iso.target, None);
        assert!(outcome.skipped < plan.files.len());
    }

    #[test]
    fn a_vm_that_was_not_running_is_not_counted_as_stopped() {
        let inv = inventory(vec![with_run_state(Target::Linux)]);
        let plan = plan(&store(), &inv, Selection::One(Target::Linux), false);
        let outcome = execute(&plan, &|_| Ok(crate::provider::Stopped::WasNotRunning));
        assert_eq!(outcome.stopped, 0);
        assert_eq!(outcome.skipped, 0);
        assert!(render_outcome(&outcome, false).contains("stopped 0 VMs"));
    }

    #[test]
    fn execution_reports_what_it_could_not_stop() {
        let inv = inventory(vec![with_run_state(Target::Linux)]);
        let plan = plan(&store(), &inv, Selection::One(Target::Linux), false);
        let outcome = execute(&plan, &|_| Err("the hypervisor said no".to_owned()));
        assert_eq!(outcome.stopped, 0);
        assert_eq!(outcome.deleted, 0);
        assert_eq!(outcome.bytes_freed, 0);
        assert!(outcome.problems[0].contains("the hypervisor said no"));
    }

    #[test]
    fn the_outcome_says_whether_a_rebuild_is_now_needed() {
        let outcome = DestroyOutcome {
            stopped: 1,
            deleted: 3,
            skipped: 0,
            bytes_freed: 1024 * 1024,
            problems: Vec::new(),
        };
        let plain = render_outcome(&outcome, false);
        assert!(
            plain.contains("stopped 1 VM, deleted 3 files, freed 1.0 MiB"),
            "{plain}"
        );
        assert!(plain.contains("golden images are untouched"), "{plain}");
        let purged = render_outcome(&outcome, true);
        assert!(purged.contains("build-image"), "{purged}");
    }
}
