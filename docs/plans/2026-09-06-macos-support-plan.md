# Plan: macOS support and a three-platform release pipeline (2026-09-06)

Written 2026-09-06 from [2026-09-06-macos-support-research.md](2026-09-06-macos-support-research.md), which holds the code survey, the platform facts and their sources. Intended for its own branch, `feat/macos`. It closes two roadmap items, "Wallpaper setting on macOS" and "Cross-platform release builds", and the stale `metal` golden set on the way.

## Summary

macOS builds, tests and renders headlessly today and refuses to set a wallpaper before rendering anything. Parity with Windows and Linux is five pieces of platform code (the monitor list, the display watcher, the wallpaper setter, a window-position check, a session-end listener), two defects that only show on macOS (the single-instance lock file, the textures lookup inside an `.app`), one asset (an `.icns`), a bundle format, and the e2e suite's platform tables. The tray is Slint's and already compiles for macOS. The release pipeline is rebuilt around a host-side `cargo xtask bundle` that the VM `dist` command and the GitHub jobs share, producing one archive per platform with textures and notices inside, on every `v*` tag.

The constraint that shapes the plan is that nobody here has a Mac. Everything is written blind, compiled and unit-tested on `macos-latest`, run once through the desktop e2e suite on that runner to learn what its session allows, and then handed to testers as an ad-hoc signed `.app` with a checklist. Every macOS claim in the docs carries which of those three it rests on.

## What is missing, in one table

| Area | State | Work |
|---|---|---|
| Wallpaper setter | refuses | `wallpaper/macos.rs` over `NSWorkspace`, main-thread hop, per-screen |
| Monitor list | `None` | CoreGraphics enumeration, pixel rectangles, ColorSync UUID ids |
| Display change watcher | `None` | `CGDisplayRegisterReconfigurationCallback` |
| Window position check | coarse range check only | a macOS `outputs()` in points |
| Session end | `None`; AppKit's default never blocks a logout | widen the SIGTERM listener to `cfg(unix)`; the ordered teardown stays a roadmap item |
| Tray | compiles, unrun | verify; flip `tray_supported()` |
| Single instance | lock file in the working directory, absent from Finder | an absolute lock path |
| Textures in an `.app` | not found | one more lookup candidate |
| Logging | stderr only, which an `.app` has none of | a log file when there is no terminal |
| App icon | no `.icns` | bake it with the `icns` crate |
| Bundle | no macOS layout | `Sunlit Earth.app`, ad-hoc signed, `ditto` zip, plus a bare-binary tarball for Terminal users and testers |
| xtask | `Target` is two guests | `bundle::Platform` with three, `cargo xtask bundle` |
| Release workflow | Windows only, bare exe, tag glob that has never matched | three platforms, one tag, verified bundles |
| e2e tables | macOS listed as no tray, no setter | extend both in the setter's commit |
| `metal` goldens | 14 missing, 3 stale | one `golden.yml` dispatch |
| Docs | "cannot set a wallpaper" | every doc in step 9 |

## Goals

1. `cargo run` on a Mac shows the settings window and the status item, sets the wallpaper on every screen in all three display modes, follows display changes, and exits cleanly, with the same config, IPC and CLI as the other two platforms.
2. Pushing a `v*` tag produces a GitHub release with `sunlit-earth-<version>-windows.zip`, `sunlit-earth-<version>-linux.tar.gz`, `sunlit-earth-<version>-macos.zip` and `sunlit-earth-<version>-macos.tar.gz`, each verified to run and find its textures on the runner that built it, with a `build-info.json` beside each.
3. A tester with no developer tools can download the macOS zip, get past Gatekeeper with the steps in the README, and report back with a log file.
4. Nothing changes for Windows or Linux users, and `cargo xtask dist` keeps working and shares the bundle code.

## Non-goals

