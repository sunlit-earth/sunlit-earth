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
/// Not [`Target`], which names a guest this host can boot: there is no macOS
/// guest and never will be one, and every `match` in `dist.rs` is exhaustive
/// over the two that exist. What a bundle needs is a layout and an archive
/// format, and those have a third case, so the third case lives here and
/// [`From<Target>`] carries a guest into it.
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
    /// All three, in the order reports list them.
    ///
    /// Only the tests walk the set today: production code is handed one
    /// platform by a command line or by a [`Target`], and clap derives its own
    /// list of the variants for the flag.
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

/// The package the bundle is named after, which is also the binary's stem.
pub const PACKAGE: &str = "sunlit-earth";

/// The licence the workspace declares, as a file at the repository root.
pub const LICENSE: &str = "LICENSE";

/// The record, written beside the archive and not inside it.
pub const RECORD: &str = "build-info.json";

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
            // A tarball on macOS, and deliberately not a zip: decision 16 makes
            // it the archive a tester unpacks in Terminal, where `tar` sets no
            // quarantine attribute and the binary starts without a Gatekeeper
            // dialog. The `.app` beside it is the zip, and `ditto` writes it.
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

/// One file in the bundle: where it sits, and what it is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Its path inside the bundle's top-level directory, with forward slashes.
    /// Both archive formats want them that way, and it is what the read-back
    /// comparison is made in.
    pub path: String,
    /// Where its bytes come from on the host: the binary this run built, or
    /// something the repository ships.
    pub source: PathBuf,
    /// Whether the tarball's header says 0755. Without it the first thing a
    /// Linux user does is `chmod +x`.
    pub executable: bool,
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
/// in the record.
pub fn bundle_name(version: &str, platform: Platform) -> String {
    format!("{PACKAGE}-{version}-{}", platform.slug())
}

