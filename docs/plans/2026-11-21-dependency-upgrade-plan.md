# Slint 1.19 and the dependency upgrade

## Problem

Slint 1.19.0 (released 2026-11-16) carries the upstream fixes two open issues are waiting for, #94 (the menu bar flickers when a menu item's state changes) and #66 (the About window's tabs cannot scroll to the end), and it closes or moves several roadmap items that name Slint 1.18. It needs Rust 1.95, and the tree pins 1.94.0. Since the toolchain has to move anyway, this plan takes every other direct dependency with a newer major version along, in separate steps so each regression has one suspect.

## How this was established

- The Slint 1.19.0 changelog, read from `CHANGELOG.md` at tag `v1.19.0`, and the Slint sources at that tag for the details below (`internal/compiler/layout.rs`, `internal/compiler/project_file.rs`, `internal/compiler/diagnostics.rs`, `api/rs/macros/lib.rs`, `api/rs/build/lib.rs`, `internal/backends/winit/muda.rs`, `internal/common/enums.rs`, the editions page under `docs/astro`).
- That the two fixes are in the release: `gh api repos/slint-ui/slint/compare/<commit>...v1.19.0` reports `ahead` for both c923cab24 (slint-ui/slint#13683, fixing #13655) and fbd87584 (the merge of slint-ui/slint#13119).
- `cargo update --dry-run --verbose` on `main` at a376f23 for the list of held back crates, and `rustup check` for the current stable (1.100.0, 2026-11-09).
- wgpu/naga 30 and the twelve other crates: two research agents, each on a throwaway copy of the workspace in a scratch directory, never in the repository. Both bumped the crates, fixed the compile errors, and ran what the Windows host could run. Their results are summarized per crate below. The Linux and macOS only crates (x11rb, signal-hook, mach2) were read, not compiled.

## Toolchain

Slint 1.19 declares `rust-version = 1.95`. Of the other upgrades the highest MSRV is zip 9.8.1 at 1.89, and wgpu 30 is 1.87, so 1.95.0 is the floor and nothing else raises it.

The pin in `rust-toolchain.toml` reaches CI (the jobs run `rustup toolchain install` in a rustup home of their own) and the builders (`dist` runs `rustup toolchain install <channel>` inside the pristine builder overlay on every build, `commands/dist.rs::build_job`). No image needs rebuilding. The builder's build cache is keyed on the channel (`store/cache.rs`), so the first `dist` per target after the bump is a cold build. The builder images still carry 1.94.0 from their Packer variable; rebuilding them later saves the download per `dist` and is not required.

Newer compilers bring new rustc and clippy lints. CI denies rustc warnings; clippy pedantic is on and not gated, so new clippy findings are this plan's to fix too.

## Slint 1.19

### What it fixes for us

| Issue or roadmap item | Slint 1.19 | Action |
|---|---|---|
| #94, the View menu's checks made the window flicker on Windows (roadmap: "On Windows the preview flashed white...") | slint-ui/slint#13119 is in: the muda menu bar is diffed instead of rebuilt. Changelog: "Fixed the menu bar and task bar flickering when the menu changes" and "Fixed stale native menu events activating items after a menu rebuild". | Bring back the checks of 6c58e99 (Controls, Frame Guides, Pin Time Bar, two way bound, Controls through an inverse of `overlays-hidden`), remove the comment at `main.slint:821`, and confirm on the Windows desktop that nothing flickers. Close #94. |
| #66, the About tabs end early (roadmap: "Since Slint 1.18 the About window's Attributions...") | c923cab24 is in and is not edition gated: a repeated or conditional cell of a layout is measured at the width it is given. Non-layout parents such as a `Flickable` counting their `if`/`for` children (slint-ui/slint#407) is edition 1.19 only. | Remove the preferred width workaround in `about.slint` (the `attributions-column` and `third-party-column` properties and what reads them). The two `scrolls_to_the_end_of_its_document` tests in `slint_ui.rs` decide whether it holds without edition 1.19; if they fail before step 3, they must pass after it. Keep the tests. Close #66. |
| Tray checks drawn as a `✓ ` prefix (roadmap: "Slint's tray backends ignore `checked`") | slint-ui/slint#14419: "SystemTrayIcon: Checkable menu items show their checked state" on all three backends. | Drop the prefix and the comment in `tray.slint`; `checkable` and `checked` are already set. |
| macOS App menu (roadmap: "Slint menu roles for the macOS App menu", and "Whether the macOS App menu's Quit (⌘Q) saves...") | `MenuItem.role` (`about`, `preferences`, `quit`, `services`, `hide`, `hide-others`, `show-all`). On macOS a role item replaces muda's predefined one in the App menu and keeps its own `activated`; elsewhere `about`, `preferences` and `quit` stay where declared. | Decision 3, step 5. With `role: quit` on File > Quit, ⌘Q would run `menu::quit` and save the geometry; with `role: about` on Help > About, the App menu's About opens our window. Hide stays AppKit's (`activated` is not invoked for that role), so its `window_state` bypass stays open. |
| Wayland activation token (roadmap: "Wayland activation needs the token at window creation") | slint-ui/slint#14360 only applies the launcher's token to the first window of a process, from the environment. Handing a second launch's token to an existing window is slint-ui/slint#14822, still open. | Not closed. Update the roadmap entry to name #14822. Note that Slint now clears `XDG_ACTIVATION_TOKEN` and `DESKTOP_STARTUP_ID` when it creates the first window; `Launch::of_this_process` reads them in `hand_over` before any window exists, so this is safe as the code stands. |
| ksni `assume_sni_available` (roadmap) | Not mentioned in 1.19. slint-ui/slint#13693 (tray menu sometimes empty) is a different fix. | Unchanged. |
| macOS see-through title bar (roadmap, waiting on a Mac) | Nothing new. | Unchanged. |

Not tied to an item but relevant: winit no longer renders every change twice on Windows and X11 and no longer wakes at the refresh rate while a window is hidden or minimized; a `Window` dropped while visible no longer keeps the event loop running; the default font family on Windows and macOS was fixed (#13540), which changes text metrics and so possibly the size of hints and rows; Ctrl shortcuts work under Caps Lock and some xkb modifier setups were fixed (#13233); `changed` handlers now run right after a property is set from `invoke_from_event_loop`. Any of these can move a `slint_ui` test or an e2e measurement, and none of them should need code.

### Breaking or noisy changes that hit the tree

1. **`slint!` without an edition is a compile error in CI.** The macro emits a missing edition warning as a `#[deprecated]` constant (`diagnostics::macro_warning`), and CI sets `RUSTFLAGS=-D warnings`. `#[edition = "1.18"]` warns as well; only `"1.19"` is silent. The three `slint::slint!` blocks in `crates/sunlit-app/tests/slint_ui.rs` (lines 2592, 5298, 6096) need `#[edition = "1.19"]` as the first line of their body.
2. **`slint-build` without an edition** reports an info, and `slint-build` drops infos (`diag.remove_infos()`), so nothing shows. New 1.19 compiler warnings, such as a value with no effect at the end of an `if` branch, surface as `cargo:warning` lines and do not fail CI. Fix what appears.
3. **The internal API in the tests.** `slint_ui.rs:2835` reaches `i_slint_core::window::WindowInner` and `InternalToken`, and `i-slint-backend-testing` and `i-slint-core` are pinned `=1.18.1`. Both pins move to `=1.19.0`; the internal call may need adjusting.
4. **The 1.19 language edition** (step 3) changes three behaviors: `if` and `for` elements size their parent like any other child, a `width`, `height`, `x` or `y` set by only some states keeps its default outside them, and inner binding loops are reported as errors. Each can turn into a binding loop error or a changed layout. The editions page lists the fixes.
5. **`SLINT_ENABLE_EXPERIMENTAL_FEATURES`** now takes names, and `1` is an error. The tree sets neither it nor `SLINT_FEATURE_EXPERIMENTAL`.
6. **Transitive changes**: muda 0.19 to 0.21, femtovg 0.27 to 0.28. winit stays 0.30. Slint's own wgpu support is behind `unstable-wgpu-29`/`-30`, which the app does not enable; the wgpu 30.0.1 already in `Cargo.lock` comes from a target this project does not build (`cargo tree -i wgpu@30.0.1` prints nothing for the host), so the shipped binaries hold one wgpu today and must still hold one afterwards.

### Workarounds that 1.19 may or may not retire

- `older-version.slint:34` sets the wrapped text's preferred width by hand because the window takes its height from the layout's preferred height. c923cab24 keeps measuring at the preferred width for "a root that no layout sizes, such as a window or a popup", so this one probably stays. Try removing it, look at the window, and keep it with an updated comment if the window comes out one line tall.
- The gesture row in the overlay hints (`common.slint`, 9ced79d, preview overlays plan) places glyph, text and keys by hand because 1.18 measured the `if` section at its preferred width. With 1.19 and edition 1.19 it could go back to a layout. Optional; the hand placement works.

## wgpu and naga 28 to 30

Eleven edits in `sunlit-core`, all following the compiler's suggestions. After them `cargo clippy --all-targets -p sunlit-core` was clean and every sunlit-core suite passed on WARP on this host with the real textures (697 unit, 4 bc7, 157 engine, 25 golden, 51 render_pipeline, 7 shading, 1 soak), on 30.0.1.

| Change | Call sites | Fix |
|---|---|---|
| `Instance::new` takes the descriptor by value, and `InstanceDescriptor` has no `Default` | `wgpu_init.rs:62`, `:68` | Drop the `&`; `..wgpu::InstanceDescriptor::new_without_display_handle()` in place of `..Default::default()`. The `clippy.toml` entry `wgpu::Instance::new` still matches. |
| `PipelineLayoutDescriptor.bind_group_layouts` is `&[Option<&BindGroupLayout>]` | `gpu_setup.rs:335`; `tests/bc7.rs:243`; `tests/render_pipeline.rs:184`, `:2762` | `Some(...)` |
| `VertexState.buffers` is `&[Option<VertexBufferLayout>]` | `gpu_setup.rs:631` to `:660`; `tests/render_pipeline.rs:199`, `:2772` | `Some(...)`, and the tuple type at `gpu_setup.rs:637` |
| `DepthStencilState.depth_write_enabled` and `depth_compare` are `Option` | `gpu_setup.rs:682`; `tests/render_pipeline.rs:220`, `:2793` | `Some(...)` |
| `BufferSlice::get_mapped_range` returns a `Result` | `render_pass.rs:764`; `tests/common/mod.rs:124`; `tests/shading.rs:227` | `map_err(...)?` in `render_pass.rs`, which already returns `Result<_, String>` |

**The DX12 shader compiler default changed** from `Fxc` (wgpu-types 28.0.0) to `Auto` (30.0.1, confirmed in `backend.rs`): a statically linked DXC if built in (it is not), then a `dxcompiler.dll` found by name, then FXC. A user with `dxcompiler.dll` on the search path would get different shader translation than one without. Decision 4 pins FXC.

Unchanged and checked: `Device::poll` and `PollType`, adapter enumeration and selection, the features and limits requested, `InstanceFlags` and `with_env()`, the internal counters and `generate_allocator_report` that `memory-report` and the soak test read, `copy_texture_to_buffer`, cube, array and MSAA textures, and naga's WGSL parse, validation and MSL output for `sphere.wgsl` and `blend.wgsl`. The tree drops `metal`, `objc`, `gpu-descriptor` and naga 28 and gains `naga-types`, `wgpu-naga-bridge` and `raw-window-metal`, all under licenses already in the corpus.

Not verified: lavapipe on Linux, the Metal paravirtual device on macOS, and 30.0.2 itself (30.0.1 was run; the 30.0.2 sources were read).

## The other crates

Bumped on a copy, compiled with `cargo check --workspace --all-targets` on Windows after the fixes listed.

| Crate | From, to | MSRV | Code changes | Notes |
|---|---|---|---|---|
| glam | 0.30, 0.34.3 | 1.85 | 0.34 removed `Mat4::look_at_rh` and `perspective_rh`. `camera.rs:319`, `:327`, `moon.rs:219`, `tests/render_pipeline.rs:28`, `:31`, `:3525`, `:3526` move to `glam::camera::rh::view::look_at_mat4` and `glam::camera::rh::proj::directx::perspective`; the doc comment at `display/layout.rs:16` follows. | Bit identical on 39 cases. |
| toml | 0.9, 1.2.2 | 1.85 | None. | Serialized `ConfigFile` byte identical both ways, so `config.toml` is unaffected. Removes a duplicate toml. |
| zip | 6, 9.8.1 | 1.89 | `ZipFile::name()` returns a `Result`: `bundle.rs:757` and the test at `:1851`, `:1856`. | Since 7.2 the "version made by" system is the host's, so the Windows archive built on Windows would lose its Unix modes: add `.system(zip::System::Unix)` to the options at `bundle.rs:677`, which restores zip 6's bytes. Resolve with `cargo update -p zip -p zeroize`, or zip stops at 9.4.1. |
| resvg | 0.48, 0.50 | 1.85 | None. | Icon bake unchanged except the `.ico`, below. |
| ico | 0.5, 0.6 | none declared | None. | `sunlit-earth.ico` re-bakes to different bytes (likely its move to png 0.18), so `cargo xtask bake icon` and a committed `.ico` go with it. Removes png 0.17. |
| icns | 0.4, 0.5 | 1.87 | None. | `.icns` byte identical. |
| windows-sys | 0.59, 0.61.2 | 1.71 | `BOOL` moved to `windows_sys::core`: `wallpaper/windows.rs:82`. | All eleven feature names used still exist. 0.59 stays in the tree through other crates. |
| dirs | 6, 7 | none declared | None. | The only change is Windows `preference_dir` (unused). `data_local_dir`, `picture_dir`, `home_dir` unchanged, so no file moves for existing users. |
| pollster | 0.4, 1.0.3 | 1.69 | None. | Removes a duplicate. |
| x11rb | 0.13, 0.14 | 1.68 | None expected; not compiled. | Drops abstract socket support for reaching an X server. Adds a duplicate, since winit, softbuffer and arboard stay on 0.13. |
| signal-hook | 0.3, 0.4.6 | 1.66 | None expected; not compiled. | Only `low_level::pipe` changed; the tree uses `iterator::Signals`. |
| mach2 | 0.6, 0.7 | 1.85 (edition 2024) | None expected; not compiled. | Additions only in the modules `memory/macos.rs` uses. |

x11rb is the only bump that adds a duplicate and buys nothing measurable. It is in the plan because it is small and the Linux guests exercise every call site, but it is the first to drop if the Linux e2e runs show anything.

`cargo xtask bake licenses` has to run after every step that changes `Cargo.lock`, or `the_committed_bake_matches_a_fresh_one` fails.

## Decisions

Settled with the user on 2026-11-21.

1. **Toolchain: 1.100.0**, the current stable, rather than the 1.95.0 floor. The toolchain moves anyway, and the new lints are fixed in step 1.
2. **Slint edition 1.19 everywhere**: the three `slint!` blocks in the tests and `build.rs`. The `build.rs` edition is its own step after the plain upgrade, so a layout change is attributable to the edition and not to the version.
3. **macOS menu roles in this upgrade** (step 5). `role: about` on Help > About and `role: quit` on File > Quit. Nobody here can observe the App menu, so the step is verified on the macOS runner and recorded as reviewed, not observed, in `docs/platforms.md`.
4. **DX12 compiler pinned to FXC** through `backend_options.dx12.shader_compiler` in `wgpu_init.rs`, so shader translation does not depend on whether a user has `dxcompiler.dll` on the path and this upgrade does not move the WARP goldens. Moving to DXC can be a deliberate change later.

## Steps

Each step is one commit on a branch in `.worktrees/`, and each ends with `cargo build`, `cargo test`, `cargo clippy --all-targets` and `cargo fmt --check` passing, plus `cargo xtask bake licenses` where `Cargo.lock` moved.

1. **Baseline and toolchain.** Record `cargo test` on `main`. Bump `rust-toolchain.toml` to `1.100.0`, update the channel in the toolchain tests (`guest/toolchain.rs`, and the `moved_channel` fixture in `dist.rs:2682` if it must differ from the pin), run `cargo update` for the semver compatible updates, fix new lints.
2. **Slint 1.19, behavior unchanged.** `slint` and `slint-build` to `~1.19`, `i-slint-backend-testing` and `i-slint-core` to `=1.19.0`, `#[edition = "1.19"]` in the three `slint!` blocks, the internal API in `slint_ui.rs` if it broke. No `build.rs` edition yet. Update the "Slint ~1.18" line in `CLAUDE.md` and the Slint version mentions in `docs/architecture.md` and `docs/platforms.md`.
3. **Edition 1.19 for the UI.** `.with_edition((1, 19))` in `crates/sunlit-ui/build.rs`. Fix binding loops the compiler reports, then look at every window and sheet on the desktop for layout changes (main window, sidebar, overlays and hints, time bar, export sheet, About, older version notice, shortcut sheet).
4. **Retire the 1.18 workarounds.** In order, each checked on its own: the About column widths (#66), the View checks (#94), the tray prefix, then the trial removal of `older-version.slint`'s and optionally the gesture row's. Update the roadmap entries they close and the menu bar plan's decision 5 note.
5. **macOS menu roles.** `role: about` on Help > About Sunlit Earth and `role: quit` on File > Quit in `main.slint`. File > Quit is behind `if Platform.os != OperatingSystemType.macos` today (`main.slint:803`) because the App menu Slint builds has its own; with the role it becomes one item on every platform, which Slint moves into the App menu on macOS (`muda.rs::take_role_entries`), and the role defaults its shortcut to ⌘Q there. On Windows and Linux both items stay where they are, so the menu tests there must not change. On macOS the App menu's About then opens the app's About window and ⌘Q runs `menu::quit`, which saves the geometry. Verify on `macos-build.yml` as far as a runner can, update the two roadmap entries (menu roles; whether ⌘Q saves) and the macOS App menu paragraph in `docs/platforms.md`, and add the check to `docs/macos-testing.md`.
6. **wgpu and naga 30.** The eleven edits, the FXC pin, `cargo tree -d` showing one wgpu on Windows, Linux and macOS targets, goldens on WARP (`cargo test -p sunlit-core --test golden`).
7. **The other crates.** glam, toml, zip (with `System::Unix`), resvg, ico, icns, windows-sys, dirs, pollster, then x11rb, signal-hook and mach2. Re-bake the icon. `cargo xtask bundle --platform windows --exe <release exe> --verify` to exercise the zip writer and reader.
8. **Cross platform verification.** `cargo xtask e2e --target windows`; `cargo xtask e2e --target linux` on KDE (X11), GNOME on Wayland, sway and i3, which together cover x11rb, the Wayland window fixes and the tray; the goldens on lavapipe through the Linux guest or `golden.yml`; `macos-build.yml` in `build-and-e2e` mode for mach2, objc2 and Metal; `cargo xtask dist --target all` for the release builds and the cold builder cache.
9. **On the Windows desktop with a GPU**, by the user: the View menu checks flip without flicker, the About tabs reach their end, the tray checks show, and the app idles at the same CPU when hidden.
10. **Docs and release.** Roadmap entries closed or reworded, `docs/testing.md` if a test changed, CHANGELOG. The version bump and tag follow the release steps in `CLAUDE.md`.

## Follow-up: macOS fixes after step 9

The user's check on the GitHub macOS runner (step 9, macOS row in Results) found three problems, recorded in `docs/roadmap.md` with what the sources say about each. Decided by the user on 2026-11-22:

11. **Plain View entries on macOS.** On macOS only, View goes back to the plain entries it had before this upgrade: "Toggle Controls", "Toggle Frame Guides" and "Toggle Time Bar Pin", each flipping its property in `activated` after `leave-export-sheet`, with no `checkable` and no `checked` binding, so nothing in View changes after the menu is built. Windows and Linux keep the checks of step 4, which the user confirmed on Windows without flicker. The reason, from the sources: Slint 1.19 replaces a changed entry by index, muda removes the native item at that index, and AppKit's own items in the menu titled View make the index point at the wrong item, so the entry duplicates. Nothing else in View may read changing state on macOS either, which a comment at the menu says. The `slint_ui` tests for the View entries cover both forms, picked by OS as the existing macOS cases are. Docs: `docs/architecture.md`, `docs/usage.md` and `docs/platforms.md` where they describe View, and the roadmap entry, which stays open as a Slint or muda bug to report and links this plan.
12. **The window's position on macOS.** The app saves `window.position()`, the frame's top-left, but restores it with `set_position` before the window exists, which winit on macOS takes as the content's top-left, so each save and restart moves the window up by one title bar height. On macOS, apply the saved position again once the window exists, through Slint's `set_position` on the live window, which goes through `setFrameOrigin` and places the frame exactly. Confirm the mechanism in the sources first (winit 0.30.13 `window.rs` and `platform_impl/macos/window_delegate.rs`, i-slint-backend-winit 1.19.0 `winitwindowadapter.rs`), keep the change to macOS, and test what can be tested without a Mac: the macOS CI jobs compile and run it, and the user checks on the runner that several quits and restarts keep the window where it was. The separate Retina problem (screens in points against a position in pixels in `display/macos.rs`) is not part of this step; it stays an open roadmap entry.
13. **Leaving full screen on macOS** is a known issue and not fixed here: the roadmap entry says so, with what was seen and the three candidate causes.

Gates as before: `cargo test`, `cargo clippy --all-targets` with no warnings, `cargo fmt --check`, the WSL clippy pass if Linux code changes, and `ci.yml` on `macos-latest` plus `macos-build.yml` for the macOS code. Then the user's look on the runner.

## Departures

1. **Tile packs rebuild once.** The semver compatible `cargo update` of step 1 moved jxl-grid to 0.6.2, jxl-modular to 0.11.3, jxl-oxide to 0.12.6, jxl-render to 0.12.4 and image to 0.25.10. Those versions are part of `DECODER_VERSIONS`, which keys the tile packs, so every user's packs are rebuilt once on the first start after the release. Holding the decoders back would have meant pinning five crates below their compatible releases for no gain beyond skipping one rebuild, so the update stays. Whether the CHANGELOG mentions the rebuild is left to the user, who edits it.
2. **Clippy fixes outside the plan's file list.** Clippy 1.100 pedantic found new lints in about 40 files the plan did not name (`as_chunks`, `assert_eq` on empties, `map_or_default`, `midpoint`, `Duration` units, and two `float_cmp` expectations that no longer fire). rustc 1.100 also deprecates the atomics' `fetch_update`, so `activation.rs` and a test in `cloud_fetcher.rs` call `try_update` instead. All of them are mechanical and the plan already counted new lints as its own to fix; this records that they reached well beyond the files listed.
3. **Network access goes through cargo 1.94.0.** The host firewall refuses sockets to cargo 1.100.0 (WSAEACCES), so lockfile updates and registry fetches ran through `cargo +1.94.0` with `CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=allow`, and every build and test ran `--offline` on 1.100.0. The lockfile is the same either way; only the process differs from the plan.

4. **The shortcut sheet's card is three pixels tighter.** Slint 1.19 puts `SystemUi` ahead of `SansSerif` in its fallback families (`i-slint-common` `sharedfontique::FALLBACK_FAMILIES`), which is the default font fix: on Windows text is now set in Segoe UI rather than Arial, whose taller lines make the sheet's two wrapped keyboard rows taller. At 800 by 700 the card then scrolled by 9 pixels, which `test_the_shortcut_sheet_keeps_its_text_on_the_card` forbids. The card layout's spacing drops from 9 to 6 pixels and its top and bottom padding from 12 to 10, which leaves 2 pixels to spare there, as two earlier plans tightened the same card for the same test. The plan expected the font change to need no code; this is the one place it did.
5. **No look at the windows on a desktop in step 3.** The app's GUI may not be launched on the machine this upgrade was made on, so the windows and sheets step 3 lists were not looked at there. With edition 1.19 the UI compiled with no binding loop and no warning, and the `slint_ui` cases, which measure the sheet, the overlays, the time bar, the export sheet and the About tabs, all passed unchanged. What a person would have seen is left to the e2e runs in step 8 and to step 9 on the Windows desktop.

6. **Quit is titled Quit Sunlit Earth on macOS, and the macOS menu tests changed.** A role item keeps its own title in the App menu (`create_app_menu` in Slint's `muda.rs`), and muda's predefined Quit it replaces read "Quit" and the app's name, as macOS apps do, so File's Quit takes that title on macOS and stays "Quit" elsewhere. The testing backend has no menu roles (`supports_application_menu_roles` is false there), so on the macOS runner Quit stays in File, and Slint's core matches `@keys(Control + Q)` against the Control modifier with no remap of its own, the winit backend being what reports ⌘ as Control. So `test_file_quits_except_on_macos` became `test_file_quits` on every OS, `test_control_q_quits_from_the_preview` expects a quit on every OS, and `test_quit_and_about_carry_their_menu_roles` reads the two roles from `main.slint`, since nothing a test can run shows the App menu. With Quit in Slint's menu bar on macOS too, `preview-keys` lets go of a held Q when Ctrl goes down there as well, as it already did for E.

7. **Five `slint_ui` cases fixed for macOS, four of them already failing on `main`.** `ci.yml` on `macos-latest` at 39e4384 failed five `slint_ui` cases. Four are the full screen cases, and `ci.yml` on `main` at a376f23 with Slint 1.18 fails the same four (run 45005690690), because full screen (#92) landed after the last macOS run of `ci.yml`: they tap F11, which `main.slint` leaves out of the View menu on macOS, where ⌃⌘F is the key, and they expect the drawn menu bar gone in full screen, which the testing backend draws on every OS (`supports_native_menu_bar` is false there) and `main.slint` keeps on macOS because the real bar is the system's. The fix is in the tests: they tap ⌃⌘F on macOS, which no case drove before, and expect the testing backend's bar to stay there (`menu_bar_kept_in_full_screen`). The fifth, `test_a_wrapping_gesture_keeps_its_hint_around_it`, passed on macOS under Slint 1.18 and fails under 1.19: `FALLBACK_FAMILIES` in `i-slint-common` puts `SystemUi` before `SansSerif` in 1.19 and the other way round in 1.18, so the runner sets the text in the system font, where the semibold title is 15 px and the two wrapped regular lines of the gesture 29 px together, half a pixel short of the case's two title lines. The gesture still wraps; the case now asks for more than one and a half title lines, which a single line, at most 18 px, cannot reach. The app is unchanged by this departure.

## Results

| Step | `cargo test` | clippy | fmt | Notes |
|---|---|---|---|---|
| 1 | 2037 passed, 0 failed, 24 ignored | 0 warnings | clean | Baseline on `main` was the same count. |
| 2 | 2037 passed, 0 failed, 24 ignored | 0 warnings | clean | Both `scrolls_to_the_end_of_its_document` tests pass with the workaround still in. One failure before departure 4, the shortcut sheet at 800 by 700. Licenses re-baked. |
| 3 | 2037 passed, 0 failed, 24 ignored | 0 warnings | clean | Edition 1.19 reported no binding loop and no warning. |
| 4 | 2037 passed, 0 failed, 24 ignored | 0 warnings | clean | About widths removed (the two scroll cases pass), View checks back with their tests, tray prefix dropped (the tray cases read the bound state), `older-version.slint`'s stated width removed: the window measured 440 by 168 with the body three lines tall with and without it, and the older version case now checks that the body wraps and the window holds its button. The optional gesture row rework was not attempted. |
| 5 | 2038 passed, 0 failed, 24 ignored | 0 warnings | clean | Roles on Quit and About (departure 6). `macos-build.yml` in `check` mode at 651886c succeeded (<https://github.com/sunlit-earth/sunlit-earth/actions/runs/44963464439>): every target compiles on macOS under `-D warnings` and the lib and bin tests pass. That mode does not run `slint_ui`, so the macOS branches of the menu tests run only in `ci.yml` on `macos-latest`. |
| 6 | 2039 passed, 0 failed, 24 ignored | 0 warnings | clean | wgpu and naga 30.0.2, which Slint's optional copy resolves to as well; `cargo tree -i wgpu` names one wgpu, 30.0.2, for each of the five release targets. FXC pinned with a unit test. Goldens 25 of 25 on WARP unchanged. The HAL counters and `generate_allocator_report` are kept by the same backends as in 28 (wgpu-hal sources compared), so the soak's and `memory-report`'s readings stand. |
| 7 | 2039 passed, 0 failed, 24 ignored | 0 warnings | clean | glam 0.34.3, toml 1.2.2, zip 9.8.1, resvg 0.50, ico 0.6, icns 0.5, windows-sys 0.61, dirs 7, pollster 1, x11rb 0.14, signal-hook 0.4.6, mach2 0.7 with the code changes the plan lists. The icon bake changed only `sunlit-earth.ico`. `cargo xtask bundle --platform windows --verify` on a release build: 90 files read back and matched, two renders from the archive, and every zip entry records Unix as its system, `sunlit-earth.exe` 0755 and the rest 0644. x11rb, signal-hook and mach2 compile only in step 8's Linux and macOS runs. |
| 8 | 2039 passed, 0 failed, 24 ignored | 0 warnings | clean | At 373fa38. `RUSTFLAGS="-D warnings" cargo build --all-targets` at 39e4384: no warnings. Clippy in WSL, the first compile of x11rb 0.14 and signal-hook 0.4.6: two `map_or_default` findings in `desktop/probe.rs`, fixed, then none. `cargo xtask e2e --target windows`: 24 passed, 0 failed. `--target linux`: 23 passed, 0 failed on each of KDE on X11, GNOME on Wayland, sway and i3, the window a Wayland client on GNOME and sway and an X11 client on i3; every skip is one the suite makes by design for that session (no tray on GNOME, sway or i3, no login autostart on sway or i3, `xrandr` and the window's rectangle under Wayland, Plasma only cases, the wallpaper read-back under `dbus-send`, and no wallpaper file to measure where i3's setter paints the X11 root). `macos-build.yml` in `build-and-e2e` mode at 39e4384 succeeded (<https://github.com/sunlit-earth/sunlit-earth/actions/runs/44965718179>). `ci.yml` on `macos-latest` at 373fa38 succeeded (<https://github.com/sunlit-earth/sunlit-earth/actions/runs/45006748508>) after departure 7. `cargo xtask dist --target all` at 31d939d, both builder caches cold because the toolchain channel moved: Windows built in 11 min 3 s with 27 imports and none of them the Visual C++ runtime, Linux in 6 min 49 s against glibc 2.35 and fontconfig, and each archive (90 and 100 files) read back, matched and rendered from its own textures in the desktop guest of its target. The goldens on lavapipe: the Linux job of `ci.yml` at 39e4384 (<https://github.com/sunlit-earth/sunlit-earth/actions/runs/44965853753>) ran `tests/golden.rs`, 25 passed and 0 failed, every case under the adapter key `lavapipe` and compared against the committed references, since only `golden.yml` sets `SUNLIT_EARTH_UPDATE_GOLDEN`. Nothing after 39e4384 touches rendering: the changes since are `slint_ui.rs`, the X11 probe in `desktop/probe.rs`, the CHANGELOG and three documents. |
| 9 | | | | By the user on the Windows desktop with a GPU, 2026-11-22: rendering looks right, the Segoe UI text looks good, and the View menu and tray checks work without flicker. |
| 9, macOS | | | | By the user on the GitHub macOS runner held open, 2026-11-22: the App menu's About works, fonts and hints look right, and the window leaves the Dock when hidden to the tray as intended. Three problems, recorded as open in the roadmap: the View menu duplicates Frame Guides and Pin Time Bar once chosen, the lower copy stuck checked; after ⌘Q the window often comes back at the top of the screen; leaving full screen breaks the window. |

## Validation

**Round 1** (range a376f23..5f27874): gates measured at 5f27874, 2039 passed, 0 failed, 24 ignored; clippy 0 warnings; fmt clean. 1 major, 3 minor.

- Major: the roadmap's Wayland token entry did not name slint-ui/slint#14822 as the plan's Slint table asks. Fixed: the entry now describes the 1.19 behavior and names #14822.
- Minor: a roadmap entry said the app uses Slint 1.18.1. Fixed: 1.19.0.
- Minor: architecture.md said a `changed is-open` handler makes Slint 1.18.1's compiler panic, unchecked on 1.19. Fixed: stated in the past tense, as a fact about 1.18.1.
- Minor: `test_quit_and_about_carry_their_menu_roles` reads `main.slint` rather than the menu at run time. Declined: the testing backend has no menu roles to read (departure 6), the roles' effect on macOS is covered by the `macos-build.yml` build and a person on a Mac, and the test fails when either role is removed.

**Round 2** (range 5f27874..53f6870, docs only): the three fixes verified and the decline accepted. 0 major, 0 minor. Final gates at 53f6870: 2039 passed, 0 failed, 24 ignored; clippy 0 warnings; fmt clean.

## Risks

- **Text metrics.** The Windows and macOS default font fix may make text wider or narrower; layouts sized to their text (hints, pills, the export sheet) are what to look at in step 3.
- **Edition 1.19 binding loops** in components that read a parent's size from inside an `if`. The compiler reports them, so they cannot ship silently, but each needs a decision on which side owns the size.
- **Windows creation errors.** Slint 1.19's winit backend reports an error when no renderer supports the required graphics API instead of running without a window. The guests have no OpenGL; the Windows guest forces `winit-software` and the Linux guest uses Mesa's llvmpipe for GL, so both should be fine, and the e2e runs in step 8 are what proves it.
- **lavapipe and Metal on wgpu 30** were not run by the research; step 8 covers both.
- **The macOS menu roles** cannot be observed here; the runner shows the App menu over VNC at best, and a person on a Mac is the real check.
