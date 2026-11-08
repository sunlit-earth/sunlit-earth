//! File > Export Image…: the sheet's controller.
//!
//! The sheet (`overlays/export-sheet.slint`) is drawn inside the main window
//! and talks to this module through the `ExportState` global. Opening it holds
//! the engine's scene (`EngineCommand::Hold`), so what the preview shows is
//! what every export draws, and closing it lets go. An export asks the engine
//! for its pixels and encodes them on a worker thread of its own, one at a
//! time; the sheet is the worker's consumer and hears its progress through the
//! event loop.
//!
//! The controller lives in a thread local on the UI thread, so the tray, the
//! IPC handler and the window's hide paths can close the sheet without being
//! handed it.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use slint::ComponentHandle;
use tracing::{debug, info, warn};

use sunlit_core::config::{self, ExportFormat};
use sunlit_core::display::Monitor;
use sunlit_core::engine::EngineCommand;
use sunlit_core::image_file::{self, ImageFormat, WriteError};

use crate::displays::SharedMonitors;
use crate::engine_client::EngineLink;
use crate::{ExportState, MainWindow};

/// The PNG deflate levels the sheet offers. Level 4 is left out: on the
/// renders measured it was never smaller than level 3 and always slower
/// (`docs/plans/2026-11-08-export-image-research.md`, Measurements).
pub const PNG_LEVELS: [u8; 8] = [1, 2, 3, 5, 6, 7, 8, 9];

/// The common sizes the Size list offers after the monitors, and how each is
/// named.
const COMMON_SIZES: [(u32, u32, &str); 9] = [
    (1920, 1080, ""),
    (2560, 1440, ""),
    (3840, 2160, ""),
    (5120, 2880, ""),
    (7680, 4320, ""),
    (2560, 1600, ""),
    (3440, 1440, ""),
    (1290, 2796, ", phone"),
    (2048, 2048, ""),
];

/// The size an export starts from with no monitor and no saved size.
const FALLBACK_SIZE: (u32, u32) = (1920, 1080);

/// Where the progress bar stands when the request has gone out, when the
/// pixels have arrived, and when the encoder is done; the rename takes the
/// rest. Measured in Step 1 of the plan: the render and the readback are a
/// tenth or less of an export at the default level.
const REQUESTED: f32 = 0.02;
const RENDERED: f32 = 0.10;
const ENCODED: f32 = 0.98;

/// How often the worker tells the sheet how far it is, at most.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

/// The long edge of the thumbnail, and how long the aspect must hold before
/// it is drawn again.
const THUMBNAIL_EDGE: u32 = 200;
const THUMBNAIL_SETTLE: Duration = Duration::from_millis(300);

/// A save dialog that answers sooner than this did not open: nobody cancels
/// that fast.
const NO_PICKER: Duration = Duration::from_millis(300);

/// The file name an export starts with before the first one.
const DEFAULT_NAME: &str = "sunlit-earth";

/// One entry of the Size list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub label: String,
    /// `None` for Custom, the last entry.
    pub size: Option<(u32, u32)>,
}

/// The Size list for these monitors: each size of monitor once, naming every
/// monitor of that size, the anchor's first; then the common sizes no monitor
/// already has; then Custom.
///
/// :param monitors: the session's monitors
/// :param anchor: the stored anchor monitor's id, or `None` for the primary
/// :returns: the list, Custom last
#[must_use]
pub fn presets(monitors: &[Monitor], anchor: Option<&str>) -> Vec<Preset> {
    let mut groups: Vec<((u32, u32), Vec<&str>)> = Vec::new();
    let anchor =
        sunlit_core::display::layout::resolve_anchor(monitors, anchor).map(|anchor| anchor.index);
    let order = anchor
        .into_iter()
        .chain((0..monitors.len()).filter(|&index| Some(index) != anchor));
    for index in order {
        let monitor = &monitors[index];
        let size = (monitor.width, monitor.height);
        if size.0 == 0 || size.1 == 0 {
            continue;
        }
        match groups.iter_mut().find(|(seen, _)| *seen == size) {
            Some((_, names)) => names.push(&monitor.label),
            None => groups.push((size, vec![&monitor.label])),
        }
    }
    let mut list: Vec<Preset> = groups
        .iter()
        .map(|((width, height), names)| Preset {
            label: format!("{} · {width} × {height}", names.join(", ")),
            size: Some((*width, *height)),
        })
        .collect();
    for (width, height, note) in COMMON_SIZES {
        if groups.iter().all(|(size, _)| *size != (width, height)) {
            list.push(Preset {
                label: format!("{width} × {height}{note}"),
                size: Some((width, height)),
            });
        }
    }
    list.push(Preset {
        label: "Custom".to_owned(),
        size: None,
    });
    list
}

/// The entry of `presets` that is `size`, or Custom.
#[must_use]
pub fn preset_index(presets: &[Preset], size: (u32, u32)) -> usize {
    presets
        .iter()
        .position(|preset| preset.size == Some(size))
        .unwrap_or(presets.len().saturating_sub(1))
}

