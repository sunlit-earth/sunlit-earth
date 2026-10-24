//! A second launch hands over to the instance that is already running.
//!
//! The first instance in tray mode always listens on a local socket of its
//! own, apart from the opt-in test IPC of `--ipc-socket`. A second launch that
//! finds the single-instance guard taken connects, reads `pid=<n>`, lets that
//! process take the foreground on Windows, and writes `key=value` lines that
//! say which copy was launched, ending with an empty line. The first instance
//! then shows and focuses its window, and says so when the launched copy is
//! another version.
//!
//! The name carries a per-user part, because Windows named pipes are visible
//! machine-wide and Linux abstract sockets to every user on the host: the
//! session id on Windows, which is also the scope of the guard's mutex there,
//! and the uid on Unix.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use interprocess::local_socket::traits::{ListenerExt, Stream as _};
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, ToNsName};
use tracing::{debug, info, warn};

use crate::MainWindow;
use crate::engine_client::EngineLink;

/// How long a second launch keeps trying to connect, which covers a first
/// instance that holds the guard and has not bound its socket yet.
pub const CONNECT_RETRY: Duration = Duration::from_secs(2);

/// How long a second launch waits between two attempts to connect.
const CONNECT_POLL: Duration = Duration::from_millis(50);

/// How long a second launch waits for the whole exchange before it gives up
/// and exits. The first instance binds before it selects an adapter and
/// answers once its window exists, so this covers a slow start as well as the
/// connect retry.
pub const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

/// The most lines and bytes one launch may send. A launch sends at most five
/// lines; anything past these limits is not this program talking.
const MAX_LINES: usize = 16;
const MAX_BYTES: u64 = 64 * 1024;

/// What a second launch says about itself. Every field is optional on the
/// wire, so a launch that leaves one out is still understood.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launch {
    /// The version of the copy that was launched.
    pub version: Option<String>,
    /// The executable that was launched.
    pub exe: Option<String>,
    /// The Wayland activation token the launcher gave it.
    pub activation_token: Option<String>,
    /// The X11 startup notification id the launcher gave it.
    pub startup_id: Option<String>,
}

impl Launch {
    /// This process, as the launch it is.
    #[must_use]
    pub fn of_this_process() -> Self {
        Self {
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            exe: std::env::current_exe()
                .ok()
                .map(|path| path.to_string_lossy().into_owned()),
            activation_token: non_empty_env("XDG_ACTIVATION_TOKEN"),
            startup_id: non_empty_env("DESKTOP_STARTUP_ID"),
        }
    }

    /// The lines a launch writes, the empty line that ends them included.
    #[must_use]
    pub fn encode(&self) -> String {
        let mut text = String::new();
        for (key, value) in self.fields() {
            if let Some(value) = value {
                text.push_str(key);
                text.push('=');
                text.push_str(&percent_encode(value));
                text.push('\n');
            }
        }
        text.push('\n');
        text
    }

