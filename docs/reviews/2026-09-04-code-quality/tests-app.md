<!-- Reviewer notes for docs/reviews/2026-09-04-code-quality-review.md. Line numbers refer to commit 3046327. "The brief" is the shared review instruction; "the maintainer's rules" are the project's comment conventions. Runtime claims here are reasoned from the code; the measured figures are in test-timing.md. -->

# Review: the test suites of `crates/sunlit-app`

Area: `crates/sunlit-app/tests/slint_ui.rs`, `crates/sunlit-app/tests/e2e.rs`, `crates/sunlit-app/tests/fixtures/`.
Nothing was modified, no cargo command was run.

## 1. Summary

`slint_ui.rs` is fast and correct but over-populated: 45 tests, 44 of which build a fresh `MainWindow`, and about 15 of them assert something another test in the same file already asserts. Five tests exercise Slint's own generated getters and setters and say nothing about this product. Four pin literal defaults out of `main.slint`, which is the one convention `docs/testing.md` states outright ("changing a preset or a default must not break a test"); a fifth, `test_default_horizon_matches_the_config`, shows the right way to do it and should be the template for the other four.

`e2e.rs` is the file worth the effort. Of its 2862 lines, 1439 are the 15 test bodies (avg 95 lines, of which 17% are comments), 985 are 45 free helper functions, and 438 are imports, structs and banner comments. The plumbing is only half-shared: `spawn_for_ipc` exists and 4 cases use it, while 8 cases repeat the same 30-line spawn-and-watch block inline. Nine cases repeat the same "no ERROR in stderr" loop and ten repeat the same quit-and-assert tail. Timeout values for identical waits differ without a reason (10 s vs 15 s for the same shutdown, 30 s vs 60 s for the same readiness). The `SIGNAL:memory` contract is written twice, producer in `ipc.rs` and parser in `e2e.rs`, with the field names as string literals on both sides.

Comment-wise, `e2e.rs` carries roughly 200 lines of narrative that the maintainer's own rules put in commit messages: what a bug used to be, what a measurement was on a given host on a given date, what a case was doing when it started failing. One 26-line doc comment is attached to the wrong function.

## 2. Metrics

| | `slint_ui.rs` | `e2e.rs` | `fixtures/e2e_config.toml` |
|---|---|---|---|
| Total lines | 1165 | 2862 | 16 |
| Tests | 45 | 15 (14 on Linux, 15 on Windows) | n/a |
| Test-body lines | 1165 (whole file is tests) | 1439 (avg 95) | n/a |
| Helper / infrastructure lines | ~120 | 985 free fns + 438 types, imports, banners | n/a |
| Comment lines | 167 (14%) | 544 (19%); 254 inside test bodies | 6 (37%) |
| Comment blocks > 3 lines | 11 | 44 | 1 |
| ... classified keep / move / delete | 6 / 3 / 2 | 17 / 19 / 8 | 1 / 0 / 0 |
| Functions over 80 lines | 0 | 7 (see §4B) | n/a |
| `#[allow(clippy::too_many_lines)]` | 0 | 1 (e2e.rs:1630) | n/a |
| `#[serial]` | none | all 15 | n/a |
| GPU / engine per test | none | one real process per test | n/a |
| `pub` items used only in-crate | n/a (integration target, nothing `pub`) | n/a | n/a |

Answers to the direct questions, up front:

- **`slint_ui.rs` creates a window alone.** 44 `create_window()` calls plus one `AboutWindow::new()`, one per test. No engine, no `EngineLink`, no wgpu device, no GPU: grepping the file for `engine`, `wgpu` or `gpu` finds only prose. `i_slint_backend_testing::init_no_event_loop()` installs a `TestingBackend` with `mock_time: true, threading: false` and no renderer.
- **No `#[serial]` in `slint_ui.rs`**, and none is needed: Slint's `set_platform` is per thread, which is why `init()` (slint_ui.rs:26) guards with a `thread_local!` flag rather than a `Once`. Every test in `e2e.rs` is `#[serial]` and `cargo e2e` adds `--test-threads=1` on top.
- **No per-slider test explosion.** The 23 celestial parameters are covered by two mega-tests (slint_ui.rs:122 and :174), one per direction. The explosion is elsewhere: 3 tests for one preset button binding, 3 for one `advanced-open` gate, 2 for one `atmo-enabled` gate, 5 for Slint's own property accessors.
- **`ui_callbacks.rs` has no unit tests at all** (0 `#[test]` in 674 lines), so `slint_ui.rs` duplicates nothing; it is the only coverage that code has. Almost all of it takes `&MainWindow`, so it cannot be tested without a window. The one exception is `ComboIndices::of` (ui_callbacks.rs:367), which takes `&AppConfig`, `&[u32]`, `u32`, `&[String]` and is pure. The `SceneParams` to `AppConfig` half is already unit-tested in core (`crates/sunlit-core/src/params.rs:488-533`), so the celestial tests are not duplicating that either.
- **The `SIGNAL:memory` contract is written twice.** Produced at `crates/sunlit-app/src/ipc.rs:149-152` as an inline `format!("memory rss_bytes={} peak_rss_bytes={} private_bytes={}", ...)`; parsed at `crates/sunlit-app/tests/e2e.rs:706-724` by `parse_memory_field`, which splits on whitespace and strips a `key=` prefix. The parser is generic, so the shared thing is three field-name string literals, spelled once on each side with nothing tying them together. The `SIGNAL:displays` line is the same shape: produced by `crates/sunlit-app/src/displays.rs:360-383`, parsed by `displays_field` at e2e.rs:2238.