/// The format a recognized extension of `path` names: `png`, `jpg` or
/// `jpeg` in any case.
#[must_use]
pub fn format_of_path(path: &str) -> Option<ExportFormat> {
    let extension = Path::new(path).extension()?.to_str()?;
    if extension.eq_ignore_ascii_case("png") {
        Some(ExportFormat::Png)
    } else if extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg") {
        Some(ExportFormat::Jpeg)
    } else {
        None
    }
}

/// The extension a file of `format` is given.
#[must_use]
pub fn extension_of(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Png => "png",
        ExportFormat::Jpeg => "jpg",
    }
}

/// `path` with a recognized extension of another format replaced by
/// `format`'s, for the path field when the format is chosen. A path with no
/// recognized extension, or with `format`'s own in any spelling, is left as
/// it is.
#[must_use]
pub fn with_format(path: &str, format: ExportFormat) -> String {
    match format_of_path(path) {
        Some(found) if found != format => {
            let stem_len = path.len() - Path::new(path).extension().map_or(0, std::ffi::OsStr::len);
            format!("{}{}", &path[..stem_len], extension_of(format))
        }
        _ => path.to_owned(),
    }
}

/// The file an export of `format` to the path typed as `path` writes: the
/// path as it is when its extension is `format`'s, with the extension
/// replaced when it is the other format's, and with `format`'s appended
/// otherwise. A trailing dot is not an extension.
#[must_use]
pub fn export_target(path: &str, format: ExportFormat) -> PathBuf {
    let path = path.trim();
    match format_of_path(path) {
        Some(found) if found == format => PathBuf::from(path),
        Some(_) => PathBuf::from(with_format(path, format)),
        None => PathBuf::from(format!(
            "{}.{}",
            path.strip_suffix('.').unwrap_or(path),
            extension_of(format)
        )),
    }
}

/// What is wrong with exporting to `target`, if anything: a relative path or
/// a folder that does not exist.
pub fn check_target(target: &Path) -> Result<(), String> {
    if !target.is_absolute() {
        return Err("Type a full path, starting from a drive or the root folder.".to_owned());
    }
    match target.parent() {
        Some(folder) if folder.is_dir() => Ok(()),
        Some(folder) => Err(format!("The folder {} does not exist.", folder.display())),
        None => Err("Type a file name after the folder.".to_owned()),
    }
}

/// The folder an export starts in before the first: Pictures where it
/// exists, else the home folder, else the working directory.
#[must_use]
pub fn fallback_folder(pictures: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    pictures
        .filter(|folder| folder.is_dir())
        .or(home)
        .unwrap_or_default()
}

/// The path the field and the save dialog start from: the remembered one
/// where its folder still exists, else the remembered name in `fallback`, or
/// `sunlit-earth` there before the first export, with `format`'s extension.
///
/// :param stored: `export_path` from the config, without an extension
/// :param fallback: where to start when the remembered folder is gone
/// :param format: the format whose extension the name is given
#[must_use]
pub fn starting_path(stored: &str, fallback: &Path, format: ExportFormat) -> String {
    let stored = Path::new(stored.trim());
    let path = match (stored.parent(), stored.file_name()) {
        (Some(folder), Some(_)) if folder.is_dir() && stored.is_absolute() => stored.to_path_buf(),
        (_, Some(name)) => fallback.join(name),
        _ => fallback.join(DEFAULT_NAME),
    };
    format!("{}.{}", path.display(), extension_of(format))
}

/// `path` as the config remembers it: without a recognized extension.
#[must_use]
pub fn stored_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if format_of_path(&text).is_some() {
        path.with_extension("").to_string_lossy().into_owned()
    } else {
        text.into_owned()
    }
}

/// What the sheet hears from an export's worker.
enum Report {
    Progress(f32),
    Done(Result<PathBuf, String>),
    Cancelled,
}

/// A running export.
struct Job {
    id: u64,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// The IPC command waiting for the outcome, if one asked for this export.
    ipc: bool,
    format: ExportFormat,
    size: (u32, u32),
}

/// Whether the cloud layer's first answer has come, for the sheet's wait: a
/// layer with no source never answers, so it is never waited for.
pub struct CloudGate {
    pub has_source: AtomicBool,
    pub settled: AtomicBool,
}

/// The cloud layer of this process's engine, which `app.rs` and the event
/// forwarder keep from startup, since `CloudsSettled` is sent once.
pub static CLOUDS: CloudGate = CloudGate {
    has_source: AtomicBool::new(false),
    settled: AtomicBool::new(false),
};

struct Controller {
    window: slint::Weak<MainWindow>,
    link: EngineLink,
    screens: SharedMonitors,
    presets: Vec<Preset>,
    textures_ready: bool,
    next_job: u64,
    job: Option<Job>,
    thumbnail_timer: Rc<slint::Timer>,
    thumbnail_shown: Option<(u32, u32)>,
    dialog_open: bool,
}

thread_local! {
    static CONTROLLER: RefCell<Option<Controller>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Controller) -> R) -> Option<R> {
    CONTROLLER.with(|slot| slot.borrow_mut().as_mut().map(f))
}

