//! The tile loader: the tiles the frame wants, read from their packs on a few
//! worker threads and made resident in the tile array on the engine thread.
//!
//! The engine computes the wanted set with `renderer::residency` on every draw
//! where something it depends on changed, holds the page table to the set's
//! cap, and publishes the tiles that are neither resident, failed nor already
//! on their way into a latest-value slot. The workers claim from the slot in
//! order under its mutex and park on its condition variable when nothing is
//! left; a claim is one positional read of the tile's blob from its open pack,
//! decoded to RGBA8 on the worker where the tile array is RGBA8, and the result
//! goes back through a bounded channel. The engine drains the channel on every
//! tick under a byte budget, waiting briefly for the workers while they still
//! read, evicts what it must and uploads the rest before the tick's render, so
//! the tiles and the page table that names them reach the GPU in one submit.
//!
//! Cancellation is implicit: a tile that leaves the set is never claimed, and
//! one claimed before it left is dropped when its result arrives. So is a
//! result read before the tile array was last purged, which is what the epoch
//! counts, and one read from a pack that has since been opened again. A tile
//! whose read fails is recorded as failed for its pack, is drawn from its
//! ancestor or the floor, and is not read again until the pack is.
//!
//! The array holds as many tiles as it has layers. The set is taken in its
//! order until they are spoken for, and the rest is drawn from what is
//! resident above it, or the floor; eviction takes only tiles outside what was
//! taken, the one wanted longest ago first, and only when a layer is needed.

use std::collections::{HashMap, HashSet};
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded};
use tracing::{debug, error, info, warn};

use crate::assets::cloud_fetcher::NotifyFn;
use crate::assets::tiles::{Geometry, Pack, PackKind};
use crate::config::TEXTURE_RESOLUTIONS;
use crate::renderer::Renderer;
use crate::renderer::residency::{Drag, Output, Request, Residency, Surfaces, Wanted};
use crate::renderer::tiles::{CellLevels, TileId, TileLayers, TileTexels, TileUpload, decode_tile};
use crate::scene::camera::CameraParams;
use crate::thread_priority;

/// Bytes of tiles the engine hands the queue per tick, and at least one tile:
/// about 160 BC7 tiles, 1.7 ms through this machine's GPU, or 40 RGBA8 ones,
/// about 96 ms through WARP (research section 24).
const UPLOAD_BUDGET: usize = 4 << 20;

/// How long a drain may wait for the workers to hand back more while its
/// budget is not spent and a tile is still being read. The channel holds a
/// few tiles, so a drain that took only what was queued would stop far short
/// of the budget; the workers refill it within microseconds a tile.
const DRAIN_WAIT: Duration = Duration::from_millis(4);

/// Results in flight per worker: one being handed over while the next is read.
const RESULTS_PER_WORKER: usize = 2;

/// Where the engine draws the tiles: the renderer in the app, a stand-in in
/// the tests of this module.
pub(super) trait TileTarget {
    /// The array's layers and the tile each holds.
    fn layers(&self) -> Option<&TileLayers>;
    fn upload(&mut self, tiles: Vec<TileUpload>) -> Vec<(TileId, String)>;
    fn evict(&mut self, tiles: &[TileId]);
    fn set_cap(&mut self, cap: CellLevels);
    fn purge(&mut self);
}

impl TileTarget for Renderer {
    fn layers(&self) -> Option<&TileLayers> {
        self.tile_layers()
    }

    fn upload(&mut self, tiles: Vec<TileUpload>) -> Vec<(TileId, String)> {
        self.upload_tiles(tiles)
    }

    fn evict(&mut self, tiles: &[TileId]) {
        self.evict_tiles(tiles);
    }

    fn set_cap(&mut self, cap: CellLevels) {
        self.set_tile_cap(cap);
    }

    fn purge(&mut self) {
        self.purge_tiles();
    }
}

/// What a frame is drawn as, for the wanted set.
pub(super) struct View<'a> {
    /// Each output with its framing applied, as the render takes it: the
    /// preview first, whose cap the page table is held to between exports,
    /// then every export that waits for its tiles.
    pub outputs: &'a [Output],
    /// The month in force, January 0.
    pub month: usize,
    /// `None` for the grid, which draws no tile.
    pub surfaces: Option<Surfaces>,
    /// The resolution setting, which caps the finest level.
    pub texture_resolution: u32,
    pub drag: Option<Drag>,
}

/// How long the camera may rest between two moves of one drag. A drag ends
/// when it has rested this long, and the wanted set goes back to the 1 px
/// threshold. The app sends a move per pointer event while a hand moves the
/// globe, a few to a few tens of milliseconds apart.
pub(super) const DRAG_PAUSE: Duration = Duration::from_millis(100);

/// Whether the camera is being dragged, and how fast, from the moves of its
/// longitude and latitude on the injected clock.
///
/// A move is a drag when the camera moved, or was first seen, within
/// [`DRAG_PAUSE`] before it, so a camera set once after a rest, by a click or
/// a preset, is not one. Its rate is the move's over the time since the one
/// before; moves that arrive at the same instant are measured together at
/// the next that does not.
#[derive(Debug, Default)]
pub(super) struct DragWatch {
    /// Where the camera was after its last measured move, and when.
    last: Option<(f32, f32, Duration)>,
    drag: Option<Drag>,
}

impl DragWatch {
    /// The drag in progress with the camera at `camera` at `now`.
    pub(super) fn observe(&mut self, camera: &CameraParams, now: Duration) -> Option<Drag> {
        let (longitude, latitude) = (camera.longitude, camera.latitude);
        let Some((last_longitude, last_latitude, at)) = self.last else {
            self.last = Some((longitude, latitude, now));
            return None;
        };
        if (longitude, latitude) == (last_longitude, last_latitude) {
            self.settle(now);
            return self.drag;
        }
        let since = now.saturating_sub(at);
        if since.is_zero() {
            return self.drag;
        }
        self.drag = (since <= DRAG_PAUSE).then(|| {
            let seconds = since.as_secs_f32();
            let turned = (longitude - last_longitude + 180.0).rem_euclid(360.0) - 180.0;
            Drag {
                longitude_rate: turned / seconds,
                latitude_rate: (latitude - last_latitude) / seconds,
            }
        });
        self.last = Some((longitude, latitude, now));
        self.drag
    }

