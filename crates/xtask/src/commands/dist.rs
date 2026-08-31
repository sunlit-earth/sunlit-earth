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

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::commands::bundle::{self, Bundled};
use crate::commands::vm;
use crate::guest::artifacts;
use crate::guest::job::{self, OutputTail};
use crate::guest::toolchain::Toolchain;
use crate::provider;
use crate::provider::target::{Image, Target};
use crate::runner::{Cmd, Runner};
use crate::store::cache;
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

/// What a run was asked for: which targets, and the four flags.
///
/// Four flags of one command line rather than a state machine, which is why
/// `struct_excessive_bools` is allowed here: they are independent answers to
/// independent questions and naming them together is the point. Which targets
/// is in here with them because one of the four is not a property of a target on
/// its own: `--keep` keeps the last guest of the whole run, so deciding it needs
/// to know what else the run is going to boot.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub which: Which,
    pub keep: bool,
    /// Whether to run the binary in the desktop image afterwards.
    pub verify: bool,
    /// Whether an earlier build in this image may hand anything to this one.
    /// `false` is goal 4 in its original form, on demand.
    pub cache: bool,
    pub allow_expired: bool,
    pub allow_dirty: bool,
}

/// One boot of a run, as the keep rule sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Boot {
    pub target: Target,
    /// The builder guest rather than the desktop guest that verifies.
    pub builder: bool,
    /// Whether the work in it failed, which ends the target there and makes its
    /// guest the last one the run booted.
    pub failed: bool,
}

/// Whether the guest a boot created is kept once the run is over.
///
/// `--keep` keeps the last guest the run booted, one per run (decision 13), and
/// the reason it can only be one is decision 16: a guest that is still
/// registered refuses the next boot, so a first target that kept its guest would
/// refuse the second target's build rather than leave two guests behind. Every
/// guest before the last one is therefore torn down whatever the flag says.
pub fn keeps_guest(options: Options, boot: Boot) -> bool {
    if !options.keep {
        return false;
    }
    let targets = options.which.targets();
    if targets.last() != Some(&boot.target) {
        return false;
    }
    // Inside the last target, the builder is the last guest only when nothing
    // follows it: either the verification boot was skipped, or the build failed
    // before it could happen.
    !boot.builder || !options.verify || boot.failed
}

