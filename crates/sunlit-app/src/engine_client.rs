//! The Slint shell's side of the engine boundary.
//!
//! The engine runs on its own thread and hands out preview frames as pixel
//! buffers. This module parks the newest frame in a mailbox, wakes the event
//! loop once per arrival, and turns whatever is parked into a `slint::Image`.
//! Frames are replaced rather than queued: if the UI thread falls behind, only
//! the newest frame is worth showing, and an unbounded queue of 2 MB buffers is
//! exactly the failure mode Phase 0 removed from the texture path.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;
use slint::ComponentHandle;
use tracing::info;

use sunlit_core::engine::{EngineCommand, EngineEvent};
use sunlit_core::memory_report::MemoryReport;

use crate::MainWindow;

/// A rendered preview frame on its way to the UI thread.
struct Frame {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

/// Latest-value mailbox for preview frames, plus the flag that keeps at most
/// one wake-up in flight on the Slint event loop.
#[derive(Default)]
struct PreviewMailbox {
    slot: Mutex<Option<Frame>>,
    wake_pending: AtomicBool,
}

/// Everything the UI needs to talk to the engine.
#[derive(Clone)]
pub struct EngineLink {
    tx: Sender<EngineCommand>,
    aa_labels: Arc<Vec<String>>,
    aa_counts: Arc<Vec<u32>>,
    /// Whether the texture resolution the window shows came from
    /// `--texture-resolution` rather than from the user or the stored config.
    /// Shared by every callback that holds a clone of this link.
    resolution_from_cli: Arc<AtomicBool>,
}

impl EngineLink {
    pub fn new(tx: Sender<EngineCommand>, aa_labels: Vec<String>, aa_counts: Vec<u32>) -> Self {
        Self {
            tx,
            aa_labels: Arc::new(aa_labels),
            aa_counts: Arc::new(aa_counts),
            resolution_from_cli: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Whether a save should leave the stored texture resolution alone.
    ///
    /// A `--texture-resolution` override has to reach the window, or the window
    /// claims a width the engine is not using. It must not reach the config
    /// file, because the flag is for one run. So while the window's width is
    /// the flag's, a save keeps what is on disk; the moment the width becomes
    /// the user's, through the combo box, a reset, or load-defaults, this
    /// clears and saves behave normally again.
    pub fn resolution_is_one_run_only(&self) -> bool {
        self.resolution_from_cli.load(Ordering::Relaxed)
    }

    pub fn set_resolution_is_one_run_only(&self, value: bool) {
        self.resolution_from_cli.store(value, Ordering::Relaxed);
    }

    pub fn aa_labels(&self) -> &[String] {
        &self.aa_labels
    }

    pub fn aa_counts(&self) -> &[u32] {
        &self.aa_counts
    }

    /// Send a command, ignoring the case where the engine has already stopped.
    pub fn send(&self, cmd: EngineCommand) {
        let _ = self.tx.send(cmd);
    }

    pub fn sender(&self) -> Sender<EngineCommand> {
        self.tx.clone()
    }

    /// Tell the engine whether anyone is looking at the preview.
    ///
    /// A hidden window costs a full readback plus a `SharedPixelBuffer` copy
    /// on every sun tick otherwise, which at the High tier is a 4K frame every
    /// two minutes for nothing. The engine keeps rendering either way, because
    /// the wallpaper export depends on it; only the delivery stops.
    pub fn set_preview_enabled(&self, enabled: bool) {
        self.send(EngineCommand::SetPreviewEnabled(enabled));
    }

    /// Render `width` x `height` pixels on the engine thread and wait for them.
    pub fn export_pixels(&self, width: u32, height: u32) -> Result<Vec<u8>, String> {
        let (reply, replies) = crossbeam_channel::bounded(1);
        self.tx
            .send(EngineCommand::ExportPixels {
                width,
                height,
                reply,
            })
            .map_err(|_| "engine has stopped".to_owned())?;
        replies
            .recv()
            .map_err(|_| "engine stopped before answering".to_owned())?
    }

    /// Ask the engine thread for a memory report and wait for it.
    pub fn memory_report(&self) -> Result<Box<MemoryReport>, String> {
        let (reply, replies) = crossbeam_channel::bounded(1);
        self.tx
            .send(EngineCommand::ReportMemory { reply })
            .map_err(|_| "engine has stopped".to_owned())?;
        replies
            .recv()
            .map_err(|_| "engine stopped before answering".to_owned())
    }

    /// Read the window into `SceneParams` and push it to the engine.
    ///
    /// This is the only path by which UI changes reach the renderer, so every
    /// change callback ends here instead of poking a redraw.
    pub fn push_params(&self, window: &MainWindow) {
        let params = crate::ui_callbacks::read_params_from_window(window, &self.aa_counts);
        window.set_zoom_display_distance(sunlit_core::scene::camera::zoom_to_distance(
            params.camera.zoom,
        ));
        self.send(EngineCommand::UpdateParams(Box::new(params)));
    }
}

/// Build the engine event callback that feeds a Slint window.
///
/// The returned closure runs on the engine thread; everything it does to the
/// window goes through `invoke_from_event_loop`. `screens` is the window's own
/// monitor list, which a layout change replaces.
pub fn event_forwarder(
    window: &MainWindow,
    screens: &crate::displays::SharedMonitors,
    on_textures_ready: impl Fn() + Send + Sync + 'static,
) -> Arc<dyn Fn(EngineEvent) + Send + Sync> {
    let mailbox = Arc::new(PreviewMailbox::default());
    let weak = window.as_weak();
    let screens = Arc::clone(screens);
    let first_frame = AtomicBool::new(true);

    Arc::new(move |event| match event {
        EngineEvent::PreviewFrame {
            rgba,
            width,
            height,
        } => {
            *mailbox.slot.lock().expect("preview mailbox poisoned") = Some(Frame {
                rgba,
                width,
                height,
            });
            // One wake-up per burst: the closure below always takes whatever is
            // parked, so queueing a second one would only duplicate work.
            if mailbox.wake_pending.swap(true, Ordering::AcqRel) {
                return;
            }
            let mailbox = Arc::clone(&mailbox);
            let weak = weak.clone();
            let announce = first_frame.swap(false, Ordering::Relaxed);
            let _ = slint::invoke_from_event_loop(move || {
                mailbox.wake_pending.store(false, Ordering::Release);
                let parked = mailbox
                    .slot
                    .lock()
                    .expect("preview mailbox poisoned")
                    .take();
                if let (Some(frame), Some(win)) = (parked, weak.upgrade()) {
                    win.set_rendered_image(to_image(&frame));
                    if announce {
                        println!("SIGNAL:first_frame_rendered");
                    }
                }
            });
        }
        EngineEvent::Status(text) => {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = weak.upgrade() {
                    win.set_loading_text(text.into());
                }
            });
        }
        EngineEvent::TexturesReady => on_textures_ready(),
        EngineEvent::MonitorsChanged(monitors) => {
            // The count first, so the suite can wait for the reaction instead
            // of sleeping and hoping. In the voice of `wallpaper_set`.
            crate::ipc::signal(&format!("displays_changed monitors={}", monitors.len()));
            let weak = weak.clone();
            let screens = Arc::clone(&screens);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = weak.upgrade() {
                    // The stored anchor rather than the row the combo is on: a
                    // screen that was unplugged shows as automatic, and this is
                    // what puts its own row back when it returns.
                    let stored = sunlit_core::config::load_config().anchor_monitor;
                    crate::displays::replace_monitors(&win, &screens, monitors, &stored);
                }
            });
        }
        EngineEvent::WallpaperSet(Ok(note)) => {
            info!("wallpaper updated");
            // What the desktop could not do is not a failure and does not go to
            // the error path, but it is the one thing the person looking at
            // three identical screens wants to read.
            if !note.is_empty() {
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(win) = weak.upgrade() {
                        win.set_loading_text(note.into());
                    }
                });
            }
            crate::ipc::signal("wallpaper_set");
        }
        EngineEvent::WallpaperSet(Err(e)) => {
            tracing::error!("wallpaper update failed: {e}");
            crate::ipc::signal("wallpaper_failed");
        }
    })
}

/// Copy a frame into a Slint image.
fn to_image(frame: &Frame) -> slint::Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &frame.rgba,
        frame.width,
        frame.height,
    );
    slint::Image::from_rgba8(buffer)
}
