# Start on login, and a second launch that opens the running app

Research: [2026-10-24-autostart-research.md](2026-10-24-autostart-research.md). Every platform claim this plan rests on is sourced there, and the ones nobody has observed yet are marked unverified there and turned into checks in Step 10 here.

## Problem

Sunlit Earth has no way to start when the user logs in. `--tray-start hidden` exists, but the user has to write an autostart entry by hand, and nothing tells them which path survives an upgrade.

A second launch is confusing. `app.rs` takes the single-instance guard, finds it held, logs "another instance is already running, exiting" and returns success, so a user who starts the app while it runs hidden in the tray sees nothing happen. After an update that left the old version running (Scoop skips a running app), they see the old version and no explanation.

On macOS the app always has a Dock icon, even while it lives in the menu bar with its window hidden, and once the window is hidden from the Dock's point of view nothing brings it back: winit never delivers the reopen event a Dock click or a Finder double-click sends.

## Decisions

These were settled with the user on 2026-10-24.

### 1. One switch, "Start on login", and the OS entry is the setting

A `SettingCheck` "Start on login" sits under "Auto-refresh wallpaper" in the settings window, with the hint "Start Sunlit Earth hidden in the tray when you log in." The tray menu gets the same switch (decision 9). It is not in `AppConfig` and not in `config.toml`. Its state is read from the platform's own entry at startup, every time the settings window is shown, and after every toggle from either place, so a change made in Task Manager, Plasma's or XFCE's autostart settings, or GNOME Tweaks is what the switch shows the next time it is read. Nothing tells the app when the tray menu is about to open, so between those reads the tray can show a state the system has since changed; a click there acts on what it showed.

Turning it on writes the entry for this copy of the app; turning it off removes the entry. Each write is followed by a fresh read, and the switch shows what the read found, so a write that failed leaves the switch where it was and the status line says why.

If an entry exists but runs a different executable than this one, the switch shows on and the hint names the other path ("Starts `<path>` at login"). Turning it off and on again points it at this copy. The app never rewrites an entry on its own at startup, so a development build cannot take over an installed copy's entry.

The command every entry runs is the executable and `--tray-start hidden`, nothing else.

### 2. Windows: an `HKCU\...\Run` value

Value `SunlitEarth` under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, type `REG_SZ`, data `"<current_exe()>" --tray-start hidden`. The path is quoted always and never canonicalized, so Scoop's `current` junction is what gets written (research: Windows, command line and path). HKLM is never touched.

- **Read:** enabled when the value exists and the same-named value under `HKCU\...\Explorer\StartupApproved\Run` is absent, or is REG_BINARY with an even first byte (`00`, `02`, `06`). An odd first byte (`03`, `07`) is the user's disable in Task Manager or Settings and reads as off; the hint then says "Turned off in Windows' Startup apps settings" so the user knows where it was switched off. The executable is parsed back out of the value with the C runtime rules to compare it with `current_exe()`.
- **Enable:** write the `Run` value, then delete the `StartupApproved\Run` value of that name, as Electron does. Without the delete a disable the user made in Task Manager once would keep the new entry off.
- **Disable:** delete both values.

Through `windows-sys` with the `Win32_System_Registry` feature added, not the `auto-launch` crate, which writes the path unquoted and tries HKLM first. Every FFI call carries a scoped `#[allow(unsafe_code)]` and a `// SAFETY:` comment.

### 3. Linux: an XDG autostart entry

`$XDG_CONFIG_HOME/autostart/sunlit-earth.desktop`, falling back to `~/.config/autostart` when the variable is unset or blank:

```ini
[Desktop Entry]
Type=Application
Name=Sunlit Earth
Comment=Keep the Earth wallpaper up to date
Exec=<quoted path> --tray-start hidden
TryExec=<path>
Icon=sunlit-earth
Terminal=false
StartupNotify=false
X-GNOME-Autostart-enabled=true
```

