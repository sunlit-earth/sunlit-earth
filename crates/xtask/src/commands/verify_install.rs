//! `cargo xtask verify-install`: prove that an installed `sunlit-earth` finds
//! its textures, whichever package manager put it there.
//!
//! The comparison `bundle --verify` makes on an unpacked archive, pointed at the
//! command a package manager put on `PATH` instead: a Scoop shim, or a symlink
//! in Homebrew's `bin/`. That command is what users run, so it is what has to
//! reach the textures.

use std::path::{Path, PathBuf};

use crate::commands::bundle;
use crate::runner::Runner;

/// What one `cargo xtask verify-install` run was asked for.
#[derive(Debug, Clone, clap::Args)]
pub struct Options {
    /// The installed command, as `PATH` resolves it.
    #[arg(long, value_name = "PATH")]
    pub exe: PathBuf,
    /// The directory a fresh working directory is made in, the system's
    /// temporary directory by default. Nothing already in it is touched.
    #[arg(long, value_name = "DIR")]
    pub work: Option<PathBuf>,
}

/// The name of the working directory this command makes, before its suffix.
const WORK_PREFIX: &str = "sunlit-earth-verify-install";

/// Render twice through the installed command, from a working directory this
/// run makes and is the only one to delete.
///
/// The working directory matters to the lookup only through `./textures`, the
/// one candidate relative to it; everything else is read from the executable's
/// own path. A directory made empty here cannot answer that candidate, so the
/// render can only find its textures through the executable, wherever `--work`
/// points.
pub fn run(runner: &dyn Runner, options: &Options) -> Result<u8, String> {
    if !options.exe.is_file() {
        return Err(format!(
            "there is no installed command at {}",
            options.exe.display()
        ));
    }
    let parent = options.work.clone().unwrap_or_else(std::env::temp_dir);
    let work = fresh_dir(&parent)?;

    println!(
        "verifying {} from {}",
        options.exe.display(),
        work.display()
    );
    // On a failure the directory stays, since its two renders are what says
    // which of them went wrong.
    let delta = bundle::render_comparison(runner, &options.exe, &work)
        .map_err(|e| format!("{e}\n(the renders are in {})", work.display()))?;
    std::fs::remove_dir_all(&work).map_err(|e| format!("cannot remove {}: {e}", work.display()))?;
    println!("verified: the two renders differ by {delta:.2} of a channel step");
    Ok(0)
}

/// Make a directory under `parent` that did not exist before this call.
fn fresh_dir(parent: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    for attempt in 0..1000_u32 {
        let dir = parent.join(format!("{WORK_PREFIX}-{}-{attempt}", std::process::id()));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("cannot create {}: {e}", dir.display())),
        }
    }
    Err(format!(
        "{} already holds a thousand {WORK_PREFIX} directories of this process id",
        parent.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::CommandOutput;
    use crate::runner::fake::FakeRunner;

    /// A directory the user owns, with something in it that must survive.
    fn owned_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("xtask_verify_install_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("textures")).expect("the directory");
        std::fs::write(dir.join("textures").join("world.jxl"), b"texture").expect("a texture");
        std::fs::write(dir.join("notes.txt"), b"mine").expect("a file");
        dir
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("the directory")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_missing_command_is_refused_before_the_working_directory_is_touched() {
        let dir = owned_dir("missing");
        let before = listing(&dir);
        let refusal = run(
            &FakeRunner::new(),
            &Options {
                exe: dir.join("no-such-command"),
                work: Some(dir.clone()),
            },
        )
        .expect_err("no command");
        assert!(refusal.contains("no installed command"), "{refusal}");
        assert_eq!(listing(&dir), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_named_directory_and_everything_in_it_survive_a_run() {
        let dir = owned_dir("survives");
        let exe = dir.join("sunlit-earth");
        std::fs::write(&exe, b"").expect("the command");
        let before = listing(&dir);

        // Every command succeeds and writes nothing, so the run gets as far as
        // reading the renders and fails there, with its own directory made.
        let runner = FakeRunner::new().on("sunlit-earth", CommandOutput::ok(""));
        let refusal = run(
            &runner,
            &Options {
                exe: exe.clone(),
                work: Some(dir.clone()),
            },
        )
        .expect_err("no renders were written");
        assert!(refusal.contains("the renders are in"), "{refusal}");

        let after = listing(&dir);
        for name in &before {
            assert!(after.contains(name), "{name} is gone: {after:?}");
        }
        assert_eq!(
            std::fs::read(dir.join("textures").join("world.jxl")).expect("the texture"),
            b"texture"
        );
        assert_eq!(
            std::fs::read(dir.join("notes.txt")).expect("the file"),
            b"mine"
        );
        let made: Vec<&String> = after.iter().filter(|name| !before.contains(name)).collect();
        assert_eq!(made.len(), 1, "{after:?}");
        assert!(made[0].starts_with(WORK_PREFIX), "{made:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fresh_directory_never_reuses_one_that_exists() {
        let dir = owned_dir("fresh");
        let first = fresh_dir(&dir).expect("one");
        std::fs::write(first.join("kept"), b"").expect("a file in it");
        let second = fresh_dir(&dir).expect("another");
        assert_ne!(first, second);
        assert!(first.join("kept").is_file());
        assert_eq!(listing(&second), Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
