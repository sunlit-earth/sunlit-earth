# Holding the auto-refresh back while another application is full screen

## Problem

On a Linux desktop a video playing full screen in VLC stuttered once a minute, each time the auto-refresh set a new wallpaper. Setting a wallpaper is a render at native resolution, a readback, a PNG and a reload in the desktop shell, and none of it is worth anything while a full screen application hides the desktop. The app should hold its automatic refresh back while another application is full screen, including the borderless kind, and refresh once the screens are clear.

The research is in [2026-11-23-fullscreen-pause-research.md](2026-11-23-fullscreen-pause-research.md).

## What exists today

- The auto-refresh is the engine's `auto_refresh` schedule; `tick` calls `publish_wallpaper` when it falls due. `recheck_displays` republishes after a layout change, and `reexport` makes a wallpaper again once the tiles it went out without are resident. `RenderWallpaperNow` is the user's own publish.
- `wallpaper_owed` holds one owed publish while textures, the first cloud answer or tiles are awaited.
- `EngineConfig` injects `Clock`, `CloudSource` and `WallpaperSink`. The engine tests drive the loop with a `MockClock` and recording sinks.
- `sunlit-earth displays` prints the monitors, the plan and on Linux the `setter:` line.
- Nothing in the app looks at other processes' windows.

## Decisions

Decisions 1 and 2, the idle inhibitor of decision 9, the held layout change republish of decision 3 and the KWin route of decision 9 were settled with the user on 2026-11-23.

### 1. What counts

The desktop is covered when, on any monitor, a window of another process covers the whole monitor, or the platform reports a visible window of another process as full screen. Geometry is the main test because it is what catches a borderless window; the full screen state is a second way to say yes. Where a rule is in doubt it leans towards covered: a false positive holds back a wallpaper nobody can see, a false negative is the stutter.

Windows of this process never count, so the app's own full screen preview changes nothing.

### 2. One covered monitor holds everything

A cover on any monitor holds the whole publish, whatever the display mode. The desktops repaint every screen in one operation, so the work that disturbs the full screen application does not depend on which screen it is on. A publish per screen is not part of this plan.

### 3. What is held and what is not

Held:

- The auto-refresh when it falls due.
- The republish after a layout change (`recheck_displays`): a game that switches the monitor's mode as it enters full screen fires exactly this.
- The `reexport` after tiles arrive.

Not held:

- `RenderWallpaperNow`: Set as Wallpaper, Refresh Now and the IPC command. What the user asks for happens.
- The publish at exit, which pays only `wallpaper_owed` and a pending request. A refresh held by a cover is neither, so quitting while covered does not publish it.

### 4. The engine side

- `EngineConfig` gains `cover: Option<Arc<dyn CoverProbe>>`. `None` is today's behavior and the default of `EngineConfig::headless`.
- `CoverProbe::probe(&self) -> Coverage`, with `Coverage` one of `Covered(Cover)`, `Clear` or `Unknown(String)`. A `Cover` names the window as well as the platform can (application, class or app ID, process ID), the monitor where known, and the signal: `FullScreenState`, `Geometry`, `ExclusiveDirect3D`, `PresentationMode` or `IdleInhibited`.
- An automatic publish that falls due asks the probe first. `Covered` sets `cover_held` and a recheck `COVER_RECHECK` (10 s) ahead and publishes nothing. While `cover_held` stands, `tick` asks again at each recheck. `Clear` or `Unknown` publishes as today.
- Once a held publish goes out, the auto-refresh countdown starts again from that publish, so a cover lifting a few seconds before the next refresh does not publish twice.
- A `RenderWallpaperNow` while held publishes and clears `cover_held`, since it publishes the same wallpaper.
- Logging: info once when a publish is first held, naming the cover, and once when it is released, with how long it was held. Each probe at debug level with its duration. `Unknown` is logged once per distinct reason at info, then at debug.
- The probe runs on the engine thread. Every route has a timeout of its own (decision 9) so a probe cannot stall the loop for more than a quarter of a second.

