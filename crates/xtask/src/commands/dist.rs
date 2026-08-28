//! `cargo xtask dist`: a release binary built in a pristine guest, from the
//! committed tree, with a toolchain the repository pins.
//!
//! The point of the command is the environment it does not run in. A local
//! `cargo build --release` produces whatever this host's environment makes of
//! the tree: the toolchain rustup happens to default to, the LLVM on `PATH`, the
//! Visual Studio that is installed, `RUSTFLAGS` if any are set, and a `target/`
//! directory that has seen every branch this checkout was ever on. Nothing
//! records which of those a given binary came from. Here the answer is written
//! down beside the binary, and the only things that reach the build are the
//! source archive of `HEAD` and the pinned channel's name.
//!
//! Two claims about a release binary are checked on the artifact itself rather
//! than argued about: on Windows that it links the C runtime statically, so a
//! clean Windows 10 needs no redistributable, and on Linux that its glibc floor
//! is 2.35 and its `NEEDED` set is the four libraries the port actually uses.
//! Both come from the builder reading its own output with the tools it has, and
//! both are parsed here, where the parsers have tests.
//!
//! The binary is then run in the desktop image of the same target, which is a
//! machine that did not build it: that is what proves it starts and renders
//! somewhere other than in a guest with a compiler in it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::commands::vm;
use crate::guest::artifacts;
use crate::guest::job::{self, OutputTail};
use crate::guest::toolchain::Toolchain;
use crate::provider;
use crate::provider::target::{Image, Target};
use crate::runner::{Cmd, Runner};
use crate::store::state::StartReason;
use crate::store::{self, Store};
use crate::util;

/// Which targets a run covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Which {
    Windows,
    Linux,
    All,
}

impl Which {
    /// The targets, in the order a run does them.
    pub fn targets(self) -> Vec<Target> {
        match self {
            Self::Windows => vec![Target::Windows],
            Self::Linux => vec![Target::Linux],
            Self::All => Target::ALL.to_vec(),
        }
    }
}

/// What a run was asked for.
///
/// Four flags of one command line rather than a state machine, which is why
/// `struct_excessive_bools` is allowed here: they are independent answers to
/// independent questions and naming them together is the point.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub keep: bool,
    /// Whether to run the binary in the desktop image afterwards.
    pub verify: bool,
    pub allow_expired: bool,
    pub allow_dirty: bool,
}

/// How long a release build may take inside a guest.
///
/// A cold build with fat LTO and one codegen unit on eight virtual cores, plus
/// the crate download, lands between fifteen and forty minutes. Two hours is
/// generous on purpose: the cost of a timeout that is too short is a discarded
/// build, and the cost of one too long is waiting. It sits under the three hours
/// the Windows guest's scheduled task allows, which is the real ceiling there.
pub const BUILD_TIMEOUT: Duration = Duration::from_mins(120);

/// How long the verification render may take.
pub const VERIFY_TIMEOUT: Duration = Duration::from_mins(15);

/// What the verification render is asked for, and what the host checks it is.
pub const SMOKE_WIDTH: u32 = 640;
pub const SMOKE_HEIGHT: u32 = 360;
pub const SMOKE_FILE: &str = "smoke.png";

/// The lowest glibc a Linux release binary may require.
///
/// 2.35 is Ubuntu 22.04, which is what the builder image is. Anything higher
/// means the binary was linked somewhere else and will refuse to start on the
/// distributions this floor exists to cover.
pub const GLIBC_FLOOR: (u32, u32) = (2, 35);

/// The shared libraries a Linux release binary is expected to name.
///
/// X11, xcb, xkbcommon and EGL are `dlopen`ed by winit and glutin rather than
/// linked, so they are deliberately not here: a binary that named them at link
/// time would refuse to start on a machine with no display at all, which is
/// where the `render` subcommand runs.
pub const EXPECTED_NEEDED: [&str; 4] = [
    "libc.so.6",
    "libm.so.6",
    "libgcc_s.so.1",
    "libfontconfig.so.1",
];

/// The Windows runtime DLLs a static C runtime means the binary does not import.
pub const FORBIDDEN_IMPORTS: [&str; 2] = ["vcruntime140.dll", "msvcp140.dll"];

/// The executable's name inside a guest and in the dist directory.
pub fn exe_name(target: Target) -> &'static str {
    match target {
        Target::Windows => "sunlit-earth.exe",
        Target::Linux => "sunlit-earth",
    }
}

/// What the git facts of the tree being built are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitFacts {
    pub commit: String,
    /// `git describe --tags --always`, which is the readable name of the commit.
    pub describe: String,
    /// Whether tracked files are modified or staged. Untracked files are not
    /// dirt: they never reach the guest, because the archive is of `HEAD`.
    pub dirty: bool,
}

/// Whether `git status --porcelain --untracked-files=no` reported anything.
pub fn parse_dirty(status: &str) -> bool {
    status.lines().any(|line| !line.trim().is_empty())
}

/// Read the tree's git facts.
pub fn git_facts(runner: &dyn Runner, repo: &Path) -> Result<GitFacts, String> {
    let ask = |args: &[&str]| -> Result<String, String> {
        let cmd = Cmd::new("git")
            .args(args.iter().map(|a| (*a).to_owned()))
            .cwd(repo);
        let out = runner
            .capture(&cmd)
            .map_err(|e| format!("cannot run git: {e}"))?;
        if out.success() {
            Ok(out.stdout.trim().to_owned())
        } else {
            Err(format!(
                "git {} failed: {}",
                args.join(" "),
                out.stderr.trim()
            ))
        }
    };
    let commit = ask(&["rev-parse", "HEAD"])?;
    let describe = ask(&["describe", "--tags", "--always"]).unwrap_or_else(|_| commit.clone());
    let status = ask(&["status", "--porcelain", "--untracked-files=no"])?;
    Ok(GitFacts {
        commit,
        describe,
        dirty: parse_dirty(&status),
    })
}

