# Research: Holding the auto-refresh back while another application is full screen

## Question

On a Linux desktop a video playing full screen in VLC stuttered once a minute, each time the auto-refresh set a new wallpaper. The app should skip the automatic refresh while another application is full screen, on by default, with a switch once the settings window exists. The detection has to work on every supported platform and catch the borderless kind of full screen as well: a window without decorations sized to a whole monitor, which is what browsers, video players and most games in "borderless windowed" mode use. This document collects what each platform lets an ordinary process learn about other processes' windows, what mature projects do, and what the checks cost. The plan is [2026-11-23-fullscreen-pause-plan.md](2026-11-23-fullscreen-pause-plan.md).

The code was read on `main` at `9ca90b9`. The platform facts were read from the primary sources listed at the end on 2026-11-23, by two research passes whose claims are marked here as read in a source or as inferred. Nothing was run except the timing on the Windows development desktop that says so. What could not be confirmed is under Open questions.

## What the app has today

- The auto-refresh is one `Schedule` in the engine loop, `auto_refresh`, set from `auto_refresh_enabled` and `auto_refresh_interval_minutes` and changed at run time by `EngineCommand::SetAutoRefresh`. When it falls due, `tick` calls `publish_wallpaper` (`engine/mod.rs`, the block after the metrics sample).
- Three other publishes start without the user: `recheck_displays` republishes after a layout change when this process put the current wallpaper up (`engine/publish.rs`), a publish that went out with tiles missing is made again once they are resident (`reexport`), and the engine publishes what is owed as it stops (`publish_wallpaper_at_exit`). `RenderWallpaperNow` (Set as Wallpaper, Refresh Now, the IPC command) is the user's own.
- A publish can already be held back: `wallpaper_owed` keeps one owed publish while the cubes, the first cloud answer or the tiles are awaited, and `tick` comes back to it every 50 ms. One request is remembered, not a queue.
- `EngineConfig` injects the `Clock`, the `CloudSource` and the `WallpaperSink`; the engine tests drive the loop with a `MockClock` and recording sinks.
- `sunlit-earth displays` prints the monitors and the plan, and on Linux a `setter:` line with the chosen setter and its evidence (`headless.rs`).
- Monitors are `display::Monitor`, in physical pixels of the virtual desktop. On Windows the process is declared per monitor DPI aware v2 by `wallpaper::windows::ensure_dpi_awareness`. On Linux the list comes from `xrandr`, which under Wayland answers through XWayland and can be skewed by scaling. On macOS it comes from CoreGraphics, because `NSScreen` is main thread only and the monitor query runs on other threads.
- The app has a full screen of its own: winit's borderless full screen on the window's monitor. There is no IPC command for it.
- The workspace already depends on `windows` and `windows-sys`, `objc2-core-graphics`, `x11rb` (with `randr`), `zbus` (blocking API) and `wayland-client`. `desktop::probe` already reads the Wayland globals, the session bus names and the X server's desktop windows to choose a setter.
- `docs/platforms.md` records that the X11 desktop shells the setter table knows (pcmanfm, pcmanfm-qt, xfdesktop, nemo-desktop, caja, plasmashell) map a window of type `_NET_WM_WINDOW_TYPE_DESKTOP`.

## Why geometry rather than a flag

A full screen flag misses exactly the case asked about. On Windows a borderless window has no flag at all; on X11 an application can size an undecorated or override-redirect window to the monitor without `_NET_WM_STATE_FULLSCREEN`. What these cases share is that the desktop on that monitor is covered. So the test is a window of another process whose rectangle covers a whole monitor, with the platform's full screen state as a second way to say yes where it exists.

The two kinds of error do not cost the same. A false positive holds back a wallpaper that nobody can see at that moment, and it goes out once the screen is uncovered. A false negative is the stutter. So where a rule is in doubt it leans towards "covered": a maximized window with an auto-hiding taskbar or no panel covers the monitor and counts, which is harmless.

