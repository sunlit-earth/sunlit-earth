//! Wallpaper export: where the file goes, how it is encoded, and on Windows the
//! monitor query and `SystemParametersInfoW` that make it the desktop.
//!
//! The first two halves are the same everywhere, which is why this module is no
//! longer Windows-only: every platform with a setter writes the same PNG into
//! the same directory under its own data directory, and only the act of handing
//! it to the desktop differs. The Linux side of that act is a table of per-desktop
//! commands in [`crate::desktop`], because there is no system call to make
//! there; the Win32 half is here, behind a `cfg`, because it is one.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing::debug;

#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
pub(crate) use linux::{check_supported, set_wallpaper_job};
#[cfg(windows)]
pub(crate) use windows::{enumerate_monitors, set_wallpaper_job};

/// Return the wallpaper output directory, creating it if it does not exist.
///
/// [`crate::app_data_dir`], which is where the config file and the cloud cache
/// already are.
///
/// In a unit-test build this refuses to resolve to that live directory: a publish
/// under test writes real PNGs and sweeps what is there, so a test that reached
/// the real directory would overwrite the developer's own wallpaper. The tests
/// point it at a scratch directory through `tests::Scratch`, and a resolution
/// with no scratch set panics rather than falling back to the live directory, so
/// a test that forgets the isolation fails loudly in CI instead of quietly on a
/// desktop.
pub(crate) fn wallpaper_dir() -> Result<PathBuf, String> {
    #[cfg(test)]
    let dir = scratch_override().expect(
        "a unit test resolved wallpaper_dir() without a scratch override; wrap the \
         publish in tests::Scratch so it cannot write to the real desktop directory",
    );
    #[cfg(not(test))]
    let dir = data_dir_wallpaper_path()?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create wallpaper directory: {e}"))?;
    Ok(dir)
}

/// The live wallpaper directory under this system's local data directory.
///
/// The real resolution, kept apart from [`wallpaper_dir`] so the test build can
/// redirect the latter without losing a way to check this one, and so nothing in
/// a test ever creates it by accident.
fn data_dir_wallpaper_path() -> Result<PathBuf, String> {
    crate::app_data_dir().ok_or_else(|| "no local data directory on this system".to_owned())
}

/// The scratch directory the current test thread publishes into, if any.
///
/// Thread-local so a serialized test owns its own directory, and consulted by
/// [`wallpaper_dir`] ahead of the live directory.
#[cfg(test)]
fn scratch_override() -> Option<PathBuf> {
    tests::SCRATCH_DIR.with(|dir| dir.borrow().clone())
}

/// One publish's own directory, and the files it wrote.
///
/// Every publish gets a directory of its own, `gen-<id>` under the wallpaper
/// directory, and writes its images into it: `gen-<id>/<index>.png` per screen,
/// plus `gen-<id>/canvas.png` where the mode spans them. No two publishes ever
/// share a directory, so no path a desktop was handed is ever written or deleted
/// again while that desktop still holds it. That is what keeps a shell that
/// watches its wallpaper file, which Plasma does, from decoding a half-written
/// image or asserting on a file that changed under a watch it had moved on from.
///
/// The unique directory also satisfies, by construction and for every publish
/// rather than every other one, the requirement a desktop keys on: a new image
/// arrives on a path the desktop is not already showing, so the setter has a file
/// to load. plasmashell ignores a second `plasma-apply-wallpaperimage` of a file
/// it already shows, and a `gsettings` or `xfconf-query` write of the value
/// already stored notifies nothing; a fresh path per publish sidesteps both.
///
/// Windows needs none of this in principle, since `SystemParametersInfoW` reads
/// whatever path it is handed, but it writes through the same directories so
/// there is one lifecycle rather than two.
#[derive(Clone)]
struct Generation {
    dir: PathBuf,
    files: Vec<PathBuf>,
}

/// The generation the last publish in this process wrote.
///
/// Remembered rather than asked of the filesystem every time, because two
/// publishes can land inside one tick of the clock that stamps their
/// modification times, and the read-back has to name the newer of them. Empty
/// until this process has published, where the modification times are all there
/// is to go on. It also names the immediately previous generation the sweep must
/// keep.
///
/// A poisoned lock is taken as it stands: the value behind it has no invariant a
/// panic could leave half-built, and `commit` holds it across a `remove_dir_all`,
/// so refusing it would turn one panic into a publish path that panics for the
/// rest of the session.
static PUBLISHED: Mutex<Option<Generation>> = Mutex::new(None);