### 5. Unknown publishes

A probe that cannot tell does not hold anything: the app behaves as it does today, and says why in the log and on the `displays` line. A session this plan has no route for (Mutter without XWayland windows and without the session manager, a Wayland compositor with neither the wlr protocol nor KWin) is `Unknown`.

### 6. The setting

`AppConfig` gains `auto_refresh_skip_fullscreen`, `true` by default. A new key with a default needs no migration step. It is read at startup and sets `EngineConfig::cover`; there is no control until the settings window exists, which will add an `EngineCommand` to switch it at run time. `docs/usage.md` describes the key.

### 7. Windows

- Enumerate every top-level window with `EnumWindows`, not only the foreground one.
- Skip a window that is not visible, minimized, cloaked (`DWMWA_CLOAKED`), a tool window, layered and transparent to input (`WS_EX_LAYERED` with `WS_EX_TRANSPARENT`, the overlays), of this process, or of the shell's classes: `Progman`, `WorkerW`, `Shell_TrayWnd`, `Shell_SecondaryTrayWnd`, `NotifyIconOverflowWindow`, `Windows.UI.Core.CoreWindow`, `MultitaskingViewFrame`, `XamlExplorerHostIslandWindow`, `WindowsDashboard`.
- A window covers a monitor when its `GetWindowRect` contains the monitor's `rcMonitor`. Containment rather than equality, so a borderless window a few pixels larger still counts.
- `SHQueryUserNotificationState` answering `QUNS_RUNNING_D3D_FULL_SCREEN` or `QUNS_PRESENTATION_MODE` is covered too, without a monitor. `QUNS_BUSY` is not used.
- The process is per monitor DPI aware v2 (`ensure_dpi_awareness`, called before the first probe), so both rectangles are physical pixels.

### 8. macOS

- `CGWindowListCopyWindowInfo` with on-screen windows only and no desktop elements. Skip windows whose layer is not 0 or whose process is this one. Read only the process ID, the layer and the bounds, never a name, since names need the Screen Recording permission.
- A window covers a display when its bounds contain `CGDisplayBounds` horizontally and at the bottom, and its top lies at most `NOTCH_TOLERANCE` (50 points) below the display's top. That admits a full screen window under a notch, and also a zoomed window under a visible menu bar with the Dock hidden, which is the harmless kind of false positive.
- CoreGraphics only, like `display/macos.rs`, so the probe runs off the main thread.
- If the keys are missing without the permission, the answer is `Unknown` with that reason, not `Clear`.

### 9. Linux

The probe asks every route that applies to the session and combines them: any `Covered` wins, `Clear` needs every route clear, otherwise `Unknown`.