/// The archive's file name, which is the bundle's name plus its format.
pub fn archive_name(version: &str, platform: Platform) -> String {
    format!(
        "{}.{}",
        bundle_name(version, platform),
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
    let mut items = vec![Item {
        path: exe_name(platform).to_owned(),
        source: sources.exe.to_path_buf(),
        executable: true,
    }];

    for name in TEXTURE_FILES {
        items.push(Item {
            path: format!("textures/{name}"),
            source: sources.textures.join(name),
            executable: false,
        });
    }

    items.push(Item {
        path: LICENSE.to_owned(),
        source: sources.repo.join(LICENSE),
        executable: false,
    });
    items.push(Item {
        path: bake_licenses::NOTICES_NAME.to_owned(),
        source: sources.repo.join(bake_licenses::NOTICES_PATH),
        executable: false,
    });

    if platform == Platform::Linux {
        let linux = sources.repo.join("assets").join("linux");
        let icon = sources.repo.join("assets").join("icon");
        items.push(Item {
            path: "assets/linux/install-user.sh".to_owned(),
            source: linux.join("install-user.sh"),
            executable: true,
        });
        items.push(Item {
            path: "assets/linux/sunlit-earth.desktop".to_owned(),
            source: linux.join("sunlit-earth.desktop"),
            executable: false,
        });
        items.push(Item {
            path: format!("assets/icon/{}.svg", bake_icon::ICON_NAME),
            source: icon.join(format!("{}.svg", bake_icon::ICON_NAME)),
            executable: false,
        });
        // Named through the bake's own list rather than by walking the baked
        // directory, so a bundle carries exactly what the bake writes and a
        // size that appeared on one side and not the other fails a test here.
        let baked = sources.repo.join(bake_icon::BAKED_DIR);
        for size in bake_icon::HICOLOR_SIZES {
            let relative = bake_icon::hicolor_path(size);
            items.push(Item {
                path: format!(
                    "assets/icon/baked/{}",
                    relative.to_string_lossy().replace('\\', "/")
                ),
                source: baked.join(relative),
                executable: false,
            });
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
        std::fs::copy(&item.source, &target).map_err(|e| {
            format!(
                "the bundle needs {} and cannot copy it: {e}",
                item.source.display()
            )
        })?;
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
    let mut text = format!(
        "  and {}, which holds the binary with its textures beside it, the record and the licence",
        archive.display()
    );
    if !verified {
        text.push_str(
            "\n  nothing has run this bundle: --no-verify skipped the boot that renders \
             from it, so nothing has shown that its textures are found where it puts them, \
             and its record says so with a null verified_in",
        );
    }
    text
}

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
/// The command's own arguments, declared beside it rather than in `main.rs`:
/// the flag names and the doc comments that become their help are what this
/// module means, and `Platform` already carries a clap derive for the same
/// reason.
#[derive(Debug, Clone, clap::Args)]
pub struct Options {
    /// Which platform's layout and archive format.
    #[arg(long)]
    pub platform: Platform,
    /// The release binary to put in it. Not built here.
    #[arg(long, value_name = "PATH")]
    pub exe: PathBuf,
    /// Where the archive and its record go. The dist directory by default, so
    /// a bundle written by hand lands where `dist` puts one.
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
    /// Unpack the archive and render from it twice, which is what proves the
    /// binary finds the textures beside it. This host's own platform only.
    #[arg(long)]
    pub verify: bool,
}

/// Where the bundle is assembled before it is archived, under the output
/// directory and removed when the run ends.
const STAGE_DIR: &str = ".stage";

/// Where the archive is unpacked to be rendered from.
const VERIFY_DIR: &str = ".verify";

/// Assemble, archive, read back, optionally render from it, and write the
/// record beside it.
///
/// The verification is the same two renders `dist` runs in a desktop guest and
/// for the same reason: a binary that did not find its textures still writes a
/// 640x360 PNG, so only the comparison against a render made against an empty
/// directory can tell a globe from a grid. What differs is where it runs. A
/// runner is not a pristine guest, and the record says which of the two it was
/// rather than leaving a reader to assume the stronger one.
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
    // Not the skip `dist` falls back to: that run has a loose binary to publish
    // instead, and this command has nothing else to produce.
    let textures = artifacts::textures_present(&repo).map_err(|why| {
        format!(
            "{why}\n`git lfs pull` fetches the texture assets; without them a bundle \
             would render the procedural grid under a name that promises a release."
        )
    })?;

    // Absolute before anything is written into it, because `--verify` runs the
    // binary from a working directory of its own choosing: a relative `--out`
    // would reach the child as a path relative to that instead, and every one
    // of the paths derived from it here is handed to the child.
    let out = std::path::absolute(
        options
            .out
            .clone()
            .unwrap_or_else(|| dist::dist_dir(platform)),
    )
    .map_err(|e| format!("cannot resolve the output directory: {e}"))?;
    std::fs::create_dir_all(&out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;

    let name = bundle_name(&version, platform);
    let items = layout(
        platform,
        &Sources {
            repo: &repo,
            exe: &options.exe,
            textures: &textures,
        },
    );
    let stage = out.join(STAGE_DIR);
    let root = assemble(&stage, &name, &items)?;
    println!("bundle: {name}/, {}", util::count(items.len(), "file"));

    let format = Format::of(platform);
    let archive = out.join(archive_name(&version, platform));
    let bytes = write(format, &root, &name, &items, &archive)?;
    let assembled = walk(&root)?;
    let archived = read_back(format, &archive)?;
    verify(&name, &assembled, &archived)?;
    println!(
        "  {} ({}), {} read back and matched",
        archive_name(&version, platform),
        util::format_bytes(bytes),
        util::count(archived.len(), "file")
    );

    let outcome = verify_here(
        runner,
        &out,
        &archive,
        format,
        &name,
        platform,
        options.verify,
    );
    let delta = match outcome {
        Ok(delta) => delta,
        Err(why) => {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(why);
        }
    };

    let record = record(
        runner,
        &repo,
        platform,
        BundleInfo {
            name: name.clone(),
            archive: archive_name(&version, platform),
            entries: items.len(),
            texture_lookup_delta: delta,
        },
        started,
    )?;
    std::fs::write(out.join(RECORD), record.to_json())
        .map_err(|e| format!("cannot write {}: {e}", out.join(RECORD).display()))?;
    let _ = std::fs::remove_dir_all(&stage);

    println!();
    println!("{platform}: {}", archive.display());
    println!("{}", summary(&archive, delta.is_some()));
    Ok(0)
}

/// Unpack an archive into a directory, with the crate that wrote it.
///
/// The tarball's mode field is restored here, and that is what makes the render
/// below possible at all on the two platforms that have one: an archive that
/// carried 0644 on the binary would fail with a permission error. Rendering
/// from the unpacked archive rather than from the directory it was assembled in
/// is the point, because the archive is what a user is handed.
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
/// `None` when nothing ran it, which is the same answer the record carries and
/// the reason this takes the flag rather than being called behind one: whether
/// a bundle was run is a property of the bundle, and the one function that can
/// say so is the one that would have run it.
fn verify_here(
    runner: &dyn Runner,
    out: &Path,
    archive: &Path,
    format: Format,
    name: &str,
    platform: Platform,
    asked: bool,
) -> Result<Option<f64>, String> {
    if !asked {
        println!("  nothing has run this bundle: --verify was not given");
        return Ok(None);
    }
    if Platform::host() != Some(platform) {
        return Err(format!(
            "--verify runs the bundle, and this host is not {platform}: a {platform} \
             bundle is verified on a {platform} machine, which for the release \
             pipeline is the runner that built it"
        ));
    }
    let work = out.join(VERIFY_DIR);
    let _ = std::fs::remove_dir_all(&work);
    unpack(format, archive, &work)?;
    let root = work.join(name);
    if !root.is_dir() {
        return Err(format!(
            "the archive unpacked to something other than {}, so it does not hold the \
             one directory a bundle is",
            root.display()
        ));
    }

    // The empty directory is what makes the grid render a grid: the loader takes
    // the variable's directory when it is one and finds no files in it.
    let empty = work.join(dist::EMPTY_TEXTURES);
    std::fs::create_dir_all(&empty)
        .map_err(|e| format!("cannot create {}: {e}", empty.display()))?;

    let exe = root.join(exe_name(platform));
    let at = |cmd: Cmd| -> Result<(), String> {
        let shown = cmd.display();
        let output = runner
            .capture(&cmd)
            .map_err(|e| format!("cannot run the bundled binary: {e}"))?;
        if output.success() {
            Ok(())
        } else {
            Err(format!(
                "`{shown}` failed: {} {}",
                output.stderr.trim(),
                output.stdout.trim()
            ))
        }
    };

    // The working directory is outside the bundle on purpose, and it is
    // load-bearing rather than tidiness: `resolve_textures_dir` tries a
    // `textures` beside the working directory before it walks up from the
    // executable, so a run from inside the bundle would answer with the first
    // branch and leave the walk-up, which is what the layout depends on,
    // untested.
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
    std::fs::copy(&smoke, out.join(SMOKE_FILE))
        .map_err(|e| format!("cannot keep the bundle's own render: {e}"))?;
    let _ = std::fs::remove_dir_all(&work);
    println!("  verified here: the two renders differ by {delta:.2} of a channel step");
    Ok(Some(delta))
}

/// The record written beside the archive.
fn record(
    runner: &dyn Runner,
    repo: &Path,
    platform: Platform,
    bundle: BundleInfo,
    started: std::time::Instant,
) -> Result<BuildInfo, String> {
    let git = dist::git_facts(runner, repo)?;
    let hosted = hosted_info();
    let verified = bundle.texture_lookup_delta.is_some();
    Ok(BuildInfo {
        format_version: BUILD_INFO_VERSION,
        target: platform.slug().to_owned(),
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
        bundle: Some(bundle),
        xtask_version: env!("CARGO_PKG_VERSION").to_owned(),
    })
}

/// What the runner this ran on is, and where its log is.
///
/// The GitHub variables or nothing: a developer running this by hand gets their
/// own operating system and no run to link to, which is the honest answer
/// rather than a fabricated one.
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
        for name in TEXTURE_FILES {
            // Deliberately compressible bytes, so a writer that deflated a JXL
            // rather than storing it would be visible in the size.
            write(&format!("textures/{name}"), &vec![b'j'; 4096]);
        }
        write("bin/sunlit-earth", &vec![b'x'; 8192]);
        write("bin/sunlit-earth.exe", &vec![b'x'; 8192]);
        repo
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
            bundle_name("0.1.0", Platform::Windows),
            "sunlit-earth-0.1.0-windows"
        );
        assert_eq!(
            archive_name("0.1.0", Platform::Windows),
            "sunlit-earth-0.1.0-windows.zip"
        );
        assert_eq!(
            archive_name("0.1.0", Platform::Linux),
            "sunlit-earth-0.1.0-linux.tar.gz"
        );
        assert_eq!(
            archive_name("0.1.0-beta.1", Platform::Windows),
            "sunlit-earth-0.1.0-beta.1-windows.zip"
        );
        assert_eq!(Format::of(Platform::Windows), Format::Zip);
        assert_eq!(Format::of(Platform::Linux), Format::TarGz);
        // The archive unpacks to one directory of the bundle's own name.
        for platform in Platform::ALL {
            let name = bundle_name("9.9.9", platform);
            assert!(
                archive_name("9.9.9", platform).starts_with(&name),
                "{platform}"
            );
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
            let name = bundle_name("0.1.0", platform);
            let root = assemble(&dir, &name, &items).expect("assembled");

            let assembled = walk(&root).expect("walked");
            assert_eq!(assembled.len(), items.len(), "{platform}");

            let format = Format::of(platform);
            let archive = dir.join(archive_name("0.1.0", platform));
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
        let name = bundle_name("0.1.0", Platform::Windows);
        let root = assemble(&dir, &name, &items).expect("assembled");
        let archive = dir.join(archive_name("0.1.0", Platform::Windows));
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
        let path = Path::new("/t/dist/linux/sunlit-earth-0.1.0-linux.tar.gz");
        let verified = summary(path, true);
        assert!(
            verified.contains("sunlit-earth-0.1.0-linux.tar.gz"),
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

    /// Decision 11: a bundle has three platforms and a guest has two, and the
    /// conversion runs one way only. `Target` staying two variants is what
    /// keeps every `match` in `dist.rs` exhaustive without a macOS arm that
    /// could never be reached.
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

    /// Decision 16: the macOS archive a tester unpacks in Terminal is a
    /// tarball, because `tar` sets no quarantine attribute where a browser and
    /// Archive Utility do.
    #[test]
    fn the_macos_bundle_is_a_tarball_named_like_the_others() {
        assert_eq!(Format::of(Platform::MacOs), Format::TarGz);
        assert_eq!(
            archive_name("0.1.0-beta.4", Platform::MacOs),
            "sunlit-earth-0.1.0-beta.4-macos.tar.gz"
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
            let name = bundle_name("0.1.0", platform);
            let root = assemble(&dir, &name, &items).expect("assembled");
            let format = Format::of(platform);
            let archive = dir.join(archive_name("0.1.0", platform));
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

    /// The record a runner writes names the runner and, when a workflow drove
    /// it, the run somebody can open. Run outside one there is no run to link
    /// to, and an invented link would be worse than none.
    #[test]
    fn a_hosted_record_never_links_to_a_run_it_does_not_have() {
        let hosted = hosted_info();
        assert!(!hosted.runner_image.is_empty());
        assert!(
            hosted.run_url.is_none() || hosted.run_id.is_some(),
            "{hosted:?}"
        );
    }
}
