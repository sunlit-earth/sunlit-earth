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

Acceptance: the shading target creates one device; the render_pipeline target's wall time is reported before and after; the golden target is untouched in behavior.

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

Numbered, appended by the orchestrator from the handovers, with the reasoning. None yet.

## Validation record

One entry per package: run, package, validator round date, MAJOR and MINOR counts, what was fixed, what was declined. None yet.

## Budget record

| Run | Started at (window %) | Ended at (window %) | Implementers | Notes |
|---|---|---|---|---|
| | | | | |

## Declined findings

Review findings the maintainer or an implementer declined, with the reason. None yet.
