<!-- Reviewer notes for docs/reviews/2026-09-04-code-quality-review.md. Line numbers refer to commit 19312ba, the tree the review read, and were moved on 2026-09-05 for the files that changed on main since; the main report lists those under "Changes since the review". The changed code was not re-reviewed. "The brief" is the shared review instruction; "the maintainer's rules" are the project's comment conventions. Runtime claims here are reasoned from the code; the measured figures are in test-timing.md. -->

# Review: the engine thread and the wallpaper path (`sunlit-core`)

## 1. Summary

The engine loop is not the monolith the `#[allow]`s suggest: `run` is 43 lines, `tick` 69, and the two `too_many_lines`/`too_many_arguments` allows sit on `Engine::new` (144 lines) and `spawn_cloud_worker` (8 arguments), neither of which is the loop. The documented scheduling contract holds: the loop never sleeps on wall time, every deadline comes off the injected `Clock`, and all five cross-thread queues in the area are bounded or unbounded-with-reasoning. Commentary in `engine/mod.rs` is largely earned; the concentrations of design history are in `wgpu_init.rs::adapter_key` (34 lines of measurements) and in test doc comments across `wallpaper.rs` and `wallpaper_sink.rs`, several of which narrate bug history the maintainer's own rule forbids.

The two structural problems worth fixing before a release are that `wallpaper.rs` is 1609 lines of which about 670 non-test lines are Windows-only and belong in `wallpaper/windows.rs`, and that the Linux publish path lives in `engine/wallpaper_sink.rs` while the Windows one lives in `wallpaper.rs`, which is why the same Arc-dedupe write loop is written twice.

One resilience gap: `wgpu_init::init` panics (assert + expect) where a machine has no usable adapter, and that surfaces to the user as a double panic from `engine::start` with no window and no message. Eight `pub` items in `wallpaper.rs` never leave the crate, and one of them, `wallpaper_on_monitor`, has no caller anywhere.

## 2. Metrics

| File | Total | Test module | Tests | Comment lines | Blocks >3 lines (keep / move / delete) | Fns >80 lines | `pub` used only in-crate |
|---|---|---|---|---|---|---|---|
| `engine/mod.rs` | 1485 | 1394-1485 (92) | 10 | 373 (25%) | 35 (29 / 6 / 0) | `new` 144, `handle` 102, `build_wallpaper_job` 81, `spawn_cloud_worker` 86 | 0 |
| `engine/clock.rs` | 136 | 85-136 (52) | 5 | 15 (11%) | 1 (1 / 0 / 0) | none | 0 |
| `engine/wallpaper_sink.rs` | 755 | 515-755 (241) | 12 | 175 (23%) | 23 (18 / 3 / 2) | `write_placement` 85 | 1 (`DEFAULT_TARGET_SIZE`) |
| `wallpaper.rs` | 1609 | 1106-1609 (504) + `cfg(test)` at 40-44, 67-70 | 19 | 436 (27%) | 41 (32 / 5 / 4) | `set_wallpaper_job` 117, `enumerate_monitors` 82 | 8 |
| `wgpu_init.rs` | 339 | 211-339 (129) | 7 | 103 (30%) | 7 (4 / 2 / 1) | none | 2 pub + 1 over-visible `pub(crate)` |
| `lib.rs` | 30 | none | 0 | 10 | 0 | none | 0 |

The `cfg(test)` items at `wallpaper.rs:40` and `:67` are the test-isolation guard, not stray test code: `wallpaper_dir()` resolves through a thread-local scratch override under `cfg(test)` and panics if none is set, so a unit test cannot overwrite the developer's real wallpaper. It is well designed and the `should_panic` test at 1217-1228 pins it. Note the scope limit: `cfg(test)` applies only to `sunlit-core`'s own unit tests, so `tests/engine.rs`, `tests/soak.rs` and the app compile the live path. That is correct today (those use `RecordingSink`/`CountingSink`), but it is a property nothing asserts.

## 3. Findings

### A. Commentary

**A1. `wgpu_init.rs:113-146` (34 lines) is a measurements essay in a doc comment. Severity: low.**
`adapter_key`'s doc carries per-adapter mean-channel-difference numbers, an observation that the ordering "was not the expected one", and a paragraph on why Metal lands closer to WARP than lavapipe does. The rule the function implements ("software rasterizers by name, hardware by backend") and the known coarseness note about llvmpipe are worth keeping; the numbers belong in `docs/testing.md` next to the golden-image tolerance they justify. Recommendation: keep roughly lines 113-117 and 132-146, move 118-131.

