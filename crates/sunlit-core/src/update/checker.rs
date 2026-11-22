//! When a check is due, and what one does.
//!
//! The checker owns the kept state and answers to the update thread's loop,
//! which wakes it on every request and at least every [`WAKE_INTERVAL`]. It
//! asks the injected [`Clock`] what is due and never sleeps on wall time, so a
//! test walks days of schedule with a [`MockClock`](crate::engine::clock::MockClock)
//! and a scripted source.

use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Receiver;
use semver::Version;
use tracing::{info, warn};

use super::feed::{self, Feed, Offer, Verdict};
use super::source::{Fetched, UpdateSource};
use super::state::{Store, UpdateState};
use crate::engine::clock::Clock;

/// How old the last successful check may get before the next one.
pub const CHECK_INTERVAL: Duration = Duration::from_hours(24);
/// How long after the start the first check waits, so it never competes with
/// the first frame.
pub const STARTUP_DELAY: Duration = Duration::from_secs(30);
/// How long after a failed scheduled check the next attempt waits.
pub const RETRY_DELAY: Duration = Duration::from_hours(1);
/// The longest the thread waits for a request before asking whether a check is
/// due.
pub const WAKE_INTERVAL: Duration = Duration::from_mins(1);

const DEFAULT_UPDATE_URL: &str = "https://sunlit.earth/releases/v1.json";
/// Environment variable replacing the release file's URL.
const ENV_UPDATE_URL: &str = "SUNLIT_EARTH_UPDATE_URL";
/// Environment variable whose presence turns scheduled checks off.
const ENV_NO_UPDATE_CHECK: &str = "SUNLIT_EARTH_NO_UPDATE_CHECK";

fn resolve_update_url(raw: Option<&str>) -> String {
    raw.unwrap_or(DEFAULT_UPDATE_URL).to_owned()
}

/// The release file's URL, honoring `SUNLIT_EARTH_UPDATE_URL`.
pub fn update_url() -> String {
    resolve_update_url(crate::env_override(ENV_UPDATE_URL).as_deref())
}

fn resolve_scheduled(enabled: bool, suppressed: bool) -> bool {
    enabled && !suppressed
}

/// Whether checks run on a schedule: `enabled` from the config, unless
/// `SUNLIT_EARTH_NO_UPDATE_CHECK` is set to anything at all.
pub fn scheduled_checks(enabled: bool) -> bool {
    resolve_scheduled(enabled, std::env::var_os(ENV_NO_UPDATE_CHECK).is_some())
}

/// The version this build was compiled as.
pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the workspace version is SemVer")
}

/// What the app asks of the update thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Check now, whatever the schedule says, and report the outcome even when
    /// it is a failure.
    CheckNow,
    /// Stop pointing at this version in the tray and the banner.
    Skip(String),
    /// Stop showing the banner for this version.
    Dismiss(String),
}

/// What the last check, or the kept file, says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    UpToDate,
    Available(Offer),
    /// Only ever reported for a check somebody asked for.
    Failed(String),
}

/// One report from the update thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateEvent {
    pub status: UpdateStatus,
    /// Whether this answers a [`Request::CheckNow`].
    pub manual: bool,
    pub skipped_version: Option<String>,
    pub dismissed_version: Option<String>,
}

/// A callback taking every report, run on the update thread.
pub type EventSink = Box<dyn Fn(UpdateEvent) + Send>;

pub struct Checker {
    clock: Arc<dyn Clock>,
    source: Box<dyn UpdateSource>,
    store: Store,
    current: Version,
    scheduled: bool,
    state: UpdateState,
    /// When the next scheduled attempt may run after a failure, on the clock's
    /// elapsed time. Not kept: a restart starts the schedule over.
    retry_at: Option<Duration>,
    emit: EventSink,
}

impl Checker {
    /// Read the kept state. Reports nothing until [`Self::start`].
    pub fn new(
        clock: Arc<dyn Clock>,
        source: Box<dyn UpdateSource>,
        store: Store,
        current: Version,
        scheduled: bool,
        emit: EventSink,
    ) -> Self {
        let (state, _) = store.load();
        Self {
            clock,
            source,
            store,
            current,
            scheduled,
            state,
            retry_at: None,
            emit,
        }
    }

