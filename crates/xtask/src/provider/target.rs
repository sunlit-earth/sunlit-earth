//! Guest targets, the images that hold them, host operating systems, and the
//! provider matrix that pairs them.

use std::fmt;

use clap::ValueEnum;

/// A guest the desktop e2e suite can run in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, ValueEnum)]
pub enum Target {
    /// The Windows 11 Enterprise evaluation guest: the full suite, including
    /// everything that needs a real Windows desktop session.
    Windows,
    /// The Debian 13 guest, which carries four desktops and logs into whichever
    /// one the boot asked for: the windowed-mode subset.
    Linux,
}

impl Target {
    /// Both targets, in the order reports list them.
    pub const ALL: [Self; 2] = [Self::Windows, Self::Linux];

    /// The lowercase name used in paths, VM names, and manifests.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
        }
    }

    /// Read one back out of a record, which stores [`Self::slug`].
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL.into_iter().find(|t| t.slug() == value)
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

/// The prefix every VM, overlay directory, and guest artifact the xtask creates
/// carries. Nothing without it is ever stopped, deleted, or reported.
pub const VM_NAME_PREFIX: &str = "sunlit-e2e-";

/// An image in the store.
///
/// The store grew from an image per target to an image per slug when release
/// builds arrived: an image is either a *base*, installed from media, or a
/// *layer*, provisioned over a named parent. The two desktop images the e2e
/// suite runs in keep their slugs and therefore their paths, so the images
/// already on disk stay valid.
///
/// | slug | kind | what it is for |
/// |---|---|---|
/// | `windows` | base | the e2e desktop guest |
/// | `linux` | base | the e2e desktop guest |
/// | `windows-builder` | layer over `windows` | release builds |
/// | `linux-builder` | base | release builds |
///
/// The distinction that matters everywhere else is [`Self::target`], the
/// operating system: that is what the provider matrix, the guest root, the job
/// scripts and the SSH account all key on, and a builder is the same operating
/// system as the desktop guest it builds for. What keys on the image itself is
/// what belongs to one disk: its paths in the store, its VM name, its manifest,
/// and how much of the host a guest of it gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, ValueEnum)]
pub enum Image {
    /// Windows 11 Enterprise evaluation with a desktop session: the e2e guest.
    Windows,
    /// Debian 13 with four desktops: the e2e guest.
    Linux,
    /// The Windows desktop image plus MSVC, libclang and rustup, as a
    /// differencing child of it.
    WindowsBuilder,
    /// Ubuntu 22.04 with a build toolchain and no graphics stack. Its own base
    /// rather than a layer over Debian 13, because the glibc a binary is linked
    /// against is the floor it carries: Debian 13's 2.41 would refuse to run on
    /// Ubuntu 24.04, and 22.04's 2.35 runs on everything since.
    LinuxBuilder,
}

impl Image {
    /// All four, in the order reports list them: each base followed by its
    /// builder.
    pub const ALL: [Self; 4] = [
        Self::Windows,
        Self::WindowsBuilder,
        Self::Linux,
        Self::LinuxBuilder,
    ];

