//! Starting the engine, and the handle a client holds afterwards.
//!
//! Everything on this side of the channel: the configuration a caller fills in,
//! the thread spawn that turns it into a running engine, and the handle whose
//! blocking round trips (export, memory report, render to file) are the only
//! way in that is not fire and forget.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Sender, bounded, unbounded};
use tracing::error;

use super::clock::{Clock, SystemClock};
use super::wallpaper_sink::{self, WallpaperSink};
use super::{Engine, EngineCommand, EngineEvent};
use crate::assets::cloud_source::CloudSource;
use crate::assets::cube_layout::CubeTextures;
use crate::assets::mailbox::TextureMailbox;
use crate::assets::tiles::{GEOMETRY, Geometry};
use crate::config::QualityTier;
use crate::memory_report::MemoryReport;
use crate::params::SceneParams;

/// Everything the engine needs to start.
pub struct EngineConfig {
    /// Force the CPU adapter.
    pub force_software: bool,
    /// One entry per file-backed texture slot, in slot order after the grid:
    /// the Moon, then the Milky Way.
    pub texture_paths: Vec<Option<PathBuf>>,
    /// Initial preview size; quantized by the engine.
    pub preview_size: (u32, u32),
    /// Whether to produce preview frames at all.
    pub preview_enabled: bool,
    /// Initial scene parameters.
    pub params: SceneParams,
    /// Caps the preview size and the MSAA sample count.
    pub quality: QualityTier,
    /// The resolution setting: the equirectangular width whose detail the
    /// finest tile level may reach (8192 the 2048 level, 4096 the 1024 level,
    /// 2048 the floors alone), the width the Milky Way is capped at, and the
    /// cloud image variant. Independent of the quality tier, which governs how
    /// much work a frame is allowed to be rather than how much texture memory
    /// the app holds.
    pub texture_resolution: u32,
    pub clock: Arc<dyn Clock>,
    /// `None` disables cloud fetching entirely (the `SUNLIT_EARTH_NO_CLOUDS`
    /// case, and the default for tests that do not care about clouds).
    pub cloud: Option<Arc<dyn CloudSource>>,
    pub cloud_poll_interval: Duration,
    /// The app's data directory, holding the cloud image cache and the tile
    /// packs. `None` disables both, and with them the cube surface.
    pub cache_dir: Option<PathBuf>,
    /// Unattended wallpaper refresh interval; `None` disables it.
    pub auto_refresh: Option<Duration>,
    pub wallpaper: Arc<dyn WallpaperSink>,
    /// How this session's monitors relate to each other.
    pub display_mode: crate::display::layout::DisplayMode,
    /// The monitor the plan is anchored to; `None` follows the system primary.
    pub anchor_monitor: Option<String>,
    /// Called on the engine thread for every event. Clients that need to be on
    /// another thread (the UI) forward from here.
    pub on_event: Arc<dyn Fn(EngineEvent) + Send + Sync>,
    /// Write periodic memory samples to the metrics CSV.
    pub record_metrics: bool,
    /// The mailbox decoded textures are parked in, with one slot per flat
    /// texture (`texture_paths.len() + 2`). `None` builds one.
    ///
    /// Injectable for the same reason the clock and the cloud source are: a
    /// caller holding the same mailbox the engine drains can produce an arrival
    /// order that no amount of waiting makes reliable.
    pub mailbox: Option<TextureMailbox>,
    /// The cube faces the textures directory holds. With every one of them
    /// there and a cache directory to build the tile packs in, the globe is
    /// drawn from the cube surface; otherwise it is the grid.
    pub cube_textures: CubeTextures,
    /// The sizes the tile packs are cut to: `tiles::GEOMETRY` for the shipped
    /// faces, `tiles::FIXTURE` for the test bake's.
    pub tile_geometry: Geometry,
    /// Layers of the tile array. `None` takes the budget for the adapter
    /// (`renderer::tiles::TILE_LAYER_BUDGET`, or `CPU_TILE_LAYER_BUDGET` on a
    /// CPU adapter), as the app does; a test sets fewer to make the loader
    /// evict, or none to draw the floors alone.
    pub tile_layers: Option<u32>,
    /// Holds the tile loader's reads while shut; open, and never shut, in the
    /// app.
    pub tile_gate: super::TileGate,
    /// Holds the transcoder at the start of each pack it builds; open, and
    /// never held, in the app.
    pub build_gate: crate::assets::tiles::BuildGate,
    /// Whether every month's day floor stays resident. `None` takes the
    /// adapter's way, as the app does: every month on a GPU, and on a CPU
    /// adapter the month in force's and the month ahead's alone, the others
    /// made resident when their month comes into force. The tests, which run
    /// on a software adapter, set it to cover a GPU's way.
    pub every_floor: Option<bool>,
}

