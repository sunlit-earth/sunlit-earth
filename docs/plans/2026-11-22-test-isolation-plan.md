# Test isolation and window geometry clamp

## Problem

Running `cargo test` overwrites the developer's real settings. On 2026-11-22 the config under `%LOCALAPPDATA%\SunlitEarth\config.toml` held the headless test window's state: `window_x = 0`, `window_y = 0`, `window_width = 1200`, `window_height = 5000`, `time_zone = "utc"`, `scene_time = "2026-04-30T09:15:00Z"` and default values for everything else, auto refresh switched off among them. Every build started afterwards, 0.5.0 included, opened a window 5000 px tall at the top left corner of the screen, and Windows threw it back on screen as soon as it was moved or resized. This is not the first time a test has reached the user's files.

Three things are wrong, and this plan fixes all three:

1. A test wired the app's real Set as Wallpaper callback, and nothing stopped it from saving.
2. Isolation is opt-in. Every test has to remember to redirect every location it might touch, and a test that forgets reaches the user's files with no failure.
3. A saved geometry that does not fit any screen is restored as it is. The only check is that the title bar overlaps a monitor, which a window at (0, 0) passes whatever its size.

## What happened

- `test_every_menu_item_works_under_the_sheet` (`crates/sunlit-app/tests/slint_ui.rs`, added in 7da2b5e on 2026-11-08, after 0.5.0) chooses File > Set as Wallpaper on the `export_sheet()` fixture, which wires the real callbacks through `wired_window_and_link()`. `register_action_callbacks` installs `on_set_wallpaper`, which calls `config::save_config(&read_config_from_window(...))`. `config_path()` falls back to the real data folder because the `slint_ui` process has no `SUNLIT_EARTH_CONFIG`, and cannot set one: `std::env::set_var` is `unsafe` in edition 2024 and `unsafe_code` is denied.
- `create_window()` sizes every test window 1200 by 5000 so the testing backend lays out every control. The testing backend reports position (0, 0).
- Confirmed by running each of the 192 `slint_ui` tests alone against a scratch `SUNLIT_EARTH_CONFIG` and comparing the file afterwards: this test is the only one that writes.
- Restoring that file: `install_tray_and_geometry` (`app.rs`) passes it through `validated_window_geometry`, whose test is a 30 px title bar strip overlapping a monitor. (0, 0) passes, and `set_size` asks for an inner height of 5000.

## What reaches user state today

The places a process can reach that belong to the user rather than to the process, and how each resolves:

| What | Resolved by | Override today |
|---|---|---|
| Data folder (logs, wallpaper output, `gen-*` folders) | `sunlit_core::app_data_dir()` (`dirs::data_local_dir`) | none for the folder itself |
| `config.toml` | `config::config_path()` | `SUNLIT_EARTH_CONFIG` |
| Cloud cache and tile cache | `cloud_fetcher::cache_dir()` | `SUNLIT_EARTH_CACHE_DIR`, or `EngineConfig::cache_dir` |
| `memory-metrics.csv` | `memory` module | `SUNLIT_EARTH_METRICS_DIR` |
| Export's default folder | `export.rs`, `dirs::picture_dir()` / `dirs::home_dir()` | none |
| Autostart entry | `autostart::windows::Location::system()` (HKCU), `linux::system_dir()` (`XDG_CONFIG_HOME`, `HOME`), `macos::system_dir()` (`HOME`) | the backends take a location, so tests pass a scratch one |
| Browser and file manager | `open::url`, `open::folder` | none |
| Desktop wallpaper | the real `WallpaperSink` | engine tests inject a fake |

