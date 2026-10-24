# Research: start on login, and what a second launch does

## Question

How to add a "start on login" option that launches Sunlit Earth with its window hidden when the user logs in, on Windows, Linux and macOS, reliably and without admin rights. A second question was added during the research: a second launch of the app today finds the single-instance guard taken, logs one line and exits, which looks to a new user like the app did nothing. It should instead bring up the running instance's window, and say so when the copy that was launched is not the one that is running. The two belong together: an app that starts hidden at login is exactly the one the user will try to launch again.

Everything below was researched on 2026-10-24 from specifications, vendor documentation and source code. Nothing was run on a real desktop except where a line says so. Claims that could not be confirmed are marked unverified.

## What the app has today

- `--tray-start hidden` (`cli.rs`) hides the window on the first event loop turn and turns the preview off (`app.rs`, `run_event_loop`). It only applies in tray mode, and `run` refuses it with `--mode window`.
- The single-instance guard (`tray::acquire_single_instance`, the `single-instance` crate) runs in tray mode only, before the window and the GPU device exist. A second instance logs "another instance is already running, exiting" and returns `ExitCode::SUCCESS`. It tells the first instance nothing.
- The IPC socket (`ipc.rs`, `interprocess` local sockets) is opt-in through `--ipc-socket`. It already has `show-window`, but nothing binds it unless the flag is given.
- With no tray host, `tray::create_tray` returns `None` and the app runs windowed: `--tray-start hidden` is then ignored and the window shows, and closing it exits.
- On Linux the tray is Slint's `ksni` backend (`i-slint-core-1.18.1/items/system_tray/ksni.rs`). It calls ksni's `spawn()` without `assume_sni_available`, so if `org.kde.StatusNotifierWatcher` is not on the session bus at that instant the spawn fails and the app runs windowed. ksni 0.3.6, which the lock file has, already offers `assume_sni_available(true)`, which routes a missing watcher to `watcher_offline` and registers the item once the watcher appears. Slint does not use it.
- On Windows the tray is Slint's own `Shell_NotifyIconW` code, which re-adds the icon on the `TaskbarCreated` broadcast, so an Explorer restart is covered.
- On Wayland, Slint's `hide()` destroys the winit window and `show()` creates a new one (`i-slint-backend-winit-1.18.1/winitwindowadapter.rs`, the "Wayland doesn't support hiding a window" branch). Raising a hidden window there is creating a new toplevel.
- `docs/roadmap.md` already carries two related items: macOS autostart "would be a LaunchAgent" (with `LSUIElement` and the ordered teardown), and "an autostart entry has to point at the paths that survive an upgrade".
- Distribution: a Windows zip and the Scoop bucket; Linux tarballs and a Homebrew formula (`libexec.install`, `bin.install_symlink libexec/"sunlit-earth"`); macOS `Sunlit Earth.app` (bundle id `earth.sunlit.SunlitEarth`, `LSMinimumSystemVersion` 11.0, ad-hoc signed, not notarized) through a zip or the tap's cask, plus a bare terminal binary.

## Windows

### Mechanisms

Three per-user mechanisms need no admin rights.

