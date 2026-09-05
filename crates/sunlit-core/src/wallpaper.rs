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
#[cfg(windows)]
use tracing::{info, warn};
#[cfg(windows)]
use windows_sys::Win32::Foundation::GetLastError;
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SPI_SETDESKWALLPAPER, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SystemParametersInfoW,
};

/// Return the wallpaper output directory, creating it if it does not exist.
///
/// `%LOCALAPPDATA%\SunlitEarth` on Windows and `~/.local/share/SunlitEarth` on
/// Linux, which is what `dirs::data_local_dir` answers on each and the same
/// directory the config file and the texture cache already live in. Asked through
/// `dirs` rather than through `LOCALAPPDATA` directly so that the three agree
/// wherever the app runs.
///
/// In a unit-test build this refuses to resolve to that live directory: a publish
/// under test writes real PNGs and sweeps what is there, so a test that reached
/// the real directory would overwrite the developer's own wallpaper. The tests
/// point it at a scratch directory through [`tests::Scratch`], and a resolution
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
    Ok(dirs::data_local_dir()
        .ok_or_else(|| "no local data directory on this system".to_owned())?
        .join("SunlitEarth"))
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
///
/// The process id and a counter keep two writers from sharing a temporary name,
/// so neither truncates the other's file mid-encode, following
/// [`crate::assets::texture_cache`].
fn unfinished(path: &Path) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nonce = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{}.{nonce}.tmp", std::process::id()));
    PathBuf::from(name)
}

/// Encode one RGBA8 image as a PNG at `path`.
fn encode_png(path: &Path, pixels: &[u8], width: u32, height: u32) -> Result<(), String> {
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};

    let file =
        std::fs::File::create(path).map_err(|e| format!("Failed to create PNG file: {e}"))?;
    let writer = std::io::BufWriter::new(file);
    let encoder = PngEncoder::new_with_quality(writer, CompressionType::Fast, FilterType::Sub);
    encoder
        .write_image(pixels, width, height, image::ColorType::Rgba8.into())
        .map_err(|e| format!("Failed to encode PNG: {e}"))
}

/// Declare this process per-monitor DPI aware, once, before anything asks Win32
/// where a monitor is.
///
/// `rcMonitor` is in virtual-screen coordinates, and those are physical pixels
/// only for a per-monitor aware process; a DPI-unaware one is handed the
/// virtualized rectangle instead, so under mixed scaling every monitor's size
/// and position would be wrong. winit does set
/// `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2`, but only from `EventLoop::new`,
/// which is reached when the settings window is built and never at all in a
/// headless run: `sunlit-earth displays` and `render` create no window, and
/// `displays` is precisely the command that exists to report these rectangles.
/// So the declaration is made here rather than left to whoever creates a window
/// first.
///
/// Idempotent by construction. A process whose awareness is already set refuses
/// the call with `ERROR_ACCESS_DENIED`, which is the case where winit got here
/// first and is the outcome this wants either way, so the return value is not an
/// error to report.
#[cfg(windows)]
pub(crate) fn ensure_dpi_awareness() {
    use std::sync::Once;

    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };

    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: SetProcessDpiAwarenessContext takes one of the predefined
        // pseudo-handles by value and touches nothing of ours. It is safe to
        // call on any thread and at any time; it merely fails where the
        // awareness is already set.
        #[allow(unsafe_code)]
        let set =
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        debug!(
            already_aware = set == 0,
            "declared this process per-monitor DPI aware"
        );
    });
}

