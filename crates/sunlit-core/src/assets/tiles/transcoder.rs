//! The worker that builds the tile packs in the background.
//!
//! A [`Transcoder`] owns one thread and one rayon pool of its own, both at
//! below-normal priority, and runs [`ensure_pack`] over every [`PackKind`], one
//! pack at a time:
//!
//! 1. It first opens every pack on disk and compares its key, which takes tens
//!    of milliseconds for all fourteen; each current pack is ready at once.
//! 2. It then builds the rest in rank order: the packs the first frame needs,
//!    which are the mask, the month in force and the night, and after them the
//!    other eleven months nearest that month first, the next month before the
//!    previous one.
//!
//! The month in force can change while it works. The queue follows it, and the
//! pack in progress is cancelled and put back only when it is a day pack other
//! than the new month in force and the new month's pack still waits to be
//! built. The mask, the night and the month in force are never cancelled for a
//! change of month.
//!
//! The pause gate is for the engine: while it is closed only the first-frame
//! packs start, and a build of any other pack in progress is cancelled at the
//! next point [`ensure_pack`] looks at its flag (between two face decodes or
//! two batches of 64 tiles) and starts over when the gate opens, since a
//! paused build would hold its decoded faces for as long as the gate stays
//! shut. Closing it costs that pack's progress, so it is meant for
//! spans of busy time rather than single frames.
//!
//! A pack that fails is reported with its reason and the others go on; it is
//! tried again on the next start, never in a loop. A textures directory that
//! does not hold the whole cube, such as a checkout without the Git LFS
//! objects, builds nothing at all. Dropping the handle cancels the pack in
//! progress and joins the thread; a pack is written under a temporary name and
//! renamed into place, so a cancel or a killed process leaves the previous pack
//! or none, and the next start sweeps what a killed one left, whether or not that pack needs a build.

use std::num::NonZeroUsize;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded};
use tracing::{error, info, warn};

use super::{BuildError, Ensured, GEOMETRY, Geometry, Pack, PackKind, ensure_pack, expected_key};
use crate::assets::cube_layout::{CubeTextures, MONTHS};
use crate::thread_priority;

/// Every pack there is: the twelve months, the night and the mask.
pub const PACKS: usize = MONTHS + 2;

/// The packs ranked below this are the ones the first frame needs.
const FIRST_FRAME: usize = 3;

/// Called on the worker thread after every change of the status, with the new
/// status, and never under the transcoder's lock. It should return promptly:
/// the worker waits for it.
///
/// It must never block on the thread that holds the [`Transcoder`], since
/// dropping the handle joins the worker and a callback waiting for the dropping
/// thread would never return; a wake-up that cannot block, such as a send on an
/// unbounded channel or a `try_send`, is what it is for. It must not panic
/// either: a panic there ends the worker outside the guard around a build, and
/// the phase it leaves behind never settles.
pub type TranscodeNotify = Arc<dyn Fn(&TranscodeStatus) + Send + Sync>;

/// What the worker is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Opening the packs on disk to find the current ones.
    Checking,
    /// Building a pack that was missing, stale or unreadable.
    Building(PackKind),
    /// Waiting at the closed pause gate with only the rest of the year left.
    Paused,
    /// Every pack is ready or has failed, and the worker has finished.
    Done,
    /// The handle was stopped or dropped before every pack was ready or had
    /// failed.
    Stopped,
    /// The textures directory does not hold every cube face, so there is no
    /// cube to build from and nothing is built.
    Unavailable(String),
}

/// A pack whose build failed in this run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackFailure {
    pub kind: PackKind,
    pub reason: String,
}

/// The latest word from the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscodeStatus {
    pub phase: Phase,
    /// The packs that can be read, in the order they became so.
    pub ready: Vec<PackKind>,
    pub failed: Vec<PackFailure>,
}

impl TranscodeStatus {
    #[must_use]
    pub fn is_ready(&self, kind: PackKind) -> bool {
        self.ready.contains(&kind)
    }

    /// Whether the worker has nothing more to do in this run.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        matches!(
            self.phase,
            Phase::Done | Phase::Stopped | Phase::Unavailable(_)
        )
    }
}

/// How many threads the pool gets unless told otherwise: the cores less two,
/// which leaves the engine and the UI theirs, and at most four, where a month
/// takes 3.7 s against 9.9 s on one thread and the decode stops gaining
/// (research section 21).
#[must_use]
pub fn default_threads() -> usize {
    let cores = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    cores.saturating_sub(2).clamp(1, 4)
}

/// Holds the transcoder at the start of each build, with the pack named in
/// its status, until the gate lets that build through.
///
/// Open unless someone holds it, and the app never does: an engine test holds
/// it to see each pack of a first run named while it builds, which a fixture
/// pack built in milliseconds would not leave time for. A build that waits
/// gives way to whatever would cancel it once it runs.
#[derive(Clone, Default)]
pub struct BuildGate(Arc<(Mutex<Option<usize>>, Condvar)>);