/// Why a dirty tree is refused.
///
/// The archive is of `HEAD` whatever the working tree holds, so building a dirty
/// tree produces a binary whose recorded commit does not describe it. That is
/// the one thing a build record must not be wrong about, so it is a refusal
/// rather than a warning, and `--allow-dirty` is the way to say it is understood.
pub fn dirty_refusal() -> String {
    "the working tree has modified or staged files, and a release build is of \
     HEAD: the archive that goes into the guest is the commit, not what is on \
     disk here, so the binary would not be the tree you are looking at.\n\
     Commit or stash them, or pass --allow-dirty to build HEAD anyway and have \
     the record say `dirty: true`."
        .to_owned()
}

/// The source archive: the committed tree without the texture assets.
///
/// `textures/**` is Git LFS, so an archive of it carries pointer files rather
/// than images, and nothing in the build reads them: what `include_bytes!` pulls
/// in is the star catalog, the icon rasters and the shaders, all of which are in
/// here. `--prefix=src/` is what makes the extraction one directory rather than
/// a tree loose in the guest root.
pub fn archive_args(output: &Path) -> Vec<String> {
    vec![
        "archive".to_owned(),
        "--format=tar".to_owned(),
        format!("--output={}", output.display()),
        "--prefix=src/".to_owned(),
        "HEAD".to_owned(),
        ":!textures".to_owned(),
    ]
}

/// Write the archive, and answer how large it is.
pub fn write_archive(runner: &dyn Runner, repo: &Path, output: &Path) -> Result<u64, String> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let _ = std::fs::remove_file(output);
    let cmd = Cmd::new("git").args(archive_args(output)).cwd(repo);
    let out = runner
        .capture(&cmd)
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !out.success() {
        return Err(format!("git archive failed: {}", out.stderr.trim()));
    }
    std::fs::metadata(output)
        .map(|meta| meta.len())
        .map_err(|e| format!("git archive wrote no {}: {e}", output.display()))
}

/// Where the archive is copied to inside a guest.
pub fn guest_archive(target: Target) -> String {
    match target {
        Target::Windows => format!(r"{}\src.tar", crate::provider::GUEST_ROOT_WINDOWS),
        Target::Linux => format!("{}/src.tar", crate::provider::GUEST_ROOT_LINUX),
    }
}

/// The command that answers whether the builder has a toolchain in it.
///
/// Asked after the boot and before the source goes in, because a build that
/// fails twenty seconds into its job with "cargo is not recognized" is a worse
/// message than one that names the rebuild.
pub fn toolchain_probe(target: Target) -> String {
    match target {
        Target::Windows => {
            format!(r"if exist %USERPROFILE%\.cargo\bin\cargo.exe echo {TOOLCHAIN_MARKER}")
        }
        Target::Linux => {
            format!("test -x \"$HOME/.cargo/bin/cargo\" && echo {TOOLCHAIN_MARKER} || true")
        }
    }
}

/// What the probe prints when the toolchain is there.
pub const TOOLCHAIN_MARKER: &str = "SUNLIT_TOOLCHAIN_PRESENT";

/// What to say when it is not.
pub fn missing_toolchain(image: Image) -> String {
    format!(
        "the {image} guest booted and has no cargo in it, so there is nothing here \
         to build with. That is an image built before the toolchain was added, or \
         one whose provisioning did not finish.\n\
         Rebuild it: cargo xtask vm build-image {image}"
    )
}

/// The build job.
///
/// Every path is absolute and nothing depends on `PATH`, which is the rule the
/// guest job scripts already follow: the job runs from a scheduled task on
/// Windows and a detached session on Linux, and neither inherits a shell anybody
/// configured. `rustup toolchain install` runs explicitly rather than relying on
/// rustup installing a missing toolchain by itself, which 1.28.0 removed and
/// 1.28.1 restored behind a variable; it is a no-op when the image already
/// carries the channel.
///
/// The last two steps are what make the release claims checkable on the host:
/// the toolchain that built it, and the binary's own imports as the builder's
/// tools report them.
pub fn build_job(target: Target, pinned: &Toolchain) -> String {
    let channel = &pinned.channel;
    match target {
        Target::Linux => format!(
            "#!/usr/bin/env bash\n\
             set -euo pipefail\n\
             root={root}\n\
             export CARGO_NET_RETRY=5\n\
             export CARGO_TERM_COLOR=never\n\
             cargo=\"$HOME/.cargo/bin/cargo\"\n\
             rustc=\"$HOME/.cargo/bin/rustc\"\n\
             rustup=\"$HOME/.cargo/bin/rustup\"\n\
             \"$rustup\" toolchain install {channel} --profile minimal\n\
             rm -rf \"$root/src\"\n\
             tar -xf \"$root/src.tar\" -C \"$root\"\n\
             cd \"$root/src\"\n\
             \"$cargo\" \"+{channel}\" build --release --locked -p sunlit-earth\n\
             exe=\"$root/src/target/release/{exe}\"\n\
             cp \"$exe\" \"$SUNLIT_E2E_ARTIFACTS/{exe}\"\n\
             {{\n  \
               \"$rustc\" \"+{channel}\" -vV\n  \
               \"$cargo\" \"+{channel}\" -V\n\
             }} > \"$SUNLIT_E2E_ARTIFACTS/toolchain.txt\"\n\
             {{\n  \
               echo '== readelf -d'\n  \
               readelf -d \"$exe\"\n  \
               echo '== objdump -T'\n  \
               objdump -T \"$exe\"\n\
             }} > \"$SUNLIT_E2E_ARTIFACTS/deps.txt\"\n\
             ls -l \"$SUNLIT_E2E_ARTIFACTS\"\n",
            root = crate::provider::GUEST_ROOT_LINUX,
            exe = exe_name(target),
        ),
        Target::Windows => format!(
            "@echo off\r\n\
             set ROOT={root}\r\n\
             set CARGO_NET_RETRY=5\r\n\
             set CARGO_TERM_COLOR=never\r\n\
             set LIBCLANG_PATH={libclang}\r\n\
             set CARGO=%USERPROFILE%\\.cargo\\bin\\cargo.exe\r\n\
             set RUSTC=%USERPROFILE%\\.cargo\\bin\\rustc.exe\r\n\
             set RUSTUP=%USERPROFILE%\\.cargo\\bin\\rustup.exe\r\n\
             \"%RUSTUP%\" toolchain install {channel} --profile minimal || exit /b 1\r\n\
             if exist \"%ROOT%\\src\" rmdir /s /q \"%ROOT%\\src\"\r\n\
             tar.exe -xf \"%ROOT%\\src.tar\" -C \"%ROOT%\" || exit /b 1\r\n\
             cd /d \"%ROOT%\\src\" || exit /b 1\r\n\
             \"%CARGO%\" +{channel} build --release --locked -p sunlit-earth || exit /b 1\r\n\
             set EXE=%ROOT%\\src\\target\\release\\{exe}\r\n\
             copy /y \"%EXE%\" \"%SUNLIT_E2E_ARTIFACTS%\\{exe}\" || exit /b 1\r\n\
             \"%RUSTC%\" +{channel} -vV > \"%SUNLIT_E2E_ARTIFACTS%\\toolchain.txt\"\r\n\
             \"%CARGO%\" +{channel} -V >> \"%SUNLIT_E2E_ARTIFACTS%\\toolchain.txt\"\r\n\
             set VSWHERE=%ProgramFiles(x86)%\\Microsoft Visual Studio\\Installer\\vswhere.exe\r\n\
             set DUMPBIN=\r\n\
             for /f \"usebackq delims=\" %%%%i in (`\"%VSWHERE%\" -latest -products * \
             -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 \
             -find **\\Hostx64\\x64\\dumpbin.exe`) do set DUMPBIN=%%%%i\r\n\
             if not defined DUMPBIN echo no dumpbin found & exit /b 1\r\n\
             \"%DUMPBIN%\" /dependents \"%SUNLIT_E2E_ARTIFACTS%\\{exe}\" \
             > \"%SUNLIT_E2E_ARTIFACTS%\\deps.txt\" || exit /b 1\r\n\
             dir \"%SUNLIT_E2E_ARTIFACTS%\"\r\n\
             exit /b 0\r\n",
            root = crate::provider::GUEST_ROOT_WINDOWS,
            libclang = crate::commands::build_layer::LIBCLANG_DIR,
            exe = exe_name(target),
        ),
    }
}