- Notarization and a Developer ID. 99 USD a year and an enrollment; the plan leaves the hook (a signing step that takes a certificate from a secret) but ships ad-hoc.
- Per-Space wallpapers. There is no API.
- A macOS VM or `cargo xtask e2e --target macos`. The hosted runner is the only macOS this project can run on.
- Hiding the Dock icon (`LSUIElement`), the `exiting` teardown hook, and a LaunchAgent for autostart. All three need Slint's `unstable-winit-030` feature or new product surface, and go on the roadmap.
- Skia. The UI stays on FemtoVG (OpenGL on macOS) unless a tester reports it broken.
- Universal binaries by default. See decision 9.

## Decisions

1. **Three tiers of evidence, named in the docs.** Compiled and unit-tested on the runner; run in the runner's session (the e2e probe, step 7); confirmed by a tester on a real Mac. `platforms.md`'s macOS column says which tier each row is at, and a row moves up only when the evidence exists. This is the retrospective's rule about not pretending: a setter that has never painted a real desktop is not "yes" in that table.

2. **Wallpaper through `NSWorkspace`, from the main thread, one screen at a time.** `wallpaper/macos.rs` mirrors `linux.rs`. `check_supported()` asks `CGGetActiveDisplayList` for at least one display; a login over SSH has none, and that is the refusal, in the voice of the others ("this session has no display to paint"). `set_wallpaper_job` writes the images through the shared `Publication::write_job` (per monitor, with `AcrossScreens` already cut into one piece per screen by `image_for`, so macOS has `PerMonitor` reach like Windows), then hands the AppKit half to the main thread: `dispatch2::DispatchQueue::main().exec_async` with a channel the engine thread waits on for at most ten seconds, so a main thread that never answers is a refusal the status line shows ("the main thread did not take the wallpaper within 10 s") and not a hang. `exec_sync` is not used because the app's shutdown joins the engine from the main thread, and a synchronous hop there is a deadlock waiting for its moment. On the main thread: `NSScreen::screens(mtm)`, each screen's `CGDirectDisplayID` from `deviceDescription`'s `NSScreenNumber`, matched to `Monitor::id` (decision 3), then `setDesktopImageURL_forScreen_options_error` with `NSImageScaling::ScaleProportionallyUpOrDown` and allow-clipping true, which is Fill. Screens the job does not paint are left alone; a monitor with no matching `NSScreen` is a note in the Windows setter's voice ("macOS named no screen for {label}, so it kept the wallpaper it had"). `desktopImageURLForScreen` is read after each set and logged at debug level, not asserted, because nobody who uses it trusts it. The `unsafe` call sites (the set, the CG list) each carry a scoped `#[allow(unsafe_code)]` and a `// SAFETY:` argument, as `memory/macos.rs` does. Not `osascript`, which needs a TCC grant a bare binary cannot hold, and not the `wallpaper` crate, which wraps `osascript` and stopped in 2021.

3. **The monitor list from CoreGraphics, off the main thread.** `display::monitors()` on macOS walks `CGGetActiveDisplayList` and for each display takes `CGDisplayBounds` (points, top-left origin, the global display space, so no flip), the current mode's pixel size, `CGDisplayIsMain` for `primary`, and `CGDisplayIsBuiltin` for the label ("Display 1 (built-in)", "Display 2", numbered the way the Windows labels are). The `Monitor` rectangle is the bounds scaled by that display's own factor (pixel width over point width), which is exact for one screen and for several at the same scale, and puts mixed-scale layouts in the state Windows has open on the roadmap: per-monitor mode exact, the span canvas's geometry unverified. `id` is the ColorSync UUID from `CGDisplayCreateUUIDFromDisplayID` (`objc2-color-sync`, feature `ColorSyncDevice`) as a string, which survives a reboot for most displays, with `display-<CGDirectDisplayID>` as the fallback when the UUID call fails; identical twin monitors can swap and the docs say so. All of it is CoreGraphics because the engine thread and the `displays` subcommand call `monitors()` and neither has a `MainThreadMarker`. `display::outputs()` gets a macOS arm returning the same displays in points, which is the space winit reports window positions in, so `is_position_on_screen`'s existing `not(windows)` arm works unchanged.

