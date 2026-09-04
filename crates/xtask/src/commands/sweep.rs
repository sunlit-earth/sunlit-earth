//! Delete build artifacts no recent build has used.
//!
//! Cargo never removes an artifact it has stopped using. A toolchain bump, a
//! dependency bump or an edited profile mints a new hash for every unit and
//! leaves the old files where they were, and a build that alternates between
//! feature sets or test targets keeps several sets alive at once, so the
//! directory only ever grows. cargo-sweep is the tool that removes them; this
//! command is the policy around it: which passes to run, in which order, and
//! when the age pass cannot be trusted to mean what it says.

use std::path::{Path, PathBuf};

use crate::runner::{Cmd, Runner};
use crate::store;
use crate::util;

/// The external tool, as `cargo install cargo-sweep` leaves it on `PATH`.
///
/// Invoked as the binary with its own `sweep` subcommand rather than through
/// `cargo sweep`, so that [`Runner::which`] answering yes is the same question
/// as the invocation working.
const TOOL: &str = "cargo-sweep";

/// What the passes are given.
pub struct Options {
    /// Keep artifacts a build has used within this many days.
    pub days: u32,
    /// The size the directory is shrunk to, in gibibytes.
    pub gibibytes: u32,
    /// Report what would go and delete nothing.
    pub dry_run: bool,
}

/// Whether this volume records file access times.
///
/// cargo-sweep dates an artifact by the access time of its `.fingerprint`
/// files, and cargo reads those on every build whose graph contains the unit,
/// whether or not it recompiles it. That is what makes the age pass mean "no
/// build has needed this in N days" rather than "this was not rebuilt in N
/// days", and the difference is most of the target directory: a dependency
/// compiled once and reused ever since carries a months-old write time and a
/// fresh access time. Where the volume does not record access times the two
/// collapse into one and the age pass deletes what every build still uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LastAccess {
    Recorded,
    NotRecorded,
    /// No answer, which is every host but Windows: on Linux the default
    /// `relatime` records what the age pass needs, and a `noatime` mount is a
    /// deliberate choice by whoever made it rather than something to work
    /// around here.
    Unknown,
}

/// Run the command.
pub fn run(runner: &dyn Runner, options: &Options) -> Result<u8, String> {
    if runner.which(TOOL).is_none() {
        return Err(missing_tool());
    }

    let repo = store::repo_root();
    println!("sweeping {}", repo.display());

    let mut passes = Vec::new();
    if last_access(runner) == LastAccess::NotRecorded {
        println!("{}", access_times_off(options.days));
    } else {
        println!(
            "  age: dropping what no build has used in {} days",
            options.days
        );
        passes.push(age_pass(&repo, options));
    }
    println!(
        "  size: shrinking to {} GiB, oldest first",
        options.gibibytes
    );
    passes.push(size_pass(&repo, options));

    for pass in &passes {
        let code = runner
            .stream(pass)
            .map_err(|e| format!("cannot run {}: {e}", pass.display()))?;
        if code != 0 {
            return Err(format!("{} exited {code}", pass.display()));
        }
    }

    let target = target_dir();
    let incremental: u64 = incremental_dirs(&target).iter().map(|d| dir_size(d)).sum();
    if let Some(note) = incremental_note(incremental) {
        println!("{note}");
    }
    Ok(0)
}

/// Everything no build has used in `days` days.
fn age_pass(repo: &Path, options: &Options) -> Cmd {
    pass(
        repo,
        options,
        ["--time".to_owned(), options.days.to_string()],
    )
}

/// Oldest first until the directory fits, which is the pass that catches a
/// profile change: it orphans every dependency at once, and all of them were
/// in use yesterday.
fn size_pass(repo: &Path, options: &Options) -> Cmd {
    pass(
        repo,
        options,
        ["--maxsize".to_owned(), format!("{}GiB", options.gibibytes)],
    )
}

/// One invocation, given the two arguments that name its mode.
///
/// cargo-sweep takes exactly one mode and refuses two, which is why the age
/// and size passes are separate runs. It wants the project rather than the
/// target directory, and asks `cargo metadata` where that directory is, so a
/// moved `CARGO_TARGET_DIR` is honoured without this command reading it.
fn pass(repo: &Path, options: &Options, mode: [String; 2]) -> Cmd {
    let cmd = Cmd::new(TOOL).arg("sweep").args(mode);
    let cmd = if options.dry_run {
        cmd.arg("--dry-run")
    } else {
        cmd
    };
    cmd.arg(repo.display().to_string())
}

/// What Windows says about access times, and nothing on any other host.
fn last_access(runner: &dyn Runner) -> LastAccess {
    if !cfg!(windows) {
        return LastAccess::Unknown;
    }
    let query = Cmd::new("fsutil").args(["behavior", "query", "DisableLastAccess"]);
    match runner.capture(&query) {
        Ok(output) if output.success() => parse_last_access(&output.stdout),
        _ => LastAccess::Unknown,
    }
}

/// Read `fsutil behavior query DisableLastAccess`.
///
/// It answers `DisableLastAccess = 2  (...)`, where the parenthesis is in the
/// host's language and the number is not. The low bit is the switch and the
/// other says who set it, so 0 and 2 record access times and 1 and 3 do not.
fn parse_last_access(stdout: &str) -> LastAccess {
    let value = stdout
        .split_once('=')
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .and_then(|token| token.parse::<u8>().ok());
    match value {
        Some(0 | 2) => LastAccess::Recorded,
        Some(1 | 3) => LastAccess::NotRecorded,
        _ => LastAccess::Unknown,
    }
}

