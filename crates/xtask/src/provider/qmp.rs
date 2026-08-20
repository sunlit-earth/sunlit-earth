//! The QEMU Machine Protocol, which is JSON lines over a socket.
//!
//! Two callers use it. Teardown asks a running QEMU to stop, over QMP rather
//! than by killing the process, so QEMU flushes and closes the overlay before
//! the file is deleted. A build watches its guest through it: whether the
//! machine is running, where it stopped if it is not, and what is on its screen
//! (`commands::build_watch`). The socket is TCP on loopback rather than a Unix
//! socket, because the same code has to work on a Windows host.

use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::Path;
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

/// A command line with arguments.
pub fn request_with(execute: &str, arguments: &serde_json::Value) -> String {
    format!("{{\"execute\":\"{execute}\",\"arguments\":{arguments}}}\n")
}

/// A `send-key` line for one key, named the way QEMU names keys.
///
/// This injects at the input device rather than through a VNC client, which is
/// the whole reason it exists: Packer types its boot command over VNC, and on
/// this host those keystrokes never reach the guest, while the same key sent
/// here does. `hold_ms` is QEMU's `hold-time`.
pub fn send_key_request(qcode: &str, hold_ms: u32) -> String {
    format!(
        "{{\"execute\":\"send-key\",\"arguments\":{{\"keys\":[{{\"type\":\"qcode\",\
         \"data\":\"{qcode}\"}}],\"hold-time\":{hold_ms}}}}}\n"
    )
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

/// The instruction pointer out of `info registers`.
///
/// A stopped machine is worth little on its own; a stopped machine that is
/// still at the same instruction after being started again is a machine that
/// cannot run, which is the distinction the build watcher needs. HMP prints the
/// whole register file, and this takes the one field out of it rather than
/// parsing the rest.
pub fn parse_rip(info_registers: &str) -> Option<String> {
    info_registers
        .split_whitespace()
        .find_map(|token| token.strip_prefix("RIP="))
        .map(|rip| format!("0x{rip}"))
}

/// A connection to one guest's monitor, with capabilities already negotiated.
///
/// Anything that asks more than one question holds one of these, and that is
/// not only about handshake cost: QEMU's QMP server serves one client at a
/// time, and a second connection waits in the accept queue, unanswered, until
/// the first one closes. So everything that asks a build's guest anything asks
/// through the same session.
pub struct Session {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
}

impl Session {
    /// Connect and negotiate capabilities.
    pub fn connect(port: u16) -> Result<Self, String> {
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        let stream = TcpStream::connect_timeout(&address, TIMEOUT)
            .map_err(|e| format!("cannot reach the QMP socket on port {port}: {e}"))?;
        stream
            .set_read_timeout(Some(TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(TIMEOUT)))
            .map_err(|e| format!("cannot configure the QMP socket: {e}"))?;
        let reader = BufReader::new(
            stream
                .try_clone()
                .map_err(|e| format!("cannot clone the QMP socket: {e}"))?,
        );
        let mut session = Self { stream, reader };
        session.reply("the greeting")?;
        session.write(&request("qmp_capabilities"))?;
        session.reply("qmp_capabilities")?;
        Ok(session)
    }

    /// Run one command that takes no arguments and hand back its result.
    pub fn command(&mut self, execute: &str) -> Result<serde_json::Value, String> {
        self.write(&request(execute))?;
        self.reply(execute)
    }

    /// Run one command with arguments.
    pub fn command_with(
        &mut self,
        execute: &str,
        arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.write(&request_with(execute, arguments))?;
        self.reply(execute)
    }

    /// QEMU's own word for what the machine is doing: `running`, `paused`,
    /// `internal-error`, and so on.
    pub fn status(&mut self) -> Result<String, String> {
        let value = self.command("query-status")?;
        value
            .get("status")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "query-status answered without a status".to_owned())
    }

    /// Where the first vCPU is, or `None` if the monitor would not say.
    ///
    /// Diagnostic only, so a failure here is an absent answer rather than an
    /// error for the caller to handle.
    pub fn instruction_pointer(&mut self) -> Option<String> {
        let value = self
            .command_with(
                "human-monitor-command",
                &serde_json::json!({"command-line": "info registers"}),
            )
            .ok()?;
        parse_rip(value.as_str()?)
    }

    /// Write the guest's screen to `path` as a PNG.
    ///
    /// QEMU encodes it, which is why the build can keep a screenshot without
    /// this crate knowing anything about image formats. The `format` argument
    /// arrived in QEMU 7.1; an older one answers with an error rather than a
    /// PPM under a `.png` name, and the caller reports that once and stops
    /// asking.
    pub fn screendump(&mut self, path: &Path) -> Result<(), String> {
        self.command_with(
            "screendump",
            &serde_json::json!({"filename": path.to_string_lossy(), "format": "png"}),
        )
        .map(|_| ())
    }

    /// Start a stopped machine again.
    pub fn cont(&mut self) -> Result<(), String> {
        self.command("cont").map(|_| ())
    }

    /// End the guest.
    ///
    /// The socket closes as the reply is sent, so an unfinished conversation
    /// here is success rather than a failure to report.
    pub fn quit(&mut self) -> Result<(), String> {
        self.write(&request("quit"))?;
        let _ = self.reply("quit");
        Ok(())
    }

    fn write(&mut self, line: &str) -> Result<(), String> {
        send(&mut self.stream, line)
    }

    fn reply(&mut self, what: &str) -> Result<serde_json::Value, String> {
        read_until_reply(&mut self.reader, what)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

/// Connect, negotiate capabilities, and run one command.
pub fn execute(port: u16, command: &str) -> Result<(), String> {
    let mut session = Session::connect(port)?;
    if command == "quit" {
        return session.quit();
    }
    session.command(command).map(|_| ())
}

/// Press one key on a running guest until `stop` says to, and report how many
/// presses landed.
///
/// The caller is answering a prompt whose exact moment is unknown, hence the
/// repetition; `stop` is how it says the prompt has been answered, which
/// matters because these keystrokes go somewhere once the guest is past it.
pub fn press_key_until(
    session: &mut Session,
    qcode: &str,
    max_presses: u32,
    gap: Duration,
    hold_ms: u32,
    stop: &dyn Fn() -> bool,
) -> Result<u32, String> {
    let line = send_key_request(qcode, hold_ms);
    let mut sent = 0;
    for press in 0..max_presses {
        session.write(&line)?;
        session.reply("send-key")?;
        sent += 1;
        if stop() {
            break;
        }
        if press + 1 < max_presses {
            std::thread::sleep(gap);
        }
    }
    Ok(sent)
}

fn send(writer: &mut impl Write, line: &str) -> Result<(), String> {
    writer
        .write_all(line.as_bytes())
        .and_then(|()| writer.flush())
        .map_err(|e| format!("cannot write to the QMP socket: {e}"))
}

/// Read until something that is a reply arrives, and hand back its payload.
///
/// Events arriving in the middle are skipped rather than mistaken for replies:
/// QEMU emits them unprompted, so the first line after a command is not
/// reliably its answer.
fn read_until_reply(reader: &mut impl BufRead, what: &str) -> Result<serde_json::Value, String> {
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
            _ => {
                let value: serde_json::Value = serde_json::from_str(line.trim())
                    .map_err(|e| format!("QMP sent unreadable JSON: {e}"))?;
                return Ok(value
                    .get("return")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null));
            }
        }
    }
    Err(format!("QMP sent only events while waiting for {what}"))
}