4. **The display watcher is `CGDisplayRegisterReconfigurationCallback` on the main thread.** `display::watch::start` on macOS registers an `extern "C-unwind"` callback that ignores `kCGDisplayBeginConfigurationFlag` and calls the `NotifyFn` on everything else; `Watcher::stop` removes it. The `NotifyFn` sits in a `OnceLock<Mutex<Option<NotifyFn>>>` as the Win32 arm's does. The callback is delivered on the registering thread's run loop, and `app.rs` calls `start` on the main thread before the event loop runs, so no thread is spawned. The engine's two-second settle and re-query need nothing new.

5. **Session end: take AppKit's default and widen SIGTERM.** winit does not implement `applicationShouldTerminate:`, so a logout terminates the app at once and the app cannot block one, which is the property that matters. The ordered teardown is lost, and the plan accepts that: config is written when a setting changes, the wallpaper lives in the desktop's own store, and the e2e suite's clean-exit cases run through IPC `quit`. What a logout can lose is one debounced interval save. `session_end`'s SIGTERM module widens from `target_os = "linux"` to `unix` (`signal-hook` moves to a `cfg(unix)` target table, the macOS stub goes), so `kill` from a terminal and a future LaunchAgent's stop are the same clean exit they are on Linux. Hooking winit's `exiting` through Slint's `with_custom_application_handler` needs `unstable-winit-030` and is a roadmap item.

6. **The tray is done; the tables are not.** No tray code changes. `tests/common/process.rs`'s `tray_supported()` returns `true` on macOS (every session has a menu bar) and `WALLPAPER_PLATFORM` includes macOS, both in the same commit as the setter, because `test_set_wallpaper` asserts a refusal on any platform off that list. Two things the tester checks: that the status item shows the 32 px mark legibly against a light and a dark menu bar (macOS does not template it; if it reads badly, the fix is a monochrome template image, a `tray.slint` question for later), and that "Open" brings the window to the front. If it does not, the app crate gains an `NSApplication::activate` call in the open callback, main thread, no `unsafe`.

7. **The single-instance lock gets a real path on macOS.** `single-instance` 0.3.3 uses `flock` on a file at the literal name there, and the app passes a relative name, so from Finder the create fails in `/` and the guard is silently absent. On macOS the name becomes `app_data_dir()/instance-<name>.lock`; Windows (a named mutex) and Linux (an abstract socket) keep the bare name. A unit test pins that the macOS name is absolute. If the crate misbehaves beyond the path, the IPC listener's bind, which already happens before the adapter, is the natural replacement on every platform.

8. **An `.app` bundle, ad-hoc signed, zipped by `ditto`.** `Sunlit Earth.app` with `Contents/Info.plist` from a template at `assets/macos/Info.plist` (`CFBundleIdentifier` `earth.sunlit.SunlitEarth`, the project's own domain `sunlit.earth` reversed plus the app name, which is the convention; the redundancy is cosmetic and the string is never shown to a user, it has no hyphen so it also serves as a Flatpak app ID later, and it never changes once chosen because preferences and TCC grants key on it; `CFBundleExecutable` `sunlit-earth`, `CFBundleIconFile` `sunlit-earth`, `CFBundleShortVersionString` and `CFBundleVersion` from the workspace version, `LSMinimumSystemVersion` `11.0`, `NSHighResolutionCapable` true, `NSSupportsAutomaticGraphicsSwitching` true), `Contents/MacOS/sunlit-earth`, and under `Contents/Resources/`: `sunlit-earth.icns`, `textures/` with the four JXL files, `LICENSE` and `THIRD-PARTY-LICENSES.md`. `resolve_textures_dir` gains one candidate per ancestor, `<dir>/Resources/textures`, which finds `Contents/Resources/textures` from `Contents/MacOS/` and changes nothing elsewhere. After assembly the xtask runs `codesign --force -s - "Sunlit Earth.app"` (the linker's ad-hoc seal covers only the raw executable, and the bundle would otherwise report itself damaged; no `--deep`, there is no nested code) and `ditto -c -k --keepParent` into `sunlit-earth-<version>-macos.zip`, whose only top-level entry is the `.app`. Both tools exist only on macOS, so this format refuses on any other host with a message; the layout is unit-tested everywhere. `LSUIElement` is not set: winit forces the regular activation policy anyway, so the app has a Dock icon while it runs, and closing the window hides it to the status item as on the other platforms. The version fields carry the prerelease suffix as is; the App Store would object, and this is not going there.