    /// Read a launch back from its lines, without the empty one that ends
    /// them. A key this program does not know, or a value that does not
    /// decode, is left out.
    #[must_use]
    pub fn decode<'a>(lines: impl IntoIterator<Item = &'a str>) -> Self {
        let mut launch = Self::default();
        for line in lines {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let Some(value) = percent_decode(value) else {
                continue;
            };
            let slot = match key {
                "version" => &mut launch.version,
                "exe" => &mut launch.exe,
                "activation-token" => &mut launch.activation_token,
                "startup-id" => &mut launch.startup_id,
                _ => continue,
            };
            *slot = Some(value);
        }
        launch
    }

    /// Whether the launched copy is another version than `running`. A launch
    /// that does not say which version it is, the Dock's reopen being one, is
    /// taken to be this one.
    #[must_use]
    pub fn version_differs(&self, running: &str) -> bool {
        self.version.as_deref().is_some_and(|v| v != running)
    }

    fn fields(&self) -> [(&'static str, Option<&str>); 4] {
        [
            ("version", self.version.as_deref()),
            ("exe", self.exe.as_deref()),
            ("activation-token", self.activation_token.as_deref()),
            ("startup-id", self.startup_id.as_deref()),
        ]
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Percent-encode everything but ASCII letters, digits and `-._~/\:`, so a
/// value of any characters fits on one line and never holds a `=`.
pub(crate) fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/\\:".contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// The other side of [`percent_encode`]. `None` for a `%` not followed by two
/// hex digits, or bytes that are not UTF-8.
pub(crate) fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The name of the activation socket for an instance name and a user.
///
/// :param instance: the name the single-instance guard is taken under
/// :param user: the per-user part, [`user_part`] for this process
/// :returns: the socket name
#[must_use]
pub fn socket_name(instance: &str, user: &str) -> String {
    format!("{instance}-activation-{user}")
}

/// The per-user part of this process's names: the session id on Windows and
/// the uid on Unix.
#[must_use]
pub fn user_part() -> String {
    platform::user_part()
}

/// The X11 server time a startup notification id carries in its `_TIME`
/// suffix, which a window manager compares against the user's last input.
#[must_use]
pub fn startup_time(startup_id: &str) -> Option<u32> {
    let at = startup_id.rfind("_TIME")?;
    startup_id[at + "_TIME".len()..].parse().ok()
}

/// A socket that is bound and not yet being answered.
pub struct ActivationListener {
    listener: interprocess::local_socket::Listener,
    name: String,
}

/// Take the activation socket of `instance`.
///
/// A file left by a run that crashed is replaced on the platforms where the
/// name is a file, which is safe because the caller holds the guard.
///
/// :param instance: the name the single-instance guard is taken under
/// :returns: the bound socket, or why it could not be taken
pub fn bind(instance: &str) -> Result<ActivationListener, String> {
    let name = socket_name(instance, &user_part());
    let ns_name = name
        .as_str()
        .to_ns_name::<GenericNamespaced>()
        .map_err(|e| format!("`{name}` is not a usable socket name: {e}"))?;
    let listener = ListenerOptions::new()
        .name(ns_name)
        .try_overwrite(true)
        .max_spin_time(Duration::from_secs(1))
        .create_sync()
        .map_err(|e| format!("the activation socket `{name}` could not be opened: {e}"))?;
    Ok(ActivationListener { listener, name })
}

impl ActivationListener {
    /// Answer every launch that connects, on a thread of its own.
    ///
    /// Each connection is answered on a short thread of its own, so a client
    /// that connects and never finishes holds up nobody but itself. The
    /// listener's consumer is `on_launch`, and it runs for as long as the
    /// process does.
    ///
    /// :param `on_launch`: what a launch that said all it had to say does,
    ///     called on the connection's thread
    /// :returns: the listener thread, or why it could not be started
    pub fn serve(
        self,
        on_launch: impl Fn(Launch) + Send + Sync + 'static,
    ) -> Result<std::thread::JoinHandle<()>, String> {
        let Self { listener, name } = self;
        let on_launch: Arc<dyn Fn(Launch) + Send + Sync> = Arc::new(on_launch);
        let thread = std::thread::Builder::new()
            .name("activation".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    match conn {
                        Ok(stream) => {
                            let on_launch = Arc::clone(&on_launch);
                            let answered = std::thread::Builder::new()
                                .name("activation-answer".into())
                                .spawn(move || answer(stream, on_launch.as_ref()));
                            if let Err(e) = answered {
                                warn!("a second launch could not be answered: {e}");
                            }
                        }
                        Err(e) => warn!("activation accept error: {e}"),
                    }
                }
            })
            .map_err(|e| format!("the activation thread could not be started: {e}"))?;
        info!("activation socket ready on {name}");
        Ok(thread)
    }
}

