# Plan: Phase 0, Fix the Tray-Mode Memory Leak with a Regression Test

## Summary

Fix the unbounded memory growth in tray mode (7.3 GB RSS observed after 10 days) and prove the fix with a Windows e2e regression test that reproduces the leak in about a minute. The test is written first and must be red against the current code before the fix lands. Ships as 0.1.1. This is Phase 0 of the roadmap in `../retrospective-2026-08.md`, section 10.

## Stakes Classification

**Level**: Medium

**Rationale**: The fix replaces the texture message channel and adds a timer, touching `main.rs`, `renderer/mod.rs`, `renderer/textures.rs`, and `cloud_fetcher.rs`, but it does not change the rendering pipeline, shaders, or UI. The failure mode being fixed is well understood (see Root Cause), the new mailbox semantics are strictly narrower than the old queue semantics (only the latest frame per slot is ever useful), and the regression test plus the existing e2e suite guard the behavior.

## Research

- `../retrospective-2026-08.md` section 4.1 (evidence chain) and section 8.2 (verification procedure)
- `2026-03-30-fresh-astro-state-plan.md`: experimental confirmation that `BeforeRendering` stops firing when the window is hidden
- `2026-03-28-research-slint-tray-app-patterns.md`: confirmation that Slint timers keep firing while the window is hidden (the drain timer relies on this)
- Cloud source update cadence: eight times a day, every three hours (matteason/live-cloud-maps documentation)

## Root Cause (from code reading, runtime confirmation is Step 3)

1. The cloud fetcher decodes each 8192x4096 cloud JPEG to RGBA8 (about 134 MB) and sends it into a channel (`src/cloud_fetcher.rs:266-277`).
2. The channel is `std::sync::mpsc::channel`, unbounded (`src/main.rs:222`).
3. The only consumer is `process_decoded_textures`, called exclusively from `BeforeRendering` (`src/renderer/mod.rs:432`, `src/renderer/textures.rs:29`).
4. `BeforeRendering` does not fire while the window is hidden to the tray, so messages accumulate forever: up to 8 x 134 MB per day.

Three design mistakes compound: an unbounded producer paired with a conditional consumer, decoded pixels parked in a queue instead of compressed data, and consumption tied to window visibility in an app whose purpose is background operation.

## Fix Design

**Mailbox instead of queue.** A `TextureMailbox` (shared `Mutex<Vec<Option<DecodedTextureMessage>>>`, one slot per texture) replaces the mpsc channel. Producers `post()` with replacement semantics: a newer frame for the same slot overwrites the parked one. Worst case parked memory is one frame per slot, no matter how long the consumer stalls.

**Drain independent of visibility.** A repeated Slint timer on the event loop (period 5 s) calls a new `renderer::drain_texture_updates()`, which runs the same routine as today (create GPU texture, mips, bind group) whether or not the window is visible. Slint timers fire while hidden (confirmed in the March research), and `GPU_RESOURCES` lives on the same thread as the timer callback. This is a behavior fix too, not just a memory fix: cloud updates keep flowing to the GPU while hidden, so scheduled wallpaper exports get fresh clouds instead of the clouds from when the window was last visible.

**Payload stays decoded pixels.** With a prompt drain, a frame is parked for at most one timer period, so switching the payload to compressed bytes is not needed for the leak. It remains documented as optional hardening (retrospective section 10, Phase 0 item 3) for the one residual case: `--tray-start hidden` with the window never shown means `GPU_RESOURCES` is `None`, the drain no-ops, and up to one decoded frame per slot (about 134 MB for 8K clouds) stays parked. That is bounded, not a leak; accepted for Phase 0.

## Success Criteria

- [x] Regression test demonstrates the leak against unfixed code (red run recorded in Results below)
- [x] Regression test passes against fixed code: bounded memory across 15 cloud updates while hidden
- [x] Cloud updates are processed while the window is hidden (GPU texture creation observed between hide and show)
- [x] `cargo test` passes and `cargo clippy` reports no new warnings; the suite still carries pre-existing pedantic warnings that predate this work, so "clean" is not literally true and was never achieved. Full e2e suite (`cargo test --test e2e -- --ignored`) passes
- [x] Release build writes memory metrics CSV and warns above the budget (CSV verified on the release binary; the `warn!` path is code-reviewed only, since tripping a 2 GiB budget on purpose is not worth the test time). Caveat: rows carry no process identity, so a CSV cannot be attributed to a particular binary after the fact. `SUNLIT_EARTH_METRICS_DIR` keeps test-spawned processes out of the real file, which is what makes the multi-day sample trustworthy
- [ ] Multi-day validation on the real desktop: metrics CSV flat while hidden with live cloud updates, then check off the roadmap bug entry and tag 0.1.1

