//! The one owner of every place that belongs to the user rather than to the
//! process: the data folder and what lives in it, the config file, the caches,
//! the memory metrics, the pictures folder, and the state that is not a path
//! (the autostart entry, the browser and file manager, the desktop wallpaper).
//!
//! A process reaches any of it only after [`claim`], which the binary's `main`
//! calls first and nothing else calls. In a test process nothing has claimed,
//! so resolving a location or calling [`require`] panics, and the test that
//! reached for the user's files fails instead of writing to them. A test that
//! wants to see a write holds [`scratch_for_this_thread`], under which the
//! locations resolve into a folder of its own.
//!
//! The claim is checked before any environment override is read, so an
//! override exported in a developer's shell opens no path around it.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Moves the data folder, and with it everything that has no override of its
/// own: the log files, the wallpaper output, the instance lock on macOS.
const ENV_DATA_DIR: &str = "SUNLIT_EARTH_DATA_DIR";
/// Moves the config file.
const ENV_CONFIG: &str = "SUNLIT_EARTH_CONFIG";
/// Moves the cloud cache and the tile cache.
const ENV_CACHE_DIR: &str = "SUNLIT_EARTH_CACHE_DIR";
/// Moves the folder of the memory metrics CSV.
const ENV_METRICS_DIR: &str = "SUNLIT_EARTH_METRICS_DIR";

/// The data folder's name under the platform's local data directory.
const APP_FOLDER: &str = "SunlitEarth";
/// The config file's name in the data folder.
const CONFIG_FILE_NAME: &str = "config.toml";

static CLAIMED: OnceLock<()> = OnceLock::new();

thread_local! {
    static SCRATCH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Let this process reach the user's files and state.
///
/// Called once, as the first statement of the binary's `main`. The library
/// never calls it, and no test does.
pub fn claim() {
    let _ = CLAIMED.set(());
}

/// Fail unless this process has claimed user state.
///
/// For state that is not a path, which a scratch folder cannot stand in for,
/// so it fails under [`scratch_for_this_thread`] too.
///
/// :param what: the thing about to be reached, for the panic message
///
/// # Panics
///
/// When nothing has claimed user state, which is every test process.
#[track_caller]
pub fn require(what: &str) {
    if CLAIMED.get().is_none() {
        unclaimed(what);
    }
}

/// The folder this application keeps its own files in.
///
/// `SUNLIT_EARTH_DATA_DIR` where set, else `SunlitEarth` under
/// `dirs::data_local_dir`: `%LOCALAPPDATA%\SunlitEarth` on Windows,
/// `~/.local/share/SunlitEarth` on Linux and
/// `~/Library/Application Support/SunlitEarth` on macOS. `None` on a system
/// with no local data directory.
///
/// # Panics
///
/// When this process has not claimed user state and the thread holds no
/// scratch folder.
#[track_caller]
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    sources("the data folder").data_dir()
}

/// The config file: `SUNLIT_EARTH_CONFIG`, else `config.toml` in
/// [`data_dir`].
///
/// # Panics
///
/// As [`data_dir`].
#[track_caller]
#[must_use]
pub fn config_file() -> Option<PathBuf> {
    sources("the config file").config_file()
}

/// The folder of the cloud cache and the tile cache:
/// `SUNLIT_EARTH_CACHE_DIR`, else [`data_dir`].
///
/// # Panics
///
/// As [`data_dir`].
#[track_caller]
#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    sources("the cache folder").cache_dir()
}

/// The folder of the memory metrics CSV: `SUNLIT_EARTH_METRICS_DIR`, else
/// [`data_dir`].
///
/// # Panics
///
/// As [`data_dir`].
#[track_caller]
#[must_use]
pub fn metrics_dir() -> Option<PathBuf> {
    sources("the metrics folder").metrics_dir()
}

/// Where an export starts: the user's pictures folder where it exists, else
/// the home folder.
///
/// # Panics
///
/// As [`data_dir`].
#[track_caller]
#[must_use]
pub fn pictures_dir() -> Option<PathBuf> {
    sources("the pictures folder").pictures_dir()
}

/// Resolve every location of this module under `dir` on the calling thread,
/// for as long as the guard lives.
///
/// The environment overrides are ignored under it, and [`require`] still
/// fails. Other threads, those the code under test starts included, see no
/// scratch folder. Guards nest: dropping one puts back what was there before.
///
/// :param dir: the folder to resolve under, which the caller owns
#[doc(hidden)]
#[must_use = "the scratch folder applies only while the guard lives"]
pub fn scratch_for_this_thread(dir: &Path) -> ScratchGuard {
    let previous = SCRATCH.with(|slot| slot.replace(Some(dir.to_path_buf())));
    ScratchGuard {
        previous,
        _this_thread: PhantomData,
    }
}

