//! Where the release file comes from.
//!
//! The checker talks to an `UpdateSource` rather than to HTTP, so its tests
//! answer from a list instead of a server.

use std::time::Duration;

use super::feed::BODY_LIMIT;

/// How long one request may take, connecting and reading the body included.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// What one request brought back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    /// The server answered 304 to the known `ETag`.
    Unchanged,
    /// A body, with the `ETag` it came with.
    Body {
        bytes: Vec<u8>,
        etag: Option<String>,
    },
}

/// A source of the release file.
pub trait UpdateSource: Send {
    /// Fetch the file, conditionally when `known_etag` is given.
    fn fetch(&self, known_etag: Option<&str>) -> Result<Fetched, String>;
}

/// The production source: one `GET`, with `If-None-Match` when an `ETag` is
/// known.
///
/// An agent of its own rather than the cloud fetcher's, which lives on another
/// thread with another timeout.
pub struct HttpUpdateSource {
    agent: ureq::Agent,
    url: String,
}

/// What every request says about the client: the version, the operating system
/// and the architecture, and nothing about the installation.
pub fn user_agent() -> String {
    format!(
        "sunlit.earth/{} ({}; {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

impl HttpUpdateSource {
    pub fn new(url: String) -> Self {
        let agent = ureq::Agent::config_builder()
            .user_agent(user_agent())
            .timeout_global(Some(REQUEST_TIMEOUT))
            .build()
            .new_agent();
        Self { agent, url }
    }

    fn answered(&self, status: u16) -> String {
        format!("{} answered {status}", self.url)
    }
}

impl UpdateSource for HttpUpdateSource {
    fn fetch(&self, known_etag: Option<&str>) -> Result<Fetched, String> {
        let mut request = self.agent.get(&self.url);
        if let Some(etag) = known_etag {
            request = request.header("If-None-Match", etag);
        }
        let mut response = request.call().map_err(|e| match e {
            ureq::Error::StatusCode(status) => self.answered(status),
            e => format!("could not reach {}: {e}", self.url),
        })?;
        let status = response.status().as_u16();
        if status == 304 {
            return Ok(Fetched::Unchanged);
        }
        if status != 200 {
            return Err(self.answered(status));
        }
        let etag = response
            .headers()
            .get("ETag")
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let bytes = response
            .body_mut()
            .with_config()
            .limit(BODY_LIMIT)
            .read_to_vec()
            .map_err(|e| format!("could not read {}: {e}", self.url))?;
        Ok(Fetched::Body { bytes, etag })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// Answer one connection with `response` on loopback, and hand back the
    /// request's `If-None-Match` and `User-Agent`.
    fn serve_once(response: Vec<u8>) -> (String, mpsc::Receiver<(Option<String>, String)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/releases/v1.json", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let (mut if_none_match, mut agent) = (None, String::new());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
                let lower = line.to_ascii_lowercase();
                let value = || line.split_once(':').unwrap().1.trim().to_owned();
                if lower.starts_with("if-none-match:") {
                    if_none_match = Some(value());
                } else if lower.starts_with("user-agent:") {
                    agent = value();
                }
            }
            let _ = stream.write_all(&response);
            let _ = tx.send((if_none_match, agent));
        });
        (url, rx)
    }

    fn ok(body: &[u8], etag: &str) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nETag: {etag}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    #[test]
    fn a_body_comes_back_with_its_etag_and_the_request_carries_the_user_agent() {
        let (url, seen) = serve_once(ok(b"{\"releases\":[]}", "\"v1\""));
        let fetched = HttpUpdateSource::new(url).fetch(None).unwrap();
        assert_eq!(
            fetched,
            Fetched::Body {
                bytes: b"{\"releases\":[]}".to_vec(),
                etag: Some("\"v1\"".to_owned())
            }
        );
        let (if_none_match, agent) = seen.recv().unwrap();
        assert_eq!(if_none_match, None);
        assert_eq!(agent, user_agent());
    }

    #[test]
    fn a_known_etag_is_sent_and_a_304_is_unchanged() {
        let (url, seen) = serve_once(
            b"HTTP/1.1 304 Not Modified\r\nETag: \"v1\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        );
        let fetched = HttpUpdateSource::new(url).fetch(Some("\"v1\"")).unwrap();
        assert_eq!(fetched, Fetched::Unchanged);
        assert_eq!(seen.recv().unwrap().0.as_deref(), Some("\"v1\""));
    }

    #[test]
    fn a_body_over_the_limit_fails() {
        let body = vec![b' '; usize::try_from(BODY_LIMIT).unwrap() + 1];
        let (url, _seen) = serve_once(ok(&body, "\"big\""));
        let reason = HttpUpdateSource::new(url.clone()).fetch(None).unwrap_err();
        assert!(reason.contains(&url), "{reason}");
    }

    #[test]
    fn a_server_error_fails_and_names_the_url() {
        let (url, _seen) = serve_once(
            b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_vec(),
        );
        let reason = HttpUpdateSource::new(url.clone()).fetch(None).unwrap_err();
        assert_eq!(reason, format!("{url} answered 503"));
    }

    #[test]
    fn a_server_that_is_not_there_fails_and_names_the_url() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/releases/v1.json", listener.local_addr().unwrap());
        drop(listener);
        let reason = HttpUpdateSource::new(url.clone()).fetch(None).unwrap_err();
        assert!(
            reason.starts_with(&format!("could not reach {url}: ")),
            "{reason}"
        );
    }

    #[test]
    fn the_user_agent_names_the_version_the_system_and_the_architecture_only() {
        let agent = user_agent();
        assert!(agent.starts_with("sunlit.earth/"), "{agent}");
        assert!(agent.contains(env!("CARGO_PKG_VERSION")), "{agent}");
        assert!(
            agent.ends_with(&format!(
                "({}; {})",
                std::env::consts::OS,
                std::env::consts::ARCH
            )),
            "{agent}"
        );
    }
}
