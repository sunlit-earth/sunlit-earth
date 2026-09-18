# macOS support: research (2026-09-06)

Written 2026-09-06 for [2026-09-06-macos-support-plan.md](2026-09-06-macos-support-plan.md). Two sources: a read of every platform seam in the workspace at commit 811e863, and web research on the macOS APIs and the hosted runners, checked the same day. The developer has no Apple hardware, so nothing here was run on a Mac. What has run on a Mac is the hosted CI job phase 2 recorded, and that record is cited where it applies.

## 1. What the code does on macOS today

One fact shapes everything: there is no `cfg(unix)` and no `cfg(target_family = "unix")` anywhere under `crates/`. Every seam is `cfg(windows)`, `cfg(target_os = "linux")`, `cfg(target_os = "macos")` or `not(any(windows, target_os = "linux"))`, so macOS inherits nothing from the Linux arms and always lands in the fallback. Two places have a real macOS arm: `memory/macos.rs` and `about.rs::platform_open`.

| Seam | Where | macOS today | What parity needs |
|---|---|---|---|
| `SystemWallpaper::check_supported` / `publish` | `engine/wallpaper_sink.rs:210-260` | `Err("setting the desktop wallpaper is not supported on this platform yet")` before anything renders | a `wallpaper/macos.rs` exporting `check_supported() -> Result<(), String>` and `set_wallpaper_job(&WallpaperJob) -> Result<String, String>`; the module and `use` lines in `wallpaper/mod.rs:16-24`; the cfgs on `WrittenImages` and `write_job` (`mod.rs:250`, `:322`) widened; the test at `wallpaper_sink.rs:452` that pins the refusal removed |
| `display::monitors()` | `display/mod.rs:217-220` | `None`, so the sink renders one 2560x1440 image for "the default screen size" | `Option<Vec<Monitor>>` with `x, y, width, height` in physical pixels, top-left origin, a stable `id` and `primary`; the test at `display/mod.rs:408` re-gated |
| `display::watch::start` | `display/watch.rs:36-49` | logs "this platform has no way to watch the displays" and returns `None` | `Watcher` with `stop(self)` and `start(NotifyFn) -> Option<Watcher>` over `CGDisplayRegisterReconfigurationCallback`, which the file's own doc names |
| `config::is_position_on_screen` | `config/window_geometry.rs:65-84` | the `not(windows)` arm calls `display::outputs()`, which is `None` off Linux, so only the i16 coordinate range check runs | a macOS `outputs()` in points |
| `session_end::install` | `session_end.rs:370-382` | `None` | see section 3.4; the SIGTERM module is named `unix` but gated `target_os = "linux"` |
| tray | `ui/tray.slint`, `tray.rs` | nothing platform-gated in the app; Slint's `SystemTrayIcon` element | nothing to write: Slint 1.17.1's `system-tray` default feature compiles `items/system_tray/appkit.rs` (`NSStatusBar`, `NSStatusItem`, `NSMenu` through `objc2-app-kit`) on macOS. Never run. `tests/common/process.rs:71-82` `tray_supported()` returns `false` on macOS and must change |
| single instance | `tray.rs:41-69`, `app.rs:94-109` | `single-instance` 0.3.3 on macOS is `flock` on a file at the literal name; the app passes `sunlit-earth-app`, a relative path | a defect: the lock file lands in the working directory, which is `/` for an app launched from Finder, so the create fails, the app warns and runs unguarded. Needs an absolute path under `app_data_dir()` |
| IPC socket | `ipc.rs` | `interprocess` `GenericNamespaced` maps to a socket file in a temporary directory on non-Linux Unix (`SpecialDirUdSocket`, which the crate marks deprecated for its choice of directory) | should work; verify. Both ends use the same mapping |
| textures lookup | `assets/texture_loader.rs:130-154` | walks up from the executable looking for `textures/` | an `.app` puts the binary at `Contents/MacOS/`, so `Contents/Resources/textures` is never visited; one more candidate per ancestor |
| app data | `lib.rs:43-45` | `dirs::data_local_dir()` gives `~/Library/Application Support/SunlitEarth`, already documented | nothing |
| links, fonts | `about.rs:55-61`, `:422-428` | `open`, Menlo | nothing |
| memory | `memory/macos.rs` | `task_info(TASK_VM_INFO)` through `mach2`, scoped `#[allow(unsafe_code)]` with a `// SAFETY:` argument | nothing; this is the FFI pattern every new call follows |
| TLS | `Cargo.toml:19` | `ureq` on rustls with `webpki-roots` | nothing; no keychain integration to add |
| logging | `logging.rs` | stderr only | an `.app` launched from Finder has no stderr, so a tester's report has no log; see the plan |
| `build.rs` | `sunlit-app/build.rs` | Slint compile and the `.built-for` triple record; the exe icon and the stack flag are Windows only | nothing for the binary; the icon belongs to the bundle |
| xtask | `provider/target.rs`, `commands/dist.rs`, `commands/bundle.rs` | `Target` is `{Windows, Linux}`; every `match` in `dist.rs` is exhaustive over the two; `HostOs::Other` exists for a macOS host and refuses every VM command | `bundle.rs` is separable: `layout`, `assemble`, `write`, `read_back`, `verify` and `version` are public, touch no VM, and its tests run against a fabricated repository. A third platform belongs there, not in `Target` |
| icon bake | `commands/bake_icon.rs` | `.ico`, hicolor PNGs, `tray-32.rgba`, `about-256.png` from four SVGs through `resvg` | no `.icns` writer in the tree |
| e2e | `tests/e2e.rs`, `tests/common/` | 15 cases; on macOS 8 run, 2 skip for the tray, 1 does not compile (`WM_ENDSESSION`), `test_set_wallpaper` asserts the sink refuses and then skips, 3 skip for the opt-in | `WALLPAPER_PLATFORM` (`process.rs:135`) and `tray_supported()` extended in the same change as the setter, or `test_set_wallpaper` fails |
| release workflow | `.github/workflows/release.yml` | Windows only, zips the bare exe with no textures, and its tag glob `v[0-9]+.[0-9]+.[0-9]+` matches none of the three tags that exist (`v0.1.0-beta.1` to `.3`), so it has never run | replaced, see the plan |

