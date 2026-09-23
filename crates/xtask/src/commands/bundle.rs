//! The release bundle: the binary with everything it needs to run, in the
//! archive format the target's own users open without a tool.
//!
//! Amendment decisions 29 to 31. A bare binary is not something anyone can be
//! handed: the app reads its four surface textures from disk, and those are the
//! Git LFS assets the source archive deliberately leaves out, so an artifact on
//! its own renders the procedural grid on any machine but one with a checkout
//! beside it. The bundle carries them at the layout
//! `assets::texture_loader::resolve_textures_dir` already looks for, which is a
//! `textures/` directory beside the executable.
//!
//! One assembled directory, two writers. The layout exists in one place and the
//! format question is the last thing that happens to it, which is what keeps
//! the file-mode question out of the Windows path entirely: a tar header has a
//! mode field, so 0755 on the binary is an ordinary value rather than a
//! convention somebody has to remember, and a zip's external attributes mean
//! nothing to the extractor Windows ships.
//!
//! Both archives are written with fixed timestamps, so a bundle is a function
//! of what is in it. The zip epoch is 1980 and cannot say otherwise; the
//! tarball uses the Unix epoch for the same reason rather than for that one.

use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use clap::ValueEnum;

use crate::commands::dist::{
    self, BUILD_INFO_VERSION, BuildInfo, Builder, BundleInfo, GRID_FILE, HostedInfo, SMOKE_FILE,
    SMOKE_HEIGHT, SMOKE_WIDTH, TEXTURE_LOOKUP_FLOOR,
};
use crate::commands::{bake_icon, bake_licenses};
use crate::guest::artifacts::{self, TEXTURE_FILES};
use crate::guest::toolchain;
use crate::provider::target::Target;
use crate::runner::{Cmd, Runner};
use crate::store;
use crate::util;

/// The platforms a bundle can be built for.
///
/// Not [`Target`], which names a guest this host can boot and has no macOS
/// variant to keep every `match` in `dist.rs` exhaustive. A bundle needs a
/// layout and an archive format, and those have the third case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, ValueEnum)]
pub enum Platform {
    Windows,
    Linux,
    /// Spelled the way the archives and the record spell it, rather than the
    /// `mac-os` clap would derive from the variant.
    #[value(name = "macos")]
    MacOs,
}

impl Platform {
    /// What this platform ships, in the order a run writes them.
    pub fn packages(self) -> &'static [Package] {
        match self {
            Self::Windows | Self::Linux => &[Package::Plain],
            Self::MacOs => &[Package::Plain, Package::App],
        }
    }

    /// All three, in the order reports list them.
    ///
    /// Only the tests walk the set; clap derives its own list for the flag.
    #[cfg(test)]
    pub const ALL: [Self; 3] = [Self::Windows, Self::Linux, Self::MacOs];

    /// The lowercase name in archive names, paths and records.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::MacOs => "macos",
        }
    }

    /// What this host is, which is the only platform whose bundle it can run.
    ///
    /// `None` on anything else, because a bundle is verified by rendering from
    /// it and nothing here emulates another operating system.
    pub fn host() -> Option<Self> {
        if cfg!(windows) {
            Some(Self::Windows)
        } else if cfg!(target_os = "linux") {
            Some(Self::Linux)
        } else if cfg!(target_os = "macos") {
            Some(Self::MacOs)
        } else {
            None
        }
    }
}

impl From<Target> for Platform {
    fn from(target: Target) -> Self {
        match target {
            Target::Windows => Self::Windows,
            Target::Linux => Self::Linux,
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

/// The instruction set a bundled binary is compiled for.
///
/// Read out of the binary's own header rather than passed in, so an archive
/// cannot be named for an architecture other than the one inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    #[cfg(test)]
    pub const ALL: [Self; 2] = [Self::X86_64, Self::Aarch64];

    /// The name in archive names and records, which is Rust's own spelling in
    /// a target triple.
    pub fn slug(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }

    /// The platform and architecture a binary's header declares.
    ///
    /// PE for Windows, ELF for Linux and a thin 64-bit Mach-O for macOS. A
    /// universal Mach-O is refused rather than named for one of its slices:
    /// every archive holds exactly one architecture.
    pub fn of_binary(bytes: &[u8]) -> Result<(Platform, Self), String> {
        let u16_le = |at: usize| {
            bytes
                .get(at..at + 2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
        };
        let u32_le = |at: usize| {
            bytes
                .get(at..at + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let truncated = || "the binary's header is truncated".to_owned();
        match bytes.get(..4).ok_or_else(truncated)? {
            [b'M', b'Z', ..] => {
                let pe = u32_le(0x3C).ok_or_else(truncated)? as usize;
                if bytes.get(pe..pe + 4) != Some(b"PE\0\0") {
                    return Err("an MZ header without a PE signature after it".to_owned());
                }
                match u16_le(pe + 4).ok_or_else(truncated)? {
                    0x8664 => Ok((Platform::Windows, Self::X86_64)),
                    0xAA64 => Ok((Platform::Windows, Self::Aarch64)),
                    other => Err(format!("a PE binary for machine type {other:#06x}")),
                }
            }
            [0x7F, b'E', b'L', b'F'] => {
                let machine = match bytes.get(5) {
                    Some(1) => u16_le(18),
                    Some(2) => bytes.get(18..20).map(|b| u16::from_be_bytes([b[0], b[1]])),
                    _ => return Err("an ELF binary of unknown byte order".to_owned()),
                };
                match machine.ok_or_else(truncated)? {
                    62 => Ok((Platform::Linux, Self::X86_64)),
                    183 => Ok((Platform::Linux, Self::Aarch64)),
                    other => Err(format!("an ELF binary for machine {other}")),
                }
            }
            [0xCF, 0xFA, 0xED, 0xFE] => match u32_le(4).ok_or_else(truncated)? {
                0x0100_0007 => Ok((Platform::MacOs, Self::X86_64)),
                0x0100_000C => Ok((Platform::MacOs, Self::Aarch64)),
                other => Err(format!("a Mach-O binary for CPU type {other:#010x}")),
            },
            [0xCA, 0xFE, 0xBA, 0xBE | 0xBF] => Err(
                "a universal Mach-O, which holds more than one architecture; bundle \
                 each slice on its own"
                    .to_owned(),
            ),
            _ => Err("not a PE, ELF or Mach-O binary".to_owned()),
        }
    }

    /// Read the header of the binary at `exe`.
    pub fn of(exe: &Path) -> Result<(Platform, Self), String> {
        use std::io::Read as _;
        let mut head = Vec::with_capacity(4096);
        std::fs::File::open(exe)
            .and_then(|file| file.take(4096).read_to_end(&mut head))
            .map_err(|e| format!("cannot read {}: {e}", exe.display()))?;
        Self::of_binary(&head).map_err(|why| format!("{}: {why}", exe.display()))
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

/// The package the bundle is named after, which is also the binary's stem.
pub const PACKAGE: &str = "sunlit-earth";

/// The licence the workspace declares, as a file at the repository root.
pub const LICENSE: &str = "LICENSE";

/// The record, written beside the archives and not inside them.
pub const RECORD: &str = "build-info.json";

/// The macOS bundle's own directory name, which is what a user sees in Finder
/// and double-clicks, and the one top-level entry its zip holds.
pub const APP_DIR: &str = "Sunlit Earth.app";

/// The `Info.plist` the bundle is assembled from, relative to the repository
/// root. Every value in it is fixed but the version.
pub const INFO_PLIST: &str = "assets/macos/Info.plist";

/// What the template carries where the workspace version goes.
pub const VERSION_PLACEHOLDER: &str = "@VERSION@";

/// What a bundle is shaped like, which is not the same question as which
/// platform it is for.
///
/// macOS ships both: the `.app` a person double-clicks, and the plain layout
/// in a tarball, which is the one a tester unpacks in Terminal where `tar` sets
/// no quarantine attribute. The same binary is in both, so the linker's ad-hoc
/// signature travels inside the Mach-O either way and nothing is signed twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Package {
    /// One directory holding the executable with its textures beside it.
    Plain,
    /// `Sunlit Earth.app`, ad-hoc signed and zipped by Apple's own `ditto`.
    App,
}

/// The two archive formats, one per target.
///
/// Each target gets what its own users expect rather than one format for both:
/// a zip is what Windows opens with no tool at all, and a tarball is what a
/// Linux user reaches for and what carries a file mode in the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Zip,
    TarGz,
}

impl Format {
    pub fn of(platform: Platform) -> Self {
        match platform {
            Platform::Windows => Self::Zip,
            // A tarball and deliberately not a zip: it is what a tester
            // unpacks in Terminal, past Gatekeeper. The `.app` is the zip.
            Platform::Linux | Platform::MacOs => Self::TarGz,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::TarGz => "tar.gz",
        }
    }
}

/// Where one file's bytes come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A file on the host: the binary this run built, or something the
    /// repository ships.
    File(PathBuf),
    /// Bytes this run made, which is the macOS `Info.plist` and nothing else,
    /// because its version is not known until the manifest is read.
    Made(Vec<u8>),
}

/// One file in the bundle: where it sits, and what it is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Its path inside the bundle's top-level directory, with forward slashes.
    /// Both archive formats want them that way, and it is what the read-back
    /// comparison is made in.
    pub path: String,
    pub source: Source,
    /// Whether the tarball's header says 0755. Without it the first thing a
    /// Linux user does is `chmod +x`.
    pub executable: bool,
}

impl Item {
    /// A file copied from the host.
    fn file(path: impl Into<String>, source: impl Into<PathBuf>, executable: bool) -> Self {
        Self {
            path: path.into(),
            source: Source::File(source.into()),
            executable,
        }
    }

    /// A file this run wrote the bytes of.
    fn made(path: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            path: path.into(),
            source: Source::Made(bytes),
            executable: false,
        }
    }
}

/// A bundle assembled on disk, waiting to be staged and archived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundled {
    /// The one directory an unpack produces.
    pub name: String,
    /// Where that directory is on the host.
    pub root: PathBuf,
    pub items: Vec<Item>,
}

/// The host paths a bundle is assembled from.
#[derive(Debug, Clone, Copy)]
pub struct Sources<'a> {
    pub repo: &'a Path,
    /// The release binary, as it came back from the builder.
    pub exe: &'a Path,
    /// The repository's `textures/`, which decision 33 makes the condition for
    /// there being a bundle at all: without the assets it would promise a
    /// release and render a grid.
    pub textures: &'a Path,
}