impl BuildGate {
    /// Hold every build that has not started.
    pub fn hold(&self) {
        *self.lock() = Some(0);
    }

    /// Let `builds` more builds through a held gate.
    pub fn allow(&self, builds: usize) {
        let mut permits = self.lock();
        if let Some(left) = permits.as_mut() {
            *left += builds;
        }
        drop(permits);
        self.0.1.notify_all();
    }

    /// Let every build through.
    pub fn open(&self) {
        *self.lock() = None;
        self.0.1.notify_all();
    }

    fn lock(&self) -> MutexGuard<'_, Option<usize>> {
        self.0
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Wait until a build may start. Returns `false` when `cancelled` says
    /// the build is to give way first.
    fn pass(&self, cancelled: impl Fn() -> bool) -> bool {
        let mut permits = self.lock();
        loop {
            match permits.as_mut() {
                None => return true,
                Some(left) if *left > 0 => {
                    *left -= 1;
                    return true;
                }
                Some(_) if cancelled() => return false,
                Some(_) => {
                    permits = self
                        .0
                        .1
                        .wait_timeout(permits, Duration::from_millis(10))
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0;
                }
            }
        }
    }
}

/// What a [`Transcoder`] builds from, and where.
pub struct TranscoderConfig {
    /// The app's cache directory; the packs go under its `tile_cache`.
    pub cache_dir: PathBuf,
    pub textures: CubeTextures,
    pub geometry: Geometry,
    /// The month in force, January 0.
    pub month: usize,
    /// The size of the pool the packs are built on.
    pub threads: usize,
    pub notify: TranscodeNotify,
    pub gate: BuildGate,
}

impl TranscoderConfig {
    /// The shipped geometry, [`default_threads`], nobody to notify and an
    /// open gate.
    #[must_use]
    pub fn new(cache_dir: PathBuf, textures: CubeTextures, month: usize) -> Self {
        Self {
            cache_dir,
            textures,
            geometry: GEOMETRY,
            month,
            threads: default_threads(),
            notify: Arc::new(|_| {}),
            gate: BuildGate::default(),
        }
    }
}

/// Where `kind` stands in the build order while `month` is in force, 0 first.
fn rank(kind: PackKind, month: usize) -> usize {
    match kind {
        PackKind::Mask => 0,
        PackKind::Day(m) if m == month => 1,
        PackKind::Night => 2,
        PackKind::Day(m) => {
            let ahead = (m + MONTHS - month) % MONTHS;
            let behind = MONTHS - ahead;
            FIRST_FRAME - 1 + 2 * ahead.min(behind) - usize::from(ahead <= behind)
        }
    }
}

/// Whether `kind` waits at the closed pause gate while `month` is in force.
fn gated(kind: PackKind, month: usize) -> bool {
    rank(kind, month) >= FIRST_FRAME
}

struct State {
    status: TranscodeStatus,
    month: usize,
    paused: bool,
    stopping: bool,
    /// The packs not yet ready, failed or in progress.
    waiting: Vec<PackKind>,
}

impl State {
    fn next(&self) -> Option<PackKind> {
        self.waiting
            .iter()
            .copied()
            .min_by_key(|&kind| rank(kind, self.month))
    }

    /// Whether the build in progress should give way to what just changed.
    fn should_cancel(&self) -> bool {
        let Phase::Building(kind) = self.status.phase else {
            return false;
        };
        let preempted = matches!(kind, PackKind::Day(m) if m != self.month)
            && self.waiting.contains(&PackKind::Day(self.month));
        self.stopping || preempted || (self.paused && gated(kind, self.month))
    }
}

struct Shared {
    state: Mutex<State>,
    /// Signalled on every change of the state, by either side.
    changed: Condvar,
    /// Handed to [`ensure_pack`]; raised to cancel the build in progress, and
    /// lowered under the lock when the next one starts, or when the reason to
    /// cancel has gone before the build saw it.
    cancel: AtomicBool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("the transcoder's lock is poisoned")
    }

    /// Change the controls under the lock, cancelling the build in progress
    /// when it should give way.
    fn control(&self, change: impl FnOnce(&mut State)) {
        let mut state = self.lock();
        change(&mut state);
        if state.should_cancel() {
            self.cancel.store(true, Ordering::Relaxed);
        } else if !state.stopping {
            self.cancel.store(false, Ordering::Relaxed);
        }
        self.changed.notify_all();
    }
}

/// The handle to the worker. Dropping it cancels the pack in progress and
/// joins the thread.
pub struct Transcoder {
    shared: Arc<Shared>,
    /// Each pack that became ready, once, in the order it did. Bounded at the
    /// number of packs, since a pack is ready at most once per start, so the
    /// worker's send never finds it full whenever its consumer, the holder of
    /// this handle, drains it.
    landed: Receiver<PackKind>,
    thread: Option<JoinHandle<()>>,
}

