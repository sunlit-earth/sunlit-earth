//! Where a finished wallpaper goes.
//!
//! A publish is no longer one image: it is a list of monitors, a mode saying how
//! they relate, and either one picture per screen or one canvas over all of them.
//! What the sink is handed is [`WallpaperJob`]; what it does with it is the
//! platform's business, and it is allowed to do less than the mode asked as long
//! as it says so.
//!
//! Behind a trait so the engine can be soak-tested for simulated weeks without
//! repainting the desktop of the machine running the tests.

use std::sync::Arc;

use crate::display::Monitor;
use crate::display::layout::{DisplayMode, Rect, crop};

/// One finished image, as tightly packed RGBA8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl Frame {
    pub fn new(pixels: Vec<u8>, width: u32, height: u32) -> Self {
        Self {
            pixels,
            width,
            height,
        }
    }

    /// Whether the buffer is as large as the size it claims.
    pub fn is_well_formed(&self) -> bool {
        self.pixels.len() == (self.width as usize) * (self.height as usize) * 4
    }
}

/// The pictures one publish is made of.
///
/// Shared rather than owned per monitor, because two screens of the same
/// resolution get the same picture and it is tens of megabytes: the render
/// grouping's whole point is that they cost one render, and an `Arc` is what
/// carries that as far as the file the sink writes.
pub enum JobImages {
    /// One image per monitor, each already at that monitor's own size, in the
    /// same order as `WallpaperJob::monitors`.
    ///
    /// `None` is a screen this mode does not paint, which is every screen but
    /// the anchor in [`DisplayMode::OneScreen`]. A desktop that addresses
    /// monitors individually leaves those alone; one that cannot gives them the
    /// anchor's picture and says so.
    PerMonitor(Vec<Option<Arc<Frame>>>),
    /// One canvas over `bounds`; a monitor's image is the crop at its rectangle.
    ///
    /// Carried whole rather than pre-cut because some desktops want it that way
    /// and cutting it for them would be wasted work: Windows has a span position
    /// and the gsettings schemas have a `spanned` picture-option.
    Spanned { canvas: Arc<Frame>, bounds: Rect },
}

/// Everything a publish needs to know about what it is publishing.
pub struct WallpaperJob {
    /// What the settings asked for. The sink may do less; it may not do more.
    pub mode: DisplayMode,
    /// Every monitor this publish covers, in the layout's own order.
    pub monitors: Vec<Monitor>,
    /// Which of them the plan is anchored to.
    ///
    /// The screen [`DisplayMode::OneScreen`] paints, the screen the span is
    /// centered on, and the one image a desktop that holds only one is given.
    pub anchor: usize,
    pub images: JobImages,
}

impl WallpaperJob {
    /// The monitor the plan is anchored to.
    pub fn anchor_monitor(&self) -> Option<&Monitor> {
        self.monitors.get(self.anchor)
    }

    /// The picture for one monitor, cutting the canvas where the job spans.
    ///
    /// `None` is a screen this publish deliberately leaves as it is. The crop of
    /// a span is a new buffer; a per-monitor image is shared rather than copied.
    pub fn image_for(&self, index: usize) -> Result<Option<Arc<Frame>>, String> {
        let monitor = self
            .monitors
            .get(index)
            .ok_or_else(|| format!("no monitor at position {index} in this publish"))?;
        match &self.images {
            JobImages::PerMonitor(images) => Ok(images
                .get(index)
                .ok_or_else(|| format!("no image slot for {}", monitor.label))?
                .clone()),
            JobImages::Spanned { canvas, bounds } => {
                // A screen with no pixels is one there is nothing to cut for,
                // and every other walk over a layout passes over it rather than
                // failing: `bounds_of`, `render_groups`, the diagram, and the
                // per-monitor branch above, which was never given a slot for it.
                if monitor.rect().is_empty() {
                    return Ok(None);
                }
                let rect = monitor.rect().relative_to(bounds);
                let pixels =
                    crop(&canvas.pixels, canvas.width, canvas.height, rect).ok_or_else(|| {
                        format!(
                            "{} is not inside the canvas this publish rendered",
                            monitor.label
                        )
                    })?;
                Ok(Some(Arc::new(Frame::new(pixels, rect.width, rect.height))))
            }
        }
    }

    /// The picture for the screen the plan is anchored to.
    ///
    /// Always there: a publish that painted nothing for its own anchor is one
    /// nothing downstream can make sense of.
    pub fn anchor_image(&self) -> Result<Arc<Frame>, String> {
        self.image_for(self.anchor)?
            .ok_or_else(|| "this publish has no image for its own anchor".to_owned())
    }