/// Every monitor Windows has, with the rectangle each occupies.
///
/// `EnumDisplayMonitors` + `GetMonitorInfoW`, keeping every monitor rather than
/// only the one flagged primary, because the position of each is what a
/// multi-monitor plan is built out of. `rcMonitor` is in virtual-screen
/// coordinates, which are physical pixels for a per-monitor DPI aware process;
/// see `docs/platforms.md` for what makes this one aware.
///
/// The `id` is `szDevice` (`\\.\DISPLAY1`) until [`desktop_wallpaper`] can
/// improve on it: `IDesktopWallpaper` addresses a monitor by a device path that
/// survives a reboot, and `szDevice` does not. Off Windows the same question is
/// answered by [`crate::display`], which parses `xrandr --query`: there is no
/// API in this crate to ask, so it asks a program.
#[cfg(windows)]
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub(crate) fn enumerate_monitors() -> Result<Vec<crate::display::Monitor>, String> {
    use std::ptr;

    use windows_sys::Win32::Foundation::{BOOL, LPARAM, RECT, TRUE};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

    #[allow(unsafe_code)]
    unsafe extern "system" fn enum_callback(
        hmonitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        // SAFETY: lparam was set to a valid &mut Vec<HMONITOR> pointer
        // by the caller below, and this callback runs synchronously on
        // the same thread within EnumDisplayMonitors.
        #[allow(unsafe_code)]
        let monitors = unsafe { &mut *(lparam as *mut Vec<HMONITOR>) };
        monitors.push(hmonitor);
        TRUE
    }

    ensure_dpi_awareness();
    let mut handles: Vec<HMONITOR> = Vec::new();

    // SAFETY: EnumDisplayMonitors with null HDC/RECT enumerates all monitors.
    // The callback receives a valid lparam pointing to our Vec. The call is
    // synchronous -- the callback runs on this thread before the function returns.
    #[allow(unsafe_code)]
    let success = unsafe {
        EnumDisplayMonitors(
            ptr::null_mut(),
            ptr::null(),
            Some(enum_callback),
            (&raw mut handles) as LPARAM,
        )
    };
    if success == 0 {
        return Err("EnumDisplayMonitors failed".to_owned());
    }

    let mut monitors = Vec::with_capacity(handles.len());
    for &hmon in &handles {
        let mut info: MONITORINFOEXW = {
            // SAFETY: MONITORINFOEXW is a plain-old-data C struct.
            // Zeroing it is safe; we set cbSize immediately after.
            #[allow(unsafe_code)]
            let zeroed: MONITORINFOEXW = unsafe { std::mem::zeroed() };
            zeroed
        };
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;

        // SAFETY: hmon is a valid HMONITOR from EnumDisplayMonitors.
        // info is a properly sized MONITORINFOEXW with cbSize set.
        // GetMonitorInfoW writes into info and returns TRUE on success.
        #[allow(unsafe_code)]
        let ok = unsafe { GetMonitorInfoW(hmon, (&raw mut info).cast::<MONITORINFO>()) };
        if ok == 0 {
            continue;
        }

        let rc = info.monitorInfo.rcMonitor;
        let device = wide_to_string(&info.szDevice);
        monitors.push(crate::display::Monitor {
            label: display_label(&device, monitors.len()),
            id: device,
            x: rc.left,
            y: rc.top,
            width: (rc.right - rc.left) as u32,
            height: (rc.bottom - rc.top) as u32,
            primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        });
    }
    adopt_device_paths(&mut monitors);
    debug!(count = monitors.len(), "enumerated the monitors");
    Ok(monitors)
}

/// Replace each monitor's id with the device path the shell addresses it by.
///
/// The two enumerations have no key in common, so the rectangle is the join:
/// `GetMonitorRECT` and `rcMonitor` describe the same screen in the same
/// coordinates. Mirrored monitors are two entries with one rectangle, and each
/// path takes the first monitor not already claimed, which keeps the mapping a
/// bijection where a rectangle alone would be ambiguous.
///
/// A failure anywhere leaves `szDevice` in place. That is a name the shell will
/// not take, so a publish that needs to address a monitor says so rather than
/// setting the wrong screen's wallpaper.
#[cfg(windows)]
fn adopt_device_paths(monitors: &mut [crate::display::Monitor]) {
    let api = match shell::DesktopWallpaperApi::open() {
        Ok(api) => api,
        Err(e) => {
            debug!(error = %e, "no shell wallpaper interface; keeping the display device names");
            return;
        }
    };
    let paths = match api.monitor_paths() {
        Ok(paths) => paths,
        Err(e) => {
            debug!(error = %e, "the shell listed no monitor device paths");
            return;
        }
    };
    let mut claimed = vec![false; monitors.len()];
    for path in paths {
        let Ok((left, top, right, bottom)) = api.monitor_rect(&path) else {
            continue;
        };
        let matched = monitors.iter().enumerate().position(|(index, monitor)| {
            !claimed[index]
                && monitor.x == left
                && monitor.y == top
                && i64::from(monitor.width) == i64::from(right) - i64::from(left)
                && i64::from(monitor.height) == i64::from(bottom) - i64::from(top)
        });
        if let Some(index) = matched {
            claimed[index] = true;
            monitors[index].id = path;
        }
    }
}

