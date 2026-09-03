//! Being told that the display layout moved, rather than finding out later.
//!
//! One thread per process, blocked on the platform's own notification, calling
//! a closure once per hint. It has no opinion about what a hint means: whether
//! anything actually changed is settled by asking [`super::monitors`] again,
//! which is the engine's job and not this module's.
//!
//! Nothing already in the process could be reused. winit selects `RandR` input on
//! the root for its own monitor cache and surfaces no event for it, and handles
//! `WM_SETTINGCHANGE` for the theme and nothing for `WM_DISPLAYCHANGE`; Slint
//! has no monitor API at all. So the subscription has to be the program's own,
//! and it has to live somewhere that exists whether or not a window does. That
//! is the same conclusion `session_end.rs` in the app reached for the shutdown
//! messages, and the Windows half here has the same shape as a result.
//!
//! A watcher that cannot start is not an error. The app carries on exactly as
//! it did before this existed: the monitor list is re-queried on every publish,
//! and the auto-refresh interval decides how long a stale layout stays stale.

use std::sync::Arc;

/// Called once per hint the platform gives, on the watcher's own thread.
///
/// It may run for changes that turn out to be nothing, so a consumer has to be
/// cheap about a hint that leads nowhere.
pub type NotifyFn = Arc<dyn Fn() + Send + Sync>;

#[cfg(target_os = "linux")]
pub use x11::{Watcher, start};

#[cfg(windows)]
pub use win32::{CLASS_NAME, Watcher, start};

/// `RandR` on Linux and `WM_DISPLAYCHANGE` on Windows. macOS has
/// `CGDisplayRegisterReconfigurationCallback`, and that belongs to the setter
/// on the day there is one.
#[cfg(not(any(windows, target_os = "linux")))]
#[derive(Debug)]
pub struct Watcher;

#[cfg(not(any(windows, target_os = "linux")))]
impl Watcher {
    pub fn stop(self) {}
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn start(_notify: NotifyFn) -> Option<Watcher> {
    tracing::debug!("this platform has no way to watch the displays");
    None
}

/// `RandR`, through the pure-Rust X11 connection.
///
/// The connection is to the X server the query already talks to: under a
/// Wayland session that is `XWayland`, which implements `RandR` over the
/// compositor's outputs and is expected to fire for the same changes. That half
/// is unverified until a Wayland session is something the suite runs in. A
/// Wayland session with no `XWayland` has no `DISPLAY`, no watcher, and no query,
/// all for the same reason.
///
/// `RandR` rather than the alternatives, each considered. D-Bus signals exist per
/// desktop (`KScreen`, Mutter's `DisplayConfig`) and there is no common one; udev
/// hotplug events see a connector change rather than a layout change made in a
/// settings dialog, and see nothing at all through `XWayland`; polling `xrandr` is
/// the busy loop this feature exists to avoid.
#[cfg(target_os = "linux")]
mod x11 {
    use std::sync::Arc;
    use std::thread::JoinHandle;

    use tracing::{debug, warn};
    use x11rb::connection::{Connection, RequestConnection as _};
    use x11rb::protocol::Event;
    use x11rb::protocol::randr::{self, ConnectionExt as _, NotifyMask};
    use x11rb::protocol::xproto::{
        Atom, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, Window,
        WindowClass,
    };
    use x11rb::rust_connection::RustConnection;

    use super::NotifyFn;

    /// The atom the stop message carries, so the wake this module sends is
    /// distinguishable from any other client message that reaches the window.
    const STOP: &str = "SUNLIT_EARTH_DISPLAY_WATCH_STOP";

    /// The running watcher, and the two things stopping it needs.
    ///
    /// The connection is shared with the thread on purpose. It is the one place
    /// this program uses a connection from two threads at once, and it is the
    /// case `RustConnection` is built for: the reader releases the inner lock
    /// while it waits, so the sender below gets through.
    #[derive(Debug)]
    pub struct Watcher {
        conn: Arc<RustConnection>,
        window: Window,
        stop: Atom,
        thread: JoinHandle<()>,
    }

