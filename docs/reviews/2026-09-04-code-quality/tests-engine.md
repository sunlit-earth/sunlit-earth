<!-- Reviewer notes for docs/reviews/2026-09-04-code-quality-review.md. Line numbers refer to commit 19312ba, the tree the review read, and were moved on 2026-09-05 for the files that changed on main since; the main report lists those under "Changes since the review". The changed code was not re-reviewed. "The brief" is the shared review instruction; "the maintainer's rules" are the project's comment conventions. Runtime claims here are reasoned from the code; the measured figures are in test-timing.md. -->

# Review: the engine integration tests of `sunlit-core`

Files reviewed in full: `crates/sunlit-core/tests/engine.rs` (4765 lines, 78 tests), `tests/soak.rs` (326), `tests/support/mod.rs` (374), `tests/common/mod.rs` (124). Cross-checked against `tests/golden.rs`, `tests/render_pipeline.rs`, `crates/sunlit-core/src/engine/mod.rs`, `src/engine/wallpaper_sink.rs`, `Cargo.toml`, `docs/testing.md`.

## 1. Summary

The suite is careful and the assertions are good: it tests behavior, not constants, and the tolerances are measured rather than guessed. Its problem is one number. `engine.rs` starts **91 wgpu devices for 78 tests**, and a device plus the seven render pipelines built from the 1144-line `sphere.wgsl` costs about **1.2 seconds** on WARP. That single fact explains 109 s of the 161.6 s the target takes. The two tests that create no device are the only two under one second (0.299 s and under 0.1 s); every other test pays the floor.

The fix is already written, in the same directory, by the same author: `golden.rs` shares one engine across all 20 of its cases through `static ENGINE: LazyLock<Mutex<EngineHandle>>` and finishes in 4.0 s; `shading.rs` and `render_pipeline.rs` share one device through `common::GPU` and finish in 1.6 s and 2.1 s. `engine.rs` is the one GPU target that did not adopt the pattern. Applying it to the groups whose configuration is identical would take engine starts from 91 to roughly 28 and the target from 161.6 s to roughly 80 s, with no test deleted.

Second, `soak.rs` runs unconditionally in `cargo test` and costs 65.6 s, 40% of what all 78 engine tests cost. Its own comment records 44.6 s when it was tuned, so it has drifted 47% and now sits at 55% of the two-minute bound it asserts against itself.

Commentary is the other stated concern and it is real but narrower than the line count suggests: 863 comment lines in `engine.rs`, 78 blocks over three lines. Most carry a measured number a reader needs to interpret a tolerance. About 20 blocks narrate a bug, a plan step or a code review, and one references a test that does not exist.

## 2. Metrics

| | `engine.rs` | `soak.rs` | `support/mod.rs` | `common/mod.rs` |
|---|---|---|---|---|
| Total lines | 4765 | 326 | 374 | 124 |
| Tests | 78 | 1 | 0 | 0 |
| wgpu devices created | **91** | 1 | 0 | 0 (lazy, 1 per consumer binary) |
| Measured time | 161.6 s | 65.6 s | n/a | n/a |
| Comment lines | 863 (18%) | 77 (24%) | 118 (32%) | 24 (19%) |
| Comment blocks > 3 lines | 78 | 6 | 12 | 3 |
| classified keep / move / delete | 55 / 20 / 3 | 4 / 2 / 0 | 11 / 1 / 0 | 3 / 0 / 0 |
| Functions over 80 lines | 6 | 1 (169) | 0 | 0 |
| `#[allow(clippy::too_many_lines)]` | 0 | 1 | 0 | 0 |
| Unconditional `thread::sleep` | 17 × 300-500 ms, 4 × 400 ms, 1 × 2 s ≈ 9.9 s | 1 × 500 ms | 0 | 0 |
| Polling loops with real-time deadlines | 4 | 1 | 0 | 0 |
| `TIMEOUT` per wait | 60 s | 30 s | | |

Comparison targets, same run: `golden.rs` 20 tests / 4.00 s (one shared engine), `render_pipeline.rs` 21 / 2.08 s (one shared device), `shading.rs` 12 / 1.55 s (one shared device), all 1215 unit tests 4.17 s.

## 3. Timing

### 3.1 What the floor is

`engine.rs`: 2 tests under 1 s, 53 between 1 and 2 s, 16 between 2 and 3 s, 5 between 3 and 5 s, 2 over 5 s.

The floor is **one engine start**, and nothing else. The evidence is exact:

- The cheapest test that starts an engine is `textures_ready_fires_for_the_procedural_grid` (engine.rs:2618) at **1.189 s**. It starts an engine, drains events until `TexturesReady`, and returns. No fixture, no export, no sleep, no clock. 1.189 s is therefore the cost of `sunlit_core::engine::start` plus one tick.
- The only two tests under 1 s are the only two that create no device. `no_bright_star_is_baked_into_the_real_panorama` (engine.rs:4007) decodes a 4096×2048 JXL and scans it: 0.299 s. `a_mailbox_that_does_not_match_the_slot_count_is_refused` (engine.rs:1844) is under 0.1 s, and reading `src/engine/mod.rs:571-580` confirms why: the slot-count assertion fires *before* `crate::wgpu_init::init(force_software)` at line 583, so no device is ever created. (It also does not take `gpu_lock()`, which looks like an omission and is not one.)
- Every multi-engine test is a clean multiple of the floor. `one_monitor_is_one_image_at_its_own_size_in_every_mode` starts 3 engines (a loop over `DisplayMode::ALL`, 3 variants) and takes 3.718 s = 1.24 s each. `the_sunrise_band_arrives_with_the_suns_image` starts 3 (one per `sunrise_excess` call) and takes 3.600 s = 1.20 each. Every 2-engine test lands between 2.4 s and 2.8 s: `a_sun_grazing_the_limb` 2.838, `the_physical_exposure_has_no_peak` 2.784, `a_switched_off_moon_and_a_missing_texture` 2.602, `a_taller_canvas` 2.599, `one_screen_paints_the_anchor` 2.578, `the_anchors_crop_of_a_span` 2.526, `the_glare_peaks` 2.516, `a_pan_past_the_frame_corner` 2.517, `every_screen_renders_one_image_per_distinct_size` 2.435.

The whole target fits `time ≈ 1.2 s × engine_starts + explicit sleeps + real work`. 91 starts × 1.2 s = **109 s of the 161.6 s (68%)**.

What costs the 1.2 s, from the code: `wgpu_init::init` requests the WARP adapter and a device, then `Renderer::new` compiles `shaders/sphere.wgsl` (1144 lines) and `blend.wgsl` and builds **seven** render pipelines (`src/renderer/gpu_setup.rs:353, 420, 479, 534, 582, 635, 695`), builds the procedural grid texture with its mip chain, and uploads the embedded HYG star catalog. Naga translates the WGSL once and the D3D12 backend compiles each pipeline. None of this depends on anything a test varies. It is pure per-test overhead, paid 91 times.

The remaining ~53 s is accounted for by, in order:

| Cost | Where | Seconds |
|---|---|---|
| Real 8K JXL assets, two decodes plus 341 MiB of mip chains | `lowering_the_resolution_lowers_the_process_footprint` (2047) | ~19 of its 20.3 |
| Real 4096 JXL decoded 3× behind 3 engine starts | `the_panorama_follows_the_texture_resolution_cap` (4149) | ~4 of its 7.6 |
| `camera_showing` brute-force search, 5 calls | `the_real_panorama_has_the_galactic_plane…` (3885) | ~2.5 of its 5.0 |
| Unconditional `thread::sleep` | 22 sites | 9.9 |
| Fixture image generation and PNG encoding | 8 moon, 4 surface pairs, 5 panorama | ~3 |
| Everything else (renders, exports, polls) | | ~15 |

The sleep figure decomposes exactly: `a_switch_keeps_the_old_cloud_texture_until_the_new_one_lands` (2417) is 3.263 s = 1.26 s engine + a hard `SETTLE: Duration::from_secs(2)` at line 2436. `a_switch_to_the_current_resolution_does_nothing` is 2.299 = 1.5 s of work + `drained_frame(300)` + `drained_frame(500)`. `unchanged_parameters_do_not_produce_another_frame` (1.992), `disabling_the_preview…` (2.011) and `a_stale_arrival_produces_no_frame_at_all` (2.083) are the same 800 ms of sleep over the same floor.

### 3.2 `camera_showing` is the one gratuitous CPU cost

`engine.rs:3318` searches longitude in 0.5° steps from -180 to 180 and latitude in 0.5° steps from -85 to 85: **244,800 iterations**, each building an `OrbitalCamera`, a view matrix, a `sky_lens_disc`, an `mvp_matrix` and a `globe_screen_circle`. It is called 8 times in the file (once each in three panorama tests, five times in `the_real_panorama_has_the_galactic_plane_where_the_plane_is`), so roughly **2 million camera constructions**. `Cargo.toml` sets `[profile.dev.package."*"] opt-level = 2`, which optimizes dependencies but **not** the workspace crates, so this loop and the `sunlit-core` functions it calls run at `opt-level = 0`. A coarse pass at 4° followed by a fine pass in the winning 8° neighborhood gives the same answer for about 1/50 of the work.

### 3.3 `soak.rs`

