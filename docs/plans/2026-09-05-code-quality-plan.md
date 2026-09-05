# Plan: Code quality implementation (2026-09-05)

## Summary

This plan turns the findings of [reviews/2026-09-04-code-quality-review.md](../reviews/2026-09-04-code-quality-review.md) into work that background agents can carry out without stepping on each other and without running the account out of its five-hour usage window. The review is the research document; this plan is the contract. Its section 5 lists the items and their order, and its notes under `reviews/2026-09-04-code-quality/` carry the line-level detail. Nothing here re-decides what the review found; it says who does what, where, in which order, and how done is proven.

The work is cut into five runs. Each run is one orchestrated session with three implementers working in parallel on disjoint sets of files, one validator round per implementer, and one pull request at the end. Runs 1 to 3 are the release-relevant half: the startup crashes, the public surface, the wrong comments, the test suite's cost and value, and the comment pass. Runs 4 and 5 are the file splits and the shared helpers; they are not release blockers and may be scheduled after the release.

## Stakes

- The app panics at startup on a missing adapter, an invalid or occupied `--ipc-socket` name, a missing tray host, and three `PlatformError`s. Each of these reaches a user as a crash rather than a message.
- The `Uniforms` layout that the shaders depend on is checked against a hand-written copy, not the real struct.
- `cargo test` spends 227 of its 235 seconds of test time in two integration targets whose cost is an avoidable engine start per test.
- About a thousand comment lines, about 90 crate-internal `pub` items, 33 tests that pin defaults against the project's own rule, and about 210 redundant tests make the code slower to read than it needs to be before a release invites outside readers.

## Constraints that shape the plan

**Budget.** The account is a Max plan with a rolling five-hour window shared with other agents. The review itself, ten parallel Opus readers plus synthesis, moved the window from 38 to 75 percent. An implementer reads a similar volume and then edits, builds and re-reads, so one is assumed to cost two to three times a reader. That fixes the shape: three implementers per run, one run per window, and a run started only when less than half the window is used. Run 1 measures the real cost and the plan is re-sized from it.

**Conflicts.** Three agents editing one tree at once collide unless their files are disjoint. Every package below names the paths its agent may edit, and two packages in one run never name the same file. Files that everyone would want (`docs/`, `CLAUDE.md`, `README.md`, the two `lib.rs`, every `Cargo.toml`) are reserved to the orchestrator; agents describe the change they need in their handover and the orchestrator applies it once. Where a signature change in one package has callers in another, the plan says which agent lands it first and the other merges the run branch before touching the call site.

**Build cache.** A fresh worktree costs a cold build of 5m22s for `cargo test --workspace --no-run` on the development host, and copying the main checkout's `target/` into a worktree did not help: cargo asked for 252 fingerprint files the copy did not contain, and even leaf crates such as `cfg-if` were assigned a hash the main checkout has never produced, with no mtime staleness involved. Whether that is inherent to the path or environmental is unresolved, and the plan does not depend on it. Instead the worktrees are a persistent pool: four worktrees under `.worktrees/`, built once, never moved, never recreated. An agent checks its branch out inside a pool member and only the workspace crates rebuild. A shared `CARGO_TARGET_DIR` is ruled out by cargo issue 12516, where path crates at the same relative path collide, which is this workspace's shape. `sccache` keys on the working directory and never caches linking, so it does not help across worktrees.

**One GPU.** Every `cargo test` in flight creates a device. Three concurrent runs on one adapter have not been observed to fail, but the shading flake noted in the roadmap makes a fourth unwise, and the orchestrator's own full run in the integration worktree is the authoritative one.

## Decisions

1. Orchestrator, implementers and validator run on Opus. Sonnet may replace Opus for the run 3 deletions if run 1 shows the budget is tight; the validator stays on Opus.
2. Three implementers per run, two in run 5. Never more.
3. One validator round per package, on the package branch, as soon as its implementer declares done. Findings go back to the same implementer, which still holds the files, through a message. There is no second validator round; the orchestrator checks the listed fixes at the diff level and runs the gates on the merged run branch.
4. The worktree pool is `.worktrees/pool-0` (integration: merges, doc edits, the final gate run) and `.worktrees/pool-1` to `pool-3` (one per implementer). They are created once from `main`, warmed once with `cargo test --workspace --no-run`, and reused for every run. The main checkout is the maintainer's working copy and no agent touches it.
5. One run branch per run, `refactor/quality-run-<n>`, cut from `main`. One package branch per package, `refactor/quality-run-<n>-<package>`, cut from the run branch and merged back by the orchestrator. One draft pull request per run.
6. Test doc comments that explain why a test exists are deleted, keeping only those that explain a non-obvious setup mechanism. The maintainer's rule in `CLAUDE.md` already says so; the review found the code disagreeing with the rule, and the rule wins.
7. Runs happen in the order written. Run 2 precedes run 3 because deleting a test deletes its comments for free; run 3 precedes run 4 because fewer lines then move in the splits.
8. Runs 4 and 5 may be scheduled after the release. Runs 1 to 3 come before it.

## The pool

Created once, by the orchestrator, from the main checkout:

```
git worktree add --detach .worktrees/pool-0 main
git worktree add --detach .worktrees/pool-1 main
git worktree add --detach .worktrees/pool-2 main
git worktree add --detach .worktrees/pool-3 main
```

then `cargo test --workspace --no-run` in each, which may run in parallel and is paid once. Before a run, each member is brought to the run branch with `git checkout -B <package branch> <run branch>` inside it. After a run, each member is returned to `git checkout --detach main`. A pool member's `target/` grows across branches the way the main checkout's does; `cargo xtask sweep` inside it, run by the orchestrator between runs, is the remedy. Members are never deleted and never moved; a member that is deleted costs a cold build to replace.

## Ownership and reserved files

Reserved to the orchestrator in every run: `docs/**`, `CLAUDE.md`, `README.md`, `crates/*/README.md`, `crates/sunlit-core/src/lib.rs`, `crates/sunlit-app/src/lib.rs`, every `Cargo.toml` and `Cargo.lock`, `.cargo/`, `.github/`, `vm/`, `assets/`, `textures/`, `crates/xtask/**`. An agent may edit the `mod` line of a module it owns in its crate's `lib.rs` and nothing else there. Everything an agent needs changed in a reserved file goes into its handover under a "Reserved files" heading, as exact text, and the orchestrator applies it in `pool-0` before the final gate run.

Never done by an agent, in any run: `cargo e2e`, any `cargo xtask` command, anything that boots a VM, anything that sets the real wallpaper, `SUNLIT_EARTH_UPDATE_GOLDEN`, `git push`, `git rebase` or any history rewrite, creating or editing pull requests, deleting or creating worktrees, touching another pool member or the main checkout. Regenerating a golden image is forbidden; a golden that no longer passes is a finding to report, not a file to update.

## Gates

Every run, in the integration worktree, on the merged run branch:

- `cargo fmt --check` clean.
- `cargo clippy --all-targets` with zero warnings.
- `cargo test` green, the whole workspace.
- `git status` shows nothing under `crates/sunlit-core/tests/golden/`; the golden test passing on the software adapter is the pixel check.
- `git diff main --stat` touches only the paths the run's packages own plus the reserved-file edits the orchestrator applied.

Per run, in addition:

| Run | Gate |
|---|---|
| 1 | `cargo run -- render --output <tmp>/r.png --width 640 --height 360` succeeds. The maintainer starts the app once with an invalid `--ipc-socket` name and once normally and sees a message rather than a panic. The e2e suite runs once in the Windows guest, by the maintainer or by the orchestrator with explicit permission. |
| 2 | libtest's `finished in` line for `cargo test -p sunlit-core --test engine` is under 60 s (from 161.6) and for `--test soak` under 35 s (from 65.6) on the development host. Every test removed or merged is listed in the package's handover with the test that now covers its assertion. The e2e target compiles and passes once in the Windows guest. |
| 3 | For packages 3.1 and 3.2 the diff contains only comment lines and blank lines; the validator checks with `git diff -U0` and a sample of hunks. Measurements moved out of comments land in `docs/testing.md` and `docs/rendering.md` through the orchestrator. |
| 4 | `git diff --color-moved=dimmed-zebra` shows moved code, not rewritten code, in every split. The public API of `sunlit-core` as seen from `sunlit-app` is unchanged except where a package lists the change. |
| 5 | As run 4. The e2e suite runs once in the Windows guest after `main.rs` moves into the library. |

## Budget rules

The orchestrator reads `~/.claude/rate-limits.json` at kickoff and at every check-in, every 20 minutes.

- A run starts only when `five_hour.used_percentage` is below 50.
- Above 80, no new agent is spawned: no validator, no successor, no fixer. Running agents finish their current step.
- At 90, the controlled pause from the orchestrate skill: every agent writes its handover and stops, the session handover goes into the run directory, a one-shot resumption is scheduled a few minutes after `resets_at`, and the maintainer is told at what percentage the run paused and when it resumes.
- After every run the orchestrator writes the measured window share into the budget record below. If run 1 exceeds 50 percent, runs 2 to 5 shrink: two implementers instead of three, or a run split into two sessions, decided and recorded here before the next run starts.
- Check-in interval halves above 75 percent.

## How a run works

1. Kickoff. Confirm the plan is committed and the review's pull request is merged. Confirm the pool exists and is warm (`cargo test --workspace --no-run` in `pool-0` finishes in seconds). Read the budget. Create the run branch from `main`. Create the run directory in the orchestrator's scratchpad with `status.md`. Echo the kickoff summary to the maintainer: packages, agents, gates, budget reading.
2. Spawn. For each package, check the package branch out in its pool member, write the agent brief (section below) with absolute paths, and spawn the implementer. All three start together.
3. Supervise. Every 20 minutes read `status.md` and the budget; post a short progress line to the maintainer; correct course by message. Above 60 percent of an agent's own context, or when its handover rule fires, let it finish the step and write its handover, then spawn a successor from the plan, the handover and the status log.
4. Validate. When an implementer declares done with green gates in its worktree, spawn one validator on its branch with the package's item list and acceptance criteria. Relay MAJOR and MINOR findings to the implementer by message. When the implementer reports the fixes, check them at the diff level; no second validator. MINOR findings the implementer declines are recorded under Declined findings with the reason.
5. Merge. In `pool-0`, merge each finished package branch into the run branch. Apply the reserved-file edits from the handovers. Add the run's line to `docs/roadmap.md` and update whichever of `docs/architecture.md`, `docs/testing.md`, `docs/rendering.md` the handovers name. Run the gates. Commit.
6. Wrap up. Push the run branch, open a draft pull request whose description says what shipped and what was measured, append the validation record and the budget record to this plan on the run branch, return the pool members to detached `main`, confirm nothing is still running, and write the session handover into the run directory. Report to the maintainer: what shipped, measurements, validation history, declined findings, open items, the handover's path.

## Agent brief

Every implementer receives, as absolute paths: this plan and its package id; the review and the notes for its area; its pool worktree and branch; the run's `status.md` and its handover file path. It is told:

- Read this plan's package section, then the review's sections the package cites, then the area notes in full. The notes' line numbers were correct at commit `bdc2918` and drift as soon as anyone edits; find items by symbol name, not by line.
- Edit only the paths the package names. Keep the workspace compiling and `cargo unit` green at every commit. Run `cargo clippy --all-targets` and the integration targets you touched before declaring done, then one full `cargo test`.
- Commit on the package branch in reasonable chunks with an imperative, sentence-case subject in the repository's style. No `Co-Authored-By` or other trailers. Do not push.
- Follow `CLAUDE.md`: comments only where the code cannot be made clear without one, no history or ticket references in code, `approx` for floats, no test that pins a default or preset, the `SAFETY` pattern for `unsafe`, LF endings, clippy pedantic clean.
- Append to `status.md` at every milestone, blocker and surprise, one line each: `[HH:MM] <step> | done|in-progress|blocked | ctx <NN>% | <substance>`. Never rewrite earlier lines.
- When reality disagrees with the plan, do the right thing and record a numbered departure in the handover for the orchestrator to copy into this document. Do not silently diverge and do not blindly comply.
- Write the handover before stopping for any reason: Done with evidence, In flight, Departures, Next steps in order, Traps, Reserved files, and for run 2 the list of removed or merged tests.

Every validator receives the plan, the package id, the branch and the commit range, and is told: fresh context, read the package's items and acceptance criteria, verify each against the diff and by running the gates in the package's worktree, classify findings MAJOR (wrong behavior, broken contract, plan item not met) or MINOR (style, documentation, small hazard), write them to the run directory, and change no code.

## Runs

The item numbers refer to the review's section 5. Estimates are the review's, for a person who knows the code; an agent's wall time differs and only the budget matters.

### Run 1: resilience and surface

Review items A1 to A5. Branch `refactor/quality-run-1`.

**Package 1.1, engine side.** Paths: `crates/sunlit-core/src/engine/**`, `src/wgpu_init.rs`, `src/display/**`, `src/display.rs`, `src/desktop.rs`, `src/wallpaper.rs`, `src/memory.rs`, `src/memory_report.rs`; the `read_texture_rgba8` function in `src/renderer/mod.rs` and nothing else in that file; the call sites of `engine::start` and `read_texture_rgba8` wherever they are, including `crates/sunlit-core/tests/**` and `crates/sunlit-app/src/main.rs`, and nothing else in those files. Notes: engine.md, display.md, config-memory.md.

