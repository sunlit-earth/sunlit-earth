//! The engine: one thread that owns the GPU device, the renderer, the asset
//! mailbox, and the schedule.
//!
//! Clients (the Slint shell, the `render` subcommand, tests) send commands and
//! receive events. Nothing the engine does depends on a window existing, which
//! is the whole point: hiding the settings window removes a client, it does not
//! half-suspend the machinery.

pub mod clock;
mod cloud_worker;
pub mod excursion;
mod handle;
mod loading;
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

use crate::assets::cloud_fetcher::CloudSlots;
use crate::assets::mailbox::TextureMailbox;
use crate::assets::tiles::TranscoderConfig;
use crate::config::QualityTier;
use crate::params::SceneParams;
use crate::renderer::residency::{Output, surfaces_for};
use crate::renderer::{
    RenderOutcome, Renderer, RendererConfig, SlotLayout, SurfaceFormats, resolve_sample_count,
    surface_sampler_descriptor,
};
use crate::scene::sky::{self, SkyState};

use clock::Clock;
use cloud_worker::{CloudWorker, FirstAnswer, spawn_cloud_worker};
use excursion::{Excursion, ExcursionCommand};
use handle::AdapterReport;
pub use handle::{EngineConfig, EngineHandle, start};
pub use loading::TILES_NAMED_AFTER;
pub use protocol::{EngineCommand, EngineEvent};
use schedule::Schedule;
use surface::SurfaceFeed;
use tile_loader::{LoaderConfig, View, Watched};
pub use tile_loader::{TileGate, TileReport};
use wallpaper_sink::WallpaperSink;

/// How long the loop blocks on the command channel before re-checking the
/// schedule. Short enough that a real-clock deadline is never missed by more
/// than this, cheap enough to ignore: the wake does nothing when nothing is due.
const TICK: Duration = Duration::from_millis(50);

/// The longest the loop waits while an excursion runs and the preview is
/// shown: about 30 frames a second where the adapter keeps up. The time comes
/// from the clock, so a slower adapter drops frames and keeps the speed.
const EXCURSION_TICK: Duration = Duration::from_millis(33);

/// How often decoded textures are uploaded to the GPU. Unconditional: this is
/// what keeps cloud updates flowing while no window is visible.
const DRAIN_INTERVAL: Duration = Duration::from_secs(5);

/// How often the sky state (the sun, the sky rotation, the planets) is
/// recomputed when rendering live time, unless [`sky_interval`] says the
/// preview needs it sooner.
const SKY_INTERVAL: Duration = Duration::from_mins(2);

/// The shortest interval [`sky_interval`] gives the sky schedule: the floor
/// under how often a shown preview of a Sun-fixed camera is redrawn to keep up
/// with the Earth's turn, which at the closest zoom moves a pixel every two
/// seconds. `docs/rendering.md` has the measurement behind it.
const SKY_FLOOR: Duration = Duration::from_secs(5);

