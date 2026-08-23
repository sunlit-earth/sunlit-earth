# Plan: App Icon Integration

## Summary

The Sunlit Earth mark exists: a flat night-side globe with a bold outline, tilted aurora arcs, and the sun rising occluded behind the limb, designed over six review iterations on 2026-08-23 and saved as four SVGs under `assets/icon/` (the master plus 32, 24, and 16 pixel variants that shed detail instead of scaling it). This plan takes it everywhere the app shows an icon: the exe and therefore the desktop shortcut and taskbar on Windows, the tray on both platforms, the Linux desktop entry and hicolor icon set (first-class Linux support lands with the phase 5 parity PR, so the icon ships for it too), and the About window that the celestial phase A plan introduces. Linux system packaging (deb, Flatpak, AppImage) and macOS stay deferred to the existing cross-platform release roadmap item; this plan ships the Linux assets and their install locations, not an installer.

## Stakes Classification

Low. Everything is additive or a like-for-like swap of a decorative asset; the riskiest touch is the app's build script, where a mistake fails the build loudly rather than corrupting anything. No persisted state, no rendering behavior, no test semantics change.

## Research

Recorded from code reconnaissance on 2026-08-23, references to the tree at that date.

The tray icon is generated procedurally: `tray::create_icon` (`tray.rs:29-58`) fills a 32x32 RGBA buffer and wraps it in `slint::Image::from_rgba8` via `SharedPixelBuffer::clone_from_slice`, so the natural swap is baked RGBA bytes through the same two calls, with no image-decoding dependency added to the app crate. `crates/sunlit-app/build.rs` already exists (slint compilation plus a Windows `/STACK` link argument), so icon embedding extends it rather than creating it. The exe currently embeds no icon resource; the first icon resource in a Windows exe is what Explorer, the desktop shortcut, and the taskbar display, so one embedded ICO covers all three. `.gitattributes` routes only `textures/**` through LFS, so `assets/icon/` stays plain git (the SVGs are 1 to 3 KB). Nothing in the tree rasterizes SVG today; the xtask is the established home for developer tooling (`xtask/src/main.rs:43` is the clap command enum). The About window does not exist yet; it is phase A of the celestial work ([2026-08-23-celestial-a-stars-plan.md](2026-08-23-celestial-a-stars-plan.md)), and its logo should come from this pipeline. On Linux the tray is the same code path: Slint registers the icon over D-Bus through ksni, so the `create_icon` swap covers it with no platform work, and it shows wherever the session has a StatusNotifierWatcher (Plasma and xfce4-panel in the test guest; GNOME and Cinnamon have no tray there, which the e2e suite already gates on). Launchers, docks, and app switchers on Linux take their icon from a freedesktop `.desktop` entry naming an icon in the hicolor theme (`icons/hicolor/<size>x<size>/apps/<name>.png` plus `scalable/apps/<name>.svg`); under Wayland the association runs through the compositor matching the window's app id to the desktop-file name, and under X11 the window can additionally carry its own icon. The app has no Linux packaging yet, so install locations are the user-local `~/.local/share/applications` and `~/.local/share/icons/hicolor` until the release packaging item picks this up. The four SVGs encode the per-size detail budget, whose rule is: what cannot survive the raster drops (the dawn band and warm limb arc at 16, the aurora's soft glow layer below 48), and everything that stays grows in canvas units as the raster shrinks (the outline, the aurora arcs in both width and span, the glare's reach, the sun core), so no element fades into sub-pixel noise. The raster mapping is: 16 from `sunlit-earth-16.svg`, 20 and 24 from `-24`, 32 and 40 from `-32`, 48 and up from the master.

## Key Design Decisions

1. **Baking is an xtask subcommand, and the baked outputs are checked in.** `cargo xtask bake-icon` rasterizes the four SVGs to PNGs (16, 20, 24, 32, 40, 48, 64, 128, 256) with the `resvg` crate, assembles `assets/icon/baked/sunlit-earth.ico` with the `ico` crate, and additionally emits `tray-32.rgba` (raw RGBA8 bytes) and `about-256.png`. Checked in rather than baked at build time so `cargo build` gains no new dependencies and no SVG rasterizer runs on user machines; the bake reruns only when the SVGs change. Both crates are pure Rust and xtask-only.

2. **The exe icon goes through `embed-resource` in the existing build script.** A minimal `.rc` naming the ICO, compiled into the exe under `#[cfg(windows)]` gating in `crates/sunlit-app/build.rs`, beside the existing link argument. This makes the desktop shortcut, Explorer, and the taskbar correct with no installer work, which is all the "desktop icon" requires on Windows.

3. **The tray swaps to baked bytes with the same signature.** `create_icon` becomes `include_bytes!` of `tray-32.rgba` plus the existing `SharedPixelBuffer` wrap; the procedural gradient drawing retires. The function keeps its name and return type so `tray.rs`'s callers and tests do not move. The raw-RGBA format is deliberate: no PNG decode, no new app dependency, and the bake tool owns the pixels.

4. **The window icon is a verification task, not a promise.** Whether Slint exposes a per-window icon for the settings window needs checking against the pinned Slint version; if it does, wire the same image. On Windows dropping the sub-task is harmless (taskbar grouping follows the exe icon), but on Linux it matters more: an X11 window without an icon shows a placeholder in task switchers, and under Wayland the icon comes from the desktop-file association, which needs the window's app id to match `sunlit-earth.desktop`. The X11 half gets verified in the Linux test guest; the Wayland half cannot be (the guest stays on X11 by design) and joins the existing Wayland roadmap item as one more thing that session will check.

5. **The About window logo comes from this pipeline.** Phase A's About window uses `about-256.png`. If this plan lands first, the asset waits; if phase A lands first, its placeholder is swapped. No ordering constraint beyond that.

6. **Linux ships its assets now; packaging and macOS stay deferred.** The bake emits the hicolor PNG set (16, 24, 32, 48, 64, 128, 256, from the same size-to-variant mapping) alongside the ICO, the scalable slot is the master SVG itself, and the repo gains `assets/linux/sunlit-earth.desktop` with `Icon=sunlit-earth`. Since there is no installer, a short documented install step (or a small script beside the desktop file) copies the entry and icons into the user-local paths; the deb/Flatpak/AppImage question and the macOS `.icns` remain with the cross-platform release roadmap item, which then consumes these same assets. The idea of tinting the tray rim green during live aurora activity is recorded as a possible future flourish and deliberately not part of this plan.

## Success Criteria

1. `cargo xtask bake-icon` regenerates every baked output from the SVGs deterministically, and the outputs are committed.
2. A release build's exe shows the mark in Explorer, on a desktop shortcut, and in the taskbar on Windows 11.
3. The tray shows the mark on Windows and in the Linux test guest under KDE and XFCE (the two desktops whose sessions have a tray); tray-dependent e2e cases stay green.
4. The `.desktop` entry and the hicolor PNG set are in the repo, and after the documented user-local install the Linux guest's launcher and task switcher show the mark.
5. The 16 and 24 pixel rasters are judged on a real taskbar (light and dark) before the bake is finalized; pixel-grid nudges to the two small SVGs happen here if needed.
6. `cargo test`, `cargo clippy --all-targets`, `cargo fmt --check` green on Windows and in WSL; CLAUDE.md (assets directory, bake command) and `docs/roadmap.md` updated.

## Implementation Steps

### Step 1: The bake tool

`bake-icon` in the xtask with the size-to-variant mapping, the ICO assembly, the hicolor PNG set, the raw tray bytes, and the About PNG; commit the baked outputs.

### Step 2: Exe embedding

`embed-resource`, the `.rc`, the build-script extension, verified on a clean release build.

### Step 3: The tray swap

`create_icon` on baked bytes, procedural drawing removed, e2e tray cases run.

### Step 4: Small-size tuning

The real-taskbar check of criterion 4, with any stroke nudges applied to `sunlit-earth-16.svg` and `-24.svg` and the bake rerun.

### Step 5: Linux desktop entry and guest verification

`assets/linux/sunlit-earth.desktop`, the documented user-local install step (or the small script beside it), and a pass through the Linux test guest: the tray mark under KDE and XFCE, and the launcher and task-switcher icons after the install.

### Step 6: Window icon verification and documentation

Decision 4's check on both platforms, the About hook if phase A has landed, CLAUDE.md and roadmap.

## Risks and Mitigations

- `resvg` fidelity: the mark uses a focal-point radial gradient and an alpha mask, both supported, but the bake must be eyeballed against the browser rendering once before anything is committed.
- The ICO consumer path (Explorer caching): icon caches on Windows are sticky; verifying on a fresh shortcut or another machine avoids chasing a cache ghost.
- `embed-resource` needs `rc.exe` or `windres` at build time; it vendors a fallback, but CI's Windows job should build once before this merges to prove it.
- The 16 pixel mark is four elements by design (disk, outline, aurora arcs, sun with glare); if the taskbar check in Step 4 still finds it muddy, the tuning budget is stroke nudges and, at worst, dropping the aurora arcs from the 16 variant, not new geometry.
- Tray panels on Linux receive the icon as a pixmap over D-Bus and scale it themselves, and Plasma and xfce4-panel may scale differently; the guest pass in Step 5 is what catches an ugly resample, and the fallback is baking a second, larger tray pixmap if a panel wants one.
- The Wayland app id association is unverifiable in the X11 guest; it is recorded on the Wayland roadmap item rather than assumed to work.

## Rollback Strategy

Feature branch, plain revert. The SVGs and baked assets are inert data; removing the build-script lines restores an icon-less exe; the old procedural tray icon is one commit away in history.