/// A QMP server that answers from a script, for testing the client and the
/// things built on it without a QEMU.
///
/// Enough of the protocol to be worth trusting: the greeting arrives before
/// anything is asked, capabilities are negotiated first, and every line the
/// client sent is recorded so a test can assert what was actually asked rather
/// than what the code looks like it asks.
#[cfg(test)]
pub mod fake {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    pub struct Server {
        pub port: u16,
        lines: Arc<Mutex<Vec<String>>>,
    }

    impl Server {
        /// Start a server on a loopback port of the operating system's
        /// choosing, answering `query-status` with each of `statuses` in turn
        /// and repeating the last one after that.
        pub fn start(statuses: &[&str]) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
            let port = listener.local_addr().expect("the bound address").port();
            let lines = Arc::new(Mutex::new(Vec::new()));
            let recorded = Arc::clone(&lines);
            let statuses: Vec<String> = statuses.iter().map(|s| (*s).to_owned()).collect();
            std::thread::spawn(move || {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let mut writer = stream.try_clone().expect("clone the accepted socket");
                let mut reader = BufReader::new(stream);
                if writer
                    .write_all(b"{\"QMP\": {\"version\": {}, \"capabilities\": []}}\n")
                    .is_err()
                {
                    return;
                }
                let mut asked = 0;
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                    recorded
                        .lock()
                        .expect("the recording lock")
                        .push(line.trim().to_owned());
                    let answer = if line.contains("query-status") {
                        let status = statuses
                            .get(asked)
                            .or_else(|| statuses.last())
                            .cloned()
                            .unwrap_or_else(|| "running".to_owned());
                        asked += 1;
                        format!(
                            "{{\"return\": {{\"status\": \"{status}\", \"running\": {}}}}}\n",
                            status == "running"
                        )
                    } else if line.contains("info registers") {
                        "{\"return\": \"CPU#0 RAX=0000000080000000 \
                         RIP=00000000fffd3da2 RFL=00000086\"}\n"
                            .to_owned()
                    } else {
                        "{\"return\": {}}\n".to_owned()
                    };
                    if writer.write_all(answer.as_bytes()).is_err() {
                        return;
                    }
                    if line.contains("\"quit\"") {
                        return;
                    }
                }
            });
            Self { port, lines }
        }

        /// Every line the client sent, in order.
        pub fn received(&self) -> Vec<String> {
            self.lines.lock().expect("the recording lock").clone()
        }
    }
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
    fn a_request_with_arguments_is_still_one_json_line() {
        let line = request_with("screendump", &serde_json::json!({"filename": "a.png"}));
        assert_eq!(line.matches('\n').count(), 1, "the protocol is JSON lines");
        let parsed: serde_json::Value =
            serde_json::from_str(line.trim()).expect("must be valid JSON");
        assert_eq!(parsed["execute"], "screendump");
        assert_eq!(parsed["arguments"]["filename"], "a.png");
    }

    #[test]
    fn a_send_key_request_names_the_key_and_holds_it() {
        let line = send_key_request("spc", 100);
        assert_eq!(line.matches('\n').count(), 1, "the protocol is JSON lines");
        let parsed: serde_json::Value =
            serde_json::from_str(line.trim()).expect("send-key must be valid JSON");
        assert_eq!(parsed["execute"], "send-key");
        assert_eq!(parsed["arguments"]["keys"][0]["type"], "qcode");
        assert_eq!(parsed["arguments"]["keys"][0]["data"], "spc");
        assert_eq!(parsed["arguments"]["hold-time"], 100);
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
            "{\"return\": {\"status\": \"running\"}}\n"
        )
        .as_bytes();
        let value = read_until_reply(&mut input, "query-status").expect("a reply follows");
        assert_eq!(value["status"], "running");
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

    #[test]
    fn a_session_negotiates_first_and_then_answers_questions() {
        let server = fake::Server::start(&["running"]);
        let mut session = Session::connect(server.port).expect("connect to the fake monitor");
        assert_eq!(session.status().as_deref(), Ok("running"));
        assert_eq!(
            session.instruction_pointer().as_deref(),
            Some("0x00000000fffd3da2")
        );
        let shot = std::env::temp_dir().join("sunlit_xtask_qmp_screendump.png");
        assert_eq!(session.screendump(&shot), Ok(()));
        assert_eq!(session.quit(), Ok(()));

        let sent = server.received();
        assert!(
            sent.first()
                .is_some_and(|line| line.contains("qmp_capabilities")),
            "capabilities are negotiated before anything is asked: {sent:?}"
        );
        assert!(
            sent.iter().any(|line| line.contains("query-status")),
            "{sent:?}"
        );
        assert!(
            sent.iter()
                .any(|line| line.contains("screendump") && line.contains("\"png\"")),
            "the screenshot is asked for as a PNG: {sent:?}"
        );
        assert!(
            sent.iter().any(|line| line.contains("\"quit\"")),
            "{sent:?}"
        );
    }

    #[test]
    fn a_key_is_pressed_until_the_caller_says_to_stop() {
        let server = fake::Server::start(&["running"]);
        let mut session = Session::connect(server.port).expect("connect to the fake monitor");
        let stop = || true;
        let sent = press_key_until(&mut session, "spc", 60, Duration::from_secs(30), 100, &stop)
            .expect("press once");
        // One press, and no waiting: the gap belongs between presses, not after
        // the last one, or a build would pay for a key it has stopped needing.
        assert_eq!(sent, 1);
        let lines = server.received();
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains("send-key"))
                .count(),
            1,
            "{lines:?}"
        );
    }

    #[test]
    fn the_instruction_pointer_comes_out_of_the_register_dump() {
        // Trimmed from a real `info registers`, which is what the monitor
        // answers with: CRLF line ends and the whole register file.
        let dump = "\r\nCPU#0\r\nRAX=0000000080000000 RBX=00000000008129c0\r\n\
                    RIP=00000000fffd3da2 RFL=00000086 [--S--P-] CPL=0\r\n\
                    CR0=80000033 CR2=fffffffffffffce6\r\n";
        assert_eq!(parse_rip(dump).as_deref(), Some("0x00000000fffd3da2"));
        assert_eq!(parse_rip("no registers here"), None);
    }
}