## 3. Timing

Filled in from the timed run (test-timing.md):

- `tests/slint_ui.rs`: **0.61 s for all 45 tests**, 0 tests over 1 s, 1 test over 0.1 s (`test_the_adapter_line_never_wraps`, 0.139 s, which is the only test that calls `widest_panel_state` plus two `materialize` passes).
- `tests/e2e.rs`: 15 ignored, as designed. It never runs in `cargo test`.
- `sunlit-app` unit tests: 0.03 s.

**Runtime is not a problem in this area.** A `MainWindow` costs about 13 ms to build and query, and deleting a third of the tests would buy back roughly 0.2 s. So none of the recommendations below are justified by speed; they are justified by the file being easier to read and harder to fool. The rest of this report is weighted to structure, redundancy and the e2e harness accordingly.

The e2e suite's own budget is worth stating even though it is not a `cargo test` cost. Worst case, defined as every wait consuming its full timeout and then succeeding (a wait that actually times out panics and ends that case early):

| Case | Worst case |
|---|---|
| `test_binary_exists` | ~0 s |
| `test_render_and_exit` | 60 s |
| `test_tray_mode_ipc_lifecycle` | 120 s |
| `test_session_end_shuts_down_promptly` (Windows only) | 85 s |
| `test_windowed_mode_graceful_shutdown` | 70 s |
| `test_single_instance_second_exits` | 50 s |
| `test_tray_hide_show_cycle` | 90 s |
| `test_gpu_persistence_after_hide` | 110 s |
| `test_hidden_window_cloud_updates_do_not_grow_memory` | 586 s (incl. 16 s of unconditional `sleep`) |
| `test_memory_report` | 165 s |
| `test_set_wallpaper` | 315 s |
| `test_displays_reports_the_session_layout` | 90 s |
| `test_across_screens_writes_what_this_desktop_can_hold` | 195 s |
| `test_a_layout_change_republishes_the_wallpaper` | 675 s |
| `test_plasmashell_survives_rapid_republishing` | 675 s |
| **Linux total** (no `session_end`) | **3201 s = 53.4 min** |
| **Windows total** (layout-change and plasmashell skip early) | **1936 s = 32.3 min** |

`crates/xtask/src/commands/e2e.rs:48` gives the guest job 30 minutes on Linux and 45 on Windows. So on Linux the suite's own budget exceeds the orchestrator's by 23 minutes: a pathologically slow guest is killed by the job timeout, and the failure message names the job rather than the signal that never arrived. See finding E-3.

## 4. Findings

### A. Commentary

`slint_ui.rs`: 11 blocks over three lines, 6 keep, 3 move, 2 delete. `e2e.rs`: 44 blocks over three lines, 17 keep, 19 move, 8 delete. The counts are of blocks longer than three lines only; `e2e.rs` additionally has 55 numbered step comments ("// 1.", "// 2.") which are individually short and collectively the largest single category.

**A-1 (medium) e2e.rs:1864-1892: a 26-line doc comment is attached to the wrong function.** The block beginning "Ask this desktop what its wallpaper is, and check the answer is ours" runs to line 1889 and is immediately followed, with no blank line, by "The placement the app would have made for this session" at 1890 and then `fn placement_of` at 1894. Rustdoc joins the two, so `placement_of` is documented as if it were `assert_the_desktop_holds_the_wallpaper` (line 1916), which itself has no doc comment at all. Fix: insert a blank line at 1890 and move the first block down to 1915. Classification of the block's content once relocated: paragraph 1 keep, paragraphs 2 to 5 move (the XFCE incident is history).

