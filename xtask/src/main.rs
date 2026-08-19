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

// Temporary while the phase lands step by step: the store paths, the provider
// matrix, and the state fields are written in step 1 and first used by the
// providers in steps 3 and 7. Removed once every step is in.
#![allow(dead_code)]

mod destroy;
mod doctor;
mod facts;
mod hash;
mod inventory;
mod manifest;
mod runner;
mod setup;
mod state;
mod status;
mod store;
mod target;
mod util;

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
}

#[derive(Subcommand)]
enum VmCommand {
    /// Check whether this host can run the VM suite. Unelevated, changes
    /// nothing.
    Doctor,
    /// Prepare this host. Elevated on Windows; reports what needs a restart or
    /// a relogin but never performs one.
    Setup,
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
        Command::Vm { command } => match command {
            VmCommand::Doctor => doctor::run(&runner),
            VmCommand::Setup => run_setup(&runner),
            VmCommand::Status => run_status(),
            VmCommand::Destroy { target, purge } => run_destroy(target.into(), purge),
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

fn run_status() -> Result<u8, String> {
    let store = store::store()?;
    let inventory = inventory::scan(&store);
    print!("{}", status::render(&inventory, util::now_unix()));
    Ok(0)
}

fn run_destroy(selection: destroy::Selection, purge: bool) -> Result<u8, String> {
    let store = store::store()?;
    let inventory = inventory::scan(&store);
    let plan = destroy::plan(&store, &inventory, selection, purge);
    print!("{}", plan.render());
    if plan.is_empty() {
        return Ok(0);
    }
    let outcome = destroy::execute(&plan, &|state| {
        Err(format!(
            "no provider is wired up for {} yet, so the VM was not stopped; \
             delete its overlay by hand if it is not running",
            state.provider
        ))
    });
    print!("{}", destroy::render_outcome(&outcome, purge));
    Ok(u8::from(!outcome.problems.is_empty()))
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
    let probe = runner::Cmd::new("wsl.exe").args([
        "-d",
        facts::WSL_DISTRO,
        "--user",
        "root",
        "--",
        "bash",
        "-lc",
        "command -v cargo >/dev/null && command -v clang >/dev/null && echo READY",
    ]);
    runner
        .capture(&probe)
        .is_ok_and(|out| out.stdout.contains("READY"))
}