/// The closing line about the one guest a run kept.
pub fn kept_summary(image: Image) -> String {
    format!(
        "the {} guest is still up, because --keep was given; \
         `cargo xtask vm down {image}` ends it",
        image.vm_name()
    )
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

/// The other render of the same scene, made against a directory with nothing in
/// it, which is the procedural grid by construction.
pub const GRID_FILE: &str = "grid.png";

/// The directory the grid render is pointed at, inside the guest.
pub const EMPTY_TEXTURES: &str = "empty-textures";

/// How far apart the two verification renders have to be before the bundle is
/// believed to have found its textures.
///
/// A render that failed to find them still produces a 640x360 PNG of the
/// procedural grid, so the header check cannot tell the two apart: this is what
/// can. The floor is far above the fraction of a channel step a second or two of
/// the Earth turning between the two renders accounts for, and far below the
/// difference measured live between a grid and a textured globe, which is in the
/// amendment's validation record.
pub const TEXTURE_LOOKUP_FLOOR: f64 = 8.0;

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

/// Where the archive is written on the host.
///
/// At the run directory's root rather than in a subdirectory of it, because the
/// root is what the inventory scans and what a teardown deletes: a file one
/// level deeper is invisible to `vm status` and survives `vm down`, and this one
/// is eight megabytes per target. It is run state in every other sense too, so
/// this is the shelf it belongs on rather than a place chosen to be seen.
pub fn archive_path(store: &Store, builder: Image) -> PathBuf {
    store.run_dir(builder).join("src.tar")
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

/// Where the guest keeps the build directory.
///
/// Decision 25: out of the source tree, so the `rm -rf src` and the fresh
/// extraction at the top of every job cannot touch it, the archive has one fixed
/// path on both operating systems, and the executable is copied from a path that
/// does not move with the source layout.
pub const GUEST_TARGET_DIR: &str = "cargo-target";

/// Where the job writes the archives the host pulls back out.
///
/// A directory of its own, so what the job packed can never be mistaken for
/// what the host copied in: an archive to restore sits at the guest root beside
/// the source archive, and one to save is written in here.
pub const GUEST_CACHE_OUT: &str = "cache";

/// Where the host copies a cache archive to, for the job to unpack.
pub fn guest_cache_in(target: Target, kind: cache::Kind) -> String {
    guest_join(target, provider::guest_root(target), &kind.archive())
}

/// Where the job writes one for the host to pull.
pub fn guest_cache_out(target: Target, kind: cache::Kind) -> String {
    let dir = guest_join(target, provider::guest_root(target), GUEST_CACHE_OUT);
    guest_join(target, &dir, &kind.archive())
}

/// What the build job does about the cache.
///
/// Decided on the host before the guest boots, so the script carries exactly the
/// clauses this run needs rather than a set of conditionals over files that may
/// or may not be there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheJob {
    /// The archives the host copied in, to unpack before the build.
    pub restore: Vec<cache::Kind>,
    /// The archives to pack afterwards, for the host to pull.
    pub save: Vec<cache::Kind>,
}

/// The file the job writes to say what it actually packed.
pub const CACHE_REPORT: &str = "cache.txt";

/// What every crate of this workspace's fingerprint directory is named after.
///
/// `sunlit-core` and `sunlit-earth` are the two a release build compiles, and
/// cargo files each unit's fingerprint under its own package name and a hash.
/// Nothing else in the build directory begins with this.
pub const WORKSPACE_FINGERPRINTS: &str = "sunlit-*";

/// What a `cache.txt` line says was packed.
const PACKED: &str = "packed ";

/// Which archives the guest says it wrote.
///
/// The guest's own statement rather than the host's intention: a pack can fail
/// on disk space after a build has already succeeded, and that must cost the
/// cache rather than the build.
pub fn parse_packed(text: &str) -> Vec<cache::Kind> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix(PACKED))
        .filter_map(|slug| {
            cache::Kind::ALL
                .into_iter()
                .find(|kind| kind.slug() == slug.trim())
        })
        .collect()
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
/// The source is extracted with `-m` and the cached trees deliberately without
/// it (decision 23). `git archive` stamps its entries with the commit's own
/// time, so rebuilding an older commit over a newer cache would otherwise
/// present cargo with sources older than the artifacts and produce a binary of
/// the previous commit under this commit's record. The cached trees keep the
/// times they were archived with, because their whole value is that nothing in
/// them looks newer than what was built from it.
///
/// That argument is about clocks, so it is not left to hold on its own: a
/// restored build directory gives up this workspace's own fingerprints and the
/// binary they link to before the build starts. A fingerprint that is not there
/// is a unit cargo rebuilds whatever the times say, so `sunlit-core` and
/// `sunlit-earth` are compiled from the extracted source in every warm build,
/// and what the cache serves is the dependency tree `--locked` pins. Deleting
/// the binary alone would not do it: cargo would notice the missing output and
/// relink it out of whatever rlibs it still believed in.
///
/// A restore that fails clears what it was writing into and the build goes on
/// cold, and a pack that fails costs the cache and not the build: by the time
/// either happens the binary is either not built yet or already in the artifacts
/// directory. The drop is the one step of the three that may end the build,
/// because it is the guarantee rather than the convenience, and a guarantee
/// that quietly did not happen is the binary of another commit. `set -e` is
/// what says so on Linux; the Windows job looks at the directories again,
/// since its `rmdir` runs in a loop whose errorlevel is its last iteration's.
///
/// Every cache step says what it cost, because the risk this cache was weighed
/// against is that moving a gigabyte costs more than the compiling it saves and
/// a whole-build total cannot be taken apart afterwards. The two jobs say it
/// differently. bash has `SECONDS`, a counter that costs nothing to read, so
/// the Linux job prints a duration. `cmd.exe` has no arithmetic on its own clock
/// that is not either a process spawn per reading or a bet on the locale's time
/// format, so the Windows job prints `%TIME%` on either side of each step and
/// leaves the subtraction to whoever reads the log: two readings that cannot be
/// wrong beat one number that can. The host times its own two copies itself and
/// puts them in the record.
///
/// The last two steps are what make the release claims checkable on the host:
/// the toolchain that built it, and the binary's own imports as the builder's
/// tools report them.
#[allow(clippy::too_many_lines)]
pub fn build_job(target: Target, pinned: &Toolchain, cache_job: &CacheJob) -> String {
    let channel = &pinned.channel;
    match target {
        Target::Linux => {
            let root = crate::provider::GUEST_ROOT_LINUX;
            let exe = exe_name(target);
            let mut script = format!(
                "#!/usr/bin/env bash\n\
                 set -euo pipefail\n\
                 root={root}\n\
                 export CARGO_NET_RETRY=5\n\
                 export CARGO_TERM_COLOR=never\n\
                 export CARGO_TARGET_DIR=\"$root/{GUEST_TARGET_DIR}\"\n\
                 cargo=\"$HOME/.cargo/bin/cargo\"\n\
                 rustc=\"$HOME/.cargo/bin/rustc\"\n\
                 rustup=\"$HOME/.cargo/bin/rustup\"\n\
                 \"$rustup\" toolchain install {channel} --profile minimal\n"
            );
            for kind in &cache_job.restore {
                let (into, clear) = match kind {
                    cache::Kind::Registry => (
                        "\"$HOME\"",
                        "\"$HOME/.cargo/registry\" \"$HOME/.cargo/git\"",
                    ),
                    cache::Kind::Target => ("\"$root\"", "\"$CARGO_TARGET_DIR\""),
                };
                let _ = write!(
                    script,
                    "echo 'cache: unpacking {label}'\n\
                     unpack_started=$SECONDS\n\
                     if tar -xf \"$root/{archive}\" -C {into}; then\n  \
                       echo \"cache: unpacked {label} in $((SECONDS - unpack_started))s\"\n\
                     else\n  \
                       echo 'cache: {label} did not unpack; building cold'\n  \
                       rm -rf {clear}\n\
                     fi\n",
                    label = kind.label(),
                    archive = kind.archive(),
                );
                if *kind == cache::Kind::Target {
                    let _ = write!(
                        script,
                        "echo 'cache: dropping this workspace out of the restored build directory'\n\
                         rm -rf \"$CARGO_TARGET_DIR/release/.fingerprint\"/{WORKSPACE_FINGERPRINTS} \
                         \"$CARGO_TARGET_DIR/release/{exe}\"\n"
                    );
                }
            }
            let _ = write!(
                script,
                "rm -rf \"$root/src\"\n\
                 tar -xmf \"$root/src.tar\" -C \"$root\"\n\
                 cd \"$root/src\"\n\
                 \"$cargo\" \"+{channel}\" build --release --locked -p sunlit-earth\n\
                 exe=\"$CARGO_TARGET_DIR/release/{exe}\"\n\
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
                 }} > \"$SUNLIT_E2E_ARTIFACTS/deps.txt\"\n"
            );
            if !cache_job.save.is_empty() {
                let _ = writeln!(script, "mkdir -p \"$root/{GUEST_CACHE_OUT}\"");
            }
            for kind in &cache_job.save {
                let out = format!("$root/{GUEST_CACHE_OUT}/{}", kind.archive());
                match kind {
                    cache::Kind::Registry => {
                        let _ = write!(
                            script,
                            "members=\"\"\n\
                             if [ -d \"$HOME/.cargo/registry\" ]; then members=\"$members .cargo/registry\"; fi\n\
                             if [ -d \"$HOME/.cargo/git\" ]; then members=\"$members .cargo/git\"; fi\n\
                             pack_started=$SECONDS\n\
                             if [ -n \"$members\" ] && tar --zstd -cf \"{out}\" -C \"$HOME\" $members; then\n  \
                               echo '{PACKED}{slug}' >> \"$SUNLIT_E2E_ARTIFACTS/{CACHE_REPORT}\"\n  \
                               echo \"cache: packed {label} in $((SECONDS - pack_started))s\"\n\
                             else\n  \
                               echo 'cache: could not pack {label}'\n\
                             fi\n",
                            slug = kind.slug(),
                            label = kind.label(),
                        );
                    }
                    cache::Kind::Target => {
                        let _ = write!(
                            script,
                            "pack_started=$SECONDS\n\
                             if tar --zstd -cf \"{out}\" -C \"$root\" {GUEST_TARGET_DIR}; then\n  \
                               echo '{PACKED}{slug}' >> \"$SUNLIT_E2E_ARTIFACTS/{CACHE_REPORT}\"\n  \
                               echo \"cache: packed {label} in $((SECONDS - pack_started))s\"\n\
                             else\n  \
                               echo 'cache: could not pack {label}'\n\
                             fi\n",
                            slug = kind.slug(),
                            label = kind.label(),
                        );
                    }
                }
            }
            let _ = writeln!(script, "ls -l \"$SUNLIT_E2E_ARTIFACTS\"");
            script
        }
        Target::Windows => {
            let root = crate::provider::GUEST_ROOT_WINDOWS;
            let exe = exe_name(target);
            let libclang = crate::commands::build_layer::LIBCLANG_DIR;
            let mut script = format!(
                "@echo off\r\n\
                 set ROOT={root}\r\n\
                 set CARGO_NET_RETRY=5\r\n\
                 set CARGO_TERM_COLOR=never\r\n\
                 set LIBCLANG_PATH={libclang}\r\n\
                 set CARGO_TARGET_DIR=%ROOT%\\{GUEST_TARGET_DIR}\r\n\
                 set CARGO=%USERPROFILE%\\.cargo\\bin\\cargo.exe\r\n\
                 set RUSTC=%USERPROFILE%\\.cargo\\bin\\rustc.exe\r\n\
                 set RUSTUP=%USERPROFILE%\\.cargo\\bin\\rustup.exe\r\n\
                 \"%RUSTUP%\" toolchain install {channel} --profile minimal || exit /b 1\r\n"
            );
            for kind in &cache_job.restore {
                // No parenthesized block: `cmd.exe` mishandles those, and a
                // label costs one line and is unambiguous.
                let into = match kind {
                    cache::Kind::Registry => "%USERPROFILE%",
                    cache::Kind::Target => "%ROOT%",
                };
                let _ = write!(
                    script,
                    "echo cache: unpacking {label} at %TIME%\r\n\
                     tar.exe -xf \"%ROOT%\\{archive}\" -C \"{into}\"\r\n\
                     if not errorlevel 1 goto cache_in_{slug}\r\n\
                     echo cache: {label} did not unpack; building cold\r\n",
                    label = kind.label(),
                    archive = kind.archive(),
                    slug = kind.slug(),
                );
                for path in match kind {
                    cache::Kind::Registry => {
                        vec![
                            r"%USERPROFILE%\.cargo\registry",
                            r"%USERPROFILE%\.cargo\git",
                        ]
                    }
                    cache::Kind::Target => vec!["%CARGO_TARGET_DIR%"],
                } {
                    let _ = write!(script, "rmdir /s /q \"{path}\"\r\n");
                }
                let _ = write!(
                    script,
                    ":cache_in_{slug}\r\n\
                     echo cache: unpacking {label} ended at %TIME%\r\n",
                    slug = kind.slug(),
                    label = kind.label(),
                );
                if *kind == cache::Kind::Target {
                    let fingerprints = format!(
                        r"%CARGO_TARGET_DIR%\release\.fingerprint\{WORKSPACE_FINGERPRINTS}"
                    );
                    let cached_exe = format!(r"%CARGO_TARGET_DIR%\release\{exe}");
                    // The one clause in the cache region that may end the
                    // build. Everything else here is a convenience, and a
                    // convenience that failed costs a slower build; this drop
                    // is a correctness guarantee, and one that quietly did not
                    // happen is worse than a build that stopped. What it looks
                    // at is the directories rather than an errorlevel, because
                    // the loop's is its last iteration's: a handle held on the
                    // first of two would be masked by the second deleting
                    // cleanly.
                    let _ = write!(
                        script,
                        "echo cache: dropping this workspace out of the restored build directory\r\n\
                         for /d %%d in (\"{fingerprints}\") do rmdir /s /q \"%%d\"\r\n\
                         if exist \"{cached_exe}\" del /f /q \"{cached_exe}\"\r\n\
                         set STALE=\r\n\
                         for /d %%d in (\"{fingerprints}\") do set STALE=%%d\r\n\
                         if exist \"{cached_exe}\" set STALE={cached_exe}\r\n\
                         if defined STALE echo cache: the restored build directory would not \
                         give up %STALE% & exit /b 1\r\n"
                    );
                }
            }
            let _ = write!(
                script,
                "if exist \"%ROOT%\\src\" rmdir /s /q \"%ROOT%\\src\"\r\n\
                 tar.exe -xmf \"%ROOT%\\src.tar\" -C \"%ROOT%\" || exit /b 1\r\n\
                 cd /d \"%ROOT%\\src\" || exit /b 1\r\n\
                 \"%CARGO%\" +{channel} build --release --locked -p sunlit-earth || exit /b 1\r\n\
                 set EXE=%CARGO_TARGET_DIR%\\release\\{exe}\r\n\
                 copy /y \"%EXE%\" \"%SUNLIT_E2E_ARTIFACTS%\\{exe}\" || exit /b 1\r\n\
                 \"%RUSTC%\" +{channel} -vV > \"%SUNLIT_E2E_ARTIFACTS%\\toolchain.txt\"\r\n\
                 \"%CARGO%\" +{channel} -V >> \"%SUNLIT_E2E_ARTIFACTS%\\toolchain.txt\"\r\n\
                 set VSWHERE=%ProgramFiles(x86)%\\Microsoft Visual Studio\\Installer\\vswhere.exe\r\n\
                 set DUMPBIN=\r\n\
                 for /f \"usebackq delims=\" %%i in (`\"%VSWHERE%\" -latest -products * \
                 -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 \
                 -find **\\Hostx64\\x64\\dumpbin.exe`) do set DUMPBIN=%%i\r\n\
                 if not defined DUMPBIN echo no dumpbin found & exit /b 1\r\n\
                 \"%DUMPBIN%\" /dependents \"%SUNLIT_E2E_ARTIFACTS%\\{exe}\" \
                 > \"%SUNLIT_E2E_ARTIFACTS%\\deps.txt\" || exit /b 1\r\n"
            );
            if !cache_job.save.is_empty() {
                let _ = write!(
                    script,
                    "if not exist \"%ROOT%\\{GUEST_CACHE_OUT}\" mkdir \"%ROOT%\\{GUEST_CACHE_OUT}\"\r\n"
                );
            }
            for kind in &cache_job.save {
                let out = format!("%ROOT%\\{GUEST_CACHE_OUT}\\{}", kind.archive());
                let _ = write!(
                    script,
                    "echo cache: packing {label} at %TIME%\r\n",
                    label = kind.label(),
                );
                // `&&` rather than a block, for the same reason as the label
                // above: a pack that failed costs the cache and not the build,
                // so nothing here may `exit /b`.
                match kind {
                    cache::Kind::Registry => {
                        let _ = write!(
                            script,
                            "set MEMBERS=.cargo/registry\r\n\
                             if exist \"%USERPROFILE%\\.cargo\\git\" set MEMBERS=%MEMBERS% .cargo/git\r\n\
                             tar.exe --zstd -cf \"{out}\" -C \"%USERPROFILE%\" %MEMBERS% \
                             && echo {PACKED}{slug}>> \"%SUNLIT_E2E_ARTIFACTS%\\{CACHE_REPORT}\"\r\n",
                            slug = kind.slug(),
                        );
                    }
                    cache::Kind::Target => {
                        let _ = write!(
                            script,
                            "tar.exe --zstd -cf \"{out}\" -C \"%ROOT%\" {GUEST_TARGET_DIR} \
                             && echo {PACKED}{slug}>> \"%SUNLIT_E2E_ARTIFACTS%\\{CACHE_REPORT}\"\r\n",
                            slug = kind.slug(),
                        );
                    }
                }
                let _ = write!(
                    script,
                    "echo cache: packing {label} ended at %TIME%\r\n",
                    label = kind.label(),
                );
            }
            let _ = write!(script, "dir \"%SUNLIT_E2E_ARTIFACTS%\"\r\nexit /b 0\r\n");
            script
        }
    }
}

