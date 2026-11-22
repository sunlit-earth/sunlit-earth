//! What the update thread keeps on disk between runs.
//!
//! Two files in [`cache_dir`](crate::assets::cloud_fetcher::cache_dir), beside
//! the cloud cache: `update_feed.json`, the last body received exactly as it
//! came, and `update_state.toml` with the `ETag` that body came with, when the
//! last check succeeded, and the version whose banner the user dismissed.
//! Only the update thread writes either.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tracing::warn;

use super::feed::{self, Feed};

const FEED_FILE: &str = "update_feed.json";
const STATE_FILE: &str = "update_state.toml";

/// The contents of `update_state.toml`. A missing field reads as `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateState {
    /// The `ETag` the kept body came with.
    pub etag: Option<String>,
    /// When the last check succeeded, UTC in RFC 3339.
    pub last_checked: Option<String>,
    pub dismissed_version: Option<String>,
}

impl UpdateState {
    /// [`Self::last_checked`] as a time, `None` when it does not parse.
    pub fn last_checked_at(&self) -> Option<OffsetDateTime> {
        OffsetDateTime::parse(self.last_checked.as_deref()?, &Rfc3339).ok()
    }

    pub fn set_last_checked(&mut self, at: OffsetDateTime) {
        self.last_checked = at.format(&Rfc3339).ok();
    }
}

/// Where the two files live, or nowhere on a system without a cache directory.
#[derive(Debug, Clone)]
pub struct Store {
    dir: Option<PathBuf>,
}

impl Store {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir }
    }

    fn path(&self, name: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|dir| dir.join(name))
    }

    /// The state, and the kept body as far as it reads.
    ///
    /// A missing or broken state file reads as empty. A kept `ETag` without a
    /// readable kept body is dropped, so the next check asks unconditionally
    /// rather than being told "unchanged" about a body that is not there.
    pub fn load(&self) -> (UpdateState, Option<Feed>) {
        let mut state = self.load_state();
        let kept = self.load_feed();
        if kept.is_none() {
            state.etag = None;
        }
        (state, kept)
    }

    fn load_state(&self) -> UpdateState {
        let Some(path) = self.path(STATE_FILE) else {
            return UpdateState::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return UpdateState::default();
        };
        toml::from_str(&text).unwrap_or_else(|e| {
            warn!(path = %path.display(), error = %e, "the update state does not read, starting from none");
            UpdateState::default()
        })
    }

    /// The kept body, read by the rules every check reads by.
    pub fn load_feed(&self) -> Option<Feed> {
        let path = self.path(FEED_FILE)?;
        let body = std::fs::read(&path).ok()?;
        feed::parse(&body)
            .inspect_err(|e| warn!(path = %path.display(), error = %e, "the kept release file does not read"))
            .ok()
    }

    pub fn save_state(&self, state: &UpdateState) {
        if let Some(path) = self.path(STATE_FILE) {
            crate::files::write_toml(state, &path, "update state");
        }
    }

    /// Put `body` in place of the kept one. Returns whether it is there now.
    pub fn save_feed(&self, body: &[u8]) -> bool {
        let Some(path) = self.path(FEED_FILE) else {
            return false;
        };
        write_atomically(&path, body)
            .inspect_err(
                |e| warn!(path = %path.display(), error = %e, "could not keep the release file"),
            )
            .is_ok()
    }
}

fn write_atomically(path: &Path, body: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let unfinished = crate::files::unfinished(path, "~");
    std::fs::write(&unfinished, body)?;
    if let Err(e) = std::fs::rename(&unfinished, path) {
        let _ = std::fs::remove_file(&unfinished);
        return Err(e);
    }
    crate::files::sweep_unfinished(path, "~");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ScratchDir;

    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/update_feed_v1.json");

    fn full_state() -> UpdateState {
        let mut state = UpdateState {
            etag: Some("\"abc\"".to_owned()),
            last_checked: None,
            dismissed_version: Some("0.6.1".to_owned()),
        };
        state.set_last_checked(OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000));
        state
    }

    #[test]
    fn the_state_survives_a_round_trip() {
        let dir = ScratchDir::new("update_state_round_trip");
        let store = Store::new(Some(dir.path().to_owned()));
        assert!(store.save_feed(FIXTURE));
        store.save_state(&full_state());

        let (state, kept) = store.load();
        assert_eq!(state, full_state());
        assert_eq!(
            state.last_checked_at(),
            Some(OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000))
        );
        assert_eq!(kept, Some(feed::parse(FIXTURE).unwrap()));
    }

    #[test]
    fn a_missing_state_reads_as_empty() {
        let dir = ScratchDir::new("update_state_missing");
        let (state, kept) = Store::new(Some(dir.path().to_owned())).load();
        assert_eq!(state, UpdateState::default());
        assert_eq!(kept, None);
    }

    #[test]
    fn a_broken_state_reads_as_empty() {
        let dir = ScratchDir::new("update_state_broken");
        std::fs::write(dir.join(STATE_FILE), "etag = [unclosed").unwrap();
        let (state, _) = Store::new(Some(dir.path().to_owned())).load();
        assert_eq!(state, UpdateState::default());
    }

    #[test]
    fn a_state_with_a_skipped_version_from_an_older_build_still_reads() {
        let dir = ScratchDir::new("update_state_old_field");
        let store = Store::new(Some(dir.path().to_owned()));
        assert!(store.save_feed(FIXTURE));
        std::fs::write(
            dir.join(STATE_FILE),
            r#"etag = "\"abc\""
last_checked = "2024-10-04T00:00:00Z"
skipped_version = "0.7.0"
dismissed_version = "0.6.1"
"#,
        )
        .unwrap();

        let (state, _) = store.load();
        assert_eq!(state.etag.as_deref(), Some("\"abc\""));
        assert_eq!(state.dismissed_version.as_deref(), Some("0.6.1"));
        assert!(state.last_checked_at().is_some());
    }

    #[test]
    fn an_etag_without_a_kept_body_is_dropped() {
        let dir = ScratchDir::new("update_state_orphan_etag");
        let store = Store::new(Some(dir.path().to_owned()));
        store.save_state(&full_state());

        let (state, kept) = store.load();
        assert_eq!(kept, None);
        assert_eq!(state.etag, None);
        assert_eq!(state.dismissed_version, full_state().dismissed_version);

        std::fs::write(dir.join(FEED_FILE), b"not json").unwrap();
        assert_eq!(store.load().0.etag, None);
    }

    #[test]
    fn no_directory_keeps_nothing_and_reads_as_empty() {
        let store = Store::new(None);
        assert!(!store.save_feed(FIXTURE));
        store.save_state(&full_state());
        assert_eq!(store.load(), (UpdateState::default(), None));
    }

    #[test]
    fn a_time_that_does_not_parse_reads_as_never() {
        let state = UpdateState {
            last_checked: Some("yesterday".to_owned()),
            ..UpdateState::default()
        };
        assert_eq!(state.last_checked_at(), None);
    }
}
