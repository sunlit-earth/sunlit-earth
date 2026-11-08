# Full screen

## Problem

The preview is the reason the window exists, and it shares the window with the menu bar, the sidebar and the splitter. There is no way to look at the globe, or watch an excursion run, on the whole screen. The roadmap has carried the entry since the menu bar plan: Enter Full Screen in View, F11 on Windows and Linux and ⌃⌘F on macOS.

Full screen has to be easy to leave for someone who entered it by accident, and stay out of the way for someone who uses it every day.

## What the app has today

- `MainWindow` (`crates/sunlit-ui/ui/main.slint`) lays out the `MenuBar`, the sidebar `panel`, the `splitter`, and `image-container` to the right of the splitter. Everything drawn over the preview is a child of `image-container`: the frame guides, the camera panel, the `show-button` that brings hidden controls back, the time bar, the hover hints, the shortcut sheet, the version notice, the gesture tip and the loading label.
- `overlays-hidden` hides the controls. H on the preview and the camera panel's eye button set it, and the View menu's Controls check is its inverse (`controls-shown`). While it is set, `show-button` (the crossed eye) stays in the top right corner, and the frame guides hide too (`guides-visible`).
- The camera panel's footer is one row of three `OverlayButton`s, 24 by 24 with 6 px between them, in a panel 92 px wide: frame guides (`icons/frame.svg`, four outward corner brackets, lit while `guides-shown`), the shortcut sheet (`help.svg`) and hide (`eye.svg`).
- The camera panel and the time bar each have their own idle look already, which is a fade to a lower opacity, not hiding: the panel after 900 ms without hover or use (`idle`, `OverlayStyle.idle-opacity`), the time bar while the pointer is outside the preview and it is not pinned (opacity 0.42). Both animate over `OverlayStyle.idle-fade`, 400 ms.
- `time-bar-pinned` keeps the time bar open. The View menu has it as Pin Time Bar.
- The preview's keys live in the `preview-keys` FocusScope. Escape is a `KeyBinding` there that closes the shortcut sheet and does nothing else. F toggles the frame guides, H the controls.
- The gesture tip (`GestureTip`) appears at the top center of the preview, 14 px from the top, for 2.8 s after a camera panel control is used whose gesture the user has not tried.
- The window's geometry is saved by `menu::save_geometry` (`crates/sunlit-app/src/menu.rs`), called by Quit, the close handler, the tray's hide and IPC `hide-window`. Every show and hide goes through `window_state.rs`.
- The preview renders at `image-container`'s physical size, capped in width by the quality tier, and sends a new size once the window has settled.

## What Slint 1.18.1 and winit 0.30.13 do

Read from the cargo registry. Nothing here has been observed on a Mac.