/// The bundle's own name, which is also the one directory an unpack produces.
///
/// From the workspace `Cargo.toml`'s version rather than from `git describe`:
/// this repository has no tags, so `describe` is a bare hash and
/// `sunlit-earth-4b4cb2e-windows.zip` tells a user nothing. The hash is inside,
/// in the record. The architecture is in the name because one release carries
/// more than one per platform.
pub fn bundle_name(version: &str, platform: Platform, arch: Arch) -> String {
    format!("{PACKAGE}-{version}-{}-{}", platform.slug(), arch.slug())
}

/// Which writer and reader one package of one platform uses.
///
/// The `.app` is always a zip, because `ditto` writes one and because that is
/// what macOS itself hands round; the plain layout takes whatever its platform's
/// users open without a tool.
pub fn archive_format(platform: Platform, package: Package) -> Format {
    match package {
        Package::Plain => Format::of(platform),
        Package::App => Format::Zip,
    }
}

/// Named for the release rather than for the `.app` inside it: a download
/// called `Sunlit Earth.app.zip` says nothing about which version it is.
pub fn app_archive_name(version: &str, arch: Arch) -> String {
    format!("{}.zip", bundle_name(version, Platform::MacOs, arch))
}

/// What the `.app` holds, in the order it is assembled.
///
/// The layout Apple's bundle format fixes: the executable at `Contents/MacOS/`,
/// everything it reads at `Contents/Resources/`, and the property list that
/// names them both at `Contents/`. `resolve_textures_dir` finds
/// `Contents/Resources/textures` by walking up from the executable, which is
/// the one candidate this layout needed adding.
///
/// `CFBundleShortVersionString` is what a user is shown, so the version is
/// substituted here rather than left to a build step. The prerelease suffix
/// goes in as it is.
pub fn app_layout(sources: &Sources, version: &str) -> Result<Vec<Item>, String> {
    let template_path = sources.repo.join(INFO_PLIST);
    let template = std::fs::read_to_string(&template_path)
        .map_err(|e| format!("cannot read {}: {e}", template_path.display()))?;
    if !template.contains(VERSION_PLACEHOLDER) {
        return Err(format!(
            "{} carries no {VERSION_PLACEHOLDER}, so the bundle would be named after \
             no version at all",
            template_path.display()
        ));
    }
    let plist = template.replace(VERSION_PLACEHOLDER, version);

    let resources = "Contents/Resources";
    let mut items = vec![
        Item::made("Contents/Info.plist", plist.into_bytes()),
        Item::file(format!("Contents/MacOS/{PACKAGE}"), sources.exe, true),
        Item::file(
            format!("{resources}/{}", bake_icon::ICNS_FILE),
            sources
                .repo
                .join(bake_icon::BAKED_DIR)
                .join(bake_icon::ICNS_FILE),
            false,
        ),
    ];
    for name in TEXTURE_FILES {
        items.push(Item::file(
            format!("{resources}/textures/{name}"),
            sources.textures.join(name),
            false,
        ));
    }
    items.push(Item::file(
        format!("{resources}/{LICENSE}"),
        sources.repo.join(LICENSE),
        false,
    ));
    items.push(Item::file(
        format!("{resources}/{}", bake_licenses::NOTICES_NAME),
        sources.repo.join(bake_licenses::NOTICES_PATH),
        false,
    ));
    Ok(items)
}

/// The archive's file name, which is the bundle's name plus its format.
pub fn archive_name(version: &str, platform: Platform, arch: Arch) -> String {
    format!(
        "{}.{}",
        bundle_name(version, platform, arch),
        Format::of(platform).extension()
    )
}

/// The version out of the workspace manifest's `[workspace.package]`.
///
/// Not `env!("CARGO_PKG_VERSION")`, which is the version of the tree the xtask
/// binary was compiled from and not necessarily the one being built.
pub fn parse_version(manifest: &str) -> Result<String, String> {
    let file: toml::Value = toml::from_str(manifest)
        .map_err(|e| format!("the workspace manifest does not parse: {e}"))?;
    file.get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            "the workspace manifest has no [workspace.package] version, which is \
             what a bundle is named after"
                .to_owned()
        })
}

/// Read it from a repository root.
pub fn version(repo: &Path) -> Result<String, String> {
    let path = repo.join("Cargo.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_version(&text)
}

/// What the bundle holds, in the order it is assembled.
///
/// A Windows bundle carries no install kit, and the asymmetry is deliberate:
/// there is nothing to install there, because the icon is a resource inside the
/// exe and there is no menu entry to place. On Linux `install-user.sh` exists
/// exactly for somebody holding a binary and no package, and it reads the entry
/// and the icons by a path relative to itself, so the three keep the layout the
/// repository gives them.
pub fn layout(platform: Platform, sources: &Sources) -> Vec<Item> {
    let mut items = vec![Item::file(
        exe_name(platform).to_owned(),
        sources.exe.to_path_buf(),
        true,
    )];

    for name in TEXTURE_FILES {
        items.push(Item::file(
            format!("textures/{name}"),
            sources.textures.join(name),
            false,
        ));
    }

    items.push(Item::file(
        LICENSE.to_owned(),
        sources.repo.join(LICENSE),
        false,
    ));
    items.push(Item::file(
        bake_licenses::NOTICES_NAME.to_owned(),
        sources.repo.join(bake_licenses::NOTICES_PATH),
        false,
    ));

    if platform == Platform::Linux {
        let linux = sources.repo.join("assets").join("linux");
        let icon = sources.repo.join("assets").join("icon");
        items.push(Item::file(
            "assets/linux/install-user.sh".to_owned(),
            linux.join("install-user.sh"),
            true,
        ));
        items.push(Item::file(
            "assets/linux/sunlit-earth.desktop".to_owned(),
            linux.join("sunlit-earth.desktop"),
            false,
        ));
        items.push(Item::file(
            format!("assets/icon/{}.svg", bake_icon::ICON_NAME),
            icon.join(format!("{}.svg", bake_icon::ICON_NAME)),
            false,
        ));
        // Named through the bake's own list rather than by walking the baked
        // directory, so a bundle carries exactly what the bake writes and a
        // size that appeared on one side and not the other fails a test here.
        let baked = sources.repo.join(bake_icon::BAKED_DIR);
        for size in bake_icon::HICOLOR_SIZES {
            let relative = bake_icon::hicolor_path(size);
            items.push(Item::file(
                format!(
                    "assets/icon/baked/{}",
                    relative.to_string_lossy().replace('\\', "/")
                ),
                baked.join(relative),
                false,
            ));
        }
    }

    items
}

/// The binary's name inside the bundle.
pub fn exe_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Windows => "sunlit-earth.exe",
        Platform::Linux | Platform::MacOs => "sunlit-earth",
    }
}

/// Whether an entry is stored rather than deflated.
///
/// JXL is already compressed, so deflating it spends time to make it slightly
/// larger. Everything else in a bundle is a binary, a PNG or text, and the
/// first of those is the one that compresses several fold.
pub fn stored(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jxl"))
}

/// Build the bundle as a directory under `parent`, and answer where it is.
///
/// A directory first, because it is what the verification boot stages into the
/// desktop guest: what is under test there is the lookup a user's machine does,
/// walking up from the executable to the `textures/` beside it, and that needs
/// the real layout rather than an archive.
pub fn assemble(parent: &Path, name: &str, items: &[Item]) -> Result<PathBuf, String> {
    let root = parent.join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    for item in items {
        let target = root.join(&item.path);
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        match &item.source {
            Source::File(source) => {
                std::fs::copy(source, &target).map_err(|e| {
                    format!(
                        "the bundle needs {} and cannot copy it: {e}",
                        source.display()
                    )
                })?;
            }
            Source::Made(bytes) => {
                std::fs::write(&target, bytes)
                    .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
            }
        }
    }
    Ok(root)
}

