# A menu bar

## Problem

The main window has no menu bar. Commands that do not belong in the panel have nowhere to go, and four features that are coming need one: exporting the current view as an image (today only the `render` subcommand), a dedicated settings window for app preferences, an update checker, and a fullscreen mode. Commands people look for in a menu (About, the shortcuts, the data folder, the website, quitting) are either buttons deep in the panel or missing.

The tray menu says "Open" and has no way to hide the window, which on macOS, where a click on the icon opens the menu rather than toggling the window, leaves the close button as the only way. It says "Exit" where the version notice says "Quit Sunlit Earth".

## What the app has today

- The main window (`MainWindow` in `crates/sunlit-ui/ui/main.slint`) has no menu bar. Its top-level actions are buttons in the panel: Set as Wallpaper, Load Defaults and Reset. About is a button at the foot of the Advanced section (`show-about`). The shortcut sheet opens with `?`, `F1` or `Ctrl + ?` on the preview (`shortcut-sheet-open`).
- The camera panel resets three things, each through `camera-reset(CameraReset)`: the N on the roll ring turns north up (`north`, tilt to 0), and the two ↺ badges put the look (`look`, yaw and pitch) and the frame (`frame`, the offsets) back to their defaults (`ui_callbacks.rs::reset_targets`). Position and zoom have no reset; the presets set them.
- The controls over the preview hide with H or the camera panel's button (`overlays-hidden`), the frame guides toggle with F or their button (`guides-shown`), and the time bar can be pinned (`time-bar-pinned`).
- The tray menu (`crates/sunlit-ui/ui/tray.slint`) is Open, Refresh Now, Auto-refresh, Start on login, About, Exit. A left click on the icon toggles the window through `window_state::toggle` on Windows and Linux; on macOS the click opens the menu.
- Every show and hide of the main window goes through `crates/sunlit-app/src/window_state.rs`: `show`, `show_and_focus`, `show_at_startup`, `hide`, `toggle`, and `hidden` for the close button's hide, which Slint performs after the handler answers.
- The close button's handler is registered in `app.rs::register_close_handler`. With a tray it saves the window geometry, calls `window_state::hidden` and answers `HideWindow`; without one it quits the event loop.
- The tray's Exit and the version notice's "Quit Sunlit Earth" (`activation.rs::register_version_notice`) both call `slint::quit_event_loop()`. Neither saves the window geometry, so a quit with the window on screen forgets a move or resize made since it was last closed.
- `about.rs` owns the only URL opener: `open_url` checks the URL against `is_web_url` and hands it to `cmd /C start` on Windows, `open` on macOS and `xdg-open` elsewhere. The repository URL is a property of `AboutWindow` (`repository`). The website, `https://sunlit.earth`, appears only in README.
- `sunlit_core::app_data_dir()` is the one folder that holds `config.toml`, the logs, the caches and the generated wallpapers: `%LOCALAPPDATA%\SunlitEarth`, `~/.local/share/SunlitEarth`, `~/Library/Application Support/SunlitEarth`. `SUNLIT_EARTH_CACHE_DIR` moves the cloud cache elsewhere; nothing else moves.

## What Slint 1.18.1 does with a `MenuBar`

Read from `i-slint-backend-winit-1.18.1`, `i-slint-core-1.18.1`, `i-slint-common-1.18.1` and `i-slint-compiler-1.18.1` in the cargo registry. Nothing here has been observed on a Mac.

