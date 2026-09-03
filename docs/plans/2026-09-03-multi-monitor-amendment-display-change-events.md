# Plan Amendment: Reacting to a Display Layout That Changed

Amends `2026-08-30-multi-monitor-plan.md`. Written 2026-09-03, after the feature merged, from two things seen on the machine this ships to. The layout diagram in the Displays group keeps the arrangement the window opened with, however the screens are rearranged afterwards. And undocking the notebook leaves the wallpaper that was rendered for the external screen on the internal one, at the wrong size, until the auto-refresh interval comes around or somebody presses the button. Both trace to two sentences in the plan: under "Non-uniform layouts", "There is no display-change subscription: `WM_DISPLAYCHANGE` and RandR events are both real and both a separate piece of work, and the auto-refresh timer means the layout is never stale for long"; and under "Not in this", "Display-change events. Re-querying on publish is enough while the auto-refresh runs."

Neither holds up. The auto-refresh is off by default and is minutes apart when it is on, so a stale layout is stale for exactly that long, on the screen a person is looking at. And the settings window was never covered by the timer at all: its monitor list is one query at startup, captured by value into three callbacks. This amendment is that separate piece of work. The constraint it was written under is the one the request named: react to changes as they happen, with nothing that wakes up to look.

## What changes, and what does not

The layout math, the modes, the anchor, the sink contract, the per-desktop reach table, the config fields, the `displays` subcommand and the IPC command are untouched. So is the rule that a publish re-queries the monitor list; this adds a reason for a publish to happen, not a cache in front of it.

What is added:

- A display watcher per platform, `sunlit_core::display::watch`, whose thread blocks on the platform's own notification and delivers a hint. It has no opinion about what a hint means.
- `EngineCommand::DisplaysChanged`, the hint, and `EngineEvent::MonitorsChanged(Vec<Monitor>)`, what the engine found when it looked.
- In the engine: hints are settled, then the list is re-queried once and compared; a list that differs is announced, and the wallpaper is re-rendered when the desk holds a picture this process made for a layout that is gone.
- In the app: the Displays group is rebuilt from the event, and the three callbacks that captured a monitor list share one that the event replaces.
- A `SIGNAL:displays_changed` line, so the e2e suite can wait for the reaction rather than sleep and hope.

Two statements in the plan are superseded rather than extended, and the plan carries a pointer here at both: the "A layout that changed" bullet under "Non-uniform layouts", and the "Display-change events" bullet under "Not in this".

## Facts read for this amendment

Each read on 2026-09-03 rather than remembered.

