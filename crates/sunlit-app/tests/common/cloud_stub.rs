//! A cloud server the size of one test.
//!
//! It serves a JPEG over loopback and counts what was downloaded, so the cloud
//! cases exercise the real fetcher without the real network.

use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Shared state of the stub cloud server: the currently published image
/// version and the number of full downloads served.
pub(crate) struct StubState {
    pub(crate) version: AtomicU64,
    pub(crate) gets: AtomicU64,
}

/// Start a minimal HTTP/1.1 server on an ephemeral port that serves `jpeg`
/// as the cloud image.
///
/// `HEAD` answers `304 Not Modified` when the request's `If-None-Match` matches
/// the current version and `200` otherwise; `GET` always returns the body and
/// increments the download counter. Every response carries `Content-Length` and
/// `Connection: close` and the socket is closed afterwards, so `ureq` never
/// waits for a keep-alive continuation.
///
/// Returns the bound port and the shared state, which the test uses to publish
/// new versions and observe downloads.
pub(crate) fn spawn_cloud_stub(jpeg: Vec<u8>) -> (u16, Arc<StubState>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind cloud stub server");
    let port = listener
        .local_addr()
        .expect("cloud stub server has no local address")
        .port();
    let state = Arc::new(StubState {
        version: AtomicU64::new(1),
        gets: AtomicU64::new(0),
    });

    let server_state = Arc::clone(&state);
    std::thread::Builder::new()
        .name("cloud-stub".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                serve_cloud_request(stream, &jpeg, &server_state);
            }
        })
        .expect("failed to spawn cloud stub thread");

    (port, state)
}

/// Handle a single stub request, then close the connection.
///
/// Any connection that does not deliver a complete request within the read
/// timeout is abandoned rather than blocking the accept loop; a stray local
/// connection (a port scanner, say) would otherwise wedge the server and hang
/// the test.
fn serve_cloud_request(mut stream: TcpStream, jpeg: &[u8], state: &StubState) {
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
    let method = request_line
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned();

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

    let etag = format!("\"v{}\"", state.version.load(Ordering::SeqCst));
    let response = if method == "HEAD" && if_none_match.as_deref() == Some(etag.as_str()) {
        format!(
            "HTTP/1.1 304 Not Modified\r\nETag: {etag}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
    } else {
        format!(
            "HTTP/1.1 200 OK\r\nETag: {etag}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            jpeg.len()
        )
    };

    if stream.write_all(response.as_bytes()).is_err() {
        return;
    }
    if method == "GET" {
        if stream.write_all(jpeg).is_err() {
            return;
        }
        if stream.flush().is_err() {
            return;
        }
        state.gets.fetch_add(1, Ordering::SeqCst);
    }
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Both);
}

/// Encode a JPEG the stub server can serve as the cloud image.
///
/// The gradient keeps the encoded file small while the decoded RGBA buffer is
/// `width * height * 4` bytes, which is the allocation the case is watching.
pub(crate) fn cloud_fixture_jpeg(width: u32, height: u32) -> Vec<u8> {
    let mut img = image::RgbImage::new(width, height);
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        let r = u8::try_from(x % 256).expect("modulo 256 fits in u8");
        let g = u8::try_from(y % 256).expect("modulo 256 fits in u8");
        *pixel = image::Rgb([r, g, 128]);
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Jpeg)
        .expect("failed to encode cloud fixture JPEG");
    buf.into_inner()
}

/// Block until the stub server has served at least `target` downloads.
pub(crate) fn wait_for_downloads(stub: &StubState, target: u64, timeout: Duration) {
    let start = Instant::now();
    loop {
        let served = stub.gets.load(Ordering::SeqCst);
        if served >= target {
            return;
        }
        assert!(
            start.elapsed() <= timeout,
            "timed out after {:.0}s waiting for cloud download #{target} (served {served})",
            timeout.as_secs_f64()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
