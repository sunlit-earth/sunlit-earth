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
use sunlit_core::params::SceneParams;

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
}

impl EngineLink {
    pub fn new(tx: Sender<EngineCommand>, aa_labels: Vec<String>, aa_counts: Vec<u32>) -> Self {
        Self {
            tx,
            aa_labels: Arc::new(aa_labels),
            aa_counts: Arc::new(aa_counts),
        }
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

    /// Push explicit parameters (used at startup, before any callback fires).
    pub fn push(&self, params: SceneParams) {
        self.send(EngineCommand::UpdateParams(Box::new(params)));
    }
}

/// Build the engine event callback that feeds a Slint window.
///
/// The returned closure runs on the engine thread; everything it does to the
/// window goes through `invoke_from_event_loop`.
pub fn event_forwarder(
    window: &MainWindow,
    on_textures_ready: impl Fn() + Send + Sync + 'static,
) -> Arc<dyn Fn(EngineEvent) + Send + Sync> {
    let mailbox = Arc::new(PreviewMailbox::default());
    let weak = window.as_weak();
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
                let parked = mailbox.slot.lock().expect("preview mailbox poisoned").take();
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
        EngineEvent::WallpaperSet(Ok(())) => info!("wallpaper updated"),
        EngineEvent::WallpaperSet(Err(e)) => tracing::error!("wallpaper update failed: {e}"),
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