    /// Report what the kept file says, before any request, so a known update
    /// shows the moment the app is up. Says nothing when nothing is kept.
    pub fn start(&self) {
        if let Some(kept) = self.store.load_feed() {
            self.report(Ok(&kept), false);
        }
    }

    /// How long the thread may wait for a request before calling [`Self::tick`].
    pub fn wait(&self) -> Duration {
        let now = self.clock.elapsed();
        [Some(STARTUP_DELAY), self.retry_at]
            .into_iter()
            .flatten()
            .filter_map(|at| at.checked_sub(now).filter(|left| !left.is_zero()))
            .fold(WAKE_INTERVAL, Duration::min)
    }

    /// Whether a scheduled check is due now.
    pub fn due(&self) -> bool {
        let now = self.clock.elapsed();
        if !self.scheduled || now < STARTUP_DELAY {
            return false;
        }
        if self.retry_at.is_some_and(|at| now < at) {
            return false;
        }
        let Some(last) = self.state.last_checked_at() else {
            return true;
        };
        let wall = self.clock.now_utc();
        last > wall || wall - last >= CHECK_INTERVAL
    }

    /// Run a scheduled check if one is due.
    pub fn tick(&mut self, pending: &Receiver<Request>) {
        if self.due() {
            self.check(false, pending);
        }
    }

    pub fn handle(&mut self, request: Request, pending: &Receiver<Request>) {
        match request {
            Request::CheckNow => self.check(true, pending),
            Request::Skip(version) => {
                self.state.skipped_version = Some(version);
                self.store.save_state(&self.state);
            }
            Request::Dismiss(version) => {
                self.state.dismissed_version = Some(version);
                self.store.save_state(&self.state);
            }
        }
    }

    /// One check, reported when it succeeds or when it was asked for.
    ///
    /// Requests that arrived while the request was out are taken here: a
    /// `CheckNow` among them is answered by this check, which is as fresh as
    /// one made after it would be.
    fn check(&mut self, manual: bool, pending: &Receiver<Request>) {
        let result = self.fetch();
        let mut manual = manual;
        for request in pending.try_iter() {
            if request == Request::CheckNow {
                manual = true;
            } else {
                self.handle(request, pending);
            }
        }
        match &result {
            Ok(_) => {
                self.retry_at = None;
                info!(manual, "checked for updates");
            }
            Err(reason) => {
                self.retry_at = Some(self.clock.elapsed() + RETRY_DELAY);
                warn!(manual, reason, "the update check failed");
            }
        }
        if result.is_ok() || manual {
            self.report(result.as_ref(), manual);
        }
    }

    fn fetch(&mut self) -> Result<Feed, String> {
        let feed = match self.source.fetch(self.state.etag.as_deref())? {
            Fetched::Unchanged => {
                let Some(kept) = self.store.load_feed() else {
                    self.state.etag = None;
                    self.store.save_state(&self.state);
                    return Err("the server said nothing changed, and nothing is kept".to_owned());
                };
                kept
            }
            Fetched::Body { bytes, etag } => {
                let feed = feed::parse(&bytes)?;
                self.state.etag = if self.store.save_feed(&bytes) {
                    etag
                } else {
                    None
                };
                feed
            }
        };
        self.state.set_last_checked(self.clock.now_utc());
        self.store.save_state(&self.state);
        Ok(feed)
    }