Two more facts. `.cargo/config.toml` has only the MSVC `+crt-static` line and `rust-toolchain.toml` lists no `targets`, so a second Apple slice needs `rustup target add`. The `bake licenses` target list already includes `aarch64-apple-darwin`, so `THIRD-PARTY-LICENSES.md` covers the Apple crates.

Config is saved when a setting changes (auto-refresh toggled, the interval moved behind a short debounce, a wallpaper set, the display plan changed), not at exit. That matters for section 3.4.

## 2. The bindings already in the tree

Slint's winit backend pulls in `objc2` 0.6.4, `objc2-app-kit` 0.3.2, `objc2-foundation` 0.3.2, `objc2-core-graphics` 0.3.2, `block2` 0.6.2 and `dispatch2` 0.3.1 on the Apple target, so a macOS arm in `sunlit-core` adds those as direct dependencies without a new download. What they offer, read from the crate sources in the local registry:

- `NSWorkspace` is not main-thread-only. `setDesktopImageURL_forScreen_options_error` is `unsafe` (its options dictionary is untyped); `desktopImageURLForScreen` is safe.
- `NSScreen` is `MainThreadOnly`: `NSScreen::screens(mtm: MainThreadMarker)`, `frame` (points, bottom-left origin), `backingScaleFactor`, `deviceDescription` (its `NSScreenNumber` key holds the `CGDirectDisplayID`) and `localizedName` (10.15 and later) are safe but need a marker, and `MainThreadMarker::new()` answers `None` off the main thread. `publish` runs on the engine thread, so the AppKit half of a publish has to hop.
- `objc2-core-graphics` has `CGGetActiveDisplayList`, `CGDisplayBounds` (points, top-left origin, the global display space), `CGDisplayCopyDisplayMode` with `CGDisplayModeGetPixelWidth` and `Height`, `CGDisplayIsMain`, `CGDisplayIsBuiltin`, and `CGDisplayRegisterReconfigurationCallback`. None of these need the main thread. `CGDisplayCreateUUIDFromDisplayID` is not there: it lives in `objc2-color-sync` 0.3.2 behind its `ColorSyncDevice` feature (not deprecated, but moved).
- `dispatch2::DispatchQueue::main()` runs a block on the main thread; the main queue drains while the winit event loop pumps the main run loop.

