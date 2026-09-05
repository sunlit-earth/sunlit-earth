//! The engine: one thread that owns the GPU device, the renderer, the asset
//! mailbox, and the schedule.
//!
//! Clients (the Slint shell, the `render` subcommand, tests) send commands and
//! receive events. Nothing the engine does depends on a window existing, which
//! is the whole point: hiding the settings window removes a client, it does not
//! half-suspend the machinery.

pub mod clock;
pub mod wallpaper_sink;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded, unbounded};
use tracing::{debug, error, info, warn};

use crate::assets::cloud_fetcher::{CloudUpdater, PollOutcome, RetryBackoff};
use crate::assets::cloud_source::CloudSource;
use crate::assets::mailbox::TextureMailbox;
use crate::config::QualityTier;
use crate::memory_report::MemoryReport;
use crate::params::SceneParams;
use crate::renderer::{
    RenderOutcome, Renderer, RendererConfig, SlotLayout, quantize_to_granularity,
    resolve_sample_count,
};
use crate::scene::sky::{self, SkyState};

use clock::{Clock, SystemClock};
use wallpaper_sink::{Frame, JobImages, WallpaperJob, WallpaperSink};

/// How long the loop blocks on the command channel before re-checking the
/// schedule. Short enough that a real-clock deadline is never missed by more
/// than this, cheap enough to ignore: the wake does nothing when nothing is due.
const TICK: Duration = Duration::from_millis(50);

/// How often decoded textures are uploaded to the GPU. Unconditional: this is
/// what keeps cloud updates flowing while no window is visible.
const DRAIN_INTERVAL: Duration = Duration::from_secs(5);

/// How often the sky state (the sun, the sky rotation, the planets) is
/// recomputed when rendering live time.
const SKY_INTERVAL: Duration = Duration::from_mins(2);

/// How often a memory sample is appended to the metrics CSV.
const METRICS_INTERVAL: Duration = Duration::from_mins(10);

/// How long a burst of display-change hints is allowed to settle before the
/// monitors are asked for.
///
/// A layout change is a burst on both platforms: `RandR` sends one event per CRTC
/// and one per output, and Windows sends one `WM_DISPLAYCHANGE` per applied
/// step, with a docking station bringing its screens up one at a time. Waiting
/// costs nothing anybody can see, and settling too early costs a second full
/// render and a second visible swap when the late step lands. It is read off
/// the injected clock and checked on the tick the loop already makes, so it
/// adds no timer and no wakeup.
const DISPLAY_SETTLE: Duration = Duration::from_secs(2);

/// Things a client asks the engine to do.
pub enum EngineCommand {
    /// Replace the scene parameters. Coalesced: only the newest survives a
    /// backlog, because intermediate drag positions are not worth rendering.
    UpdateParams(Box<SceneParams>),
    /// Resize the preview target. Quantized by the engine.
    SetPreviewSize(u32, u32),
    /// Turn preview frames on or off. Off saves the readback when nothing is
    /// looking (the window is hidden, or the client is the render subcommand).
    SetPreviewEnabled(bool),
    /// Render at the sink's native resolution and publish it now.
    RenderWallpaperNow,
    /// Render to a PNG file at an explicit size and report back.
    RenderToFile {
        path: PathBuf,
        width: u32,
        height: u32,
        reply: Sender<Result<(), String>>,
    },
    /// Render at an explicit size and hand the raw pixels back. Used by the
    /// e2e `export-test` probe, which checks that the GPU path still works
    /// while the window is hidden.
    ExportPixels {
        width: u32,
        height: u32,
        reply: Sender<Result<Vec<u8>, String>>,
    },
    /// Reload the file-backed textures at a different width.
    ///
    /// Not part of `UpdateParams`: the width does not describe what to draw, it
    /// decides which pixels to load, and acting on it means freeing GPU
    /// textures and re-reading files, which a parameter push cannot express.
    SetTextureResolution(u32),
    /// Assemble a memory report and hand it back.
    ///
    /// Answered on the engine thread because the device is owned there, in the
    /// same reply-channel shape as `ExportPixels`.
    ReportMemory { reply: Sender<Box<MemoryReport>> },
    /// Turn the unattended wallpaper refresh on or off.
    SetAutoRefresh { enabled: bool, interval: Duration },
    /// Replace the mode and the anchor a wallpaper is planned with.
    ///
    /// Not part of `UpdateParams` for the same reason `SetTextureResolution` is
    /// not: neither field describes what to draw, so neither belongs in
    /// `SceneParams` or in the digest that decides whether a frame is worth
    /// rendering. Both are read only when a wallpaper is published.
    SetDisplayPlan {
        mode: crate::display::layout::DisplayMode,
        anchor: Option<String>,
    },
    /// The display layout may have moved. A hint and nothing more: it carries
    /// no list, because the watcher that sends it has no opinion about what
    /// changed and the engine asks the sink itself once the burst has settled.
    DisplaysChanged,
    /// Re-evaluate the schedule now. Tests send this after advancing a mock
    /// clock; production uses it as a "something happened" nudge.
    Poke,
    /// Finish the current iteration and stop.
    Shutdown,
}

/// Things the engine tells its clients about.
pub enum EngineEvent {
    /// A freshly rendered preview frame, as tightly packed RGBA8.
    PreviewFrame {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
    },
    /// Every texture the current mode needs has finished loading. Fires once
    /// per set of textures, so again after a resolution change has reloaded
    /// them.
    TexturesReady,
    /// A wallpaper publish attempt finished.
    ///
    /// `Ok` carries what the desktop could not do, and is empty where it did
    /// exactly what the mode asked. A desktop with one wallpaper for every
    /// screen has not failed by giving them all the same image, but somebody
    /// looking at three identical screens deserves the sentence that says why.
    WallpaperSet(Result<String, String>),
    /// The monitors this session has, after a hint turned out to be a real
    /// change. Carries the list the engine will plan its next wallpaper with,
    /// so a client showing the layout does not have to query for itself.
    MonitorsChanged(Vec<crate::display::Monitor>),
    /// Loading-indicator text; empty when nothing is loading.
    Status(String),
}