- **X11**, whenever `DISPLAY` is set, so also for XWayland windows in a Wayland session. Over x11rb: the RandR monitors; `_NET_CLIENT_LIST_STACKING`; for each client, sent before any reply is read, `_NET_WM_STATE`, `_NET_WM_DESKTOP`, `_NET_WM_WINDOW_TYPE`, `_NET_WM_PID`, the attributes and the geometry, translated to root coordinates. Skip hidden windows, windows on another desktop than `_NET_CURRENT_DESKTOP` (all desktops excepted), desktop and dock windows, unviewable windows and this process's. Covered by `_NET_WM_STATE_FULLSCREEN` or by geometry containing a monitor. Then the root's viewable override-redirect children by geometry. A timeout of 250 ms on the whole.
- **wlroots compositors**, where the Wayland globals have `zwlr_foreign_toplevel_manager_v1`: a connection of its own for the probe, the manager bound at version 2 or later, one round trip, and every toplevel whose state has full screen and which has entered an output (the output's name from `wl_output`). Then stop the manager and disconnect. Through `wayland-protocols-wlr`, a new dependency.
- **KDE Plasma on Wayland**, where `org.kde.KWin` is on the session bus and the wlr global is not: a KWin script. The probe writes a script with a per-probe token into the cache directory, calls `loadScript` and `run`, and waits up to 250 ms for the script's `callDBus` to a small object the probe exports on the app's existing session bus connection; then `unloadScript`. The script reports, for windows that are normal, not minimized, on the current desktop and of another process, those with `fullScreen` or a `frameGeometry` that contains their `output`'s geometry. If step 0 finds that a one-shot script costs KWin a frame, the script stays loaded and reports changes instead, and the probe reads the last report.
- **GNOME on Wayland**, and any session where `org.gnome.SessionManager` is on the bus and no route above has window state for native Wayland clients: `IsInhibited(8)`. An idle inhibitor counts as covered with the signal `IdleInhibited`, and the `Cover` names the inhibiting application from `GetInhibitors`, so a wallpaper that stops refreshing says why. Accepted by the user on 2026-11-23 as a stand-in: it also fires for a video in a window, and an application that keeps the screen awake for good holds the refresh for as long as it does.

### 10. Saying what it sees

- `sunlit-earth displays` prints a `fullscreen:` line on every platform: `none`, the cover (`fullscreen: vlc (pid 4711) on DP-1, full screen state`), or `unknown; <reason>`, with the probe's duration. That line is what the e2e cases and bug reports read.
- IPC gains `enter-full-screen` and `leave-full-screen`, answered by `SIGNAL:` lines once the window has changed, so the e2e cases can make the app's window the cover for a second process.

### 11. Where the code lives

`sunlit-core/src/cover/`, beside `display/` and `desktop/`:

- `mod.rs`: `CoverProbe`, `Coverage`, `Cover`, `Signal`, `system_probe()`, and the decision as pure functions over gathered facts: a list of candidate windows (process ID, rectangle, flags) and monitor rectangles in the same coordinate space in, a `Coverage` out. Tested on every platform, the way `desktop::choose` is.
- `windows.rs`, `macos.rs`: gather the candidates.
- `linux/mod.rs` with the route choice and the combination, `linux/x11.rs`, `linux/wlr.rs`, `linux/kwin.rs`, `linux/session.rs` for the idle inhibitor.

Each route compares rectangles only within its own coordinate space, never against `display::Monitor`, which on Linux comes from `xrandr` and can be skewed under Wayland scaling.

### Not chosen

- **`QUNS_BUSY`**: false positives documented by Rainmeter, and no word on monitors.
- **The foreground window only**: misses a video on the second monitor.
- **Event hooks** (`SetWinEventHook`, `ABN_FULLSCREENAPP`, a resident wlr listener): a poll when a publish is due costs less than a millisecond on Windows and needs no thread, message loop or appbar.
- **A GNOME Shell extension**: exact, but something to install and to keep working across GNOME releases.
- **The idle inhibitor everywhere**: it answers a different question and can hold the refresh indefinitely, so it is used only where no window state exists.
- **`org_kde_plasma_window_management`**: needs Plasma 6.8 or a `.desktop` file declaring it. It becomes the KDE route once 6.8 is the oldest Plasma the app supports.

## Implementation steps

0. **Checks before any code.**
   - In the Linux guest under KDE Wayland: load, run and unload a script that calls back over D-Bus, by hand with `qdbus`, and time it; see whether `callDBus` reaches a unique bus name; watch KWin's frame timing during a hundred loads. This decides the one-shot or the resident script.
   - In the Linux guest under KDE X11 and XFCE with picom: list the root's viewable children and see whether the compositor's overlay window is one of them.
   - That `sunlit-earth displays` runs as its own process beside a running app, so the app's own full screen window is another process's to it.
   - That the GNOME image has `gnome-session-inhibit`, which the e2e case uses.
1. **The decision.** `cover/mod.rs` with the types and the pure functions of decisions 1, 7, 8 and 9, and their unit tests.
2. **The engine.** `EngineConfig::cover`, `cover_held` and the recheck, the gate on the three automatic publishes of decision 3, the countdown restart, the logging. `AppConfig::auto_refresh_skip_fullscreen` and `system_probe()` wired in the app.
3. **Windows.** `cover/windows.rs`; the `windows-sys` features it needs.
4. **X11.** `cover/linux/x11.rs` over x11rb.
5. **wlroots.** `cover/linux/wlr.rs`; add `wayland-protocols-wlr`.
6. **KDE Wayland.** `cover/linux/kwin.rs` in the shape step 0 chose.
7. **GNOME.** `cover/linux/session.rs` over zbus.
8. **macOS.** `cover/macos.rs`; the `objc2-core-graphics` and `objc2-core-foundation` features it needs.
9. **Diagnostics.** The `fullscreen:` line in `displays`, the two IPC commands and their signals.
10. **e2e** (Tests below).
11. **Licenses.** `cargo xtask bake licenses`, for `wayland-protocols-wlr`.
12. **Docs** (below).
13. **Checks on the platforms.**
    - Windows desktop: VLC full screen, a browser video full screen, a game in borderless windowed mode, each on the primary and on the second monitor; the auto-refresh at one minute is held and goes out once each is left. A maximized window with the taskbar shown does not hold it. The NVIDIA or Xbox Game Bar overlay does not hold it.
    - Linux guest: the e2e case in KDE X11 and Wayland, GNOME Wayland, XFCE, Cinnamon, sway and i3.
    - The user's own Linux desktop: VLC full screen with a one minute refresh, the stutter gone.
    - macOS runner: the e2e case. The notch tolerance and the permission question go to `docs/macos-testing.md` for a tester.

## Tests

### Unit, `sunlit-core`

- A window whose rectangle equals, contains, or falls short of a monitor by one pixel on each edge; on the second of two monitors; spanning both.
- Each Windows skip rule: hidden, minimized, cloaked, tool window, transparent overlay, this process, each shell class. A maximized window short of the taskbar does not cover.
- The Direct3D and presentation states cover without a monitor; `QUNS_BUSY` alone does not.
- Each X11 skip rule: `_NET_WM_STATE_HIDDEN`, another desktop, all desktops, desktop and dock types, unviewable, this process. The full screen state covers at any size; an override-redirect window covers by geometry.
- A wlr toplevel full screen with an output covers; full screen without one, or with an output and not full screen, does not.
- The macOS rule: a window under the notch tolerance covers, one lower does not, layer other than 0 does not.
- The combination: covered wins over unknown, clear needs every route clear.
- The route choice for fabricated sessions: X11 alone, sway with XWayland, KDE Wayland, GNOME Wayland, a Wayland compositor with nothing.

### Engine, `sunlit-core`

With a `MockClock`, a recording sink and a scripted probe:

- A covered probe holds the auto-refresh; nothing is published until the probe answers clear at a recheck, then exactly one publish.
- The probe is asked again at `COVER_RECHECK` and not between.
- `Unknown` publishes as today.
- `RenderWallpaperNow` while held publishes and ends the hold.
- The layout change republish and the reexport are held the same way.
- After a held publish the next auto-refresh is a full interval later.
- No probe configured is today's behavior, and quitting while held publishes nothing.

### e2e

`test_another_full_screen_window_is_seen`: start the app, `enter-full-screen`, run `sunlit-earth displays` and expect a `fullscreen:` line naming the app's process and its monitor; `leave-full-screen`, run it again and expect `none`. On Windows that is the geometry route, on X11 the state route, on sway the wlr route, on KDE Wayland the KWin route. On GNOME Wayland the case holds `gnome-session-inhibit --inhibit idle` around the first query instead and expects the inhibitor named. Where the session has no route the case expects `unknown` and says so.

## Docs

- `docs/architecture.md`: the engine section gains the cover probe and the held automatic publishes; the environment knobs table is unchanged.
- `docs/platforms.md`: a section "Another application in full screen" with the route per platform and session, and the evidence per row in the file's three tiers.
- `docs/usage.md`: `auto_refresh_skip_fullscreen`.
- `docs/testing.md`: the e2e case and what each session runs.
- `docs/macos-testing.md`: what a tester should try.
- `docs/roadmap.md`: the entry, and the switch for the settings window.

## Gates

`cargo fmt --check`, `cargo clippy --all-targets`, `cargo test`, the e2e case in each Linux session of the guest and on the Windows desktop, and the user's own check with VLC.

## Departures

1. **The KWin route stays one-shot, and rounds KWin's geometry.** Step 0 ran in the Linux guest under Plasma 6.3.6 on Wayland. Loading, running and unloading a script that calls back with `callDBus` to the caller's unique bus name works, so no well-known name is needed. Five probes with no window open took 4.4 ms median from `loadScript` to `unloadScript`; a hundred probes at ten a second beside a full screen GTK 4 client took 13 ms median and 30 ms at most, and the client's frame intervals did not move (median 25.7 ms against 25.4 ms, 99th percentile 40.5 ms against 40.0 ms, intervals over 50 ms 6 of 913 against 5 of 612 without probes; the guest composites in software at about 40 frames a second, so a lost frame of a few milliseconds would hide in that noise). That is no measurable cost at one probe per ten seconds, so the resident script was not built. The same run showed that KWin's geometry is fractional under scaling: the full screen client's `frameGeometry` was 1828.57 by 1028.57 on an output of 1829 by 1029. A window's edges are rounded to the nearest pixel before the containment test, which the plan did not say.

2. **The compositor overlay window was checked where the image allows.** The image has no picom, and xfwm4's compositing is off in the XFCE session (`use_compositing` false, no `_NET_WM_CM_S0` owner), so the XFCE check is of an uncomposited session. Under KDE X11 (kwin_x11) and XFCE the root's viewable children were the window manager's frames, which are not override-redirect and are skipped by design, and override-redirect windows of 1 by 1, 5 by 5 and one pixel wide edge strips; none covered a monitor, and no overlay window was among them. Cinnamon, whose Muffin always composites, is checked with the e2e case.

3. **On wlroots compositors the app's own window counts.** The wlr foreign toplevel protocol carries an app ID and no process ID, so decision 1's rule that windows of this process never count cannot be applied there: the app's own full screen preview holds its auto-refresh under sway. That is the harmless kind of false positive, and it is what lets the e2e case see the app's window from the `displays` process on that route.

4. **The KWin route exports its report object on a session bus connection of the probe's own.** `sunlit-core` has no long-lived session bus connection to share (the tray's lives in the app crate), so the Linux probe opens one on first use, keeps it for the process, and uses it for the bus name checks, the KWin route and the session manager route alike.