**A2. `wgpu_init.rs:41-47` records the history of a removed feature. Severity: low.**
"The adapter is not returned. It used to be, so the Slint shell could assemble a `WGPUConfiguration::Manual` and share this device; nothing does that any more." That is a commit message. Delete; the remaining sentence about the device keeping the adapter alive is the part that earns its place.

**A3. `wgpu_init.rs:5-23` mixes a real external invariant with test history. Severity: low, keep most.**
The Mesa TLS-destructor / `dlclose` SIGSEGV explanation is exactly the kind of comment the brief says to keep: it is an external bug and the only reason the `static` exists. Lines 13-17 ("that killed the engine integration tests at the moment the engine thread was joined: every time under `cargo test`, and intermittently when a single test was run on its own") are the diagnosis story. Trim those five lines, keep the rest.

**A4. Test doc comments that narrate bug history. Severity: low, but the maintainer's rule is explicit.**
- `wallpaper.rs:1305-1309`: "That name is not empty, which is what this used to be checked for, so nothing was ever recognized as unaddressable and a screen with no device path was handed to `SetWallpaper` anyway."
- `wallpaper.rs:1409-1411`: "this is the assertion the in-place rewrite that crashed plasmashell would have failed."
- `wallpaper_sink.rs:644-646`: "Refusing it made one screen with no pixels the end of the whole publish, and only in the spanned mode."
- `wallpaper_sink.rs:678-683`: "Split by cfg rather than skipped, because each half is an assertion" (why the test exists).
These are the clearest deletes under "no notes on why a test exists". There are roughly a dozen more test docs in the two files in the same style; they are a consistent house style rather than an accident, so this is one decision to make once rather than twelve nitpicks.

**A5. `wallpaper.rs:1052, 1058, 1062, 1067, 1079` restate the next line. Severity: low.**
`// Verify the file exists and is non-empty`, `// Canonicalize to absolute path`, `// Set "Fill" wallpaper style before applying`, `// Encode path as null-terminated UTF-16`, `// null terminator`. Also `wallpaper.rs:499-500` ("Callback invoked by EnumDisplayMonitors for each monitor. Pushes each HMONITOR handle into the Vec pointed to by lparam") duplicates both the signature and the SAFETY comment three lines below. Delete all six. This block is noticeably older in style than the rest of the file; the `// SAFETY:` comments around it are good and should stay.

**A6. `engine/mod.rs` design-history blocks. Severity: low.**
Six blocks are rationale that belongs in `docs/`: `191-198` (why the mailbox is injectable), `203-209` ("A test that passes only on one of the two is worse than no test"), `561-571` (the mailbox slot assert, where the assert message already says what is wrong), `986-999` (enumerates every caller of `publish_wallpaper`, a list that will rot), `1039-1052` (the last paragraph duplicates `docs/architecture.md:151` almost verbatim), and part of `368-389`. About 60 lines. Everything else in this file earns its place: the 256-byte readback padding note at `1176-1180`, the `DISPLAY_SETTLE` burst explanation at `52-61`, the cloud-slot-not-purged note at `774-780`, and the retry-loop placement notes at `1303-1312` all carry non-obvious invariants.

**A7. The unbounded-channel justification at `engine/mod.rs:368-389`. Severity: none, keep.**
22 lines, and `docs/architecture.md:211` explicitly points at it and says "keep it honest if any of those premises change". The two sentences that could go are the "Phase 0 leak" reference and "A bound was considered and rejected", which are history; the four bullets are the invariant.

**A8. `wallpaper_sink.rs:138-145` embeds a measurement. Severity: low.**
"about 14 MB at 2560x1440, on a CPU rasterizer where there is no hardware to help". The invariant ("a sink that will refuse has to say so before the render") is the keeper; the number belongs in `docs/`.

### B. Length and structure

**B1. `wallpaper.rs` at 1609 lines should become `wallpaper/mod.rs` + `wallpaper/windows.rs`. Severity: medium.**
The file has one portable half and one Windows half, cleanly separated by line:

| Lines | Content | Platform |
|---|---|---|
| 1-431 | `wallpaper_dir`, `Generation`, `PUBLISHED`, `generation_*`, `Publication`, `sweep_*`, `unfinished`, `encode_png` | portable (~430) |
| 433-660 | `ensure_dpi_awareness`, `enumerate_monitors`, `adopt_device_paths`, `wide_to_string`, `display_label`, `get_primary_monitor_resolution` | `cfg(windows)` (~228) |
| 662-857 | `mod shell` (the `IDesktopWallpaper` COM wrapper) | `cfg(windows)` (~196) |
| 859-1104 | `wallpaper_on_monitor`, `set_wallpaper_job`, `is_device_path`, `ensure_fill_style`, `set_wallpaper` | `cfg(windows)` (~246) |