/// Tell one connection this process's id and read the launch it describes.
fn answer(
    mut stream: interprocess::local_socket::Stream,
    on_launch: &(dyn Fn(Launch) + Send + Sync),
) {
    if let Err(e) = stream
        .write_all(format!("pid={}\n", std::process::id()).as_bytes())
        .and_then(|()| stream.flush())
    {
        debug!("a second launch left before it was told the pid: {e}");
        return;
    }
    if let Some(launch) = read_launch(BufReader::new(stream.take(MAX_BYTES))) {
        on_launch(launch);
    } else {
        debug!("a second launch left before it said which copy it was");
    }
}

/// The lines up to the empty one, or `None` where the connection ends first
/// or says more than any launch would.
fn read_launch(reader: impl BufRead) -> Option<Launch> {
    let mut lines = Vec::new();
    for line in reader.lines() {
        let line = line.ok()?;
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            return Some(Launch::decode(lines.iter().map(String::as_str)));
        }
        if lines.len() == MAX_LINES {
            return None;
        }
        lines.push(line.to_owned());
    }
    None
}

/// What a second launch did about the instance that is already running.
#[derive(Debug, PartialEq, Eq)]
pub enum SecondLaunch {
    /// A hidden start, an autostart entry firing beside a running app, which
    /// must not open the window and so says nothing.
    StayedQuiet,
    /// The running instance, this process id, was told to show its window.
    HandedOver(u32),
    /// The running instance could not be reached, and why.
    Unreachable(String),
}

/// What a launch that found the guard taken does.
///
/// :param instance: the name the single-instance guard is taken under
/// :param hidden: whether this launch was asked to start hidden
/// :returns: what came of it
pub fn second_launch(instance: &str, hidden: bool) -> SecondLaunch {
    if hidden {
        return SecondLaunch::StayedQuiet;
    }
    match hand_over(instance, Launch::of_this_process()) {
        Ok(pid) => SecondLaunch::HandedOver(pid),
        Err(e) => SecondLaunch::Unreachable(e),
    }
}

/// Hand this launch over to the running instance of `instance`.
///
/// The exchange runs on a thread of its own and is given up after
/// [`EXCHANGE_TIMEOUT`], so a first instance that never answers cannot keep
/// the second one around.
///
/// :param instance: the name the single-instance guard is taken under
/// :param launch: what to tell it
/// :returns: the running instance's process id, or why it could not be
///     reached
pub fn hand_over(instance: &str, launch: Launch) -> Result<u32, String> {
    let name = socket_name(instance, &user_part());
    let (tx, rx) = crossbeam_channel::bounded(1);
    std::thread::Builder::new()
        .name("activation-client".into())
        .spawn(move || {
            let _ = tx.send(exchange(&name, &launch));
        })
        .map_err(|e| format!("the hand-over thread could not be started: {e}"))?;
    rx.recv_timeout(EXCHANGE_TIMEOUT).unwrap_or_else(|_| {
        Err(format!(
            "the running instance did not answer within {} s",
            EXCHANGE_TIMEOUT.as_secs()
        ))
    })
}

fn exchange(name: &str, launch: &Launch) -> Result<u32, String> {
    let stream = connect(name)?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("the running instance's answer could not be read: {e}"))?;
    let pid: u32 = line
        .trim()
        .strip_prefix("pid=")
        .and_then(|pid| pid.parse().ok())
        .ok_or_else(|| format!("the running instance answered `{}`", line.trim()))?;
    platform::allow_foreground(pid);
    let stream = reader.get_mut();
    stream
        .write_all(launch.encode().as_bytes())
        .and_then(|()| stream.flush())
        .map_err(|e| format!("the launch could not be handed over: {e}"))?;
    Ok(pid)
}

