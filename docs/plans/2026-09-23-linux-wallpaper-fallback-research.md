# Research: Setting the Wallpaper on Linux Desktops Outside the Table

Code reconnaissance and four web research passes, all on 2026-09-23, with the claims the recommendation rests on checked against upstream source afterwards. The plan that uses this is [2026-09-23-linux-wallpaper-fallback-plan.md](2026-09-23-linux-wallpaper-fallback-plan.md). Every claim says how far it was verified: read in source, stated by upstream documentation, reported by a forum or bug tracker, or inferred. "Unverified" means somebody looked and could not confirm it.

The goal this serves: a desktop in the table keeps its preferred setter, and a session outside it is probed for the mechanism most likely to work instead of being refused.

## 1. What the code does today

Read in the current tree.

- `desktop::detect` (`crates/sunlit-core/src/desktop/mod.rs:430`) splits `XDG_CURRENT_DESKTOP` on `:` and matches each token exactly, case-insensitively, against `BACKENDS`: KDE Plasma, XFCE, Cinnamon, MATE, LXQt, Budgie, GNOME (with `unity`, `gnome-classic`, `gnome-flashback`). No other variable is read.
- `check_supported` (`wallpaper/linux.rs:143`) runs before any render (`engine/publish.rs:43`). It refuses when no row matches, and also when the row's program is not on `PATH`. The refusal text lists the supported desktops.
- Monitors come from `xrandr --query` (`display::outputs`) and changes from an x11rb RandR subscription. Where neither answers, the sink plans for a 2560x1440 default (`engine/wallpaper_sink.rs:175`).
- The app sets wallpapers only as a long-running process. `render` and `displays` are the only one-shot subcommands (`sunlit-app/src/cli.rs:112`).
- `docs/platforms.md` records the XDG desktop portal as rejected because "it puts a confirmation dialog in front of every set". Section 5 shows that is only true with `show-preview: true`.

## 2. Prior art

Read in each project's source.