9. **Apple Silicon only, universal on request.** The build is `aarch64-apple-darwin` on `macos-latest` (macOS 26, arm64). Both workflows take a `universal` input, off by default in both, that adds `rustup target add x86_64-apple-darwin`, a second `cargo build --target`, and `lipo -create`. Off by default because the second slice doubles the compile on the runner whose minutes cost ten times the others, and no tester with an Intel Mac exists yet; the input is there for the release that gets one, and `build-info.json` records the architectures either way. The x86_64 slice is smoke-tested with `arch -x86_64` only if `softwareupdate --install-rosetta --agree-to-license` succeeds on the runner, and otherwise ships with a line in `build-info.json` saying it was not run, since Rosetta is not on the image. `MACOSX_DEPLOYMENT_TARGET=11.0` is set for both slices so the two halves agree, and it matches `LSMinimumSystemVersion`. A `macos-15-intel` runner is the fallback if the cross-compile of the C dependencies (Astronomy Engine through `cc` and bindgen, `ring`) misbehaves; it costs another 10x job and the label ends in fall 2027.

10. **Spaces and the sweep.** A publish paints the active Space of each screen; a Space that was not active keeps the path it was given last time, and `Publication::commit` sweeps every generation but the newest two. On macOS the sweep keeps twelve generations instead, an hour at the default five-minute refresh, as a provisional number: with the walk-back no state file records, this is the cheapest way to keep a stale Space's file alive long enough that switching to it shows an hour-old Earth rather than a missing file. What a Space does when its file has gone is question 1 in the research document; the tester answers it and the number moves with the answer. The constant is per platform in `wallpaper/mod.rs`, and the sweep test takes it as a parameter.

11. **One `bundle` command, three platforms, shared by `dist` and the runners.** `bundle.rs` already runs without a VM. It gets a `Platform { Windows, Linux, MacOs }` enum with `From<Target>`, replacing `Target` in its signatures, so `Target` stays the two-variant guest enum every `match` in `dist.rs` is exhaustive over. A new `cargo xtask bundle --platform <windows|linux|macos> --exe <path> [--out <dir>] [--verify]` assembles the layout from the checkout, writes the archive, reads it back, and with `--verify` unpacks it into a scratch directory and runs the two renders `dist` already defines (`SUNLIT_EARTH_TEXTURES` pointed at an empty directory, then unset, from a working directory outside the bundle), requiring `render_difference` at or above `TEXTURE_LOOKUP_FLOOR`. It writes `build-info.json` beside the archive; `BuildInfo::builder` becomes an enum of the VM record it has today and a `Hosted { runner_image, run_id, run_url }` record for the runners, and `linkage` becomes optional since the runner has no `objdump` or `dumpbin` step. `dist` calls the same functions and its behavior does not change; its tests prove that.

12. **`release.yml` builds three platforms from one tag.** Trigger `v[0-9]+.[0-9]+.[0-9]+*`, which the current glob is not (none of the three existing tags match it, which is why the workflow has never run), plus `workflow_dispatch` with a `publish` input defaulting to false so the pipeline can be exercised without a tag or a release. One matrix job per platform, `fail-fast: false`, `contents: write`, `actions/checkout` with `lfs: true` (the four textures are 4.7 MB), the toolchain from `rust-toolchain.toml` rather than `@stable` (`rustup toolchain install` with no argument reads the file on rustup 1.28 and later; two toolchains on one runner is wasted minutes), the per-OS setup `ci.yml` already has, `Swatinem/rust-cache` with a `release-<platform>` key, a gate that the tag equals `v<workspace version>` so an archive can never be named after a version other than the tag's, `cargo build --release --locked`, `cargo xtask bundle --verify`, and the archive plus its record uploaded as a job artifact. A fourth job downloads every artifact and creates the release with `softprops/action-gh-release`, `prerelease` set when the tag has a hyphen, and `generate_release_notes`. Runners: `windows-latest`, `ubuntu-22.04` (glibc 2.35, the floor `linux-builder` gives; if the label is retired, `ubuntu-latest` with `container: ubuntu:22.04`), `macos-latest`. The Windows job inherits `+crt-static` from `.cargo/config.toml` like every build of the tree.