/// The verification job: one headless render in the desktop image.
///
/// The same smoke test `ci.yml` runs, for the same reason and with the same
/// check on the result: it asserts the output is a PNG of the size asked for
/// rather than merely a file of non-trivial size. `SUNLIT_EARTH_TEXTURES` is set
/// exactly when the textures were staged, on the rule the e2e job follows: a
/// directory that is not there would make the app fall back to the procedural
/// grid anyway, and naming one would be a lie in the script.
pub fn verify_job(target: Target, exe: &str, textures: Option<&str>) -> String {
    match target {
        Target::Linux => format!(
            "#!/usr/bin/env bash\n\
             set -uo pipefail\n\
             {textures}\
             export RUST_BACKTRACE=1\n\
             {exe} --version\n\
             {exe} render --output \"$SUNLIT_E2E_ARTIFACTS/{file}\" \
             --width {width} --height {height}\n",
            textures = textures.map_or_else(String::new, |dir| format!(
                "export SUNLIT_EARTH_TEXTURES={}\n",
                artifacts::shell_quote(dir)
            )),
            exe = artifacts::shell_quote(exe),
            file = SMOKE_FILE,
            width = SMOKE_WIDTH,
            height = SMOKE_HEIGHT,
        ),
        Target::Windows => format!(
            "@echo off\r\n\
             {textures}\
             set RUST_BACKTRACE=1\r\n\
             \"{exe}\" --version\r\n\
             \"{exe}\" render --output \"%SUNLIT_E2E_ARTIFACTS%\\{file}\" \
             --width {width} --height {height}\r\n\
             exit /b %ERRORLEVEL%\r\n",
            textures = textures.map_or_else(String::new, |dir| format!(
                "set SUNLIT_EARTH_TEXTURES={dir}\r\n"
            )),
            exe = exe,
            file = SMOKE_FILE,
            width = SMOKE_WIDTH,
            height = SMOKE_HEIGHT,
        ),
    }
}

/// The width and height in a PNG's IHDR, or `None` if this is not a PNG.
///
/// Read rather than trusted: a `render` that failed after opening its output
/// leaves a file, and a file's existence is not evidence that anything was
/// drawn. The header is the first 24 bytes and its layout is fixed.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 24 || bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

/// The shared libraries `readelf -d` reported as `NEEDED`.
pub fn parse_needed(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| line.contains("(NEEDED)"))
        .filter_map(|line| {
            let start = line.find('[')?;
            let end = line[start..].find(']')? + start;
            Some(line[start + 1..end].trim().to_owned())
        })
        .collect()
}

/// The highest `GLIBC_x.y` symbol version the binary imports, which is the
/// oldest glibc it can run against.
pub fn parse_glibc_floor(text: &str) -> Option<(u32, u32)> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
        .filter_map(|token| token.strip_prefix("GLIBC_"))
        .filter_map(|version| {
            let mut parts = version.split('.');
            let major = parts.next()?.parse().ok()?;
            let minor = parts.next()?.parse().ok()?;
            Some((major, minor))
        })
        .max()
}

/// The DLLs `dumpbin /dependents` listed.
pub fn parse_dependents(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| {
            line.len() > 4 && line.to_ascii_lowercase().ends_with(".dll") && !line.contains(' ')
        })
        .map(std::borrow::ToOwned::to_owned)
        .collect()
}

/// What the builder's own reading of the binary says about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Linkage {
    /// Linux: the `NEEDED` set, in the order the binary lists it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needed: Vec<String>,
    /// Linux: the oldest glibc this binary runs against, as `2.35`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glibc_floor: Option<String>,
    /// Windows: what the binary imports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<String>,
    /// Windows: whether the C runtime is linked in rather than imported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crt_static: Option<bool>,
}