    /// End the drag in progress if the camera has rested [`DRAG_PAUSE`] by
    /// `now`. Returns whether one ended.
    pub(super) fn settle(&mut self, now: Duration) -> bool {
        let rested = self
            .last
            .is_some_and(|(_, _, at)| now.saturating_sub(at) >= DRAG_PAUSE);
        rested && self.drag.take().is_some()
    }
}

/// What the tile loader holds, for a client that asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileReport {
    /// Layers the tile array has.
    pub capacity: u32,
    /// The tiles taken from the wanted set, most wanted first: every tile of
    /// the set until the layers are spoken for, a failed one taking none.
    pub wanted: Vec<TileId>,
    /// How many of `wanted`, from its start, are in view rather than margin.
    pub in_view: usize,
    /// Tiles of the wanted set left for want of layers, drawn from what is
    /// resident above them or the floor.
    pub beyond: usize,
    pub resident: Vec<TileId>,
    pub failed: Vec<TileId>,
    /// Results the engine has taken from the workers.
    pub reads: u64,
    /// How many times the tile array was purged.
    pub epoch: u64,
    /// Whether the set was computed for a drag in progress, at the drag's
    /// threshold rather than 1 px.
    pub dragging: bool,
}

/// A pack tiles are read from, and when it was opened.
struct OpenPack {
    pack: Arc<Pack>,
    /// Counted across every pack, so a result names the opening it was read
    /// from.
    generation: u64,
}

impl OpenPack {
    fn stores(&self, id: TileId) -> bool {
        self.pack
            .find(id.key)
            .is_some_and(|entry| !entry.ocean && !entry.whole_face)
    }
}

/// The slot the engine publishes to and the workers claim from.
struct Slot {
    /// The tiles to read, most wanted first, and how many have been claimed.
    /// Replaced whole by a publish, which starts the claims over; a claim
    /// takes its tile under the lock, so no worker holds a place in a list
    /// that has been replaced.
    jobs: Vec<TileId>,
    next: usize,
    /// Claimed and not yet taken back by the engine. A publish leaves them
    /// out, so a tile is never read twice at once.
    claimed: HashSet<TileId>,
    packs: HashMap<PackKind, (Arc<Pack>, u64)>,
    epoch: u64,
    /// Whether a blob is decoded to RGBA8 before it is handed back.
    decode: bool,
    stopping: bool,
}

struct Claim {
    id: TileId,
    pack: Arc<Pack>,
    generation: u64,
    epoch: u64,
    decode: bool,
}

impl Slot {
    /// Whether a worker holds a tile it has not handed back, or has one left
    /// to claim.
    fn reading(&self) -> bool {
        !self.claimed.is_empty()
            || self.jobs[self.next.min(self.jobs.len())..]
                .iter()
                .any(|id| self.packs.contains_key(&id.pack))
    }

    fn claim(&mut self) -> Option<Claim> {
        while let Some(&id) = self.jobs.get(self.next) {
            self.next += 1;
            let Some((pack, generation)) = self.packs.get(&id.pack) else {
                continue;
            };
            self.claimed.insert(id);
            return Some(Claim {
                id,
                pack: Arc::clone(pack),
                generation: *generation,
                epoch: self.epoch,
                decode: self.decode,
            });
        }
        None
    }
}

struct Shared {
    slot: Mutex<Slot>,
    /// Signalled by every publish and by the stop.
    published: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.slot
            .lock()
            .expect("the tile loader's lock is poisoned")
    }

    /// The next tile to read, waiting while there is none; `None` once the
    /// loader stops.
    fn next(&self) -> Option<Claim> {
        let mut slot = self.lock();
        loop {
            if slot.stopping {
                return None;
            }
            if let Some(claim) = slot.claim() {
                return Some(claim);
            }
            slot = self
                .published
                .wait(slot)
                .expect("the tile loader's lock is poisoned");
        }
    }
}

/// One tile read back.
struct Loaded {
    id: TileId,
    generation: u64,
    epoch: u64,
    texels: Result<TileTexels, String>,
}

impl Claim {
    /// Read the tile's blob, and decode it where the array wants texels, a
    /// panic in either failing this tile alone.
    fn read(self) -> Loaded {
        let Self {
            id,
            pack,
            generation,
            epoch,
            decode,
        } = self;
        let read = || {
            let entry = pack
                .find(id.key)
                .filter(|entry| !entry.ocean && !entry.whole_face)
                .ok_or_else(|| format!("{:?} has no blob in its pack", id.key))?;
            let blob = pack.read(entry).map_err(|e| e.to_string())?;
            if decode {
                decode_tile(&blob, pack.layer()).map(TileTexels::Decoded)
            } else {
                Ok(TileTexels::Blocks(blob))
            }
        };
        let texels = panic::catch_unwind(AssertUnwindSafe(read))
            .unwrap_or_else(|_| Err("the read panicked".to_owned()));
        Loaded {
            id,
            generation,
            epoch,
            texels,
        }
    }
}

/// What the engine does with a result.
#[derive(Debug, PartialEq, Eq)]
enum Admit {
    Upload,
    Fail(String),
    Drop(&'static str),
}

/// Holds the tile loader's reads while shut.
///
/// Open unless someone shuts it, and the app never does: an engine test shuts
/// it to keep the tiles a frame wants on their way for as long as the case
/// needs, which no amount of waiting on the workers could make reliable. A
/// worker holds its claimed tile while it waits, so the tile counts as on its
/// way.
#[derive(Clone, Default)]
pub struct TileGate(Arc<(Mutex<bool>, Condvar)>);

impl TileGate {
    /// Hold every read that has not started.
    pub fn shut(&self) {
        *self.lock() = true;
    }

    /// Let the reads go on.
    pub fn open(&self) {
        *self.lock() = false;
        self.0.1.notify_all();
    }