13. **A manual `macos-build.yml` for builds between tags and for the probe.** `workflow_dispatch` with a `mode` input (`check`, `build`, `build-and-e2e`) and the `universal` flag. `check` runs `cargo check --all-targets` and the `sunlit-core` unit tests, which is the cheap iteration loop for code that cannot be type-checked from Windows (the C build script in Astronomy Engine fails a `cargo check --target aarch64-apple-darwin` here). `build` builds, bundles, signs and verifies as the release job does and uploads the `ditto` zip and the record as artifacts (the zip inside the artifact zip, since `upload-artifact` drops executable bits). `build-and-e2e` also builds the test profile and runs `cargo e2e` with `SUNLIT_EARTH_E2E_WALLPAPER=1`, `screencapture -x` before and after, `sunlit-earth displays`, and `system_profiler SPDisplaysDataType`, uploading the log and the screenshots. This is how a binary reaches a tester without a tag, and its first `build-and-e2e` run is step 7.

14. **A log file when there is no terminal.** Logging goes to stderr only, and an `.app` launched from Finder has none, so a tester's report would have nothing in it. When stderr is not a terminal, `logging.rs` also writes `sunlit-earth.log` under the app data directory through `tracing_appender::rolling` (daily, keep seven), on every platform. The README's tester section says where it is.

15. **The `metal` golden set is regenerated first.** It is fourteen references short and three stale, so the macOS `ci.yml` job is red on the golden suite before this branch touches anything. `golden.yml` is on the default branch and dispatchable now; one dispatch, a review of the images, one commit. Every later macOS CI run then means something.

16. **A bare-binary tarball beside the `.app`.** The macOS runner also writes `sunlit-earth-<version>-macos.tar.gz` in the Linux archive's layout: the executable, `textures/`, `LICENSE` and `THIRD-PARTY-LICENSES.md` under one directory, no bundle. Quarantine is an extended attribute that browsers and Archive Utility set and `tar` in Terminal does not, so `curl -L <url> | tar xz` followed by `./sunlit-earth` meets no Gatekeeper dialog and prints the log to the terminal it was started from, which is the shape a tester's report wants. It is the same binary as the `.app`'s, so the linker's ad-hoc signature travels inside the Mach-O and nothing is signed twice. The README says curl, not Safari, because Archive Utility propagates the attribute and a quarantined executable started from Terminal is refused the same way a double-clicked `.app` is.

## What the runner proves and what it cannot

The runner compiles and unit-tests every line of macOS code, renders through the paravirtual Metal device (phase 2 measured it against WARP), and packages the bundle. The e2e probe (step 7) tells us whether its session can open a window, show a status item and take a wallpaper. If it can, `test_set_wallpaper`, `test_tray_mode_ipc_lifecycle`, `test_single_instance_second_exits`, `test_displays_reports_the_session_layout` and `test_across_screens_writes_what_this_desktop_can_hold` run there and the setter moves to the second tier. If it cannot, those cases skip with their reasons printed, the bundle is still verified headlessly, and the second tier waits for a tester. Either way a real Apple GPU, a Retina screen, a second monitor, Spaces, Gatekeeper's dialogs and logout are tester questions.

## Costs and frugality

The repository stays private (decided 2026-09-06), so every minute is metered: Linux 1x, Windows 2x, macOS 10x, against 2,000 included minutes a month on Free and 3,000 on Pro or Team. Estimates from phase 2's record of the macOS test job (about 18 minutes cold, under two warm) and the Linux and Windows CI jobs.