/// Read `deps.txt` and decide whether the binary is the one this command
/// promises.
///
/// The whole point of the file: the two claims a release binary makes are about
/// its own linkage, and neither can be observed by running it. A Windows guest
/// with the redistributable installed runs a dynamically linked binary
/// perfectly, and a Linux binary with too high a floor runs fine on the machine
/// that built it.
pub fn check_linkage(target: Target, deps: &str) -> Result<Linkage, String> {
    match target {
        Target::Linux => {
            let needed = parse_needed(deps);
            if needed.is_empty() {
                return Err(
                    "deps.txt lists no NEEDED libraries, so the builder did not \
                            read the binary it built"
                        .to_owned(),
                );
            }
            let mut unexpected: Vec<&String> = needed
                .iter()
                .filter(|lib| !EXPECTED_NEEDED.contains(&lib.as_str()))
                .collect();
            unexpected.sort();
            if !unexpected.is_empty() {
                return Err(format!(
                    "the binary links libraries this port does not use: {}. \
                     The expected set is {}, and anything else is a dependency a \
                     user would have to install.",
                    unexpected
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    EXPECTED_NEEDED.join(", ")
                ));
            }
            let floor = parse_glibc_floor(deps).ok_or_else(|| {
                "deps.txt names no GLIBC_ symbol version, so the glibc floor cannot \
                 be read"
                    .to_owned()
            })?;
            if floor > GLIBC_FLOOR {
                return Err(format!(
                    "the binary requires glibc {}.{}, above the {}.{} floor this \
                     builder exists to give it: it would refuse to start on the \
                     distributions the floor covers",
                    floor.0, floor.1, GLIBC_FLOOR.0, GLIBC_FLOOR.1
                ));
            }
            Ok(Linkage {
                needed,
                glibc_floor: Some(format!("{}.{}", floor.0, floor.1)),
                imports: Vec::new(),
                crt_static: None,
            })
        }
        Target::Windows => {
            let imports = parse_dependents(deps);
            if imports.is_empty() {
                return Err(
                    "deps.txt lists no imported DLLs, so the builder did not read \
                            the binary it built"
                        .to_owned(),
                );
            }
            let found: Vec<&str> = FORBIDDEN_IMPORTS
                .into_iter()
                .filter(|dll| {
                    imports
                        .iter()
                        .any(|import| import.eq_ignore_ascii_case(dll))
                })
                .collect();
            if !found.is_empty() {
                return Err(format!(
                    "the binary imports {}, so it needs the Visual C++ \
                     redistributable on every machine that runs it. The static C \
                     runtime in .cargo/config.toml is what stops that.",
                    found.join(" and ")
                ));
            }
            Ok(Linkage {
                needed: Vec::new(),
                glibc_floor: None,
                imports,
                crt_static: Some(true),
            })
        }
    }
}

/// What a release binary was built from, written beside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildInfo {
    pub format_version: u32,
    pub target: String,
    pub commit: String,
    pub describe: String,
    pub dirty: bool,
    pub built_utc: String,
    pub duration_secs: u64,
    /// The channel `rust-toolchain.toml` pins.
    pub channel: String,
    /// `rustc -vV` and `cargo -V` as the guest reported them.
    pub toolchain: String,
    /// The builder image and what its manifest says about itself.
    pub builder: BuilderInfo,
    pub linkage: Linkage,
    /// The desktop image the binary was run in, when verification ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_in: Option<String>,
    pub xtask_version: String,
}

/// Which image built it, and which build of that image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuilderInfo {
    pub image: String,
    pub template_hash: String,
    pub built_utc: String,
    pub source: String,
    /// For a layer, the parent's checksum, which is the other half of its
    /// identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_checksum: Option<String>,
}

impl BuildInfo {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    #[cfg(test)]
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("malformed build-info.json: {e}"))
    }
}

/// The version of the record's own layout.
pub const BUILD_INFO_VERSION: u32 = 1;

/// Where the artifacts land: `<target dir>/dist/<target>/`.
///
/// `CARGO_TARGET_DIR` if it is set, because a developer who moved their target
/// directory moved it for a reason, and `<repo>/target` otherwise.
pub fn dist_dir(target: Target) -> PathBuf {
    let base = util::env_var("CARGO_TARGET_DIR")
        .map_or_else(|| store::repo_root().join("target"), PathBuf::from);
    base.join("dist").join(target.slug())
}

/// Run the command.
pub fn run(runner: &dyn Runner, which: Which, options: Options) -> Result<u8, String> {
    let pinned = crate::guest::toolchain::pinned()?;
    let store = store::store()?;
    let repo = store::repo_root();
    let git = git_facts(runner, &repo)?;
    if git.dirty && !options.allow_dirty {
        return Err(dirty_refusal());
    }

    let targets = which.targets();
    let mut summary = Vec::new();
    let mut failed = false;
    for target in targets {
        println!();
        println!("== {target}: a release build of {}", git.describe);
        match one_target(runner, &store, &repo, target, &pinned, &git, options) {
            Ok(line) => summary.push(line),
            Err(e) => {
                println!("error: {e}");
                summary.push(format!("{target}: failed"));
                failed = true;
            }
        }
    }

    println!();
    for line in &summary {
        println!("{line}");
    }
    Ok(u8::from(failed))
}

/// One target, end to end.
#[allow(clippy::too_many_lines)]
fn one_target(
    runner: &dyn Runner,
    store: &Store,
    repo: &Path,
    target: Target,
    pinned: &Toolchain,
    git: &GitFacts,
    options: Options,
) -> Result<String, String> {
    let builder = Image::builder(target);
    let desktop = Image::desktop(target);
    let started = Instant::now();

    // Both images are checked before anything boots, so a missing or expired
    // desktop image is refused up front rather than after a twenty-minute build.
    vm::check_image(store, builder, options.allow_expired)?;
    if options.verify {
        vm::check_image(store, desktop, options.allow_expired)?;
    }
    let builder_info = builder_info(store, builder)?;

    let archive = store.run_dir(builder).join("dist").join("src.tar");
    let bytes = write_archive(runner, repo, &archive)?;
    println!(
        "  source:  {} of {} ({})",
        util::format_bytes(bytes),
        git.commit,
        if git.dirty {
            "dirty tree"
        } else {
            "clean tree"
        }
    );
    println!("  builder: {builder}, built {}", builder_info.built_utc);

    let results = build_in_builder(runner, store, builder, pinned, &archive, options)?;

    let exe = results.join("artifacts").join(exe_name(target));
    if !exe.is_file() {
        return Err(format!(
            "the build reported success and brought back no {}",
            exe.display()
        ));
    }
    let deps = std::fs::read_to_string(results.join("artifacts").join("deps.txt"))
        .map_err(|e| format!("the build brought back no readable deps.txt: {e}"))?;
    let linkage = check_linkage(target, &deps)?;
    let toolchain = std::fs::read_to_string(results.join("artifacts").join("toolchain.txt"))
        .unwrap_or_default()
        .trim()
        .to_owned();
    println!("  linkage: {}", linkage_summary(target, &linkage));

    let smoke = if options.verify {
        Some(verify_in_desktop(runner, store, desktop, &exe, options)?)
    } else {
        println!("  skipping the verification boot, because --no-verify was given");
        None
    };

    let info = BuildInfo {
        format_version: BUILD_INFO_VERSION,
        target: target.slug().to_owned(),
        commit: git.commit.clone(),
        describe: git.describe.clone(),
        dirty: git.dirty,
        built_utc: util::format_unix_utc(util::now_unix()),
        duration_secs: started.elapsed().as_secs(),
        channel: pinned.channel.clone(),
        toolchain,
        builder: builder_info,
        linkage,
        verified_in: smoke.as_ref().map(|_| desktop.slug().to_owned()),
        xtask_version: env!("CARGO_PKG_VERSION").to_owned(),
    };

    let dist = dist_dir(target);
    publish(&dist, &exe, &results, smoke.as_deref(), &info)?;

    println!();
    println!("{target}: {}", dist.join(exe_name(target)).display());
    println!("  built from {} in the {builder} image", git.describe);
    println!("  {}", runtime_requirements(target));
    Ok(format!(
        "{target}: built in {} and {}",
        util::format_duration(started.elapsed()),
        if smoke.is_some() {
            format!("rendered in the {desktop} guest")
        } else {
            "not verified".to_owned()
        }
    ))
}

