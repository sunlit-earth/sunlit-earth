//! Feeding the cube surface to the renderer: the transcoder that builds the
//! packs, the month in force, the pause gate the engine holds while it is
//! busy, and the tile loader that refines the floors.
//!
//! The transcoder is the producer and this is its consumer, on the engine
//! thread: every tick drains the packs that landed and hands the ones the
//! month in force needs to the renderer, which reads the floors and the mask
//! out of them and uploads them there and then, and to the tile loader, which
//! reads the tiles out of them as the frame wants them. The rest of the year
//! stays on disk until a month needs it.

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
        }
    }

    /// Hand every pack that landed since the last call to the renderer and
    /// the tile loader, every failure the transcoder reported to the
    /// renderer, and the tiles that were read since to the tile array.
    /// Returns whether a cube or a tile became resident.
    pub(super) fn drain(&mut self, renderer: &mut Renderer) -> bool {
        let mut installed = false;
        for kind in self.transcoder.landed() {
            installed |= self.install(kind, renderer);
        }
        installed |= self.tiles.drain(renderer);
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
        installed
    }

    /// The month in force, January 0.
    pub(super) fn month(&self) -> usize {
        self.month
    }

    /// Make `month` the month in force, when it is not already. The
    /// transcoder builds it next if it has not yet, and its floor is made
    /// resident now if its pack is ready. Returns whether a cube became
    /// resident.
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
        renderer.set_surface_month(month);
        self.transcoder.set_month(month);
        self.transcoder.status().is_ready(PackKind::Day(month))
            && self.install(PackKind::Day(month), renderer)
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

    /// End a drag the camera has rested from by `now`. Returns whether one
    /// ended, which is when the tiles of the 1 px threshold are wanted again.
    pub(super) fn settle_drag(&mut self, now: Duration) -> bool {
        self.drag.settle(now)
    }

    /// The cap the page table takes to draw `output`.
    pub(super) fn cap_for(&self, output: &Output) -> CellLevels {
        self.tiles.cap_for(output)
    }

    /// Whether a tile the frame needs in view is on its way.
    pub(super) fn tiles_pending(&self, renderer: &Renderer) -> bool {
        self.tiles.missing(renderer) > 0
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

    /// Make the cube of pack `kind` resident, if the month in force needs it,
    /// and have the tile loader read its tiles. A floor that is resident
    /// already, the month's again after a date that went away from it and
    /// back, only has its pack opened for the tiles if they are not read
    /// already.
    fn install(&mut self, kind: PackKind, renderer: &mut Renderer) -> bool {
        if matches!(kind, PackKind::Day(month) if month != self.month) {
            return false;
        }
        let layer = layer_of(kind);
        let tiled = kind != PackKind::Mask;
        let resident = renderer.surface_resident(layer);
        if resident && (!tiled || self.tiles.holds(kind)) {
            return false;
        }
        let path = pack_path(&self.cache_dir, kind);
        let result = Pack::open(&path)
            .map_err(|e| e.to_string())
            .and_then(|pack| {
                if !resident {
                    renderer.install_surface(layer, &pack)?;
                }
                if tiled {
                    self.tiles.open(Arc::new(pack));
                }
                Ok(())
            });
        match result {
            Ok(()) if resident => false,
            Ok(()) => {
                info!(?layer, "a surface cube is resident");
                true
            }
            Err(e) => {
                error!(path = %path.display(), error = %e, "a surface cube could not be made resident");
                renderer.mark_surface_failed(layer);
                false
            }
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
