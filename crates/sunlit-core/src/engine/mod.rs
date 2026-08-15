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
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded, unbounded};
use tracing::{debug, error, info, warn};

use crate::assets::cloud_fetcher::{CloudUpdater, PollOutcome};
use crate::assets::cloud_source::CloudSource;
use crate::assets::mailbox::TextureMailbox;
use crate::config::QualityTier;
use crate::params::SceneParams;
use crate::renderer::{
    CLOUDS_SLOT, RenderOutcome, Renderer, RendererConfig, quantize_to_granularity,
};
use crate::scene::sun;

use clock::{Clock, SystemClock};
use wallpaper_sink::WallpaperSink;

/// How long the loop blocks on the command channel before re-checking the
/// schedule. Short enough that a real-clock deadline is never missed by more
/// than this, cheap enough to ignore: the wake does nothing when nothing is due.
const TICK: Duration = Duration::from_millis(50);

/// How often decoded textures are uploaded to the GPU. Unconditional: this is
/// what keeps cloud updates flowing while no window is visible.
const DRAIN_INTERVAL: Duration = Duration::from_secs(5);

/// How often the sun position is recomputed when rendering live time.
const SUN_INTERVAL: Duration = Duration::from_secs(120);

/// How often a memory sample is appended to the metrics CSV.
const METRICS_INTERVAL: Duration = Duration::from_secs(600);

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
    /// Turn the unattended wallpaper refresh on or off.
    SetAutoRefresh { enabled: bool, interval: Duration },
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
    /// Every texture the current mode needs has finished loading. Fires once.
    TexturesReady,
    /// A wallpaper publish attempt finished.
    WallpaperSet(Result<(), String>),
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
    /// Caps the preview size and the MSAA sample count, and selects the cloud
    /// image variant.
    pub quality: QualityTier,
    pub clock: Arc<dyn Clock>,
    /// `None` disables cloud fetching entirely (the `SUNLIT_EARTH_NO_CLOUDS`
    /// case, and the default for tests that do not care about clouds).
    pub cloud: Option<Arc<dyn CloudSource>>,
    pub cloud_poll_interval: Duration,
    pub cloud_cache_dir: Option<PathBuf>,
    /// Unattended wallpaper refresh interval; `None` disables it.
    pub auto_refresh: Option<Duration>,
    pub wallpaper: Arc<dyn WallpaperSink>,
    /// Called on the engine thread for every event. Clients that need to be on
    /// another thread (the UI) forward from here.
    pub on_event: Arc<dyn Fn(EngineEvent) + Send + Sync>,
    /// Write periodic memory samples to the metrics CSV.
    pub record_metrics: bool,
}

impl EngineConfig {
    /// A minimal headless configuration: system clock, no clouds, no
    /// auto-refresh, events dropped on the floor.
    pub fn headless(preview_size: (u32, u32)) -> Self {
        Self {
            force_software: false,
            // Two file-backed slots (day, night) so the slot layout matches
            // production even when no texture files are present.
            texture_paths: vec![None, None],
            preview_size,
            preview_enabled: true,
            params: SceneParams::default(),
            // Tests always run at the cheap tier, whatever the build profile.
            quality: QualityTier::Low,
            clock: Arc::new(SystemClock::new()),
            cloud: None,
            cloud_poll_interval: Duration::from_secs(3600),
            cloud_cache_dir: None,
            auto_refresh: None,
            wallpaper: Arc::new(wallpaper_sink::SystemWallpaper),
            on_event: Arc::new(|_| {}),
            record_metrics: false,
        }
    }
}

/// Client-side handle to a running engine.
pub struct EngineHandle {
    tx: Sender<EngineCommand>,
    thread: Option<JoinHandle<()>>,
    adapter_info: String,
    supported_sample_counts: Vec<u32>,
}