/// What the record says about the image that built it.
fn builder_info(store: &Store, builder: Image) -> Result<BuilderInfo, String> {
    let inventory = crate::store::inventory::scan(store);
    let entry = inventory
        .for_image(builder)
        .ok_or_else(|| format!("nothing is known about the {builder} image"))?;
    let manifest = entry.manifest.as_ref().ok_or_else(|| {
        format!("the {builder} image has no manifest, so nothing could be recorded about it")
    })?;
    Ok(BuilderInfo {
        image: builder.slug().to_owned(),
        template_hash: manifest.template_hash.clone(),
        built_utc: manifest.built_utc.clone(),
        source: manifest.source.clone(),
        parent_checksum: manifest.parent.as_ref().map(|p| p.checksum.clone()),
    })
}

/// Boot the builder, run the build, bring the results back, and take it down.
fn build_in_builder(
    runner: &dyn Runner,
    store: &Store,
    builder: Image,
    pinned: &Toolchain,
    archive: &Path,
    options: Options,
) -> Result<PathBuf, String> {
    let target = builder.target();
    let mut session = vm::boot(
        runner,
        store,
        builder,
        StartReason::Dist,
        options.allow_expired,
        None,
    )?;

    // From here the guest exists, so nothing may return without saying what
    // happened to it.
    let outcome = (|| -> Result<PathBuf, String> {
        let probe = session
            .provider
            .exec(&session.state, &toolchain_probe(target))?;
        if !probe.stdout.contains(TOOLCHAIN_MARKER) {
            return Err(missing_toolchain(builder));
        }

        session
            .provider
            .copy_in(&session.state, archive, &guest_archive(target))?;

        println!("  building; cargo's own output follows");
        let scratch = store.run_dir(builder).join("job");
        let mut tail = OutputTail::new();
        let code = job::run_watching(
            session.provider.as_ref(),
            &session.state,
            target,
            &build_job(target, pinned),
            &scratch,
            BUILD_TIMEOUT,
            Some(&mut tail),
        )?;
        let results = store.results_dir(builder);
        session.provider.collect_results(
            &session.state,
            &provider::guest_results(target),
            &results,
        )?;
        if code != 0 {
            return Err(format!(
                "the build exited {code} inside the guest; its output is above and \
                 {} has what it wrote",
                results.display()
            ));
        }
        Ok(results)
    })();

    // The builder is torn down whichever way it went, unless the run is keeping
    // the last guest it booted and there is no verification boot after this one.
    let keep = options.keep && (!options.verify || outcome.is_err());
    match &outcome {
        Ok(_) if !keep => {
            if let Err(e) = session.tear_down(store) {
                println!(
                    "warning: {} could not be destroyed: {e}",
                    session.state.vm_name
                );
                println!("{}", session.reach_hint());
            }
        }
        Ok(_) => {
            let enhanced = vm::hand_over(&mut session, store);
            println!(
                "{}",
                vm::lifecycle_explainer(
                    builder,
                    vm::Prepared {
                        staged: false,
                        enhanced_session: enhanced,
                    }
                )
            );
            println!("{}", kept_builder_note(builder));
        }
        Err(_) => println!("{}", vm::after_failure(&session, store, keep)),
    }
    outcome
}

/// What is in a builder that was kept.
pub fn kept_builder_note(builder: Image) -> String {
    format!(
        "The source tree it built is in the guest's own root, with its \
         `target/release` beside it, so a build can be repeated in there by hand. \
         `cargo xtask vm down {builder}` ends it and takes the overlay with it."
    )
}