## Implementation Steps

### Step 1: Test knobs in the cloud fetcher

**Files**: `src/cloud_fetcher.rs`

Make the three hardcoded values overridable via environment variables, read once at fetcher startup, falling back to the current constants:

- `SUNLIT_EARTH_CLOUD_URL` overrides `CLOUD_URL`
- `SUNLIT_EARTH_CLOUD_POLL_SECS` overrides `POLL_INTERVAL` (parse `u64`, ignore invalid values with a `warn!`)
- `SUNLIT_EARTH_CACHE_DIR` overrides `cache_dir()`, so tests do not overwrite the developer's real cloud cache in `%LOCALAPPDATA%\SunlitEarth`

This follows the existing knob convention (`SUNLIT_EARTH_NO_CLOUDS`, `SUNLIT_EARTH_TEXTURES`, `SUNLIT_EARTH_SYNC_LOG`). Add unit tests for the parse-and-fallback helpers.

**Verify**: `cargo test cloud_fetcher`

### Step 2: `query-memory` IPC command

**Files**: `src/ipc.rs`, `tests/e2e.rs`

Add a `query-memory` command that reads `crate::memory::snapshot()` and prints `SIGNAL:memory rss_bytes=<n> peak_rss_bytes=<n> private_bytes=<n>` via the existing `signal()` path. Answer directly on the IPC listener thread; `GetProcessMemoryInfo` is process-wide and needs no event-loop hop, so the command works even when the event loop is busy or idle.

In `tests/e2e.rs`, extend `StdoutWatcher` with a `wait_for_signal_line()` variant that returns the matched line, and add a `query_memory()` helper that sends the command and parses the fields.

**Verify**: `cargo build`, manual: run the app with `--ipc-socket`, send `query-memory`, observe the signal line.

### Step 3: The regression test (must be red here)

**Files**: `tests/e2e.rs`

New test `test_hidden_window_cloud_updates_do_not_grow_memory`, `#[ignore]` and `#[serial]` like the rest of the suite.

**Cloud stub server**: a minimal hand-rolled HTTP/1.1 server on `127.0.0.1:0` (no new dev-dependencies; the surface is two request types). Shared state `Arc<StubState { version: AtomicU64, gets: AtomicU64 }>`:

- `HEAD /clouds.jpg`: if the request's `If-None-Match` equals `"v{version}"` respond `304`, else `200` with `ETag: "v{version}"`. Always send `Content-Length` and `Connection: close`.
- `GET /clouds.jpg`: respond `200` with the fixture JPEG body, `ETag: "v{version}"`, increment `gets`.

**Fixture**: generated in-test with the `image` crate (JPEG encoding is available in tests via feature unification with the main dependency; the existing `decode_cloud_jpeg_valid_minimal` test already encodes JPEG). Size 2048x1024 so each decoded frame is 8 MiB: large enough that leaked frames dominate noise, small enough that decode and mips are fast.

**Flow**:

1. Create a temp cache dir; start the stub server.
2. Spawn the binary with `--mode window --ipc-socket <name> --log-level debug` and env `SUNLIT_EARTH_CLOUD_URL=http://127.0.0.1:<port>/clouds.jpg`, `SUNLIT_EARTH_CLOUD_POLL_SECS=1`, `SUNLIT_EARTH_CACHE_DIR=<tmp>`. Window mode avoids the tray icon and the single-instance mutex; hiding via IPC still exercises the exact leak condition because `run_event_loop_until_quit()` is used in all modes and `BeforeRendering` stops on hide either way. Do not set `SUNLIT_EARTH_NO_CLOUDS`.
3. Wait for `SIGNAL:ipc_listener_ready` and `SIGNAL:first_frame_rendered`; wait until `gets >= 1` (initial download, processed while visible).
4. Send `hide-window`, wait for `SIGNAL:window_hidden`, let things settle briefly, then take the baseline via `query-memory`.
5. Repeat 15 times: bump `version`, wait until `gets` increments (timeout 15 s per update).
6. Take the end sample via `query-memory`.
7. **Assert on `private_bytes`, not RSS**: growth < 40 MiB. Private bytes (commit charge) is immune to working-set trimming, which can mask heap growth in RSS. Unfixed code is expected to grow by roughly 15 x 8 MiB = 120 MiB, well past the bound; fixed code parks at most one frame (8 MiB) plus noise. Log both metrics for the record.
8. Assert at least one `GPU texture created` log line appeared after `window_hidden` (proves the drain processes updates while hidden, not merely discards them). Send `export-test` while still hidden and expect `export_test_ok`.
9. Send `show-window`, then `quit`; assert exit code 0 and no ` ERROR ` lines in stderr.

