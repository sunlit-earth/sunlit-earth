//! The QEMU Machine Protocol, which is JSON lines over a socket.
//!
//! The only thing the orchestrator asks a running QEMU is to stop, but it asks
//! over QMP rather than by killing the process, so QEMU flushes and closes the
//! overlay before the file is deleted. The socket is TCP on loopback rather
//! than a Unix socket, because the same code has to work on a Windows host.

use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::time::Duration;

/// How long to wait for the QMP socket and for each reply.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// What one line from the server is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// The banner sent on connect, before capabilities are negotiated.
    Greeting,
    /// A successful command result.
    Return,
    /// A command that failed, with the server's description.
    Error(String),
    /// An asynchronous event, which is not a reply to anything.
    Event(String),
    /// Valid JSON that is none of the above.
    Other,
}

/// A command line, ready to send.
pub fn request(execute: &str) -> String {
    format!("{{\"execute\":\"{execute}\"}}\n")
}

/// Classify one line of server output.
pub fn classify(line: &str) -> Result<Reply, String> {
    let value: serde_json::Value =
        serde_json::from_str(line.trim()).map_err(|e| format!("QMP sent unreadable JSON: {e}"))?;
    if value.get("QMP").is_some() {
        return Ok(Reply::Greeting);
    }
    if let Some(event) = value.get("event").and_then(serde_json::Value::as_str) {
        return Ok(Reply::Event(event.to_owned()));
    }
    if let Some(error) = value.get("error") {
        let description = error
            .get("desc")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unspecified QMP error");
        return Ok(Reply::Error(description.to_owned()));
    }
    if value.get("return").is_some() {
        return Ok(Reply::Return);
    }
    Ok(Reply::Other)
}

/// Connect, negotiate capabilities, and run one command.
///
/// Events arriving in the middle are skipped rather than mistaken for replies:
/// QEMU emits them unprompted, so the first line after a command is not
/// reliably its answer.
pub fn execute(port: u16, command: &str) -> Result<(), String> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let stream = TcpStream::connect_timeout(&address, TIMEOUT)
        .map_err(|e| format!("cannot reach the QMP socket on port {port}: {e}"))?;
    stream
        .set_read_timeout(Some(TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(TIMEOUT)))
        .map_err(|e| format!("cannot configure the QMP socket: {e}"))?;

    let mut writer = stream
        .try_clone()
        .map_err(|e| format!("cannot clone the QMP socket: {e}"))?;
    let mut reader = BufReader::new(&stream);

    read_until_reply(&mut reader, "the greeting")?;
    send(&mut writer, &request("qmp_capabilities"))?;
    read_until_reply(&mut reader, "qmp_capabilities")?;
    send(&mut writer, &request(command))?;

    // `quit` is answered and then the socket closes, so a clean end of stream
    // right after the command is success, not a truncated conversation.
    match read_until_reply(&mut reader, command) {
        Ok(()) | Err(_) if command == "quit" => Ok(()),
        other => other,
    }?;
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

fn send(writer: &mut impl Write, line: &str) -> Result<(), String> {
    writer
        .write_all(line.as_bytes())
        .and_then(|()| writer.flush())
        .map_err(|e| format!("cannot write to the QMP socket: {e}"))
}

fn read_until_reply(reader: &mut impl BufRead, what: &str) -> Result<(), String> {
    for _ in 0..32 {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .map_err(|e| format!("cannot read the QMP reply to {what}: {e}"))?;
        if read == 0 {
            return Err(format!("the QMP socket closed while waiting for {what}"));
        }
        match classify(&line)? {
            Reply::Event(_) => {}
            Reply::Error(description) => return Err(format!("{what} failed: {description}")),
            _ => return Ok(()),
        }
    }
    Err(format!("QMP sent only events while waiting for {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_one_json_line() {
        assert_eq!(request("quit"), "{\"execute\":\"quit\"}\n");
        assert!(request("system_powerdown").ends_with('\n'));
    }

    #[test]
    fn the_greeting_is_recognized() {
        let line = r#"{"QMP": {"version": {"qemu": {"major": 9}}, "capabilities": []}}"#;
        assert_eq!(classify(line), Ok(Reply::Greeting));
    }

    #[test]
    fn a_result_and_an_error_are_told_apart() {
        assert_eq!(classify(r#"{"return": {}}"#), Ok(Reply::Return));
        assert_eq!(
            classify(r#"{"error": {"class": "GenericError", "desc": "no such command"}}"#),
            Ok(Reply::Error("no such command".to_owned()))
        );
        assert_eq!(
            classify(r#"{"error": {"class": "GenericError"}}"#),
            Ok(Reply::Error("unspecified QMP error".to_owned()))
        );
    }

    #[test]
    fn events_are_not_replies() {
        assert_eq!(
            classify(r#"{"event": "SHUTDOWN", "timestamp": {"seconds": 1}}"#),
            Ok(Reply::Event("SHUTDOWN".to_owned()))
        );
    }

    #[test]
    fn unreadable_output_is_an_error_rather_than_a_shrug() {
        assert!(classify("not json").is_err());
        assert!(classify("").is_err());
    }

    #[test]
    fn reading_skips_events_until_a_reply_arrives() {
        let mut input = concat!(
            "{\"event\": \"RESUME\"}\n",
            "{\"event\": \"NIC_RX_FILTER_CHANGED\"}\n",
            "{\"return\": {}}\n"
        )
        .as_bytes();
        assert_eq!(read_until_reply(&mut input, "quit"), Ok(()));
    }

    #[test]
    fn reading_reports_an_error_reply_with_its_description() {
        let mut input = "{\"error\": {\"desc\": \"nope\"}}\n".as_bytes();
        let err = read_until_reply(&mut input, "quit").unwrap_err();
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn a_closed_socket_is_reported_as_such() {
        let mut input = "".as_bytes();
        let err = read_until_reply(&mut input, "the greeting").unwrap_err();
        assert!(err.contains("closed"), "{err}");
    }

    #[test]
    fn an_endless_event_stream_does_not_hang_forever() {
        let flood = "{\"event\": \"RTC_CHANGE\"}\n".repeat(64);
        let mut input = flood.as_bytes();
        assert!(read_until_reply(&mut input, "quit").is_err());
    }
}
