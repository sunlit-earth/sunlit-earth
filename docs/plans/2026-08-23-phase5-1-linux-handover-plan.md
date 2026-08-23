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

(numbered, appended as they happen)

## Validation record

(appended per round)