Run the test now against the unfixed code and record the observed growth in Results. It must fail on the private-bytes assertion.

**Verify**: `cargo test --test e2e -- --ignored test_hidden_window_cloud_updates_do_not_grow_memory` fails with growth around 120 MiB.

### Step 4: The fix

**Files**: `src/renderer/textures.rs`, `src/renderer/mod.rs`, `src/main.rs`, `src/cloud_fetcher.rs`

1. Add `TextureMailbox` in `renderer/textures.rs`: `post(DecodedTextureMessage)` replaces the slot's parked message; `take(slot)` / drain iteration for the consumer. Internally `Mutex<Vec<Option<DecodedTextureMessage>>>` sized to the slot count, shared via `Arc`.
2. Replace the `mpsc::Sender`/`Receiver` pair throughout: `GpuResources.texture_tx`/`texture_rx`, `setup_rendering_notifier`, `init_texture_system` (`src/main.rs:222`), `maybe_spawn_texture_load`, `spawn_cloud_fetcher`. Producers keep their `upgrade_in_event_loop(request_redraw)` calls so a visible window still updates promptly.
3. Rework `process_decoded_textures` to drain the mailbox; the `BeforeRendering` call site stays as is.
4. Add `pub fn drain_texture_updates()` in `renderer/mod.rs`: `GPU_RESOURCES.with(...)`, run `process_decoded_textures`, and request a redraw only if something was processed and the window is visible.
5. In `run_event_loop` (`src/main.rs`), start a repeated 5 s Slint timer calling `drain_texture_updates()`. Keep it alive like the sun timer (including the `mem::forget` at exit, matching the existing pattern).

**Verify**: `cargo test`, `cargo clippy`, then the Step 3 regression test goes green; full e2e suite passes; manual: run the app, hide it, watch `GPU texture created` appear in the log when the stub (or the real service) publishes an update.

### Step 5: Production memory telemetry

**Files**: `src/memory.rs`, `src/main.rs`

Release builds compile out debug/info logging (`release_max_level_warn`), which is why the leak produced zero telemetry in 10 days. Rather than changing global tracing levels:

1. `memory.rs` gains `append_metrics_sample(path)`: appends a CSV line `unix_ts,rss_bytes,peak_rss_bytes,private_bytes` with simple rotation (rename to `.old` when the file exceeds 1 MiB, roughly 120 days at the chosen cadence).
2. A watchdog Slint timer in `run_event_loop`, every 10 minutes: write a sample to `%LOCALAPPDATA%\SunlitEarth\memory-metrics.csv`, and emit `warn!` (which survives release builds) when private bytes exceed a budget constant (2 GiB to start).
3. Unit tests for the CSV format and rotation.

**Verify**: `cargo test memory`, then run a release build for half an hour and confirm the CSV appears and grows by one line per interval.

### Step 6: Documentation

**Files**: `CLAUDE.md`, `docs/roadmap.md`, `docs/retrospective-2026-08.md`

Update the module descriptions in `CLAUDE.md` (cloud fetcher knobs, `query-memory` IPC command, mailbox + drain timer in the renderer, memory metrics file). Check off the roadmap bug entry only after Step 7's multi-day validation. Mark Phase 0 progress in the retrospective if anything deviated from this plan.

**Verify**: read through diffs.

### Step 7: Verification and release

1. `cargo test`, `cargo clippy`, `cargo test --test e2e -- --ignored` all green.
2. Red-then-green record for the regression test is filled into Results below.
3. Multi-day validation per retrospective section 8.2: run the release build hidden to tray with live cloud updates for at least two days; the metrics CSV must stay flat (private bytes within one frame of baseline).
4. Check off the roadmap entry, bump version to 0.1.1, tag after explicit approval.

## Risks and Mitigations