    fn lock(&self) -> MutexGuard<'_, bool> {
        self.0
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Wait while the gate is shut. Returns `false` when `stopping` says the
    /// loader is going first.
    fn pass(&self, stopping: impl Fn() -> bool) -> bool {
        let mut shut = self.lock();
        while *shut {
            if stopping() {
                return false;
            }
            shut = self
                .0
                .1
                .wait_timeout(shut, Duration::from_millis(10))
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        true
    }
}

/// How the loader reads.
pub(super) struct LoaderConfig {
    /// Whether the tile array is RGBA8, so the workers decode the blocks.
    pub decode: bool,
    /// The surface sampler's anisotropy, which the wanted set counts texels
    /// with.
    pub anisotropy: u16,
    /// Worker threads; `tiles::default_threads()` in the engine.
    pub workers: usize,
    pub gate: TileGate,
}

/// The engine's side of the loader, and the handle to its workers. Dropping it
/// stops and joins them.
pub(super) struct TileLoader {
    shared: Arc<Shared>,
    /// Bounded at `RESULTS_PER_WORKER` a worker, so a worker blocks when the
    /// engine falls behind rather than reading ahead of it. The engine thread
    /// is its consumer and drains it on every tick, which the loop runs at
    /// least every 50 ms whatever else happens; it holds at most a few tiles,
    /// 104 KB each decoded to RGBA8, in transit. `None` only while dropping.
    results: Option<Receiver<Loaded>>,
    workers: Vec<JoinHandle<()>>,
    /// Wakes the engine, at most once until the engine next drains.
    wake: NotifyFn,
    poked: Arc<AtomicBool>,
    residency: Residency,
    anisotropy: u16,
    packs: HashMap<PackKind, OpenPack>,
    generations: u64,
    epoch: u64,
    /// What the wanted set in force was computed from.
    inputs: Option<Inputs>,
    /// The tiles taken from the wanted set, as `TileReport::wanted`.
    taken: Vec<TileId>,
    taken_set: HashSet<TileId>,
    in_view: usize,
    beyond: usize,
    /// How many wanted sets have been computed, and the last of them that
    /// took each resident tile, which is what eviction orders by.
    computations: u64,
    last_wanted: HashMap<TileId, u64>,
    failed: HashSet<TileId>,
    /// A result was dropped whose tile is still wanted, so it has to be
    /// published again.
    republish: bool,
    reads: u64,
}

/// The inputs of one computation, compared whole to decide whether the next
/// draw needs another.
#[derive(Debug, Clone, PartialEq)]
struct Inputs {
    outputs: Vec<Output>,
    month: usize,
    surfaces: Option<Surfaces>,
    finest: u8,
    drag: Option<Drag>,
}

impl TileLoader {
    /// Start the workers over `geometry`'s tiles, with no pack open yet.
    ///
    /// `wake` is called on a worker thread when a result is waiting, at most
    /// once until the engine next drains, and by a drain that leaves results
    /// behind, so it must return at once; the engine's sends on its own
    /// unbounded command channel.
    ///
    /// # Panics
    ///
    /// When a worker thread cannot be spawned.
    pub(super) fn start(geometry: Geometry, config: &LoaderConfig, wake: NotifyFn) -> Self {
        let workers = config.workers.max(1);
        let shared = Arc::new(Shared {
            slot: Mutex::new(Slot {
                jobs: Vec::new(),
                next: 0,
                claimed: HashSet::new(),
                packs: HashMap::new(),
                epoch: 0,
                decode: config.decode,
                stopping: false,
            }),
            published: Condvar::new(),
        });
        let (tx, results) = bounded(RESULTS_PER_WORKER * workers);
        let poked = Arc::new(AtomicBool::new(false));
        let threads = (0..workers)
            .map(|index| {
                let (shared, tx, wake, poked, gate) = (
                    Arc::clone(&shared),
                    tx.clone(),
                    Arc::clone(&wake),
                    Arc::clone(&poked),
                    config.gate.clone(),
                );
                std::thread::Builder::new()
                    .name(format!("sunlit-tiles-{index}"))
                    .spawn(move || work(&shared, &tx, &wake, &poked, &gate))
                    .expect("failed to spawn a tile loader thread")
            })
            .collect();
        Self {
            shared,
            results: Some(results),
            workers: threads,
            wake,
            poked,
            residency: Residency::new(geometry),
            anisotropy: config.anisotropy,
            packs: HashMap::new(),
            generations: 0,
            epoch: 0,
            inputs: None,
            taken: Vec::new(),
            taken_set: HashSet::new(),
            in_view: 0,
            beyond: 0,
            computations: 0,
            last_wanted: HashMap::new(),
            failed: HashSet::new(),
            republish: false,
            reads: 0,
        }
    }

    /// Read `kind`'s tiles from `pack` from now on: a pack that landed, or a
    /// month whose floor is drawn again. A day pack closes every other month's,
    /// whose tiles the page table no longer names. Tiles that failed in the
    /// pack's previous opening are tried again.
    pub(super) fn open(&mut self, pack: Arc<Pack>) {
        let kind = pack.kind();
        self.generations += 1;
        let generation = self.generations;
        if matches!(kind, PackKind::Day(_)) {
            self.packs
                .retain(|other, _| !matches!(other, PackKind::Day(_)) || *other == kind);
        }
        self.failed.retain(|id| id.pack != kind);
        self.packs.insert(kind, OpenPack { pack, generation });
        self.shared.lock().packs = self
            .packs
            .iter()
            .map(|(&kind, open)| (kind, (Arc::clone(&open.pack), open.generation)))
            .collect();
        self.inputs = None;
        debug!(?kind, generation, "the tile loader opened a pack");
    }

    /// Whether `kind`'s tiles are read from an open pack.
    pub(super) fn holds(&self, kind: PackKind) -> bool {
        self.packs.contains_key(&kind)
    }

    /// Compute the wanted set for `view` if anything it depends on changed
    /// since the last one, hold the page table to its cap, and publish what is
    /// missing.
    pub(super) fn want(&mut self, view: &View<'_>, target: &mut impl TileTarget) {
        let geometry = self.residency.geometry();
        let inputs = Inputs {
            outputs: view.outputs.to_vec(),
            month: view.month,
            surfaces: view.surfaces,
            finest: finest_allowed(&geometry, view.texture_resolution),
            drag: view.drag,
        };
        if self.inputs.as_ref() == Some(&inputs) {
            return;
        }
        let wanted = match inputs.surfaces {
            None => Wanted {
                tiles: Vec::new(),
                cap: CellLevels::uniform(&geometry, Geometry::level_of(geometry.floor)),
            },
            Some(surfaces) => {
                let stored =
                    |id: TileId| self.packs.get(&id.pack).is_some_and(|open| open.stores(id));
                self.residency.wanted(&Request {
                    outputs: &inputs.outputs,
                    month: inputs.month,
                    surfaces,
                    finest: inputs.finest,
                    drag: inputs.drag,
                    anisotropy: self.anisotropy,
                    stored: &stored,
                })
            }
        };
        let drawn = inputs
            .outputs
            .first()
            .map(|output| self.cap_of(&inputs, output));
        self.inputs = Some(inputs);
        self.apply(&wanted, target);
        if let Some(cap) = drawn {
            target.set_cap(cap);
        }
    }