On native Wayland the geometry is not needed (inferred): a client cannot place its own window, so one that covers a monitor has asked the compositor for full screen, and a window the compositor tiles over a whole output without a bar is again a desktop nobody sees.

## Windows

### `SHQueryUserNotificationState`

Read in Microsoft's reference: one machine-wide answer per user, polled, with "no notifications sent when the user starts or stops a full-screen application". `QUNS_BUSY` is "a full-screen application is running or Presentation Settings are applied", `QUNS_RUNNING_D3D_FULL_SCREEN` a Direct3D application in exclusive mode, `QUNS_PRESENTATION_MODE` the user's presentation settings. Nothing documents which monitor or whether the window must have focus.

`QUNS_BUSY` is not trustworthy alone. Rainmeter's source says it fires for a browser's F11 "but also is triggered when the desktop has no open windows in some cases" and trusts only the Direct3D value. Chromium's `IsFullScreenMode()` deliberately counts only `QUNS_RUNNING_D3D_FULL_SCREEN` and `QUNS_PRESENTATION_MODE` and adds its own geometry test. A Microsoft Q&A thread reports games returning `QUNS_BUSY` where `QUNS_RUNNING_D3D_FULL_SCREEN` was expected, and a pull request in another project notes the Direct3D value lingers briefly after an exclusive game exits. Whether `QUNS_BUSY` fires for a window on a secondary monitor is not documented anywhere found.

### The shell's own rule

Raymond Chen describes the taskbar's detection as a window whose client area covers the whole screen, with `NonRudeHWND` to opt out and `ITaskbarList2::MarkFullscreenWindow` to opt in. That is the same geometry test, which is why the borderless case reaches the shell's state at all.

### What mature projects do

- **Lively Wallpaper** (read in its source): polls on a timer, and its comment rejects `EVENT_OBJECT_LOCATIONCHANGE` as too noisy. Its default "grid" mode and its "all" mode look at every window, not only the foreground one. A window counts when `IsWindowVisible`, not cloaked (`DWMWA_CLOAKED`), not minimized, not transparent, not a tool window, not `WS_EX_NOACTIVATE` unless `WS_EX_APPWINDOW`, with a title, top level. It excludes the classes `WorkerW`, `Progman`, `Windows.UI.Core.CoreWindow`, `MultitaskingViewFrame`, `XamlExplorerHostIslandWindow`, `WindowsDashboard`, `Shell_TrayWnd`, `Shell_SecondaryTrayWnd`, `NotifyIconOverflowWindow` and a few of its own. Its game mode uses `SHQueryUserNotificationState` and pauses only on the Direct3D value. The manifest is per monitor v2.
- **Chromium**: foreground window, `EqualRect` with the monitor's `rcMonitor`, no thick or dialog frame, primary monitor only.
- **Wallpaper Engine**: has per-monitor and all-monitor pause options for another application in full screen; its help pages do not describe the rule.

Every window rather than only the foreground one matters for this app: a video full screen on the second monitor while the user types on the first covers a desktop just the same.

A maximized window's rectangle extends past the work area by its resize borders but stops at the taskbar, so with a visible taskbar it does not contain the monitor's rectangle. With an auto-hiding taskbar it may, which is the harmless false positive above.

### DPI

Read in Microsoft's DPI guide: a thread that is not per monitor aware can be handed virtualized coordinates. The process is per monitor v2 already, so window rectangles and monitor rectangles are both physical pixels.

### Events

`SetWinEventHook` needs a message loop on its thread; `ABN_FULLSCREENAPP` reaches only a registered appbar, says nothing about monitors or borderless windows, and registering an appbar has side effects. A poll at the moment a refresh is due needs neither.

## macOS

Read in the Apple developer forums: there is no public API for another application's full screen state, and Apple's own reply suggests `kCGWindowIsOnscreen` and `kCGWindowBounds` from `CGWindowListCopyWindowInfo` as the approximation. libwebrtc does exactly that: on-screen windows without desktop elements, layer 0, bounds equal to a display's bounds.