/// Write the archive the target's format names, from the assembled directory.
pub fn write(
    format: Format,
    root: &Path,
    name: &str,
    items: &[Item],
    output: &Path,
) -> Result<u64, String> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let _ = std::fs::remove_file(output);
    match format {
        Format::Zip => write_zip(root, name, items, output),
        Format::TarGz => write_tar_gz(root, name, items, output),
    }?;
    std::fs::metadata(output)
        .map(|meta| meta.len())
        .map_err(|e| format!("the bundle writer left no {}: {e}", output.display()))
}

fn write_zip(root: &Path, name: &str, items: &[Item], output: &Path) -> Result<(), String> {
    let file = std::fs::File::create(output)
        .map_err(|e| format!("cannot create {}: {e}", output.display()))?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    for item in items {
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(if stored(&item.path) {
                zip::CompressionMethod::Stored
            } else {
                zip::CompressionMethod::Deflated
            })
            // Meaningless to the extractor Windows ships and read by the ones
            // it does not, which is the whole reason the mode question belongs
            // to the tarball instead.
            .unix_permissions(if item.executable { 0o755 } else { 0o644 })
            // A release binary is under the four-gigabyte mark by two orders of
            // magnitude, so this is about the header the reader gets rather
            // than about the size.
            .large_file(false);
        let entry = format!("{name}/{}", item.path);
        zip.start_file(&entry, options)
            .map_err(|e| format!("cannot start {entry} in the zip: {e}"))?;
        let bytes = std::fs::read(root.join(&item.path))
            .map_err(|e| format!("cannot read the assembled {}: {e}", item.path))?;
        zip.write_all(&bytes)
            .map_err(|e| format!("cannot write {entry} into the zip: {e}"))?;
    }
    zip.finish()
        .map_err(|e| format!("cannot finish the zip: {e}"))?
        .flush()
        .map_err(|e| format!("cannot flush the zip: {e}"))
}

fn write_tar_gz(root: &Path, name: &str, items: &[Item], output: &Path) -> Result<(), String> {
    let file = std::fs::File::create(output)
        .map_err(|e| format!("cannot create {}: {e}", output.display()))?;
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for item in items {
        let bytes = std::fs::read(root.join(&item.path))
            .map_err(|e| format!("cannot read the assembled {}: {e}", item.path))?;
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(if item.executable { 0o755 } else { 0o644 });
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        let entry = format!("{name}/{}", item.path);
        builder
            .append_data(&mut header, &entry, bytes.as_slice())
            .map_err(|e| format!("cannot write {entry} into the tarball: {e}"))?;
    }
    builder
        .into_inner()
        .map_err(|e| format!("cannot finish the tarball: {e}"))?
        .finish()
        .map(|_| ())
        .map_err(|e| format!("cannot finish the gzip stream: {e}"))
}

/// One entry as the finished archive itself reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archived {
    pub path: String,
    pub bytes: u64,
    /// The mode in the header, for the format that has one. A zip's external
    /// attributes are not a file mode on the platform that reads them, so this
    /// is `None` there rather than a number nobody should trust.
    pub mode: Option<u32>,
}

/// Read the finished archive back with the crate that wrote it.
///
/// The cheap half of proving the bundle: that the archive holds what the
/// directory holds. The expensive half is the two-render comparison in the
/// desktop guest, which is what proves the textures are found.
pub fn read_back(format: Format, archive: &Path) -> Result<Vec<Archived>, String> {
    let file = std::fs::File::open(archive)
        .map_err(|e| format!("cannot reopen {}: {e}", archive.display()))?;
    let mut out = Vec::new();
    match format {
        Format::Zip => {
            let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
                .map_err(|e| format!("{} is not a readable zip: {e}", archive.display()))?;
            for index in 0..zip.len() {
                let entry = zip
                    .by_index(index)
                    .map_err(|e| format!("entry {index} of the zip does not read: {e}"))?;
                if entry.is_dir() {
                    continue;
                }
                out.push(Archived {
                    path: entry.name().to_owned(),
                    bytes: entry.size(),
                    mode: None,
                });
            }
        }
        Format::TarGz => {
            let decoder = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
            let mut tar = tar::Archive::new(decoder);
            let entries = tar
                .entries()
                .map_err(|e| format!("{} is not a readable tarball: {e}", archive.display()))?;
            for entry in entries {
                let entry =
                    entry.map_err(|e| format!("an entry of the tarball does not read: {e}"))?;
                let header = entry.header();
                if !header.entry_type().is_file() {
                    continue;
                }
                let path = entry
                    .path()
                    .map_err(|e| format!("an entry of the tarball has no readable path: {e}"))?
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push(Archived {
                    path,
                    bytes: header.size().unwrap_or(0),
                    mode: header.mode().ok(),
                });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// Every file in the assembled directory, by its path inside it.
pub fn walk(root: &Path) -> Result<Vec<(String, u64)>, String> {
    let mut out = Vec::new();
    collect(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, u64)>) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out)?;
        } else if let Ok(meta) = std::fs::metadata(&path) {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| format!("{} escaped the bundle", path.display()))?
                .to_string_lossy()
                .replace('\\', "/");
            out.push((relative, meta.len()));
        }
    }
    Ok(())
}

/// Whether the archive holds exactly what the directory holds.
///
/// Names and sizes both: a writer that truncated an entry is the failure this
/// exists for, and an archive missing the textures is a bundle that promises a
/// release and renders a grid.
pub fn verify(
    name: &str,
    assembled: &[(String, u64)],
    archived: &[Archived],
) -> Result<(), String> {
    let expected: Vec<(String, u64)> = assembled
        .iter()
        .map(|(path, bytes)| (format!("{name}/{path}"), *bytes))
        .collect();
    let found: Vec<(String, u64)> = archived
        .iter()
        .map(|entry| (entry.path.clone(), entry.bytes))
        .collect();
    if expected == found {
        return Ok(());
    }
    let missing: Vec<&str> = expected
        .iter()
        .filter(|(path, _)| !found.iter().any(|(other, _)| other == path))
        .map(|(path, _)| path.as_str())
        .collect();
    let extra: Vec<&str> = found
        .iter()
        .filter(|(path, _)| !expected.iter().any(|(other, _)| other == path))
        .map(|(path, _)| path.as_str())
        .collect();
    let resized: Vec<String> = expected
        .iter()
        .filter_map(|(path, bytes)| {
            found
                .iter()
                .find(|(other, _)| other == path)
                .filter(|(_, other)| other != bytes)
                .map(|(_, other)| {
                    format!("{path} is {other} bytes in the archive and {bytes} on disk")
                })
        })
        .collect();
    Err(format!(
        "the archive does not hold what was assembled: {} missing, {} unexpected, {} the wrong size{}{}{}",
        missing.len(),
        extra.len(),
        resized.len(),
        list(" missing: ", &missing),
        list(" unexpected: ", &extra),
        if resized.is_empty() {
            String::new()
        } else {
            format!("; {}", resized.join("; "))
        }
    ))
}

fn list(prefix: &str, items: &[&str]) -> String {
    if items.is_empty() {
        String::new()
    } else {
        format!(";{prefix}{}", items.join(", "))
    }
}

/// What the closing summary prints about a bundle that was written.
///
/// A bundle nothing ran carries the caveat on the same line that names it,
/// rather than leaving it to the run summary underneath: `--no-verify` writes
/// the archive like any other run, and the one thing that separates it from a
/// verified one is a sentence somebody has to still be reading to see.
pub fn summary(archive: &Path, verified: bool) -> String {
    let mut text = format!("  and {}, which holds {HOLDS}", archive.display());
    if !verified {
        text.push('\n');
        text.push_str(UNVERIFIED);
    }
    text
}

/// What every archive holds, wherever it is said.
const HOLDS: &str = "the binary with its textures beside it, the record and the licence";

/// What a run that skipped the verification has to say about the bundle it
/// therefore never ran.
const UNVERIFIED: &str = "  nothing has run this bundle: --no-verify skipped the boot that \
                          renders from it, so nothing has shown that its textures are found \
                          where it puts them, and its record says so with a null verified_in";

/// Why there is no bundle, when the host holds Git LFS pointers rather than the
/// assets.
///
/// A bundle without the textures would be a bundle that renders a grid under a
/// name promising a release, which is worse than not writing one.
pub fn skipped_note() -> String {
    "no bundle: this checkout holds Git LFS pointers rather than the texture \
     assets, and a bundle without them would render the procedural grid under a \
     name that promises a release. `git lfs pull` fetches them; the loose binary \
     and its record are in the dist directory either way."
        .to_owned()
}