| Run | Wall time | Billed as |
|---|---|---|
| `release.yml`, cold caches, arm64 | Linux 12 min, Windows 25 min, macOS 18 min | about 240 minutes |
| `release.yml`, warm caches, arm64 | Linux 5, Windows 10, macOS 8 | about 105 minutes |
| `release.yml`, warm caches, universal | Linux 5, Windows 10, macOS 15 | about 175 minutes |
| `macos-build.yml` `check`, warm | 5 min | about 50 minutes |
| `macos-build.yml` `build`, arm64, warm | 10 min | about 100 minutes |
| `macos-build.yml` `build-and-e2e`, arm64, warm | 25 min | about 250 minutes |
| `golden.yml` on macOS, once | 20 min | about 200 minutes |

A month with one release, four checks and four tester builds is about 850 minutes on warm caches, so the caches are what make the budget work. The measures, each of which the workflow files carry as a comment:

- One `Swatinem/rust-cache` key per profile and platform (`release-<platform>` beside the existing `ci-<platform>`), because release and test artifacts share nothing and a mixed cache evicts itself. `CARGO_INCREMENTAL=0` as in `ci.yml`, since incremental artifacts are large and useless across runs.
- A tag run restores from `main`'s cache, but its own save is reachable by no later ref, so on tag runs `save-if` is false and the upload minutes are not spent. The cache `main` holds is warmed by the `publish`-false dispatch, which is also the step that proves the pipeline; run it after a dependency bump, not on every push.
- The release job builds one thing: `cargo build --release --locked -p sunlit-earth --bin sunlit-earth`. No tests, no other members.
- `cargo xtask bundle` compiles the xtask in the dev profile on the runner, which is a second dependency tree. Step 1 measures what that costs cold on each runner; if it is more than a minute or two, the bake dependencies (`resvg`, `ico`, `icns`) and the VM orchestration move behind cargo features so `bundle` compiles without them. The measurement decides, not a guess.
- The toolchain the file pins is installed once; `@stable` beside it is a second download on every cold runner.
- macOS runs are rationed by mode: `check` for iteration on macOS code, `ci.yml` on macOS at the end of a step rather than per commit, `build` when a tester needs a binary, `build-and-e2e` for the probe and after changes to the UI or the suite. `golden.yml` on macOS runs once in this plan.
- Universal builds are off by default (decision 9).
- Both manual workflows carry a `concurrency` group with `cancel-in-progress`, so a second dispatch does not run beside a forgotten first one.
- `lfs: true` costs 4.7 MB of LFS bandwidth per run against a 1 GiB monthly quota, which is not the constraint; if it ever is, an `actions/cache` keyed on the LFS pointer hashes removes it.

## Steps

Each step is a commit or two, `cargo test` and `cargo clippy --all-targets` clean on the host, and where it touches macOS code, a green `macos-build.yml` `check` and then a green `ci.yml` dispatch on macOS.

0. **Regenerate the `metal` goldens.** Dispatch `golden.yml` on `macos-latest`, download `golden-metal`, review, commit. Evidence: a macOS `ci.yml` dispatch passes the golden suite.

1. **`bundle::Platform` and `cargo xtask bundle`.** The enum, the command, `--verify` on the host, the `BuildInfo` changes, `dist` calling the shared functions. Tests: the layout tests gain the macOS case; a test builds a fake `.app` layout and checks `resolve_textures_dir` finds `Contents/Resources/textures` from `Contents/MacOS/`; `cargo xtask dist --target linux` still passes end to end. No macOS code yet.

2. **`release.yml` for Windows and Linux, and `macos-build.yml` in `check` mode.** The new trigger, the version gate, the matrix, the bundle step, the release job, the `publish` input. Evidence: a dispatch with `publish` false produces two archives verified on their runners; `macos-build.yml` `check` is green on the unchanged tree.