/// Wire the sheet to `window`, the engine behind `link` and the monitors in
/// `screens`.
pub fn register(window: &MainWindow, link: &EngineLink, screens: &SharedMonitors) {
    CONTROLLER.with(|slot| {
        *slot.borrow_mut() = Some(Controller {
            window: window.as_weak(),
            link: link.clone(),
            screens: Arc::clone(screens),
            presets: Vec::new(),
            textures_ready: false,
            next_job: 0,
            job: None,
            thumbnail_timer: Rc::new(slint::Timer::default()),
            thumbnail_shown: None,
            dialog_open: false,
        });
    });
    let state = window.global::<ExportState>();
    state.set_png_levels(slint::ModelRc::new(slint::VecModel::from(
        PNG_LEVELS
            .iter()
            .map(|&level| i32::from(level))
            .collect::<Vec<_>>(),
    )));
    window.on_open_export_sheet(open);
    state.on_close(close);
    state.on_size_chosen(|index| {
        with(|c| c.size_chosen(usize::try_from(index).unwrap_or(0)));
    });
    state.on_size_edited(|| {
        with(Controller::size_edited);
    });
    state.on_format_chosen(|index| {
        with(|c| c.format_chosen(index));
    });
    state.on_path_edited(|| {
        with(Controller::path_edited);
    });
    state.on_browse(browse);
    state.on_export(|| {
        with(|c| c.start(false));
    });
    state.on_cancel(|| {
        with(Controller::cancel);
    });
    state.on_show_in_folder(|| {
        with(Controller::show_in_folder);
    });
}

/// Open the sheet: hold the scene, fill the fields from the config, and wait
/// for the scene to be ready before Export is enabled.
pub fn open() {
    with(Controller::open);
}

/// Close the sheet if it is open, cancelling a running export and letting
/// the scene go. Every path that hides the main window comes through here.
pub fn close() {
    with(Controller::close);
}

/// Whether the sheet is open.
#[must_use]
pub fn is_open() -> bool {
    with(|c| c.window().is_some_and(|w| w.get_export_sheet_open())).unwrap_or(false)
}

/// The engine announced `TexturesReady`.
pub fn textures_ready() {
    with(|c| {
        c.textures_ready = true;
        c.update_ready();
    });
}

/// The engine announced `CloudsSettled`.
pub fn clouds_settled() {
    with(Controller::update_ready);
}

