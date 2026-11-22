//! A release file server the size of one test.
//!
//! It serves one body over loopback with an `ETag` and answers a matching
//! `If-None-Match` with a 304, which is what the website's Worker does for
//! `releases/v1.json`, so the update case exercises the real check without
//! the real network.

use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The stub's body and what it has answered.
pub(crate) struct FeedStub {
    port: u16,
    /// The body and its version, which names the `ETag`.
    body: Mutex<(u64, Vec<u8>)>,
    /// Answers with a body.
    pub(crate) full: AtomicU64,
    /// Answers of `304 Not Modified`.
    pub(crate) not_modified: AtomicU64,
    stopped: AtomicBool,
}

impl FeedStub {
    /// The URL the app is pointed at.
    pub(crate) fn url(&self) -> String {
        format!("http://127.0.0.1:{}/releases/v1.json", self.port)
    }

    /// Serve `body` from now on, under a new `ETag`.
    pub(crate) fn publish(&self, body: Vec<u8>) {
        let mut current = self.body.lock().expect("feed stub lock poisoned");
        *current = (current.0 + 1, body);
    }

    /// Close the port, so that every later request is refused.
    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        // Wakes the accept loop, which sees the flag and drops the listener.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

/// Start a minimal HTTP/1.1 server on an ephemeral port that serves `body` to
/// every `GET`.
///
/// Every response carries `Content-Length` and `Connection: close` and the
/// socket is closed afterwards, so `ureq` never waits for a keep-alive
/// continuation. The accept loop returns once [`FeedStub::stop`] is called,
/// and with it the listener is gone.
pub(crate) fn spawn_feed_stub(body: Vec<u8>) -> Arc<FeedStub> {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind the feed stub");
    let port = listener
        .local_addr()
        .expect("the feed stub has no local address")
        .port();
    let stub = Arc::new(FeedStub {
        port,
        body: Mutex::new((1, body)),
        full: AtomicU64::new(0),
        not_modified: AtomicU64::new(0),
        stopped: AtomicBool::new(false),
    });

    let server = Arc::clone(&stub);
    std::thread::Builder::new()
        .name("feed-stub".into())
        .spawn(move || {
            for stream in listener.incoming() {
                if server.stopped.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                serve_feed_request(stream, &server);
            }
        })
        .expect("failed to spawn the feed stub thread");
    stub
}

/// Answer one request, then close the connection.
///
/// A connection that does not deliver a complete request within the read
/// timeout is abandoned rather than left to block the accept loop.
fn serve_feed_request(mut stream: TcpStream, stub: &FeedStub) {
    const INM: &str = "if-none-match:";
    const READ_TIMEOUT: Duration = Duration::from_secs(5);

    if stream.set_read_timeout(Some(READ_TIMEOUT)).is_err() {
        return;
    }
    let Ok(peek) = stream.try_clone() else { return };
    let mut reader = BufReader::new(peek);

    let mut request_line = String::new();
    if !matches!(reader.read_line(&mut request_line), Ok(n) if n > 0) {
        return;
    }
    let mut if_none_match: Option<String> = None;
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return,
        }
        if header.trim().is_empty() {
            break;
        }
        if header.to_ascii_lowercase().starts_with(INM) {
            if_none_match = Some(header[INM.len()..].trim().to_owned());
        }
    }

    let (version, body) = stub.body.lock().expect("feed stub lock poisoned").clone();
    let etag = format!("\"v{version}\"");
    let written = if if_none_match.as_deref() == Some(etag.as_str()) {
        stub.not_modified.fetch_add(1, Ordering::SeqCst);
        stream.write_all(
            format!(
                "HTTP/1.1 304 Not Modified\r\nETag: {etag}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
    } else {
        stub.full.fetch_add(1, Ordering::SeqCst);
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nETag: {etag}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .and_then(|()| stream.write_all(&body))
    };
    if written.is_ok() {
        let _ = stream.flush();
    }
    let _ = stream.shutdown(Shutdown::Both);
}

/// A release file listing `versions`, newest first, in the format the
/// website writes.
pub(crate) fn feed_listing(versions: &[&str]) -> Vec<u8> {
    let releases: Vec<String> = versions
        .iter()
        .map(|version| {
            format!(
                r#"{{"version":"{version}","date":"2026-12-01","notes":"https://github.com/sunlit-earth/sunlit-earth/releases/tag/v{version}","highlights":["A line about {version}"],"assets":{{}}}}"#
            )
        })
        .collect();
    format!(r#"{{"releases":[{}]}}"#, releases.join(",")).into_bytes()
}