It has no `#[ignore]` and no `required-features`, so **yes, it runs in every `cargo test`**, and `cargo unit` (`test --workspace --lib --bins`) is the only alias that skips it. It is designed as 14 simulated days at one step per hour (`STEPS = 14 * 24 = 336`), asserts against itself that the run finishes in under 2 minutes (line 266), and its comment at line 256 records the measurement it was tuned against: "49.8 s at the original 30-minute step, 44.6 s at the hourly step used now, so the margin to the bound is about 2.7x." It is now 65.6 s. The margin is 1.8x, not 2.7x, and the comment is stale.

It obeys the injected clock for the *schedule*: `clock.advance(STEP)` plus `EngineCommand::Poke` per step, never a real wait for a deadline. It does wait on wall time in three other ways: one `thread::sleep(500 ms)` after warm-up (line 179), and two spin loops (`wait_until`, line 148) that poll a condition every 1 ms against an `Instant::now()` deadline of 30 s per step. The spin is bounded by the engine's real progress, not by a fixed duration, so it is honest; the worst case if the engine stalls is 30 s per step × 336 steps, which the 2-minute assertion would never reach because it is checked only at the end. The 65.6 s is 336 × ~195 ms, and the per-step work is one 320×192 wallpaper export render plus, on 112 of the steps, a 2048×1024 JPEG decode and a full mip upload. The preview is disabled, so there is no second render.

### 3.4 What would cut the most for the least loss

| # | Change | Saves | Coverage lost |
|---|---|---|---|
| 1 | Share one engine per configuration group (below) | **~75 s** | none |
| 2 | Halve `soak.rs` `STEPS` to 168 (7 days, 56 publications) | **~33 s** | the growth assertion runs over 56 updates instead of 112; a one-frame-per-update leak is still 450 MiB against a 16 MiB limit |
| 3 | Delete the 2 s `SETTLE` at engine.rs:2436, replace with a poll on `retarget_widths()` | 2 s | none |
| 4 | `drained_frame` 300/500 ms → 150 ms, or make it event-driven | ~5 s | a slower machine could make a negative assertion flaky; measure first |
| 5 | Coarse-to-fine `camera_showing` | ~3 s | none |
| 6 | `soak.rs` `EXPORT_SIZE` 320×192 → 160×96 | ~10 s (unverified) | none; its own comment says the size is arbitrary |
| 7 | `[profile.test] opt-level = 1` for workspace crates | unknown, likely several seconds across every pixel-scanning loop | none; costs compile time |

Together, 1+2+3+5 take the two targets from **227 s to about 110 s** without removing a single assertion.

## 4. Findings

### A. Commentary

**A1 (medium): a comment references a test that does not exist.** `engine.rs:1725`: "`a_stale_decode_arriving_after_a_switch_is_never_applied` is that one." No such test exists anywhere in the repository (grepped). The intended target is `a_stale_decode_must_not_replace_the_texture_that_superseded_it` at line 1780. **Fix the name or drop the sentence.**

**A2 (medium): plan and review references, which the project's own rules forbid.** Classified *move* (commit message or `docs/plans/`):

- `engine.rs:1259-1265` "and it is on purpose: **departure 9 in the plan**."
- `engine.rs:404-412` "which is what **the amendment's criterion 4** asks for and cannot have."
- `engine.rs:1892-1898` "which is the state **Step 3** promises to survive."
- `engine.rs:1229-1237` "`ExportPixels` is the path this feature replaced… what the comparison holds the publish against is the wallpaper **the build before this one** would have written."

**A3 (medium): bug narration.** Six blocks record what was broken and what the user saw. Each states a real property in its first sentence and then tells the story; the story belongs in the commit.

- `engine.rs:1614-1618` "Before the engine resolved it against the adapter, that reached `create_render_textures` and killed the engine thread… the window came up, IPC answered, and no frame ever arrived." **Keep sentence 1, move the rest.**
- `engine.rs:4301-4309` "at the old hardcoded 0.05 the deck comes out darker than it, 13.0 against 42.0… so this fails on the code before this change."
- `engine.rs:4345-4357` "`fs_cloud` **used to** read the same uniform: its ramp became `smoothstep(1.0, -1.0, n_dot_l)`… so clouds were bright at local midnight." The sentinel convention (`-1.0` in `terminator_width` outside blend mode) is a real invariant and is worth keeping; the past tense around it is not.
- `engine.rs:4441-4453` "The **old straight multiply** could not reach the deck's own value… which is what 'one hundred percent still lets it through' meant." Quotes a bug report.
- `engine.rs:539-546` "…which is what **the user saw**."
- `soak.rs:256-264` the nine-line note on 49.8 s vs 44.6 s, which is now wrong (65.6 s). Either update it or move the tuning history to `docs/testing.md`, which already carries the soak measurements.

**A4 (low): the 32-line block at `engine.rs:3974-4005`**, the longest in the file. Roughly half earns its place: the measured ratio range (0.73 to 1.26 across 21 records, Antares the brightest) is what makes `RATIO_BOUND = 2.0` readable, and the paragraph on what the window *cannot* see is an honest statement of the test's limit. The paragraph on why a peak metric was rejected is design history. **Trim to about 12 lines, move the rejected-alternative paragraph to `docs/rendering.md`.**