/// Cancel a running export and wait for its worker, after the engine has
/// stopped and answered whatever it was rendering. The last thing a run does
/// with the sheet, so a temporary file is never left behind by Quit.
pub fn shutdown() {
    let job = with(|c| c.job.take()).flatten();
    if let Some(mut job) = job {
        job.cancel.store(true, Ordering::Relaxed);
        if let Some(thread) = job.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Start an export the way the sheet's Export button does, with the fields
/// set from an IPC `export-image` command, opening the sheet first if it is
/// closed. The outcome is printed as `SIGNAL:exported path=<path>` or
/// `SIGNAL:export_failed reason=<text>`.
pub fn export_from_ipc(path: &str, width: u32, height: u32, format: ExportFormat, value: u8) {
    let started = with(|c| {
        let Some(window) = c.window() else {
            return Err("the window is gone".to_owned());
        };
        if !window.get_export_sheet_open() {
            c.open();
        }
        let state = window.global::<ExportState>();
        state.set_width_text(width.to_string().into());
        state.set_height_text(height.to_string().into());
        c.size_edited();
        state.set_format_index(format_index(format));
        match format {
            ExportFormat::Png => {
                let position = PNG_LEVELS
                    .iter()
                    .position(|&level| level >= value)
                    .unwrap_or(PNG_LEVELS.len() - 1);
                #[expect(clippy::cast_precision_loss, reason = "a slider position")]
                state.set_png_position(position as f32);
            }
            ExportFormat::Jpeg => state.set_jpeg_quality(f32::from(value.clamp(1, 100))),
        }
        state.set_path(path.into());
        c.path_edited();
        c.start_when_ready();
        Ok(())
    });
    match started {
        Some(Ok(())) => {}
        Some(Err(reason)) => crate::ipc::signal(&format!("export_failed reason={reason}")),
        None => crate::ipc::signal("export_failed reason=the export sheet is not wired"),
    }
}

/// Cancel the running export, as the sheet's Cancel button does.
pub fn cancel_from_ipc() {
    with(Controller::cancel);
}

fn format_index(format: ExportFormat) -> i32 {
    match format {
        ExportFormat::Png => 0,
        ExportFormat::Jpeg => 1,
    }
}

fn format_at(index: i32) -> ExportFormat {
    if index == 1 {
        ExportFormat::Jpeg
    } else {
        ExportFormat::Png
    }
}

impl Controller {
    fn window(&self) -> Option<MainWindow> {
        self.window.upgrade()
    }

    fn open(&mut self) {
        let Some(window) = self.window() else {
            return;
        };
        if window.get_export_sheet_open() {
            return;
        }
        debug!("export sheet opened");
        let stored = config::load_config();
        let monitors = crate::displays::monitors_of(&self.screens);
        self.presets = presets(&monitors, stored.anchor().as_deref());
        let size = match (stored.export_width, stored.export_height) {
            (Some(width), Some(height)) if width > 0 && height > 0 => (width, height),
            _ => self.presets[0].size.unwrap_or(FALLBACK_SIZE),
        };
        let state = window.global::<ExportState>();
        state.set_sizes(slint::ModelRc::new(slint::VecModel::from(
            self.presets
                .iter()
                .map(|preset| slint::SharedString::from(preset.label.as_str()))
                .collect::<Vec<_>>(),
        )));
        state.set_width_text(size.0.to_string().into());
        state.set_height_text(size.1.to_string().into());
        state.set_size_index(index_i32(preset_index(&self.presets, size)));
        state.set_format_index(format_index(stored.export_format));
        let position = PNG_LEVELS
            .iter()
            .position(|&level| level >= stored.export_png_compression)
            .unwrap_or(PNG_LEVELS.len() - 1);
        #[expect(clippy::cast_precision_loss, reason = "a slider position")]
        state.set_png_position(position as f32);
        state.set_jpeg_quality(f32::from(stored.export_jpeg_quality.clamp(1, 100)));
        let fallback = fallback_folder(dirs::picture_dir(), dirs::home_dir());
        state.set_path(starting_path(&stored.export_path, &fallback, stored.export_format).into());
        state.set_thumbnail(slint::Image::default());
        self.thumbnail_shown = None;
        self.textures_ready = false;
        state.set_ready(false);
        state.set_exporting(false);
        state.set_cancelling(false);
        Self::clear_status(&window);
        Self::check_replace(&window);

        self.link.send(EngineCommand::Hold(true));
        window.set_export_sheet_open(true);
    }

    fn close(&mut self) {
        let Some(window) = self.window() else {
            return;
        };
        if !window.get_export_sheet_open() {
            return;
        }
        debug!("export sheet closed");
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.thumbnail_timer.stop();
        self.link.send(EngineCommand::Hold(false));
        window.set_export_sheet_open(false);
    }

    /// Whether the cloud layer still owes the answer an export waits for.
    fn clouds_pending(&self, window: &MainWindow) -> bool {
        CLOUDS.has_source.load(Ordering::Relaxed)
            && !CLOUDS.settled.load(Ordering::Relaxed)
            && crate::ui_callbacks::read_params_from_window(window, self.link.aa_counts())
                .draws_clouds()
    }

    fn update_ready(&mut self) {
        let Some(window) = self.window() else {
            return;
        };
        if !window.get_export_sheet_open() {
            return;
        }
        let ready = self.textures_ready && !self.clouds_pending(&window);
        let state = window.global::<ExportState>();
        if ready && !state.get_ready() {
            state.set_ready(true);
            self.request_thumbnail();
        }
    }

    fn typed_size(window: &MainWindow) -> Option<(u32, u32)> {
        let state = window.global::<ExportState>();
        let width = state.get_width_text().trim().parse::<u32>().ok()?;
        let height = state.get_height_text().trim().parse::<u32>().ok()?;
        (width > 0 && height > 0).then_some((width, height))
    }

    fn size_chosen(&mut self, index: usize) {
        let Some(window) = self.window() else {
            return;
        };
        if let Some(Some((width, height))) = self.presets.get(index).map(|preset| preset.size) {
            let state = window.global::<ExportState>();
            state.set_width_text(width.to_string().into());
            state.set_height_text(height.to_string().into());
        }
        self.changed(&window);
    }

    fn size_edited(&mut self) {
        let Some(window) = self.window() else {
            return;
        };
        if let Some(size) = Self::typed_size(&window) {
            let index = preset_index(&self.presets, size);
            window
                .global::<ExportState>()
                .set_size_index(index_i32(index));
        } else {
            window
                .global::<ExportState>()
                .set_size_index(index_i32(self.presets.len().saturating_sub(1)));
        }
        self.changed(&window);
    }

    fn format_chosen(&mut self, index: i32) {
        let Some(window) = self.window() else {
            return;
        };
        let state = window.global::<ExportState>();
        let path = with_format(&state.get_path(), format_at(index));
        state.set_path(path.into());
        Self::check_replace(&window);
        self.changed(&window);
    }

    fn path_edited(&mut self) {
        let Some(window) = self.window() else {
            return;
        };
        let state = window.global::<ExportState>();
        if let Some(format) = format_of_path(&state.get_path()) {
            state.set_format_index(format_index(format));
        }
        Self::check_replace(&window);
        self.changed(&window);
    }

    /// Something in the sheet changed: the outcome of the last export goes,
    /// and the thumbnail follows the size once it has settled.
    fn changed(&mut self, window: &MainWindow) {
        if self.job.is_none() {
            Self::clear_status(window);
        }
        self.schedule_thumbnail();
    }

    fn clear_status(window: &MainWindow) {
        let state = window.global::<ExportState>();
        state.set_status_shown(false);
        state.set_progress_shown(false);
        state.set_saved(false);
        state.set_status("".into());
        state.set_progress(0.0);
    }

    fn show_status(window: &MainWindow, text: &str, progress: bool) {
        let state = window.global::<ExportState>();
        state.set_status_shown(true);
        state.set_progress_shown(progress);
        state.set_status(text.into());
    }

    fn check_replace(window: &MainWindow) {
        let state = window.global::<ExportState>();
        let path = state.get_path();
        let target = export_target(&path, format_at(state.get_format_index()));
        state.set_replace_warning(!path.trim().is_empty() && target.is_file());
    }

    fn schedule_thumbnail(&mut self) {
        self.thumbnail_timer
            .start(slint::TimerMode::SingleShot, THUMBNAIL_SETTLE, || {
                with(Controller::request_thumbnail);
            });
    }

    /// Draw the thumbnail at the aspect of the typed size, unless it is
    /// already drawn at that aspect or the scene is not ready.
    fn request_thumbnail(&mut self) {
        let Some(window) = self.window() else {
            return;
        };
        let state = window.global::<ExportState>();
        if !window.get_export_sheet_open() || !state.get_ready() {
            return;
        }
        let Some(size) = Self::typed_size(&window) else {
            return;
        };
        let small = thumbnail_size(size);
        if self.thumbnail_shown == Some(small) {
            return;
        }
        self.thumbnail_shown = Some(small);
        let Ok(replies) = self.link.request_pixels(small.0, small.1) else {
            return;
        };
        // One short-lived thread per thumbnail. Its consumer is the sheet,
        // which drops a thumbnail that arrives after it was closed; it runs
        // whenever the aspect changes while the sheet is open, and ends with
        // the engine's answer.
        let spawned = std::thread::Builder::new()
            .name("export-thumbnail".to_owned())
            .spawn(move || {
                let Ok(Ok(pixels)) = replies.recv() else {
                    return;
                };
                let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                    &pixels, small.0, small.1,
                );
                drop(pixels);
                let _ = slint::invoke_from_event_loop(move || {
                    with(|c| {
                        if let Some(window) = c.window()
                            && window.get_export_sheet_open()
                            && c.thumbnail_shown == Some(small)
                        {
                            window
                                .global::<ExportState>()
                                .set_thumbnail(slint::Image::from_rgba8(buffer));
                        }
                    });
                });
            });
        if let Err(e) = spawned {
            warn!("the thumbnail could not be drawn: {e}");
        }
    }

    fn show_in_folder(&mut self) {
        let Some(window) = self.window() else {
            return;
        };
        let state = window.global::<ExportState>();
        let target = export_target(&state.get_path(), format_at(state.get_format_index()));
        if let Some(folder) = target.parent() {
            crate::open::folder(folder);
        }
    }

    /// Start the export as soon as the scene is ready, for IPC, which cannot
    /// press a button that is not enabled yet.
    fn start_when_ready(&mut self) {
        let Some(window) = self.window() else {
            return;
        };
        if window.global::<ExportState>().get_ready() {
            self.start(true);
            return;
        }
        let timer = Rc::new(slint::Timer::default());
        let keep = Rc::clone(&timer);
        let started = Instant::now();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(50),
            move || {
                let _keep = &keep;
                let ready = with(|c| {
                    c.window()
                        .is_some_and(|w| w.global::<ExportState>().get_ready())
                })
                .unwrap_or(false);
                if ready {
                    keep.stop();
                    with(|c| c.start(true));
                } else if started.elapsed() > Duration::from_mins(2) || !is_open() {
                    keep.stop();
                    crate::ipc::signal("export_failed reason=the scene did not become ready");
                }
            },
        );
    }

    /// Export with what the sheet says now.
    fn start(&mut self, ipc: bool) {
        let Some(window) = self.window() else {
            return;
        };
        let state = window.global::<ExportState>();
        if self.job.is_some() {
            if ipc {
                crate::ipc::signal("export_failed reason=an export is already running");
            }
            return;
        }
        let fail = |text: &str| {
            Self::show_status(&window, text, false);
            if ipc {
                crate::ipc::signal(&format!("export_failed reason={text}"));
            }
        };
        let Some((width, height)) = Self::typed_size(&window) else {
            fail("Type a width and a height of at least one pixel.");
            return;
        };
        let format = format_at(state.get_format_index());
        let target = export_target(&state.get_path(), format);
        if let Err(text) = check_target(&target) {
            fail(&text);
            return;
        }
        let image_format = match format {
            ExportFormat::Png => ImageFormat::Png {
                compression: u8::try_from(state.get_png_level()).unwrap_or(3),
            },
            ExportFormat::Jpeg => ImageFormat::Jpeg {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "a slider from 1 to 100"
                )]
                quality: state.get_jpeg_quality().round().clamp(1.0, 100.0) as u8,
            },
        };
        let replies = match self.link.request_pixels(width, height) {
            Ok(replies) => replies,
            Err(e) => {
                fail(&e.to_string());
                return;
            }
        };
        info!(width, height, path = %target.display(), "exporting an image");
        self.next_job += 1;
        let id = self.next_job;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_target = target.clone();
        // The export's worker. Its consumer is the sheet, through the event
        // loop; it runs once per export, there is at most one, and it ends
        // with the file written, a failure or the cancel. The pixel buffer
        // lives on this thread alone.
        let spawned = std::thread::Builder::new()
            .name("export-image".to_owned())
            .spawn(move || {
                let report = encode(
                    &replies,
                    &worker_target,
                    width,
                    height,
                    image_format,
                    &worker_cancel,
                    id,
                );
                post(id, report);
            });
        let thread = match spawned {
            Ok(thread) => thread,
            Err(e) => {
                fail(&format!("The export could not start: {e}"));
                return;
            }
        };
        self.job = Some(Job {
            id,
            cancel,
            thread: Some(thread),
            ipc,
            format,
            size: (width, height),
        });
        state.set_exporting(true);
        state.set_cancelling(false);
        state.set_saved(false);
        state.set_progress(REQUESTED);
        Self::show_status(&window, "Rendering…", true);
    }

    fn cancel(&mut self) {
        let Some(job) = &self.job else {
            return;
        };
        job.cancel.store(true, Ordering::Relaxed);
        if let Some(window) = self.window() {
            window.global::<ExportState>().set_cancelling(true);
            Self::show_status(&window, "Cancelling…", true);
        }
    }

    fn report(&mut self, id: u64, report: Report) {
        if self.job.as_ref().is_none_or(|job| job.id != id) {
            return;
        }
        let window = self.window();
        if let Report::Progress(fraction) = report {
            if let Some(window) = window {
                let state = window.global::<ExportState>();
                if fraction >= RENDERED && state.get_progress() < RENDERED {
                    state.set_status(
                        if state.get_cancelling() {
                            "Cancelling…"
                        } else {
                            "Saving…"
                        }
                        .into(),
                    );
                }
                state.set_progress(fraction);
            }
            return;
        }
        let Some(mut job) = self.job.take() else {
            return;
        };
        if let Some(thread) = job.thread.take() {
            let _ = thread.join();
        }
        let open = window
            .as_ref()
            .is_some_and(MainWindow::get_export_sheet_open);
        let state = window.as_ref().map(ComponentHandle::global::<ExportState>);
        if let Some(state) = &state {
            state.set_exporting(false);
            state.set_cancelling(false);
        }
        match report {
            Report::Done(Ok(path)) => {
                info!(path = %path.display(), "image exported");
                remember(&path, job.format, job.size, state.as_ref());
                if let (Some(window), Some(state)) = (&window, &state) {
                    state.set_progress(1.0);
                    let name = path.file_name().map_or_else(
                        || path.display().to_string(),
                        |name| name.to_string_lossy().into_owned(),
                    );
                    Self::show_status(window, &format!("Saved to {name}"), true);
                    state.set_saved(open);
                    Self::check_replace(window);
                }
                if job.ipc {
                    crate::ipc::signal(&format!("exported path={}", path.display()));
                }
            }
            Report::Done(Err(reason)) => {
                warn!("the export failed: {reason}");
                if let Some(window) = &window {
                    Self::show_status(window, &reason, true);
                }
                if job.ipc {
                    crate::ipc::signal(&format!("export_failed reason={reason}"));
                }
            }
            Report::Cancelled => {
                debug!("the export was cancelled");
                if let Some(window) = &window {
                    Self::show_status(window, "Cancelled.", true);
                }
                if job.ipc {
                    crate::ipc::signal("export_failed reason=cancelled");
                }
            }
            Report::Progress(_) => {}
        }
    }
}