The area brief asks whether `wallpaper/{windows,linux,macos}.rs` would be cleaner. Not in that shape: there is no Linux and no macOS code in this file at all. The right split is `wallpaper/mod.rs` (1-431 plus the portable tests, about 610 lines) and `wallpaper/windows.rs` (433-1104 plus the `cfg(windows)` tests, about 800 lines), optionally with `mod shell` moved down again to `wallpaper/windows/shell.rs` (196 lines) to keep the leaf under 700.

On consistency with the rest of the crate: `memory.rs` keeps three per-OS implementations inline with `#[cfg]` in one 859-line file, and `display.rs:210-228` writes the same `pub fn monitors()` three times behind three `cfg`s. So inline `cfg` is the crate's idiom and `wallpaper_sink.rs` follows it correctly. The argument for splitting `wallpaper.rs` is not the `cfg` style, it is the 670 lines of Win32 and COM that no other platform compiles and that nobody reading the publication lifecycle needs to scroll past. `desktop.rs` is the precedent that matters: it is Linux-only in purpose but not `cfg`-gated at all, because it is pure functions over an environment, and that is exactly why it could be pulled out into its own module.

**B2. The Linux publish path and the Windows publish path live in different modules. Severity: medium.**
`SystemWallpaper::publish` on Windows (`wallpaper_sink.rs:269-273`) is a one-line delegation to `wallpaper::set_wallpaper_job`, while on Linux (`281-312`) it inlines the whole implementation and pulls in `write_placement` (344-428), `which` (435-439) and `run` (446-460), about 140 lines of platform code in the sink module. That asymmetry is the direct cause of the duplication in D1 and D2. Recommendation: move `write_placement`, `which` and `run` into `wallpaper/linux.rs`, give it a `set_wallpaper_job(job) -> Result<String, String>` with the same signature as the Windows one, and reduce both `publish` arms to the same delegation. `wallpaper_sink.rs` then holds the trait, the job types and the two test doubles, and drops to about 610 lines.

**B3. `Engine::new` (534-677, 144 lines) carries the `too_many_lines` allow. Severity: low.**
Three seams, each a clean extraction:
- 561-581 the mailbox slot check → `fn checked_mailbox(mailbox: Option<TextureMailbox>, slots: SlotLayout, paths: usize) -> TextureMailbox`
- 601-630 sample-count resolution and renderer construction → `fn build_renderer(gpu, &mut params, quality, ...) -> Renderer`
- 632-644 the cloud worker is already one call
That leaves `new` at roughly 60 lines of destructure plus struct literal and removes the allow.

**B4. `Engine::handle` (724-825, 102 lines). Severity: low.**
Four arms have bodies over five lines; two of them are the whole excess. Extract `fn set_texture_resolution(&mut self, width: u32)` (766-787) and `fn set_auto_refresh(&mut self, enabled: bool, interval: Duration)` (791-806) and every arm becomes one or two lines, leaving `handle` at about 70.

**B5. `engine/mod.rs` at 1485 lines: a worthwhile split, in this order. Severity: low.**
The engine loop is *not* one giant function, so the split is about navigation rather than untangling.
- `engine/schedule.rs`: `Schedule` (414-446) plus its four unit tests (1398-1425). ~75 lines.
- `engine/protocol.rs`: `EngineCommand` and `EngineEvent` (64-150) plus `command_payload_is_small` (1461-1475). ~105 lines.
- `engine/handle.rs`: `EngineConfig` (152-235), `EngineHandle`, `AdapterReport`, `join_engine` (237-361) and `start` (363-412). ~260 lines.
- `engine/cloud_worker.rs`: `CloudWorker` (513-530), its impl (1242-1263) and `spawn_cloud_worker` (1265-1355). ~125 lines.
- `engine/publish.rs`: an `impl Engine` block holding `publish_wallpaper`, `report_wallpaper`, `recheck_displays`, `build_wallpaper_job`, `check_export_fits`, `export_framed` (1000-1205). ~205 lines.
What remains is `Engine`, `PreviewState`, `new`, `run`, `handle`, `tick` and the render helpers: about 500 lines, which is the file a reader of the loop actually wants.

**B6. `save_png` is in the wrong module. Severity: low.**
`engine/mod.rs:1381-1392` is a generic PNG writer with no engine state, used by `sunlit-app/src/displays.rs:417,425` and `tests/golden.rs:294` as well as by `render_to_file`. It sits between `preview_target_size` and the test module for no reason. It also duplicates `wallpaper.rs:420-431`'s `encode_png` and a third copy in `assets/texture_cache.rs:330`. See D5.