/// The number of publishes so far in this process, which makes a generation id
/// unique even for two publishes inside one clock tick.
static GENERATION_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A directory name unique to this publish.
///
/// The wall-clock millisecond orders generations across a restart the way the
/// slot scheme's modification times did; the process id and a counter make it
/// unique when two publishes share a millisecond or two processes publish at
/// once, so no path is ever reused.
fn generation_name() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let counter = GENERATION_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("gen-{millis}-{}-{counter}", std::process::id())
}

/// Whether a directory entry is a generation directory.
fn is_generation_dir(path: &Path) -> bool {
    path.is_dir()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("gen-"))
}

/// The publish counter a generation directory carries, for ordering two that a
/// coarse-grained filesystem clock stamped with the same modification time.
///
/// The trailing field of `gen-<millis>-<pid>-<counter>`. Zero where it cannot be
/// read, which only matters as a tie-break under an equal modification time and
/// never decides the answer between two publishes a clock tick apart.
fn generation_ordinal(dir: &Path) -> u64 {
    dir.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.rsplit('-').next())
        .and_then(|digits| digits.parse().ok())
        .unwrap_or(0)
}

/// A generation's recency: its newest file's modification time, then its publish
/// counter. `None` where it holds no file to date.
fn generation_recency(dir: &Path) -> Option<(std::time::SystemTime, u64)> {
    generation_modified(dir).map(|time| (time, generation_ordinal(dir)))
}

/// Every generation directory under the wallpaper directory.
fn generation_dirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_generation_dir(path))
        .collect()
}

/// The PNG files of one generation, in name order.
fn generation_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
        })
        .collect();
    files.sort();
    files
}

/// When the most recently written file of a generation was written, where there
/// is one to ask.
fn generation_modified(dir: &Path) -> Option<std::time::SystemTime> {
    generation_files(dir)
        .iter()
        .filter_map(|path| modified(path))
        .max()
}

/// The most recently written generation directory, where there is one.
///
/// By the modification time of its newest file, which is what carries the
/// alternation across a restart: a process that did not do the publishing reads
/// the newest generation and its next publish is a newer one still.
fn newest_generation(dir: &Path) -> Option<PathBuf> {
    generation_dirs(dir)
        .into_iter()
        .filter_map(|dir| generation_recency(&dir).map(|key| (key, dir)))
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, dir)| dir)
}

/// The files the most recent publish wrote, empty where nothing has published.
///
/// What this process wrote, where it has written anything, and otherwise the
/// newest generation on disk, which is how a process that did not do the
/// publishing gets the same answer. Where the setter ran and took them, that is
/// also what the desktop's own store holds, which is what lets a test read the
/// setting back and recognize it; a publish whose setter failed is named here
/// too, because the generation is committed before the setter runs.
pub fn published_wallpaper_files() -> Result<Vec<PathBuf>, String> {
    if let Some(generation) = PUBLISHED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
    {
        return Ok(generation.files);
    }
    let dir = wallpaper_dir()?;
    Ok(newest_generation(&dir)
        .map(|dir| generation_files(&dir))
        .unwrap_or_default())
}

/// When a file was last written, or `None` where there is no file to ask.
fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

/// One publish in progress: its own directory, and what it has written so far.
///
/// Nothing existing is touched while a publish is in flight. The directory is new
/// and empty, each image is written into it atomically, and only [`commit`]
/// sweeps what earlier publishes left, so a failed encode leaves the desktop
/// exactly as it was.
///
/// [`commit`]: Publication::commit
pub(crate) struct Publication {
    dir: PathBuf,
    files: Vec<PathBuf>,
}

/// Where each of a job's images went, index for index with `job.monitors`.
#[cfg(any(windows, target_os = "linux"))]
pub(crate) struct WrittenImages {
    /// The file that screen's picture went to, or `None` for a screen this mode
    /// does not paint and which keeps the wallpaper it has.
    pub(crate) paths: Vec<Option<PathBuf>>,
    /// The anchor screen's own file, absent where the anchor has no picture.
    pub(crate) anchor: Option<PathBuf>,
}

