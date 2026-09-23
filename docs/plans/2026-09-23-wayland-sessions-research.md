# Research: KDE and GNOME Wayland Sessions in the Linux Test Guest

Code reconnaissance and one web research pass, both on 2026-09-23. The plan that uses this is [2026-09-23-wayland-sessions-plan.md](2026-09-23-wayland-sessions-plan.md). Every claim below says how far it was verified: read from the archive or the source, stated by upstream documentation, or inferred.

## 1. What the image already has

Read from packages.debian.org file lists and dependency tables for trixie.

- `plasma-workspace` 4:6.3.6-2 ships both `/usr/share/xsessions/plasmax11.desktop` and `/usr/share/wayland-sessions/plasma.desktop`. It hard-depends on `kwin-wayland`, `kwin-common` and `xwayland`, so the image, which installs `plasma-workspace`, already holds the Plasma Wayland compositor and Xwayland. The `kwin-x11` gap `desktop.sh` works around is the opposite one: X11's window manager is the half nothing pulls in. There is no `plasma-workspace-wayland` package in trixie.
- `gnome-session` 48.0-1 ships `/usr/share/wayland-sessions/gnome.desktop` and `/usr/share/wayland-sessions/gnome-wayland.desktop`, identical apart from `Name=`, and hard-depends on `xwayland`. `gnome-session-xsession`, which the image installs for `gnome-xorg`, also ships `/usr/share/xsessions/gnome.desktop`.
- `kscreen-doctor` is in `libkscreen-bin` (4:6.3.4-1). `gnome-monitor-config` is not in the Debian archive at all; Mutter's layout is changed through `org.gnome.Mutter.DisplayConfig.ApplyMonitorsConfig` on the session bus, after `GetCurrentState` for the serial.

So the Wayland sessions need no new packages. What they need is the selector to name them.

Sources: https://packages.debian.org/trixie/plasma-workspace, https://packages.debian.org/trixie/amd64/kwin-wayland/filelist, https://packages.debian.org/trixie/amd64/gnome-session/filelist, https://packages.debian.org/trixie/amd64/gnome-session-xsession/filelist, https://sources.debian.org/src/gnome-session/48.0-1%2Bdeb13u1/data/, https://packages.debian.org/trixie/libkscreen-bin

## 2. sddm and the session name

- `[General] DisplayServer` in `sddm.conf(5)` selects the display server for the greeter only. The image's `DisplayServer=x11` does not decide the session type an autologin starts. Source: https://man.archlinux.org/man/sddm.conf.5
- sddm issue #837, open: autologin resolves `Session=<name>` by looking in the X11 session directory before the Wayland one, matching on basename. A name present in both directories starts the X11 session. `gnome` is such a name in this image; `gnome-wayland` and `plasma` are not. Source: https://github.com/sddm/sddm/issues/837
- sddm issue #1407: autologin into Plasma Wayland failing, traced by its reporter to `getty@ttyN` units racing for the VT. Environment specific, recorded as a risk. Source: https://github.com/sddm/sddm/issues/1407
- GNOME Wayland started by a display manager other than GDM: reports are mixed and anecdotal, and no bug with a root cause was found. Unverified either way, which is why the plan tests it by hand before anything is built.

## 3. Compositors on virtio-gpu with llvmpipe

Mesa's `kms_swrast` gives GBM and EGL over any KMS node, virtio_gpu's included, rendered by llvmpipe; QEMU's documentation says llvmpipe works out of the box with virtio-gpu, and virgl is not needed. KWin and Mutter both avoid atomic mode setting on virtual GPUs for the cursor hotspot problem, so no `KWIN_DRM_*` or `MUTTER_*` variable should be required. The atomic denylists are confirmed through secondary sources only, not the merged diffs. Sources: https://www.qemu.org/docs/master/system/devices/virtio/virtio-gpu.html, https://bugs.kde.org/show_bug.cgi?id=443357

## 4. The session's environment

- Plasma: `plasma-kwin_wayland.service` starts KWin with `--xwayland`, so Xwayland is up from session start, with a generated Xauthority. KWin adds the local user to xhost itself (KDE bug 442362, fixed in 5.23.4). That `DISPLAY` and `XAUTHORITY` reach an XDG autostart program is inferred from those two facts, not documented.
- GNOME: Xwayland starts on demand, the default since GNOME 40. Mutter reserves the display and sets `DISPLAY` at startup and forks Xwayland on the first connection; its Xauthority is `$XDG_RUNTIME_DIR/.mutter-Xwaylandauth.<random>`, and it also adds the local user to xhost. Whether `DISPLAY` and `XAUTHORITY` are already in the environment an autostart program gets is unverified for GNOME 48.
- A process started over SSH as the session's user, with `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY` copied from the session, reaches the compositor through the socket's file permissions alone. Neither compositor checks where a client came from; `security-context-v1` scoping applies only to sandboxed clients that opt in. Reasoned from the protocol architecture, not verified against KWin or Mutter source.

