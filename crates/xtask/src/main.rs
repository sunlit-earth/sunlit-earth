//! Developer tooling for sunlit-earth.
//!
//! `cargo xtask vm ...` prepares a host, builds the golden VM images, and runs
//! guests; `cargo xtask e2e ...` runs the desktop end-to-end suite on the host
//! or inside one of those guests. The design is in
//! `docs/plans/2026-08-19-phase3-vm-orchestration-plan.md`; `docs/vm-setup.md`
//! is the human guide.
//!
//! Two rules shape the code. Everything that touches a process goes through
//! `runner::Runner`, and everything that decides something is a pure function
//! over data, so the decisions are unit-tested without a hypervisor.

mod commands;
mod guest;
mod host;
mod provider;
mod runner;
mod store;
mod util;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::commands::{build_image, doctor, e2e, setup, teardown, vm};
use crate::host::facts;
use crate::provider::target::{HostOs, Target};
use crate::runner::RealRunner;

#[derive(Parser)]
#[command(
    name = "xtask",
    about = "Developer tooling: local VM orchestration for the desktop e2e suite",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage the local test virtual machines.
    Vm {
        #[command(subcommand)]
        command: VmCommand,
    },
    /// Run the desktop end-to-end suite, here or in a guest.
    E2e {
        /// Where to run it.
        #[arg(long, default_value = "host")]
        target: e2e::Where,
        /// Leave the VM running afterwards for inspection.
        #[arg(long)]
        keep: bool,
        /// Run even though the image's evaluation licence has expired.
        #[arg(long)]
        allow_expired_image: bool,
    },
}

#[derive(Subcommand)]
enum VmCommand {
    /// Check whether this host can run the VM suite. Unelevated, changes
    /// nothing.
    Doctor,
    /// Build a golden image from the templates in `vm/<target>/`.
    BuildImage {
        /// Which guest to build.
        target: Target,
    },
    /// Prepare this host. Elevated on Windows; reports what needs a restart or
    /// a relogin but never performs one.
    Setup,
    /// Boot an interactive guest without running any tests.
    Up {
        target: Target,
        /// Boot even though the image's evaluation licence has expired.
        #[arg(long)]
        allow_expired_image: bool,
    },
    /// Open a shell in the running guest, or run one command in it.
    Ssh {
        target: Target,
        /// A command to run instead of an interactive shell.
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,
    },
    /// Open the running guest's desktop.
    View { target: Target },
    /// Boot, run a trivial job through the guest contract, collect it, take it down.
    Smoke {
        target: Target,
        /// Leave the VM running afterwards.
        #[arg(long)]
        keep: bool,
    },
    /// List the images, media, overlays, and VMs the xtask owns.
    Status,
    /// End the guest and delete its run state. The golden image stays.
    Down {
        /// Which target, or `all`.
        target: TeardownTarget,
    },
    /// Delete what a target has on disk. Everything unless a flag narrows it,
    /// and it asks first. Rebuilding costs one `vm build-image` per target and
    /// re-downloading the Windows media costs 6.6 GB.
    Purge {
        /// Which target, or `all`.
        target: TeardownTarget,
        /// Only the VM: its overlay and run state.
        #[arg(long)]
        vm: bool,
        /// Only the golden image, its manifest, and the build leftovers.
        #[arg(long)]
        image: bool,
        /// Only the cached installation media.
        #[arg(long)]
        iso: bool,
        /// Do not ask.
        #[arg(short, long)]
        force: bool,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum TeardownTarget {
    Windows,
    Linux,
    All,
}

impl From<TeardownTarget> for teardown::Selection {
    fn from(value: TeardownTarget) -> Self {
        match value {
            TeardownTarget::Windows => Self::One(Target::Windows),
            TeardownTarget::Linux => Self::One(Target::Linux),
            TeardownTarget::All => Self::All,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runner = RealRunner;

    let result = match cli.command {
        Command::E2e {
            target,
            keep,
            allow_expired_image,
        } => e2e::run(&runner, target, keep, allow_expired_image),
        Command::Vm { command } => match command {
            VmCommand::Doctor => doctor::run(&runner),
            VmCommand::BuildImage { target } => build_image::run(&runner, target),
            VmCommand::Setup => run_setup(&runner),
            VmCommand::Up {
                target,
                allow_expired_image,
            } => vm::up(&runner, target, allow_expired_image),
            VmCommand::Ssh { target, command } => vm::ssh(&runner, target, &command),
            VmCommand::View { target } => vm::view(&runner, target),
            VmCommand::Smoke { target, keep } => vm::smoke(&runner, target, keep),
            VmCommand::Status => vm::status(&runner),
            VmCommand::Down { target } => vm::down(&runner, target.into()),
            VmCommand::Purge {
                target,
                vm,
                image,
                iso,
                force,
            } => vm::purge(
                &runner,
                target.into(),
                teardown::Scope::from_flags(vm, image, iso),
                force,
            ),
        },
    };

    match result {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run_setup(runner: &dyn runner::Runner) -> Result<u8, String> {
    let host = HostOs::current();
    let store = store::store()?;
    let facts = facts::collect(runner, host, store.root());

    let mut inputs = setup::SetupInputs {
        wsl_ready: false,
        ssh_key_present: store.ssh_key().is_file(),
        facts,
    };

    let steps = match host {
        HostOs::Windows => {
            let windows = inputs.facts.windows.clone().unwrap_or_default();
            if !windows.elevated {
                println!("{}", setup::elevation_message());
                return Ok(1);
            }
            inputs.wsl_ready = wsl_ready(runner, &windows);
            setup::windows_plan(&inputs, &store)
        }
        HostOs::Linux => setup::linux_plan(&inputs, &store),
        HostOs::Other => {
            return Err(
                "this host is not a supported VM host; macOS coverage runs on hosted CI runners"
                    .to_owned(),
            );
        }
    };

    println!(
        "setup plan for {} ({})",
        host.name(),
        store.root().display()
    );
    for step in &steps {
        println!(
            "  [{}] {}: {}",
            if step.needed { "run " } else { "skip" },
            step.name,
            step.note
        );
    }
    println!();

    let outcome = setup::execute(runner, &steps, host);
    print!("{}", setup::render_outcome(&outcome));
    Ok(u8::from(!outcome.failed.is_empty()))
}

/// Whether the WSL distribution already has the toolchain and the build
/// dependencies. Asking costs a distro start, which is why the doctor does not.
fn wsl_ready(runner: &dyn runner::Runner, windows: &facts::WindowsFacts) -> bool {
    if windows.distro(facts::WSL_DISTRO).is_none() {
        return false;
    }
    // Probed as the account that will do the building, which is the
    // distribution's default user, not root. Asking root whether cargo is
    // available answers a question nobody has: root is not who runs the build,
    // and the two accounts have separate toolchains and separate PATHs.
    let probe = runner::Cmd::new("wsl.exe").args([
        "-d",
        facts::WSL_DISTRO,
        "--",
        "bash",
        "-lc",
        "command -v cargo >/dev/null && command -v clang >/dev/null && echo READY",
    ]);
    runner
        .capture(&probe)
        .is_ok_and(|out| out.stdout.contains("READY"))
}
