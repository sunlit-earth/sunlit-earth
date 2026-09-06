<!-- Reviewer notes for docs/reviews/2026-09-04-code-quality-review.md. Line numbers refer to commit 19312ba, the tree the review read, and were moved on 2026-09-05 for the files that changed on main since; the main report lists those under "Changes since the review". The changed code was not re-reviewed. "The brief" is the shared review instruction; "the maintainer's rules" are the project's comment conventions. Runtime claims here are reasoned from the code; the measured figures are in test-timing.md. -->

# Review: `crates/sunlit-app` (the Slint shell)

## 1. Summary

The crate is in good shape: no `unwrap()`, no `panic!`, no `todo!` anywhere in `src/`, every `unsafe` is scoped with a real `// SAFETY:` argument, and the latest-value preview mailbox in `engine_client.rs` is race-free on inspection. Three things stand out.

First, commentary: roughly a third of the long comment blocks restate what `docs/architecture.md`, `docs/roadmap.md`, and the retrospective already say, sometimes three times over in one file. The clearest case is the missing-globe-texture argument, written out at `main.rs:275`, `main.rs:436`, `main.rs:942` and `docs/roadmap.md:100`.

Second, `main.rs` is one 200-line `run_app` inside a file that is CLI types, logging, texture resolution, two subcommands and the windowed path all at once. It splits cleanly along seams that already exist.

Third, `ui_callbacks.rs` has zero test coverage for the four `register_*` functions (330 of its 674 lines), and its `on_load_defaults` and `on_reset` bodies are 25 duplicated lines.

