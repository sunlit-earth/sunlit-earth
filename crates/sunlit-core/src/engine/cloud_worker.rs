//! The thread that fetches and decodes the cloud texture, and the handle the
//! engine holds it by.
//!
//! It never touches the GPU. Everything here is network and JPEG work parked in
//! the mailbox for the engine loop to upload on its own schedule, which is what
//! keeps a slow fetch off the render path.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Sender, TryRecvError, bounded};
use tracing::{debug, info, warn};

use super::schedule::Schedule;
use crate::assets::cloud_fetcher::{CloudSlots, CloudUpdater, PollOutcome, RetryBackoff};
use crate::assets::cloud_source::CloudSource;
use crate::assets::mailbox::TextureMailbox;

/// The cloud fetch thread and the flag that keeps requests from piling up.
pub(super) struct CloudWorker {
    tx: Sender<()>,
    busy: Arc<AtomicBool>,
    /// The texture resolution the worker should be fetching the variant of.
    ///
    /// Read by the worker before every poll rather than sent as a message, so a
    /// switch that lands while a fetch is in flight (or while a failed poll is
    /// backing off) takes effect on the next attempt instead of queueing behind
    /// one that may be minutes from finishing.
    target: Arc<AtomicU32>,
    pub(super) schedule: Schedule,
    /// A poll came due but the worker was busy, so it still owes one. Kept
    /// separate from the schedule so a busy worker does not drag the deadline
    /// backwards and forwards on every tick.
    pub(super) owed: bool,
    /// What the worker's first attempt came to, a [`FirstAnswer`] as its
    /// discriminant. Written once by the worker, before a failed attempt's
    /// backoff, and read by the engine's tick.
    answer: Arc<AtomicU8>,
    /// Whether the cloud layer is switched on, which the engine writes on
    /// every tick and the worker reads before every attempt and while it
    /// backs off, so a retry stops when the layer is switched off.
    enabled: Arc<AtomicBool>,
    /// The worker ended a request without finishing it because the layer was
    /// switched off. The engine takes it and owes the poll again.
    abandoned: Arc<AtomicBool>,
    _thread: JoinHandle<()>,
}

/// What the worker's first attempt came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum FirstAnswer {
    /// No attempt has finished yet.
    Pending,
    /// A map is on its way to the mailbox, from the cache or the network.
    Posted,
    /// There was no cached map and the poll failed, so nothing is coming until
    /// a retry succeeds.
    Unavailable,
}

impl FirstAnswer {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Posted,
            2 => Self::Unavailable,
            _ => Self::Pending,
        }
    }
}

impl CloudWorker {
    /// What the worker's first attempt came to, so far.
    pub(super) fn first_answer(&self) -> FirstAnswer {
        FirstAnswer::from_u8(self.answer.load(Ordering::SeqCst))
    }

    /// Point the worker at a different texture resolution's cloud variant.
    ///
    /// Returns whether that changed anything, so the caller can decide whether
    /// a poll is worth asking for.
    pub(super) fn retarget(&self, resolution: u32) -> bool {
        self.target.swap(resolution, Ordering::SeqCst) != resolution
    }

    /// Tell the worker whether the cloud layer is switched on.
    pub(super) fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }

    /// Whether the worker gave up a request because the layer was switched
    /// off, clearing the flag.
    pub(super) fn take_abandoned(&self) -> bool {
        self.abandoned.swap(false, Ordering::SeqCst)
    }

    /// Ask the worker for one poll. Returns `false` when the worker is still
    /// busy with the previous one, so the caller can retry soon.
    pub(super) fn request(&self) -> bool {
        if self.busy.swap(true, Ordering::SeqCst) {
            return false;
        }
        if self.tx.send(()).is_err() {
            self.busy.store(false, Ordering::SeqCst);
            return false;
        }
        true
    }
}