    /// The cap the page table takes to draw `output` under the set in force:
    /// its own, and every cell at the floor for the grid, or before a set was
    /// computed.
    pub(super) fn cap_for(&self, output: &Output) -> CellLevels {
        if let Some(inputs) = &self.inputs {
            self.cap_of(inputs, output)
        } else {
            let geometry = self.residency.geometry();
            CellLevels::uniform(&geometry, Geometry::level_of(geometry.floor))
        }
    }

    fn cap_of(&self, inputs: &Inputs, output: &Output) -> CellLevels {
        let geometry = self.residency.geometry();
        let finest = match inputs.surfaces {
            Some(_) => inputs.finest,
            None => Geometry::level_of(geometry.floor),
        };
        self.residency.cap(output, finest, self.anisotropy)
    }

    /// How many tiles of the set in force the frame needs in view that are
    /// neither resident nor failed: tiles on their way, or about to be.
    pub(super) fn missing(&self, target: &impl TileTarget) -> usize {
        let layers = target.layers();
        self.taken[..self.in_view]
            .iter()
            .filter(|id| {
                !self.failed.contains(id)
                    && layers.is_none_or(|layers| layers.layer_of(**id).is_none())
            })
            .count()
    }

    /// Whether the set in force is the one at the 1 px threshold and every
    /// tile it needs in view is resident or failed. Tiles left for want of
    /// layers do not count (plan departure 22).
    pub(super) fn complete(&self, target: &impl TileTarget) -> bool {
        self.inputs
            .as_ref()
            .is_some_and(|inputs| inputs.drag.is_none())
            && self.missing(target) == 0
    }

    /// Make `wanted` the set in force: take it in order until the layers are
    /// spoken for, hold the page table to its cap, and publish what is
    /// missing.
    fn apply(&mut self, wanted: &Wanted, target: &mut impl TileTarget) {
        self.computations += 1;
        let capacity = target.layers().map_or(0, TileLayers::capacity) as usize;
        let (mut taken, mut layers, mut in_view, mut beyond) = (Vec::new(), 0, 0, 0);
        for tile in &wanted.tiles {
            let failed = self.failed.contains(&tile.id);
            if !failed && layers == capacity {
                beyond += 1;
                continue;
            }
            layers += usize::from(!failed);
            in_view += usize::from(!tile.margin);
            taken.push(tile.id);
        }
        if (beyond > 0) != (self.beyond > 0) {
            if beyond > 0 {
                warn!(
                    wanted = wanted.tiles.len(),
                    layers = capacity,
                    beyond,
                    "the frame wants more tiles than the tile array holds; the rest are drawn from what is resident above them"
                );
            } else {
                info!("the tile array holds every tile the frame wants again");
            }
        }
        self.taken_set = taken.iter().copied().collect();
        self.taken = taken;
        self.in_view = in_view;
        self.beyond = beyond;
        if let Some(layers) = target.layers() {
            for (id, _) in layers.resident() {
                if self.taken_set.contains(&id) {
                    self.last_wanted.insert(id, self.computations);
                }
            }
        }
        self.publish(target);
    }

    /// Replace the slot's tiles with what is taken and not resident, failed or
    /// on its way.
    fn publish(&mut self, target: &impl TileTarget) {
        let layers = target.layers();
        let mut slot = self.shared.lock();
        let jobs: Vec<TileId> = self
            .taken
            .iter()
            .copied()
            .filter(|id| {
                !self.failed.contains(id)
                    && !slot.claimed.contains(id)
                    && layers.is_none_or(|layers| layers.layer_of(*id).is_none())
            })
            .collect();
        slot.jobs = jobs;
        slot.next = 0;
        drop(slot);
        self.shared.published.notify_all();
        self.republish = false;
    }

    /// Take results under the byte budget and upload them, evicting where a
    /// layer is needed. Returns whether a tile was uploaded or failed, either
    /// of which can complete the set.
    ///
    /// What is queued is taken at once, and while a worker is still reading,
    /// the drain waits for it, [`DRAIN_WAIT`] at the most, until the budget
    /// is spent. Every result is uploaded before the drain returns, so no
    /// tile's texels outlive it.
    pub(super) fn drain(&mut self, target: &mut impl TileTarget) -> bool {
        self.drain_within(target, DRAIN_WAIT)
    }

    fn drain_within(&mut self, target: &mut impl TileTarget, wait: Duration) -> bool {
        let Some(results) = self.results.clone() else {
            return false;
        };
        self.poked.store(false, Ordering::Release);
        let deadline = Instant::now() + wait;
        let failures = self.failed.len();
        let (mut spent, mut received, mut uploads) = (0, 0_u64, Vec::new());
        while spent < UPLOAD_BUDGET {
            let loaded = match results.try_recv() {
                Ok(loaded) => loaded,
                Err(_) if self.shared.lock().reading() => match results.recv_deadline(deadline) {
                    Ok(loaded) => loaded,
                    Err(_) => break,
                },
                Err(_) => break,
            };
            received += 1;
            self.shared.lock().claimed.remove(&loaded.id);
            match self.admit(&loaded, target.layers()) {
                Admit::Upload => {
                    let texels = loaded.texels.expect("admitted only when read");
                    spent += texels.len();
                    uploads.push(TileUpload {
                        id: loaded.id,
                        texels,
                        layer: None,
                    });
                }
                Admit::Fail(reason) => {
                    warn!(tile = ?loaded.id, %reason, "a tile could not be read; it is drawn from its ancestor");
                    self.failed.insert(loaded.id);
                }
                Admit::Drop(why) => {
                    debug!(tile = ?loaded.id, why, "a tile read was dropped");
                    self.republish |= self.taken_set.contains(&loaded.id);
                }
            }
        }
        let more = !results.is_empty();
        self.reads += received;
        let uploaded = self.upload(uploads, target);
        if self.republish {
            self.publish(target);
        }
        if more {
            (self.wake)();
        }
        uploaded || self.failed.len() > failures
    }