3. **The macOS core.** `display/macos.rs` (monitors, outputs, watch), `wallpaper/macos.rs`, the cfg widening in `wallpaper/mod.rs` and `wallpaper_sink.rs`, the two re-gated tests, the sweep constant, the new dependencies under `cfg(target_os = "macos")` (`objc2-app-kit`, `objc2-core-graphics`, `objc2-color-sync`, `objc2-foundation`, `dispatch2`). Unit tests that need no display: the point-to-pixel scaling, the label numbering, the UUID fallback, the note text, the timeout refusal. Evidence: `check`, then a macOS `ci.yml` dispatch.

4. **The macOS app crate.** The lock path, the SIGTERM widening, `tray_supported()` and `WALLPAPER_PLATFORM`, the log file, and an `activate` hook behind the tray's open callback if the probe shows the window not coming forward. Evidence: as step 3; the e2e binary keeps compiling on macOS.

5. **The bundle assets.** `bake icon` gains the `.icns` (`icns` 0.4; sizes 16, 32, 128, 256 and 512 with their `@2x` pairs, from the SVG masters the existing `source_for` picks), committed under `assets/icon/baked/`, with the compare-against-a-fresh-bake test extended; `assets/macos/Info.plist`; the `MacOs` layout in `bundle.rs`; `codesign` and `ditto` in the xtask's macOS format. `docs/app-icon.md` gains the row.

6. **The macOS job in `release.yml` and `macos-build.yml`'s `build` mode.** Evidence: a `publish`-false dispatch of `release.yml` produces four verified archives (the macOS runner writes the `.app` zip and the tarball), and `codesign --verify --strict` passes on the `.app`.

7. **The probe.** `macos-build.yml` in `build-and-e2e` mode. Read the log for each of the 15 cases, the screenshots, and the `displays` output; record the outcome in this document's validation section and set each `platforms.md` row's tier. This is where the question whether the runner's session can show a window closes.

8. **The tester round.** Write `docs/macos-testing.md`: download, the Gatekeeper steps for 15 and 26 and the `xattr` alternative, what to look at (the status item on a light and a dark menu bar, Open, the wallpaper on each screen in each mode, a second Space, `sunlit-earth displays` in Terminal, logout), and what to send back (the log file, the `displays` output, the macOS version and machine, screenshots). Send the zip from a `build` run. Fold the answers into the docs and into this document's departures.

9. **Docs.** `platforms.md` (the table with tiers, a macOS section beside the Linux one: `NSWorkspace`, Spaces, the sweep, the main-thread hop, the UUID ids, what the runner proved), `roadmap.md` (close the two items, add the deferred three: `LSUIElement`, the `exiting` hook, autostart on macOS), `testing.md` (the workflows), `README.md` (the macOS download, install and Gatekeeper steps, the log file), `building.md`, `architecture.md`'s dependency list, `app-icon.md`, `vm-setup.md`'s one sentence about hosted runners, and the platform line in `CLAUDE.md`.

## Acceptance criteria

1. `cargo test` and `cargo clippy --all-targets` pass on the host, and a `ci.yml` dispatch passes on all three OSes, golden suite included.
2. `cargo xtask dist --target linux` and `--target windows` behave as before, and their bundles are the layouts `bundle.rs`'s tests describe.
3. A `release.yml` run on a `v*` tag publishes four archives (the Windows zip, the Linux tarball, the macOS `.app` zip and the macOS tarball) with a record beside each; each archive was verified on its own runner by the two-render comparison; the macOS zip unpacks to a signed `Sunlit Earth.app` that passes `codesign --verify --strict` on the runner.
4. `sunlit-earth displays` on the macOS runner reports at least one monitor with a non-empty id and a pixel size, and `test_displays_reports_the_session_layout` passes there.
5. The e2e probe's outcome for each of the 15 cases is recorded, with its reason where it skipped.
6. At least one tester on macOS 15 or 26 reports the wallpaper set on every screen in per-monitor mode, the status item present, and the window opening from it; or the failures are recorded here with the log attached.
7. `platforms.md`'s macOS column has a tier on every row, and no row claims more than its tier.

## Risks