**A5: comments that earn their place, do not touch.** `engine.rs:8-11` (why `GPU_SERIAL` exists), `134-139` (why `wait_for_slot_texture` polls the memory report: `TexturesReady` deliberately excludes overlays, so there is no event to wait on), `713-720` (the dither and rounding budget, with the per-adapter counts that make `mean < 0.15 && outliers <= 64` readable), `1136-1138` ("Identity rather than equality: two screens of one size are meant to share the buffer, and two equal buffers would not prove they did"), `2068-2071` (why the narrow copies are built before the first snapshot, a trap that would silently invalidate the measurement), `4592-4595` (why `DISPLAY_SETTLE` is spelled locally), `support/mod.rs:66-76` (the mip-level argument for eight cycles) and `78-88` (the inside-the-sphere orientation convention, a real coordinate convention). `common/mod.rs` is clean throughout.

### B. Length and structure

**B1 (high): `engine.rs` at 4765 lines is the longest file in the repository and should be split, but into modules of one target, not into several targets.** Splitting into several integration test targets would be a mistake: each is a separate binary that links `sunlit-core` and `wgpu` with `+crt-static`, so more targets means several more link steps, and the parallel GPU work would put several software rasterizers on the same cores. The gain would be small and the link cost certain.

The right shape is **one target, many modules**, which cargo supports as `tests/<name>/main.rs` plus plain sibling files (a directory without `main.rs` is not a target, which is exactly why `support/mod.rs` and `common/mod.rs` are shaped that way today). Proposed split, with current line ranges:

| New file | Contents | Lines |
|---|---|---|
| `tests/engine/main.rs` | module doc, `mod` declarations, `#[path = "../support/mod.rs"] mod support;` | ~30 |
| `harness.rs` | 13-173 `GPU_SERIAL`, `TIMEOUT`, `test_params`, `Harness` and **both** its `impl` blocks (the second is stranded at 4528-4567), `has_lit_pixels` | ~230 |
| `sinks.rs` | 977-1236 `RefusingSink`, `screen`, `RecordingSink`, `Publication`, `publish_plan*`, `publish_once`, `two_screens` | ~260 |
| `stars.rs` | 175-270, 3 tests | ~95 |
| `sun.rs` | 272-733, 10 tests | ~460 |
| `lifecycle.rs` | 792-983, preview and export, 10 tests | ~190 |
| `display_plan.rs` | 1229-1590 plus `picture`/`compare`/`globe`, 10 tests | ~360 |
| `textures.rs` | 729-790 `TextureFixtures` + 1591-1985, 11 tests | ~460 |
| `memory.rs` | 1982-2131 + 2457-2634, 6 tests | ~330 |
| `clouds_variant.rs` | 2132-2456 `VariantCloud`, 3 tests | ~325 |
| `moon.rs` | 2635-3305, 8 tests | ~670 |
| `panorama.rs` | 3306-4210, 7 tests | ~900 |
| `clouds.rs` | 4211-4501, 4 tests | ~290 |
| `display_change.rs` | 4503-4765, 6 tests | ~260 |

`panorama.rs` and `moon.rs` would still be large, but they are large because the placement mathematics they share (`camera_showing`, `eqj_direction`, `eqj_screen_position`, `globe_circle`, `moon_placement`, `disc_pixels`) is needed by every case in the group. That is a good reason to be long.

**B2 (low): six functions over 80 lines in `engine.rs`.** `the_wrap_column_is_not_a_band_of_the_coarsest_mip` (3665, 102), `no_bright_star_is_baked_into_the_real_panorama` (4007, 102), `a_moon_over_the_sun_fades_the_glare_around_it` (3143, 99), `the_panorama_tracks_the_sky_field_of_view` (3775, 94), `the_real_panorama_has_the_galactic_plane` (3885, 88), `lowering_the_resolution_lowers_the_process_footprint` (2047, 84). All six are one scenario each, and in a test that is acceptable; the setup-heavy ones would read better with the framing choice extracted, but there is no bug here. `soak.rs`'s single 169-line test with `#[allow(clippy::too_many_lines)]` is the same situation.

**B3 (low): the second `impl Harness` block at engine.rs:4528.** `hint`, `advance`, `next_layout` and `wait_for_publish` were bolted onto `Harness` 4400 lines below the type. `wait_for_publish` is the general helper seven other tests hand-roll (see D1). Move the whole block up to the type.

### C. Consistency