What that call returns without the Screen Recording permission is undocumented and has moved. Window titles have been withheld since 10.15; a July 2026 forum thread reports `kCGWindowOwnerName` as nil without the permission on macOS 26 and present on the 27 beta, and Apple's reply there says which keys are populated without permission "may change at any time". A Rust crate's comment says the process ID, the layer and the bounds are returned without permission; that was not confirmed on 26. No source found says the call itself raises a permission prompt.

A full screen window on a Mac with a notch sits below the notch, so it is shorter than `CGDisplayBounds` (a forum claim, not seen). Private SkyLight calls that tell a full screen Space apart exist (yabai, AltTab) and are out of the question for this app; the Accessibility attribute `AXFullScreen` needs the user to grant Accessibility.

Whether setting the desktop picture disturbs a full screen app on macOS at all was not found in any source.

## Linux, X11 sessions

KDE on X11, XFCE, Cinnamon, i3 and GNOME before the X11 session was removed (disabled by default in GNOME 49, removed in 50, from secondary sources), and every XWayland window in a Wayland session.

Read in the EWMH specification: `_NET_CLIENT_LIST_STACKING` is bottom to top, `_NET_WM_STATE_HIDDEN` marks a window that is not visible, `_NET_WM_DESKTOP` names its desktop (`0xFFFFFFFF` for all), `_NET_WM_WINDOW_TYPE_DESKTOP` and `_DOCK` mark the shell's own windows. Read in i3's source: it sets `_NET_WM_STATE_FULLSCREEN` on the client and unmaps windows of hidden workspaces.

What the projects do, read in their source:

- **linux-wallpaperengine**: ignores EWMH, walks the root's children with `XQueryTree`, skips unviewable ones and its own, and reports full screen when a child's rectangle equals a RandR CRTC rectangle. It does not skip desktop windows.
- **picom**: full screen when `_NET_WM_STATE_FULLSCREEN` is set or the window's geometry covers the whole root.
- **xidlehook**: walks the whole tree for `_NET_WM_STATE_FULLSCREEN`, regardless of desktop or minimized state.
- **caffeine-ng, xfce4-screensaver, xfce4-power-manager**: the active window's full screen state only.
- **Hidamari**: every window on the active workspace that is full screen or maximized and not minimized, not per monitor, and switched off under Wayland.

Combined, the test is: walk the stacking list; skip hidden windows, windows on another desktop, desktop and dock windows, windows that are not viewable, and windows whose `_NET_WM_PID` is this process; count a window that has `_NET_WM_STATE_FULLSCREEN` or whose rectangle, translated to root coordinates because the window manager reparents it into a frame, covers a RandR monitor. Override-redirect windows are not in the client list, so the viewable children of the root are checked by geometry too.

X11 sees only X11 windows: under Wayland an XWayland query finds Wine, Proton and other X11 games and misses native Wayland clients (secondary sources: a KDE bug on `wmctrl`, a Wine thread on `_NET_ACTIVE_WINDOW` going to None).

## Linux, Wayland

Read in the protocol definitions and on wayland.app:

- `zwlr_foreign_toplevel_management_v1` carries each toplevel's state (maximized, minimized, activated, and full screen since version 2) and `output_enter`/`output_leave`, and no geometry. Sway, Hyprland, niri, labwc and Wayfire implement it; Mutter and KWin do not.
- `ext-foreign-toplevel-list-v1` carries no state by design. A state extension is a draft merge request in wayland-protocols that could not be read.

### sway and the other wlroots compositors

Read in sway's source: both foreign toplevel globals are hidden only from clients with a security context, so an ordinary client gets them; `set_fullscreen` updates the handle's full screen state, and `output_enter` follows the scene. A full screen window on a hidden workspace probably still reports full screen, so full screen together with an entered output is the signal (inferred). linux-wallpaperengine's Wayland detector uses this protocol, counting full screen toplevels without regard to outputs. Sway's IPC (`get_tree`, `fullscreen_mode`, `rect`) answers too, for sway alone.

