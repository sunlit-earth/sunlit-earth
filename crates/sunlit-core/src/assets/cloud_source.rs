//! Where cloud imagery comes from.
//!
//! The engine talks to a `CloudSource` rather than to HTTP directly, so tests
//! can publish frames on a simulated schedule without a network or a server.

use std::sync::Mutex;
use std::time::Duration;

use tracing::info;

/// One fetched cloud image plus the freshness metadata that came with it.
pub struct CloudImage {
    pub bytes: Vec<u8>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
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
            .timeout_global(Some(Duration::from_secs(120)))
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
            bytes,
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