/// A null-terminated fixed-width UTF-16 field as a `String`.
#[cfg(windows)]
fn wide_to_string(field: &[u16]) -> String {
    let end = field.iter().position(|&c| c == 0).unwrap_or(field.len());
    String::from_utf16_lossy(&field[..end])
}

/// What to call a Windows monitor in the settings window.
///
/// `\\.\DISPLAY2` is what Windows answers with and is not what its own display
/// settings show anybody, so the digit is lifted out of it and the position in
/// the enumeration stands in where there is no digit to lift.
#[cfg(windows)]
fn display_label(device: &str, index: usize) -> String {
    let number = device
        .rsplit('\\')
        .next()
        .and_then(|name| name.strip_prefix("DISPLAY"))
        .and_then(|digits| digits.parse::<u32>().ok());
    match number {
        Some(number) => format!("Display {number}"),
        None => format!("Display {}", index + 1),
    }
}

/// Detect the primary monitor's physical resolution in pixels.
///
/// One enumeration, not two: this is [`enumerate_monitors`] narrowed to the
/// monitor a single-screen wallpaper is sized for, with the same fallback to
/// the first that [`crate::display::primary_of`] makes for xrandr.
#[cfg(windows)]
pub fn get_primary_monitor_resolution() -> Result<(u32, u32), String> {
    let monitors = enumerate_monitors()?;
    let monitor = crate::display::primary_monitor_of(&monitors)
        .ok_or_else(|| "No primary monitor found".to_owned())?;
    debug!(
        width = monitor.width,
        height = monitor.height,
        "detected primary monitor resolution"
    );
    Ok((monitor.width, monitor.height))
}

