//! Where cloud imagery comes from.
//!
//! The engine talks to a `CloudSource` rather than to HTTP directly, so tests
//! can publish frames on a simulated schedule without a network or a server.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tracing::info;

/// One fetched cloud image plus the freshness metadata that came with it.
pub struct CloudImage {
    pub bytes: Download,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// The downloaded cloud images of this process, as [`downloads`] counts them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DownloadCount {
    /// Buffers alive now.
    pub buffers: u64,
    /// Bytes the live buffers hold.
    pub bytes: u64,
    /// Buffers made since the process started, alive or not.
    pub made: u64,
}

static LIVE_BUFFERS: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static MADE_BUFFERS: AtomicU64 = AtomicU64::new(0);

/// The [`Download`]s alive in this process, and how many there have been.
///
/// A download lives only through the poll that fetched it, or the cache read
/// that loaded it, while it is decoded, and for a poll written to the cache, so between
/// updates the live count is zero, and a buffer that stays alive is a download
/// parked in a queue, a cache or a long-lived struct. `made` tells "dropped"
/// from "not fetched yet", as it does for the decoded frames.
pub fn downloads() -> DownloadCount {
    DownloadCount {
        made: MADE_BUFFERS.load(Ordering::SeqCst),
        buffers: LIVE_BUFFERS.load(Ordering::SeqCst),
        bytes: LIVE_BYTES.load(Ordering::SeqCst),
    }
}

/// The body of a cloud image as it came over the wire, or back off the disk
/// cache, counted in [`downloads`] for as long as it exists.
///
/// Counted on creation and uncounted on drop, on the buffer rather than as
/// `Drop` on [`CloudImage`], whose freshness fields the updater moves out.
pub struct Download(Vec<u8>);

impl Download {
    /// Count `bytes` as one download.
    pub fn new(bytes: Vec<u8>) -> Self {
        LIVE_BYTES.fetch_add(bytes.capacity() as u64, Ordering::SeqCst);
        LIVE_BUFFERS.fetch_add(1, Ordering::SeqCst);
        MADE_BUFFERS.fetch_add(1, Ordering::SeqCst);
        Self(bytes)
    }
}

impl Drop for Download {
    fn drop(&mut self) {
        LIVE_BYTES.fetch_sub(self.0.capacity() as u64, Ordering::SeqCst);
        LIVE_BUFFERS.fetch_sub(1, Ordering::SeqCst);
    }
}

impl std::ops::Deref for Download {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

/// A source of equirectangular cloud imagery.
///
/// Implementations must be cheap to call when nothing has changed: the engine
/// polls on a schedule and expects `None` most of the time.
pub trait CloudSource: Send + Sync {
    /// Fetch the current image if it differs from `known_etag`.
    ///
    /// Returns `Ok(None)` when the source is unchanged.
    fn fetch_if_changed(&self, known_etag: Option<&str>) -> Result<Option<CloudImage>, String>;

    /// Human-readable description for logs.
    fn describe(&self) -> String;

    /// Point this source at a different URL.
    ///
    /// The texture resolution selects which cloud image variant to fetch, and
    /// the variant is a path segment of the URL, so a resolution switch has to
    /// move the source rather than replace it: the engine holds it behind an
    /// `Arc` shared with a worker thread that is mid-poll as often as not.
    /// Takes `&self` for that reason, and does nothing by default, which is the
    /// right answer for every source that serves one fixed thing.
    fn retarget(&self, _url: &str) {}
}

/// The production source: an HTTP endpoint polled with HEAD + `If-None-Match`.
pub struct HttpCloudSource {
    agent: ureq::Agent,
    /// Behind a lock because [`CloudSource::retarget`] moves it while the
    /// worker thread owns the only handle.
    url: Mutex<String>,
}

impl HttpCloudSource {
    pub fn new(url: String) -> Self {
        let user_agent = format!("sunlit.earth/{}", env!("CARGO_PKG_VERSION"));
        let agent = ureq::Agent::config_builder()
            .user_agent(&user_agent)
            .timeout_global(Some(Duration::from_mins(2)))
            .build()
            .new_agent();
        Self {
            agent,
            url: Mutex::new(url),
        }
    }

    /// The URL as of right now.
    ///
    /// A copy rather than a guard: the request below outlives any sensible
    /// lock hold, and a poisoned lock means a panic elsewhere rather than a
    /// reason to stop fetching clouds.
    fn url(&self) -> String {
        self.url
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Check if the remote image has changed using HEAD + `If-None-Match`.
    ///
    /// Returns `Ok(true)` if unchanged (304), `Ok(false)` if new content is
    /// available (200), or `Err` on transport/server errors.
    #[tracing::instrument(skip(self), fields(etag = %etag))]
    fn check_freshness(&self, etag: &str) -> Result<bool, String> {
        let response = self
            .agent
            .head(&self.url())
            .header("If-None-Match", etag)
            .call()
            .map_err(|e| format!("Cloud freshness check failed: {e}"))?;

        Ok(response.status().as_u16() == 304)
    }

    /// Download the cloud image unconditionally.
    #[tracing::instrument(skip(self))]
    fn download(&self) -> Result<CloudImage, String> {
        let mut response = self
            .agent
            .get(&self.url())
            .call()
            .map_err(|e| format!("Cloud download failed: {e}"))?;

        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(String::from)
        };
        let etag = header("ETag");
        let last_modified = header("Last-Modified");

        // 8K cloud JPEG can be ~15 MB, raise the default 10 MB limit
        let bytes = response
            .body_mut()
            .with_config()
            .limit(50 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| format!("Failed to read cloud image body: {e}"))?;

        Ok(CloudImage {
            bytes: Download::new(bytes),
            etag,
            last_modified,
        })
    }
}

impl CloudSource for HttpCloudSource {
    fn fetch_if_changed(&self, known_etag: Option<&str>) -> Result<Option<CloudImage>, String> {
        if let Some(etag) = known_etag {
            if self.check_freshness(etag)? {
                info!("cloud image unchanged (304 Not Modified)");
                return Ok(None);
            }
            info!("cloud image has changed, downloading");
        } else {
            info!("no cached cloud ETag, downloading");
        }
        self.download().map(Some)
    }

    fn describe(&self) -> String {
        self.url()
    }

    fn retarget(&self, url: &str) {
        let mut current = self
            .url
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        current.clear();
        current.push_str(url);
    }
}