/// What the verification boot is asked to prove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verification<'a> {
    /// The bundle, as a user would have it. Two renders of the same scene: one
    /// with `SUNLIT_EARTH_TEXTURES` pointed at a directory the job creates and
    /// leaves empty, which is the procedural grid by construction, and one with
    /// the variable unset, which is the lookup a user's machine does.
    Bundle {
        /// The binary inside the staged bundle.
        exe: &'a str,
        /// The empty directory, which the job makes and nothing fills.
        empty: &'a str,
    },
    /// The loose binary, which is what a run whose checkout holds Git LFS
    /// pointers falls back to (decision 33). There is no second render to
    /// compare against, because there are no textures to find, so this is the
    /// plan's own verification: one render, measured from its own header.
    Loose { exe: &'a str },
}

/// The verification job.
///
/// The same smoke test `ci.yml` runs, for the same reason and with the same
/// check on the result: it asserts the output is a PNG of the size asked for
/// rather than merely a file of non-trivial size. What the bundle adds is the
/// second render, because the first check cannot tell a globe from a grid.
///
/// The job changes directory to the guest root before it runs anything, and that
/// is load-bearing rather than tidiness: `resolve_textures_dir` tries a
/// working-directory-relative `textures` before it walks up from the executable,
/// so a job that ran from inside the bundle would answer with the first branch
/// and leave the walk-up, which is the one a bundle depends on, untested. The
/// guest root has no `textures/` in it, because a bundle run stages none of its
/// own and every boot is a fresh overlay of a golden disk.
pub fn verify_job(target: Target, verification: &Verification) -> String {
    match target {
        Target::Linux => {
            let mut script = format!(
                "#!/usr/bin/env bash\nset -uo pipefail\nexport RUST_BACKTRACE=1\ncd {root}\n",
                root = crate::provider::GUEST_ROOT_LINUX,
            );
            if let Verification::Bundle { empty, .. } = verification {
                let empty = artifacts::shell_quote(empty);
                let _ = write!(script, "rm -rf {empty}\nmkdir -p {empty}\n");
            }
            let exe = artifacts::shell_quote(match verification {
                Verification::Bundle { exe, .. } | Verification::Loose { exe } => exe,
            });
            let _ = writeln!(script, "{exe} --version || exit 1");
            if let Verification::Bundle { empty, .. } = verification {
                let _ = writeln!(
                    script,
                    "SUNLIT_EARTH_TEXTURES={empty} {exe} render \
                     --output \"$SUNLIT_E2E_ARTIFACTS/{GRID_FILE}\" \
                     --width {SMOKE_WIDTH} --height {SMOKE_HEIGHT} || exit 1",
                    empty = artifacts::shell_quote(empty),
                );
            }
            let _ = writeln!(
                script,
                "{exe} render --output \"$SUNLIT_E2E_ARTIFACTS/{SMOKE_FILE}\" \
                 --width {SMOKE_WIDTH} --height {SMOKE_HEIGHT} || exit 1"
            );
            script
        }
        Target::Windows => {
            let mut script = format!(
                "@echo off\r\nset RUST_BACKTRACE=1\r\ncd /d \"{root}\" || exit /b 1\r\n",
                root = crate::provider::GUEST_ROOT_WINDOWS,
            );
            if let Verification::Bundle { empty, .. } = verification {
                let _ = write!(
                    script,
                    "if exist \"{empty}\" rmdir /s /q \"{empty}\"\r\n\
                     mkdir \"{empty}\" || exit /b 1\r\n"
                );
            }
            let exe = match verification {
                Verification::Bundle { exe, .. } | Verification::Loose { exe } => exe,
            };
            let _ = write!(script, "\"{exe}\" --version || exit /b 1\r\n");
            if let Verification::Bundle { empty, .. } = verification {
                let _ = write!(
                    script,
                    "set SUNLIT_EARTH_TEXTURES={empty}\r\n\
                     \"{exe}\" render --output \"%SUNLIT_E2E_ARTIFACTS%\\{GRID_FILE}\" \
                     --width {SMOKE_WIDTH} --height {SMOKE_HEIGHT} || exit /b 1\r\n\
                     set SUNLIT_EARTH_TEXTURES=\r\n"
                );
            }
            let _ = write!(
                script,
                "\"{exe}\" render --output \"%SUNLIT_E2E_ARTIFACTS%\\{SMOKE_FILE}\" \
                 --width {SMOKE_WIDTH} --height {SMOKE_HEIGHT} || exit /b 1\r\n\
                 exit /b 0\r\n"
            );
            script
        }
    }
}

/// How far apart two renders of the same size are, as a mean channel difference
/// over every sample.
///
/// Decoded rather than compared byte for byte: two PNG encodings of the same
/// pixels can differ, and the question is whether the same scene was drawn from
/// the same data.
pub fn render_difference(grid: &[u8], bundled: &[u8]) -> Result<f64, String> {
    let decode = |bytes: &[u8], what: &str| -> Result<image::RgbaImage, String> {
        image::load_from_memory(bytes)
            .map(image::DynamicImage::into_rgba8)
            .map_err(|e| format!("the {what} render does not decode: {e}"))
    };
    let grid = decode(grid, "grid")?;
    let bundled = decode(bundled, "bundle's")?;
    if grid.dimensions() != bundled.dimensions() {
        return Err(format!(
            "the two renders are {:?} and {:?}, so they are not of the same scene",
            grid.dimensions(),
            bundled.dimensions()
        ));
    }
    let total: u64 = grid
        .as_raw()
        .iter()
        .zip(bundled.as_raw())
        .map(|(a, b)| u64::from(a.abs_diff(*b)))
        .sum();
    // Both numbers are far inside what an `f64` holds exactly: a 640x360 render
    // is 921,600 samples and the largest total any pair of them can reach is 255
    // times that.
    #[allow(clippy::cast_precision_loss)]
    let mean = total as f64 / grid.as_raw().len() as f64;
    Ok(mean)
}

/// What to say when the two renders are not far enough apart.
pub fn grid_refusal(delta: f64, results: &Path) -> String {
    format!(
        "the bundle's own render and one made against an empty textures directory \
         differ by {delta:.2} of a channel step, under the {TEXTURE_LOOKUP_FLOOR:.1} \
         this asks for: the binary did not find the `textures/` beside it, so the \
         bundle would ship a procedural grid under a name that promises a release.\n\
         Both renders are in {}, and it is `resolve_textures_dir` walking up from \
         the executable that the bundle's layout depends on.",
        results.display()
    )
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
///
/// `Eq` and not only `PartialEq` everywhere but here: the bundle's own section
/// carries a measured difference between two renders, and a float has no total
/// equality to derive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// What each half of the build cache did: restored and from when, or the
    /// one-line reason it was not, and whether this run wrote a fresh one back.
    /// Decision 27, which is what keeps decision 19's argument checkable after
    /// the fact rather than a claim in a document.
    #[serde(default)]
    pub cache: Vec<cache::Report>,
    /// The release bundle written beside the loose binary, when the host held
    /// the texture assets rather than Git LFS pointers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<BundleInfo>,
    /// The desktop image the binary was run in, or `null` where `--no-verify`
    /// skipped that boot.
    ///
    /// Written either way rather than left out, unlike every other optional
    /// field here. The record travels inside the bundle, so the person reading
    /// it is usually not the person who ran the command and saw the two lines
    /// that said so; a field that is not there reads as one the writer had no
    /// answer for, and "nobody ran this" is an answer.
    #[serde(default)]
    pub verified_in: Option<String>,
    pub xtask_version: String,
}