/// `IDesktopWallpaper` behind a small safe wrapper.
///
/// The COM interface is the only way to address one monitor:
/// `SystemParametersInfoW` sets the wallpaper of a whole session and has no
/// parameter for which screen. It is Windows 8 and later, which is everything
/// this ships to.
///
/// Every call is `unsafe` in the generated bindings and every one of them is
/// wrapped here, so the rest of the module never writes `unsafe` and the
/// invariants are argued once each rather than at every call site.
#[cfg(windows)]
mod shell {
    use std::path::Path;

    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    };
    use windows::Win32::UI::Shell::{
        DESKTOP_WALLPAPER_POSITION, DWPOS_FILL, DWPOS_SPAN, DesktopWallpaper, IDesktopWallpaper,
    };
    use windows::core::{HSTRING, PCWSTR, PWSTR};

    /// A string the shell allocated with the COM task allocator.
    ///
    /// `GetMonitorDevicePathAt` hands back memory the caller owns, and the
    /// wrapper exists so that every early return frees it.
    struct TaskMem(PWSTR);

    impl TaskMem {
        /// Its contents as a `String`, empty where the shell answered with
        /// nothing. Not `Display`, because a wrapper around a raw pointer that
        /// prints itself invites being printed.
        fn read(&self) -> String {
            if self.0.is_null() {
                return String::new();
            }
            // SAFETY: a non-null PWSTR from a COM out-parameter is a
            // null-terminated UTF-16 string the shell allocated and this owns.
            #[allow(unsafe_code)]
            unsafe {
                String::from_utf16_lossy(self.0.as_wide())
            }
        }
    }

    impl Drop for TaskMem {
        fn drop(&mut self) {
            if self.0.is_null() {
                return;
            }
            // SAFETY: the pointer came from a COM method that transfers
            // ownership to the caller, and this is the only release of it.
            #[allow(unsafe_code)]
            unsafe {
                CoTaskMemFree(Some(self.0.as_ptr().cast()));
            }
        }
    }

    /// Initialize COM on this thread, once, and never tear it down.
    ///
    /// A single-threaded apartment, which is what a desktop shell object wants
    /// and what the UI thread is already in. A thread that is already in the
    /// multi-threaded apartment answers `RPC_E_CHANGED_MODE`, and that is not an
    /// error to report: COM is initialized there, the apartment is simply
    /// somebody else's, and an in-process shell object works either way. What
    /// must not happen is calling `CoUninitialize` on a thread this did not
    /// initialize, which is why nothing here ever does.
    fn initialize() {
        thread_local! {
            static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        }
        DONE.with(|done| {
            if done.replace(true) {
                return;
            }
            // SAFETY: CoInitializeEx takes no pointers of ours and is safe to
            // call on any thread. Its result is inspected rather than asserted,
            // because both "already initialized" answers are fine here.
            #[allow(unsafe_code)]
            let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            tracing::debug!(hresult = hr.0, "initialized COM on this thread");
        });
    }

    /// Where an image is fitted on the screens it is set on.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Position {
        /// One screen's own picture, filled to it. What `WallpaperStyle=10` does.
        Fill,
        /// One image stretched over the whole virtual desktop.
        Span,
    }

    impl From<Position> for DESKTOP_WALLPAPER_POSITION {
        fn from(position: Position) -> Self {
            match position {
                Position::Fill => DWPOS_FILL,
                Position::Span => DWPOS_SPAN,
            }
        }
    }

    /// The shell's own wallpaper object.
    pub struct DesktopWallpaperApi(IDesktopWallpaper);

    impl DesktopWallpaperApi {
        /// Create the shell object, initializing COM on this thread first.
        pub fn open() -> Result<Self, String> {
            initialize();
            // SAFETY: DesktopWallpaper is an in-process shell class and the
            // requested interface is the one the type parameter names, so the
            // generated wrapper checks the QueryInterface itself.
            #[allow(unsafe_code)]
            let api: IDesktopWallpaper =
                unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL) }
                    .map_err(|e| format!("cannot reach the desktop wallpaper interface: {e}"))?;
            Ok(Self(api))
        }

        /// Every monitor the shell will address, by the device path it takes.
        ///
        /// The path is what survives a reboot, which `\\.\DISPLAY1` does not, so
        /// it is what the anchor setting stores.
        pub fn monitor_paths(&self) -> Result<Vec<String>, String> {
            // SAFETY: no arguments, and the count is a plain out-parameter.
            #[allow(unsafe_code)]
            let count = unsafe { self.0.GetMonitorDevicePathCount() }
                .map_err(|e| format!("cannot count the monitors the shell addresses: {e}"))?;
            let mut paths = Vec::with_capacity(count as usize);
            for index in 0..count {
                // SAFETY: the index is below the count the shell just gave, and
                // the returned string is owned by TaskMem from here on.
                #[allow(unsafe_code)]
                let path = unsafe { self.0.GetMonitorDevicePathAt(index) }
                    .map_err(|e| format!("cannot read monitor {index}'s device path: {e}"))?;
                paths.push(TaskMem(path).read());
            }
            Ok(paths)
        }

        /// The rectangle the shell says one device path occupies.
        ///
        /// This is what maps a device path onto an `HMONITOR`: the two
        /// enumerations have no key in common and this rectangle is the only
        /// thing both of them report.
        pub fn monitor_rect(&self, id: &str) -> Result<(i32, i32, i32, i32), String> {
            let id = HSTRING::from(id);
            // SAFETY: the HSTRING outlives the call, and PCWSTR borrows it.
            #[allow(unsafe_code)]
            let rect = unsafe { self.0.GetMonitorRECT(PCWSTR(id.as_ptr())) }
                .map_err(|e| format!("cannot read that monitor's rectangle: {e}"))?;
            Ok((rect.left, rect.top, rect.right, rect.bottom))
        }

        /// How the images this object sets are fitted.
        pub fn set_position(&self, position: Position) -> Result<(), String> {
            // SAFETY: the position is one of the enumeration's own values.
            #[allow(unsafe_code)]
            unsafe { self.0.SetPosition(position.into()) }
                .map_err(|e| format!("cannot set the wallpaper position: {e}"))
        }

        /// Put one image on one monitor, or on every monitor where `monitor` is
        /// `None`.
        pub fn set(&self, monitor: Option<&str>, image: &Path) -> Result<(), String> {
            let id = monitor.map(HSTRING::from);
            let image = HSTRING::from(image.as_os_str());
            let id = id.as_ref().map_or(PCWSTR::null(), |id| PCWSTR(id.as_ptr()));
            // SAFETY: both HSTRINGs outlive the call, and a null monitor id is
            // the interface's own way of naming every monitor.
            #[allow(unsafe_code)]
            unsafe { self.0.SetWallpaper(id, PCWSTR(image.as_ptr())) }.map_err(|e| {
                format!(
                    "cannot set the wallpaper of {}: {e}",
                    monitor.unwrap_or("every monitor")
                )
            })
        }
    }
}