### KDE Plasma 6

Read in KDE's bug tracker: KWin will not implement the wlr protocol (bug 502647, resolved intentional) or the ext list (bug 483227). Its own `org_kde_plasma_window_management` has a full screen state, a geometry event and virtual desktops. Read in KWin's source: from Plasma 6.0 to 6.7 that global is restricted to clients whose executable an installed `.desktop` file names with the interface in `X-KDE-Wayland-Interfaces`; from 6.8 (commit f9bf0ee6, 2026-06-18) only sandboxed clients are refused.

KWin scripting over D-Bus has no such check: `org.kde.KWin` `/Scripting` `loadScript(path, name)`, then `run` on the script's object, then `unloadScript(name)`. A script can read every window's `fullScreen`, `frameGeometry`, `output`, `minimized`, `desktops`, `desktopWindow` and `pid`, and report with `callDBus`. That route is read in the code and has not been run end to end. kdotool, an xdotool for KDE Wayland, is said to work this way; its source was not read for this document.

### GNOME

Read in gnome-shell's source: `org.gnome.Shell.Introspect.GetWindows` is allowed only for the GTK and GNOME portal backends and carries no full screen field anyway; `org.gnome.Shell.Eval` needs unsafe mode. Mutter tracks full screen per monitor internally (`get_monitor_in_fullscreen`, `in-fullscreen-changed`), reachable only inside the shell. Hanabi does it with a shell extension; Hidamari switches its detection off on Wayland. Without an extension, nothing tells an ordinary client about native Wayland windows on GNOME.

## The idle inhibitor as a stand-in

Video players, browsers playing video and SDL games keep the screen awake while they play, and some sessions let anyone ask whether that is happening. It answers a different question ("something is playing", windowed or not), and an application that keeps the screen awake for good, such as a "keep awake" tool, holds it indefinitely.

Read in the sources:

- `org.freedesktop.ScreenSaver` has no query for inhibitors.
- GNOME and Cinnamon: `org.gnome.SessionManager.IsInhibited(flags)` with idle = 8, `GetInhibitors`, the `InhibitedActions` property. gnome-settings-daemon's screen saver proxy forwards `org.freedesktop.ScreenSaver.Inhibit` there with the idle flag, and Mutter forwards a Wayland client's idle inhibitor there while the inhibiting surface is not obscured. So on GNOME Wayland it covers native Wayland clients.
- KDE: `org.kde.Solid.PowerManagement.PolicyAgent.HasInhibition(types)`. KWin's Wayland idle inhibitor does not reach it (a TODO in KWin's source).
- sway: the Wayland idle inhibit protocol has no query side.
- VLC's inhibit modules: the Wayland idle inhibitor when its window is a Wayland window, otherwise the first D-Bus service present among `org.freedesktop.ScreenSaver`, `org.freedesktop.PowerManagement.Inhibit`, `org.mate.SessionManager` and `org.gnome.SessionManager`.

## What the checks cost

The probe runs only when an automatic publish is due, and while one is held every few seconds until the screens are clear. A two hour film at a 10 s recheck is about 700 probes.

- **Windows, measured** on the development desktop on 2026-11-23 through .NET P/Invoke: a pass over 455 top-level windows, 24 of them visible, asking each visible one for visibility, minimized state, cloaking, rectangle, extended style, process and class, took 0.13 ms on average over 200 passes. `SHQueryUserNotificationState` took 0.06 ms.
- **X11, estimated**: one request for the stacking list and a few property and geometry requests per window, all sent before any reply is read, so two or three round trips to a local server whatever the window count. Around a millisecond, all reads.
- **wlroots, estimated**: a short-lived connection, the registry, the manager bound, one round trip in which the compositor lists every toplevel's state. A few milliseconds.
- **GNOME, estimated**: one D-Bus call, under a millisecond, plus the XWayland check.
- **macOS, not measured**: `CGWindowListCopyWindowInfo` goes through the window server and builds a dictionary per window. No figure found.
- **KDE Wayland, not known**: loading, running and unloading a script runs JavaScript in KWin, on the thread that composites. If a one-shot script costs a frame, a script left loaded that reports changes, or `org_kde_plasma_window_management` on 6.8, costs nothing per probe.