    impl Watcher {
        /// Wake the thread and join it.
        ///
        /// Microseconds: the wake is an event sent to a window this process
        /// created, on a connection it already holds.
        pub fn stop(self) {
            if let Err(e) = self.wake() {
                // The thread is blocked on a connection that will not carry the
                // wake, so joining it would block for as long as the process
                // lives. Letting it go is what the process shutdown does anyway.
                warn!(error = %e, "the display watcher could not be woken to stop");
                return;
            }
            if self.thread.join().is_err() {
                warn!("the display watcher thread panicked");
            }
            debug!("display watcher stopped");
        }

        /// Send the stop message and get it onto the wire.
        ///
        /// An empty event mask delivers the event to the client that created
        /// the destination window, which is this connection, and the thread
        /// blocked in `wait_for_event` is what receives it.
        fn wake(&self) -> Result<(), Box<dyn std::error::Error>> {
            let event = ClientMessageEvent::new(32, self.window, self.stop, [0u32; 5]);
            self.conn
                .send_event(false, self.window, EventMask::NO_EVENT, event)?
                .check()?;
            self.conn.flush()?;
            Ok(())
        }
    }

    /// Start watching, or say why not.
    pub fn start(notify: NotifyFn) -> Option<Watcher> {
        // `DISPLAY` unset is the ordinary headless case, and `outputs()` already
        // decided that it is not worth a line in the log.
        crate::env_override("DISPLAY")?;
        match connect(notify) {
            Ok(watcher) => {
                debug!("display watcher running");
                Some(watcher)
            }
            Err(e) => {
                warn!(error = %e, "the displays cannot be watched in this session");
                None
            }
        }
    }

    /// Everything that can fail, in one place, so `start` reads as a decision.
    fn connect(notify: NotifyFn) -> Result<Watcher, Box<dyn std::error::Error>> {
        let (conn, screen) = RustConnection::connect(None)?;
        let conn = Arc::new(conn);
        if conn
            .extension_information(randr::X11_EXTENSION_NAME)?
            .is_none()
        {
            return Err("this X server has no RandR extension".into());
        }
        // The version has to be declared before the server will send anything
        // but the 1.0 screen-change event, so this is what makes the CRTC and
        // output notifications arrive at all.
        conn.randr_query_version(1, 3)?.reply()?;

        let root = conn.setup().roots[screen].root;
        let window = conn.generate_id()?;
        // Something to be woken through, and nothing else: unmapped, one pixel,
        // and `InputOnly`, so it has no pixels to draw and never appears.
        conn.create_window(
            0,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new(),
        )?
        .check()?;

        // `OUTPUT_PROPERTY` is deliberately left out: it fires for EDID and
        // backlight properties, which move no rectangle.
        let mask = NotifyMask::SCREEN_CHANGE | NotifyMask::CRTC_CHANGE | NotifyMask::OUTPUT_CHANGE;
        conn.randr_select_input(root, mask)?.check()?;

        let stop = conn.intern_atom(false, STOP.as_bytes())?.reply()?.atom;
        conn.flush()?;

        let thread_conn = Arc::clone(&conn);
        let thread = std::thread::Builder::new()
            .name("display-watch".to_owned())
            .spawn(move || pump(&thread_conn, window, stop, &notify))?;
        Ok(Watcher {
            conn,
            window,
            stop,
            thread,
        })
    }