/// Everything the engine needs to start.
pub struct EngineConfig {
    /// Force the CPU adapter.
    pub force_software: bool,
    /// One entry per file-backed texture slot, in slot order after the grid.
    pub texture_paths: Vec<Option<PathBuf>>,
    /// Initial preview size; quantized by the engine.
    pub preview_size: (u32, u32),
    /// Whether to produce preview frames at all.
    pub preview_enabled: bool,
    /// Initial scene parameters.
    pub params: SceneParams,
    /// Caps the preview size and the MSAA sample count.
    pub quality: QualityTier,
    /// Width the file-backed surface textures are loaded at, and the cloud
    /// image variant that goes with it. Independent of the quality tier, which
    /// governs how much work a frame is allowed to be rather than how much
    /// texture memory the app holds.
    pub texture_resolution: u32,
    pub clock: Arc<dyn Clock>,
    /// `None` disables cloud fetching entirely (the `SUNLIT_EARTH_NO_CLOUDS`
    /// case, and the default for tests that do not care about clouds).
    pub cloud: Option<Arc<dyn CloudSource>>,
    pub cloud_poll_interval: Duration,
    /// The app's data directory, holding both the cloud image cache and the
    /// downscaled copies of the surface textures. `None` disables both caches.
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
    /// The mailbox decoded textures are parked in, with one slot per texture
    /// (`texture_paths.len() + 2`). `None` builds one.
    ///
    /// Injectable for the same reason the clock and the cloud source are. A
    /// caller holding the same mailbox the engine drains can produce an arrival
    /// order that otherwise needs a decode still running when the resolution
    /// changes, which is the ordering the generation stamp exists for and the
    /// one no amount of waiting makes reliable.
    pub mailbox: Option<TextureMailbox>,
}