- **Native or drawn.** On Windows the menu is a native Win32 menu attached to the window (`muda`, `init_for_hwnd_with_theme`, light or dark from winit's theme). On macOS it is the global menu bar (`init_for_nsapp`), set again whenever the window becomes active. On Linux, and in the testing backend, Slint draws it inside the window as its own widget, taking height from the content.
- **The macOS App menu.** The first menu on macOS belongs to the app. Slint builds it itself in `muda.rs::create_default_app_menu`, with `muda`'s predefined About, Services, Hide, Hide Others, Show All and Quit, on every build of the menu bar ("Until we have menu roles, always create an app menu on macOS"). The app cannot add, remove or retarget anything in it, and a rebuild, which happens whenever a property the menu reads changes, discards anything put there from outside. The same menu is installed for a window with no `MenuBar` (`with_default_menu_bar` defaults to on), so macOS users already have it.
- **Its About** opens AppKit's standard About panel, built from the bundle's `Info.plist`, not the app's About window.
- **Its Quit** sends AppKit's `terminate:`. Whether that takes the path `slint::quit_event_loop` takes is not known; the roadmap already has the macOS teardown open, and other toolkits report Quit there skipping their close handling (Qt, wxWidgets; sources at the end).
- **Shortcuts.** `MenuItem` has `shortcut: @keys(...)`, accepted only inside a `MenuBar`. `Control` is Command on macOS (`keys_to_accelerator`), so `@keys(Control + Q)` is Ctrl+Q on Windows and Linux and ⌘Q on macOS. On Windows and macOS a shortcut is a native accelerator the menu takes for the whole window, so a bare letter would also fire while a text field has focus.
- **Checkmarks.** The native path builds a `CheckMenuItem` and the drawn menu draws a check for `checked`. This plan uses none (decision 5).
- **Closing from code.** `Window::dispatch_event(WindowEvent::CloseRequested)` runs the `on_close_requested` handler and hides the window when it answers `HideWindow` (`api.rs`).

Keys the preview takes that matter here: W, A, S, D carry their modifiers like the arrows, so Ctrl+W (⌘W on macOS) moves the frame up. Any other key with Ctrl, Alt or Meta is passed on, so Ctrl+Q reaches nothing on the preview.

## Decisions

Settled with the user on 2026-11-08.

### 1. Three menus: File, View and Help

```
File
  Set as Wallpaper
  ─────
  Revert to Saved
  Load Defaults
  ─────
  Close Window
  Quit                       Ctrl+Q     not on macOS, see decision 7

View
  Reset Camera
  ─────
  Hide Controls / Show Controls
  Show Frame Guides / Hide Frame Guides
  Pin Time Bar / Unpin Time Bar

Help
  Mouse and Keyboard
  ─────
  Website
  Report an Issue…
  Open Data Folder
  ─────
  About Sunlit Earth
```

- **Set as Wallpaper** calls `set-wallpaper`, the button's callback.
- **Revert to Saved** calls `reset` and **Load Defaults** calls `load-defaults`, the buttons' callbacks. The Reset button is renamed Revert to Saved (decision 4).
- **Close Window** does what the title bar's close button does (decision 8).
- **Quit** ends the app and saves the window geometry (decision 3).
- **Reset Camera** runs the camera panel's three resets at once: north up, the look, the frame. Position and zoom stay, since they are the view a preset or the user chose.
- **Hide Controls**, **Show Frame Guides** and **Pin Time Bar** toggle `overlays-hidden`, `guides-shown` and `time-bar-pinned` (decision 5).
- **Mouse and Keyboard** opens the shortcut sheet (`shortcut-sheet-open = true`). The title follows the sheet's own heading, "Mouse and keyboard".
- **Website** opens `https://sunlit.earth`; **Report an Issue…** opens `https://github.com/sunlit-earth/sunlit-earth/issues/new`.
- **Open Data Folder** opens `sunlit_core::app_data_dir()` in the platform's file manager (decision 10).
- **About Sunlit Earth** calls `show-about`, as the button in the panel does.

There are no Wallpaper or Time menus. Every command they would hold has a control in the panel or the time bar, and the shortcut sheet is how those are found. View is the exception because fullscreen needs a home and the overlay toggles have no place outside the preview.

### 2. Items left out until their feature exists

Each of these is added together with its feature, designed in its own plan, and is not in the menu before then, not even disabled:

| Item | Where | Shortcut it is expected to take |
|---|---|---|
| Export Image… | File, first | Ctrl+E |
| Settings… | File, between Load Defaults and Close Window; the tray, under Show / Hide | Ctrl+, |
| Check for Updates… | Help, above About | none |
| Enter Full Screen | View, last, after a separator | F11 on Windows and Linux, ⌃⌘F on macOS |

For the fullscreen plan: on macOS the window already has the green title bar button, and AppKit may add its own "Enter Full Screen" to a menu titled View; whether it does under Slint's menu bar is to be checked then, so the app's item does not appear twice.

### 3. "Quit" everywhere, and Quit saves the geometry

The tray's Exit becomes Quit; the version notice already says "Quit Sunlit Earth". Windows' older guidelines say Exit and GNOME, KDE and macOS say Quit; the app picks one word, and Quit is also what macOS's App menu says whatever the app chooses.

Quit in the menu, Quit in the tray, the version notice's button and the close button without a tray all end the app through one function, `menu::quit(window)`. It saves the window geometry when the window is on screen, with the same `config::save_window_geometry` call the tray-mode close handler makes (moved into a helper both use), and then calls `slint::quit_event_loop()`. The IPC `quit` command is left alone, so the e2e suite's quits are unchanged.

### 4. The panel buttons take the menu's words

The Reset button becomes "Revert to Saved", beside "Load Defaults", so the panel and the menu use the same two labels. Its hint stays. The longer label may raise the panel's content floor (`panel-min-width` and the tests that hold the sidebar to it); if "Revert to Saved" and "Load Defaults" no longer fit side by side at the default sidebar width, the two buttons stack, and that goes in Departures.

### 5. View toggles flip their titles

The three View toggles are plain items whose title says what choosing them does, as Finder's View menu does ("Hide Toolbar", "Show Path Bar"), the same pattern as the tray's Show / Hide. They are not checkable items: a checked "Hide Controls" reads backwards, and a checkable item's `checked` would need a two-way binding to a property the item inverts. The menu rebuilds when a title's property changes, so the titles follow the H and F keys and the overlay buttons too.

### 6. Shortcuts

Only Quit gets one, `@keys(Control + Q)`. Close Window gets none, since Ctrl+W and ⌘W move the frame on the preview. The View and Help entries get none either: the preview's single-letter keys cannot be menu accelerators without firing while a text field has focus, and the shortcut sheet keeps its own F1, `?` and Ctrl+?.

### 7. macOS: Slint's App menu stays as it is

- The app's own About is "About Sunlit Earth" in Help. The App menu's About opens AppKit's standard panel, so a macOS user has two Abouts.
- File has no Quit on macOS, since the App menu's Quit holds ⌘Q and is where macOS users look. The item is behind `if Platform.os != OperatingSystemType.macos`.
- Settings… and Check for Updates… will go in File and Help on macOS as everywhere else.
- The App menu's Hide (⌘H) hides the app through AppKit without passing through `window_state`, so the tray then reads "Hide" while nothing is on screen. Choosing it hides the window properly and the next choice shows it. Accepted, and noted in `docs/platforms.md`.

Patching the App menu through `objc2` after Slint builds it is not pursued: Slint rebuilds the whole menu bar, App menu included, whenever a property the menu reads changes, and the View titles of decision 5 change often. If Slint adds menu roles later, moving About, Settings and Check for Updates into the App menu is a follow-up.

### 8. Close Window is the close button

The item calls a new `close-window` callback, whose Rust side dispatches `WindowEvent::CloseRequested` to the window. That runs the handler `register_close_handler` installed, so Close Window hides to the tray when there is a tray and quits through `menu::quit` when there is not. The title is "Close Window" in both cases.

### 9. The tray menu

```
Show / Hide
─────
Refresh Now
✓ Auto-refresh
✓ Start on login
─────
Website
About
Quit
```

- **Show / Hide** replaces Open. It reads "Hide" while the main window is on screen and "Show" while it is not, and calls `toggle-window`, the callback the icon's click already uses. Slint rebuilds the tray menu when a property its titles read changes, as the two checkmark titles already rely on, so the title follows a new `window-shown` property.
- **Website** opens `https://sunlit.earth` through the same opener as the menu bar.
- **Quit** replaces Exit (decision 3).

`window-shown` is kept current from `window_state.rs`. The module gets a visibility listener next to `OTHER_WINDOWS`: `register_visibility_listener(Fn(bool))`, called with `true` by `show` and by `show_at_startup` on its shown path, and with `false` by `hide` and `hidden`. `tray::wire` registers one that sets the property, and sets it once from `window.window().is_visible()` when it wires.

### 10. One opener for URLs and the data folder

`about.rs`'s `open_url`, `is_web_url` and `platform_open` move to a new module, `crates/sunlit-app/src/open.rs`, with their tests, and gain a folder opener:

- `open::url(url)`: unchanged behavior, the scheme and character check and then `cmd /C start "" <url>`, `open <url>` or `xdg-open <url>`.
- `open::folder(path)`: `explorer.exe <path>` on Windows, `open <path>` on macOS, `xdg-open <path>` elsewhere. None goes through a shell, so the path needs no character check. It creates the folder first (`create_dir_all`) and logs a warning on failure as `open_url` does.

Each platform's command comes from a function returning a `std::process::Command`, so a test can read its program and arguments without running it.

The three URLs live in one Slint global, `Links` in a new `crates/sunlit-ui/ui/links.slint` (`website`, `repository`, `new-issue`). `AboutWindow` reads `repository` from it, and `MainWindow` and `TrayIcon` pass `Links.website` and `Links.new-issue` to their own `open-url(string)` callback, wired in Rust to `open::url`.

### 11. Where the Rust side lives

A new `crates/sunlit-app/src/menu.rs` wires the menu bar's new callbacks (`close-window`, `open-url`, `open-data-folder`, `reset-camera`) and holds `quit` and the geometry helper, the way `tray.rs::wire` wires the tray. `activation.rs::register_version_notice` and the no-tray close handler call `menu::quit`. The View toggles need no Rust: they assign the window's own properties in Slint.

### 12. Window names

The app's windows are named for what they are, in code, comments, docs and anything a user reads:

- **main window**: `MainWindow`, the window with the preview, the panel and, after this plan, the menu bar. It was called "the settings window" in most of the tree.
- **About window**: `AboutWindow`.
- **settings window**: only the dedicated window for app preferences that Settings… will open (decision 2). Nothing is called that until it exists.

"Settings window" for `MainWindow` becomes "main window" everywhere it appears today: about 80 lines across code comments, one test's assertion message (`window_state.rs`), one lint reason (`params.rs`), the crate READMEs, CLAUDE.md and the current docs (architecture, platforms, usage, testing, roadmap, app-icon, macos-testing, `docs/README.md`). Two places in `slint_ui.rs` split the phrase across a line break, so a search has to look for "settings" at the end of a comment line too. Historical documents keep the old term: `docs/plans/`, `docs/reviews/`, `CHANGELOG.md` and the technology survey `docs/tech.md`. "The panel" stays the name of the controls on the main window's left side.

The top-level `README.md` is the exception in timing: its two mentions change after 0.6.0 is released, together with the Revert to Saved sentence (see Docs), because README describes the release users have installed.

### 13. A copyright line in the macOS About panel

The App menu's About (decision 7) opens AppKit's standard panel, which builds itself from the bundle's `Info.plist`: the icon, `CFBundleName`, a version line from `CFBundleShortVersionString` and `CFBundleVersion`, a copyright line from `NSHumanReadableCopyright`, and credits from a `Credits.html`, `Credits.rtf` or `Credits.rtfd` in `Contents/Resources`. `assets/macos/Info.plist` has no copyright key, so the panel shows none.

The template gains `NSHumanReadableCopyright` with "© 2026 Sebastian Straub", the line the app's own About window shows (`about.slint`). No credits file: the About window's Attributions tab already carries them. The version keys stay as they are; their format (Apple documents both as period-separated integers, and a prerelease puts `-beta.1` in them) belongs to the update checker's plan, since updaters on macOS compare them.

### Not chosen

- **Wallpaper and Time menus** mirroring the panel and the time bar: a second copy of every action and state to keep in sync, for commands that already have a visible control.
- **Disabled placeholders** for the items of decision 2.
- **"Exit" on Windows and "Quit" elsewhere**, which follows each platform's older convention but splits the app's own wording.
- **Ctrl+W for Close Window**, which would take the key from the preview's frame movement.
- **A Reset Camera submenu** with the three resets apart. The camera panel already offers each one where its stick is.

## Implementation steps

1. **Opener.** Add `open.rs` with `url` and `folder`, moving the code and tests out of `about.rs`, which then calls `open::url`. Add `links.slint` with the `Links` global, export it from `sunlit-ui`, and have `about.slint` read `repository` from it.
2. **Menu bar.** In `main.slint`, add a `MenuBar` to `MainWindow` with the entries of decision 1 and the callbacks `close-window()`, `open-url(string)`, `open-data-folder()` and `reset-camera()`. Quit is a conditional `MenuItem` with `shortcut: @keys(Control + Q)`; if Slint refuses a shortcut on a conditional item, the condition goes, Quit shows on macOS too, and a note goes in Departures. Rename the Reset button to Revert to Saved.
3. **Menu wiring.** Add `menu.rs` with `wire`, `quit` and the geometry helper. `reset-camera` calls the existing camera reset for `north`, `look` and `frame`; the three ease different fields, which is to be confirmed by watching them run together. Call `menu::wire` from `app.rs` where the other window callbacks are registered; switch `register_version_notice` and the no-tray close handler to `menu::quit`, and the tray-mode close handler to the geometry helper.
4. **Tray.** In `tray.slint`, Open becomes Show / Hide reading `window-shown`, Exit becomes Quit, Website is added, and `open-url(string)` and `quit()` (renamed from `exit-app`) are declared. In `tray.rs::wire`, wire them to `open::url` and `menu::quit`, and register the visibility listener. Remove `open-window` if nothing else calls it.
5. **Visibility listener.** Add `register_visibility_listener` to `window_state.rs` and the calls in `show`, `show_at_startup`, `hide` and `hidden`. Update the module's doc comment, which names "the tray's Open".
6. **Window names.** The rename of decision 12, outside the historical documents and README. Done last among the code steps, so the comments written in steps 1 to 5 use the new name from the start and the rename catches what was there before.
7. **Info.plist.** Add `NSHumanReadableCopyright` to `assets/macos/Info.plist` and assert it in `bundle.rs`'s plist test (`the_plist_carries_the_version_and_the_identity_that_never_changes`).
8. **Docs** (below).
9. **Checks on the platforms.**
   - Windows desktop, `cargo app`: the menu is native and follows the dark or light theme; every entry does what decision 1 says; Ctrl+Q quits and the next start opens at the position and size the window had; Ctrl+W still moves the frame on the preview; the View titles flip when H and F are pressed on the preview; the tray's Show / Hide title follows the window when it is closed with the button, hidden from the tray, and shown by a second launch.
   - Linux guest, KDE on X11 and GNOME on Wayland (`cargo xtask vm up linux`, `vm view`): the drawn menu bar and its keyboard navigation; Ctrl+Q with the preview focused and with a text field focused; Open Data Folder opening a file manager.
   - macOS runner, as far as it can be driven: the build compiles; File has no Quit; whether the App menu's Quit takes the app's teardown and saves the geometry, logged either way. The result goes in `docs/platforms.md`, and a roadmap entry if it skips them.

## Tests

### Unit

- `open.rs`: the moved `is_web_url` and document tests; every `Links` URL passes `is_web_url`; the folder command on this platform is the program decision 10 names with the path as its one argument; the URL command is unchanged.
- `window_state.rs`: a listener registered twice is called by both, and nothing is called before one is registered.

### `slint_ui`

- The tray's Show / Hide title reads "Show" for a hidden window, "Hide" after `window_state::show`, and "Show" after `window_state::hide` and after the close handler's `hidden`, through the existing `wired_tray` helper.
- The tray's titles say "Quit" and not "Exit".
- `close-window` on a window with the tray-mode close handler hides it and turns the tray's title to "Show".
- `open-url` from the menu and from the tray reaches the opener with `Links.website` and `Links.new-issue`, with the opener replaced by a recorder the way the About window's link test does it.
- `reset-camera` puts tilt to 0 and the look and the frame to their defaults once the eases settle, and leaves position and zoom alone.
- The View titles follow `overlays-hidden`, `guides-shown` and `time-bar-pinned`, if the testing backend exposes the menu entries; otherwise the title properties are exposed as `out` properties and read from there.
- The test that finds "Load Defaults" by accessible label finds "Revert to Saved" the same way.
- Layout tests that read positions keep passing with the drawn menu bar on top; any test comparing an absolute y against a constant is the one to look at, and so is every test of the panel's width floor after the rename.

### e2e

No new case. `test_tray_hide_show_cycle` and the cases that quit through IPC keep passing, since neither the IPC commands nor their signals change.

## Docs

- `docs/architecture.md`: the `TrayIcon` paragraph's list of entries; a sentence on the `MenuBar`, and `menu.rs` and `open.rs` in the module list.
- `docs/platforms.md`: the menu bar per platform (native on Windows and macOS, drawn on Linux), Slint's macOS App menu and its two Abouts, the App menu's Hide and the tray title, and what step 9 found about the App menu's Quit.
- `docs/usage.md`: the menu bar's entries and the tray's Show / Hide.
- `docs/roadmap.md`: a done entry for the menu bar and the tray menu; open entries for Export Image, Settings, Check for Updates, fullscreen, and Slint menu roles for the macOS App menu.
- `README.md` waits until 0.6.0 is released: then "Save or restore settings" names Revert to Saved instead of Reset, and its two mentions of the settings window say main window (decision 12).

## Gates

`cargo fmt --check`, `cargo test`, `cargo clippy --all-targets`, and step 9.

## Departures

1. **A held Q is let go when Ctrl goes down.** The preview does take Q under Ctrl: Q and E roll whatever modifiers are down, so the claim above that Ctrl+Q reaches nothing on the preview does not hold. And Slint's window matches menu bar shortcuts against both the press and the release of a key before the focused item sees either (`process_menubar_shortcuts` in `i-slint-core` 1.18.1's `window.rs`, on every platform, the native menus included), so with Quit's shortcut in place, Q held, Ctrl pressed and Q let go would hand the release to the menu and the roll would run on with no key down. The preview's Ctrl branch in `main.slint` now lets go of a held Q when Ctrl goes down, behind the same `Platform.os != macos` condition as the Quit item. The held-key test's Ctrl and Q case passes through that path.