impl EngineHandle {
    /// Send a command. Fails silently once the engine has stopped, because
    /// every caller here is a best-effort UI notification.
    pub fn send(&self, cmd: EngineCommand) {
        if self.tx.send(cmd).is_err() {
            debug!("engine command dropped: the engine has stopped");
        }
    }

    /// Sender for callers that need to enqueue from another thread.
    pub fn sender(&self) -> Sender<EngineCommand> {
        self.tx.clone()
    }

    /// Description of the selected GPU adapter, for the UI's renderer label.
    pub fn adapter_info(&self) -> &str {
        &self.adapter_info
    }

    /// MSAA sample counts the adapter supports for the render format.
    pub fn supported_sample_counts(&self) -> &[u32] {
        &self.supported_sample_counts
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

    /// Render a PNG at `width` x `height` and block until it is written.
    pub fn render_to_file(
        &self,
        path: PathBuf,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
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
        self.send(EngineCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(EngineCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Start the engine on its own thread.
///
/// Blocks until the GPU device exists, so the returned handle can report the
/// adapter and the supported sample counts without another round trip.
pub fn start(config: EngineConfig) -> EngineHandle {
    let (tx, rx) = unbounded();
    let (ready_tx, ready_rx) = bounded(1);

    let command_tx = tx.clone();
    let thread = std::thread::Builder::new()
        .name("sunlit-engine".into())
        .spawn(move || {
            let mut engine = Engine::new(config, rx, &command_tx, &ready_tx);
            drop(ready_tx);
            engine.run();
        })
        .expect("failed to spawn engine thread");

    let (adapter_info, supported_sample_counts) = ready_rx
        .recv()
        .expect("engine thread died before reporting its adapter");

    EngineHandle {
        tx,
        thread: Some(thread),
        adapter_info,
        supported_sample_counts,
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
struct Engine {
    rx: Receiver<EngineCommand>,
    clock: Arc<dyn Clock>,
    on_event: Arc<dyn Fn(EngineEvent) + Send + Sync>,
    wallpaper: Arc<dyn WallpaperSink>,
    renderer: Renderer,
    params: SceneParams,
    quality: QualityTier,
    preview: PreviewState,
    /// Set when something happened that the next render must pick up.
    dirty: bool,
    textures_ready: bool,
    last_status: String,
    drain: Schedule,
    sun: Schedule,
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
    schedule: Schedule,
    _thread: JoinHandle<()>,
}

impl Engine {
    #[allow(clippy::too_many_lines)]
    fn new(
        config: EngineConfig,
        rx: Receiver<EngineCommand>,
        command_tx: &Sender<EngineCommand>,
        ready: &Sender<(String, Vec<u32>)>,
    ) -> Self {
        let EngineConfig {
            force_software,
            texture_paths,
            preview_size,
            preview_enabled,
            mut params,
            quality,
            clock,
            cloud,
            cloud_poll_interval,
            cloud_cache_dir,
            auto_refresh,
            wallpaper,
            on_event,
            record_metrics,
        } = config;

        let gpu = crate::wgpu_init::init(force_software);
        let _ = ready.send((gpu.adapter_info.clone(), gpu.supported_sample_counts.clone()));
        crate::memory::log_memory_usage("engine: after wgpu init");

        // Slots: grid + one per texture path + clouds.
        let mailbox = TextureMailbox::new(texture_paths.len() + 2);

        // Every background producer wakes the engine loop through the same
        // command channel, so there is exactly one place that decides what to
        // do about new work.
        let poke_tx = command_tx.clone();
        let notify: crate::assets::cloud_fetcher::NotifyFn = Arc::new(move || {
            let _ = poke_tx.send(EngineCommand::Poke);
        });

        params.sample_count = params.sample_count.min(quality.max_sample_count());
        let (width, height) = preview_target_size(preview_size, quality);
        let renderer = Renderer::new(
            gpu.device,
            gpu.queue,
            RendererConfig {
                sample_count: params.sample_count,
                width,
                height,
                texture_paths,
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
                cloud_cache_dir,
                cloud_poll_interval,
                now,
            )
        });

        Self {
            rx,
            clock,
            on_event,
            wallpaper,
            renderer,
            params,
            quality,
            preview: PreviewState {
                enabled: preview_enabled,
                owed: false,
            },
            dirty: true,
            textures_ready: false,
            last_status: String::new(),
            drain: Schedule::new(DRAIN_INTERVAL, now),
            sun: Schedule::new(SUN_INTERVAL, now),
            metrics: record_metrics.then(|| Schedule::new(METRICS_INTERVAL, now)),
            auto_refresh: auto_refresh.map(|i| Schedule::new(i, now)),
            cloud,
        }
    }

    fn run(&mut self) {
        info!("engine started");
        if self.metrics.is_some() {
            // Leave a startup baseline so a short run is still comparable.
            crate::memory::record_metrics_sample();
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
                self.params.sample_count = self
                    .params
                    .sample_count
                    .min(self.quality.max_sample_count());
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
                debug!(enabled, interval_secs = interval.as_secs(), "auto-refresh changed");
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

        if let Some(cloud) = &mut self.cloud
            && cloud.schedule.due(now)
        {
            cloud.request();
        }

        // Live time keeps moving even when nothing else changes, so the
        // terminator has to be recomputed on a schedule of its own.
        if self.sun.due(now) && !self.params.datetime.use_custom {
            self.dirty = true;
        }

        if let Some(metrics) = &mut self.metrics
            && metrics.due(now)
        {
            crate::memory::record_metrics_sample();
        }

        if let Some(schedule) = &mut self.auto_refresh
            && schedule.due(now)
        {
            info!("auto-refresh: updating wallpaper");
            self.publish_wallpaper();
        }

        let emitted = self.render_if_dirty();
        if self.preview.enabled && self.preview.owed {
            if !emitted && self.renderer.has_frame() {
                self.emit_preview();
            }
            self.preview.owed = false;
        }
    }

    /// The sun direction for the current parameters and clock reading.
    fn sun_direction(&self) -> glam::Vec3 {
        sun::compute_sun_direction_at(&self.params.datetime, self.clock.now_utc())
    }

    /// Returns whether a preview frame was emitted.
    fn render_if_dirty(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        let sun_dir = self.sun_direction();
        let outcome = self.renderer.render(&self.params, sun_dir);
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
            self.emit(EngineEvent::TexturesReady);
        }

        if matches!(outcome, RenderOutcome::Rendered { .. }) && self.preview.enabled {
            self.emit_preview();
            return true;
        }
        false
    }

    /// Read the preview target back and hand the pixels to the client.
    fn emit_preview(&self) {
        let (width, height) = self.renderer.size();
        let rgba = self.renderer.read_preview_pixels();
        self.emit(EngineEvent::PreviewFrame {
            rgba,
            width,
            height,
        });
    }

    /// Render at the sink's native resolution and hand the pixels over.
    fn publish_wallpaper(&mut self) {
        let result = self.render_wallpaper_pixels().and_then(|(pixels, w, h)| {
            self.wallpaper.publish(&pixels, w, h)
        });
        if let Err(e) = &result {
            error!(error = %e, "wallpaper update failed");
        }
        self.emit(EngineEvent::WallpaperSet(result));
    }

    fn render_wallpaper_pixels(&mut self) -> Result<(Vec<u8>, u32, u32), String> {
        let (width, height) = self.wallpaper.target_size()?;
        self.prepare_export();
        let pixels = self.renderer.export_image(width, height)?;
        Ok((pixels, width, height))
    }

    /// Make sure a frame exists to replay, with a current sun direction.
    ///
    /// Nothing may have been rendered yet when the window started hidden, and
    /// the stored sun direction is as old as the last frame, which for a hidden
    /// window can be hours.
    fn prepare_export(&mut self) {
        self.renderer.drain_texture_updates();
        let sun_dir = self.sun_direction();
        self.renderer.render(&self.params, sun_dir);
        self.renderer.set_sun_direction(sun_dir);
    }

    fn render_to_file(&mut self, path: &std::path::Path, width: u32, height: u32) -> Result<(), String> {
        self.prepare_export();
        let pixels = self.renderer.export_image(width, height)?;
        save_png(path, width, height, &pixels)
    }

    fn emit(&self, event: EngineEvent) {
        (self.on_event)(event);
    }
}

impl CloudWorker {
    /// Ask the worker for one poll, unless it is still busy with the last one.
    fn request(&self) {
        if self.busy.swap(true, Ordering::SeqCst) {
            debug!("cloud poll skipped: previous poll still running");
            return;
        }
        if self.tx.send(()).is_err() {
            self.busy.store(false, Ordering::SeqCst);
        }
    }
}

/// Spawn the thread that does cloud network I/O and JPEG decoding.
///
/// It never touches the GPU: it parks decoded frames in the mailbox and pokes
/// the engine, which uploads them on its own schedule.
fn spawn_cloud_worker(
    source: Arc<dyn CloudSource>,
    mailbox: TextureMailbox,
    notify: crate::assets::cloud_fetcher::NotifyFn,
    cache_dir: Option<PathBuf>,
    interval: Duration,
    now: Duration,
) -> CloudWorker {
    info!(source = %source.describe(), poll_secs = interval.as_secs(), "cloud source configured");

    let (tx, rx) = bounded::<()>(1);
    let busy = Arc::new(AtomicBool::new(false));
    let worker_busy = Arc::clone(&busy);

    let thread = std::thread::Builder::new()
        .name("sunlit-cloud".into())
        .spawn(move || {
            let mut updater = CloudUpdater::new(source, mailbox, notify, CLOUDS_SLOT, cache_dir);
            // Show whatever is on disk before touching the network.
            updater.post_cached();
            while rx.recv().is_ok() {
                match updater.poll_once() {
                    PollOutcome::Updated => debug!("cloud frame updated"),
                    PollOutcome::Unchanged => {}
                    PollOutcome::Failed => warn!("cloud poll failed"),
                }
                worker_busy.store(false, Ordering::SeqCst);
            }
        })
        .expect("failed to spawn cloud worker thread");

    let mut worker = CloudWorker {
        tx,
        busy,
        // Poll immediately on the first tick, then on the configured interval.
        schedule: Schedule {
            interval,
            next: now,
        },
        _thread: thread,
    };
    worker.schedule.next = now;
    worker
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
pub fn save_png(path: &std::path::Path, width: u32, height: u32, pixels: &[u8]) -> Result<(), String> {
    use image::{ImageBuffer, Rgba};
    let img: ImageBuffer<Rgba<u8>, _> = ImageBuffer::from_raw(width, height, pixels.to_vec())
        .ok_or_else(|| "pixel buffer size mismatch".to_owned())?;
    img.save(path).map_err(|e| format!("failed to save PNG: {e}"))
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
        assert!(s.due(Duration::from_secs(86_400)));
        assert!(!s.due(Duration::from_secs(86_400)));
        assert!(s.due(Duration::from_secs(86_410)));
    }

    #[test]
    fn schedule_interval_change_restarts_the_countdown() {
        let mut s = Schedule::new(Duration::from_secs(600), Duration::ZERO);
        s.set_interval(Duration::from_secs(60), Duration::from_secs(30));
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

    #[test]
    fn save_png_rejects_a_short_buffer() {
        let dir = std::env::temp_dir().join("sunlit_earth_test_save_png");
        let _ = std::fs::create_dir_all(&dir);
        let err = save_png(&dir.join("x.png"), 4, 4, &[0; 8]).unwrap_err();
        assert!(err.contains("size mismatch"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
