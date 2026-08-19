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

mod artifacts;
mod build_image;
mod cargo_json;
mod destroy;
mod doctor;
mod e2e;
mod facts;
mod firmware;
mod hash;
mod inventory;
mod job;
mod manifest;
mod provider;
mod qmp;
mod runner;
#[cfg(test)]
mod script_syntax;
mod setup;
mod ssh;
mod state;
mod status;
mod store;
mod target;
mod util;
mod vm;
mod windows_media;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::runner::RealRunner;
use crate::target::{HostOs, Target};

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
    /// Boot, run a trivial job through the guest contract, collect it, destroy.
    Smoke {
        target: Target,
        /// Leave the VM running afterwards.
        #[arg(long)]
        keep: bool,
    },
    /// List the images, media, overlays, and VMs the xtask owns.
    Status,
    /// Tear down run state, and with `--purge` the golden images too.
    Destroy {
        /// Which target, or `all`.
        target: DestroyTarget,
        /// Also delete the golden images, the installation media, and the
        /// manifests. This is the disk-space recovery path; rebuilding costs
        /// one `vm build-image` per target.
        #[arg(long)]
        purge: bool,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum DestroyTarget {
    Windows,
    Linux,
    All,
}

impl From<DestroyTarget> for destroy::Selection {
    fn from(value: DestroyTarget) -> Self {
        match value {
            DestroyTarget::Windows => Self::One(Target::Windows),
            DestroyTarget::Linux => Self::One(Target::Linux),
            DestroyTarget::All => Self::All,
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
            VmCommand::Destroy { target, purge } => {
                vm::destroy_command(&runner, target.into(), purge)
            }
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