/// The Earth's turn against the Sun, in radians a second: one turn a solar
/// day.
const EARTH_TURN: f32 = std::f32::consts::TAU / 86_400.0;

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
    /// The sky the wanted set in force was computed under. The outputs an
    /// export waits on are resolved under it too: the set answers for the
    /// cameras it was computed for, and a Sun-fixed camera resolved at another
    /// instant is not one of them.
    wanted_sky: Option<SkyState>,
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
    /// The preview's time while it is away from the setting, running or
    /// paused.
    excursion: Option<Excursion>,
    /// The instant the last draw was drawn at, which the preview frame read
    /// back from it shows.
    drawn_instant: time::OffsetDateTime,
    /// Set when something happened that the next render must pick up.
    dirty: bool,
    textures_ready: bool,
    /// Whether `CloudsSettled` has gone out, which it does once.
    clouds_settled: bool,
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
            build_gate,
            every_floor,
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
        let month = crate::scene::month::month_in_force(&params.time, clock.now_utc());
        let drawn_instant = params.time.instant(clock.now_utc());
        let cpu_adapter = gpu.device_type == wgpu::DeviceType::Cpu;
        let formats = SurfaceFormats::for_adapter(
            gpu.device
                .features()
                .contains(wgpu::Features::TEXTURE_COMPRESSION_BC),
            cpu_adapter,
        );
        let mut surface = start_surface(
            cache_dir.as_ref(),
            TranscoderConfig {
                geometry: tile_geometry,
                gate: build_gate,
                ..TranscoderConfig::new(std::path::PathBuf::new(), cube_textures, month)
            },
            now,
            &notify,
            &LoaderConfig {
                decode: formats.color == wgpu::TextureFormat::Rgba8Unorm,
                anisotropy: surface_sampler_descriptor(cpu_adapter).anisotropy_clamp,
                workers: crate::assets::tiles::default_threads(),
                gate: tile_gate,
            },
        );
        if let Some(surface) = &mut surface {
            surface.keep_every_floor(every_floor.unwrap_or(!cpu_adapter));
        }

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
                CloudSlots {
                    map: slots.clouds(),
                    background: slots.cloud_background(),
                },
                params.clouds_enabled,
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
            wanted_sky: None,
            publish_asked: false,
            display_mode,
            anchor_monitor,
            display_recheck: None,
            known_monitors: None,
            published: false,
            excursion: None,
            drawn_instant,
            dirty: true,
            textures_ready: false,
            clouds_settled: false,
            memory_dumped: false,
            last_status: String::new(),
            drain: Schedule::new(DRAIN_INTERVAL, now),
            sky: Schedule::new(sky_interval(&params, false, height, preview_enabled), now),
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
            match self.rx.recv_timeout(self.wait()) {
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
                if params.time != self.params.time {
                    self.set_excursion(None);
                }
                self.params = *params;
                self.resolve_requested_sample_count();
                self.dirty = true;
                self.retime_sky();
            }
            EngineCommand::SetPreviewSize(w, h) => {
                let (qw, qh) = preview_target_size((w, h), self.quality);
                if (qw, qh) != self.renderer.size() {
                    self.renderer.resize(qw, qh);
                    self.dirty = true;
                }
                self.retime_sky();
            }
            EngineCommand::SetPreviewEnabled(enabled) => {
                if enabled && !self.preview.enabled {
                    self.preview.owed = true;
                }
                if !enabled {
                    self.renderer.release_preview_readback();
                    if let Some(mut excursion) = self.excursion.filter(Excursion::is_running) {
                        excursion.pause(self.clock.elapsed());
                        self.set_excursion(Some(excursion));
                    }
                }
                self.preview.enabled = enabled;
                self.retime_sky();
            }
            EngineCommand::RenderWallpaperNow => self.publish_asked = true,
            EngineCommand::Excursion(command) => self.excursion_command(command),
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
            EngineCommand::ReportMemory { reply, settled } => {
                if settled {
                    self.renderer.flush_and_wait();
                }
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

    /// How long the loop waits for a command: a frame's worth while an
    /// excursion runs and the preview is shown, [`TICK`] otherwise.
    fn wait(&self) -> Duration {
        if self.preview.enabled && self.excursion.as_ref().is_some_and(Excursion::is_running) {
            EXCURSION_TICK
        } else {
            TICK
        }
    }

    /// Apply a client's excursion command.
    fn excursion_command(&mut self, command: ExcursionCommand) {
        let now = self.clock.elapsed();
        let setting = self.params.time.instant(self.clock.now_utc());
        let next = excursion::apply(self.excursion, command, now, setting);
        self.set_excursion(next);
    }

    /// Replace the excursion, and when its state changed tell clients and
    /// draw the preview again.
    fn set_excursion(&mut self, next: Option<Excursion>) {
        let before = self.excursion.map(|excursion| excursion.state());
        let moved = self.excursion != next;
        self.excursion = next;
        let after = next.map(|excursion| excursion.state());
        if before != after {
            self.emit(EngineEvent::Excursion(after));
        }
        if moved {
            self.dirty = true;
            self.retime_sky();
        }
    }

    /// The instant the preview shows now: the excursion's while there is
    /// one, the time setting's otherwise.
    fn preview_instant(&self) -> time::OffsetDateTime {
        match &self.excursion {
            Some(excursion) => excursion.instant_at(self.clock.elapsed()),
            None => self.params.time.instant(self.clock.now_utc()),
        }
    }

    /// The instant every export draws at: the time setting's.
    fn setting_instant(&self) -> time::OffsetDateTime {
        self.params.time.instant(self.clock.now_utc())
    }

    /// The instant the month in force follows in a draw at `at`: the time
    /// setting's while a wallpaper or an export waits for its tiles, since
    /// they are of the setting's month and the set has one month, and `at`
    /// otherwise. An excursion in another month is drawn from the setting's
    /// month meanwhile, rather than the two months taking turns in force and
    /// each turn starting the tile wait over.
    fn month_instant(&self, at: time::OffsetDateTime) -> time::OffsetDateTime {
        if self.outputs_waiting() {
            self.setting_instant()
        } else {
            at
        }
    }

    /// Whether a wallpaper or an export waits for its tiles.
    fn outputs_waiting(&self) -> bool {
        !self.wallpaper_shots.is_empty() || !self.exports.is_empty()
    }

    /// Point the renderer at another texture resolution's set of assets.
    fn set_texture_resolution(&mut self, width: u32) {
        if self.renderer.set_texture_resolution(width) {
            info!(texture_resolution = width, "surface texture resolution");
            // The tiles of the old cap go, the floors stay, and the next draw
            // asks for the tiles the new one allows.
            if let Some(surface) = &mut self.surface {
                surface.purge_tiles(&mut self.renderer);
                self.restart_export_waits();
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

    /// Give the sky schedule the interval the scene and the preview now call
    /// for, counting it from now when that is a change.
    fn retime_sky(&mut self) {
        let interval = sky_interval(
            &self.params,
            self.excursion.is_some(),
            self.renderer.size().1,
            self.preview.enabled,
        );
        if interval != self.sky.interval {
            debug!(
                interval_secs = interval.as_secs_f32(),
                "the sky schedule's interval changed"
            );
            self.sky.set_interval(interval, self.clock.elapsed());
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
        self.note_clouds();

        // Every tick rather than on the drain's schedule: a pack that landed is
        // one cheap check away, and the first frame of a first run waits on it.
        // A pack opened for its tiles that changes nothing drawn, the month
        // ahead's, has its tiles asked for without a frame.
        let mut opened = false;
        if let Some(surface) = &mut self.surface {
            let before = surface.packs_opened();
            if surface.drain(&mut self.renderer) {
                self.dirty = true;
            }
            opened = surface.packs_opened() != before;
        }
        if opened && !self.dirty {
            self.want_now();
        }
        // A pack that fails changes nothing drawn but can complete what the
        // latch waits for, and the latch is judged after a draw.
        if !self.textures_ready && !self.dirty && self.textures_ready() {
            self.dirty = true;
        }

        if let Some(cloud) = &mut self.cloud {
            // A request the worker gave up because the layer went off is owed
            // again, and goes out when it is switched back on.
            cloud.set_enabled(self.params.clouds_enabled);
            if cloud.take_abandoned() {
                cloud.owed = true;
            }
            if cloud.schedule.due(now) {
                cloud.owed = true;
            }
            // Retry on the next tick rather than waiting out another whole
            // interval when the worker was still busy with the previous poll.
            // While the layer is switched off a poll that falls due stays
            // owed, so switching it on sends one at once.
            if cloud.owed && self.params.clouds_enabled && cloud.request() {
                cloud.owed = false;
            }
        }

        // Live time keeps moving even when nothing else changes, so the
        // terminator and the sky have to be recomputed on a schedule of their
        // own.
        if self.sky.due(now) && self.params.time.is_live() && self.excursion.is_none() {
            self.dirty = true;
        }

        // A running excursion moves the preview on every tick it is shown,
        // and stops at the edge of the range.
        if let Some(mut excursion) = self.excursion.filter(Excursion::is_running) {
            if excursion.stop_at_the_edge(now) {
                self.set_excursion(Some(excursion));
            }
            if self.preview.enabled {
                self.dirty = true;
            }
        }

        if let Some(metrics) = &mut self.metrics
            && metrics.due(now)
        {
            crate::memory::record_metrics_sample(self.renderer.texture_resolution());
        }

        if self.surface.is_some() {
            let camera = self.watched_camera();
            if let Some(surface) = &mut self.surface
                && surface.settle_drag(&camera, now)
            {
                self.dirty = true;
            }
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
            && !self.renderer.textures_pending(&self.params)
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
        self.note_status();
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

    /// The astronomy state the preview is drawn under now.
    fn sky_state(&self) -> SkyState {
        sky::compute_sky_state_for(self.preview_instant())
    }

    /// Derive the month in force from the date, as the sky state is, and move
    /// the surface to it when it changed.
    ///
    /// Outside the digest like the sun direction: the month is not a
    /// parameter anyone sets but a consequence of the date. A floor that
    /// is not resident yet reopens the readiness latch, so clients hear
    /// `TexturesReady` again when it lands. A change marks the frame dirty
    /// whichever path saw it first, a draw or a publish, so the next tick
    /// draws the new month and asks for its tiles; a publish that is owed
    /// and still waiting returns without drawing, and nothing else would.
    fn sync_month(&mut self, at: time::OffsetDateTime) {
        // The live clock only goes forward; a fixed time and an excursion can
        // go either way.
        let both_ways = !self.params.time.is_live() || self.excursion.is_some();
        let Some(surface) = &mut self.surface else {
            return;
        };
        let scene_time = crate::scene::scene_time::SceneTime::fixed(at);
        let month = crate::scene::month::month_in_force(&scene_time, at);
        let ahead = crate::scene::month::month_across_at(at, both_ways);
        if surface.set_month(month, ahead, &mut self.renderer) {
            self.dirty = true;
        }
        if !self.renderer.textures_ready(&self.params) {
            self.textures_ready = false;
        }
    }

    /// Whether everything the frame needs is resident: the cubes the mode
    /// needs, and every tile it wants in view at the 1 px threshold, a failed
    /// one counting as there (plan decision 10).
    fn textures_ready(&self) -> bool {
        self.renderer.textures_ready(&self.params)
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
    ///
    /// The wanted set is computed under the preview's sky whichever is drawn,
    /// so an export during an excursion neither moves the preview's output
    /// in it nor shows the motion observer the setting's camera and Sun
    /// between two of the excursion's. Its month and Blend mode's Sun are the
    /// setting's while outputs wait, which `month_instant` and `want_tiles`
    /// decide.
    fn draw(&mut self, at: time::OffsetDateTime, preview: bool) -> (RenderOutcome, SkyState) {
        self.sync_month(self.month_instant(at));
        self.mark_busy();
        let sky = sky::compute_sky_state_for(at);
        if preview || self.excursion.is_none() {
            self.want_tiles(&sky);
        } else {
            self.want_tiles(&self.sky_state());
        }
        let outcome = self.renderer.render(&self.params, &sky);
        self.drawn_instant = at;
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
        self.note_status();
        let ready = self.textures_ready();
        if ready && !self.textures_ready {
            debug!("textures ready");
            self.dump_memory_report();
            self.emit(EngineEvent::TexturesReady);
        }
        self.textures_ready = ready;
    }

    /// Tell clients what the loading line says now, when it changed: the pack
    /// the transcoder is preparing, and otherwise the surfaces the frame
    /// waits for, a cube the mode needs or, once they have been on their way
    /// for a moment, the tiles in view.
    ///
    /// After every draw and at the end of every tick, since the transcoder
    /// prepares the rest of the year while nothing is drawn.
    fn note_status(&mut self) {
        let (mut day, mut night) = self.renderer.cubes_waiting(&self.params);
        let mut preparing = None;
        if let Some(surface) = &mut self.surface {
            preparing = surface.preparing();
            let tiles = surface.tiles_waiting(&self.renderer, self.clock.elapsed());
            day |= tiles.0;
            night |= tiles.1;
        }
        let status = loading::text(preparing, day, night);
        if status != self.last_status {
            self.last_status.clone_from(&status);
            self.emit(EngineEvent::Status(status));
        }
    }

    /// Ask the tile loader for the tiles the preview wants under `sky`, and
    /// every export waiting for its tiles, before the frame is drawn, so the
    /// page table it draws with is held to the preview's cap.
    ///
    /// The preview is resolved under the sky of the instant it shows, and
    /// the exports under the time setting's, which is the one they are drawn
    /// at; the two are one sky but during an excursion. While an excursion
    /// runs, the motion observer watches the Sun as well as the camera, and
    /// the camera the preview shows rather than the one the settings hold, so
    /// in the Sun-fixed mode the read-ahead follows the Earth's turn; outside
    /// one it watches the held camera alone, as a drag always was. A run
    /// starting or stopping changes what is watched, which is not a move.
    ///
    /// In Blend mode the set has one Sun to choose the day and the night by:
    /// the setting's while a wallpaper or an export waits, as the month in
    /// force is (plan departure 9), and the preview's otherwise.
    fn want_tiles(&mut self, sky: &SkyState) {
        let exports_sky = if self.excursion.is_some() {
            sky::compute_sky_state_for(self.setting_instant())
        } else {
            sky.clone()
        };
        let sun = if self.outputs_waiting() {
            exports_sky.sun_direction
        } else {
            sky.sun_direction
        };
        let running = self.excursion.as_ref().is_some_and(Excursion::is_running);
        let Some(surface) = &mut self.surface else {
            return;
        };
        let (width, height) = self.renderer.size();
        let mut outputs = vec![tile_output(&self.params, sky, width, height, false)];
        outputs.extend(self.wallpaper_shots.iter().map(|shot| {
            tile_output(
                &shot.framing.applied_to(&self.params),
                &exports_sky,
                shot.width,
                shot.height,
                true,
            )
        }));
        outputs.extend(self.exports.iter().map(|export| {
            tile_output(
                &self.params,
                &exports_sky,
                export.width,
                export.height,
                true,
            )
        }));
        let view = View {
            outputs: &outputs,
            month: surface.month(),
            ahead: surface.ahead(),
            surfaces: surfaces_for(&self.params, sun),
            texture_resolution: self.renderer.texture_resolution(),
            drag: surface.drag(
                &if running {
                    self.params.camera_at(sky)
                } else {
                    self.params.held_camera()
                },
                // Outside a run the Sun's motion is the live clock's, which the
                // read-ahead has never treated as a drag.
                if running {
                    sky.sun_direction
                } else {
                    glam::Vec3::ZERO
                },
                if running { Watched::Run } else { Watched::Held },
                self.clock.elapsed(),
            ),
        };
        surface.want_tiles(&view, &mut self.renderer);
        self.wanted_sky = Some(exports_sky);
    }

    /// Bring the wanted set up to date with the outputs as they stand,
    /// without drawing: for renders planned while a cube is still on its
    /// way, whose tiles are read ahead of it.
    fn want_now(&mut self) {
        let sky = self.sky_state();
        self.want_tiles(&sky);
    }

    /// The camera the motion observer last watched, for deciding whether it
    /// has rested: the one the preview shows while an excursion runs.
    fn watched_camera(&self) -> crate::scene::camera::CameraParams {
        if self.excursion.as_ref().is_some_and(Excursion::is_running) {
            self.params.camera_at(&self.sky_state())
        } else {
            self.params.held_camera()
        }
    }

    /// Returns whether a new frame was drawn. Emitting it is `tick`'s, so that
    /// a tick asks the preview target for its pixels once however it got here.
    fn render_if_dirty(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        let (outcome, _) = self.draw(self.preview_instant(), true);
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
            instant: self.drawn_instant,
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
        self.note_clouds();
        if let Some(surface) = &mut self.surface {
            surface.drain(&mut self.renderer);
        }
        let (outcome, sky) = self.draw(self.setting_instant(), false);
        if self.excursion.is_some() {
            // The preview target now holds the setting's frame, and the
            // preview has to go back to the excursion's.
            self.dirty = true;
        }
        if matches!(outcome, RenderOutcome::Skipped) {
            // A skipped frame kept the sky the last render was drawn with, and
            // for a hidden window that can be hours old. A rendered one is
            // already holding this one.
            self.renderer.set_sky_state(sky);
        }
    }

    /// Tell clients, once, what the cloud layer's first answer was: drawn when
    /// the map and its background are uploaded, not when the worker's first
    /// attempt had nothing to show.
    ///
    /// After every drain, and on every tick, since an answer of nothing posts
    /// nothing and so wakes nobody.
    fn note_clouds(&mut self) {
        if self.clouds_settled {
            return;
        }
        let Some(cloud) = &self.cloud else {
            return;
        };
        let drawn = if self.renderer.clouds_resident() {
            true
        } else if cloud.first_answer() == FirstAnswer::Unavailable {
            false
        } else {
            return;
        };
        self.clouds_settled = true;
        if drawn {
            debug!("the cloud layer is resident");
        } else {
            debug!("the cloud layer has nothing to show yet");
        }
        self.emit(EngineEvent::CloudsSettled { drawn });
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
             {paths} file-backed, the cloud overlay and its background"
        );
    }
    mailbox.unwrap_or_else(|| TextureMailbox::new(slots.count()))
}

/// One output of the wanted set: `framed` is the scene with the output's
/// framing applied, as the render takes it (`Framing::applied_to`), and its
/// camera is resolved under `sky`, the sky the output is drawn under, so the
/// tiles and the picture cannot disagree on what is in the frame.
fn tile_output(
    framed: &SceneParams,
    sky: &SkyState,
    width: u32,
    height: u32,
    export: bool,
) -> Output {
    Output {
        camera: framed.camera_at(sky),
        width,
        height,
        export,
    }
}

/// Start feeding the cube surface, when every face is there and there is a
/// cache directory to build its packs in.
fn start_surface(
    cache_dir: Option<&std::path::PathBuf>,
    mut transcoder: TranscoderConfig,
    now: Duration,
    wake: &crate::assets::cloud_fetcher::NotifyFn,
    tiles: &LoaderConfig,
) -> Option<SurfaceFeed> {
    if !transcoder.textures.is_complete() {
        return None;
    }
    let Some(dir) = cache_dir else {
        warn!(
            "the cube faces are there but no cache directory is, so the tile packs cannot be built"
        );
        return None;
    };
    transcoder.cache_dir.clone_from(dir);
    Some(SurfaceFeed::start(transcoder, now, Arc::clone(wake), tiles))
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

/// Clamp a requested preview size to the tier's cap, keeping the aspect ratio
/// as closely as whole pixels allow, and never below one pixel either way.
fn preview_target_size(requested: (u32, u32), quality: QualityTier) -> (u32, u32) {
    let (mut w, mut h) = requested;
    let max_w = quality.max_preview_width();
    if w > max_w {
        let scaled = (u64::from(h) * u64::from(max_w) + u64::from(w) / 2) / u64::from(w);
        h = scaled.try_into().unwrap_or(u32::MAX);
        w = max_w;
    }
    (w.max(1), h.max(1))
}

/// How often the sky schedule redraws a scene drawn at live time, for a
/// preview `preview_height` pixels tall that is shown or not.
///
/// [`SKY_INTERVAL`], except for a Sun-fixed camera under live time with the
/// preview shown: the surface beneath that camera crosses the frame at about
/// `focal * EARTH_TURN / (distance - 1)` pixels a second, so the interval is
/// the time it takes to move one pixel, clamped between [`SKY_FLOOR`] and
/// [`SKY_INTERVAL`]. A hidden preview has nobody to show a frame to, and an
/// Earth-fixed camera or an overridden instant does not move with the turn.
fn sky_interval(
    params: &SceneParams,
    excursion: bool,
    preview_height: u32,
    shown: bool,
) -> Duration {
    if !shown || !params.sun_fixed || !params.time.is_live() || excursion {
        return SKY_INTERVAL;
    }
    let camera = &params.camera;
    let distance = crate::scene::camera::zoom_to_distance(camera.zoom);
    #[expect(
        clippy::cast_precision_loss,
        reason = "a preview's height is far inside the f32 mantissa"
    )]
    let focal = preview_height as f32 / (2.0 * (camera.fov_deg.to_radians() / 2.0).tan());
    let pixels_a_second = focal * EARTH_TURN / (distance - 1.0);
    Duration::try_from_secs_f32(pixels_a_second.recip())
        .map_or(SKY_INTERVAL, |pixel| pixel.clamp(SKY_FLOOR, SKY_INTERVAL))
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
    fn preview_size_below_the_cap_comes_back_as_asked() {
        for size in [(1, 1), (300, 200), (1023, 641), (1280, 777)] {
            assert_eq!(preview_target_size(size, QualityTier::Low), size);
        }
        assert_eq!(
            preview_target_size((3841, 2161), QualityTier::High),
            (3841, 2161)
        );
    }

    #[test]
    fn preview_size_above_the_cap_keeps_the_aspect_at_the_caps_width() {
        for tier in [QualityTier::Low, QualityTier::Medium] {
            let cap = tier.max_preview_width();
            for (rw, rh) in [(3840, 2160), (2561, 1439), (5000, 917), (cap + 1, 3)] {
                let (w, h) = preview_target_size((rw, rh), tier);
                assert_eq!(w, cap, "{rw}x{rh} at {tier:?}");
                let ideal = f64::from(rh) * f64::from(cap) / f64::from(rw);
                assert!(
                    (f64::from(h) - ideal).abs() <= 0.5,
                    "{rw}x{rh} at {tier:?} came back {w}x{h}, ideal height {ideal}"
                );
            }
        }
    }

    #[test]
    fn preview_size_is_never_zero() {
        for tier in [QualityTier::Low, QualityTier::Medium, QualityTier::High] {
            assert_eq!(preview_target_size((0, 0), tier), (1, 1));
            assert_eq!(preview_target_size((0, 300), tier), (1, 300));
            assert_eq!(preview_target_size((300, 0), tier), (300, 1));
        }
        assert_eq!(preview_target_size((100_000, 1), QualityTier::Low).1, 1);
    }

    #[test]
    fn preview_size_cap_grows_with_the_tier() {
        let low = preview_target_size((3840, 2160), QualityTier::Low);
        let medium = preview_target_size((3840, 2160), QualityTier::Medium);
        let high = preview_target_size((3840, 2160), QualityTier::High);
        assert!(low.0 < medium.0);
        assert!(medium.0 < high.0);
        assert_eq!(high, (3840, 2160));
    }

    fn sun_fixed_live(zoom: f32) -> SceneParams {
        let mut params = SceneParams {
            sun_fixed: true,
            ..SceneParams::default()
        };
        params.camera.zoom = zoom;
        params.time = crate::scene::scene_time::SceneTime::Live;
        params
    }

    /// One pixel of the turn at the camera's own lens and distance, by the
    /// geometry: the point beneath the camera moves `EARTH_TURN` radii a
    /// second at `distance - 1` from the eye.
    #[test]
    fn a_shown_sun_fixed_preview_redraws_once_a_pixel_of_the_turn() {
        let params = sun_fixed_live(crate::scene::camera::distance_to_zoom(8.0));
        let height = 1080;
        let focal = 1080.0 / (2.0 * (params.camera.fov_deg.to_radians() / 2.0).tan());
        let expected = 7.0 / (focal * EARTH_TURN);
        let interval = sky_interval(&params, false, height, true);
        approx::assert_relative_eq!(interval.as_secs_f32(), expected, max_relative = 1e-3);
        assert!(SKY_FLOOR < interval && interval < SKY_INTERVAL);
    }

    /// A closer camera and a taller preview both move the surface faster
    /// across the frame, down to the floor and never below it.
    #[test]
    fn the_interval_shortens_with_the_zoom_and_the_preview_down_to_the_floor() {
        let far = sky_interval(&sun_fixed_live(0.5), false, 720, true);
        let near = sky_interval(&sun_fixed_live(0.3), false, 720, true);
        let taller = sky_interval(&sun_fixed_live(0.5), false, 1440, true);
        assert!(near < far, "{near:?} against {far:?}");
        assert!(taller < far, "{taller:?} against {far:?}");
        assert_eq!(
            sky_interval(&sun_fixed_live(0.0), false, 2160, true),
            SKY_FLOOR
        );
        assert_eq!(
            sky_interval(&sun_fixed_live(1.0), false, 64, true),
            SKY_INTERVAL
        );
    }

    /// Hidden, Earth-fixed, or at an overridden instant, the picture has no
    /// reason to be redrawn more often than the Sun and the stars need.
    #[test]
    fn only_a_shown_sun_fixed_camera_at_live_time_shortens_the_interval() {
        let live = sun_fixed_live(0.0);
        assert!(sky_interval(&live, false, 1080, true) < SKY_INTERVAL);
        assert_eq!(sky_interval(&live, false, 1080, false), SKY_INTERVAL);
        let earth = SceneParams {
            sun_fixed: false,
            ..live
        };
        assert_eq!(sky_interval(&earth, false, 1080, true), SKY_INTERVAL);
        let mut overridden = live;
        overridden.time =
            crate::scene::scene_time::SceneTime::fixed(time::OffsetDateTime::UNIX_EPOCH);
        assert_eq!(sky_interval(&overridden, false, 1080, true), SKY_INTERVAL);
        assert_eq!(
            sky_interval(&live, true, 1080, true),
            SKY_INTERVAL,
            "an excursion counts as a fixed time"
        );
        assert_eq!(sky_interval(&live, false, 0, true), SKY_INTERVAL);
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
