//! Feeding the cube surface to the renderer: the transcoder that builds the
//! packs, the month in force, the pause gate the engine holds while it is
//! busy, and the tile loader that refines the floors.
//!
//! The transcoder is the producer and this is its consumer, on the engine
//! thread: every tick drains the packs that landed, opens each for the tile
//! loader, which reads the tiles out of them as the frame wants them, and
//! hands the ones the frame draws, the mask, the night and the month in
//! force, to the renderer, which reads their floors out of them and uploads
//! them there and then. The other months' floors follow one a tick while the
//! engine is idle, and every floor stays resident, so a date in any month is
//! drawn from its own floor the moment it names it (plan decision 4).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tracing::{error, info, warn};

use crate::assets::cube_layout::CubeTextures;
use crate::assets::tiles::{
    Geometry, Pack, PackKind, Phase, TranscodeNotify, Transcoder, TranscoderConfig, pack_path,
};
use crate::renderer::residency::{Drag, Output};
use crate::renderer::tiles::CellLevels;
use crate::renderer::{Renderer, SurfaceLayer};
use crate::scene::camera::CameraParams;

use super::tile_loader::{DragWatch, LoaderConfig, TileLoader, TileReport, View};

/// How long after its last frame the engine counts as busy, which is how long
/// the transcoder's pause gate stays closed after one.
///
/// A drag or a slider sends a frame every tick, so the gate stays closed
/// through it and opens this long after it stops; a lone frame every two
/// minutes of live time closes it for this long and no more. Closing the gate
/// cancels a build of the rest of the year, which costs up to one month's
/// build each time, so the span is seconds rather than one tick.
const BUSY_AFTER_A_FRAME: Duration = Duration::from_secs(2);

/// The transcoder and what the engine has done with its output.
pub(super) struct SurfaceFeed {
    transcoder: Transcoder,
    tiles: TileLoader,
    drag: DragWatch,
    cache_dir: PathBuf,
    /// The month in force, January 0.
    month: usize,
    /// Whether the pause gate is closed.
    paused: bool,
    /// Until when the engine counts as busy, on the injected clock.
    busy_until: Duration,
    /// How many of the transcoder's failures have reached the renderer.
    failures_seen: usize,
    /// The worker has finished, and its status will not change again.
    settled: bool,
    /// Day packs of other months than the one in force that landed and whose
    /// floors are not resident yet, in the order they landed, which is the
    /// nearest month first. Each holds its index and an open file, not
    /// pixels; the engine thread is the consumer, and makes one floor
    /// resident a tick while the engine is idle, or one at once when its
    /// month comes into force.
    floors: VecDeque<Arc<Pack>>,
    /// Counts the times the tiles of the month in force or of the night
    /// could newly be read: the month in force changed, or its pack or the
    /// night's was opened.
    renewals: u64,
}

impl SurfaceFeed {
    /// Start the transcoder over `textures`, with `month` in force.
    ///
    /// The engine is busy until [`BUSY_AFTER_A_FRAME`] after `now`, since its
    /// first frames are on their way.
    ///
    /// `wake` is called on the transcoder's thread after every change of its
    /// status, and on a tile loader thread when a tile is waiting, so it must
    /// return at once and never panic; the engine's is a send on its own
    /// unbounded command channel.
    pub(super) fn start(
        cache_dir: PathBuf,
        textures: CubeTextures,
        geometry: Geometry,
        month: usize,
        now: Duration,
        wake: Arc<dyn Fn() + Send + Sync>,
        tiles: &LoaderConfig,
    ) -> Self {
        let tiles = TileLoader::start(geometry, tiles, Arc::clone(&wake));
        let mut config = TranscoderConfig::new(cache_dir.clone(), textures, month);
        config.geometry = geometry;
        let notify: TranscodeNotify = Arc::new(move |_| wake());
        config.notify = notify;
        info!(
            month = month + 1,
            "starting the transcoder for the cube surface"
        );
        // Closed from the start: the engine is busy until its first frames are
        // out, and the first-frame packs go through it anyway.
        let transcoder = Transcoder::start(config);
        transcoder.set_paused(true);
        Self {
            transcoder,
            tiles,
            drag: DragWatch::default(),
            cache_dir,
            month,
            paused: true,
            busy_until: now + BUSY_AFTER_A_FRAME,
            failures_seen: 0,
            settled: false,
            floors: VecDeque::new(),
            renewals: 0,
        }
    }