**A-2 (medium) e2e.rs, 55 numbered step comments: delete.** Examples: `// 1. Create a temp directory and output path` (963), `// 2. Spawn the binary with the render subcommand` (980), `// 4. Assert exit code is 0` (1015), `// 5. Assert the render output file exists and is non-empty` (1023), `// 6. Decode the PNG with the image crate` (1034), `// 7. Assert image dimensions match the requested size` (1037), `// 3. Send quit via IPC` (1391), `// 5. B should have exited with code 0` (1469), `// 6. No errors in log` (1612). Every one restates the line beneath it, which is the maintainer's third rule verbatim. They also carry a maintenance cost the file has already paid: the numbering in `test_hidden_window_cloud_updates` runs 1 to 10 across 149 lines and has to be renumbered whenever a step is inserted. Delete all 55; keep the handful that state a reason rather than an action (1182-1185 on the deferred-hide race, 1716-1719 on why the cursor is taken before the show, 1752-1753 on why private bytes rather than RSS).

**A-3 (medium) e2e.rs:968-977 and 1004-1012: move.** Two blocks totalling 19 lines inside `test_render_and_exit` that record measurements and history: "measured in the Linux guest on 2026-09-01, the same render peaked at 2088 MB and settled to 444 MB with a cold cache and sat flat at 381 MB with a warm one", and "What used to guarantee the cold cache was the order the suite happens to run in ... until a new case sorted before it and the assertion started failing on an app that had not changed", then "Timed on this host on 2026-09-01, debug build, empty cache ... 5.9 s on the GPU and 8.0 s on the software adapter, against 2.5 s warm". The invariant worth keeping is one line: "a cache directory of its own, so this case pays the decode rather than inheriting a warm cache from whichever case ran first". The numbers and the dates belong in `docs/testing.md` beside the soak measurements that are already there.

**A-4 (medium) e2e.rs:1116-1127: move, 12 lines.** Explains that `rss <= peak` holds by construction, which is worth one sentence, then narrates "which is what the case was doing when it started failing". Trim to the first three sentences.

**A-5 (low) e2e.rs:1618-1626, 2768-2784: move.** Both open with the defect the case was written for ("This is the regression test for the tray-mode memory leak: decoded cloud frames used to accumulate in an unbounded channel"; "The file-lifecycle defect this branch fixes is Plasma's alone ... on a distro that ships the plugin with assertions live the violated one takes the shell down"). The mechanism half is a keep, because a reader cannot otherwise tell why the case publishes five times with no pause. The "this branch fixes" and "used to" halves are commit-message material. `docs/platforms.md` already carries the plasmashell story in full, so this is also a duplicate.

**A-6 (low) e2e.rs:366-380: trim, 15 lines on two constants.** The first half states a real invariant ("a patch and not a pixel, because the sky is drawn there ... what the case actually means is that the corner is mostly empty"), which is a keep. The middle sentence, "the answer moved to within four pixels of the top-left corner the moment stars were turned on by default", is history. Cut to about six lines.

**A-7 (keep, do not touch) e2e.rs:57-76, 121-134, 143-154, 800-805, 2523-2527.** `tray_supported`'s 20 lines explain a probe whose failure direction is a deliberate safety choice and which way the three groups of cases split on it; that cannot be recovered from the code. `wallpaper_supported` and `WALLPAPER_OPT_IN` explain why one answer is a session property and the other a build property, which is the whole reason there are two. `serve_cloud_request`'s block states the hang the read timeout prevents. `a_different_mode` explains that it parses the block `display::parse_outputs` deliberately skips. All earn their place.

**A-8 (low) slint_ui.rs:912-922: trim, 11 lines.** The first paragraph of `test_a_layout_that_changed_rebuilds_the_displays_group` restates the assertions below it in prose. The second paragraph ("the anchor row is the return value rather than a property read, because setting it goes through `defer_combobox_indices`, which needs an event loop this backend deliberately does not have") is the only part a reader cannot derive, and is a clear keep.

**A-9 (low) slint_ui.rs:322-325, 368-370: delete.** "Register a callback that applies the Europe preset values (PRESETS[0]). These values are read from scene/camera.rs: the test verifies that clicking the button fires the callback which sets properties, not that any particular constant has a specific value." This is the test defending itself against a convention it does not actually satisfy (see F-4); the comment is the tell. Same for "mimics main.rs behavior. We read AppConfig::default() to get the canonical defaults so this test does not hard-code constants" at 368-370.

**A-10 (keep) `fixtures/e2e_config.toml:11-16`.** Six lines saying why `milky_way_intensity` and `cloud_opacity` are zero. A reader deleting either would break `test_render_and_exit`'s corner sampling with no clue why. Keep; it could lose its last sentence.

### B. Length and structure

**B-1 (medium) e2e.rs at 2862 lines: split along the seam that already exists.** The file has three distinct layers that are currently interleaved, and the last of them is already Linux-only:

| New module | What moves | Lines |
|---|---|---|
| `tests/common/process.rs` | `binary`, `fixture`, `skip_case`, `ChildGuard`, `wait_with_timeout`, `create_temp_dir`, `cleanup_temp_dir`, `TempDirGuard`, `isolated_state_dir`, `isolated_config_path`, `SOCKET_COUNTER`, `unique_socket_name`, `send_ipc_command`, `StderrWatcher`, `StdoutWatcher`, `MemoryQuery`, `parse_memory_field`, `query_memory`, `mib`, `memory_report`, `spawn_for_ipc` | 26-58, 150-330, 483-751, 2246-2270 (~480) |
| `tests/common/pixels.rs` | `rgb_at`, `CORNER_PATCH`, `CORNER_BLACK_FRACTION`, `assert_space_corner`, `assert_night_ocean`, `assert_night_land`, `assert_greenish`, `assert_yellowish`, `assert_blue`, `assert_ice`, `parse_memory_entries` | 314-481 (~170) |
| `tests/common/cloud_stub.rs` | `StubState`, `spawn_cloud_stub`, `serve_cloud_request`, `cloud_fixture_jpeg`, `wait_for_downloads` | 753-902 (~150) |
| `tests/common/desktop_linux.rs` | `placement_of`, `assert_the_desktop_holds_the_wallpaper`, `readback_of`, `read_setting`, `xrandr`, `words`, `LayoutGuard`, `LayoutChange`, `a_different_mode`, `a_layout_change_this_session_can_make`, `plasmashell_pid`, `plasmashell_answers`, `tray_supported`, `status_notifier_watcher_present`, `wallpaper_supported`, `lifecycle_mode_args` | 57-148, 1890-2071, 2468-2598, 2720-2766 (~450) |

That leaves the 15 cases plus `displays_field`, `find_session_listener` and `assert_the_files_match_the_layout` in `e2e.rs`, about 1500 lines, which is still long but is one thing. Worth doing: integration targets support `tests/common/` submodules with no Cargo change, and the four groups have no coupling to each other.

**B-2 (low) seven functions over 80 lines in `e2e.rs`.** `test_render_and_exit` (962-1143, 182), `test_set_wallpaper` (2090-2235, 146), `test_hidden_window_cloud_updates_do_not_grow_memory` (1631-1779, 149), `test_across_screens_writes_what_this_desktop_can_hold` (2351-2466, 116), `test_a_layout_change_republishes_the_wallpaper` (2615-2718, 104), `assert_the_desktop_holds_the_wallpaper` (1916-2017, 102), `test_tray_mode_ipc_lifecycle` (1148-1230, 83). Only the third carries `#[allow(clippy::too_many_lines)]`, because clippy's counter ignores comments and the other four are padded past 100 raw lines by comment blocks alone. Applying D-1 through D-3 takes the four spawn-heavy cases under 80 without splitting anything; `test_render_and_exit`'s pixel section (1046-1071) and memory section (1085-1142) are two natural sub-assertions (`assert_the_globe_is_where_the_config_put_it`, `assert_the_memory_profile_settled`) that would leave the body around 60 lines. `assert_the_desktop_holds_the_wallpaper` is one procedure and should stay whole.

**B-3 (low) `slint_ui.rs` at 1165 lines is fine as one file.** No function exceeds 80 lines; the banner comments (`// --- Property round-trip tests (Step 2.2) ---`) already section it. The section banners do carry stale plan references ("Step 2.1", "Step 2.3", "Step 2.6") that mean nothing now; drop the step numbers, keep the banners.

### C. Consistency

**C-1 (medium) e2e.rs: the same wait gets different budgets for no stated reason.** Readiness (`ipc_listener_ready` plus `first_frame_rendered`) is 30 s at 1186, 1303, 1387, 1450, 1526, 1588, 2158 and 2266 but 60 s at 1686 and 1815. Graceful shutdown after `quit` is 10 s at 1202, 1393, 1467, 1485, 1541 and 1604 but 15 s at 1724, 1828, 2220, 2333, 2460, 2712 and 2856. `window_hidden` is 10 s at 1198, 1532 and 1594 but 15 s at 1694. A publish is 2 min everywhere (2176, 2382, 2617, 2834), which is the one budget that is consistent. Recommendation: four named constants at the top of the file, `READY`, `SIGNAL_REPLY`, `PUBLISH`, `SHUTDOWN`, each with one line saying what it is sized for, and no bare `Duration::from_secs` in a test body.

**C-2 (low) e2e.rs: the stderr watcher is bound to two different names.** `let watcher = StderrWatcher::new(child)` at 1180, 1301 and 1384; `let stderr_watcher = ...` at 1523, 1585, 1682, 1813 and 2156. In the first three, `watcher` sits next to `stdout_watcher`, so the reader has to know which is which by elimination. Rename all to `stderr`/`stdout` or `stderr_watcher`/`stdout_watcher`.