    /// Whether a result goes to the GPU, fails its tile, or is dropped.
    fn admit(&self, loaded: &Loaded, layers: Option<&TileLayers>) -> Admit {
        if self.packs.get(&loaded.id.pack).map(|open| open.generation) != Some(loaded.generation) {
            return Admit::Drop("read from a pack that has since been opened again or closed");
        }
        if let Err(reason) = &loaded.texels {
            return Admit::Fail(reason.clone());
        }
        if loaded.epoch != self.epoch {
            return Admit::Drop("read before the tile array was purged");
        }
        if !self.taken_set.contains(&loaded.id) {
            return Admit::Drop("no longer wanted");
        }
        if layers.is_some_and(|layers| layers.layer_of(loaded.id).is_some()) {
            return Admit::Drop("already resident");
        }
        Admit::Upload
    }

    /// Evict as many tiles outside the set as `uploads` needs layers beyond
    /// the free ones, the one wanted longest ago first, and upload. Returns
    /// whether a tile was uploaded.
    fn upload(&mut self, mut uploads: Vec<TileUpload>, target: &mut impl TileTarget) -> bool {
        if uploads.is_empty() {
            return false;
        }
        let Some(layers) = target.layers() else {
            return false;
        };
        let free = layers.free() as usize;
        let mut victims: Vec<(u64, u32, TileId)> = Vec::new();
        if uploads.len() > free {
            victims = layers
                .resident()
                .filter(|(id, _)| !self.taken_set.contains(id))
                .map(|(id, layer)| (self.last_wanted.get(&id).copied().unwrap_or(0), layer, id))
                .collect();
            victims.sort_unstable_by_key(|&(wanted, layer, _)| (wanted, layer));
            victims.truncate(uploads.len() - free);
            if uploads.len() > free + victims.len() {
                uploads.truncate(free + victims.len());
                self.republish = true;
            }
        }
        let victims: Vec<TileId> = victims.into_iter().map(|(_, _, id)| id).collect();
        if !victims.is_empty() {
            for id in &victims {
                self.last_wanted.remove(id);
            }
            target.evict(&victims);
        }
        let ids: Vec<TileId> = uploads.iter().map(|upload| upload.id).collect();
        let failed = target.upload(uploads);
        for (id, reason) in &failed {
            warn!(tile = ?id, %reason, "a tile could not be uploaded; it is drawn from its ancestor");
            self.failed.insert(*id);
        }
        for id in ids.iter().filter(|id| !self.failed.contains(id)) {
            self.last_wanted.insert(*id, self.computations);
        }
        debug!(
            uploaded = ids.len() - failed.len(),
            evicted = victims.len(),
            "tiles made resident"
        );
        ids.len() > failed.len()
    }

    /// Let go of every tile in the array, and of every read on its way: the
    /// resolution setting changed, or the device the array was on is gone.
    /// The next draw computes the wanted set afresh.
    pub(super) fn purge(&mut self, target: &mut impl TileTarget) {
        self.epoch += 1;
        target.purge();
        self.last_wanted.clear();
        self.taken.clear();
        self.taken_set.clear();
        self.inputs = None;
        let mut slot = self.shared.lock();
        slot.epoch = self.epoch;
        slot.jobs.clear();
        slot.next = 0;
        drop(slot);
        info!(epoch = self.epoch, "the tile array was purged");
    }