fn connect(name: &str) -> Result<interprocess::local_socket::Stream, String> {
    let deadline = Instant::now() + CONNECT_RETRY;
    loop {
        let ns_name = name
            .to_ns_name::<GenericNamespaced>()
            .map_err(|e| format!("`{name}` is not a usable socket name: {e}"))?;
        match interprocess::local_socket::Stream::connect(ns_name) {
            Ok(stream) => return Ok(stream),
            Err(e) if Instant::now() >= deadline => {
                return Err(format!("the running instance could not be reached: {e}"));
            }
            Err(_) => std::thread::sleep(CONNECT_POLL),
        }
    }
}

/// Show, focus and announce: what a second launch, and on macOS the Dock's
/// reopen, does in the running instance.
///
/// Called on the event loop.
///
/// :param window: the settings window
/// :param engine: the engine whose preview follows the window
/// :param launch: what the second launch said about itself
pub fn activate(window: &MainWindow, engine: &EngineLink, launch: &Launch) {
    let running = env!("CARGO_PKG_VERSION");
    let differs = launch.version_differs(running);
    info!(
        launched_version = launch.version.as_deref().unwrap_or("unknown"),
        launched_exe = launch.exe.as_deref().unwrap_or("unknown"),
        version_differs = differs,
        "another launch asked for the window"
    );
    crate::window_state::show_and_focus(window, engine, launch.startup_id.as_deref());
    if differs {
        show_version_notice(
            window,
            running,
            launch.version.as_deref().unwrap_or_default(),
        );
    }
    crate::ipc::signal(&format!("activated version_differs={differs}"));
}

/// Open the notice that the launched copy, another version, did not start.
///
/// :param window: the settings window
/// :param running: this instance's version
/// :param launched: the version that was launched
pub fn show_version_notice(window: &MainWindow, running: &str, launched: &str) {
    window.set_version_notice_running(running.into());
    window.set_version_notice_launched(launched.into());
    window.set_version_notice_open(true);
}

/// Wire the notice's "Quit Sunlit Earth" to the same end as the tray's Exit.
///
/// :param window: the settings window
pub fn register_version_notice(window: &MainWindow) {
    window.on_quit_app(|| {
        debug!("the version notice asked to quit");
        slint::quit_event_loop().ok();
    });
}

#[cfg(windows)]
mod platform {
    pub(super) fn user_part() -> String {
        use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;

        let mut session = 0u32;
        // SAFETY: the pointer is to a live local the call writes one u32 to.
        #[allow(unsafe_code)]
        let ok = unsafe { ProcessIdToSessionId(std::process::id(), &raw mut session) };
        if ok == 0 {
            "session-unknown".to_owned()
        } else {
            format!("session{session}")
        }
    }

    pub(super) fn allow_foreground(pid: u32) {
        use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

        // SAFETY: takes a process id by value and touches no memory of ours.
        #[allow(unsafe_code)]
        let ok = unsafe { AllowSetForegroundWindow(pid) };
        if ok == 0 {
            tracing::debug!("the running instance could not be allowed the foreground");
        }
    }
}

#[cfg(unix)]
mod platform {
    pub(super) fn user_part() -> String {
        // SAFETY: getuid has no preconditions and cannot fail.
        #[allow(unsafe_code)]
        let uid = unsafe { libc::getuid() };
        format!("uid{uid}")
    }