/// The bundle this run wrote, as its own record describes it.
///
/// The record travels inside the bundle as well as beside it, so it cannot
/// carry the finished archive's size: that is a number the archive would have to
/// contain about itself. What it carries instead is what the bundle is, and the
/// one measurement that says the textures in it are found rather than assumed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BundleInfo {
    /// The one directory an unpack produces, which is also the archive's stem.
    pub name: String,
    /// The archive's file name, beside the binary in the dist directory.
    pub archive: String,
    pub entries: usize,
    /// The mean channel difference between the bundle's own render and one made
    /// against an empty textures directory, or `null` where no boot rendered
    /// from this bundle at all. Present either way, for the reason
    /// `verified_in` is.
    #[serde(default)]
    pub texture_lookup_delta: Option<f64>,
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
pub fn run(runner: &dyn Runner, options: Options) -> Result<u8, String> {
    let pinned = crate::guest::toolchain::pinned()?;
    let store = store::store()?;
    let repo = store::repo_root();
    let git = git_facts(runner, &repo)?;
    if git.dirty && !options.allow_dirty {
        return Err(dirty_refusal());
    }

    let targets = options.which.targets();
    let mut summary = Vec::new();
    let mut failed = false;
    // The one guest the run is allowed to leave behind, recorded where it was
    // left rather than inferred afterwards: a target can fail before it boots
    // anything, and a summary claiming a guest that is not there is worse than
    // no summary at all.
    let mut kept = None;
    for target in targets {
        println!();
        println!("== {target}: a release build of {}", git.describe);
        match one_target(
            runner, &store, &repo, target, &pinned, &git, options, &mut kept,
        ) {
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
    if let Some(image) = kept {
        println!("{}", kept_summary(image));
    }
    Ok(u8::from(failed))
}

/// One target, end to end.
///
/// `kept` is where a guest this target left running records itself, which is the
/// one thing about a target that outlives it.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn one_target(
    runner: &dyn Runner,
    store: &Store,
    repo: &Path,
    target: Target,
    pinned: &Toolchain,
    git: &GitFacts,
    options: Options,
    kept: &mut Option<Image>,
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
    // Read before anything boots too, for the same reason: a bundle is named
    // after it, and a manifest that will not parse is a twenty-minute build
    // wasted on a name that cannot be chosen.
    let version = bundle::version(repo)?;

    let archive = archive_path(store, builder);
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

    let facts = cache_facts(pinned, builder, &builder_info, repo)?;
    let product = build_in_builder(
        runner,
        store,
        builder,
        pinned,
        &archive,
        options,
        kept,
        &facts,
        &git.commit,
    )?;
    let results = product.results;

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

    let mut info = BuildInfo {
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
        cache: product.cache,
        bundle: None,
        verified_in: None,
        xtask_version: env!("CARGO_PKG_VERSION").to_owned(),
    };

    // Everything from here writes into the bundle scratch, which is run state:
    // it belongs to the guest the bundle is staged into and is worth nothing
    // once this target is done, so it goes whichever way the rest went.
    let scratch = store.bundle_scratch(desktop);
    let _ = std::fs::remove_dir_all(&scratch);
    let outcome = (|| -> Result<String, String> {
        let bundled = assemble_bundle(repo, target, &version, &exe, &scratch, &mut info)?;

        let verified = if options.verify {
            Some(verify_in_desktop(
                runner,
                store,
                desktop,
                &exe,
                bundled.as_ref(),
                options,
                kept,
            )?)
        } else {
            println!("  skipping the verification boot, because --no-verify was given");
            None
        };

        info.verified_in = verified.as_ref().map(|_| desktop.slug().to_owned());
        if let (Some(bundle), Some(verified)) = (info.bundle.as_mut(), verified.as_ref()) {
            bundle.texture_lookup_delta = verified.delta;
        }
        info.duration_secs = started.elapsed().as_secs();

        // The record goes into the bundle only now, so that the copy inside it
        // and the copy beside it are one file: what verification found is part
        // of what a release binary was built from, and a bundle carrying a
        // record that stopped short of it would be the stale one of two.
        let archive = match &bundled {
            Some(bundled) => Some(seal_bundle(bundled, &version, target, &scratch, &info)?),
            None => None,
        };

        let dist = dist_dir(target);
        publish(
            &dist,
            &exe,
            &results,
            verified.as_ref().map(|v| v.smoke.as_path()),
            archive.as_deref(),
            &info,
        )?;

        println!();
        println!("{target}: {}", dist.join(exe_name(target)).display());
        println!("  built from {} in the {builder} image", git.describe);
        if let Some(archive) = &archive {
            println!(
                "{}",
                bundle::summary(&dist.join(file_name(archive)), verified.is_some())
            );
        }
        println!("  {}", runtime_requirements(target));
        Ok(format!(
            "{target}: built in {} and {}",
            util::format_duration(started.elapsed()),
            if verified.is_some() {
                format!("rendered in the {desktop} guest")
            } else {
                "not verified".to_owned()
            }
        ))
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    outcome
}

/// Assemble the bundle, unless this checkout holds Git LFS pointers.
///
/// Decision 33: where there is nothing to put in a `textures/` there is no
/// bundle to write and none to verify, so the run falls back to the plan's own
/// verification and one line says why. A bundle without the assets would render
/// the procedural grid under a name that promises a release, which is worse than
/// not writing one.
fn assemble_bundle(
    repo: &Path,
    target: Target,
    version: &str,
    exe: &Path,
    scratch: &Path,
    info: &mut BuildInfo,
) -> Result<Option<Bundled>, String> {
    let textures = match artifacts::textures_present(repo) {
        Ok(dir) => dir,
        Err(why) => {
            println!("  no bundle: {why}");
            println!("  {}", bundle::skipped_note());
            return Ok(None);
        }
    };
    let name = bundle::bundle_name(version, target);
    let items = bundle::layout(
        target,
        &bundle::Sources {
            repo,
            exe,
            textures: &textures,
            // Rewritten in place by `seal_bundle` once verification has said
            // what it found; written now so the directory that is staged into
            // the guest is the whole bundle rather than most of it.
            record: &info.to_json(),
        },
    );
    let root = bundle::assemble(scratch, &name, &items)?;
    info.bundle = Some(BundleInfo {
        name: name.clone(),
        archive: bundle::archive_name(version, target),
        entries: items.len(),
        texture_lookup_delta: None,
    });
    println!("  bundle:  {name}/, {}", util::count(items.len(), "file"));
    Ok(Some(Bundled { name, root, items }))
}

/// Write the final record into the assembled bundle, archive it, and read the
/// archive back with the crate that wrote it.
///
/// The read-back is the cheap half of proving the bundle: that the archive holds
/// exactly what the directory holds, at the sizes the directory has. The
/// expensive half is the two renders in the desktop guest.
fn seal_bundle(
    bundled: &Bundled,
    version: &str,
    target: Target,
    scratch: &Path,
    info: &BuildInfo,
) -> Result<PathBuf, String> {
    let record = bundled.root.join(bundle::RECORD);
    std::fs::write(&record, info.to_json())
        .map_err(|e| format!("cannot write {}: {e}", record.display()))?;

    let format = bundle::Format::of(target);
    let archive = scratch.join(bundle::archive_name(version, target));
    let bytes = bundle::write(
        format,
        &bundled.root,
        &bundled.name,
        &bundled.items,
        &archive,
    )?;
    let assembled = bundle::walk(&bundled.root)?;
    let archived = bundle::read_back(format, &archive)?;
    bundle::verify(&bundled.name, &assembled, &archived)?;
    println!(
        "  bundle:  {} ({}), {} read back and matched",
        archive
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        util::format_bytes(bytes),
        util::count(archived.len(), "file")
    );
    Ok(archive)
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

/// What this build is, for a cache sidecar to be compared against.
fn cache_facts(
    pinned: &Toolchain,
    builder: Image,
    info: &BuilderInfo,
    repo: &Path,
) -> Result<cache::Facts, String> {
    let lockfile = repo.join("Cargo.lock");
    let lockfile_hash = crate::store::hash::checksum_file(&lockfile)
        .map_err(|e| format!("cannot read {}: {e}", lockfile.display()))?;
    Ok(cache::Facts {
        channel: pinned.channel.clone(),
        image: builder.slug().to_owned(),
        template_hash: info.template_hash.clone(),
        image_built_utc: info.built_utc.clone(),
        lockfile_hash,
    })
}

/// Which halves of the cache this run may restore, and which it should save.
///
/// Decided before the guest boots, so the job script carries exactly the clauses
/// this run needs. A refusal names the field that moved rather than saying
/// nothing, because a cache that was silently not used and one that is not being
/// written at all read the same from here.
fn plan_cache(
    store: &Store,
    builder: Image,
    facts: &cache::Facts,
    enabled: bool,
) -> (CacheJob, Vec<cache::Report>) {
    let mut job = CacheJob::default();
    let mut reports = Vec::new();
    for kind in cache::Kind::ALL {
        let mut report = cache::Report {
            archive: kind.slug().to_owned(),
            restored: false,
            bytes: None,
            written_utc: None,
            reason: None,
            saved: false,
            copied_in_secs: None,
            copied_out_secs: None,
        };
        if !enabled {
            report.reason = Some("--no-cache was given".to_owned());
            reports.push(report);
            continue;
        }
        let sidecar = match cache::read_sidecar(&store.cache_sidecar(builder, kind)) {
            Ok(sidecar) => sidecar,
            Err(e) => {
                report.reason = Some(e);
                None
            }
        };
        match &sidecar {
            None => {
                report
                    .reason
                    .get_or_insert_with(|| "there is none for this image yet".to_owned());
            }
            Some(sidecar) => {
                if let Err(why) = cache::restorable(sidecar, facts) {
                    report.reason = Some(why);
                } else {
                    let archive = store.cache_archive(builder, kind);
                    match std::fs::metadata(&archive) {
                        Ok(meta) if meta.is_file() => {
                            job.restore.push(kind);
                            report.restored = true;
                            report.bytes = Some(meta.len());
                            report.written_utc = Some(sidecar.written_utc.clone());
                        }
                        _ => {
                            report.reason = Some(format!(
                                "its sidecar is here and {} is not",
                                archive.display()
                            ));
                        }
                    }
                }
            }
        }
        // Decision 26: `--locked` means an unchanged lockfile is an unchanged
        // registry, so an ordinary build has nothing new to send back for it.
        //
        // Only a sidecar this run actually restored can say that, which is what
        // the filter is: one that was refused describes an archive no future
        // build will read either, so skipping the save on its lockfile hash
        // would leave the registry dead until `Cargo.lock` happened to move.
        let worth = match kind {
            cache::Kind::Registry => cache::registry_worth_saving(
                sidecar.as_ref().filter(|_| report.restored),
                &facts.lockfile_hash,
            ),
            cache::Kind::Target => true,
        };
        if worth {
            job.save.push(kind);
        }
        reports.push(report);
    }
    (job, reports)
}

/// Pull one archive the guest packed, and write the sidecar that says what it
/// was made from.
///
/// Temp-then-rename under a name carrying the process id, the discipline
/// `assets::texture_cache` already uses, so an interrupted pull cannot leave a
/// truncated archive for the next build to read. The sidecar goes last and its
/// old copy goes first, so an interruption anywhere leaves an archive with no
/// sidecar, which the next run reads as no cache at all.
fn pull_cache(
    session: &vm::Session,
    store: &Store,
    builder: Image,
    kind: cache::Kind,
    facts: &cache::Facts,
    commit: &str,
) -> Result<Pulled, String> {
    let final_path = store.cache_archive(builder, kind);
    let temp = cache::temp_path(&final_path);
    let started = Instant::now();
    session.provider.copy_out(
        &session.state,
        &guest_cache_out(builder.target(), kind),
        &temp,
    )?;
    let copied_out_secs = started.elapsed().as_secs();
    let bytes = std::fs::metadata(&temp)
        .map(|meta| meta.len())
        .map_err(|e| format!("the guest packed {kind} and nothing came back: {e}"))?;
    let sidecar_path = store.cache_sidecar(builder, kind);
    let _ = std::fs::remove_file(&sidecar_path);
    cache::commit(&temp, &final_path)?;
    let now = util::now_unix();
    cache::write_sidecar(
        &sidecar_path,
        &cache::Sidecar {
            format_version: cache::FORMAT_VERSION,
            archive: kind.archive(),
            bytes,
            channel: facts.channel.clone(),
            image: facts.image.clone(),
            template_hash: facts.template_hash.clone(),
            image_built_utc: facts.image_built_utc.clone(),
            lockfile_hash: facts.lockfile_hash.clone(),
            commit: commit.to_owned(),
            written_utc: util::format_unix_utc(now),
            written_unix: now,
        },
    )?;
    Ok(Pulled {
        bytes,
        copied_out_secs,
    })
}

/// What one archive cost to bring back: its size, and how long it took to
/// cross.
struct Pulled {
    bytes: u64,
    copied_out_secs: u64,
}

/// What the builder guest produced.
struct BuildProduct {
    results: PathBuf,
    cache: Vec<cache::Report>,
}

/// Boot the builder, run the build, bring the results back, and take it down.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn build_in_builder(
    runner: &dyn Runner,
    store: &Store,
    builder: Image,
    pinned: &Toolchain,
    archive: &Path,
    options: Options,
    kept: &mut Option<Image>,
    facts: &cache::Facts,
    commit: &str,
) -> Result<BuildProduct, String> {
    let target = builder.target();
    let (mut plan, mut reports) = plan_cache(store, builder, facts, options.cache);
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
    let outcome = (|| -> Result<BuildProduct, String> {
        let probe = session
            .provider
            .exec(&session.state, &toolchain_probe(target))?;
        if !probe.stdout.contains(TOOLCHAIN_MARKER) {
            return Err(missing_toolchain(builder));
        }

        session
            .provider
            .copy_in(&session.state, archive, &guest_archive(target))?;
        // The guest has it now, so the host's copy is eight megabytes of a
        // commit that `git archive` reproduces in a second. A copy that failed
        // leaves it where `vm status` counts it and `vm down` removes it.
        let _ = std::fs::remove_file(archive);

        // A copy that failed is a cold build rather than a failed one, so the
        // job is generated from what actually reached the guest.
        plan.restore.retain(|kind| {
            let from = store.cache_archive(builder, *kind);
            let started = Instant::now();
            match session
                .provider
                .copy_in(&session.state, &from, &guest_cache_in(target, *kind))
            {
                Ok(()) => {
                    if let Some(report) = reports.iter_mut().find(|r| r.archive == kind.slug()) {
                        report.copied_in_secs = Some(started.elapsed().as_secs());
                    }
                    true
                }
                Err(e) => {
                    println!("warning: {e}");
                    if let Some(report) = reports.iter_mut().find(|r| r.archive == kind.slug()) {
                        report.restored = false;
                        report.bytes = None;
                        report.written_utc = None;
                        report.reason = Some("it could not be copied into the guest".to_owned());
                    }
                    false
                }
            }
        });
        for report in &reports {
            println!("{}", report.line());
        }

        println!("  building; cargo's own output follows");
        let scratch = store.job_scratch(builder);
        let mut tail = OutputTail::new();
        let code = job::run_watching(
            session.provider.as_ref(),
            &session.state,
            target,
            &build_job(target, pinned, &plan),
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
            // Decision 26: a failed build's tree is not saved, because a build
            // that failed because of what was in its cache would otherwise make
            // that failure stick.
            return Err(format!(
                "the build exited {code} inside the guest; its output is above and \
                 {} has what it wrote",
                results.display()
            ));
        }

        let packed = parse_packed(
            &std::fs::read_to_string(results.join("artifacts").join(CACHE_REPORT))
                .unwrap_or_default(),
        );
        for kind in packed {
            match pull_cache(&session, store, builder, kind, facts, commit) {
                Ok(pulled) => {
                    println!(
                        "  cache:   {kind}: saved {}, {} to come back",
                        util::format_bytes(pulled.bytes),
                        util::format_duration(Duration::from_secs(pulled.copied_out_secs))
                    );
                    if let Some(report) = reports.iter_mut().find(|r| r.archive == kind.slug()) {
                        report.saved = true;
                        report.copied_out_secs = Some(pulled.copied_out_secs);
                    }
                }
                // The build is done and its binary is in the results: a cache
                // that could not be brought back costs the next build's warmth
                // and nothing else.
                Err(e) => println!("warning: the {kind} cache was not saved: {e}"),
            }
        }
        Ok(BuildProduct {
            results,
            cache: std::mem::take(&mut reports),
        })
    })();

    // The builder is torn down whichever way it went, unless this is the last
    // guest the whole run boots.
    let keep = keeps_guest(
        options,
        Boot {
            target,
            builder: true,
            failed: outcome.is_err(),
        },
    );
    if keep {
        *kept = Some(builder);
    }
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
                        staged: vm::Staging::skipped_for(
                            store,
                            provider::target::HostOs::current(),
                            target
                        ),
                        console: session.provider.kind(),
                        enhanced_session: enhanced,
                    }
                )
            );
            println!("{}", kept_builder_note(builder));
        }
        Err(_) => println!("{}", vm::after_failure(&mut session, store, keep)),
    }
    outcome
}