**B7. `set_wallpaper_job` (`wallpaper.rs:890-1006`, 117 lines) has three independent branches. Severity: low.**
Single-monitor (901-907), spanned (909-922), and per-monitor (924-1005). The third is the long one and splits at 952: everything before is "write the files", everything after is "hand them to the shell and describe what went wrong". Extract `fn address_each_monitor(api, images, anchor_path, job) -> Result<String, String>` for 954-1005.

### C. Consistency

**C1. Eight `pub` items in `wallpaper.rs` never leave the crate, and one has no caller at all. Severity: medium.**
Verified by grepping `crates/sunlit-app/src`, `crates/sunlit-app/tests`, `crates/sunlit-core/tests` and `crates/xtask`:

| Item | Line | Only caller |
|---|---|---|
| `wallpaper_on_monitor` | 864 | **none anywhere** |
| `get_primary_monitor_resolution` | 650 | its own tests only (1260, 1290, 1334) |
| `wallpaper_dir` | 39 | `wallpaper.rs` itself |
| `ensure_dpi_awareness` | 452 | `wallpaper.rs:517` |
| `set_wallpaper` | 1049 | `wallpaper.rs` (980, 905) |
| `begin_publication` / `Publication` / `write` / `commit` | 251, 241, 278, 324 | `wallpaper_sink.rs:350` |
| `set_wallpaper_job` | 890 | `wallpaper_sink.rs:270` |
| `enumerate_monitors` | 490 | `display.rs:217` |

`published_wallpaper_files` (212) is the only one with an external consumer (`sunlit-app/tests/e2e.rs`, five call sites). Recommendation: delete `wallpaper_on_monitor` outright, demote the rest to `pub(crate)`. `get_primary_monitor_resolution` is only kept alive by its own tests, so it is a delete candidate too, but `docs/plans/` references it and it is a reasonable public query; demoting it will make the compiler tell you.

Also `wallpaper_sink.rs:175 DEFAULT_TARGET_SIZE` is `pub` and used only inside its own file (194, 246, 752), and `wgpu_init.rs:172 adapter_type_rank` is `pub(crate)` but used only inside `wgpu_init.rs` (207 and its tests): make it private. `wgpu_init::WgpuContext` and `init` are `pub` but only the engine uses them; `instance` and `adapter_key` really are used by `tests/` and must stay `pub`.

**C2. `wgpu_init.rs` has no module doc. Severity: low.**
Every other file in the area opens with `//!`. It is not unique (`scene/mod.rs` and `geometry/mod.rs` also lack one), but it is the only one in this area, and `init`'s doc opens with "This gives us full control over adapter selection", the only first-person voice in the crate.

**C3. Error-message capitalization is inconsistent inside `wallpaper.rs`. Severity: low.**
The older half uses sentence case: "Failed to create wallpaper directory: {e}" (48), "Failed to create the wallpaper generation directory" (255), "Failed to put the wallpaper PNG in place" (295), "Failed to create PNG file" (425), "Wallpaper file not found" (1053), "Wallpaper file is empty" (1055), "No primary monitor found" (653). The newer half uses lowercase: "cannot reach the desktop wallpaper interface" (778), "this publish has no image for its own anchor" (979), "cannot set the wallpaper position" (822). Everything in `engine/mod.rs` and `wallpaper_sink.rs` is lowercase. These strings reach the status line, so it is user-visible. Recommendation: lowercase throughout, which is also the Rust convention.

**C4. `#[allow]` versus `#[expect]`, with a live example. Severity: low.**
The workspace uses `#[allow]` everywhere (110 in `sunlit-core/src`, 2 `#[expect]` in the whole workspace). One allow in this area is provably unnecessary: `engine/mod.rs:1456 #[allow(clippy::cast_precision_loss)]` sits on `let ratio = f64::from(w) / f64::from(h);`, where `w` and `h` are `u32` and `f64::from` is a lossless `From` impl, not an `as` cast, so the lint cannot fire. `#[expect]` would have reported it as unfulfilled. Also `wallpaper.rs:501` puts `#[allow(unsafe_code)]` on `enum_callback` and then again at 511 inside its body; the inner one is redundant.

**C5. Cast allows that could be a helper. Severity: low.**
`wallpaper.rs:489` carries `#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]` for the whole 82-line `enumerate_monitors`, covering `(rc.right - rc.left) as u32` (563), `(rc.bottom - rc.top) as u32` (564) and `size_of::<MONITORINFOEXW>() as u32` (545). A negative or inverted rectangle would wrap to a huge `u32` rather than being caught. `u32::try_from(rc.right - rc.left).unwrap_or(0)` needs no allow and cannot wrap, and a monitor with zero width is already handled everywhere downstream (`Rect::is_empty`, `bounds_of`, `render_groups`). Confidence: the wrap is plausible rather than confirmed; `rcMonitor` is normalized in practice.