**C1 (medium): two scratch-directory idioms, one of which leaks on failure.** `TextureFixtures` (engine.rs:786) and `PanoramaHarness` (engine.rs:3473) clean up in `Drop`. Twelve other tests end with a trailing `let _ = std::fs::remove_dir_all(&dir);` (22 occurrences), which is skipped when an assertion panics, so a failing run leaves directories in `CARGO_TARGET_TMPDIR`. **Give the moon and cloud groups the same RAII guard the other two groups already have.**

**C2 (low): two temp-directory roots.** `render_to_file_writes_a_png_at_the_requested_size` (914) and `render_to_file_works_with_the_preview_disabled` (938) write into `std::env::temp_dir()`; every other test in the file uses `env!("CARGO_TARGET_TMPDIR")` (18 sites). The first pollutes the user's temp directory and is not cleaned by `cargo clean`.

**C3 (low): `support` and `common` are two generic names for two unrelated things.** `common/mod.rs` is GPU plumbing (`GpuContext`, the shared `GPU` `LazyLock`, `read_buffer`) used by `shading.rs` and `render_pipeline.rs`. `support/mod.rs` is image fixture generation (moon, panorama, surface, cloud source) used by `engine.rs` and `golden.rs`. They do not overlap in content at all, and the split is by consumer group rather than by accident, so keeping two modules is right; the names are not. **Rename to `gpu_context/mod.rs` and `fixtures/mod.rs`** (note `tests/fixtures/` already exists as a data directory, so pick `image_fixtures` or move the data). They also silence dead code differently: `support/mod.rs` has a file-level `#![allow(dead_code)]`, `common/mod.rs` has four per-item `#[allow(dead_code)]`. Pick one.

**C4 (low): `CountingSink` and `MockClock` are `pub` items in `sunlit-core`'s library that no non-test code uses.** Grepped: `CountingSink` appears only in `wallpaper_sink.rs` itself, `tests/engine.rs` and `tests/soak.rs`; `MockClock` only in `clock.rs`, `tests/engine.rs` and `tests/soak.rs`. They are test doubles shipped in the public API of the library. Integration tests cannot see `#[cfg(test)]` items, so this is a real constraint, not a mistake, but a `#[doc(hidden)]` or a `testing` feature would keep them out of the published surface.

**C5 (low): `DISPLAY_SETTLE` is copied.** `engine.rs:4596` redeclares `Duration::from_secs(2)` to match the private `DISPLAY_SETTLE` in `src/engine/mod.rs:62`. The comment argues this is not pinning a constant because the cases step *over* it, but `a_hint_during_the_settle_moves_the_deadline_it_found` (4654) advances 1500 ms, then 1000, then 1000, which is only correct while the engine's settle is between 1.5 s and 2.5 s. If the engine's value changes, that test fails silently in the wrong way. **Make the engine's constant `pub` (or expose it through a `#[doc(hidden)]` accessor) so the two cannot drift.**

### D. Duplication

**D1 (high): the "drain events until `WallpaperSet`" loop is written seven times.** `engine.rs:966, 1211, 1511, 1535, 1573, 1965, 4560`. The seventh is `Harness::wait_for_publish` (4557), the general version, added last and used only by the display-change section. **Use it everywhere; that deletes six loops and about 60 lines.**

**D2 (medium): `memory_counters_supported`, `private_bytes` and `mib` are duplicated between `engine.rs:1987-1998` and `soak.rs:126-145`.** The doc comment on the `engine.rs` copy even says "and the same helper in the soak test". These three belong in `support/mod.rs`, which both binaries can include.

**D3 (medium): the LFS-pointer size check is written twice inside `engine.rs`.** `real_textures` (2006) and `real_panorama` (4111) each declare `const MIN_BYTES: u64 = 64 * 1024` and each implement the same "metadata length distinguishes a pointer from the asset" rule with different reporting. One `fn real_asset(name: &str) -> Result<PathBuf, String>` covers both.