/// Make a finished job the Windows desktop's wallpaper.
///
/// A session with one monitor keeps `SystemParametersInfoW` and the registry
/// style write exactly as they were, so the one configuration that is
/// regression-tested on every desktop and in the Hyper-V guest does not move.
/// Everything beyond one screen goes through `IDesktopWallpaper`, which is the
/// only interface that can address a monitor.
///
/// Addressing one needs the device path [`adopt_device_paths`] maps onto it, and
/// that mapping is a rectangle comparison nothing guarantees: `GetMonitorRECT`
/// and `rcMonitor` disagreeing under mixed DPI would leave every screen holding
/// the `szDevice` name it came in with, which the shell does not take. Three
/// answers follow, and none of them is failing the publish. A screen with no
/// device path keeps the wallpaper it has and the note says which. A screen the
/// shell refuses costs that screen alone, because an unplug between the
/// enumeration and the call looks exactly like that from here and the screens
/// still there should not pay for it. And where *no* screen could be addressed,
/// this falls back to the single-monitor path with the anchor's picture: that is
/// what every session did before this feature, and losing the primary's
/// wallpaper on a configuration nobody has verified is the worst outcome
/// available.
#[cfg(windows)]
pub(crate) fn set_wallpaper_job(
    job: &crate::engine::wallpaper_sink::WallpaperJob,
) -> Result<String, String> {
    use std::sync::Arc;

    use crate::display::layout::DisplayMode;
    use crate::engine::wallpaper_sink::Frame;

    let spanning = job.mode == DisplayMode::AcrossScreens;
    let mut publication = begin_publication()?;

    if job.monitors.len() <= 1 {
        let frame = job.anchor_image()?;
        let path = publication.write("0", &frame.pixels, frame.width, frame.height)?;
        publication.commit();
        set_wallpaper(&path)?;
        return Ok(String::new());
    }

    if spanning {
        let canvas = job
            .canvas()
            .ok_or_else(|| "a view across the screens was asked for without a canvas".to_owned())?;
        let path = publication.write("canvas", &canvas.pixels, canvas.width, canvas.height)?;
        publication.commit();
        let api = shell::DesktopWallpaperApi::open()?;
        // Windows does the cutting: one file instead of one per screen, and the
        // path it keeps consistent by itself when a monitor is unplugged.
        api.set_position(shell::Position::Span)?;
        api.set(None, &path)?;
        info!(path = %path.display(), "spanned the wallpaper across every monitor");
        return Ok(String::new());
    }

    let mut written: Vec<(Arc<Frame>, PathBuf)> = Vec::new();
    let mut images: Vec<(&crate::display::Monitor, PathBuf)> = Vec::new();
    let mut anchor_path: Option<PathBuf> = None;
    for (index, monitor) in job.monitors.iter().enumerate() {
        // A screen with no picture is one this mode does not paint, and it is
        // left holding whatever it already had.
        let Some(frame) = job.image_for(index)? else {
            continue;
        };
        // Two screens showing the same picture cost one render, and this is
        // what carries that as far as the file: one encode and one path.
        let seen = written
            .iter()
            .find(|(seen, _)| Arc::ptr_eq(seen, &frame))
            .map(|(_, path)| path.clone());
        let path = if let Some(path) = seen {
            path
        } else {
            let path =
                publication.write(&index.to_string(), &frame.pixels, frame.width, frame.height)?;
            written.push((Arc::clone(&frame), path.clone()));
            path
        };
        if index == job.anchor {
            anchor_path = Some(path.clone());
        }
        images.push((monitor, path));
    }
    publication.commit();

    let api = shell::DesktopWallpaperApi::open()?;
    api.set_position(shell::Position::Fill)?;
    let mut unaddressed = Vec::new();
    let mut refused = Vec::new();
    let mut painted = 0usize;
    for (monitor, path) in &images {
        if !is_device_path(&monitor.id) {
            unaddressed.push(monitor.label.clone());
            continue;
        }
        match api.set(Some(&monitor.id), path) {
            Ok(()) => painted += 1,
            Err(e) => {
                warn!(
                    monitor = %monitor.label,
                    error = %e,
                    "the shell would not take this screen's wallpaper"
                );
                refused.push(monitor.label.clone());
            }
        }
    }

    if painted == 0 {
        let path =
            anchor_path.ok_or_else(|| "this publish has no image for its own anchor".to_owned())?;
        set_wallpaper(&path)?;
        let anchor = job
            .anchor_monitor()
            .map_or("the anchor", |m| m.label.as_str());
        return Ok(format!(
            "Windows would not take a wallpaper for any screen by name, so every \
             screen got {anchor}'s picture the way they all did before"
        ));
    }

    info!(screens = painted, "set a wallpaper per monitor");
    let mut notes = Vec::new();
    if !unaddressed.is_empty() {
        notes.push(format!(
            "Windows named no device for {}, so those screens kept the wallpaper they had",
            unaddressed.join(", ")
        ));
    }
    if !refused.is_empty() {
        notes.push(format!(
            "Windows would not take a wallpaper for {}, which is what a screen \
             unplugged since the layout was read looks like from here",
            refused.join(", ")
        ));
    }
    Ok(notes.join("; "))
}

