//! Reacting to the session ending: a reboot, a shutdown, or a sign-out.
//!
//! Two mechanisms, because the two platforms ask in completely different ways.
//! Windows sends window messages, which is most of this file. A Linux desktop
//! sends SIGTERM and then kills what has not exited, so the Linux half is a
//! signal listener and is much shorter. Both end in the same place: quit the
//! event loop once, and exit anyway if that does not take.
//!
//! Windows asks every top-level window for permission first
//! (`WM_QUERYENDSESSION`) and then tells it the session is going
//! (`WM_ENDSESSION`). An application that answers neither is the one the
//! shutdown screen names as preventing the reboot, and Windows kills it once
//! its timeout runs out. winit handles neither message, and Slint is winit, so
//! nothing in this program ever learned that Windows was going down: the tray
//! process sat there until it was killed.
//!
//! The listener is a window of this module's own rather than a hook into
//! Slint's. Reaching into Slint's window means subclassing a handle winit owns
//! and following whatever it does with it; a window here is independent of
//! whether the settings window is shown, hidden, or gone. It is created and
//! never shown, and it is deliberately an ordinary top-level window rather than
//! a message-only one, because `HWND_MESSAGE` windows do not receive these two
//! messages at all.
//!
//! It pumps its own messages on its own thread, so the answer does not wait on
//! whatever the main thread is doing. The answer to `WM_QUERYENDSESSION` is
//! always yes: vetoing is what puts an application on that screen. Nothing is
//! torn down until `WM_ENDSESSION` says the session is really ending, because
//! a proposed shutdown can still be cancelled.
//!
//! What the Linux half is not is the whole conversation. A desktop that wants to
//! know whether an application is *ready* to be closed asks over the session bus,
//! through logind's inhibitor protocol; SIGTERM arrives after that decision has
//! been made. So this uses the grace period rather than participating in the
//! question, and the inhibitor half stays on the roadmap. macOS gets nothing yet.

use std::time::Duration;

/// `WM_QUERYENDSESSION`: may the session end?
pub const WM_QUERYENDSESSION: u32 = 0x0011;

/// `WM_ENDSESSION`: it is ending, or it was cancelled.
pub const WM_ENDSESSION: u32 = 0x0016;

/// How long the ordinary teardown gets before the process ends the blunt way.
///
/// Windows kills an application that has not exited within its own timeout,
/// five seconds by default, and shows the "these apps are preventing you from
/// shutting down" screen while it waits. A clean exit here takes milliseconds,
/// so this is only for the case where the event loop cannot be reached at all,
/// and it sits well inside the window Windows gives us.
pub const FORCE_EXIT_AFTER: Duration = Duration::from_secs(3);

/// What one window message means for us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Answer yes, and do nothing else.
    Permit,
    /// The session is ending: shut down now.
    End,
    /// Not one of ours.
    Ignore,
}

/// The whole decision, as a pure function of the message.
///
/// `ending` is `WM_ENDSESSION`'s `wParam`, which is false when a shutdown was
/// proposed and then cancelled. Quitting on that would close the application
/// for a reboot that never happened.
pub fn classify(message: u32, ending: bool) -> Action {
    match message {
        WM_QUERYENDSESSION => Action::Permit,
        WM_ENDSESSION if ending => Action::End,
        _ => Action::Ignore,
    }
}