    /// The lowercase name used in paths, VM names, manifests and run records.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::WindowsBuilder => "windows-builder",
            Self::LinuxBuilder => "linux-builder",
        }
    }

    /// The operating system inside it.
    pub fn target(self) -> Target {
        match self {
            Self::Windows | Self::WindowsBuilder => Target::Windows,
            Self::Linux | Self::LinuxBuilder => Target::Linux,
        }
    }

    /// The image this one is a differencing child of, if any.
    ///
    /// A layer holds only what its own provisioning wrote, and it is unreadable
    /// without its parent: `Hyper-V` refuses to attach a child whose parent's
    /// identifier changed, and qcow2 reads garbage silently. So the parent is
    /// part of a layer's identity, and the inventory checks it.
    pub fn parent(self) -> Option<Self> {
        match self {
            Self::WindowsBuilder => Some(Self::Windows),
            Self::Windows | Self::Linux | Self::LinuxBuilder => None,
        }
    }

    pub fn is_layer(self) -> bool {
        self.parent().is_some()
    }

    /// The images that are differencing children of this one, which is what a
    /// rebuild or a purge of a base has to say something about.
    pub fn children(self) -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|image| image.parent() == Some(self))
            .collect()
    }

    /// The stem of the image's disk files, which says at a glance whether a
    /// file in the store stands on its own.
    pub fn disk_stem(self) -> &'static str {
        if self.is_layer() { "layer" } else { "golden" }
    }

    /// The VM name this image's guest runs under.
    ///
    /// The prefix is what makes every VM the xtask creates recognizable as its
    /// own, so `vm status` and `vm down` never touch anything else on the host.
    /// It still says `e2e` for a builder guest, because it is the ownership
    /// marker every teardown checks and nothing about it means anything to a
    /// hypervisor.
    pub fn vm_name(self) -> String {
        format!("{}{}", VM_NAME_PREFIX, self.slug())
    }

    /// Whether this image sits on an evaluation licence that expires.
    ///
    /// True for the Windows base and for the layer over it, which shares the
    /// installation the clock started on. Where the clock is measured from
    /// differs: a base counts from its own build, a layer from its parent's.
    pub fn has_eval_expiry(self) -> bool {
        matches!(self, Self::Windows | Self::WindowsBuilder)
    }

    /// Whether a desktop session logs on in this image.
    ///
    /// The Linux builder is the only image with none: it is built from a cloud
    /// image with no desktop in it, and writes its readiness marker from a
    /// oneshot unit at boot because it has no session to write one from. The
    /// Windows builder is a differencing child of the Windows desktop image and
    /// inherits the autologon it was built with, so a boot of it waits for a
    /// session and gets one, and `vm view` opens a desktop there. What tells a
    /// builder from an image the suite runs in is [`Self::is_builder`], and
    /// asking this instead is what once called the same guest a console in one
    /// line and a desktop four lines later.
    pub fn has_desktop(self) -> bool {
        matches!(self, Self::Windows | Self::Linux | Self::WindowsBuilder)
    }

    /// Whether this is an image a release binary is built in rather than one the
    /// e2e suite runs in.
    ///
    /// Both builders carry a toolchain and neither carries the suite, which is
    /// the question every text about what an image is *for* is asking: which
    /// command uses it, which command leaves one of its guests behind, and what
    /// a smoke test should ask it to prove.
    pub fn is_builder(self) -> bool {
        matches!(self, Self::WindowsBuilder | Self::LinuxBuilder)
    }

    /// What `vm view` opens for this image, as the word that labels the line
    /// offering it.
    ///
    /// The Linux builder is the one image with no session, so what a viewer
    /// attaches to there is a text console and calling it a desktop describes an
    /// image nobody built. Both spellings are seven letters, which is what keeps
    /// that line's command in the same column as the `ssh:` and `down:` lines
    /// around it.
    pub fn console_label(self) -> &'static str {
        if self.has_desktop() {
            "desktop"
        } else {
            "console"
        }
    }

    /// The desktop image the e2e suite and a release verification run in.
    pub fn desktop(target: Target) -> Self {
        match target {
            Target::Windows => Self::Windows,
            Target::Linux => Self::Linux,
        }
    }

    /// The image a release binary for `target` is built in.
    pub fn builder(target: Target) -> Self {
        match target {
            Target::Windows => Self::WindowsBuilder,
            Target::Linux => Self::LinuxBuilder,
        }
    }

    /// What a report calls this image.
    pub fn label(self) -> &'static str {
        match self {
            Self::Windows => "Windows 11 desktop guest",
            Self::Linux => "Debian 13 desktop guest",
            Self::WindowsBuilder => "Windows release builder",
            Self::LinuxBuilder => "Ubuntu 22.04 release builder",
        }
    }

    /// Read one back out of a manifest or a run record, which store
    /// [`Self::slug`].
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL.into_iter().find(|image| image.slug() == value)
    }
}

impl fmt::Display for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

/// The host this xtask is running on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOs {
    Windows,
    Linux,
    /// Anything else, macOS in practice. Supported for `e2e --target host`
    /// only: macOS coverage lives on hosted runners, per retrospective 8.3.
    Other,
}

impl HostOs {
    /// The host this binary was compiled for.
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::Other => "unsupported",
        }
    }
}

/// Which hypervisor drives a given guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// `PowerShell` cmdlets, a differencing VHDX child, the Default Switch.
    HyperV,
    /// A QEMU process plus QMP over a localhost socket, qcow2 overlay.
    Qemu,
}

