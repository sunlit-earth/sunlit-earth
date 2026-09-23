//! Building the guest's binaries on the host, and getting them in.
//!
//! Plan decision 8: build on the host, copy the artifacts in. A Windows host
//! builds the Windows guest's binaries natively and the Linux guest's through
//! WSL.
//!
//! The WSL distribution is Ubuntu 22.04 and the guest is now Debian 13, so the
//! two are no longer the same userland. What has to hold is only the direction:
//! glibc is backwards compatible, so a binary linked against the older one runs
//! against the newer, and Ubuntu 22.04's glibc 2.35 is comfortably the older of
//! the pair. The reverse would not work, which is why the builder is the old
//! distribution and not the guest.

use std::path::{Path, PathBuf};

use crate::commands::vm::Session;
use crate::guest::cargo_json::{self, Artifact};
use crate::guest::handover;
use crate::provider;
use crate::provider::target::{HostOs, Image, Target};
use crate::runner::{Cmd, Runner};
use crate::store::{self, Store};

/// The Cargo package and test target the suite lives in.
pub const PACKAGE: &str = "sunlit-earth";
pub const TEST_TARGET: &str = "e2e";

/// What the guest needs, on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostArtifacts {
    pub app: PathBuf,
    pub harness: PathBuf,
    pub fixtures: PathBuf,
    /// The repository's texture directory, when it holds the real assets.
    ///
    /// `None` is not a failure: the suite then runs in the guest exactly as it
    /// runs on a host that has never fetched them, with the render case looking
    /// at the procedural grid.
    pub textures: Option<PathBuf>,
}

/// The texture files the app resolves, and therefore the ones the guest needs.
///
/// `resolve_texture_paths` in the app names these four, and nothing else
/// connects the two crates, so `the_staged_textures_are_the_ones_the_app_asks_for`
/// reads that function and asserts all of them are still spelled this way.
pub const TEXTURE_FILES: [&str; 4] = [
    "world.topo.200405.jxl",
    "BlackMarble_2016.jxl",
    "lroc_color_poles_1k.jxl",
    "milkyway_2020_4k.jxl",
];

/// Smaller than any real asset here and far larger than a Git LFS pointer.
const TEXTURE_MIN_BYTES: u64 = 64 * 1024;