| Project | Detection | Unknown desktop |
|---|---|---|
| xyproto/wallutils (Go) | No central detection. Each backend has `Running()` (env probe) and `ExecutablesExists()` (`PATH`); the first backend where both hold is used, and a failure falls through to the next. Order: Hyprpaper, Sway, Deepin, Xfce4, Mate, Cinnamon, Plasma, Gnome3, Gnome2, Pekwm, PCManFMQt, SwayBG, Weston, Feh. Substring match over `GDMSESSION`, `XDG_SESSION_DESKTOP`, `XDG_CURRENT_DESKTOP`, `DESKTOP_SESSION`. Hyprpaper also checks its socket exists; PCManFMQt checks the process table for `pcmanfm-qt --desktop`. | SwayBG for any Wayland session (`WAYLAND_DISPLAY` or `XDG_SESSION_TYPE=wayland`), then feh, whose `Running()` is always true. `pkill swaybg` before starting a new one. |
| reujab/wallpaper.rs (Rust) | Exact match on `XDG_CURRENT_DESKTOP` only. | Spawn `swaybg`; if that fails, `feh --bg-fill` without checking it exists. |
| more-wallpapers (Rust) | `SWAYSOCK` wins outright; then exact `XDG_CURRENT_DESKTOP`; then `XDG_SESSION_TYPE`. | X11: xrandr plus `xwallpaper --output <name>`. Wayland outside a short list: error. The only prior art with per-monitor as its core model. Its Cinnamon backend re-sets the X11 wallpaper in a loop for about 900 ms because Cinnamon resets it. |
| Variety `set_wallpaper` (bash) | Lower-cased substring match on `XDG_CURRENT_DESKTOP` over a long list (gnome, unity, budgie, kde, xfce, lxde, lxqt, mate, cinnamon, deepin, trinity, fluxbox, sway, hyprland, enlightenment, moksha); `awesome` from `XDG_SESSION_DESKTOP`/`DESKTOP_STARTUP_ID`; COSMIC by exact equality. | `feh --bg-fill`, else `nitrogen`, and always `exit 0`, so a total failure is silent. For wlroots: `wpaperd` if running (`wpaperctl set`), else `awww` if running (`awww img`), else start a new `swaybg` and kill the old one a second later, to avoid a grey flash (swaybg issue #17). Its KDE branch leaves containments whose wallpaper plugin is not `org.kde.image` alone. |
| pywal (Python) | First set of `XDG_CURRENT_DESKTOP`, `DESKTOP_SESSION`, `GNOME_DESKTOP_SESSION_ID`, `MATE_DESKTOP_SESSION_ID`, `SWAYSOCK`, `DESKTOP_STARTUP_ID`, then substring match. | First of `feh`, `xwallpaper`, `hsetroot`, `nitrogen`, `bgs`, `habak`, `display` on `PATH`; none found is a logged no-op. |
| swww/awww | No desktop detection at all: it works wherever `zwlr_layer_shell_v1` can be bound, and its README says it will not run on GNOME. Renamed to awww and moved to Codeberg in October 2025. | n/a |

What they agree on: `XDG_CURRENT_DESKTOP` first; for anything unknown, a generic setter chosen by what is installed or running, feh-like on X11 and layer-shell tools on Wayland. What none of them does: probe D-Bus names, check for a desktop window, use the portal, or set the X11 root pixmap themselves. Only wallutils and more-wallpapers require two independent signals (session says so, tool exists) before choosing a backend. None verifies the wallpaper actually changed.

Sources: https://github.com/xyproto/wallutils (`wallpaper.go`, `backend.go`, `swaybg.go`, `hyprpaper.go`, `pcmanfmqt.go`, `feh.go`), https://github.com/reujab/wallpaper.rs/blob/master/src/linux/mod.rs, https://github.com/LuckyTurtleDev/more-wallpapers (`src/linux/*.rs`), https://github.com/varietywalls/variety/blob/master/variety/data/scripts/set_wallpaper, https://github.com/dylanaraps/pywal/blob/master/pywal/wallpaper.py, https://github.com/LGFae/swww/blob/main/README.md

## 3. Full desktops missing from the table

| Desktop | `XDG_CURRENT_DESKTOP` | Setter | Evidence |
|---|---|---|---|
| LXDE | `LXDE` | `pcmanfm --set-wallpaper=<path> --wallpaper-mode=<mode>`, needs pcmanfm managing the desktop | docs (pcmanfm(1)) |
| Deepin / DDE | `Deepin` (capitalization inferred) | D-Bus `org.deepin.dde.Appearance1` at `/org/deepin/dde/Appearance1`, `SetMonitorBackground(s monitor, s file)`: per monitor by name. Older releases: `com.deepin.daemon.Appearance` at `/com/deepin/daemon/Appearance`, same method | source (linuxdeepin/dde-appearance `appearance1.h`); older one docs |
| Trinity | `TDE`, sometimes `TDE:KDE` | `dcop kdesktop KBackgroundIface setWallpaper <path> <mode>` | docs and mailing list. Note `TDE:KDE` would match the KDE row today and run a Plasma script against Trinity |
| UKUI | `UKUI` (inferred) | Reportedly `org.mate.background picture-filename`, watched by ukui-settings-daemon | an AI code summary (DeepWiki), medium confidence |
| Unity | `Unity`, `Unity:Unity7` on 22.04 and later | GNOME schema; already covered by the `unity` token | forum and Launchpad #1680008 |
| Pop!_OS on GNOME, Zorin, Endless | `pop:GNOME`; Zorin reported both as `GNOME` and `zorin:GNOME`; Endless unverified | GNOME schema; already covered by walking the colon list | forum |
| Regolith on Wayland | `Regolith-Wayland:GNOME:sway` | `regolith.wallpaper.file` in `~/.config/regolith3/Xresources`, then `regolith-look refresh` | source (regolith-session-wayland). Today this matches GNOME and writes gsettings nobody draws from |
| Pantheon | `Pantheon` | Unverified: GNOME schema or an `io.elementary` one, conflicting reports | unverified |
| Enlightenment / Moksha | `Enlightenment` (inferred) | `enlightenment_remote -desktop-bg-add ...` takes an `.edj`, not a PNG, so a PNG needs packaging with `edje_cc` first | mailing list |
| Lomiri | `Lomiri` | No known unattended method; it moved off the GNOME key to an internal one, name not found | unverified |
| COSMIC | `COSMIC` | No CLI or D-Bus call (cosmic-bg issue #109 open). cosmic-bg reads its cosmic-config files and reportedly rebuilds on change, so writing the config is possible but undocumented. COSMIC's compositor implements layer-shell | issue tracker; config watching from a secondary source only |
| Budgie 10.9 and later on Wayland | unverified | X11 Budgie uses the GNOME schema. Budgie 10.10 on Wayland reportedly draws its background with swaybg; whether the GNOME key still reaches it is unverified | unverified |

A gsettings row needs more than its schema. `gsettings list-schemas` says only that the compiled schema is installed, and `gsettings set` succeeds silently when the daemon that draws the key is not running (linuxmint/cinnamon#6799, NixOS reports). The schema is a necessary signal, not a sufficient one: the owning process or bus name has to be alive too. This matters for probing: several unrelated desktops ship the GNOME schemas.

## 4. Wayland compositors outside the table

| Compositor | Identification | Native setter | Evidence |
|---|---|---|---|
| sway | `XDG_CURRENT_DESKTOP=sway` (set by the session, not hardcoded); `SWAYSOCK` | `swaymsg output <name\|*> bg <path> fill`, which runs swaybg itself; per output | docs |
| Hyprland | `XDG_CURRENT_DESKTOP=Hyprland`, set by the compositor; `HYPRLAND_INSTANCE_SIGNATURE` | hyprpaper, if running: `hyprctl hyprpaper preload <path>` then `hyprctl hyprpaper wallpaper "<monitor>,<path>"`; per monitor | docs |
| niri | `niri` via `niri-session` (not confirmed hardcoded); `NIRI_SOCKET` | none; users run swaybg, wpaperd or awww | docs |
| river | `river` from the session | none; swaybg recommended | docs |
| Wayfire | `Wayfire` (unverified); `WAYFIRE_SOCKET` only with its IPC plugin | `wf-background` from wf-shell, configured in `wayfire.ini` | docs; its protocol unverified |
| labwc | `labwc:wlroots` | none; swaybg in `~/.config/labwc/autostart` | docs, secondary |
| COSMIC | `COSMIC` | see section 3 | |

Wallpaper daemons a session may already run:

| Daemon | Change at runtime | Per output | Detect |
|---|---|---|---|
| swaybg (C, raw wayland-client) | no IPC: start a new one, kill the old | yes | process only |
| hyprpaper | `hyprctl hyprpaper ...` | yes | its socket, Hyprland only |
| wpaperd (Rust, sctk 0.20) | `wpaperctl set` over its socket | yes | socket, or `~/.local/state/wpaperd/` |
| swww / awww (Rust) | `swww img <path> [-o outputs]` | yes | `swww query`; socket `$XDG_RUNTIME_DIR/swww-$WAYLAND_DISPLAY.socket` |
| wbg | no IPC, restart | unverified | process only |

swaybg read in source (`main.c`); wpaperd and swww read in their `Cargo.toml`; the rest from docs.

`zwlr_layer_shell_v1` is implemented by sway, Hyprland, niri, river, Wayfire, labwc, KWin and COSMIC's compositor (Wayland Explorer's compatibility table, docs level). Mutter does not implement it, which every source agrees on; the tracking issue (GNOME/mutter#973) could not be read in full. No `ext-` protocol for placing a background exists; `ext-background-effect-v1` is blur and unrelated.

Two consequences for this app:

- Output names. Under Xwayland, `xrandr` reports synthetic `XWAYLAND0`, `XWAYLAND1`, not the compositor's connector names (corroborated by several forums, not by Xwayland source). Every per-output setter above takes the compositor's names, so per-monitor work on these compositors needs `wl_output` v4 `name` (with `xdg-output` for logical geometry) or the compositor's own IPC (`swaymsg -t get_outputs`, `niri msg outputs`) instead of `display::outputs`. A session without Xwayland answers `xrandr` with nothing, and the sink falls back to 2560x1440.
- Being the daemon. The app could create one background-layer surface per output itself through `smithay-client-toolkit`, which is already in the dependency tree through winit; wpaperd and cosmic-bg do exactly that in Rust. That would cover every compositor in the list with no external program. It also means the wallpaper exists only while the app runs, and conflicts with the rule in CLAUDE.md that decoded pixel buffers are never held in long-lived structs: a `wl_shm` buffer has to stay valid until the compositor releases it, and whether a surface keeps showing its content after its buffer is destroyed is compositor-dependent (inferred from the protocol, not verified). What happens when the app's surface and an already running swaybg or cosmic-bg share the background layer is also unverified.

Sources: https://wiki.archlinux.org/title/Sway, https://wiki.hypr.land/Hypr-Ecosystem/hyprpaper/, https://github.com/niri-wm/niri/wiki/Important-Software, https://wiki.archlinux.org/title/River, https://github.com/WayfireWM/wf-shell, https://github.com/swaywm/swaybg/blob/master/main.c, https://github.com/danyspin97/wpaperd, https://www.lgfae.com/posts/2025-10-29-RenamingSwww.html, https://wayland.app/protocols/wlr-layer-shell-unstable-v1, https://wayland.app/protocols/xdg-output-unstable-v1, https://gitlab.gnome.org/GNOME/mutter/-/issues/973

## 5. The XDG desktop portal

Read in source on xdg-desktop-portal `main` (`desktop-portal/wallpaper.c`, `shared/xdp-app-info-host.c`, `data/org.freedesktop.host.portal.Registry.xml`, `NEWS.md`) unless marked otherwise.

- `SetWallpaperURI` / `SetWallpaperFile` take `show-preview` (default false) and `set-on` (`background`, `lockscreen`, `both`).
- With `show-preview` false the frontend reads a stored permission. `NO` refuses silently. `UNSET` shows one generic access dialog ("Allow %s to Set Backgrounds?") and stores the answer. `YES` goes straight to the backend with no dialog. So the rejection in `docs/platforms.md` holds only for `show-preview: true`: an unattended refresh costs one approval, once.
- The permission is keyed by app id. A host (non-Flatpak) process gets its id from its systemd user unit, only when that unit is named `app-<launcher>-<id>-<random>.scope` or `app-<id>.service`; otherwise the id is empty, and every unidentified host app shares one record, so another program's "Deny" would silently deny this one. `org.freedesktop.host.portal.Registry.Register(app_id, options)` fixes the id, once per connection and before any other portal call. It was introduced in 1.19.4 and first shipped stable in 1.20.0; which distributions carry 1.20 was not checked.
- The interface is exported only when a configured backend implements `impl.portal.Wallpaper` and an access dialog implementation exists, so its presence on `/org/freedesktop/portal/desktop` is a usable probe on current releases. Older releases had introspection problems (portal #686, PR #766, docs level).
- Backends: GNOME and GTK implement it; KDE reportedly since about September 2025 (KDE bug 485966, read by a research pass, not rechecked); xapp (Cinnamon, MATE, Xfce) reportedly implements it without its own dialog; wlr, hyprland, cosmic and lxqt had no implementation in either pass. The GTK backend, read in source (`xdg-desktop-portal-gtk/src/wallpaper.c`), sets `org.gnome.desktop.background picture-uri` and `picture-options=zoom`. So a successful portal call on a session served by the GTK backend, for example a bare window manager, says nothing about whether anything is drawn: the portal can report success and nothing changes on screen.
- The portal has no per-monitor or span option; it is one image on every screen.

Sources: https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Wallpaper.html, https://github.com/flatpak/xdg-desktop-portal/blob/main/desktop-portal/wallpaper.c, https://github.com/flatpak/xdg-desktop-portal/blob/main/shared/xdp-app-info-host.c, https://github.com/flatpak/xdg-desktop-portal/blob/main/data/org.freedesktop.host.portal.Registry.xml, https://github.com/flatpak/xdg-desktop-portal-gtk/blob/main/src/wallpaper.c

## 6. X11 sessions with a plain window manager

Read in the source of feh (`src/wallpaper.c`), Esetroot, hsetroot and xwallpaper (`main.c`, `outputs.c`) unless marked otherwise.

The root pixmap convention, in the order feh, Esetroot and xwallpaper agree on:

1. Draw the new image into a new pixmap the size of the root window.
2. Read `_XROOTPMAP_ID` and `ESETROOT_PMAP_ID` from the root (interned with `only_if_exists`). If both name the same pixmap, `KillClient` that resource id: it belongs to the previous setter's connection, which closed with `RetainPermanent`, so this frees exactly the old pixmap.
3. Set both atoms to the new pixmap, set it as the root's background pixmap, and clear the root window.
4. Flush, set the close-down mode to `RetainPermanent`, disconnect. feh and Esetroot use a second, short-lived connection for this, so the pixmap always belongs to a connection that is gone.

hsetroot's blanket `KillClient(AllTemporary)` variant is the fragile one (a misspelled atom broke its cleanup for years, hsetroot#35). The exact-id version is what keeps a refresh every few minutes from leaking one full-screen pixmap per update.

- Multi-monitor comes free: one pixmap spans the screen and each monitor's image is drawn at its rectangle. xwallpaper takes rectangles from RandR, feh, hsetroot and Esetroot from Xinerama. Every `Reach` is possible, including per-monitor and the view across screens.
- Compositors (xcompmgr in source, picom by documentation) and pseudo-transparent terminals read `_XROOTPMAP_ID` and watch it with `PropertyNotify`, so the atoms must change with every new pixmap.
- The root is hidden where something draws a desktop window: xfdesktop, nautilus, caja, nemo-desktop, `pcmanfm --desktop`, and full desktops generally. EWMH marks that window `_NET_WM_WINDOW_TYPE_DESKTOP`, so walking `_NET_CLIENT_LIST` for it tells whether the root pixmap would be visible (spec is docs; using it as this probe is inferred).
- The window manager's name is in `_NET_WM_NAME` on the window `_NET_SUPPORTING_WM_CHECK` points at. dwm and stock xmonad set no check window at all, which is itself a signal.
- Under Xwayland the root pixmap is not shown (architecture and several reports, not a spec statement).
- External tools instead: `xwallpaper --output <name> <file>` touches one output and writes no state file. `feh --bg-*` writes `~/.fehbg` unless `--no-fehbg`, and nitrogen `--save` writes a config that `nitrogen --restore` in an autostart replays over the app's image at login. Doing it through x11rb, which the app already links, needs neither.

Sources: https://github.com/derf/feh/blob/master/src/wallpaper.c, https://github.com/bbidulock/esetroot/blob/master/Esetroot.c, https://github.com/himdel/hsetroot/blob/master/hsetroot.c, https://github.com/stoeckmann/xwallpaper (`main.c`, `outputs.c`), https://github.com/raspberrypi-ui/xcompmgr/blob/master/xcompmgr.c, https://specifications.freedesktop.org/wm/latest/

## 7. Usage share

One dated data point: GamingOnLinux profile statistics as of 2025-02-02, 2,142 respondents: KDE Plasma 43.2 %, GNOME 26.4 %, window manager only 8.9 %, Cinnamon 7.9 %, Xfce 6.4 %, MATE 2.2 %, COSMIC 1.0 %, Budgie 0.8 %. A self-selected gaming audience, not representative. It still puts window-manager-only sessions, which today are refused, above Xfce, MATE and Budgie combined, rows the table already carries. No distribution-wide figures were found. Source: https://www.gamingonlinux.com/index.php?module=statistics&view=monthly

## 8. What this suggests

Inferences from the above, for the plan to decide.

- Detection by signals, not by name alone. Keep the table for named desktops, and gate every row, new or old, on two signals: the session names it, and its setter exists; for gsettings rows, the owning shell's bus name or process is alive too. Where `XDG_CURRENT_DESKTOP` is empty, read `XDG_SESSION_DESKTOP` and `DESKTOP_SESSION` before giving up.
- Cheap rows to add from documented setters: LXDE, Deepin (per monitor), sway (per output), Hyprland through hyprpaper when it runs, Trinity (and stop `TDE:KDE` reaching the Plasma row). COSMIC, Pantheon, UKUI, Regolith, Enlightenment and Lomiri need a real session before anything is written for them.
- For a session outside the table, a probe ladder, first working rung wins:
  - Wayland (`WAYLAND_DISPLAY`): a running daemon with IPC (awww, wpaperd, hyprpaper); compositor IPC (`SWAYSOCK`); `swaybg` on `PATH` with layer-shell advertised, started new and the old one killed afterwards; or the app's own layer-shell surface.
  - X11 without a desktop window: the root pixmap through x11rb, which reaches every monitor and needs no program installed.
  - X11 with a desktop window: identify its owner (pcmanfm, xfdesktop, caja, nemo) and use that setter.
  - The portal where it is exported and its backend is not GTK on a non-GNOME session, registered under a fixed app id, as the rung before refusing.
  - Then refuse, naming what was probed and found.
- Monitor enumeration has to follow the setter: the compositor's names on Wayland, RandR on X11. Otherwise per-monitor rows on wlroots address `XWAYLAND0`.
- Owning the surface versus spawning swaybg is the one architectural choice here, and the memory rule makes it more than a detail.
- Testing: the table is already tested against fabricated sessions, and the probes can be too, if what they read (env, `PATH`, process list, bus names, X properties, Wayland globals) comes in as one snapshot. For real coverage, sway and a bare X11 window manager such as i3 or Openbox are small additions to the Linux guest image, and would exercise the Wayland and root pixmap rungs.