#[cfg(windows)]
mod platform {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use tracing::{debug, info, warn};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG,
        RegisterClassW, WNDCLASSW, WS_OVERLAPPED,
    };

    use super::{Action, classify};

    /// The window class this module registers.
    pub const CLASS_NAME: &str = "SunlitEarthSessionEnd";

    /// The installed listener.
    #[derive(Debug, Clone, Copy)]
    pub struct Watcher {
        hwnd: isize,
    }

    impl Watcher {
        /// The listening window, for a test that delivers the messages Windows
        /// would.
        pub fn hwnd(self) -> isize {
            self.hwnd
        }
    }

    struct Handler {
        on_end: Box<dyn Fn() + Send + Sync>,
        force_exit_after: Option<Duration>,
        started: AtomicBool,
    }

    /// One per process: a window procedure is a plain function pointer with
    /// nowhere to keep state, and the application installs exactly one.
    static HANDLER: OnceLock<Handler> = OnceLock::new();

    /// Start listening. `None` if a listener is already installed or the window
    /// could not be created.
    pub fn install(
        force_exit_after: Option<Duration>,
        on_end: impl Fn() + Send + Sync + 'static,
    ) -> Option<Watcher> {
        let handler = Handler {
            on_end: Box::new(on_end),
            force_exit_after,
            started: AtomicBool::new(false),
        };
        if HANDLER.set(handler).is_err() {
            warn!("a session-end listener is already installed");
            return None;
        }

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("session-end".to_owned())
            .spawn(move || {
                let hwnd = create_window();
                let _ = tx.send(hwnd);
                if hwnd != 0 {
                    pump();
                }
            })
            .ok()?;

        let hwnd = rx.recv().ok()?;
        if hwnd == 0 {
            warn!("could not create the session-end listener window");
            return None;
        }
        debug!("session-end listener running");
        Some(Watcher { hwnd })
    }

    /// Run the shutdown once, however many times Windows asks.
    fn begin_shutdown() {
        let Some(handler) = HANDLER.get() else {
            return;
        };
        if handler.started.swap(true, Ordering::SeqCst) {
            return;
        }
        info!("windows is ending the session; shutting down");
        (handler.on_end)();

        let Some(after) = handler.force_exit_after else {
            return;
        };
        // The retrospective retired `process::exit` from the ordinary path, and
        // this is not it: the session is going either way, and the choice here
        // is between exiting ourselves and being the application Windows names
        // on the shutdown screen before killing it.
        let _ = std::thread::Builder::new()
            .name("session-end-guard".to_owned())
            .spawn(move || {
                std::thread::sleep(after);
                warn!("the event loop did not stop in {after:?}; exiting now");
                std::process::exit(0);
            });
    }

    /// A NUL-terminated UTF-16 copy of `text`, as every wide Win32 call wants.
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// A zeroed `WNDCLASSW` or `MSG`, which is what Win32 wants for the fields
    /// this code does not set.
    fn zeroed<T>() -> T {
        // SAFETY: both structures are plain C aggregates of integers and
        // pointers, and all-zero is the documented "unused" value for every
        // field left alone here.
        #[allow(unsafe_code)]
        unsafe {
            std::mem::zeroed()
        }
    }

    /// Register the class and create the invisible top-level window.
    fn create_window() -> isize {
        let class = wide(CLASS_NAME);
        let title = wide("Sunlit Earth session listener");

        // SAFETY: `GetModuleHandleW(null)` is the documented way to ask for
        // this executable's own module handle, and it takes nothing else.
        #[allow(unsafe_code)]
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };

        let mut class_def: WNDCLASSW = zeroed();
        class_def.lpfnWndProc = Some(wndproc);
        class_def.hInstance = instance;
        class_def.lpszClassName = class.as_ptr();

        // SAFETY: a valid `WNDCLASSW` whose string outlives the call. A class
        // that is already registered fails harmlessly and the creation below
        // still succeeds, so the result is only worth logging.
        #[allow(unsafe_code)]
        let atom = unsafe { RegisterClassW(&raw const class_def) };
        if atom == 0 {
            debug!("the session-end window class was already registered");
        }

        // SAFETY: both wide strings outlive the call, the parent and menu
        // handles are null because this is an unowned top-level window, and the
        // instance is this module's own. The window is never shown.
        #[allow(unsafe_code)]
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPED,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        hwnd as isize
    }

    /// The message loop. `GetMessageW` is also what lets Windows deliver these
    /// two messages, which it sends rather than posts.
    fn pump() {
        loop {
            let mut message: MSG = zeroed();
            // SAFETY: a valid, writable `MSG` and a null window filter, which
            // means every message for this thread.
            #[allow(unsafe_code)]
            let result = unsafe { GetMessageW(&raw mut message, std::ptr::null_mut(), 0, 0) };
            if result <= 0 {
                return;
            }
            // SAFETY: dispatching the message just filled in, unmodified.
            #[allow(unsafe_code)]
            unsafe {
                DispatchMessageW(&raw const message);
            }
        }
    }

    /// The window procedure. Windows calls this on the listener thread.
    #[allow(unsafe_code)]
    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match classify(message, wparam != 0) {
            // TRUE: yes, the session may end.
            Action::Permit => 1,
            Action::End => {
                begin_shutdown();
                0
            }
            // SAFETY: forwarding a message the system sent, unchanged, to the
            // default handler, which is what a window procedure must do with
            // everything it does not handle itself.
            Action::Ignore => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }
}