5. **One deadline for the whole Linux probe rather than one per route.** The session's facts, read once and kept, and every route each run on a thread of their own, and the probe waits for all of them under one 250 ms deadline, so a probe of three routes cannot take three quarters of a second. A route that has not returned is not started again on top of itself: the next probe answers unknown for it until it does.

6. **Two small rules the plan left open.** An automatic publish that falls due while a publish is already owed (waiting for textures, the cloud layer or tiles) joins it without asking the probe, since the owed one goes out regardless. A `RenderWallpaperNow` that ends a hold restarts the auto-refresh countdown as a released hold does, since the held publish went out with it.

7. **An input-only window is never a cover, which closes the check of departure 2.** The first e2e run under Cinnamon reported the desktop covered by an override-redirect root child of 1920 by 1080 with neither a class nor a process ID, before and after the app's full screen. It is Muffin's guard window (`WM_NAME` "mutter guard window"), an `InputOnly` window over the whole screen that Mutter and its forks keep mapped below every client, so GNOME on X11 and Xwayland under GNOME have one too. An input-only window draws nothing, so the X11 route now treats it as not viewable, client or override-redirect alike. Cinnamon's other root children were the 1 by 1 windows departure 2 saw elsewhere and Nemo's desktop window, a client of type desktop; no compositor overlay window was among them.