- **`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.** Runs at logon, ignored in Safe Mode, order unspecified, command line limited to 260 characters, and "the system does not provide guarantees about how promptly the programs in the Run key are run" ([Run and RunOnce keys](https://learn.microsoft.com/en-us/windows/win32/setupapi/run-and-runonce-registry-keys)). Explorer runs it in its low priority startup phase, after a delay and after each previous program settles ([The Old New Thing, 2016-05-02](https://devblogs.microsoft.com/oldnewthing/20160502-00/?p=93395)). It is listed in Task Manager's Startup apps and in Settings > Apps > Startup, which can disable it.
- **A shortcut in the Startup folder.** Same phase and listing. A `.lnk` needs COM `IShellLink` to write, and the user sees it as a file.
- **A Task Scheduler logon task.** Can be registered without elevation (tested on the development host with a filtered admin token; a true standard user is unverified). It is not tied to Explorer, so it may run before the taskbar exists. The PowerShell cmdlets' defaults stop it on battery and kill it after 72 hours, which a tray app has to override. It is not listed in Startup apps, so the user's usual switch cannot turn it off. That last point is the main reason against it.

### StartupApproved

When the user disables an entry in Task Manager or Settings, the `Run` value stays and a REG_BINARY of the same name under `HKCU\...\Explorer\StartupApproved\Run` changes. Read on the development host: enabled is `02` followed by 11 zero bytes, disabled is `03` followed by three zero bytes and a FILETIME. `06` and `07` are reported elsewhere; the consensus is that the low bit of the first byte decides ([MS forum archive](https://learn.microsoft.com/en-us/archive/msdn-technet-forums/b4c9f990-ecb5-46e1-9711-df72bb896f31)). A `Run` value without a StartupApproved value is enabled.

The entry is keyed by name only. StartupApproved values outlive their `Run` values (seven orphans on the development host), and a third party found the blob unchanged when the `Run` value was deleted and recreated ([calendarweek-tray research notes](https://github.com/AckermannM/calendarweek-tray)). So a user who once disabled the entry in Task Manager, then turns it on in the app, stays disabled unless the app also resets StartupApproved. Whether that holds at an actual logon is inferred, not observed.

Electron is the reference implementation ([browser_win.cc](https://github.com/electron/electron/blob/main/shell/browser/browser_win.cc)): enabling writes the `Run` value and deletes the StartupApproved value, disabling deletes both, and "will launch at login" is a matching `Run` value whose StartupApproved value is absent or starts with `00` or `02`.

### Command line and path

- Quote the executable path: `"C:\path with spaces\sunlit-earth.exe" --tray-start hidden`. Arguments follow the C runtime rules ([parsing C command-line arguments](https://learn.microsoft.com/en-us/cpp/c-language/parsing-c-command-line-arguments)).
- Scoop's shims and Start menu shortcuts point at `~\scoop\apps\sunlit-earth\current\...` ([install.ps1](https://github.com/ScoopInstaller/Scoop/blob/master/lib/install.ps1), [core.ps1](https://github.com/ScoopInstaller/Scoop/blob/master/lib/core.ps1)), and `std::env::current_exe()` returns that junction path, not the versioned directory behind it, whether launched directly, through the shim or through a shortcut (tested on the development host). So `current_exe()` without `canonicalize` is the right source, which is what the roadmap item asks for.
- Do not rely on the working directory of a Run-key process (reported as `System32`, unverified). The app already resolves textures from the executable.

### Crates

`auto-launch` 0.6.0 (2026-01-10) writes the `Run` value with the path unquoted, tries HKLM first by default, and its `is_enabled` decides from the last eight bytes of the StartupApproved value, so a disable with a zero FILETIME reads as enabled. Quoting is fixed on its `main` branch, unreleased. `tauri-plugin-autostart` wraps it. The logic is about thirty lines over `windows-registry` or `windows-sys`, which the app already depends on, and owning it is simpler than working around the crate.

### Bringing the running instance forward

A background process may not take the foreground; Windows flashes its taskbar button instead ([SetForegroundWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setforegroundwindow)). The launching process, which the user just started and which therefore may set the foreground, can pass the right on with `AllowSetForegroundWindow(pid)` before it signals ([AllowSetForegroundWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-allowsetforegroundwindow)). Chromium does exactly that before its `WM_COPYDATA` ([chrome_process_finder.cc](https://github.com/chromium/chromium/blob/main/chrome/browser/win/chrome_process_finder.cc)). winit 0.30's `focus_window` on Windows additionally injects an Alt press before `SetForegroundWindow` ([window.rs](https://github.com/rust-windowing/winit/blob/v0.30.13/src/platform_impl/windows/window.rs)). Whether Slint's `show()` on a hidden window activates it is unverified.

## Linux

### Mechanisms

- **XDG autostart** ([spec](https://specifications.freedesktop.org/autostart-spec/latest/)): a desktop entry in `$XDG_CONFIG_HOME/autostart` (default `~/.config/autostart`). Processed by gnome-session, xfce4-session and cinnamon-session themselves, and on Plasma 6 by `systemd-xdg-autostart-generator` through `xdg-desktop-autostart.target`. Not processed by sway or i3: their documentation describes only `exec` lines in the config ([i3 user guide](https://i3wm.org/docs/userguide.html#_automatically_starting_applications_on_i3_startup), [sway wiki](https://github.com/swaywm/sway/wiki)); users of those run `dex -a` or add an `exec` line. Beyond the six sessions the guest has: LXQt documents XDG autostart ([LXQt wiki](https://lxqt-project.org/wiki/Miscellaneous.html)), and the `mate-session` and `budgie-session` man pages list `~/.config/autostart` among their directories ([mate-session(1)](https://man.archlinux.org/man/mate-session.1.en), [budgie-session(1)](https://man.archlinux.org/man/budgie-session.1.en)). Hyprland's own mechanism is `exec-once`, and the autostart folder reaches it only through `xdg-desktop-autostart.target` wired by hand ([Arch forum](https://bbs.archlinux.org/viewtopic.php?id=300224)). COSMIC is reported to honor it, depending on the build's `autostart` feature and the login path ([Arch forum](https://bbs.archlinux.org/viewtopic.php?id=301295), [Artix forum](https://forum.artixlinux.org/index.php?msg=48603)); unverified for current releases.
- **systemd user unit** `WantedBy=graphical-session.target`. Reached by Plasma 6 and GNOME, and by Cinnamon only on recent builds. XFCE never starts that target, i3 does not, sway only if the user wires it, and the target refuses a manual start. `DISPLAY` and `WAYLAND_DISPLAY` reach the unit only if the session imports them. It adds nothing on the desktops where it works and fails silently on the others.
- **The Background portal** (`org.freedesktop.portal.Background.RequestBackground` with `autostart`). For a host app it skips the dialog, but it needs an app ID, which a host process only has after `org.freedesktop.host.portal.Registry.Register` with an installed `<app_id>.desktop`, or when launched as a systemd `app-*.scope`. It writes `~/.config/autostart/<app_id>.desktop` with `Exec` joined from `commandline` without quoting, and without `commandline` writes no `Exec` at all for a host app ([background.c](https://github.com/flatpak/xdg-desktop-portal/blob/main/desktop-portal/background.c)). GNOME's background apps list shows Flatpak instances only. It buys nothing over writing the file directly.

### What "disabled" looks like in each settings UI

| UI | Turns the entry off by | Reads |
|---|---|---|
| Plasma 6 System Settings, Autostart | `Hidden=true` in the user file ([autostartmodel.cpp](https://github.com/KDE/plasma-workspace/blob/master/kcms/autostart/autostartmodel.cpp)) | `Hidden` |
| GNOME Tweaks, Startup Applications | deleting the user file ([utils.py](https://github.com/GNOME/gnome-tweaks/blob/main/gtweak/utils.py)) | `X-GNOME-Autostart-enabled=false` |
| XFCE Session and Startup | `Hidden=true` ([xfae-model.c](https://github.com/xfce-mirror/xfce4-session/blob/master/settings/xfae-model.c)) | `Hidden` |
| Cinnamon Startup Applications | `X-GNOME-Autostart-enabled=false` ([cs_startup.py](https://github.com/linuxmint/cinnamon/blob/master/files/usr/share/cinnamon/cinnamon-settings/modules/cs_startup.py)) | both |

`Hidden=true` is honored by all four sessions; `X-GNOME-Autostart-enabled=false` is ignored by Plasma's generator and by XFCE. So the app's "enabled" is: the user file exists, `Hidden` is not true, and `X-GNOME-Autostart-enabled` is not false. The app's own off switch deletes the file, which every session and UI understands.

### Exec line and path

- The `Exec` key quotes any argument holding a space or one of the reserved characters in double quotes, escaping `"`, `` ` ``, `$` and `\` inside, and the general string unescaping runs first, so a literal backslash is four in the file ([Exec variables](https://specifications.freedesktop.org/desktop-entry/latest/exec-variables.html)). Plasma's generator logs a parse error and creates no unit for a malformed line.
- `current_exe()` on Linux reads `/proc/self/exe`, which is the resolved target. For the Homebrew formula that is `<prefix>/Cellar/sunlit-earth/<version>/libexec/sunlit-earth`, which the next `brew upgrade` removes. The entry has to be written as `<prefix>/opt/sunlit-earth/bin/sunlit-earth`, recognized by the `Cellar/sunlit-earth/<version>/` component and checked to resolve to the same file. A tarball has no such alias: the entry points where it was extracted.