/// What a caught signal means for us.
///
/// The Linux analog of [`classify`], and a pure function for the same reason: the
/// decision is the part worth testing, and the delivery mechanism is not
/// something a test can produce on demand without ending the test process.
///
/// SIGTERM only. A desktop session ending sends SIGTERM and then, after its own
/// grace period, SIGKILL, which nothing can answer. SIGHUP is deliberately not
/// treated as the session ending: it arrives when a controlling terminal goes
/// away, and the e2e suite starts this application from a process whose terminal
/// is not its own, so answering it would end a run in the middle of a test.
/// SIGINT is left to its default, because Ctrl-C in a terminal already ends the
/// process and a developer pressing it is not a session ending.
#[cfg(target_os = "linux")]
pub fn classify_signal(signal: i32) -> Action {
    if signal == signal_hook::consts::SIGTERM {
        Action::End
    } else {
        Action::Ignore
    }
}

#[cfg(windows)]
pub use platform::{CLASS_NAME, Watcher, install};

#[cfg(target_os = "linux")]
pub use unix::{Watcher, install};

/// SIGTERM, on its own thread, which is the Linux session's way of saying it is
/// going (phase 5 decision 8).
///
/// The same shape as the Windows listener and for the same reasons: its own
/// thread, so the answer does not wait on whatever the main thread is doing, and
/// one shutdown however many times the question is asked. What it is not is the
/// whole of the story. A desktop that wants to know whether an application is
/// ready to be closed asks over the session bus, through logind's inhibitor
/// protocol, and this answers nothing there: what it does is take the ten to
/// ninety seconds of grace the session gives a process it has already decided to
/// end, and use them to exit cleanly rather than be killed. The inhibitor half is
/// a roadmap item.
///
/// A signal handler may call almost nothing, and quitting a Slint event loop is
/// not on that list, so nothing here runs in a handler: `signal-hook`'s iterator
/// blocks on a self-pipe the handler writes one byte to, and everything below
/// happens on an ordinary thread.
#[cfg(target_os = "linux")]
mod unix {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use signal_hook::consts::SIGTERM;
    use signal_hook::iterator::Signals;
    use tracing::{debug, info, warn};

    use super::{Action, classify_signal};

    /// The installed listener. Nothing to reach into, unlike the Windows one,
    /// which owns a window a test delivers messages to.
    #[derive(Debug, Clone, Copy)]
    pub struct Watcher;

    /// Start listening. `None` if the signal could not be hooked at all.
    pub fn install(
        force_exit_after: Option<Duration>,
        on_end: impl Fn() + Send + Sync + 'static,
    ) -> Option<Watcher> {
        let mut signals = match Signals::new([SIGTERM]) {
            Ok(signals) => signals,
            Err(e) => {
                warn!(error = %e, "could not listen for SIGTERM");
                return None;
            }
        };
        std::thread::Builder::new()
            .name("session-end".to_owned())
            .spawn(move || {
                let started = AtomicBool::new(false);
                for signal in &mut signals {
                    if classify_signal(signal) != Action::End {
                        continue;
                    }
                    if started.swap(true, Ordering::SeqCst) {
                        continue;
                    }
                    info!(signal, "the session is ending; shutting down");
                    on_end();
                    if let Some(after) = force_exit_after {
                        // The same trade the Windows listener makes: the session
                        // is going either way, so the choice is between exiting
                        // and being killed partway through doing it.
                        std::thread::sleep(after);
                        warn!("the event loop did not stop in {after:?}; exiting now");
                        std::process::exit(0);
                    }
                    return;
                }
            })
            .ok()?;
        debug!("session-end listener running");
        Some(Watcher)
    }
}