    pub(super) fn allow_foreground(_pid: u32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launch_survives_its_encoding() {
        let launch = Launch {
            version: Some("1.2.3-beta.1".to_owned()),
            exe: Some(r"C:\Program Files\Sunlit 100% = Earth\sunlit-earth.exe".to_owned()),
            activation_token: Some("kwin-42_TIME1234".to_owned()),
            startup_id: Some("/home/jörg/Ünïcode ☀/sunlit-earth".to_owned()),
        };
        let text = launch.encode();
        assert!(text.ends_with("\n\n"), "{text:?}");
        let lines: Vec<&str> = text.lines().take_while(|line| !line.is_empty()).collect();
        for line in &lines {
            assert_eq!(line.matches('=').count(), 1, "{line}");
            assert!(line.is_ascii(), "{line}");
        }
        assert_eq!(Launch::decode(lines), launch);
    }

    #[test]
    fn a_launch_that_leaves_fields_out_is_still_understood() {
        let launch = Launch {
            version: Some("1.0.0".to_owned()),
            ..Launch::default()
        };
        let text = launch.encode();
        assert_eq!(Launch::decode(text.lines()), launch);
        assert_eq!(
            Launch::decode(["unknown=x", "no equals sign", "exe=%zz", "version=2"]),
            Launch {
                version: Some("2".to_owned()),
                ..Launch::default()
            }
        );
    }

    #[test]
    fn the_notice_is_for_another_version_only() {
        let running = "0.5.0";
        let launch = |version: Option<&str>| Launch {
            version: version.map(str::to_owned),
            ..Launch::default()
        };
        assert!(!launch(Some(running)).version_differs(running));
        assert!(launch(Some("0.6.0")).version_differs(running));
        assert!(
            !launch(None).version_differs(running),
            "a reopen that says nothing is this version"
        );
    }

    #[test]
    fn the_running_instance_reads_up_to_the_empty_line() {
        let launch = Launch {
            version: Some("9.9.9".to_owned()),
            exe: Some("/opt/sunlit earth".to_owned()),
            ..Launch::default()
        };
        let text = launch.encode() + "trailing=ignored\n";
        assert_eq!(read_launch(text.as_bytes()), Some(launch));
        assert_eq!(
            read_launch("version=1\n".as_bytes()),
            None,
            "a launch that ends before its empty line said nothing"
        );
        let flood = "x=y\n".repeat(MAX_LINES + 1) + "\n";
        assert_eq!(read_launch(flood.as_bytes()), None);
    }

    #[test]
    fn the_socket_name_is_per_user_and_stable() {
        let a = socket_name("sunlit-earth-app", "uid1000");
        assert_eq!(a, socket_name("sunlit-earth-app", "uid1000"));
        assert_ne!(a, socket_name("sunlit-earth-app", "uid1001"));
        assert_ne!(a, socket_name("sunlit-earth-e2e", "uid1000"));
        assert_eq!(user_part(), user_part());
    }

    #[test]
    fn the_startup_time_is_the_ids_time_suffix() {
        assert_eq!(startup_time("kdeinit;host;1234;0_TIME5678"), Some(5678));
        assert_eq!(startup_time("no-time-here"), None);
        assert_eq!(startup_time("broken_TIMEnow"), None);
    }

    #[test]
    fn a_hidden_second_launch_does_not_connect() {
        let instance = format!("sunlit-earth-activation-quiet-{}", std::process::id());
        let (tx, rx) = crossbeam_channel::bounded(1);
        let _thread = bind(&instance)
            .expect("the socket binds")
            .serve(move |launch| {
                let _ = tx.send(launch);
            })
            .expect("the listener starts");
        assert_eq!(second_launch(&instance, true), SecondLaunch::StayedQuiet);
        assert!(
            rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "a hidden launch reached the running instance"
        );
    }

    /// The whole exchange over a real socket, as a second launch and the
    /// running instance do it.
    #[test]
    fn a_second_launch_reaches_the_running_instance() {
        let instance = format!("sunlit-earth-activation-test-{}", std::process::id());
        let (tx, rx) = crossbeam_channel::bounded(1);
        let _thread = bind(&instance)
            .expect("the socket binds")
            .serve(move |launch| {
                let _ = tx.send(launch);
            })
            .expect("the listener starts");
        let launch = Launch {
            version: Some("0.0.1".to_owned()),
            exe: Some("/a path/with = signs %".to_owned()),
            ..Launch::default()
        };
        let pid = hand_over(&instance, launch.clone()).expect("the exchange completes");
        assert_eq!(pid, std::process::id());
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5))
                .expect("the launch arrives"),
            launch
        );
    }
}