/// Whether a monitor id is one `IDesktopWallpaper` will take.
///
/// Every id starts life as `szDevice` (`\\.\DISPLAY1`) and
/// [`adopt_device_paths`] replaces the ones the shell could be matched to with
/// the device path it addresses that screen by. So an id still in the
/// `\\.\` shape is one the mapping did not reach, and giving it to
/// `SetWallpaper` addresses nothing at all: the interface takes the path, and a
/// display device name is not one.
#[cfg(windows)]
fn is_device_path(id: &str) -> bool {
    !id.is_empty() && !id.starts_with(r"\\.\")
}

/// Set the wallpaper display style to "Fill" (style 10, tile 0) via the
/// registry keys `HKCU\Control Panel\Desktop\WallpaperStyle` and
/// `HKCU\Control Panel\Desktop\TileWallpaper`.
#[cfg(windows)]
fn ensure_fill_style() -> Result<(), String> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};

    debug!("setting Fill wallpaper style");

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags("Control Panel\\Desktop", KEY_SET_VALUE)
        .map_err(|e| format!("Failed to open Desktop registry key: {e}"))?;

    key.set_value("WallpaperStyle", &"10")
        .map_err(|e| format!("Failed to set WallpaperStyle: {e}"))?;
    key.set_value("TileWallpaper", &"0")
        .map_err(|e| format!("Failed to set TileWallpaper: {e}"))?;

    Ok(())
}