impl EngineConfig {
    /// A minimal headless configuration: software adapter, system clock, no
    /// clouds, no auto-refresh, events dropped on the floor.
    ///
    /// The software adapter is the default here so the test suite behaves the
    /// same on a developer machine with a discrete GPU as it does on CI, where
    /// WARP is all there is.
    pub fn headless(preview_size: (u32, u32)) -> Self {
        Self {
            force_software: true,
            // Two file-backed slots (the Moon, the Milky Way) so the slot
            // layout matches production even when no texture files are present.
            texture_paths: vec![None, None],
            preview_size,
            preview_enabled: true,
            params: SceneParams::default(),
            // Tests always run at the cheap tier, whatever the build profile.
            quality: QualityTier::Low,
            texture_resolution: crate::config::DEFAULT_TEXTURE_RESOLUTION,
            clock: Arc::new(SystemClock::new()),
            cloud: None,
            cloud_poll_interval: Duration::from_hours(1),
            cache_dir: None,
            auto_refresh: None,
            wallpaper: Arc::new(wallpaper_sink::SystemWallpaper),
            display_mode: crate::display::layout::DisplayMode::default(),
            anchor_monitor: None,
            on_event: Arc::new(|_| {}),
            record_metrics: false,
            mailbox: None,
            cube_textures: CubeTextures::default(),
            tile_geometry: GEOMETRY,
            tile_layers: None,
            tile_gate: super::TileGate::default(),
            build_gate: crate::assets::tiles::BuildGate::default(),
            every_floor: None,
        }
    }

    /// Whether the engine will draw the globe from the cube surface.
    pub fn takes_cube(&self) -> bool {
        self.cube_textures.is_complete() && self.cache_dir.is_some()
    }
}

/// Client-side handle to a running engine.
pub struct EngineHandle {
    tx: Sender<EngineCommand>,
    thread: Option<JoinHandle<()>>,
    adapter: AdapterReport,
}

/// What the engine thread reports back about the GPU it opened.
///
/// Sent once, before the loop starts, so `start` can return a handle that
/// already knows what it is running on.
pub(super) struct AdapterReport {
    pub(super) info: String,
    pub(super) key: String,
    pub(super) supported_sample_counts: Vec<u32>,
}

impl EngineHandle {
    /// Send a command.
    ///
    /// A dead channel means the engine thread is gone, which during normal
    /// operation only happens after `Shutdown`. Anywhere else it means the
    /// thread died, and the app would otherwise sit there with a window that
    /// never updates and no explanation, so this is logged at error level.
    pub fn send(&self, cmd: EngineCommand) {
        if self.tx.send(cmd).is_err() {
            error!("engine command dropped: the engine thread is no longer running");
        }
    }

    /// Sender for callers that need to enqueue from another thread.
    pub fn sender(&self) -> Sender<EngineCommand> {
        self.tx.clone()
    }

    /// Description of the selected GPU adapter, for the UI's renderer label.
    pub fn adapter_info(&self) -> &str {
        &self.adapter.info
    }

    /// Slug naming the adapter's implementation, for per-adapter golden
    /// references. See `wgpu_init::adapter_key`.
    pub fn adapter_key(&self) -> &str {
        &self.adapter.key
    }

    /// MSAA sample counts the adapter supports for the render format.
    pub fn supported_sample_counts(&self) -> &[u32] {
        &self.adapter.supported_sample_counts
    }