/// Start a fresh generation to publish into.
///
/// The directory is created empty and no earlier generation is touched. Sweeping
/// waits for [`Publication::commit`], so a publish that fails part way through
/// removes nothing the desktop is still showing.
pub(crate) fn begin_publication() -> Result<Publication, String> {
    let root = wallpaper_dir()?;
    let dir = root.join(generation_name());
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create the wallpaper generation directory: {e}"))?;
    Ok(Publication {
        dir,
        files: Vec::new(),
    })
}

impl Publication {
    /// Encode one RGBA8 image into this generation and answer with its path.
    ///
    /// Written atomically: the PNG is encoded into a temporary file in the same
    /// directory and then renamed onto its final name, so a reader watching the
    /// destination sees the whole file or no file, never a truncated one. The
    /// temporary name carries the process id and a counter, the discipline
    /// [`crate::assets::texture_cache`] uses for the same reason, and it is
    /// removed if the encode fails. The destination is a name no publish has used
    /// before, so the rename creates it rather than replacing a file some shell
    /// may hold open.
    ///
    /// Fast compression (`CompressionType::Fast`), because the user waits for
    /// the "Set as Wallpaper" operation to complete and a larger file is the
    /// cheaper half of that trade. Windows preserves PNG wallpapers losslessly
    /// (no JPEG transcode).
    pub(crate) fn write(
        &mut self,
        suffix: &str,
        pixels: &[u8],
        width: u32,
        height: u32,
    ) -> Result<PathBuf, String> {
        let path = self.dir.join(format!("{suffix}.png"));
        debug!(path = %path.display(), width, height, "saving wallpaper PNG");

        let temp = unfinished(&path);
        if let Err(e) = encode_png(&temp, pixels, width, height) {
            let _ = std::fs::remove_file(&temp);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&temp, &path) {
            let _ = std::fs::remove_file(&temp);
            return Err(format!("Failed to put the wallpaper PNG in place: {e}"));
        }

        self.files.push(path.clone());
        Ok(path)
    }

    /// Write every image one job carries and answer with where each went.
    ///
    /// Two screens showing the same picture cost one render, and this is what
    /// carries that as far as the file: one encode and one path, named after
    /// whichever screen came first. A screen the mode does not paint is `None`
    /// rather than a gap, so the answer stays index-for-index with
    /// `job.monitors` and a caller can tell which screen each path belongs to.
    #[cfg(any(windows, target_os = "linux"))]
    pub(crate) fn write_job(
        &mut self,
        job: &crate::engine::wallpaper_sink::WallpaperJob,
    ) -> Result<WrittenImages, String> {
        use std::sync::Arc;

        use crate::engine::wallpaper_sink::Frame;

        let mut written: Vec<(Arc<Frame>, PathBuf)> = Vec::new();
        let mut paths: Vec<Option<PathBuf>> = Vec::with_capacity(job.monitors.len());
        let mut anchor = None;
        for index in 0..job.monitors.len() {
            let Some(frame) = job.image_for(index)? else {
                paths.push(None);
                continue;
            };
            let seen = written
                .iter()
                .find(|(seen, _)| Arc::ptr_eq(seen, &frame))
                .map(|(_, path)| path.clone());
            let path = if let Some(path) = seen {
                path
            } else {
                let path =
                    self.write(&index.to_string(), &frame.pixels, frame.width, frame.height)?;
                written.push((Arc::clone(&frame), path.clone()));
                path
            };
            if index == job.anchor {
                anchor = Some(path.clone());
            }
            paths.push(Some(path));
        }
        Ok(WrittenImages { paths, anchor })
    }