impl Transcoder {
    /// Start the worker. It checks and builds on its own thread and returns
    /// at once.
    ///
    /// # Panics
    ///
    /// When `config.month` is not a month, or the thread cannot be spawned.
    #[must_use]
    pub fn start(config: TranscoderConfig) -> Self {
        assert!(
            config.month < MONTHS,
            "month {} is not a month",
            config.month
        );
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                status: TranscodeStatus {
                    phase: Phase::Checking,
                    ready: Vec::new(),
                    failed: Vec::new(),
                },
                month: config.month,
                paused: false,
                stopping: false,
                waiting: PackKind::all().collect(),
            }),
            changed: Condvar::new(),
            cancel: AtomicBool::new(false),
        });
        let (tx, landed) = bounded(PACKS);
        let worker = Worker {
            shared: Arc::clone(&shared),
            landed: tx,
            config,
        };
        let thread = std::thread::Builder::new()
            .name("sunlit-transcoder".into())
            .spawn(move || worker.run())
            .expect("failed to spawn the transcoder thread");
        Self {
            shared,
            landed,
            thread: Some(thread),
        }
    }

    #[must_use]
    pub fn status(&self) -> TranscodeStatus {
        self.shared.lock().status.clone()
    }

    /// The packs that became ready since the last call, in the order they did.
    #[must_use]
    pub fn landed(&self) -> Vec<PackKind> {
        self.landed.try_iter().collect()
    }

    /// Make `month` the month in force, January 0.
    ///
    /// # Panics
    ///
    /// When `month` is not a month.
    pub fn set_month(&self, month: usize) {
        assert!(month < MONTHS, "month {month} is not a month");
        self.shared.control(|state| state.month = month);
    }

    /// Close or open the pause gate.
    pub fn set_paused(&self, paused: bool) {
        self.shared.control(|state| state.paused = paused);
    }

    /// Ask the worker to stop, cancelling the pack in progress, without
    /// waiting for it; dropping the handle waits.
    pub fn stop(&self) {
        self.shared.control(|state| state.stopping = true);
    }

    /// Block until the status satisfies `until`, or `timeout` passes, and
    /// return the status that did, or `None` on the timeout. For a caller with
    /// nothing else to do, such as a one-shot render.
    pub fn wait_until(
        &self,
        timeout: Duration,
        mut until: impl FnMut(&TranscodeStatus) -> bool,
    ) -> Option<TranscodeStatus> {
        let state = self.shared.lock();
        let (state, waited) = self
            .shared
            .changed
            .wait_timeout_while(state, timeout, |state| !until(&state.status))
            .expect("the transcoder's lock is poisoned");
        (!waited.timed_out()).then(|| state.status.clone())
    }
}

impl Drop for Transcoder {
    fn drop(&mut self) {
        self.stop();
        if let Some(thread) = self.thread.take()
            && let Err(payload) = thread.join()
        {
            error!(reason = %panic_reason(&*payload), "the transcoder thread panicked");
        }
    }
}

fn panic_reason(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic payload".to_owned())
}

/// The pool the packs are built on, its threads below normal priority as the
/// worker's is.
fn pool(threads: usize) -> Result<rayon::ThreadPool, rayon::ThreadPoolBuildError> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads.max(1))
        .thread_name(|i| format!("sunlit-transcoder-{i}"))
        .start_handler(|_| thread_priority::lower_current_thread("transcoder pool"))
        .build()
}

struct Worker {
    shared: Arc<Shared>,
    landed: Sender<PackKind>,
    config: TranscoderConfig,
}

impl Worker {
    fn run(self) {
        thread_priority::lower_current_thread("transcoder");
        let started = Instant::now();
        let textures = &self.config.textures;
        if !textures.is_complete() {
            let why = format!(
                "{} of the {} cube texture files are present",
                textures.found(),
                CubeTextures::total()
            );
            info!(reason = %why, "no tile packs: the cube is not available");
            let mut state = self.shared.lock();
            state.waiting.clear();
            state.status.phase = Phase::Unavailable(why);
            drop(self.publish(state));
            return;
        }
        let pool = match pool(self.config.threads) {
            Ok(pool) => pool,
            Err(e) => {
                warn!(error = %e, "no tile packs: the transcoder's pool could not be built");
                let mut state = self.shared.lock();
                let reason = format!("the transcoder's pool could not be built: {e}");
                let failed: Vec<_> = state
                    .waiting
                    .drain(..)
                    .map(|kind| PackFailure {
                        kind,
                        reason: reason.clone(),
                    })
                    .collect();
                state.status.failed = failed;
                state.status.phase = Phase::Done;
                drop(self.publish(state));
                return;
            }
        };
        info!(
            threads = pool.current_num_threads(),
            month = self.config.month,
            "the transcoder is checking the tile packs"
        );

        self.check_all();
        let state = self.build_all(&pool);
        info!(
            ready = state.status.ready.len(),
            failed = state.status.failed.len(),
            phase = ?state.status.phase,
            secs = format_args!("{:.2}", started.elapsed().as_secs_f64()),
            "the transcoder has finished"
        );
    }