- **Nothing already in the process can be reused.** winit 0.30.13's X11 backend selects RandR input on the root (`select_xrandr_input`, `CRTC_CHANGE | OUTPUT_PROPERTY | SCREEN_CHANGE`) for its own monitor cache and surfaces no event for it; its Windows backend handles `WM_SETTINGCHANGE` for the theme and nothing for `WM_DISPLAYCHANGE`. Slint 1.17 has no monitor API of any kind. So the subscription has to be the program's own, and it has to live somewhere that exists whether or not a window does, which is the same conclusion `session_end.rs` reached for the shutdown messages.
- **A message-only window will not do on Windows.** Microsoft's own words: a message-only window "is not visible, has no z-order, cannot be enumerated, and does not receive broadcast messages", and `WM_DISPLAYCHANGE` is a broadcast to top-level windows. The established shape is an invisible top-level window with its own message pump. `session_end.rs` already builds exactly that for `WM_QUERYENDSESSION`, and GLFW's Win32 helper window is the same thing: `CreateWindowExW` with a null parent, hidden with `ShowWindow(SW_HIDE)`, and in its window procedure `case WM_DISPLAYCHANGE: _glfwPollMonitorsWin32();` is the one message that re-reads the monitors. GLFW's `WM_DEVICECHANGE` case is for joysticks.
- **x11rb is already in the tree.** `Cargo.lock` has x11rb 0.13.2 through winit, with `allow-unsafe-code`, `dl-libxcb`, `randr`, `resource_manager`, `xinput` and `xkb`. A `sunlit-core` dependency with `default-features = false, features = ["randr"]` (`randr` pulls `render`) is a subset of that, so a workspace build compiles nothing it did not compile before, and `cargo test -p sunlit-core` alone compiles the crate with two protocol features. `RustConnection` is the pure-Rust connection, needs no libxcb and no `unsafe` on our side, and is built for several threads on one connection: a thread waiting for an event releases the lock so another can send, which is what the stop path below relies on. The randr module has `randr_select_input`, and the event enum has `RandrScreenChangeNotify` and `RandrNotify`.
- **The Windows guest cannot change its own resolution.** `hyperv::video_script` sets `-ResolutionType Single`, chosen so the host decides the size, and its own comment names the cost: "changing the resolution from inside the guest". So there is no automated way to make Windows send a real `WM_DISPLAYCHANGE` in the guest. What there is, is the pattern `session_end`'s Windows test uses: `SendMessageW` to the listener's own window, which runs the window procedure synchronously.
- **The Linux guest keeps an xrandr change made over SSH.** The boot places the outputs with `xrandr --output <b> --auto --right-of <a>` and the session leaves them there (`docs/vm-setup.md`), so the e2e process can move the layout the same way.
- **The engine loop is the right place to wait.** It blocks on the command channel with a 50 ms timeout and asks an injected `Clock` what is due, so a deadline added to it costs no new wakeup and is testable with `MockClock`. Every background producer already wakes it through the same channel; the cloud worker's `notify` is the precedent for a closure that sends one command. `EngineCommand` is pinned to 64 bytes inline and a unit variant does not move it.
- **Where the UI's list comes from.** `init_ui` calls `display::monitors()` once; `register_action_callbacks` and `register_display_callbacks` each copy the list into their closures; the group hides itself on `display-tiles.length <= 1`, so redrawing the diagram is also what shows and hides it; and `defer_combobox_indices` already exists because a Slint combo needs its index set after its model changes.

## The mechanism

### The watcher

```rust
pub struct Watcher { /* thread and its wake handle */ }

/// Start watching. `notify` runs on the watcher's own thread once per hint the
/// platform gives, and may run for changes that turn out to be nothing.
/// `None` where this platform has no way to watch, and where the subscription
/// could not be made; both are logged once and the app carries on as before.
pub fn start(notify: Arc<dyn Fn() + Send + Sync>) -> Option<Watcher>;

impl Watcher {
    /// Wake the thread and join it. Microseconds: the wake is a message to a
    /// window, or an event to a connection, that this process owns.
    pub fn stop(self);
}
```

The watcher knows nothing about the engine. `main` hands it a closure that sends `EngineCommand::DisplaysChanged`, and that is the whole coupling. It is not a trait injected through `EngineConfig` the way the clock and the sink are, because the engine needs nothing from it but the hint, and a test produces the hint by sending the command. Only the app's ordinary run starts one: `render`, `displays` and the tests never do.

**Windows.** A thread named `display-watch` registers a class of its own (`SunlitEarthDisplayWatch`), creates an invisible `WS_OVERLAPPED` top-level window and pumps `GetMessageW`, which blocks in the kernel until something arrives. `WM_DISPLAYCHANGE` (`0x007E`, spelled as a local constant the way `session_end` spells its two) calls `notify`; everything else goes to `DefWindowProcW`. Stopping posts `WM_CLOSE`, which `DefWindowProcW` turns into `DestroyWindow`, and the procedure answers `WM_DESTROY` with `PostQuitMessage`, which ends the pump. Nothing on this thread enumerates or touches COM; the enumeration happens on the engine thread as it does today, behind `ensure_dpi_awareness`. `sunlit-core`'s `windows-sys` gains `Win32_System_LibraryLoader` for `GetModuleHandleW`, which `sunlit-app` already has for the same reason.