    fn report(&self, result: Result<&Feed, &String>, manual: bool) {
        let status = match result {
            Ok(feed) => match feed.verdict(&self.current) {
                Verdict::UpToDate => UpdateStatus::UpToDate,
                Verdict::Available(offer) => UpdateStatus::Available(offer),
            },
            Err(reason) => UpdateStatus::Failed(reason.clone()),
        };
        (self.emit)(UpdateEvent {
            status,
            manual,
            skipped_version: self.state.skipped_version.clone(),
            dismissed_version: self.state.dismissed_version.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use crossbeam_channel::{Sender, bounded};
    use time::OffsetDateTime;

    use super::*;
    use crate::engine::clock::MockClock;
    use crate::test_support::ScratchDir;

    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/update_feed_v1.json");
    /// Below every version in the fixture, so the fixture offers its newest.
    const OLD: &str = "0.5.0";
    const START: OffsetDateTime = time::macros::datetime!(2027-02-01 12:00 UTC);

    #[derive(Default)]
    struct Script {
        answers: Mutex<VecDeque<Result<Fetched, String>>>,
        asked: Mutex<Vec<Option<String>>>,
    }

    struct ScriptedSource(Arc<Script>);

    impl UpdateSource for ScriptedSource {
        fn fetch(&self, known_etag: Option<&str>) -> Result<Fetched, String> {
            self.0
                .asked
                .lock()
                .unwrap()
                .push(known_etag.map(ToOwned::to_owned));
            self.0
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err("the script ran out".to_owned()))
        }
    }

    struct Rig {
        dir: ScratchDir,
        clock: Arc<MockClock>,
        script: Arc<Script>,
        events: Arc<Mutex<Vec<UpdateEvent>>>,
        sender: Sender<Request>,
        pending: Receiver<Request>,
    }

    impl Rig {
        fn new(name: &str) -> Self {
            let (sender, pending) = bounded(8);
            Self {
                dir: ScratchDir::new(name),
                clock: Arc::new(MockClock::new(START)),
                script: Arc::default(),
                events: Arc::default(),
                sender,
                pending,
            }
        }

        fn store(&self) -> Store {
            Store::new(Some(self.dir.path().to_owned()))
        }

        fn checker(&self, current: &str, scheduled: bool) -> Checker {
            let events = Arc::clone(&self.events);
            Checker::new(
                self.clock.clone(),
                Box::new(ScriptedSource(Arc::clone(&self.script))),
                self.store(),
                Version::parse(current).unwrap(),
                scheduled,
                Box::new(move |event| events.lock().unwrap().push(event)),
            )
        }

        fn answer(&self, answer: Result<Fetched, String>) {
            self.script.answers.lock().unwrap().push_back(answer);
        }

        fn answer_body(&self, body: &[u8], etag: &str) {
            self.answer(Ok(Fetched::Body {
                bytes: body.to_vec(),
                etag: Some(etag.to_owned()),
            }));
        }

        fn asked(&self) -> Vec<Option<String>> {
            self.script.asked.lock().unwrap().clone()
        }

        fn events(&self) -> Vec<UpdateEvent> {
            self.events.lock().unwrap().clone()
        }

        fn last_checked_ago(&self, ago: Duration) {
            let mut state = self.store().load().0;
            state.set_last_checked(START - ago);
            self.store().save_state(&state);
        }

        fn run_for(&self, checker: &mut Checker, span: Duration) {
            let end = self.clock.elapsed() + span;
            while self.clock.elapsed() < end {
                self.clock.advance(WAKE_INTERVAL);
                checker.tick(&self.pending);
            }
        }
    }

    fn offered(event: &UpdateEvent) -> Option<String> {
        match &event.status {
            UpdateStatus::Available(offer) => Some(offer.latest().version.to_string()),
            _ => None,
        }
    }

    #[test]
    fn the_overrides_resolve() {
        assert_eq!(resolve_update_url(None), DEFAULT_UPDATE_URL);
        assert_eq!(
            resolve_update_url(Some("http://127.0.0.1:9/v1.json")),
            "http://127.0.0.1:9/v1.json"
        );
        assert!(resolve_scheduled(true, false));
        assert!(!resolve_scheduled(true, true));
        assert!(!resolve_scheduled(false, false));
    }

    #[test]
    fn no_check_runs_before_the_startup_delay() {
        let rig = Rig::new("update_startup_delay");
        let mut checker = rig.checker(OLD, true);
        rig.answer_body(FIXTURE, "\"a\"");
        rig.clock
            .advance(STARTUP_DELAY.saturating_sub(Duration::from_secs(1)));
        checker.tick(&rig.pending);
        assert_eq!(rig.asked(), Vec::<Option<String>>::new());
        assert_eq!(checker.wait(), Duration::from_secs(1));

        rig.clock.advance(Duration::from_secs(1));
        checker.tick(&rig.pending);
        assert_eq!(rig.asked(), [None]);
        assert_eq!(offered(&rig.events()[0]).as_deref(), Some("0.7.0"));
        assert!(!rig.events()[0].manual);
    }

    #[test]
    fn a_check_runs_after_the_delay_when_the_last_one_is_a_day_old_never_made_or_in_the_future() {
        for (name, ago) in [
            ("update_day_old", Some(CHECK_INTERVAL)),
            ("update_never", None),
            ("update_future", Some(Duration::ZERO)),
        ] {
            let rig = Rig::new(name);
            match ago {
                Some(Duration::ZERO) => {
                    let mut state = UpdateState::default();
                    state.set_last_checked(START + Duration::from_hours(5));
                    rig.store().save_state(&state);
                }
                Some(ago) => rig.last_checked_ago(ago),
                None => {}
            }
            let mut checker = rig.checker(OLD, true);
            rig.answer_body(FIXTURE, "\"a\"");
            rig.run_for(&mut checker, STARTUP_DELAY);
            assert_eq!(rig.asked().len(), 1, "{name}");
        }
    }

    #[test]
    fn no_check_runs_when_the_last_one_is_an_hour_old_and_the_next_runs_a_day_after_it() {
        let rig = Rig::new("update_hour_old");
        rig.last_checked_ago(Duration::from_hours(1));
        let mut checker = rig.checker(OLD, true);
        rig.answer_body(FIXTURE, "\"a\"");
        rig.answer(Ok(Fetched::Unchanged));

        rig.run_for(&mut checker, Duration::from_hours(22));
        assert_eq!(rig.asked(), Vec::<Option<String>>::new());
        rig.run_for(&mut checker, Duration::from_hours(1) + WAKE_INTERVAL);
        assert_eq!(rig.asked().len(), 1);

        rig.run_for(
            &mut checker,
            CHECK_INTERVAL.saturating_sub(2 * WAKE_INTERVAL),
        );
        assert_eq!(rig.asked().len(), 1);
        rig.run_for(&mut checker, 2 * WAKE_INTERVAL);
        assert_eq!(rig.asked(), [None, Some("\"a\"".to_owned())]);
    }

    #[test]
    fn a_failure_says_nothing_tries_again_after_an_hour_and_leaves_the_kept_answer() {
        let rig = Rig::new("update_failure");
        rig.store().save_feed(FIXTURE);
        let mut checker = rig.checker(OLD, true);
        checker.start();
        assert_eq!(offered(&rig.events()[0]).as_deref(), Some("0.7.0"));

        rig.answer(Err("offline".to_owned()));
        rig.answer_body(FIXTURE, "\"a\"");
        rig.run_for(&mut checker, STARTUP_DELAY);
        assert_eq!(rig.asked().len(), 1);
        assert_eq!(rig.events().len(), 1);
        assert_eq!(
            checker.store.load_feed(),
            Some(feed::parse(FIXTURE).unwrap())
        );

        rig.run_for(&mut checker, RETRY_DELAY.saturating_sub(WAKE_INTERVAL));
        assert_eq!(rig.asked().len(), 1);
        rig.run_for(&mut checker, WAKE_INTERVAL);
        assert_eq!(rig.asked().len(), 2);
        assert_eq!(rig.events().len(), 2);
    }

    #[test]
    fn a_304_reads_the_kept_body() {
        let rig = Rig::new("update_304");
        let mut checker = rig.checker(OLD, false);
        rig.answer_body(FIXTURE, "\"a\"");
        rig.answer(Ok(Fetched::Unchanged));
        checker.handle(Request::CheckNow, &rig.pending);
        checker.handle(Request::CheckNow, &rig.pending);

        assert_eq!(rig.asked(), [None, Some("\"a\"".to_owned())]);
        let events = rig.events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].status, events[1].status);
        assert!(events.iter().all(|event| event.manual));
    }

