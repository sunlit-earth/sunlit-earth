//! Building the guest's binaries on the host, and getting them in.
//!
//! Plan decision 8: build on the host, copy the artifacts in. A Windows host
//! builds the Windows guest's binaries natively and the Linux guest's through
//! WSL, whose distribution is pinned to the same Ubuntu 22.04 the guest runs so
//! that glibc agrees.

use std::path::{Path, PathBuf};

use crate::cargo_json::{self, Artifact};
use crate::provider;
use crate::runner::{Cmd, Runner};
use crate::store::{self, Store};
use crate::target::{HostOs, Target};
use crate::vm::Session;

/// The Cargo package and test target the suite lives in.
pub const PACKAGE: &str = "sunlit-earth";
pub const TEST_TARGET: &str = "e2e";

/// What the guest needs, on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostArtifacts {
    pub app: PathBuf,
    pub harness: PathBuf,
    pub fixtures: PathBuf,
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
/// the arrangement CLAUDE.md documents: sharing `target/` between the Windows
/// and Linux builds makes them fight over the same directory.
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

/// Whether this host can build a guest's binaries at all, and whether it does
/// so natively.
///
/// Asked before anything is created, because the answer does not depend on the
/// VM and finding out afterwards means a booted guest with nothing to run in
/// it. `build` uses the same function, so the check and the attempt cannot
/// disagree about what is possible.
///
/// A Windows guest needs Windows binaries, and a Linux host has no toolchain
/// for those: cross-compiling them would mean mingw-w64 or a Windows SDK, a
/// second target triple, and a second set of link-time problems, for the one
/// cell of the matrix that a Windows host covers natively. Left unsupported
/// deliberately rather than half-built.
pub fn check_can_build(host: HostOs, target: Target) -> Result<bool, String> {
    match (host, target) {
        (HostOs::Windows, Target::Windows) | (HostOs::Linux, Target::Linux) => Ok(true),
        // WSL builds the Linux guest's binaries against the same Ubuntu the
        // guest runs.
        (HostOs::Windows, Target::Linux) => Ok(false),
        (HostOs::Linux, Target::Windows) => Err(
            "the Windows guest's binaries cannot be built on a Linux host, so \
             `--target windows` needs a Windows host. The Linux guest works here."
                .to_owned(),
        ),
        _ => Err(format!(
            "a {} host cannot build binaries for a {target} guest",
            host.name()
        )),
    }
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

    let host = HostOs::current();
    let native = check_can_build(host, target)?;

    if native {
        println!("building the e2e suite for the {target} guest (a few minutes if cold)");
        let out = runner
            .capture(&Cmd::new("cargo").args(build_args()).cwd(&repo))
            .map_err(|e| format!("cannot run cargo: {e}"))?;
        if !out.success() {
            return Err(format!(
                "building the e2e suite failed:\n{}",
                out.stderr.trim()
            ));
        }
        let (app, harness) = select(&cargo_json::parse_artifacts(&out.stdout))?;
        return Ok(HostArtifacts {
            app,
            harness,
            fixtures,
        });
    }

    build_in_wsl(runner, store, &repo, fixtures)
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
) -> Result<HostArtifacts, String> {
    let distro = crate::facts::WSL_DISTRO;
    let repo_wsl = wslpath(runner, distro, "-u", &repo.to_string_lossy())?;

    println!("building the e2e suite for the linux guest in {distro} (a few minutes if cold)");
    let out = runner
        .capture(&wsl_build_command(distro, &repo_wsl))
        .map_err(|e| format!("cannot run wsl.exe: {e}"))?;
    if !out.success() {
        return Err(format!(
            "building the Linux e2e suite in {distro} failed:\n{}",
            out.stderr.trim()
        ));
    }
    let (app, harness) = select(&cargo_json::parse_artifacts(&out.stdout))?;

    let staging = store.build_dir(Target::Linux).join("artifacts");
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
    })
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
}

/// The guest-side paths for a target, given the host file names.
pub fn guest_paths(target: Target, app: &str, harness: &str) -> GuestPaths {
    match target {
        Target::Windows => GuestPaths {
            app: format!(r"{}\{app}", provider::guest_bin(target)),
            harness: format!(r"{}\{harness}", provider::guest_bin(target)),
            fixtures: format!(r"{}\fixtures", provider::GUEST_ROOT_WINDOWS),
        },
        Target::Linux => GuestPaths {
            app: format!("{}/{app}", provider::guest_bin(target)),
            harness: format!("{}/{harness}", provider::guest_bin(target)),
            fixtures: format!("{}/fixtures", provider::GUEST_ROOT_LINUX),
        },
    }
}

/// Build for a guest and copy everything in.
pub fn stage(runner: &dyn Runner, store: &Store, session: &Session) -> Result<GuestPaths, String> {
    let target = session.target;
    let built = build(runner, store, target)?;

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

    let paths = guest_paths(target, &file_name(&built.app), &file_name(&built.harness));
    if target == Target::Linux {
        // scp does not carry the executable bit onto every filesystem, and a
        // harness that cannot be executed fails in a way that looks like a
        // missing file.
        let _ = session.provider.exec(
            &session.state,
            &format!("chmod +x {} {}", paths.app, paths.harness),
        );
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_linux_host_says_it_cannot_build_the_windows_guest_before_anything_boots() {
        // The provider matrix has a hypervisor for this cell, which is not the
        // same as being able to produce the binaries to put in it.
        let err = check_can_build(HostOs::Linux, Target::Windows).unwrap_err();
        assert!(err.contains("needs a Windows host"), "{err}");

        assert_eq!(check_can_build(HostOs::Linux, Target::Linux), Ok(true));
        assert_eq!(check_can_build(HostOs::Windows, Target::Windows), Ok(true));
        // Not native: built through WSL.
        assert_eq!(check_can_build(HostOs::Windows, Target::Linux), Ok(false));
        assert!(check_can_build(HostOs::Other, Target::Linux).is_err());
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
        let linux = guest_paths(Target::Linux, "sunlit-earth", "e2e-1a2b");
        assert_eq!(linux.app, "/var/lib/sunlit-e2e/bin/sunlit-earth");
        assert_eq!(linux.harness, "/var/lib/sunlit-e2e/bin/e2e-1a2b");
        assert_eq!(linux.fixtures, "/var/lib/sunlit-e2e/fixtures");

        let windows = guest_paths(Target::Windows, "sunlit-earth.exe", "e2e-1a2b.exe");
        assert_eq!(windows.app, r"C:\sunlit-e2e\bin\sunlit-earth.exe");
        assert_eq!(windows.harness, r"C:\sunlit-e2e\bin\e2e-1a2b.exe");
        assert_eq!(windows.fixtures, r"C:\sunlit-e2e\fixtures");
    }
}
