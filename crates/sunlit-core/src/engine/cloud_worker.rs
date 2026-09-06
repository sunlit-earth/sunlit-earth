//! The thread that fetches and decodes the cloud texture, and the handle the
//! engine holds it by.
//!
//! It never touches the GPU. Everything here is network and JPEG work parked in
//! the mailbox for the engine loop to upload on its own schedule, which is what
//! keeps a slow fetch off the render path.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Sender, TryRecvError, bounded};
use tracing::{debug, info, warn};

use super::schedule::Schedule;
use crate::assets::cloud_fetcher::{CloudUpdater, PollOutcome, RetryBackoff};
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
    _thread: JoinHandle<()>,
}

impl CloudWorker {
    /// Point the worker at a different texture resolution's cloud variant.
    ///
    /// Returns whether that changed anything, so the caller can decide whether
    /// a poll is worth asking for.
    pub(super) fn retarget(&self, resolution: u32) -> bool {
        self.target.swap(resolution, Ordering::SeqCst) != resolution
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
    clouds_slot: usize,
) -> CloudWorker {
    info!(source = %source.describe(), poll_secs = interval.as_secs(), "cloud source configured");

    let (tx, rx) = bounded::<()>(1);
    let busy = Arc::new(AtomicBool::new(false));
    let worker_busy = Arc::clone(&busy);
    let target = Arc::new(AtomicU32::new(texture_resolution));
    let worker_target = Arc::clone(&target);

    let thread = std::thread::Builder::new()
        .name("sunlit-cloud".into())
        .spawn(move || {
            let mut updater = CloudUpdater::new(
                source,
                mailbox,
                notify,
                clouds_slot,
                cache_dir,
                texture_resolution,
            );
            let mut backoff = RetryBackoff::new();
            // Show whatever is on disk before touching the network.
            updater.post_cached();
            while rx.recv().is_ok() {
                // Retry a failed poll with exponential backoff rather than
                // waiting out the whole poll interval, which in production is
                // an hour: a thirty-second outage should not cost an hour of
                // stale clouds. The engine's own requests are dropped while
                // this runs (the worker is busy) and retried on the next tick.
                loop {
                    // Inside the retry loop, not outside it: an outage can hold
                    // this thread for minutes at a time, and a resolution
                    // switch made during one should change what the next
                    // attempt asks for rather than wait its turn.
                    updater.set_resolution(worker_target.load(Ordering::SeqCst));
                    match updater.poll_once() {
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
                            std::thread::sleep(delay);
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
        _thread: thread,
    }
}