**Hand-rolled HTTP stub confuses ureq.** Mitigation: always send `Content-Length` and `Connection: close`, close the socket after each response; ureq issues independent requests, no keep-alive needed. If it proves brittle, fall back to the `tiny_http` dev-dependency.

**Memory assertions are flaky on CI or under memory pressure.** Mitigation: assert on private bytes (not RSS), keep the bound (40 MiB) an order of magnitude below the unfixed growth (about 120 MiB), and keep the test `#[ignore]` desktop-only for now, like the rest of the e2e suite.

**Drain timer causes UI jank when an 8K texture arrives while visible.** No change from the status quo: `BeforeRendering` already does this work on the UI thread today (retrospective section 4.2); the proper fix is scheduled for Phase 1. While hidden, the occasional 1-2 s of CPU work every three hours is acceptable.

**Timer does not fire while hidden.** Contradicted by the March research (`2026-03-28-research-slint-tray-app-patterns.md`: timers continue firing when the window is hidden) and by the shipped auto-refresh scheduler, which relies on exactly this. The regression test would catch it regardless.

**`--tray-start hidden`, never shown: mailbox holds up to one decoded frame per slot (about 134 MB for 8K clouds).** Bounded, accepted for Phase 0; documented as the trigger for the compressed-payload hardening.

## Rollback Strategy

All changes are in `src/cloud_fetcher.rs`, `src/ipc.rs`, `src/renderer/textures.rs`, `src/renderer/mod.rs`, `src/main.rs`, `src/memory.rs`, and `tests/e2e.rs`, landing as separate commits per step; `git revert` the fix commit independently of the test commits if a regression appears.

## Status

- [x] Plan approved
- [x] Steps 1-2: test knobs and query-memory landed
- [x] Step 3: regression test red against unfixed code (record below)
- [x] Step 4: fix landed, regression test green
- [x] Step 5: telemetry landed
- [x] Step 6: docs updated (`CLAUDE.md`, `docs/roadmap.md`, `docs/retrospective-2026-08.md`)
- [x] Step 7 items 1-2: full suite green, red-then-green record filled in below
- [ ] Step 7 items 3-4: multi-day validation, roadmap check-off, 0.1.1 tag

## Deviations

Recorded as they happened, smallest change that kept the plan's intent.

1. **Step 3, extra `SUNLIT_EARTH_CONFIG` knob in `src/config.rs`.** The plan isolated the cloud
   cache but not the app config. Every e2e test spawns the binary with the developer's real
   config from `%LOCALAPPDATA%\SunlitEarth`, and with auto-refresh enabled there a test run
   replaces the desktop wallpaper of the machine running the tests, using whatever the test
   happens to render. `config_path()` now honors `SUNLIT_EARTH_CONFIG`, and every spawn site in
   `tests/e2e.rs` points at a throwaway config file. Same rationale as the cache-dir knob, one
   file further.
2. **Step 3, assertion order.** The plan asserts memory (item 7) before shutting the process down
   (item 9). The test instead collects both samples, runs the export check, shows the window,
   quits, and asserts afterwards. A failing run then leaves a gracefully exited process and a
   complete log rather than one killed by `ChildGuard`. The private-bytes assertion is still the
   first assertion, so it is what a regression reports.
3. **Step 3, "processed while hidden" measurement.** Counting `GPU texture created` lines from
   the hide onwards is wrong: `show-window` drains everything that was parked, so the burst that
   arrives at the end of the test lands inside the counted range and the unfixed code appears to
   have processed all 15 updates while hidden. The count is taken between a cursor set after the
   hide and a second cursor set immediately before `show-window`.
4. **Step 4, extra `texture_dirty` flag on `GpuResources`.** With two consumers, the drain timer
   can take a message before `BeforeRendering` sees it. `process_decoded_textures` then returns
   `false` on the next frame and the dirty check skips the render, so a freshly uploaded texture
   would not be displayed until some other parameter changed. The flag is set whenever a texture
   is uploaded and taken by `BeforeRendering`, so the re-render happens no matter which consumer
   drained the mailbox.
5. **Step 5, one metrics sample at startup.** The plan has the watchdog write only on its timer,
   which means nothing is recorded for the first ten minutes and a short-lived run leaves no
   trace at all. `run_event_loop` writes one sample before starting the timer, so every run has a
   startup baseline for later samples to be compared against. It also makes the feature verifiable
   without waiting out a full interval.
