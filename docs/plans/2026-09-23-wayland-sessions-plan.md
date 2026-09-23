# Plan: KDE and GNOME Wayland Sessions in the Linux Test Guest

## Summary

The Linux guest gains two more sessions, Plasma on Wayland and GNOME on Wayland, chosen per boot the same way the four X11 sessions are. Nothing new is installed for them, since the packages the image already has ship both sessions. What changes is the selector's allowlist, a second host flag that picks the session type, the guest contract carrying `WAYLAND_DISPLAY` so the app under test is a native Wayland client rather than an Xwayland one, a check that the session which came up is the one that was asked for, and the few e2e cases and boot steps that are X11 commands learning to skip under Wayland. The payoff is the roadmap's untested Wayland wallpaper item, observed on the two desktops that matter most, and the rest of the suite running as a Wayland client.

The research is in [2026-09-23-wayland-sessions-research.md](2026-09-23-wayland-sessions-research.md).

## Stakes Classification

Low to medium. The app changes by one call (the app id) and one signal line; everything else is the xtask, the guest scripts and the test harness. The X11 sessions stay the default and must come out of the rebuild behaving exactly as before. The cost is one image rebuild, about an hour, and it is reproducible from the templates.

## Key Design Decisions

1. **One image, a session type chosen per boot.** No second image and no layer. The Wayland session files are already in the image (research section 1), so the only thing that differs between an X11 boot and a Wayland boot is the name sddm is told, and the fw_cfg channel already carries exactly that. The guest keeps receiving a single session basename; it never learns that there are two axes.

2. **The session type is its own flag, not two more desktops.** `--session-type <x11|wayland>`, default `x11`, on `vm up`, `e2e` and `vm smoke`, beside `--desktop`. The desktop decides which wallpaper backend runs and the session type decides the display server, and those are independent questions; folding them into `kde-wayland` and `gnome-wayland` variants would make every place that asks "which desktop" also have to strip a suffix. `Desktop::session` becomes a function of both, returning `None` for a pair the image has no session for, and XFCE or Cinnamon with `wayland` is refused before anything boots, naming the two desktops that have one. The name follows `XDG_SESSION_TYPE`, which is what the guest reports back (decision 5).

3. **The Wayland session names are `plasma` and `gnome-wayland`.** Not `gnome`: that basename exists in both session directories, and sddm's autologin looks in the X11 one first (sddm issue #837), so `Session=gnome` would start GNOME on Xorg while every record said Wayland. `desktop.sh` asserts at build time that each Wayland name exists under `/usr/share/wayland-sessions` and does not exist under `/usr/share/xsessions`, so a later package that adds a colliding file fails the build rather than a boot. `DisplayServer=x11` stays, because it governs the greeter, which autologin never shows.

4. **The guest contract carries `WAYLAND_DISPLAY`, and only when it is set.** `sunlit-e2e-session-ready` writes the line only when the variable is non-empty, so an X11 session's `session.env` is byte for byte what it is today and winit sees no blank variable there. With it present, the job, the hand-over launcher and `vm ssh` commands that source the file all start the app as a Wayland client, because winit prefers Wayland when the variable is in its environment. `DISPLAY` and `XAUTHORITY` keep being written, since the display query, the watcher and several setters still reach Xwayland through them, and `xhost` stays as it is: both compositors already grant the local user, so it does nothing there and costs nothing.

5. **The boot proves the session it got.** After the ready marker appears, the xtask reads `XDG_SESSION_TYPE` and `XDG_CURRENT_DESKTOP` out of `session.env` and refuses to continue when they are not the ones asked for, naming both. Today nothing reads `XDG_SESSION_TYPE`; with sddm's name resolution and a selector that falls back to Plasma X11 on anything it does not recognize, a silent fallback is the likeliest failure this feature has, and it would produce a green run about the wrong session. The check applies to X11 boots too, where it is equally true and free.