Not a second use of `session_end`'s window. The two are the same dozen lines of Win32 and nothing else: different messages, different consumers, different lifetimes, and the Linux halves have nothing in common at all, so a shared module would be shared on one platform only. Not Slint's window either, for the reason `session_end.rs` gives: it is winit's, it is hidden in tray mode, and the message would have to be caught in a procedure this program does not own.

`WM_DEVICECHANGE` and `WM_SETTINGCHANGE` are deliberately not taken. `EnumDisplayMonitors` lists the active monitors, and that set moves only through an applied display configuration change, which is what `WM_DISPLAYCHANGE` announces. A plugged-in monitor Windows has not activated is not in the list either way. If the hand check under Testing finds a change the message does not cover, adding a second one to the procedure is one line, and the rest of the path is built to take a redundant hint at the cost of one comparison.

**Linux.** A thread named `display-watch` opens `RustConnection::connect(None)`, which reads `DISPLAY` the way `xrandr` does; unset is the headless case and answers `None` without a log line, because `outputs()` already made that decision. It checks that the RandR extension is present, creates a 1x1 unmapped `InputOnly` window under the root to have something to be woken through, selects `SCREEN_CHANGE | CRTC_CHANGE | OUTPUT_CHANGE` on the root with `randr_select_input`, and loops on `wait_for_event`, which blocks in `poll` on the socket. `RandrScreenChangeNotify` and `RandrNotify` call `notify`; a `ClientMessage` on the window it created means stop; a connection error is logged once and ends the thread, and the app carries on without a watcher, which is what it had before this amendment. `OUTPUT_PROPERTY` is left out: it fires for EDID and backlight properties that move nothing.

Stopping sends a `ClientMessageEvent` to that window with `send_event(propagate = false, destination = window, event_mask = NO_EVENT)` and flushes. With an empty event mask the server delivers to the client that created the destination window, which is this connection, and the waiting thread returns. That is the one place the connection is used from two threads at once, and it is the case `RustConnection` is designed for.

Under a Wayland session the connection is to XWayland, exactly as the query is, and XWayland implements RandR over the compositor's outputs. It is expected to fire for the same changes; it is unverified until a Wayland session is something the suite runs in, and the roadmap already carries that session for the query's sake. A Wayland session with no XWayland has no `DISPLAY`, no watcher, and no query, all for the same reason.