/// Wait for the pixels and encode them, telling the sheet how far it got.
fn encode(
    replies: &crossbeam_channel::Receiver<Result<Vec<u8>, sunlit_core::engine::ExportError>>,
    target: &Path,
    width: u32,
    height: u32,
    format: ImageFormat,
    cancel: &AtomicBool,
    id: u64,
) -> Report {
    let pixels = match replies.recv() {
        Ok(Ok(pixels)) => pixels,
        Ok(Err(e)) => return Report::Done(Err(e.to_string())),
        Err(_) => return Report::Done(Err("The renderer stopped before answering.".to_owned())),
    };
    if cancel.load(Ordering::Relaxed) {
        return Report::Cancelled;
    }
    post(id, Report::Progress(RENDERED));
    let last = std::cell::Cell::new(Instant::now());
    let written = image_file::write_image(
        target,
        &pixels,
        width,
        height,
        format,
        &|fraction| {
            if last.get().elapsed() >= PROGRESS_INTERVAL {
                last.set(Instant::now());
                post(
                    id,
                    Report::Progress(RENDERED + fraction * (ENCODED - RENDERED)),
                );
            }
        },
        cancel,
    );
    match written {
        Ok(()) => Report::Done(Ok(target.to_path_buf())),
        Err(WriteError::Cancelled) => Report::Cancelled,
        Err(WriteError::Failed(reason)) => Report::Done(Err(reason)),
    }
}