    pub(super) fn report(&self, target: &impl TileTarget) -> TileReport {
        let layers = target.layers();
        let mut resident: Vec<TileId> = layers
            .map(|layers| layers.resident().map(|(id, _)| id).collect())
            .unwrap_or_default();
        resident.sort_by_key(|id| (pack_order(id.pack), id.key));
        let mut failed: Vec<TileId> = self.failed.iter().copied().collect();
        failed.sort_by_key(|id| (pack_order(id.pack), id.key));
        TileReport {
            capacity: layers.map_or(0, TileLayers::capacity),
            wanted: self.taken.clone(),
            in_view: self.in_view,
            beyond: self.beyond,
            resident,
            failed,
            reads: self.reads,
            epoch: self.epoch,
            dragging: self
                .inputs
                .as_ref()
                .is_some_and(|inputs| inputs.drag.is_some()),
        }
    }
}

impl Drop for TileLoader {
    fn drop(&mut self) {
        self.shared.lock().stopping = true;
        self.shared.published.notify_all();
        // A worker blocked on a full channel is released by its receiver
        // going away.
        drop(self.results.take());
        for worker in self.workers.drain(..) {
            if worker.join().is_err() {
                error!("a tile loader thread panicked");
            }
        }
    }
}

/// A worker: claim, read, hand back, until the loader stops.
fn work(
    shared: &Shared,
    results: &Sender<Loaded>,
    wake: &NotifyFn,
    poked: &AtomicBool,
    gate: &TileGate,
) {
    thread_priority::lower_current_thread("tile loader");
    while let Some(claim) = shared.next() {
        if !gate.pass(|| shared.lock().stopping) {
            return;
        }
        if results.send(claim.read()).is_err() {
            return;
        }
        if !poked.swap(true, Ordering::AcqRel) {
            wake();
        }
    }
}

/// The finest level the resolution setting allows for `geometry`: its face at
/// the widest setting, one level coarser for each halving of it. On the
/// shipped geometry that is `residency::finest_level`: 8192 the 2048 level,
/// 4096 the 1024 level, 2048 the floor's.
fn finest_allowed(geometry: &Geometry, texture_resolution: u32) -> u8 {
    let widest = TEXTURE_RESOLUTIONS
        .iter()
        .copied()
        .max()
        .unwrap_or(texture_resolution);
    let halvings =
        Geometry::level_of(widest).saturating_sub(Geometry::level_of(texture_resolution.max(1)));
    Geometry::level_of(geometry.face).saturating_sub(halvings)
}

/// An order of the packs, for the report's sorted lists.
fn pack_order(pack: PackKind) -> usize {
    match pack {
        PackKind::Day(month) => month,
        PackKind::Night => 12,
        PackKind::Mask => 13,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::assets::cube_layout::CubeTextures;
    use crate::assets::tiles::{FIXTURE, GEOMETRY, ensure_pack, pack_path};
    use crate::renderer::residency::{WantedTile, finest_level};
    use crate::test_support::{ScratchDir, write_cube_fixture};

    /// The tile array as the loader sees it, without a GPU: what is resident,
    /// and every call it was made.
    struct Stand {
        layers: TileLayers,
        uploaded: Vec<TileId>,
        evicted: Vec<TileId>,
        purges: usize,
    }

    impl Stand {
        fn new(capacity: u32) -> Self {
            Self {
                layers: TileLayers::new(capacity),
                uploaded: Vec::new(),
                evicted: Vec::new(),
                purges: 0,
            }
        }

        fn holds(&self, id: TileId) -> bool {
            self.layers.layer_of(id).is_some()
        }
    }

    impl TileTarget for Stand {
        fn layers(&self) -> Option<&TileLayers> {
            Some(&self.layers)
        }

        fn upload(&mut self, tiles: Vec<TileUpload>) -> Vec<(TileId, String)> {
            let mut failed = Vec::new();
            for tile in tiles {
                match self.layers.claim(tile.id, tile.layer) {
                    Ok(_) => self.uploaded.push(tile.id),
                    Err(why) => failed.push((tile.id, why)),
                }
            }
            failed
        }

        fn evict(&mut self, tiles: &[TileId]) {
            for &id in tiles {
                self.layers.release(id);
                self.evicted.push(id);
            }
        }

        fn set_cap(&mut self, _cap: CellLevels) {}

        fn purge(&mut self) {
            self.layers = TileLayers::new(self.layers.capacity());
            self.purges += 1;
        }
    }

    /// The fixture's January day pack, built into `dir`.
    fn day_pack(dir: &ScratchDir) -> Arc<Pack> {
        write_cube_fixture(&dir.join("textures"));
        let textures = CubeTextures::resolve(&dir.join("textures"));
        let cache = dir.join("cache");
        ensure_pack(
            &cache,
            PackKind::Day(0),
            &textures,
            &FIXTURE,
            &AtomicBool::new(false),
        )
        .expect("the fixture's January pack");
        Arc::new(Pack::open(&pack_path(&cache, PackKind::Day(0))).expect("open the pack"))
    }

    /// Every tile the pack stores a blob for, coarse levels first.
    fn stored(pack: &Pack) -> Vec<TileId> {
        pack.entries()
            .iter()
            .filter(|entry| !entry.ocean && !entry.whole_face)
            .map(|entry| TileId {
                pack: pack.kind(),
                key: entry.key,
            })
            .collect()
    }

    fn loader(workers: usize, pack: &Arc<Pack>) -> TileLoader {
        let mut loader = TileLoader::start(
            FIXTURE,
            &LoaderConfig {
                decode: false,
                anisotropy: 8,
                workers,
                gate: TileGate::default(),
            },
            Arc::new(|| {}),
        );
        loader.open(Arc::clone(pack));
        loader
    }

    fn wanted(tiles: &[TileId]) -> Wanted {
        Wanted {
            tiles: tiles
                .iter()
                .map(|&id| WantedTile {
                    id,
                    deficit: 1,
                    margin: false,
                })
                .collect(),
            cap: CellLevels::finest(&FIXTURE),
        }
    }

    /// Poll `until` without sleeping on it for anything but its own answer.
    fn wait_for(what: &str, mut until: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !until() {
            assert!(Instant::now() < deadline, "{what} did not happen");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn in_transit(loader: &TileLoader) -> usize {
        loader.results.as_ref().map_or(0, Receiver::len)
    }

    fn claimed(loader: &TileLoader) -> usize {
        loader.shared.lock().claimed.len()
    }

    /// One worker has filled the channel and holds the tile after them,
    /// blocked on the send.
    fn wait_until_blocked(loader: &TileLoader) {
        let bound = RESULTS_PER_WORKER;
        wait_for("a full channel", || {
            in_transit(loader) == bound && claimed(loader) == bound + 1
        });
    }

    fn drain_until(
        loader: &mut TileLoader,
        stand: &mut Stand,
        what: &str,
        done: impl Fn(&Stand) -> bool,
    ) {
        wait_for(what, || {
            loader.drain(stand);
            done(stand)
        });
    }

    #[test]
    fn a_worker_blocks_on_a_full_channel_and_goes_on_when_it_is_drained() {
        let dir = ScratchDir::new("tile_loader_full");
        let pack = day_pack(&dir);
        let tiles = stored(&pack);
        assert!(
            tiles.len() > 2 * RESULTS_PER_WORKER,
            "the fixture stores {} tiles",
            tiles.len()
        );
        let mut loader = loader(1, &pack);
        let mut stand = Stand::new(64);
        loader.apply(&wanted(&tiles), &mut stand);

        wait_until_blocked(&loader);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            (in_transit(&loader), claimed(&loader)),
            (RESULTS_PER_WORKER, RESULTS_PER_WORKER + 1),
            "a blocked worker claims nothing more"
        );

        drain_until(&mut loader, &mut stand, "every tile resident", |stand| {
            tiles.iter().all(|&id| stand.holds(id))
        });
        assert_eq!(stand.uploaded, tiles, "each tile once, in the order wanted");
        assert_eq!(loader.reads, tiles.len() as u64);
        assert_eq!(claimed(&loader), 0);
    }

    #[test]
    fn a_drain_takes_what_the_workers_read_while_it_runs_and_not_only_what_was_queued() {
        let dir = ScratchDir::new("tile_loader_drain_waits");
        let pack = day_pack(&dir);
        let tiles = stored(&pack);
        assert!(tiles.len() > 2 * (RESULTS_PER_WORKER + 1));
        let mut loader = loader(1, &pack);
        let mut stand = Stand::new(64);
        loader.apply(&wanted(&tiles), &mut stand);
        wait_until_blocked(&loader);

        let started = Instant::now();
        assert!(loader.drain_within(&mut stand, Duration::from_secs(20)));
        assert_eq!(stand.uploaded, tiles, "the whole set in one drain");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the drain waited out its time with nothing left to read"
        );
        assert_eq!(claimed(&loader), 0);
    }

    #[test]
    fn a_tile_read_after_it_left_the_set_is_dropped() {
        let dir = ScratchDir::new("tile_loader_left");
        let pack = day_pack(&dir);
        let tiles = stored(&pack);
        let (before, after) = tiles.split_at(tiles.len() / 2);
        assert!(before.len() > RESULTS_PER_WORKER && !after.is_empty());
        let mut loader = loader(1, &pack);
        let mut stand = Stand::new(64);
        loader.apply(&wanted(before), &mut stand);
        wait_until_blocked(&loader);

        loader.apply(&wanted(after), &mut stand);
        drain_until(&mut loader, &mut stand, "the new set resident", |stand| {
            after.iter().all(|&id| stand.holds(id))
        });
        assert!(
            stand.uploaded.iter().all(|id| after.contains(id)),
            "a tile the set no longer wants was uploaded: {:?}",
            stand.uploaded
        );
        assert_eq!(
            loader.reads,
            (RESULTS_PER_WORKER + 1 + after.len()) as u64,
            "the tiles in transit were read, and nothing else of the old set"
        );
    }

    #[test]
    fn a_tile_read_before_a_purge_is_dropped_and_read_again() {
        let dir = ScratchDir::new("tile_loader_purge");
        let pack = day_pack(&dir);
        let tiles = stored(&pack);
        let mut loader = loader(1, &pack);
        let mut stand = Stand::new(64);
        loader.apply(&wanted(&tiles), &mut stand);
        wait_until_blocked(&loader);

        loader.purge(&mut stand);
        assert_eq!(stand.purges, 1);
        loader.apply(&wanted(&tiles), &mut stand);
        drain_until(&mut loader, &mut stand, "every tile resident", |stand| {
            tiles.iter().all(|&id| stand.holds(id))
        });
        let mut once = stand.uploaded.clone();
        once.sort_by_key(|id| id.key);
        once.dedup();
        assert_eq!(
            once.len(),
            stand.uploaded.len(),
            "a tile was uploaded twice"
        );
        assert_eq!(stand.uploaded.len(), tiles.len());
        assert_eq!(
            loader.reads,
            (tiles.len() + RESULTS_PER_WORKER + 1) as u64,
            "the reads in transit at the purge were made again"
        );
    }

    #[test]
    fn a_tile_read_from_a_pack_since_opened_again_is_dropped_and_read_again() {
        let dir = ScratchDir::new("tile_loader_reopen");
        let pack = day_pack(&dir);
        let tiles = stored(&pack);
        let mut loader = loader(1, &pack);
        let mut stand = Stand::new(64);
        loader.apply(&wanted(&tiles), &mut stand);
        wait_until_blocked(&loader);

        loader.open(Arc::new(
            Pack::open(&pack_path(&dir.join("cache"), PackKind::Day(0))).expect("open again"),
        ));
        drain_until(&mut loader, &mut stand, "every tile resident", |stand| {
            tiles.iter().all(|&id| stand.holds(id))
        });
        assert_eq!(stand.uploaded.len(), tiles.len(), "each tile once");
        assert_eq!(loader.reads, (tiles.len() + RESULTS_PER_WORKER + 1) as u64);
    }

    #[test]
    fn a_tile_that_fails_its_read_is_failed_until_its_pack_is_opened_again() {
        let dir = ScratchDir::new("tile_loader_failed");
        let pack = day_pack(&dir);
        let tiles = stored(&pack);
        let broken = tiles[0];
        let entry = pack.find(broken.key).expect("an entry").clone();
        let path = pack_path(&dir.join("cache"), PackKind::Day(0));
        let mut bytes = std::fs::read(&path).expect("read the pack");
        let at = usize::try_from(entry.offset).expect("an offset");
        bytes[at] ^= 0xFF;
        std::fs::write(&path, &bytes).expect("damage one blob");
        let pack = Arc::new(Pack::open(&path).expect("the index is intact"));

        let mut loader = loader(1, &pack);
        let mut stand = Stand::new(64);
        loader.apply(&wanted(&tiles), &mut stand);
        drain_until(&mut loader, &mut stand, "the rest resident", |stand| {
            tiles[1..].iter().all(|&id| stand.holds(id))
        });
        wait_for("the failure recorded", || {
            loader.drain(&mut stand);
            loader.failed.contains(&broken)
        });
        assert!(!stand.holds(broken));
        let report = loader.report(&stand);
        assert_eq!(report.failed, [broken]);
        assert!(
            report.wanted.contains(&broken),
            "a failed tile stays in the set"
        );

        let reads = loader.reads;
        loader.apply(&wanted(&tiles), &mut stand);
        assert!(
            loader.shared.lock().jobs.is_empty(),
            "nothing is read again"
        );
        loader.drain(&mut stand);
        assert_eq!(loader.reads, reads);

        loader.open(Arc::clone(&pack));
        assert!(
            loader.failed.is_empty(),
            "the pack's next opening tries it again"
        );
        loader.apply(&wanted(&tiles), &mut stand);
        wait_for("the second failure", || {
            loader.drain(&mut stand);
            loader.failed.contains(&broken)
        });
        assert_eq!(loader.reads, reads + 1);
    }

    #[test]
    fn eviction_waits_for_a_layer_to_be_needed_and_takes_the_tile_wanted_longest_ago() {
        let dir = ScratchDir::new("tile_loader_evict");
        let pack = day_pack(&dir);
        let tiles = stored(&pack);
        assert!(tiles.len() >= 7, "the fixture stores {} tiles", tiles.len());
        let (a, b) = (&tiles[..4], &tiles[4..7]);
        let mut loader = loader(1, &pack);
        let mut stand = Stand::new(4);
        let resident = |stand: &Stand, set: &[TileId]| set.iter().all(|&id| stand.holds(id));

        loader.apply(&wanted(a), &mut stand);
        drain_until(&mut loader, &mut stand, "the first four", |s| {
            resident(s, a)
        });
        loader.apply(&wanted(&a[2..]), &mut stand);
        loader.drain(&mut stand);
        assert!(
            stand.evicted.is_empty(),
            "nothing is evicted while no layer is needed"
        );

        for (step, &tile) in b.iter().enumerate() {
            loader.apply(&wanted(&[tile]), &mut stand);
            drain_until(&mut loader, &mut stand, "the next tile", |s| s.holds(tile));
            assert_eq!(stand.evicted.len(), step + 1, "one layer for one tile");
        }
        assert_eq!(
            stand.evicted,
            [a[0], a[1], a[2]],
            "the two tiles wanted once go first, then the older of the two wanted twice"
        );
    }

    #[test]
    fn a_set_larger_than_the_array_takes_its_most_wanted_tiles() {
        let dir = ScratchDir::new("tile_loader_capacity");
        let pack = day_pack(&dir);
        let tiles = stored(&pack);
        let mut loader = loader(1, &pack);
        let mut stand = Stand::new(3);
        loader.failed.insert(tiles[1]);
        let mut set = wanted(&tiles[..6]);
        set.tiles[5].margin = true;
        loader.apply(&set, &mut stand);

        let report = loader.report(&stand);
        assert_eq!(
            report.wanted,
            tiles[..4],
            "three layers, and a failed tile needs none"
        );
        assert_eq!(report.in_view, 4);
        assert_eq!(report.beyond, 2);
        drain_until(&mut loader, &mut stand, "the three resident", |stand| {
            [tiles[0], tiles[2], tiles[3]]
                .iter()
                .all(|&id| stand.holds(id))
        });
        assert_eq!(stand.layers.free(), 0);
        assert!(stand.evicted.is_empty());
    }

    #[test]
    fn dropping_the_loader_releases_a_worker_blocked_on_a_full_channel() {
        let dir = ScratchDir::new("tile_loader_drop");
        let pack = day_pack(&dir);
        let mut loader = loader(1, &pack);
        let mut stand = Stand::new(64);
        loader.apply(&wanted(&stored(&pack)), &mut stand);
        wait_until_blocked(&loader);

        let (done, dropped) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            drop(loader);
            let _ = done.send(());
        });
        assert!(
            dropped.recv_timeout(Duration::from_secs(20)).is_ok(),
            "the drop did not join its worker"
        );
    }

