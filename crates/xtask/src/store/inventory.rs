//! What the image store currently holds.
//!
//! One model, two consumers: `vm doctor` turns it into pass/warn/fail lines and
//! `vm status` turns it into an inventory with disk footprints. Scanning is a
//! thin effectful function; everything that decides what the contents mean is
//! pure and tested against fabricated inventories.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::provider::target::Target;
use crate::store::Store;
use crate::store::manifest::{Currency, EvalState, Manifest};
use crate::store::state::RunState;

/// One file found in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    pub path: PathBuf,
    pub bytes: u64,
    pub mtime_unix: u64,
}

impl FileInfo {
    pub fn new(path: impl Into<PathBuf>, bytes: u64, mtime_unix: u64) -> Self {
        Self {
            path: path.into(),
            bytes,
            mtime_unix,
        }
    }

    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// Everything the store holds for one target.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TargetInventory {
    pub target: Option<Target>,
    /// The golden images: `golden.qcow2`, plus `golden.vhdx` for Windows.
    pub images: Vec<FileInfo>,
    pub manifest: Option<Manifest>,
    /// Why the manifest could not be read, when there is a file but it is
    /// unusable. A manifest that is simply absent leaves both fields empty.
    pub manifest_error: Option<String>,
    /// Hash of `vm/<target>/` as the repo has it now, or `None` if the
    /// templates could not be read.
    pub template_hash: Option<String>,
    /// Size of the manifest file itself, when one is present. Small, but it is
    /// one of the things `--purge` removes and the report adds up.
    pub manifest_bytes: Option<u64>,
    /// Whatever the image builder left in its working directory.
    pub build_files: Vec<FileInfo>,
    /// Overlays, state files, and logs from runs.
    pub run_files: Vec<FileInfo>,
    pub state: Option<RunState>,
    pub state_error: Option<String>,
    /// Filled in by the provider layer: whether the recorded VM is alive.
    /// `None` means the question was not asked.
    pub running: Option<bool>,
}

impl TargetInventory {
    pub fn target(&self) -> Target {
        self.target.unwrap_or(Target::Linux)
    }

    /// Bytes in the golden images themselves, which only `--purge` reclaims.
    pub fn image_bytes(&self) -> u64 {
        self.images.iter().map(|f| f.bytes).sum::<u64>() + self.manifest_bytes.unwrap_or(0)
    }

    /// Bytes left in the image builder's working directory, also `--purge`.
    pub fn build_bytes(&self) -> u64 {
        self.build_files.iter().map(|f| f.bytes).sum()
    }

    /// Bytes that a plain `vm destroy` would reclaim for this target.
    pub fn run_bytes(&self) -> u64 {
        self.run_files.iter().map(|f| f.bytes).sum()
    }

    pub fn total_bytes(&self) -> u64 {
        self.image_bytes() + self.build_bytes() + self.run_bytes()
    }

    /// The single most important thing to say about this target's image.
    ///
    /// The order is deliberate: a missing image makes every other question
    /// moot, corruption outranks age, and an expired evaluation outranks a
    /// stale template because a rebuild fixes both while a stale image still
    /// runs.
    pub fn condition(&self, now_unix: u64) -> ImageCondition {
        if self.images.is_empty() {
            return ImageCondition::Missing;
        }
        let Some(manifest) = &self.manifest else {
            return ImageCondition::Unmanifested {
                detail: self
                    .manifest_error
                    .clone()
                    .unwrap_or_else(|| "no manifest next to the image".to_owned()),
                // Without a manifest there is no build timestamp, and without
                // that the evaluation clock cannot be read at all. For the
                // Windows image that is exactly the situation decision 5
                // exists to prevent, so it blocks a boot; the Linux image has
                // no clock, so the only thing lost is knowing it is current.
                expiry_unknown: self.target().has_eval_expiry(),
            };
        };
        if let Some(detail) = self.size_mismatch(manifest) {
            return ImageCondition::Corrupt { detail };
        }
        if let Some(state) = manifest.eval_state(self.target(), now_unix)
            && state.is_expired()
        {
            return ImageCondition::Expired { state };
        }
        if let Currency::Stale {
            built_from,
            repo_has,
        } = manifest.currency(self.template_hash.as_deref())
        {
            return ImageCondition::Stale {
                built_from,
                repo_has,
            };
        }
        match manifest.eval_state(self.target(), now_unix) {
            Some(state @ EvalState::Expiring { .. }) => ImageCondition::Expiring { state },
            _ => ImageCondition::Ok,
        }
    }