/// Set the given image file as the Windows desktop wallpaper.
///
/// Verifies the file exists and is non-empty, sets "Fill" display style,
/// then calls `SystemParametersInfoW` with `SPI_SETDESKWALLPAPER`.
#[cfg(windows)]
pub(crate) fn set_wallpaper(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    let metadata = std::fs::metadata(path).map_err(|e| format!("Wallpaper file not found: {e}"))?;
    if metadata.len() == 0 {
        return Err("Wallpaper file is empty".to_owned());
    }

    let abs_path =
        std::fs::canonicalize(path).map_err(|e| format!("Failed to canonicalize path: {e}"))?;

    ensure_fill_style()?;

    info!(path = %abs_path.display(), "applying wallpaper via SystemParametersInfoW");

    let mut wide_path: Vec<u16> = abs_path.as_os_str().encode_wide().collect();
    // Strip the \\?\ prefix that canonicalize adds on Windows
    let unc_prefix: [u16; 4] = [
        u16::from(b'\\'),
        u16::from(b'\\'),
        u16::from(b'?'),
        u16::from(b'\\'),
    ];
    if wide_path.starts_with(&unc_prefix) {
        wide_path = wide_path[4..].to_vec();
    }
    wide_path.push(0); // null terminator

    // SAFETY: wide_path is a valid null-terminated UTF-16 string pointing to
    // an existing, non-empty file. SystemParametersInfoW with SPI_SETDESKWALLPAPER
    // reads the string and sets the wallpaper. SPIF_UPDATEINIFILE persists the
    // setting across reboots; SPIF_SENDCHANGE notifies other windows.
    #[allow(unsafe_code)]
    let result = unsafe {
        SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            wide_path.as_mut_ptr().cast(),
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        )
    };

    if result == 0 {
        // SAFETY: GetLastError is a trivial Win32 function that reads the
        // calling thread's last-error code. No preconditions.
        #[allow(unsafe_code)]
        let err = unsafe { GetLastError() };
        return Err(format!("SystemParametersInfoW failed (error code {err})"));
    }

    Ok(())
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

    /// The Windows half of the platform seam, asserted against whatever this
    /// machine has: shape rather than values, because the values are the
    /// machine's.
    ///
    /// One enumeration, not three: on Windows an enumeration is
    /// `EnumDisplayMonitors`, a `GetMonitorInfoW` per monitor, and a COM object
    /// opened for the device paths.
    #[test]
    #[cfg(windows)]
    fn every_monitor_is_enumerated_with_a_rectangle_and_one_of_them_is_primary() {
        let monitors = enumerate_monitors().expect("Windows can enumerate its monitors");
        assert!(!monitors.is_empty(), "a desktop session has a monitor");
        for monitor in &monitors {
            assert!(
                monitor.width > 0 && monitor.height > 0,
                "a monitor with no pixels: {monitor:?}"
            );
            assert!(!monitor.id.is_empty(), "nothing to address: {monitor:?}");
            assert!(monitor.label.starts_with("Display "), "{monitor:?}");
        }
        assert_eq!(
            monitors.iter().filter(|m| m.primary).count(),
            1,
            "Windows marks exactly one monitor primary: {monitors:?}"
        );
        // And the narrowed query answers out of the same list rather than
        // enumerating a second time with its own rules.
        let primary = monitors.iter().find(|m| m.primary).unwrap();
        assert_eq!(
            get_primary_monitor_resolution().unwrap(),
            (primary.width, primary.height)
        );
        assert!(
            primary.width >= 640 && primary.height >= 480,
            "a desktop nobody could use: {primary:?}"
        );
    }

    #[test]
    #[cfg(windows)]
    fn a_display_device_is_labeled_by_its_own_number() {
        assert_eq!(display_label(r"\\.\DISPLAY2", 0), "Display 2");
        // A device path with no number to lift falls back to where it came in
        // the enumeration rather than to a name that addresses nothing.
        assert_eq!(display_label("", 3), "Display 4");
        assert_eq!(display_label(r"\\.\WEIRD", 0), "Display 1");
    }

    #[test]
    #[cfg(windows)]
    fn a_display_device_name_is_not_something_the_shell_can_be_given() {
        assert!(!is_device_path(r"\\.\DISPLAY1"));
        assert!(!is_device_path(""));
        assert!(is_device_path(
            r"\\?\DISPLAY#GSM5B09#5&d0e4e51&0&UID4353#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
        ));
    }

    #[test]
    #[cfg(windows)]
    fn a_fixed_width_device_field_stops_at_its_terminator() {
        let mut field = [0u16; 32];
        for (slot, c) in field.iter_mut().zip("ok".encode_utf16()) {
            *slot = c;
        }
        assert_eq!(wide_to_string(&field), "ok");
        assert_eq!(wide_to_string(&[]), "");
    }

    #[test]
    #[cfg(windows)]
    fn set_wallpaper_rejects_a_file_it_cannot_hand_over() {
        assert!(
            set_wallpaper(Path::new(r"C:\nonexistent\fake_wallpaper.png")).is_err(),
            "a file that is not there is not a wallpaper"
        );

        let scratch = crate::test_support::ScratchDir::new("wallpaper_empty_file");
        let empty_file = scratch.join("empty.png");
        std::fs::write(&empty_file, b"").unwrap();
        assert!(
            set_wallpaper(&empty_file).is_err(),
            "an empty file is not a wallpaper"
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