**C6. Poison handling is inconsistent between production and test code in `wallpaper.rs`. Severity: low-medium.**
Production uses `.expect("the published generation is poisoned")` at 214 and 326; the test helper at 1140 and 1171 uses `unwrap_or_else(std::sync::PoisonError::into_inner)`. `PUBLISHED` guards a plain `Option<Generation>` with no invariant a panic could break, and `commit` holds the lock across `remove_dir_all` of every stale generation, so a panic under the lock is not impossible. If it ever happened, every subsequent publish would panic on the engine thread and the app would look alive with nothing rendering. Recommendation: use `into_inner` in production too, which is what the tests already decided is correct.

### D. Duplication

**D1. The Arc-dedupe image-write loop is written twice. Severity: medium.**
`wallpaper_sink.rs:359-389` (Linux) and `wallpaper.rs:927-951` (Windows) are the same 25 lines: iterate monitors, skip `None`, linear-search `written` with `Arc::ptr_eq`, write with `index.to_string()` as the suffix, remember the anchor's path. Even the comment is duplicated word for word ("Two screens showing the same picture cost one render, and this is what carries that as far as the file: one encode and one path"). Recommendation: one method on `Publication`, `fn write_job(&mut self, job: &WallpaperJob) -> Result<WrittenImages, String>`, returning the per-index paths, the anchor path and the untouched list. Removes about 25 lines from each side and makes B2 nearly free.

**D2. Three error strings are duplicated across the two modules. Severity: low.**
`"a view across the screens was asked for without a canvas"` at `wallpaper_sink.rs:404` and `wallpaper.rs:912`; `"this publish has no image for its own anchor"` at `wallpaper_sink.rs:124`, `wallpaper_sink.rs:391` and `wallpaper.rs:979`. The second already has a canonical home in `WallpaperJob::anchor_image` (122-125); the other two sites should call it or reuse its message.

**D3. The MSAA fallback warning is written twice. Severity: low.**
`engine/mod.rs:607-614` (in `new`) and `912-919` (in `resolve_requested_sample_count`) build the same `warn!` with the same fields and the same message. A free `fn resolve_and_warn(requested: u32, supported: &[u32], max: u32) -> u32` would serve both, and `new` currently cannot call the method because `self` does not exist yet, which is the only reason the copy is there.

**D4. The Linux backend lookup is written twice. Severity: low.**
`wallpaper_sink.rs:217-221` and `282-286` are the identical five-line `detect_current().ok_or_else(|| no_backend_message(&env_override(DESKTOP_ENV).unwrap_or_default()))`. Extract `fn current_backend() -> Result<Backend, String>`. The doc at 277-279 explains why the *lookup* is repeated at runtime, which is correct and should stay; the *code* need not be.

**D5. Three PNG encoders in one crate. Severity: low.**
`engine::save_png` (`engine/mod.rs:1381`, via `ImageBuffer` + `img.save`), `wallpaper::encode_png` (`wallpaper.rs:420`, via `PngEncoder` with `CompressionType::Fast`), and `assets::texture_cache::save_png` (330). The three differ in compression settings, which for the wallpaper one is a deliberate and documented choice, so this is not a straight merge; but the engine one and the texture-cache one could share.

**D6. `newest_generation` and `newest_generation_other_than` differ by one filter. Severity: low.**
`wallpaper.rs:197-203` and `351-358` are the same fold; the second adds `.filter(|dir| dir != current)`. Merge into `fn newest_generation(root: &Path, excluding: Option<&Path>) -> Option<PathBuf>`.

### E. Correctness and resilience

**E1. `wgpu_init::init` panics on a machine with no usable adapter, and the failure reaches the user as a double panic. Severity: medium-high. Confirmed by reading.**
`wgpu_init.rs:73` is `assert!(!adapters.is_empty(), "No wgpu adapters found")` and `:97` is `.expect("Failed to create wgpu device")`. Both run inside `Engine::new`, on the engine thread, before `ready` is sent. The thread unwinds, `ready_tx` drops, and `engine/mod.rs:403-405` panics on the main thread with "engine thread died before reporting its adapter". `crates/sunlit-app/src/main.rs` installs no panic hook and no `catch_unwind`, so a user whose D3D12 runtime or Vulkan loader is broken gets two Rust backtraces and no window. `--software-rendering` does not help: `select_adapter` falls through with a `warn!` and the same `init` still runs. Recommendation for a first public release: make `init` return `Result<WgpuContext, String>`, send `Result<AdapterReport, String>` through the ready channel, and have `start` return `Result<EngineHandle, String>` so `main` can print one sentence. If that is too large a change now, at minimum a panic hook that prints a human message. `wgpu_init.rs:208 .expect("No adapters available")` is unreachable given the assert at 73 and can go either way.