/// What is in a builder that was kept.
pub fn kept_builder_note(builder: Image) -> String {
    format!(
        "The source tree it built is in the guest's own root, with \
         `{GUEST_TARGET_DIR}/release` beside it rather than inside it, so a build \
         can be repeated in there by hand. `cargo xtask vm down {builder}` ends it \
         and takes the overlay with it."
    )
}

/// What the verification boot found.
#[derive(Debug, Clone)]
struct Verified {
    /// The render that goes into the dist directory: the bundle's own, when
    /// there was a bundle.
    smoke: PathBuf,
    /// How far it is from the render made against an empty textures directory,
    /// when there was a bundle to compare.
    delta: Option<f64>,
}

/// Boot the desktop image, stage what is to be proved, render, and take it down.
#[allow(clippy::too_many_lines)]
fn verify_in_desktop(
    runner: &dyn Runner,
    store: &Store,
    desktop: Image,
    exe: &Path,
    bundled: Option<&Bundled>,
    options: Options,
    kept: &mut Option<Image>,
) -> Result<Verified, String> {
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

    let outcome = (|| -> Result<Verified, String> {
        let root = provider::guest_root(target);
        let empty = guest_join(target, root, EMPTY_TEXTURES);
        let guest_exe = if let Some(bundled) = bundled {
            println!("  staging the bundle, and nothing else");
            session
                .provider
                .copy_in(&session.state, &bundled.root, &format!("{root}/"))?;
            let staged = guest_join(target, root, &bundled.name);
            let guest_exe = guest_join(target, &staged, exe_name(target));
            if target == Target::Linux {
                // scp carries a file's mode, and the tarball's header is what a
                // user's own unpack reads; this is belt and braces over a
                // directory copy rather than a fix for something observed.
                let _ = session.provider.exec(
                    &session.state,
                    &format!("chmod +x {guest_exe} {staged}/assets/linux/install-user.sh"),
                );
            }
            guest_exe
        } else {
            let bin_dir = provider::guest_bin(target);
            session.provider.copy_in(&session.state, exe, &bin_dir)?;
            let guest_exe = guest_join(target, &bin_dir, exe_name(target));
            if target == Target::Linux {
                let _ = session
                    .provider
                    .exec(&session.state, &format!("chmod +x {guest_exe}"));
            }
            guest_exe
        };
        let verification = if bundled.is_some() {
            Verification::Bundle {
                exe: &guest_exe,
                empty: &empty,
            }
        } else {
            Verification::Loose { exe: &guest_exe }
        };

        let scratch = store.job_scratch(desktop);
        let code = job::run(
            session.provider.as_ref(),
            &session.state,
            target,
            &verify_job(target, &verification),
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
        let bytes = read_render(&smoke)?;
        println!(
            "  rendered {SMOKE_WIDTH}x{SMOKE_HEIGHT}, {}",
            util::format_bytes(bytes.len() as u64)
        );

        // Decision 32: a render that never found the textures still produces a
        // 640x360 PNG, so the header check above cannot tell a globe from a
        // grid. This can, and it is what makes the bundle's own texture lookup
        // a tested claim rather than an assumed one.
        let delta = if bundled.is_some() {
            let grid = read_render(&results.join("artifacts").join(GRID_FILE))?;
            let delta = render_difference(&grid, &bytes)?;
            if delta < TEXTURE_LOOKUP_FLOOR {
                return Err(grid_refusal(delta, &results.join("artifacts")));
            }
            println!(
                "  the bundle's own render is {delta:.1} of a channel step from the \
                 grid, so its textures were found"
            );
            Some(delta)
        } else {
            None
        };
        Ok(Verified { smoke, delta })
    })();

    let keep = keeps_guest(
        options,
        Boot {
            target,
            builder: false,
            failed: outcome.is_err(),
        },
    );
    if keep {
        *kept = Some(desktop);
    }
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
                        staged: vm::Staging::skipped_for(
                            store,
                            provider::target::HostOs::current(),
                            target
                        ),
                        console: session.provider.kind(),
                        enhanced_session: enhanced,
                    }
                )
            );
            println!("{}", kept_desktop_note(bundled.is_some()));
        }
        Err(_) => println!("{}", vm::after_failure(&mut session, store, keep)),
    }
    outcome
}