Against that, a refresh that is held back saves a render at native resolution, its readback, the PNG and the desktop's reload of it.

## Recommendation

A probe injected into the engine like the clock and the sink, asked when an automatic publish is due and answering covered (by what, on which monitor), clear, or unknown with a reason. Covered holds the publish and asks again every few seconds; clear or unknown publishes as today. The user's own Set as Wallpaper is never held. Per platform: every visible top-level window by geometry on Windows, with the Direct3D and presentation values as extra positives; on-screen layer 0 windows by bounds on macOS; EWMH state and RandR geometry on X11 and for XWayland; the wlr foreign toplevel state on wlroots compositors; a KWin script on KDE Wayland; and on GNOME Wayland the session manager's idle inhibition, which the user accepted on 2026-11-23 as a stand-in.

## Open questions

- Whether bounds, layer and process ID come back from `CGWindowListCopyWindowInfo` on macOS 26 without the Screen Recording permission, and whether the call ever prompts. Only a tester with a Mac can say.
- How far below the display's top a native full screen window starts on a Mac with a notch.
- What a one-shot KWin script costs KWin's compositing thread, and whether `callDBus` can address a unique bus name rather than a well-known one.
- Whether the compositing overlay window of an X11 compositor appears among the root's viewable children, where it would cover every monitor.
- Whether `QUNS_BUSY` behaves the same for a window on a secondary monitor. The recommendation does not use it.
- The status of the foreign toplevel state extension for the ext list protocol.

## Sources