2. **Two tests stop short of the real path.** The testing backend shows no tray menu, so the tray's titles and its Website entry cannot be clicked from a test: the tray's Show / Hide title is read from its `window-title` property, the Quit wording is checked in `tray.slint`'s source, and the tray's `open-url` is not driven through a recorder. The drawn menu bar, on the other hand, can be clicked, so every menu bar test chooses its entry the way a person does. The Close Window test installs a tray-mode close handler of its own, without the geometry write, because `app.rs`'s handler saves to the real config file and the tests have no per-test config path.

3. **The tray title cases skip on macOS.** `test_the_tray_show_hide_title_follows_the_window` and `test_close_window_hides_to_the_tray` failed on the `macos-latest` runner of `ci.yml` (<https://github.com/sunlit-earth/sunlit-earth/actions/runs/42489836889>) with "SystemTrayIcon must be created on the main thread on macOS", from `i-slint-core` 1.18.1's `items/system_tray/appkit.rs`. Slint creates a tray icon's platform item lazily, from a change handler, and the AppKit backend asks for the main thread when it does, which a libtest case never has. These are the only tray cases that show the window, and the other tray cases, which build the same `TrayIcon`, pass there, so showing the window is what lets the handler run. Both cases now return early on macOS with a printed reason, the convention the e2e suite follows for what a session cannot do, and still run on Windows and Linux. What they check on macOS, the tray's title following the window, is left to the hand check over VNC or a tester.

4. **The hand checks of step 9 go to the user.** Step 9's checks on the Windows desktop and the interactive ones in the Linux guest take over the user's screen and wallpaper, so the run left them out and ran only the e2e suite in the Linux guest (KDE on X11, GNOME on Wayland) and on the macOS runner. What remains for the user: on the Windows desktop, the native menu following the theme, every entry, Ctrl+Q quitting and the next start opening at the same position and size, Ctrl+Q with a text field focused, Ctrl+W still moving the frame, Q held and then Ctrl not leaving the roll running, the View titles flipping on H and F, the tray's Show / Hide title across the close button, a hide from the tray, an icon click and a second launch, and Revert to Saved sitting beside Load Defaults; in the Linux guest on KDE X11 and GNOME Wayland, the drawn menu bar and its keyboard navigation, Ctrl+Q with the preview and with a text field focused, Open Data Folder, and the pickers and the shortcut sheet opening below the bar; on a Mac, the App menu's About showing the copyright, ⌘Q and the geometry it keeps, the App menu's Hide against the tray's title, and AppKit's own Enter Full Screen.

5. **A hide from the tray or through IPC saves the geometry too.** Decision 3 saves it on Quit when the window is on screen, and the close button in tray mode already saved it, but the tray's Show / Hide, the icon click and IPC `hide-window` hid the window without a save, so a move followed by Hide and then Quit from the tray lost the move. The tray's toggle and the IPC hide now call `menu::save_geometry` before hiding. The save sits in those two callers rather than in `window_state::hide`, because a `slint_ui` case calls `window_state::hide` directly and the tests have no per-test config path; the e2e suite's IPC hide writes to its isolated config. `save_geometry` also skips a minimized window, which Windows reports as visible at (-32000, -32000).

## Validation

- Round 1 (`ca634ac..cbccb0e`): 2 majors, 3 minors. A minimized window's geometry was saved on Quit; step 9's hand checks were not done (departure 4); the release readmes said Exit; two stale sentences in `docs/architecture.md`. All fixed in `3be37ba` and `ebcdb0f`.
- Round 2 (`cbccb0e..ebcdb0f`): every round 1 fix verified, 0 majors, 1 minor: a hide from the tray did not save the geometry (departure 5). The "Failed to create system tray icon" lines on `slint_ui`'s stderr come from the two tray cases that show a window with no tray image set, and are harmless.

## Sources

- [Apple HIG, the menu bar](https://developer.apple.com/design/human-interface-guidelines/the-menu-bar)
- [Microsoft, Windows Interface Components](https://learn.microsoft.com/ko-kr/previous-versions/ms997491(v=msdn.10)): Exit at the bottom of the File menu
- [GNOME 2 HIG, keyboard](https://mbkma.github.io/gnome-2-hig-book/input-keyboard.html): Quit, Ctrl+Q
- [Qt forum](https://forum.qt.io/post/809582) and [an OrcaSlicer commit](https://git.trueserve.org/PublicCodeMirror/OrcaSlicer-bambulab/commit/48cc358b7271b70f6c5d1ca2aa15ba42ab37eac8.patch): the macOS App menu's Quit skipping a toolkit's close handling
- [Inkscape MR 3767](https://gitlab.com/inkscape/inkscape/-/merge_requests/3767): menu roles moving About, Preferences and Quit into the App menu
