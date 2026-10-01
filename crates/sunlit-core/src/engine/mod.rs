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
mod surface;
mod tile_loader;
pub mod wallpaper_sink;

use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use tracing::{debug, info, warn};

use crate::assets::mailbox::TextureMailbox;
use crate::config::QualityTier;
use crate::params::SceneParams;
use crate::renderer::residency::{Output, surfaces_for};
use crate::renderer::{
    RenderOutcome, Renderer, RendererConfig, SlotLayout, SurfaceFormats, quantize_to_granularity,
    resolve_sample_count, surface_sampler_descriptor,
};
use crate::scene::sky::{self, SkyState};

use clock::Clock;
use cloud_worker::{CloudWorker, spawn_cloud_worker};
use handle::AdapterReport;
pub use handle::{EngineConfig, EngineHandle, start};
pub use protocol::{EngineCommand, EngineEvent};
use schedule::Schedule;
use surface::SurfaceFeed;
use tile_loader::{LoaderConfig, View};
pub use tile_loader::{TileGate, TileReport};
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

/// How long an export waits for the tiles its frame wants, on the injected
/// clock, before it is made with what is resident in their place: their
/// resident ancestors, or the floor. A wallpaper made that way is made again
/// once the tiles are resident.
///
/// Public because the integration tests step over it.
pub const TILE_WAIT: Duration = Duration::from_secs(5);