- [Microsoft, `SHQueryUserNotificationState`](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shqueryusernotificationstate) and [`QUERY_USER_NOTIFICATION_STATE`](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ne-shellapi-query_user_notification_state)
- [Microsoft Q&A, games reported as `QUNS_BUSY`](https://learn.microsoft.com/en-us/answers/questions/1086527/shqueryusernotificationstate-returns-quns-busy-ins)
- [Rainmeter, `GameMode.cpp`](https://github.com/rainmeter/rainmeter/blob/master/Library/GameMode.cpp)
- [Chromium, `fullscreen_win.cc`](https://raw.githubusercontent.com/chromium/chromium/main/ui/base/fullscreen_win.cc)
- [Raymond Chen, how the taskbar decides a window is full screen](https://devblogs.microsoft.com/oldnewthing/20250522-00/?p=111211)
- [Lively Wallpaper](https://github.com/rocksdanister/lively), `Core/Suspend/Playback.cs`, `WindowUtil.cs`, `WindowClassExclusions.cs`, and its [performance wiki page](https://github.com/rocksdanister/lively/wiki/Performance)
- [Microsoft, high DPI desktop applications](https://learn.microsoft.com/en-us/windows/win32/hidpi/high-dpi-desktop-application-development-on-windows), [`SetWinEventHook`](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwineventhook), [`ABN_FULLSCREENAPP`](https://learn.microsoft.com/en-us/windows/win32/shell/abn-fullscreenapp)
- Apple developer forums: [fullscreen detection](https://developer.apple.com/forums/thread/792917), [being told when another app goes full screen](https://developer.apple.com/forums/thread/813033), [window names and the permission on 10.15](https://developer.apple.com/forums/thread/126860), [keys populated without permission on 26](https://developer.apple.com/forums/thread/839069), [display rectangles](https://developer.apple.com/forums/thread/779272)
- [libwebrtc, `window_list_utils.cc`](https://searchfox.org/firefox-main/source/third_party/libwebrtc/modules/desktop_capture/mac/window_list_utils.cc)
- [EWMH, application window properties](https://specifications.freedesktop.org/wm/latest/ar01s05.html) and [root window properties](https://specifications.freedesktop.org/wm/latest/ar01s03.html)
- linux-wallpaperengine, the [X11](https://raw.githubusercontent.com/Almamu/linux-wallpaperengine/master/src/WallpaperEngine/Render/Drivers/Detectors/X11FullScreenDetector.cpp) and [Wayland](https://raw.githubusercontent.com/Almamu/linux-wallpaperengine/master/src/WallpaperEngine/Render/Drivers/Detectors/WaylandFullScreenDetector.cpp) detectors
- [picom, `win.c`](https://raw.githubusercontent.com/yshui/picom/HEAD/src/wm/win.c), [xidlehook, `xcb.rs`](https://raw.githubusercontent.com/jD91mZM2/xidlehook/master/xidlehook-core/src/modules/xcb.rs), [caffeine-ng, `triggers.py`](https://raw.githubusercontent.com/WhyNotHugo/caffeine-ng/HEAD/caffeine/triggers.py), [xfce4-screensaver, `gs-listener-x11.c`](https://raw.githubusercontent.com/xfce-mirror/xfce4-screensaver/master/src/gs-listener-x11.c), [xfce4-power-manager, `xfpm-power.c`](https://raw.githubusercontent.com/xfce-mirror/xfce4-power-manager/master/src/xfpm-power.c), [Hidamari, `utils.py`](https://raw.githubusercontent.com/jeffshee/hidamari/HEAD/src/hidamari/utils.py)
- [i3, IPC](https://raw.githubusercontent.com/i3/i3/next/docs/ipc) and [`src/con.c`](https://raw.githubusercontent.com/i3/i3/next/src/con.c)
- [wlr foreign toplevel management](https://wayland.app/protocols/wlr-foreign-toplevel-management-unstable-v1), [ext foreign toplevel list](https://wayland.app/protocols/ext-foreign-toplevel-list-v1)
- [sway, `server.c`](https://github.com/swaywm/sway/blob/master/sway/server.c) and [`sway-ipc(7)`](https://github.com/swaywm/sway/blob/master/sway/sway-ipc.7.scd)
- KDE bugs [502647](https://bugs.kde.org/show_bug.cgi?id=502647) and [483227](https://bugs.kde.org/show_bug.cgi?id=483227); [plasma window management protocol](https://invent.kde.org/libraries/plasma-wayland-protocols/-/raw/master/src/protocols/plasma-window-management.xml); KWin [`wayland_server.cpp` on 6.7](https://invent.kde.org/plasma/kwin/-/raw/Plasma/6.7/src/wayland_server.cpp) and [on 6.8](https://invent.kde.org/plasma/kwin/-/raw/Plasma/6.8/src/wayland_server.cpp), [`scripting.cpp`](https://invent.kde.org/plasma/kwin/-/raw/Plasma/6.7/src/scripting/scripting.cpp)
- gnome-shell [`introspect.js`](https://raw.githubusercontent.com/GNOME/gnome-shell/main/js/misc/introspect.js) and [the Introspect interface](https://raw.githubusercontent.com/GNOME/gnome-shell/main/data/dbus-interfaces/org.gnome.Shell.Introspect.xml); [Hanabi, `autoPause.ts`](https://raw.githubusercontent.com/jeffshee/gnome-ext-hanabi/HEAD/src/autoPause.ts)
- [gnome-session, `org.gnome.SessionManager.xml`](https://raw.githubusercontent.com/GNOME/gnome-session/main/gnome-session/org.gnome.SessionManager.xml), [gnome-settings-daemon, screen saver proxy](https://raw.githubusercontent.com/GNOME/gnome-settings-daemon/main/plugins/screensaver-proxy/gsd-screensaver-proxy-manager.c), [powerdevil, PolicyAgent](https://invent.kde.org/plasma/powerdevil/-/raw/master/daemon/dbus/org.kde.Solid.PowerManagement.PolicyAgent.xml), [VLC, `modules/misc/inhibit/dbus.c`](https://code.videolan.org/videolan/vlc/-/raw/master/modules/misc/inhibit/dbus.c)