impl EngineConfig {
    /// A minimal headless configuration: software adapter, system clock, no
    /// clouds, no auto-refresh, events dropped on the floor.
    ///
    /// The software adapter is the default here so the test suite behaves the
    /// same on a developer machine with a discrete GPU as it does on CI, where
    /// WARP is all there is. A test that passes only on one of the two is worse
    /// than no test.
    pub fn headless(preview_size: (u32, u32)) -> Self {
        Self {
            force_software: true,
            // Four file-backed slots (day, night, moon, Milky Way) so the slot
            // layout matches production even when no texture files are present.
            texture_paths: vec![None, None, None, None],
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
        }
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
struct AdapterReport {
    info: String,
    key: String,
    supported_sample_counts: Vec<u32>,
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
        let (reply, replies) = bounded(1);
        self.tx
            .send(EngineCommand::ReportMemory { reply })
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
    //   is not gated on a window, a client, or visibility, which is exactly the
    //   property the Phase 0 leak lacked.
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

/// Deadline bookkeeping for one periodic job.
struct Schedule {
    interval: Duration,
    next: Duration,
}

impl Schedule {
    fn new(interval: Duration, now: Duration) -> Self {
        Self {
            interval,
            next: now + interval,
        }
    }

    /// Whether the job is due, rolling the deadline forward if so.
    ///
    /// The deadline is recomputed from `now` rather than accumulated, so a long
    /// stall (a sleeping laptop, a mock clock jumping a day) produces one run
    /// rather than a burst of catch-up runs.
    fn due(&mut self, now: Duration) -> bool {
        if now >= self.next {
            self.next = now + self.interval;
            true
        } else {
            false
        }
    }

    fn set_interval(&mut self, interval: Duration, now: Duration) {
        self.interval = interval;
        self.next = now + interval;
    }
}

/// The engine's own state, private to its thread.
///
/// The bools are independent latches on a private struct rather than
/// parameters anyone passes, which is the confusion the lint is about.
#[allow(clippy::struct_excessive_bools)]
struct Engine {
    rx: Receiver<EngineCommand>,
    clock: Arc<dyn Clock>,
    on_event: Arc<dyn Fn(EngineEvent) + Send + Sync>,
    wallpaper: Arc<dyn WallpaperSink>,
    renderer: Renderer,
    params: SceneParams,
    quality: QualityTier,
    /// The adapter slug the memory report names itself after.
    adapter_key: String,
    /// MSAA sample counts the adapter supports for the render format. Every
    /// requested count is resolved against this before it can reach wgpu.
    supported_sample_counts: Vec<u32>,
    /// The last count a client asked for, so the clamp is logged when the
    /// request changes rather than on every slider tick.
    requested_sample_count: u32,
    preview: PreviewState,
    /// A wallpaper update was asked for while a texture was still on its way,
    /// and happens as soon as it arrives.
    wallpaper_owed: bool,
    /// How this session's monitors relate to each other, and which one the plan
    /// is built around. Read only when a wallpaper is published.
    display_mode: crate::display::layout::DisplayMode,
    anchor_monitor: Option<String>,
    /// When the monitors are next worth asking for, set by a display-change
    /// hint and pushed out again by every hint that follows it.
    display_recheck: Option<Duration>,
    /// The last monitor list the engine saw, from a publish or from a recheck.
    /// `None` until one of the two has happened, which is what makes the first
    /// hint an announcement rather than a comparison against nothing.
    known_monitors: Option<Vec<crate::display::Monitor>>,
    /// Whether the desk is holding a picture this process made. Set when a sink
    /// accepted one and never on a refusal, so a layout change cannot produce a
    /// wallpaper nobody asked for or an error out of nowhere.
    published: bool,
    /// Set when something happened that the next render must pick up.
    dirty: bool,
    textures_ready: bool,
    /// Whether the one debug-level memory dump has already been written. It
    /// goes out the first time the textures are ready, which is the first
    /// moment the numbers describe a loaded scene rather than a half-built one.
    memory_dumped: bool,
    last_status: String,
    drain: Schedule,
    sky: Schedule,
    metrics: Option<Schedule>,
    auto_refresh: Option<Schedule>,
    cloud: Option<CloudWorker>,
}

/// Whether preview frames are wanted, and whether one is owed right now.
///
/// Re-enabling the preview has to deliver a frame even though the scene did
/// not change, otherwise a window that was hidden and shown again would sit on
/// a stale image until the user touched something.
struct PreviewState {
    enabled: bool,
    owed: bool,
}

/// The cloud fetch thread and the flag that keeps requests from piling up.
struct CloudWorker {
    tx: Sender<()>,
    busy: Arc<AtomicBool>,
    /// The texture resolution the worker should be fetching the variant of.
    ///
    /// Read by the worker before every poll rather than sent as a message, so a
    /// switch that lands while a fetch is in flight (or while a failed poll is
    /// backing off) takes effect on the next attempt instead of queueing behind
    /// one that may be minutes from finishing.
    target: Arc<AtomicU32>,
    schedule: Schedule,
    /// A poll came due but the worker was busy, so it still owes one. Kept
    /// separate from the schedule so a busy worker does not drag the deadline
    /// backwards and forwards on every tick.
    owed: bool,
    _thread: JoinHandle<()>,
}

impl Engine {
    #[allow(clippy::too_many_lines)]
    fn new(
        config: EngineConfig,
        rx: Receiver<EngineCommand>,
        command_tx: &Sender<EngineCommand>,
        ready: &Sender<Result<AdapterReport, String>>,
    ) -> Result<Self, String> {
        let EngineConfig {
            force_software,
            texture_paths,
            preview_size,
            preview_enabled,
            mut params,
            quality,
            texture_resolution,
            clock,
            cloud,
            cloud_poll_interval,
            cache_dir,
            auto_refresh,
            wallpaper,
            display_mode,
            anchor_monitor,
            on_event,
            record_metrics,
            mailbox,
        } = config;

        // One slot per texture: the grid, one per path, and the cloud overlay.
        //
        // Checked rather than trusted, and before the device exists, because
        // both ways of getting it wrong are bad and neither is visible where it
        // happens. A mailbox with too few slots drops the posts for the high
        // ones, leaving those slots waiting for a load that was thrown away; one
        // with too many hands the consumer a slot index its own array does not
        // have, which is a panic in the middle of a session. Both were
        // impossible by construction until the mailbox could be injected.
        // Failing here means failing before `ready` is sent, so `start` reports
        // a thread that never got as far as its adapter rather than handing back
        // a handle to a thread that quietly died.
        let slots = SlotLayout::new(texture_paths.len());
        if let Some(mailbox) = &mailbox {
            assert_eq!(
                mailbox.slot_count(),
                slots.count(),
                "the texture mailbox must have one slot per texture: the grid, \
                 {} file-backed, and the cloud overlay",
                texture_paths.len()
            );
        }

        let gpu = match crate::wgpu_init::init(force_software) {
            Ok(gpu) => gpu,
            Err(reason) => {
                let _ = ready.send(Err(reason.clone()));
                return Err(reason);
            }
        };
        let _ = ready.send(Ok(AdapterReport {
            info: gpu.adapter_info.clone(),
            key: gpu.adapter_key.clone(),
            supported_sample_counts: gpu.supported_sample_counts.clone(),
        }));
        crate::memory::log_memory_usage("engine: after wgpu init");

        let mailbox = mailbox.unwrap_or_else(|| TextureMailbox::new(slots.count()));

        // Every background producer wakes the engine loop through the same
        // command channel, so there is exactly one place that decides what to
        // do about new work.
        let poke_tx = command_tx.clone();
        let notify: crate::assets::cloud_fetcher::NotifyFn = Arc::new(move || {
            let _ = poke_tx.send(EngineCommand::Poke);
        });

        let requested_sample_count = params.sample_count;
        params.sample_count = resolve_sample_count(
            requested_sample_count,
            &gpu.supported_sample_counts,
            quality.max_sample_count(),
        );
        if params.sample_count != requested_sample_count {
            warn!(
                requested = requested_sample_count,
                using = params.sample_count,
                supported = ?gpu.supported_sample_counts,
                "MSAA sample count is not available, falling back"
            );
        }
        info!(texture_resolution, "surface texture resolution");
        let (width, height) = preview_target_size(preview_size, quality);
        let renderer = Renderer::new(
            gpu.device,
            gpu.queue,
            RendererConfig {
                sample_count: params.sample_count,
                width,
                height,
                texture_paths,
                texture_resolution,
                texture_cache_dir: cache_dir.clone(),
                mailbox: mailbox.clone(),
                notify: Arc::clone(&notify),
            },
        );

        let now = clock.elapsed();
        let cloud = cloud.map(|source| {
            spawn_cloud_worker(
                source,
                mailbox,
                notify,
                cache_dir,
                cloud_poll_interval,
                now,
                texture_resolution,
                slots.clouds(),
            )
        });

        Ok(Self {
            rx,
            clock,
            on_event,
            wallpaper,
            renderer,
            params,
            quality,
            adapter_key: gpu.adapter_key,
            supported_sample_counts: gpu.supported_sample_counts,
            requested_sample_count,
            preview: PreviewState {
                enabled: preview_enabled,
                owed: false,
            },
            wallpaper_owed: false,
            display_mode,
            anchor_monitor,
            display_recheck: None,
            known_monitors: None,
            published: false,
            dirty: true,
            textures_ready: false,
            memory_dumped: false,
            last_status: String::new(),
            drain: Schedule::new(DRAIN_INTERVAL, now),
            sky: Schedule::new(SKY_INTERVAL, now),
            metrics: record_metrics.then(|| Schedule::new(METRICS_INTERVAL, now)),
            auto_refresh: auto_refresh.map(|i| Schedule::new(i, now)),
            cloud,
        })
    }

    fn run(&mut self) {
        info!("engine started");
        if self.metrics.is_some() {
            // Leave a startup baseline so a short run is still comparable.
            crate::memory::record_metrics_sample(self.renderer.texture_resolution());
        }
        loop {
            match self.rx.recv_timeout(TICK) {
                Ok(cmd) => {
                    if !self.handle(cmd) {
                        break;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }

            // Drain whatever else queued up so a backlog of parameter updates
            // collapses into one render.
            let mut stop = false;
            loop {
                match self.rx.try_recv() {
                    Ok(cmd) => {
                        if !self.handle(cmd) {
                            stop = true;
                            break;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        stop = true;
                        break;
                    }
                }
            }
            if stop {
                break;
            }

            self.tick();
        }
        info!("engine stopped");
    }

    /// Apply one command. Returns `false` when the engine should stop.
    fn handle(&mut self, cmd: EngineCommand) -> bool {
        match cmd {
            EngineCommand::UpdateParams(params) => {
                self.params = *params;
                self.resolve_requested_sample_count();
                self.dirty = true;
            }
            EngineCommand::SetPreviewSize(w, h) => {
                let (qw, qh) = preview_target_size((w, h), self.quality);
                if (qw, qh) != self.renderer.size() {
                    self.renderer.resize(qw, qh);
                    self.dirty = true;
                }
            }
            EngineCommand::SetPreviewEnabled(enabled) => {
                if enabled && !self.preview.enabled {
                    self.preview.owed = true;
                }
                self.preview.enabled = enabled;
            }
            EngineCommand::RenderWallpaperNow => self.publish_wallpaper(),
            EngineCommand::SetDisplayPlan { mode, anchor } => {
                self.display_mode = mode;
                self.anchor_monitor = anchor;
            }
            EngineCommand::RenderToFile {
                path,
                width,
                height,
                reply,
            } => {
                let result = self.render_to_file(&path, width, height);
                let _ = reply.send(result);
            }
            EngineCommand::ExportPixels {
                width,
                height,
                reply,
            } => {
                self.prepare_export();
                let _ = reply.send(self.renderer.export_image(width, height));
            }
            EngineCommand::SetTextureResolution(width) => {
                if self.renderer.set_texture_resolution(width) {
                    info!(texture_resolution = width, "surface texture resolution");
                    // The textures the current mode needs are gone until the
                    // reload lands, so the readiness latch has to reopen or
                    // clients would never hear about the new ones.
                    self.textures_ready = false;
                    self.dirty = true;
                    // The cloud variant follows the same setting, but its slot
                    // is deliberately not purged: the switch must not depend on
                    // the network, and a cloudless gap while a download runs
                    // would be a worse picture than a cloud layer at the
                    // previous variant. Asking for a poll now is the whole of
                    // the change; the existing update path replaces texture,
                    // view, and bind group together when it lands.
                    if let Some(cloud) = &mut self.cloud
                        && cloud.retarget(width)
                    {
                        cloud.owed = true;
                    }
                }
            }
            EngineCommand::ReportMemory { reply } => {
                let _ = reply.send(Box::new(self.renderer.memory_report(&self.adapter_key)));
            }
            EngineCommand::SetAutoRefresh { enabled, interval } => {
                let now = self.clock.elapsed();
                if enabled {
                    match &mut self.auto_refresh {
                        Some(schedule) => schedule.set_interval(interval, now),
                        None => self.auto_refresh = Some(Schedule::new(interval, now)),
                    }
                } else {
                    self.auto_refresh = None;
                }
                debug!(
                    enabled,
                    interval_secs = interval.as_secs(),
                    "auto-refresh changed"
                );
            }
            EngineCommand::DisplaysChanged => {
                // Trailing rather than leading: every hint pushes the deadline
                // out, so a burst is one query at the end of it.
                let due = self.clock.elapsed() + DISPLAY_SETTLE;
                self.display_recheck = Some(due);
                debug!(
                    settle_secs = DISPLAY_SETTLE.as_secs(),
                    "the display layout may have moved"
                );
            }
            EngineCommand::Poke => {
                // A poke means a producer has something waiting, so bring the
                // next drain forward instead of sitting out the interval.
                self.drain.next = Duration::ZERO;
            }
            EngineCommand::Shutdown => return false,
        }
        true
    }

    /// Run everything the schedule says is due, then render if anything changed.
    fn tick(&mut self) {
        let now = self.clock.elapsed();

        if self.drain.due(now) && self.renderer.drain_texture_updates() {
            self.dirty = true;
        }

        if let Some(cloud) = &mut self.cloud {
            if cloud.schedule.due(now) {
                cloud.owed = true;
            }
            // Retry on the next tick rather than waiting out another whole
            // interval when the worker was still busy with the previous poll.
            if cloud.owed && cloud.request() {
                cloud.owed = false;
            }
        }

        // Live time keeps moving even when nothing else changes, so the
        // terminator and the sky have to be recomputed on a schedule of their
        // own.
        if self.sky.due(now) && !self.params.datetime.use_custom {
            self.dirty = true;
        }

        if let Some(metrics) = &mut self.metrics
            && metrics.due(now)
        {
            crate::memory::record_metrics_sample(self.renderer.texture_resolution());
        }

        if let Some(schedule) = &mut self.auto_refresh
            && schedule.due(now)
        {
            info!("auto-refresh: updating wallpaper");
            self.publish_wallpaper();
        }

        // After the auto-refresh, so a recheck that lands on the same tick
        // compares against the list that refresh planned with rather than
        // publishing twice for one layout.
        if self.display_recheck.is_some_and(|due| now >= due) {
            self.display_recheck = None;
            self.recheck_displays();
        }

        let emitted = self.render_if_dirty();
        if self.preview.enabled && self.preview.owed {
            if emitted {
                self.preview.owed = false;
            } else if self.renderer.has_frame() {
                // Nothing changed while the preview was off, so re-send the
                // frame that is already in the texture. A window that was
                // hidden and shown again would otherwise show nothing until
                // the user touched a control.
                self.emit_preview();
                self.preview.owed = false;
            }
            // If no frame exists yet the debt stands: the first render will
            // pay it.
        }

        // After the render, so that a publish held back by a reload goes out on
        // the tick the reload lands and in the order the events describe: the
        // textures became ready, and then the wallpaper was set from them.
        if self.wallpaper_owed {
            self.publish_wallpaper();
        }
    }

    /// Replace the requested MSAA count with one the adapter and the tier both
    /// allow, warning once per distinct request.
    ///
    /// This is the single place a sample count becomes real: a config file, a
    /// combo box built against a different adapter, or a stale saved setting
    /// all funnel through here rather than into `create_render_textures`, where
    /// an unsupported count is a wgpu validation error that kills this thread.
    fn resolve_requested_sample_count(&mut self) {
        let requested = self.params.sample_count;
        let resolved = resolve_sample_count(
            requested,
            &self.supported_sample_counts,
            self.quality.max_sample_count(),
        );
        if resolved != requested && requested != self.requested_sample_count {
            warn!(
                requested,
                using = resolved,
                supported = ?self.supported_sample_counts,
                "MSAA sample count is not available, falling back"
            );
        }
        self.requested_sample_count = requested;
        self.params.sample_count = resolved;
    }

    /// The astronomy state for the current parameters and clock reading.
    fn sky_state(&self) -> SkyState {
        sky::compute_sky_state_at(&self.params.datetime, self.clock.now_utc())
    }

    /// Returns whether a preview frame was emitted.
    fn render_if_dirty(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        let sky = self.sky_state();
        let outcome = self.renderer.render(&self.params, &sky);
        self.dirty = false;
        if matches!(outcome, RenderOutcome::Rendered { first_frame: true }) {
            info!("first frame rendered");
        }

        let status = self.renderer.loading_text(self.params.texture_index);
        if status != self.last_status {
            self.last_status.clone_from(&status);
            self.emit(EngineEvent::Status(status));
        }

        if !self.textures_ready && self.renderer.textures_ready(self.params.texture_index) {
            self.textures_ready = true;
            debug!("textures ready");
            self.dump_memory_report();
            self.emit(EngineEvent::TexturesReady);
        }

        if matches!(outcome, RenderOutcome::Rendered { .. }) && self.preview.enabled {
            self.emit_preview();
            return true;
        }
        false
    }

    /// Write the memory report to the log once, the first time the scene is
    /// fully loaded.
    ///
    /// Behind the level check because assembling the report walks the
    /// allocator's live allocations behind its own lock, and a release build
    /// compiles `debug!` out without compiling out what feeds it.
    fn dump_memory_report(&mut self) {
        if self.memory_dumped || !tracing::enabled!(tracing::Level::DEBUG) {
            return;
        }
        self.memory_dumped = true;
        debug!("\n{}", self.renderer.memory_report(&self.adapter_key));
    }

    /// Read the preview target back and hand the pixels to the client.
    ///
    /// A readback that fails costs this frame and nothing more: the next tick
    /// tries again, and a device that is gone for good will say so on the paths
    /// that have somewhere to report it.
    fn emit_preview(&self) {
        let (width, height) = self.renderer.size();
        let rgba = match self.renderer.read_preview_pixels() {
            Ok(rgba) => rgba,
            Err(e) => {
                warn!(error = %e, "the preview frame could not be read back");
                return;
            }
        };
        self.emit(EngineEvent::PreviewFrame {
            rgba,
            width,
            height,
        });
    }

    /// Render at the sink's native resolution and hand the pixels over.
    ///
    /// Held back while a texture the current mode needs is on its way. The
    /// renderer falls back to the procedural grid while a slot is empty, which
    /// is fine for a preview and not fine for someone's desktop, and a
    /// resolution switch empties one for as long as the reload takes. Every
    /// caller that publishes arrives here, so this covers all of them: the "Set
    /// as Wallpaper" button, the tray's "Refresh Now", the IPC `set-wallpaper`
    /// command, and the auto-refresh schedule. `RenderToFile` and
    /// `ExportPixels` are deliberately not covered; they answer a caller
    /// holding a reply channel, which decides for itself what it will wait for.
    ///
    /// One request is remembered, not a queue of them: two wallpaper updates
    /// asked for during one reload are the same wallpaper.
    fn publish_wallpaper(&mut self) {
        // Support first, before the size query, the render, and the wait below.
        // Off Windows this is the whole answer, and everything after it is
        // something to pay for on the way to a refusal that was already known:
        // a native-resolution render and its readback, or seconds of waiting for
        // textures that will not change it.
        if let Err(e) = self.wallpaper.check_supported() {
            self.report_wallpaper(Err(e));
            return;
        }

        if self.renderer.textures_pending(self.params.texture_index) {
            if !self.wallpaper_owed {
                info!("wallpaper update deferred until the textures have loaded");
            }
            self.wallpaper_owed = true;
            return;
        }

        let result = self
            .build_wallpaper_job()
            .and_then(|(job, note)| Ok(join_notes(note, self.wallpaper.publish(&job)?)));
        self.report_wallpaper(result);
    }

    /// Report a finished publish attempt and settle the debt for it.
    fn report_wallpaper(&mut self, result: Result<String, String>) {
        self.wallpaper_owed = false;
        self.published |= result.is_ok();
        match &result {
            Err(e) => error!(error = %e, "wallpaper update failed"),
            Ok(note) if !note.is_empty() => {
                info!(note, "the wallpaper is not quite what was asked");
            }
            Ok(_) => {}
        }
        self.emit(EngineEvent::WallpaperSet(result));
    }

    /// Ask what the monitors are now, and act if they are not what they were.
    ///
    /// One query per settled burst, whatever the burst was made of: the hints
    /// this design pays for and discards on purpose are a resume from sleep, a
    /// scaling change and a color depth change, each of which costs one
    /// enumeration and an equal comparison.
    ///
    /// The rule for the wallpaper is one sentence. The desk holds a picture
    /// this process made for a layout that is gone, so make one for the layout
    /// that is here. Its two edges are deliberate: somebody who opened the
    /// settings window to look and never asked for a wallpaper does not get one
    /// because they moved a screen, and a sink that refuses is never asked
    /// unprompted, so a layout change cannot put an error in the status line
    /// out of nowhere.
    fn recheck_displays(&mut self) {
        let monitors = match self.wallpaper.monitors() {
            Ok(monitors) => monitors,
            Err(e) => {
                warn!(error = %e, "the monitors could not be listed after a display change");
                return;
            }
        };
        if self.known_monitors.as_ref() == Some(&monitors) {
            debug!(
                screens = monitors.len(),
                "the display layout is the one already known"
            );
            return;
        }
        info!(screens = monitors.len(), "the display layout changed");
        self.known_monitors = Some(monitors.clone());
        self.emit(EngineEvent::MonitorsChanged(monitors));
        if self.published {
            self.publish_wallpaper();
        }
    }

    /// Render everything this session's monitors need, and say what was odd.
    ///
    /// The monitor list is asked for on every publish rather than cached: the
    /// layout changes without telling anybody, and the auto-refresh means a
    /// stale one would be on the screen for as long as the interval.
    fn build_wallpaper_job(&mut self) -> Result<(WallpaperJob, String), String> {
        use crate::display::layout;

        let monitors = self.wallpaper.monitors()?;
        // The list a publish planned with is the one a later hint is compared
        // against, so a layout that kept moving is published again only if it
        // is different again.
        self.known_monitors = Some(monitors.clone());
        let anchor = layout::resolve_anchor(&monitors, self.anchor_monitor.as_deref())
            .ok_or_else(|| "this session has no monitor to put a wallpaper on".to_owned())?;
        let mut note = String::new();
        if anchor.fell_back {
            note = format!(
                "the screen this was set to draw on is not connected, so {} is standing in for it",
                monitors[anchor.index].label
            );
            warn!(note, "the stored anchor monitor is gone");
        }

        let settings = layout::Framing {
            camera_fov: self.params.camera.fov_deg,
            sky_fov: self.params.sky_fov,
            offset_x: self.params.camera.offset_x,
            offset_y: self.params.camera.offset_y,
        };
        let groups = layout::render_groups(&monitors, self.display_mode, anchor.index);
        if groups.is_empty() {
            return Err("this session has no screen with any pixels on it".to_owned());
        }
        for group in &groups {
            self.check_export_fits(group.width, group.height)?;
        }
        self.prepare_export();

        if self.display_mode == layout::DisplayMode::AcrossScreens {
            let bounds = layout::bounds_of(&monitors)
                .ok_or_else(|| "this session has no screen with any pixels on it".to_owned())?;
            let derived = layout::canvas_framing(settings, monitors[anchor.index].rect(), bounds);
            if derived.sky_clamped {
                let clamped = "the sky is as wide as it goes, so it does not continue \
                               across the screens as exactly as the globe does";
                info!(clamped, "the derived sky lens hit the shader's limit");
                note = join_notes(note, clamped.to_owned());
            }
            let pixels = self.export_framed(&derived.framing, bounds.width, bounds.height)?;
            return Ok((
                WallpaperJob {
                    mode: self.display_mode,
                    monitors,
                    anchor: anchor.index,
                    images: JobImages::Spanned {
                        canvas: Arc::new(Frame::new(pixels, bounds.width, bounds.height)),
                        bounds,
                    },
                },
                note,
            ));
        }

        // One render per distinct size, shared by every screen of that size.
        // A screen with no group is one this mode does not paint, and the sink
        // leaves it alone where the desktop lets it.
        let mut images: Vec<Option<Arc<Frame>>> = vec![None; monitors.len()];
        for group in &groups {
            let framing = layout::screen_framing(settings, group.width, group.height);
            let pixels = self.export_framed(&framing, group.width, group.height)?;
            let frame = Arc::new(Frame::new(pixels, group.width, group.height));
            for index in &group.monitors {
                images[*index] = Some(Arc::clone(&frame));
            }
        }
        Ok((
            WallpaperJob {
                mode: self.display_mode,
                monitors,
                anchor: anchor.index,
                images: JobImages::PerMonitor(images),
            },
            note,
        ))
    }

    /// Refuse a render this device cannot make, before it is attempted.
    ///
    /// Both limits are reachable in the span mode and neither fails in a way
    /// anybody could read: past the texture dimension wgpu refuses the texture,
    /// and past the buffer size it panics in the readback.
    fn check_export_fits(&self, width: u32, height: u32) -> Result<(), String> {
        let (max_dimension, max_buffer) = self.renderer.export_limits();
        if width > max_dimension || height > max_dimension {
            return Err(format!(
                "a {width}x{height} wallpaper is larger than this GPU renders \
                 ({max_dimension} pixels on a side); one screen at a time will still work"
            ));
        }
        // The readback buffer, not the image: `read_texture_rgba8` pads every
        // row out to 256 bytes, so a width that is not a multiple of 64 pixels
        // costs more than four bytes each. The guard exists to turn an
        // oversized readback into a sentence instead of a panic, which it can
        // only do if it counts the same bytes the allocation does.
        let bytes = (u64::from(width) * 4).next_multiple_of(256) * u64::from(height);
        if bytes > max_buffer {
            return Err(format!(
                "a {width}x{height} wallpaper reads back {bytes} bytes, and this GPU \
                 takes {max_buffer} at once"
            ));
        }
        debug!(width, height, bytes, "wallpaper export budget");
        Ok(())
    }

    /// Replay the current scene with one screen's framing.
    fn export_framed(
        &mut self,
        framing: &crate::display::layout::Framing,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, String> {
        let mut params = self.params;
        params.camera.fov_deg = framing.camera_fov;
        params.sky_fov = framing.sky_fov;
        params.camera.offset_x = framing.offset_x;
        params.camera.offset_y = framing.offset_y;
        self.renderer.export_image_with(&params, width, height)
    }

    /// Make sure a frame exists to replay, with a current sun direction.
    ///
    /// Nothing may have been rendered yet when the window started hidden, and
    /// the stored sun direction is as old as the last frame, which for a hidden
    /// window can be hours.
    fn prepare_export(&mut self) {
        self.renderer.drain_texture_updates();
        let sky = self.sky_state();
        if matches!(
            self.renderer.render(&self.params, &sky),
            RenderOutcome::Skipped
        ) {
            // A skipped frame kept the sky the last render was drawn with, and
            // for a hidden window that can be hours old. A rendered one is
            // already holding this one.
            self.renderer.set_sky_state(sky);
        }
    }

    fn render_to_file(
        &mut self,
        path: &std::path::Path,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        self.prepare_export();
        let pixels = self.renderer.export_image(width, height)?;
        save_png(path, width, height, &pixels)
    }

    fn emit(&self, event: EngineEvent) {
        (self.on_event)(event);
    }
}

impl CloudWorker {
    /// Point the worker at a different texture resolution's cloud variant.
    ///
    /// Returns whether that changed anything, so the caller can decide whether
    /// a poll is worth asking for.
    fn retarget(&self, resolution: u32) -> bool {
        self.target.swap(resolution, Ordering::SeqCst) != resolution
    }

    /// Ask the worker for one poll. Returns `false` when the worker is still
    /// busy with the previous one, so the caller can retry soon.
    fn request(&self) -> bool {
        if self.busy.swap(true, Ordering::SeqCst) {
            return false;
        }
        if self.tx.send(()).is_err() {
            self.busy.store(false, Ordering::SeqCst);
            return false;
        }
        true
    }
}

/// Spawn the thread that does cloud network I/O and JPEG decoding.
///
/// It never touches the GPU: it parks decoded frames in the mailbox and pokes
/// the engine, which uploads them on its own schedule.
#[allow(clippy::too_many_arguments)]
fn spawn_cloud_worker(
    source: Arc<dyn CloudSource>,
    mailbox: TextureMailbox,
    notify: crate::assets::cloud_fetcher::NotifyFn,
    cache_dir: Option<PathBuf>,
    interval: Duration,
    now: Duration,
    texture_resolution: u32,
    clouds_slot: usize,
) -> CloudWorker {
    info!(source = %source.describe(), poll_secs = interval.as_secs(), "cloud source configured");

    let (tx, rx) = bounded::<()>(1);
    let busy = Arc::new(AtomicBool::new(false));
    let worker_busy = Arc::clone(&busy);
    let target = Arc::new(AtomicU32::new(texture_resolution));
    let worker_target = Arc::clone(&target);

    let thread = std::thread::Builder::new()
        .name("sunlit-cloud".into())
        .spawn(move || {
            let mut updater = CloudUpdater::new(
                source,
                mailbox,
                notify,
                clouds_slot,
                cache_dir,
                texture_resolution,
            );
            let mut backoff = RetryBackoff::new();
            // Show whatever is on disk before touching the network.
            updater.post_cached();
            while rx.recv().is_ok() {
                // Retry a failed poll with exponential backoff rather than
                // waiting out the whole poll interval, which in production is
                // an hour: a thirty-second outage should not cost an hour of
                // stale clouds. The engine's own requests are dropped while
                // this runs (the worker is busy) and retried on the next tick.
                loop {
                    // Inside the retry loop, not outside it: an outage can hold
                    // this thread for minutes at a time, and a resolution
                    // switch made during one should change what the next
                    // attempt asks for rather than wait its turn.
                    updater.set_resolution(worker_target.load(Ordering::SeqCst));
                    match updater.poll_once() {
                        PollOutcome::Updated => {
                            debug!("cloud frame updated");
                            backoff.reset();
                            break;
                        }
                        PollOutcome::Unchanged => {
                            backoff.reset();
                            break;
                        }
                        PollOutcome::Failed => {
                            let delay = backoff.fail();
                            warn!(
                                retry_delay_secs = delay.as_secs(),
                                "cloud poll failed, retrying"
                            );
                            std::thread::sleep(delay);
                            // Give up if the engine went away while we slept.
                            if matches!(rx.try_recv(), Err(TryRecvError::Disconnected)) {
                                return;
                            }
                        }
                    }
                }
                worker_busy.store(false, Ordering::SeqCst);
            }
        })
        .expect("failed to spawn cloud worker thread");

    CloudWorker {
        tx,
        busy,
        target,
        // Poll immediately on the first tick, then on the configured interval.
        schedule: Schedule {
            interval,
            next: now,
        },
        owed: false,
        _thread: thread,
    }
}

/// Two sentences for the status line, where either may be empty.
fn join_notes(first: String, second: String) -> String {
    match (first.is_empty(), second.is_empty()) {
        (true, _) => second,
        (_, true) => first,
        _ => format!("{first}; {second}"),
    }
}

/// Clamp a requested preview size to the tier's cap, preserving the aspect
/// ratio, then quantize it to the renderer's texture granularity.
fn preview_target_size(requested: (u32, u32), quality: QualityTier) -> (u32, u32) {
    let (mut w, mut h) = requested;
    let max_w = quality.max_preview_width();
    if w > max_w && w > 0 {
        h = ((u64::from(h) * u64::from(max_w)) / u64::from(w))
            .try_into()
            .unwrap_or(max_w);
        w = max_w;
    }
    quantize_to_granularity(w, h)
}

/// Encode RGBA8 pixels as PNG and write them to `path`.
pub fn save_png(
    path: &std::path::Path,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<(), String> {
    use image::{ImageBuffer, Rgba};
    let img: ImageBuffer<Rgba<u8>, _> = ImageBuffer::from_raw(width, height, pixels.to_vec())
        .ok_or_else(|| "pixel buffer size mismatch".to_owned())?;
    img.save(path)
        .map_err(|e| format!("failed to save PNG: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_is_not_due_before_its_interval() {
        let mut s = Schedule::new(Duration::from_secs(10), Duration::ZERO);
        assert!(!s.due(Duration::from_secs(9)));
    }

    #[test]
    fn schedule_fires_at_the_interval() {
        let mut s = Schedule::new(Duration::from_secs(10), Duration::ZERO);
        assert!(s.due(Duration::from_secs(10)));
    }

    #[test]
    fn schedule_does_not_burst_after_a_long_stall() {
        let mut s = Schedule::new(Duration::from_secs(10), Duration::ZERO);
        // One jump of a simulated day must produce one run, not 8640.
        assert!(s.due(Duration::from_hours(24)));
        assert!(!s.due(Duration::from_hours(24)));
        assert!(s.due(Duration::from_secs(86_410)));
    }

    #[test]
    fn schedule_interval_change_restarts_the_countdown() {
        let mut s = Schedule::new(Duration::from_mins(10), Duration::ZERO);
        s.set_interval(Duration::from_mins(1), Duration::from_secs(30));
        assert!(!s.due(Duration::from_secs(89)));
        assert!(s.due(Duration::from_secs(90)));
    }

    #[test]
    fn preview_size_passes_through_below_the_cap() {
        assert_eq!(
            preview_target_size((1024, 640), QualityTier::Low),
            quantize_to_granularity(1024, 640)
        );
    }

    #[test]
    fn preview_size_is_capped_at_the_low_tier() {
        let (w, h) = preview_target_size((3840, 2160), QualityTier::Low);
        assert!(w <= QualityTier::Low.max_preview_width());
        // 3840x2160 scaled to 1280 wide is 720 high, quantized to 704.
        assert_eq!((w, h), (1280, 704));
    }

    #[test]
    fn preview_size_cap_grows_with_the_tier() {
        let low = preview_target_size((3840, 2160), QualityTier::Low);
        let medium = preview_target_size((3840, 2160), QualityTier::Medium);
        let high = preview_target_size((3840, 2160), QualityTier::High);
        assert!(low.0 < medium.0);
        assert!(medium.0 < high.0);
        assert_eq!(high, quantize_to_granularity(3840, 2160));
    }

    #[test]
    fn preview_size_cap_preserves_the_aspect_ratio() {
        let (w, h) = preview_target_size((2560, 1440), QualityTier::Low);
        #[allow(clippy::cast_precision_loss)]
        let ratio = f64::from(w) / f64::from(h);
        assert!((ratio - 16.0 / 9.0).abs() < 0.05, "got {w}x{h}");
    }

    /// The unbounded-channel justification at the `start` declaration rests on
    /// commands being small. This pin catches inline-size regressions such as
    /// un-boxing `SceneParams` (about 200 bytes). It cannot catch a variant
    /// that carries a heap buffer (`Vec<u8>` is 24 bytes inline), so a command
    /// that transported pixels would pass; the review guard for that is the
    /// justification comment itself, which any such variant must update.
    #[test]
    fn command_payload_is_small() {
        let size = std::mem::size_of::<EngineCommand>();
        assert!(
            size <= 64,
            "EngineCommand grew to {size} bytes inline; revisit the unbounded \
             channel justification in `start`"
        );
    }

    #[test]
    fn save_png_rejects_a_short_buffer() {
        let dir = std::env::temp_dir().join("sunlit_earth_test_save_png");
        let _ = std::fs::create_dir_all(&dir);
        let err = save_png(&dir.join("x.png"), 4, 4, &[0; 8]).unwrap_err();
        assert!(err.contains("size mismatch"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
