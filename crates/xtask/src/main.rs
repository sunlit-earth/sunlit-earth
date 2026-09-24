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

use crate::commands::{
    bake_icon, bake_licenses, bake_stars, build_image, bundle, dist, doctor, e2e, manifests, setup,
    sweep, teardown, vm,
};
use crate::host::facts;
use crate::provider::desktop::{Desktop, SessionType};
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
        #[command(flatten)]
        session: SessionArgs,
        /// How many screens to give the guest. Linux guest only, and X11 only.
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
    /// Assemble a release bundle around a binary that is already built, and
    /// write the archive its platform's users open without a tool.
    Bundle(bundle::Options),
    /// Write the Scoop manifest, the Homebrew cask and the Homebrew formula for
    /// one release, from its archives.
    Manifests(manifests::Options),
    /// Regenerate a committed asset from its source.
    Bake {
        #[command(subcommand)]
        command: BakeCommand,
    },
    /// Delete build artifacts no recent build has used. Needs cargo-sweep:
    /// `cargo install cargo-sweep`.
    Sweep {
        /// Keep artifacts a build has used within this many days.
        #[arg(long, default_value_t = 7, value_name = "DAYS")]
        time: u32,
        /// Shrink the target directory to this size, oldest artifacts first.
        #[arg(long, default_value_t = 25, value_name = "GIB")]
        maxsize: u32,
        /// Report what both passes would delete, and delete nothing.
        #[arg(long)]
        dry_run: bool,
    },
}

/// The three committed assets that are generated rather than authored.
///
/// One verb with three objects rather than three sibling verbs: they all mean
/// "regenerate a committed asset from its source", they are all rare, and the
/// nested shape is the one `VmCommand` already established in this CLI.
#[derive(Subcommand)]
enum BakeCommand {
    /// Rasterize the icon SVGs into the outputs the app ships. The results are
    /// committed; rerun this when a source SVG changes.
    Icon {
        /// Write the small-size review sheet into this directory instead of
        /// baking. For the judgment a test cannot make.
        #[arg(long, value_name = "DIR")]
        review: Option<std::path::PathBuf>,
    },
    /// Bake the HYG star catalog into the runtime instance buffer format.
    Stars {
        /// HYG v4.4 CSV input.
        #[arg(long, value_name = "CSV")]
        input: std::path::PathBuf,
        /// Binary catalog output.
        #[arg(long, value_name = "BIN")]
        output: std::path::PathBuf,
    },
    /// Walk the shipping dependency tree and write the two third-party
    /// notices: the About window's crate list and the license texts that
    /// travel with the binary.
    Licenses,
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
        #[command(flatten)]
        session: SessionArgs,
        /// How many screens to give the guest, laid out left to right. Linux
        /// guest only, and X11 only; `vm view` opens one viewer per screen.
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
        #[command(flatten)]
        session: SessionArgs,
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

/// Which session a Linux guest logs into, as the three commands that boot one
/// take it.
#[derive(clap::Args, Clone, Copy)]
struct SessionArgs {
    /// Which desktop to log into. Linux guest only; the image default is KDE
    /// Plasma.
    #[arg(long)]
    desktop: Option<Desktop>,
    /// Which display server that desktop runs on. Linux guest only, Wayland for
    /// KDE and GNOME only; X11 when not given.
    #[arg(long)]
    session_type: Option<SessionType>,
}

impl SessionArgs {
    fn with_screens(self, screens: u16) -> vm::BootRequest {
        vm::BootRequest {
            desktop: self.desktop,
            session_type: self.session_type,
            screens,
        }
    }
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

#[expect(
    clippy::too_many_lines,
    reason = "one match arm per subcommand, and splitting the dispatch would only move it"
)]
fn main() -> ExitCode {
    let cli = Cli::parse();
    let runner = RealRunner;

    let result = match cli.command {
        Command::E2e {
            target,
            keep,
            allow_expired_image,
            session,
            screens,
        } => e2e::run(
            &runner,
            target,
            keep,
            allow_expired_image,
            session.with_screens(screens),
        ),
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
        Command::Bundle(options) => bundle::run(&runner, &options),
        Command::Manifests(options) => manifests::run(&options),
        Command::Bake { command } => match command {
            BakeCommand::Icon { review } => bake_icon::run(review),
            BakeCommand::Stars { input, output } => bake_stars::run(&input, &output),
            BakeCommand::Licenses => bake_licenses::run(&runner),
        },
        Command::Sweep {
            time,
            maxsize,
            dry_run,
        } => sweep::run(
            &runner,
            &sweep::Options {
                days: time,
                gibibytes: maxsize,
                dry_run,
            },
        ),
        Command::Vm { command } => match command {
            VmCommand::Doctor => doctor::run(&runner),
            VmCommand::BuildImage { image } => build_image::run(&runner, image),
            VmCommand::Setup => run_setup(&runner),
            VmCommand::Up {
                image,
                allow_expired_image,
                session,
                screens,
            } => vm::up(
                &runner,
                image,
                allow_expired_image,
                session.with_screens(screens),
            ),
            VmCommand::Stop { image } => vm::stop(&runner, image),
            VmCommand::Start { image } => vm::start(&runner, image),
            VmCommand::Ssh { image, command } => vm::ssh(&runner, image, &command),
            VmCommand::View { image } => vm::view(&runner, image),
            VmCommand::Smoke {
                image,
                keep,
                session,
            } => vm::smoke(&runner, image, keep, session.with_screens(1)),
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
        let _ = check_usage_lines("dist", 5, &["CLAUDE.md", "docs/vm-setup.md"]);
    }

    /// The same rule for the command the release runners drive.
    #[test]
    fn the_docs_spell_out_every_flag_bundle_takes() {
        let usage = check_usage_lines("bundle", 4, &["CLAUDE.md"]);
        for platform in <bundle::Platform as clap::ValueEnum>::value_variants() {
            let slug = platform.slug();
            assert!(
                usage.contains(slug),
                "CLAUDE.md does not name {slug}: {usage}"
            );
        }
    }

    /// Every long flag of a subcommand appears on that subcommand's usage line
    /// in each document, which is what keeps a usage line complete rather than
    /// merely present.
    fn check_usage_lines(subcommand: &str, least: usize, docs: &[&str]) -> String {
        let cli = Cli::command();
        let command = cli
            .find_subcommand(subcommand)
            .unwrap_or_else(|| panic!("a {subcommand} subcommand"));
        let flags: Vec<String> = command
            .get_arguments()
            .filter_map(clap::Arg::get_long)
            .filter(|long| *long != "help" && *long != "version")
            .map(|long| format!("--{long}"))
            .collect();
        assert!(flags.len() >= least, "{flags:?}");

        let prefix = format!("cargo xtask {subcommand} ");
        let mut found = String::new();
        for doc in docs {
            let path = store::repo_root().join(doc);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            let usage = text
                .lines()
                .find(|line| line.starts_with(&prefix))
                .unwrap_or_else(|| panic!("{doc} has no `{prefix}...` usage line"));
            for flag in &flags {
                assert!(usage.contains(flag), "{doc} does not name {flag}: {usage}");
            }
            found = usage.to_owned();
        }
        found
    }
}