    /// Compare the manifest's record of each image against the file on disk.
    ///
    /// Size is checked on every run because it is free; the recorded checksum
    /// is only verified on demand, since re-reading tens of gigabytes to answer
    /// a question a size comparison already answers would make the doctor
    /// unusable. Truncation, the failure that actually happens when a build or
    /// a copy is interrupted, changes the size.
    fn size_mismatch(&self, manifest: &Manifest) -> Option<String> {
        for record in &manifest.images {
            let found = self.images.iter().find(|f| f.name() == record.file);
            match found {
                None => {
                    return Some(format!(
                        "{} is in the manifest but not in the image directory",
                        record.file
                    ));
                }
                Some(file) if file.bytes != record.bytes => {
                    return Some(format!(
                        "{} is {} bytes, the manifest records {}",
                        record.file, file.bytes, record.bytes
                    ));
                }
                Some(_) => {}
            }
        }
        None
    }
}

/// The one-line verdict on a target's golden image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageCondition {
    /// Nothing built yet.
    Missing,
    /// Images without a usable manifest: age and currency cannot be judged.
    Unmanifested {
        detail: String,
        /// Whether this target has an evaluation clock that can no longer be
        /// read, which is what makes an unmanifested image unusable rather
        /// than merely unlabelled.
        expiry_unknown: bool,
    },
    /// The files on disk disagree with what the build recorded.
    Corrupt {
        detail: String,
    },
    /// Past the evaluation window.
    Expired {
        state: EvalState,
    },
    /// Built from templates the repo has since changed.
    Stale {
        built_from: String,
        repo_has: String,
    },
    /// Inside the evaluation window, but not by much.
    Expiring {
        state: EvalState,
    },
    Ok,
}

impl ImageCondition {
    /// A short label for report lines.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Unmanifested { .. } => "no manifest",
            Self::Corrupt { .. } => "corrupt",
            Self::Expired { .. } => "expired",
            Self::Stale { .. } => "stale",
            Self::Expiring { .. } => "expiring",
            Self::Ok => "ok",
        }
    }

    /// The detail line that goes with the label.
    pub fn detail(&self) -> String {
        match self {
            Self::Missing => "no golden image built yet".to_owned(),
            Self::Corrupt { detail } => detail.clone(),
            Self::Unmanifested {
                detail,
                expiry_unknown,
            } => {
                if *expiry_unknown {
                    format!("{detail}, so the evaluation age cannot be read")
                } else {
                    format!("{detail}, so it cannot be told whether it is current")
                }
            }
            Self::Expired { state } | Self::Expiring { state } => state.summary(),
            Self::Stale {
                built_from,
                repo_has,
            } => format!("built from template {built_from}, the repo now has {repo_has}"),
            Self::Ok => "current".to_owned(),
        }
    }

    /// Whether a VM may be started from an image in this condition without an
    /// explicit override.
    pub fn blocks_boot(&self) -> bool {
        matches!(
            self,
            Self::Missing
                | Self::Corrupt { .. }
                | Self::Expired { .. }
                | Self::Unmanifested {
                    expiry_unknown: true,
                    ..
                }
        )
    }
}

/// The whole store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inventory {
    pub root: PathBuf,
    pub root_exists: bool,
    pub targets: Vec<TargetInventory>,
    /// Cached installation media.
    pub iso: Vec<FileInfo>,
    /// The generated key pair, if `vm setup` has run.
    pub ssh_key: Option<FileInfo>,
}

impl Inventory {
    pub fn for_target(&self, target: Target) -> Option<&TargetInventory> {
        self.targets.iter().find(|t| t.target == Some(target))
    }

    pub fn iso_bytes(&self) -> u64 {
        self.iso.iter().map(|f| f.bytes).sum()
    }

