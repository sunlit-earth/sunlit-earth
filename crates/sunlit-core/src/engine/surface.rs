//! Feeding the cube surface to the renderer: the transcoder that builds the
//! packs, the month in force, and the pause gate the engine holds while it is
//! busy.
//!
//! The transcoder is the producer and this is its consumer, on the engine
//! thread: every tick drains the packs that landed and hands the ones the
//! month in force needs to the renderer, which reads the floors and the mask
//! out of them and uploads them there and then. The rest of the year stays on
//! disk until a month needs it.

use std::path::PathBuf;
use std::sync::Arc;

use tracing::{error, info, warn};

use crate::assets::cube_layout::CubeTextures;
use crate::assets::tiles::{
    Geometry, Pack, PackKind, Phase, TranscodeNotify, Transcoder, TranscoderConfig, pack_path,
};
use crate::renderer::{Renderer, SurfaceLayer};

/// The transcoder and what the engine has done with its output.
pub(super) struct SurfaceFeed {
    transcoder: Transcoder,
    cache_dir: PathBuf,
    /// The month in force, January 0.
    month: usize,
    /// Whether the pause gate is closed.
    paused: bool,
    /// How many of the transcoder's failures have reached the renderer.
    failures_seen: usize,
    /// The worker has finished, and its status will not change again.
    settled: bool,
}

impl SurfaceFeed {
    /// Start the transcoder over `textures`, with `month` in force.
    ///
    /// `wake` is called on the transcoder's thread after every change of its
    /// status, so it must return at once and never panic; the engine's is a
    /// send on its own unbounded command channel.
    pub(super) fn start(
        cache_dir: PathBuf,
        textures: CubeTextures,
        geometry: Geometry,
        month: usize,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
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
            cache_dir,
            month,
            paused: true,
            failures_seen: 0,
            settled: false,
        }
    }

    /// Hand every pack that landed since the last call to the renderer, and
    /// every failure the transcoder reported. Returns whether a cube became
    /// resident.
    pub(super) fn drain(&mut self, renderer: &mut Renderer) -> bool {
        let mut installed = false;
        for kind in self.transcoder.landed() {
            installed |= self.install(kind, renderer);
        }
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

    /// Close the pause gate while the engine is busy and open it when it is
    /// not, telling the transcoder only when that changes.
    pub(super) fn set_busy(&mut self, busy: bool) {
        if busy != self.paused {
            self.paused = busy;
            self.transcoder.set_paused(busy);
        }
    }

    /// Make the cube of pack `kind` resident, if the month in force needs it.
    fn install(&self, kind: PackKind, renderer: &mut Renderer) -> bool {
        if matches!(kind, PackKind::Day(month) if month != self.month) {
            return false;
        }
        let layer = layer_of(kind);
        if renderer.surface_resident(layer) {
            return false;
        }
        let path = pack_path(&self.cache_dir, kind);
        let result = Pack::open(&path)
            .map_err(|e| e.to_string())
            .and_then(|pack| renderer.install_surface(layer, &pack));
        match result {
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