/// Boot the desktop image, stage the binary, render once, and take it down.
#[allow(clippy::too_many_lines)]
fn verify_in_desktop(
    runner: &dyn Runner,
    store: &Store,
    desktop: Image,
    exe: &Path,
    options: Options,
) -> Result<PathBuf, String> {
    let target = desktop.target();
    println!();
    println!("  verifying it in the {desktop} guest, which did not build it");
    let mut session = vm::boot(
        runner,
        store,
        desktop,
        StartReason::Dist,
        options.allow_expired,
        None,
    )?;

    let outcome = (|| -> Result<PathBuf, String> {
        let bin_dir = provider::guest_bin(target);
        session.provider.copy_in(&session.state, exe, &bin_dir)?;
        let guest_exe = match target {
            Target::Windows => format!(r"{bin_dir}\{}", exe_name(target)),
            Target::Linux => format!("{bin_dir}/{}", exe_name(target)),
        };
        if target == Target::Linux {
            // scp keeps the mode of what it copied, and the host's copy came out
            // of a tar the guest wrote, so this is belt and braces rather than a
            // fix for something observed.
            let _ = session
                .provider
                .exec(&session.state, &format!("chmod +x {guest_exe}"));
        }

        // The same size check the e2e staging makes, and for the same reason: a
        // checkout without the Git LFS objects holds pointer files under the
        // asset names, and naming a directory of those costs a decode failure
        // where naming nothing at all draws the procedural grid quietly.
        let textures = artifacts::host_textures(&store::repo_root());
        let guest_textures = match &textures {
            Some(dir) => {
                println!("  staging the textures from {}", dir.display());
                session
                    .provider
                    .copy_in(&session.state, dir, &provider::guest_textures(target))?;
                Some(provider::guest_textures(target))
            }
            None => None,
        };

        let scratch = store.run_dir(desktop).join("job");
        let code = job::run(
            session.provider.as_ref(),
            &session.state,
            target,
            &verify_job(target, &guest_exe, guest_textures.as_deref()),
            &scratch,
            VERIFY_TIMEOUT,
        )?;
        let results = store.results_dir(desktop);
        session.provider.collect_results(
            &session.state,
            &provider::guest_results(target),
            &results,
        )?;
        if let Ok(log) = std::fs::read_to_string(results.join("output.log")) {
            for line in log.lines() {
                println!("    {line}");
            }
        }
        if code != 0 {
            return Err(format!(
                "the binary exited {code} in the {desktop} guest, so it does not run \
                 on a machine that did not build it"
            ));
        }
        let smoke = results.join("artifacts").join(SMOKE_FILE);
        let bytes = std::fs::read(&smoke)
            .map_err(|e| format!("the render brought back no {}: {e}", smoke.display()))?;
        match png_size(&bytes) {
            Some((SMOKE_WIDTH, SMOKE_HEIGHT)) => {
                println!(
                    "  rendered {SMOKE_WIDTH}x{SMOKE_HEIGHT}, {}",
                    util::format_bytes(bytes.len() as u64)
                );
                Ok(smoke)
            }
            Some((width, height)) => Err(format!(
                "the render is {width}x{height}, not the {SMOKE_WIDTH}x{SMOKE_HEIGHT} \
                 it was asked for"
            )),
            None => Err(format!(
                "{} is not a PNG, so nothing was drawn",
                smoke.display()
            )),
        }
    })();

    let keep = options.keep;
    match &outcome {
        Ok(_) if !keep => {
            if let Err(e) = session.tear_down(store) {
                println!(
                    "warning: {} could not be destroyed: {e}",
                    session.state.vm_name
                );
                println!("{}", session.reach_hint());
            }
        }
        Ok(_) => {
            let enhanced = vm::hand_over(&mut session, store);
            println!(
                "{}",
                vm::lifecycle_explainer(
                    desktop,
                    vm::Prepared {
                        staged: false,
                        enhanced_session: enhanced,
                    }
                )
            );
            println!("The binary this run built is in the guest's own bin directory.");
        }
        Err(_) => println!("{}", vm::after_failure(&session, store, keep)),
    }
    outcome
}

/// Replace the dist directory with what this run produced.
///
/// Wholesale, and only on success: a failed rebuild leaves the previous
/// artifact where it was, because half a dist directory is worse than an old one
/// and there is nothing here that says which files came from which run.
fn publish(
    dist: &Path,
    exe: &Path,
    results: &Path,
    smoke: Option<&Path>,
    info: &BuildInfo,
) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(dist);
    std::fs::create_dir_all(dist).map_err(|e| format!("cannot create {}: {e}", dist.display()))?;

    let name = exe
        .file_name()
        .ok_or_else(|| "the built binary has no file name".to_owned())?;
    copy(exe, &dist.join(name))?;
    // The builder's own log, which is the whole of what cargo said.
    let log = results.join("output.log");
    if log.is_file() {
        copy(&log, &dist.join("build.log"))?;
    }
    if let Some(smoke) = smoke {
        copy(smoke, &dist.join(SMOKE_FILE))?;
    }
    std::fs::write(dist.join("build-info.json"), info.to_json())
        .map_err(|e| format!("cannot write build-info.json: {e}"))
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("cannot copy {} to {}: {e}", from.display(), to.display()))
}

/// One line about what the linkage check found.
fn linkage_summary(target: Target, linkage: &Linkage) -> String {
    match target {
        Target::Linux => format!(
            "glibc {} and {}",
            linkage.glibc_floor.as_deref().unwrap_or("unknown"),
            linkage.needed.join(", ")
        ),
        Target::Windows => format!(
            "{} imports, none of them the Visual C++ runtime",
            linkage.imports.len()
        ),
    }
}