- The hosted session cannot show windows. Then the second tier never exists and every UI and wallpaper claim waits for a tester; the plan still ships a verified headless bundle. Step 7 answers this early.
- The main-thread hop deadlocks or the main queue does not drain under Slint's loop. The bounded wait turns it into a refusal the status line shows, and the probe's `test_set_wallpaper` would surface it. Fallback: the app crate hands the engine a main-thread executor built on `slint::invoke_from_event_loop`, which is more plumbing but no AppKit assumption.
- `setDesktopImageURL` succeeds and the desktop does not change (the Tahoe reports of late or lost sets). Only a tester sees it; the read-back is logged for that conversation.
- FemtoVG on OpenGL misrenders or is slow on Apple GPUs. Fallback is the software renderer via `SLINT_BACKEND=winit-software`, then Skia if it comes to that.
- Cross-compiling the x86_64 slice fails in `cc` or bindgen. Ship arm64 only (the manual job's default) and record it.
- `ubuntu-22.04` is retired before this lands. The container form is a two-line change.
- Cost. A cold arm64 release is an eighth of a Free plan's month and a cold universal one a fifth. The repository stays private, so the measures under Costs and frugality are the levers, and the cache behavior on tag runs is the one to check first if a release costs more than the table says.
- `CGDisplayCreateUUIDFromDisplayID` needs the ColorSync framework linked and a bindings crate this tree does not have yet. If it fails to link or the UUIDs prove unstable in the tester's report, the fallback id is the display id and the docs say the anchor may not survive a reboot.

## Open questions

1. The sweep depth on macOS (decision 10's twelve) once research question 1 is answered.
2. Whether an Intel tester turns up, which is what switches `universal` on for a release.
3. Whether the xtask's own compile on the runners is cheap enough to leave as it is, or wants the feature gate described under Costs and frugality. Step 1 measures it.

Settled on 2026-09-06 after the first draft: the identifier is `earth.sunlit.SunlitEarth` (decision 8), the bare-binary tarball ships (decision 16), and the repository stays private (Costs and frugality).

## Departures

1. **Step 0 moves after step 3, because the macOS build does not compile.** The first `golden.yml` dispatch on `macos-latest` (run 34055254636, 4 minutes, failed) never reached a test: `sunlit-core` does not build on macOS under `RUSTFLAGS: -D warnings`, with eleven dead-code errors in `wallpaper/mod.rs`. `Publication` and its `write` and `commit`, `begin_publication`, `generation_name`, `GENERATION_COUNTER`, `newest_generation_other_than`, `sweep_generations`, `sweep_legacy_files`, `unfinished` and `encode_png` are reached only from `write_job`, which is `cfg(any(windows, target_os = "linux"))`, so on the one platform with no setter the whole file lifecycle is unused code. This predates the branch: the ref was `f4068dc`, which differs from `main` only in the two plan documents, so macOS CI has been red on `main` since the wallpaper file lifecycle change of 2026-09-03 and nothing dispatched it after. The fix is the setter, not an `allow`: step 3's `wallpaper/macos.rs` calls `write_job`, and widening the two `cfg`s in `mod.rs` is already in that step's list. So step 0 runs after step 3 rather than before it, and the two macOS dispatches step 3 was budgeted for do both jobs: `golden.yml` first, which compiles `sunlit-core` and regenerates the `metal` set in one job, then the macOS-only `ci.yml` run for the whole suite. Decision 15's argument survives the reordering, since what it wanted was that every later macOS CI run means something, and the first one that can run at all is the one after step 3.

## Validation record

Filled in as steps land.

### Step 0, first attempt (2026-09-06)

`gh workflow run golden.yml --ref feat/macos -f os=macos-latest` at commit f4068dc. Run <https://github.com/sunlit-earth/sunlit-earth/actions/runs/34055254636>, 4 minutes, failed in the `Regenerate` step with the eleven dead-code errors departure 1 records. macOS job 1 of the 6 this branch is allowed. No references were produced; the `metal` set is still fourteen short and three stale.