/// Holds a thread's scratch folder; see [`scratch_for_this_thread`].
#[doc(hidden)]
pub struct ScratchGuard {
    previous: Option<PathBuf>,
    _this_thread: PhantomData<*const ()>,
}

impl Drop for ScratchGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        let _ = SCRATCH.try_with(|slot| *slot.borrow_mut() = previous);
    }
}

/// The inputs every location is resolved from.
#[derive(Debug, Default)]
struct Sources {
    data_dir: Option<PathBuf>,
    config_file: Option<PathBuf>,
    cache_dir: Option<PathBuf>,
    metrics_dir: Option<PathBuf>,
    local_data: Option<PathBuf>,
    pictures: Option<PathBuf>,
    home: Option<PathBuf>,
}

impl Sources {
    /// The user's own: the environment overrides and what `dirs` answers.
    #[expect(
        clippy::disallowed_methods,
        reason = "the one place the user's folders are asked for"
    )]
    fn user() -> Self {
        let env = |name| crate::env_override(name).map(PathBuf::from);
        Self {
            data_dir: env(ENV_DATA_DIR),
            config_file: env(ENV_CONFIG),
            cache_dir: env(ENV_CACHE_DIR),
            metrics_dir: env(ENV_METRICS_DIR),
            local_data: dirs::data_local_dir(),
            pictures: dirs::picture_dir(),
            home: dirs::home_dir(),
        }
    }

    /// Everything under `root`, with no override.
    fn scratch(root: &Path) -> Self {
        Self {
            data_dir: Some(root.to_path_buf()),
            home: Some(root.to_path_buf()),
            ..Self::default()
        }
    }

    fn data_dir(&self) -> Option<PathBuf> {
        self.data_dir
            .clone()
            .or_else(|| Some(self.local_data.as_ref()?.join(APP_FOLDER)))
    }

    fn config_file(&self) -> Option<PathBuf> {
        self.config_file
            .clone()
            .or_else(|| Some(self.data_dir()?.join(CONFIG_FILE_NAME)))
    }

    fn cache_dir(&self) -> Option<PathBuf> {
        self.cache_dir.clone().or_else(|| self.data_dir())
    }

    fn metrics_dir(&self) -> Option<PathBuf> {
        self.metrics_dir.clone().or_else(|| self.data_dir())
    }

    fn pictures_dir(&self) -> Option<PathBuf> {
        self.pictures
            .clone()
            .filter(|folder| folder.is_dir())
            .or_else(|| self.home.clone())
    }
}

/// The sources for this thread: its scratch folder, else the user's own once
/// the process has claimed them.
#[track_caller]
fn sources(what: &str) -> Sources {
    if let Some(root) = SCRATCH.with(|slot| slot.borrow().clone()) {
        return Sources::scratch(&root);
    }
    require(what);
    Sources::user()
}