**E2. While a wallpaper publish is deferred, `check_supported` runs 20 times a second. Severity: low. Confirmed by reading.**
`publish_wallpaper` (1000) calls `self.wallpaper.check_supported()` before anything else, then returns early with `wallpaper_owed = true` when textures are pending (1011-1017). `tick` re-enters `publish_wallpaper` on every iteration while the debt stands (893-895), and iterations are at most `TICK` = 50 ms apart. On Linux `check_supported` (216-231) reads `XDG_CURRENT_DESKTOP`, runs the desktop detection table, and does a full `PATH` walk with an `is_file()` stat per directory. For the duration of an 8K texture reload that is a few thousand stat calls for an answer that cannot change. The `info!` is already guarded against repeating (1012-1014), so the author saw the loop. Recommendation: check `textures_pending` before `check_supported`, or cache the support answer for the life of one debt.

**E3. `Publication::commit` records the generation before the setter runs, so `published_wallpaper_files` can name files the desktop never took. Severity: low. Confirmed by reading.**
`write_placement` (`wallpaper_sink.rs:393`) and `set_wallpaper_job` (`wallpaper.rs:904, 914, 952`) all commit before running the setter, which the doc at 302-315 justifies well for the sweep. The consequence is that `published_wallpaper_files`'s own doc ("This is what a desktop's own store holds once the setter has run") is not quite true when the setter fails. The e2e suite reads it. A sentence in the doc is enough.

**E4. `RenderWallpaperNow` is not coalesced. Severity: low. Confirmed by reading.**
`UpdateParams` coalesces naturally because `handle` only assigns and the render happens in `tick`. `RenderWallpaperNow` (744) publishes immediately inside `handle`, so two of them in one drain batch produce two full native-resolution renders, readbacks and PNG encodes. Reachable from a double-click on "Set as Wallpaper" or from the tray plus IPC arriving together. Recommendation: set a `publish_owed` latch in `handle` and do the publish once in `tick`, which is also where the auto-refresh and the display-recheck publishes already happen and where the ordering comment at 866-868 lives.

**E5. `Frame` derives `Debug` while holding a multi-megabyte pixel buffer. Severity: low.**
`wallpaper_sink.rs:18`. `assert_eq!(job.anchor_image().unwrap(), left)` at 566 would, on failure, print two full buffers; in production a stray `{:?}` on a `WallpaperJob` would print tens of millions of integers. A hand-written `Debug` printing `width`, `height` and `pixels.len()` is three lines.

**E6. `save_png` copies the whole framebuffer. Severity: low.**
`engine/mod.rs:1388` is `ImageBuffer::from_raw(width, height, pixels.to_vec())`, which duplicates up to 33 MB for a 4K render before encoding. `wallpaper.rs:420-431` already shows the borrowing form (`PngEncoder::write_image` on `&[u8]`). Same fix, no behavior change except that `save_png` would then honor its name rather than inferring the format from the extension.

**E7. Verified: the loop never sleeps on wall time, and every queue is bounded or justified.**
- `run` (679-721) blocks only on `rx.recv_timeout(TICK)` and `rx.try_recv()`; there is no `sleep` anywhere in the engine thread. The only `std::thread::sleep` in the module is `1330`, inside the cloud worker's retry backoff on its own thread, and it wakes to check for disconnection (1332-1334).
- Every deadline is `self.clock.elapsed()` (829, 792, 810, 869); `Schedule::due` (433-440) recomputes from `now` rather than accumulating, and `schedule_does_not_burst_after_a_long_stall` pins it.
- Cross-thread queues in this area: the command channel is `unbounded` with the four-bullet justification at the declaration (368-389, and `docs/architecture.md:211` points at it); the ready channel is `bounded(1)` (391); the three reply channels are `bounded(1)` (290, 305, 316); the cloud request channel is `bounded::<()>(1)` (1282) with the `busy` flag documented at 525-528. All five satisfy the rule.
- The reply channels use `recv()` with no timeout, but every caller is either `main` before the event loop or the IPC thread (`sunlit-app/src/ipc.rs:54` spawns its own), so a slow engine cannot freeze the UI.

**E8. `expect` audit in non-test code. All four are real invariants.**
`engine/mod.rs:401` and `:1341` are `thread::Builder::spawn` failures at startup; `:405` is the deliberate "a failure in `Engine::new` becomes a panic in `start`" design documented at 570-571, and it is only a problem because of what E1 lets reach it. `clock.rs:70,77` are `MockClock` mutex poisoning; `MockClock` is a test double that happens to live in non-test code, so this is fine. `wallpaper.rs:41` is inside `cfg(test)`. The `assert_eq!` at `engine/mod.rs:574` (mailbox slot count) is a real construction invariant and its message is good. `wallpaper.rs:214` and `:326` are the poison handling in C6.