**C-3 (low) e2e.rs: `ChildGuard`'s private field is read directly in nine places.** `guard.child.as_mut().unwrap()` at 1177, 1298, 1381, 1444, 1520, 1582, 1680, 1811, 2154 and 2264, while the struct offers `take()` as its API and its doc comment describes the guard as owning the child. Add `fn child_mut(&mut self) -> &mut Child` and use it; the `unwrap()` then lives in one place instead of ten.

**C-4 (low) e2e.rs:947-957 `test_binary_exists` is gated for reasons that do not apply to it.** It carries `#[ignore = "requires desktop environment and GPU"]` and `#[serial]` while doing nothing but `path.exists()`. It needs neither a desktop nor a GPU nor exclusivity, and every other case fails with a clearer message when the binary is missing (`.expect("failed to spawn sunlit-earth binary")`). Either drop the `#[ignore]` so it runs in CI as free coverage that the binary got built, or delete it.

**C-5 (low) `SOCKET_COUNTER` serves two purposes.** Declared at e2e.rs:488 as "monotonically increasing counter for unique socket names" and then also consumed by `isolated_config_path` at line 303. Harmless, but the name is now wrong. Rename to `UNIQUE_ID`.

**C-6 (low) slint_ui.rs mixes two float-comparison styles.** `approx::assert_relative_eq!` almost everywhere, which is the documented convention, but hand-rolled epsilon comparisons at 390-393 (`(window.get_camera_longitude() - 123.0).abs() < 0.01`), 639, 683 and 684 (`(saved.longitude - 42.0).abs() < f32::EPSILON`). Use the macro.

### D. Duplication

**D-1 (high) e2e.rs: the spawn-and-watch block is written nine times.** `spawn_for_ipc` at 2246-2270 does exactly this and is used by four cases (2284, 2378, 2646, 2827). The other eight spawn inline with the same shape: 1159-1194, 1280-1305, 1363-1389, 1433-1451, 1507-1528, 1569-1590, 1654-1688, 1798-1817, 2141-2160. Between them these nine blocks are about 230 lines that differ only in the argument list and in whether readiness means `first_frame_rendered` or `window_hidden_deferred`. The one thing stopping them from calling `spawn_for_ipc` today is that it returns only a `StdoutWatcher` and they all need stderr as well. Recommendation: change `spawn_for_ipc` to take the extra args and return `(ChildGuard, StdoutWatcher, StderrWatcher)`, with a variant or a flag for the hidden-start readiness signal. This alone removes roughly 180 lines and takes four of the seven long functions under 80.

**D-2 (medium) e2e.rs: the "no ERROR in stderr" loop is written nine times** at 1075-1077, 1227-1229, 1351-1353, 1413-1415, 1551-1553, 1613-1615, 1776-1778, 1859-1861 and 2232-2234, identical in all nine. One `fn assert_no_error_lines(lines: &[String])`.

**D-3 (medium) e2e.rs: the quit-and-assert tail is written twelve times** at 1201-1209, 1392-1400, 1484-1492, 1540-1548, 1603-1610, 1723-1724 (plus the assert at 1770-1774), 1827-1828 (assert at 1854-1858), 2219-2225, 2332-2338, 2459-2465, 2711-2717, 2855-2861. Ten of the twelve are the same three statements verbatim. One `fn quit_and_expect_clean_exit(socket: &str, guard: &mut ChildGuard, stderr: &StderrWatcher)` folds D-2 into it and removes about 90 more lines. The two exceptions (the cloud-memory case, which deliberately quits before asserting so a failing run still yields a full log, and `test_session_end`, which times the exit) keep their own tails and should say so in one line each.

**D-4 (medium) the `SIGNAL:` line formats are duplicated across the crate boundary.** `ipc.rs:149-152` builds `memory rss_bytes=... peak_rss_bytes=... private_bytes=...` from an inline `format!`; `e2e.rs:706-724` reads those three names back as string literals. `displays.rs:360-383` builds six `key=value` pairs; `e2e.rs:2238-2244` reads four of them back as string literals. Nothing links the two sides, so renaming a field in the producer produces a panic in a guest half an hour later rather than a compile error. `displays.rs` is `pub` and already in the app's library, so the cheap fix is to put the field names in one `pub const` array or, better, a small `pub fn parse(line: &str) -> Option<...>` next to each producer that the test calls; that also gives the producer a unit test it does not have today. The `memory` line has no home in the library yet and would need one.

**D-5 (low) slint_ui.rs: the load-defaults handler is written twice**, identically, at 371-386 and 414-429 (nine `set_*` calls each). See F-3: the two tests should be one.