fn post(id: u64, report: Report) {
    let _ = slint::invoke_from_event_loop(move || {
        with(|c| c.report(id, report));
    });
}

/// Save what an export that succeeded was made with.
fn remember(path: &Path, format: ExportFormat, size: (u32, u32), state: Option<&ExportState<'_>>) {
    let mut stored = config::load_config();
    stored.export_width = Some(size.0);
    stored.export_height = Some(size.1);
    stored.export_format = format;
    stored.export_path = stored_path(path);
    if let Some(state) = state {
        stored.export_png_compression = u8::try_from(state.get_png_level()).unwrap_or(3);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a slider from 1 to 100"
        )]
        let quality = state.get_jpeg_quality().round().clamp(1.0, 100.0) as u8;
        stored.export_jpeg_quality = quality;
    }
    config::save_config(&stored);
}

/// The size of a thumbnail of an export of `size`: [`THUMBNAIL_EDGE`] on
/// its long edge.
#[must_use]
pub fn thumbnail_size((width, height): (u32, u32)) -> (u32, u32) {
    let long = u64::from(width.max(height));
    let scale = |side: u32| {
        u32::try_from((u64::from(side) * u64::from(THUMBNAIL_EDGE) + long / 2) / long)
            .unwrap_or(1)
            .max(1)
    };
    (scale(width), scale(height))
}

