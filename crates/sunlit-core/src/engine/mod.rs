//! The engine: one thread that owns the GPU device, the renderer, the asset
//! mailbox, and the schedule.
//!
//! Clients (the Slint shell, the `render` subcommand, tests) send commands and
//! receive events. Nothing the engine does depends on a window existing, which
//! is the whole point: hiding the settings window removes a client, it does not
//! half-suspend the machinery.

pub mod clock;
mod cloud_worker;
mod handle;
mod protocol;
mod publish;
mod schedule;
pub mod wallpaper_sink;

use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use tracing::{debug, info, warn};

use crate::assets::mailbox::TextureMailbox;
use crate::config::QualityTier;
use crate::params::SceneParams;
use crate::renderer::{
    RenderOutcome, Renderer, RendererConfig, SlotLayout, quantize_to_granularity,
    resolve_sample_count,
};
use crate::scene::sky::{self, SkyState};

use clock::Clock;
use cloud_worker::{CloudWorker, spawn_cloud_worker};
use handle::AdapterReport;
pub use handle::{EngineConfig, EngineHandle, start};
pub use protocol::{EngineCommand, EngineEvent};
use schedule::Schedule;
use wallpaper_sink::WallpaperSink;

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
///
/// Public because the integration tests step over it. A test that spelled the
/// number itself would still pass when this one moved, having quietly stopped
/// stepping over anything.
pub const DISPLAY_SETTLE: Duration = Duration::from_secs(2);

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
    /// A client asked for a wallpaper update. The publish happens in `tick`, so
    /// several requests drained together cost one native-resolution render
    /// rather than one each.
    publish_asked: bool,
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
    /// Whether the last readback failed. A device that keeps failing is still
    /// retried on every tick, because it may come back, but it costs one line
    /// in the log rather than twenty a second.
    readback_failed: bool,
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

        // Checked rather than trusted, and before the device exists, because
        // neither way of getting it wrong is visible where it happens. Too few
        // slots drops the posts for the high ones, leaving those slots waiting
        // for a load that was thrown away; too many hands the consumer a slot
        // index its own array does not have, which is a panic in the middle of a
        // session. Failing here means failing before `ready` is sent, so `start`
        // reports a thread that never got as far as its adapter rather than
        // handing back a handle to a thread that quietly died.
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
                readback_failed: false,
            },
            wallpaper_owed: false,
            publish_asked: false,
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
            EngineCommand::RenderWallpaperNow => self.publish_asked = true,
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
            EngineCommand::Shutdown => {
                // A publish asked for in the same drain batch as the shutdown
                // is still someone's request, and there is no tick left to do
                // it in.
                if std::mem::take(&mut self.publish_asked) {
                    self.publish_wallpaper();
                }
                return false;
            }
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

        // A frame just drawn goes out, and so does a debt from the preview
        // being turned back on: nothing changed while it was off, so the frame
        // already in the texture is the answer. Either way the readback happens
        // once per tick, and a debt no readback could pay stands until one can,
        // including the case where no frame exists yet.
        let rendered = self.render_if_dirty();
        if self.preview.enabled
            && (rendered || self.preview.owed)
            && self.renderer.has_frame()
            && self.emit_preview()
        {
            self.preview.owed = false;
        }

        // After the render, so that a publish held back by a reload goes out on
        // the tick the reload lands and in the order the events describe: the
        // textures became ready, and then the wallpaper was set from them.
        //
        // One publish however many asked for it: a double-click on "Set as
        // Wallpaper", or the tray and IPC arriving together, are one wallpaper.
        if std::mem::take(&mut self.publish_asked) || self.wallpaper_owed {
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

    /// Returns whether a new frame was drawn. Emitting it is `tick`'s, so that
    /// a tick asks the preview target for its pixels once however it got here.
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

        matches!(outcome, RenderOutcome::Rendered { .. })
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

    /// Read the preview target back and hand the pixels to the client, saying
    /// whether one got there.
    ///
    /// A readback that fails costs this frame and nothing more, and the caller
    /// keeps whatever debt it was paying, so the next tick tries again. Only the
    /// first failure of a run is logged, because the retry is every 50 ms and a
    /// device that is gone stays gone. A device that is gone for good says so on
    /// the paths that have somewhere to report it.
    fn emit_preview(&mut self) -> bool {
        let (width, height) = self.renderer.size();
        let rgba = match self.renderer.read_preview_pixels() {
            Ok(rgba) => rgba,
            Err(e) => {
                if !self.preview.readback_failed {
                    self.preview.readback_failed = true;
                    warn!(error = %e, "the preview frame could not be read back");
                }
                return false;
            }
        };
        self.preview.readback_failed = false;
        self.emit(EngineEvent::PreviewFrame {
            rgba,
            width,
            height,
        });
        true
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
    fn preview_size_passes_through_below_the_cap() {
        assert_eq!(
            preview_target_size((1024, 640), QualityTier::Low),
            quantize_to_granularity(1024, 640)
        );
    }

    /// A source wider than the tier's cap comes back under the cap and already
    /// quantized. The exact granularity is `quantize_to_granularity`'s own
    /// business and has its own tests; what matters here is that this path goes
    /// through it, and that the tier's cap rather than a written-down width is
    /// what bounds the result.
    #[test]
    fn preview_size_is_capped_at_the_tiers_own_width() {
        let (w, h) = preview_target_size((3840, 2160), QualityTier::Low);
        let cap = QualityTier::Low.max_preview_width();
        assert!(w <= cap, "{w} is above the tier's cap of {cap}");
        assert_eq!(
            (w, h),
            quantize_to_granularity(w, h),
            "a preview size that is not already quantized never reached the quantizer"
        );
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

    /// The length check comes before the filesystem does, so this needs no
    /// directory to write into and never reaches one.
    #[test]
    fn save_png_rejects_a_short_buffer() {
        let path = std::path::Path::new("no-directory-is-touched/x.png");
        let err = save_png(path, 4, 4, &[0; 8]).unwrap_err();
        assert!(err.contains("size mismatch"), "unexpected error: {err}");
        assert!(!path.exists());
    }
}