    pub fn total_bytes(&self) -> u64 {
        self.targets
            .iter()
            .map(TargetInventory::total_bytes)
            .sum::<u64>()
            + self.iso_bytes()
    }

    /// Every VM that has a state file, whether or not it is still alive.
    #[cfg(test)]
    pub fn recorded_vms(&self) -> Vec<&RunState> {
        self.targets
            .iter()
            .filter_map(|t| t.state.as_ref())
            .collect()
    }
}

/// Read the store. Missing directories are not errors: an empty store is the
/// normal state before the first build.
pub fn scan(store: &Store) -> Inventory {
    let mut inventory = Inventory {
        root: store.root().to_path_buf(),
        root_exists: store.root().is_dir(),
        ..Inventory::default()
    };

    for target in Target::ALL {
        let mut entry = TargetInventory {
            target: Some(target),
            images: list_files(&store.image_dir(target))
                .into_iter()
                .filter(|f| f.name().starts_with("golden."))
                .collect(),
            run_files: list_files(&store.run_dir(target)),
            build_files: list_tree(&store.build_dir(target)),
            manifest_bytes: file_info(&store.manifest(target)).map(|f| f.bytes),
            ..TargetInventory::default()
        };

        match read_optional(&store.manifest(target)) {
            Ok(Some(text)) => match Manifest::from_json(&text) {
                Ok(manifest) => entry.manifest = Some(manifest),
                Err(e) => entry.manifest_error = Some(e),
            },
            Ok(None) => {}
            Err(e) => entry.manifest_error = Some(format!("cannot read the manifest: {e}")),
        }

        match read_optional(&store.state_file(target)) {
            Ok(Some(text)) => match RunState::from_json(&text) {
                Ok(state) if state.is_ours() => entry.state = Some(state),
                Ok(state) => {
                    entry.state_error = Some(format!(
                        "the state file names '{}', which is not one of ours; ignoring it",
                        state.vm_name
                    ));
                }
                Err(e) => entry.state_error = Some(e),
            },
            Ok(None) => {}
            Err(e) => entry.state_error = Some(format!("cannot read the state file: {e}")),
        }

        entry.template_hash = crate::store::hash::read_tree(&crate::store::template_dir(target))
            .ok()
            .map(|files| crate::store::hash::template_hash(&files));

        inventory.targets.push(entry);
    }

    inventory.iso = list_files(&store.iso_dir());
    inventory.ssh_key = file_info(&store.ssh_key());
    inventory
}

/// Every file directly inside `dir`, ignoring the directory not existing.
fn list_files(dir: &Path) -> Vec<FileInfo> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<FileInfo> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| file_info(&e.path()))
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Every file under `dir`, recursively. The image builder's working directory
/// is the only nested one, and it is nested arbitrarily deep.
fn list_tree(dir: &Path) -> Vec<FileInfo> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => out.extend(list_tree(&entry.path())),
            Ok(kind) if kind.is_file() => out.extend(file_info(&entry.path())),
            _ => {}
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn file_info(path: &Path) -> Option<FileInfo> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    Some(FileInfo::new(path, meta.len(), mtime))
}

/// Read a file, distinguishing "not there" from "there but unreadable".
fn read_optional(path: &Path) -> std::io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
pub mod fixtures {
    //! Fabricated inventories, which is how everything downstream is tested.

    use super::{FileInfo, Inventory, TargetInventory};
    use crate::provider::target::Target;
    use crate::store::manifest::{ImageRecord, Manifest};
    use crate::util::SECS_PER_DAY;

    /// The build time every fixture uses, so ages are easy to reason about.
    pub const BUILT: u64 = 1_000 * SECS_PER_DAY;
    pub const TEMPLATE: &str = "crc32:aaaa1111";

    pub fn manifest(target: Target, image: &str, bytes: u64) -> Manifest {
        Manifest::new(
            target,
            TEMPLATE.to_owned(),
            BUILT,
            "source.iso".to_owned(),
            "packer 1.16.0".to_owned(),
            vec![ImageRecord {
                file: image.to_owned(),
                bytes,
                checksum: "crc32:12345678".to_owned(),
            }],
        )
    }