impl ProviderKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::HyperV => "hyperv",
            Self::Qemu => "qemu",
        }
    }

    /// Parse the `SUNLIT_EARTH_VM_PROVIDER` override.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "hyperv" | "hyper-v" => Some(Self::HyperV),
            "qemu" => Some(Self::Qemu),
            _ => None,
        }
    }
}

/// The provider matrix from retrospective section 8.3: `Hyper-V` for the
/// Windows guest on a Windows host, QEMU everywhere else.
///
/// This answers which hypervisor would drive a guest, which is not the same as
/// whether this host can run the suite in one. A Linux host has a hypervisor
/// for the Windows guest and no way to build the binaries to put in it; that
/// question belongs to `artifacts::check_can_build`, which the commands that
/// need binaries ask first.
///
/// The two coexist on a Windows host by design, because QEMU's WHPX
/// acceleration runs on top of the `Hyper-V` hypervisor.
pub fn provider_for(host: HostOs, target: Target) -> Option<ProviderKind> {
    match (host, target) {
        (HostOs::Windows, Target::Windows) => Some(ProviderKind::HyperV),
        (HostOs::Windows, Target::Linux) | (HostOs::Linux, _) => Some(ProviderKind::Qemu),
        (HostOs::Other, _) => None,
    }
}

/// The provider to use, honoring an explicit override.
///
/// An override is how a Windows host can be pushed onto QEMU for the Windows
/// guest, which is the configuration a Linux host uses anyway and therefore the
/// one worth being able to reproduce.
pub fn resolve_provider(
    host: HostOs,
    target: Target,
    override_value: Option<&str>,
) -> Result<ProviderKind, String> {
    if let Some(raw) = override_value {
        return ProviderKind::parse(raw)
            .ok_or_else(|| format!("SUNLIT_EARTH_VM_PROVIDER={raw} is not one of: hyperv, qemu"));
    }
    provider_for(host, target).ok_or_else(|| {
        format!(
            "no VM provider for a {target} guest on a {} host; \
             macOS coverage runs on hosted CI runners, not in local VMs",
            host.name()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_names_carry_the_ownership_prefix() {
        assert_eq!(Image::Windows.vm_name(), "sunlit-e2e-windows");
        assert_eq!(Image::Linux.vm_name(), "sunlit-e2e-linux");
        assert_eq!(
            Image::WindowsBuilder.vm_name(),
            "sunlit-e2e-windows-builder"
        );
        for image in Image::ALL {
            assert!(image.vm_name().starts_with(VM_NAME_PREFIX));
        }
    }

    #[test]
    fn only_the_windows_installation_has_an_evaluation_clock() {
        // The layer shares its parent's installation, so it shares the clock.
        assert!(Image::Windows.has_eval_expiry());
        assert!(Image::WindowsBuilder.has_eval_expiry());
        assert!(!Image::Linux.has_eval_expiry());
        assert!(!Image::LinuxBuilder.has_eval_expiry());
    }

    #[test]
    fn every_image_has_a_slug_a_name_and_a_label_of_its_own() {
        for (index, image) in Image::ALL.into_iter().enumerate() {
            assert_eq!(Image::parse(image.slug()), Some(image));
            for other in &Image::ALL[index + 1..] {
                assert_ne!(image.slug(), other.slug());
                assert_ne!(image.vm_name(), other.vm_name());
                assert_ne!(image.label(), other.label());
            }
        }
        assert_eq!(Image::parse("Windows"), None, "records store the slug");
        assert_eq!(Image::parse(""), None);
        assert_eq!(Image::parse("macos"), None);
    }

    /// The two images that were the whole store keep their slugs, because the
    /// slug is the directory name: anything else would leave the images already
    /// on disk in a place nothing looks.
    #[test]
    fn the_desktop_images_keep_the_slugs_the_store_was_built_with() {
        assert_eq!(Image::Windows.slug(), Target::Windows.slug());
        assert_eq!(Image::Linux.slug(), Target::Linux.slug());
    }

    #[test]
    fn a_builder_is_the_same_operating_system_as_the_guest_it_builds_for() {
        for target in Target::ALL {
            assert_eq!(Image::builder(target).target(), target);
            assert_eq!(Image::desktop(target).target(), target);
            assert_ne!(Image::builder(target), Image::desktop(target));
            assert!(Image::builder(target).is_builder());
            assert!(!Image::desktop(target).is_builder());
        }
        for image in Image::ALL {
            assert_eq!(image.is_builder(), image == Image::builder(image.target()));
        }
    }

    /// A session and a purpose are two questions, and the Windows builder is
    /// where they part: it is a differencing child of the desktop image, so it
    /// logs on the session its parent was built with, and it is still not
    /// something the suite can run in.
    #[test]
    fn only_the_linux_builder_has_no_session_and_both_builders_are_builders() {
        assert!(Image::WindowsBuilder.has_desktop());
        assert!(Image::WindowsBuilder.is_builder());
        assert!(!Image::LinuxBuilder.has_desktop());
        for image in Image::ALL {
            assert_eq!(image.has_desktop(), image != Image::LinuxBuilder, "{image}");
        }
    }

    #[test]
    fn only_the_windows_builder_is_a_layer_and_it_names_its_parent() {
        assert_eq!(Image::WindowsBuilder.parent(), Some(Image::Windows));
        assert!(Image::WindowsBuilder.is_layer());
        assert_eq!(Image::Windows.children(), vec![Image::WindowsBuilder]);
        for image in [Image::Windows, Image::Linux, Image::LinuxBuilder] {
            assert_eq!(image.parent(), None, "{image}");
            assert!(!image.is_layer(), "{image}");
        }
        assert!(Image::Linux.children().is_empty());
        // A layer's disk says in its name that it does not stand on its own.
        assert_eq!(Image::WindowsBuilder.disk_stem(), "layer");
        assert_eq!(Image::Windows.disk_stem(), "golden");
        // And no image is its own parent, however the table is edited.
        for image in Image::ALL {
            assert_ne!(image.parent(), Some(image), "{image}");
        }
    }

    #[test]
    fn the_provider_matrix_matches_the_retrospective() {
        assert_eq!(
            provider_for(HostOs::Windows, Target::Windows),
            Some(ProviderKind::HyperV)
        );
        assert_eq!(
            provider_for(HostOs::Windows, Target::Linux),
            Some(ProviderKind::Qemu)
        );
        assert_eq!(
            provider_for(HostOs::Linux, Target::Windows),
            Some(ProviderKind::Qemu)
        );
        assert_eq!(
            provider_for(HostOs::Linux, Target::Linux),
            Some(ProviderKind::Qemu)
        );
        assert_eq!(provider_for(HostOs::Other, Target::Linux), None);
    }

    /// Five texts print this word in front of a `vm view` line whose command has
    /// to stay in the same column as the `ssh:` and `down:` lines around it, so
    /// the two spellings are the same width. The Windows builder is the case
    /// worth naming: the guest a viewer reaches there is its parent's desktop,
    /// and the same closing text says so twice further down.
    #[test]
    fn only_the_image_with_no_session_offers_a_console() {
        assert_eq!(Image::Linux.console_label(), "desktop");
        assert_eq!(Image::Windows.console_label(), "desktop");
        assert_eq!(Image::WindowsBuilder.console_label(), "desktop");
        assert_eq!(Image::LinuxBuilder.console_label(), "console");
        for image in Image::ALL {
            assert_eq!(image.console_label().len(), 7, "{image}");
        }
    }

    #[test]
    fn an_unsupported_host_explains_itself_rather_than_defaulting() {
        let err = resolve_provider(HostOs::Other, Target::Windows, None).unwrap_err();
        assert!(err.contains("hosted CI runners"), "{err}");
    }

    #[test]
    fn the_provider_override_wins_and_rejects_nonsense() {
        assert_eq!(
            resolve_provider(HostOs::Windows, Target::Windows, Some("qemu")),
            Ok(ProviderKind::Qemu)
        );
        assert_eq!(
            resolve_provider(HostOs::Linux, Target::Windows, Some("Hyper-V")),
            Ok(ProviderKind::HyperV)
        );
        let err =
            resolve_provider(HostOs::Windows, Target::Windows, Some("virtualbox")).unwrap_err();
        assert!(err.contains("hyperv, qemu"), "{err}");
    }
}
