# Phase 5.1: Linux guest hand-over parity and stale wording

Written 2026-08-23. Follows the phase 5 plan (`2026-08-21-phase5-linux-vm-and-parity-plan.md`), on the same branch `feat/phase5-linux-parity`.

## Context

Phase 5 gave the Linux guest four desktops and the app a wallpaper backend per desktop, but left the hand-over half behind: `guest::handover::prepare` returns early for anything that is not the Windows guest, so a person in a Linux guest gets no launcher and no desktop shortcuts, and the closing text (`guest_environment_note`) is empty for Linux, so nothing ever names `/var/lib/sunlit-e2e`. The first person to sit in a KDE guest could not tell where the app was.

Two texts also went stale. The `vm up` lifecycle explainer opens with "There is no stop or pause", which argues with the reader's correct mental model: `vm down` is the stop, and stopping an idle guest is the right move because it holds 4 GiB (Linux) or 6 GiB (Windows) the whole time it is up. What the text means is that there is no save or suspend and nothing worth saving. And `host::facts::WSL_DISTRO`'s comment claims the builder distribution "matches the guest's base image so glibc agrees", which stopped being true when the guest moved to Debian 13; the build still works because Ubuntu 22.04's glibc 2.35 is older than trixie's 2.41, and old-builds-run-on-new is the direction that works, but the stated rationale is false and the printed message ("building the e2e suite for the linux guest in Ubuntu-22.04") reads like a leftover.

## Goals

1. A Linux guest handed to a person (`vm up`, `e2e --keep`) carries a launcher and desktop entries, and the closing text names `/var/lib/sunlit-e2e` and how to start the app.
2. The lifecycle explainer tells the truth: `vm down` is the stop, it frees the RAM and the overlay, there is no save or pause, and nothing in the guest is worth saving.
3. The `WSL_DISTRO` comment and the build message state the real rationale: a deliberately older glibc than the guest's.

## Non-goals

- No image rebuild. Everything is staged per boot at runtime, exactly like the Windows hand-over, and for the same reason: the launcher has to know what this boot staged.
- No Windows guest changes; its hand-over stays as it is.
- No enhanced-session equivalent: a VNC console asks for nothing, so there is no credential work.
- No Wayland or portal work.

## Decisions

1. The launcher is a generated shell script, `/var/lib/sunlit-e2e/run-app.sh`, written per boot: it sets `SUNLIT_EARTH_TEXTURES` only when this boot staged textures and execs `bin/sunlit-earth`. No `SLINT_BACKEND`: Mesa answers GL in the guest, which is why only the Windows launcher sets it. The script must be executable, and its syntax should be covered by the same kind of parse check the shipped guest scripts get.
2. Desktop entries per the XDG spec, mirroring the Windows pair: one application entry for the app (through the launcher) and one entry for the staged folder. Written to both the desktop directory (KDE, XFCE and Cinnamon show desktop icons) and `~/.local/share/applications` (the menu, which is all GNOME has: it shows no desktop icons at all). The desktop directory is asked for with `xdg-user-dir DESKTOP` rather than assumed, and created if absent. The folder entry may be `Type=Link` or an `xdg-open` application entry; the implementer decides and records which and why. An `Icon=` line is optional; the app ships no icon file, so a stock icon or none is fine.
3. Trust guards: Plasma, xfdesktop and Nemo quarantine desktop launchers they have not blessed. Mark the entries executable and use whatever per-desktop blessing works at runtime (gio metadata where a desktop honors it). A dialog on first activation is an acceptable fallback where it cannot be avoided without an image rebuild, but it must be recorded per desktop in this plan rather than discovered by the next user.
4. The closing text (`guest_environment_note`, Linux arm): a staged guest names the root, the launcher, and the terminal command; a bare guest (`vm smoke --keep`) keeps promising nothing and names `vm up`, mirroring the Windows arm. The Windows arm's wording is untouched.
5. The lifecycle explainer reword keeps its true content (destroying loses nothing, the golden image survives, the next `vm up` is pristine, watching is harmless and clicking perturbs, the clipboard note) but stops denying that a stop exists. The pinning tests in `commands/vm.rs` move with the text; they pin behavior-relevant phrases of the new wording, not the old.
6. `WSL_DISTRO`: the comment states the real rationale (a deliberately older glibc than the guest, because old-builds-run-on-new is the direction that works, and what actually pins it is the guest moving beneath it). The build message must not read as if the guest were Ubuntu. Stretch, only if it stays small: an env override `SUNLIT_EARTH_WSL_DISTRO` following the xtask's blank-is-unset rule, added to CLAUDE.md's second env table; if it is skipped, record that in a departure.