6. **Post-review, `SUNLIT_EARTH_METRICS_DIR` knob in `src/memory.rs`.** Combined with the startup
   sample above, the unconditional `metrics_path()` meant every test-spawned process appended to
   the developer's real metrics CSV. Since rows carry no process identity, those samples could not
   be filtered out again, which would have quietly corrupted the multi-day validation this plan
   still depends on. The directory is now overridable and all eight e2e spawn sites redirect it,
   the same treatment `SUNLIT_EARTH_CONFIG` gets. The `env_override` helper that defines "unset or
   blank" moved to `lib.rs` so the cloud, config, and metrics knobs share one definition, each
   resolved by a pure `*_from(Option<&str>)` function with unit tests on both branches.

## Results

### Red run (unfixed code)

`cargo test --test e2e -- --ignored --nocapture test_hidden_window_cloud_updates_do_not_grow_memory`
on the development desktop (Windows 11, real GPU), 2026-08-15, commit `f4a6121` plus the test:

```
baseline: rss=241.0 MiB private=340.4 MiB
after 15 hidden cloud updates: rss=361.4 MiB private=460.8 MiB peak_rss=373.4 MiB
growth: private=120.4 MiB rss=120.3 MiB (limit 40 MiB), GPU textures created while hidden: 0
FAILED: private bytes grew by 120.4 MiB across 15 cloud updates while hidden (limit 40 MiB)
```

120.4 MiB over 15 updates is 8.03 MiB per update, exactly the decoded size of the 2048x1024
fixture (2048 x 1024 x 4 = 8 MiB). Three consecutive runs reproduced this within 0.3 MiB.

The child's log confirms the mechanism directly: the window was hidden at 11:20:30.5 and no
`GPU texture created` line appeared until `show-window` at 11:21:01.4, at which point all 15
parked frames were processed in a 1.4 second burst. Nothing consumes the channel while the
window is hidden, which is the leak.

### Green run (fixed code)

Same command and machine, immediately after the mailbox and drain timer landed:

```
baseline: rss=240.8 MiB private=340.5 MiB
after 15 hidden cloud updates: rss=242.5 MiB private=342.3 MiB peak_rss=270.5 MiB
growth: private=1.8 MiB rss=1.7 MiB (limit 40 MiB), GPU textures created while hidden: 4
ok
```

Private-bytes growth drops from 120.4 MiB to 1.8 MiB, a 67x reduction, and stays well under one
decoded frame. A second run inside the full suite reported 2.0 MiB, so the result is stable.

The four GPU textures created between hide and show are the mailbox working as designed, not
updates being lost. The drain timer runs every 5 seconds while the 15 updates arrive over about
24 seconds, so each drain finds only the newest frame parked in the cloud slot and the
intermediate ones have already been replaced. Every drain uploads the most recent cloud image,
which is the only one that matters. Peak RSS also drops from 373.4 MiB to 270.5 MiB, because
the 15 parked frames are no longer processed in one burst when the window is shown again.

Full suite: `cargo test --test e2e -- --ignored` passes all 8 tests in 43 seconds.

### Release telemetry check

`cargo build --release`, then the binary run windowed with the window hidden over IPC for twelve
minutes, `%LOCALAPPDATA%\SunlitEarth\memory-metrics.csv` afterwards:

```
unix_ts,rss_bytes,peak_rss_bytes,private_bytes
1786793992,200224768,201232384,319733760
1786794592,257556480,258437120,370249728
```

The second sample is exactly 600 seconds after the first, so the watchdog timer fires in a
release build, and the header plus both rows confirm the format. Release stderr was empty: with
`release_max_level_warn` nothing below `warn!` is emitted and private bytes stayed far under the
2 GiB budget, which is the expected quiet case.

Caveat found in review: CSV rows record only a timestamp and three counters, so a row cannot be
attributed to the binary that wrote it. Before `SUNLIT_EARTH_METRICS_DIR` existed, every
test-spawned process appended to the same real file (an e2e run left it at 22 lines), and those
rows could not be told apart from production samples afterwards. The knob keeps test runs out of
the file entirely, which is the prerequisite for the multi-day validation below meaning anything.
Attributing rows to a run, for example a process-start marker column, is left for later.

Verified after the fix: the real CSV was 22 lines with checksum `9e5411cb...` before a full e2e
run and 22 lines with the same checksum after it, while the redirected file in the test scratch
directory picked up 9 lines (a header plus one startup sample from each of the eight spawns).

### Multi-day validation

To be filled in during Step 7: metrics CSV summary from the release build.