6. **The app says which window system it is on, and the suite checks it.** Once the first window exists, the app prints `SIGNAL:windowing <wayland|x11|other>`, taken from the Slint window's raw window handle, and logs the same at info level. A Linux e2e case asserts that a session whose `XDG_SESSION_TYPE` is `wayland` produced `wayland`. This is what turns "the suite ran in a Wayland session" into "the app under test was a Wayland client", which decision 4 intends and nothing else would show. It needs Slint's raw window handle support; if Step 1 finds that costs a feature this build does not want, the fallback is the harness asking Xwayland whether the process owns a window (`xdotool search --pid`, already in the image) while the window is shown, which is weaker because it proves an absence.

7. **The app sets its app id.** `slint::set_xdg_app_id("sunlit-earth")` before the first window, on Linux. Under Wayland a task manager matches a window to `sunlit-earth.desktop` by app id, and without one the icon association that works on X11 through `StartupWMClass=sunlit-earth` has nothing to match (roadmap, the Wayland wallpaper item). The value is the basename the desktop entry already uses. Step 1 confirms it leaves X11's `WM_CLASS` at the value `StartupWMClass` names, since changing the X11 behavior is not the point.

8. **X11 commands skip under Wayland, with the reason.** `test_a_layout_change_republishes_the_wallpaper` gates on the session type before anything else and skips with "xrandr cannot move a Wayland session's outputs". Without that it would find outputs through Xwayland, pass its own gate, apply a change that does nothing and fail waiting for a republish. `--screens` above one with `--session-type wayland` is refused before boot, because both `place_screens_command` and `map_pointer_command` are `xrandr` and `xinput`. The three cases that read the layout through `display::monitors()` keep running: at the guest's scale of 1 Xwayland should report the true rectangles, and a run that shows it is evidence for the roadmap's Xwayland skew item. Everything else runs unchanged.

9. **Layout changes under Wayland are follow-up work, not this plan.** Porting the layout change case and `--screens` means `kscreen-doctor` on Plasma and `ApplyMonitorsConfig` over D-Bus on GNOME, and at that point the native per-desktop display query the roadmap already describes is the better foundation. That is its own plan. `libkscreen-bin` is not added to the image for it now.

## Success Criteria

1. `vm up linux --desktop kde --session-type wayland` and `--desktop gnome --session-type wayland` each boot into that session from the rebuilt image. `loginctl show-session` reports `Type=wayland`, `vm status` names the session as Wayland, and the screen shows the desktop.
2. `--desktop xfce --session-type wayland`, `--desktop cinnamon --session-type wayland`, and `--session-type wayland --screens 2` are refused before any guest boots, each naming what would work instead. `--session-type` against the Windows image is refused, as `--desktop` is.
3. A boot whose session did not come up as asked fails with both the requested and the observed session named. Shown once on purpose by booting with a selector allowlist that lacks the name.
4. `cargo xtask e2e --target linux --desktop kde --session-type wayland` and the GNOME equivalent pass, with the layout change case reporting its skip, `SIGNAL:windowing wayland` asserted, and `test_set_wallpaper` passing with the wallpaper visibly changed in a screendump.
5. The four X11 sessions pass the suite on the rebuilt image as they did before, and their `session.env` has no `WAYLAND_DISPLAY` line.
6. The Plasma Wayland task manager shows the app's own icon for its window.
7. `cargo test`, `cargo clippy --all-targets` and `cargo fmt --check` green on Windows and in WSL.
8. `docs/vm-setup.md`, `docs/vm-internals.md`, `docs/platforms.md`, `docs/testing.md` where it lists the Linux sessions, `docs/roadmap.md`, and the VM section of `CLAUDE.md` say what the sessions are and what skips under Wayland.

## Implementation Steps

### Step 1: A hand spike in the current image

Before any code, prove the risky half on the image that exists, using the fact that a desktop guest's changes last until `vm down`. `vm up linux --desktop gnome`, then over `vm ssh`: write `Session=gnome-wayland` into `/etc/sddm.conf.d/10-sunlit-autologin.conf`, restart sddm, and confirm `loginctl` reports a Wayland session. Read the `session.env` the autostart marker rewrote and record whether `DISPLAY`, `XAUTHORITY` and `WAYLAND_DISPLAY` were set when it ran (research section 4 leaves this open for GNOME). Start the app by hand with `WAYLAND_DISPLAY` exported and confirm it draws, then run the wallpaper setter once. Repeat for `plasma` from a KDE boot. Also confirm: whether Slint 1.17's raw window handle is reachable without a feature this build lacks (decision 6), and what `WM_CLASS` the window has on X11 with and without `set_xdg_app_id` (decision 7). If GNOME Wayland does not start under sddm at all, stop and report, because the answer to that changes the plan.