#[track_caller]
fn unclaimed(what: &str) -> ! {
    panic!(
        "{what} belongs to the user, and this process has not claimed user state. \
         Only the binary's main claims it; a test that needs a location holds \
         sunlit_core::user_state::scratch_for_this_thread, and one that needs \
         the real state is an e2e case. See docs/testing.md, \"User state\"."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ScratchDir;

    const UNCLAIMED: &str = "has not claimed user state";

    #[test]
    #[should_panic(expected = "has not claimed user state")]
    fn the_data_folder_is_refused_unclaimed() {
        let _ = data_dir();
    }

    #[test]
    #[should_panic(expected = "has not claimed user state")]
    fn the_config_file_is_refused_unclaimed() {
        let _ = config_file();
    }

    #[test]
    #[should_panic(expected = "has not claimed user state")]
    fn the_cache_folder_is_refused_unclaimed() {
        let _ = cache_dir();
    }

    #[test]
    #[should_panic(expected = "has not claimed user state")]
    fn the_metrics_folder_is_refused_unclaimed() {
        let _ = metrics_dir();
    }

    #[test]
    #[should_panic(expected = "has not claimed user state")]
    fn the_pictures_folder_is_refused_unclaimed() {
        let _ = pictures_dir();
    }

    #[test]
    #[should_panic(expected = "the startup entry belongs to the user")]
    fn state_that_is_not_a_path_is_refused_unclaimed() {
        require("the startup entry");
    }

    #[test]
    #[should_panic(expected = "the desktop wallpaper belongs to the user")]
    fn state_that_is_not_a_path_is_refused_under_a_scratch_folder() {
        let scratch = ScratchDir::new("user_state_require");
        let _guard = scratch_for_this_thread(scratch.path());
        require("the desktop wallpaper");
    }

    #[test]
    fn every_location_resolves_under_the_scratch_folder() {
        let scratch = ScratchDir::new("user_state_scratch");
        let _guard = scratch_for_this_thread(scratch.path());
        let root = scratch.path();
        assert_eq!(data_dir().as_deref(), Some(root));
        assert_eq!(config_file(), Some(root.join(CONFIG_FILE_NAME)));
        assert_eq!(cache_dir().as_deref(), Some(root));
        assert_eq!(metrics_dir().as_deref(), Some(root));
        assert_eq!(pictures_dir().as_deref(), Some(root));
    }

    #[test]
    fn a_scratch_folder_reads_no_override() {
        let root = Path::new("/scratch");
        let scratch = Sources::scratch(root);
        assert!(scratch.config_file.is_none() && scratch.cache_dir.is_none());
        assert!(scratch.metrics_dir.is_none() && scratch.local_data.is_none());
        assert!(scratch.pictures.is_none());
        assert_eq!(scratch.data_dir().as_deref(), Some(root));
    }

    #[test]
    fn the_scratch_folder_belongs_to_one_thread() {
        let scratch = ScratchDir::new("user_state_thread");
        let _guard = scratch_for_this_thread(scratch.path());
        let other = std::thread::spawn(|| std::panic::catch_unwind(data_dir))
            .join()
            .expect("the thread itself ends");
        let message = other.expect_err("another thread resolved without a claim");
        let message = message
            .downcast_ref::<String>()
            .expect("a formatted panic message");
        assert!(message.contains(UNCLAIMED), "{message}");
    }

    #[test]
    fn dropping_a_guard_restores_the_one_before() {
        let outer = ScratchDir::new("user_state_outer");
        let inner = ScratchDir::new("user_state_inner");
        let _outer = scratch_for_this_thread(outer.path());
        {
            let _inner = scratch_for_this_thread(inner.path());
            assert_eq!(data_dir().as_deref(), Some(inner.path()));
        }
        assert_eq!(data_dir().as_deref(), Some(outer.path()));
    }

    #[test]
    fn the_data_folder_is_the_app_folder_under_local_data() {
        let sources = Sources {
            local_data: Some(PathBuf::from("/local")),
            ..Sources::default()
        };
        let data = Path::new("/local").join(APP_FOLDER);
        assert_eq!(sources.data_dir().as_deref(), Some(data.as_path()));
        assert_eq!(sources.config_file(), Some(data.join(CONFIG_FILE_NAME)));
        assert_eq!(sources.cache_dir().as_deref(), Some(data.as_path()));
        assert_eq!(sources.metrics_dir().as_deref(), Some(data.as_path()));
    }

    #[test]
    fn the_data_override_moves_everything_without_its_own() {
        let sources = Sources {
            data_dir: Some(PathBuf::from("/moved")),
            local_data: Some(PathBuf::from("/local")),
            ..Sources::default()
        };
        let moved = Path::new("/moved");
        assert_eq!(sources.data_dir().as_deref(), Some(moved));
        assert_eq!(sources.config_file(), Some(moved.join(CONFIG_FILE_NAME)));
        assert_eq!(sources.cache_dir().as_deref(), Some(moved));
        assert_eq!(sources.metrics_dir().as_deref(), Some(moved));
    }

    #[test]
    fn each_override_wins_for_its_own_location() {
        let sources = Sources {
            data_dir: Some(PathBuf::from("/moved")),
            config_file: Some(PathBuf::from("/elsewhere/custom.toml")),
            cache_dir: Some(PathBuf::from("/cache")),
            metrics_dir: Some(PathBuf::from("/metrics")),
            local_data: Some(PathBuf::from("/local")),
            ..Sources::default()
        };
        assert_eq!(
            sources.config_file().as_deref(),
            Some(Path::new("/elsewhere/custom.toml"))
        );
        assert_eq!(sources.cache_dir().as_deref(), Some(Path::new("/cache")));
        assert_eq!(
            sources.metrics_dir().as_deref(),
            Some(Path::new("/metrics"))
        );
        assert_eq!(sources.data_dir().as_deref(), Some(Path::new("/moved")));
    }

    #[test]
    fn no_local_data_directory_leaves_only_the_overrides() {
        let sources = Sources::default();
        assert_eq!(sources.data_dir(), None);
        assert_eq!(sources.config_file(), None);
        assert_eq!(sources.cache_dir(), None);
        let sources = Sources {
            config_file: Some(PathBuf::from("/custom.toml")),
            ..Sources::default()
        };
        assert_eq!(
            sources.config_file().as_deref(),
            Some(Path::new("/custom.toml"))
        );
    }

    #[test]
    fn pictures_fall_back_to_home_where_the_folder_is_missing() {
        let scratch = ScratchDir::new("user_state_pictures");
        let pictures = scratch.join("Pictures");
        let sources = Sources {
            pictures: Some(pictures.clone()),
            home: Some(scratch.path().to_path_buf()),
            ..Sources::default()
        };
        assert_eq!(sources.pictures_dir().as_deref(), Some(scratch.path()));
        std::fs::create_dir(&pictures).expect("create the pictures folder");
        assert_eq!(sources.pictures_dir(), Some(pictures));
        assert_eq!(Sources::default().pictures_dir(), None);
    }

    /// What reaches the user's state other than through this module.
    const TOKENS: [&str; 8] = [
        "dirs::",
        "env::home_dir",
        "\"HOME\"",
        "\"XDG_CONFIG_HOME\"",
        "\"APPDATA\"",
        "\"LOCALAPPDATA\"",
        "\"USERPROFILE\"",
        "HKEY_CURRENT_USER",
    ];

    /// The files that may name a token, and why.
    const ALLOWED: [(&str, &str, &str); 5] = [
        (
            "sunlit-app/src/autostart/windows.rs",
            "HKEY_CURRENT_USER",
            "Location::system() calls require, and the tests use scratch keys",
        ),
        (
            "sunlit-app/src/autostart/linux.rs",
            "\"XDG_CONFIG_HOME\"",
            "system_dir() calls require before reading it",
        ),
        (
            "sunlit-app/src/autostart/linux.rs",
            "\"HOME\"",
            "system_dir() calls require before reading it",
        ),
        (
            "sunlit-app/src/autostart/macos.rs",
            "\"HOME\"",
            "system_dir() calls require before reading it",
        ),
        (
            "sunlit-core/src/wallpaper/windows.rs",
            "HKEY_CURRENT_USER",
            "only a publish writes it, and SystemWallpaper's publish calls require",
        ),
    ];

    /// The one file that may name the app binary, so every spawn of it goes
    /// through `app_command()`.
    const SPAWNER: &str = "sunlit-app/tests/common/process.rs";
    const BINARY: [&str; 2] = [
        "env!(\"CARGO_BIN_EXE_sunlit-earth\")",
        "Command::new(binary())",
    ];

    fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
        let entries = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .map(|entry| entry.expect("a directory entry").path());
        for path in entries {
            if path.is_dir() {
                rust_files(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push(path);
            }
        }
    }

    #[test]
    fn nothing_reaches_user_state_around_this_module() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = Vec::new();
        for krate in ["sunlit-core", "sunlit-app", "sunlit-ui"] {
            for part in ["src", "tests"] {
                let dir = crates.join(krate).join(part);
                if dir.is_dir() {
                    rust_files(&dir, &mut files);
                }
            }
        }
        assert!(
            files.len() > 50,
            "the scan found only {} files",
            files.len()
        );

        let mut found = Vec::new();
        for path in &files {
            let name = path
                .strip_prefix(&crates)
                .expect("under the crates folder")
                .to_string_lossy()
                .replace('\\', "/");
            if name == "sunlit-core/src/user_state.rs" {
                continue;
            }
            let text = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            for token in TOKENS {
                let allowed = ALLOWED
                    .iter()
                    .any(|&(file, allowed, _)| file == name && allowed == token);
                if text.contains(token) && !allowed {
                    found.push(format!("{name} names {token}"));
                }
            }
            if BINARY.iter().any(|binary| text.contains(binary)) && name != SPAWNER {
                found.push(format!(
                    "{name} names the app binary; spawn it through app_command() in {SPAWNER}"
                ));
            }
        }
        assert!(
            found.is_empty(),
            "user state reached around sunlit_core::user_state (see docs/testing.md, \
             \"User state\"):\n{}",
            found.join("\n")
        );
    }

    #[test]
    fn every_allowance_is_still_needed() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        for (file, token, reason) in ALLOWED {
            let text = std::fs::read_to_string(crates.join(file))
                .unwrap_or_else(|e| panic!("read {file}: {e}"));
            assert!(
                text.contains(token),
                "{file} no longer names {token}, so its allowance ({reason}) can go"
            );
        }
    }
}