    /// The whole virtual desktop as one image, where this publish has one.
    pub fn canvas(&self) -> Option<&Arc<Frame>> {
        match &self.images {
            JobImages::PerMonitor(_) => None,
            JobImages::Spanned { canvas, .. } => Some(canvas),
        }
    }
}

/// Destination for finished wallpapers.
pub trait WallpaperSink: Send + Sync {
    /// Whether this sink can accept a wallpaper at all.
    ///
    /// Checked before anything is rendered. Producing a wallpaper is the most
    /// expensive thing the engine does (a render at the display's native
    /// resolution, then a readback of that whole image: about 14 MB at 2560x1440,
    /// on a CPU rasterizer where there is no hardware to help), so a sink that
    /// is going to refuse the work has to say so before it happens rather than
    /// after. Sinks that always accept keep the default.
    fn check_supported(&self) -> Result<(), String> {
        Ok(())
    }

    /// The monitors to plan for, in the platform's own order.
    ///
    /// Never empty on success: a sink with no display to ask answers with the
    /// one documented default screen rather than with nothing, because a
    /// wallpaper at a plausible size beats a refusal.
    fn monitors(&self) -> Result<Vec<Monitor>, String>;

    /// Make a finished job the desktop's wallpaper.
    ///
    /// `Ok` carries what the desktop could not do, empty where it did exactly
    /// what the mode asked. That string reaches the status line, which is where
    /// every other thing the wallpaper path has to say already goes.
    fn publish(&self, job: &WallpaperJob) -> Result<String, String>;
}

/// Render size used where there is no display to ask.
///
/// Windows enumerates the real monitors (`wallpaper::enumerate_monitors`) and
/// Linux parses `xrandr --query` (`display::outputs`). This is what is left when
/// neither answers: a headless run, a session with no output that has a mode
/// assigned, or a platform with no query at all. A common desktop resolution,
/// because the alternative is refusing to render a file somebody asked for.
///
/// Every use of it is logged where it happens, so a wallpaper at this size is
/// never silently a guess.
pub const DEFAULT_TARGET_SIZE: (u32, u32) = (2560, 1440);

/// Message returned where there is no wallpaper setter at all.
///
/// Deliberately a plain error rather than a stub that writes a PNG somewhere
/// and reports success: the UI shows this string in the status line, and a
/// wallpaper that silently did not change is worse than one that says so.
///
/// macOS only, now that Linux has a setter. It keeps the platform's name out of
/// it because what it says is true of any platform that reaches it.
#[cfg(not(any(windows, target_os = "linux")))]
const UNSUPPORTED: &str = "setting the desktop wallpaper is not supported on this platform yet";

/// The one screen a session with no display to ask is planned around.
///
/// An empty id, because there is no monitor here for a setter to address: the
/// per-monitor paths all fall back to the single image, which is what a
/// display-less run could do before there was a plan at all.
fn default_monitor() -> Monitor {
    let (width, height) = DEFAULT_TARGET_SIZE;
    Monitor {
        id: String::new(),
        label: "the default screen size".to_owned(),
        x: 0,
        y: 0,
        width,
        height,
        primary: true,
    }
}

/// The real desktop: save the PNGs and hand them to the OS.
pub struct SystemWallpaper;