// --- `cargo xtask bundle` -------------------------------------------------
//
// Everything above is the library `dist` has always called from inside a VM
// run. What follows is the same work asked for directly, which is what the
// release runners do: they have the binary already and no hypervisor at all.

/// What one `cargo xtask bundle` run was asked for.
///
/// Declared here rather than in `main.rs`, as `Platform` already is.
#[derive(Debug, Clone, clap::Args)]
pub struct Options {
    /// Which platform's layout and archive format.
    #[arg(long)]
    pub platform: Platform,
    /// The release binary to put in it. Not built here.
    #[arg(long, value_name = "PATH")]
    pub exe: PathBuf,
    /// Where the archives and their record go. The dist directory by default,
    /// so a bundle written by hand lands where `dist` puts one.
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
    /// Unpack each archive and render from it twice, which is what proves the
    /// binary finds the textures beside it. This host's own platform only.
    #[arg(long)]
    pub verify: bool,
}

/// Where a bundle is assembled before it is archived, under the output
/// directory and removed when the run ends.
const STAGE_DIR: &str = ".stage";

/// Where an archive is unpacked to be rendered from.
const VERIFY_DIR: &str = ".verify";

/// Everything one package of one run needs.
struct Run<'a> {
    runner: &'a dyn Runner,
    repo: &'a Path,
    exe: &'a Path,
    textures: &'a Path,
    out: &'a Path,
    version: &'a str,
    platform: Platform,
    arch: Arch,
    verify: bool,
}

impl Run<'_> {
    fn sources(&self) -> Sources<'_> {
        Sources {
            repo: self.repo,
            exe: self.exe,
            textures: self.textures,
        }
    }
}