    #[test]
    fn a_304_with_nothing_kept_fails_and_the_next_check_asks_unconditionally() {
        let rig = Rig::new("update_304_lost");
        let mut checker = rig.checker(OLD, false);
        rig.answer_body(FIXTURE, "\"a\"");
        rig.answer(Ok(Fetched::Unchanged));
        rig.answer_body(FIXTURE, "\"b\"");
        checker.handle(Request::CheckNow, &rig.pending);
        std::fs::remove_file(rig.dir.join("update_feed.json")).unwrap();
        checker.handle(Request::CheckNow, &rig.pending);
        checker.handle(Request::CheckNow, &rig.pending);

        assert_eq!(rig.asked(), [None, Some("\"a\"".to_owned()), None]);
        let events = rig.events();
        assert!(matches!(events[1].status, UpdateStatus::Failed(_)));
        assert!(offered(&events[2]).is_some());
    }

    #[test]
    fn a_body_that_does_not_read_fails_and_keeps_the_previous_one() {
        let rig = Rig::new("update_bad_body");
        let mut checker = rig.checker(OLD, false);
        rig.answer_body(FIXTURE, "\"a\"");
        rig.answer_body(b"<html>maintenance</html>", "\"b\"");
        checker.handle(Request::CheckNow, &rig.pending);
        checker.handle(Request::CheckNow, &rig.pending);

        assert!(matches!(rig.events()[1].status, UpdateStatus::Failed(_)));
        let (state, kept) = rig.store().load();
        assert_eq!(state.etag.as_deref(), Some("\"a\""));
        assert_eq!(kept, Some(feed::parse(FIXTURE).unwrap()));
    }