    /// Block on the connection until a hint arrives or the stop message does.
    ///
    /// A connection error ends the thread. The app then has what it had before
    /// this module existed, which is a monitor list re-queried on every publish.
    fn pump(conn: &RustConnection, window: Window, stop: Atom, notify: &NotifyFn) {
        loop {
            match conn.wait_for_event() {
                Ok(Event::ClientMessage(message))
                    if message.window == window && message.type_ == stop =>
                {
                    return;
                }
                Ok(Event::RandrScreenChangeNotify(_) | Event::RandrNotify(_)) => {
                    debug!("randr says the layout may have moved");
                    notify();
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(error = %e, "the display watcher's X11 connection ended");
                    return;
                }
            }
        }
    }
}

/// `WM_DISPLAYCHANGE`, on an invisible top-level window of this module's own.
///
/// A message-only window will not do. Microsoft's own words are that one "is
/// not visible, has no z-order, cannot be enumerated, and does not receive
/// broadcast messages", and `WM_DISPLAYCHANGE` is a broadcast to top-level
/// windows. So this is the established shape instead: an ordinary top-level
/// window that is never shown, pumping its own messages on its own thread.
/// `GLFW`'s Win32 helper window is the same thing for the same reason.
///
/// Not a second use of the app's `session_end` window, and not Slint's. The two
/// listeners are the same dozen lines of Win32 and nothing else: different
/// messages, different consumers, different lifetimes, and their Linux halves
/// have nothing in common, so a shared module would be shared on one platform
/// only. Slint's window is winit's, it is hidden in tray mode, and the message
/// would have to be caught in a procedure this program does not own.
///
/// `WM_DEVICECHANGE` and `WM_SETTINGCHANGE` are deliberately not taken.
/// `EnumDisplayMonitors` lists the active monitors, and that set moves only
/// through an applied display configuration change, which is what
/// `WM_DISPLAYCHANGE` announces; a monitor Windows has not activated is absent
/// from the list either way. Adding a second message here is one line, and the
/// rest of the path takes a redundant hint at the cost of one comparison.
#[cfg(windows)]
mod win32 {
    use std::sync::{Mutex, OnceLock};
    use std::thread::JoinHandle;

    use tracing::{debug, warn};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG,
        PostMessageW, PostQuitMessage, RegisterClassW, WM_CLOSE, WM_DESTROY, WNDCLASSW,
        WS_OVERLAPPED,
    };

    use super::NotifyFn;

    /// `WM_DISPLAYCHANGE`: the display resolution, depth or arrangement has
    /// changed. Spelled here the way `session_end` spells the two it answers.
    const WM_DISPLAYCHANGE: u32 = 0x007E;

    /// The window class this module registers.
    pub const CLASS_NAME: &str = "SunlitEarthDisplayWatch";

    /// The running watcher.
    #[derive(Debug)]
    pub struct Watcher {
        hwnd: isize,
        thread: JoinHandle<()>,
    }

    impl Watcher {
        /// The listening window, for a test that delivers the message Windows
        /// would.
        pub fn hwnd(&self) -> isize {
            self.hwnd
        }

        /// Wake the thread and join it.
        ///
        /// `WM_CLOSE` is what `DefWindowProcW` turns into `DestroyWindow`, and
        /// the procedure answers `WM_DESTROY` with `PostQuitMessage`, which is
        /// what ends the pump.
        pub fn stop(self) {
            // SAFETY: posting a documented message to a window this module
            // created and has not destroyed.
            #[allow(unsafe_code)]
            let posted = unsafe { PostMessageW(self.hwnd as _, WM_CLOSE, 0, 0) };
            if posted == 0 {
                warn!("the display watcher could not be woken to stop");
                clear_notify();
                return;
            }
            if self.thread.join().is_err() {
                warn!("the display watcher thread panicked");
            }
            clear_notify();
            debug!("display watcher stopped");
        }
    }

    /// The callback the window procedure reaches, which is a plain function
    /// pointer with nowhere to keep state of its own.
    ///
    /// A `Mutex` rather than a `OnceLock` so that stopping a watcher really does
    /// end it: the slot is emptied on the way out and a later `start` can fill
    /// it again. The lock is released before the callback runs, because what it
    /// does is send on a channel and nothing here may depend on how long that
    /// takes.
    static NOTIFY: OnceLock<Mutex<Option<NotifyFn>>> = OnceLock::new();