- **Path:** `current_exe()` reads `/proc/self/exe`, which for the Homebrew formula is `<prefix>/Cellar/sunlit-earth/<version>/libexec/sunlit-earth` and disappears at the next `brew upgrade`. When the path has a `Cellar/sunlit-earth/<version>/` component, the entry is written with `<prefix>/opt/sunlit-earth/bin/sunlit-earth`, but only if that path exists and canonicalizes to the same file; otherwise `current_exe()` is used. The same mapping is what the comparison in decision 1 uses.
- **Quoting:** the `Exec` rules of the desktop entry specification: an argument holding a space or a reserved character is double quoted with `"`, `` ` ``, `$` and `\` escaped inside, then the string escapes of the key file format are applied on top, and `%` is doubled. `TryExec` is unquoted.
- **Read:** enabled when the file exists, `Hidden` is not `true` and `X-GNOME-Autostart-enabled` is not `false`. Plasma and XFCE switch it off with the first, Cinnamon with the second, GNOME Tweaks deletes the file.
- **Enable:** write the file whole (a `Hidden=true` the user left behind goes with the old file). **Disable:** delete it.
- **Sessions that ignore the folder:** when `XDG_CURRENT_DESKTOP` names sway, i3 or Hyprland (compared case-insensitively, any element of the colon-separated list), the switch is disabled and its hint says "This session does not start apps from the autostart folder. Add `exec <path> --tray-start hidden` to its config." with the real path. Every other value, an empty one included, gets a working switch.

The Background portal and systemd user units are not used (research: Linux, mechanisms).

### 4. macOS: a LaunchAgent that starts the app through `open`

`~/Library/LaunchAgents/earth.sunlit.SunlitEarth.plist`:

- `Label` `earth.sunlit.SunlitEarth`, `RunAtLoad` true, `LimitLoadToSessionType` `Aqua`, `ProcessType` `Interactive`, `AssociatedBundleIdentifiers` `[earth.sunlit.SunlitEarth]`.
- `ProgramArguments` for the bundle: `[/usr/bin/open, -a, <bundle path>, --args, --tray-start, hidden]`, where the bundle path is `current_exe()` up three levels when it ends in `.app/Contents/MacOS/<name>`. For the bare binary: `[<path>, --tray-start, hidden]`.
- A bundle path containing `/AppTranslocation/` is refused with "Move Sunlit Earth to the Applications folder first", because that path is random per launch.
- **Read:** enabled when the file exists. A switch-off in System Settings > Login Items is not reflected; `SMAppService.statusForLegacyPlist` might report it, but nobody has seen what it returns for an ad-hoc signed agent and the runner cannot flip that switch, so it stays out of this change and on the roadmap.
- **Enable:** write the file. **Disable:** delete it. The app never calls `launchctl`: `bootstrap` would start a copy at once, and `bootout` of a direct-exec agent would end the running app. Both take effect at the next login.

The plist is written by hand with XML escaping; no plist crate.

### 5. A second launch opens the running app

- **The channel.** The first instance always listens on its own local socket, separate from the opt-in test IPC of `--ipc-socket`. Its name derives from the instance name the guard uses (`sunlit-earth-app`, or the one `--ipc-socket` gives, so the e2e suite's instances stay apart) plus a per-user part: Windows named pipes are machine-wide and Linux abstract sockets are shared by every user on the host, so two users must not reach each other's instance. The guard's own name gets the same per-user part on Linux, where the abstract socket has the same problem today. The implementer picks the per-user part (the session id on Windows, the uid on Unix) and records it.
- **The exchange.** The second process connects, reads one line `pid=<n>` from the first, calls `AllowSetForegroundWindow(n)` on Windows, then writes `key=value` lines `version=`, `exe=`, and when present `activation-token=` (from `XDG_ACTIVATION_TOKEN`) and `startup-id=` (from `DESKTOP_STARTUP_ID`), ends with an empty line, and exits 0. Values are percent-encoded so a path with any character fits on one line. Connecting retries for up to two seconds, which covers a first instance that holds the guard and has not bound yet; if it still fails the second process logs that and exits 0 as today.
- **A hidden second launch says nothing.** With `--tray-start hidden` the second process exits 0 without connecting: that is an autostart entry firing beside a running app, and the window must not open.
- **The first instance** shows its window through the function of decision 7 and focuses it, turns the preview on, and prints `SIGNAL:activated version_differs=<bool>`. On X11 the forwarded startup id is set before the show. On Wayland the token cannot reach the recreated window through Slint 1.18 (research), so a "ready" notification or a highlighted task bar entry is the expected result there and is accepted for this change.
- **Different versions.** When the second launch's `version` differs from the running one, the window shows a notice over the preview: "Sunlit Earth `<running>` is already running, so the copy you opened (`<launched>`) did not start. To use `<launched>`, quit Sunlit Earth and open it again." with two buttons, "Quit Sunlit Earth" (the tray's "Exit" path) and "Close". Same version, nothing is shown beyond the window. The launched path is in the log line, not in the notice.

### 6. At login on Linux, wait for the tray before deciding there is none

When starting hidden on Linux and `org.kde.StatusNotifierWatcher` has no owner on the session bus, the app waits for it through `zbus` (`NameOwnerChanged`) for at most `TRAY_HOST_WAIT`, ten seconds, before it creates the tray, and logs how long it waited. If the watcher comes, the app runs hidden in the tray; if not, it does what it does today: no tray, window shown, close exits (the user's decision 1). The wait happens only when starting hidden, so a launch from the app menu is not slowed. Slint creates the tray, so ksni's `assume_sni_available` cannot be passed through; filing that upstream is out of scope.

### 7. macOS: the Dock icon follows the window

- `assets/macos/Info.plist` gains `LSUIElement` true. winit 0.30.13 sets no activation policy for a bundled app when none is given, so the bundle starts with no Dock icon and a hidden start at login never shows one.
- Every place that shows or hides the main window goes through one pair of functions in a new `window_state.rs` (the tray's open and toggle, the close handler, the deferred hide, the IPC commands, decision 5, and the reopen handler below). On macOS showing sets the policy to regular and activates in the order the research reports as the workaround for the stale menu bar: show the window, set the regular policy, then activate. Hiding sets accessory when no other window of the app (the About window) is still visible. On the other platforms the functions only show or hide.
- The bare binary has no bundle, so winit sets the regular policy at launch; the first hide switches it to accessory like the bundle.
- **Reopen.** `applicationShouldHandleReopen:hasVisibleWindows:` is added to winit's `WinitApplicationDelegate` with `class_addMethod`, the way Slint 1.18 adds `applicationShouldAutomaticallyLocalizeKeyEquivalents:`, guarded by `instance_method(sel).is_none()`. It posts the show of decision 5 to the event loop and returns `NO`. It is added after the Slint backend exists and before the event loop runs.

`docs/roadmap.md`'s claim that `LSUIElement` needs `unstable-winit-030` is wrong since winit 0.30.6 and is corrected. The ordered teardown on that roadmap entry still needs the feature and stays.

### 8. Code layout

- `crates/sunlit-app/src/autostart/mod.rs`: `Autostart` state (`Off`, `On { exe }`, `TurnedOffBySystem { exe }`, `Unsupported { reason }`), `read`, `enable`, `disable`, and the command line every entry runs. `windows.rs`, `linux.rs` and `macos.rs` behind `cfg`. Each backend takes its location (registry key paths, the autostart directory, the LaunchAgents directory) as a value, so unit tests point it at a scratch key or a temp directory and never at the real one.
- `crates/sunlit-app/src/activation.rs`: the socket, the exchange, its encoding, and the version decision.
- `crates/sunlit-app/src/window_state.rs`: decision 7's show and hide, and the macOS policy and reopen code.
- `zbus` becomes a Linux dependency of `sunlit-app` (already in the tree through `sunlit-core`), and the `objc2` crates `sunlit-core` already uses become macOS dependencies of the app. No new crate enters the tree; if one does, `cargo xtask bake licenses` is rerun and that is a departure.
- Two IPC commands for the tests, `autostart-enable` and `autostart-disable`, and one query, `autostart-status`, answered with `SIGNAL:autostart state=<off|on|off-by-system|unsupported> exe=<percent-encoded>`.

### 9. The tray menu shows its checkmarks again, and gains "Start on login"

**Why the checkmark is gone.** Until 2026-08-15 the tray was the `tray-icon` crate, and "Auto-refresh" was a `CheckMenuItem`. Commit 3645b55 replaced it with Slint's `SystemTrayIcon`, and `ui/tray.slint` declares the item correctly (`checkable: true`, `checked <=> root.auto-refresh-enabled`). Slint copies both fields into its `MenuEntry`, but none of its three tray backends reads them: the Windows one calls `AppendMenuW` with `MF_STRING` and never `MF_CHECKED`, the Linux one builds every item as a ksni `StandardItem` rather than a `CheckmarkItem`, and the macOS one sets `enabled` on the `NSMenuItem` and never its state (`i-slint-core-1.18.1/items/system_tray/{windows,ksni,appkit}.rs`). Slint's `master` branch is the same as of 2026-10-24, 1.18.1 is the newest release, and a search of the issues in `slint-ui/slint` found none that reports it. The toggle itself still works, because Slint flips `checked` before `activated` fires whether or not the backend draws it, so only the mark went missing.

**The fix here.** The state goes into the item's title: "✓ Auto-refresh" when on and "Auto-refresh" when off, and the same for "Start on login". Slint rebuilds the tray menu whenever a property read while building it changes, so a title bound to the state is redrawn on every platform without anything from Rust. Each title is an `out property` of `TrayIcon` so a test can read it. `checkable` and `checked` stay on both items, so the toggle keeps its current path and a Slint release that draws the mark only needs the prefix removed. The unchecked title has no padding: the text shifts by the mark's width, which is accepted rather than guessed at with spaces whose width differs per menu font.

**The new item.** "Start on login" sits under "Auto-refresh":

```
Open
---
Refresh Now
Auto-refresh
Start on login
---
About
Exit
```

Its `checked` is the read of decision 1, set from Rust after each read. Activating it writes or removes the entry through the same function the settings switch uses, then reads again and sets both the tray item and the switch from that read. Where autostart is unsupported (sway, i3, Hyprland) the item is `enabled: false`, which every backend honors. A failed write from the tray has no status line to show it in, so it is logged as a warning and the item shows whatever the read after it found.

**Upstream.** The real fix is a few lines per backend in Slint (`MF_CHECKED`, `CheckmarkItem`, `NSMenuItem.state`). Opening an issue or a pull request on `slint-ui/slint` is for the user to decide and is not part of an implementer's work; the roadmap gets an item to drop the prefix once a Slint release draws the mark.

### Not chosen

- A Task Scheduler logon task on Windows: not listed in Startup apps, so the user's usual switch cannot turn it off, and its defaults stop it on battery.
- `SMAppService` on macOS: 13 and later only, takes no arguments, and has one report of an ad-hoc signed registration breaking at an update.
- The `auto-launch` crate: quoting and the reading of the user's disable are wrong in its released version on both Windows and Linux.
- Offering to start the launched copy from the version notice.
- Patching `i-slint-core` through `[patch.crates-io]` to draw real checkmarks: a vendored copy of a core Slint crate to carry across every Slint update, for three lines of difference.
- Going back to the `tray-icon` crate for the menu: it is what 3645b55 removed along with its own message pump.

## Implementation steps

Each step leaves `cargo test` and `cargo clippy --all-targets` green and is its own commit or few.

1. **`window_state.rs`.** Route every show and hide through it, with no behavior change on Windows and Linux. The existing `slint_ui` and e2e cases cover that nothing moved.
2. **macOS policy and reopen.** `LSUIElement`, the policy switch, the reopen method. Unit tests for what can run anywhere (the "another window is still visible" decision); the rest is Step 10.
3. **`activation.rs`.** The per-user socket name, the exchange, the hidden-launch silence, the first instance's show and signal. Replace the early return in `app.rs` with the client side.
4. **The version notice.** A component in `ui/overlays/`, shown by the first instance when the versions differ, with its two buttons wired.
5. **Autostart, Windows backend.**
6. **Autostart, Linux backend**, with the Homebrew mapping and the sway, i3 and Hyprland check.
7. **Autostart, macOS backend.**
8. **The switch** in `main.slint` and its callbacks, the status line on a failed write, the hint for another path and for the system's disable. The IPC commands.
9. **The tray menu** (decision 9): the title prefix for "Auto-refresh", the "Start on login" item, and the shared write and read with the settings switch. This step fixes the missing checkmark on its own and can go first if the user wants it released earlier.
10. **Validation in the guests and on the runner** (see Tests). This step runs `cargo xtask vm` and the macOS workflow, which the implementer's constraints forbid, so it is done by the supervising session with the user, and its results go into the validation record.
11. **Linux tray wait** (decision 6). After Step 10's Linux login checks, because those show how often the race happens; it is implemented either way.
12. **Docs.**

## Tests

Assert behavior, not constants: step over `TRAY_HOST_WAIT` and the connect retry by name.

### Unit

1. Windows command line: a path with spaces and one without round trip through the writer and the C runtime parser to the same executable and arguments; an embedded quote is refused.
2. StartupApproved: absent, `02` plus zeros, `03` plus a FILETIME, `03` plus zeros, `06`, `07`, and a value shorter than one byte each read as the right state.
3. Windows enable and disable against a scratch key under `HKCU\Software\SunlitEarthTest-<pid>`, removed afterwards: enable over a `03` leaves the entry on; disable removes both values; another executable's entry reads as on with that path.
4. Desktop entry: `Exec` quoting for a plain path, spaces, `$`, `` ` ``, `"`, `\` and `%`, checked against a small parser written from the specification, not against strings copied from the writer.
5. Desktop entry reading: missing file, `Hidden=true`, `Hidden=false`, `X-GNOME-Autostart-enabled=false`, both keys, and a file another tool wrote with extra keys and comments.
6. `XDG_CONFIG_HOME` set, unset and blank.
7. The Homebrew mapping on a fabricated `Cellar` and `opt` tree in a temp directory: mapped when `opt` resolves to the same file, not mapped when it does not or is missing, and a path with no `Cellar` component left alone.
8. `XDG_CURRENT_DESKTOP`: `sway`, `i3`, `Hyprland`, `ubuntu:GNOME`, `KDE`, empty.
9. The plist: escaping of `&`, `<` and `"` in a path, the `open` form for a bundle path, the direct form for a bare binary, the refusal for a translocated path.
10. The exchange: encoding round trip with a path holding spaces, `=`, `%` and non-ASCII; the version decision; a hidden launch does not connect.
11. The per-user socket name differs between two user ids and is stable for one.

