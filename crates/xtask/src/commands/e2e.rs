//! `cargo xtask e2e --target <host|windows|linux>`.
//!
//! One command for the desktop suite wherever it runs. `--target host` is what
//! `cargo e2e` already does, kept as the same command so the manual real-GPU
//! run and the VM runs are not two different things (retrospective 8.4).

use std::time::Duration;

use clap::ValueEnum;

use crate::commands::vm;
use crate::guest::artifacts::{self, GuestPaths};
use crate::guest::job;
use crate::provider;
use crate::provider::target::Target;
use crate::runner::{Cmd, Runner};
use crate::store;
use crate::store::state::StartReason;

/// Where to run the suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Where {
    /// This desktop, with its real GPU. What `cargo e2e` does.
    Host,
    /// The Windows guest: the whole suite.
    Windows,
    /// The Linux guest: the windowed-mode subset.
    Linux,
}

impl Where {
    pub fn guest(self) -> Option<Target> {
        match self {
            Self::Host => None,
            Self::Windows => Some(Target::Windows),
            Self::Linux => Some(Target::Linux),
        }
    }
}

/// How long the suite may take inside a guest.
///
/// The desktop run is well under a minute; a guest renders on a CPU rasterizer
/// with a fraction of the memory, and the Windows one boots a scheduled task to
/// get there. These are deliberately generous: the cost of a timeout that is
/// too short is a false failure, and the cost of one too long is waiting.
pub fn job_timeout(target: Target) -> Duration {
    match target {
        Target::Windows => Duration::from_secs(45 * 60),
        Target::Linux => Duration::from_secs(30 * 60),
    }
}

/// The Slint backend the Windows guest runs the app with.
///
/// A Hyper-V guest's synthetic display adapter offers no OpenGL, and Windows
/// ships no software implementation of it, so Slint's default winit renderer
/// (femtovg, which is GL) has nothing to initialize: the app dies with
/// "Could not locate glCreateShader symbol" before its event loop starts, and
/// every windowed case then times out waiting for the first frame. WARP covers
/// wgpu and not this, because WARP is a Direct3D adapter and Slint asks for GL.
///
/// `winit-software` keeps the winit event loop, which is what the windowed cases
/// are about, and swaps the renderer for Slint's CPU one: the same choice for
/// Slint that WARP already is for wgpu in this guest. Slint parses the value as
/// `<event loop>-<renderer>`, so this selects the software renderer by name
/// rather than falling back to it; `renderer-software` is one of the crate's
/// default features, so the renderer is compiled in.
///
/// The Linux guest needs no equivalent: Mesa is a software GL implementation
/// and llvmpipe answers there, which is the same reason that guest has a GPU
/// story for wgpu at all.
pub const WINDOWS_SLINT_BACKEND: &str = "winit-software";

/// The job script that runs the suite inside a guest.
///
/// `--test-threads=1` on top of the suite's own `#[serial]` attributes: a guest
/// has one desktop session, and two windowed tests sharing it is not a race
/// worth having.
///
/// The Windows job opts the wallpaper case in. That case replaces the desktop
/// wallpaper of whatever machine runs it, which is the whole reason it is
/// opt-in: a throwaway guest is the one place where doing so costs nothing.
///
/// `SUNLIT_EARTH_TEXTURES` appears only when the textures were staged. The
/// render case samples the Sahara and the Atlantic, so it needs the real map;
/// pointing the app at a directory that is not there would make it fall back to
/// the procedural grid anyway, and saying so would then be a lie in the script.
pub fn job_script(target: Target, paths: &GuestPaths) -> String {
    match target {
        Target::Linux => format!(
            "#!/usr/bin/env bash\n\
             set -uo pipefail\n\
             export SUNLIT_EARTH_BIN={app}\n\
             export SUNLIT_EARTH_E2E_FIXTURES={fixtures}\n\
             {textures}\
             export RUST_BACKTRACE=1\n\
             {harness} --ignored --test-threads=1 --nocapture\n",
            app = artifacts::shell_quote(&paths.app),
            fixtures = artifacts::shell_quote(&paths.fixtures),
            textures = paths.textures.as_ref().map_or_else(String::new, |dir| {
                format!(
                    "export SUNLIT_EARTH_TEXTURES={}\n",
                    artifacts::shell_quote(dir)
                )
            }),
            harness = artifacts::shell_quote(&paths.harness),
        ),
        Target::Windows => format!(
            "@echo off\r\n\
             set SUNLIT_EARTH_BIN={app}\r\n\
             set SUNLIT_EARTH_E2E_FIXTURES={fixtures}\r\n\
             set SUNLIT_EARTH_E2E_WALLPAPER=1\r\n\
             {textures}\
             set SLINT_BACKEND={backend}\r\n\
             set RUST_BACKTRACE=1\r\n\
             \"{harness}\" --ignored --test-threads=1 --nocapture\r\n\
             exit /b %ERRORLEVEL%\r\n",
            app = paths.app,
            fixtures = paths.fixtures,
            textures = paths.textures.as_ref().map_or_else(String::new, |dir| {
                format!("set SUNLIT_EARTH_TEXTURES={dir}\r\n")
            }),
            backend = WINDOWS_SLINT_BACKEND,
            harness = paths.harness,
        ),
    }
}

