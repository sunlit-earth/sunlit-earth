use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crossbeam_channel::Receiver;
use crossbeam_channel::Sender;
use sunlit_core::display::Monitor;
use sunlit_core::engine::EngineCommand;
use sunlit_core::engine::EngineConfig;
use sunlit_core::engine::EngineEvent;
use sunlit_core::engine::EngineHandle;
use sunlit_core::engine::clock::MockClock;
use sunlit_core::params::SceneParams;

static GPU_SERIAL: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Proof that this thread is the one allowed to drive an engine.
///
/// Every case holds one for its whole body. The shared engines below are
/// reachable only through a reference to it, so nothing can render while
/// another case is rendering, and a case that starts an engine of its own is
/// serialized against the shared ones too.
pub(crate) struct Gpu(#[allow(dead_code)] MutexGuard<'static, ()>);

/// Take the GPU, recovering it even if a previous case panicked holding it.
pub(crate) fn gpu() -> Gpu {
    Gpu(GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

/// How long to wait for the engine to produce something before giving up.
pub(crate) const TIMEOUT: Duration = Duration::from_mins(1);

/// Deterministic test parameters: the procedural grid texture, no MSAA, a
/// fixed date so the sun does not move between runs.
///
/// The four overlays are switched off rather than left at their defaults,
/// because a shared engine has fixtures behind slots an individual case may
/// know nothing about. Off here is the same frame the case would have got from
/// an engine with an empty slot, and a case about one of them switches its own
/// back on.
pub(crate) fn test_params() -> SceneParams {
    let mut params = SceneParams {
        texture_index: 0,
        sample_count: 1,
        // Camera mode is off for every case here, the way the goldens pin it:
        // the spikes and the ghosts reach past the glare's own cone, so a
        // default-on flare would put pixels in frames that are counting what
        // the Sun itself paints.
        sun_flare: 0.0,
        moon_brightness: 0.0,
        milky_way_intensity: 0.0,
        cloud_opacity: 0.0,
        cloud_opacity_night: 0.0,
        ..SceneParams::default()
    };
    params.datetime.use_custom = true;
    params.datetime.custom_hour = 12.0;
    params.datetime.custom_day_of_year = 80;
    params.datetime.custom_year = 2026;
    params
}

pub(crate) struct Harness {
    pub(crate) engine: EngineHandle,
    pub(crate) events: Receiver<EngineEvent>,
    /// What `reset` puts the preview back to.
    preview_size: (u32, u32),
    /// The width the slots load at when no case has asked for another, and
    /// what the last case to ask left behind.
    texture_resolution: AtomicU32,
    default_resolution: u32,
}

impl Harness {
    pub(crate) fn start(configure: impl FnOnce(&mut EngineConfig)) -> Self {
        let (tx, events): (Sender<EngineEvent>, Receiver<EngineEvent>) =
            crossbeam_channel::unbounded();
        let mut config = EngineConfig::headless((512, 288));
        config.params = test_params();
        config.on_event = Arc::new(move |event| {
            let _ = tx.send(event);
        });
        configure(&mut config);
        let preview_size = config.preview_size;
        let default_resolution = config.texture_resolution;
        Self {
            engine: sunlit_core::engine::start(config)
                .expect("the harness needs a working adapter"),
            events,
            preview_size,
            texture_resolution: AtomicU32::new(default_resolution),
            default_resolution,
        }
    }

    /// Wait for the engine to finish a whole iteration, then throw away every
    /// event it has produced so far.
    ///
    /// A reply is sent while the loop is draining commands and the render comes
    /// after that drain, so two replies bracket one complete iteration:
    /// whatever the engine still owed when the first came back has been emitted
    /// by the time the second does, and the drain that follows takes all of it.
    pub(crate) fn settle(&self) {
        for _ in 0..2 {
            let _ = self.engine.memory_report();
        }
        while self.events.try_recv().is_ok() {}
    }

    /// Put a shared engine back the way its group's cases expect to find it.
    ///
    /// The scene is deliberately not part of this: every case sets its own
    /// before it looks at anything, and re-rendering a baseline nobody reads
    /// would cost a render per case. Neither is the texture width, because how
    /// to wait for a reload depends on what is behind the slots, which is the
    /// group's business rather than the harness's.
    pub(crate) fn reset(&self) {
        self.engine.send(EngineCommand::SetPreviewEnabled(true));
        self.engine.send(EngineCommand::SetPreviewSize(
            self.preview_size.0,
            self.preview_size.1,
        ));
        self.settle();
    }

    /// Put the slots back at the group's own width, and answer whether that
    /// was a change the caller has to wait out.
    pub(crate) fn restore_resolution(&self) -> bool {
        if self
            .texture_resolution
            .swap(self.default_resolution, Ordering::SeqCst)
            == self.default_resolution
        {
            return false;
        }
        self.engine
            .send(EngineCommand::SetTextureResolution(self.default_resolution));
        true
    }

    /// Reload the slots at `width`, remembering it so `reset` puts it back.
    pub(crate) fn set_texture_resolution(&self, width: u32) {
        self.texture_resolution.store(width, Ordering::SeqCst);
        self.engine.send(EngineCommand::SetTextureResolution(width));
    }

    /// Set the scene and hand back the picture it makes, rendered now.
    ///
    /// Synchronous, through the export path: the reply cannot arrive before the
    /// parameters have been applied, so there is no frame from the case before
    /// this one to mistake for this one's.
    pub(crate) fn picture(&self, params: &SceneParams, (width, height): (u32, u32)) -> Vec<u8> {
        self.engine
            .send(EngineCommand::UpdateParams(Box::new(*params)));
        self.export(width, height)
    }

    /// The picture at the current scene, rendered now.
    pub(crate) fn export(&self, width: u32, height: u32) -> Vec<u8> {
        self.engine
            .export_pixels(width, height)
            .expect("the engine should be able to export")
    }

    /// Apply the scene and wait until the engine has finished with it.
    pub(crate) fn settle_at(&self, params: &SceneParams) {
        self.engine
            .send(EngineCommand::UpdateParams(Box::new(*params)));
        self.settle();
    }

    /// Set the scene and block until the frame the change itself produces.
    ///
    /// For the cases that are about whether a change produces a frame at all,
    /// which means the caller has to have left the engine on another scene. A
    /// case that only wants to look at pixels asks for a `picture` instead,
    /// which renders synchronously and needs no change to have happened.
    pub(crate) fn frame_after_change(&self, params: &SceneParams) -> (Vec<u8>, u32, u32) {
        self.engine
            .send(EngineCommand::UpdateParams(Box::new(*params)));
        self.next_frame()
    }

    /// Block until the next preview frame, or panic on timeout.
    pub(crate) fn next_frame(&self) -> (Vec<u8>, u32, u32) {
        let deadline = std::time::Instant::now() + TIMEOUT;
        while let Ok(event) = self.events.recv_deadline(deadline) {
            if let EngineEvent::PreviewFrame {
                rgba,
                width,
                height,
            } = event
            {
                return (rgba, width, height);
            }
        }
        panic!("no preview frame within {TIMEOUT:?}");
    }

    /// Block until the textures the current mode needs are loaded, or panic on
    /// timeout. Preview frames arriving in the meantime are discarded.
    pub(crate) fn wait_for_textures(&self, what: &str) {
        let deadline = std::time::Instant::now() + TIMEOUT;
        while let Ok(event) = self.events.recv_deadline(deadline) {
            if matches!(event, EngineEvent::TexturesReady) {
                return;
            }
        }
        panic!("{what}: no TexturesReady within {TIMEOUT:?}");
    }

    /// Block until a status event whose text `matches`, or panic on timeout.
    ///
    /// The status is the loading indicator, so this is how a test observes that
    /// a background decode has started or finished without guessing at a sleep.
    pub(crate) fn wait_for_status(&self, matches: impl Fn(&str) -> bool, what: &str) {
        let deadline = std::time::Instant::now() + TIMEOUT;
        while let Ok(event) = self.events.recv_deadline(deadline) {
            if let EngineEvent::Status(text) = event
                && matches(&text)
            {
                return;
            }
        }
        panic!("{what}: no matching status within {TIMEOUT:?}");
    }

    /// Block until an overlay's texture has reached the GPU, by its GPU label.
    ///
    /// `TexturesReady` deliberately excludes the overlays, because nothing in
    /// the engine waits for one. So a test that wants the Moon or the Milky Way
    /// in a frame asks the memory report whether the renderer owns the texture
    /// yet, which is the only thing that answers it.
    pub(crate) fn wait_for_slot_texture(&self, label: &str) {
        let deadline = std::time::Instant::now() + TIMEOUT;
        while std::time::Instant::now() < deadline {
            let report = self
                .engine
                .memory_report()
                .expect("the engine should answer with a report");
            if report.expected.iter().any(|texture| texture.label == label) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the {label} did not arrive within {TIMEOUT:?}");
    }

    /// Drain events already queued and report whether any frame was among them.
    pub(crate) fn drained_frame(&self, settle: Duration) -> Option<(u32, u32)> {
        std::thread::sleep(settle);
        let mut found = None;
        while let Ok(event) = self.events.try_recv() {
            if let EngineEvent::PreviewFrame { width, height, .. } = event {
                found = Some((width, height));
            }
        }
        found
    }

    /// Ask for a wallpaper and block until the engine has finished trying.
    pub(crate) fn publish(&self) -> Result<String, String> {
        self.engine.send(EngineCommand::RenderWallpaperNow);
        self.wait_for_publish()
    }

    /// Block until the engine has finished a publish attempt.
    pub(crate) fn wait_for_publish(&self) -> Result<String, String> {
        let deadline = std::time::Instant::now() + TIMEOUT;
        while let Ok(event) = self.events.recv_deadline(deadline) {
            if let EngineEvent::WallpaperSet(result) = event {
                return result;
            }
        }
        panic!("no publish finished within {TIMEOUT:?}");
    }

    /// Send a display-change hint and wait until the engine has taken it.
    ///
    /// The round trip matters rather than the report: commands are handled in
    /// the order they were sent, so an answer to a later one is proof that the
    /// hint was read at the clock reading the case meant it to be read at.
    pub(crate) fn hint(&self) {
        self.engine.send(EngineCommand::DisplaysChanged);
        let _ = self.engine.memory_report();
    }

    /// Move the clock and ask the engine to look at its schedule now.
    pub(crate) fn advance(&self, clock: &MockClock, by: Duration) {
        clock.advance(by);
        self.engine.send(EngineCommand::Poke);
    }

    /// The next layout the engine announces, or `None` if it announces none.
    pub(crate) fn next_layout(&self, within: Duration) -> Option<Vec<Monitor>> {
        let deadline = std::time::Instant::now() + within;
        while let Ok(event) = self.events.recv_deadline(deadline) {
            if let EngineEvent::MonitorsChanged(monitors) = event {
                return Some(monitors);
            }
        }
        None
    }
}

/// A frame is "lit" when at least one pixel is clearly brighter than the
/// clear color (which is near-black).
pub(crate) fn has_lit_pixels(rgba: &[u8]) -> bool {
    rgba.chunks_exact(4)
        .any(|px| px[0] > 40 || px[1] > 40 || px[2] > 40)
}
