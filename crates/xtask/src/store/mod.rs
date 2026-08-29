//! Where the multi-gigabyte artifacts live, and where the repo's templates are.
//!
//! Plan decision 4: images live outside the repo in a platform data directory,
//! overridable with `SUNLIT_EARTH_VM_DIR`; the repo carries only the templates.
//! Every path the xtask reads or writes is derived here, which is also what
//! lets a teardown prove that a path it is about to delete belongs to it.
//!
//! Everything under `images/`, `run/`, `build/` and `results/` is keyed by
//! [`Image`] rather than by target, because those are properties of one disk
//! and there are two disks per operating system now. The two desktop images
//! keep their slugs, so every path they had is the path they have.

pub mod hash;
pub mod inventory;
pub mod manifest;
pub mod state;
pub mod windows_media;

use std::path::{Path, PathBuf};

use crate::provider::target::{Image, Target};
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

    /// One image's disks and its manifest.
    pub fn image_dir(&self, image: Image) -> PathBuf {
        self.root.join("images").join(image.slug())
    }

    /// The canonical qcow2 image, which is what Packer builds for every base.
    ///
    /// For a layer this is the differencing child, named `layer.qcow2` rather
    /// than `golden.qcow2`: the file does not stand on its own, and a name that
    /// says so is worth more than one that matches its parent's.
    pub fn qcow2(&self, image: Image) -> PathBuf {
        self.image_dir(image)
            .join(format!("{}.qcow2", image.disk_stem()))
    }

    /// The VHDX form, which is what the `Hyper-V` provider makes its
    /// differencing children from.
    pub fn vhdx(&self, image: Image) -> PathBuf {
        self.image_dir(image)
            .join(format!("{}.vhdx", image.disk_stem()))
    }

    pub fn manifest(&self, image: Image) -> PathBuf {
        self.image_dir(image).join("manifest.json")
    }

    /// Per-image run state: the throwaway overlay, the state file, and the
    /// hypervisor's log.
    pub fn run_dir(&self, image: Image) -> PathBuf {
        self.root.join("run").join(image.slug())
    }

    pub fn overlay(&self, image: Image) -> PathBuf {
        let name = match image.target() {
            Target::Windows => "overlay.vhdx",
            Target::Linux => "overlay.qcow2",
        };
        self.run_dir(image).join(name)
    }

    /// The qcow2 overlay used when an image runs under QEMU. For the Windows
    /// guest that is the Linux-host cell of the provider matrix, so both
    /// overlay shapes exist for it.
    pub fn qemu_overlay(&self, image: Image) -> PathBuf {
        self.run_dir(image).join("overlay.qcow2")
    }

    pub fn state_file(&self, image: Image) -> PathBuf {
        self.run_dir(image).join("vm.json")
    }

    /// Where a job's script is written before it is copied into the guest.
    ///
    /// Run state like the overlay and the record: it belongs to the guest that
    /// is running the job, it means nothing once that guest is gone, and the
    /// teardown takes all three together. One place says where it is, because
    /// every command that runs a job in a guest names it and the teardown has to
    /// name the same directory.
    pub fn job_scratch(&self, image: Image) -> PathBuf {
        self.run_dir(image).join("job")
    }

    /// Where the Windows hand-over launcher is written before it is copied into
    /// the guest, for the same reason and with the same lifetime as the job
    /// scratch: it is generated per boot from what that boot staged, and it
    /// means nothing once the guest that took a copy is gone.
    pub fn handover_scratch(&self, image: Image) -> PathBuf {
        self.run_dir(image).join("handover")
    }

    /// The throwaway copy of the firmware's variables store a QEMU boot makes,
    /// so a guest writes its boot entries into its own rather than into the
    /// shared one the host installed.
    pub fn firmware_vars(&self, image: Image) -> PathBuf {
        self.run_dir(image).join("efi-vars.fd")
    }

    /// The disk a native install writes into, before it becomes the golden
    /// image.
    ///
    /// In the run directory rather than the build directory, because it is run
    /// state: it belongs to a VM that exists right now, and `vm down` is what
    /// gets rid of both together. A half-built image is worth nothing, so
    /// nothing about it is worth keeping past the guest that was writing it
    /// (amendment decision 17).
    pub fn build_disk(&self, image: Image) -> PathBuf {
        self.run_dir(image).join("build.vhdx")
    }

    pub fn vm_log(&self, image: Image) -> PathBuf {
        self.run_dir(image).join("vm.log")
    }

    /// Downloaded installation media, cached so a rebuild does not re-download.
    pub fn iso_dir(&self) -> PathBuf {
        self.root.join("iso")
    }

    pub fn windows_iso(&self) -> PathBuf {
        self.iso_dir().join("windows11-enterprise-eval.iso")
    }

    /// The same media repacked without the "Press any key to boot from CD or
    /// DVD" prompt, which the native Hyper-V install boots (amendment decision
    /// 16). Cached beside the original because making it costs a full copy of
    /// the media out of a mounted image and back.
    pub fn windows_iso_noprompt(&self) -> PathBuf {
        self.iso_dir()
            .join("windows11-enterprise-eval-noprompt.iso")
    }

    /// What that copy was repacked from: the original's size and checksum, so a
    /// cached repack can be told from one made out of a different download.
    /// Beside the two ISOs, so `vm purge windows --iso` takes it with them.
    pub fn windows_iso_noprompt_source(&self) -> PathBuf {
        self.iso_dir().join(windows_media::SOURCE_MARK_FILE)
    }

    /// Packer's working directory for one image, which also holds its log.
    pub fn build_dir(&self, image: Image) -> PathBuf {
        self.root.join("build").join(image.slug())
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
    pub fn results_dir(&self, image: Image) -> PathBuf {
        self.root.join("results").join(image.slug())
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

/// The template directory for one image, `vm/<slug>/` in the repo.
pub fn template_dir(image: Image) -> PathBuf {
    repo_root().join("vm").join(image.slug())
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
        let mut paths = vec![
            store.windows_iso(),
            store.windows_iso_noprompt(),
            store.windows_iso_noprompt_source(),
            store.ssh_key(),
        ];
        for image in Image::ALL {
            paths.extend([
                store.qcow2(image),
                store.vhdx(image),
                store.manifest(image),
                store.overlay(image),
                store.qemu_overlay(image),
                store.state_file(image),
                store.job_scratch(image),
                store.handover_scratch(image),
                store.firmware_vars(image),
                store.build_disk(image),
                store.build_dir(image),
                store.results_dir(image),
                store.vm_log(image),
            ]);
        }
        for path in paths {
            assert!(
                store.contains(&path),
                "{} escaped the store",
                path.display()
            );
        }
    }

    #[test]
    fn no_two_images_share_a_path() {
        let store = Store::new("/srv/vm");
        for (index, image) in Image::ALL.into_iter().enumerate() {
            for other in &Image::ALL[index + 1..] {
                assert_ne!(store.qcow2(image), store.qcow2(*other));
                assert_ne!(store.image_dir(image), store.image_dir(*other));
                assert_ne!(store.run_dir(image), store.run_dir(*other));
                assert_ne!(store.overlay(image), store.overlay(*other));
                assert_ne!(store.manifest(image), store.manifest(*other));
                assert_ne!(template_dir(image), template_dir(*other));
            }
        }
    }

    /// The images already on disk were built under the two original slugs, and
    /// the store has to keep reading them from exactly where they are. A layer
    /// sits beside them under its own slug and names its disk differently,
    /// because that file cannot be read without its parent.
    #[test]
    fn the_desktop_images_keep_the_paths_they_were_built_at() {
        let store = Store::new("/srv/vm");
        assert!(
            store
                .qcow2(Image::Linux)
                .ends_with("images/linux/golden.qcow2")
        );
        assert!(
            store
                .vhdx(Image::Windows)
                .ends_with("images/windows/golden.vhdx")
        );
        assert!(
            store
                .vhdx(Image::WindowsBuilder)
                .ends_with("images/windows-builder/layer.vhdx")
        );
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
    fn the_build_disk_and_the_two_media_files_are_all_distinct() {
        let store = Store::new("/srv/vm");
        // The build disk shares the run directory with the overlay a guest
        // boots from, and a build and a guest must never write to one file.
        assert_ne!(
            store.build_disk(Image::Windows),
            store.overlay(Image::Windows)
        );
        assert_ne!(store.windows_iso(), store.windows_iso_noprompt());
        // Both media files sit in the one directory `vm purge --iso` clears.
        assert_eq!(
            store.windows_iso_noprompt().parent(),
            store.windows_iso().parent()
        );
    }

    #[test]
    fn windows_overlays_differ_by_provider_and_linux_has_only_qcow2() {
        let store = Store::new("/srv/vm");
        for image in [Image::Windows, Image::WindowsBuilder] {
            assert!(
                store.overlay(image).to_string_lossy().ends_with(".vhdx"),
                "{image}"
            );
        }
        for image in [Image::Linux, Image::LinuxBuilder] {
            assert_eq!(store.overlay(image), store.qemu_overlay(image), "{image}");
        }
    }
}