    #[test]
    fn the_resolution_caps_the_level_as_the_residency_rule_does() {
        for resolution in TEXTURE_RESOLUTIONS {
            assert_eq!(
                finest_allowed(&GEOMETRY, resolution),
                finest_level(resolution),
                "at {resolution}"
            );
        }
        let floor = Geometry::level_of(FIXTURE.floor);
        let face = Geometry::level_of(FIXTURE.face);
        assert_eq!(finest_allowed(&FIXTURE, 8192), face);
        assert_eq!(finest_allowed(&FIXTURE, 4096), face - 1);
        assert_eq!(finest_allowed(&FIXTURE, 2048), floor);
    }

    #[test]
    fn a_decoding_loader_hands_back_texels() {
        let dir = ScratchDir::new("tile_loader_decode");
        let pack = day_pack(&dir);
        let tile = stored(&pack)[0];
        let entry = pack.find(tile.key).expect("an entry");
        let blob = pack.read(entry).expect("read the blob");
        let claim = Claim {
            id: tile,
            pack: Arc::clone(&pack),
            generation: 1,
            epoch: 0,
            decode: true,
        };
        assert_eq!(
            claim.read().texels,
            Ok(TileTexels::Decoded(
                decode_tile(&blob, pack.layer()).expect("decode")
            ))
        );
    }