**D4 (low): `great_circle_degrees` is written twice in `support/mod.rs`.** Once inline inside `write_moon_fixture` (lines 48-53, with a copy of the free function's doc comment as a `//` comment) and once as the free function at line 265 used by `write_surface_fixtures`. Identical arithmetic. Call the function.

**D5 (low): three `CloudSource` fixtures.** `support::FixtureClouds`, `soak::FixtureCloud`, `engine::VariantCloud`. Each reimplements the same etag-compare-then-serve logic. `soak::FixtureCloud` differs only by a version counter that `FixtureClouds` could take; `VariantCloud` needs `retarget`. Merging the first two is worth about 40 lines.

**D6 (low): `textures_ready_fires_for_the_procedural_grid` (2618) is `Harness::wait_for_textures` inlined.** It is the same loop over the same event with the same panic, and it is already exercised implicitly by every `blend_harness` test's `wait_for_textures("at startup")`. It is the cheapest test in the file at 1.189 s, but it is 1.189 s for an assertion the suite already makes 12 times.

### E. Correctness and resilience

**E1 (medium, confirmed): `re_enabling_the_preview_resends_the_current_frame_unchanged` does not test its name.** engine.rs:893-910. It disables the preview, re-enables it, takes one frame and asserts `has_lit_pixels`. It never captures the frame from before the disable, so it cannot and does not check that the resent frame is *unchanged*. The property in the name (the frame comes from the texture that is already there rather than from a re-render the dirty check would suppress) is the interesting one and is untested. **Capture the pre-disable frame and `assert_eq!` the buffers.**

**E2 (low, confirmed): `Harness` drop order is correct and load-bearing, and nothing says so.** The fields are declared `engine, events, _guard` (65-69), so `EngineHandle::drop` (which sends `Shutdown` and joins the thread, `src/engine/mod.rs:337`) runs before the `MutexGuard` is released. Reordering the fields would release `GPU_SERIAL` while a device is still alive, which is the exact crash the lock exists to prevent. This is the rare case where a one-line comment earns its place; there is none.

**E3 (low, confirmed): `wait_for_slot_texture` uses a query for its side effect.** engine.rs:140-154 polls `engine.memory_report()` every 20 ms, and the doc on `wait_for_cloud_size` (2250) states the reason plainly: "Polling the report is also what keeps the engine ticking." A test that drives the engine by asking it questions is fragile: if `ReportMemory` ever stopped implying a tick, four groups of tests would hang for 60 s each. **Send `EngineCommand::Poke` explicitly in the loop, or add an `EngineEvent` for an overlay slot arriving.**

**E4 (low): the worst case on a hung engine is 78 minutes.** `TIMEOUT = Duration::from_mins(1)` (engine.rs:43) and there are 15 real-time deadline sites. A systematic failure (a broken shader, a device that will not come up) makes every test wait the full minute in series. 20 s would be ten times the slowest observed wait and still generous.

**E5 (low): the event channel is unbounded and each `PreviewFrame` carries a 512 KiB `Vec`.** engine.rs:73-79. Tests that do not drain accumulate them. `lowering_the_resolution_lowers_the_process_footprint` measures `private_bytes` and would be corrupted by this, but it calls `drained_frame(500 ms)` before each of its two snapshots (2089, 2100), so it is safe as written. Worth a comment at the drain sites, since deleting one would silently break the measurement.

**E6: verified not a problem.** No test holds two `Harness` values at once, so `gpu_lock()` (a non-reentrant `std::sync::Mutex`) cannot deadlock. `publish_one_screen` returns a live `Harness`, but its only caller binds and drops it inside one loop iteration (1240). `a_switched_off_moon_and_a_missing_texture_draw_the_same_frame` (2730) creates its two engines in separate blocks. `gpu_lock` recovers from poisoning with `into_inner`, so a panicking test does not cascade.

### F. The tests themselves

78 tests. I would **merge or delete 21**, ending at 57, and would cut engine starts from 91 to about 28. Nothing below removes a property that is asserted anywhere else.

**F1 (high): the sun family creates 12 engines to render 24 frames.** `sun_off_and_on_at` (engine.rs:311) starts a fresh engine for every `(params, glow)` pair, and it is invoked 12 times at run time: 1 each in `a_sun_behind_the_painted_globe_paints_nothing`, `zero_sun_glow_takes_the_sun_out_of_the_frame`, `a_sliver_of_sun_over_the_limb_still_glares`, `an_enlarged_disk_reaching_the_frame_corner_is_drawn`; 2 each in `a_sun_grazing_the_limb_turns_the_glare_warm`, `the_glare_peaks_as_the_disk_clears_the_horizon`, `the_physical_exposure_has_no_peak`, `a_pan_past_the_frame_corner_still_draws_the_sun`. Every one of them differs only in `SceneParams`, which `UpdateParams` sets on a running engine. `sunrise_excess` (515) is the same shape and adds 3 more. **Change `sun_off_and_on_at` and `sunrise_excess` to take `&Harness`, hold one shared engine for the group: 15 starts become 1, saving ~17 s.** No assertion changes.

**F2 (high): the display-plan family creates 16 engines for 10 tests, and one of its own tests proves it does not need to.** `a_new_display_plan_changes_the_next_publish` (1523) asserts that "the plan is re-read on every publish rather than cached at startup", and `RecordingSink::set_monitors` (1094) already moves the monitor list under a running engine (the display-change section relies on it). So mode, anchor and monitor list are all mutable on a live engine. `publish_plan`, `publish_plan_with` and `publish_one_screen` should take a `&Harness` and a `&RecordingSink` and drive them with `SetDisplayPlan` + `set_monitors`. **16 starts become 2 or 3, saving ~15 s.** Specifically:
- `one_monitor_is_one_image_at_its_own_size_in_every_mode` (3 engines, 3.718 s) becomes one engine and one loop.
- `every_screen_renders_one_image_per_distinct_size` and `one_screen_paints_the_anchor_and_leaves_the_others_alone` (2 each) become one publish each on the shared engine.
- `the_anchors_crop_of_a_span…` and `a_taller_canvas…` (2 each) publish twice on one engine.

**F3 (medium): merge candidates that assert the same thing with a small variation:**

| Merge | Into | Why |
|---|---|---|
| `an_unsupported_sample_count_arriving_later_still_renders` (1635) | `an_unsupported_sample_count_still_renders` (1613) | Same property at startup vs at run time; the second can send `UpdateParams` on the first's engine. Saves 1 start. |
| `re_enabling_the_preview_resends_the_current_frame_unchanged` (893) and `enabling_the_preview_before_any_frame_exists_still_delivers_one` (880) | `disabling_the_preview_stops_frames_without_stopping_the_engine` (856) | Three states of the same owed-frame debt. One engine can cover: start disabled → enable → frame; disable → no frames; change params while off → still no frames; enable → the same frame back (fixing E1). Saves 2 starts. |
| `render_to_file_works_with_the_preview_disabled` (937) | `render_to_file_writes_a_png_at_the_requested_size` (913) | Identical but for the size and `preview_enabled`, and the second asserts strictly less (no lit-pixel check). `SetPreviewEnabled(false)` then a second export. Saves 1 start. |
| `the_measured_and_computed_texture_totals_agree` (2513) | `the_report_names_the_textures_the_renderer_owns` (2472) | Byte-identical setup (`blend_harness`, `wait_for_textures`, `next_frame`) and both read one report. Saves 1 start. |
| `the_allocator_section_reports_reserved_at_least_as_large_as_allocated` (2590) | the same shared engine | Needs no textures at all. Saves 1 start. |
| `every_hint_of_one_burst_collapses_into_a_single_query` (4628) | `a_hint_about_a_layout_that_did_not_change…` (4604) | Both assert `sink.queries() == before + 1`; one hint vs five. Two phases on one engine. Saves 1 start and 0.4 s. |
| `the_lit_limb_faces_the_sun` (3081) | `the_lit_fraction_tracks_the_ephemeris_at_three_phases` (3012) | Same fixture, same instant (day 199, hour 16), same 1600×800 viewport, same `moon_earthshine = 0.0`. The second's frame *is* the first's first frame. Compute the centroid from the pixels already exported. Saves 1 start and one 1.28 Mpx software render. |
| `a_night_side_cloud_is_brighter_than_the_land_under_it` (4311) and `a_dayside_cloud_is_brighter_than_a_night_side_one_in_every_mode` (4359) | one test | Identical `cloud_harness(&dir, cloud_case_params(3, 180.0, NIGHT_HOUR))`. Saves 1 start and one surface-fixture generation (two 1024×512 PNGs). |
| `a_switch_to_the_current_resolution_does_nothing` (1703) and `switches_in_quick_succession_end_on_the_last_one` (1728) | `a_resolution_switch_reloads_the_textures_in_both_directions` (1669) | Same `blend_harness` at the same width; three sequential phases on one engine. Saves 2 starts and 1.3 s of `drained_frame`. |
| `the_moon_texture_lands_in_its_own_slot_without_delaying_readiness` (2829) | `a_moon_on_the_night_sky_only_adds_light` (2674) | The first's only unique assertion is `expected_widths(report, "moon_texture") == [MOON_FIXTURE_WIDTH]`, one line; the second already calls `wait_for_slot_texture("moon_texture")`. Saves 1 start. |
| `textures_ready_fires_for_the_procedural_grid` (2619) | delete | D6: it is `wait_for_textures` inlined, and twelve tests already depend on it. |

**F4 (low): tests that pin a constant rather than a behavior.** The suite is unusually disciplined here; I found three, and two are defensible.
- `engine_renders_a_first_preview_frame` (793) asserts `(512, 288)` quantizes to `(512, 256)`, and `preview_size_changes_are_quantized_and_applied` (844) asserts `(300, 200)` quantizes to `(256, 192)`. Both pin the 64-pixel quantization. The second is the test for that behavior; the first should assert only that a lit frame of the right buffer length arrives.
- `the_report_names_the_textures_the_renderer_owns` (2504) asserts `expected_widths(report, "render_texture") == [512]`, pinning the same quantization a third time. It is a reasonable cross-check that the report reflects the render target, but derive the expected value from the configured preview size rather than writing `512`.
- `the_panorama_follows_the_texture_resolution_cap` (4188) asserts `(4096, 2048)` for the shipped asset. That is a property of the file rather than of the code, and it is asserted on purpose (the point is that 8192 is a cap and the file is smaller), so it earns its place; but if `milkyway_2020_4k.jxl` is ever re-baked, the failure will be confusing. A comment naming the asset version would help.
- `an_opacity_at_zero_switches_off_only_its_own_hemisphere` and `the_night_opacity_reaches_full_cover` deserve credit for the opposite: they derive `deck` from `base.cloud_night` and take the mid reading from `SceneParams::default().cloud_opacity_night`, so changing either default moves the expectation with it. That is exactly the convention `docs/testing.md` asks for.

**F5 (low): one test in the GPU binary needs no GPU.** `no_bright_star_is_baked_into_the_real_panorama` (4007) decodes the shipped JXL and scans texel windows; it starts no engine and takes no lock. It is correctly placed in the sense that it needs `support::panorama_texel` and the star catalog, but it is a unit test of an asset sitting in the slowest binary in the workspace. Low priority.

### G. Best practices worth mentioning

**G1 (medium): `camera_showing` (3318).** See §3.2. A 244,800-point linear scan called 8 times. Coarse-to-fine, or a closed-form solve, for the same answer.

**G2 (low): `the_real_panorama_has_the_galactic_plane_where_the_plane_is` (3937-3947)** calls `globe_circle(&params, viewport)` twice inside one `assert!`, once for `.center` and once for `.radius`. Bind it once.

**G3 (low): `RecordingSink::publish` (1125)** collects raw pointers into a `Vec<*const u8>` to count distinct buffers. That is the right idea (identity, not equality, as the comment says) but `Vec::contains` is O(n²) in the monitor count; with at most four screens it does not matter, and the comment explains the intent, so this is fine as written. Noted only so a future reader does not "fix" it into equality.

**G4 (low): 20 `export_pixels` call sites repeat the same `.expect("the engine should be able to export")` (17 identical strings).** A `Harness::export(width, height) -> Vec<u8>` would remove them and give the panorama and cloud groups a common shape they currently each reinvent (`PanoramaHarness::export`, the cloud `read` closures).

## 5. Recommended refactors, by value over effort

| # | Refactor | Effort | Risk | Value |
|---|---|---|---|---|
| 1 | Share one engine across the sun/star group (F1) and the display-plan group (F2), following the `static ENGINE: LazyLock<Mutex<..>>` pattern already in `golden.rs:96` and `common::GPU`. Add a `Harness::reset()` that re-sends `test_params`, `SetPreviewEnabled(true)` and the default resolution. | 4-6 h | Low. `SceneParams` is sent whole, not as a delta, so state does not leak; `gpu_lock` already survives poisoning. | **~32 s** |
| 2 | Halve `soak.rs` `STEPS` to 168 and update the stale measurement comment at line 256. | 30 min | Low. The growth assertion still runs over 56 cloud updates. | **~33 s** |
| 3 | Apply the same sharing to the memory-report, resolution, cloud, moon and preview groups (F3). | 4-6 h | Low-medium. `a_switch_while_the_first_load_is_running`, the two mailbox-injection tests and the LFS-asset tests must keep their own engines. | **~15 s** |
| 4 | Delete the 2 s `SETTLE` at engine.rs:2436; poll `source.retarget_widths()` instead. | 30 min | Low | 2 s |
| 5 | Split the file into `tests/engine/main.rs` plus the 13 modules in B1; move the stranded `impl Harness` up; use `wait_for_publish` in all seven places (D1). | 3-4 h | Low. Pure mechanical move, but confirm `cargo test --test engine` still resolves the target and `#[path = "../support/mod.rs"]` works before committing. | readability; no time |
| 6 | Comment pass: fix the dangling test name (A1), strip the four plan references (A2), move the six bug narrations to `docs/` or leave them in git history (A3), trim the 32-line block (A4). | 2-3 h | None | the maintainer's stated concern |
| 7 | Coarse-to-fine `camera_showing` (G1). | 1-2 h | Low; assert the chosen framing still clears the globe by the same margin. | ~3 s |
| 8 | Fix `re_enabling_the_preview_resends_the_current_frame_unchanged` to compare buffers (E1); RAII cleanup for the moon and cloud scratch dirs (C1); `CARGO_TARGET_TMPDIR` for the two `render_to_file` tests (C2). | 1-2 h | None | correctness |
| 9 | Deduplicate `memory_counters_supported`/`private_bytes`/`mib` into `support` (D2); one `real_asset` helper (D3); call `great_circle_degrees` in `write_moon_fixture` (D4). | 1 h | None | ~90 lines |
| 10 | Lower `TIMEOUT` to 20 s (E4); make the engine's `DISPLAY_SETTLE` visible to the tests (C5); rename `common`/`support` (C3). | 1 h | Low | fail-fast, drift |
| 11 | Try `[profile.test] opt-level = 1` for the workspace crates and measure. | 30 min + a run | Low; costs compile time. | unknown, possibly several seconds |

Items 1, 2, 3 and 4 alone take `engine.rs` + `soak.rs` from **227 s to roughly 110 s** and delete no assertion.
