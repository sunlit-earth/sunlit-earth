# Plan: Linux wallpaper setter fallbacks (2026-09-23)

Written 2026-09-23 from [2026-09-23-linux-wallpaper-fallback-research.md](2026-09-23-linux-wallpaper-fallback-research.md), which holds the prior art, the per-desktop facts and their sources. Intended for its own branch, `feat/linux-wallpaper-fallbacks`. The guest image part (step 7) builds on PR #57 (KDE and GNOME Wayland sessions, merged 2026-09-23): its `--session-type` flag, `provider::desktop::Login`, the `WAYLAND_DISPLAY` line in the guest contract, and the boot's check of `XDG_SESSION_TYPE` and `XDG_CURRENT_DESKTOP` against what was asked for.

## Summary

Today a Linux session gets a wallpaper only when `XDG_CURRENT_DESKTOP` names one of seven desktops, and everything else is refused before a frame is rendered. This plan keeps the table for the desktops it names and puts a probe ladder behind it: a session that names no known desktop, or names one whose setter is not there, is examined for the mechanism most likely to work, and only refused once every rung has declined. The new mechanisms, in the order they are built: five more table rows (sway, Hyprland through hyprpaper, LXDE, Deepin, Trinity); the X11 root window pixmap, set natively through x11rb, for plain window managers; a Wayland rung that uses a running wallpaper daemon (awww, wpaperd) or starts and owns a `swaybg`; and the XDG desktop portal as the last rung. Detection becomes a pure function over a snapshot of the session, which keeps it testable on every platform the way the table is today.

## Stakes

Medium. Every Linux session goes through the new chooser, so the seven desktops that work today must choose exactly what they choose now, and a test pins that. Two sessions deliberately change: `TDE:KDE` stops reaching the Plasma script and Regolith on Wayland stops reaching GNOME's gsettings. Nothing changes on Windows or macOS. The new external effects are an X server resource retained across the app's exit, a `swaybg` process that outlives the app, and a one-time permission dialog from the portal; each is a decision below.

## What is there today