    fn at(longitude: f32, latitude: f32) -> CameraParams {
        CameraParams {
            longitude,
            latitude,
            ..CameraParams::default()
        }
    }

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn a_camera_set_once_after_a_rest_is_not_a_drag() {
        let mut watch = DragWatch::default();
        assert_eq!(watch.observe(&at(0.0, 0.0), Duration::ZERO), None);
        let rest = DRAG_PAUSE + MS;
        assert_eq!(watch.observe(&at(40.0, 10.0), rest), None);
        assert_eq!(watch.observe(&at(40.0, 10.0), rest + DRAG_PAUSE), None);
    }

    #[test]
    fn moves_within_the_pause_are_a_drag_at_the_rate_of_the_last() {
        let mut watch = DragWatch::default();
        watch.observe(&at(0.0, 0.0), Duration::ZERO);
        let rest = DRAG_PAUSE * 10;
        watch.observe(&at(1.0, 0.0), rest);
        let drag = watch
            .observe(&at(6.0, -1.0), rest + 50 * MS)
            .expect("a second move within the pause");
        approx::assert_relative_eq!(drag.longitude_rate, 100.0, max_relative = 1e-4);
        approx::assert_relative_eq!(drag.latitude_rate, -20.0, max_relative = 1e-4);
    }

    #[test]
    fn a_drag_across_the_antimeridian_turns_the_short_way() {
        let mut watch = DragWatch::default();
        watch.observe(&at(178.0, 0.0), Duration::ZERO);
        let drag = watch
            .observe(&at(-178.0, 0.0), 40 * MS)
            .expect("a move within the pause");
        approx::assert_relative_eq!(drag.longitude_rate, 100.0, max_relative = 1e-4);
    }

    #[test]
    fn moves_at_one_instant_are_measured_together_at_the_next() {
        let mut watch = DragWatch::default();
        watch.observe(&at(0.0, 0.0), Duration::ZERO);
        assert_eq!(watch.observe(&at(2.0, 0.0), Duration::ZERO), None);
        let drag = watch
            .observe(&at(5.0, 0.0), 50 * MS)
            .expect("a move within the pause");
        approx::assert_relative_eq!(drag.longitude_rate, 100.0, max_relative = 1e-4);
    }

    #[test]
    fn a_drag_ends_when_the_camera_has_rested_the_pause() {
        let mut watch = DragWatch::default();
        watch.observe(&at(0.0, 0.0), Duration::ZERO);
        assert!(watch.observe(&at(5.0, 0.0), 50 * MS).is_some());
        let last = 50 * MS;
        assert!(
            !watch.settle((last + DRAG_PAUSE).saturating_sub(MS)),
            "rested less than the pause"
        );
        assert!(
            watch
                .observe(&at(5.0, 0.0), (last + DRAG_PAUSE).saturating_sub(MS))
                .is_some()
        );
        assert!(watch.settle(last + DRAG_PAUSE));
        assert!(!watch.settle(last + DRAG_PAUSE * 2), "ended once");
        assert_eq!(watch.observe(&at(5.0, 0.0), last + DRAG_PAUSE * 2), None);
    }
}