    #[test]
    fn check_now_checks_at_once_with_the_schedule_off_and_reports_a_failure() {
        let rig = Rig::new("update_manual");
        let mut checker = rig.checker(OLD, false);
        rig.answer(Err("no route to host".to_owned()));
        checker.handle(Request::CheckNow, &rig.pending);

        let events = rig.events();
        assert_eq!(rig.asked().len(), 1);
        assert!(events[0].manual);
        assert_eq!(
            events[0].status,
            UpdateStatus::Failed("no route to host".to_owned())
        );
    }

    #[test]
    fn a_disabled_schedule_asks_nothing() {
        let rig = Rig::new("update_disabled");
        let mut checker = rig.checker(OLD, false);
        rig.run_for(&mut checker, Duration::from_hours(72));
        assert_eq!(rig.asked(), Vec::<Option<String>>::new());
        assert_eq!(rig.events(), Vec::new());
    }

    #[test]
    fn the_running_version_is_up_to_date_against_the_same_kept_file() {
        let rig = Rig::new("update_after_update");
        rig.store().save_feed(FIXTURE);
        rig.checker("0.7.0", true).start();
        assert_eq!(rig.events()[0].status, UpdateStatus::UpToDate);
    }

    #[test]
    fn a_check_now_sent_while_a_scheduled_check_runs_is_answered_by_it() {
        let rig = Rig::new("update_coalesce");
        let mut checker = rig.checker(OLD, true);
        rig.answer_body(FIXTURE, "\"a\"");
        rig.sender.try_send(Request::CheckNow).unwrap();
        rig.sender.try_send(Request::CheckNow).unwrap();
        rig.clock.advance(STARTUP_DELAY);
        checker.tick(&rig.pending);

        assert_eq!(rig.asked().len(), 1);
        let events = rig.events();
        assert_eq!(events.len(), 1);
        assert!(events[0].manual);
    }

    #[test]
    fn skip_and_dismiss_reach_the_state_file_and_leave_a_newer_offer_alone() {
        let rig = Rig::new("update_skip");
        let mut checker = rig.checker(OLD, false);
        checker.handle(Request::Skip("0.6.1".to_owned()), &rig.pending);
        checker.handle(Request::Dismiss("0.6.0".to_owned()), &rig.pending);

        let (state, _) = rig.store().load();
        assert_eq!(state.skipped_version.as_deref(), Some("0.6.1"));
        assert_eq!(state.dismissed_version.as_deref(), Some("0.6.0"));

        let mut reopened = rig.checker(OLD, false);
        rig.answer_body(FIXTURE, "\"a\"");
        reopened.handle(Request::CheckNow, &rig.pending);
        let event = &rig.events()[0];
        assert_eq!(offered(event).as_deref(), Some("0.7.0"));
        assert_eq!(event.skipped_version.as_deref(), Some("0.6.1"));
        assert_eq!(event.dismissed_version.as_deref(), Some("0.6.0"));
    }
}