## Steps

1. The three text fixes: closing text for Linux, lifecycle reword, `WSL_DISTRO` comment and message. Unit tests updated or added alongside. Commit.
2. Launcher generation for the Linux guest in `guest::handover`, wired into `prepare` beside the Windows arm. Unit tests on the generated script (textures line present exactly when staged, exec line, no `SLINT_BACKEND`). Commit.
3. Desktop entries: generation, writing into the guest, trust blessing. Unit tests on the entry content (absolute Exec, valid group header). Commit.
4. Live verification per desktop (see criteria). Fix what it finds; record what cannot be fixed at runtime.
5. One `cargo xtask e2e --target linux --desktop kde` run as a regression check, because `stage`/`prepare` changed and the suite goes through the same path.
6. Docs: the CLAUDE.md hand-over paragraph currently says staging writes the extra things "into a Windows one"; update it and vm-setup.md where they describe the Windows-only shortcuts.

## Acceptance criteria

1. `vm up linux` ends with text naming `/var/lib/sunlit-e2e`, the launcher, and the run command; `vm smoke --keep` still promises nothing staged.
2. In a KDE guest, activating the desktop entry launches the app with the globe textured, proven by screenshot; in a GNOME guest the same through the applications menu.
3. XFCE and Cinnamon: entries present and activatable, with first-activation behavior (trust dialog or none) recorded per desktop.
4. The reworded lifecycle text no longer denies a stop, and its tests pin the new wording.
5. The `WSL_DISTRO` comment and build message are truthful about Debian 13 and the builder.
6. The KDE e2e run is green.
7. Gates at the tip: `cargo test` on Windows, `cargo clippy --all-targets` with zero warnings, `cargo fmt --check`, and the WSL leg (`wsl -d Ubuntu-22.04 -- bash -lc 'cd /mnt/c/Workspace/rustrover/sunlit-earth && CARGO_TARGET_DIR=$HOME/sunlit-target cargo test --workspace'`), where the known `tests/shading.rs` BadAccess flake does not fail the gate when a rerun passes.

## Risks

- The trust guards differ per desktop and per version; what blesses an entry in Plasma may do nothing in xfdesktop. Research in the live guest, not from memory.
- GNOME has no desktop icons, so the menu entry is the whole parity there; the closing text must not promise icons on every desktop.
- The desktop directory may not exist in a fresh home; `xdg-user-dir` answers and `mkdir -p` cures.
- Staging connects as `tester`, so home-directory writes have the right owner for free; anything written elsewhere must not need root.
- Cinnamon's shell crashes into fallback at login (phase 5 departure 12); its desktop check may need the recorded `cinnamon --replace` trick.
- The ports fix (commit 3850ebf) means the QMP port in a guest booted while WinNAT holds 4390-4489 is 4490, not 4444; tooling that assumes 4444 must read `vm.json`.

## Departures

1. **The closing text shipped with the launcher rather than with the other two wording fixes.** Step 1 groups three texts and step 2 the launcher, but the Linux arm of `guest_environment_note` names the launcher's path, and it takes it from `handover::linux_launcher_path` rather than spelling it again. So the commits are: the `WSL_DISTRO` truth on its own, then the launcher, the entries' first cut and the closing text together with the lifecycle reword, then the two live fixes. The steps all happened; only the commit boundaries moved.