    /// Wake the waiters and tell the notify callback, outside the lock, and
    /// take the lock back.
    fn publish<'a>(&'a self, state: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        let status = state.status.clone();
        self.shared.changed.notify_all();
        drop(state);
        (self.config.notify)(&status);
        self.shared.lock()
    }

    fn land(&self, state: &mut State, kind: PackKind) {
        state.waiting.retain(|&k| k != kind);
        state.status.ready.push(kind);
        if let Err(e) = self.landed.try_send(kind) {
            warn!(?kind, error = %e, "a landed pack could not be announced");
        }
    }

    fn is_current(&self, kind: PackKind) -> bool {
        let TranscoderConfig {
            cache_dir,
            textures,
            geometry,
            ..
        } = &self.config;
        expected_key(kind, textures, geometry).is_ok_and(|key| {
            Pack::open(&super::pack_path(cache_dir, kind)).is_ok_and(|pack| pack.key() == key)
        })
    }

    fn check_all(&self) {
        let mut order: Vec<_> = PackKind::all().collect();
        let month = self.shared.lock().month;
        order.sort_by_key(|&kind| rank(kind, month));
        for kind in order {
            if self.shared.lock().stopping {
                return;
            }
            if self.is_current(kind) {
                let mut state = self.shared.lock();
                self.land(&mut state, kind);
                drop(self.publish(state));
            }
        }
    }

    fn build_all(&self, pool: &rayon::ThreadPool) -> MutexGuard<'_, State> {
        let mut state = self.shared.lock();
        loop {
            let next = state.next();
            let phase = match next {
                None => Phase::Done,
                _ if state.stopping => Phase::Stopped,
                Some(kind) if state.paused && gated(kind, state.month) => Phase::Paused,
                Some(kind) => Phase::Building(kind),
            };
            let settled = matches!(phase, Phase::Stopped | Phase::Done);
            if phase == Phase::Paused && state.status.phase == Phase::Paused {
                state = self
                    .shared
                    .changed
                    .wait(state)
                    .expect("the transcoder's lock is poisoned");
                continue;
            }
            if let Phase::Building(kind) = phase {
                state.waiting.retain(|&k| k != kind);
                self.shared.cancel.store(false, Ordering::Relaxed);
            }
            state.status.phase = phase.clone();
            state = self.publish(state);
            if settled {
                return state;
            }
            let Phase::Building(kind) = phase else {
                continue;
            };
            drop(state);
            let result = self.build(pool, kind);
            state = self.shared.lock();
            match result {
                Ok(_) => self.land(&mut state, kind),
                Err(BuildError::Cancelled) => {
                    if !state.stopping {
                        info!(?kind, "a tile pack gave way and waits its turn again");
                    }
                    state.waiting.push(kind);
                }
                Err(e) => {
                    warn!(?kind, error = %e, "a tile pack could not be built");
                    state.status.failed.push(PackFailure {
                        kind,
                        reason: e.to_string(),
                    });
                }
            }
        }
    }

    /// Build one pack on the pool, a panic in the decoder or the encoder
    /// failing that pack alone.
    fn build(&self, pool: &rayon::ThreadPool, kind: PackKind) -> Result<Ensured, BuildError> {
        let TranscoderConfig {
            cache_dir,
            textures,
            geometry,
            ..
        } = &self.config;
        let cancel = &self.shared.cancel;
        if !self.config.gate.pass(|| cancel.load(Ordering::Relaxed)) {
            return Err(BuildError::Cancelled);
        }
        let run = || pool.install(|| ensure_pack(cache_dir, kind, textures, geometry, cancel));
        panic::catch_unwind(AssertUnwindSafe(run)).unwrap_or_else(|payload| {
            Err(BuildError::Failed(format!(
                "the build panicked: {}",
                panic_reason(&*payload)
            )))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use PackKind::{Day, Mask, Night};
    use crossbeam_channel::{Receiver, Sender};

    use super::*;
    use crate::assets::tiles::build::UNFINISHED_SUFFIX;
    use crate::assets::tiles::{CACHE_SUBDIR, FIXTURE, pack_path};
    use crate::test_support::{ScratchDir, write_cube_fixture};

    #[test]
    fn the_order_is_a_ranking_of_every_pack_for_every_month() {
        for month in 0..MONTHS {
            let mut ranks: Vec<_> = PackKind::all().map(|kind| rank(kind, month)).collect();
            ranks.sort_unstable();
            assert_eq!(ranks, (0..PACKS).collect::<Vec<_>>(), "month {month}");
        }
        assert_eq!(PackKind::all().count(), PACKS);
    }

    fn order(month: usize) -> Vec<PackKind> {
        let mut kinds: Vec<_> = PackKind::all().collect();
        kinds.sort_by_key(|&kind| rank(kind, month));
        kinds
    }

    #[test]
    fn the_first_frame_comes_first_and_then_the_nearest_months_next_first() {
        assert_eq!(
            order(4),
            [
                Mask,
                Day(4),
                Night,
                Day(5),
                Day(3),
                Day(6),
                Day(2),
                Day(7),
                Day(1),
                Day(8),
                Day(0),
                Day(9),
                Day(11),
                Day(10),
            ]
        );
        assert_eq!(
            order(11)[..7],
            [Mask, Day(11), Night, Day(0), Day(10), Day(1), Day(9)],
            "the year wraps"
        );
        assert_eq!(order(0)[13], Day(6), "the month opposite comes last");
    }

    #[test]
    fn only_the_rest_of_the_year_waits_at_the_gate() {
        for month in 0..MONTHS {
            let open: Vec<_> = PackKind::all().filter(|&k| !gated(k, month)).collect();
            assert_eq!(open, [Day(month), Night, Mask]);
        }
    }

    #[test]
    fn the_pool_leaves_two_cores_and_takes_at_most_four() {
        let threads = default_threads();
        assert!((1..=4).contains(&threads), "{threads}");
    }

    #[test]
    fn the_handle_can_move_to_and_be_shared_with_another_thread() {
        fn send_and_sync<T: Send + Sync>() {}
        send_and_sync::<Transcoder>();
    }

    /// Long enough for any fixture run, and only ever reached by a failure.
    const WAIT: Duration = Duration::from_secs(120);

    struct Setup {
        dir: ScratchDir,
        textures: CubeTextures,
    }

    impl Setup {
        fn new(name: &str) -> Self {
            let dir = ScratchDir::new(&format!("transcoder_{name}"));
            write_cube_fixture(&dir.join("textures"));
            let textures = CubeTextures::resolve(&dir.join("textures"));
            Self { dir, textures }
        }

        fn cache(&self) -> PathBuf {
            self.dir.join("cache")
        }

        fn start(&self, month: usize, probe: &Probe) -> Transcoder {
            Transcoder::start(TranscoderConfig {
                geometry: FIXTURE,
                threads: 2,
                notify: probe.notify(),
                ..TranscoderConfig::new(self.cache(), self.textures.clone(), month)
            })
        }

        /// The names in the pack directory, sorted.
        fn files(&self) -> Vec<String> {
            let mut names: Vec<_> = fs::read_dir(self.cache().join(CACHE_SUBDIR))
                .map(|entries| {
                    entries
                        .flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names
        }
    }

    fn file_names(kinds: &[PackKind]) -> Vec<String> {
        let mut names: Vec<_> = kinds.iter().map(|kind| kind.file_name()).collect();
        names.sort();
        names
    }

    type Seen = Vec<(TranscodeStatus, Option<std::cmp::Ordering>)>;

    /// The notify callback's view: every status the worker published, where
    /// the worker's priority stood each time, and a hold that stops the worker
    /// the first time it announces one phase until the test lets it go.
    struct Probe {
        seen: Arc<Mutex<Seen>>,
        hold: Option<Phase>,
        reached: (Sender<()>, Receiver<()>),
        release: (Sender<()>, Receiver<()>),
    }

    impl Probe {
        fn new() -> Self {
            Self {
                seen: Arc::default(),
                hold: None,
                reached: crossbeam_channel::bounded(1),
                release: crossbeam_channel::bounded(1),
            }
        }

        fn holding(phase: Phase) -> Self {
            Self {
                hold: Some(phase),
                ..Self::new()
            }
        }

        fn notify(&self) -> TranscodeNotify {
            let seen = Arc::clone(&self.seen);
            let armed = AtomicBool::new(self.hold.is_some());
            let hold = self.hold.clone();
            let reached = self.reached.0.clone();
            let release = self.release.1.clone();
            Arc::new(move |status: &TranscodeStatus| {
                let lowered = thread_priority::lowered();
                seen.lock().expect("seen").push((status.clone(), lowered));
                if hold.as_ref() == Some(&status.phase) && armed.swap(false, Ordering::SeqCst) {
                    reached.send(()).expect("the test waits for the hold");
                    release
                        .recv_timeout(WAIT)
                        .expect("the test lets the worker go");
                }
            })
        }

        /// Wait until the worker is held.
        fn reached(&self) {
            self.reached
                .1
                .recv_timeout(WAIT)
                .expect("the worker reaches the hold");
        }

        fn release(&self) {
            self.release.0.send(()).expect("the worker is held");
        }

        fn phases(&self) -> Vec<Phase> {
            let seen = self.seen.lock().expect("seen");
            seen.iter()
                .map(|(status, _)| status.phase.clone())
                .collect()
        }

        fn builds(&self, kind: PackKind) -> usize {
            self.phases()
                .iter()
                .filter(|&phase| *phase == Phase::Building(kind))
                .count()
        }
    }

    fn settled(transcoder: &Transcoder) -> TranscodeStatus {
        transcoder
            .wait_until(WAIT, TranscodeStatus::is_settled)
            .expect("the transcoder settles")
    }

    /// `first`, then the rest of `order(month)` in its order.
    fn then(first: &[PackKind], month: usize) -> Vec<PackKind> {
        let rest = order(month)
            .into_iter()
            .filter(|kind| !first.contains(kind));
        first.iter().copied().chain(rest).collect()
    }

    #[test]
    fn a_first_start_builds_every_pack_in_order_and_says_so() {
        let setup = Setup::new("order");
        let probe = Probe::new();
        let transcoder = setup.start(4, &probe);
        let status = settled(&transcoder);
        assert_eq!(status.phase, Phase::Done);
        assert_eq!(status.ready, order(4));
        assert!(status.failed.is_empty(), "{:?}", status.failed);
        assert_eq!(transcoder.landed(), order(4));
        assert!(transcoder.landed().is_empty(), "each pack lands once");
        drop(transcoder);

        let mut expected: Vec<_> = order(4).into_iter().map(Phase::Building).collect();
        expected.push(Phase::Done);
        assert_eq!(probe.phases(), expected);
        let seen = probe.seen.lock().expect("seen");
        for (i, (status, lowered)) in seen.iter().enumerate() {
            assert_eq!(status.ready, order(4)[..i], "the status after {i} builds");
            assert!(
                lowered.is_none_or(|o| o == std::cmp::Ordering::Equal),
                "the worker runs below normal priority: {lowered:?}"
            );
        }
        assert_eq!(setup.files(), file_names(&order(4)));
    }

    #[test]
    fn a_second_start_over_a_whole_cache_builds_nothing() {
        let setup = Setup::new("second_start");
        settled(&setup.start(0, &Probe::new()));
        let stamps = |setup: &Setup| -> Vec<_> {
            PackKind::all()
                .map(|kind| {
                    fs::metadata(pack_path(&setup.cache(), kind))
                        .and_then(|m| m.modified())
                        .expect("a pack")
                })
                .collect()
        };
        let before = stamps(&setup);

        let probe = Probe::new();
        let transcoder = setup.start(7, &probe);
        let status = settled(&transcoder);
        assert_eq!(status.phase, Phase::Done);
        assert_eq!(status.ready, order(7), "checked in the new month's order");
        assert_eq!(transcoder.landed(), order(7));
        let phases = probe.phases();
        assert!(
            !phases
                .iter()
                .any(|phase| matches!(phase, Phase::Building(_))),
            "{phases:?}"
        );
        assert_eq!(stamps(&setup), before, "no pack was written again");
    }

    #[test]
    fn a_new_month_takes_over_from_a_day_pack_in_progress() {
        let setup = Setup::new("month_preempts");
        let probe = Probe::holding(Phase::Building(Day(1)));
        let transcoder = setup.start(0, &probe);
        probe.reached();
        transcoder.set_month(6);
        probe.release();
        let status = settled(&transcoder);

        assert_eq!(status.ready, then(&[Mask, Day(0), Night], 6));
        assert_eq!(
            probe.builds(Day(1)),
            2,
            "February gave way and was built last"
        );
        assert_eq!(
            setup.files(),
            file_names(&order(6)),
            "no temporary file is left"
        );
    }

    #[test]
    fn a_new_month_waits_for_a_first_frame_pack_in_progress() {
        let setup = Setup::new("month_waits");
        let probe = Probe::holding(Phase::Building(Night));
        let transcoder = setup.start(0, &probe);
        probe.reached();
        transcoder.set_month(6);
        probe.release();
        let status = settled(&transcoder);

        assert_eq!(status.ready, then(&[Mask, Day(0), Night], 6));
        assert_eq!(probe.builds(Night), 1);
    }

    #[test]
    fn a_new_month_that_is_built_already_only_reorders_the_rest() {
        let setup = Setup::new("month_reorders");
        // With January in force the mask, January, the night and February come
        // first, then December, which is building when February comes into
        // force.
        let probe = Probe::holding(Phase::Building(Day(11)));
        let transcoder = setup.start(0, &probe);
        probe.reached();
        transcoder.set_month(1);
        probe.release();
        let status = settled(&transcoder);

        assert_eq!(
            status.ready,
            then(&[Mask, Day(0), Night, Day(1), Day(11)], 1)
        );
        assert_eq!(probe.builds(Day(11)), 1, "December was not cancelled");
    }

    #[test]
    fn the_rest_of_the_year_waits_while_the_gate_is_closed() {
        let setup = Setup::new("gate");
        let probe = Probe::holding(Phase::Building(Mask));
        let transcoder = setup.start(3, &probe);
        probe.reached();
        transcoder.set_paused(true);
        probe.release();

        let paused = transcoder
            .wait_until(WAIT, |s| s.phase == Phase::Paused)
            .expect("the worker stops at the gate");
        assert_eq!(
            paused.ready,
            [Mask, Day(3), Night],
            "the first frame's packs"
        );

        transcoder.set_month(8);
        let paused = transcoder
            .wait_until(WAIT, |s| s.phase == Phase::Paused && s.is_ready(Day(8)))
            .expect("the month in force is built behind the closed gate");
        assert_eq!(paused.ready, [Mask, Day(3), Night, Day(8)]);

        transcoder.set_paused(false);
        let status = settled(&transcoder);
        assert_eq!(status.phase, Phase::Done);
        assert_eq!(status.ready, then(&[Mask, Day(3), Night, Day(8)], 8));
    }

    #[test]
    fn closing_the_gate_cancels_a_build_of_the_rest_of_the_year() {
        let setup = Setup::new("gate_cancels");
        let probe = Probe::holding(Phase::Building(Day(4)));
        let transcoder = setup.start(3, &probe);
        probe.reached();
        transcoder.set_paused(true);
        probe.release();

        let paused = transcoder
            .wait_until(WAIT, |s| s.phase == Phase::Paused)
            .expect("the worker stops at the gate");
        assert_eq!(paused.ready, [Mask, Day(3), Night]);
        assert_eq!(
            setup.files(),
            file_names(&[Mask, Day(3), Night]),
            "May is not there, and neither is a temporary file"
        );

        transcoder.set_paused(false);
        let status = settled(&transcoder);
        assert_eq!(status.ready, order(3));
        assert_eq!(
            probe.builds(Day(4)),
            2,
            "May started over when the gate opened"
        );
    }

    #[test]
    fn a_month_that_comes_back_in_force_is_not_cancelled() {
        let setup = Setup::new("month_returns");
        let probe = Probe::holding(Phase::Building(Day(1)));
        let transcoder = setup.start(0, &probe);
        probe.reached();
        transcoder.set_month(6);
        transcoder.set_month(1);
        probe.release();
        let status = settled(&transcoder);

        assert_eq!(status.ready, then(&[Mask, Day(0), Night, Day(1)], 1));
        assert_eq!(probe.builds(Day(1)), 1, "February finished as it was");
    }

    #[test]
    fn a_gate_that_opens_again_at_once_cancels_nothing() {
        let setup = Setup::new("gate_flicker");
        let probe = Probe::holding(Phase::Building(Day(4)));
        let transcoder = setup.start(3, &probe);
        probe.reached();
        transcoder.set_paused(true);
        transcoder.set_paused(false);
        probe.release();
        let status = settled(&transcoder);

        assert_eq!(status.ready, order(3));
        assert_eq!(probe.builds(Day(4)), 1, "May was not started over");
    }

    #[test]
    fn a_stop_or_a_drop_at_the_closed_gate_ends_the_worker() {
        let paused = |name: &str| {
            let setup = Setup::new(name);
            let probe = Probe::holding(Phase::Building(Mask));
            let transcoder = setup.start(3, &probe);
            probe.reached();
            transcoder.set_paused(true);
            probe.release();
            transcoder
                .wait_until(WAIT, |s| s.phase == Phase::Paused)
                .expect("the worker stops at the gate");
            (setup, transcoder)
        };

        let (_setup, transcoder) = paused("gate_stop");
        let (done, stopped) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            transcoder.stop();
            let status = settled(&transcoder);
            drop(transcoder);
            let _ = done.send(status);
        });
        let status = stopped
            .recv_timeout(WAIT)
            .expect("a stop settles a worker waiting at the gate, and the drop joins it");
        assert_eq!(status.phase, Phase::Stopped);
        assert_eq!(status.ready, [Mask, Day(3), Night]);

        let (_setup, transcoder) = paused("gate_drop");
        let (done, dropped) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            drop(transcoder);
            let _ = done.send(());
        });
        dropped
            .recv_timeout(WAIT)
            .expect("dropping the handle joins a worker waiting at the gate");
    }

    #[test]
    fn a_pack_that_fails_is_reported_and_the_rest_are_built() {
        let setup = Setup::new("failure");
        let face = setup.textures.day[5][4].clone().expect("June's +Z face");
        let good = fs::read(&face).expect("read the face");
        fs::write(&face, b"not a JPEG XL file").expect("damage the face");

        let probe = Probe::new();
        let transcoder = setup.start(5, &probe);
        let status = settled(&transcoder);
        assert_eq!(status.phase, Phase::Done);
        assert_eq!(status.failed.len(), 1, "{:?}", status.failed);
        assert_eq!(status.failed[0].kind, Day(5));
        assert!(
            status.failed[0].reason.contains("pz.jxl"),
            "{}",
            status.failed[0].reason
        );
        let others: Vec<_> = order(5).into_iter().filter(|&k| k != Day(5)).collect();
        assert_eq!(status.ready, others);
        assert_eq!(probe.builds(Day(5)), 1, "tried once, not in a loop");
        drop(transcoder);

        fs::write(&face, good).expect("repair the face");
        let probe = Probe::new();
        let status = settled(&setup.start(5, &probe));
        let phases: Vec<_> = probe
            .phases()
            .into_iter()
            .filter(|phase| *phase != Phase::Checking)
            .collect();
        assert_eq!(phases, [Phase::Building(Day(5)), Phase::Done]);
        assert!(status.failed.is_empty());
        assert_eq!(status.ready.len(), PACKS);
    }

    #[test]
    fn a_cube_that_is_not_all_there_builds_nothing() {
        let mut setup = Setup::new("incomplete");
        let face = setup.textures.mask[0].clone().expect("the mask's +X face");
        fs::remove_file(face).expect("remove a face");
        setup.textures = CubeTextures::resolve(&setup.dir.join("textures"));

        let transcoder = setup.start(0, &Probe::new());
        let status = settled(&transcoder);
        let Phase::Unavailable(why) = &status.phase else {
            panic!("{status:?}");
        };
        assert!(why.contains("83 of the 84"), "{why}");
        assert!(status.ready.is_empty() && status.failed.is_empty());
        assert!(transcoder.landed().is_empty());
        assert!(!setup.cache().exists(), "nothing was written");
    }

    #[test]
    fn a_stop_mid_build_leaves_no_pack_and_the_next_start_sweeps_a_killed_one() {
        let setup = Setup::new("stop");
        let probe = Probe::holding(Phase::Building(Day(2)));
        let transcoder = setup.start(2, &probe);
        probe.reached();
        transcoder.stop();
        probe.release();
        let status = settled(&transcoder);
        drop(transcoder);
        assert_eq!(status.phase, Phase::Stopped);
        assert_eq!(status.ready, [Mask]);
        assert_eq!(
            setup.files(),
            ["mask.pack"],
            "March is not there, whole or in part"
        );

        // What a process killed while it wrote March leaves behind.
        let orphan =
            crate::files::unfinished(&pack_path(&setup.cache(), Day(2)), UNFINISHED_SUFFIX);
        fs::write(&orphan, b"half a pack").expect("write the orphan");
        let status = settled(&setup.start(2, &Probe::new()));
        assert_eq!(status.ready, order(2));
        assert_eq!(setup.files(), file_names(&order(2)), "the orphan is swept");
    }

    #[test]
    fn a_drop_at_any_point_of_a_build_leaves_whole_packs_or_none() {
        for round in 0..3 {
            let setup = Setup::new(&format!("drop_{round}"));
            let transcoder = setup.start(0, &Probe::new());
            transcoder
                .wait_until(WAIT, |s| s.is_ready(Mask))
                .expect("the mask is built first");
            drop(transcoder);

            for name in setup.files() {
                let kind = PackKind::all()
                    .find(|kind| kind.file_name() == name)
                    .unwrap_or_else(|| panic!("{name} is not a pack"));
                let key = expected_key(kind, &setup.textures, &FIXTURE).expect("a key");
                let pack = Pack::open(&pack_path(&setup.cache(), kind)).expect("a whole pack");
                assert_eq!(pack.key(), key, "{name}");
            }
        }
    }

    /// A held gate keeps each build named in the status and unstarted until
    /// it lets that build through, and a drop at a held gate ends the worker.
    #[test]
    fn a_held_build_gate_names_each_build_and_lets_through_what_it_is_told() {
        let setup = Setup::new("build_gate");
        let gate = BuildGate::default();
        gate.hold();
        let probe = Probe::new();
        let transcoder = Transcoder::start(TranscoderConfig {
            geometry: FIXTURE,
            threads: 2,
            notify: probe.notify(),
            gate: gate.clone(),
            ..TranscoderConfig::new(setup.cache(), setup.textures.clone(), 4)
        });
        let held = |kind| {
            let status = transcoder
                .wait_until(WAIT, |s| s.phase == Phase::Building(kind))
                .unwrap_or_else(|| panic!("{kind:?} was never named"));
            std::thread::sleep(Duration::from_millis(50));
            assert_eq!(transcoder.status(), status, "{kind:?} stays held");
            assert!(!pack_path(&setup.cache(), kind).exists(), "{kind:?}");
        };
        held(Mask);
        gate.allow(1);
        held(Day(4));
        assert_eq!(transcoder.status().ready, [Mask]);
        gate.allow(2);
        held(Day(5));
        assert_eq!(transcoder.status().ready, [Mask, Day(4), Night]);

        let dropped = Instant::now();
        drop(transcoder);
        assert!(dropped.elapsed() < Duration::from_secs(5), "{dropped:?}");
        assert!(!pack_path(&setup.cache(), Day(5)).exists());

        let transcoder = Transcoder::start(TranscoderConfig {
            geometry: FIXTURE,
            threads: 2,
            gate: gate.clone(),
            ..TranscoderConfig::new(setup.cache(), setup.textures.clone(), 4)
        });
        gate.open();
        let done = transcoder
            .wait_until(WAIT, TranscodeStatus::is_settled)
            .expect("an open gate lets the rest through");
        assert_eq!((done.phase, done.ready.len()), (Phase::Done, PACKS));
    }

    #[test]
    fn the_pool_runs_below_normal_priority() {
        let pool = pool(3).expect("a pool");
        for lowered in pool.broadcast(|_| thread_priority::lowered()) {
            assert!(
                lowered.is_none_or(|o| o == std::cmp::Ordering::Equal),
                "{lowered:?}"
            );
        }
    }
}