    /// Record this publication as the one the desktop is being handed, and sweep
    /// the generations no screen holds any more.
    ///
    /// Called once the images are written and before the setter runs, which is
    /// the same moment the single-file publish recorded its name: a failed
    /// encode must not spend a generation the next publish is going to need.
    ///
    /// The sweep runs here rather than after the setter because keeping the
    /// previous generation makes it safe either way: the desktop is showing that
    /// previous generation until the setter points it at this one, and everything
    /// this removes is older than it and referenced by nothing the desktop
    /// currently holds. A setter that then fails leaves the desktop on the
    /// previous generation, which is still on disk.
    ///
    /// The legacy flat files an older version left are the exception, and are
    /// swept only once a prior generation exists. A desktop upgraded from that
    /// version is still showing one of them, so deleting it here, before this
    /// first generation publish's setter has switched the desktop onto a
    /// generation, would delete the file under a live watch: the very thing this
    /// lifecycle exists to prevent. They are the previous "generation" for one
    /// cycle, kept through the first publish and swept on the second, by which
    /// point a generation has been set.
    pub(crate) fn commit(self) -> Vec<PathBuf> {
        let mut published = PUBLISHED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(root) = self.dir.parent() {
            let previous = published
                .as_ref()
                .map(|generation| generation.dir.clone())
                .or_else(|| newest_generation_other_than(root, &self.dir));
            sweep_generations(root, &self.dir, previous.as_deref());
            if previous.is_some() {
                sweep_legacy_files(root);
            }
        }
        *published = Some(Generation {
            dir: self.dir.clone(),
            files: self.files.clone(),
        });
        self.files
    }
}

/// The newest generation on disk that is not `current`.
///
/// The one a previous process left, which the desktop is still showing, so the
/// first publish of a fresh process keeps it rather than sweeping the layout
/// under the live wallpaper.
fn newest_generation_other_than(root: &Path, current: &Path) -> Option<PathBuf> {
    generation_dirs(root)
        .into_iter()
        .filter(|dir| dir != current)
        .filter_map(|dir| generation_recency(&dir).map(|key| (key, dir)))
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, dir)| dir)
}

/// Remove every generation except the one just published and the one before it.
///
/// `keep` names the two survivors. A directory that is neither is one no screen
/// holds any more, so its whole layout of full-resolution PNGs is removed at
/// once.
fn sweep_generations(root: &Path, current: &Path, previous: Option<&Path>) {
    for generation in generation_dirs(root) {
        if generation == current || Some(generation.as_path()) == previous {
            continue;
        }
        match std::fs::remove_dir_all(&generation) {
            Ok(()) => debug!(path = %generation.display(), "swept an old wallpaper generation"),
            Err(e) => {
                debug!(path = %generation.display(), error = %e, "could not sweep an old wallpaper generation");
            }
        }
    }
}