### `slint_ui`

12. The switch shows the state it is given, including "turned off by the system" and "starts another path", and is disabled with the `exec` hint for an unsupported session.
13. The version notice appears for differing versions only, and "Close" dismisses it.
14. The tray's "Auto-refresh" title carries the mark exactly when auto-refresh is on, follows a change made in the settings window, and a change from the tray reaches the window. "Start on login" follows the state it is set to and is disabled for an unsupported session. If the testing backend cannot instantiate `TrayIcon`, these move to unit tests of the Rust side and that is a departure.

### e2e (`crates/sunlit-app/tests/e2e.rs`)

15. `test_single_instance_second_exits` becomes: A is started hidden in tray mode, B is started with the same instance name, B exits 0, A prints `SIGNAL:activated version_differs=false`, and A's window is visible afterwards.
16. B started with `--tray-start hidden` beside a running A exits 0 and A prints no `activated` signal within a few seconds.
17. Autostart round trip over IPC: enable, status reads `on` with this executable, disable, status reads `off`. It writes the real entry, so it runs only when `SUNLIT_EARTH_E2E_AUTOSTART` is set, which the generated guest jobs set and nothing else does, and it removes the entry whatever happens. Without the variable it skips and says why.

### Step 10, in the guests and on the runner

In every session below, right-click the tray icon once with auto-refresh on and once with it off, and once with autostart on and off, and record that the mark is there and readable (Windows, KDE, XFCE, the macOS runner's interactive session).

The e2e process does not survive a logout, so these are driven from the host and recorded by hand in the validation record.

- **Linux**, for KDE on X11, KDE on Wayland, GNOME, XFCE and Cinnamon: `cargo xtask vm up linux --desktop <d>`, enable over IPC, `cargo xtask vm ssh linux "sudo systemctl restart sddm"`, wait for the new session's `session.env`, then check that the app runs with `--tray-start hidden`, whether its window is visible (hidden where there is a tray, shown on GNOME), and how long the log says it waited for the tray. Then switch the entry off the way that desktop's own settings do (`Hidden=true`, or `X-GNOME-Autostart-enabled=false` on Cinnamon), confirm the status reads `off`, restart sddm, and confirm the app did not start. sway and i3: the switch reads unsupported. The second-launch cases on KDE Wayland and GNOME Wayland, noting what the compositor does (focus, attention, or a notification).
- **Windows**: enable, reboot the guest with `shutdown /r /t 0` over `vm ssh`, confirm the app runs hidden in the console session after the autologon. Write `03` plus zeros into StartupApproved with `reg add`, confirm the status reads `off-by-system`, reboot, confirm it did not start. Enable from the app, reboot, confirm it starts. Second launch from a process in the console session: the window comes to the front rather than flashing. Note the working directory the Run-key process got.
- **macOS**, in `macos-build.yml`'s `build-and-e2e` mode, added as steps of that job: install the bundle to `/Applications`, enable over IPC, quit the app, `launchctl bootstrap gui/$UID` the plist, and confirm the app runs with `--tray-start hidden` and that `lsappinfo info` reports it as a UI element with no Dock icon. `open -a "/Applications/Sunlit Earth.app"` must produce `SIGNAL:activated` through the reopen path. Log `sfltool dumpbtm` so the Login Items name of an `open` agent is on record. One interactive session to look at the Dock icon and the menu bar after hiding and showing a few times.

## Docs

- `docs/platforms.md`: a section per platform on the autostart entry, what reads it, what turns it off, and what was observed in Step 10; the second launch; the macOS Dock icon and reopen.
- `docs/architecture.md`: the three new modules, the activation socket next to the IPC bullet, the `autostart-*` IPC commands, and the second launch where `app.rs`'s single-instance check is described.
- `docs/usage.md`: "Start on login" with the per-OS note on where else it can be switched off, the `exec` line for sway, i3 and Hyprland, and that the macOS entry is named after `open` or the executable in Login Items if Step 10 shows that.
- `docs/testing.md`: the new cases and `SUNLIT_EARTH_E2E_AUTOSTART`.
- `docs/roadmap.md`: a done entry; close the "autostart entry has to point at the paths that survive an upgrade" item; correct the macOS `LSUIElement` item and leave the teardown; add an item for reading the Login Items switch on macOS, one for Wayland activation needing a token at window creation in Slint, one for ksni's `assume_sni_available` in Slint, and one for Slint's tray backends ignoring `checked`, with the title prefix to remove once a release draws the mark.
- `docs/architecture.md`'s `tray.rs` bullet: the menu, its two state items and why their titles carry the mark.
- `README.md` is not edited.

## Gates

Run from the worktree root, in this order, and report the real output.

```bash
cargo fmt --check
cargo clippy --all-targets
cargo unit
cargo test -p sunlit-earth --test slint_ui
cargo test
```

`cargo clippy --all-targets` must be clean (pedantic is on; CI builds with `-D warnings`). `cargo test` is the last word. A skipped test is reported as skipped. The macOS and Linux code must also compile: the implementer cannot build either from this host without WSL, so a `ci.yml` dispatch on the branch, run by the supervising session, is the gate for those before Step 10.

## Constraints for every agent

- Work only in `C:\Workspace\rustrover\sunlit-earth\.worktrees\autostart` on branch `feature/autostart`. Never touch the main checkout or the other worktrees under `.worktrees/`.
- Never run `cargo xtask vm`, `cargo xtask e2e`, `cargo xtask dist`, `cargo e2e`, anything elevated, anything through WSL, or any golden regeneration.
- Never write the real autostart entry of the host: not the `Run` key, not `~/.config/autostart`, not `~/Library/LaunchAgents`. Tests use scratch keys and temp directories.
- Do not edit `README.md`. Do not push. Do not create or touch PRs. Do not amend or rewrite commits.
- Commits: on the run branch, in reasonable chunks, with plain messages. No `Co-Authored-By` trailer or any other co-author line. Do not override the git identity.
- Code comments only where the code cannot be made clear without them, and never about history, tickets, or why a test exists. Match the surrounding comment density. Docstrings use the Sphinx style the crate uses (`:param:`, `:returns:`).
- Writing style in docs, comments, logs, UI text and commit messages: American English, no em dashes, no en dashes, avoid dashes in sentences generally, no "genuinely", "delve", "crucial", "honestly". Bold and italics only for real callouts.
- Departures from this plan are numbered, sequentially from 1, and recorded in the section below with a paragraph of reasoning.

## Departures

1. **Step 11 was done before Step 10.** The implementer cannot run the guests or the runner, and the tray wait is implemented either way; Step 10's Linux login checks still decide whether `TRAY_HOST_WAIT` is long enough, and they now test the wait rather than its absence.

2. **The tray wait asks `NameHasOwner` every 100 ms rather than waiting for `NameOwnerChanged`.** zbus's blocking signal iterator has no timeout, so waiting for the signal with one takes a thread that stays blocked on the session bus, holding its connection, for the rest of a run in which the watcher never comes (GNOME without the extension, the very case the timeout exists for). Async zbus with a timer would avoid the thread but means taking `async-io` and `futures-util` as direct dependencies, writing async code that cannot be compiled on the implementer's host. A hundred calls of a cheap method over ten seconds, on the main thread before the event loop starts and only for a hidden start, is a bounded cost with no thread and no state left behind. The decision (`wait_for` in `tray.rs`) is unit-tested with a fake sleep on every platform.

3. **On Linux the activation socket is a file in `$XDG_RUNTIME_DIR`, not an abstract name.** Decision 5 has every platform use a name with a per-user part. Validation round 1 found that a Linux abstract socket has no permissions, so another user on the host could connect, bring the window up and write any text into the version notice, and could take the name first. `interprocess` binds a filesystem socket with the same calls (`GenericFilePath` in place of `GenericNamespaced`), and `$XDG_RUNTIME_DIR` is the user's own folder with mode 0700, so on Linux the socket is `<instance>-activation.sock` there and only its user can reach it. Departure 5 adds `/run/user/<uid>` as a second folder; only where neither folder exists is the abstract name of decision 5 used, and there any user on the host can reach the socket or take the name first. Windows and macOS keep their names. The hazards that are left are written down in `docs/platforms.md`.

4. **The hint for Hyprland names `exec-once =`, not `exec`.** Decision 3 gives all three sessions the same `exec <path> --tray-start hidden` line, but Hyprland's config starts a program at login with `exec-once = <command>`, and a user who pastes the `exec` line gets an error. sway and i3 keep `exec`.

5. **On Linux the running instance binds in `/run/user/<uid>` when `XDG_RUNTIME_DIR` is missing, and the abstract name is never a fallback beside a file.** Round 2 had the instance bind one address (the `$XDG_RUNTIME_DIR` file, else the abstract name) and a second launch try the abstract name last. Validation round 3 found that last try reaches whoever squatted the name while the first instance's file is not listening yet, before `activation::bind` or after a failed one, and hands that user the launch's version, path and token. Dropping the name from the list whenever a folder exists closes that, but alone it would strand an instance started without the variable, which bound the name, from a launch in the same environment, which would now look only in `/run/user/<uid>`. So the instance and the launch share one list, `activation::addresses`: the file in `$XDG_RUNTIME_DIR`, then the file in `/run/user/<uid>` when that folder exists, and the abstract name only when neither does; the instance binds the first entry that takes the socket and the launch tries them all. `/run/user/<uid>` is made by logind for that user with mode 0700, so a file there is as private as one in `$XDG_RUNTIME_DIR`. Two cases are left unreachable and are written down in `docs/platforms.md`: an instance started with neither folder and a launch from a session with one, and an instance whose `XDG_RUNTIME_DIR` is a folder other than `/run/user/<uid>` with a launch that does not see that same value.

## Validation rounds

### Round 1

Reviewed d00fc1a..2cca179: 0 MAJOR, 6 MINOR, and one side effect. All seven are fixed.

1. **An older version holding the bare Linux guard name went unnoticed** (`tray.rs`), so the first start after an upgrade could run a second full instance. Fixed in 8bb5982: on Linux the bare name is asked for as well; held elsewhere, it is an older version, and the launch exits quietly with a log line, since there is no activation socket to ask; taken, it is kept, so an older version started later finds this one. The decision is the pure `decide`, unit-tested on every platform, and `docs/platforms.md` describes the transition. Round 2 changed the keeping of the bare name (N1).
2. **Activation answer threads were unbounded and the Linux socket was open to every user** (`activation.rs`). Fixed in 9dd40be: at most four connections are answered at once and the rest are closed unanswered, and a read deadline of five seconds is set where the platform's sockets take one (Windows named pipes do not). On Linux the socket is now a file in `$XDG_RUNTIME_DIR`, departure 3, with the abstract name kept as the fallback; the cross-user and squatting hazard that remains is in `docs/platforms.md`.
3. **String continuations and line escapes lost in the new e2e messages.** Fixed in 63ef76f, the third occurrence in the shared spawn helper included.
4. **A comment explaining why a test exists in the xtask's e2e tests.** Removed in 63ef76f.
5. **Hyprland's hint said `exec`.** Fixed in 278f0d7: it says `exec-once = <path> --tray-start hidden`, and sway and i3 keep `exec`; departure 4, recorded in 9dd40be. `usage.md`, `platforms.md` and the roadmap entry follow.
6. **macOS Dock icon after About closes, and the reopen's timing** (`window_state.rs`). Fixed in c9481d3: closing the About window looks, on the next turn, at whether any window of the app is still on screen and drops the Dock icon if none is (`drops_dock_icon_after_close`, unit-tested); the reopen goes through the same zero timer as a second launch.
7. **A second launch during the Linux tray wait could time out.** Fixed in af0341c: the activation socket is answered from before the wait, so the exchange completes at once; the activation itself runs from a zero timer once the event loop starts, after the deferred hide of a hidden start, and the tray's creation in between hides nothing.

### Round 2

Reviewed at 9bbf95f: all seven round 1 fixes verified, and 1 MAJOR and 1 MINOR found. Both are fixed.

- **N1 MAJOR: keeping the bare Linux guard name turned other users away** (`tray.rs`, `acquire_single_instance`). Abstract names are shared by every user on the host, so a new instance that held `sunlit-earth-app` made another user's new instance read it as an older version and exit. Fixed in 072cdd1: the bare name is asked for through `probe` and let go at once, never kept. An older version started after a new one now runs beside it, which is accepted for the transition, and a false "older version" exit remains possible only while another user on the host runs a version from before this change; `docs/platforms.md` says both.
- **N2 MINOR: a second launch from a shell without `XDG_RUNTIME_DIR` could not reach the running instance** (`activation.rs`). Fixed in c920bf1: the running instance still binds one address, and a second launch tries `client_addresses` in order until one connects within `CONNECT_RETRY`: the file in its own `$XDG_RUNTIME_DIR` when set and absolute, the file in `/run/user/<uid>` when that folder exists and is another one, then the per-user abstract name. The list is a pure function of the variable, the uid and a folder probe, unit-tested on every platform, and `docs/platforms.md` gives the order beside departure 3's socket file. Round 3 (item 1) made it the list the instance binds from as well and dropped the abstract name wherever a folder exists.

### Round 3

Reviewed 9bbf95f..2574193: both round 2 fixes verified, 0 MAJOR, 5 MINOR, and one pre-existing hazard for the roadmap. All five are fixed.

1. **The abstract name was still tried beside the socket file** (`activation.rs`, `connect` with `client_addresses`). While the first instance's file was not listening yet, or after a failed bind, a second launch fell through to the abstract name and handed its version, path and token to whoever had squatted it, and departure 3 said only the user could reach the socket. Fixed in cb4a475, departure 5: `activation::addresses` is one list for both sides, the file in `$XDG_RUNTIME_DIR`, then the file in `/run/user/<uid>` when that folder exists, and the abstract name only when neither does. The instance binds the first entry that takes the socket, so a launch without the variable still reaches the desktop session's instance through `/run/user/<uid>`. Departure 3's sentence and `docs/platforms.md` now say which sessions still use the name, what another user can do there, and the one case left unreachable.
2. **The test for a runtime folder that is `/run/user/<uid>` asserted at most one file**, which passed with none and meant nothing on Windows. Fixed in cb4a475: it asserts the exact one-entry list and runs under `cfg(unix)`, where the path is absolute; the other address tests assert exact lists too.
3. **An unreachable instance named only the last address's error.** Fixed in cb4a475: `connect` keeps each address's last error and `describe_failures` joins them, and `bind` reports every address it could not take the same way; unit-tested.
4. **`docs/architecture.md` described the client connecting to one address.** Fixed in cb4a475: it names `addresses`, its order, and that the launch retries the whole list.
5. **Two users' new launches could race on the bare legacy name** (`tray.rs`), and the loser exited saying an older version runs. Fixed in 0fc3b26: `probe_legacy` asks once more after `LEGACY_RECHECK`, 50 ms, when the name is held, and only a name still held then counts as an older version. It takes the probe and the sleep as values and is unit-tested; `docs/platforms.md` says so.

Roadmap only: `single-instance` 0.3.3 creates its Linux guard socket without `SOCK_CLOEXEC`, so a `swaybg` the app spawns may inherit it and hold the name after the app quits. Recorded as unverified with the two candidate fixes in de16229.

### Round 4

Reviewed 2574193..6048059: all five round 3 fixes and the roadmap item's wording verified, 0 MAJOR, 1 MINOR. The pairings of instance and launch environments, a first candidate that fails to bind, stale socket files, the unchanged Windows and macOS lists, the e2e instance names, and the legacy re-probe on the normal startup path were checked without a finding.

1. **The docs named one unreachable case where there are two** (`docs/platforms.md`, departure 5). An instance whose `XDG_RUNTIME_DIR` is a folder other than `/run/user/<uid>` binds its file there, and a launch that does not see the same value never looks in it. Not a regression, since round 2's list missed it too. Fixed by naming the second case in both places.