/// Assemble every package this platform ships, archive each, read each back,
/// optionally render from each, and write one record beside them.
///
/// The verification is the two renders `dist` runs in a desktop guest, for the
/// same reason: a binary that found no textures still writes a 640x360 PNG, and
/// only the comparison against one rendered against an empty directory tells a
/// globe from a grid. A runner is not a pristine guest, and the record says
/// which of the two it was.
pub fn run(runner: &dyn Runner, options: &Options) -> Result<u8, String> {
    let started = std::time::Instant::now();
    let repo = store::repo_root();
    let version = version(&repo)?;
    let platform = options.platform;

    if !options.exe.is_file() {
        return Err(format!(
            "there is no binary at {}, and this command bundles one rather than \
             building it: `cargo build --release -p sunlit-earth` writes it under \
             the target directory",
            options.exe.display()
        ));
    }
    let (built_for, arch) = Arch::of(&options.exe)?;
    if built_for != platform {
        return Err(format!(
            "{} is a {built_for} binary, and this is a {platform} bundle",
            options.exe.display()
        ));
    }
    println!("binary: {platform} {arch}");
    // Not the skip `dist` falls back to: that run has a loose binary to publish
    // instead, and this command has nothing else to produce.
    let textures = artifacts::textures_present(&repo).map_err(|why| {
        format!(
            "{why}\n`git lfs pull` fetches the texture assets; without them a bundle \
             would render the procedural grid under a name that promises a release."
        )
    })?;

    // Absolute before anything is derived from it: `--verify` runs the binary
    // from a working directory of its own, and every path derived here reaches
    // that child.
    let out = std::path::absolute(
        options
            .out
            .clone()
            .unwrap_or_else(|| dist::dist_dir(platform)),
    )
    .map_err(|e| format!("cannot resolve the output directory: {e}"))?;
    std::fs::create_dir_all(&out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;

    let run = Run {
        runner,
        repo: &repo,
        exe: &options.exe,
        textures: &textures,
        out: &out,
        version: &version,
        platform,
        arch,
        verify: options.verify,
    };

    let mut bundles = Vec::new();
    let outcome = (|| -> Result<(), String> {
        for package in platform.packages() {
            if *package == Package::App && Platform::host() != Some(Platform::MacOs) {
                println!(
                    "  no {APP_DIR}: `codesign` and `ditto` are macOS's own tools and \
                     this host is not one, so only the tarball was written. Its binary \
                     is the same one the bundle would hold."
                );
                continue;
            }
            bundles.push(one_package(&run, *package)?);
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(out.join(STAGE_DIR));
    outcome?;

    let record = record(runner, &repo, platform, arch, bundles, started)?;
    std::fs::write(out.join(RECORD), record.to_json())
        .map_err(|e| format!("cannot write {}: {e}", out.join(RECORD).display()))?;

    println!();
    for bundle in &record.bundles {
        let archive = out.join(&bundle.archive);
        println!("{platform}: {}, which holds {HOLDS}", archive.display());
        if bundle.texture_lookup_delta.is_none() {
            println!("{UNVERIFIED}");
        }
    }
    Ok(0)
}

/// Assemble, archive, read back and verify one package.
fn one_package(run: &Run, package: Package) -> Result<BundleInfo, String> {
    let (name, archive_file) = match package {
        Package::Plain => (
            bundle_name(run.version, run.platform, run.arch),
            archive_name(run.version, run.platform, run.arch),
        ),
        Package::App => (APP_DIR.to_owned(), app_archive_name(run.version, run.arch)),
    };
    let items = match package {
        Package::Plain => layout(run.platform, &run.sources()),
        Package::App => app_layout(&run.sources(), run.version)?,
    };
    let stage = run.out.join(STAGE_DIR);
    let root = assemble(&stage, &name, &items)?;
    println!("bundle: {name}, {}", util::count(items.len(), "file"));

    let archive = run.out.join(&archive_file);
    let bytes = match package {
        Package::Plain => write(
            archive_format(run.platform, package),
            &root,
            &name,
            &items,
            &archive,
        )?,
        Package::App => {
            // The linker's ad-hoc seal covers the raw executable only, and a
            // bundle around it reports itself damaged until the bundle too is
            // sealed. No `--deep`: it is deprecated, and there is no nested
            // code here for it to reach.
            sign(run.runner, &root)?;
            ditto_pack(run.runner, &root, &archive)?
        }
    };

    // Walked after signing, because `codesign` writes `_CodeSignature` into the
    // bundle and that is part of what the archive has to carry.
    let assembled = walk(&root)?;
    let archived: Vec<Archived> = read_back(archive_format(run.platform, package), &archive)?
        .into_iter()
        .filter(|entry| !is_apple_metadata(&entry.path))
        .collect();
    verify(&name, &assembled, &archived)?;
    println!(
        "  {archive_file} ({}), {} read back and matched",
        util::format_bytes(bytes),
        util::count(archived.len(), "file")
    );

    let delta = verify_here(run, package, &archive, &name)?;
    Ok(BundleInfo {
        name,
        archive: archive_file,
        entries: assembled.len(),
        texture_lookup_delta: delta,
    })
}

/// Whether an archive entry is metadata the archiver added rather than a file
/// the bundle holds.
///
/// `ditto` stores extended attributes and resource forks beside the files they
/// belong to, under `__MACOSX/` and as `._`-prefixed siblings. Dropping them
/// from the read-back is what keeps the comparison exact.
fn is_apple_metadata(path: &str) -> bool {
    path.starts_with("__MACOSX/")
        || Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("._"))
}

/// Ad-hoc sign the assembled bundle in place.
fn sign(runner: &dyn Runner, app: &Path) -> Result<(), String> {
    let cmd = Cmd::new("codesign").args([
        "--force".to_owned(),
        "--sign".to_owned(),
        "-".to_owned(),
        app.to_string_lossy().into_owned(),
    ]);
    tool(runner, &cmd).map(|_| ())
}

/// Zip the bundle with Apple's own archiver, which is what preserves the
/// signature, the permissions and the symlinks a `.app` may hold.
fn ditto_pack(runner: &dyn Runner, app: &Path, archive: &Path) -> Result<u64, String> {
    let _ = std::fs::remove_file(archive);
    let cmd = Cmd::new("ditto").args([
        "-c".to_owned(),
        "-k".to_owned(),
        // The bundle itself is the one top-level entry an unpack produces,
        // rather than its contents scattered into the download folder.
        "--keepParent".to_owned(),
        app.to_string_lossy().into_owned(),
        archive.to_string_lossy().into_owned(),
    ]);
    tool(runner, &cmd)?;
    std::fs::metadata(archive)
        .map(|meta| meta.len())
        .map_err(|e| format!("ditto left no {}: {e}", archive.display()))
}

/// Unpack a `.app` zip with the tool that wrote it.
fn ditto_unpack(runner: &dyn Runner, archive: &Path, into: &Path) -> Result<(), String> {
    std::fs::create_dir_all(into).map_err(|e| format!("cannot create {}: {e}", into.display()))?;
    let cmd = Cmd::new("ditto").args([
        "-x".to_owned(),
        "-k".to_owned(),
        archive.to_string_lossy().into_owned(),
        into.to_string_lossy().into_owned(),
    ]);
    tool(runner, &cmd).map(|_| ())
}

/// Run one of Apple's tools and answer with its output.
fn tool(runner: &dyn Runner, cmd: &Cmd) -> Result<String, String> {
    let shown = cmd.display();
    let out = runner
        .capture(cmd)
        .map_err(|e| format!("cannot run `{shown}`: {e}"))?;
    if out.success() {
        Ok(out.stdout)
    } else {
        Err(format!(
            "`{shown}` failed: {} {}",
            out.stderr.trim(),
            out.stdout.trim()
        ))
    }
}

/// Unpack an archive into a directory, with the crate that wrote it.
///
/// The tarball's mode field is restored here, without which the binary unpacks
/// at 0644 and the render below fails on permissions. Rendering from the
/// unpacked archive is the point: the archive is what a user is handed.
pub fn unpack(format: Format, archive: &Path, into: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive)
        .map_err(|e| format!("cannot reopen {}: {e}", archive.display()))?;
    std::fs::create_dir_all(into).map_err(|e| format!("cannot create {}: {e}", into.display()))?;
    match format {
        Format::Zip => zip::ZipArchive::new(std::io::BufReader::new(file))
            .map_err(|e| format!("{} is not a readable zip: {e}", archive.display()))?
            .extract(into)
            .map_err(|e| format!("cannot unpack {}: {e}", archive.display())),
        Format::TarGz => {
            let decoder = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
            tar::Archive::new(decoder)
                .unpack(into)
                .map_err(|e| format!("cannot unpack {}: {e}", archive.display()))
        }
    }
}

/// Unpack the archive and render from it twice, and answer how far apart the
/// two renders are.
///
/// `None` when nothing ran it, which is what the record carries too, and the
/// reason this takes the flag rather than being called behind one.
fn verify_here(
    run: &Run,
    package: Package,
    archive: &Path,
    name: &str,
) -> Result<Option<f64>, String> {
    if !run.verify {
        println!("  nothing has run this bundle: --verify was not given");
        return Ok(None);
    }
    if Platform::host() != Some(run.platform) {
        return Err(format!(
            "--verify runs the bundle, and this host is not {}: a {} bundle is \
             verified on a {} machine, which for the release pipeline is the \
             runner that built it",
            run.platform, run.platform, run.platform
        ));
    }
    let work = run.out.join(VERIFY_DIR);
    let _ = std::fs::remove_dir_all(&work);
    match package {
        Package::Plain => unpack(archive_format(run.platform, package), archive, &work)?,
        Package::App => ditto_unpack(run.runner, archive, &work)?,
    }
    let root = work.join(name);
    if !root.is_dir() {
        return Err(format!(
            "the archive unpacked to something other than {}, so it does not hold the \
             one directory a bundle is",
            root.display()
        ));
    }
    if package == Package::App {
        // The seal survived the archive and the unpack, which is the whole
        // reason this format is `ditto`'s and not the zip crate's.
        tool(
            run.runner,
            &Cmd::new("codesign").args([
                "--verify".to_owned(),
                "--strict".to_owned(),
                root.to_string_lossy().into_owned(),
            ]),
        )?;
        println!("  codesign --verify --strict passed on the unpacked bundle");
    }

    // The empty directory is what makes the grid render a grid: the loader takes
    // the variable's directory when it is one and finds no files in it.
    let empty = work.join(dist::EMPTY_TEXTURES);
    std::fs::create_dir_all(&empty)
        .map_err(|e| format!("cannot create {}: {e}", empty.display()))?;

    let exe = match package {
        Package::Plain => root.join(exe_name(run.platform)),
        Package::App => root.join("Contents").join("MacOS").join(PACKAGE),
    };
    let at = |cmd: Cmd| -> Result<(), String> { tool(run.runner, &cmd).map(|_| ()) };

    // The working directory is outside the bundle on purpose:
    // `resolve_textures_dir` tries a `textures` beside it before walking up
    // from the executable, and it is the walk-up this has to test.
    let render = |output: &Path| {
        vec![
            "render".to_owned(),
            "--output".to_owned(),
            output.to_string_lossy().into_owned(),
            "--width".to_owned(),
            SMOKE_WIDTH.to_string(),
            "--height".to_owned(),
            SMOKE_HEIGHT.to_string(),
        ]
    };
    let base = || Cmd::new(exe.to_string_lossy().into_owned()).cwd(&work);
    at(base().arg("--version"))?;

    let grid = work.join(GRID_FILE);
    at(base()
        .args(render(&grid))
        .env("SUNLIT_EARTH_TEXTURES", empty.to_string_lossy()))?;

    let smoke = work.join(SMOKE_FILE);
    at(base().args(render(&smoke)).unset("SUNLIT_EARTH_TEXTURES"))?;

    let read = |path: &Path| -> Result<Vec<u8>, String> {
        let bytes = std::fs::read(path)
            .map_err(|e| format!("the render left no {}: {e}", path.display()))?;
        match dist::png_size(&bytes) {
            Some((SMOKE_WIDTH, SMOKE_HEIGHT)) => Ok(bytes),
            Some((w, h)) => Err(format!(
                "{} is {w}x{h}, and the render was asked for {SMOKE_WIDTH}x{SMOKE_HEIGHT}",
                path.display()
            )),
            None => Err(format!("{} is not a PNG", path.display())),
        }
    };
    let delta = dist::render_difference(&read(&grid)?, &read(&smoke)?)?;
    if delta < TEXTURE_LOOKUP_FLOOR {
        // Left on disk, because the refusal names it: both renders are what a
        // reader needs to see to know which of the two went wrong.
        return Err(dist::grid_refusal(delta, &work));
    }
    // The render the bundle made travels with it, the way `dist` publishes one,
    // and the unpacked copy does not: a runner uploads this directory whole.
    std::fs::copy(&smoke, run.out.join(SMOKE_FILE))
        .map_err(|e| format!("cannot keep the bundle's own render: {e}"))?;
    let _ = std::fs::remove_dir_all(&work);
    println!("  verified here: the two renders differ by {delta:.2} of a channel step");
    Ok(Some(delta))
}

/// The record written beside the archives.
fn record(
    runner: &dyn Runner,
    repo: &Path,
    platform: Platform,
    arch: Arch,
    bundles: Vec<BundleInfo>,
    started: std::time::Instant,
) -> Result<BuildInfo, String> {
    let git = dist::git_facts(runner, repo)?;
    let hosted = hosted_info();
    let verified = bundles
        .iter()
        .any(|bundle| bundle.texture_lookup_delta.is_some());
    Ok(BuildInfo {
        format_version: BUILD_INFO_VERSION,
        target: platform.slug().to_owned(),
        arch: arch.slug().to_owned(),
        commit: git.commit,
        describe: git.describe,
        dirty: git.dirty,
        built_utc: util::format_unix_utc(util::now_unix()),
        duration_secs: started.elapsed().as_secs(),
        channel: toolchain::read(repo).map(|t| t.channel).unwrap_or_default(),
        toolchain: host_toolchain(runner),
        verified_in: verified.then(|| hosted.runner_image.clone()),
        builder: Builder::Hosted(hosted),
        linkage: None,
        cache: Vec::new(),
        bundles,
        xtask_version: env!("CARGO_PKG_VERSION").to_owned(),
    })
}

/// What the runner this ran on is, and where its log is.
///
/// The GitHub variables or nothing: run by hand there is no run to link to.
pub fn hosted_info() -> HostedInfo {
    let runner_image = util::env_var("ImageOS").map_or_else(
        || format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        |image| match util::env_var("ImageVersion") {
            Some(version) => format!("{image} {version}"),
            None => image,
        },
    );
    let run_id = util::env_var("GITHUB_RUN_ID");
    let run_url = match (
        util::env_var("GITHUB_SERVER_URL"),
        util::env_var("GITHUB_REPOSITORY"),
        run_id.as_ref(),
    ) {
        (Some(server), Some(repo), Some(id)) => Some(format!("{server}/{repo}/actions/runs/{id}")),
        _ => None,
    };
    HostedInfo {
        runner_image,
        run_id,
        run_url,
    }
}

/// `rustc -vV` and `cargo -V` as this host reports them, in the shape the
/// builder guests write into the same field.
fn host_toolchain(runner: &dyn Runner) -> String {
    let ask = |program: &str, arg: &str| {
        runner
            .capture(&Cmd::new(program).arg(arg))
            .ok()
            .filter(crate::runner::CommandOutput::success)
            .map(|out| out.stdout.trim().to_owned())
            .unwrap_or_default()
    };
    format!("{}\n{}", ask("rustc", "-vV"), ask("cargo", "-V"))
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::target::Target;

    fn sources<'a>(repo: &'a Path, exe: &'a Path, textures: &'a Path) -> Sources<'a> {
        Sources {
            repo,
            exe,
            textures,
        }
    }

    /// A fabricated repository with everything a bundle names in it, so the
    /// layout can be assembled and archived without the real assets.
    fn fabricate(dir: &Path) -> PathBuf {
        let repo = dir.join("repo");
        let _ = std::fs::remove_dir_all(&repo);
        let write = |relative: &str, body: &[u8]| {
            let path = repo.join(relative);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
            std::fs::write(&path, body).expect("write");
        };
        write(LICENSE, b"GNU GENERAL PUBLIC LICENSE\n");
        write(bake_licenses::NOTICES_PATH, b"# Third-party licenses\n");
        write("assets/linux/install-user.sh", b"#!/usr/bin/env bash\n");
        write("assets/linux/sunlit-earth.desktop", b"[Desktop Entry]\n");
        write(
            &format!("assets/icon/{}.svg", bake_icon::ICON_NAME),
            b"<svg/>",
        );
        for size in bake_icon::HICOLOR_SIZES {
            let relative = bake_icon::hicolor_path(size);
            write(
                &format!(
                    "{}/{}",
                    bake_icon::BAKED_DIR,
                    relative.to_string_lossy().replace('\\', "/")
                ),
                b"PNG",
            );
        }
        write(INFO_PLIST, real_plist().as_bytes());
        write(
            &format!("{}/{}", bake_icon::BAKED_DIR, bake_icon::ICNS_FILE),
            b"icns",
        );
        for name in TEXTURE_FILES {
            // Deliberately compressible bytes, so a writer that deflated a JXL
            // rather than storing it would be visible in the size.
            write(&format!("textures/{name}"), &vec![b'j'; 4096]);
        }
        write("bin/sunlit-earth", &vec![b'x'; 8192]);
        write("bin/sunlit-earth.exe", &vec![b'x'; 8192]);
        repo
    }

    /// The committed template, so a fabricated repository's plist is the one
    /// the release really uses rather than a stand-in that could drift from it.
    fn real_plist() -> String {
        std::fs::read_to_string(crate::store::repo_root().join(INFO_PLIST))
            .expect("the committed Info.plist template")
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sunlit_xtask_bundle_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    /// Decision 29: named for the package version, not for `git describe`, and
    /// the archive is each platform's own format.
    #[test]
    fn a_bundle_is_named_for_the_version_and_takes_its_targets_format() {
        assert_eq!(
            bundle_name("0.1.0", Platform::Windows, Arch::X86_64),
            "sunlit-earth-0.1.0-windows-x86_64"
        );
        assert_eq!(
            archive_name("0.1.0", Platform::Windows, Arch::X86_64),
            "sunlit-earth-0.1.0-windows-x86_64.zip"
        );
        assert_eq!(
            archive_name("0.1.0", Platform::Linux, Arch::X86_64),
            "sunlit-earth-0.1.0-linux-x86_64.tar.gz"
        );
        assert_eq!(
            archive_name("0.1.0-beta.1", Platform::Windows, Arch::X86_64),
            "sunlit-earth-0.1.0-beta.1-windows-x86_64.zip"
        );
        assert_eq!(Format::of(Platform::Windows), Format::Zip);
        assert_eq!(
            archive_name("0.1.0", Platform::Linux, Arch::Aarch64),
            "sunlit-earth-0.1.0-linux-aarch64.tar.gz"
        );
        assert_eq!(Format::of(Platform::Windows), Format::Zip);
        assert_eq!(Format::of(Platform::Linux), Format::TarGz);
        // The archive unpacks to one directory of the bundle's own name, and
        // no two builds of one release share an archive name.
        let mut names = std::collections::HashSet::new();
        for platform in Platform::ALL {
            for arch in Arch::ALL {
                let name = bundle_name("9.9.9", platform, arch);
                assert!(
                    archive_name("9.9.9", platform, arch).starts_with(&name),
                    "{platform} {arch}"
                );
                assert!(names.insert(archive_name("9.9.9", platform, arch)));
                if platform.packages().contains(&Package::App) {
                    assert!(names.insert(app_archive_name("9.9.9", arch)));
                }
            }
        }
    }

    /// The version comes out of the manifest the build reads, not out of the
    /// xtask binary, which may have been compiled from another tree.
    #[test]
    fn the_version_is_read_from_the_workspace_manifest() {
        let manifest = "[workspace]\nmembers = [\"crates/*\"]\n\
                        [workspace.package]\nversion = \"1.2.3\"\nedition = \"2024\"\n";
        assert_eq!(parse_version(manifest).as_deref(), Ok("1.2.3"));

        let err = parse_version("[workspace]\nmembers = []\n").unwrap_err();
        assert!(err.contains("[workspace.package] version"), "{err}");
        assert!(parse_version("this is not toml =").is_err());

        // And the real one, which is what a live run reads.
        let real = version(&crate::store::repo_root()).expect("the workspace version");
        let core = real.split(['-', '+']).next().unwrap_or_default();
        assert!(
            core.split('.').count() == 3
                && core
                    .split('.')
                    .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit())),
            "{real}"
        );
    }

    /// Decision 30, both halves: what every bundle carries, and the one thing
    /// only the Linux bundle does.
    #[test]
    fn every_bundle_carries_the_textures_the_licence_and_the_third_party_notices() {
        let repo = PathBuf::from("/repo");
        let exe = PathBuf::from("/out/sunlit-earth");
        let textures = repo.join("textures");
        for platform in Platform::ALL {
            let items = layout(platform, &sources(&repo, &exe, &textures));
            let paths: Vec<&str> = items.iter().map(|i| i.path.as_str()).collect();
            assert!(paths.contains(&exe_name(platform)), "{platform}: {paths:?}");
            for name in TEXTURE_FILES {
                assert!(
                    paths.contains(&format!("textures/{name}").as_str()),
                    "{platform}: {paths:?}"
                );
            }
            assert!(paths.contains(&LICENSE), "{platform}: {paths:?}");
            assert!(
                paths.contains(&bake_licenses::NOTICES_NAME),
                "{platform}: {paths:?}"
            );
            for absent in [
                "textures/PROVENANCE.md",
                RECORD,
                "ATTRIBUTION.md",
                "assets/ATTRIBUTION.md",
            ] {
                assert!(!paths.contains(&absent), "{platform} carries {absent}");
            }
            // The binary is the one thing a tar header has to call executable.
            let exe_item = items
                .iter()
                .find(|i| i.path == exe_name(platform))
                .expect("the binary");
            assert!(exe_item.executable, "{platform}");
        }

        // The install kit is Linux only: there is nothing to install on
        // Windows, because the icon is a resource inside the exe.
        let linux = layout(Platform::Linux, &sources(&repo, &exe, &textures));
        let linux_paths: Vec<&str> = linux.iter().map(|i| i.path.as_str()).collect();
        assert!(
            linux_paths.contains(&"assets/linux/install-user.sh"),
            "{linux_paths:?}"
        );
        assert!(
            linux_paths.contains(&"assets/linux/sunlit-earth.desktop"),
            "{linux_paths:?}"
        );
        assert!(
            linux_paths.contains(&"assets/icon/sunlit-earth.svg"),
            "{linux_paths:?}"
        );
        assert_eq!(
            linux_paths
                .iter()
                .filter(|p| p.starts_with("assets/icon/baked/hicolor/"))
                .count(),
            bake_icon::HICOLOR_SIZES.len()
        );
        for bare in [Platform::Windows, Platform::MacOs] {
            let items = layout(bare, &sources(&repo, &exe, &textures));
            assert!(
                !items.iter().any(|i| i.path.starts_with("assets/")),
                "a {bare} bundle has nothing to install"
            );
        }

        // `install-user.sh` finds the entry and the icons by walking up from
        // itself, so the bundle has to keep the layout the repository gives
        // them rather than flattening it.
        assert!(
            linux_paths.contains(&"assets/linux/install-user.sh")
                && linux_paths.contains(&"assets/icon/sunlit-earth.svg"),
            "the install script's own relative paths are the layout"
        );
        // And it has to be runnable without a chmod, like the binary.
        assert!(
            linux
                .iter()
                .find(|i| i.path == "assets/linux/install-user.sh")
                .is_some_and(|i| i.executable)
        );
    }

    /// Every path inside a bundle uses forward slashes and stays inside it,
    /// because both archive formats are written from those strings.
    #[test]
    fn no_bundle_path_escapes_the_bundle_or_carries_a_backslash() {
        let repo = PathBuf::from(r"C:\repo");
        let exe = PathBuf::from(r"C:\out\sunlit-earth.exe");
        let textures = repo.join("textures");
        for platform in Platform::ALL {
            for item in layout(platform, &sources(&repo, &exe, &textures)) {
                assert!(!item.path.contains('\\'), "{}", item.path);
                assert!(!item.path.starts_with('/'), "{}", item.path);
                assert!(!item.path.contains(".."), "{}", item.path);
            }
        }
    }

    /// Decision 31's read-back, over a fabricated tree: what the archive holds
    /// is what the directory holds, and the tarball says the binary is 0755.
    #[test]
    fn each_archive_holds_exactly_what_was_assembled() {
        let dir = scratch("archives");
        let repo = fabricate(&dir);
        for platform in Platform::ALL {
            let exe = repo.join("bin").join(exe_name(platform));
            let items = layout(platform, &sources(&repo, &exe, &repo.join("textures")));
            let name = bundle_name("0.1.0", platform, Arch::X86_64);
            let root = assemble(&dir, &name, &items).expect("assembled");

            let assembled = walk(&root).expect("walked");
            assert_eq!(assembled.len(), items.len(), "{platform}");

            let format = Format::of(platform);
            let archive = dir.join(archive_name("0.1.0", platform, Arch::X86_64));
            let bytes = write(format, &root, &name, &items, &archive).expect("written");
            assert!(bytes > 0, "{platform}");

            let archived = read_back(format, &archive).expect("read back");
            verify(&name, &assembled, &archived).expect("the archive matches the directory");

            // Every entry is under the one top-level directory, so an unpack
            // anywhere produces one folder rather than a scattering.
            for entry in &archived {
                assert!(
                    entry.path.starts_with(&format!("{name}/")),
                    "{}",
                    entry.path
                );
            }

            if format == Format::TarGz {
                let binary = archived
                    .iter()
                    .find(|e| e.path.ends_with("/sunlit-earth"))
                    .expect("the binary");
                assert_eq!(binary.mode, Some(0o755), "the binary needs no chmod");
                let licence = archived
                    .iter()
                    .find(|e| e.path.ends_with(&format!("/{LICENSE}")))
                    .expect("the licence");
                assert_eq!(licence.mode, Some(0o644));
            }
            if platform == Platform::Linux {
                let script = archived
                    .iter()
                    .find(|e| e.path.ends_with("/install-user.sh"))
                    .expect("the install script");
                assert_eq!(script.mode, Some(0o755));
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The comparison has to fail when the archive and the directory disagree,
    /// or it is a check that always passes.
    #[test]
    fn a_truncated_or_missing_entry_fails_the_read_back() {
        let assembled = vec![("sunlit-earth".to_owned(), 10), ("LICENSE".to_owned(), 4)];
        let good = vec![
            Archived {
                path: "b/LICENSE".to_owned(),
                bytes: 4,
                mode: None,
            },
            Archived {
                path: "b/sunlit-earth".to_owned(),
                bytes: 10,
                mode: None,
            },
        ];
        // The comparison is order-independent only insofar as both sides are
        // sorted, which `walk` and `read_back` both do.
        let mut sorted = assembled.clone();
        sorted.sort();
        assert_eq!(verify("b", &sorted, &good), Ok(()));

        let missing = vec![good[0].clone()];
        let err = verify("b", &sorted, &missing).unwrap_err();
        assert!(err.contains("b/sunlit-earth"), "{err}");

        let mut truncated = good.clone();
        truncated[1].bytes = 3;
        let err = verify("b", &sorted, &truncated).unwrap_err();
        assert!(err.contains("3 bytes in the archive"), "{err}");

        let mut extra = good.clone();
        extra.push(Archived {
            path: "b/stowaway".to_owned(),
            bytes: 1,
            mode: None,
        });
        let err = verify("b", &sorted, &extra).unwrap_err();
        assert!(err.contains("b/stowaway"), "{err}");
    }

    /// JXL is already compressed, so a zip that deflated it would spend time to
    /// make it slightly larger. Everything else is deflated.
    #[test]
    fn the_already_compressed_entries_are_stored_and_the_rest_deflated() {
        assert!(stored("textures/world.topo.200405.jxl"));
        assert!(!stored("sunlit-earth.exe"));
        assert!(!stored("LICENSE"));
        assert!(!stored(
            "assets/icon/baked/hicolor/32x32/apps/sunlit-earth.png"
        ));

        let dir = scratch("methods");
        let repo = fabricate(&dir);
        let exe = repo.join("bin").join("sunlit-earth.exe");
        let items = layout(
            Platform::Windows,
            &sources(&repo, &exe, &repo.join("textures")),
        );
        let name = bundle_name("0.1.0", Platform::Windows, Arch::X86_64);
        let root = assemble(&dir, &name, &items).expect("assembled");
        let archive = dir.join(archive_name("0.1.0", Platform::Windows, Arch::X86_64));
        write(Format::Zip, &root, &name, &items, &archive).expect("written");

        let file = std::fs::File::open(&archive).expect("open");
        let mut zip = zip::ZipArchive::new(file).expect("a zip");
        let mut checked = 0;
        for index in 0..zip.len() {
            let entry = zip.by_index(index).expect("an entry");
            let expected = if stored(entry.name()) {
                zip::CompressionMethod::Stored
            } else {
                zip::CompressionMethod::Deflated
            };
            assert_eq!(entry.compression(), expected, "{}", entry.name());
            checked += 1;
        }
        assert_eq!(checked, items.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The closing line names the archive and what is in it, because the point
    /// of a bundle is that a reader knows it is not just a binary. A bundle
    /// nothing ran says that on the same line, because `--no-verify` writes an
    /// archive that is in every other way the one a full run writes.
    #[test]
    fn the_closing_line_names_the_archive_and_whether_anything_ran_it() {
        let path = Path::new("/t/dist/linux/sunlit-earth-0.1.0-linux-x86_64.tar.gz");
        let verified = summary(path, true);
        assert!(
            verified.contains("sunlit-earth-0.1.0-linux-x86_64.tar.gz"),
            "{verified}"
        );
        assert!(verified.contains("textures"), "{verified}");
        assert!(!verified.contains("--no-verify"), "{verified}");

        let unverified = summary(path, false);
        assert!(unverified.starts_with(&verified), "{unverified}");
        assert!(unverified.contains("--no-verify"), "{unverified}");
        assert!(unverified.contains("verified_in"), "{unverified}");
    }

    /// The one line a run without the assets prints instead of a bundle.
    #[test]
    fn the_skip_says_what_is_missing_and_what_was_produced_anyway() {
        let text = skipped_note();
        assert!(text.contains("git lfs pull"), "{text}");
        assert!(text.contains("grid"), "{text}");
        assert!(text.contains("loose binary"), "{text}");
    }

    /// The bundle's binary is the one the dist directory holds, under the same
    /// name: two spellings of it would be two things to keep in step.
    #[test]
    fn the_bundles_binary_is_the_one_dist_names() {
        for target in Target::ALL {
            assert_eq!(
                exe_name(Platform::from(target)),
                crate::commands::dist::exe_name(target)
            );
        }
    }

    /// A bundle has three platforms and a guest two, and the conversion runs
    /// one way only, which is what keeps `dist.rs` exhaustive without a macOS
    /// arm that could never be reached.
    #[test]
    fn a_platform_names_itself_and_a_guest_target_becomes_one() {
        assert_eq!(Platform::Windows.slug(), "windows");
        assert_eq!(Platform::Linux.slug(), "linux");
        assert_eq!(Platform::MacOs.slug(), "macos");
        assert_eq!(Platform::MacOs.to_string(), "macos");
        for platform in Platform::ALL {
            assert!(!platform.slug().is_empty());
        }
        for target in Target::ALL {
            assert_eq!(Platform::from(target).slug(), target.slug());
        }
        // And the host is one of them, on every platform this tree builds for.
        assert!(Platform::host().is_some());
    }

    /// The macOS archive a tester unpacks in Terminal is a tarball, because
    /// `tar` sets no quarantine attribute where a browser does.
    #[test]
    fn the_macos_bundle_is_a_tarball_named_like_the_others() {
        assert_eq!(Format::of(Platform::MacOs), Format::TarGz);
        assert_eq!(
            archive_name("0.1.0-beta.4", Platform::MacOs, Arch::X86_64),
            "sunlit-earth-0.1.0-beta.4-macos-x86_64.tar.gz"
        );
        assert_eq!(exe_name(Platform::MacOs), "sunlit-earth");
    }

    /// What `--verify` reads: the archive unpacks to the one directory the
    /// bundle is, with every file in it, and the tarball's mode field survives
    /// the round trip on the platform that has one.
    #[test]
    fn an_archive_unpacks_to_the_directory_it_was_made_from() {
        let dir = scratch("unpack");
        let repo = fabricate(&dir);
        for platform in Platform::ALL {
            let exe = repo.join("bin").join(exe_name(platform));
            let items = layout(platform, &sources(&repo, &exe, &repo.join("textures")));
            let name = bundle_name("0.1.0", platform, Arch::X86_64);
            let root = assemble(&dir, &name, &items).expect("assembled");
            let format = Format::of(platform);
            let archive = dir.join(archive_name("0.1.0", platform, Arch::X86_64));
            write(format, &root, &name, &items, &archive).expect("written");

            let into = dir.join(format!("unpacked-{platform}"));
            unpack(format, &archive, &into).expect("unpacked");
            let unpacked = into.join(&name);
            assert!(unpacked.is_dir(), "{platform}: {}", unpacked.display());
            assert_eq!(
                walk(&unpacked).expect("walked"),
                walk(&root).expect("walked")
            );

            #[cfg(unix)]
            if format == Format::TarGz {
                use std::os::unix::fs::PermissionsExt as _;
                let mode = std::fs::metadata(unpacked.join(exe_name(platform)))
                    .expect("the unpacked binary")
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o777, 0o755, "{platform}: it would not run");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The record names the runner, and the run where a workflow drove it.
    #[test]
    fn a_hosted_record_never_links_to_a_run_it_does_not_have() {
        let hosted = hosted_info();
        assert!(!hosted.runner_image.is_empty());
        assert!(
            hosted.run_url.is_none() || hosted.run_id.is_some(),
            "{hosted:?}"
        );
    }

    /// The layout Apple's bundle format fixes: the executable at
    /// `Contents/MacOS/`, everything it reads at `Contents/Resources/`, and the
    /// plist that names them both at `Contents/`. The one that matters beyond
    /// tidiness is `Contents/Resources/textures`, which is the candidate
    /// `resolve_textures_dir` gained.
    #[test]
    fn the_app_bundle_puts_each_file_where_macos_looks_for_it() {
        let dir = scratch("app_layout");
        let repo = fabricate(&dir);
        let exe = repo.join("bin").join("sunlit-earth");
        let items = app_layout(&sources(&repo, &exe, &repo.join("textures")), "1.2.3")
            .expect("the app layout");
        let paths: Vec<&str> = items.iter().map(|i| i.path.as_str()).collect();

        assert!(paths.contains(&"Contents/Info.plist"), "{paths:?}");
        assert!(paths.contains(&"Contents/MacOS/sunlit-earth"), "{paths:?}");
        assert!(
            paths.contains(&"Contents/Resources/sunlit-earth.icns"),
            "{paths:?}"
        );
        for name in TEXTURE_FILES {
            assert!(
                paths.contains(&format!("Contents/Resources/textures/{name}").as_str()),
                "{paths:?}"
            );
        }
        assert!(paths.contains(&"Contents/Resources/LICENSE"), "{paths:?}");
        assert!(
            paths.contains(&format!("Contents/Resources/{}", bake_licenses::NOTICES_NAME).as_str()),
            "{paths:?}"
        );

        // Nothing at the top of the bundle but `Contents`, which is what makes
        // it a bundle rather than a folder macOS opens.
        for item in &items {
            assert!(item.path.starts_with("Contents/"), "{}", item.path);
            assert!(!item.path.contains(".."), "{}", item.path);
        }
        // The binary is the one thing that has to stay executable.
        assert!(
            items
                .iter()
                .find(|i| i.path == "Contents/MacOS/sunlit-earth")
                .is_some_and(|i| i.executable)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `CFBundleShortVersionString` is what a user is shown and what an updater
    /// would compare, so a bundle whose plist still said `@VERSION@` would be a
    /// release nobody could name.
    #[test]
    fn the_plist_carries_the_version_and_the_identity_that_never_changes() {
        let dir = scratch("app_plist");
        let repo = fabricate(&dir);
        let exe = repo.join("bin").join("sunlit-earth");
        let items = app_layout(
            &sources(&repo, &exe, &repo.join("textures")),
            "0.1.0-beta.4",
        )
        .expect("the app layout");
        let plist = items
            .iter()
            .find(|i| i.path == "Contents/Info.plist")
            .expect("the plist");
        let Source::Made(bytes) = &plist.source else {
            panic!(
                "the plist is made, not copied: its version is not known until the manifest is read"
            );
        };
        let text = String::from_utf8(bytes.clone()).expect("the plist is text");
        assert!(text.contains("0.1.0-beta.4"), "{text}");
        assert!(!text.contains(VERSION_PLACEHOLDER), "{text}");

        // The real template, not the fabricated one: the identifier can never
        // change once chosen, because preferences and TCC grants key on it.
        let real = std::fs::read_to_string(crate::store::repo_root().join(INFO_PLIST))
            .expect("the committed template");
        for value in [
            "earth.sunlit.SunlitEarth",
            "sunlit-earth",
            "Sunlit Earth",
            "11.0",
            "NSHighResolutionCapable",
            VERSION_PLACEHOLDER,
        ] {
            assert!(real.contains(value), "the template has no {value}");
        }
        // `LSUIElement` would do nothing: winit sets the regular activation
        // policy at startup whatever the plist says.
        assert!(!real.contains("<key>LSUIElement</key>"), "{real}");

        // A template with the placeholder taken out is a refusal, not a bundle
        // named after nothing.
        std::fs::write(repo.join(INFO_PLIST), "<plist/>").expect("write");
        let err = app_layout(&sources(&repo, &exe, &repo.join("textures")), "1.0.0").unwrap_err();
        assert!(err.contains(VERSION_PLACEHOLDER), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// macOS ships two archives of one binary and the other platforms one, and
    /// the two macOS names have to differ or the second would overwrite the
    /// first in the release.
    #[test]
    fn macos_ships_two_archives_and_everyone_else_one() {
        assert_eq!(Platform::Windows.packages(), &[Package::Plain]);
        assert_eq!(Platform::Linux.packages(), &[Package::Plain]);
        assert_eq!(Platform::MacOs.packages(), &[Package::Plain, Package::App]);

        let tarball = archive_name("0.1.0", Platform::MacOs, Arch::X86_64);
        let app = app_archive_name("0.1.0", Arch::X86_64);
        assert_eq!(tarball, "sunlit-earth-0.1.0-macos-x86_64.tar.gz");
        assert_eq!(app, "sunlit-earth-0.1.0-macos-x86_64.zip");
        assert_ne!(tarball, app);
        // The `.app` is a zip whatever the platform's plain format is, because
        // `ditto` writes one.
        assert_eq!(archive_format(Platform::MacOs, Package::App), Format::Zip);
        assert_eq!(
            archive_format(Platform::MacOs, Package::Plain),
            Format::TarGz
        );
    }

    fn pe_header(machine: u16) -> Vec<u8> {
        let mut bytes = vec![0; 0x90];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        bytes[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
        bytes
    }

    fn elf_header(machine: u16) -> Vec<u8> {
        let mut bytes = vec![0; 64];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[18..20].copy_from_slice(&machine.to_le_bytes());
        bytes
    }

    fn macho_header(cpu: u32) -> Vec<u8> {
        let mut bytes = vec![0; 32];
        bytes[..4].copy_from_slice(&0xFEED_FACFu32.to_le_bytes());
        bytes[4..8].copy_from_slice(&cpu.to_le_bytes());
        bytes
    }

    #[test]
    fn the_platform_and_architecture_come_out_of_the_binarys_header() {
        let cases = [
            (pe_header(0x8664), Platform::Windows, Arch::X86_64),
            (pe_header(0xAA64), Platform::Windows, Arch::Aarch64),
            (elf_header(62), Platform::Linux, Arch::X86_64),
            (elf_header(183), Platform::Linux, Arch::Aarch64),
            (macho_header(0x0100_0007), Platform::MacOs, Arch::X86_64),
            (macho_header(0x0100_000C), Platform::MacOs, Arch::Aarch64),
        ];
        for (bytes, platform, arch) in cases {
            assert_eq!(Arch::of_binary(&bytes), Ok((platform, arch)));
        }
    }

    #[test]
    fn a_binary_that_is_not_one_thin_known_architecture_is_refused() {
        let universal = 0xCAFE_BABEu32.to_be_bytes();
        assert!(
            Arch::of_binary(&universal)
                .unwrap_err()
                .contains("universal")
        );
        assert!(Arch::of_binary(&pe_header(0x014C)).is_err());
        assert!(Arch::of_binary(&elf_header(40)).is_err());
        assert!(Arch::of_binary(b"#!/bin/sh\n").is_err());
        assert!(Arch::of_binary(b"MZ").is_err());
        assert!(Arch::of_binary(&[]).is_err());
    }

    /// The xtask binary running this test is a real executable of this host's
    /// own platform and architecture.
    #[test]
    fn the_running_binary_reads_as_this_host() {
        let exe = std::env::current_exe().expect("the test binary");
        let (platform, arch) = Arch::of(&exe).expect("a readable header");
        assert_eq!(Some(platform), Platform::host());
        assert_eq!(arch.slug(), std::env::consts::ARCH);
    }

    /// `ditto` stores extended attributes beside the files they belong to, and
    /// no comparison against an assembled directory can account for them. What
    /// must not happen is the filter swallowing a real file.
    #[test]
    fn only_the_archivers_own_metadata_is_dropped_from_the_read_back() {
        assert!(is_apple_metadata(
            "__MACOSX/Sunlit Earth.app/Contents/._Info.plist"
        ));
        assert!(is_apple_metadata("Sunlit Earth.app/Contents/._Info.plist"));
        assert!(!is_apple_metadata("Sunlit Earth.app/Contents/Info.plist"));
        assert!(!is_apple_metadata(
            "Sunlit Earth.app/Contents/MacOS/sunlit-earth"
        ));
        assert!(!is_apple_metadata("sunlit-earth-0.1.0-macos/sunlit-earth"));
        // A file whose own name merely starts with an underscore is not one.
        assert!(!is_apple_metadata(
            "Sunlit Earth.app/Contents/_CodeSignature/CodeResources"
        ));
    }
}