Source: https://github.com/KDE/kwin/blob/master/plasma-kwin_wayland.service.in, https://bugs.kde.org/show_bug.cgi?id=442362, https://www.phoronix.com/news/GNOME-40-XWayland-On-Demand, https://wayland.app/protocols/security-context-v1

## 5. The app under Wayland

- The build has Wayland support: `slint` is `~1.17` with default features, and the resolved `winit` 0.30.13 pulls in `wayland-client`, `smithay-client-toolkit` and `sctk-adwaita` as well as `x11rb` (Cargo.lock). Nothing in the app sets `SLINT_BACKEND` on Linux, and the Linux e2e job does not either (`only_the_windows_job_picks_slints_software_renderer` pins that).
- winit chooses Wayland when `WAYLAND_DISPLAY` is in its environment and X11 otherwise. The guest contract's `session.env` does not carry `WAYLAND_DISPLAY` today, so an app started by the job inside a Wayland session would be an Xwayland client, and a suite run there would say nothing about Wayland.
- `set_outer_position` returns `NotSupportedError` on Wayland. No e2e case reads a window's position back; saved position restore becomes the compositor's decision.
- The app sets no app id. Slint has `slint::set_xdg_app_id`, available since 1.9 and Unix only, which must be called before the first window is shown. That it maps onto winit's `WindowAttributesExtWayland::with_name` is inferred from its description. Source: https://github.com/slint-ui/slint/pull/6851
- `display::outputs`, `display::watch` and the non-Windows `is_position_on_screen` all go through `DISPLAY`: `xrandr --query` and an x11rb RandR subscription. Under Wayland they answer through Xwayland. `XDG_SESSION_TYPE` is captured in `session.env` and read by nothing in the app or the xtask.

## 6. The harness under Wayland

Code reconnaissance, current working tree.

| Place | X11 dependency | Under Wayland |
|---|---|---|
| `guest-contract.sh`, `sunlit-e2e-session-ready` | `xhost`, `DISPLAY`, `XAUTHORITY` | harmless, both compositors already grant it; `WAYLAND_DISPLAY` missing |
| `guest-contract.sh`, `sunlit-e2e-run-job` | `DISPLAY` defaults to `:0` | fine; needs `WAYLAND_DISPLAY` from `session.env` |
| `guest/handover.rs` launcher | sources `session.env` | inherits whatever `session.env` carries |
| `commands/vm.rs:836-853`, `place_screens_command` | `xrandr --output ... --right-of` | cannot move a Wayland output |
| `commands/vm.rs:878-889`, `map_pointer_command` | `xinput --map-to-output` | X only |
| `e2e.rs:1041`, `test_a_layout_change_republishes_the_wallpaper` | `xrandr` to change a mode | the gate finds outputs through Xwayland and passes, the change does nothing, so the case would fail rather than skip |
| `e2e.rs:842`, `test_displays_reports_the_session_layout` | compares against `display::monitors()` | runs through Xwayland; at the guest's scale of 1 no skew is expected |
| `e2e.rs:905`, `test_across_screens_writes_what_this_desktop_can_hold` | same | same |
| `e2e.rs:717`, `test_set_wallpaper`, and `e2e.rs:1158`, `test_plasmashell_survives_rapid_republishing` | none: `gsettings`, `plasma-apply-wallpaperimage`, `dbus-send` | expected to run unchanged |
| tray, lifecycle, memory, render cases | none directly | run as a Wayland client once `WAYLAND_DISPLAY` is there |

The suite's skip idiom is `common::process::skip_case(case, why)` followed by `return` (`tests/common/process.rs:174`).

On the host side, `Desktop` (`provider/desktop.rs:24`) has four variants mapped to X11 session names, `desktop_for` (`commands/vm.rs:449`) refuses `--desktop` off the Linux image, `RunState.desktop` (`store/state.rs:168`) records the flag, `vm status` prints `Desktop::label`, and `the_guest_accepts_exactly_the_sessions_the_host_can_ask_for` (`provider/desktop.rs:142`) parses the selector's `case` arm and its `/usr/share/xsessions/${session}.desktop` checks out of `desktop.sh`.

## 7. Seeing the result

QEMU's `screendump` reads the emulated display device's scanout, so it shows what KWin or Mutter posted exactly as it shows Xorg's. Inferred from how the device works; no QEMU document says it in those words.