## 3. Platform facts

### 3.1 Setting the wallpaper

`NSWorkspace.setDesktopImageURL(_:for:options:)` is what every maintained tool uses on macOS 14, 15 and 26: `macos-wallpaper` (Swift, v2.3.4 of 2026-04-19, with a "fix macOS 26 compatibility" release in October 2025) and `desktoppr` (the MDM world's CLI, a bare binary with no bundle). Four things about it:

- Re-setting the same path with new file contents does not refresh the picture. `macos-wallpaper` bounces through an empty URL and waits 0.4 s. This project's generation directories give every publish a new path, which sidesteps it.
- It paints the active Space of each screen and nothing else. There is no API for the other Spaces. A February 2026 report on Tahoe says "Show on all Spaces" switches itself off after a programmatic set.
- Fill without letterboxing is `NSImageScaling.scaleProportionallyUpOrDown` with `NSWorkspaceDesktopImageAllowClippingKey` true, which is also the default when the scaling key is omitted.
- The read-back `desktopImageURL(for:)` is not trusted by anyone who uses it: reports of a directory, the default wallpaper, or the wrong screen's image on 14, 15 and 26. It can be logged, not asserted.

The set needs a process inside the user's GUI login session (a LaunchAgent, not a daemon), not an `.app`. Whether the image persists across logout and login is undocumented; one report describes the stock wallpaper showing for ten seconds after login before the set image returns. Two single-reporter Tahoe 26.1 threads describe sets applying a minute late, with no replies.

`osascript` (`tell application "System Events" to set picture of every desktop`) is ruled out: it needs a TCC Automation grant, TCC attributes a bare binary's request to the terminal that launched it, and since Big Sur the consent dialog often never appears for such callers; the failure is error -1743 with no way to add the entry by hand. The Rust `wallpaper` crate shells out to `osascript` and was last published in 2021.

Sources: https://github.com/sindresorhus/macos-wallpaper/blob/main/Sources/wallpaper/Wallpaper.swift, https://github.com/sindresorhus/macos-wallpaper/issues/44, https://github.com/sindresorhus/macos-wallpaper/issues/53, https://developer.apple.com/forums/thread/807254, https://developer.apple.com/forums/thread/814926, https://github.com/scriptingosx/desktoppr, https://eclecticlight.co/2025/11/08/explainer-permissions-privacy-and-tcc/, https://www.qt.io/blog/the-curious-case-of-the-responsible-process

### 3.2 Displays

`CGDirectDisplayID` is transient across reboots. The closest thing to Windows' device path is the ColorSync UUID from `CGDisplayCreateUUIDFromDisplayID`; BetterDisplay's maintainer recommends it, and displayplacer warns that even it can change when external screens wake in a nondeterministic order. Serial numbers collide (identical models report the same 32-bit serial). `CGDisplayRegisterReconfigurationCallback` fires on the thread that registered it and needs that thread's run loop pumping; the main thread under winit qualifies.

Sources: https://github.com/jakehilborn/displayplacer, https://www.wowsignal.io/articles/multiple_screens_m1, https://github.com/waydabber/BetterDisplay/discussions/3628, https://docs.rs/objc2-core-graphics/latest/objc2_core_graphics/fn.CGDisplayRegisterReconfigurationCallback.html

### 3.3 Slint on macOS

The tray: verified in the vendored source, `i-slint-core-1.17.1/items/system_tray.rs` selects `appkit.rs` under `all(feature = "system-tray", target_os = "macos")`, and `system-tray` is in the `slint` crate's default feature list, which is what the workspace uses. `ui/tray.slint` already carries the one macOS-aware comment: a click on the status item opens the attached menu rather than toggling the window, which is why "Open" is a menu entry.

Renderers: the workspace enables Slint's defaults, `renderer-femtovg` and `renderer-software`, and the selector tries Skia only when compiled in. On macOS FemtoVG is OpenGL through CGL, deprecated since 10.14 and still present in 26. Skia would be Metal at a large compile cost. The software renderer is the fallback.

Activation: winit sets the activation policy to `.regular` at startup whatever `Info.plist` says, so `LSUIElement` does nothing for a winit app unless the policy is set through `EventLoopBuilderExtMacOS::with_activation_policy`, which Slint exposes through `Backend::builder().with_event_loop_builder(...)` behind its `unstable-winit-030` feature. Windows from a process that was not launched by Finder can appear without becoming key; `NSApplication::activate` is the fix if a tester sees that.

Sources: https://docs.slint.dev/latest/docs/slint/guide/backends-and-renderers/backends_and_renderers/, https://github.com/slint-ui/slint/blob/v1.17.0/internal/backends/winit/lib.rs, https://github.com/rust-windowing/winit/issues/261, https://slint.dev/blog/slint-1.17-released

### 3.4 Logout and shutdown

At logout `loginwindow` sends each GUI app a quit Apple event, which AppKit turns into `applicationShouldTerminate:`, and waits up to 45 seconds per app. winit 0.30 does not implement `applicationShouldTerminate:` (checked at 0.30.12 and on master), so AppKit's default applies and the app terminates at once: the app can never block a logout, which is the property the Windows `session_end` work exists for. What is lost is the ordered teardown: winit's `applicationWillTerminate:` dispatches `exiting` and the process ends when it returns, so `slint::run_event_loop()` never returns and nothing after it runs. Since config is saved on change rather than at exit, what that can lose is one debounced interval save. Slint 1.17 can hand that callback to the app through `with_custom_application_handler`, again behind `unstable-winit-030`. `NSWorkspace.willPowerOffNotification` is a supplement that often arrives after termination. An ordinary GUI app gets no SIGTERM at logout; launchd-managed agents do, then SIGKILL after `ExitTimeOut` (20 s default).

Sources: https://github.com/rust-windowing/winit/blob/v0.30.12/src/platform_impl/macos/app_state.rs, https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/Lifecycle.html, https://developer.apple.com/documentation/bundleresources/information-property-list/lsuielement

### 3.5 Shipping without an Apple Developer account

- Apple Silicon runs only signed code, ad-hoc included; Apple's linker ad-hoc signs at link time, so a bare `cargo build` binary runs. Wrapping it in an `.app` breaks that seal, so the assembled bundle is re-signed with `codesign --force -s - "<app>"`. `--deep` is deprecated and unneeded for a bundle with no nested code.
- Gatekeeper on 15.1 and later has no Control-click "Open" bypass. A user who double-clicks an ad-hoc signed download sees "Apple could not verify..." and must go to System Settings > Privacy & Security > Open Anyway; on 26 that step asks for the administrator password (one source). `xattr -dr com.apple.quarantine <app>` still works. A tarball extracted with `tar` in Terminal never gets the quarantine attribute; Safari and Archive Utility set and propagate it.
- Notarization needs the Apple Developer Program at 99 USD a year (individual or organization; organizations need a D-U-N-S number), a Developer ID Application certificate, and `notarytool` with an App Store Connect API key, all of which run on a hosted runner with no Mac of one's own.
- Packaging: `ditto -c -k --keepParent` is Apple's own way to zip an `.app`, preserving permissions, symlinks and extended attributes; `actions/upload-artifact` drops executable bits and symlinks, so an artifact must be an archive. Whether `softprops/action-gh-release` preserves bytes exactly was not found in writing; it uploads through the Releases API.
- Rust's deployment targets: `aarch64-apple-darwin` 11.0, `x86_64-apple-darwin` 10.12, both overridable with `MACOSX_DEPLOYMENT_TARGET`.
- The `icns` crate (mdsteele) is pure Rust, at 0.4.0 since 2026-02-06 after a six-year gap, and needs no `iconutil`, so the icon can be baked on the Windows host.

Sources: https://www.osnews.com/story/141055/, https://wiki.hacks.guide/wiki/Open_unsigned_applications_on_macOS_Sequoia_and_newer, https://eclecticlight.co/2020/08/22/, https://github.com/deskflow/deskflow/issues/9923, https://developer.apple.com/help/account/membership/program-enrollment, https://scriptingosx.com/2021/07/notarize-a-command-line-tool-with-notarytool/, https://github.com/actions/upload-artifact/issues/38, https://doc.rust-lang.org/rustc/platform-support/apple-darwin.html, https://crates.io/crates/icns

### 3.6 The hosted runners

- `macos-latest` is macOS 26 on arm64 since 2026-07-15. Also live: `macos-15`, `macos-26` (arm64), `macos-15-intel`, `macos-26-intel` (x86_64). `macos-13` was retired 2025-12-08, `macos-14` is unsupported from 2026-11-02, and GitHub ends x86_64 macOS runners when macOS 15 retires in fall 2027.
- Rosetta 2 is not listed in the arm64 image READMEs, so an x86_64 slice cannot be smoke-tested there without `softwareupdate --install-rosetta`, which may or may not work in the runner VM.
- The macOS 26 image ships Xcode 26.6 with the command line tools, whose toolchain carries `libclang.dylib` at the path `ci.yml` already probes.
- Billing for a private repository: Linux 1x, Windows 2x, macOS 10x, against 2,000 included minutes a month on Free and 3,000 on Pro and Team. Public repositories get standard runners free and unmetered. Five concurrent macOS jobs on every plan. This repository is private (`gh repo view` on 2026-09-06).
- Ubuntu: `ubuntu-latest` is 24.04 (glibc 2.39); `ubuntu-22.04` (glibc 2.35, the floor `linux-builder` gives) is still offered with no retirement date found.
- GPU: the research turned up a 2022 discussion asking for Metal on hosted runners with no resolution, but this repository's own record contradicts it for the arm64 images: phase 2 ran the whole pipeline on `macos-latest` against `Apple Paravirtual device (Metal, IntegratedGpu)`, generated the `metal` golden set there, and measured its agreement with WARP. That record stands. What is unproven is the window server: GUI apps do open windows on the macOS images in other projects' CI, but one closed issue reports screenshots that show no windows, Dock or status menu. Whether the settings window, the tray and `setDesktopImageURL` work in that session is a question the first probe run answers.

Sources: https://github.com/actions/runner-images/issues/14167, https://github.blog/changelog/2026-02-26-macos-26-is-now-generally-available-for-github-hosted-runners/, https://docs.github.com/en/actions/reference/runners/github-hosted-runners, https://docs.github.com/en/actions/reference/limits, https://github.blog/changelog/2025-12-16-coming-soon-simpler-pricing-and-a-better-experience-for-github-actions/, https://github.com/actions/runner-images/blob/main/images/macos/macos-26-arm64-Readme.md, https://github.com/actions/runner-images/discussions/6138, https://github.com/actions/runner-images/issues/8951, and this repository's [2026-08-15-phase2-cross-platform-plan.md](2026-08-15-phase2-cross-platform-plan.md) (the macOS probe section).

## 4. What only a Mac can answer

1. Whether `setDesktopImageURL` from this app changes the desktop, per screen, on 15 and 26, and what a Space that was not active at publish time shows afterwards, in particular once the sweep has deleted the generation it was handed.
2. Whether the status item appears, its menu works, and the settings window comes to the front when opened from it.
3. Whether the window opens and renders through FemtoVG on OpenGL on a real Apple GPU, and how the preview performs.
4. What `sunlit-earth displays` reports on a Retina laptop with an external 1x monitor, which is the mixed-scale question Windows also has open.
5. Whether logout is instant and whether anything is lost by the missing teardown.
6. What the Gatekeeper dialogs say on 15 and 26 for the ad-hoc signed bundle, step by step, so the README can quote them.
7. Whether the hosted runner's session can show a window at all. A probe answers this without a tester.