- `desktop::detect` (`crates/sunlit-core/src/desktop/mod.rs:430`) maps `XDG_CURRENT_DESKTOP` tokens to a `Backend { desktop, program, kind }`; `Kind` is `Gsettings`, `Kde`, `Xfce`, `Lxqt`. `Backend::reach`, `commands`, `discovery`, `degradation` and `nothing_to_run` are pure.
- `wallpaper/linux.rs` owns the files and the running: `check_supported` (detect plus a `PATH` lookup), `write_placement` by reach, `set_wallpaper_job` (detect again, discover, run each `Invocation`).
- Monitors come from `xrandr --query`; with no answer the sink plans for 2560x1440.
- `x11rb` 0.13 is a `sunlit-core` dependency with only the `randr` feature. `zbus` 5.14 and `wayland-client` 0.31 are in `Cargo.lock` through Slint, winit and ksni, but not dependencies of `sunlit-core`.
- The e2e read-back (`tests/common/desktop_linux.rs`) derives what to ask from the commands the sink would run, returns the values the desktop holds (PR #57 changed it from file names, since every publish writes the same names into its own directory), and skips where a setter has no store to ask.
- The app sets its XDG app id to `sunlit-earth` on Linux (PR #57), which is also the basename of its desktop file, so the portal registration of decision 10 uses the same id the compositor already sees.
- The guest boots KDE and GNOME on X11 or Wayland and XFCE and Cinnamon on X11, six sessions from one image, chosen by `--desktop` and `--session-type`.
- The environment variable reference is `docs/architecture.md#environment-knobs`, so nothing here needs README.

## Goals

1. The seven desktops in the table choose the same setter as today, with the same commands.
2. An X11 session with a plain window manager (i3, Openbox, bspwm, awesome, dwm and the like) gets a wallpaper on every monitor, in all three display modes, with no program installed.
3. A Wayland session on a layer-shell compositor outside the table (niri, river, labwc, Wayfire and the like) gets a wallpaper through the daemon it already runs, or through `swaybg` if it runs none.
4. sway, Hyprland with hyprpaper, LXDE, Deepin and Trinity have rows of their own.
5. Every choice is explained: the log says which setter was chosen and on what evidence, `sunlit-earth displays` prints the same, and a refusal lists what was probed and what was found.
6. A user can force a setter with one environment variable when the ladder guesses wrong.

## Non-goals

- The app drawing its own layer-shell surface. It conflicts with the rule that decoded pixel buffers are not held in long-lived structs, ties the wallpaper to the app's lifetime, and `swaybg` covers the same compositors.
- COSMIC, Pantheon, UKUI, Enlightenment, Lomiri, and Budgie on Wayland. Each lacks a documented method or has conflicting evidence; they go on the roadmap with what is known.
- Per-monitor images on the Wayland rungs. They need the compositor's output names, which `xrandr` through Xwayland does not report; the native Wayland display query the roadmap already describes is the foundation for that, and it is its own plan. Every Wayland rung here is `OneImage`.
- Shelling out to feh, nitrogen, xwallpaper or hsetroot. The native root pixmap supersedes them without their state files.
- Verifying that a setter changed what is on screen. No mechanism offers it, and none of the prior art tries.

## Decisions

1. **The chooser is a pure function over a `Session` trait.** `desktop::choose(session: &dyn Session) -> Result<Choice, Refusal>`. `Session` answers the questions the rungs ask: an environment variable (through `env_override`, blank as unset), whether a program is on `PATH`, whether a process with a given name runs (`/proc/*/comm`, and `cmdline` where the name is ambiguous), whether a name is owned on the session bus, the X11 facts of decision 7, whether the Wayland registry advertises a global, and the portal facts of decision 10. The live implementation (`desktop/probe.rs`, Linux only) answers each lazily and caches for the length of one choice, so a session decided by its first token never opens an X or Wayland connection. Tests implement `Session` over a table of fabricated answers, on every platform. `Choice` carries the `Backend` and an `evidence: Vec<String>` that reads as a sentence ("Wayland session; wlr-layer-shell advertised; swaybg on PATH"). `Refusal` carries every rung that was tried and why it declined. `check_supported` and `set_wallpaper_job` both call `choose`, as both call `detect` today, for the same reason: the session can change under a running sink.

2. **Named desktops first, gated by two signals.** The table stays the first stage and keeps its token walk in the session's own order. A row applies when its token matches and its setter is present, and for the rows decision 3 names, when the process that draws the key runs. A row whose gate fails does not refuse: the walk continues with the next token, then with the ladder. When `XDG_CURRENT_DESKTOP` is empty, `XDG_SESSION_DESKTOP` and then `DESKTOP_SESSION` are walked the same way. This is also what fixes Regolith: `Regolith-Wayland:GNOME:sway` reaches the GNOME row, finds no GNOME shell, and walks on to `sway`.

3. **The gsettings rows that get a process gate: GNOME and Cinnamon only.** `gsettings set` succeeds silently when nothing draws the key, which is harmless while a refusal is the alternative and wrong once a ladder is. The GNOME row requires `gnome-shell` or `gnome-flashback`; the Cinnamon row requires `cinnamon`. Both are desktops the guest boots, so the gate is observed rather than guessed. `unity` leaves the GNOME row for a row of its own with the same mechanism and no gate, since no process name for Unity was verified. MATE and Budgie stay ungated for the same reason; open question 1.

4. **Five new rows.**

   | Row | Tokens | Setter | Reach | Evidence tier |
   |---|---|---|---|---|
   | sway | `sway`, or `SWAYSOCK` set with no earlier token matched | `swaymsg output * bg <path> fill` | `OneImage` | exercised in step 7 |
   | Hyprland | `hyprland`, gated on hyprpaper's socket | `hyprctl hyprpaper preload <path>`, then `hyprctl hyprpaper wallpaper ",<path>"`, then `unload unused` | `OneImage` | reviewed, not run |
   | LXDE | `lxde` | `pcmanfm --set-wallpaper=<path> --wallpaper-mode=crop` | `OneImage` | reviewed, not run |
   | Deepin | `deepin` | `dbus-send` to `org.deepin.dde.Appearance1.SetMonitorBackground`, once per monitor by its `xrandr` name; `com.deepin.daemon.Appearance` where only that name is on the bus | `PerMonitor` | reviewed, not run |
   | Trinity | `tde` | `dcop kdesktop KBackgroundIface setWallpaper <path> 6` | `OneImage` | reviewed, not run |

   Hyprland without hyprpaper falls through to the Wayland ladder, which is where awww users on Hyprland land. The hyprpaper `unload unused` keeps the daemon from holding every image it was ever given; the exact keyword is checked against hyprpaper's documentation in step 2. The Trinity mode number and the pcmanfm mode name are taken from their documentation in step 2 and pinned by tests. `TDE:KDE` now matches Trinity before KDE because the walk follows the session's order.

5. **The ladder, after the table.** First working rung wins.

   | Session | Rung | Applies when |
   |---|---|---|
   | Wayland (`WAYLAND_DISPLAY` set) | awww | its socket exists and `awww` (or `swww`) is on `PATH` |
   | | wpaperd | its socket exists and `wpaperctl` is on `PATH` |
   | | swaybg | `zwlr_layer_shell_v1` is advertised and `swaybg` is on `PATH` |
   | X11 (`DISPLAY` set, no `WAYLAND_DISPLAY`) | desktop owner | a desktop window exists and its owner has a row (decision 7) |
   | | root pixmap | no desktop window, and the window manager is not a compositing shell (decision 7) |
   | either | portal | decision 10 |

   A running daemon comes before `swaybg` so the app does not start a second background surface under one the user already runs. Both socket names are accepted for awww, `swww-$WAYLAND_DISPLAY.socket` and the `awww` spelling, because the rename's socket path was not confirmed; step 2 reads the awww source for it. The wpaperd command is taken from `wpaperctl`'s documentation in step 2.

6. **`SUNLIT_EARTH_WALLPAPER_SETTER` forces a setter.** Its value is a setter name (`gnome`, `kde`, `xfce`, ..., `sway`, `awww`, `wpaperd`, `swaybg`, `root-pixmap`, `portal`), read through `env_override`. A forced setter skips detection but not its own presence check, so a forced `swaybg` with no `swaybg` installed is still a refusal, naming the variable. An unknown name is a refusal that lists the valid ones. It is the escape hatch for a session the ladder misjudges and the switch the e2e suite uses to exercise a rung deterministically. Documented under the environment knobs in `docs/architecture.md`.

7. **The X11 root pixmap, natively.** A `Kind::RootPixmap` with `Reach::PerMonitor` that needs no program. Its placement composes every monitor's image into one root-sized image at each monitor's RandR rectangle, which covers all three modes (the view across screens is the canvas itself), and writes no PNG at all, because nothing reads one; the sink skips `Publication` for this kind. The X side follows feh and xwallpaper (research section 6): a new connection; create a pixmap of the root's depth; upload in strips that fit the server's maximum request length (x11rb's `image` feature if it does this, otherwise a loop of `PutImage`); read `_XROOTPMAP_ID` and `ESETROOT_PMAP_ID`, and if both name the same pixmap, `KillClient` that id; set both atoms, set the root's background pixmap, clear the root; set the close-down mode to `RetainPermanent` and disconnect. The pixel conversion to the root visual's byte order is a pure function with its own tests. The image buffer lives only for the duration of the call. The retained pixmap is held by the X server, not by this process, and is freed by the next publish; that is the external effect this decision accepts, and it is the convention every root setter uses.

   Two probes guard the rung, each read once per choice over the same connection. A desktop window: any window on `_NET_CLIENT_LIST` whose `_NET_WM_WINDOW_TYPE` includes `_NET_WM_WINDOW_TYPE_DESKTOP`. If there is one, its `WM_CLASS` picks a row (`pcmanfm` LXDE, `pcmanfm-qt` LXQt, `xfdesktop` XFCE, `nemo-desktop` Cinnamon, `caja` MATE, `plasmashell` KDE) and that row applies if its setter is present; with no match, the root pixmap is not tried, since it would be painted under something else. A compositing shell: the window manager's name from `_NET_SUPPORTING_WM_CHECK` and `_NET_WM_NAME`; a name containing `Mutter`, `Muffin`, `GNOME Shell` or `KWin` means the shell draws its own background and the root pixmap would not show, so the rung declines. A missing check window (dwm, stock xmonad) is not a reason to decline.

   x11rb gains the features this needs (`image` if used); no new crate.

8. **`swaybg` is started and owned by the sink, and outlives the app.** For each publish: spawn `swaybg -o '*' -i <path> -m fill` with stdio to null; wait up to 500 ms for it to exit, and if it did, that is the refusal, with its stderr; otherwise, after a further second, terminate every `swaybg` of this user whose command line names a file under the app's wallpaper directory, except the new one. The second's grace is Variety's, and avoids a grey frame between the two surfaces (swaybg issue #17). The grace is not spent blocking the engine loop: the old processes are ended from a one-shot thread that sleeps, signals and reaps, and a child of this process is reaped with `wait`. The app leaves its last `swaybg` running at exit, so the wallpaper stays after quitting, which is how every other desktop behaves; the next start finds it by its command line, not through a state file. A `swaybg` whose image is not ours belongs to the user and is left alone; since two surfaces on the background layer stack in an order no protocol defines, the evidence line says one was found, and open question 2 asks whether to end it. `swaybg` reads its image at startup (research, from its source), so the generation sweep deleting older files does not disturb a running instance.

9. **Dependencies for the probes.** `zbus` (blocking API, the version and features already in the lock) for the bus-name and portal questions, and `wayland-client` for one registry round trip, both as `cfg(target_os = "linux")` dependencies of `sunlit-core`. Neither adds a crate to the lock; step 1 confirms that with `cargo tree` in WSL and records it. The Wayland connection is opened only when the ladder reaches a Wayland rung and dropped after the round trip.

10. **The portal, last, registered, and distrusted where it cannot mean anything.** The rung applies when `org.freedesktop.portal.Wallpaper` is exported on `/org/freedesktop/portal/desktop`, and declines when the only portal backend on the bus that implements `org.freedesktop.impl.portal.Wallpaper` is the GTK one (`org.freedesktop.impl.portal.desktop.gtk`) and no GNOME shell runs, because that backend writes `org.gnome.desktop.background`, which nothing draws there. The sink holds one zbus connection for the portal, calls `org.freedesktop.host.portal.Registry.Register("sunlit-earth", {})` as its first call where the Registry exists (it must be the first portal call on the connection, and the id must match a desktop file basename, which `assets/linux/sunlit-earth.desktop` is), and then `SetWallpaperFile` with the anchor's image, `show-preview` false and `set-on` `background`. Without a Registry (portal older than 1.19.4) the call is still made, and the evidence says the permission is shared with other unidentified apps. The first call shows the portal's one-time dialog; a denial is stored by the portal and every later call is a silent refusal, so a response code 1 or 2 becomes a refusal that says so and tells the user where the permission can be reset. `OneImage` reach.

11. **The explanation is part of the feature.** `choose`'s evidence goes to the info log on every choice that differs from the previous one, to the status line as part of a refusal, and to `sunlit-earth displays`, which gains a `setter:` line with the evidence. A bug report on an unusual desktop then carries the one line that matters.

## Steps

Each step is a commit or two, `cargo test` and `cargo clippy --all-targets` clean on the host and in WSL, `cargo fmt --check` clean.

1. **The chooser, with no new behavior.** `Session`, the live probe, `choose`, `Choice` and `Refusal`, with the table as the only stage and the secondary variables of decision 2. `detect_current` stays as a thin wrapper while its callers move. Tests: every fabricated session the existing tests use chooses the same row with the same commands; `TDE:KDE` still reaches KDE at this step (it changes in step 2, so the change shows in that diff). Record the `cargo tree` result for decision 9.
2. **The five rows and the gates.** Decisions 3, 4 and 6, with the setter details the decisions leave to this step read from each tool's documentation and pinned by tests. Tests: each row's commands; `TDE:KDE` chooses Trinity; `Regolith-Wayland:GNOME:sway` with no `gnome-shell` chooses sway; GNOME with `gnome-shell` absent and no other token is not GNOME; the forcing variable, its unknown-name refusal and its presence check.
3. **The root pixmap.** Decision 7: the composition, the byte order conversion, the X sequence, the two probes, the sink path that writes no PNG. Unit tests for the composition (each monitor's pixels at its rectangle, the canvas for the view across screens, a monitor left alone keeps the background colour) and the conversion. A hand run in WSLg is not evidence (it is a Wayland session); the evidence is step 7.
4. **The Wayland rungs.** Decisions 5 and 8: awww and wpaperd commands, the owned `swaybg` and its lifecycle. Tests: the ladder order over fabricated sessions; the process matching that tells ours from the user's, over fabricated `/proc` entries.
5. **The portal.** Decision 10. Tests: the GTK distrust rule, and the response code mapping. The zbus calls themselves are exercised in step 7.
6. **The explanation.** Decision 11: the log line, the refusal text, the `displays` line. Test: a refusal over a fabricated empty session lists every rung with its reason.
7. **Two guest sessions, sway and i3.** The Linux image installs `sway`, `swaybg` and `i3-wm` (package names and session file basenames confirmed against trixie's archive first). `Desktop` gains `Sway` and `I3`; `Desktop::session` answers only `(Sway, Wayland)` and `(I3, X11)`, `Login::new`'s refusal stops saying every missing pair is an experimental Debian session and names what the desktop does run on, and a desktop with exactly one session gets it when `--session-type` is not given, so `--desktop sway` needs no second flag. `current_desktop` gives the boot's session check what each session sets: read it from the session files, and if i3's has no `DesktopNames`, so that sddm exports an empty `XDG_CURRENT_DESKTOP`, the check compares `XDG_SESSION_DESKTOP` for that desktop instead, which is also the variable the chooser falls back to (decision 2). `desktop.sh`'s allowlist and its Wayland directory case gain the two basenames, and `the_guest_accepts_exactly_the_sessions_the_host_can_ask_for` covers them. `--screens` works on i3, which is X11, and is refused on sway as on every Wayland session. The e2e wallpaper case learns three read-backs, for setters whose writes the command-derived read-back cannot follow (the root pixmap runs no command at all): under the root pixmap, `_XROOTPMAP_ID` names a pixmap that changes between two publishes, and the previous id is gone (a `GetGeometry` on it fails), which is the leak check; under `swaybg`, exactly one `swaybg` names a file under the wallpaper directory and it is the newest; under the portal, the GNOME key holds the file. Runs: `--desktop i3` (root pixmap by the ladder), `--desktop sway` (the sway row), `--desktop sway` with `SUNLIT_EARTH_WALLPAPER_SETTER=swaybg` (the owned swaybg), `--desktop gnome` with `SUNLIT_EARTH_WALLPAPER_SETTER=portal` (the portal and its dialog, answered by hand once through `vm view`, then a second publish that must be silent), and the six existing sessions unchanged. Each with a screendump after the wallpaper case. The image rebuild is about an hour; the user approved it for this plan on 2026-09-23.
8. **Docs.** `docs/platforms.md`: the Linux section describes the table, the gates and the ladder, the rejection of the portal is replaced by decision 10's reasoning, and the reach table gains the new rows and rungs with their evidence tier. `docs/roadmap.md`: the Linux backends item is updated with what step 7 observed; new items for per-monitor on the Wayland rungs (on top of the native Wayland display query), COSMIC waiting on cosmic-bg #109, Pantheon, UKUI and Budgie on Wayland needing a real session, and open question 2. `docs/architecture.md`: the forcing variable under the environment knobs. `docs/testing.md` and `docs/vm-setup.md`: the two sessions. `CLAUDE.md`: the desktop count and the `--desktop` list in the VM section.

## Acceptance criteria

1. The seven existing desktops choose the same setter with the same commands, by test, and the six existing guest sessions pass the suite on the rebuilt image as before.
2. In the i3 session the wallpaper case passes through the root pixmap, the screendump shows the globe, and the leak check passes.
3. In the sway session the wallpaper case passes through the sway row, and again through the owned `swaybg` when forced, with exactly one of ours running afterwards; quitting the app leaves the wallpaper on screen.
4. In the GNOME session with the portal forced, the first publish shows one dialog and the second is silent.
5. `sunlit-earth displays` prints the chosen setter and its evidence in each of the eight sessions.
6. A fabricated session that no rung accepts is refused with every rung and its reason, before anything is rendered.
7. `cargo test`, `cargo clippy --all-targets` and `cargo fmt --check` green on Windows and in WSL.

## Risks

- A gate misjudges a working desktop and pushes it down the ladder. Limited to the two gated rows, both observed in the guest, and the forcing variable is the user's way out.
- The root pixmap is chosen where something else paints the background in a way neither probe detects: a desktop window without the EWMH type, or a shell not on decision 7's list. The wallpaper then silently does not show. The evidence line in the log and in `displays` is what makes that diagnosable, and the forcing variable what makes it fixable.
- `PutImage` sizing. A 4K root image is about 33 MB and larger than a single request, even with BIG-REQUESTS; the strip loop is the answer and the i3 guest at its console resolution is the smallest real test. A multi-monitor test at a larger canvas is `--screens` on the i3 session, which is X11 and supports it.
- `swaybg` processes accumulate if the terminate path fails. The command-line match finds all of ours on every publish, so a leak lasts one publish; the swaybg read-back pins it.
- The portal's dialog appears in an unattended session at an unexpected moment. It is once per install, it says which app is asking once registered, and the portal is the last rung, so a session with any other working mechanism never sees it.
- The Hyprland, LXDE, Deepin and Trinity rows are written from documentation, as MATE, LXQt and Budgie were, and `docs/platforms.md` says so.

## Open questions

1. Whether MATE and Budgie rows get a process gate, once someone confirms which process draws their key.
2. Whether a foreign `swaybg` found running should be ended after ours is up (wallutils ends every `swaybg`; Variety only its own). The plan leaves it running and says so in the evidence.
3. Whether the portal rung should be offered at all where the Registry is missing, given the shared permission record.

## Implementation record

What the steps left to be read from documentation, as it was read on 2026-09-23, and what step 1 confirmed.

- Decision 9: `cargo tree` in WSL with `zbus` 5.14 (`async-io`, `blocking-api`, no default features) and `wayland-client` 0.31 added as Linux dependencies of `sunlit-core`, and `image` added to x11rb's features: `Cargo.lock` gains two dependency lines under `sunlit-core` and no package. x11rb's `Image::put` splits an upload to fit the server's maximum request length itself, so decision 7's strip loop is x11rb's.
- pcmanfm: `--set-wallpaper=FILE` and `--wallpaper-mode=MODE`, MODE one of `color|stretch|fit|crop|center|tile|screen`, `crop` being "stretch and crop to fill monitor" (`src/pcmanfm.c`, `data/pcmanfm.1.in`). lxsession sets `XDG_CURRENT_DESKTOP=LXDE`.
- Trinity: `KBackgroundSettings::WallpaperMode` counts from `NoWallpaper` = 0, so `Scaled` is 6 and `ScaleAndCrop` is 8 (`kcontrol/background/bgsettings.h`); the row passes 8, which zooms to cover as every other row does. `KBackgroundIface` has `setWallpaper(TQString wallpaper, int mode)` (`kdesktop/KBackgroundIface.h`), and `starttde` sets `XDG_CURRENT_DESKTOP=TDE`.
- Deepin: `SetMonitorBackground(monitorName, imageGile)` accepts a plain path or a `file://` URI, since the daemon decodes only what parses as a URI (`dde-appearance`, `appearancemanager.cpp`, `utils.cpp`). The row passes a plain path.
- sway: `output <name> bg <file> <mode>` with `*` for every output (`sway-output(5)`). sway does not set `XDG_CURRENT_DESKTOP` itself; its session file carries `DesktopNames=sway;wlroots`. `swaybg -o '*' -i <path> -m fill` is the documented form, and swaybg's default mode is `stretch`, so `-m fill` is needed.
- i3's session file carries `DesktopNames=i3`, so sddm exports `XDG_CURRENT_DESKTOP=i3` and step 7's fallback to `XDG_SESSION_DESKTOP` for the boot check is not needed for i3.
- awww: `awww img <path> --transition-type none`; `none` is an alias for `simple` with a step of 255, which finishes at once, and `--resize` defaults to `crop` (`client/src/cli.rs`). The daemon's socket is `$XDG_RUNTIME_DIR/<basename of WAYLAND_DISPLAY>-awww-daemon.sock`, and in the last swww release (0.11.2) `...-swww-daemon.sock`; the rename shipped no `swww` binary (`common/src/ipc/socket.rs` in both, awww's changelog for 0.12.0).
- wpaperd: `wpaperctl set <path> [monitor...]`, every monitor when none is named (`cli/src/opts.rs`), over `$XDG_RUNTIME_DIR/wpaperd.sock` (`ipc/src/lib.rs`; the `xdg` crate's `get_runtime_directory` does not apply the prefix).
- hyprpaper: listens on `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.hyprpaper.sock` (`src/ipc/IPC.cpp`). Its IPC is in departure 1.
- The portal: `SetWallpaperFile(s parent_window, h fd, a{sv} options) -> o handle` on `org.freedesktop.portal.Desktop` at `/org/freedesktop/portal/desktop`, where `org.freedesktop.host.portal.Registry.Register(s app_id, a{sv} options)` also lives. A stored denial answers with response 2, not 1 (`src/wallpaper.c` in 1.20.3, permission table and id both `wallpaper`), and `flatpak permission-remove wallpaper wallpaper <app id>` resets it. Debian trixie ships xdg-desktop-portal 1.20.3, which has the Registry.

## Departures

1. **hyprpaper takes one call, not three.** Decision 4 wrote the Hyprland row as `preload`, `wallpaper` and `unload unused`. hyprpaper 0.8 was rewritten around a new IPC and has neither `preload` nor `unload` any more: its protocol schema has only a wallpaper object, a search of its source finds neither word, and its wiki documents `hyprctl hyprpaper wallpaper '[mon], [path], [fit_mode]'` alone. The row therefore runs `hyprctl hyprpaper wallpaper ", <path>, cover"`, an empty monitor being every monitor and `cover` the zoom every other row uses. A hyprpaper older than 0.8 would refuse an image it was not given with `preload` first, and the refusal carries hyprctl's own message; supporting both would need the version, which hyprctl does not report through the socket the gate checks, and the row was reviewed, not run, either way. How `hyprctl` passes the text to the new protocol was not found in Hyprland's source, so the evidence tier stays "reviewed, not run".
2. **Deepin also answers to `DDE`.** Decision 4 gave the Deepin row the token `deepin` with the capitalization inferred. No Deepin source that sets `XDG_CURRENT_DESKTOP` was found; `xdg-desktop-portal-dde` ships `UseIn=DDE`, which only works if sessions call themselves `DDE`, while Variety and wallutils match `deepin`. The row takes both tokens, which costs nothing, since no other desktop uses either.