**D-6 (low) slint_ui.rs:1057-1071 duplicates a constant from the UI.** `const WINDOW_MIN_WIDTH: u32 = 520` (994) restates `min-width: 520px` at `crates/sunlit-app/ui/main.slint:209`. If someone raises the `.slint` value the test keeps checking 520 and passes for the wrong reason, which is precisely the failure the test's own doc comment says it exists to prevent. Expose the root's minimum as an `out property` the way `panel-min-width` (main.slint:332) already is, and read it.

### E. Correctness and resilience

**E-1 (medium, plausible, unverified) e2e.rs:981-1013: `test_render_and_exit` pipes both streams and drains neither until the child exits.** Every other case attaches a `StdoutWatcher` and a `StderrWatcher` immediately, and the `StdoutWatcher` doc comment at 594-598 gives the reason ("to avoid pipe buffer congestion"). This case runs the child with `--log-level debug` for up to 60 seconds with `Stdio::piped()` on both and reads them only inside `wait_with_timeout` after `try_wait` reports exit. If the debug log for an 800x800 render with a cold texture cache exceeds the pipe buffer, the child blocks on write and the case fails as a 60-second timeout that names nothing. I did not measure the log volume or the platform buffer size, so this is a hazard rather than a confirmed bug; the fix is one line, attaching the two watchers the rest of the file already uses.

**E-2 (low, confirmed) e2e.rs:291-305: `isolated_state_dir()` is never cleaned up.** It is a fixed path, `temp_dir()/sunlit_earth_e2e_state`, and every spawned binary writes a `config_<pid>_<n>.toml` into it (299-305) and points `SUNLIT_EARTH_METRICS_DIR` at it (984, 1163, 1284, and so on). `TempDirGuard` cleans the per-case directories; nothing ever removes this one. On a developer desktop it accumulates one config file per spawned process per `cargo e2e` run, forever, plus the metrics CSV rows the directory exists to keep out of the real one. Give it a process-scoped subdirectory and a guard, or sweep it at the start of the run.

**E-3 (low, confirmed) the suite's own timeout budget exceeds the guest job timeout on Linux.** 53.4 minutes of internal budget against `job_timeout(Target::Linux) = 30 min` at `crates/xtask/src/commands/e2e.rs:50`. The practical consequence is diagnostic rather than functional: a guest slow enough to spend more than half its per-wait budgets is killed by the orchestrator, and the operator gets "the job timed out" instead of "timed out after 120s waiting for SIGNAL:wallpaper_set", which the per-wait panic would have printed along with every stdout line collected. Either raise the Linux job timeout above the suite's budget or lower the two 675-second cases; note that `xtask`'s own comment at e2e.rs:44 claims "the desktop run is well under a minute", which looks stale given that `test_hidden_window_cloud_updates` alone sleeps 16 s unconditionally and then waits on 15 downloads at a 1 s poll interval.

**E-4 (low, confirmed) e2e.rs:775-798: the cloud stub thread and its listener outlive the case that owns them.** `spawn_cloud_stub` spawns a detached thread with `for stream in listener.incoming()` and returns no handle and no shutdown. After `test_hidden_window_cloud_updates_do_not_grow_memory` finishes, the thread and the bound port stay alive for the rest of the test process. Nothing connects to it afterwards, so the effect is one leaked thread and one held port for the remaining cases. Worth a `TcpListener` dropped through an `Arc<AtomicBool>` or simply a note; it is not a bug today.

**E-5 (low, confirmed) e2e.rs:1467: instance B's pipes are also drained only after exit.** Same shape as E-1, but bounded: instance B logs the single-instance message and exits at once, so the exposure is small.

### F. The tests themselves

Removals and merges I would make, by name. 45 tests to about 29.

**F-1 (medium) slint_ui.rs:62-119: five tests assert Slint's generated accessors, not this product.** `test_camera_longitude_roundtrip`, `test_camera_latitude_roundtrip`, `test_camera_zoom_roundtrip`, `test_bool_property_roundtrip`, `test_int_property_roundtrip` each set a property and read it back with no product code in between. If any of them ever fails, Slint is broken. Delete all five; `test_camera_fov_roundtrips_through_scene_params` (85-100) shows what a property test with a point looks like, because it goes through `read_params_from_window` and `apply_params_to_window`. Net minus 5.

**F-2 (medium) slint_ui.rs: four tests pin literal defaults, against the file's own stated convention.** `test_default_star_tuning_uses_balanced_profile` (229-238, seven literals), `test_default_sun_shows_the_glare_and_a_trace_of_the_camera` (241-248, five), `test_default_moon_is_enlarged_two_and_a_half_times` (267-272, three), `test_default_milky_way_is_on_at_a_fifth_of_full_strength` (275-278, one). `docs/testing.md:32` says changing a default must not break a test, and the test names themselves encode the values ("two and a half times", "a fifth of full strength"), so a tuning change means editing four names and sixteen numbers. `test_default_horizon_matches_the_config` (254-264) does the right thing: it asserts the window and `AppConfig::default()` agree, which is the actual invariant (the doc comment at 250-252 states it exactly). Replace all four with one `test_the_window_and_the_config_start_from_the_same_defaults` that loops the same sixteen fields against `AppConfig::default()`. Net minus 3, and one convention violation gone.