impl WallpaperSink for SystemWallpaper {
    /// Whether this desktop has a setter, asked before anything is rendered.
    ///
    /// On Linux both halves of the answer are cheap and both matter: which
    /// desktop this is, and whether its setter is installed. Finding out after a
    /// full-resolution render and readback is what this exists to avoid.
    #[cfg(target_os = "linux")]
    fn check_supported(&self) -> Result<(), String> {
        let backend = crate::desktop::detect_current().ok_or_else(|| {
            crate::desktop::no_backend_message(
                &crate::env_override(crate::desktop::DESKTOP_ENV).unwrap_or_default(),
            )
        })?;
        if which(backend.program).is_none() {
            return Err(format!(
                "this is {desktop}, whose wallpaper is set with `{program}`, and \
                 that program is not on PATH",
                desktop = backend.desktop,
                program = backend.program,
            ));
        }
        Ok(())
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    fn check_supported(&self) -> Result<(), String> {
        Err(UNSUPPORTED.to_owned())
    }

    /// This session's monitors, or the one documented default screen.
    ///
    /// Never an error, for the same reason the render size never was: a
    /// wallpaper at a plausible size is worth more than a refusal, and the two
    /// ways of not knowing are logged differently because they mean different
    /// things. No display to ask is ordinary; a display that answered with
    /// nothing usable is not.
    fn monitors(&self) -> Result<Vec<Monitor>, String> {
        let (width, height) = DEFAULT_TARGET_SIZE;
        let Some(monitors) = crate::display::monitors() else {
            tracing::info!(
                width,
                height,
                "no display to ask about its monitors; rendering the wallpaper \
                 at the documented default size"
            );
            return Ok(vec![default_monitor()]);
        };
        if monitors.is_empty() {
            tracing::warn!(
                width,
                height,
                "no display output has a mode assigned; rendering the wallpaper \
                 at the documented default size"
            );
            return Ok(vec![default_monitor()]);
        }
        Ok(monitors)
    }

    #[cfg(windows)]
    fn publish(&self, job: &WallpaperJob) -> Result<String, String> {
        let note = crate::wallpaper::set_wallpaper_job(job)?;
        crate::memory::log_memory_usage("after wallpaper set");
        Ok(note)
    }

    /// Write the PNGs and run the desktop's own setter.
    ///
    /// The backend is looked up again rather than cached from `check_supported`:
    /// the sink outlives a session change, and running the previous desktop's
    /// setter would fail in a way that named the wrong desktop.
    #[cfg(target_os = "linux")]
    fn publish(&self, job: &WallpaperJob) -> Result<String, String> {
        let backend = crate::desktop::detect_current().ok_or_else(|| {
            crate::desktop::no_backend_message(
                &crate::env_override(crate::desktop::DESKTOP_ENV).unwrap_or_default(),
            )
        })?;
        let placement = write_placement(job, backend.reach())?;

        let discovered = match backend.discovery() {
            Some(query) => run(&query)?,
            None => String::new(),
        };
        let commands = backend.commands(&placement, &discovered);
        if commands.is_empty() {
            return Err(backend.nothing_to_run());
        }
        for command in &commands {
            run(command)?;
        }

        tracing::info!(
            path = %placement.single.display(),
            desktop = backend.desktop,
            commands = commands.len(),
            screens = job.monitors.len(),
            "wallpaper set successfully"
        );
        crate::memory::log_memory_usage("after wallpaper set");
        Ok(backend
            .degradation(job.mode, job.monitors.len())
            .unwrap_or_default())
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    fn publish(&self, _job: &WallpaperJob) -> Result<String, String> {
        Err(UNSUPPORTED.to_owned())
    }
}

/// The images of one publish in the order a person sees the screens, left to
/// right and then top to bottom.
///
/// Not the order the monitors arrived in: `xrandr` lists connectors, and where a
/// connector sits in that list says nothing about where its screen sits. KDE is
/// the row that depends on this, because Plasma addresses a screen by position
/// and never by name, so a session whose connectors are listed right to left
/// would otherwise have its two pictures swapped.
#[cfg(any(target_os = "linux", test))]
fn in_layout_order(
    mut placed: Vec<(i32, i32, Option<std::path::PathBuf>)>,
) -> Vec<Option<std::path::PathBuf>> {
    placed.sort_by_key(|&(x, y, _)| (x, y));
    placed.into_iter().map(|(_, _, path)| path).collect()
}

/// Write the files one desktop's reach calls for, and say where they went.
///
/// Cutting a canvas is only done for a desktop that addresses monitors
/// individually. A desktop that can span is handed the canvas whole, and one
/// that holds a single image is handed the anchor's own picture even in the span
/// mode, because a canvas zoomed onto every screen separately is not the view it
/// was cut to be.
#[cfg(target_os = "linux")]
fn write_placement(
    job: &WallpaperJob,
    reach: crate::desktop::Reach,
) -> Result<crate::desktop::Placement, String> {
    use crate::desktop::{Placement, Reach};

    let mut publication = crate::wallpaper::begin_publication()?;
    match reach {
        Reach::PerMonitor => {
            let mut written: Vec<(Arc<Frame>, std::path::PathBuf)> = Vec::new();
            let mut per_monitor = Vec::with_capacity(job.monitors.len());
            let mut untouched = Vec::new();
            let mut placed: Vec<(i32, i32, Option<std::path::PathBuf>)> =
                Vec::with_capacity(job.monitors.len());
            let mut anchor = None;
            for (index, monitor) in job.monitors.iter().enumerate() {
                let Some(frame) = job.image_for(index)? else {
                    untouched.push(monitor.id.clone());
                    placed.push((monitor.x, monitor.y, None));
                    continue;
                };
                // Two screens showing the same picture cost one render, and
                // this is what carries that as far as the file: one encode and
                // one path, named after whichever screen came first.
                let seen = written
                    .iter()
                    .find(|(seen, _)| Arc::ptr_eq(seen, &frame))
                    .map(|(_, path)| path.clone());
                let path = if let Some(path) = seen {
                    path
                } else {
                    let path = publication.write(
                        &index.to_string(),
                        &frame.pixels,
                        frame.width,
                        frame.height,
                    )?;
                    written.push((Arc::clone(&frame), path.clone()));
                    path
                };
                if index == job.anchor {
                    anchor = Some(path.clone());
                }
                placed.push((monitor.x, monitor.y, Some(path.clone())));
                per_monitor.push((monitor.id.clone(), path));
            }
            let single =
                anchor.ok_or_else(|| "this publish has no image for its own anchor".to_owned())?;
            let by_position = in_layout_order(placed);
            publication.commit();
            Ok(Placement {
                per_monitor,
                untouched,
                by_position,
                single,
                spanned: false,
            })
        }
        Reach::Spanned if job.mode == DisplayMode::AcrossScreens => {
            let canvas = job.canvas().ok_or_else(|| {
                "a view across the screens was asked for without a canvas".to_owned()
            })?;
            let path = publication.write("canvas", &canvas.pixels, canvas.width, canvas.height)?;
            publication.commit();
            Ok(Placement {
                per_monitor: Vec::new(),
                untouched: Vec::new(),
                by_position: vec![Some(path.clone())],
                single: path,
                spanned: true,
            })
        }
        Reach::Spanned | Reach::OneImage => {
            let frame = job.anchor_image()?;
            let path = publication.write(
                &job.anchor.to_string(),
                &frame.pixels,
                frame.width,
                frame.height,
            )?;
            publication.commit();
            Ok(Placement::single(path))
        }
    }
}

/// Whether a program is on `PATH`, and where.
///
/// Written out rather than shelling out to `which`, which is one more program
/// that has to be installed for the check to work.
#[cfg(target_os = "linux")]
fn which(program: &str) -> Option<std::path::PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// Run one of the desktop's commands, and answer with its output.
///
/// A failure carries the program's own stderr, because the useful half of
/// "gsettings failed" is always what gsettings said.
#[cfg(target_os = "linux")]
fn run(command: &crate::desktop::Invocation) -> Result<String, String> {
    let out = std::process::Command::new(command.program)
        .args(&command.args)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", command.program))?;
    if !out.status.success() {
        return Err(format!(
            "{} {} failed: {}",
            command.program,
            command.args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A sink that reports one screen of a fixed size and throws the pixels away,
/// counting how many publishes it saw. Used by tests that care about the
/// schedule rather than the image.
pub struct CountingSink {
    size: (u32, u32),
    count: std::sync::atomic::AtomicUsize,
}

impl CountingSink {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            size: (width, height),
            count: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Number of publishes so far.
    pub fn count(&self) -> usize {
        self.count.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl WallpaperSink for CountingSink {
    fn monitors(&self) -> Result<Vec<Monitor>, String> {
        Ok(vec![Monitor {
            id: "counting-0".to_owned(),
            label: "Counting sink".to_owned(),
            x: 0,
            y: 0,
            width: self.size.0,
            height: self.size.1,
            primary: true,
        }])
    }

    fn publish(&self, job: &WallpaperJob) -> Result<String, String> {
        for index in 0..job.monitors.len() {
            let Some(frame) = job.image_for(index)? else {
                continue;
            };
            assert!(
                frame.is_well_formed(),
                "sink received a malformed frame: {} bytes for {}x{}",
                frame.pixels.len(),
                frame.width,
                frame.height
            );
        }
        self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(id: &str, x: i32, width: u32, height: u32) -> Monitor {
        Monitor {
            id: id.to_owned(),
            label: id.to_owned(),
            x,
            y: 0,
            width,
            height,
            primary: x == 0,
        }
    }

    /// A canvas whose pixels encode their own column, so a crop taken from the
    /// wrong place is not a plausible image.
    fn canvas_job(mode: DisplayMode) -> WallpaperJob {
        let monitors = vec![monitor("A", 0, 4, 2), monitor("B", 4, 4, 2)];
        let pixels: Vec<u8> = (0..2u32)
            .flat_map(|_| (0..8u32).flat_map(|x| [u8::try_from(x).unwrap(), 0, 0, 255]))
            .collect();
        WallpaperJob {
            mode,
            monitors,
            anchor: 0,
            images: JobImages::Spanned {
                canvas: Arc::new(Frame::new(pixels, 8, 2)),
                bounds: Rect {
                    x: 0,
                    y: 0,
                    width: 8,
                    height: 2,
                },
            },
        }
    }

    #[test]
    fn a_spanned_job_cuts_each_monitors_own_piece_out_of_the_canvas() {
        let job = canvas_job(DisplayMode::AcrossScreens);
        let left = job.image_for(0).unwrap().expect("the anchor's own piece");
        let right = job
            .image_for(1)
            .unwrap()
            .expect("the second screen's piece");
        assert_eq!((left.width, left.height), (4, 2));
        assert!(left.is_well_formed() && right.is_well_formed());
        assert_eq!(left.pixels[0], 0, "the canvas's first column");
        assert_eq!(right.pixels[0], 4, "the column the second screen starts at");
        assert_eq!(job.anchor_image().unwrap(), left);
        assert!(job.canvas().is_some());
        assert_eq!(job.anchor_monitor().map(|m| m.id.as_str()), Some("A"));
    }

    /// KDE addresses a screen by where it sits, so the order the connectors
    /// were listed in must not decide which screen gets which picture.
    #[test]
    fn the_images_come_out_in_the_order_the_screens_are_seen_in() {
        let path = |name: &str| Some(std::path::PathBuf::from(name));
        // Listed right to left, and above before below, which is a layout
        // xrandr will happily report and Plasma will not.
        let placed = vec![
            (1920, 0, path("/right.png")),
            (0, 1080, path("/below.png")),
            (0, 0, path("/left.png")),
        ];
        assert_eq!(
            in_layout_order(placed),
            vec![path("/left.png"), path("/below.png"), path("/right.png")]
        );
    }

    /// A screen the mode does not paint keeps its place, because dropping it
    /// would address every screen after it one position too early.
    #[test]
    fn a_screen_with_no_picture_still_holds_its_place() {
        let path = |name: &str| Some(std::path::PathBuf::from(name));
        let placed = vec![(1920, 0, path("/right.png")), (0, 0, None)];
        assert_eq!(in_layout_order(placed), vec![None, path("/right.png")]);
    }

    #[test]
    fn a_per_monitor_job_hands_out_the_images_it_was_built_with() {
        let shared = Arc::new(Frame::new(vec![0u8; 4 * 2 * 4], 4, 2));
        let job = WallpaperJob {
            mode: DisplayMode::EveryScreen,
            monitors: vec![monitor("A", 0, 4, 2), monitor("B", 4, 4, 2)],
            anchor: 0,
            images: JobImages::PerMonitor(vec![
                Some(Arc::clone(&shared)),
                Some(Arc::clone(&shared)),
            ]),
        };
        assert!(job.canvas().is_none());
        // Two screens of one size share the picture rather than copying it,
        // which is what makes the render grouping worth doing.
        assert!(Arc::ptr_eq(&job.image_for(0).unwrap().unwrap(), &shared));
        assert!(Arc::ptr_eq(&job.image_for(1).unwrap().unwrap(), &shared));
        assert!(job.image_for(2).is_err(), "there is no third screen");
    }

    /// One screen's mode paints one screen: the rest carry no picture at all,
    /// which is what lets a desktop that addresses them leave them as they are.
    #[test]
    fn a_one_screen_job_leaves_the_other_screens_without_a_picture() {
        let job = WallpaperJob {
            mode: DisplayMode::OneScreen,
            monitors: vec![monitor("A", 0, 4, 2), monitor("B", 4, 4, 2)],
            anchor: 0,
            images: JobImages::PerMonitor(vec![
                Some(Arc::new(Frame::new(vec![0u8; 4 * 2 * 4], 4, 2))),
                None,
            ]),
        };
        assert!(job.image_for(0).unwrap().is_some());
        assert!(job.image_for(1).unwrap().is_none());
        assert!(job.anchor_image().is_ok());
    }

    #[test]
    fn a_monitor_outside_the_canvas_is_refused_rather_than_cropped_to_black() {
        let mut job = canvas_job(DisplayMode::AcrossScreens);
        job.monitors[1] = monitor("B", 6, 4, 2);
        let refusal = job.image_for(1).expect_err("that screen leaves the canvas");
        assert!(refusal.contains('B'), "{refusal}");
    }

    /// A monitor with a zero dimension is skipped everywhere else a layout is
    /// walked, so it is skipped here too. Refusing it made one screen with no
    /// pixels the end of the whole publish, and only in the spanned mode.
    #[test]
    fn a_screen_with_no_pixels_is_passed_over_rather_than_refused() {
        let mut job = canvas_job(DisplayMode::AcrossScreens);
        job.monitors[1] = monitor("B", 4, 0, 2);
        assert!(job.image_for(1).expect("not a failure").is_none());
        // The screen that does have pixels is still cut as it was.
        assert!(job.image_for(0).unwrap().is_some());
    }

    #[test]
    fn counting_sink_starts_empty_and_counts_publishes() {
        let sink = CountingSink::new(4, 2);
        assert!(sink.check_supported().is_ok());
        let monitors = sink.monitors().unwrap();
        assert_eq!((monitors[0].width, monitors[0].height), (4, 2));
        assert_eq!(sink.count(), 0);
        let job = || WallpaperJob {
            mode: DisplayMode::EveryScreen,
            monitors: monitors.clone(),
            anchor: 0,
            images: JobImages::PerMonitor(vec![Some(Arc::new(Frame::new(
                vec![0u8; 4 * 2 * 4],
                4,
                2,
            )))]),
        };
        sink.publish(&job()).unwrap();
        sink.publish(&job()).unwrap();
        assert_eq!(sink.count(), 2);
    }

    /// The desktop sink accepts wallpapers on the platforms that can publish
    /// them.
    ///
    /// Split by cfg rather than skipped, because each half is an assertion: a
    /// platform with a setter must not report itself unsupported, and one
    /// without must refuse before anything is rendered.
    #[test]
    #[cfg(windows)]
    fn system_wallpaper_accepts_frames_on_windows() {
        assert!(SystemWallpaper.check_supported().is_ok());
    }

    /// On Linux the answer depends on the session, which a unit test does not
    /// have, so what is asserted is that the refusal explains itself. Both
    /// causes are refusals with something to act on: no desktop, and a desktop
    /// whose setter is not installed.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_linux_refusal_names_the_desktop_or_the_missing_program() {
        match SystemWallpaper.check_supported() {
            Ok(()) => {
                // A desktop with its setter present, which is what the guest
                // has: then the backend was found and probed.
                let backend =
                    crate::desktop::detect_current().expect("supported means a backend was found");
                assert!(which(backend.program).is_some());
            }
            Err(refusal) => {
                assert!(
                    refusal.contains(crate::desktop::DESKTOP_ENV)
                        || refusal.contains("not supported on")
                        || refusal.contains("not on PATH"),
                    "{refusal}"
                );
            }
        }
    }

    #[test]
    #[cfg(not(any(windows, target_os = "linux")))]
    fn system_wallpaper_refuses_before_anything_is_rendered() {
        let refusal = SystemWallpaper
            .check_supported()
            .expect_err("there is no wallpaper setter on this platform yet");
        assert_eq!(refusal, UNSUPPORTED);
        // And it stays refused at the far end, so a caller that ignores the
        // check still cannot believe a wallpaper was set.
        let job = WallpaperJob {
            mode: DisplayMode::EveryScreen,
            monitors: vec![default_monitor()],
            anchor: 0,
            images: JobImages::PerMonitor(vec![Some(Arc::new(Frame::new(vec![0u8; 4], 1, 1)))]),
        };
        assert_eq!(SystemWallpaper.publish(&job).unwrap_err(), UNSUPPORTED);
    }

    /// The monitor list is an answer or the documented default, and never an
    /// error: a wallpaper at a plausible size beats a refusal.
    #[test]
    fn the_monitors_are_this_session_or_the_documented_default() {
        let monitors = SystemWallpaper
            .monitors()
            .expect("a monitor list is always available");
        assert!(
            !monitors.is_empty(),
            "a plan with no screen in it renders nothing"
        );
        for monitor in &monitors {
            assert!(monitor.width > 0 && monitor.height > 0, "{monitor:?}");
        }
        // With no display to ask, which is what a headless test run is, it is
        // the one documented constant rather than a guess of its own.
        if crate::display::monitors().is_none_or(|list| list.is_empty()) {
            assert_eq!(monitors, vec![default_monitor()]);
            assert_eq!((monitors[0].width, monitors[0].height), DEFAULT_TARGET_SIZE);
        }
    }
}