    /// A target with a healthy, current image.
    pub fn healthy(target: Target) -> TargetInventory {
        // Both targets keep the canonical qcow2 under the same name; the
        // Windows VHDX conversion sits beside it and is not what the condition
        // logic keys on.
        let image = "golden.qcow2";
        TargetInventory {
            target: Some(target),
            images: vec![FileInfo::new(
                format!("/srv/vm/images/{target}/{image}"),
                20 * 1024 * 1024 * 1024,
                BUILT,
            )],
            manifest: Some(manifest(target, image, 20 * 1024 * 1024 * 1024)),
            template_hash: Some(TEMPLATE.to_owned()),
            ..TargetInventory::default()
        }
    }

    /// A target with nothing built.
    pub fn empty(target: Target) -> TargetInventory {
        TargetInventory {
            target: Some(target),
            ..TargetInventory::default()
        }
    }

    pub fn inventory(targets: Vec<TargetInventory>) -> Inventory {
        Inventory {
            root: "/srv/vm".into(),
            root_exists: true,
            targets,
            ..Inventory::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{BUILT, TEMPLATE, healthy};
    use super::*;
    use crate::provider::target::ProviderKind;
    use crate::store::state::StartReason;
    use crate::util::SECS_PER_DAY;

    #[test]
    fn a_healthy_current_image_is_ok() {
        let entry = healthy(Target::Linux);
        assert_eq!(entry.condition(BUILT + SECS_PER_DAY), ImageCondition::Ok);
    }

    #[test]
    fn no_image_is_missing_whatever_else_is_wrong() {
        let mut entry = healthy(Target::Windows);
        entry.images.clear();
        entry.manifest_error = Some("broken".to_owned());
        assert_eq!(
            entry.condition(BUILT + 200 * SECS_PER_DAY),
            ImageCondition::Missing
        );
    }

    #[test]
    fn an_image_without_a_manifest_says_so_and_does_not_pretend_to_be_ok() {
        let mut entry = healthy(Target::Linux);
        entry.manifest = None;
        let condition = entry.condition(BUILT);
        assert_eq!(condition.label(), "no manifest");
        assert!(condition.detail().contains("no manifest"), "{condition:?}");
    }

    #[test]
    fn an_unmanifested_windows_image_blocks_a_boot_and_a_linux_one_does_not() {
        // The manifest is the only record of when the evaluation licence
        // started. Losing it for the Windows image means losing the answer to
        // the question decision 5 exists to ask.
        let mut windows = healthy(Target::Windows);
        windows.manifest = None;
        let condition = windows.condition(BUILT);
        assert!(condition.blocks_boot(), "{condition:?}");
        assert!(
            condition.detail().contains("evaluation age"),
            "{condition:?}"
        );

        let mut linux = healthy(Target::Linux);
        linux.manifest = None;
        let condition = linux.condition(BUILT);
        assert!(!condition.blocks_boot(), "{condition:?}");
        assert!(condition.detail().contains("current"), "{condition:?}");
    }

    #[test]
    fn a_manifest_that_failed_to_parse_carries_its_reason_forward() {
        let mut entry = healthy(Target::Linux);
        entry.manifest = None;
        entry.manifest_error = Some("malformed manifest: expected value".to_owned());
        assert!(entry.condition(BUILT).detail().contains("expected value"));
    }

    #[test]
    fn a_truncated_image_is_corrupt() {
        let mut entry = healthy(Target::Linux);
        entry.images[0].bytes = 17;
        let condition = entry.condition(BUILT);
        assert_eq!(condition.label(), "corrupt");
        assert!(condition.detail().contains("17 bytes"), "{condition:?}");
        assert!(condition.blocks_boot());
    }

    #[test]
    fn an_image_the_manifest_lists_but_the_directory_lacks_is_corrupt() {
        let mut entry = healthy(Target::Windows);
        entry.images[0] = FileInfo::new("/srv/vm/images/windows/golden.vhdx", 1, BUILT);
        assert_eq!(entry.condition(BUILT).label(), "corrupt");
    }

    #[test]
    fn corruption_outranks_expiry_and_staleness() {
        let mut entry = healthy(Target::Windows);
        entry.images[0].bytes = 17;
        entry.template_hash = Some("crc32:ffffffff".to_owned());
        assert_eq!(
            entry.condition(BUILT + 200 * SECS_PER_DAY).label(),
            "corrupt"
        );
    }

    #[test]
    fn expiry_outranks_staleness_because_a_rebuild_fixes_both() {
        let mut entry = healthy(Target::Windows);
        entry.template_hash = Some("crc32:ffffffff".to_owned());
        assert_eq!(
            entry.condition(BUILT + 100 * SECS_PER_DAY).label(),
            "expired"
        );
    }

    #[test]
    fn staleness_outranks_the_expiry_warning_window() {
        let mut entry = healthy(Target::Windows);
        entry.template_hash = Some("crc32:ffffffff".to_owned());
        let condition = entry.condition(BUILT + 80 * SECS_PER_DAY);
        assert_eq!(condition.label(), "stale");
        assert!(condition.detail().contains(TEMPLATE), "{condition:?}");
        assert!(!condition.blocks_boot());
    }

    #[test]
    fn the_expiry_warning_window_shows_when_nothing_else_is_wrong() {
        let entry = healthy(Target::Windows);
        assert_eq!(
            entry.condition(BUILT + 80 * SECS_PER_DAY).label(),
            "expiring"
        );
    }

    #[test]
    fn the_linux_image_never_expires_however_old_it_is() {
        let entry = healthy(Target::Linux);
        assert_eq!(
            entry.condition(BUILT + 900 * SECS_PER_DAY),
            ImageCondition::Ok
        );
    }

    #[test]
    fn unreadable_repo_templates_do_not_declare_an_image_stale() {
        let mut entry = healthy(Target::Linux);
        entry.template_hash = None;
        assert_eq!(entry.condition(BUILT), ImageCondition::Ok);
    }

    #[test]
    fn only_missing_corrupt_and_expired_block_a_boot() {
        assert!(ImageCondition::Missing.blocks_boot());
        assert!(
            ImageCondition::Corrupt {
                detail: String::new()
            }
            .blocks_boot()
        );
        assert!(
            ImageCondition::Expired {
                state: crate::store::manifest::eval_state(0, 200 * SECS_PER_DAY)
            }
            .blocks_boot()
        );
        assert!(!ImageCondition::Ok.blocks_boot());
        assert!(
            !ImageCondition::Stale {
                built_from: String::new(),
                repo_has: String::new()
            }
            .blocks_boot()
        );
    }

    #[test]
    fn footprints_separate_what_destroy_frees_from_what_purge_frees() {
        let mut entry = healthy(Target::Linux);
        entry.run_files = vec![
            FileInfo::new("/srv/vm/run/linux/overlay.qcow2", 3 * 1024 * 1024, BUILT),
            FileInfo::new("/srv/vm/run/linux/vm.json", 512, BUILT),
        ];
        assert_eq!(entry.image_bytes(), 20 * 1024 * 1024 * 1024);
        assert_eq!(entry.run_bytes(), 3 * 1024 * 1024 + 512);
        assert_eq!(entry.total_bytes(), entry.image_bytes() + entry.run_bytes());
    }

    #[test]
    fn an_inventory_totals_images_run_state_and_isos() {
        let mut inventory = super::fixtures::inventory(vec![
            healthy(Target::Windows),
            super::fixtures::empty(Target::Linux),
        ]);
        inventory.iso = vec![FileInfo::new("/srv/vm/iso/win.iso", 1024, BUILT)];
        assert_eq!(inventory.total_bytes(), 20 * 1024 * 1024 * 1024 + 1024);
        assert!(inventory.for_target(Target::Windows).is_some());
        assert!(inventory.recorded_vms().is_empty());
    }

    #[test]
    fn recorded_vms_come_from_the_state_files() {
        let mut entry = healthy(Target::Linux);
        entry.state = Some(RunState::new(
            Target::Linux,
            ProviderKind::Qemu,
            "/srv/vm/run/linux/overlay.qcow2".into(),
            StartReason::Keep,
            BUILT,
        ));
        let inventory = super::fixtures::inventory(vec![entry]);
        assert_eq!(inventory.recorded_vms().len(), 1);
        assert_eq!(inventory.recorded_vms()[0].vm_name, "sunlit-e2e-linux");
    }
}