**F-3 (medium) slint_ui.rs: three preset tests where one suffices.** `test_preset_europe_fires_callback` (285-299) and `test_preset_earthrise_fires_callback` (302-316) are strict subsets of `test_every_preset_button_fires_the_index_it_is_named_for` (1096-1125), which loops all nine labels and includes both `("Europe", 0)` and `("Earthrise", 8)`. `test_preset_changes_camera_properties` (319-358) is worse than redundant: it registers a handler that sets eight hardcoded values and then asserts four of them came back, so it verifies its own closure. It never reads `PRESETS`, so if `PRESETS[0]` changes the test still passes while its name and comment claim to be checking the Europe preset. Delete all three. Net minus 3.

**F-4 (low) slint_ui.rs: merge the two load-defaults tests.** `test_load_defaults_fires_callback` (365-408) and `test_load_defaults_resets_multiple_properties` (411-447) register the same nine-line handler and the second asserts a superset of the first. Both are the same self-referential shape as F-3 (the handler under test belongs to the test), but they do verify that the button exists, is unique, and invokes its callback, which is real. Keep one. Net minus 1.

**F-5 (low) slint_ui.rs: three visibility tests for one binding.** `test_advanced_section_starts_closed` (454-470), `test_advanced_section_opens` (473-484), `test_advanced_section_closes` (487-500) are one table: `[(None, absent), (Some(true), present), (Some(false), absent)]`. Same for `test_atmosphere_sliders_hidden_when_disabled` (507-521) and `test_atmosphere_sliders_visible_when_enabled` (524-541). Net minus 3.

**F-6 (low) slint_ui.rs: two Displays-group presence tests are covered by a third.** `test_the_displays_group_is_absent_on_a_single_screen` (885-895) and `test_the_displays_group_is_there_on_two_screens` (898-910) both assert what `test_a_layout_that_changed_rebuilds_the_displays_group` (924-966) already asserts at lines 953-955 and 963-965, and the latter additionally covers the transition in both directions, which is the interesting part. Net minus 2.

**F-7 (medium) slint_ui.rs:700-716 `test_the_sky_slider_stops_where_one_screen_stops` asserts on source text.** It reads `ui/main.slint` from disk with `CARGO_MANIFEST_DIR` (which the e2e file documents at lines 40-43 as a compile-time path that is wrong in a guest, though this suite never runs in one), string-splits it, and requires the literal substrings `"minimum: 60.0;"` and `"maximum: 180.0;"`. Reformatting the `.slint` file, or writing `60`, breaks it without changing behavior. The reasoning in the doc comment (692-698) is sound and worth keeping; the mechanism is not. Expose the slider's bounds as `out property`s, the way `panel-min-width` already is at main.slint:332, and assert on those. Same file, one property, no string surgery.

**F-8 (low) slint_ui.rs:660-686 `test_save_preserves_settings_without_a_widget` loops three quality tiers.** Nothing in `read_config_from_window_onto` branches on the tier, so the three iterations test one thing three times. Harmless at this speed; mentioned only because the same loop shape appears in `test_save_reads_the_texture_resolution_from_its_combo_box` (557-569) where it is per-value.

**F-9 (medium) e2e.rs: two cases could be one boot, and two more are questions that need no boot of their own.** `test_tray_hide_show_cycle` (1503-1554) is hide-then-show; `test_gpu_persistence_after_hide` (1565-1616) is hide-then-export. The second is the first plus one IPC command, minus the show. Adding the show back to `test_gpu_persistence_after_hide` and deleting `test_tray_hide_show_cycle` loses nothing and saves a full app boot. `test_memory_report` (1796-1862) and `test_displays_reports_the_session_layout` (2282-2339) each boot the app to ask one question and quit; both could ride on an existing booted instance. I would take the first merge and leave the other two alone: an e2e case that fails should point at one thing, and a boot is cheap relative to a two-minute publish. Net minus 1 case, minus one boot.

**F-10 (low) e2e.rs:1697 and 1709: 16 seconds of unconditional `sleep`.** `SETTLE = 8s` twice, sized for "at least one tick of the 5 s drain timer" (1637). This is the only unconditional sleep in the suite. Polling `query_memory` until two consecutive samples agree, with the 8 s as the ceiling rather than the cost, would return in about 5 s in the common case. Worth roughly 6 s of a run that already takes minutes, so this is bookkeeping, not a priority.