### Step 2: The host side

The `SessionType` enum and flag, `Desktop::session(self, SessionType) -> Option<&str>` with the refusals from decision 2 and decision 8, the session type in `RunState` next to the desktop, `vm status` labels (`KDE Plasma (Wayland)`), and the fw_cfg argument built from the pair. Unit tests: every accepted pair has a distinct session name, the refused pairs are refused with their message, and the X11 names are unchanged. `the_guest_accepts_exactly_the_sessions_the_host_can_ask_for` compares the selector's allowlist with every accepted pair's name and checks each name against the directory its type belongs in.

### Step 3: The guest side

In `desktop.sh`: the allowlist gains `plasma` and `gnome-wayland`, the selector's existence check looks in `wayland-sessions` or `xsessions` as the name requires, and the build asserts decision 3's presence and absence rules. In `guest-contract.sh`: the conditional `WAYLAND_DISPLAY` line. In the xtask, after the ready marker: decision 5's comparison, with a unit test over fabricated `session.env` contents.

### Step 4: The app and the suite

`set_xdg_app_id` and the windowing signal with its parse helper in `ipc.rs` and a unit test; the Linux e2e assertion for the signal; the session type gate on the layout change case. `skip_case` is the idiom for the latter.

### Step 5: Rebuild and run

Rebuild the Linux image. Run the suite in all six sessions, screendump each desktop after `test_set_wallpaper`, and check criteria 1 through 6 by hand where they say so.

### Step 6: Documentation

Criterion 8. The roadmap's Wayland wallpaper item closes for KDE and GNOME with what was observed; the Xwayland display query item gains what the layout cases showed; a new item carries decision 9.

## Out of Scope

Layout changes and multiple screens under Wayland (decision 9); a native Wayland display query and change watcher; XFCE's and Cinnamon's experimental Wayland sessions; saved window position restore under Wayland, which is the compositor's decision there.

Three roadmap items are each blocked only on an image rebuild: the polkit agent for XFCE and Cinnamon, indexing off in the non KDE sessions, and GNOME's Activities overview at login. Step 5 rebuilds the image anyway. Folding them in is cheap but widens what a failed rebuild has to be bisected across, so it is a decision for kickoff rather than part of this plan.

## Risks and Mitigations

- GNOME Wayland under sddm may not start, or may start with `DISPLAY` missing from autostart's environment (research section 2 and 4). Step 1 finds out on the existing image before anything is written. A missing `DISPLAY` at autostart would mean the marker waits for it or reads it from `systemctl --user show-environment`, which is a change to the contract script only.
- sddm #837 in a form not covered by decision 3, or #1407's getty race. Decision 5 turns either into a named failure rather than a wrong green run; masking the extra gettys is the known workaround.
- Slint's FemtoVG renderer over EGL on Wayland with llvmpipe may behave differently from the X11 path. If it fails, the Wayland job sets `SLINT_BACKEND=winit-software` as the Windows job does, recorded as a departure, and `only_the_windows_job_picks_slints_software_renderer` is renamed to say what it then pins.
- The windowing signal is a new `SIGNAL:` line. It is additive and parsed by nothing except its own test and case, and `SIGNAL:memory` is untouched.
- Plasma or GNOME on Wayland may be slower on llvmpipe than on X11 with compositing off, and the suite's timeouts were measured on X11. Step 5's runs are the measurement; a timeout that needs raising is a departure with the numbers.

## Rollback Strategy

The flag is additive and defaults to X11, so a revert of the host side leaves every existing command as it was. The guest changes are additive too: an X11 boot on the rebuilt image differs from today only in the session check, and the previous image rebuilds from the previous templates.