    /// Render `width` x `height` pixels and block until they are ready.
    pub fn export_pixels(&self, width: u32, height: u32) -> Result<Vec<u8>, String> {
        let (reply, replies) = bounded(1);
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

    /// Ask the engine thread for a memory report and block until it answers.
    pub fn memory_report(&self) -> Result<Box<MemoryReport>, String> {
        self.report_memory(false)
    }

    /// Ask the engine thread for a memory report taken once the GPU has
    /// finished everything submitted, and block until it answers.
    ///
    /// wgpu's counters then count what the engine keeps: the staging buffers
    /// of the writes since the last submit, and those of a submission no poll
    /// has found finished yet, are gone by then. [`Self::memory_report`]
    /// counts them, which is what a check that an upload was submitted needs.
    pub fn settled_memory_report(&self) -> Result<Box<MemoryReport>, String> {
        self.report_memory(true)
    }

    fn report_memory(&self, settled: bool) -> Result<Box<MemoryReport>, String> {
        let (reply, replies) = bounded(1);
        self.tx
            .send(EngineCommand::ReportMemory { reply, settled })
            .map_err(|_| "engine has stopped".to_owned())?;
        replies
            .recv()
            .map_err(|_| "engine stopped before answering".to_owned())
    }

    /// Ask the engine thread what its tile loader holds and wants, and block
    /// until it answers; `None` where the globe is not drawn from the cube
    /// surface.
    pub fn tile_report(&self) -> Result<Option<Box<super::TileReport>>, String> {
        let (reply, replies) = bounded(1);
        self.tx
            .send(EngineCommand::ReportTiles { reply })
            .map_err(|_| "engine has stopped".to_owned())?;
        replies
            .recv()
            .map_err(|_| "engine stopped before answering".to_owned())
    }

    /// Render a PNG at `width` x `height` and block until it is written.
    pub fn render_to_file(&self, path: PathBuf, width: u32, height: u32) -> Result<(), String> {
        let (reply, replies) = bounded(1);
        self.tx
            .send(EngineCommand::RenderToFile {
                path,
                width,
                height,
                reply,
            })
            .map_err(|_| "engine has stopped".to_owned())?;
        replies
            .recv()
            .map_err(|_| "engine stopped before answering".to_owned())?
    }

    /// Stop the engine and wait for its thread.
    pub fn shutdown(mut self) {
        let _ = self.tx.send(EngineCommand::Shutdown);
        join_engine(self.thread.take());
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(EngineCommand::Shutdown);
        join_engine(self.thread.take());
    }
}

/// Join the engine thread, reporting a panic rather than swallowing it.
///
/// A panicking engine thread is the one failure that leaves the app looking
/// alive (the window is up, IPC answers) while nothing renders, so it must not
/// be silent.
fn join_engine(thread: Option<JoinHandle<()>>) {
    let Some(thread) = thread else {
        return;
    };
    if let Err(payload) = thread.join() {
        let reason = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic payload".to_owned());
        error!(reason, "the engine thread panicked");
    }
}

/// Start the engine on its own thread.
///
/// Blocks until the GPU device exists, so the returned handle can report the
/// adapter and the supported sample counts without another round trip. That is
/// also the step most likely to fail on a machine with no working graphics
/// driver, and it fails as an error the caller can put in front of the user.
pub fn start(config: EngineConfig) -> Result<EngineHandle, String> {
    // Unbounded, deliberately, and this is the reasoning the resource-flow rule
    // asks for at the declaration site:
    //
    // - The consumer is unconditional. The engine loop drains this channel dry
    //   on every iteration and iterations are at most `TICK` (50 ms) apart. It
    //   is not gated on a window, a client, or visibility.
    // - The producers are human-rate: UI callbacks during a drag (about 60/s),
    //   the viewport poll (5/s), the cloud worker (a few per hour), IPC (rare).
    // - The payloads are small and carry no pixel data. `EngineCommand` is a
    //   few dozen bytes (`command_payload_is_small` pins this), because
    //   `UpdateParams` boxes its `SceneParams` and the frame-carrying direction
    //   is the other way, through the event callback.
    // - The worst case is therefore a backlog for as long as one blocking
    //   operation takes: a 4K wallpaper export or an 8K mip upload, one to two
    //   seconds, so a couple of hundred entries and single-digit kilobytes.
    //
    // A bound was considered and rejected: a blocking `send` from the UI thread
    // would deadlock against an engine that is mid-export, and a non-blocking
    // `try_send` that drops `UpdateParams` can drop the *last* one, leaving the
    // window and the engine permanently disagreeing. Neither failure is better
    // than the bounded growth above.
    let (tx, rx) = unbounded();
    let (ready_tx, ready_rx) = bounded(1);

    let command_tx = tx.clone();
    let thread = std::thread::Builder::new()
        .name("sunlit-engine".into())
        .spawn(move || {
            // `Engine::new` reports its own failure through `ready`, so there is
            // nothing left to say here; the loop simply never runs.
            if let Ok(mut engine) = Engine::new(config, rx, &command_tx, &ready_tx) {
                drop(ready_tx);
                engine.run();
            }
        })
        .map_err(|e| format!("the engine thread could not be started: {e}"))?;

    match ready_rx.recv() {
        Ok(Ok(adapter)) => Ok(EngineHandle {
            tx,
            thread: Some(thread),
            adapter,
        }),
        Ok(Err(reason)) => {
            join_engine(Some(thread));
            Err(reason)
        }
        Err(_) => {
            join_engine(Some(thread));
            Err("the engine thread stopped before it could report its adapter".to_owned())
        }
    }
}
