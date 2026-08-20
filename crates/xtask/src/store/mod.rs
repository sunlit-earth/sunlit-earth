//! Where the multi-gigabyte artifacts live, and where the repo's templates are.
//!
//! Plan decision 4: images live outside the repo in a platform data directory,
//! overridable with `SUNLIT_EARTH_VM_DIR`; the repo carries only the templates.
//! Every path the xtask reads or writes is derived here, which is also what
//! lets a teardown prove that a path it is about to delete belongs to it.

pub mod hash;
pub mod inventory;
pub mod manifest;
pub mod state;
pub mod windows_media;

use std::path::{Path, PathBuf};

use crate::provider::target::Target;
use crate::util;

/// Environment override for the image store.
pub const VM_DIR_ENV: &str = "SUNLIT_EARTH_VM_DIR";

/// Environment override for the repository root, for the rare case of running
/// the xtask binary from outside its checkout.
pub const REPO_ENV: &str = "SUNLIT_EARTH_REPO";

/// The resolved location of everything the xtask owns on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The golden image and its manifest for one target.
    pub fn image_dir(&self, target: Target) -> PathBuf {
        self.root.join("images").join(target.slug())
    }

    /// The canonical qcow2 golden image, which is what Packer builds for both
    /// targets.
    pub fn qcow2(&self, target: Target) -> PathBuf {
        self.image_dir(target).join("golden.qcow2")
    }

    /// The VHDX conversion of the Windows golden image, which is what the
    /// `Hyper-V` provider makes its differencing children from.
    pub fn vhdx(&self, target: Target) -> PathBuf {
        self.image_dir(target).join("golden.vhdx")
    }

    pub fn manifest(&self, target: Target) -> PathBuf {
        self.image_dir(target).join("manifest.json")
    }

    /// Per-target run state: the throwaway overlay, the state file, and the
    /// hypervisor's log.
    pub fn run_dir(&self, target: Target) -> PathBuf {
        self.root.join("run").join(target.slug())
    }

    pub fn overlay(&self, target: Target) -> PathBuf {
        let name = match target {
            Target::Windows => "overlay.vhdx",
            Target::Linux => "overlay.qcow2",
        };
        self.run_dir(target).join(name)
    }

    /// The qcow2 overlay used when a target runs under QEMU. For the Windows
    /// guest that is the Linux-host cell of the provider matrix, so both
    /// overlay shapes exist for it.
    pub fn qemu_overlay(&self, target: Target) -> PathBuf {
        self.run_dir(target).join("overlay.qcow2")
    }

    pub fn state_file(&self, target: Target) -> PathBuf {
        self.run_dir(target).join("vm.json")
    }

    pub fn vm_log(&self, target: Target) -> PathBuf {
        self.run_dir(target).join("vm.log")
    }

    /// Downloaded installation media, cached so a rebuild does not re-download.
    pub fn iso_dir(&self) -> PathBuf {
        self.root.join("iso")
    }

    pub fn windows_iso(&self) -> PathBuf {
        self.iso_dir().join("windows11-enterprise-eval.iso")
    }

    /// Packer's working directory for one target, which also holds its log.
    pub fn build_dir(&self, target: Target) -> PathBuf {
        self.root.join("build").join(target.slug())
    }

    /// The key pair the guests trust. Generated once by `vm setup` and baked
    /// into both golden images at build time.
    pub fn ssh_key(&self) -> PathBuf {
        self.root.join("ssh").join("id_ed25519")
    }

    pub fn ssh_pubkey(&self) -> PathBuf {
        self.root.join("ssh").join("id_ed25519.pub")
    }

    /// Where results pulled back out of a guest land.
    pub fn results_dir(&self, target: Target) -> PathBuf {
        self.root.join("results").join(target.slug())
    }

    /// Whether `path` is inside the store.
    ///
    /// `vm down` and `vm purge` ask this about every path before deleting it, so a
    /// malformed state file cannot point the cleanup at something else. The
    /// comparison is lexical over normalized components, because the paths in
    /// question may not exist any more by the time it is asked.
    pub fn contains(&self, path: &Path) -> bool {
        is_inside(&self.root, path)
    }
}

/// Lexical containment check: does `path` sit under `root` once `.` and `..`
/// are resolved without touching the filesystem?
pub fn is_inside(root: &Path, path: &Path) -> bool {
    let root = normalize(root);
    let path = normalize(path);
    path.starts_with(&root) && path != root
}