/// The engine's own state, private to its thread.
///
/// The bools are independent latches on a private struct rather than
/// parameters anyone passes, which is the confusion the lint is about.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent latches on a private struct, which is not the confusion the lint is about"
)]
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
    /// The renders the wallpaper owed, or to be made again, is made of, whose
    /// tiles join the wanted set until it is made from them.
    wallpaper_shots: Vec<publish::Shot>,
    /// When the wallpaper owed began to wait for its tiles alone, the cubes
    /// being resident, which is what [`TILE_WAIT`] counts from, and how many
    /// packs had been opened for their tiles then: one opened since starts
    /// the count over.
    tiles_awaited_since: Option<(Duration, u64)>,
    /// A wallpaper went out after [`TILE_WAIT`] with tiles missing, and is
    /// made again when they are resident.
    reexport: bool,
    /// `RenderToFile` and `ExportPixels` requests waiting for their tiles.
    exports: Vec<publish::WaitingExport>,
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
    /// The transcoder and the month in force, when the globe is drawn from the
    /// cube surface.
    surface: Option<SurfaceFeed>,
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
    #[expect(
        clippy::too_many_lines,
        reason = "one linear setup sequence, most of it the state's fields one per line"
    )]
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
            cube_textures,
            tile_geometry,
            tile_layers,
            tile_gate,
        } = config;

        let slots = SlotLayout::new(texture_paths.len());
        let mailbox = checked_mailbox(mailbox, slots, texture_paths.len());
        let gpu = open_gpu(force_software, ready)?;
        if let Some(dir) = &cache_dir {
            crate::assets::texture_loader::remove_retired_downscales(dir);
        }

        // Every background producer wakes the engine loop through the same
        // command channel, so there is exactly one place that decides what to
        // do about new work. The channel is unbounded, so the send never
        // blocks, which the transcoder's notify callback relies on: it runs on
        // a thread that dropping the transcoder joins.
        let poke_tx = command_tx.clone();
        let notify: crate::assets::cloud_fetcher::NotifyFn = Arc::new(move || {
            let _ = poke_tx.send(EngineCommand::Poke);
        });

        let now = clock.elapsed();
        let month = crate::scene::month::month_in_force(&params.datetime, clock.now_utc());
        let cpu_adapter = gpu.device_type == wgpu::DeviceType::Cpu;
        let formats = SurfaceFormats::for_adapter(
            gpu.device
                .features()
                .contains(wgpu::Features::TEXTURE_COMPRESSION_BC),
            cpu_adapter,
        );
        let surface = start_surface(
            cube_textures,
            tile_geometry,
            cache_dir.as_ref(),
            month,
            now,
            &notify,
            &LoaderConfig {
                decode: formats.color == wgpu::TextureFormat::Rgba8Unorm,
                anisotropy: surface_sampler_descriptor(cpu_adapter).anisotropy_clamp,
                workers: crate::assets::tiles::default_threads(),
                gate: tile_gate,
            },
        );

        let requested_sample_count = params.sample_count;
        params.sample_count = resolve_and_warn(
            requested_sample_count,
            &gpu.supported_sample_counts,
            quality.max_sample_count(),
            None,
        );
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
                mailbox: mailbox.clone(),
                notify: Arc::clone(&notify),
                cube_month: surface.as_ref().map(|_| month),
                cpu_adapter,
                tile_geometry,
                tile_layers,
            },
        );

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
            wallpaper_shots: Vec::new(),
            tiles_awaited_since: None,
            reexport: false,
            exports: Vec::new(),
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
            surface,
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
            } => self.export(width, height, publish::ExportReply::File { path, reply }),
            EngineCommand::ExportPixels {
                width,
                height,
                reply,
            } => self.export(width, height, publish::ExportReply::Pixels(reply)),
            EngineCommand::SetTextureResolution(width) => self.set_texture_resolution(width),
            EngineCommand::ReportMemory { reply } => {
                let _ = reply.send(Box::new(self.renderer.memory_report(&self.adapter_key)));
            }
            EngineCommand::ReportTiles { reply } => {
                let report = self
                    .surface
                    .as_ref()
                    .map(|surface| surface.tile_report(&self.renderer));
                let _ = reply.send(report.map(Box::new));
            }
            EngineCommand::SetAutoRefresh { enabled, interval } => {
                self.set_auto_refresh(enabled, interval);
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
                // A publish asked for in the same drain batch as the shutdown,
                // or one still waiting for its tiles, is still someone's
                // request, and there is no tick left to do it in or to wait
                // in.
                if std::mem::take(&mut self.publish_asked) || self.wallpaper_owed {
                    self.publish_wallpaper_at_exit();
                }
                // A caller waiting on an export is answered with what is
                // resident rather than with a closed channel.
                if !self.exports.is_empty() {
                    self.prepare_export();
                    for export in std::mem::take(&mut self.exports) {
                        self.answer(export);
                    }
                }
                return false;
            }
        }
        true
    }

    /// Point the renderer at another texture resolution's set of assets.
    fn set_texture_resolution(&mut self, width: u32) {
        if self.renderer.set_texture_resolution(width) {
            info!(texture_resolution = width, "surface texture resolution");
            // The tiles of the old cap go, the floors stay, and the next draw
            // asks for the tiles the new one allows.
            if let Some(surface) = &mut self.surface {
                surface.purge_tiles(&mut self.renderer);
            }
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

    /// Start, restart or stop the auto-refresh schedule.
    fn set_auto_refresh(&mut self, enabled: bool, interval: Duration) {
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

    /// Run everything the schedule says is due, then render if anything changed.
    fn tick(&mut self) {
        let now = self.clock.elapsed();

        if self.drain.due(now) && self.renderer.drain_texture_updates() {
            self.dirty = true;
        }

        // Every tick rather than on the drain's schedule: a pack that landed is
        // one cheap check away, and the first frame of a first run waits on it.
        if let Some(surface) = &mut self.surface
            && surface.drain(&mut self.renderer)
        {
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

        if let Some(surface) = &mut self.surface
            && surface.settle_drag(&self.params.camera, now)
        {
            self.dirty = true;
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
        } else if self.reexport
            && !self.renderer.textures_pending(self.params.texture_index)
            && !self.shots_pending()
        {
            info!("the tiles a wallpaper went out without are resident; making it again");
            self.publish_wallpaper();
        }
        self.settle_exports();

        // Last, so that the gate opens only when nothing in this tick drew,
        // and so that a drag's pause counts from the end of the frame's work,
        // its readback included, which on a software adapter is most of it.
        if let Some(surface) = &mut self.surface {
            surface.relax(now);
            surface.end_tick(self.clock.elapsed());
        }
    }

    /// Replace the requested MSAA count with one [`resolve_and_warn`] allows,
    /// remembering the request so the same one is reported only once.
    fn resolve_requested_sample_count(&mut self) {
        let requested = self.params.sample_count;
        let resolved = resolve_and_warn(
            requested,
            &self.supported_sample_counts,
            self.quality.max_sample_count(),
            Some(self.requested_sample_count),
        );
        self.requested_sample_count = requested;
        self.params.sample_count = resolved;
    }

    /// The astronomy state for the current parameters and clock reading.
    fn sky_state(&self) -> SkyState {
        sky::compute_sky_state_at(&self.params.datetime, self.clock.now_utc())
    }

    /// Derive the month in force from the date, as the sky state is, and move
    /// the surface to it when it changed.
    ///
    /// Outside the digest like the sun direction: the month is not a
    /// parameter anyone sets but a consequence of the date. A floor that
    /// becomes resident here redraws through the renderer's own flag, and one
    /// that is not there yet reopens the readiness latch, so clients hear
    /// `TexturesReady` again when it lands.
    fn sync_month(&mut self) {
        let Some(surface) = &mut self.surface else {
            return;
        };
        let month =
            crate::scene::month::month_in_force(&self.params.datetime, self.clock.now_utc());
        surface.set_month(month, &mut self.renderer);
        if !self.renderer.textures_ready(self.params.texture_index) {
            self.textures_ready = false;
        }
    }

    /// Whether everything the frame needs is resident: the cubes the mode
    /// needs, and every tile it wants in view at the 1 px threshold, a failed
    /// one counting as there (plan decision 10).
    fn textures_ready(&self) -> bool {
        self.renderer.textures_ready(self.params.texture_index)
            && self
                .surface
                .as_ref()
                .is_none_or(|surface| surface.tiles_complete(&self.renderer))
    }

    /// The engine is about to draw, or has just drawn: the transcoder's pause
    /// gate closes now and stays closed for a while.
    fn mark_busy(&mut self) {
        let now = self.clock.elapsed();
        if let Some(surface) = &mut self.surface {
            surface.mark_busy(now);
        }
    }

    /// Draw the scene as it stands, for the preview or for an export, and hand
    /// back the sky it was drawn under.
    ///
    /// Every path that draws comes through here first, the preview's and each
    /// export's, so that the pause gate closes before any of them draws: a
    /// build of the rest of the year gives way at its next check rather than
    /// running through the frame.
    fn draw(&mut self) -> (RenderOutcome, SkyState) {
        self.sync_month();
        self.mark_busy();
        let sky = self.sky_state();
        self.want_tiles(&sky);
        let outcome = self.renderer.render(&self.params, &sky);
        self.note_readiness();
        (outcome, sky)
    }

    /// Tell clients what is loading, and when everything the frame needs has
    /// become resident.
    ///
    /// After every draw, the preview's and an export's alike, so that
    /// `TexturesReady` goes out before a wallpaper drawn from what it
    /// announces. The latch follows readiness both ways, so new tiles wanted,
    /// a drag, a month or a resolution switch each reopen it, and clients hear
    /// it again when what they want is there.
    fn note_readiness(&mut self) {
        let status = self.renderer.loading_text(self.params.texture_index);
        if status != self.last_status {
            self.last_status.clone_from(&status);
            self.emit(EngineEvent::Status(status));
        }
        let ready = self.textures_ready();
        if ready && !self.textures_ready {
            debug!("textures ready");
            self.dump_memory_report();
            self.emit(EngineEvent::TexturesReady);
        }
        self.textures_ready = ready;
    }

    /// Ask the tile loader for the tiles the preview wants under `sky`, and
    /// every export waiting for its tiles, before the frame is drawn, so the
    /// page table it draws with is held to the preview's cap.
    fn want_tiles(&mut self, sky: &SkyState) {
        let Some(surface) = &mut self.surface else {
            return;
        };
        let (width, height) = self.renderer.size();
        let mut outputs = vec![tile_output(&self.params, width, height, false)];
        outputs.extend(self.wallpaper_shots.iter().map(|shot| {
            tile_output(
                &shot.framing.applied_to(&self.params),
                shot.width,
                shot.height,
                true,
            )
        }));
        outputs.extend(
            self.exports
                .iter()
                .map(|export| tile_output(&self.params, export.width, export.height, true)),
        );
        let view = View {
            outputs: &outputs,
            month: surface.month(),
            surfaces: surfaces_for(&self.params, sky.sun_direction),
            texture_resolution: self.renderer.texture_resolution(),
            drag: surface.drag(&self.params.camera, self.clock.elapsed()),
        };
        surface.want_tiles(&view, &mut self.renderer);
    }

    /// Bring the wanted set up to date with the outputs as they stand,
    /// without drawing: for renders planned while a cube is still on its
    /// way, whose tiles are read ahead of it.
    fn want_now(&mut self) {
        let sky = self.sky_state();
        self.want_tiles(&sky);
    }

    /// Returns whether a new frame was drawn. Emitting it is `tick`'s, so that
    /// a tick asks the preview target for its pixels once however it got here.
    fn render_if_dirty(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        let (outcome, _) = self.draw();
        self.dirty = false;
        if matches!(outcome, RenderOutcome::Rendered { first_frame: true }) {
            info!("first frame rendered");
        }
        if matches!(outcome, RenderOutcome::Rendered { .. }) {
            self.mark_busy();
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
        if let Some(surface) = &mut self.surface {
            surface.drain(&mut self.renderer);
        }
        let (outcome, sky) = self.draw();
        if matches!(outcome, RenderOutcome::Skipped) {
            // A skipped frame kept the sky the last render was drawn with, and
            // for a hidden window that can be hours old. A rendered one is
            // already holding this one.
            self.renderer.set_sky_state(sky);
        }
    }

    fn emit(&self, event: EngineEvent) {
        (self.on_event)(event);
    }
}

/// The mailbox the engine will use, with its slot count checked against the
/// textures this engine was configured with.
///
/// Checked rather than trusted, and before the device exists, because neither
/// way of getting it wrong is visible where it happens. Too few slots drops the
/// posts for the high ones, leaving those slots waiting for a load that was
/// thrown away; too many hands the consumer a slot index its own array does not
/// have, which is a panic in the middle of a session. Failing here means failing
/// before `ready` is sent, so `start` reports a thread that never got as far as
/// its adapter rather than handing back a handle to a thread that quietly died.
fn checked_mailbox(
    mailbox: Option<TextureMailbox>,
    slots: SlotLayout,
    paths: usize,
) -> TextureMailbox {
    if let Some(mailbox) = &mailbox {
        assert_eq!(
            mailbox.slot_count(),
            slots.count(),
            "the texture mailbox must have one slot per texture: the grid, \
             {paths} file-backed, and the cloud overlay"
        );
    }
    mailbox.unwrap_or_else(|| TextureMailbox::new(slots.count()))
}

/// One output of the wanted set: `framed` is the scene with the output's
/// framing applied, as the render takes it (`Framing::applied_to`), so the
/// tiles and the picture cannot disagree on what is in the frame.
fn tile_output(framed: &SceneParams, width: u32, height: u32, export: bool) -> Output {
    Output {
        camera: framed.camera,
        width,
        height,
        export,
    }
}

/// Start feeding the cube surface, when every face is there and there is a
/// cache directory to build its packs in.
fn start_surface(
    textures: crate::assets::cube_layout::CubeTextures,
    geometry: crate::assets::tiles::Geometry,
    cache_dir: Option<&std::path::PathBuf>,
    month: usize,
    now: Duration,
    wake: &crate::assets::cloud_fetcher::NotifyFn,
    tiles: &LoaderConfig,
) -> Option<SurfaceFeed> {
    if !textures.is_complete() {
        return None;
    }
    let Some(dir) = cache_dir else {
        warn!(
            "the cube faces are there but no cache directory is, so the tile packs cannot be built"
        );
        return None;
    };
    Some(SurfaceFeed::start(
        dir.clone(),
        textures,
        geometry,
        month,
        now,
        Arc::clone(wake),
        tiles,
    ))
}

/// Open the GPU and tell `start` what was opened, or why nothing was.
///
/// One report either way, before anything else can fail: a caller blocked on
/// `ready` gets an adapter it can name or an error it can put in front of the
/// user, and never silence.
fn open_gpu(
    force_software: bool,
    ready: &Sender<Result<AdapterReport, String>>,
) -> Result<crate::wgpu_init::WgpuContext, String> {
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
    Ok(gpu)
}

/// Resolve a requested MSAA count against the adapter and the tier, saying so
/// when the answer is not what was asked for.
///
/// The single place a sample count becomes real: a config file, a combo box
/// built against a different adapter, or a stale saved setting all funnel
/// through here rather than into `create_render_textures`, where an unsupported
/// count is a wgpu validation error that kills the engine thread.
///
/// `announced` is the request that has already been reported, so a setting the
/// user pushes again is logged once rather than on every arrival. `None` where
/// nothing has been reported yet, which is the engine's own construction.
fn resolve_and_warn(requested: u32, supported: &[u32], max: u32, announced: Option<u32>) -> u32 {
    let resolved = resolve_sample_count(requested, supported, max);
    if resolved != requested && announced != Some(requested) {
        warn!(
            requested,
            using = resolved,
            supported = ?supported,
            "MSAA sample count is not available, falling back"
        );
    }
    resolved
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
///
/// `Fast` and `Adaptive` are what `image`'s own extension-driven save used
/// before this went through one writer: they are that encoder's defaults, and
/// the name `CompressionType::Default` is a level rather than the default.
/// Named here so an export keeps producing the bytes it always has.
pub fn save_png(
    path: &std::path::Path,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<(), String> {
    crate::files::write_png(
        path,
        pixels,
        width,
        height,
        crate::files::CompressionType::Fast,
        crate::files::FilterType::Adaptive,
    )
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