### The tray at login

Nothing guarantees the StatusNotifierWatcher exists when an autostart entry starts:

- Plasma 6: the watcher is a kded module; autostart units are ordered after `plasma-workspace.target`, but `plasma-kded6.service` is `Type=simple`, so started does not mean the name is owned ([statusnotifierwatcher.json](https://github.com/KDE/plasma-workspace/blob/master/statusnotifierwatcher/statusnotifierwatcher.json), [KDE bug 385828](https://bugs.kde.org/show_bug.cgi?id=385828)).
- XFCE: the panel's systray plugin owns the name ([sn-backend.c](https://github.com/xfce-mirror/xfce4-panel/blob/master/plugins/systray/sn-backend.c)), and autostart runs in parallel with it.
- Cinnamon: `xapp-sn-watcher` does not register with the session manager, so autostart races it ([cinnamon#13740](https://github.com/linuxmint/cinnamon/issues/13740), reporter's analysis).
- GNOME: no watcher at all without the AppIndicator extension. GNOME's guidelines allow an app to run with no window only if it does real background work ([status icon migration guidelines](https://wiki.gnome.org/Initiatives/StatusIconMigration/Guidelines)), which a wallpaper app does.
- sway and i3: whatever bar the user runs.

Nextcloud waits and retries up to 30 seconds on XFCE and then shows its window ([main.cpp](https://github.com/nextcloud/desktop/blob/master/src/gui/main.cpp)); KeePassXC writes `X-GNOME-Autostart-Delay=2` and `X-KDE-autostart-after=panel` ([NixUtils.cpp](https://github.com/keepassxreboot/keepassxc/blob/develop/src/gui/osutils/nixutils/NixUtils.cpp)), which today only Cinnamon honors.

With the current code a login race on Plasma, XFCE or Cinnamon makes the app run windowed, so the window pops up at login, and on GNOME it always does. The fix is a wait for the watcher before the tray is created when starting hidden. The app cannot pass `assume_sni_available` through Slint, but `sunlit-core` already depends on `zbus`, so waiting for `NameOwnerChanged` on `org.kde.StatusNotifierWatcher` with a timeout is local. Passing the option through Slint would be the better fix and is an upstream change.

### Bringing the running instance forward

The launcher gives the new process an activation token in `XDG_ACTIVATION_TOKEN` (Wayland) or `DESKTOP_STARTUP_ID` (X11), and the convention is that a second instance forwards it to the first and unsets it ([xdg-activation-v1](https://wayland.app/protocols/xdg-activation-v1)); GTK, `KDBusService`, Firefox and Chromium all do ([kdbusservice.cpp](https://github.com/KDE/kdbusaddons/blob/master/src/kdbusservice.cpp), [xdg_util.h](https://github.com/chromium/chromium/blob/main/base/nix/xdg_util.h)). Without a valid token Mutter marks the window as demanding attention and GNOME Shell shows a "ready" notification ([meta-wayland-activation.c](https://gitlab.gnome.org/GNOME/mutter/-/raw/main/src/wayland/meta-wayland-activation.c)), and KWin highlights it in the task bar.

winit 0.30 applies a token only at window creation (`with_activation_token`), and its Wayland `focus_window` is empty. Since Slint recreates the window on every show under Wayland, the token would have to reach that creation, and Slint's recreation path does not run the `with_window_attributes_hook` (read from the source, not run). So on Wayland the honest expectation today is attention rather than focus, unless Slint is changed or the app calls `xdg_activation_v1.activate` itself on the new surface. On X11, winit's `focus_window` sends `_NET_ACTIVE_WINDOW`; Mutter, Muffin and i3 (on the current workspace) focus on it, and KWin and xfwm4 with focus stealing prevention may only mark it (read from the source, not run).

## macOS

### Mechanisms

- **`SMAppService`** (macOS 13 and later). `.mainApp` takes no arguments at all ([mainApp](https://developer.apple.com/documentation/servicemanagement/smappservice/mainapp.md)), so the app would have to detect a login launch from the `keyAELaunchedAsLogInItem` Apple event, which needs a hook into winit's delegate. Two write-ups found `.mainApp` working for an ad-hoc signed app on macOS 27 and surviving a re-sign ([carrier](https://github.com/AshuLab/carrier/blob/main/docs/research/macos-app-login-item.md), [wisp](https://github.com/saddathasan/wisp/blob/main/docs/research/launch-at-login.md)); one report found `.agent` services of an ad-hoc app removed after an update with a launch constraint violation ([memql-cockpit#545](https://github.com/znasllc-io/memql-cockpit/issues/545)). Nothing covers 13 to 15. It does not exist on 11 and 12, which the app supports.
- **`SMLoginItemSetEnabled`** (10.6 to 13, deprecated) needs a helper bundle in `Contents/Library/LoginItems` that launches the main app. **AppleScript through System Events** needs an Automation consent prompt, the same TCC problem the wallpaper research rejected `osascript` for. **`LSSharedFileList`** is deprecated since 10.11 and its behavior on 11 and later is unverified.
- **A per-user LaunchAgent**: `~/Library/LaunchAgents/earth.sunlit.SunlitEarth.plist` with `ProgramArguments`, `RunAtLoad`, `LimitLoadToSessionType = Aqua` and `ProcessType = Interactive` ([launchd.plist(5)](https://keith.github.io/xcode-man-pages/launchd.plist.5.html)). Works on every version the app supports, takes real arguments, needs no helper and no consent. It takes effect at the next login, or at once through `launchctl bootstrap gui/<uid> <plist>` ([launchctl(1)](https://keith.github.io/xcode-man-pages/launchctl.1.html)). On 13 and later the system posts a "Background Items Added" notification and lists the item in Login Items; without a Team ID, `AssociatedBundleIdentifiers` cannot link it to the app, so it is listed under the executable's name. `SMAppService.statusForLegacyPlist(at:)` reports whether the user switched it off there ([statusForLegacyPlist](https://developer.apple.com/documentation/servicemanagement/smappservice/statusforlegacyplist(at:).md)); what it returns for an ad-hoc plist is unverified.

The roadmap already chose the LaunchAgent, and the research agrees: it is the one mechanism that covers the 11.0 floor with an ad-hoc signature.

### Direct exec or `open`

Apple DTS advises launching GUI apps through LaunchServices (`NSWorkspace`, which `open` uses) rather than exec'ing them from launchd, because the bootstrap namespace and security context differ ([forum thread 67473](https://developer.apple.com/forums/thread/67473)). `ProgramArguments = [/usr/bin/open, -a, "<bundle path>", --args, --tray-start, hidden]` does that, and `open` exiting at once is harmless without `KeepAlive`. Two things are unverified: whether a directly exec'd bundle binary gets its Dock icon and status item normally, and whether Login Items then lists the item as `open` rather than the app. The bare terminal binary has no bundle and can only be exec'd directly.

### The Dock icon

The roadmap says `LSUIElement` does nothing because winit forces the regular activation policy at startup. That was true before winit 0.30.6, whose changelog reads "package manifest definitions of `LSUIElement` will no longer be overridden with the default activation policy, unless explicitly provided during initialization". The lock file has 0.30.13, whose `applicationDidFinishLaunching` sets no policy for a bundled process when none was given (`platform_impl/macos/app_state.rs`), and Slint passes none. So adding `LSUIElement` to `assets/macos/Info.plist` should hide the Dock icon of the `.app` with no Slint feature at all; this is read from the source and not yet observed on a runner. The bare binary has no bundle identifier, so winit still sets the regular policy for it. An `LSUIElement` app has no Dock icon, no menu bar of its own and no entry in Command-Tab even while its window is open; an app that wants those while the window shows can switch with `NSApplication.setActivationPolicy` at runtime, through the `objc2-app-kit` that `sunlit-core` already depends on. That switch has a reported quirk: after going from accessory to regular the menu bar can keep showing the previously active app's menus even though activation succeeded ([forum thread 650270](https://developer.apple.com/forums/thread/650270), FB7743313, no Apple fix). The reported workaround is the order: show the window, then set the regular policy, then activate ([Aral Balkan](https://ar.al/2018/09/17/workaround-for-unclickable-app-menu-bug-with-window.makekeyandorderfront-and-nsapp.activate-on-macos/)), and on Sonoma reports say it still fails at times ([forum thread 739075](https://developer.apple.com/forums/thread/739075)).

The bundle path comes from `current_exe()` (`.../Sunlit Earth.app/Contents/MacOS/sunlit-earth`) by going up three levels. The cask installs to `/Applications`, which is not versioned. An app run from a quarantined zip runs from an App Translocation path, which must never be written into an agent.

### Bringing the running instance forward

Double-clicking a running app in Finder or the Dock does not start a second process: LaunchServices activates the running one and sends it `applicationShouldHandleReopen:hasVisibleWindows:` ([NSApplicationDelegate](https://developer.apple.com/documentation/appkit/nsapplicationdelegate/applicationshouldhandlereopen(_:hasvisiblewindows:).md)). winit 0.30.12's delegate implements only `applicationDidFinishLaunching:` and `applicationWillTerminate:`, and Slint's custom application handler has no reopen either. So a window hidden by `--tray-start hidden` cannot be brought back from the Dock today, and the socket path below never sees that launch. Slint 1.18 itself adds a method to `WinitApplicationDelegate` at runtime with `class_addMethod`, which makes adding the reopen method the same way an established pattern; tao implements it on its own delegate ([app_delegate.rs](https://github.com/tauri-apps/tao/dev/src/platform_impl/macos/app_delegate.rs)). A different copy of the bundle under the same identifier, and the bare binary, do start a second process; LaunchServices' behavior for the first is unverified.

## The second launch

The parts above lead to one design for the second launch, the same on every platform:

1. The first instance always listens on a local socket whose name derives from the instance name (`sunlit-earth-app`, or the one `--ipc-socket` gives, so the e2e suite's parallel instances stay apart). This is a separate, fixed purpose channel, not the opt-in test IPC.
2. A second launch that loses the guard connects, reads the first instance's pid (Windows needs it for `AllowSetForegroundWindow`), sends one line with its version, its executable path and any activation token, unsets the token variables, and exits.
3. A second launch with `--tray-start hidden`, which is what an autostart entry firing beside a running instance looks like, sends nothing and exits quietly.
4. The first instance shows and focuses its window and turns the preview on, the same as the tray's "Open". If the second launch's version differs from its own, the window says that the copy just launched did not start and the running one did, names both versions, and could offer to quit and start the other executable.
5. On macOS, the reopen handler does step 4 for launches LaunchServices routes to the running process.

## Recommendation

| | Mechanism | Path written | "Enabled" means |
|---|---|---|---|
| Windows | `HKCU\...\Run` value `SunlitEarth` | `current_exe()` quoted, never canonicalized | value present and StartupApproved absent or low bit clear |
| Linux | `$XDG_CONFIG_HOME/autostart/sunlit-earth.desktop` | `opt` path for Homebrew, else `current_exe()`, quoted per the spec | file present, `Hidden` not true, `X-GNOME-Autostart-enabled` not false |
| macOS | `~/Library/LaunchAgents/earth.sunlit.SunlitEarth.plist` | bundle path through `open`, or the binary for a bare one | plist present and `statusForLegacyPlist` not disabled on 13+ |

- Write the platform code in the app rather than taking `auto-launch`: its released version mis-quotes paths on Windows and Linux and misreads the user's disable on both.
- The OS state is the setting. It is read when the settings window opens and never stored in `config.toml`, so a change made in Task Manager or a desktop's settings is what the app shows.
- Turning it on writes the entry for this copy of the app. If an existing entry points at another path, the settings window says so rather than rewriting it silently at startup, so a development build never takes over an installed copy's entry.
- On sway and i3 the switch would do nothing; the app reads `XDG_CURRENT_DESKTOP`, disables the switch there and names the `exec` line instead.
- When starting hidden on Linux, wait for the StatusNotifierWatcher (a few seconds, through `zbus`) before creating the tray.
- macOS also needs the reopen handler, which is required for a hidden start to be recoverable from the Dock at all.

## Open questions

1. With no tray (GNOME without the extension, or a watcher that never comes), should a hidden start stay hidden and run in the background, with the app launcher reopening the window through the second launch path, or show the window, which is what the app does today and what Nextcloud does? The guidelines allow the former for an app doing real background work. Related: without a tray, closing the window exits the app today, which would stop the wallpaper updates for someone who opened the settings once.
2. Should the version notice offer to switch to the launched copy, or only explain?
3. macOS: `open` or direct exec in the agent, and is `LSUIElement` (see "The Dock icon" above) a prerequisite for a pleasant hidden start, since the Dock icon is there all along?
4. Testing at login: whether the Windows and Linux guests can log out and back in within one VM run is unknown and decides whether the e2e suite can cover the entry itself or only the second launch path. The macOS runner can at least show `launchctl print` and `sfltool dumpbtm` output.
5. To observe in a guest before relying on it: a stale StartupApproved `03` blocking a recreated `Run` value, a Run-key launch of an exe carrying the Mark of the Web, `show()` plus `focus_window` from a background process on Windows 11, and the attention fallback on GNOME and KDE Wayland.