/// What a kept verification guest has in it.
pub fn kept_desktop_note(bundled: bool) -> String {
    if bundled {
        "The bundle this run wrote is unpacked in the guest's own root, which is \
         what the render was made from."
            .to_owned()
    } else {
        "The binary this run built is in the guest's own bin directory.".to_owned()
    }
}

/// Read one render back and check its header says what was asked for.
///
/// A `render` that failed after opening its output still leaves a file, so the
/// existence of one is not evidence that anything was drawn.
fn read_render(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("the render brought back no {}: {e}", path.display()))?;
    match png_size(&bytes) {
        Some((SMOKE_WIDTH, SMOKE_HEIGHT)) => Ok(bytes),
        Some((width, height)) => Err(format!(
            "{} is {width}x{height}, not the {SMOKE_WIDTH}x{SMOKE_HEIGHT} it was \
             asked for",
            path.display()
        )),
        None => Err(format!(
            "{} is not a PNG, so nothing was drawn",
            path.display()
        )),
    }
}

/// Join two guest path components with the separator that guest's shell wants.
pub fn guest_join(target: Target, base: &str, leaf: &str) -> String {
    match target {
        Target::Windows => format!(r"{base}\{leaf}"),
        Target::Linux => format!("{base}/{leaf}"),
    }
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
    archive: Option<&Path>,
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
    // The bundle moves in rather than being written here, because the scratch it
    // was assembled in is run state and this directory is the artifact.
    if let Some(archive) = archive {
        copy(archive, &dist.join(file_name(archive)))?;
    }
    std::fs::write(dist.join("build-info.json"), info.to_json())
        .map_err(|e| format!("cannot write build-info.json: {e}"))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
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

    fn options(which: Which, keep: bool, verify: bool) -> Options {
        Options {
            which,
            keep,
            verify,
            cache: true,
            allow_expired: false,
            allow_dirty: false,
        }
    }

    fn boot(target: Target, builder: bool) -> Boot {
        Boot {
            target,
            builder,
            failed: false,
        }
    }

    /// Decision 13's `--keep` keeps the last guest the run booted, singular,
    /// and decision 16 is why it cannot be more: the next boot is refused while
    /// another guest is registered, so a first target that kept its builder
    /// would refuse the second target's build rather than leave two guests
    /// behind.
    #[test]
    fn a_run_keeps_the_last_guest_it_booted_and_nothing_before_it() {
        // The four boots of `dist --keep`, in the order the run makes them.
        let all = options(Which::All, true, true);
        assert!(!keeps_guest(all, boot(Target::Windows, true)));
        assert!(!keeps_guest(all, boot(Target::Windows, false)));
        assert!(!keeps_guest(all, boot(Target::Linux, true)));
        assert!(keeps_guest(all, boot(Target::Linux, false)));

        // With no verification boot the builder is the last guest of a target,
        // and still only of the run's last target.
        let bare = options(Which::All, true, false);
        assert!(!keeps_guest(bare, boot(Target::Windows, true)));
        assert!(keeps_guest(bare, boot(Target::Linux, true)));

        // A build that failed booted nothing after itself, which makes its
        // builder the last guest of that target and of nothing else.
        let failed = |target, builder| Boot {
            target,
            builder,
            failed: true,
        };
        assert!(!keeps_guest(all, failed(Target::Windows, true)));
        assert!(keeps_guest(all, failed(Target::Linux, true)));

        // A run of one target: its only target is its last one.
        let one = options(Which::Windows, true, true);
        assert!(!keeps_guest(one, boot(Target::Windows, true)));
        assert!(keeps_guest(one, boot(Target::Windows, false)));

        // Without the flag every guest goes, whichever boot it came from.
        for which in [Which::All, Which::Windows, Which::Linux] {
            for target in Target::ALL {
                for builder in [true, false] {
                    let without = options(which, false, true);
                    assert!(!keeps_guest(without, boot(target, builder)));
                }
            }
        }
    }

    #[test]
    fn the_summary_names_the_one_guest_that_was_kept() {
        let text = kept_summary(Image::Linux);
        assert!(text.contains(&Image::Linux.vm_name()), "{text}");
        assert!(text.contains("vm down linux"), "{text}");
        assert!(text.contains("--keep"), "{text}");
    }

    /// `vm status` counts the files in a run directory and `vm down` deletes
    /// them, and neither looks a level deeper: an archive in a subdirectory of
    /// one is eight megabytes that nothing accounts for and nothing removes.
    #[test]
    fn the_source_archive_sits_where_the_inventory_and_the_teardown_look() {
        let store = Store::new("/srv/vm");
        for target in Target::ALL {
            let builder = Image::builder(target);
            let archive = archive_path(&store, builder);
            assert_eq!(archive.parent(), Some(store.run_dir(builder).as_path()));
            assert_eq!(
                archive.file_name().and_then(|n| n.to_str()),
                Some("src.tar")
            );
        }
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

        let linux = build_job(Target::Linux, &pinned, &CacheJob::default());
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

        let windows = build_job(Target::Windows, &pinned, &CacheJob::default());
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
        // In a batch file the loop variable is doubled, and doubled exactly: a
        // single % is read as an argument reference, and cmd refuses %%%i and
        // %%%%i outright with "cannot be processed syntactically". `contains`
        // alone cannot tell the three apart.
        assert!(windows.contains("%%i"), "{windows}");
        assert!(!windows.contains("%%%"), "{windows}");
    }

    /// Decision 23, which is the cache's whole correctness argument. `git
    /// archive` stamps its entries with the commit's own time, so a source tree
    /// extracted over a newer cache without `-m` would present cargo with
    /// sources older than the artifacts and produce a binary of the previous
    /// commit under this commit's record. The cached trees keep the times they
    /// were archived with, because their value is that nothing in them looks
    /// newer than what was built from it.
    #[test]
    fn the_source_is_stamped_at_extraction_and_the_cache_is_not() {
        let pinned = pin();
        let warm = CacheJob {
            restore: cache::Kind::ALL.to_vec(),
            save: Vec::new(),
        };
        for target in Target::ALL {
            let job = build_job(target, &pinned, &warm);
            assert!(job.contains("-xmf"), "{target}: {job}");
            assert_eq!(job.matches("-xmf").count(), 1, "{target}: {job}");
            assert!(job.contains("src.tar"), "{target}: {job}");
            // Both cache archives are extracted, and neither with -m.
            for kind in cache::Kind::ALL {
                let line = job
                    .lines()
                    .find(|line| line.contains(&kind.archive()) && line.contains("-xf"))
                    .unwrap_or_else(|| panic!("{target}: no restore of {kind} in {job}"));
                assert!(!line.contains("-xmf"), "{target}: {line}");
            }
        }
    }

    fn pin() -> Toolchain {
        crate::guest::toolchain::parse("[toolchain]\nchannel = \"1.94.0\"\n").expect("parses")
    }

    /// Step 5 asked for the pack and unpack times, and the risk they settle is
    /// that moving a gigabyte costs more than the compiling it saves. A
    /// whole-build total cannot be taken apart afterwards, so each step says
    /// what it cost as it happens: a duration on Linux, where bash has a free
    /// counter, and a reading of the clock on either side of the step on
    /// Windows, where computing the difference would cost a process spawn or a
    /// bet on the locale's time format.
    #[test]
    fn every_cache_step_says_what_it_cost() {
        let pinned = pin();
        let both = CacheJob {
            restore: cache::Kind::ALL.to_vec(),
            save: cache::Kind::ALL.to_vec(),
        };

        let linux = build_job(Target::Linux, &pinned, &both);
        assert_eq!(
            linux.matches("$((SECONDS - unpack_started))s").count(),
            cache::Kind::ALL.len(),
            "{linux}"
        );
        assert_eq!(
            linux.matches("$((SECONDS - pack_started))s").count(),
            cache::Kind::ALL.len(),
            "{linux}"
        );

        // Two readings per step, one on either side of it.
        let windows = build_job(Target::Windows, &pinned, &both);
        assert_eq!(
            windows.matches("%TIME%").count(),
            cache::Kind::ALL.len() * 4,
            "{windows}"
        );

        for target in Target::ALL {
            let cold = build_job(target, &pinned, &CacheJob::default());
            assert!(!cold.contains("SECONDS"), "{target}: {cold}");
            assert!(!cold.contains("%TIME%"), "{target}: {cold}");
        }
    }

    /// The second line of defence under decision 23, which is the one thing
    /// this design must never get wrong: a binary of the previous commit under
    /// this commit's record. `-m` makes the extracted source newer than the
    /// restored artifacts, and that is an argument about the guest's clock. So
    /// a restored build directory also gives up this workspace's fingerprints,
    /// which makes cargo rebuild those crates whatever the times say, and the
    /// binary that would otherwise be copied out unchanged.
    #[test]
    fn a_restored_build_directory_gives_up_this_workspace() {
        let pinned = pin();
        for target in Target::ALL {
            let warm = build_job(
                target,
                &pinned,
                &CacheJob {
                    restore: cache::Kind::ALL.to_vec(),
                    save: Vec::new(),
                },
            );
            let drop_at = warm
                .find(WORKSPACE_FINGERPRINTS)
                .unwrap_or_else(|| panic!("{target}: nothing drops the fingerprints: {warm}"));
            let unpack = warm
                .find(&cache::Kind::Target.archive())
                .expect("the target archive is unpacked");
            let build = warm
                .find("build --release")
                .expect("the build is in the job");
            assert!(drop_at > unpack, "{target}: {warm}");
            assert!(drop_at < build, "{target}: {warm}");
            // The exe the cache holds goes with them, so a link that did not
            // happen cannot be copied out as this commit's.
            let cached_exe = warm
                .lines()
                .filter(|line| line.contains(exe_name(target)))
                .any(|line| line.contains("rm -rf") || line.contains("del /f /q"));
            assert!(cached_exe, "{target}: {warm}");

            // And none of it is in a job that restored nothing to drop.
            let cold = build_job(target, &pinned, &CacheJob::default());
            assert!(!cold.contains(WORKSPACE_FINGERPRINTS), "{target}: {cold}");
            let registry_only = build_job(
                target,
                &pinned,
                &CacheJob {
                    restore: vec![cache::Kind::Registry],
                    save: Vec::new(),
                },
            );
            assert!(
                !registry_only.contains(WORKSPACE_FINGERPRINTS),
                "{target}: {registry_only}"
            );
        }
    }

    /// And the drop is loud on both targets, which is the difference between a
    /// guarantee and a convenience: a fingerprint a handle was still held on is
    /// a unit cargo may call fresh, and the whole of decision 23 rests on it
    /// not being there. Linux gets that from `set -e`. The Windows job cannot,
    /// because its `rmdir` runs in a loop whose errorlevel is its last
    /// iteration's, so it looks at the directories again and refuses the build
    /// while one of them is there.
    #[test]
    fn a_drop_that_did_not_happen_ends_the_build() {
        let pinned = pin();
        let warm = |target| {
            build_job(
                target,
                &pinned,
                &CacheJob {
                    restore: cache::Kind::ALL.to_vec(),
                    save: Vec::new(),
                },
            )
        };

        let linux = warm(Target::Linux);
        assert!(linux.contains("set -euo pipefail"), "{linux}");
        let dropped = linux
            .lines()
            .find(|line| line.contains(WORKSPACE_FINGERPRINTS))
            .expect("the fingerprints are dropped");
        assert!(dropped.trim_start().starts_with("rm -rf"), "{dropped}");
        assert!(!dropped.contains("||"), "{dropped}");

        let windows = warm(Target::Windows);
        let build = windows
            .find("build --release")
            .expect("the build is in the job");
        let head = &windows[..build];
        let drop_at = head
            .find(r"do rmdir /s /q")
            .expect("the fingerprints are dropped");
        let looked_again = head[drop_at..]
            .find(WORKSPACE_FINGERPRINTS)
            .expect("nothing looks at the directories again");
        let refused = head[drop_at..]
            .find("exit /b 1")
            .expect("a drop that did not happen goes unnoticed");
        assert!(looked_again < refused, "{windows}");
        assert!(
            head[drop_at..][..refused].contains(exe_name(Target::Windows)),
            "{windows}"
        );
    }

    /// The job carries exactly the clauses this run needs, because the host
    /// decides before the guest boots. A cold build's script mentions the cache
    /// nowhere at all, which is what makes `--no-cache` the original goal 4
    /// rather than a flag that skips a step.
    #[test]
    fn the_build_job_carries_only_the_cache_clauses_this_run_needs() {
        let pinned = pin();
        for target in Target::ALL {
            let cold = build_job(target, &pinned, &CacheJob::default());
            assert!(!cold.contains(".tar.zst"), "{target}: {cold}");
            assert!(!cold.contains(GUEST_CACHE_OUT), "{target}: {cold}");
            assert!(!cold.contains(CACHE_REPORT), "{target}: {cold}");
            // The build directory moves out of the source tree whether or not
            // there is a cache: decision 25 is about where the archive's one
            // fixed path is, not about whether one is being made.
            assert!(cold.contains(GUEST_TARGET_DIR), "{target}: {cold}");
            assert!(!cold.contains("src/target/release"), "{target}: {cold}");
            assert!(!cold.contains(r"src\target\release"), "{target}: {cold}");

            let restore_only = build_job(
                target,
                &pinned,
                &CacheJob {
                    restore: vec![cache::Kind::Target],
                    save: Vec::new(),
                },
            );
            assert!(
                restore_only.contains(&cache::Kind::Target.archive()),
                "{target}: {restore_only}"
            );
            assert!(
                !restore_only.contains(&cache::Kind::Registry.archive()),
                "{target}: {restore_only}"
            );
            assert!(
                !restore_only.contains(CACHE_REPORT),
                "{target}: {restore_only}"
            );

            let save_only = build_job(
                target,
                &pinned,
                &CacheJob {
                    restore: Vec::new(),
                    save: vec![cache::Kind::Registry],
                },
            );
            assert!(save_only.contains(CACHE_REPORT), "{target}: {save_only}");
            assert!(save_only.contains("--zstd -cf"), "{target}: {save_only}");
            // A pack that failed costs the cache and not the build, so no line
            // that packs may end the job.
            let packing: Vec<&str> = save_only
                .lines()
                .filter(|line| line.contains("--zstd -cf"))
                .collect();
            assert_eq!(packing.len(), 1, "{target}: {save_only}");
            assert!(!packing[0].contains("exit"), "{target}: {}", packing[0]);
        }

        // A batch file doubles its loop variable, and doubles it exactly.
        let windows = build_job(
            Target::Windows,
            &pinned,
            &CacheJob {
                restore: cache::Kind::ALL.to_vec(),
                save: cache::Kind::ALL.to_vec(),
            },
        );
        assert!(windows.contains("%%i"), "{windows}");
        assert!(!windows.contains("%%%"), "{windows}");
        // And `cmd.exe` mishandles parenthesized blocks with the wrong line
        // endings, so the restore clauses use labels instead of blocks.
        assert!(!windows.contains(") else ("), "{windows}");
        for kind in cache::Kind::ALL {
            assert!(
                windows.contains(&format!(":cache_in_{}", kind.slug())),
                "{windows}"
            );
        }
    }

    /// What the host pulls is what the guest says it packed, not what the host
    /// asked for: a pack can fail on disk space after the build has already
    /// succeeded, and that must cost the cache rather than the build.
    #[test]
    fn the_host_pulls_what_the_guest_says_it_packed() {
        assert_eq!(parse_packed(""), Vec::new());
        assert_eq!(parse_packed("packed target\n"), vec![cache::Kind::Target]);
        assert_eq!(
            parse_packed("packed registry\npacked target\n"),
            vec![cache::Kind::Registry, cache::Kind::Target]
        );
        // The job's own chatter is not a claim that anything was packed.
        assert_eq!(
            parse_packed("cache: could not pack the build directory\npacked registry\n"),
            vec![cache::Kind::Registry]
        );
        assert_eq!(parse_packed("packed everything\n"), Vec::new());
    }

    /// An archive on its way in and one on its way out cannot be confused for
    /// each other, because the guest reads one and writes the other.
    #[test]
    fn a_restored_archive_and_a_packed_one_sit_in_different_places() {
        for target in Target::ALL {
            for kind in cache::Kind::ALL {
                let inbound = guest_cache_in(target, kind);
                let outbound = guest_cache_out(target, kind);
                assert_ne!(inbound, outbound, "{target} {kind}");
                assert!(outbound.contains(GUEST_CACHE_OUT), "{outbound}");
                assert!(!inbound.contains(GUEST_CACHE_OUT), "{inbound}");
                assert!(inbound.ends_with(&kind.archive()), "{inbound}");
                assert!(outbound.ends_with(&kind.archive()), "{outbound}");
            }
        }
    }

    /// Decisions 24 and 26 as the run applies them, over a store on disk: what
    /// is restored, what is refused and why, and what is worth sending back.
    #[test]
    fn the_plan_restores_what_matches_and_names_what_moved() {
        let dir = std::env::temp_dir().join("sunlit_xtask_dist_plan_cache");
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::new(&dir);
        let builder = Image::LinuxBuilder;
        let facts = cache::Facts {
            channel: "1.94.0".to_owned(),
            image: builder.slug().to_owned(),
            template_hash: "crc32:1a2b3c4d".to_owned(),
            image_built_utc: "2026-08-28T09:00:00Z".to_owned(),
            lockfile_hash: "crc32:deadbeef".to_owned(),
        };

        // Nothing on disk: both halves cold, both worth saving, and the reason
        // says so rather than saying nothing.
        let (job, reports) = plan_cache(&store, builder, &facts, true);
        assert!(job.restore.is_empty(), "{job:?}");
        assert_eq!(job.save, cache::Kind::ALL.to_vec());
        for report in &reports {
            assert!(!report.restored);
            assert!(report.reason.as_ref().is_some_and(|r| r.contains("none")));
        }

        // Both halves present and matching: both restored, and the registry is
        // not repacked because the lockfile did not move.
        for kind in cache::Kind::ALL {
            std::fs::create_dir_all(store.cache_dir(builder)).expect("mkdir");
            std::fs::write(store.cache_archive(builder, kind), b"archive").expect("write");
            cache::write_sidecar(
                &store.cache_sidecar(builder, kind),
                &cache::Sidecar {
                    format_version: cache::FORMAT_VERSION,
                    archive: kind.archive(),
                    bytes: 7,
                    channel: facts.channel.clone(),
                    image: facts.image.clone(),
                    template_hash: facts.template_hash.clone(),
                    image_built_utc: facts.image_built_utc.clone(),
                    lockfile_hash: facts.lockfile_hash.clone(),
                    commit: "abc".to_owned(),
                    written_utc: "2026-08-29T09:00:00Z".to_owned(),
                    written_unix: 1,
                },
            )
            .expect("sidecar");
        }
        let (job, reports) = plan_cache(&store, builder, &facts, true);
        assert_eq!(job.restore, cache::Kind::ALL.to_vec());
        assert_eq!(job.save, vec![cache::Kind::Target]);
        assert!(reports.iter().all(|r| r.restored), "{reports:?}");
        assert!(
            reports
                .iter()
                .all(|r| r.written_utc.as_deref() == Some("2026-08-29T09:00:00Z")),
            "{reports:?}"
        );

        // A lockfile that moved is not a refusal, it is a reason to repack.
        let mut moved_lock = facts.clone();
        moved_lock.lockfile_hash = "crc32:00000000".to_owned();
        let (job, _) = plan_cache(&store, builder, &moved_lock, true);
        assert_eq!(job.restore, cache::Kind::ALL.to_vec());
        assert_eq!(job.save, cache::Kind::ALL.to_vec());

        // A channel that moved is, and the line names it. Both halves are then
        // worth packing, the registry included: its recorded lockfile hash
        // still matches, but the archive that hash describes is one this build
        // refused and every later build will refuse too, so reading it as "the
        // host already has this" would leave the registry cold for good.
        let mut moved_channel = facts.clone();
        moved_channel.channel = "1.95.0".to_owned();
        let (job, reports) = plan_cache(&store, builder, &moved_channel, true);
        assert!(job.restore.is_empty(), "{job:?}");
        assert_eq!(job.save, cache::Kind::ALL.to_vec());
        for report in &reports {
            assert!(
                report.reason.as_ref().is_some_and(|r| r.contains("1.95.0")),
                "{report:?}"
            );
        }

        // A sidecar with no archive beside it is a cache that is not there.
        std::fs::remove_file(store.cache_archive(builder, cache::Kind::Target)).expect("rm");
        let (job, reports) = plan_cache(&store, builder, &facts, true);
        assert_eq!(job.restore, vec![cache::Kind::Registry]);
        let target = reports
            .iter()
            .find(|r| r.archive == cache::Kind::Target.slug())
            .expect("a target report");
        assert!(
            target.reason.as_ref().is_some_and(|r| r.contains("is not")),
            "{target:?}"
        );

        // `--no-cache` restores nothing and saves nothing, and the record says
        // which of the two reasons it was.
        let (job, reports) = plan_cache(&store, builder, &facts, false);
        assert_eq!(job, CacheJob::default());
        for report in &reports {
            assert_eq!(report.reason.as_deref(), Some("--no-cache was given"));
            assert!(!report.restored && !report.saved);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A sidecar nothing can read is a cache nothing can use, and the one thing
    /// that must not follow is that nothing replaces it.
    ///
    /// The archive beside it is never restored again, so a run that also
    /// declined to pack a fresh one would leave the store holding a file every
    /// future build reads the sidecar of and refuses.
    #[test]
    fn a_sidecar_that_cannot_be_read_is_a_reason_to_pack_and_not_a_reason_to_stop() {
        let dir = std::env::temp_dir().join("sunlit_xtask_dist_unreadable_sidecar");
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::new(&dir);
        let builder = Image::LinuxBuilder;
        let facts = cache::Facts {
            channel: "1.94.0".to_owned(),
            image: builder.slug().to_owned(),
            template_hash: "crc32:1a2b3c4d".to_owned(),
            image_built_utc: "2026-08-28T09:00:00Z".to_owned(),
            lockfile_hash: "crc32:deadbeef".to_owned(),
        };
        std::fs::create_dir_all(store.cache_dir(builder)).expect("mkdir");
        for kind in cache::Kind::ALL {
            std::fs::write(store.cache_archive(builder, kind), b"archive").expect("write");
            std::fs::write(store.cache_sidecar(builder, kind), b"{ not json").expect("sidecar");
        }

        let (job, reports) = plan_cache(&store, builder, &facts, true);
        assert!(job.restore.is_empty(), "{job:?}");
        assert_eq!(job.save, cache::Kind::ALL.to_vec());
        for report in &reports {
            assert!(!report.restored, "{report:?}");
            assert!(
                report
                    .reason
                    .as_ref()
                    .is_some_and(|r| r.contains("malformed")),
                "{report:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Decision 32. A render that never found the textures still produces a
    /// 640x360 PNG, so the verification renders the same scene twice: once
    /// against a directory the job creates and leaves empty, which is the grid
    /// by construction, and once with the variable unset, which is the lookup a
    /// user's machine does.
    #[test]
    fn the_bundles_verification_renders_the_grid_and_the_bundle_and_nothing_between() {
        for target in Target::ALL {
            let bundle = Verification::Bundle {
                exe: "/b/sunlit-earth",
                empty: "/e",
            };
            let job = verify_job(target, &bundle);
            assert_eq!(job.matches("render --output").count(), 2, "{target}: {job}");
            assert!(job.contains(GRID_FILE), "{target}: {job}");
            assert!(job.contains(SMOKE_FILE), "{target}: {job}");
            assert!(job.contains("--width 640"), "{target}: {job}");
            assert!(job.contains("--height 360"), "{target}: {job}");
            // The empty directory is made by the job rather than assumed, and
            // made empty rather than found empty.
            assert!(job.contains("mkdir"), "{target}: {job}");

            // The second render must see no `SUNLIT_EARTH_TEXTURES` at all, or
            // it would be testing the variable rather than the bundle. The
            // grid render is the last place the name may appear.
            let last_set = job
                .rfind("SUNLIT_EARTH_TEXTURES")
                .expect("the grid render sets it");
            let last_render = job.rfind("render --output").expect("two renders");
            assert!(last_set < last_render, "{target}: {job}");

            // And nothing runs from inside the bundle, because a
            // working-directory-relative `textures` would answer before the
            // walk-up the bundle's layout depends on.
            let root = match target {
                Target::Windows => crate::provider::GUEST_ROOT_WINDOWS,
                Target::Linux => crate::provider::GUEST_ROOT_LINUX,
            };
            assert!(
                job.contains(&format!("cd {root}")) || job.contains(&format!("cd /d \"{root}\"")),
                "{target}: {job}"
            );
        }

        // The fallback is the plan's own verification: one render, and no
        // textures directory to name, because there are none to find.
        for target in Target::ALL {
            let loose = verify_job(
                target,
                &Verification::Loose {
                    exe: "/b/sunlit-earth",
                },
            );
            assert_eq!(
                loose.matches("render --output").count(),
                1,
                "{target}: {loose}"
            );
            assert!(
                !loose.contains("SUNLIT_EARTH_TEXTURES"),
                "{target}: {loose}"
            );
            assert!(!loose.contains(GRID_FILE), "{target}: {loose}");
        }
    }

    /// A grid and a globe are far apart; two renders of the same globe seconds
    /// apart are not. Both directions, because a floor nothing can fail is not
    /// a check.
    #[test]
    fn the_two_renders_are_told_apart_by_how_far_they_are_from_each_other() {
        let flat = |value: u8| {
            let mut png = Vec::new();
            let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([value, value, value, 255]));
            image::DynamicImage::ImageRgba8(img)
                .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .expect("encodes");
            png
        };
        // The alpha channel is equal in both, so a difference of `d` over three
        // channels of four is three quarters of `d`.
        let far = render_difference(&flat(0), &flat(200)).expect("both decode");
        assert!(far > TEXTURE_LOOKUP_FLOOR, "{far}");
        let near = render_difference(&flat(120), &flat(121)).expect("both decode");
        assert!(near < TEXTURE_LOOKUP_FLOOR, "{near}");
        assert!(render_difference(&flat(9), &flat(9)).expect("identical") < 1e-9);

        // Two renders of different sizes are not two renders of one scene.
        let mut wide = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(9, 8))
            .write_to(
                &mut std::io::Cursor::new(&mut wide),
                image::ImageFormat::Png,
            )
            .expect("encodes");
        assert!(render_difference(&flat(0), &wide).is_err());
        assert!(render_difference(b"not a png", &flat(0)).is_err());

        let refusal = grid_refusal(0.3, Path::new("/t/results"));
        assert!(refusal.contains("0.30"), "{refusal}");
        assert!(
            refusal.contains("/t/results") || refusal.contains(r"\t\results"),
            "{refusal}"
        );
        assert!(refusal.contains("resolve_textures_dir"), "{refusal}");
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
            cache: vec![cache::Report {
                archive: cache::Kind::Target.slug().to_owned(),
                restored: true,
                bytes: Some(512_000_000),
                written_utc: Some("2026-08-29T09:30:00Z".to_owned()),
                reason: None,
                saved: true,
                copied_in_secs: Some(41),
                copied_out_secs: Some(40),
            }],
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
            bundle: Some(BundleInfo {
                name: "sunlit-earth-0.1.0-linux".to_owned(),
                archive: "sunlit-earth-0.1.0-linux.tar.gz".to_owned(),
                entries: 17,
                texture_lookup_delta: Some(31.75),
            }),
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
        // And the bundle section says what was written and what the two renders
        // in the desktop guest measured, which is decision 32 recorded rather
        // than claimed.
        assert!(json.contains("sunlit-earth-0.1.0-linux.tar.gz"), "{json}");
        assert!(json.contains("texture_lookup_delta"), "{json}");

        // A run with no bundle carries no bundle section at all, the way a
        // Linux record carries no Windows fields.
        let mut bare = info.clone();
        bare.bundle = None;
        assert!(!bare.to_json().contains("bundle"), "{}", bare.to_json());

        // The two verification fields are the exception, and they are the
        // exception because the record travels inside the bundle: a reader who
        // never saw the command run has to be able to tell an unverified
        // release from one nobody wrote the field for.
        let mut unverified = info.clone();
        unverified.verified_in = None;
        if let Some(bundle) = unverified.bundle.as_mut() {
            bundle.texture_lookup_delta = None;
        }
        let json = unverified.to_json();
        assert!(json.contains("\"verified_in\": null"), "{json}");
        assert!(json.contains("\"texture_lookup_delta\": null"), "{json}");
        assert_eq!(BuildInfo::from_json(&json).expect("round trip"), unverified);
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