/// Nothing to install where neither mechanism exists.
#[cfg(not(any(windows, target_os = "linux")))]
#[derive(Debug, Clone, Copy)]
pub struct Watcher;

/// macOS asks over its own notification centre, which is not wired up, and
/// saying so is better than a listener that never fires.
#[cfg(not(any(windows, target_os = "linux")))]
pub fn install(
    _force_exit_after: Option<Duration>,
    _on_end: impl Fn() + Send + Sync + 'static,
) -> Option<Watcher> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_is_answered_without_acting_on_it() {
        // Vetoing is what puts an application on the shutdown screen, and a
        // proposed shutdown can still be cancelled by something else.
        assert_eq!(classify(WM_QUERYENDSESSION, true), Action::Permit);
        assert_eq!(classify(WM_QUERYENDSESSION, false), Action::Permit);
    }

    /// The Linux decision, which is one signal and not the others.
    ///
    /// SIGHUP is the one worth pinning rather than merely leaving out: it arrives
    /// when a controlling terminal goes away, and the e2e suite starts this
    /// application from a process whose terminal is not its own, so treating it
    /// as the session ending would end a run in the middle of a test. SIGINT is
    /// left to its default, because Ctrl-C in a terminal already ends the process
    /// and a developer pressing it is not a session ending.
    #[test]
    #[cfg(target_os = "linux")]
    fn only_sigterm_means_the_linux_session_is_ending() {
        use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM, SIGUSR1};

        assert_eq!(classify_signal(SIGTERM), Action::End);
        for other in [SIGHUP, SIGINT, SIGUSR1] {
            assert_eq!(classify_signal(other), Action::Ignore, "{other}");
        }
    }

    #[test]
    fn only_a_session_that_is_really_ending_shuts_the_app_down() {
        assert_eq!(classify(WM_ENDSESSION, true), Action::End);
        assert_eq!(
            classify(WM_ENDSESSION, false),
            Action::Ignore,
            "a cancelled shutdown must not close the application"
        );
    }

    #[test]
    fn everything_else_goes_to_the_default_handler() {
        for message in [0x0001, 0x0002, 0x0010, 0x0018, 0x0400] {
            assert_eq!(classify(message, true), Action::Ignore, "{message:#06x}");
        }
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

    static ENDED: AtomicUsize = AtomicUsize::new(0);

    /// Deliver a message the way Windows does, synchronously, so the assertion
    /// after it needs no waiting.
    fn send(hwnd: isize, message: u32, wparam: usize) -> isize {
        // SAFETY: a window this test installed, and two of its documented
        // messages. `SendMessageW` returns once the window procedure has run.
        #[allow(unsafe_code)]
        unsafe {
            SendMessageW(hwnd as _, message, wparam, 0)
        }
    }

    #[test]
    fn the_listener_answers_windows_and_shuts_down_once() {
        let Some(watcher) = install(None, || {
            ENDED.fetch_add(1, Ordering::SeqCst);
        }) else {
            // No window station, which is a property of the environment rather
            // than of this code. The decision itself is covered above.
            println!("skipping: could not create a listener window here");
            return;
        };
        let hwnd = watcher.hwnd();

        assert_eq!(
            send(hwnd, WM_QUERYENDSESSION, 1),
            1,
            "a veto is what makes Windows name us as blocking the reboot"
        );
        assert_eq!(
            ENDED.load(Ordering::SeqCst),
            0,
            "a question is not a verdict"
        );

        send(hwnd, WM_ENDSESSION, 0);
        assert_eq!(
            ENDED.load(Ordering::SeqCst),
            0,
            "the shutdown was cancelled"
        );

        send(hwnd, WM_ENDSESSION, 1);
        assert_eq!(ENDED.load(Ordering::SeqCst), 1);

        send(hwnd, WM_ENDSESSION, 1);
        assert_eq!(
            ENDED.load(Ordering::SeqCst),
            1,
            "the teardown runs once, however often Windows asks"
        );
    }
}