/// What to say about the incremental caches, and nothing when there are none.
///
/// Both passes work from `.fingerprint`, and nothing under `incremental/` is an
/// artifact either of them can date, so both walk past it: of the 21,639 files
/// a `--maxsize 1` dry run named on this workspace, none were in one. Cargo
/// keeps the cache of every unit it has ever built and removes the cache of
/// none it has stopped building, so a directory that has seen a few toolchains
/// and a few profiles holds a session per unit per shape.
fn incremental_note(bytes: u64) -> Option<String> {
    (bytes > 0).then(|| {
        format!(
            "  incremental: {} in caches neither pass touches. Deleting \
             `<target>/*/incremental` is what reclaims them, at the cost of one cold \
             build of this workspace's own crates.",
            util::format_bytes(bytes)
        )
    })
}

/// Where the artifacts are, by the rule `dist` uses: `CARGO_TARGET_DIR` when a
/// developer has moved it, and `<repo>/target` otherwise.
///
/// The passes themselves need none of this, since cargo-sweep asks `cargo
/// metadata`; only the incremental caches are looked at directly.
fn target_dir() -> PathBuf {
    util::env_var("CARGO_TARGET_DIR")
        .map_or_else(|| store::repo_root().join("target"), PathBuf::from)
}

/// The incremental caches under a target directory.
///
/// Two levels deep, because `--target <triple>` puts a directory of its own
/// between the target directory and the profile.
fn incremental_dirs(target: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for profile in subdirectories(target) {
        for candidate in std::iter::once(profile.join("incremental")).chain(
            subdirectories(&profile)
                .iter()
                .map(|d| d.join("incremental")),
        ) {
            if candidate.is_dir() {
                found.push(candidate);
            }
        }
    }
    found
}

fn subdirectories(path: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect()
}

/// What a directory holds, following it down. An unreadable entry counts as
/// nothing: this figure ends up in a sentence, not in a decision.
fn dir_size(path: &Path) -> u64 {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => dir_size(&entry.path()),
            Ok(_) => entry.metadata().map_or(0, |m| m.len()),
            Err(_) => 0,
        })
        .sum()
}

fn missing_tool() -> String {
    format!("{TOOL} is not on PATH. Install it with `cargo install cargo-sweep`")
}

fn access_times_off(days: u32) -> String {
    format!(
        "  age: skipped, because this volume does not record file access times. \
         `--time {days}` would read the time each artifact was written instead and delete \
         the dependencies every build reuses rather than the ones nothing needs. \
         `fsutil behavior set DisableLastAccess 2` (elevated) turns them on."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Options {
        Options {
            days: 7,
            gibibytes: 25,
            dry_run: false,
        }
    }

    /// The number carries the answer on every host, and the sentence after it
    /// carries it on none: this developer's Windows answers in German.
    #[test]
    fn the_fsutil_answer_is_read_by_its_number_and_not_its_words() {
        assert_eq!(
            parse_last_access(
                "DisableLastAccess = 2  (Vom System verwaltet, Updates fuer letzte Zugriffszeit ENABLED)"
            ),
            LastAccess::Recorded
        );
        assert_eq!(
            parse_last_access("DisableLastAccess = 0  (User Managed, Enabled)"),
            LastAccess::Recorded
        );
        assert_eq!(
            parse_last_access("DisableLastAccess = 1  (User Managed, Disabled)"),
            LastAccess::NotRecorded
        );
        assert_eq!(
            parse_last_access("DisableLastAccess = 3  (System Managed, Disabled)"),
            LastAccess::NotRecorded
        );
    }

    /// An answer this cannot read is not an answer that the age pass is unsafe.
    /// Both passes run, which is what happens on every host that has no
    /// `fsutil` at all.
    #[test]
    fn an_unreadable_answer_is_unknown_rather_than_either_verdict() {
        assert_eq!(parse_last_access(""), LastAccess::Unknown);
        assert_eq!(
            parse_last_access("Error:  Access is denied."),
            LastAccess::Unknown
        );
        assert_eq!(
            parse_last_access("DisableLastAccess = 7"),
            LastAccess::Unknown
        );
    }

    /// cargo-sweep's modes are mutually exclusive, so each pass names exactly
    /// one of them, and the path it is given is the project: handed the target
    /// directory it looks for a manifest inside it and fails.
    #[test]
    fn each_pass_names_one_mode_and_the_project_rather_than_the_target_directory() {
        let repo = Path::new("C:/repo");
        assert_eq!(
            age_pass(repo, &options()).args,
            ["sweep", "--time", "7", "C:/repo"]
        );
        assert_eq!(
            size_pass(repo, &options()).args,
            ["sweep", "--maxsize", "25GiB", "C:/repo"]
        );
    }

    /// Neither pass can reach the incremental caches, so the command reports
    /// them rather than leaving the difference between what it shrank and what
    /// the directory still holds unexplained.
    #[test]
    fn the_incremental_caches_are_reported_when_the_directory_has_any() {
        assert_eq!(incremental_note(0), None);
        let note = incremental_note(3 * 1024 * 1024 * 1024).expect("a note");
        assert!(note.contains("3.0 GiB"), "{note}");
        assert!(note.contains("incremental"), "{note}");
    }

    #[test]
    fn a_dry_run_asks_both_passes_for_one() {
        let repo = Path::new("/repo");
        let options = Options {
            dry_run: true,
            ..options()
        };
        for cmd in [age_pass(repo, &options), size_pass(repo, &options)] {
            assert!(cmd.args.iter().any(|a| a == "--dry-run"), "{:?}", cmd.args);
        }
    }
}
