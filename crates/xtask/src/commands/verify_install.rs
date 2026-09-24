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
    /// Where the two renders are written. Has to be outside the install.
    #[arg(long, value_name = "DIR")]
    pub work: Option<PathBuf>,
}

/// The working directory when `--work` is not given.
const DEFAULT_WORK: &str = "sunlit-earth-verify-install";

pub fn run(runner: &dyn Runner, options: &Options) -> Result<u8, String> {
    if !options.exe.is_file() {
        return Err(format!(
            "there is no installed command at {}",
            options.exe.display()
        ));
    }
    let work = options
        .work
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join(DEFAULT_WORK));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| format!("cannot create {}: {e}", work.display()))?;
    refuse_work_inside_install(&options.exe, &work)?;

    println!("verifying {}", options.exe.display());
    let delta = bundle::render_comparison(runner, &options.exe, &work)?;
    let _ = std::fs::remove_dir_all(&work);
    println!("verified: the two renders differ by {delta:.2} of a channel step");
    Ok(0)
}

/// A working directory inside the install would hold the install's own
/// `textures/` as `./textures` or on the way up, and the render would find it
/// without the lookup this exists to test.
fn refuse_work_inside_install(exe: &Path, work: &Path) -> Result<(), String> {
    let canonical = |path: &Path| {
        std::fs::canonicalize(path).map_err(|e| format!("cannot resolve {}: {e}", path.display()))
    };
    let work = canonical(work)?;
    for exe in [exe.to_path_buf(), canonical(exe)?] {
        if let Some(install) = exe.parent()
            && work.starts_with(canonical(install)?)
        {
            return Err(format!(
                "the working directory {} is inside {}, where the textures would be \
                 found without the lookup from the executable",
                work.display(),
                install.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_working_directory_inside_the_install_is_refused() {
        let root =
            std::env::temp_dir().join(format!("xtask_verify_install_{}", std::process::id()));
        let install = root.join("install");
        std::fs::create_dir_all(install.join("work")).expect("the install");
        std::fs::create_dir_all(root.join("outside")).expect("the outside directory");
        let exe = install.join("sunlit-earth");
        std::fs::write(&exe, b"").expect("the executable");

        let refusal = refuse_work_inside_install(&exe, &install.join("work"))
            .expect_err("a directory inside the install");
        assert!(refusal.contains("inside"), "{refusal}");
        refuse_work_inside_install(&exe, &root.join("outside")).expect("a directory beside it");

        let _ = std::fs::remove_dir_all(&root);
    }
}