2. **The launcher writes a log when nobody started it from a terminal.** Decision 1 lists what the script does, and this is one line more than it lists: `exec >>/var/lib/sunlit-e2e/run-app.log 2>&1` when stdout is not a tty. The Windows launcher holds its console window open on a failure, which is that guest's answer to the same problem, and a desktop entry started from an icon has no terminal at all, so without this a launch that failed would leave a person with nothing whatever to read. Started from a shell the redirect does not happen, because there the terminal is the better place and a log would hide it. Both directions are pinned by a test.

3. **The folder entry is an `xdg-open` application entry, not a `Type=Link`.** Decision 2 leaves the choice open. A menu shows `Type=Application` entries and nothing else, and GNOME draws no desktop icons at all, so a link would be invisible on the one desktop where the menu is the whole hand-over. One `xdg-open` covers all four desktops, and it was seen to open a file manager in a KDE guest (Thunar, which is what `xdg-open` resolves to in this image) and in a Cinnamon one (Nemo).

4. **One trust blessing, not the two or three the risk section expected.** Decision 3's guess was that each desktop would want something different. Measured, one boot per desktop: Plasma runs an executable entry with no dialog, Nemo runs one with no dialog and with `metadata::trusted` deliberately unset, and only xfdesktop refuses, calling the desktop an insecure location whatever the mode bits say and putting up an "Untrusted application launcher" dialog with the launch behind it timing out. What it wants is `metadata::xfce-exe-checksum`, the file's own sha256, which is what its "Mark As Secure And Launch" button writes; writing that during the hand-over is the dialog answered in advance, verified by clearing the mark and watching the dialog come back. `metadata::trusted` was written for one commit and then removed, because a line whose desktop was never seen to want it is a claim the code cannot support.

5. **The stretch `SUNLIT_EARTH_WSL_DISTRO` override was skipped.** `WSL_DISTRO` is read by four things, and only one of them is the build: `vm doctor` checks that distribution's registration and WSL version, and `vm setup`'s install step pairs `wsl.exe --install -d Ubuntu-22.04` with `ubuntu2204.exe install --root`, an Appx launcher named after this distribution and no other. An override that moved the build would leave the doctor reporting on a distribution the build does not use and the setup step installing one nobody asked for, which is a worse state than having no override. Doing it properly means splitting the constant into "what the build uses" and "what setup can install", which is more than the "stays small" the decision allows.

## Validation record

### Round 1, 2026-08-23, live in the Linux guest, one boot per desktop

Every boot was `cargo xtask vm up linux --desktop <d>` on the QEMU provider, with the assets staged, and every activation was a real double click or menu click in the guest's own session rather than a command that skipped the desktop. The QMP screendumps are the evidence; the app's own log in `/var/lib/sunlit-e2e/run-app.log` says which launcher started it.

- **KDE Plasma.** Both entries drawn on the desktop. A double click on `Sunlit Earth` launched the app with the globe textured, no dialog of any kind. The folder entry opened Thunar on `/var/lib/sunlit-e2e`, which is what `xdg-open` resolves to in this image. `kioclient exec` on the entry does the same thing and says nothing, which is what said the entry itself was fine while the injected click was not (below).
- **GNOME.** No desktop icons, as expected: the two files sit in `~/Desktop` and nothing draws them. Super, then typing "sunlit", showed both entries in the overview's search, with the app under the stock `applications-graphics` icon; clicking it launched the app with the globe textured. No dialog.
- **XFCE.** Both entries drawn. The first double click produced "Untrusted application launcher": *the desktop file is in an insecure location and not marked as secure*, with `Launch Anyway`, `Mark As Secure And Launch` and `Cancel`, and behind it a "Failed to run" whose reason was a timeout, the launch having waited for the dialog. `Mark As Secure And Launch` started the app and wrote `metadata::xfce-exe-checksum`, which is exactly `sha256sum` of the entry. With the hand-over writing that mark itself, a fresh double click launched the app with no dialog.
- **Cinnamon.** Both entries drawn, even in the fallback mode the phase 5 defect leaves the session in; `cinnamon --replace` over `vm ssh` restored the shell as documented. A double click launched the app with the globe textured and no prompt, and the same held with `metadata::trusted` cleared, which is what retired that attribute (departure 4).