**F-11 (low) e2e.rs:1093-1115: `test_render_and_exit` pins three environment-dependent memory thresholds** (300 MB, 3000 MB, 1000 MB) whose provenance is a measurement on one host on one date (A-3). They are not defaults or presets, so they do not violate the stated convention, but they are the assertions most likely to fail on a machine nobody has run this on. The `if let Some(entry) = ... find(...)` guards mean the case silently skips two of the three where the log line is absent, which is the right shape; state the numbers as named constants with one line each on what they are protecting against.

### G. Best practices worth mentioning

**G-1 (low) e2e.rs:317: a regex is compiled per call.** `parse_memory_entries` builds `Regex::new(r"\x1b\[[0-9;]*m")` on every invocation and `.unwrap()`s it. Called once per run today, so the cost is nothing; a `static` `LazyLock` would remove both the recompile and the unwrap on a literal.

**G-2 (low) e2e.rs:577-582 and 645, 654: `lines()` and `lines_between` clone the whole collected buffer.** `StderrWatcher::lines()` clones a `Vec<String>` that grows for the process lifetime, and every "no ERROR" loop calls it (nine sites, two of them twice in the same test: 1212 then 1227, 1346 then 1351, 1402 then 1413). Folding D-2 into one helper that takes the lock once removes most of this. Not a hot path; a readability point.

**G-3 (low) slint_ui.rs:929, 946, 960, 976: `fabricated_monitors()` is rebuilt per call** and `both.clone()` is used to feed `replace_monitors` twice. Two `Monitor` structs with two `String`s each; no effect worth changing.

**G-4 (keep) slint_ui.rs:1001-1009 `materialize`.** The doc comment ("a layout counts an `if`-gated subtree towards its minimum width only once that repeater has been walked ... or the assertion passes for the wrong reason") is exactly the kind of non-obvious framework behavior a comment should carry, and the helper is the right abstraction over it.

## 5. Recommended refactors, by value over effort

| # | Refactor | Effort | Risk | Why |
|---|---|---|---|---|
| 1 | D-1: widen `spawn_for_ipc` to return both watchers and take the arg list; convert the eight inline spawn blocks | 1.5 h | low | Removes ~180 lines, takes four of seven long functions under 80, makes the timeout constants of C-1 land in one place |
| 2 | D-2 + D-3: `assert_no_error_lines` and `quit_and_expect_clean_exit` | 0.75 h | low | Another ~110 lines, and the two deliberate exceptions become visible instead of invisible |
| 3 | A-2: delete the 55 numbered step comments | 0.5 h | none | The single largest commentary category and the clearest rule violation |
| 4 | F-2: replace the four default-pinning tests with one config-agreement test | 0.5 h | low | Removes the only violation of the project's stated testing convention |
| 5 | F-1 + F-3 + F-4 + F-5 + F-6: delete or merge 14 redundant `slint_ui` tests | 1 h | low | 45 to 29 tests with no coverage lost; F-3's third test is currently vacuous |
| 6 | A-1: move the misattached doc comment and give `assert_the_desktop_holds_the_wallpaper` its own | 5 min | none | A documentation bug, trivially fixed |
| 7 | C-1: four named timeout constants | 0.5 h | low | Removes six unexplained 10-vs-15 and 30-vs-60 discrepancies |
| 8 | E-1: attach the two watchers in `test_render_and_exit` | 15 min | low | Closes the one place the file's own stated pipe-congestion rule is not followed |
| 9 | A-3 + A-4 + A-5: move the measurement and history narratives into `docs/testing.md` | 1 h | none | ~60 lines out of source, and the measurements become findable |
| 10 | B-1: split `e2e.rs` into `tests/common/{process,pixels,cloud_stub,desktop_linux}.rs` | 2 h | medium | 2862 to ~1500 lines; do it after 1 to 3, since those shrink what has to move |
| 11 | D-4: one parser per `SIGNAL:` line, shared with the producer | 1.5 h | medium | Turns a cross-crate string contract into a compile-time one and gives `displays::signal_line` its first test |
| 12 | F-7: expose the sky slider bounds as an `out property` and drop the source-text assertion | 0.5 h | low | Removes the only test that parses the UI source |
| 13 | E-2 + E-3 + E-4: scope the shared state dir, reconcile the Linux job timeout, shut the cloud stub down | 1 h | low | Housekeeping; E-3 is the one with a diagnostic payoff |
| 14 | F-9: fold `test_tray_hide_show_cycle` into `test_gpu_persistence_after_hide` | 20 min | low | One fewer app boot for identical coverage |

Items 1 through 8 are about 5 hours and take `e2e.rs` under 2500 lines and `slint_ui.rs` to 29 tests without touching what either suite proves.