    /// Open every pack that landed since the last call for the tile loader
    /// and make the floors the frame draws resident, then, while the engine
    /// is idle, one floor of another month; hand every failure the
    /// transcoder reported to the renderer, and the tiles that were read
    /// since to the tile array. Returns whether what the frame draws changed.
    pub(super) fn drain(&mut self, renderer: &mut Renderer) -> bool {
        let mut drawn = false;
        for kind in self.transcoder.landed() {
            drawn |= self.land(kind, renderer);
        }
        if !self.paused
            && let Some(pack) = self.floors.pop_front()
        {
            drawn |= make_resident(&pack, renderer);
        }
        drawn |= self.tiles.drain(renderer);
        if !self.settled {
            let status = self.transcoder.status();
            for failure in &status.failed[self.failures_seen..] {
                warn!(pack = ?failure.kind, reason = %failure.reason, "a tile pack could not be built");
                renderer.mark_surface_failed(layer_of(failure.kind));
            }
            self.failures_seen = status.failed.len();
            if let Phase::Unavailable(why) = &status.phase {
                warn!(reason = %why, "the cube surface cannot be built, the globe keeps the grid");
                for kind in PackKind::all() {
                    renderer.mark_surface_failed(layer_of(kind));
                }
            }
            self.settled = status.is_settled();
        }
        drawn
    }

    /// The month in force, January 0.
    pub(super) fn month(&self) -> usize {
        self.month
    }

    /// Make `month` the month in force, when it is not already: its floor is
    /// drawn and the page table's day half names its tiles from now on. The
    /// floor is resident already once its pack has landed, or made so now if
    /// it was still waiting its turn; the transcoder builds the month next if
    /// it has not yet, and the floor drawn until now stays until it lands.
    /// Returns whether the month changed.
    pub(super) fn set_month(&mut self, month: usize, renderer: &mut Renderer) -> bool {
        if month == self.month {
            return false;
        }
        info!(
            from = self.month + 1,
            to = month + 1,
            "the month in force changed"
        );
        self.month = month;
        self.renewals += 1;
        self.transcoder.set_month(month);
        if let Some(at) = self
            .floors
            .iter()
            .position(|pack| pack.kind() == PackKind::Day(month))
            && let Some(pack) = self.floors.remove(at)
        {
            make_resident(&pack, renderer);
        }
        renderer.set_surface_month(month);
        true
    }

    /// The engine is drawing or has just drawn at `now`: close the pause gate
    /// and keep it closed for the busy span from here.
    pub(super) fn mark_busy(&mut self, now: Duration) {
        self.busy_until = now + BUSY_AFTER_A_FRAME;
        self.set_paused(true);
    }

    /// Open the pause gate when the busy span has run out at `now`.
    pub(super) fn relax(&mut self, now: Duration) {
        self.set_paused(now < self.busy_until);
    }

    /// Tell the transcoder only when the gate changes.
    fn set_paused(&mut self, paused: bool) {
        if paused != self.paused {
            self.paused = paused;
            self.transcoder.set_paused(paused);
        }
    }

    /// Compute the tiles the frame wants for `view`, if anything it depends
    /// on changed, and send the loader after the ones that are missing.
    pub(super) fn want_tiles(&mut self, view: &View<'_>, renderer: &mut Renderer) {
        self.tiles.want(view, renderer);
    }

    /// The drag in progress with the camera at `camera` at `now`, if one is.
    pub(super) fn drag(&mut self, camera: &CameraParams, now: Duration) -> Option<Drag> {
        self.drag.observe(camera, now)
    }

    /// A tick's work ended at `now`, the frame it drew and read back
    /// included, which is what a drag's pause counts from; the next tick's
    /// drains have the whole upload budget again.
    pub(super) fn end_tick(&mut self, now: Duration) {
        self.drag.drawn(now);
        self.tiles.end_tick();
    }

    /// End a drag the camera, at `camera`, has rested from by `now`. Returns
    /// whether one ended, which is when the tiles of the 1 px threshold are
    /// wanted again.
    pub(super) fn settle_drag(&mut self, camera: &CameraParams, now: Duration) -> bool {
        self.drag.settle(camera, now)
    }

    /// The cap the page table takes to draw `output`.
    pub(super) fn cap_for(&self, output: &Output) -> CellLevels {
        self.tiles.cap_for(output)
    }

    /// How many tiles `output` alone needs in view that are on their way;
    /// `None` when the set in force was not computed for it.
    pub(super) fn missing_for(&self, output: &Output, renderer: &Renderer) -> Option<usize> {
        self.tiles.missing_for(output, renderer)
    }

    /// How many times a pack has been opened for its tiles.
    pub(super) fn packs_opened(&self) -> u64 {
        self.tiles.packs_opened()
    }