### F. Unit tests

Totals: 53 tests across the five files. I would remove or merge 10.

**`engine/clock.rs` (5 tests, remove 2).**
- `system_clock_elapsed_never_goes_backwards` (90) asserts the documented contract of `Instant::elapsed`. Remove.
- `mock_clock_is_shareable_across_threads` (125) asserts what `Mutex<Duration>` plus the `Clock: Send + Sync` bound already guarantee at compile time. Remove.
- `mock_clock_advances_are_cumulative` (116) adds nothing over `mock_clock_advances_both_measures_together` (105) except a loop. Merge.

**`engine/mod.rs` (10 tests, merge 2, fix 1).**
- `schedule_is_not_due_before_its_interval` (1399) and `schedule_fires_at_the_interval` (1405) are two halves of one assertion. Merge.
- `preview_size_is_capped_at_the_low_tier` (1436) asserts `(w, h) == (1280, 704)`, which pins `QualityTier::Low::max_preview_width()`. The project rule is that changing a preset must not break a test. The first assertion in the same test (`w <= max_preview_width()`) is the behavioral one; derive the expected pair from `max_preview_width()` instead of writing 1280.
- `save_png_rejects_a_short_buffer` (1478) creates and removes a temp directory it never writes into: `save_png` fails on the buffer length before touching the filesystem. Drop the directory work.
- `command_payload_is_small` (1468), `preview_size_cap_grows_with_the_tier` (1444) and `preview_size_cap_preserves_the_aspect_ratio` (1454) are good behavioral tests; keep.

**`wgpu_init.rs` (7 tests, remove 2).**
`discrete_gpu_ranks_best` (284) and `cpu_ranks_worst` (304) are both strictly implied by `full_ordering` (323). Remove both, keep `full_ordering`. `two_software_rasterizers_on_one_backend_differ` (235) is implied by `software_rasterizers_are_keyed_by_name` (216) but its name documents the invariant, so it is defensible to keep. None of the seven touches a GPU, which is right.

**`wallpaper.rs` (19 tests, merge 3 into 1).**
- `primary_resolution_is_nonzero` (1259), `primary_resolution_is_reasonable` (1333) and `every_monitor_is_enumerated_with_a_rectangle_and_one_of_them_is_primary` (1270) each run a full `enumerate_monitors`, which on Windows means `EnumDisplayMonitors`, a `GetMonitorInfoW` per monitor, and `adopt_device_paths` opening a COM object with `CoCreateInstance` plus a `GetMonitorRECT` per device path. The third already asserts everything the first two do (line 1289-1292 checks `get_primary_monitor_resolution` against the enumerated primary). Merge all three into it and add the `>= 640 / >= 480` bounds. This is the largest test-time saving in the area on Windows: three COM round trips become one.
- The seven generation and sweep tests (1240, 1363, 1385, 1413, 1444, 1492, 1530, 1559) all do real filesystem work but only with 2x2 and 4x4 PNGs; they are the right tests for this code and are cheap. Keep all.
- `the_wallpaper_lives_under_this_systems_local_data_directory` (1176) pins the directory name "SunlitEarth". It is a cross-module contract with the config and texture-cache directories rather than a preset, so it is defensible; flagging it only for the record.

**`engine/wallpaper_sink.rs` (12 tests, remove 1, merge 1).**
- `system_wallpaper_accepts_frames_on_windows` (686) asserts that a trait default method returns `Ok(())`. `SystemWallpaper` does not override `check_supported` on Windows, so nothing under test runs. Remove, or fold into `counting_sink_starts_empty_and_counts_publishes`'s sibling.
- `the_monitors_are_this_session_or_the_documented_default` (737) calls `crate::display::monitors()` a second time at 750 to decide what to assert; on Windows that is a second full enumeration and COM open. Capture the first result instead.
- The `image_for` tests (555, 599, 621, 637, 648) look like they duplicate `tests/engine.rs`'s `every_screen_renders_one_image_per_distinct_size` / `across_screens_renders_one_canvas_and_cuts_it` / `one_screen_paints_the_anchor_and_leaves_the_others_alone`, but they exercise the pure job logic with no GPU while the integration tests exercise the engine end to end. That is the right layering. Keep both.

### G. Best practices

**G1. `build_wallpaper_job:1088` clones the whole monitor list on every publish** (`self.known_monitors = Some(monitors.clone())`) before moving `monitors` into the job. With the auto-refresh on, that is one `Vec<Monitor>` clone per interval. Trivial in absolute terms; mentioned only because the same list is cloned again at `recheck_displays:1069` for the `MonitorsChanged` event.