/// Whether a textures directory holds the assets or something that only looks
/// like them.
///
/// `textures/**` is Git LFS, and a checkout without the objects leaves pointer
/// files of a couple of hundred bytes, which are there as far as anything that
/// only asks whether the file exists is concerned. Staging those would be worse
/// than staging nothing: the app would fail to decode them, and a failed decode
/// leaves the slot in the state `Renderer::textures_ready` never reports ready
/// (the open roadmap item), so a guest would wait for an event that cannot
/// arrive. Size is what tells the two apart, since the smallest of the three real
/// assets is over 250 KiB.
pub fn textures_verdict(sizes: [Option<u64>; TEXTURE_FILES.len()]) -> Result<(), String> {
    for (name, size) in TEXTURE_FILES.iter().zip(sizes) {
        match size {
            None => return Err(format!("there is no {name} in it")),
            Some(bytes) if bytes < TEXTURE_MIN_BYTES => {
                return Err(format!(
                    "{name} is {bytes} bytes, which is a Git LFS pointer rather than \
                     the asset; `git lfs pull` fetches it"
                ));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// The repository's textures directory, if it is worth copying in.
///
/// Reports what it decided either way, because the render case samples the
/// Sahara and the Atlantic: with the assets it tests the real map, and without
/// them it tests the procedural grid and says so, which is the same thing that
/// happens to `cargo e2e` on a host in this state.
pub fn host_textures(repo: &Path) -> Option<PathBuf> {
    match textures_present(repo) {
        Ok(dir) => Some(dir),
        Err(why) => {
            println!("not staging {}: {why}", repo.join("textures").display());
            println!("  the guest will render the procedural grid, as this host would");
            None
        }
    }
}

/// The same question without the commentary, for a caller with its own to make.
///
/// `dist` asks it about the release bundle rather than about staging, and what
/// it has to say when the answer is no is decision 33's sentence rather than
/// this one's.
pub fn textures_present(repo: &Path) -> Result<PathBuf, String> {
    let dir = repo.join("textures");
    // An array rather than a vector, so the sizes and the names cannot get
    // out of step with each other.
    let sizes = TEXTURE_FILES.map(|name| std::fs::metadata(dir.join(name)).ok().map(|m| m.len()));
    textures_verdict(sizes).map(|()| dir)
}

/// The `cargo` invocation that builds the suite without running it.
pub fn build_args() -> Vec<String> {
    vec![
        "test".to_owned(),
        "--no-run".to_owned(),
        "--locked".to_owned(),
        "--message-format=json".to_owned(),
        "-p".to_owned(),
        PACKAGE.to_owned(),
        "--test".to_owned(),
        TEST_TARGET.to_owned(),
    ]
}

/// The command line that builds the Linux binaries inside WSL.
///
/// `CARGO_TARGET_DIR` points into the distribution's own filesystem, which is
/// the arrangement README.md documents: two platforms' artifacts do not collide
/// in a shared `target/`, they accumulate, and the directory ends up holding
/// both.
pub fn wsl_build_command(distro: &str, repo_wsl_path: &str) -> Cmd {
    let script = format!(
        "cd {repo} && CARGO_TARGET_DIR=$HOME/sunlit-target cargo {args}",
        repo = shell_quote(repo_wsl_path),
        args = build_args().join(" ")
    );
    Cmd::new("wsl.exe").args([
        "-d".to_owned(),
        distro.to_owned(),
        "--".to_owned(),
        "bash".to_owned(),
        "-lc".to_owned(),
        script,
    ])
}

/// `wslpath`, which is the only reliable translation between the two path
/// worlds and ships inside the distribution.
pub fn wslpath_command(distro: &str, flag: &str, path: &str) -> Cmd {
    Cmd::new("wsl.exe").args([
        "-d".to_owned(),
        distro.to_owned(),
        "--".to_owned(),
        "wslpath".to_owned(),
        flag.to_owned(),
        wsl_arg(path),
    ])
}

/// Prepare a Windows path to be passed to a program inside WSL.
///
/// `wsl.exe` marshals the Windows command line into a Linux argv and treats a
/// backslash as an escape while doing it, so `C:\Workspace` arrives as
/// `C:Workspace` and `wslpath` then fails on a path that looked correct going
/// in. Both Windows and `wslpath` accept forward slashes, so converting is
/// simpler and less fragile than escaping. Measured on Windows 11 with WSL
/// 2.7.11: forward slashes and doubled backslashes both work, single
/// backslashes do not.
pub fn wsl_arg(path: &str) -> String {
    path.replace('\\', "/")
}

/// Single-quote a value for `sh`.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// What compiles a guest's binaries on this host.
///
/// Every cell of the matrix has an answer, and two of the four are not this
/// host's own toolchain. Cross-compiling is what none of them is: it would mean
/// mingw-w64 or a Windows SDK on one side and a second glibc on the other, a
/// second target triple, and a second set of link-time problems. Compiling on
/// the operating system the binaries are for keeps one triple and one linker,
/// and both sidecars are things `vm setup` and `vm build-image` already make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builder {
    /// The host's own cargo.
    Native,
    /// The WSL distribution `vm setup` registered: a Windows host building the
    /// Linux guest's binaries, against an older glibc than the guest's, which
    /// is the direction that works.
    Wsl,
    /// A builder guest of the target's own operating system, booted for the
    /// build and taken down again: a Linux host building the Windows guest's
    /// binaries. The same image `dist` compiles a release binary in.
    Guest(Image),
}

/// Which of the three this host uses for `target`, without asking whether it is
/// ready to be used.
///
/// Asked before anything is created, because the answer does not depend on the
/// VM and finding out afterwards means a booted guest with nothing to run in it.
/// `build` goes through the same function, so the check and the attempt cannot
/// disagree about what is possible.
pub fn builder_for(host: HostOs, target: Target) -> Result<Builder, String> {
    match (host, target) {
        (HostOs::Windows, Target::Windows) | (HostOs::Linux, Target::Linux) => Ok(Builder::Native),
        (HostOs::Windows, Target::Linux) => Ok(Builder::Wsl),
        (HostOs::Linux, Target::Windows) => Ok(Builder::Guest(Image::builder(target))),
        _ => Err(format!(
            "a {} host cannot build binaries for a {target} guest",
            host.name()
        )),
    }
}

/// The same, and whether what it names can be used right now.
///
/// The one thing that can be missing is a builder image, which is a
/// `vm build-image` away rather than a fact about the host, so the refusal names
/// it. What reads this is the caller that has to choose between building and
/// booting a guest with nothing in it, so the error is a sentence about why
/// nothing will be staged rather than a failure.
pub fn usable_builder(store: &Store, host: HostOs, target: Target) -> Result<Builder, String> {
    let builder = builder_for(host, target)?;
    if let Builder::Guest(image) = builder {
        let inventory = crate::store::inventory::scan(store);
        let condition = inventory
            .for_image(image)
            .map(|entry| entry.condition(crate::util::now_unix()));
        match condition {
            Some(condition) if !condition.blocks_boot() => {}
            Some(condition) => {
                return Err(format!(
                    "the {target} guest's binaries are built in the {image} guest on a \
                     {host} host, and that image is {}: {}. \
                     `cargo xtask vm build-image {image}` makes it",
                    condition.label(),
                    condition.detail(),
                    host = host.name(),
                ));
            }
            None => {
                return Err(format!(
                    "the {target} guest's binaries are built in the {image} guest on a \
                     {host} host, and nothing is known about that image. \
                     `cargo xtask vm build-image {image}` makes it",
                    host = host.name(),
                ));
            }
        }
    }
    Ok(builder)
}

/// Pick the two executables out of what Cargo reported.
pub fn select(artifacts: &[Artifact]) -> Result<(PathBuf, PathBuf), String> {
    let app = cargo_json::bin(artifacts, PACKAGE)
        .ok_or_else(|| format!("cargo built no {PACKAGE} binary"))?;
    let harness = cargo_json::test_binary(artifacts, TEST_TARGET)
        .ok_or_else(|| format!("cargo built no {TEST_TARGET} test harness"))?;
    Ok((app.executable.clone(), harness.executable.clone()))
}

/// Build the binaries a target's guest needs.
pub fn build(runner: &dyn Runner, store: &Store, target: Target) -> Result<HostArtifacts, String> {
    let repo = store::repo_root();
    let fixtures = repo
        .join("crates")
        .join("sunlit-app")
        .join("tests")
        .join("fixtures");

    let textures = host_textures(&repo);

    let host = HostOs::current();
    let builder = usable_builder(store, host, target)?;

    if builder == Builder::Native {
        println!("building the e2e suite for the {target} guest (a few minutes if cold)");
        // stdout is the JSON this parses; stderr is cargo's progress, and that
        // goes to the terminal, because the alternative is several silent
        // minutes that look like a hang.
        let out = runner
            .capture(
                &Cmd::new("cargo")
                    .args(build_args())
                    .cwd(&repo)
                    .show_stderr(),
            )
            .map_err(|e| format!("cannot run cargo: {e}"))?;
        if !out.success() {
            return Err("building the e2e suite failed; the output above says why".to_owned());
        }
        let (app, harness) = select(&cargo_json::parse_artifacts(&out.stdout))?;
        return Ok(HostArtifacts {
            app,
            harness,
            fixtures,
            textures,
        });
    }

    match builder {
        Builder::Native => unreachable!("the native arm returns above"),
        Builder::Wsl => build_in_wsl(runner, store, &repo, fixtures, textures),
        Builder::Guest(image) => build_in_guest(runner, store, image, &repo, fixtures, textures),
    }
}

/// The archive of the working tree a builder guest compiles.
///
/// Not `git archive HEAD`, which is what `dist` sends and rightly: a release
/// bundle is a commit, and this is whatever is being edited right now.
/// `git ls-files --cached --others --exclude-standard` is that tree: tracked
/// files with their working-tree content, plus new files that are not ignored,
/// which is what makes an edit that has not been committed reach the guest that
/// compiles it. `.gitignore` keeps `target/` out, and the pathspec keeps the
/// textures out, which are Git LFS and not needed to build.
///
/// `--transform` puts everything under `src/`, the prefix `dist`'s archive uses
/// and the one the job extracts. GNU tar and a pipeline are safe here because
/// this path exists for one host: a Linux one.
pub fn worktree_archive_script(output: &Path) -> String {
    format!(
        "set -euo pipefail\n\
         git ls-files -z --cached --others --exclude-standard -- ':!textures' \
         | tar --null --files-from - --transform 's,^,src/,' --create --file {output}\n",
        output = shell_quote(&output.to_string_lossy())
    )
}

/// The job that builds the suite inside a Windows builder guest.
///
/// The same cargo invocation the host would run, with the JSON on a file in the
/// results directory rather than on a pipe: the host reads it afterwards to
/// learn which two executables to bring back, because the harness carries a
/// hash in its name that only cargo knows. Everything else is what `dist`'s
/// build job does for the same guest, minus the release profile, the caches and
/// the linkage checks.
///
/// One difference from that job is load-bearing rather than incidental: the
/// extraction keeps the archive's modification times, where `dist` extracts with
/// `-m` and discards them. Cargo's freshness check for the crates in this
/// workspace is mtime-based, so files stamped with the extraction time are files
/// cargo rebuilds, and that is the expensive tail: `sunlit-core`, the Slint macro
/// expansion, the app and the final link. It costs nothing in a guest that is
/// destroyed afterwards, which is `dist`'s case, and it costs the whole benefit
/// of a build directory that outlives one run, which is this one's.
pub fn guest_build_job(channel: &str) -> String {
    let root = crate::provider::GUEST_ROOT_WINDOWS;
    let target_dir = crate::commands::dist::GUEST_TARGET_DIR;
    let libclang = crate::commands::build_layer::LIBCLANG_DIR;
    let args = build_args().join(" ");
    format!(
        "@echo off\r\n\
         set ROOT={root}\r\n\
         set CARGO_NET_RETRY=5\r\n\
         set CARGO_TERM_COLOR=never\r\n\
         set LIBCLANG_PATH={libclang}\r\n\
         set CARGO_TARGET_DIR=%ROOT%\\{target_dir}\r\n\
         set CARGO=%USERPROFILE%\\.cargo\\bin\\cargo.exe\r\n\
         set RUSTUP=%USERPROFILE%\\.cargo\\bin\\rustup.exe\r\n\
         \"%RUSTUP%\" toolchain install {channel} --profile minimal || exit /b 1\r\n\
         if exist \"%ROOT%\\src\" rmdir /s /q \"%ROOT%\\src\"\r\n\
         tar.exe -xf \"%ROOT%\\src.tar\" -C \"%ROOT%\" || exit /b 1\r\n\
         cd /d \"%ROOT%\\src\" || exit /b 1\r\n\
         \"%CARGO%\" +{channel} {args} > \"%SUNLIT_E2E_ARTIFACTS%\\{CARGO_JSON}\" || exit /b 1\r\n\
         exit /b 0\r\n"
    )
}

/// Where the job leaves cargo's machine-readable output, inside the results
/// directory that comes back to the host.
pub const CARGO_JSON: &str = "cargo.json";

/// What the build found in the store when it went looking for its builder.
///
/// The three cases want three different things done and two different things
/// done afterwards, which is why this is a value rather than a pair of booleans
/// at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// A guest of this image is up: somebody left it that way, and it stays that
    /// way.
    Running,
    /// A guest of this image is stopped, holding the build directory that makes
    /// this build a link rather than a compile. It is resumed and stopped again.
    Stopped,
    /// Nothing usable is recorded: no guest, or one a crash left behind, which
    /// `boot` clears away. What this build boots is left stopped rather than
    /// destroyed, so the next one finds the second case.
    Absent,
}

/// Which of the three a record and its liveness describe (plan decision 6).
///
/// A crashed guest reads as `Absent` on purpose: it is registered, it is not
/// running, and nothing in it can be trusted to be a build directory, which is
/// the opposite treatment from a guest that was stopped deliberately.
pub fn found_from(state: Option<&crate::store::state::RunState>, running: bool) -> Found {
    match state {
        Some(_) if running => Found::Running,
        Some(state) if state.stopped => Found::Stopped,
        _ => Found::Absent,
    }
}

/// Whether the builder is left stopped once the build in it is over.
///
/// Goal 6 in both directions: a guest somebody left running keeps running, and
/// one this command brought up goes to stopped rather than to destroyed, because
/// what it holds by then is the reason the next build is a link. So everything
/// except a guest that was already running ends stopped.
pub fn stop_after_build(found: Found) -> bool {
    found != Found::Running
}

/// Build the guest's binaries in a builder guest of its own operating system,
/// and bring the two executables back.
///
/// The build happens before the desktop guest boots rather than inside `stage`,
/// which used to be forced by the one-VM-at-a-time rule and is now a matter of
/// order: the binaries have to exist before there is a guest to put them in.
fn write_worktree_archive(
    runner: &dyn Runner,
    store: &Store,
    builder: Image,
    repo: &Path,
) -> Result<(PathBuf, u64), String> {
    let archive = crate::commands::dist::archive_path(store, builder);
    if let Some(parent) = archive.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let _ = std::fs::remove_file(&archive);
    let out = runner
        .capture(
            &Cmd::new("bash")
                .args(["-c".to_owned(), worktree_archive_script(&archive)])
                .cwd(repo),
        )
        .map_err(|e| format!("cannot run bash: {e}"))?;
    if !out.success() {
        return Err(format!(
            "archiving the working tree failed: {}",
            out.stderr.trim()
        ));
    }
    let bytes = std::fs::metadata(&archive)
        .map(|meta| meta.len())
        .map_err(|e| format!("no archive at {}: {e}", archive.display()))?;
    Ok((archive, bytes))
}

/// The guest this build runs in, and what it was before the build found it.
///
/// Three ways in, and only one of them creates anything: a guest that is up is
/// taken over, a stopped one is resumed with everything in it, and anything else
/// is a boot. What makes the third case worth telling apart is decision 6, since
/// it is the one whose guest this command has to put somewhere afterwards.
fn builder_session<'a>(
    runner: &'a dyn Runner,
    store: &'a Store,
    builder: Image,
) -> Result<(Session<'a>, Found), String> {
    let existing = crate::commands::vm::load_state(store, builder);
    let running = existing.as_ref().is_some_and(|state| {
        crate::provider::for_state(runner, store, state)
            .is_ok_and(|provider| provider.is_running(state))
    });
    let found = found_from(existing.as_ref(), running);
    let session = match (found, existing) {
        (Found::Running, Some(state)) => {
            println!("  the {builder} guest is already up; building in it as it stands");
            crate::commands::vm::adopt(runner, store, builder, state)?
        }
        (Found::Stopped, Some(state)) => {
            println!("  resuming the stopped {builder} guest, with its build directory");
            crate::commands::vm::resume(runner, store, builder, state)?
        }
        _ => crate::commands::vm::boot(
            runner,
            store,
            builder,
            crate::store::state::StartReason::Suite,
            false,
            crate::commands::vm::BootRequest::PLAIN,
        )?,
    };
    Ok((session, found))
}

fn build_in_guest(
    runner: &dyn Runner,
    store: &Store,
    builder: Image,
    repo: &Path,
    fixtures: PathBuf,
    textures: Option<PathBuf>,
) -> Result<HostArtifacts, String> {
    let target = builder.target();
    let pinned = crate::guest::toolchain::pinned()?;
    let (archive, bytes) = write_worktree_archive(runner, store, builder, repo)?;

    println!(
        "building the e2e suite for the {target} guest in the {builder} guest, \
         because this host has no {target} toolchain"
    );
    println!(
        "  the working tree is {} of source, not HEAD: what is staged is what is \
         in the tree now",
        crate::util::format_bytes(bytes)
    );

    let (session, found) = builder_session(runner, store, builder)?;

    // From here the guest exists, so nothing may return without deciding what
    // becomes of it.
    let outcome = (|| -> Result<HostArtifacts, String> {
        let probe = session.provider.exec(
            &session.state,
            &crate::commands::dist::toolchain_probe(target),
        )?;
        if !probe
            .stdout
            .contains(crate::commands::dist::TOOLCHAIN_MARKER)
        {
            return Err(crate::commands::dist::missing_toolchain(builder));
        }
        session.provider.copy_in(
            &session.state,
            &archive,
            &crate::commands::dist::guest_archive(target),
        )?;
        let _ = std::fs::remove_file(&archive);

        println!("  building; cargo's own output follows");
        let mut tail = crate::guest::job::OutputTail::new();
        let code = crate::guest::job::run_watching(
            session.provider.as_ref(),
            &session.state,
            target,
            &guest_build_job(&pinned.channel),
            &store.job_scratch(builder),
            crate::commands::dist::BUILD_TIMEOUT,
            Some(&mut tail),
        )?;
        let results = store.results_dir(builder);
        session.provider.collect_results(
            &session.state,
            &crate::provider::guest_results(target),
            &results,
        )?;
        if code != 0 {
            return Err(format!(
                "building the e2e suite in the {builder} guest exited {code}; its \
                 output is above and {} has what it wrote",
                results.display()
            ));
        }

        // The JSON names guest paths, which is what `copy_out` wants: the two
        // executables are the only things worth bringing back, and the harness
        // is the reason this is read at all, since cargo puts a hash in its
        // name.
        let json = std::fs::read_to_string(results.join("artifacts").join(CARGO_JSON))
            .map_err(|e| format!("the build wrote no {CARGO_JSON}: {e}"))?;
        let (app, harness) = select(&cargo_json::parse_artifacts(&json))?;

        let staging = store.build_dir(builder).join("artifacts");
        std::fs::create_dir_all(&staging)
            .map_err(|e| format!("cannot create {}: {e}", staging.display()))?;
        let mut local = Vec::new();
        for remote in [&app, &harness] {
            let remote = remote.to_string_lossy();
            let to = staging.join(guest_leaf(&remote));
            session.provider.copy_out(&session.state, &remote, &to)?;
            local.push(to);
        }
        println!("  the binaries are back in {}", staging.display());

        Ok(HostArtifacts {
            app: local[0].clone(),
            harness: local[1].clone(),
            fixtures,
            textures,
        })
    })();

    if stop_after_build(found) {
        leave_stopped(store, session, builder);
    }
    outcome
}

/// Put the builder back where the build found it, or one better than it found
/// it: stopped, with everything in it.
///
/// A stop that failed is a warning rather than a failure of the build, whose
/// binaries are already on the host by then. What it costs is a guest still
/// holding memory, so the message names it and how to reach it, the same way
/// every other site that leaves a guest up does.
/// What the build says about the builder it is putting away.
///
/// The stop happens where nobody is looking, at the end of a build whose output
/// the person is reading instead, so the line has to carry the difference on its
/// own: a builder that shut itself down makes the next build a link, and one
/// that was killed makes it a repair pass first. Both keep the build directory,
/// which is why both are worth leaving stopped.
pub fn left_stopped_line(vm_name: &str, how: crate::provider::Stopped) -> String {
    if how == crate::provider::Stopped::Killed {
        return format!(
            "{vm_name} would not shut down and was killed; its build directory is \
             there, but the disk was not closed, so the next build in it repairs \
             the filesystem before it compiles"
        );
    }
    format!(
        "{vm_name} is stopped with its build directory in it; the next build in \
         it resumes rather than compiles"
    )
}

fn leave_stopped(store: &Store, mut session: crate::commands::vm::Session, builder: Image) {
    match session.provider.stop(&session.state) {
        Ok(how) => {
            session.state.stopped = true;
            if let Err(e) = crate::commands::vm::write_state(store, builder, &session.state) {
                println!("warning: the guest is stopped, but its record still says otherwise: {e}");
                return;
            }
            println!("  {}", left_stopped_line(&session.state.vm_name, how));
        }
        Err(e) => {
            println!(
                "warning: {} could not be stopped: {e}",
                session.state.vm_name
            );
            println!("{}", session.reach_hint());
        }
    }
}

/// Print what the compiler said, and say what failed.
///
/// The diagnostics are on stdout as JSON rather than on the terminal, because
/// the build is asked for `--message-format=json` so that the executables can be
/// found afterwards. Without this a failed build says only that it could not
/// compile, over an error count nothing accounts for.
fn build_failure(stdout: &str, what: &str) -> String {
    let diagnostics = cargo_json::rendered_diagnostics(stdout);
    if diagnostics.is_empty() {
        return format!("{what}; the output above says why");
    }
    println!();
    for diagnostic in &diagnostics {
        println!("{}", diagnostic.trim_end());
    }
    format!(
        "{what}: {count} the compiler reported, printed above",
        count = crate::util::count(diagnostics.len(), "diagnostic"),
    )
}

/// Build the Linux binaries in WSL and copy them onto the Windows filesystem.
///
/// Copying inside the distribution rather than reaching into it from Windows
/// avoids the `\\wsl$` share entirely: `/mnt/c` is already the same disk, so a
/// plain `cp` lands the files somewhere the Windows `scp` can read.
fn build_in_wsl(
    runner: &dyn Runner,
    store: &Store,
    repo: &Path,
    fixtures: PathBuf,
    textures: Option<PathBuf>,
) -> Result<HostArtifacts, String> {
    let distro = crate::host::facts::WSL_DISTRO;
    let repo_wsl = wslpath(runner, distro, "-u", &repo.to_string_lossy())?;

    // Says whose userland this is, because the distribution named here is not
    // the guest's and reads as though it were: what makes it the right builder
    // is being older than the guest rather than being the same as it.
    println!(
        "building the e2e suite for the linux guest in {distro}, an older \
         userland than the guest's (a few minutes if cold)"
    );
    let out = runner
        .capture(&wsl_build_command(distro, &repo_wsl).show_stderr())
        .map_err(|e| format!("cannot run wsl.exe: {e}"))?;
    if !out.success() {
        return Err(build_failure(
            &out.stdout,
            &format!("building the Linux e2e suite in {distro} failed"),
        ));
    }
    let (app, harness) = select(&cargo_json::parse_artifacts(&out.stdout))?;

    let staging = store.build_dir(Image::Linux).join("artifacts");
    std::fs::create_dir_all(&staging)
        .map_err(|e| format!("cannot create {}: {e}", staging.display()))?;
    let staging_wsl = wslpath(runner, distro, "-u", &staging.to_string_lossy())?;

    let copy = Cmd::new("wsl.exe").args([
        "-d".to_owned(),
        distro.to_owned(),
        "--".to_owned(),
        "bash".to_owned(),
        "-lc".to_owned(),
        format!(
            "cp {app} {harness} {dest}/",
            app = shell_quote(&app.to_string_lossy()),
            harness = shell_quote(&harness.to_string_lossy()),
            dest = shell_quote(&staging_wsl)
        ),
    ]);
    let out = runner
        .capture(&copy)
        .map_err(|e| format!("cannot run wsl.exe: {e}"))?;
    if !out.success() {
        return Err(format!(
            "copying the Linux binaries out of {distro} failed: {}",
            out.stderr.trim()
        ));
    }

    Ok(HostArtifacts {
        app: staging.join(file_name(&app)),
        harness: staging.join(file_name(&harness)),
        fixtures,
        // The repository half, not the distribution's: `scp` runs on Windows
        // here and reads the same files the Windows host does.
        textures,
    })
}

/// The last component of a path the *guest* spelled.
///
/// `Path::file_name` is the host's answer, and on a Linux host asking it about
/// `C:\\sunlit-e2e\\cargo-target\\debug\\sunlit-earth.exe` returns the whole string:
/// a backslash is an ordinary character there. Nothing that came out of a
/// Windows guest's cargo can go through the host's path rules, so this splits on
/// both separators itself.
fn guest_leaf(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_owned()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn wslpath(runner: &dyn Runner, distro: &str, flag: &str, path: &str) -> Result<String, String> {
    let out = runner
        .capture(&wslpath_command(distro, flag, path))
        .map_err(|e| format!("cannot run wsl.exe: {e}"))?;
    if !out.success() {
        return Err(format!(
            "wslpath could not translate {path}: {}",
            out.stderr.trim()
        ));
    }
    Ok(out.trimmed().to_owned())
}

/// Where each artifact lands inside the guest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestPaths {
    pub app: String,
    pub harness: String,
    pub fixtures: String,
    /// Where the textures landed, when there were any to stage. The job script
    /// points `SUNLIT_EARTH_TEXTURES` at this, and omits the variable
    /// altogether when it is `None` rather than naming a directory the guest
    /// does not have.
    pub textures: Option<String>,
}

/// The guest-side paths for a target, given the host file names.
pub fn guest_paths(target: Target, app: &str, harness: &str, textures: bool) -> GuestPaths {
    match target {
        Target::Windows => GuestPaths {
            app: format!(r"{}\{app}", provider::guest_bin(target)),
            harness: format!(r"{}\{harness}", provider::guest_bin(target)),
            fixtures: format!(r"{}\fixtures", provider::GUEST_ROOT_WINDOWS),
            textures: textures.then(|| provider::guest_textures(target)),
        },
        Target::Linux => GuestPaths {
            app: format!("{}/{app}", provider::guest_bin(target)),
            harness: format!("{}/{harness}", provider::guest_bin(target)),
            fixtures: format!("{}/fixtures", provider::GUEST_ROOT_LINUX),
            textures: textures.then(|| provider::guest_textures(target)),
        },
    }
}

/// Build for a guest and copy everything in.
pub fn stage(
    store: &Store,
    session: &Session,
    built: &HostArtifacts,
) -> Result<GuestPaths, String> {
    let target = session.image.target();

    println!("copying the binaries into the guest");
    let bin_dir = provider::guest_bin(target);
    session
        .provider
        .copy_in(&session.state, &built.app, &bin_dir)?;
    session
        .provider
        .copy_in(&session.state, &built.harness, &bin_dir)?;
    session.provider.copy_in(
        &session.state,
        &built.fixtures,
        &format!("{}/", provider::guest_root(target)),
    )?;
    if let Some(textures) = &built.textures {
        println!("copying the textures into the guest");
        session.provider.copy_in(
            &session.state,
            textures,
            &format!("{}/", provider::guest_root(target)),
        )?;
    }

    let paths = guest_paths(
        target,
        &file_name(&built.app),
        &file_name(&built.harness),
        built.textures.is_some(),
    );
    if target == Target::Linux {
        // scp does not carry the executable bit onto every filesystem, and a
        // harness that cannot be executed fails in a way that looks like a
        // missing file.
        let _ = session.provider.exec(
            &session.state,
            &format!("chmod +x {} {}", paths.app, paths.harness),
        );
    }

    // Every guest is one somebody may end up looking at: `vm up` is for that,
    // `e2e --keep` leaves the same thing behind, and watching a run through
    // `vm view` is supported. So the launcher and the shortcuts are staged
    // alongside the binaries rather than only on the interactive path. A
    // failure here is a warning: it costs convenience, not the run.
    if let Err(e) = handover::prepare(
        session.provider.as_ref(),
        &session.state,
        store,
        session.image,
        &paths,
    ) {
        println!("warning: {e}");
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The automatic stop at the end of a build is the one nobody watches, and
    /// it is where a killed builder used to look exactly like a clean one. Both
    /// lines promise the build directory is still there, because it is; only one
    /// of them promises the next build starts by compiling.
    #[test]
    fn the_line_that_puts_a_builder_away_says_whether_it_was_killed() {
        let clean = left_stopped_line(
            "sunlit-e2e-windows-builder",
            crate::provider::Stopped::ShutDown,
        );
        assert!(
            clean.contains("is stopped with its build directory"),
            "{clean}"
        );
        assert!(!clean.contains("repair"), "{clean}");

        let killed = left_stopped_line(
            "sunlit-e2e-windows-builder",
            crate::provider::Stopped::Killed,
        );
        assert!(killed.contains("was killed"), "{killed}");
        assert!(killed.contains("repairs the filesystem"), "{killed}");
        assert!(killed.contains("build directory"), "{killed}");
    }

    #[test]
    fn the_guest_build_shows_its_progress_while_its_json_is_captured() {
        // A cold build is minutes long. Capturing both streams made `vm up`
        // look like it had hung, which is what it was reported as.
        let native = Cmd::new("cargo").args(build_args()).show_stderr();
        assert!(native.inherit_stderr);
        assert!(
            wsl_build_command(crate::host::facts::WSL_DISTRO, "/mnt/c/x")
                .show_stderr()
                .inherit_stderr
        );
        // The JSON has to keep coming back on stdout, or nothing downstream
        // knows which files were built.
        assert!(
            build_args().contains(&"--message-format=json".to_owned()),
            "{:?}",
            build_args()
        );
    }

    /// Each cell of the matrix compiles on the operating system its binaries
    /// are for, and two of the four reach it through something other than this
    /// host's own cargo.
    #[test]
    fn every_host_and_target_pair_names_what_compiles_it() {
        assert_eq!(
            builder_for(HostOs::Linux, Target::Linux),
            Ok(Builder::Native)
        );
        assert_eq!(
            builder_for(HostOs::Windows, Target::Windows),
            Ok(Builder::Native)
        );
        assert_eq!(
            builder_for(HostOs::Windows, Target::Linux),
            Ok(Builder::Wsl)
        );
        assert_eq!(
            builder_for(HostOs::Linux, Target::Windows),
            Ok(Builder::Guest(Image::WindowsBuilder))
        );
        assert!(builder_for(HostOs::Other, Target::Linux).is_err());
    }

    /// The one thing that can be absent is a builder image, and the refusal is
    /// read by a caller deciding whether to boot a guest with nothing in it, so
    /// it has to say which image and how to make it.
    #[test]
    fn a_builder_guest_that_has_not_been_built_is_named_along_with_the_command() {
        let store = Store::new(std::env::temp_dir().join("sunlit_xtask_no_builder_image"));
        let err = usable_builder(&store, HostOs::Linux, Target::Windows).unwrap_err();
        assert!(err.contains("windows-builder"), "{err}");
        assert!(
            err.contains("cargo xtask vm build-image windows-builder"),
            "{err}"
        );
        // The cells that need no image are unaffected by what the store holds.
        assert_eq!(
            usable_builder(&store, HostOs::Linux, Target::Linux),
            Ok(Builder::Native)
        );
        assert_eq!(
            usable_builder(&store, HostOs::Windows, Target::Linux),
            Ok(Builder::Wsl)
        );
    }

    /// Decision 6, as the three cases a build can find and the two things it
    /// does afterwards. The case that matters most is the middle one: a crashed
    /// guest is registered and not running exactly like a stopped one, and
    /// resuming it would compile in a guest whose disk nothing vouches for.
    #[test]
    fn a_build_reuses_a_builder_it_finds_and_leaves_it_the_way_it_found_it() {
        use crate::store::state::{RunState, StartReason};

        let record = |stopped: bool| {
            let mut state = RunState::new(
                Image::WindowsBuilder,
                crate::provider::target::ProviderKind::Qemu,
                std::path::PathBuf::from("/srv/vm/run/windows-builder/overlay.qcow2"),
                StartReason::Suite,
                0,
            );
            state.stopped = stopped;
            state
        };

        assert_eq!(found_from(Some(&record(false)), true), Found::Running);
        assert_eq!(found_from(Some(&record(true)), false), Found::Stopped);
        // A crash, which is the one that must not be resumed.
        assert_eq!(found_from(Some(&record(false)), false), Found::Absent);
        assert_eq!(found_from(None, false), Found::Absent);
        // A record that says stopped and a guest that is up: the guest wins,
        // because a stop that failed after writing the record leaves this shape.
        assert_eq!(found_from(Some(&record(true)), true), Found::Running);

        // Nothing starts running behind a person's back, and nothing throws
        // away ten minutes of compile either.
        assert!(!stop_after_build(Found::Running));
        assert!(stop_after_build(Found::Stopped));
        assert!(stop_after_build(Found::Absent));
    }

    /// What the guest compiles is the tree as it stands, which is the whole
    /// difference from `dist`: a release bundle is a commit and a staged binary
    /// is what is being edited.
    #[test]
    fn the_builder_guest_is_sent_the_working_tree_and_not_head() {
        let script = worktree_archive_script(Path::new("/srv/vm/run/windows-builder/src.tar"));
        assert!(script.contains("git ls-files"), "{script}");
        assert!(script.contains("--others --exclude-standard"), "{script}");
        assert!(!script.contains("HEAD"), "{script}");
        // Under the prefix the job extracts, and without the LFS textures.
        assert!(script.contains("'s,^,src/,'"), "{script}");
        assert!(script.contains("':!textures'"), "{script}");
    }

    /// A Linux host asking `Path::file_name` about a Windows path gets the whole
    /// path back, and the binaries staged under that name would be garbage.
    #[test]
    fn a_guest_path_is_split_on_the_guests_own_separator() {
        assert_eq!(
            guest_leaf(r"C:\sunlit-e2e\cargo-target\debug\sunlit-earth.exe"),
            "sunlit-earth.exe"
        );
        assert_eq!(
            guest_leaf(r"C:\sunlit-e2e\cargo-target\debug\deps\e2e-9f1c2b3a4d5e6f70.exe"),
            "e2e-9f1c2b3a4d5e6f70.exe"
        );
        // A Linux guest's own paths go through the same function.
        assert_eq!(
            guest_leaf("/home/tester/sunlit-target/debug/e2e-1a2b"),
            "e2e-1a2b"
        );
        assert_eq!(guest_leaf("bare.exe"), "bare.exe");
    }

    /// The job is the same cargo invocation the host would run, with the JSON
    /// kept: the harness carries a hash in its name that only cargo knows, so
    /// the host reads that file to learn what to bring back.
    #[test]
    fn the_guest_build_job_keeps_cargos_json_where_the_results_come_from() {
        let job = guest_build_job("1.94.0");
        for arg in build_args() {
            assert!(job.contains(&arg), "{arg} missing from {job}");
        }
        assert!(
            job.contains(&format!("%SUNLIT_E2E_ARTIFACTS%\\{CARGO_JSON}")),
            "{job}"
        );
        assert!(job.contains("toolchain install 1.94.0"), "{job}");
        assert!(job.contains("+1.94.0"), "{job}");
        // cmd.exe wants CRLF, and the job runner writes the script verbatim.
        assert!(job.contains("\r\n"), "{job}");
        assert!(!job.contains("--release"), "{job}");
        // The archive's modification times are kept: `-m` would stamp every
        // source file with the extraction time, and cargo rebuilds this
        // workspace's crates from mtimes.
        assert!(job.contains("tar.exe -xf "), "{job}");
        assert!(!job.contains("-xmf"), "{job}");
    }

    #[test]
    fn the_build_never_runs_the_tests_and_pins_the_lockfile() {
        let args = build_args();
        assert!(args.contains(&"--no-run".to_owned()), "{args:?}");
        assert!(args.contains(&"--locked".to_owned()), "{args:?}");
        assert!(
            args.contains(&"--message-format=json".to_owned()),
            "{args:?}"
        );
        assert!(args.contains(&"e2e".to_owned()), "{args:?}");
    }

    #[test]
    fn the_wsl_build_keeps_its_target_directory_out_of_the_windows_one() {
        let cmd = wsl_build_command("Ubuntu-22.04", "/mnt/c/work/sunlit-earth");
        let script = cmd.args.last().expect("the script");
        assert!(
            script.contains("CARGO_TARGET_DIR=$HOME/sunlit-target"),
            "{script}"
        );
        assert!(script.contains("cd '/mnt/c/work/sunlit-earth'"), "{script}");
        assert!(script.contains("--no-run"), "{script}");
        assert!(cmd.args.contains(&"Ubuntu-22.04".to_owned()));
    }

    #[test]
    fn shell_quoting_survives_an_apostrophe_in_a_path() {
        assert_eq!(
            shell_quote("/mnt/c/Users/o'brien"),
            r"'/mnt/c/Users/o'\''brien'"
        );
        assert_eq!(shell_quote("/plain"), "'/plain'");
    }

    #[test]
    fn wslpath_is_asked_rather_than_the_translation_guessed() {
        let cmd = wslpath_command("Ubuntu-22.04", "-u", r"C:\work");
        assert_eq!(cmd.program, "wsl.exe");
        assert!(cmd.args.contains(&"wslpath".to_owned()));
    }

    #[test]
    fn a_windows_path_reaches_wsl_with_its_separators_intact() {
        // wsl.exe eats single backslashes on the way in, so the path is
        // converted rather than passed through and hoped for.
        assert_eq!(wsl_arg(r"C:\Workspace\rustrover"), "C:/Workspace/rustrover");
        assert_eq!(wsl_arg("/already/unix"), "/already/unix");
        assert_eq!(
            wslpath_command("Ubuntu-22.04", "-u", r"C:\work\sunlit")
                .args
                .last()
                .map(String::as_str),
            Some("C:/work/sunlit")
        );
    }

    #[test]
    fn selecting_needs_both_the_app_and_the_harness() {
        let artifacts = cargo_json::parse_artifacts(concat!(
            r#"{"reason":"compiler-artifact","target":{"kind":["bin"],"name":"sunlit-earth"},"profile":{"test":false},"executable":"/t/sunlit-earth"}"#,
            "\n",
            r#"{"reason":"compiler-artifact","target":{"kind":["test"],"name":"e2e"},"profile":{"test":true},"executable":"/t/deps/e2e-1"}"#,
        ));
        let (app, harness) = select(&artifacts).expect("both");
        assert_eq!(app, PathBuf::from("/t/sunlit-earth"));
        assert_eq!(harness, PathBuf::from("/t/deps/e2e-1"));

        let only_app = &artifacts[..1];
        assert!(select(only_app).unwrap_err().contains("e2e test harness"));
    }

    #[test]
    fn guest_paths_follow_each_operating_systems_separator() {
        let linux = guest_paths(Target::Linux, "sunlit-earth", "e2e-1a2b", true);
        assert_eq!(linux.app, "/var/lib/sunlit-e2e/bin/sunlit-earth");
        assert_eq!(linux.harness, "/var/lib/sunlit-e2e/bin/e2e-1a2b");
        assert_eq!(linux.fixtures, "/var/lib/sunlit-e2e/fixtures");
        assert_eq!(
            linux.textures.as_deref(),
            Some("/var/lib/sunlit-e2e/textures")
        );

        let windows = guest_paths(Target::Windows, "sunlit-earth.exe", "e2e-1a2b.exe", true);
        assert_eq!(windows.app, r"C:\sunlit-e2e\bin\sunlit-earth.exe");
        assert_eq!(windows.harness, r"C:\sunlit-e2e\bin\e2e-1a2b.exe");
        assert_eq!(windows.fixtures, r"C:\sunlit-e2e\fixtures");
        assert_eq!(windows.textures.as_deref(), Some(r"C:\sunlit-e2e\textures"));

        // Nothing staged, nothing named.
        for target in Target::ALL {
            assert_eq!(guest_paths(target, "a", "h", false).textures, None);
        }
    }

    #[test]
    fn a_git_lfs_pointer_is_not_mistaken_for_a_texture() {
        // All real: the sizes of the four assets in this repository.
        let real = [
            Some(2_574_413),
            Some(1_382_310),
            Some(285_458),
            Some(9_874_855),
        ];
        assert_eq!(textures_verdict(real), Ok(()));

        // A pointer file is a few hundred bytes and is otherwise a file like
        // any other, so existence is not the question to ask.
        let mut pointer = real;
        pointer[0] = Some(130);
        let err = textures_verdict(pointer).unwrap_err();
        assert!(err.contains("world.topo.200405.jxl"), "{err}");
        assert!(err.contains("git lfs pull"), "{err}");

        // Missing is reported as missing rather than as a pointer.
        let mut absent = real;
        absent[1] = None;
        let err = textures_verdict(absent).unwrap_err();
        assert!(err.contains("BlackMarble_2016.jxl"), "{err}");
        assert!(!err.contains("pointer"), "{err}");

        // The Moon is held to the same floor as the rest, which its 285 KB
        // clears by a wide margin.
        let mut moon = real;
        moon[2] = Some(130);
        let err = textures_verdict(moon).unwrap_err();
        assert!(err.contains("lroc_color_poles_1k.jxl"), "{err}");
        assert!(err.contains("git lfs pull"), "{err}");

        // And so is the panorama, which the render case does not sample but
        // which a guest without it draws an empty sky for.
        let mut panorama = real;
        panorama[3] = Some(130);
        let err = textures_verdict(panorama).unwrap_err();
        assert!(err.contains("milkyway_2020_4k.jxl"), "{err}");
        assert!(err.contains("git lfs pull"), "{err}");
    }

    /// The app decides which files it loads; the xtask decides which files the
    /// guest gets. Nothing else connects the two, so a rename in the app would
    /// otherwise surface as the render case sampling the procedural grid in a
    /// guest that was told it had textures.
    #[test]
    fn the_staged_textures_are_the_ones_the_app_asks_for() {
        let startup = std::fs::read_to_string(
            store::repo_root()
                .join("crates")
                .join("sunlit-app")
                .join("src")
                .join("startup.rs"),
        )
        .expect("the app's startup.rs");
        let resolver = startup
            .split("fn resolve_texture_paths")
            .nth(1)
            .expect("resolve_texture_paths is where the app names its textures");
        let body = &resolver[..resolver.find("\n}").unwrap_or(resolver.len())];
        for name in TEXTURE_FILES {
            assert!(
                body.contains(name),
                "the app no longer resolves {name}, so staging it is pointless"
            );
        }
        // Every slot, and no further one the guest would be missing.
        assert_eq!(body.matches(".jxl").count(), TEXTURE_FILES.len());
    }
}