    /// How many times the tiles of the month in force or of the night could
    /// newly be read: a change of the month in force, or its pack or the
    /// night's opened. A publish's tile wait starts over on each.
    pub(super) fn renewals(&self) -> u64 {
        self.renewals
    }

    /// Whether every tile the frame needs in view at the 1 px threshold is
    /// resident or failed.
    pub(super) fn tiles_complete(&self, renderer: &Renderer) -> bool {
        self.tiles.complete(renderer)
    }

    /// Let go of every tile, for a change of the resolution setting.
    pub(super) fn purge_tiles(&mut self, renderer: &mut Renderer) {
        self.tiles.purge(renderer);
    }

    pub(super) fn tile_report(&self, renderer: &Renderer) -> TileReport {
        self.tiles.report(renderer)
    }

    /// Open the pack `kind` that landed for the tile loader, and make its
    /// cube resident if the frame draws it; a day pack of another month waits
    /// in [`Self::floors`]. Returns whether what the frame draws changed.
    fn land(&mut self, kind: PackKind, renderer: &mut Renderer) -> bool {
        let path = pack_path(&self.cache_dir, kind);
        let pack = match Pack::open(&path) {
            Ok(pack) => Arc::new(pack),
            Err(e) => {
                error!(path = %path.display(), error = %e, "a tile pack could not be opened");
                renderer.mark_surface_failed(layer_of(kind));
                return false;
            }
        };
        if kind != PackKind::Mask {
            self.tiles.open(Arc::clone(&pack));
        }
        if kind == PackKind::Night || kind == PackKind::Day(self.month) {
            self.renewals += 1;
        }
        if matches!(kind, PackKind::Day(month) if month != self.month) {
            self.floors.push_back(pack);
            return false;
        }
        make_resident(&pack, renderer)
    }
}

/// Make the cube of `pack` resident. Returns whether what the frame draws
/// changed.
fn make_resident(pack: &Pack, renderer: &mut Renderer) -> bool {
    let layer = layer_of(pack.kind());
    match renderer.install_surface(layer, pack) {
        Ok(drawn) => {
            info!(?layer, "a surface cube is resident");
            drawn
        }
        Err(e) => {
            error!(?layer, error = %e, "a surface cube could not be made resident");
            renderer.mark_surface_failed(layer);
            false
        }
    }
}

/// The cube pack `kind` makes.
fn layer_of(kind: PackKind) -> SurfaceLayer {
    match kind {
        PackKind::Day(month) => SurfaceLayer::Day(month),
        PackKind::Night => SurfaceLayer::Night,
        PackKind::Mask => SurfaceLayer::Mask,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::tiles::FIXTURE;
    use crate::test_support::{ScratchDir, write_cube_fixture};

    const JUST: Duration = Duration::from_millis(1);

    fn feed(name: &str) -> (ScratchDir, SurfaceFeed) {
        let dir = ScratchDir::new(name);
        write_cube_fixture(&dir.join("textures"));
        let textures = CubeTextures::resolve(&dir.join("textures"));
        let feed = SurfaceFeed::start(
            dir.join("cache"),
            textures,
            FIXTURE,
            0,
            Duration::ZERO,
            Arc::new(|| {}),
            &LoaderConfig {
                decode: false,
                anisotropy: 8,
                workers: 1,
                gate: super::super::tile_loader::TileGate::default(),
            },
        );
        (dir, feed)
    }

    #[test]
    fn the_gate_starts_closed_and_opens_when_the_busy_span_has_run_out() {
        let (_dir, mut feed) = feed("surface_gate_opens");
        assert!(feed.paused);

        feed.relax(BUSY_AFTER_A_FRAME.saturating_sub(JUST));
        assert!(feed.paused, "still inside the first span");
        feed.relax(BUSY_AFTER_A_FRAME);
        assert!(!feed.paused);
    }

    #[test]
    fn a_frame_closes_an_open_gate_before_the_next_tick() {
        let (_dir, mut feed) = feed("surface_gate_closes");
        let idle = BUSY_AFTER_A_FRAME * 5;
        feed.relax(idle);
        assert!(!feed.paused);

        feed.mark_busy(idle);
        assert!(
            feed.paused,
            "closed by the frame itself, not by a later tick"
        );
        feed.relax((idle + BUSY_AFTER_A_FRAME).saturating_sub(JUST));
        assert!(feed.paused);
        feed.relax(idle + BUSY_AFTER_A_FRAME);
        assert!(!feed.paused);
    }
}