fn index_i32(index: usize) -> i32 {
    i32::try_from(index).unwrap_or(0)
}

/// Ask where to save through the platform's dialog, parented on the main
/// window, starting from the path in the field.
fn browse() {
    let Some((window, path, format)) = with(|c| {
        if c.dialog_open {
            return None;
        }
        let window = c.window()?;
        let state = window.global::<ExportState>();
        let format = format_at(state.get_format_index());
        let path = state.get_path().to_string();
        c.dialog_open = true;
        Some((window.clone_strong(), path, format))
    })
    .flatten() else {
        return;
    };
    let target = export_target(&path, format);
    let mut dialog = rfd::AsyncFileDialog::new().set_title("Export Image");
    if let Some(folder) = target.parent().filter(|folder| folder.is_dir()) {
        dialog = dialog.set_directory(folder);
    }
    if let Some(name) = target.file_name() {
        dialog = dialog.set_file_name(name.to_string_lossy());
    }
    let filters: [(ExportFormat, &str, &[&str]); 2] = [
        (ExportFormat::Png, "PNG image", &["png"]),
        (ExportFormat::Jpeg, "JPEG image", &["jpg", "jpeg"]),
    ];
    for (_, name, extensions) in filters.iter().filter(|(f, _, _)| *f == format) {
        dialog = dialog.add_filter(*name, extensions);
    }
    for (_, name, extensions) in filters.iter().filter(|(f, _, _)| *f != format) {
        dialog = dialog.add_filter(*name, extensions);
    }
    dialog = dialog.set_parent(&window.window().window_handle());
    let opened = Instant::now();
    let spawned = slint::spawn_local(async move {
        let picked = dialog.save_file().await;
        with(|c| {
            c.dialog_open = false;
            let Some(window) = c.window() else {
                return;
            };
            match picked {
                Some(file) => {
                    let picked = file.path().to_path_buf();
                    let text = picked.display().to_string();
                    let state = window.global::<ExportState>();
                    let format = format_of_path(&text)
                        .unwrap_or_else(|| format_at(state.get_format_index()));
                    let full = export_target(&text, format);
                    state.set_format_index(format_index(format));
                    state.set_path(full.display().to_string().into());
                    let mut stored = config::load_config();
                    stored.export_path = stored_path(&full);
                    config::save_config(&stored);
                    Controller::check_replace(&window);
                    c.changed(&window);
                }
                None if opened.elapsed() < NO_PICKER => {
                    warn!("the save dialog answered at once; no file picker is available");
                    Controller::show_status(
                        &window,
                        "No file picker opened. Type the path above.",
                        false,
                    );
                }
                None => {}
            }
        });
    });
    if let Err(e) = spawned {
        warn!("the save dialog could not be opened: {e}");
        with(|c| c.dialog_open = false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ScratchDir;

    fn monitor(id: &str, label: &str, width: u32, height: u32, primary: bool) -> Monitor {
        Monitor {
            id: id.to_owned(),
            label: label.to_owned(),
            x: 0,
            y: 0,
            width,
            height,
            primary,
        }
    }

    fn sizes(list: &[Preset]) -> Vec<Option<(u32, u32)>> {
        list.iter().map(|preset| preset.size).collect()
    }

    #[test]
    fn one_monitor_comes_first_and_custom_last() {
        let list = presets(&[monitor("a", "Left", 2560, 1080, true)], None);
        assert_eq!(list[0].size, Some((2560, 1080)));
        assert!(list[0].label.contains("Left"), "{}", list[0].label);
        assert_eq!(list.last().unwrap().size, None);
        assert!(list.len() > 3);
    }

    #[test]
    fn two_monitors_of_one_size_are_one_entry_naming_both() {
        let list = presets(
            &[
                monitor("a", "Left", 2560, 1080, true),
                monitor("b", "Right", 2560, 1080, false),
            ],
            None,
        );
        let entries: Vec<_> = list
            .iter()
            .filter(|preset| preset.size == Some((2560, 1080)))
            .collect();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].label.contains("Left") && entries[0].label.contains("Right"));
    }

    #[test]
    fn the_anchor_comes_first() {
        let monitors = [
            monitor("a", "Left", 1680, 1050, true),
            monitor("b", "Right", 2560, 1080, false),
        ];
        assert_eq!(presets(&monitors, Some("b"))[0].size, Some((2560, 1080)));
        assert_eq!(presets(&monitors, None)[0].size, Some((1680, 1050)));
    }

    #[test]
    fn without_monitors_the_common_sizes_and_custom_remain() {
        let list = presets(&[], None);
        assert!(list.len() > 1);
        assert!(
            list[..list.len() - 1]
                .iter()
                .all(|preset| preset.size.is_some())
        );
        assert_eq!(list.last().unwrap().size, None);
    }

    #[test]
    fn a_monitor_of_a_common_size_is_listed_once() {
        let list = presets(&[monitor("a", "Office", 3840, 2160, true)], None);
        let found = sizes(&list)
            .into_iter()
            .filter(|size| *size == Some((3840, 2160)))
            .count();
        assert_eq!(found, 1);
        assert!(list[0].label.contains("Office"));
    }

    #[test]
    fn a_size_selects_its_entry_or_custom() {
        let list = presets(&[monitor("a", "Left", 2560, 1080, true)], None);
        assert_eq!(preset_index(&list, (2560, 1080)), 0);
        assert_eq!(preset_index(&list, (1234, 567)), list.len() - 1);
    }

    #[test]
    fn the_target_gets_the_formats_extension() {
        let png = ExportFormat::Png;
        let jpeg = ExportFormat::Jpeg;
        assert_eq!(
            export_target("/a/earth", png),
            PathBuf::from("/a/earth.png")
        );
        assert_eq!(
            export_target("/a/earth.PNG", png),
            PathBuf::from("/a/earth.PNG")
        );
        assert_eq!(
            export_target("/a/earth.jpeg", jpeg),
            PathBuf::from("/a/earth.jpeg")
        );
        assert_eq!(
            export_target("/a/earth.jpg", jpeg),
            PathBuf::from("/a/earth.jpg")
        );
        assert_eq!(
            export_target("/a/earth.jpg", png),
            PathBuf::from("/a/earth.png")
        );
        assert_eq!(
            export_target("/a/earth.txt", png),
            PathBuf::from("/a/earth.txt.png")
        );
        assert_eq!(
            export_target("/my.folder/earth", jpeg),
            PathBuf::from("/my.folder/earth.jpg")
        );
        assert_eq!(
            export_target("/a/earth.", png),
            PathBuf::from("/a/earth.png")
        );
    }

    #[test]
    fn a_typed_extension_names_the_format_and_a_format_rewrites_it() {
        assert_eq!(format_of_path("/a/b.JPEG"), Some(ExportFormat::Jpeg));
        assert_eq!(format_of_path("/a/b.png"), Some(ExportFormat::Png));
        assert_eq!(format_of_path("/a/b.txt"), None);
        assert_eq!(format_of_path("/a.b/c"), None);
        assert_eq!(with_format("/a/b.png", ExportFormat::Jpeg), "/a/b.jpg");
        assert_eq!(with_format("/a/b.JPG", ExportFormat::Png), "/a/b.png");
        assert_eq!(with_format("/a/b.jpeg", ExportFormat::Jpeg), "/a/b.jpeg");
        assert_eq!(with_format("/a/b", ExportFormat::Jpeg), "/a/b");
    }

    #[test]
    fn the_starting_path_keeps_a_stored_folder_that_exists() {
        let dir = ScratchDir::new("export_start");
        let fallback = dir.path().join("fallback");
        let stored = dir.path().join("renders").join("globe");
        std::fs::create_dir_all(stored.parent().unwrap()).unwrap();
        let path = starting_path(&stored.to_string_lossy(), &fallback, ExportFormat::Png);
        assert_eq!(PathBuf::from(path), stored.with_extension("png"));
    }

    #[test]
    fn a_stored_folder_that_is_gone_keeps_the_name() {
        let dir = ScratchDir::new("export_start_gone");
        let stored = dir.path().join("gone").join("globe");
        let path = starting_path(&stored.to_string_lossy(), dir.path(), ExportFormat::Jpeg);
        assert_eq!(PathBuf::from(path), dir.path().join("globe.jpg"));
    }

    #[test]
    fn before_the_first_export_the_name_is_the_apps() {
        let dir = ScratchDir::new("export_start_unset");
        let path = starting_path("", dir.path(), ExportFormat::Png);
        assert_eq!(PathBuf::from(path), dir.path().join("sunlit-earth.png"));
    }

    #[test]
    fn without_a_pictures_folder_the_home_folder_is_used() {
        let dir = ScratchDir::new("export_fallback");
        let home = dir.path().join("home");
        let pictures = dir.path().join("no-pictures");
        assert_eq!(fallback_folder(Some(pictures), Some(home.clone())), home);
        assert_eq!(fallback_folder(None, Some(home.clone())), home);
        std::fs::create_dir_all(dir.path().join("pictures")).unwrap();
        assert_eq!(
            fallback_folder(Some(dir.path().join("pictures")), Some(home)),
            dir.path().join("pictures")
        );
    }

    #[test]
    fn a_relative_path_or_a_missing_folder_is_refused() {
        let dir = ScratchDir::new("export_target");
        assert!(check_target(Path::new("earth.png")).is_err());
        assert!(check_target(&dir.path().join("missing").join("earth.png")).is_err());
        assert!(check_target(&dir.path().join("earth.png")).is_ok());
    }

    #[test]
    fn the_stored_path_has_no_extension() {
        assert_eq!(
            PathBuf::from(stored_path(Path::new("/a/b/earth.png"))),
            PathBuf::from("/a/b/earth")
        );
        assert_eq!(
            PathBuf::from(stored_path(Path::new("/a/b/earth.txt"))),
            PathBuf::from("/a/b/earth.txt")
        );
    }

    #[test]
    fn a_thumbnail_is_two_hundred_on_its_long_edge() {
        assert_eq!(thumbnail_size((3840, 2160)), (200, 113));
        assert_eq!(thumbnail_size((1290, 2796)), (92, 200));
        assert_eq!(thumbnail_size((100_000, 1)), (200, 1));
    }

    #[test]
    fn the_offered_levels_rise() {
        assert!(PNG_LEVELS.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