/// Resolve `.` and `..` lexically, and lowercase drive letters so that
/// `C:\store` and `c:\store` compare equal on Windows.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::Prefix(prefix) => {
                out.push(prefix.as_os_str().to_string_lossy().to_lowercase());
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Pick the image store: the override if set, else the platform's local data
/// directory beside the application's own state.
pub fn resolve_store(
    override_dir: Option<String>,
    data_local: Option<PathBuf>,
) -> Result<Store, String> {
    if let Some(dir) = util::non_blank(override_dir) {
        return Ok(Store::new(dir));
    }
    data_local
        .map(|base| Store::new(base.join("SunlitEarth").join("vm")))
        .ok_or_else(|| {
            format!(
                "no local data directory on this host; set {VM_DIR_ENV} to the \
                 directory the VM images should live in"
            )
        })
}

/// The store this host is using.
pub fn store() -> Result<Store, String> {
    resolve_store(std::env::var(VM_DIR_ENV).ok(), dirs::data_local_dir())
}

/// The repository root, from the compile-time location of this crate.
///
/// The crate lives at `crates/xtask`, so the root is two levels up.
pub fn repo_root() -> PathBuf {
    if let Some(dir) = util::env_var(REPO_ENV) {
        return PathBuf::from(dir);
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .nth(2)
        .map_or_else(|| manifest.to_path_buf(), Path::to_path_buf)
}

/// The template directory for one target, `vm/<target>/` in the repo.
pub fn template_dir(target: Target) -> PathBuf {
    repo_root().join("vm").join(target.slug())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_override_wins_over_the_data_directory() {
        let store = resolve_store(Some("/tmp/vm".to_owned()), Some(PathBuf::from("/home/x")))
            .expect("override resolves");
        assert_eq!(store.root(), Path::new("/tmp/vm"));
    }

    #[test]
    fn a_blank_override_falls_back_to_the_data_directory() {
        let store = resolve_store(Some("  ".to_owned()), Some(PathBuf::from("/home/x/.local")))
            .expect("data dir resolves");
        assert_eq!(
            store.root(),
            Path::new("/home/x/.local").join("SunlitEarth").join("vm")
        );
    }

    #[test]
    fn with_neither_the_error_names_the_variable_to_set() {
        let err = resolve_store(None, None).unwrap_err();
        assert!(err.contains(VM_DIR_ENV), "{err}");
    }

    #[test]
    fn every_artifact_path_sits_under_the_root() {
        let store = Store::new("/srv/vm");
        let paths = [
            store.qcow2(Target::Linux),
            store.vhdx(Target::Windows),
            store.manifest(Target::Windows),
            store.overlay(Target::Linux),
            store.state_file(Target::Windows),
            store.windows_iso(),
            store.build_dir(Target::Linux),
            store.ssh_key(),
            store.results_dir(Target::Linux),
            store.vm_log(Target::Windows),
        ];
        for path in paths {
            assert!(
                store.contains(&path),
                "{} escaped the store",
                path.display()
            );
        }
    }

    #[test]
    fn the_two_targets_never_share_a_path() {
        let store = Store::new("/srv/vm");
        assert_ne!(store.qcow2(Target::Windows), store.qcow2(Target::Linux));
        assert_ne!(store.run_dir(Target::Windows), store.run_dir(Target::Linux));
        assert_ne!(store.overlay(Target::Windows), store.overlay(Target::Linux));
    }

    #[test]
    fn containment_rejects_traversal_and_siblings() {
        let store = Store::new("/srv/vm");
        assert!(!store.contains(Path::new("/srv/vm")));
        assert!(!store.contains(Path::new("/srv/vm/../secrets/key")));
        assert!(!store.contains(Path::new("/srv/vmm/image.qcow2")));
        assert!(!store.contains(Path::new("/etc/passwd")));
        assert!(store.contains(Path::new("/srv/vm/images/linux/golden.qcow2")));
        assert!(store.contains(Path::new("/srv/vm/run/../images/linux/golden.qcow2")));
    }

    // Windows path semantics: off Windows, `Path` treats a drive-qualified
    // path as a single component, and this code only ever runs on a
    // Windows host anyway.
    #[cfg(windows)]
    #[test]
    fn containment_ignores_drive_letter_case() {
        let store = Store::new(r"C:\Users\dev\AppData\Local\SunlitEarth\vm");
        assert!(store.contains(Path::new(
            r"c:\Users\dev\AppData\Local\SunlitEarth\vm\images\linux\golden.qcow2"
        )));
    }

    #[test]
    fn windows_overlays_differ_by_provider_and_linux_has_only_qcow2() {
        let store = Store::new("/srv/vm");
        assert!(
            store
                .overlay(Target::Windows)
                .to_string_lossy()
                .ends_with(".vhdx")
        );
        assert_eq!(
            store.overlay(Target::Linux),
            store.qemu_overlay(Target::Linux)
        );
    }
}