/// Run the suite.
pub fn run(
    runner: &dyn Runner,
    location: Where,
    keep: bool,
    allow_expired: bool,
) -> Result<u8, String> {
    match location.guest() {
        None => run_on_host(runner),
        Some(target) => run_in_guest(runner, target, keep, allow_expired),
    }
}

/// The desktop run, which is `cargo e2e` under another name.
fn run_on_host(runner: &dyn Runner) -> Result<u8, String> {
    println!("running the desktop e2e suite on this host");
    println!("windows will open and close; leave the desktop alone while it runs");
    let cmd = Cmd::new("cargo")
        // The same arguments the guest job uses, so that a case behaving
        // differently in the two places is the guest's doing rather than the
        // command line's. --nocapture matters: several cases print the figures
        // that are the reason for running them.
        .args([
            "test",
            "--test",
            "e2e",
            "--",
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .cwd(store::repo_root());
    let code = runner
        .stream(&cmd)
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    Ok(u8::try_from(code).unwrap_or(1))
}

/// The VM run: boot a pristine overlay, copy the binaries in, run the suite in
/// the console session, pull the results back, and destroy it.
fn run_in_guest(
    runner: &dyn Runner,
    target: Target,
    keep: bool,
    allow_expired: bool,
) -> Result<u8, String> {
    let store = store::store()?;
    let started = std::time::Instant::now();

    // Asked before anything is created. The provider matrix has a hypervisor
    // for every cell, which is not the same as this host being able to produce
    // the binaries to put in one, and finding that out after a boot means a
    // guest running with nothing to run in it.
    artifacts::check_can_build(crate::provider::target::HostOs::current(), target)?;

    let mut session = vm::boot(runner, &store, target, StartReason::Run, allow_expired)?;

    // From here on the VM exists, so no failure may return without saying what
    // happened to it.
    let paths = match artifacts::stage(runner, &store, &session) {
        Ok(paths) => paths,
        Err(e) => {
            println!("{}", vm::after_failure(&session, &store, keep));
            return Err(e);
        }
    };

    println!("running the suite in the guest's console session");
    let script = job_script(target, &paths);
    let scratch = store.run_dir(target).join("job");
    let code = job::run(
        session.provider.as_ref(),
        &session.state,
        target,
        &script,
        &scratch,
        job_timeout(target),
    );

    let results = store.results_dir(target);
    let collected = session.provider.collect_results(
        &session.state,
        &provider::guest_results(target),
        &results,
    );

    if let Ok(log) = std::fs::read_to_string(results.join("output.log")) {
        println!("--- the guest's output ---");
        print!("{log}");
        println!("--- end ---");
    }

    // Diagnose before tearing down, while the guest can still answer. An
    // expired evaluation is the failure that otherwise reads as flakiness, so
    // it gets to name itself.
    if code.is_err()
        && target.has_eval_expiry()
        && let Ok(out) = session.provider.exec(&session.state, "slmgr /xpr")
    {
        println!("licence state in the guest: {}", out.stdout.trim());
    }

    if keep {
        // The run is over, so the console session is nobody's any more and the
        // guest can offer the enhanced session that would have taken it.
        let enhanced_session = vm::hand_over(&mut session, &store);
        println!(
            "{}",
            vm::lifecycle_explainer(
                target,
                vm::Prepared {
                    staged: true,
                    enhanced_session
                }
            )
        );
    } else if let Err(e) = session.tear_down(&store) {
        // Reported with the way out, not as a bare warning: a VM that would
        // not go away holds its memory and the ports the next run needs.
        println!(
            "warning: {} could not be destroyed: {e}",
            session.state.vm_name
        );
        println!("{}", session.reach_hint());
    }

    if let Err(e) = collected {
        println!("warning: the results could not be collected: {e}");
    }

    let code = code?;
    println!();
    println!(
        "the {target} guest finished the suite with exit code {code} after {:.0}s",
        started.elapsed().as_secs_f64()
    );
    println!("results: {}", results.display());
    Ok(u8::from(code != 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(target: Target) -> GuestPaths {
        paths_with_textures(target, true)
    }

    fn paths_with_textures(target: Target, textures: bool) -> GuestPaths {
        artifacts::guest_paths(
            target,
            if target == Target::Windows {
                "sunlit-earth.exe"
            } else {
                "sunlit-earth"
            },
            if target == Target::Windows {
                "e2e-1a2b.exe"
            } else {
                "e2e-1a2b"
            },
            textures,
        )
    }

    #[test]
    fn the_host_target_is_not_a_guest() {
        assert_eq!(Where::Host.guest(), None);
        assert_eq!(Where::Linux.guest(), Some(Target::Linux));
        assert_eq!(Where::Windows.guest(), Some(Target::Windows));
    }

    #[test]
    fn the_windows_guest_gets_more_time_than_the_linux_one() {
        assert!(job_timeout(Target::Windows) > job_timeout(Target::Linux));
        assert!(job_timeout(Target::Linux) >= Duration::from_secs(600));
    }

    #[test]
    fn the_linux_job_points_the_harness_at_the_copied_binary() {
        let script = job_script(Target::Linux, &paths(Target::Linux));
        // Absolute, so the paths do not depend on the working directory the
        // guest's job runner happens to use.
        assert!(
            script.contains("SUNLIT_EARTH_BIN='/var/lib/sunlit-e2e/bin/sunlit-earth'"),
            "{script}"
        );
        assert!(
            script.contains("SUNLIT_EARTH_E2E_FIXTURES='/var/lib/sunlit-e2e/fixtures'"),
            "{script}"
        );
        assert!(
            script.contains("'/var/lib/sunlit-e2e/bin/e2e-1a2b' --ignored"),
            "{script}"
        );
        assert!(script.contains("--test-threads=1"), "{script}");
        assert!(script.starts_with("#!/usr/bin/env bash"), "{script}");
    }

    #[test]
    fn no_job_script_depends_on_the_working_directory_it_is_run_from() {
        // Every path the job hands the guest, not just the binary: the
        // fixtures path is read by the render case, and the harness path is
        // what the shell executes. Any one of them being relative resolves
        // differently depending on where the guest's runner starts it.
        for target in Target::ALL {
            let script = job_script(target, &paths(target));
            let mut checked = 0;

            for line in script.lines() {
                let line = line.trim();
                for prefix in [
                    "set SUNLIT_EARTH_BIN=",
                    "export SUNLIT_EARTH_BIN=",
                    "set SUNLIT_EARTH_E2E_FIXTURES=",
                    "export SUNLIT_EARTH_E2E_FIXTURES=",
                ] {
                    let Some(rest) = line.strip_prefix(prefix) else {
                        continue;
                    };
                    let value = rest.trim_matches('\'');
                    assert!(
                        value.starts_with('/') || value.starts_with("C:"),
                        "{target}: {prefix}{value} is relative"
                    );
                    checked += 1;
                }
            }
            assert_eq!(checked, 2, "{target}: not both paths were checked");

            // The harness itself is invoked by path, and that path is
            // absolute too.
            let invocation = script
                .lines()
                .find(|line| line.contains("--ignored"))
                .unwrap_or_else(|| panic!("{target}: nothing runs the harness"));
            let invocation = invocation
                .trim()
                .trim_start_matches('"')
                .trim_start_matches('\'');
            assert!(
                invocation.starts_with('/') || invocation.starts_with("C:"),
                "{target}: the harness is invoked as {invocation}"
            );
        }
    }

    #[test]
    fn the_windows_job_is_a_batch_file_that_propagates_the_exit_code() {
        let script = job_script(Target::Windows, &paths(Target::Windows));
        assert!(script.starts_with("@echo off"), "{script}");
        assert!(
            script.contains(r"set SUNLIT_EARTH_BIN=C:\sunlit-e2e\bin\sunlit-earth.exe"),
            "{script}"
        );
        assert!(
            script.contains(r#""C:\sunlit-e2e\bin\e2e-1a2b.exe" --ignored"#),
            "{script}"
        );
        assert!(script.contains("exit /b %ERRORLEVEL%"), "{script}");
        // cmd.exe needs its line endings, unlike everything else here, and
        // they reach the guest unchanged: `job::run` writes the script
        // verbatim rather than normalizing it.
        assert!(script.contains("\r\n"), "{script}");
    }

    #[test]
    fn only_the_windows_job_opts_into_replacing_the_wallpaper() {
        // The Linux guest cannot set a wallpaper, and a developer's desktop
        // must not have one set behind their back, so the guest that can is
        // the one place it is switched on.
        let windows = job_script(Target::Windows, &paths(Target::Windows));
        assert!(
            windows.contains("set SUNLIT_EARTH_E2E_WALLPAPER=1"),
            "{windows}"
        );
        let linux = job_script(Target::Linux, &paths(Target::Linux));
        assert!(!linux.contains("SUNLIT_EARTH_E2E_WALLPAPER"), "{linux}");
    }

    #[test]
    fn each_job_script_carries_the_line_endings_its_shell_expects() {
        let linux = job_script(Target::Linux, &paths(Target::Linux));
        assert!(
            !linux.contains('\r'),
            "a shell script with CRLF fails to run"
        );
        let windows = job_script(Target::Windows, &paths(Target::Windows));
        assert!(windows.contains("\r\n"), "{windows}");
    }

    #[test]
    fn only_the_windows_job_picks_slints_software_renderer() {
        // A Hyper-V guest has no OpenGL and Windows ships no software
        // implementation, so the default GL renderer cannot start there at all.
        // The Linux guest has Mesa, which is a software GL implementation, so
        // asking for the CPU renderer there would test something the guest does
        // not do.
        let windows = job_script(Target::Windows, &paths(Target::Windows));
        assert!(
            windows.contains("set SLINT_BACKEND=winit-software"),
            "{windows}"
        );
        // The value is parsed by Slint as <event loop>-<renderer>, so the event
        // loop half has to stay winit: the windowed cases are about a real
        // window on a real desktop session.
        assert!(
            WINDOWS_SLINT_BACKEND.starts_with("winit-"),
            "{WINDOWS_SLINT_BACKEND}"
        );
        let linux = job_script(Target::Linux, &paths(Target::Linux));
        assert!(!linux.contains("SLINT_BACKEND"), "{linux}");
    }

    #[test]
    fn the_textures_variable_appears_only_when_there_are_textures_in_the_guest() {
        // Naming a directory the guest does not have would make the render case
        // fall back to the procedural grid while the script claimed otherwise.
        for target in Target::ALL {
            let staged = job_script(target, &paths_with_textures(target, true));
            assert!(staged.contains("SUNLIT_EARTH_TEXTURES"), "{staged}");
            let expected = crate::provider::guest_textures(target);
            assert!(staged.contains(&expected), "{staged}");

            let bare = job_script(target, &paths_with_textures(target, false));
            assert!(!bare.contains("SUNLIT_EARTH_TEXTURES"), "{bare}");
            // Everything else is unchanged by the absence, including the line
            // that runs the harness.
            assert!(bare.contains("--ignored"), "{bare}");
        }
    }

    #[test]
    fn both_jobs_run_the_ignored_cases_because_the_whole_suite_is_ignored() {
        for target in Target::ALL {
            assert!(
                job_script(target, &paths(target)).contains("--ignored"),
                "{target}"
            );
        }
    }
}