/// Remove the flat wallpaper files earlier versions wrote straight into the
/// wallpaper directory.
///
/// The two-slot scheme's `wallpaper-<slot>-*.png` and the single-image era's
/// `wallpaper-1.png` and `wallpaper-2.png`, each a full-resolution PNG nothing
/// will ever name again now that a publish writes into a generation directory.
///
/// Called only once a generation has been set, since one of these may be the
/// wallpaper the desktop is still showing until then; see [`Publication::commit`].
fn sweep_legacy_files(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        let named_like_a_slot = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("wallpaper-"));
        let is_png = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
        if path.is_file() && named_like_a_slot && is_png {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// A name for the not-yet-finished version of `path`, unique to this writer.
fn unfinished(path: &Path) -> PathBuf {
    crate::files::unfinished(path, ".tmp")
}

/// Encode one RGBA8 image as a PNG at `path`.
///
/// Fast compression and the cheapest filter: see [`Publication::write`] for
/// what the user is waiting on while this runs.
fn encode_png(path: &Path, pixels: &[u8], width: u32, height: u32) -> Result<(), String> {
    crate::files::write_png(
        path,
        pixels,
        width,
        height,
        crate::files::CompressionType::Fast,
        crate::files::FilterType::Sub,
    )
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::{Mutex, MutexGuard};

    use super::*;

    thread_local! {
        /// The directory the current test thread publishes into. Set by
        /// [`Scratch`] and read by [`super::scratch_override`].
        pub(super) static SCRATCH_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }

    /// Serializes every test that publishes, because the publish record in
    /// [`PUBLISHED`] is process-global and two publishes at once would each see
    /// the other's generation.
    static SERIAL: Mutex<()> = Mutex::new(());

    /// A disposable wallpaper directory that lasts one test.
    ///
    /// Holding it redirects [`wallpaper_dir`] at a fresh temporary directory and
    /// clears the process-global publish record, so a test publishes in isolation
    /// and never touches the developer's real wallpaper. Dropping it clears the
    /// redirect and the record and removes the directory. The lock it holds is
    /// what serializes the publishing tests.
    struct Scratch {
        _serial: MutexGuard<'static, ()>,
        dir: crate::test_support::ScratchDir,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let serial = SERIAL
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let dir = crate::test_support::ScratchDir::new(&format!("wallpaper_{name}"));
            SCRATCH_DIR.with(|slot| *slot.borrow_mut() = Some(dir.path().to_path_buf()));
            reset_published();
            Self {
                _serial: serial,
                dir,
            }
        }

        fn dir(&self) -> &Path {
            self.dir.path()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            SCRATCH_DIR.with(|slot| *slot.borrow_mut() = None);
            reset_published();
        }
    }

    /// Forget any in-process publish, so a test starts where a fresh process would.
    fn reset_published() {
        *PUBLISHED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    #[test]
    fn the_wallpaper_lives_under_this_systems_local_data_directory() {
        let dir = data_dir_wallpaper_path().expect("a local data directory on the test host");
        assert!(
            dir.ends_with("SunlitEarth"),
            "path should end with SunlitEarth, got: {dir:?}"
        );
    }

    /// The isolation itself: while a test holds a [`Scratch`], a publish resolves
    /// to that scratch directory and never to the live data directory. This is
    /// the guard that keeps a forgetful future test off the developer's desktop.
    #[test]
    fn a_publish_under_test_stays_out_of_the_real_data_directory() {
        let scratch = Scratch::new("isolation_guard");
        let resolved = wallpaper_dir().expect("the scratch directory resolves");
        assert!(
            resolved.starts_with(scratch.dir()),
            "a publish under test resolved to {resolved:?}, outside the scratch {:?}",
            scratch.dir()
        );
        let live = data_dir_wallpaper_path().expect("a local data directory on the test host");
        assert_ne!(
            resolved, live,
            "a publish under test resolved to the live wallpaper directory"
        );

        let mut publication = begin_publication().expect("a generation to publish into");
        let path = publication
            .write("0", &[255u8, 0, 0, 255], 1, 1)
            .expect("write a pixel");
        assert!(
            path.starts_with(scratch.dir()),
            "a written wallpaper {path:?} escaped the scratch {:?}",
            scratch.dir()
        );
    }

    /// The panic-guard itself, pinned: resolving the wallpaper directory with no
    /// scratch set panics rather than reaching the live data directory. A future
    /// change that let it fall back to `data_dir_wallpaper_path` under test would
    /// re-expose the developer's desktop, and this is what fails first if it does.
    #[test]
    #[should_panic(expected = "scratch override")]
    fn resolving_the_wallpaper_directory_without_a_scratch_refuses() {
        // No `Scratch` on this thread, so the thread-local override is unset.
        SCRATCH_DIR.with(|slot| {
            assert!(
                slot.borrow().is_none(),
                "a scratch leaked onto this thread, so the guard was not exercised"
            );
        });
        let _ = wallpaper_dir();
    }

    /// A tiny RGBA image whose one pixel carries `channels`, so a decode reads
    /// back something recognizable.
    fn pixels(width: u32, height: u32, channels: [u8; 4]) -> Vec<u8> {
        (0..width * height).flat_map(|_| channels).collect()
    }

    #[test]
    fn each_publish_gets_its_own_generation_directory() {
        let scratch = Scratch::new("generation_names");
        let first = begin_publication().expect("a generation");
        let second = begin_publication().expect("another generation");
        assert_ne!(first.dir, second.dir, "two publishes shared a directory");
        for dir in [&first.dir, &second.dir] {
            assert!(
                dir.starts_with(scratch.dir()),
                "{dir:?} escaped the scratch"
            );
            assert!(
                is_generation_dir(dir),
                "{dir:?} is not a generation directory"
            );
        }
    }

    /// The Arc dedupe is the whole reason two screens of one size cost one
    /// render, and it now lives in one place for both platforms, so this is
    /// where it is pinned: a shared frame is one file, a screen with no picture
    /// keeps its position, and the anchor is named.
    #[test]
    #[cfg(any(windows, target_os = "linux"))]
    fn screens_showing_the_same_picture_are_written_once_and_keep_their_places() {
        use std::sync::Arc;

        use crate::display::Monitor;
        use crate::display::layout::DisplayMode;
        use crate::engine::wallpaper_sink::{Frame, JobImages, WallpaperJob};

        let _scratch = Scratch::new("write_job");
        let screen = |id: &str, x: i32| Monitor {
            id: id.to_owned(),
            label: id.to_owned(),
            x,
            y: 0,
            width: 2,
            height: 2,
            primary: x == 0,
        };
        let shared = Arc::new(Frame::new(pixels(2, 2, [1, 2, 3, 255]), 2, 2));
        let job = WallpaperJob {
            mode: DisplayMode::EveryScreen,
            monitors: vec![screen("A", 0), screen("B", 2), screen("C", 4)],
            anchor: 1,
            images: JobImages::PerMonitor(vec![
                Some(Arc::clone(&shared)),
                Some(Arc::clone(&shared)),
                None,
            ]),
        };

        let mut publication = begin_publication().expect("a generation");
        let written = publication.write_job(&job).expect("write the images");

        assert_eq!(written.paths.len(), 3, "one answer per screen");
        assert_eq!(
            written.paths[0], written.paths[1],
            "one encode for the frame both screens share"
        );
        assert_eq!(
            written.paths[2], None,
            "the unpainted screen keeps its place"
        );
        assert_eq!(
            written.anchor, written.paths[1],
            "the anchor's own file, not the first written"
        );
        assert_eq!(
            publication.commit().len(),
            1,
            "one file on disk, not one per screen"
        );
    }

    #[test]
    fn a_written_wallpaper_is_whole_and_leaves_no_temporary() {
        let _scratch = Scratch::new("atomic_write");
        let mut publication = begin_publication().expect("a generation");
        let path = publication
            .write("0", &pixels(4, 4, [255, 0, 0, 255]), 4, 4)
            .expect("write the image");

        let decoded = image::open(&path).expect("the destination decodes as a whole PNG");
        assert_eq!((decoded.width(), decoded.height()), (4, 4));

        let leftovers: Vec<_> = generation_temporaries(&publication.dir);
        assert!(
            leftovers.is_empty(),
            "a finished write left a temporary behind: {leftovers:?}"
        );
    }

    #[test]
    fn a_write_that_cannot_be_placed_leaves_no_temporary_or_partial_file() {
        let _scratch = Scratch::new("failed_write");
        let mut publication = begin_publication().expect("a generation");
        // A directory where the finished PNG would go: the encode into the
        // temporary succeeds, and the rename onto this fails.
        let destination = publication.dir.join("0.png");
        std::fs::create_dir(&destination).expect("stand a directory in the way");

        let error = publication
            .write("0", &pixels(4, 4, [255, 0, 0, 255]), 4, 4)
            .expect_err("the rename onto a directory fails");
        assert!(!error.is_empty());

        assert!(
            destination.is_dir(),
            "the destination was clobbered instead of left as it was"
        );
        let leftovers = generation_temporaries(&publication.dir);
        assert!(
            leftovers.is_empty(),
            "a failed write left a temporary behind: {leftovers:?}"
        );
    }

    #[test]
    fn no_path_is_ever_reused_across_publishes() {
        let _scratch = Scratch::new("no_reuse");
        let publish = |suffixes: &[&str]| -> Vec<PathBuf> {
            let mut publication = begin_publication().expect("a generation");
            for suffix in suffixes {
                publication
                    .write(suffix, &pixels(2, 2, [0, 0, 255, 255]), 2, 2)
                    .expect("write an image");
            }
            publication.commit()
        };

        let first = publish(&["0", "1", "canvas"]);
        let second = publish(&["0"]);
        let third = publish(&["0", "1"]);

        for (earlier, later) in [(&first, &second), (&second, &third), (&first, &third)] {
            for path in later {
                assert!(
                    !earlier.contains(path),
                    "{} was handed out by an earlier publish too",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn the_sweep_keeps_the_last_two_generations_and_clears_the_legacy_files() {
        let scratch = Scratch::new("sweep");
        let root = scratch.dir().to_path_buf();

        // A slot-era layout and a single-image name, which nothing names any more.
        for legacy in [
            "wallpaper-1-0.png",
            "wallpaper-2-canvas.png",
            "wallpaper-1.png",
        ] {
            std::fs::write(root.join(legacy), b"not really a png").unwrap();
        }

        let publish = || -> PathBuf {
            let mut publication = begin_publication().expect("a generation");
            publication
                .write("0", &pixels(2, 2, [0, 255, 0, 255]), 2, 2)
                .expect("write an image");
            let dir = publication.dir.clone();
            publication.commit();
            dir
        };

        let first = publish();
        let second = publish();
        let third = publish();

        assert!(!first.exists(), "the oldest generation was not swept");
        assert!(second.exists(), "the previous generation must be kept");
        assert!(third.exists(), "the newest generation must be kept");

        for legacy in [
            "wallpaper-1-0.png",
            "wallpaper-2-canvas.png",
            "wallpaper-1.png",
        ] {
            assert!(
                !root.join(legacy).exists(),
                "{legacy} survived a publish, and each is as large as a screen"
            );
        }
    }

    #[test]
    fn the_first_publish_keeps_the_legacy_files_the_desktop_may_still_show() {
        let scratch = Scratch::new("legacy_first_publish");
        let root = scratch.dir().to_path_buf();
        let legacy = ["wallpaper-1-0.png", "wallpaper-2-0.png"];
        for name in legacy {
            std::fs::write(root.join(name), b"not really a png").unwrap();
        }

        let mut first = begin_publication().expect("a generation");
        first
            .write("0", &pixels(2, 2, [0, 255, 0, 255]), 2, 2)
            .unwrap();
        first.commit();
        for name in legacy {
            assert!(
                root.join(name).exists(),
                "{name} was swept while the desktop may still be showing it"
            );
        }

        let mut second = begin_publication().expect("a generation");
        second
            .write("0", &pixels(2, 2, [0, 0, 255, 255]), 2, 2)
            .unwrap();
        second.commit();
        for name in legacy {
            assert!(
                !root.join(name).exists(),
                "{name} survived the publish after a generation was set"
            );
        }
    }

    #[test]
    fn a_screen_left_alone_keeps_the_generation_it_still_references() {
        let _scratch = Scratch::new("one_screen_kept");

        // An every-screen layout the two screens both take a file from.
        let mut every = begin_publication().expect("a generation");
        let screen_two = every
            .write("1", &pixels(2, 2, [0, 0, 255, 255]), 2, 2)
            .expect("the second screen's file");
        every
            .write("0", &pixels(2, 2, [255, 0, 0, 255]), 2, 2)
            .unwrap();
        every.commit();

        // A one-screen publish that paints only the anchor.
        let mut one = begin_publication().expect("a generation");
        one.write("0", &pixels(2, 2, [255, 0, 0, 255]), 2, 2)
            .unwrap();
        one.commit();

        assert!(
            screen_two.exists(),
            "the file the second screen still shows was swept from under its watch"
        );
    }

    #[test]
    fn the_read_back_names_the_newest_generation_across_a_restart() {
        let _scratch = Scratch::new("read_back");

        let mut first = begin_publication().expect("a generation");
        first
            .write("0", &pixels(2, 2, [255, 0, 0, 255]), 2, 2)
            .unwrap();
        first.commit();

        let mut second = begin_publication().expect("a generation");
        let newest = second
            .write("0", &pixels(2, 2, [0, 0, 255, 255]), 2, 2)
            .expect("the newest file");
        let newest_dir = second.dir.clone();
        second.commit();

        // The in-process record still names what was just written.
        assert_eq!(published_wallpaper_files().unwrap(), vec![newest.clone()]);

        // As a fresh process would, with the record gone.
        reset_published();
        let read_back = published_wallpaper_files().unwrap();
        assert_eq!(
            read_back,
            vec![newest],
            "the read-back is not the newest generation"
        );
        assert!(read_back.iter().all(|path| path.starts_with(&newest_dir)));

        // And the next publish is a directory the read-back did not name.
        let next = begin_publication().expect("a generation");
        assert_ne!(
            next.dir, newest_dir,
            "a fresh publish reused the newest path"
        );
    }

    /// The temporary files left in a generation directory, `<name>.tmp`-suffixed.
    fn generation_temporaries(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
            })
            .collect()
    }
}