RandR rather than the alternatives, each considered: D-Bus signals exist per desktop (KScreen, Mutter's `DisplayConfig`) and there is no common one; udev hotplug events see a connector change and not a layout change made in a settings dialog, and see nothing through XWayland; polling `xrandr` is the busy loop the request ruled out.

**macOS.** Nothing. `CGDisplayRegisterReconfigurationCallback` is the API, and it belongs to the setter when there is one.

### What the engine does with a hint

- `DisplaysChanged` sets `display_recheck = Some(now + DISPLAY_SETTLE)`, and a hint that arrives while one is pending pushes the deadline out again. Trailing rather than leading, because a change is a burst on both platforms: RandR sends one event per CRTC and one per output, and Windows sends one `WM_DISPLAYCHANGE` per applied step, with a docking station bringing its screens up one at a time as each link trains.
- `DISPLAY_SETTLE` is 2 seconds. Nothing about a wallpaper is latency-sensitive, and the cost of settling too early is a second full render and a second visible swap when a late step arrives. It is checked on the tick the loop already makes, so it adds no timer and no wakeup, and it is read from the injected clock, so a test advances a `MockClock` past it rather than sleeping.
- When it is due, `recheck_displays` asks `self.wallpaper.monitors()` once and compares against `known_monitors: Option<Vec<Monitor>>`, the last list the engine saw from any source. Equal is a debug line and nothing else. Different, or no baseline yet, stores the list and emits `MonitorsChanged` with it; and if `published` is set, calls `publish_wallpaper`. `build_wallpaper_job` stores the list it plans with in `known_monitors` too, and `published` is set when the sink answered `Ok`, never on a refusal.
- The rule in one sentence: the desk holds a picture this process made for a layout that is gone, so make one for the layout that is here. Its two edges are deliberate. Somebody who opened the settings window to look and never asked for a wallpaper does not get one because they moved a screen. And a sink that refuses (`check_supported` failing on a Linux desktop the table does not know) is never asked unprompted, so a layout change cannot produce an error in the status line out of nowhere.
- It converges. A hint that lands during a publish waits its turn in the channel, the settle runs, and the comparison is against the list that publish planned with, so a layout that kept moving is published again only if it is different again.
- A vanished anchor is already handled: `resolve_anchor` falls back to the primary and the note reaches the status line; the combo shows the automatic row and `anchor_to_store` keeps the stored id for when the screen comes back. Nothing new is needed there.
- The hints this design pays for and then discards, on purpose: resume from sleep, a scaling change, a color depth change. Each is one enumeration, microseconds on Windows and one short-lived `xrandr` process on Linux, followed by an equal comparison.

### The UI

- `MonitorsChanged` reaches the forwarder in `engine_client.rs`, which does what it does for every other event: `invoke_from_event_loop`, then `displays::replace_monitors(window, &screens, monitors, &stored_anchor)`. That rebuilds the two combo models, puts the screen combo back on the row `anchor_index` finds for the stored id (read from the config, which is written the moment the plan changes and is therefore the plan the engine holds), and redraws the diagram, which is also what hides the group when one screen is left and shows it when a second returns. The combo index goes through `defer_combobox_indices`, for the reason that function exists.
- The three callbacks stop owning a `Vec<Monitor>` each and share an `Rc<RefCell<Vec<Monitor>>>` with the event handler. UI thread only, so no lock.
- The forwarder also prints `SIGNAL:displays_changed monitors=<n>`, in the voice of `wallpaper_set`. The IPC `displays` command is unchanged; it queries live and always did.
- Nothing about the preview window changes. It is still not a wallpaper preview.

## Testing

### Needs no display

- **Engine**, in `tests/engine.rs`, on a `MockClock`. `RecordingSink` gains `set_monitors` and a counter of how often `monitors()` was asked. The cases: an unchanged list after a hint is one query, no event and no publish. Five hints inside the settle window are one query. A hint at 1.5 s pushes a deadline that was due at 2 s, so nothing happens at 2.5 s and something does at 3.5 s. A changed list after a publish produces `MonitorsChanged` carrying the new list and then a publish whose images are the new sizes. A changed list with no publish before it produces the event and no publish. A changed list after a publish the sink refused produces the event and no publish. `command_payload_is_small` and the digest table are unaffected and stay that way.
- **Watcher**, in `display/watch.rs`. On Windows, the `session_end` pattern: start with a counting closure, `SendMessageW(hwnd, WM_DISPLAYCHANGE, ..)`, assert one hint, stop and see the thread join; skip with a printed reason where no window can be created. On Linux, with `DISPLAY` set, start and stop and assert the thread joined within a second, which proves the connection, the extension check, the `select_input` and the wake; skip with the reason where `DISPLAY` is unset, which is CI.
- **UI**, in `tests/slint_ui.rs`. `replace_monitors` with two screens, then one, then the same two: the combo rows, the anchor row following the stored id back when its screen returns, the tile count, and the group hidden at one.

### The two-head guest

`test_a_layout_change_republishes_the_wallpaper`, gated like the other wallpaper cases and additionally on `display::outputs()` reporting at least two connected outputs, else skipped with the reason. Set a wallpaper over IPC and keep the file list. Run `xrandr --output <second> --off` from the test process. Wait for `SIGNAL:displays_changed monitors=1`, then for `SIGNAL:wallpaper_set`, on the two-minute budget the publish cases already allow a software adapter. Assert the new files match the layout the session now has (`assert_the_files_match_the_layout` asks the live query) and that every path changed. Restore with `--auto --right-of <first>` and wait for `monitors=2` and the publish that follows. The restore runs from a guard so a failing assertion cannot leave the guest one-screened for the cases after it.

What the case proves is the whole path against a real X server: that a RandR change wakes the watcher, that the settle collapses the burst, that the engine sees the new list, and that the desktop was handed images for it.

### What only Windows can answer

The guest's video is a single fixed mode, so no automated case can make Windows send a real `WM_DISPLAYCHANGE` there; booting it with `-ResolutionType Maximum` for one case would allow it and is not worth a second boot profile for one message. The unit test proves the message reaches the hint. The rest is the hand check on the machine the report came from, which is the scenario itself: with the window open, undock, and within about two seconds the diagram shows one screen and the wallpaper is rendered for the internal panel; redock, and both return. Then, with the anchor on automatic, change the main display in Windows' own settings and see the anchor follow. Read the log afterwards for how many hints the dock produced and how far apart they were, which is what open question 7 needs.

## Implementation steps

1. **Engine.** `EngineCommand::DisplaysChanged`, `EngineEvent::MonitorsChanged`, `DISPLAY_SETTLE`, `display_recheck`, `known_monitors`, `published`, `recheck_displays`. Files: `crates/sunlit-core/src/engine/mod.rs`, `crates/sunlit-core/tests/engine.rs`. Verify: `cargo test -p sunlit-core --test engine`.
2. **Watcher.** `crates/sunlit-core/src/display/watch.rs`, `x11rb` in the workspace table and under `cfg(target_os = "linux")` in `sunlit-core`, `Win32_System_LibraryLoader` on its `windows-sys`. Verify: `cargo unit`, `cargo clippy --all-targets`. The Windows test runs with `cargo unit` on a Windows machine; the guest jobs are handed the e2e binary and run nothing else, so they do not cover it.
3. **App.** `displays::replace_monitors`, the shared list, the forwarder arm and its signal, `main` starting the watcher after the engine and stopping it after `engine.shutdown()`. Files: `crates/sunlit-app/src/displays.rs`, `engine_client.rs`, `ui_callbacks.rs`, `main.rs`, `tests/slint_ui.rs`. Verify: `cargo test -p sunlit-earth`, then `cargo run` and move a screen.
4. **The guest case.** `crates/sunlit-app/tests/e2e.rs`. Verify: `cargo xtask e2e --target linux --screens 2`.
5. **Docs.** `docs/architecture.md` gains the command, the event and the rule under "The engine" and "Wallpaper export"; `docs/platforms.md` gains a paragraph under "One wallpaper per screen" on how each platform is watched and the XWayland caveat; `docs/testing.md` says what the guest case proves and what only the hand check does; `docs/roadmap.md` closes the item this amendment opened and lists the Windows hand check beside the four Windows questions; `README.md` is unchanged, since there is no new flag and no new variable.

## Not in this

- **Enumerating through the same connection.** `randr_get_monitors` on the watcher's connection could answer the list and retire the `xrandr` process. A natural follow-up; not here, because nothing in this needs it and the parser is the part with the tests.
- **Native Wayland.** `wl_output` or the per-desktop D-Bus queries. Same roadmap item as the query, same reason.
- **Debouncing in the watcher thread.** It would need a timed wait per platform and would not be testable on a mock clock.
- **A switch to turn the watcher off.** No case for one yet; a watcher that could not start already leaves the app exactly as it was.

## Departures

Continuing the plan's numbering, each with what the code said that the amendment did not.

12. **The UI's shared monitor list is an `Arc<Mutex<Vec<Monitor>>>` and not an `Rc<RefCell<Vec<Monitor>>>`.** The amendment has the three callbacks and the event handler sharing an `Rc` on the grounds that all four run on the UI thread. Three of them do; the fourth does not exist. `MonitorsChanged` reaches the forwarder on the engine thread, and what the forwarder hands to `invoke_from_event_loop` is a `Box<dyn FnOnce() + Send>`, so a closure that captures an `Rc` does not compile. The alternatives were a thread-local holding the list on the UI thread, which is global mutable state that every `slint_ui` case would then share, or leaving the list out of the closure entirely, which leaves the callbacks with no way to reach the new one. A `Mutex` says what is actually true, which is that the list crosses a thread, and it is uncontended: every read is on the UI thread and the one write is the hand-off.

13. **The Linux watcher calls `randr_query_version` before selecting input.** The amendment lists the steps as connect, check the extension, create the window, `randr_select_input`, loop. The X server tracks a RandR version per client and assumes 1.0 for a client that never declared one, and `RRNotify` (the CRTC and output events) was added in 1.2: without the declaration the subscription is accepted and only `RRScreenChangeNotify` is ever delivered. So the call is not optional bookkeeping, it is what makes two of the three selected masks mean anything. `xrandr` itself does the same thing first.

14. **The guest case falls back to a mode change where the session has one screen.** The amendment has one shape for it, `xrandr --output <second> --off` in a two-head guest, because `--screens 2` is a flag the xtask already takes. On the host this was implemented on it is a flag QEMU refuses: `virtio-vga` grew its `outputs` list property in QEMU 9.0 and this host has 8.2.2, so the guest exits at `-device` and the two-head run cannot boot at all. That is a host fact rather than anything about this change, and it left the entire Linux half of the feature with no end-to-end coverage. So the case asks the session what change it can make: two outputs and it switches the non-primary one off, one output and it switches that output to another of its modes. Both are a real RandR event from a real display server, both move the rectangle the engine compares, and both end in the desktop being handed images for the layout it has now; what only the two-head form proves is the screen *count* moving, and that is what the case still does first wherever there are two screens. It also checks that the change actually took before waiting on anything, and skips with the reason where the session put its own layout straight back, which is open question 8 answered defensively rather than assumed.

## Open questions (continuing the plan's numbering)

6. Does `WM_DISPLAYCHANGE` arrive for a main-display change that moves no resolution? Believed yes, since the primary's move re-origins the virtual desktop and goes through the same applied-configuration path; the hand check decides. If not, `WM_SETTINGCHANGE` with `SPI_SETWORKAREA` is the one-line addition, because the work area follows the primary.
7. Is 2 seconds enough for a dock on the shipping machine, or does a late link produce a second publish? The log records every hint and every recheck; tune the constant from it rather than from a guess.
8. Does the two-head guest's session keep an `xrandr --output --off` made by the test process, or reapply its saved layout? Either way the layout moves and the case sees a change; what the answer decides is whether the restore step does anything.
9. Does XWayland's RandR fire for a layout change made in a Wayland compositor's settings? Expected; unobserved until a Wayland session runs the suite.
10. Should a temporary display mode count as a change? A game that takes the desktop to another resolution with `CDS_FULLSCREEN` broadcasts `WM_DISPLAYCHANGE`, and the monitor list really does differ, so this design renders a wallpaper at the game's resolution while the game loads and again when it exits. A temporary mode is not written to the registry, so comparing the current mode against `ENUM_REGISTRY_SETTINGS` would tell it from a real change in a few lines. Not decided, and not in the first implementation: see how often it happens on the shipping machine first.
11. DisplayPort monitors that disconnect when powered off are a topology change to Windows, so every power cycle costs two publishes. Consistent, since the wallpaper follows what Windows thinks the layout is, and the alternative is a stale picture if the monitor does not come back. Whether that is acceptable on the hardware this ships to is a thing to observe rather than decide here.
