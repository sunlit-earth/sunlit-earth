//! Checking for a newer release.
//!
//! One thread, `sunlit-update`, asks `https://sunlit.earth/releases/v1.json`
//! once a day and whenever the user asks, keeps the answer on disk, and reports
//! what it says to one callback. It has no part in rendering and the engine
//! knows nothing of it. Its consumer is the app's update controller, and it
//! runs for as long as the app does, with scheduled checks on or off, because
//! the manual check goes through it too.

pub mod checker;
pub mod feed;
pub mod source;
pub mod state;

use std::sync::Arc;

use crossbeam_channel::{RecvTimeoutError, Sender, TrySendError, bounded};
use tracing::warn;

pub use checker::{
    Checker, EventSink, Request, UpdateEvent, UpdateStatus, current_version, scheduled_checks,
    update_url,
};
pub use feed::{Asset, Offer, Release, asset_key};
pub use semver::Version;
pub use source::{Fetched, HttpUpdateSource, UpdateSource};
pub use state::Store;

use crate::engine::clock::{Clock, SystemClock};

/// How many requests may wait for the thread. They are clicks, a handful in a
/// session, so a full queue means the thread is stuck in a request and one
/// more click would add nothing.
const REQUEST_CAPACITY: usize = 8;

/// The app's end of the update thread. Dropping it ends the thread at its next
/// wake; a request in flight is left to its timeout, and nothing joins it.
pub struct UpdateHandle {
    requests: Sender<Request>,
}

/// The update thread is not there to take a request: it could not be started,
/// or it has ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreadEnded;

impl std::fmt::Display for ThreadEnded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the update checker is not running")
    }
}

impl std::error::Error for ThreadEnded {}

impl UpdateHandle {
    /// Hand `request` to the thread. A full queue is not an error: the thread
    /// is then in a check that answers this request too.
    pub fn send(&self, request: Request) -> Result<(), ThreadEnded> {
        match self.requests.try_send(request) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(request)) => {
                warn!(?request, "the update thread is busy, dropping the request");
                Ok(())
            }
            Err(TrySendError::Disconnected(request)) => {
                warn!(?request, "the update thread has ended");
                Err(ThreadEnded)
            }
        }
    }
}

/// Start the update thread against the real release file and the cache
/// directory, with scheduled checks if `enabled` and the environment agree.
pub fn spawn(enabled: bool, on_event: EventSink) -> UpdateHandle {
    spawn_with(
        Arc::new(SystemClock::new()),
        Box::new(HttpUpdateSource::new(update_url())),
        Store::new(crate::assets::cloud_fetcher::cache_dir()),
        current_version(),
        scheduled_checks(enabled),
        on_event,
    )
}

/// [`spawn`] with every part injected.
pub fn spawn_with(
    clock: Arc<dyn Clock>,
    source: Box<dyn UpdateSource>,
    store: Store,
    current: Version,
    scheduled: bool,
    on_event: EventSink,
) -> UpdateHandle {
    let (requests, pending) = bounded(REQUEST_CAPACITY);
    let spawned = std::thread::Builder::new()
        .name("sunlit-update".into())
        .spawn(move || {
            let mut checker = Checker::new(clock, source, store, current, scheduled, on_event);
            checker.start();
            loop {
                match pending.recv_timeout(checker.wait()) {
                    Ok(request) => checker.handle(request, &pending),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
                checker.tick(&pending);
            }
        });
    if let Err(e) = spawned {
        warn!(error = %e, "could not start the update thread, so there will be no update checks");
    }
    UpdateHandle { requests }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;
    use crate::engine::clock::MockClock;
    use crate::test_support::ScratchDir;

    struct Fixed;

    impl UpdateSource for Fixed {
        fn fetch(&self, _: Option<&str>) -> Result<Fetched, String> {
            Ok(Fetched::Body {
                bytes: include_bytes!("../../tests/fixtures/update_feed_v1.json").to_vec(),
                etag: None,
            })
        }
    }

    #[test]
    fn the_thread_answers_check_now_through_the_callback_and_ends_with_its_handle() {
        let dir = ScratchDir::new("update_thread");
        let (tx, rx) = mpsc::channel();
        let handle = spawn_with(
            Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH)),
            Box::new(Fixed),
            Store::new(Some(dir.path().to_owned())),
            Version::new(0, 5, 0),
            false,
            Box::new(move |event| {
                let _ = tx.send(event);
            }),
        );
        handle.send(Request::CheckNow).unwrap();
        let event = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(event.manual);
        assert!(matches!(event.status, UpdateStatus::Available(_)));

        drop(handle);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn a_handle_without_its_thread_says_so_and_a_busy_one_does_not() {
        let (requests, pending) = bounded(1);
        let handle = UpdateHandle { requests };
        assert_eq!(handle.send(Request::CheckNow), Ok(()));
        assert_eq!(handle.send(Request::CheckNow), Ok(()));
        drop(pending);
        assert_eq!(handle.send(Request::CheckNow), Err(ThreadEnded));
    }
}