Two real defects: the auto-refresh interval slider does a config-file read plus write on every tick, and `--ipc-socket` panics on a name that is invalid or in use. `main.slint` at 1644 lines (1868 since PR #45) is long for a defensible reason and I recommend only a small split.

## 2. Metrics

| File | Lines | Test-mod lines | Tests | Blocks >3 lines: keep / move / delete | Fns >80 lines | `pub` used only in-file |
|---|---|---|---|---|---|---|
| `main.rs` | 969 | 63 | 2 | 11 / 3 / 4 | `run_app` (200) | n/a (binary) |
| `ui_callbacks.rs` | 674 | 0 | 0 | 10 / 1 / 1 | `register_mouse_callbacks` (81), `register_action_callbacks` (91) | 0 |
| `displays.rs` | 651 | 218 | 17 | 13 / 1 / 0 | `report` (90) | 4: `screen_label`, `anchor_at`, `diagram`, `Diagram` |
| `mouse_math.rs` | 739 | 499 | 48 (38 + 10 proptest) | 12 / 6 / 0 | none | 7: `apply_globe_drag`, `wrap_longitude`, `wrap_angle_180`, `DragSpeed::speed`, `FINE_DRAG_SPEED`, `COARSE_DRAG_SPEED`, `DRAG_SPEED_TIME_CONSTANT` |
| `session_end.rs` | 519 | 111 | 5 | 3 / 3 / 1 | none | 0 |
| `engine_client.rs` | 245 | 0 | 0 | 5 / 0 / 0 | `event_forwarder` (90) | 0 |
| `ipc.rs` | 219 | 0 | 0 | 5 / 0 / 0 | `dispatch_command` (108) | 0 |
| `tray.rs` | 141 | 22 | 2 | 4 / 0 / 0 | none | 0 |
| `about.rs` | 105 (705 after PR #45) | 51 | 3 (9 after PR #45) | 0 / 0 / 0 | none | 0 |
| `lib.rs` | 10 | 0 | 0 | 0 | none | 0 |
| `build.rs` | 23 (93 after the target-directory fix) | 0 | 0 | 0 | none | n/a |
| `ui/main.slint` | 1644 (1868 after PR #45) | n/a | n/a | 12 / 1 / 0 | `MainWindow` (1355) | n/a |

Attributes: 21 `#[allow(...)]`, 0 `#[expect(...)]`. 8 of the 21 are `unsafe_code` (all in `session_end.rs` plus `main.rs:833`), 12 are cast lints, 1 is `too_many_lines` + `needless_pass_by_value` on `run_app`. `#[expect]` is used exactly once in the whole workspace (`sunlit-core/src/assets/stars.rs:35`), so `#[allow]` is the house style and this crate is consistent with it.

`expect(` in non-test code: `main.rs` 18, `ipc.rs` 3, `tray.rs` 3, `displays.rs` 2, `engine_client.rs` 2, `about.rs` 0, `session_end.rs` 0, `ui_callbacks.rs` 0.

Proptest case count: default. There is no `ProptestConfig`, no `proptest.toml`, and no `cases:` override anywhere in the workspace, so each of the 10 proptests runs 256 cases, 2560 in total, all pure `f32` arithmetic.

## 3. Findings

### A. Commentary

**A1 (medium): The same argument written three times, and a fourth time in `docs/roadmap.md`.**
`main.rs:275-281` (doc on `have_globe_texture`), `main.rs:436-449` (a 14-line block inside `run_render`), `main.rs:942-948` (doc on the test) and `docs/roadmap.md:100` all explain that `TexturesReady` never arrives for a directory holding only the Moon or the panorama. The block at `main.rs:436-449` additionally says "it is on the roadmap as its own fix", which is exactly the cross-reference the maintainer's rules put in commits and docs. Delete `436-449` entirely; the `let have_globe = have_globe_texture(...)` line plus the doc on the function already say it. Delete the test doc at `942-948` (the test name says it). Keep `275-281`, trimmed to the slot-index convention, which is the one non-obvious fact ("the slot is the index plus one").

**A2 (medium): `session_end.rs:320-336` carries a plan reference and repeats the module doc.**
Line 321 reads "which is the Linux session's way of saying it is going (phase 5 decision 8)". The rest of the block restates paragraphs already in the module doc at `1-35` and in `docs/roadmap.md:19`. Keep only the last paragraph (`334-336`), the signal-handler-safety constraint, which is a real invariant and the reason `signal-hook` is a dependency. Delete the rest.

**A3 (medium): `session_end.rs:1-35`, a 35-line module doc that is mostly history.**
Lines 9-15 narrate the bug ("the tray process sat there until it was killed"), 16-23 argue an alternative that was rejected, 32-35 point at the roadmap. `docs/architecture.md:87` says all of it. The one sentence that must stay is lines 22-23: `HWND_MESSAGE` windows do not receive `WM_QUERYENDSESSION`/`WM_ENDSESSION`, which is an undocumented-looking Win32 fact that would be re-broken the first time somebody "tidies" the window into a message-only one. Trim 35 lines to about 10.

**A4 (medium): `main.rs:782-786` and `main.rs:802-806` are history.**
"Nothing here used to answer either, so the shutdown screen named this process as the one preventing it" and "which is what retired the `process::exit(0)` that used to dodge a thread-local destruction panic in wgpu's `Queue::drop`". Both belong in the retrospective, which already has them. Delete the first; reduce the second to one sentence if the ordering constraint (engine shutdown before the display watcher stops) needs stating, which it does at `809-810` and already does.

**A5 (low): `mouse_math.rs` test bodies explain why the test exists.**
`395-408`, `412-437`, `441-449`, `463-478` carry 4-8 line prose blocks including measurements ("`fine + (coarse - fine)` rounds away from `coarse` at 62 of these 1001 zooms", "at 140 degrees of sky it is never active"). The maintainer's rules name "notes on why a test exists" as belonging in commits. The measurement in `463-466` is the strongest of them because it justifies the branch in `drag_gain`; move it to the doc on `drag_gain` (where a shorter version already is, at `133-137`) and delete it here. Keep the `image_pixels` helper doc at `372-377`, which states the coordinate convention the helper returns.

**A6 (low): `mouse_math.rs:109-125` and `127-154` duplicate `docs/architecture.md:88`.**
The architecture doc's paragraph on the drag gains is a near-verbatim expansion of these two doc comments. The `:param:`/`:returns:` sections are good API docs and should stay. The middle paragraphs ("Neither side of that ratio depends on the camera's distance, which is the point") are rationale. Trim each by about half.

**A7 (low): restating comments.** `ui_callbacks.rs:1-4` ("Groups all Slint callback registrations by category and provides helper functions for applying/reading config to/from the window") says what the four function names say. `engine_client.rs:8` ends "exactly the failure mode Phase 0 removed from the texture path", a plan reference. `mouse_math.rs:3-4` ("These functions are extracted from the mouse callback closures in `main.rs`") is history and is now wrong: they are called from `ui_callbacks.rs`, not `main.rs`. That last one is a good illustration of comment rot.

**A8: comments that earn their place (do not touch).**
`ui_callbacks.rs:593-606`, the "read-modify-write against what is on disk" doc, states an invariant that prevented real data loss and is guarded by a named test; keep it. `main.rs:235-243` (`TEXTURE_MIN_BYTES`) carries the Git LFS pointer invariant, which nothing in the code says. `main.rs:657-661` names the display watcher's consumer and its "always" condition, which `CLAUDE.md` requires of every background producer. `displays.rs:173-188` explains why `replace_monitors` returns the row rather than only setting it, a testability constraint that is not visible from the signature. `ipc.rs:158-166` states that `query-memory`'s single line is a parsing contract. All of `main.slint`'s comments are of this kind: Slint layout gotchas (`49-58`, `80-84`, `131-135`, `189-193`, `646-651`) that would be silently re-broken without them.

### B. Length and structure

**B1 (medium): `main.rs` at 969 lines is five concerns in one file, and `run_app` is 200 lines.**
Proposed split, into the library rather than into sibling modules of the binary, so `tests/` can reach them the way it already reaches `ui_callbacks` and `displays`:

| New module | Items | Lines today |
|---|---|---|
| `cli.rs` | `Cli`, `Quality`, `TextureResolution`, `Mode`, `TrayStart`, `Commands`, the two `From` impls | 32-163 (132) |
| `logging.rs` | `init_logging` | 165-233 (69) |
| `startup.rs` | `TEXTURE_MIN_BYTES`, `resolve_texture_paths`, `have_globe_texture`, `effective_texture_resolution`, `engine_config`, plus the two tests at 908-969 | 235-346 + 908-969 (174) |
| `headless.rs` | `run_displays`, `run_render` | 348-476 (129) |
| `app.rs` | `init_ui`, `register_auto_refresh_callback`, `start_viewport_timer`, `run_app` | 478-819 (342) |
| `main.rs` (left) | imports, `attach_parent_console`, `main` | ~110 |

Within `run_app` (620-819) the seams are already marked by blank lines and comments: engine start plus display watcher (636-672), tray plus geometry (681-696), the startup-refresh timer (704-721), the close handler (731-756), IPC plus show plus deferred hide plus session-end (758-795), teardown (801-818). Extracting five helpers of 15-30 lines each takes `run_app` to about 70 and removes the `#[allow(clippy::too_many_lines)]`.

**B2 (low): `ipc::dispatch_command` at 108 lines is one `match` with six arms.**
It is long but flat, and each arm is self-contained. `report_displays` is already extracted for the one arm that needed it. Splitting it would only move arms behind names the `match` already supplies. Leave it; it is long for a good reason.

**B3 (low): `displays::report` at 90 lines.**
Four sections separated by blank lines: the monitor list (280-292), the anchor (294-310), the canvas (312-334), the renders (336-351). Four private `fn write_*(out: &mut String, ...)` helpers would make it four calls. Worth doing if `report` grows again; not urgent.

**B4 (low): `engine_client::event_forwarder` at 90 lines is one closure with five match arms.**
The `PreviewFrame` arm (157-189) carries the mailbox protocol and is the only non-trivial one. Extracting it as `fn forward_frame(mailbox: &Arc<PreviewMailbox>, weak: &Weak<MainWindow>, ...)` would make the protocol readable on its own. Optional.

**B5: `ui/main.slint` at 1644 lines (1868 since PR #45): a small split only.**
It is one `MainWindow` (212-1566, 1355 lines) plus `AboutWindow` (1568-1817, 41 lines at review time and 250 since PR #45), `TrayIcon` (1818-1868), the `MonitorTile` and `AboutBlock` structs (8-31) and six small reusable components (`Splitter` 32-47, `Hint` 59-78, `SettingRow` 85-129, `SettingCheck` 136-162, `SettingCombo` 165-205, `SettingHeading` 208-210). Inside `MainWindow`, 832 of those lines are the Advanced section (613-1444), ten `GroupBox`es of `SettingRow` instances.

Worth splitting: `ui/widgets.slint` for lines 3-210 (the two structs and the six components, 208 lines) and `ui/tray.slint` for 1814-1868. That takes `main.slint` to about 1600 and gives the reusable widgets a file whose name says what they are. `AboutWindow` was 36 lines when this was written and could go with the tray or stay; at 250 lines since PR #45 it is worth its own `about.slint`, which takes `main.slint` to about 1350.

Not worth splitting: the Advanced section. Slint has no way to hand a component a group of two-way-bound properties in bulk. Extracting `GroupBox { title: "Celestial" }` (908-1152) into its own component means re-declaring all 24 of its `in-out property <float>`s in the child and writing 24 `<=>` bindings at the instantiation site, so 832 lines become about 900 across two files, and every new parameter costs three edits instead of two. The `Setting*` components already carry the repetition that could be factored out. Recommend leaving 598-1429 where it is and saying so in `docs/architecture.md`, which currently explains the widget design (line 91) but not why the file stays long.

### C. Consistency

**C1 (low): item order.** `ui_callbacks.rs` opens with a private `register_globe_drag` (28) before the public `register_mouse_callbacks` (75) that calls it; every other file in the crate puts public items first. `displays.rs` interleaves free functions, `pub type`, `pub const`, `pub struct Diagram`, more free functions, `pub struct DirectorySink` with its impls, and one private `fn shared` at 254 between two public functions. `CLAUDE.md` does not mandate an order, but the crate has one everywhere else (module doc, imports, consts, types, impls, free functions, tests).

**C2 (low): `session_end.rs` names its Windows test module `windows_tests` (459)** while every other file in the crate uses `mod tests`. `#[cfg(all(test, windows))] mod tests` would collide with the plain `mod tests` above it, so a nested `mod windows` inside `mod tests` is the shape that keeps the convention.

**C3 (medium): `#[allow(clippy::cast_*)]` at function scope where one line needs it, and one that is dead.**
`ui_callbacks.rs:607` puts `#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]` on `read_config_from_window`, whose body (608-615) contains no cast at all. It is a leftover. `ui_callbacks.rs:509` allows two cast lints across the whole 66-line `read_params_from_window` for the single `as usize` on line 511. Both are the argument for `#[expect]` over `#[allow]`: the compiler would have flagged the first, and the second would have to be narrowed. A shared helper would remove three more: `fn slider_index(value: i32) -> usize` and `fn slider_u32(value: f32) -> u32` cover `509`, `main.rs:565`, `main.rs:598` and `ui_callbacks.rs:640`, leaving one allow inside each helper.

**C4 (low): error handling style.** The crate is consistent: `String` errors across the `EngineLink`/`WallpaperSink` boundary (matching `sunlit-core`), `ExitCode` from the subcommand runners, `Option` for the two `install`/`acquire` paths, no error enum anywhere. For a shell crate whose errors all end at a log line or an exit code, that is a defensible choice, and introducing `thiserror` here would buy nothing. No change recommended.

**C5 (low): visibility.** Four items in `displays.rs` (`screen_label` 55, `anchor_at` 99, `diagram` 123, `Diagram` 108) and seven in `mouse_math.rs` are `pub` with no caller outside their own file. `DragSpeed::speed()` (`mouse_math.rs:183`) has no caller anywhere in the workspace; it is dead code that `dead_code` cannot see because the module is `pub`. `apply_globe_drag` (31) has no production caller either: `ui_callbacks.rs:59` calls `apply_globe_drag_at`, and `apply_globe_drag` exists only so eight unit tests and two proptests have a shorter signature to call. Making these private would let the compiler tell you when the next one goes dead; the tests keep working because they are in the same module.

### D. Duplication

**D1 (medium): `layout::Framing` is built from `SceneParams` in two crates.**
`main.rs:367-372` and `sunlit-core/src/engine/mod.rs:1100-1105` are the same four field assignments. If a fifth framing field is ever added, `sunlit-earth displays` prints a plan the engine does not build, silently. Fix: `impl From<&SceneParams> for layout::Framing` in `sunlit-core`, and both sites become one line. Confirmed by reading both.

**D2 (medium): `ui_callbacks.rs:248-278` and `284-314` are 25 duplicated lines.**
`on_load_defaults` and `on_reset` differ only in `AppConfig::default()` versus `config::load_config()`; everything after that (apply, defer combo indices, redraw the diagram, clear the one-run flag, send `SetTextureResolution`, send `SetDisplayPlan`, push params) is character-for-character the same. Extract `fn apply_whole_config(win: &MainWindow, engine: &EngineLink, screens: &SharedMonitors, config: &AppConfig)` and both callbacks become four lines.

**D3 (medium): the gamma slider curve is re-implemented in `main.slint`, twice.**
`main.slint:1399` and `main.slint:1425` compute `(day-gamma <= 0.5 ? 0.2 + 1.6 * day-gamma : 1.0 + 4.0 * (day-gamma - 0.5))` as the value label. That is `gamma_slider_to_value` from `sunlit-core/src/params.rs:430` with `GAMMA_MIN = 0.2` and `GAMMA_MAX = 3.0` inlined. I checked the arithmetic and it currently agrees. Change either constant and the two labels lie with no test failing. The crate already has the right pattern for this: `EngineLink::push_params` (`engine_client.rs:134`) computes `zoom_display_distance` in Rust and writes it to the window. Do the same with two `out property <float>` values and delete both inline formulas.

**D4 (low): `cli.quality.map_or(config.quality_tier, QualityTier::from)` at `main.rs:309` and `main.rs:667`.**
`effective_texture_resolution` (297) exists for exactly this shape; add `effective_quality` beside it.

**D5 (low): three copies of `.expect("the monitor list lock is poisoned")`** at `main.rs:535`, `displays.rs:34` and `displays.rs:204`. One `fn lock(screens: &SharedMonitors) -> MutexGuard<Vec<Monitor>>` in `displays.rs` would hold the message once.

**D6 (low): `main.rs:388-389`.** `engine_config(cli, config, (640, 360), false)` already sets `preview_enabled: false`; line 389 sets it again. Dead line.

**D7 (low): `mouse_math::wrap_longitude` (11-13) and `wrap_angle_180` (17-19) have identical bodies.** Two names for one function, with nine tests between them. One `fn wrap_180(angle: f32) -> f32` plus two documented aliases (or just the one function) removes the duplicate and two of its tests.

### E. Correctness and resilience

**E1 (medium, confirmed by reading): the auto-refresh interval slider writes the config file on every tick.**
`main.slint:431-439` binds the interval `Slider`'s `changed(value)` to `root.auto-refresh-changed()`. `main.rs:553-580` handles that callback and ends with `config::save_config(&ui_callbacks::read_config_from_window(&win, &engine))`. `read_config_from_window` (`ui_callbacks.rs:608`) calls `config::load_config()`, and `save_config` (`sunlit-core/src/config.rs:508`) serializes, writes `config.toml~` and renames it. So dragging the slider from 1 to 30 does about thirty read-serialize-write-rename cycles on the UI thread, plus thirty `SetAutoRefresh` commands. Fix: split the callback so the checkbox saves and the slider only sends `SetAutoRefresh`, with the save deferred to the checkbox, the "Set as Wallpaper" button, or window close, all of which already save. Alternatively debounce with a `slint::Timer`.

**E2 (medium, confirmed by reading): `--ipc-socket` panics on a bad or busy name.**
`ipc.rs:40-42` `.expect("failed to convert IPC socket name")` panics for a name `interprocess` cannot turn into a namespaced name, and `ipc.rs:44-47` `.expect("failed to create IPC listener")` panics when the name is already taken, which is the ordinary "a previous run left it behind" case on Linux and "another process holds the pipe" on Windows. `tray.rs:46-49` `.expect("failed to create single-instance mutex")` panics on the same input, since the mutex name is `format!("sunlit-earth-{name}")` (`main.rs:891-894`). All three are reachable from a documented CLI flag. Change `spawn_ipc_listener` to return `Result<JoinHandle<()>, String>` and have `run_app` log the error and continue without IPC (or return `ExitCode::FAILURE`); make `acquire_single_instance` return `Result` and treat the error as "assume we are alone" with a warning.

**E3 (medium, plausible, not verified): `tray.rs:57` and `tray.rs:117` panic if the desktop has no tray.**
`TrayIcon::new().expect("failed to create tray icon")` and `tray.show().expect("failed to show tray icon")` run in the default mode. On a Linux desktop with no `StatusNotifierItem` host, Slint's tray backend can fail; if it returns `Err` there, the app dies at startup in its default configuration with a panic message. I did not confirm Slint's behavior on that platform. Whatever it does, the resilient shape is the same: log a warning, fall back to windowed mode, and tell the user the tray is unavailable.

**E4 (low, confirmed): `MainWindow::new().expect("Failed to create window")` at `main.rs:632`, `window.show().expect(...)` at 764, `run_event_loop_until_quit().expect(...)` at 798.**
Each of these is a `slint::PlatformError` from a real environment problem (no display server, no usable backend) surfacing as a Rust panic with a backtrace. `let Ok(window) = MainWindow::new() else { error!(...); return ExitCode::FAILURE; }` costs three lines each and turns a crash report into a message.

**E5 (low, confirmed): the 13 `.parse().expect("valid directive")` calls at `main.rs:185-202`.**
The invariant is real: every argument is a string literal. But thirteen repetitions of the same fallible call is the wrong shape. A `const MODULE_LEVELS: [&str; 13]` folded with `filter_map(|d| d.parse().ok())` removes twelve `expect`s and eight lines, and cannot be wrong.

**E6 (low, confirmed): `ipc.rs:58-60` reads an unbounded line.**
`BufReader::new(stream).read_line(&mut line)` grows `line` until a newline arrives. A local process that connects and streams bytes without a newline allocates without limit. Only reachable with `--ipc-socket`, and only from the same machine, so the severity is low, but `(&mut reader).take(1024).read_line(...)` is a one-line fix.

**E7 (low, plausible): `ipc.rs:55-71` has no backoff on a repeated accept error.**
If `listener.incoming()` yields `Err` persistently rather than ending, the loop spins and writes a `warn!` per iteration. I did not verify that `interprocess` can produce that state.

**E8 (low, confirmed): `session_end.rs:127-153` leaves `HANDLER` set when installation fails.**
The `OnceLock` is populated before the listener thread is spawned. If `spawn` fails (`.ok()?`, line 147) or `create_window` returns 0 (line 152), `install` returns `None` but `HANDLER` is occupied, so a retry logs "a session-end listener is already installed" forever. There is exactly one caller, so this is theoretical today.

**E9 (low, confirmed): `session_end.rs:192-200`, `fn zeroed<T>() -> T`.**
The `// SAFETY:` comment argues about `WNDCLASSW` and `MSG`, which are the two types it is called with, but the function is generic and `std::mem::zeroed::<T>()` is unsound for many `T` (a reference, a `NonNull`, an enum with no zero variant). The safety argument is stated one level above where it applies. Two non-generic helpers, or marking the function `unsafe fn` so each call site restates the argument, would put the claim where it is true. Everything else in the module is correct: `209`, `220`, `229`, `256`, `262`, `287` and the test at `473` each have a scoped `#[allow(unsafe_code)]` and a `// SAFETY:` line that names the real precondition. The one at `270` sits on the `unsafe extern "system" fn wndproc` declaration and covers the whole body, which is broader than "scoped", but the only `unsafe` block inside carries its own argument at `284-286`.

**E10: things I checked and found correct.**
The `PreviewMailbox` protocol in `engine_client.rs:156-189` has no lost-wakeup and no frame accumulation: the consumer clears `wake_pending` before taking the slot, so a frame that lands in the gap triggers a fresh wake-up whose closure finds an empty slot and does nothing, which the `if let (Some(frame), Some(win))` guard handles. No lock is held across `invoke_from_event_loop`. `displays::replace_monitors` (189-206) takes the monitor lock only after `apply_diagram_to_window` returns, so there is no nesting anywhere and no lock-ordering question to answer. `defer_combobox_indices` (400-413) handles the no-event-loop case with `.ok()`, which is what makes the `slint_ui` tests possible. `DragSpeed::observe` clamps a negative or zero interval before dividing.

### F. Unit tests

**F1: `mouse_math.rs`: 48 tests, none slow, about 12 mergeable.**
All 48 are pure `f32` arithmetic with no I/O, no sleeps and no allocation beyond a `Vec2`. The two longest are `a_slow_hand_turns_the_globe_at_the_same_rate_at_every_zoom` (2002 iterations) and `a_sweep_turns_the_globe_exactly_as_it_always_did` (1001), which are microseconds each. The 2560 proptest cases are the same. This file is not a runtime problem, and I would not reduce the proptest counts.

What is redundant:

- The seven `wrap_longitude_*` tests (251-285) are seven one-assertion tests of one function. `identity_at_zero` and `identity_at_negative_90` are the same behavior twice. `wraps_positive_360`, `wraps_negative_360` and `wraps_540` are three spellings of "modular". Merge to two: one for the two boundaries at ±180 and one table-driven case list for the rest. Removes 5.
- `wrap_angle_180_identity_at_zero` and `wrap_angle_180_wraps_360` (290-297) test a function whose body is identical to `wrap_longitude`'s. Deduplicate the function per D7 and both disappear. Removes 2.
- `globe_drag_longitude_wraps` (335-343) asserts only that the result is in `[-180, 180)`, which is exactly what the proptest `globe_drag_longitude_always_in_range` (644-654) asserts over 256 cases. Fully redundant. Removes 1.
- `globe_drag_latitude_clamped_at_89` / `_at_minus_89` (323-332): the proptest `globe_drag_latitude_always_in_range` covers the invariant; the two exact-value tests are worth one merged test naming both ends. Removes 1.
- `frame_drag_clamped_at_positive_3` / `_negative_3` (514-525), `orient_drag_clamped_at_90` / `_at_minus_90` (547-558), `zoom_scroll_clamped_at_zero` / `_at_one` (589-596): three pairs, each covered by a matching proptest, each worth one merged test. Removes 3.

Total: 48 to 36, with no loss of coverage. The gain is readability rather than time. The five separate `*_no_movement` / `no_delta` tests are borderline; merging them into one "a zero delta changes nothing" table would remove four more, at some cost to failure locality.

**F2 (low): `main.rs:920` writes into a fixed shared path.**
`std::env::temp_dir().join("sunlit_earth_test_texture_pointers")` and a 64 KiB write. If the test panics before line 939 the directory is left behind, and two concurrent runs of the binary collide. This is the workspace convention (`sunlit-core/src/config.rs` and `assets/cloud_fetcher.rs` do the same in a dozen places), so it is a workspace-level question rather than this file's, and not worth changing here alone.

**F3 (low): `about.rs:684` calls `i_slint_backend_testing::init_no_event_loop()` with no guard.**
`tests/slint_ui.rs:26` uses a thread-local flag for exactly this, because `set_platform` panics on a second call. `about.rs` is the only unit test in the lib target that inits the backend, so it works today; the second one added will panic. Either move this test into `tests/slint_ui.rs` (where the guard is) or copy the guard.

**F4 (low): `session_end.rs:479` permanently occupies a process-wide singleton.**
`the_listener_answers_windows_and_shuts_down_once` calls `install(None, ...)`, which fills the `HANDLER` `OnceLock` for the lifetime of the test binary and leaves a window and a thread running. A second test of the installed listener can never be written. Fine as it stands, worth knowing.

**F5: `ui_callbacks.rs` has no tests of its own, and about half of it is untested anywhere.**
`tests/slint_ui.rs` covers the translation points well: `read_params_from_window` and `apply_params_to_window` round-trip at lines 88, 98, 147 and 202; `read_config_from_window_onto` at 559, 573, 628, 641, 672, 750; `ComboIndices::of` at 822. What is not covered is the four `register_*` functions and `register_globe_drag`, `ui_callbacks.rs:19-347`, 330 lines. `tests/slint_ui.rs` never mentions `register_` at all: `test_load_defaults_fires_callback` and its siblings install their own closures and assert the button fires them, which tests `main.slint`, not `ui_callbacks.rs`. The e2e suite exercises them by running the binary but asserts only on `set-wallpaper` and `displays`. So the `on_load_defaults` and `on_reset` bodies (the 25 duplicated lines of D2) have no test at all, in a crate where `test_save_preserves_settings_without_a_widget` exists because a save path silently lost a setting once. Adding `register_action_callbacks(&window, &link, &screens)` followed by `window.invoke_load_defaults()` in `slint_ui.rs` is straightforward with the no-event-loop backend as long as the assertions avoid `defer_combobox_indices`, which needs an event loop; `displays::replace_monitors` already returns its row for exactly that reason and shows the pattern.

**F6: a note for whoever reviews `tests/slint_ui.rs`.**
Four test names read as pinning defaults, which `CLAUDE.md` forbids ("changing a preset or a default must not break a test"): `test_default_star_tuning_uses_balanced_profile`, `test_default_sun_shows_the_glare_and_a_trace_of_the_camera`, `test_default_moon_is_enlarged_two_and_a_half_times`, `test_default_milky_way_is_on_at_a_fifth_of_full_strength`. I read only the names, not the bodies, so this is an inference and not a finding. `test_default_horizon_matches_the_config` is a different thing and legitimate: it checks that the `.slint` defaults and `AppConfig::default()` agree, which is a consistency check rather than a constant.

### G. Best practices with a real effect

**G1 (low): `main.rs:620` takes `cli: Cli` by value and immediately clones out of it.**
`let ipc_socket = cli.ipc_socket.clone();` at 630, then `&cli` everywhere after. That is why `#[allow(clippy::needless_pass_by_value)]` is on the function. `let Cli { ipc_socket, mode, tray_start, .. } = &cli;` or moving the field out would let the allow go. Cosmetic, but the allow is the kind of thing that hides a later real one.

**G2 (low): `main.rs:729`, `let _instance_guard = instance_guard;`.**
The parameter is already owned by the function and dropped at the same point, so the rebinding does nothing. The doc at 616-618 explains the intent; the line does not implement it. Delete the line and keep the doc.

**G3 (low): `main.rs:878`, `let (output, width, height) = (output.clone(), *width, *height);`.**
A borrow-checker dance so `&cli` can be passed on the next line. Taking `Commands` by value out of `cli.command` (with `cli.command.take()` after making the field an `Option` the code already treats as one) would avoid the clone of a `PathBuf`. Trivial cost; mentioned only because it is the kind of line that puzzles a reader.

**G4 (low): the twelve-times-repeated callback prologue in `ui_callbacks.rs`.**
`let window_weak = window.as_weak(); let engine = link.clone(); window.on_X(move || { let Some(win) = window_weak.upgrade() else { return; }; ... });` appears twelve times across `register_mouse_callbacks`, `register_change_callbacks`, `register_action_callbacks` and `register_display_callbacks`, at three lines of boilerplate each. A small declarative macro would cut about 36 lines. I am ambivalent: it also costs IDE go-to-definition on the generated `on_*` names, and the current shape is at least obvious. Worth doing only if the file is being restructured anyway.

**G5: the `SceneParams` plumbing, since it was asked about directly.**
`apply_params_to_window` (438-502) is 60 `window.set_*` lines and `read_params_from_window` (510-575) is 58 `window.get_*` lines, against 46 fields on `SceneParams` and 58 `in-out property <float>` declarations in `main.slint`. Per new shader parameter, the cost inside this crate is four edits: one `.slint` property, one `SettingRow` block of about 10 lines, one line in `apply_params_to_window`, one line in `read_params_from_window` (plus, outside this crate, `AppConfig`, `SceneParams`, `ParamsDigest`, `Uniforms` and the WGSL).

A macro could collapse the two functions. `slint::include_modules!()` generates `get_sun_glow` / `set_sun_glow` from `sun-glow`, so a `macro_rules! bridge { ($($field:ident),*) }` emitting both functions from one list of 46 names would replace 118 lines with about 50 and make the two lists structurally impossible to disagree. I do not recommend it. The two functions are the crate's stated single translation point; a macro makes them the one place a reader cannot grep for `set_sun_glow` and find, and `rust-analyzer` will not offer the generated names. The four fields that are not a plain pass-through (`day_gamma` and `night_gamma` go through `gamma_value_to_slider`, `custom_day_of_year` through an `f32::from`, `custom_year` through a clamp against `datetime::base_year()`) would each need an escape hatch in the macro, which is where this kind of macro usually stops paying. Two flat, greppable, boring lists are the right answer here. The table-driven test in `params.rs` that fails when a parameter does not change the digest is what actually catches the omission a macro would prevent, and it already exists.

## 4. Recommended refactors, by value over effort

| # | Change | Effort | Risk | Why |
|---|---|---|---|---|
| 1 | Stop saving the config on every auto-refresh slider tick (E1) | 0.5 h | low | A file write per slider pixel is the only real performance defect I found in the crate |
| 2 | Delete `main.rs:436-449`, `main.rs:782-786`, `main.rs:942-948`, `session_end.rs:320-333`, and trim `session_end.rs:1-35` (A1-A4) | 1 h | none | Removes about 90 lines that `docs/` already carries, and one plan reference the maintainer's rules forbid |
| 3 | Extract `apply_whole_config` from the two duplicated callback bodies (D2) | 0.5 h | low | 25 duplicated lines in the crate's least-tested function |
| 4 | Fold the 13 log directives into a const array (E5) | 0.25 h | none | 12 fewer `expect`s, 8 fewer lines |
| 5 | `impl From<&SceneParams> for layout::Framing` in core, use it at both sites (D1) | 0.5 h | low | Cross-crate drift that would make `displays` print a plan the engine does not build |
| 6 | Push the gamma display value from Rust instead of re-deriving it in `main.slint` (D3) | 0.5 h | low | Same class of silent drift; the `zoom-display-distance` pattern already exists |
| 7 | Return `Result` from `spawn_ipc_listener` and `acquire_single_instance`; handle the three `expect`s at `ipc.rs:42/47` and `tray.rs:47` (E2) | 1.5 h | low | Panics reachable from a documented CLI flag |
| 8 | Merge the 12 redundant `mouse_math` tests and deduplicate `wrap_angle_180` (F1, D7) | 1 h | low | 48 tests to 36 with no coverage lost; the runtime saving is nil, the readability gain is the point |
| 9 | Split `main.rs` into `cli` / `logging` / `startup` / `headless` / `app` in the library, and break `run_app` into five helpers (B1) | 3 h | medium | 969 lines to about 110, `run_app` 200 to about 70, and `engine_config` / `resolve_texture_paths` become reachable from `tests/` |
| 10 | Test `register_action_callbacks` in `slint_ui.rs` (F5) | 2 h | low | 330 lines of `ui_callbacks.rs` are untested, including a save path that has silently lost a setting before |
| 11 | Move `Splitter` / `Hint` / `Setting*` / `MonitorTile` to `ui/widgets.slint` and `TrayIcon` to `ui/tray.slint` (B5) | 1 h | low | 1644 to about 1400; the Advanced section should stay where it is and the reason should go in `docs/architecture.md` |
| 12 | Narrow the cast allows, delete the dead one at `ui_callbacks.rs:607`, add the two cast helpers, switch to `#[expect]` (C3) | 1.5 h | low | The dead allow is exactly what `#[expect]` catches |
| 13 | Make the module-private `pub` items private, delete `DragSpeed::speed` (C5) | 0.5 h | low | Lets `dead_code` do its job in future |
| 14 | Handle `PlatformError` cleanly at `main.rs:632/764/798` (E4) | 0.5 h | low | Turns three startup crashes into three messages |
| 15 | Fix `zeroed<T>`, bound the IPC line read, delete the dead lines at `main.rs:389` and `main.rs:729` (E9, E6, D6, G2) | 0.75 h | low | Small, independent, each verified |