    fn notify_slot() -> &'static Mutex<Option<NotifyFn>> {
        NOTIFY.get_or_init(|| Mutex::new(None))
    }

    fn clear_notify() {
        if let Ok(mut slot) = notify_slot().lock() {
            *slot = None;
        }
    }

    /// Start watching, or say why not.
    pub fn start(notify: NotifyFn) -> Option<Watcher> {
        {
            let mut slot = notify_slot().lock().ok()?;
            if slot.is_some() {
                warn!("a display watcher is already running");
                return None;
            }
            *slot = Some(notify);
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("display-watch".to_owned())
            .spawn(move || {
                let hwnd = create_window();
                let _ = tx.send(hwnd);
                if hwnd != 0 {
                    pump();
                }
            });
        let Ok(thread) = thread else {
            clear_notify();
            return None;
        };

        let hwnd = rx.recv().unwrap_or(0);
        if hwnd == 0 {
            warn!("could not create the display watcher window");
            clear_notify();
            return None;
        }
        debug!("display watcher running");
        Some(Watcher { hwnd, thread })
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
        let title = wide("Sunlit Earth display watcher");

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
            debug!("the display watcher window class was already registered");
        }

        // SAFETY: both wide strings outlive the call, the parent and menu
        // handles are null because this is an unowned top-level window, and the
        // instance is this module's own. The window is never shown, and it is a
        // top-level one because a message-only window receives no broadcast.
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

    /// The message loop. `GetMessageW` blocks in the kernel until something
    /// arrives, which is what makes this thread free.
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

    /// The window procedure. Windows calls this on the watcher thread.
    #[allow(unsafe_code)]
    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_DISPLAYCHANGE => {
                let notify = notify_slot().lock().ok().and_then(|slot| slot.clone());
                if let Some(notify) = notify {
                    debug!("windows says the display configuration changed");
                    notify();
                }
                0
            }
            WM_DESTROY => {
                // SAFETY: ending this thread's own message loop, which is what
                // a destroyed window with nothing else on the thread wants.
                unsafe {
                    PostQuitMessage(0);
                }
                0
            }
            // SAFETY: forwarding a message the system sent, unchanged, to the
            // default handler, which is what a window procedure must do with
            // everything it does not handle itself. `WM_CLOSE` goes here too,
            // and that is what turns the stop into a `DestroyWindow`.
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// Starting and stopping is the whole of what a test can assert here: a
    /// layout change is not something a test process may make on somebody's
    /// desk, and everything after the hint is the engine's, where a mock clock
    /// decides. What it does prove is the connection, the extension check, the
    /// `select_input` and the wake, which is every call this half makes.
    #[test]
    fn a_watcher_starts_and_stops_on_a_session_with_a_display() {
        if crate::env_override("DISPLAY").is_none() {
            println!("skipping: DISPLAY is unset, so there is no X server to watch");
            return;
        }
        let hints = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hints);
        let Some(watcher) = super::start(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })) else {
            println!("skipping: this session's X server could not be watched");
            return;
        };

        // On another thread, so a wake that never arrives is a failed
        // assertion rather than a test that hangs until the harness gives up.
        let (done, stopped) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            watcher.stop();
            let _ = done.send(());
        });
        assert!(
            stopped.recv_timeout(Duration::from_secs(1)).is_ok(),
            "a watcher that cannot be woken leaves a thread blocked for the life \
             of the process"
        );
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

    /// The message the guest cannot produce, delivered the way Windows does.
    ///
    /// The Windows guest's video is a single fixed mode, so no automated case
    /// can make it send a real `WM_DISPLAYCHANGE`; `SendMessageW` runs the
    /// window procedure synchronously, which is the same path and needs no
    /// waiting. This is the `session_end` test's pattern for the same reason.
    #[test]
    fn the_display_message_reaches_the_callback_once_per_message() {
        const WM_DISPLAYCHANGE: u32 = 0x007E;

        let hints = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hints);
        let Some(watcher) = super::start(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })) else {
            // No window station, which is a property of the environment rather
            // than of this code.
            println!("skipping: could not create a display watcher window here");
            return;
        };
        let hwnd = watcher.hwnd();

        // SAFETY: a window this test installed, and a documented message.
        // `SendMessageW` returns once the window procedure has run.
        #[allow(unsafe_code)]
        unsafe {
            SendMessageW(hwnd as _, WM_DISPLAYCHANGE, 0, 0);
        }
        assert_eq!(hints.load(Ordering::SeqCst), 1);

        // A message the watcher does not answer must not become a hint.
        #[allow(unsafe_code)]
        unsafe {
            SendMessageW(hwnd as _, 0x001A, 0, 0);
        }
        assert_eq!(
            hints.load(Ordering::SeqCst),
            1,
            "WM_SETTINGCHANGE is not one"
        );

        let (done, stopped) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            watcher.stop();
            let _ = done.send(());
        });
        assert!(
            stopped.recv_timeout(Duration::from_secs(5)).is_ok(),
            "a watcher that cannot be woken leaves a thread blocked for the life \
             of the process"
        );
    }
}