/// What a machine needs to run the artifact.
pub fn runtime_requirements(target: Target) -> String {
    match target {
        Target::Windows => "it needs Windows 10 or later and nothing else: the C runtime is \
                            linked in, so there is no redistributable to install"
            .to_owned(),
        Target::Linux => format!(
            "it needs glibc {}.{} or newer and fontconfig, which is Ubuntu 22.04 and \
             everything since; X11, xcb, xkbcommon and EGL are opened at run time \
             rather than linked",
            GLIBC_FLOOR.0, GLIBC_FLOOR.1
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_runs_both_targets_in_order() {
        assert_eq!(Which::All.targets(), vec![Target::Windows, Target::Linux]);
        assert_eq!(Which::Linux.targets(), vec![Target::Linux]);
        assert_eq!(Which::Windows.targets(), vec![Target::Windows]);
    }

    /// Untracked files are not dirt: the archive is of `HEAD`, so a file git has
    /// never seen cannot reach the guest, and refusing over one would make the
    /// command unusable in any working checkout.
    #[test]
    fn only_tracked_changes_make_a_tree_dirty() {
        assert!(!parse_dirty(""));
        assert!(!parse_dirty("\n  \n"));
        assert!(parse_dirty(" M crates/xtask/src/main.rs\n"));
        assert!(parse_dirty("A  new.rs\n"));
    }

    #[test]
    fn the_refusal_says_what_would_be_built_and_how_to_proceed() {
        let text = dirty_refusal();
        assert!(text.contains("HEAD"), "{text}");
        assert!(text.contains("--allow-dirty"), "{text}");
    }

    /// The archive is the whole of what reaches the guest, so what is in it and
    /// what is not are both load-bearing: `textures/` is Git LFS and the build
    /// does not read it, and the prefix is what keeps the extraction to one
    /// directory.
    #[test]
    fn the_archive_is_head_without_the_textures_and_under_one_prefix() {
        let args = archive_args(Path::new("/srv/vm/run/linux-builder/dist/src.tar"));
        assert_eq!(args[0], "archive");
        assert!(args.iter().any(|a| a == "--format=tar"), "{args:?}");
        assert!(args.iter().any(|a| a == "--prefix=src/"), "{args:?}");
        assert!(args.iter().any(|a| a == "HEAD"), "{args:?}");
        assert!(args.iter().any(|a| a == ":!textures"), "{args:?}");
        assert!(args.iter().any(|a| a.starts_with("--output=")), "{args:?}");
    }

    #[test]
    fn the_toolchain_probe_is_quiet_when_there_is_no_toolchain() {
        for target in Target::ALL {
            let probe = toolchain_probe(target);
            assert!(probe.contains(TOOLCHAIN_MARKER), "{probe}");
            assert!(probe.contains("cargo"), "{probe}");
        }
        assert!(toolchain_probe(Target::Linux).ends_with("|| true"));
        assert!(toolchain_probe(Target::Windows).starts_with("if exist "));
        assert!(
            missing_toolchain(Image::WindowsBuilder).contains("vm build-image windows-builder")
        );
    }

    /// Nothing in a job may depend on the working directory or on `PATH`: it
    /// runs from a scheduled task on Windows and a detached session on Linux,
    /// and neither inherits a shell anybody configured.
    #[test]
    fn the_build_job_names_every_tool_by_absolute_path() {
        let pinned =
            crate::guest::toolchain::parse("[toolchain]\nchannel = \"1.94.0\"\n").expect("parses");

        let linux = build_job(Target::Linux, &pinned);
        assert!(linux.contains("$HOME/.cargo/bin/cargo"), "{linux}");
        assert!(linux.contains("toolchain install 1.94.0"), "{linux}");
        assert!(
            linux.contains("build --release --locked -p sunlit-earth"),
            "{linux}"
        );
        assert!(linux.contains("CARGO_NET_RETRY"), "{linux}");
        assert!(linux.contains("readelf -d"), "{linux}");
        assert!(linux.contains("objdump -T"), "{linux}");
        assert!(linux.contains("$SUNLIT_E2E_ARTIFACTS/deps.txt"), "{linux}");
        assert!(!linux.contains("\r\n"), "a shell script with CRLF in it");

        let windows = build_job(Target::Windows, &pinned);
        assert!(
            windows.contains(r"%USERPROFILE%\.cargo\bin\cargo.exe"),
            "{windows}"
        );
        assert!(windows.contains("toolchain install 1.94.0"), "{windows}");
        assert!(
            windows.contains(&format!(
                "set LIBCLANG_PATH={}",
                crate::commands::build_layer::LIBCLANG_DIR
            )),
            "{windows}"
        );
        assert!(windows.contains("dumpbin"), "{windows}");
        assert!(windows.contains("vswhere.exe"), "{windows}");
        // cmd.exe wants CRLF, and every line has to be able to fail the job.
        assert!(windows.contains("\r\n"), "{windows}");
        assert!(windows.contains("|| exit /b 1"), "{windows}");
        // In a batch file the loop variable is doubled; a single % would be
        // taken as an argument reference and the loop would find nothing.
        assert!(windows.contains("%%i"), "{windows}");
    }

    #[test]
    fn the_verification_job_names_the_textures_only_when_they_were_staged() {
        let with = verify_job(
            Target::Linux,
            "/var/lib/sunlit-e2e/bin/sunlit-earth",
            Some("/t"),
        );
        assert!(with.contains("SUNLIT_EARTH_TEXTURES='/t'"), "{with}");
        let without = verify_job(Target::Linux, "/var/lib/sunlit-e2e/bin/sunlit-earth", None);
        assert!(!without.contains("SUNLIT_EARTH_TEXTURES"), "{without}");

        for target in Target::ALL {
            let job = verify_job(target, "x", None);
            assert!(job.contains("render"), "{target}: {job}");
            assert!(job.contains("--width 640"), "{target}: {job}");
            assert!(job.contains("--height 360"), "{target}: {job}");
            assert!(job.contains(SMOKE_FILE), "{target}: {job}");
        }
    }

    /// A `render` that failed after opening its output leaves a file behind, so
    /// the check is on the header rather than on the size.
    #[test]
    fn a_png_is_measured_from_its_header_and_anything_else_is_refused() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&360u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((640, 360)));

        assert_eq!(png_size(&[]), None);
        assert_eq!(png_size(b"not a png at all, but long enough to read"), None);
        let mut truncated = png.clone();
        truncated.truncate(20);
        assert_eq!(png_size(&truncated), None);
        // A file whose signature is right and whose first chunk is not IHDR is
        // not something to read a size out of.
        let mut wrong_chunk = png.clone();
        wrong_chunk[12..16].copy_from_slice(b"IDAT");
        assert_eq!(png_size(&wrong_chunk), None);
    }

    const READELF: &str = "Dynamic section at offset 0x2d1000 contains 27 entries:\n\
         \x20 Tag        Type                         Name/Value\n\
         \x20 0x0000000000000001 (NEEDED)             Shared library: [libfontconfig.so.1]\n\
         \x20 0x0000000000000001 (NEEDED)             Shared library: [libgcc_s.so.1]\n\
         \x20 0x0000000000000001 (NEEDED)             Shared library: [libm.so.6]\n\
         \x20 0x0000000000000001 (NEEDED)             Shared library: [libc.so.6]\n\
         \x20 0x000000000000000c (INIT)               0x1e000\n";

    const OBJDUMP: &str = "DYNAMIC SYMBOL TABLE:\n\
         0000000000000000      DF *UND*  0000000000000000  GLIBC_2.2.5 memcpy\n\
         0000000000000000      DF *UND*  0000000000000000  GLIBC_2.34  pthread_create\n\
         0000000000000000      DF *UND*  0000000000000000  GLIBC_2.35  __libc_start_main\n\
         0000000000000000      DF *UND*  0000000000000000  GCC_3.0     _Unwind_Resume\n";

    #[test]
    fn the_linux_linkage_is_read_out_of_the_builders_own_report() {
        let deps = format!("== readelf -d\n{READELF}== objdump -T\n{OBJDUMP}");
        let linkage = check_linkage(Target::Linux, &deps).expect("the expected linkage");
        assert_eq!(linkage.glibc_floor.as_deref(), Some("2.35"));
        assert_eq!(linkage.needed.len(), 4);
        for expected in EXPECTED_NEEDED {
            assert!(linkage.needed.iter().any(|n| n == expected), "{expected}");
        }
        assert_eq!(linkage.crt_static, None);
    }

    #[test]
    fn a_glibc_floor_above_the_builders_is_refused() {
        let deps = format!("{READELF}DYNAMIC SYMBOL TABLE:\n  GLIBC_2.38 statx\n  GLIBC_2.35 x\n");
        let err = check_linkage(Target::Linux, &deps).unwrap_err();
        assert!(err.contains("glibc 2.38"), "{err}");
        assert!(err.contains("2.35 floor"), "{err}");
    }

    #[test]
    fn a_library_the_port_does_not_use_is_refused_by_name() {
        let deps = format!(
            "{READELF}   0x0000000000000001 (NEEDED)  Shared library: [libX11.so.6]\n\
             DYNAMIC SYMBOL TABLE:\n  GLIBC_2.35 x\n"
        );
        let err = check_linkage(Target::Linux, &deps).unwrap_err();
        assert!(err.contains("libX11.so.6"), "{err}");
    }

    #[test]
    fn an_empty_report_is_a_builder_that_did_not_look_rather_than_a_pass() {
        for target in Target::ALL {
            let err = check_linkage(target, "nothing here\n").unwrap_err();
            assert!(err.contains("did not read the binary"), "{target}: {err}");
        }
    }

    const DUMPBIN: &str = "Microsoft (R) COFF/PE Dumper Version 14.44.35207.0\n\
         Copyright (C) Microsoft Corporation.  All rights reserved.\n\
         \n\
         Dump of file sunlit-earth.exe\n\
         \n\
         File Type: EXECUTABLE IMAGE\n\
         \n\
         \x20 Image has the following dependencies:\n\
         \n\
         \x20   KERNEL32.dll\n\
         \x20   ADVAPI32.dll\n\
         \x20   d3d12.dll\n\
         \n\
         \x20 Summary\n\
         \n\
         \x20     1000 .data\n";

    /// The claim a Windows release binary makes is about its own imports, and it
    /// cannot be checked by running it: a guest with the redistributable
    /// installed runs a dynamically linked binary perfectly well.
    #[test]
    fn the_windows_linkage_proves_the_static_runtime_on_the_artifact() {
        let linkage = check_linkage(Target::Windows, DUMPBIN).expect("no runtime imports");
        assert_eq!(linkage.crt_static, Some(true));
        assert!(linkage.imports.iter().any(|i| i == "KERNEL32.dll"));
        assert!(!linkage.imports.iter().any(|i| i.contains("Summary")));
        assert!(linkage.glibc_floor.is_none());

        for dll in FORBIDDEN_IMPORTS {
            let with = format!("{DUMPBIN}    {dll}\n");
            let err = check_linkage(Target::Windows, &with).unwrap_err();
            assert!(err.contains(dll), "{err}");
            assert!(
                err.contains("crt-static") || err.contains("static C runtime"),
                "{err}"
            );
        }
        // Windows spells its own DLLs in whatever case it feels like.
        let shouty = DUMPBIN.replace("d3d12.dll", "VCRUNTIME140.DLL");
        assert!(check_linkage(Target::Windows, &shouty).is_err());
    }

    #[test]
    fn a_build_record_round_trips() {
        let info = BuildInfo {
            format_version: BUILD_INFO_VERSION,
            target: Target::Linux.slug().to_owned(),
            commit: "0123456789abcdef".to_owned(),
            describe: "v0.1.0-3-g0123456".to_owned(),
            dirty: false,
            built_utc: "2026-08-28T12:00:00Z".to_owned(),
            duration_secs: 1_234,
            channel: "1.94.0".to_owned(),
            toolchain: "rustc 1.94.0".to_owned(),
            builder: BuilderInfo {
                image: Image::LinuxBuilder.slug().to_owned(),
                template_hash: "crc32:deadbeef".to_owned(),
                built_utc: "2026-08-28T09:00:00Z".to_owned(),
                source: "ubuntu 22.04 cloud image".to_owned(),
                parent_checksum: None,
            },
            linkage: Linkage {
                needed: EXPECTED_NEEDED.iter().map(|s| (*s).to_owned()).collect(),
                glibc_floor: Some("2.35".to_owned()),
                imports: Vec::new(),
                crt_static: None,
            },
            verified_in: Some(Image::Linux.slug().to_owned()),
            xtask_version: "0.1.0".to_owned(),
        };
        let parsed = BuildInfo::from_json(&info.to_json()).expect("round trip");
        assert_eq!(parsed, info);
        // The two facts a reader looks for first are the commit and the channel.
        let json = info.to_json();
        assert!(json.contains("\"commit\""), "{json}");
        assert!(json.contains("\"channel\": \"1.94.0\""), "{json}");
        assert!(json.contains("\"glibc_floor\": \"2.35\""), "{json}");
        // A Linux record carries no Windows fields at all rather than empty ones.
        assert!(!json.contains("crt_static"), "{json}");
        assert!(!json.contains("imports"), "{json}");
    }

    #[test]
    fn the_dist_directory_is_under_the_target_directory_and_per_target() {
        for target in Target::ALL {
            let dir = dist_dir(target);
            assert!(
                dir.ends_with(Path::new("dist").join(target.slug())),
                "{dir:?}"
            );
        }
        assert_ne!(dist_dir(Target::Windows), dist_dir(Target::Linux));
    }

    #[test]
    fn what_the_artifact_needs_to_run_is_stated_per_target() {
        let windows = runtime_requirements(Target::Windows);
        assert!(windows.contains("Windows 10"), "{windows}");
        assert!(windows.contains("no redistributable"), "{windows}");
        let linux = runtime_requirements(Target::Linux);
        assert!(linux.contains("2.35"), "{linux}");
        assert!(linux.contains("fontconfig"), "{linux}");
    }
}