**G2. `Publication::write` takes `suffix: &str` and every caller passes `&index.to_string()`** (`wallpaper_sink.rs:376`, `wallpaper.rs:903, 943, 419`), allocating a `String` per screen to build another. `impl std::fmt::Display` for the suffix, or taking `usize` with a `canvas` variant, avoids it.

**G3. `wallpaper.rs:1077` `wide_path = wide_path[4..].to_vec();`** reallocates the whole path to strip four `u16`s. `wide_path.drain(..4);` does it in place.

**G4. `preview_target_size:1371`'s `w > 0` guard is dead** unless `max_preview_width()` can return 0: `w > max_w` with `max_w >= 0` already implies `w > 0`. Harmless, but it reads as a division-by-zero guard that is not needed where it is written.

**G5. `engine/mod.rs:452 struct_excessive_bools` is justified and should stay.** Five independent latches on a private struct is what the lint is wrong about; the comment at 448-451 says so correctly. `1269 too_many_arguments` on `spawn_cloud_worker` (8 arguments) is the weaker case: six of the eight come straight out of `EngineConfig`, so a small `CloudConfig` struct would remove the allow, but the gain is cosmetic.

### H. Documentation drift (outside the brief, but load-bearing)

`docs/architecture.md:153` still describes the two-slot naming scheme: "The output alternates between two slots rather than being one name ... the names are `wallpaper-<slot>-<index>.png` per screen and `wallpaper-<slot>-canvas.png` for a span, and taking a slot empties it first." The code replaced that with per-publish generation directories (`gen-<millis>-<pid>-<counter>/<index>.png`, `wallpaper.rs:92-125`), and `sweep_legacy_files` (388-404) exists precisely to delete the files that paragraph describes. Severity: medium for a release, because that section is the one a contributor reads before touching `wallpaper.rs`.

## 4. Recommended refactors, by value over effort

| # | Refactor | Effort | Risk | Why |
|---|---|---|---|---|
| 1 | Delete `wallpaper_on_monitor`; demote the other seven `wallpaper.rs` items, `DEFAULT_TARGET_SIZE` and `adapter_type_rank` to `pub(crate)`/private (C1) | 0.5 h | none, the compiler proves it | Shrinks the public surface of the crate right before its first release |
| 2 | Update `docs/architecture.md:153` to the generation-directory scheme (H) | 0.5 h | none | The section is actively misleading about the code it introduces |
| 3 | Check `textures_pending` before `check_supported` in `publish_wallpaper` (E2) | 0.5 h | low | Removes a few thousand `PATH` stats per deferred publish on Linux |
| 4 | Merge the three Windows monitor-enumeration tests into one; drop the second `display::monitors()` call in `wallpaper_sink.rs:750`; remove the four tautological tests in `clock.rs` and `wgpu_init.rs` (F) | 1 h | none | Three COM round trips become one, and eight tests become four |
| 5 | Extract `Publication::write_job` and collapse the duplicated write loops and error strings (D1, D2) | 2 h | low, both paths have tests | Removes 50 lines and prevents the two platforms drifting apart |
| 6 | Split `wallpaper.rs` into `wallpaper/mod.rs` + `wallpaper/windows.rs` (B1) | 2 h | low, it is a move | 1609 lines becomes 610 + 800, and only one of them is Win32 |
| 7 | Move `write_placement`/`which`/`run` into `wallpaper/linux.rs` so both platforms delegate identically (B2) | 2 h | low, needs a Linux VM run to confirm | Makes the sink platform-agnostic; folds naturally into 5 and 6 |
| 8 | Split `Engine::new` and `Engine::handle`, removing the `too_many_lines` allow (B3, B4) | 2 h | low | The two functions the allows point at |
| 9 | Make `wgpu_init::init` and `engine::start` return `Result`, and have `main` print one sentence (E1) | 4 h | medium, touches three call sites in `main.rs` and every test harness that calls `start` | The difference between "no GPU driver" and two Rust backtraces on a user's screen |
| 10 | Split `engine/mod.rs` into `schedule.rs`, `protocol.rs`, `handle.rs`, `cloud_worker.rs`, `publish.rs` (B5) | 3 h | low, it is a move | Navigation only; do it after 8 so the seams are already clean |
| 11 | Comment pass: A1 to A6, about 120 lines moved to `docs/` or deleted | 2 h | none | The maintainer's stated concern, applied where it is actually true |
| 12 | Poison handling (C6), `Frame: Debug` (E5), `save_png` copy (E6), the dead `cast_precision_loss` allow (C4) | 1 h | none | Small, independent, each one line to a few |
