//! The wallpaper sinks the cases publish into, and what they record.

use std::sync::Arc;
use std::sync::Mutex;

use sunlit_core::display::Monitor;
use sunlit_core::display::layout::DisplayMode;
use sunlit_core::engine::EngineCommand;
use sunlit_core::engine::wallpaper_sink::Frame;
use sunlit_core::engine::wallpaper_sink::JobImages;
use sunlit_core::engine::wallpaper_sink::WallpaperJob;
use sunlit_core::engine::wallpaper_sink::WallpaperSink;
use sunlit_core::params::SceneParams;

use crate::groups::Plain;
use crate::harness::test_params;

/// One fabricated monitor.
pub(crate) fn screen(id: &str, x: i32, width: u32, height: u32, primary: bool) -> Monitor {
    Monitor {
        id: id.to_owned(),
        label: id.to_owned(),
        x,
        y: 0,
        width,
        height,
        primary,
    }
}

/// A sink that reports a fabricated layout and keeps what it was handed.
///
/// This is what proves the engine asks for the right images in each mode with
/// no display anywhere, so it runs on Windows and macOS as well as Linux. It
/// also stands in for the sink that cannot publish at all, which is what
/// `SystemWallpaper` is on a Linux desktop the table does not know: `refuse`
/// switches that on and `reset` switches it back off.
pub(crate) struct RecordingSink {
    /// Behind a lock because a display-change case moves the layout under a
    /// running engine, which is the whole thing those cases are about.
    monitors: Mutex<Vec<Monitor>>,
    /// How often the engine has asked. A recheck that changes nothing leaves no
    /// other trace, so this is what says it happened at all.
    queries: std::sync::atomic::AtomicUsize,
    /// What `check_supported` answers; `None` accepts.
    refusal: Mutex<Option<String>>,
    published: Mutex<Vec<Publication>>,
}

/// What one publish came out as, in the terms the assertions are written in.
pub(crate) struct Publication {
    pub(crate) mode: DisplayMode,
    pub(crate) anchor: usize,
    /// One entry per monitor: its size, or `None` where the mode left it alone.
    pub(crate) images: Vec<Option<(u32, u32)>>,
    /// The canvas, where the publish spanned.
    pub(crate) canvas: Option<(u32, u32)>,
    /// Each screen's finished picture, cut where the publish spanned.
    pub(crate) frames: Vec<Option<Arc<Frame>>>,
    /// How many distinct pixel buffers the publish actually rendered.
    pub(crate) renders: usize,
}

impl RecordingSink {
    /// The reason a refusing recording sink gives.
    pub(crate) const REFUSED: &str = "this recording sink refuses on purpose";

    pub(crate) fn new(monitors: Vec<Monitor>) -> Self {
        Self {
            monitors: Mutex::new(monitors),
            queries: std::sync::atomic::AtomicUsize::new(0),
            refusal: Mutex::new(None),
            published: Mutex::new(Vec::new()),
        }
    }

    /// Forget everything the previous case did to this sink.
    pub(crate) fn reset(&self, monitors: Vec<Monitor>) {
        self.set_monitors(monitors);
        *self.refusal.lock().expect("the recording is poisoned") = None;
        self.publications().clear();
        self.queries.store(0, std::sync::atomic::Ordering::SeqCst);
    }

    /// Answer every later `check_supported` with a refusal.
    pub(crate) fn refuse(&self) {
        *self.refusal.lock().expect("the recording is poisoned") = Some(Self::REFUSED.to_owned());
    }

    /// Move the layout under the running engine.
    pub(crate) fn set_monitors(&self, monitors: Vec<Monitor>) {
        *self.monitors.lock().expect("the recording is poisoned") = monitors;
    }

    pub(crate) fn queries(&self) -> usize {
        self.queries.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn publications(&self) -> std::sync::MutexGuard<'_, Vec<Publication>> {
        self.published.lock().expect("the recording is poisoned")
    }
}

impl WallpaperSink for RecordingSink {
    fn check_supported(&self) -> Result<(), String> {
        match &*self.refusal.lock().expect("the recording is poisoned") {
            Some(reason) => Err(reason.clone()),
            None => Ok(()),
        }
    }

    fn monitors(&self) -> Result<Vec<Monitor>, String> {
        self.queries
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self
            .monitors
            .lock()
            .expect("the recording is poisoned")
            .clone())
    }

    fn publish(&self, job: &WallpaperJob) -> Result<String, String> {
        let mut images = Vec::new();
        let mut frames = Vec::new();
        let mut distinct: Vec<*const u8> = Vec::new();
        for index in 0..job.monitors.len() {
            let image = job.image_for(index)?;
            images.push(image.as_ref().map(|frame| {
                assert!(frame.is_well_formed(), "a malformed frame reached the sink");
                (frame.width, frame.height)
            }));
            frames.push(image);
        }
        // Identity rather than equality: two screens of one size are meant to
        // share the buffer, and two equal buffers would not prove they did.
        if let Some(canvas) = job.canvas() {
            distinct.push(canvas.pixels.as_ptr());
        } else if let JobImages::PerMonitor(frames) = &job.images {
            for frame in frames.iter().flatten() {
                let ptr = frame.pixels.as_ptr();
                if !distinct.contains(&ptr) {
                    distinct.push(ptr);
                }
            }
        }
        self.publications().push(Publication {
            mode: job.mode,
            anchor: job.anchor,
            images,
            canvas: job.canvas().map(|frame| (frame.width, frame.height)),
            frames,
            renders: distinct.len(),
        });
        Ok(String::new())
    }
}

/// Publish once with a fabricated layout and a mode, and answer with what the
/// sink was handed.
pub(crate) fn publish_plan(
    group: &Plain,
    monitors: Vec<Monitor>,
    mode: DisplayMode,
    anchor: Option<&str>,
) -> Publication {
    publish_plan_with(group, monitors, mode, anchor, test_params())
}

/// The same, with the scene said out loud, for the cases that compare pixels.
pub(crate) fn publish_plan_with(
    group: &Plain,
    monitors: Vec<Monitor>,
    mode: DisplayMode,
    anchor: Option<&str>,
    params: SceneParams,
) -> Publication {
    group.sink.set_monitors(monitors);
    group.engine.send(EngineCommand::SetDisplayPlan {
        mode,
        anchor: anchor.map(ToOwned::to_owned),
    });
    group
        .engine
        .send(EngineCommand::UpdateParams(Box::new(params)));
    publish_once(group)
}

/// Ask for a wallpaper, wait for it, and take the one publish it made.
fn publish_once(group: &Plain) -> Publication {
    group.sink.publications().clear();
    group
        .publish()
        .expect("publishing to a recording sink cannot fail");
    let mut published = group.sink.publications();
    assert_eq!(published.len(), 1, "exactly one publish was asked for");
    published.pop().expect("the one publish")
}

/// Two screens side by side, the left one primary.
pub(crate) fn two_screens() -> Vec<Monitor> {
    vec![
        screen("A", 0, 320, 192, true),
        screen("B", 320, 320, 192, false),
    ]
}
