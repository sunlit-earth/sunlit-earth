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

use crate::commands::{bake_icon, bake_stars, build_image, dist, doctor, e2e, setup, teardown, vm};
use crate::host::facts;
use crate::provider::desktop::Desktop;
use crate::provider::target::{HostOs, Image};
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
        /// Which desktop to run the suite under. Linux guest only; the image
        /// default is KDE Plasma.
        #[arg(long)]
        desktop: Option<Desktop>,
        /// How many screens to give the guest. Linux guest only.
        #[arg(long, default_value_t = 1, value_name = "N")]
        screens: u16,
    },
    /// Build a release binary in a pristine builder guest, from the committed
    /// tree, and prove it runs in the desktop guest of the same target.
    Dist {
        /// Which target, or `all`.
        #[arg(long, default_value = "all")]
        target: dist::Which,
        /// Leave the last guest of the run up for inspection.
        #[arg(long)]
        keep: bool,
        /// Skip the boot that runs the binary in the desktop image.
        #[arg(long)]
        no_verify: bool,
        /// Download and compile everything, restoring nothing from an earlier
        /// build in this image and saving nothing for the next one.
        #[arg(long)]
        no_cache: bool,
        /// Build even though an image's evaluation licence has expired.
        #[arg(long)]
        allow_expired_image: bool,
        /// Build HEAD even though the working tree has uncommitted changes.
        #[arg(long)]
        allow_dirty: bool,
    },
    /// Rasterize the icon SVGs into the outputs the app ships. The results are
    /// committed; rerun this when a source SVG changes.
    BakeIcon {
        /// Write the small-size review sheet into this directory instead of
        /// baking. For the judgment a test cannot make.
        #[arg(long, value_name = "DIR")]
        review: Option<std::path::PathBuf>,
    },
    /// Bake the HYG star catalog into the runtime instance buffer format.
    BakeStars {
        /// HYG v4.4 CSV input.
        #[arg(long, value_name = "CSV")]
        input: std::path::PathBuf,
        /// Binary catalog output.
        #[arg(long, value_name = "BIN")]
        output: std::path::PathBuf,
    },
}

#[derive(Subcommand)]
enum VmCommand {
    /// Check whether this host can run the VM suite. Unelevated, changes
    /// nothing.
    Doctor,
    /// Build an image from the templates in `vm/<slug>/`. A layer is
    /// provisioned over its parent instead, which needs no template of media.
    BuildImage {
        /// Which image to build.
        image: Image,
    },
    /// Prepare this host. Elevated on Windows; reports what needs a restart or
    /// a relogin but never performs one.
    Setup,
    /// Boot an interactive guest without running any tests.
    Up {
        image: Image,
        /// Boot even though the image's evaluation licence has expired.
        #[arg(long)]
        allow_expired_image: bool,
        /// Which desktop to log into. Linux guest only; the image default is
        /// KDE Plasma.
        #[arg(long)]
        desktop: Option<Desktop>,
        /// How many screens to give the guest, laid out left to right. Linux
        /// guest only; `vm view` opens one viewer per screen.
        #[arg(long, default_value_t = 1, value_name = "N")]
        screens: u16,
    },
    /// End a builder guest and keep everything in it, so the next build in it
    /// resumes. Builder images only: a guest the suite runs in is pristine on
    /// every boot.
    Stop { image: Image },
    /// Resume a stopped builder guest, with the build directory it was holding.
    Start { image: Image },
    /// Open a shell in the running guest, or run one command in it.
    Ssh {
        image: Image,
        /// A command to run instead of an interactive shell.
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,
    },
    /// Open the running guest's desktop.
    View { image: Image },
    /// Boot, run a trivial job through the guest contract, collect it, take it down.
    Smoke {
        image: Image,
        /// Leave the VM running afterwards.
        #[arg(long)]
        keep: bool,
        /// Which desktop to log into. Linux guest only; the image default is
        /// KDE Plasma.
        #[arg(long)]
        desktop: Option<Desktop>,
    },
    /// List the images, media, overlays, and VMs the xtask owns.
    Status,
    /// End the guest and delete its run state. The image stays.
    Down {
        /// Which image, or `all`.
        image: TeardownImage,
    },
    /// Delete what an image has on disk. Everything unless a flag narrows it,
    /// and it asks first. Rebuilding costs one `vm build-image` per image and
    /// re-downloading the Windows media costs 6.6 GB.
    Purge {
        /// Which image, or `all`.
        image: TeardownImage,
        /// Only the VM: its overlay and run state.
        #[arg(long)]
        vm: bool,
        /// Only the image itself, its manifest, and the build leftovers.
        #[arg(long = "image")]
        image_only: bool,
        /// Only the cached installation media.
        #[arg(long)]
        iso: bool,
        /// Only the build cache an earlier `dist` left on this host.
        #[arg(long)]
        cache: bool,
        /// Do not ask.
        #[arg(short, long)]
        force: bool,
    },
}