- **The property.** `Window` has `in-out property <bool> full-screen` (`i-slint-compiler-1.18.1/builtin_elements.rs`). The winit backend turns it into `set_fullscreen(Some(Fullscreen::Borderless(None)))`, the monitor the window is on, and `None` to leave (`winitwindowadapter.rs`, the property sync).
- **The other direction.** When winit reports a resize, the adapter compares `winit_window.fullscreen()` with the Slint window's state and calls `set_fullscreen` to match. So full screen entered or left by the system, macOS's green title bar button above all, reaches `full-screen` without the app doing anything. Whether `Window::set_fullscreen` from that path updates the `full-screen` property a `.slint` binding reads is to be confirmed in step 1.
- **macOS.** winit enters `Borderless` through AppKit's `toggleFullScreen:`, the native full screen with its own Space and animation, and records `Borderless(current_monitor)` when the system enters it on its own (`platform_impl/macos/window_delegate.rs`). The global menu bar slides down when the pointer reaches the top of the screen.
- **The menu bar.** `MenuBar` takes a `visible` binding, passed to the native menu on Windows and macOS and to the drawn one on Linux (`passes/lower_menus.rs`). A `MenuBar` cannot sit in a conditional or repeated element. The Windows backend makes the window opaque when it has a native menu, citing the menu bar appearing translucent in full screen (Slint #8793).
- **Shortcuts on macOS.** Slint swaps the modifiers on Apple platforms: `Control` is ⌘ and `Meta` is ⌃ (`i-slint-core-1.18.1/input.rs`). `@keys(Meta + Control + F)` is therefore ⌃⌘F there.

## AppKit's own Enter Full Screen item

AppKit adds a full screen entry to an app's menus by itself.

- **Apple's statement.** The AppKit release notes for OS X 10.11, as quoted in Mozilla's bug 1408118: "AppKit automatically creates an "Enter Full Screen" menu item after the application finishes launching if an equivalent menu item isn't found." An app that does not want it sets the `NSFullScreenMenuItemEverywhere` user default to NO before `NSApplicationDidFinishLaunchingNotification`.
- **Where and what counts as equivalent.** Apple's note says neither. A Xojo engineer, in 2016: the OS "scans the View menu for an item with its action set to something like toggleFullscreen: and inserts it if absent". One Xojo user found that a top-level menu titled "VIEW " with a trailing space gets no item, which suggests the menu is found by its title. Both are testimony, not documentation.
- **It comes back.** In Firefox the item was inserted again after switching to another app and back, giving two entries, although Firefox had its own; setting the default fixed both cases. Electron set the same default for years and in February 2026 replaced it with removing hidden injected `toggleFullScreenMode:` items when a menu opens, because macOS injects one when a submenu has a delegate.
- **This app.** `muda` 0.19.3 gives every item Slint builds the action `fireMenuItemAction:` and sets a delegate on every `NSMenu`. Neither Slint, `muda` nor winit sets `NSFullScreenMenuItemEverywhere`. So AppKit cannot recognize any entry of ours as the equivalent, and the macOS build very likely has AppKit's entry in View already today, unasked; `docs/macos-testing.md` asks a tester to look.
- **What AppKit's entry does.** Its action is `toggleFullScreen:`, sent to the key window, which is the native full screen winit uses, and AppKit flips its title between Enter and Exit Full Screen itself. winit records the change and Slint copies it into `full-screen`, so the app would follow it like the green button.
- **Turning it off** takes setting the user default from Rust before the event loop finishes launching, which is new FFI through `objc2-foundation`, for a duplicate that adding nothing of our own avoids.

## Platform conventions

- **Windows and Linux:** F11 toggles full screen in Chrome, Firefox, Edge, Explorer and the GNOME apps that have it.
- **macOS:** Apple lists Control-Command-F as "use or stop using the app in full screen, if supported by the app", and Chrome uses it. Native apps also answer Fn-F (Globe-F) through the system, which this app gets for free if AppKit treats its window as a normal full screen window; no Apple document confirms it, so it is not promised. F11 on a Mac is Show Desktop.

## Decisions

Settled with the user on 2026-11-08.

### 1. What full screen shows

The preview on the whole screen of the monitor the window is on, with the controls drawn over it as they are now: camera panel, time bar, frame guides when on, hints, tips, the shortcut sheet and the version notice. The sidebar, the splitter and the window's frame go. The menu bar goes on Windows and Linux; on macOS it is the system's and the system hides it.

Full screen is session state like the frame guides: never saved, never restored at start, and the app always opens windowed.

### 2. Entering

- **Keys:** F11 on Windows and Linux, ⌃⌘F on macOS. Both toggle, so the same key leaves.
- **Menu on Windows and Linux:** Enter Full Screen as the last entry of View, after a separator, with F11. The menu bar is gone in full screen, so the entry is a plain item rather than a check that could never be seen checked.
- **Menu on macOS:** no entry of ours. AppKit's own Enter Full Screen in View does the job, flips its own title, and cannot end up twice, since AppKit only adds it when there is no equivalent and ours could never count as one. ⌃⌘F is a `KeyBinding` in `preview-keys`, in case AppKit's entry does not carry it. If the Mac check finds no AppKit entry, ours is added on macOS as a conditional `MenuItem` with ⌃⌘F, and that goes in Departures.
- **Button:** a fourth button in the camera panel's footer (decision 4). In full screen it turns into the exit button: inward corners, drawn in the accent color the way the guides button is lit.
- **macOS green button:** works as the system makes it work, and the app follows through the sync above.

### 3. Leaving

- The key that entered: F11, or ⌃⌘F on macOS.
- **Escape** closes the topmost transient thing first: an open rate menu, the date or time picker, the shortcut sheet, the version notice. Only with none of them open does it leave full screen. Escape leaves full screen on macOS too; the convention there is ⌃⌘F, but Escape costs nothing and is what a lost user tries first.
- The footer button.
- The exit pill (decision 6).

### 4. The camera panel footer becomes a 2 by 2 grid

At most three buttons to a row. Four buttons as two rows of two, centered, keeping the 24 px buttons and 6 px gaps:

```
[guides] [help      ]
[hide  ] [full screen]
```

The top row holds the aids, the bottom row changes what is on screen. Full screen takes the bottom right corner, where video players keep it.

The four outward corner brackets of `frame.svg` are what nearly every app uses for "enter full screen", so they move to the full screen button (`fullscreen.svg`, with `fullscreen-exit.svg` for the inward corners). The frame guides get a new icon that is not a rectangle either, since the guides themselves are rectangles and nested ones would sit too close to the brackets: two dashed guide lines crossing at a dot, chosen from six candidates (a thirds grid, a reticle, a set square, rulers with a guide, these guide lines, a ruler).

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 14 14">
  <path d="M9 1.5v1.3M9 4.6v1.3M9 8.1v1.3M9 11.2v1.3M1.5 5h1.3M4.6 5h1.3M8.1 5h1.3M11.2 5h1.3" fill="none" stroke="#000" stroke-width="1.3" stroke-linecap="round"/>
  <circle cx="9" cy="5" r="1.3" fill="#000"/>
</svg>
```

The hints name both: "Frame guides" with F, "Full screen" with F11 or ⌃⌘F, and "Exit full screen" with Esc and the platform key. F stays on the frame guides; YouTube's F for full screen is not worth moving a key people already use.

### 5. Controls hide when idle, in full screen only

- **Two flags.** A new `idle-hidden` beside `overlays-hidden`; the controls show when neither is set. H keeps its meaning, "hide until I say otherwise", in both modes. `idle-hidden` is only ever set in full screen and is cleared on leaving.
- **Delay:** 3 s without activity. Fading out takes `OverlayStyle.idle-fade`; coming back is immediate.
- **Activity:** pointer movement over the window, a mouse button, the wheel, any key.
- **What hides:** everything decision 1 lists as drawn over the preview except the shortcut sheet, the version notice and the loading label, plus the mouse cursor. The frame guides hide with the controls, as they do with H. In full screen `show-button` hides when idle as well, so a user who pressed H gets a clean picture too.
- **What keeps them up:** the pointer over any control; a control or a drag held; a rate menu or picker open; the shortcut sheet or the version notice open. The exit pill's long form stays for its 10 s whatever else hides (decision 6).
- **The time bar pinned:** nothing hides. Each control keeps the idle look it has in the window: the camera panel fades to `OverlayStyle.idle-opacity` as it does now, the pinned bar stays as it is, and the short exit pill fades like the camera panel. The cursor still hides, since it is not a control.
- **Detecting activity in Slint.** Movement over a control already keeps the controls up, so only movement over the bare preview has to be seen. The preview's `viewport-touch` reports it through `mouse-x` and `mouse-y` changes and `pointer-event`, the wheel through `scroll-event`, and keys arrive at `preview-keys`, which has focus in full screen because nothing else is left to take it. If that leaves a gap (a key while a popup has focus, a move the touch area does not report), the fallback is the winit backend's `on_winit_window_event` hook from the Rust side, which sees every event before Slint does and would set the same activity timestamp.

### 6. The exit pill

A pill at the top center of the preview, shown only in full screen.

```
first 10 s after entering:   ( ⤡  Exit full screen   [Esc] )
afterwards:                  ( ⤡  Exit )
```

- The whole pill is a button that leaves full screen, in both forms. "Exit" rather than a key name, because the people the pill is for click.
- **The long form** shows for 10 s after entering, visible the whole time even when the other controls have hidden at 3 s. It names the key so the user learns it.
- **Every entry starts long** (departure 1): F11, ⌃⌘F, the menu, the button and the green button alike.
- **The short form** belongs to the controls: it shows and hides with them, so after 3 s of idle the picture is clean, and moving the mouse brings it back. With the time bar pinned it fades rather than hides, like the camera panel (decision 5).
- With H, it hides with everything else; Escape and the key still work, and `show-button` brings it back.
- Hovering it shows the hint "Exit full screen" with Esc and the platform key, and keeps the controls up like any control.
- It looks like the folded time bar: 30 px tall, `OverlayStyle.panel`, the border, radius 15, 14 px from the top.
- **The gesture tip** shares the top center. While both show, the tip moves below the pill.

Why the top center: it is where the lost window frame was, the place people look first, and the top right where an × would go is taken by the camera panel.

### 7. Window state around full screen

- `menu::save_geometry` does nothing while the window is in full screen, so the monitor's size is never saved as the window's.
- Leaving full screen restores the size, position and maximized state from before. That is expected of winit and the window managers but has not been read in their code; step 9 checks it on every platform, and if one does not, the app remembers the rectangle itself on entering.
- The tray's hide, IPC `hide-window`, Close Window and the close button leave full screen first, then save the geometry and hide. The window shows again windowed.
- Quit while in full screen saves nothing new, since the geometry from before entering is what was saved last.

### 8. The switch goes through black

Added with the user on 2026-11-08 after trying the first build on Windows. Entering showed three things in turn: the windowed frame, sidebar included, stretched to the monitor by the system until the app drew at the new size (long enough to see in a debug build, a flicker in a release build); then the controls in place over the last windowed preview scaled up with `cover`; then, once the engine had rendered at the new size, the full picture, with a jump in framing unless the windowed preview had the monitor's aspect. The system's stretch cannot be stopped from the app, but what it stretches can be chosen.

- **Before the switch,** a black cover is drawn over the whole window, and the switch happens only once that frame is on screen. The system then stretches black. The cover carries nothing else in that frame, since anything drawn on it would be stretched with it.
- **The new size goes to the engine at once.** A full screen change is one jump, not a drag, so it skips the two-poll wait of `ViewportSettle`.
- **On entering, the first full screen frame is black with a notice:** centered, in the overlays' text colors, a line saying the view is being rendered with an activity indicator, and under it that Esc leaves full screen (Esc or ⌃⌘F on macOS). The controls are drawn over it as usual, the exit pill in its long form included. This is for the case where rendering takes long or never finishes (a software adapter on a very large screen, a stuck engine): the user always sees what is happening and how to get out, and Esc, the key and the pill work throughout.
- **The cover stays until a frame for the new size arrives,** then fades out over about 150 to 200 ms. There is no fallback to the old picture: the notice is what shows while waiting. "For the new size" means for the size last sent to the engine, not the window's, since the quality tier can cap the width.
- **On leaving,** the same black frame covers the switch. Once windowed, the sidebar and menu draw at once, and the preview shows the last full screen frame scaled by the short edge, centered, so the Earth stays where it was, until the windowed frame replaces it. No notice: the window is back and nothing is lost by waiting.
- **The idle clock** starts when the cover has gone, so the 3 s and the long pill's 10 s count from when the picture is visible.
- **An entry by the system** (macOS's green button and AppKit's entry) cannot be covered beforehand, since macOS runs its own animation. The cover and notice start when `full-screen` turns true and behave as above from there.

### Not chosen

- **An exit tab that appears only when the pointer touches the top edge.** Invisible to experts, but newcomers would not find it either.
- **A × in the top right corner.** The camera panel lives there.
- **Moving the frame guides off F so F can be full screen.** Changes a key people use for a convention this app does not otherwise follow.
- **Idle hiding in the window too.** Not asked for, and the controls already fade there.
- **Our own macOS entry with AppKit's turned off** through `NSFullScreenMenuItemEverywhere`. New FFI, and while Firefox fixed its duplicates with it, Electron replaced it in 2026 with a cleanup of its own, so it is not clear that it covers every case.

## Implementation steps

1. **Confirm the plumbing.** A throwaway check on Windows that setting `full-screen` from `.slint` enters borderless full screen, that F11 pressed in full screen with the menu bar's `visible` false still reaches a `KeyBinding` in `preview-keys`, and that the native menu goes. Whether a `MenuItem`'s `activated` can tell its shortcut from a click: it cannot, and Slint runs menu shortcuts before any `KeyBinding` (departure 1), so the long form always shows.
2. **Icons.** Add `fullscreen.svg` (today's `frame.svg`), `fullscreen-exit.svg` and `guides.svg` (decision 4); remove `frame.svg` if nothing else uses it.
3. **Layout.** In `main.slint`: `MenuBar { visible: !root.full-screen || Platform.os == OperatingSystemType.macos; }`, the panel and the splitter hidden in full screen, and `image-container` at x 0 and the full width. Callbacks `toggle-full-screen()` and `leave-full-screen()` on `MainWindow`.
4. **Menu and keys.** The View entry of decision 2 on Windows and Linux, conditional on the platform the way Quit is. `KeyBinding`s for F11, ⌃⌘F on macOS, and the Escape order of decision 3 in `preview-keys`.
5. **Footer.** The 2 by 2 grid, the full screen button and its `hovered-hint`, `hovered-y` per row, and the new hint texts in `OverlayHints`.
6. **Idle hiding.** `idle-hidden`, the activity timestamp, the timer, the blockers, the cursor, and `visible` on every overlay reading the combined flag.
7. **The exit pill.** A component in `overlays/` beside `time-bar.slint`, its two forms and 10 s timer, its hint, and the gesture tip's new position.
8. **Rust.** `save_geometry` skips in full screen; `window_state::hide` and the close handler leave full screen first.
9. **Checks on the platforms.**
   - Windows desktop, `cargo app`: all three ways in and all four ways out; the menu bar is gone and comes back; the window returns to its size, and to maximized when it was; the cursor hides and returns; the pill's long form for entries by button and menu only; the tray's hide in full screen and the show after it. The user checked on 2026-11-08 that the cursor hides and returns, that the tray's hide works in full screen, and that closing the app in full screen works. Whether the native menu bar goes and whether a maximized window comes back maximized are still open.
   - Linux guest, KDE and GNOME on X11 and on Wayland, sway and i3: the same list, with the drawn menu bar.
   - Two screens in the Linux guest (`--screens 2`): full screen fills the screen the window is on.
   - macOS runner, as far as it can be driven, and `docs/macos-testing.md` for a person with a Mac: whether View has AppKit's Enter Full Screen, once only, and still after switching to another app and back and after pressing H, which rebuilds the menu; whether it shows ⌃⌘F, and that ⌃⌘F toggles once rather than twice; that the green button and AppKit's entry both hide the sidebar and the pill comes up long; that Escape leaves.
10. **The switch through black (decision 8).** How to know the black frame is on screen before switching is decided in the step: a rendering notifier (`Window::set_rendering_notifier`) where the renderer supports one, otherwise a timer of a couple of frames; the software renderer and the e2e guests must work too. The immediate size send, the cover with the notice, the match of the arriving frame against the size sent, the fade, the short-edge scaling of the old frame on leaving, and the idle clock starting when the cover goes. Check on the Windows desktop in a debug and a release build that the stretched sidebar no longer shows, and in a guest with the software adapter that the notice shows while the first frame renders.

## Tests

### `slint_ui`

- Entering hides the panel and the splitter and gives `image-container` the window's width; leaving restores both.
- The controls hide after 3 s of `mock_elapsed_time` in full screen, never in the window, and come back on a pointer move and on a key.
- Each blocker of decision 5 keeps them up. With the time bar pinned nothing hides after 3 s, and the camera panel and the short pill are at their idle opacity.
- The long pill is still visible at 9 s with the controls hidden, and gone at 10 s.
- Escape's order: with the shortcut sheet open, the first Escape closes it and the window stays in full screen; the second leaves.
- The pill: long for 10 s after every entry, F11 and the button alike, short after, and a click on either form leaves.
- H in full screen hides everything, idle never brings it back, and `show-button` idles out and returns on a move.
- The switch (decision 8): a toggle shows the cover before `full-screen` changes; on entering, the notice shows until a frame of the size last sent arrives and the cover then fades, and a frame of another size does not end it; Escape leaves while the notice shows; on leaving, no notice, and the old frame is scaled by the short edge until the new one arrives; the idle clock starts when the cover has gone. A unit test that a full screen change sends the new size without waiting for a second poll.

The testing backend has no real window, so these drive `full-screen` as a property; whether the platform honored it is step 9's.

### e2e

A case that sends F11 to the window and checks that its rectangle matches its monitor's, then Escape and that the rectangle from before is back. The suite has no query for a window's rectangle today, so the case needs one: `GetWindowRect` on Windows and `xwininfo` or `xdotool` on X11, or an IPC command that reports what the app's own window says. Which of these is cheapest is decided in the step. It skips on Wayland and wherever the rectangle cannot be read, and says why.

## Docs

- `docs/roadmap.md`: the full screen entry, ticked when done, pointing here.
- `docs/architecture.md`: full screen as UI state outside `SceneParams` and `AppConfig`, and `idle-hidden` beside `overlays-hidden`.
- `docs/platforms.md`: the keys per platform, the macOS menu bar, and what step 9 found.
- `docs/macos-testing.md`: the questions of step 9's macOS item.
- The shortcut sheet gains F11 (⌃⌘F) and Esc.

## Gates

`cargo fmt --check`, `cargo test`, `cargo clippy --all-targets`, and `cargo xtask e2e --target linux` and `--target windows`.

## Departures

1. **The pill always starts long; by-key detection is impossible.** Step 1 read Slint 1.18.1 rather than running a throwaway check: `WindowInner::process_key_input` runs the menu bar's shortcuts before any focus scope sees a key, on every platform and with the bar's `visible` false too (`process_menubar_shortcuts`; `setup_menubar_shortcuts` filters on `enabled` alone), and on Windows nothing calls `TranslateAcceleratorW`, so the native accelerator never fires on its own. A `KeyBinding` for F11 in `preview-keys` would never run, and F11 is indistinguishable from a click on the View menu's entry. The one way around it was the winit backend's `on_winit_window_event` hook behind slint's `unstable-winit-030` feature; the user chose always-long over a winit hook. So decision 6's "entered by key, no long form" is dropped: the pill starts in its long form for 10 s however full screen was entered (F11, ⌃⌘F, the menu, the footer button, the green button, AppKit's entry), and there is no `entered-by-key` flag. The same reading confirmed the other half of step 1: the backend's resize handler calls `Window::set_fullscreen`, which sets the window item's `full-screen`, so a system entry reaches every binding and `changed full-screen` in `.slint`. That a menu bar on Windows goes when hidden is left to step 9.

2. **The cursor is refreshed with a pointer move to where it is.** Slint picks the cursor only on pointer events, so `mouse-cursor: none` on `viewport-touch` would not hide a pointer at rest, nor bring it back on a key. `main.slint` calls `cursor-hidden-changed` with the pointer's position when idle starts or ends, and `full_screen::wire` dispatches a `PointerMoved` to that position, which makes Slint pick the cursor again; the activity check ignores a move to where the pointer already was. Public API, no winit access.

3. **The geometry is saved on entering.** The plan has Quit in full screen keep the geometry saved before entering, but nothing saved it then: the geometry is written at quit, close and hide. `toggle-full-screen` calls `full-screen-entering`, which saves it, and `save_geometry` skips while in full screen. An entry by the system (the green button) saves nothing, so there the last saved geometry stays. The hides leave through `slint::Window::set_fullscreen(false)` rather than the `.slint` function, since that hands the change to the backend at once, before the hide.

4. **Escape also closes the speeds and the pickers in the window**, not only in full screen: the order is the same in both, and full screen is simply the last thing it closes. A picker's own popup closes on Escape before `preview-keys` sees it; `TimeBar::close-transient` covers both cases in case it does not.

5. **One shortcut sheet row, and a tighter card.** The sheet gains one row, the platform's full screen key with "Full screen; Esc leaves", rather than two: with two the 800 by 700 window's card no longer held its content without scrolling, which `test_the_shortcut_sheet_keeps_its_text_on_the_card` requires. Even one row overflowed by 2 px, so the card's padding went from 13 to 12 px and its column spacing from 10 to 9.

6. **The e2e case closes GNOME's overview.** The case reads the rectangle with `GetWindowRect` on Windows and `xdotool` with `xrandr` on X11, and presses the keys with `PostMessageW` and with `xdotool` through XTEST. A GNOME session logs in to its overview, where `_NET_ACTIVE_WINDOW` is unset and `xdotool windowactivate` exits 0 without activating anything, so F11 went to the overview and the case failed in GNOME on X11. Before a key, the helper now sets GNOME Shell's `OverviewActive` to false over D-Bus and checks that the app's window is the active one, failing with that reason when it is not. Step 9 in the Wayland sessions was not done: the guest has no tool that types into a Wayland client, so it is left to a person, as `docs/platforms.md` records.

7. **The long pill fades out rather than going at 10 s.** The Tests section has the long pill "gone at 10 s" when the controls have already hidden, and so it was: the pill's visibility read `exit-pill-long || !idle-gone`, so with the controls long gone it vanished the moment the long form ended, the one thing in full screen that did not fade. Validation round 1 asked for the fade. The pill now has its own `exit-pill-gone`: once short, it fades with idle controls over `OverlayStyle.idle-fade` from whichever came later, the controls idling or the long form ending, and goes when that fade has run, about 10.5 s after entering with the mouse still. With the controls up at 10 s it still just shrinks to "Exit", as decision 6 has it.

## Sources

- Apple, "Mac keyboard shortcuts": https://support.apple.com/102650
- Apple Community, Fn-F in native apps: https://discussions.apple.com/thread/253599135
- Mozilla bug 1408118, two Enter Full Screen entries, quoting the AppKit 10.11 release notes: https://bugzilla.mozilla.org/show_bug.cgi?id=1408118
- Xojo forum, how AppKit finds the View menu's item: https://forum.xojo.com/t/enter-full-screen-menuitem/31148
- Electron, "fix: duplicate fullscreen macOS menu item (#49597)", read from a mirror of the commit: https://ayakael.net/mirrors/electron/commit/dc254e4bfc8eed03a7d09350bb89fd7b2e8aba3e