- `wgpu_init::init` and `engine::start` return `Result`; no adapter, no device and a failed engine start become an error the caller reports. `read_texture_rgba8` returns `Result` and the export path reports instead of unwrapping (A1, review 4.6).
- Land those signatures and their call-site adaptations as the first commit and say so in `status.md`; the orchestrator merges that commit into the run branch and tells 1.3, which merges before touching the startup path.
- `check_supported` runs only when a publish is due, not every 50 ms while textures are pending; `RenderWallpaperNow` coalesces; `into_inner` on the wallpaper lock; the missing `SAFETY` comment at the `watch.rs` `SendMessageW` site (A5).
- Narrow every crate-internal `pub` item in these paths to `pub(crate)` or private, delete the dead ones, `#[doc(hidden)]` on the test doubles (A3, review 4.3, the notes' visibility lists).
- Fix the wrong or misattached comments in these paths from the review's section 4.1 table (A4). Documentation drifts the table names go into the handover.

Acceptance: `cargo run` with no usable adapter, simulated by the implementer if it can be, prints a message and exits nonzero; the engine tests still pass with the new `start` signature; the visibility list for these paths in the notes is exhausted or each remaining item has a stated reason.

**Package 1.2, renderer side.** Paths: `crates/sunlit-core/src/renderer/**` except `read_texture_rgba8`, `crates/sunlit-core/shaders/**`, `src/scene/**`, `src/assets/**`, `src/geometry/**`, `src/config.rs`, `src/params.rs`, `crates/sunlit-core/tests/render_pipeline.rs`. Notes: renderer.md, scene.md, assets.md, config-memory.md.

- `renderer::uniforms::Uniforms` becomes `pub`; `tests/render_pipeline.rs` imports it and the Rust copy, the WGSL copy and the `generate_uv_sphere` copy are deleted; the offset test then guards production (A2).
- The `render_pass.rs` unwraps on device loss become errors (A1, review 4.6).
- `decode_cloud_jpeg` goes through `orient` so the 8K decode peaks 100 MB lower; `transit_fraction` is deleted (A5).
- Visibility narrowing and dead-code deletion in these paths (A3).
- The wrong comments in these paths, including the reversed coordinate frame doc in `scene/sun.rs` and the inverted sentence in `scene/datetime.rs` (A4).

Acceptance: the offset test fails when a field is added to `Uniforms` without its WGSL counterpart, demonstrated once and reverted; the golden test passes unchanged; the shading and render_pipeline targets pass.

**Package 1.3, the app.** Paths: `crates/sunlit-app/src/**` except the engine start call in `main.rs` until the run branch carries 1.1's first commit, `crates/sunlit-app/ui/**`, `crates/sunlit-app/build.rs`, `crates/sunlit-app/tests/slint_ui.rs`. Notes: app.md, tests-app.md.

- `ipc.rs` and `tray.rs` stop panicking on an invalid or occupied `--ipc-socket` name: the listener failure is a warning and the app runs without IPC; the single-instance mutex failure is a warning and the app assumes it is alone. Tray creation and show failures are a warning and the app runs windowed. The three `PlatformError` `expect`s in `main.rs` become reported errors (A1).
- The auto-refresh interval slider no longer saves the config on every tick; the save moves to the checkbox, the button and window close, which already save (A5).
- The `about.rs` test that calls `init_no_event_loop` gets the same guard `tests/slint_ui.rs` uses, or moves there (review 4.5 hygiene).
- Visibility narrowing and dead code in these paths (A3); the wrong comments in these paths (A4).
- After 1.1's first commit is on the run branch: merge it and adapt the startup path so an engine start error is shown and the process exits nonzero.

Acceptance: `cargo run -- --ipc-socket <name in use>` prints a warning and the window opens; the `slint_ui` target passes; no `expect` remains in `ipc.rs`, `tray.rs` or `main.rs` outside test code, or each remaining one has a comment saying why it cannot fail.

### Run 2: test cost and value

Review items A6 and C1 to C5. Branch `refactor/quality-run-2`.

**Package 2.1, the engine targets.** Paths: `crates/sunlit-core/tests/engine.rs`, `tests/soak.rs`, `tests/support/**`, `tests/common/**`, and one `pub` visibility change for `DISPLAY_SETTLE` or its equivalent in `src/engine/mod.rs` if the test needs the real constant. Notes: tests-engine.md, test-timing.md.

- One engine per configuration group in `engine.rs`, in the order the review names: sun, display plan, then memory, resolution, cloud, moon, preview. The golden target already shows the pattern (A6).
- `soak.rs` halved; the 2 s sleep deleted; `camera_showing` coarse to fine, 4 degrees then the 8 degree neighborhood (A6).
- The tests that do not test their name in these files, starting with `re_enabling_the_preview_resends_the_current_frame_unchanged` (C3).
- Redundant tests merged, 78 toward 57 in `engine.rs`, each removal listed in the handover with the test now covering it (C2).
- The test-only `DISPLAY_SETTLE` copy replaced by the real value (review 4.5).
- Adopt the scratch-directory helper from 2.3 once it is on the run branch; until then keep the existing guards.

Acceptance: the two timing gates; every engine test still asserts what it asserted, per the handover list; no `let _ = remove_dir_all` left in `engine.rs`.

**Package 2.2, the GPU targets and the renderer-side test modules.** Paths: `crates/sunlit-core/tests/render_pipeline.rs`, `tests/shading.rs`, `tests/golden.rs`; the `mod tests` blocks only of `src/renderer/**`, `src/scene/**`, `src/params.rs`, `src/geometry/**`. Notes: tests-gpu.md, renderer.md, scene.md.

- `render_pipeline.rs`: 8 probes instead of 144, the compiled shader module reused, the offset test table-driven, the readback shader rebuilt on `sphere.wgsl` (C5).
- `shading.rs`: a `LazyLock` for the software context instead of a second device; the redundant tests merged, 12 toward 7; `nyc_day_side_not_dominated_by_city_lights` made to test something (C3, C4).
- The fresnel, gamma and saturation tests that render once and compare nothing, fixed or deleted (C3).
- `scene/sky.rs`: a moon-only helper and a twice-daily grid (C5). `scene/camera.rs` and `scene/datetime.rs` one-assertion families table-driven, 31 toward 14 and 49 toward 14 (C2). `renderer::quantize` table-driven (C2).
- The default-pinning tests in these modules replaced by property or agreement tests: `camera.rs`, `params.rs` gamma endpoints, `datetime.rs` range, `renderer/mod.rs` quantization (C1).
- The `render_to_file_*` tests write under `CARGO_TARGET_TMPDIR` like the rest (C4).

Acceptance: the shading target creates at most two devices, one per adapter and neither per test, with the second reached only by the test that needs it (**amended during run 2**: "one device" is unreachable, because the only route to it deletes `software_adapter_produces_correct_results`, whose whole point is a second independent adapter, and forcing the file onto the software adapter would make the cross-check compare an adapter with itself; the review's E2 says not to delete that test, and its recommendation 12 asks for a `LazyLock` or a documented exception, which is what shipped); the render_pipeline target's wall time is reported before and after; the golden target is untouched in behavior.

**Package 2.3, the remaining test modules and the app tests.** Paths: the `mod tests` blocks only of `crates/sunlit-core/src/config.rs`, `src/assets/**`, `src/memory.rs`, `src/memory_report.rs`, `src/display/**`, `src/display.rs`, `src/desktop.rs`, `src/wallpaper.rs`, `src/engine/**`; a new `crates/sunlit-core/src/test_support.rs` under `#[cfg(test)]`; `crates/sunlit-app/tests/slint_ui.rs`; the `mod tests` blocks of `crates/sunlit-app/src/**`. Notes: config-memory.md, assets.md, display.md, engine.md, tests-app.md.

- First commit: the scratch-directory helper with a pid suffix and a `Drop` guard in `src/test_support.rs`, reachable from the integration tests through `#[path]` in `tests/common/mod.rs` (that one file edit is allowed here and 2.1 is told). Say so in `status.md`; the orchestrator merges it into the run branch for 2.1.
- `config.rs`: the 13 missing-field tests become one struct equality; the 16 default-pinning tests become property or agreement tests, 73 toward 45 (C1, C2). `validated_geometry_accepts_on_screen` skips without a monitor (C4).
- `cloud_fetcher.rs`: the sequential lifecycle tests merged, 42 toward 31; `TEXTURE_RESOLUTIONS` no longer hardcoded; scratch directories through the helper (C1, C2, C4). `texture_cache.rs` and `texture_loader.rs` redundancy, and the cleanup that today only runs at start (C2, C4).
- `memory.rs`: the 3 GiB budget assertion replaced by the bounds the neighboring property tests already state; `assets/stars.rs`: the exact star count replaced by a bound (C1). `layout.rs` `bounds_of` table-driven, `DisplayMode::default()` no longer pinned (C1, C2). `watch.rs`: `SendMessageTimeoutW` in the test (C4). `wallpaper.rs` and `wallpaper_sink.rs`: `system_wallpaper_accepts_frames_on_windows` and `mock_clock_is_shareable_across_threads` deleted (C3).
- `slint_ui.rs`: the four default-pinning tests follow `test_default_horizon_matches_the_config`; `test_preset_changes_camera_properties` reads `PRESETS`; the five tests of Slint's own getters and setters deleted; 45 toward 29, each listed (C1, C2, C3). `mouse_math.rs` clamp pairs folded into the proptest, 48 toward 36 (C2). The `main.rs` scratch directory through the helper (C4).

Acceptance: `cargo unit` time is unchanged or lower; two concurrent `cargo unit` runs over the same checkout do not interfere; the handover lists every removed test.

The e2e helpers (C6) are deliberately not in this run; they go with the e2e comment pass in package 3.3.

### Run 3: the comment pass

Review Phase B, plus item C6. Branch `refactor/quality-run-3`.

Every package works from its notes' section A: the classification of every comment block as keep, move or delete, with the maintainer's rules as the standard. Delete the restatements, the bug narratives, the plan and phase references, the second copies of `docs/`, and the test doc comments that only say why a test exists (decision 6). Cut a comment that carries one real property in its first sentence down to that sentence. Everything classified "move" goes into the handover with the destination the note names, as exact text, for the orchestrator to place in `docs/testing.md`, `docs/rendering.md` or `docs/architecture.md`. Nothing in the review's "what to keep" list (section 4.1) is touched.

**Package 3.1, core engine side.** Paths: `crates/sunlit-core/src/engine/**`, `src/display/**`, `src/display.rs`, `src/desktop.rs`, `src/wallpaper.rs`, `src/wgpu_init.rs`, `src/memory.rs`, `src/memory_report.rs`, `crates/sunlit-core/tests/engine.rs`, `tests/soak.rs`, `tests/support/**`, `tests/common/**`. Notes: engine.md, display.md, config-memory.md, tests-engine.md.

**Package 3.2, core renderer side.** Paths: `crates/sunlit-core/src/renderer/**`, `crates/sunlit-core/shaders/**`, `src/scene/**`, `src/assets/**`, `src/geometry/**`, `src/config.rs`, `src/params.rs`, `crates/sunlit-core/tests/render_pipeline.rs`, `tests/shading.rs`, `tests/golden.rs`. Notes: renderer.md, scene.md, assets.md, config-memory.md, tests-gpu.md.

**Package 3.3, the app and the e2e suite.** Paths: `crates/sunlit-app/**` except `Cargo.toml` and `lib.rs`. Notes: app.md, tests-app.md. Besides the comment pass, this package does item C6 on `tests/e2e.rs`: widen `spawn_for_ipc`, extract `assert_no_error_lines` and `quit_and_expect_clean_exit`, four named timeout constants, attach the watchers in `test_render_and_exit`, one parser per `SIGNAL:` line shared with the producer in `ipc.rs`, and reconcile the Linux job timeout with the handover naming the CI change for the orchestrator. The 55 numbered step comments go first. The e2e target must compile (`cargo test -p sunlit-earth --test e2e --no-run`); it is run once in the guest by the orchestrator or the maintainer at the gate.

Acceptance for 3.1 and 3.2: the diff has no code changes. For 3.3: the diff outside `tests/e2e.rs` and `src/ipc.rs` has no code changes. For all three: the comment line count of the owned paths, before and after, in the handover.

### Run 4: structure, core

Review items D1 to D4 for `sunlit-core`. Branch `refactor/quality-run-4`. Every split is a move: `git mv` where a whole file moves, otherwise cut and paste with no edits in the same commit, so `--color-moved` can prove it. Behavior changes ride in separate commits and are named in the handover.

**Package 4.1, engine and wallpaper.** Paths: `crates/sunlit-core/src/engine/**`, `src/wallpaper.rs` and the new `src/wallpaper/**`, `crates/sunlit-core/tests/engine.rs` and the new `tests/engine/**`, `tests/support/**`, `tests/common/**`. Notes: engine.md, display.md, tests-engine.md.

- `wallpaper/{mod,windows,linux}.rs` with `Publication::write_job` shared, following the per-OS submodule pattern (D1, D4).
- `engine/{schedule,protocol,handle,cloud_worker,publish}.rs`; `Engine::new` and `handle` extracted (D4).
- `tests/engine/` as a directory target with the helpers beside the tests that use them; `tests/common/` split if the note's seam is still there (D4).
- Delete `wallpaper::get_primary_monitor_resolution` and, with it, `display::primary_monitor_of`, whose only other caller is a `display.rs` test. Run 2 established that the function's sole caller is its own test and that narrowing it to `pub(crate)` makes it dead code under `-D warnings`; the deletion has to be one commit because it spans `wallpaper.rs` and `display.rs`, and 4.3's `display.rs` work is where the second half lands. The merged test `wallpaper::tests::every_monitor_is_enumerated_with_a_rectangle_and_one_of_them_is_primary` composes `enumerate_monitors()` and the primary it finds directly once the function is gone. (Deferred from run 2 as departure 15.)

**Package 4.2, the renderer.** Paths: `crates/sunlit-core/src/renderer/**`, `crates/sunlit-core/shaders/**`, `crates/sunlit-core/tests/render_pipeline.rs`, `tests/golden.rs`. Notes: renderer.md, tests-gpu.md.

- `gpu_setup.rs`: the `Pipelines` table replacing the ten descriptors built twice; golden run before and after (D2).
- `draw_scene` extracted for the preview and export passes; `Overlays` to `Option<Overlay>`; `encode_and_submit` from 20 arguments to 9 (D3).
- `renderer/{sizing,slots}.rs` (D4). The shader `shell_vertex` shared, golden run (D7, the part that lives in the shaders).

**Package 4.3, scene, config, display, desktop, memory.** Paths: `crates/sunlit-core/src/scene/**`, `src/config.rs` and the new `src/config/**`, `src/display.rs`, `src/display/**`, `src/desktop.rs` and the new `src/desktop/**`, `src/memory.rs` and the new `src/memory/**`, `src/memory_report.rs`. Notes: scene.md, config-memory.md, display.md.

- `git mv display.rs display/mod.rs`, the one module without a `mod.rs` (D1).
- `memory/{windows,linux,macos}.rs` in the per-OS pattern (D1).
- `scene/sky_lens.rs` and `scene/limb_extinction.rs`; `SKY_FOV_*` in `sun_occlusion.rs`; the `sky.rs` FFI helpers deduplicated (D4, D7).
- `config/window_geometry.rs` over `display::monitors()`; `desktop/{xfce,kde}.rs` (D4).

Acceptance for all three: every moved block is shown as a move by `git diff --color-moved=dimmed-zebra`; the golden test passes unchanged; `cargo doc` builds; the handover lists every `pub` path that changed so the orchestrator can update `docs/architecture.md`.

### Run 5: structure, app and cross-cutting

Review items D4 to D7 for the rest. Branch `refactor/quality-run-5`. Two implementers, in sequence: 5.2 starts only after 5.1 is merged, because it touches every directory.

**Package 5.1, the app.** Paths: `crates/sunlit-app/**` except `Cargo.toml`. Notes: app.md, tests-app.md.

- `main.rs` into the library: the binary becomes a thin `main` over `sunlit_earth::run` (D4).
- `ui/widgets.slint` for the structs and the six components, `ui/tray.slint`, and `ui/about.slint` for the About window, which is 250 lines since the overhaul (D4, app.md B5).
- The gamma display value pushed from Rust through an `out property` instead of the two inlined formulas (D7, app.md D3). The window's minimum width and the sky slider's bounds exposed as `out property`s so `slint_ui.rs` reads them instead of restating or string-matching them (tests-app.md D-6, F-7).
- The `on_load_defaults` and `on_reset` bodies deduplicated and given a test through `register_action_callbacks` (app.md D2, F5).

**Package 5.2, cross-cutting, alone.** Paths: everything under `crates/sunlit-core/src/**` and `crates/sunlit-app/src/**` that the items below touch, plus their tests. Notes: renderer.md F1 and F2, scene.md, config-memory.md, app.md.

- `ParamsDigest`, `digest()` and the mutation table generated from one macro list in `params.rs`, so a parameter added to the list cannot be missing from the digest or the table (D5).
- `#[expect(..., reason = "...")]` replacing the cast allows, the six small cast helpers, `#[expect]` adopted as the convention; the ten allows that no longer fire deleted (D6).
- The shared helpers: `app_data_dir`, one PNG writer, one `unfinished`, one atomic TOML write, `From<&SceneParams> for Framing` (D7).
- The parameter checklist count corrected in `CLAUDE.md` and `docs/rendering.md`, through the handover (review 4.4).

Acceptance: the `params.rs` digest test fails when a field is added to the macro list without a WGSL counterpart, demonstrated once and reverted; clippy clean with `#[expect]` so an allow that stops firing is an error; the golden test unchanged.

## Settled, and not to be relitigated

- Disposable worktrees are not used; the pool is. A cold build is paid once per pool member, ever.
- One shared `CARGO_TARGET_DIR` is not used, for the collision reason above, whatever the lock behavior turns out to be.
- Three implementers per run. A fourth is not added to finish faster.
- The validator does not fix; the implementer that wrote the code does.
- Agents do not edit documentation. The orchestrator does, once per run, from the handovers.
- No golden image is regenerated in any run. A change that moves a golden is a defect in the change.

## Departures

Numbered, appended by the orchestrator from the handovers, with the reasoning. One sequence across
every run: a handover that numbers its own departures from 1 is renumbered into it at merge time.

1. **Run 1's branch is cut from `docs/code-quality-review`, not from `main`.** The kickoff condition was that the review's pull request is merged; #46 was still a draft. The maintainer chose stacked pull requests over waiting for the merge, so `refactor/quality-run-1` sits on top of the docs branch and the plan and the notes travel inside every pool worktree. Every later run stacks the same way until #46 lands. The wrap-up step that appends to this plan on the run branch works unchanged.
2. **Package 1.3 moves the IPC listener bind ahead of the renderer and engine startup**, beyond the plan's "the listener failure is a warning". The baseline smoke test on the development host showed a second instance on an occupied socket name selecting the adapter, creating every texture and logging `engine started` before it reached the bind and panicked, about six seconds in. Decided by the maintainer at run 1's kickoff.

3. **Agents may not launch the application on the development host, and may run it inside a VM guest only under an orchestrator-granted lease.** The plan barred every `cargo xtask` command and every VM boot outright, and said nothing about the host desktop; run 1's implementers read their acceptance criteria as licence to open windows on the maintainer's screen while the maintainer was working there. The host desktop is now closed to agents entirely, with runtime acceptance checks moving to the orchestrator at the gate. The guests are open, but one desktop guest runs at a time on a host, so an agent requests the lease by message, waits for an explicit grant, and releases it with `vm down` immediately afterwards. The orchestrator holds the queue. Decided by the maintainer during run 1.

4. **`read_texture_rgba8` is defined in `renderer/render_pass.rs`, not `renderer/mod.rs`.** The plan's ownership lines give package 1.1 "the `read_texture_rgba8` function in `src/renderer/mod.rs`" and give package 1.2 the item "the `render_pass.rs` unwraps on device loss become errors". Both name the same three unwraps, because the function is defined in `render_pass.rs` and only re-exported from `mod.rs`. Package 1.2 stopped rather than guessing, the orchestrator confirmed it, and 1.1 discharged the item in `a0d1dc9`. The ownership line was wrong about the path, not about the function. No other unwrap or expect remains in that file.

5. **Coalescing `RenderWallpaperNow` needed a flush in the `Shutdown` arm.** Moving the publish out of `handle` into `tick` would otherwise silently drop a request that shares a drain batch with `Shutdown`, which is an IPC `set-wallpaper` immediately followed by `quit`. With the flush the change is a pure optimization rather than a small behavior regression.

6. **`wallpaper::get_primary_monitor_resolution` stays `pub`.** Its only callers are its own three tests, so `pub(crate)` is dead code on every platform and CI builds with `-D warnings`. Deleting it is the honest alternative but takes two Windows monitor tests with it, and run 2's package 2.3 is already scheduled to merge those three tests into one. The decision belongs there, with the evidence in hand. Validated as sound.

7. **Narrowing an item whose only caller is on another platform makes it dead code locally, so four items took a `cfg` rather than an `allow`.** The xrandr parser and its two helpers are `#[cfg(any(target_os = "linux", test))]`, the pattern `memory.rs` already documents for its `/proc` parsers; `Output::overlaps` is `#[cfg(any(not(windows), test))]`; `Watcher::hwnd` is `#[cfg(test)]`. `desktop.rs` is the exception, since its whole design is per-desktop behavior as data with no `cfg`, so it takes one module-scoped `#[cfg_attr(not(target_os = "linux"), allow(dead_code))]`.

8. **Deleting `wallpaper_on_monitor` cascaded into `shell::DesktopWallpaperApi::get`**, which existed only for it. It was the only Windows wallpaper read-back; the e2e suite's read-back is `Backend::discovery`, which is Linux-only, so nothing is lost today. A Windows read-back assertion would have to bring it back from git.

9. **`Uniforms` is plain `pub`, not `#[doc(hidden)] pub`.** The review offers both. Plain `pub` matches the plan's wording, and the struct is a real part of the contract `CLAUDE.md`'s parameter checklist describes rather than a test double. The module carries a `//!` saying why it is public.

10. **Five items the notes call narrowable stay `pub`, because item 1 gave them an external caller.** `Uniforms`, `Vertex`, `SphereMesh` and `generate_uv_sphere` are read by `tests/render_pipeline.rs`, which is outside the crate. `config::DEFAULT_TEXTURE_RESOLUTION` was already read by `crates/sunlit-app/tests/slint_ui.rs`, which the notes' grep did not cover.

11. **Two occlusion tests went with `transit_fraction`.** `a_sun_in_the_annulus_is_visible_and_fully_in_transit` and `a_sun_past_the_atmosphere_is_out_of_transit` had the band as their whole subject; with the field gone each merely restated `a_sun_clear_of_the_silhouette_is_fully_visible` at another distance. The validator checked the geometry and agreed. The test helper `atmosphere()` went too, as its last caller.

12. **The auto-refresh interval also gets a one-second deferred save.** The plan's item says the save moves to "the checkbox, the button and window close, which already save". Window close saves the geometry only, through `config::save_window_geometry`, which starts from the file rather than the window, so it does not merely fail to persist an unsaved interval, it writes the stored one back over it. Every Slint style routes the accessibility set-value action to the slider's `changed` and never to `released`, so without a deferred save a screen reader's change would reach the engine and never the disk. The timer is restarted on every tick and stopped by the release that would write anyway, so a drag still writes exactly once. Residual hole: an accessibility change followed by an exit inside the same second.

13. **The readiness signal stays at serve time rather than moving to the bind**, even though the bind moved ahead of the GPU. Moving the signal with it would make the e2e suite's readiness wait a lie. Validator round 1 then found the signal was printed before the spawn it announces, which is fixed: it is now printed on the `Ok` of the spawn.

14. **Run 2 started at 64 percent of the five-hour window, above the plan's threshold of 50.** The maintainer authorised spending the remainder because the window rolls over in about 38 minutes, which makes the usual risk, a run force-stopped mid-flight, a short wait rather than a loss. The controlled pause at 90 percent still applies until the rollover. Run 2's branch stacks on `refactor/quality-run-1` for the reason in departure 1, so runs 1 and 2 are a two-deep stack on `docs/code-quality-review` until #46 merges.

The three handovers each numbered their own departures from 15 or from 1, so they are renumbered here into the single
sequence. Package 2.1 supplied 15 to 20, package 2.2 21 to 24, package 2.3 25 to 31.

15. **`engine.rs` goes 78 tests to 71, not to 57.** The review's 57 came from its F3 merge table; the merges that would
    have reached it turned out to remove assertions rather than duplication. The run's gates are the two timings and the
    rule that every removal names its successor, both met, so the count is a report rather than a miss.
16. **The display-change cases need an engine that never publishes**, because the engine remembers that it has
    published. A property of the engine that 91 disposable engines had hidden.
17. **`camera_showing` refines eight coarse candidates rather than one.** The refinement lattice is a subset of the base
    0.5 degree lattice, so the answer can only be a point the exhaustive scan also considered: 12.6k evaluations against
    244.8k. Verified by the validator as a lattice argument, not as an equality.
18. **`soak.rs`'s `EXPORT_SIZE` stays 160x96.** The review's item 6 estimated a saving that measurement did not support.
19. **The landmark fixture is 5 degrees where one of its two cases used 4.** Closed after validation by moving the
    vacuity floor with the area, 50 to 78 against 1101 measured lit pixels, rather than by declaring it.
20. **The 2 s `SETTLE` became a poll on the source's own log plus `NOTHING_HAPPENS_IN`**, which is sound because the
    slot purge, if it happened, would already have happened by the time the retarget is observable.
21. **The `shading` target still creates two devices, and should.** See the amended acceptance line above: one device is
    unreachable without deleting the cross-adapter test the review forbids deleting.
22. **The review's fix for `fresnel_specular_brighter_at_grazing` does not hold, and the physics says why.** The review
    is right about Schlick and wrong about the camera. At the specular peak `n_dot_v` is `cos(theta/2)`, so Fresnel only
    beats head-on past about 110 degrees of sun-eye separation, and the old camera sat at 45 degrees behind a lens the
    globe overflows, so the highlight was never in frame. Measured peak glint at distance 12 rises from 24.0 head-on to
    78.2 at 150 degrees. The replacement compares head-on against 140 degrees and fails with the specular term switched
    off, which both the implementer and the validator reproduced independently. **This corrects the review, not the
    code, and matters for the runs still specified against that document.**
23. **Item 6 belongs to package 2.1**, because both `render_to_file_*` tests live in `tests/engine.rs`. The plan's
    ownership line misassigned it; confirmed by grep from two sides.
24. **Two measurement sentences trimmed from `sky.rs` test doc comments**, because halving the sampling grid invalidated
    the provenance they recorded.
25. **Departure 6 is resolved: `wallpaper::get_primary_monitor_resolution` stays `pub` with one test.** Deleting it
    cascades into `display::primary_monitor_of`, whose only other caller is a `display.rs` test, so both would be dead
    under `-D warnings` and the deletion spans two files package 2.3 does not own. Written into run 4's package 4.1 item
    list so it does not fall between 4.1 and 4.3.
26. **`config.rs:319`'s redundant serde default is production code**, so package 2.3 verified it redundant (45 config
    tests green without it) and handed it to the orchestrator, who applied it.
27. **`load_partial_file_fills_defaults` was kept and strengthened rather than merged away.**
28. **`ScratchDir` creates its own directory**, so three tests asserting that production code creates a parent were
    rewritten to name a path below the scratch root that does not exist yet. Without that they would have become
    tautologies. Recorded in `docs/testing.md` so the next person does not rediscover it by writing a test that cannot
    fail.
29. **`display.rs` (12 tests to 8) and `desktop.rs` (26 to 23) were merged although the item list does not name them.**
    Both files are in the package's paths and both are C2 targets, so doing them here saves a later run reopening the
    files. Declared rather than shipped silently.
30. **`crates/sunlit-app/src/main.rs` gained one `#[cfg(test)]` module declaration outside its `mod tests` block**, to
    reach the shared scratch helper. Outside the package's stated paths; granted by the orchestrator as the identical
    allowance the plan already made for `tests/common/mod.rs`, and it compiles into no release build.
31. **The plan's wording for `test_preset_changes_camera_properties` was not followed literally**, the merged test being
    a better shape than the one the plan described.

## Validation record

One entry per package: run, package, validator round date, MAJOR and MINOR counts, what was fixed, what was declined.

| Run | Package | Round | Date | MAJOR | MINOR | Outcome |
|---|---|---|---|---|---|---|
| 1 | 1.1 engine side | 1 | 2026-09-05 | 0 | 4 | All four fixed in `656ed05`, none declined. The substantive one: `render_if_dirty` emitted the preview as a side effect while `tick` emitted it again for the debt, so a failed readback cost two attempts per tick and a persistently failing device logged 20 to 40 lines a second. `render_if_dirty` now reports only whether a frame was drawn, `tick` is the single readback site, and a `readback_failed` latch logs the transition rather than the state. The other three: a comment describing the pre-change unwinding hazard, `private_bytes_budget` narrowed to private to match its two siblings, and an unwrapped doc line. |
| 1 | 1.2 renderer side | 1 | 2026-09-05 | 0 | 4 | Two fixed in `288e7e5`, both comments. A replacement comment claimed a render target is never larger than what was asked for, contradicted by two tests twelve lines below it; and the `PREVIEW_USAGE` doc read as settled on the open question of `TEXTURE_BINDING`. MINOR 3 is informational and recorded under Open items. MINOR 4 was the orchestrator's, a departure-numbering collision, handled at merge. The validator reproduced review E1's swap itself and got `day_gamma: got 0.8, expected 1.5`. |
| 1 | 1.3 the app | 1 | 2026-09-05 | 0 | 2 | Both fixed in `2ef528b`, neither declined. `SIGNAL:ipc_listener_ready` was printed before the thread it announces spawned, so a failed spawn would have sent every e2e client to a socket nobody accepts on; the signal moved to the `Ok` of the spawn. The auto-refresh save became departure 12 rather than a decline, after the implementer established that window close writes the stored interval back over an unsaved one. |

| 2 | 2.1 engine targets | 1 | 2026-09-05 | **2** | 5 | Both MAJORs were one defect and both are fixed in `81459bf`. `SURFACE` was built at `texture_index: 0`, so its constructor's wait returned on the procedural grid with the file-backed day and night slots still empty, and the restructure had dropped the per-case waits that used to fill them. Four cases therefore passed only in their position (67 of 71 pass when each is run alone), and `a_dayside_cloud_is_brighter_than_a_night_side_one_in_every_mode` rendered all three modes against the grid, so a broken day or night path would not have failed it. The validator reproduced both by running every case alone, by reversing the name order and by an ordinary filtered run. The fix starts the group in blend mode and waits for both slots by name; the implementer then found `REAL_SKY` had the same shape and fixed it too, and ruled out the other five with reasons. Independence proved afterwards three ways: 71 of 71 alone, reversed order green, seven filtered runs green. MINORs: the anchor case keeps a configured engine so `display_mode` and `anchor_monitor` are exercised at startup again; three 500 ms negative windows restored; the landmark floor moved with the area; `GROWTH_LIMIT` 16 to 8 MiB after the soak window halved. |
| 2 | 2.2 GPU targets | 1 | 2026-09-05 | 0 | 3 | Two fixed in `116218e`, both tolerances the merges had silently loosened: three `datetime` rows asserting an exact zero went from epsilon 0.1 to 1.0, and `camera`'s per-component tolerances became one per row. Both restored to exactly what their predecessors carried. The third was the orchestrator's, a mislabelled gate block that would have put an error into this plan. |
| 2 | 2.3 test modules and app tests | 1 | 2026-09-05 | **1** | 5 | All six fixed in `d183a8d`, none declined. The MAJOR: `desktop.rs:928` replaced the literal for xfconf's zoomed enumerant with the production constant that supplies it, making both sides of the assertion the same symbol, so changing that constant would ship a letterboxed wallpaper to every XFCE user with the suite green. It is an interop value rather than a project default, so `CLAUDE.md`'s no-pinning rule does not reach it. Restored with a comment saying why, and falsified. MINORs: the quality tier round trip used the debug default so a `sanitize` regression would only fail in release; a 1e-3 tolerance applied to all 55 agreement rows where one needed it; a lens equality loosened; the cloud cache folder name unasserted; the preview-size test re-deriving its own body. |

No validator found a MAJOR finding, a broken behavior, or an unmet plan item in run 1. All three validators hit the same
harness limitation and returned their reports as text rather than writing them; the orchestrator transcribed all three
into the run directory as `findings-1.1.md`, `findings-1.2.md` and `findings-1.3.md`.

## Budget record

| Run | Started at (window %) | Ended at (window %) | Implementers | Notes |
|---|---|---|---|---|
| 1 | 22 | 57 | 3 | 35 points of the five-hour window for three implementers, three validators and the orchestrator, well under the 50 the plan budgeted for a whole run. The pool's four cold builds were paid once here and are not repeated. Runs 2 to 5 need no shrinking on this evidence; three implementers per run stands. |
| 2 | 64, with the window rolling over 38 minutes in | 41 of the new window | 3 | The maintainer authorised finishing the old window and continuing into the new one, so the run spans a rollover and the two numbers are not comparable. Measured cost after the rollover, covering all three validator rounds, three fix rounds, the merges and the gates: 39 points. Comparable to run 1's 35. |

## Declined findings

Review findings the maintainer or an implementer declined, with the reason.

None in runs 1 to 3. Every MAJOR and MINOR finding from all validator rounds was fixed. Three items were deliberately
carried forward from run 1 rather than declined; two have since been resolved:

- `renderer::PREVIEW_USAGE` still unions `TEXTURE_BINDING`, which nothing binds now that `Renderer::preview_texture` is
  deleted. No test in the suite would show that dropping it is safe, so the flag stays and the comment says it is an
  open question rather than a decision. Renderer note C6. **Still open**, and run 3 confirmed the comment is intact, so
  the next reader meets the question rather than the flag alone.
- `assets/texture_loader.rs`'s `flip_matches_the_image_crates_own` is kept. It still asserts that our flip matches the
  reference implementation every golden image was generated with. **Resolved in run 2**: package 2.3 weighed it and kept
  it, which is the decision run 1 deferred to it.
- `config.rs:319`'s `#[serde(default = "default_custom_year")]` was redundant under the struct-level
  `#[serde(default)]`. **Resolved in run 2**, departure 26: package 2.3 established the redundancy (45 config tests green
  without it) and the orchestrator removed it, since the attribute is production code the package did not own.

## Run 1 gate record

Run on the merged run branch in `pool-0`, with the adapter to itself.

- `cargo fmt --check` clean. `cargo clippy --all-targets` clean, all three crates rechecked, zero warnings.
- `cargo test` green across the workspace: core lib 536, engine 78 in 173.49 s, golden 20 in 3.23 s, render_pipeline 21
  in 0.78 s, shading 12, soak 1 in 66.28 s, app 82 + 2, e2e 15 ignored and compiling, slint_ui 53, xtask 633.
- `git status --short crates/sunlit-core/tests/golden/` empty, and the whole worktree clean.
- `cargo run -- render --output <tmp>/r.png --width 640 --height 360` exit 0, a 460 KB PNG written.
- `sunlit-earth.exe --ipc-socket 'bad
ame'` exit 0 with no panic: the app came up, loaded the cached cloud image, set
  the wallpaper, ran its event loop and shut down through `event loop exited`. On this run's parent commit the same
  command panicked at `tray.rs:48` with `failed to create single-instance mutex: MutexError(3)` and exit 101.
  What was observed is the absence of the panic and a complete clean run; the warning line itself was filtered out of
  the captured tail and was not read. The warning path is covered headlessly by
  `ipc::tests::a_name_something_else_holds_is_an_error_rather_than_a_panic` and
  `tray::tests::a_mutex_name_windows_refuses_leaves_the_app_running_alone`, both confirmed by the validator to fail if
  the panics return.
- The separate "once normally" start was skipped by the maintainer's decision: the invalid-socket run exercises the same
  startup path end to end apart from the socket name, and the baseline run before any change had already been observed.
- The e2e suite in the Windows guest: **done and green.** `cargo xtask e2e --target windows` on `cdaa33c`, exit 0,
  `15 passed; 0 failed` in 97.43 s, no error or panic line in the run, guest destroyed afterwards. Two cases gated
  themselves at runtime and said why, as designed: `test_a_layout_change_republishes_the_wallpaper` needs a second
  output or a second mode and `display::outputs` is a Linux query, and `test_plasmashell_survives_rapid_republishing`
  needs KDE. This is the first end-to-end exercise of package 1.3's `bind`/`serve` split; the cases that depend on the
  readiness signal, among them `test_tray_mode_ipc_lifecycle`, `test_memory_report`, `test_set_wallpaper` and
  `test_single_instance_second_exits`, all passed, so the signal still arrives when and as the suite expects. The
  harness log does not echo `SIGNAL:` lines, so the evidence is those cases passing rather than a line read directly.

## Run 2 gate record

Run on the merged run branch in `pool-0`, adapter to itself.

- `cargo fmt --check` clean. `cargo clippy --all-targets` clean, zero warnings.
- `cargo test` green across the workspace: core lib 398 in 0.15 s, engine 71, golden 20 in 3.18 s, render_pipeline 17
  in 0.69 s, shading 7 in 0.80 s, soak 1 in 33.46 s, app lib 69, app bin 2, e2e 15 ignored and compiling, slint_ui 35,
  xtask 633.
- `git status --short crates/sunlit-core/tests/golden/` empty; no golden regenerated anywhere in the run.

**The two timing gates, honestly.**

| | review baseline | orchestrator baseline | after, warm | after, cold |
|---|---|---|---|---|
| `engine` | 161.6 s | 173.49 s | **57.21 s, 56.35 s** | **65.73 s** |
| `soak` | 65.6 s | 66.28 s | **33.21 s, 33.46 s** | |

The engine gate of 60 s is **met warm and missed cold**. The first run in a freshly merged worktree, with the downscale
cache empty, was 65.73 s here and 61.82 s on the validator's host; every subsequent run is 56 to 57 s. A fresh checkout
and CI are cold, so the gate as worded is met only on a warm target directory. This is not a regression, since the base
paid the same cost, and nothing in run 2 introduced it. It is recorded rather than chased.

The soak gate of 35 s is met in every condition measured.

**The speedup is 3.1x on the engine target**, against the orchestrator's own idle-machine baseline of 173.49 s. The
implementer's paired measurement reported 197.01 s before and 55.22 s after, a 3.5x ratio, but the before half was taken
while packages 2.2 and 2.3 were competing for the adapter; the implementer's own third figure of 180.18 s, labelled at
the time as taken under load, corroborates that. The validator argued the drift theory does not hold, because a
uniformly slow host would have inflated the after half equally and a quiet host would then show about 49 s, where 54 to
57 s is what reproduces. The pairing keeps the ratio internally consistent, but 3.1x is the number to quote.

Not done in run 2: the e2e suite was neither run nor needed, since the run touches no production behavior beyond one
`pub` and one deleted serde attribute. The cross-platform compile is still owed from run 1.

## Open items

- **The offset guard's real reach.** `uniform_buffer_field_offsets_match_wgsl` catches any change that moves an existing
  field's offset, which is what item A2 asked for and what review E1's scenario exercises. It does not catch a field
  appended into the trailing padding: replacing `_pad8` with a real field and setting it in `write_uniforms` leaves the
  struct at 544 bytes with every probed offset unchanged, so all 21 tests pass while the shader still calls those bytes
  padding. Closing that wants a field-count or offset-table check, and run 5's macro work over `params.rs` is the
  natural home for it.
- **Two rows of the review's 4.1 comment table fall in `tests/engine.rs` and `tests/soak.rs`**, which no run 1 package
  owns. They belong to run 3's package 3.1.
- **Neither Linux nor macOS was compiled in run 1.** Every new `cfg` gate was reasoned through by an implementer and
  re-checked by a validator, but the only compiler run was Windows MSVC. Note that `.github/workflows/ci.yml` is
  `workflow_dispatch` only, so pushing the run branch does not check this: it has to be asked for with
  `gh workflow run ci.yml --ref refactor/quality-run-1`, and it bills against the private repository's minutes at 2x for
  Windows and 10x for macOS. Run 1 changed per-OS code in `display/**`, `desktop.rs`, `memory.rs` and `wallpaper.rs` and
  added five new `cfg` gates, so this is worth one dispatch before the run merges.
- **The e2e suite has not run.** It compiles, with all 15 tests `#[ignore]`d as expected. Run 1's per-run gate says it
  runs once in the Windows guest; that is outstanding and is the maintainer's or the orchestrator's under an explicit
  grant.