/// One image or all of them, which is what the two teardowns take.
///
/// A separate enum from [`Image`] because `all` is not an image, and clap needs
/// one type for the argument.
#[derive(Clone, Copy, clap::ValueEnum)]
enum TeardownImage {
    Windows,
    WindowsBuilder,
    Linux,
    LinuxBuilder,
    All,
}

impl From<TeardownImage> for teardown::Selection {
    fn from(value: TeardownImage) -> Self {
        match value {
            TeardownImage::Windows => Self::One(Image::Windows),
            TeardownImage::WindowsBuilder => Self::One(Image::WindowsBuilder),
            TeardownImage::Linux => Self::One(Image::Linux),
            TeardownImage::LinuxBuilder => Self::One(Image::LinuxBuilder),
            TeardownImage::All => Self::All,
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
            desktop,
            screens,
        } => e2e::run(&runner, target, keep, allow_expired_image, desktop, screens),
        Command::Dist {
            target,
            keep,
            no_verify,
            no_cache,
            allow_expired_image,
            allow_dirty,
        } => dist::run(
            &runner,
            dist::Options {
                which: target,
                keep,
                verify: !no_verify,
                cache: !no_cache,
                allow_expired: allow_expired_image,
                allow_dirty,
            },
        ),
        Command::BakeIcon { review } => bake_icon::run(review),
        Command::BakeStars { input, output } => bake_stars::run(&input, &output),
        Command::Vm { command } => match command {
            VmCommand::Doctor => doctor::run(&runner),
            VmCommand::BuildImage { image } => build_image::run(&runner, image),
            VmCommand::Setup => run_setup(&runner),
            VmCommand::Up {
                image,
                allow_expired_image,
                desktop,
                screens,
            } => vm::up(&runner, image, allow_expired_image, desktop, screens),
            VmCommand::Stop { image } => vm::stop(&runner, image),
            VmCommand::Start { image } => vm::start(&runner, image),
            VmCommand::Ssh { image, command } => vm::ssh(&runner, image, &command),
            VmCommand::View { image } => vm::view(&runner, image),
            VmCommand::Smoke {
                image,
                keep,
                desktop,
            } => vm::smoke(&runner, image, keep, desktop),
            VmCommand::Status => vm::status(&runner),
            VmCommand::Down { image } => vm::down(&runner, image.into()),
            VmCommand::Purge {
                image,
                vm,
                image_only,
                iso,
                cache,
                force,
            } => vm::purge(
                &runner,
                image.into(),
                teardown::Scope::from_flags(vm, image_only, iso, cache),
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// The two teardowns take their own enum, because `all` is not an image and
    /// clap needs one type for the argument. Nothing else joins the two lists,
    /// so a fifth image would leave `vm down` and `vm purge` unable to name it
    /// with nothing failing anywhere, which is the same pair of lists
    /// `the_guest_accepts_exactly_the_sessions_the_host_can_ask_for` exists to
    /// hold together.
    #[test]
    fn the_teardowns_can_name_every_image_and_nothing_else() {
        use clap::ValueEnum;
        let named: Vec<teardown::Selection> = TeardownImage::value_variants()
            .iter()
            .copied()
            .map(teardown::Selection::from)
            .collect();
        for image in Image::ALL {
            assert!(
                named
                    .iter()
                    .any(|s| matches!(s, teardown::Selection::One(one) if *one == image)),
                "no `vm down`/`vm purge` argument names {image}"
            );
        }
        assert!(
            named.iter().any(|s| matches!(s, teardown::Selection::All)),
            "nothing names all of them"
        );
        assert_eq!(named.len(), Image::ALL.len() + 1);
    }

    /// A flag nobody documented is a flag nobody finds. Both documents spell
    /// `dist` out in one line, and the line has to be the command as it is:
    /// `--allow-expired-image` was in neither until this test asked.
    #[test]
    fn the_docs_spell_out_every_flag_dist_takes() {
        let cli = Cli::command();
        let dist = cli.find_subcommand("dist").expect("a dist subcommand");
        let flags: Vec<String> = dist
            .get_arguments()
            .filter_map(clap::Arg::get_long)
            .filter(|long| *long != "help" && *long != "version")
            .map(|long| format!("--{long}"))
            .collect();
        assert!(flags.len() >= 5, "{flags:?}");

        for doc in ["CLAUDE.md", "docs/vm-setup.md"] {
            let path = store::repo_root().join(doc);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            let usage = text
                .lines()
                .find(|line| line.starts_with("cargo xtask dist ["))
                .unwrap_or_else(|| panic!("{doc} has no `cargo xtask dist [...]` usage line"));
            for flag in &flags {
                assert!(usage.contains(flag), "{doc} does not name {flag}: {usage}");
            }
        }
    }
}