Two things worth writing down for whoever gathers this kind of evidence next:

- A QMP-injected double click does not activate a desktop icon. Single clicks land exactly where they are sent (`xdotool getmouselocation` read back 56,40 for the coordinates computed from the screendump) and select the icon, but two of them, whether batched in one `input-send-event` or sent over two connections a couple of hundred milliseconds apart, never became a double click. `xdotool click --repeat 2 --delay 180` inside the guest does, and it is the desktop's own activation path either way. What a real VNC viewer's clicks do was not tested.
- `gio set` writes nothing without a session bus: over SSH it answers "Setting attribute metadata::trusted not supported", and with `/var/lib/sunlit-e2e/session.env` sourced it writes the attribute. The metadata daemon is reached over the bus, and the guest already writes that environment out at logon for processes the orchestrator starts.

Suite: `cargo xtask e2e --target linux --desktop kde` after the last hand-over change, 10 passed and 0 failed in 46 s, guest exit code 0 (criterion 6).

Both closing texts were read off a live command rather than a test: `vm up linux --desktop kde` printed the root, the launcher and the shell command, and `vm smoke linux --keep` printed that nothing was staged and named `vm up`, on a guest whose desktop was in fact empty.

Gates at f876369: `cargo test` on Windows exit 0 (381 core unit, 31 engine, 6 golden, 19 render_pipeline, 12 shading, 1 soak, 42 and 20 in the app, 433 xtask); `cargo clippy --all-targets` no warnings; `cargo fmt --check` clean; the WSL leg exit 0 with all 13 suites ok and no `tests/shading.rs` flake.

### Adversarial review, 2026-08-23, range 77c6efc..bdb82a6: 0 MAJOR, 3 MINOR, all fixed

A validator with fresh context re-ran the four gates at bdb82a6 itself, checked all five departures against the code, re-verified every acceptance criterion live (one boot per desktop, its own clicks, plus a negative control: clearing `metadata::xfce-exe-checksum` brought xfdesktop's dialog back on the next double click, so the blessing is load-bearing), re-ran the KDE e2e suite (10 passed in 46 s), and mutation-tested the new tests: 11 mutations, 9 killed, 2 survived. The survivors became two of the three minors; none was a major, so the round did not block completion.

1. **The generated-script parse gate could not catch a mismatched heredoc delimiter.** `bash -n` reports an unterminated here-document as a warning while exiting 0, so a terminator typo would swallow the rest of the install script and every test would stay green (in production the missing `HANDOVER=ready` marker still fails the hand-over, but the gate meant to catch it first would not fire). Fixed in 3668ba9: the check also rejects a `warning:` on stderr, with a control pinning that bash still reports the case that way.
2. **Nothing failed if the launcher was never made executable.** Replacing its `chmod 0755` with `true` survived the whole suite, and both entries run the launcher as their `Exec`, so every activation would fail with a permission error. Fixed in 3668ba9: a test pins the chmod, placed after the write that creates the file.
3. **Two doc sentences claimed `xdg-open` reaches each desktop's own file manager**, while the KDE guest opens Thunar. Both now claim only what was seen: one `xdg-open` finds a file manager in all four sessions. Fixed in 3668ba9.

Both surviving mutations were re-run against 3668ba9 in a throwaway worktree and died. Gates at 3668ba9: `cargo test` on Windows exit 0 (434 xtask, one test more), `cargo clippy --all-targets` no warnings, `cargo fmt --check` clean, WSL leg complete with every suite ok.