The env overrides work for a spawned binary (the e2e harness, CI's smoke run, the bundle verification) and do nothing for an in-process test. A spawned binary whose stderr is not a terminal writes its log file into the real data folder, since no variable redirects the folder itself (`logging::file_writer`).

## Decisions

### 1. A process claims user state, and only `main` claims it

A new module, `sunlit_core::user_state`, is the one owner of every location in the table above.

```rust
pub fn claim();                              // main.rs, first statement
pub fn data_dir() -> Option<PathBuf>;        // replaces app_data_dir()
pub fn config_file() -> Option<PathBuf>;     // SUNLIT_EARTH_CONFIG, else data_dir()/config.toml
pub fn cache_dir() -> Option<PathBuf>;       // SUNLIT_EARTH_CACHE_DIR, else data_dir()
pub fn metrics_dir() -> Option<PathBuf>;     // SUNLIT_EARTH_METRICS_DIR, else data_dir()
pub fn pictures_dir() -> Option<PathBuf>;    // dirs::picture_dir(), else dirs::home_dir()
pub fn require(what: &str);                  // for state that is not a path
```

`claim()` sets a process-wide `OnceLock`. Every function above panics when nothing has claimed, with a message naming `what` and pointing at `docs/testing.md`. `main.rs` calls `claim()` before `sunlit_earth::run()`, so every mode and subcommand of the binary has it, and the library never claims on its own. No test calls `claim()`.

The claim is checked before the env overrides are read, so a developer's shell that happens to export `SUNLIT_EARTH_CONFIG` does not open a path around the gate in a test process.

`require(what)` is called at the top of `autostart::windows::Location::system()`, `autostart::linux::system_dir()`, `autostart::macos::system_dir()`, `open::url`, `open::folder` and the constructor of the real `WallpaperSink`.

Explicit paths are never gated. `load_config_from(path)`, `render --config <path>`, `EngineConfig::cache_dir` and the autostart backends given a location keep working in tests. Only resolving the user's default locations is.

**Why a panic and not `None`.** Every caller already treats `None` as "this system has no data folder" and carries on with defaults. A test that tried to save would pass silently, which is how this bug survived two weeks. A panic fails the test that reached for user state, in every test target that exists and every one written later, with nothing for its author to remember.

**Why not `[env]` in `.cargo/config.toml`.** Cargo applies `[env]` to `cargo run` too, so `cargo app` would stop reading the developer's config.

**Why not a cargo feature on a dev-dependency.** Resolver 2 unifies dev-dependency features into the build of the binary that `cargo test` compiles for the e2e suite, and the e2e binary would then behave differently from the shipped one. The isolation would also depend on Cargo's feature rules rather than on one line in `main`.

**Production risk.** Code that resolves a location before `main`'s first statement would panic: a static initializer or a `LazyLock` read during argument parsing. Step 1 greps for those, and the e2e suite in both guests exercises every entry point.

### 2. Tests that want to observe a write get their own folder

```rust
#[doc(hidden)]
pub fn scratch_for_this_thread(dir: &Path) -> ScratchGuard;
```

A thread-local root that `data_dir()`, `config_file()`, `cache_dir()`, `metrics_dir()` and `pictures_dir()` resolve under while the guard lives, ignoring the env overrides. `require()` still panics under it: a scratch folder has no registry key, browser or wallpaper.

It is thread-local because Slint's testing backend runs callbacks and timers on the test thread, and libtest runs tests on parallel threads: one test's folder never leaks into another's. A thread the test did not create (the engine thread, a cloud worker) sees no scratch root and panics if it resolves a location, which is the fail-closed answer; those already take their paths through `EngineConfig`.

The `slint_ui` fixtures that wire real callbacks (`wired_window_and_link()` and through it `export_sheet()`) hold a guard over a per-test `ScratchDir`, so a successful export's `remember()`, Revert to Saved's `load_config()` and Set as Wallpaper's save all land in it.

### 3. Nothing bypasses the module

Two enforcement layers, because clippy is not run in CI:

- **A source scan test** in `sunlit-core`, in the manner of the tests that already read `main.slint` and `sphere.wgsl`. It walks `crates/sunlit-core`, `crates/sunlit-app` and `crates/sunlit-ui`, sources and tests both, and fails on any of `dirs::`, `env::home_dir`, the string literals `"HOME"`, `"XDG_CONFIG_HOME"`, `"APPDATA"`, `"LOCALAPPDATA"`, `"USERPROFILE"`, and `HKEY_CURRENT_USER`, outside `user_state.rs`. The allowlist is a list of `(file, token, reason)` in the test, starting with `autostart/windows.rs` for `HKEY_CURRENT_USER` (its `system()` calls `require`, and its tests use scratch keys under HKCU). The scan also fails on `env!("CARGO_BIN_EXE_sunlit-earth")` outside `tests/common/process.rs`, which is what holds decision 4 in place. This runs in CI.
- **`clippy.toml` `disallowed-methods`** for `dirs::data_local_dir`, `data_dir`, `config_dir`, `config_local_dir`, `cache_dir`, `home_dir`, `picture_dir`, `document_dir`, `download_dir` and `desktop_dir`, with `user_state.rs` and the xtask's VM store (`crates/xtask/src/store/mod.rs`, a tool with its own store, outside this rule) allowing the lint by name. This gives the error in the editor, as the `wgpu::Instance::new` entry does.

### 4. Spawned binaries get one redirect for everything

A new override, `SUNLIT_EARTH_DATA_DIR`, moves the data folder itself, so a spawned binary's log file, wallpaper output and everything else that has no variable of its own follow it. The three existing variables keep their meaning and win over it for their one location.

Every spawn of the app binary in `crates/sunlit-app/tests` goes through one helper in `tests/common/process.rs`, `app_command()`, which sets `SUNLIT_EARTH_DATA_DIR`, `SUNLIT_EARTH_CONFIG`, `SUNLIT_EARTH_CACHE_DIR` and `SUNLIT_EARTH_METRICS_DIR` to the test's temporary folder. The `.env("SUNLIT_EARTH_CONFIG", ...)` lines scattered through `e2e.rs` move into it. The source scan in decision 3 is what keeps a new spawn from going around it.

`SUNLIT_EARTH_DATA_DIR` is also set wherever `SUNLIT_EARTH_CONFIG` is today: `ci.yml`'s smoke run, `macos-build.yml`, and the xtask's bundle verification.

### 5. The restored window fits the screen it lands on

`validated_window_geometry` becomes a fit rather than a yes or no. The decision is a pure function in `config/window_geometry.rs`, tested on every platform:

```rust
fn fit(saved: Rect, frame: Frame, areas: &[Rect]) -> Option<Rect>
```

- Pick the area with the largest overlap with the saved title bar strip. No overlap is `None` and the OS places the window, as today.
- If the outer size (the saved inner size plus `frame`) is larger than the area in either dimension, shrink the inner size to the area minus the frame.
- Move the position so the outer rectangle lies inside the area, keeping the top left corner where it can.

Slint's `set_position` takes the outer position, frame included, and `set_size` takes the inner size, frame excluded (read from `i-slint-core-1.19.0/api.rs`), which is why `frame` is an input.

Per platform:

- **Windows.** Areas are each monitor's work area, `rcWork` from `GetMonitorInfoW`, so the taskbar is excluded. The frame comes from `AdjustWindowRectExForDpi` with `WS_OVERLAPPEDWINDOW`, `bMenu = TRUE` (the native menu bar is non-client area), and the chosen monitor's DPI from `GetDpiForMonitor`. Both are in features `sunlit-core` already enables (`Win32_Graphics_Gdi`, `Win32_UI_WindowsAndMessaging`, `Win32_UI_HiDpi`). The DWM's invisible resize borders are part of the outer rectangle, so the visible window ends a few pixels inside the area, which is the conservative side.
- **Linux and macOS.** Areas are the `display::outputs()` rectangles the on-screen check already uses. No work area: xrandr does not report panels. The frame is zero horizontally and the existing 30 px title bar allowance vertically, an estimate rather than a measurement; window managers constrain an oversized window themselves as well. Under Wayland the position is ignored by the compositor and only the size matters. On macOS `install_tray_and_geometry` places the window a second time once it exists (`PLACES_AGAIN_ONCE_CREATED`, `place_once_created`); that second placement takes the fitted position, not the saved one.
- The window's minimum size is left to Slint, which enlarges a window below the `min-width` its layout declares.

### 6. One rule for when a geometry is saved

`menu::save_geometry` saves only a window that is shown, not minimized and not in full screen. `read_config_from_window_onto` (`ui_callbacks.rs`) writes `window_x` through `window_height` with no such check, and it is behind Set as Wallpaper, the display plan change and the auto refresh change (`app.rs`), which the tray's checkmark drives with the window hidden. A save from a minimized window stores (-32000, -32000) on Windows, which the next start rejects and the user's geometry is lost; a save in full screen stores the monitor's size.

Both paths go through one function, `menu::current_geometry(window) -> Option<(i32, i32, u32, u32)>`, and `read_config_from_window_onto` keeps the stored geometry when it returns `None`. Whether full screen can reach Set as Wallpaper today is confirmed by the test in step 9 rather than assumed.

### Not in this plan

- An older build drops keys it does not know and its next save erases them (`AppConfig`'s doc comment). Running 0.5.0 after a newer build loses `export_*` today. Saving over the file's existing TOML table would keep them; it is its own plan.
- A separate config file for debug builds.
- Restoring the developer's settings. The newest backup with them is `config.v3.toml` from 2026-10-24.

## Implementation steps

1. Add `user_state` with `claim`, the location functions, `require`, and `scratch_for_this_thread`. Grep for statics, `LazyLock`s and `OnceLock`s that resolve a location, and for xtask code calling into the moved functions.
2. Move the resolution out of `config_path()`, `cloud_fetcher::cache_dir()`, the metrics path, `wallpaper/mod.rs`, `logging::file_writer`, `menu.rs` (Open Data Folder), `tray.rs` and `export.rs` into calls of `user_state`. Remove `sunlit_core::app_data_dir()`. Add `SUNLIT_EARTH_DATA_DIR`, through `env_override`.
3. Add the `require` calls to the autostart `system` constructors, `open::url`, `open::folder` and the real `WallpaperSink`.
4. Call `user_state::claim()` first in `main.rs`.
5. Run `cargo test`. Every panic is a test that reached user state. Expected: `test_every_menu_item_works_under_the_sheet`, `config_path_without_override_uses_app_folder` (`config/mod.rs`), and any `slint_ui` export test whose successful export reaches `remember()`. List what panics in the validation section before fixing anything: that list is the inventory of the leaks this plan closes.
6. Fix them. The fixtures from decision 2 take a scratch guard; `test_every_menu_item_works_under_the_sheet` then also asserts that Set as Wallpaper wrote a config into its scratch folder, since that is what the item doing its job means. `config_path_without_override_uses_app_folder` tests the pure resolution with an injected root and the scratch guard.
7. Add the source scan test and the `clippy.toml` entries.
8. Add `app_command()` to `tests/common/process.rs` and move every spawn in `e2e.rs` to it. Set `SUNLIT_EARTH_DATA_DIR` in `ci.yml`, `macos-build.yml` and the xtask's bundle verification.
9. Add `menu::current_geometry`, use it in `save_geometry` and `read_config_from_window_onto`.
10. Add `fit` and the per-platform areas and frame, and use them in `install_tray_and_geometry`.

## Tests

### Unit (`sunlit-core`)

- `user_state`: unclaimed and without a scratch guard, each location function and `require` panics; with a guard, the locations resolve under it and ignore the env overrides; the guard is per thread (a second thread spawned in the test panics); `require` panics under a guard.
- The source scan test from decision 3.
- `fit`: the 1200 by 5000 window at (0, 0) on a 3440 by 1400 area comes back as tall as the area minus the frame; a window larger in both dimensions; one hanging off the right edge moves left; a title bar on no area is `None`; with two areas, the one the title bar is on wins, including a monitor at negative coordinates left of the primary; a window that already fits comes back unchanged.

### `slint_ui`

- `test_every_menu_item_works_under_the_sheet` as in step 6.
- `read_config_from_window_onto` keeps the stored geometry for a hidden window, and for a window in full screen if the testing backend reports one.

### e2e

- A case that starts the app with `window_x = 0`, `window_y = 0`, `window_width = 1200`, `window_height = 5000` in its isolated config and checks the window's outer rectangle lies inside the work area: `GetWindowRect` against `rcWork` on Windows, the frame window's geometry as X11 reports it against the output on X11 (the tool the guest image has for that is checked in step 10), skipped under Wayland with the reason printed.

## Docs

- `docs/testing.md`, Conventions: no test resolves user state; the panic and what it means; `scratch_for_this_thread` for a test that wants to see a write; `app_command()` for a spawned binary; the source scan and its allowlist.
- `CLAUDE.md`, Testing conventions: one line pointing at the above.
- `docs/architecture.md`: `user_state`, and that `main` is its one claimant.
- `docs/platforms.md`: the restore fit per OS.
- `docs/roadmap.md`: the entry.
- `README.md`: `SUNLIT_EARTH_DATA_DIR` belongs in its environment variable table. README is edited only with permission, so this is asked for at step 8.
- The update check branch (`feature/update-check`) adds `update_state.toml` and `update_feed.json` under the data folder; once this lands it resolves them through `user_state`, and the rebase is where that happens.

## Gates

`cargo fmt --check`, `cargo clippy --all-targets`, `cargo test`, `cargo e2e` on the desktop, `cargo xtask e2e --target windows` and `cargo xtask e2e --target linux` (KDE on X11, and GNOME on Wayland for the skip).

## Validation

- Hash every file under `%LOCALAPPDATA%\SunlitEarth` (and `~/.local/share/SunlitEarth` in WSL or the Linux guest), run `cargo test` and `cargo e2e`, hash again: nothing changed, nothing added. Also check that no new file appeared under Pictures.
- Revert the fix to `test_every_menu_item_works_under_the_sheet` locally and confirm the test now fails with the `user_state` message instead of writing.
- Start the release build with the 2026-11-22 config (1200 by 5000 at the origin): the window fits the work area of the monitor it opens on and stays put when moved.
- Record the step 5 inventory here.