/// Spawn the thread that does cloud network I/O and JPEG decoding.
///
/// It never touches the GPU: it parks decoded frames in the mailbox and pokes
/// the engine, which uploads them on its own schedule.
#[expect(
    clippy::too_many_arguments,
    reason = "the worker's whole configuration, handed over once at spawn"
)]
pub(super) fn spawn_cloud_worker(
    source: Arc<dyn CloudSource>,
    mailbox: TextureMailbox,
    notify: crate::assets::cloud_fetcher::NotifyFn,
    cache_dir: Option<PathBuf>,
    interval: Duration,
    now: Duration,
    texture_resolution: u32,
    slots: CloudSlots,
    enabled: bool,
) -> CloudWorker {
    info!(source = %source.describe(), poll_secs = interval.as_secs(), "cloud source configured");

    let (tx, rx) = bounded::<()>(1);
    let busy = Arc::new(AtomicBool::new(false));
    let worker_busy = Arc::clone(&busy);
    let target = Arc::new(AtomicU32::new(texture_resolution));
    let worker_target = Arc::clone(&target);
    let answer = Arc::new(AtomicU8::new(FirstAnswer::Pending as u8));
    let worker_answer = Arc::clone(&answer);
    let enabled = Arc::new(AtomicBool::new(enabled));
    let worker_enabled = Arc::clone(&enabled);
    let abandoned = Arc::new(AtomicBool::new(false));
    let worker_abandoned = Arc::clone(&abandoned);

    let thread = std::thread::Builder::new()
        .name("sunlit-cloud".into())
        .spawn(move || {
            let mut updater = CloudUpdater::new(
                source,
                mailbox,
                notify,
                slots,
                cache_dir,
                texture_resolution,
            );
            let mut backoff = RetryBackoff::new();
            let mut first = true;
            let give_up = || {
                info!("the cloud layer is switched off, the poll waits until it is on again");
                worker_abandoned.store(true, Ordering::SeqCst);
            };
            while rx.recv().is_ok() {
                if !worker_enabled.load(Ordering::SeqCst) {
                    give_up();
                    worker_busy.store(false, Ordering::SeqCst);
                    continue;
                }
                // Show whatever is on disk before touching the network, on the
                // first request rather than at spawn, so a session that starts
                // with the layer switched off decodes nothing until it is on.
                // A resolution switched before then posts its own entry.
                if std::mem::take(&mut first)
                    && !updater.set_resolution(worker_target.load(Ordering::SeqCst))
                {
                    updater.post_cached();
                }
                // Retry a failed poll with exponential backoff rather than
                // waiting out the whole poll interval, which in production is
                // an hour: a thirty-second outage should not cost an hour of
                // stale clouds. The engine's own requests are dropped while
                // this runs (the worker is busy) and retried on the next tick.
                loop {
                    // Before every attempt, so a layer switched off during an
                    // outage makes no further one; a poll already under way
                    // runs to its end and posts.
                    if !worker_enabled.load(Ordering::SeqCst) {
                        give_up();
                        break;
                    }
                    // Inside the retry loop, not outside it: an outage can hold
                    // this thread for minutes at a time, and a resolution
                    // switch made during one should change what the next
                    // attempt asks for rather than wait its turn.
                    updater.set_resolution(worker_target.load(Ordering::SeqCst));
                    let outcome = updater.poll_once();
                    if worker_answer.load(Ordering::SeqCst) == FirstAnswer::Pending as u8 {
                        let answer = if updater.has_posted() {
                            FirstAnswer::Posted
                        } else {
                            FirstAnswer::Unavailable
                        };
                        worker_answer.store(answer as u8, Ordering::SeqCst);
                    }
                    match outcome {
                        PollOutcome::Updated => {
                            debug!("cloud frame updated");
                            backoff.reset();
                            break;
                        }
                        PollOutcome::Unchanged => {
                            backoff.reset();
                            break;
                        }
                        PollOutcome::Failed => {
                            let delay = backoff.fail();
                            warn!(
                                retry_delay_secs = delay.as_secs(),
                                "cloud poll failed, retrying"
                            );
                            rest_while_enabled(delay, &worker_enabled);
                            // Give up if the engine went away while we slept.
                            if matches!(rx.try_recv(), Err(TryRecvError::Disconnected)) {
                                return;
                            }
                        }
                    }
                }
                worker_busy.store(false, Ordering::SeqCst);
            }
        })
        .expect("failed to spawn cloud worker thread");

    CloudWorker {
        tx,
        busy,
        target,
        // Poll immediately on the first tick, then on the configured interval.
        schedule: Schedule {
            interval,
            next: now,
        },
        owed: false,
        answer,
        enabled,
        abandoned,
        _thread: thread,
    }
}

/// How often a backoff looks at whether the layer is still switched on.
const REST_SLICE: Duration = Duration::from_millis(100);

/// Sleep out `delay`, or less as soon as `enabled` reads false.
fn rest_while_enabled(delay: Duration, enabled: &AtomicBool) {
    let until = std::time::Instant::now() + delay;
    while enabled.load(Ordering::SeqCst) {
        let left = until.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return;
        }
        std::thread::sleep(left.min(REST_SLICE));
    }
}
