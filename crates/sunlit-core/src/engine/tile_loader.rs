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
//! tick under a byte budget, evicts what it must and uploads the rest before
//! the tick's render, so the tiles and the page table that names them reach the
//! GPU in one submit.
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

use crossbeam_channel::{Receiver, Sender, bounded};
use tracing::{debug, error, info, warn};

use crate::assets::cloud_fetcher::NotifyFn;
use crate::assets::tiles::{Geometry, Pack, PackKind};
use crate::config::TEXTURE_RESOLUTIONS;
use crate::renderer::Renderer;
use crate::renderer::residency::{Output, Request, Residency, Surfaces, Wanted};
use crate::renderer::tiles::{CellLevels, TileId, TileLayers, TileTexels, TileUpload, decode_tile};
use crate::thread_priority;

/// Bytes of tiles the engine hands the queue per tick, and at least one tile:
/// about 160 BC7 tiles, 1.7 ms through this machine's GPU, or 40 RGBA8 ones,
/// about 96 ms through WARP (research section 24).
const UPLOAD_BUDGET: usize = 4 << 20;

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
    /// Each output with its framing applied, as the render takes it.
    pub outputs: &'a [Output],
    /// The month in force, January 0.
    pub month: usize,
    /// `None` for the grid, which draws no tile.
    pub surfaces: Option<Surfaces>,
    /// The resolution setting, which caps the finest level.
    pub texture_resolution: u32,
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

/// How the loader reads.
pub(super) struct LoaderConfig {
    /// Whether the tile array is RGBA8, so the workers decode the blocks.
    pub decode: bool,
    /// The surface sampler's anisotropy, which the wanted set counts texels
    /// with.
    pub anisotropy: u16,
    /// Worker threads; `tiles::default_threads()` in the engine.
    pub workers: usize,
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
                let (shared, tx, wake, poked) = (
                    Arc::clone(&shared),
                    tx.clone(),
                    Arc::clone(&wake),
                    Arc::clone(&poked),
                );
                std::thread::Builder::new()
                    .name(format!("sunlit-tiles-{index}"))
                    .spawn(move || work(&shared, &tx, &wake, &poked))
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
                    drag: None,
                    anisotropy: self.anisotropy,
                    stored: &stored,
                })
            }
        };
        self.inputs = Some(inputs);
        self.apply(wanted, target);
    }

    /// Make `wanted` the set in force: take it in order until the layers are
    /// spoken for, hold the page table to its cap, and publish what is
    /// missing.
    fn apply(&mut self, wanted: Wanted, target: &mut impl TileTarget) {
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
        target.set_cap(wanted.cap);
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

    /// Take the results that arrived, under the byte budget, and upload them,
    /// evicting where a layer is needed. Returns whether a tile was uploaded.
    pub(super) fn drain(&mut self, target: &mut impl TileTarget) -> bool {
        let Some(results) = self.results.clone() else {
            return false;
        };
        self.poked.store(false, Ordering::Release);
        let (mut spent, mut received, mut uploads) = (0, Vec::new(), Vec::new());
        while spent < UPLOAD_BUDGET {
            let Ok(loaded) = results.try_recv() else {
                break;
            };
            received.push(loaded.id);
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
        self.reads += received.len() as u64;
        if !received.is_empty() {
            let mut slot = self.shared.lock();
            for id in &received {
                slot.claimed.remove(id);
            }
        }
        let uploaded = self.upload(uploads, target);
        if self.republish {
            self.publish(target);
        }
        if more {
            (self.wake)();
        }
        uploaded
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
fn work(shared: &Shared, results: &Sender<Loaded>, wake: &NotifyFn, poked: &AtomicBool) {
    thread_priority::lower_current_thread("tile loader");
    while let Some(claim) = shared.next() {
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
        loader.apply(wanted(&tiles), &mut stand);

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
    fn a_tile_read_after_it_left_the_set_is_dropped() {
        let dir = ScratchDir::new("tile_loader_left");
        let pack = day_pack(&dir);
        let tiles = stored(&pack);
        let (before, after) = tiles.split_at(tiles.len() / 2);
        assert!(before.len() > RESULTS_PER_WORKER && !after.is_empty());
        let mut loader = loader(1, &pack);
        let mut stand = Stand::new(64);
        loader.apply(wanted(before), &mut stand);
        wait_until_blocked(&loader);

        loader.apply(wanted(after), &mut stand);
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
        loader.apply(wanted(&tiles), &mut stand);
        wait_until_blocked(&loader);

        loader.purge(&mut stand);
        assert_eq!(stand.purges, 1);
        loader.apply(wanted(&tiles), &mut stand);
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
        loader.apply(wanted(&tiles), &mut stand);
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
        loader.apply(wanted(&tiles), &mut stand);
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
        loader.apply(wanted(&tiles), &mut stand);
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
        loader.apply(wanted(&tiles), &mut stand);
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

        loader.apply(wanted(a), &mut stand);
        drain_until(&mut loader, &mut stand, "the first four", |s| {
            resident(s, a)
        });
        loader.apply(wanted(&a[2..]), &mut stand);
        loader.drain(&mut stand);
        assert!(
            stand.evicted.is_empty(),
            "nothing is evicted while no layer is needed"
        );

        for (step, &tile) in b.iter().enumerate() {
            loader.apply(wanted(&[tile]), &mut stand);
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
        loader.apply(set, &mut stand);

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
        loader.apply(wanted(&stored(&pack)), &mut stand);
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
}
