# Plan: Phase 2, Basic Cross-Platform Support

## Summary

Make the workspace build, run, and test on Linux and macOS, then expand CI from the single Windows job to a three-OS matrix with the two commented-out gates (`cargo fmt`, `-D warnings`) re-enabled. The engine and render layers must pass on lavapipe (Linux) and Metal (macOS) in addition to WARP (Windows). Wallpaper setting, tray integration on Linux, and VM orchestration are explicitly out of scope; they belong to the "Later" list and Phase 3 (see `docs/retrospective-2026-08.md`, sections 8.3 and 10).

## Stakes Classification

Medium. Nothing here changes Windows behavior for existing users; the risk is churn (a codebase-wide reformat, warning fixes across the workspace) and CI cost (three OS runners per push). Both are deliberate one-off payments the retrospective schedules before the matrix multiplies their cost.

## Research

Settled in `docs/retrospective-2026-08.md`:

- Hosted macOS Apple Silicon runners expose a paravirtualized Metal GPU; wgpu's own CI runs GPU tests on plain `macos-14` runners (section 11, question 1). A one-time probe of our specific pipeline (MSAA, mipmapped textures, buffer readback) is still required.
- Linux hosted runners are the most capable windowed platform: xvfb plus lavapipe run the whole stack (section 8.3).
- Hosted Windows runners have no interactive desktop; the e2e suite stays `#[ignore]`d and Windows-only until Phase 3 moves it into VMs (section 8.3).
- Software adapters are conformant implementations, so all application and wgpu code is exercised; the untested layer is vendor drivers and silicon (section 8.4). With macOS added, all three naga targets (SPIR-V, HLSL, MSL) get CI coverage.

Current state of the codebase (surveyed 2026-08-15):

- Some non-Windows gating already exists from Phase 1: `engine/wallpaper_sink.rs` and `config.rs` have `cfg(not(windows))` branches, `memory::snapshot()` returns `None` off Windows, and `lib.rs` gates the wallpaper module. The workspace has never been compiled or tested on a non-Windows target, so treat all of it as unverified.
- `wallpaper.rs` (Win32 wallpaper API plus monitor enumeration) and `main.rs` (`AttachConsole`) are Windows-only code behind gates.
- Golden tests force the software adapter and skip when references are missing; references were generated on WARP, so lavapipe and Metal need their own reference sets.
- Every memory assertion in the test suite is a no-op off Windows because `snapshot()` returns `None`; the soak test early-returns.

## Key Design Decisions

1. **Reformat and warning cleanup first, as their own commits.** The retrospective orders it this way because a three-OS matrix multiplies the cost of warning debt. The `cargo fmt` commit is mechanical and reviewed as "diff is whitespace-only"; the warning cleanup is reviewed normally.
2. **Per-OS memory snapshots instead of a crate dependency.** Linux reads `/proc/self/status` (VmRSS, VmHWM) and `/proc/self/smaps_rollup` (private = Private_Clean + Private_Dirty); this is file parsing, no unsafe. macOS calls `task_info(TASK_VM_INFO)` through scoped `#[allow(unsafe_code)]` with `// SAFETY:` comments, matching the existing Windows FFI pattern: `phys_footprint` maps to private bytes, `resident_size` to RSS, `resident_size_peak` to peak RSS. The `MemorySnapshot` struct and the CSV format do not change. This activates the soak test's memory assertions on all three OSes.
3. **Wallpaper on non-Windows is a clean "unsupported" error, not a stub that pretends.** `SystemWallpaper::set` returns an error string on Linux and macOS; the UI surfaces it in the status line. The headless `render` subcommand works everywhere and is the useful cross-platform mode today. Real setters are future work.
4. **Native resolution fallback.** The Win32 monitor enumeration keeps its gate; non-Windows uses the primary monitor size reported by the windowing layer when a window exists, and a documented 2560x1440 default in headless mode. The e2e and soak tests already inject sizes, so only the interactive path uses the fallback.
5. **Per-adapter golden references.** References move from one flat set to per-adapter directories keyed by the adapter's backend (`warp/`, `lavapipe/`, `metal/`). The comparison tolerance and the pairwise-distinguishability test are unchanged within each set. Regeneration happens through a manual `workflow_dispatch` job that runs with `SUNLIT_EARTH_UPDATE_GOLDEN=1` and uploads the references as an artifact to be committed; the golden test keeps its existing skip-when-missing behavior so the matrix can land before all three reference sets exist.
6. **One workflow, one matrix.** `ci.yml` becomes a matrix over `ubuntu-latest`, `windows-latest`, `macos-latest` with per-OS setup steps (LLVM pin stays Windows-only; Ubuntu installs mesa's Vulkan drivers, xvfb, and Slint's documented build dependencies; macOS runners need no GPU setup). Linux tests run under `xvfb-run`. The rust-cache `shared-key` gets an OS suffix. `fmt` runs once on Ubuntu, not per OS.
7. **The e2e suite does not join the matrix.** It stays `#[ignore]`d and Windows-gated; hosted runners have no interactive desktop and Phase 3 owns the VM story. `slint_ui` tests use the testing backend and run on all three OSes.

## Success Criteria

1. CI is green on all three operating systems with `cargo fmt --check` and `RUSTFLAGS: -D warnings` re-enabled.
2. `cargo test` passes on Linux (lavapipe under xvfb) and macOS (Metal), including the engine integration tests and the soak test with its memory assertions active (bounds may need per-OS calibration; see risks).
3. The `render` subcommand produces a PNG on all three OSes in CI (smoke step in the workflow).
4. `memory::snapshot()` returns `Some` on all three OSes, verified by the existing `snapshot_returns_some_on_windows` test generalized to all platforms.
5. Golden references exist for at least WARP and lavapipe; the Metal set may follow after the probe run, using the skip-when-missing path in the meantime.
6. No new unconditional `unsafe`; new FFI sites follow the scoped-allow-plus-SAFETY-comment convention.
7. The Windows job's runtime stays within a few minutes of its Phase 1 baseline (warm cache), so the matrix does not degrade the inner loop.

## Implementation Steps

### Step 1: Reformat and re-enable the fmt gate

`cargo fmt` across the workspace as one mechanical commit. Re-enable the `fmt` job in `ci.yml` (runs once, on Ubuntu). Remove the corresponding roadmap entry.

### Step 2: Warning cleanup and re-enable `-D warnings`

Fix everything `cargo clippy --all-targets` and rustc report on current stable, uncomment `RUSTFLAGS: "-D warnings"` in `ci.yml`. Where a lint is wrong for this codebase, prefer a scoped `#[allow]` with a one-line reason over a global allow.

### Step 3: Linux build and test, locally via WSL

Finish the cfg gates until `cargo build` and `cargo unit` pass in WSL (Ubuntu 22.04 is available on the dev machine). Implement the Linux memory snapshot. Then the GPU layers: install mesa Vulkan drivers in WSL and run the engine, soak, golden (regenerating lavapipe references locally counts as a first draft; CI regeneration is authoritative), shading, and render_pipeline suites. Fix what breaks. Single-instance and IPC (`interprocess`) are expected to work on Unix sockets; verify with the existing unit and slint_ui layers.

### Step 4: macOS build, blind, then probed

Implement the macOS memory snapshot and any remaining gates. There is no local macOS hardware, so compilation correctness is verified by a temporary CI probe job (build plus unit tests) before the full matrix lands. Then extend the probe to the engine and render layers to answer the section 11 question about the paravirtual GPU. Keep the probe's findings in this plan's Results section.

### Step 5: The three-OS matrix

Rewrite `ci.yml` as a matrix, fold in the per-OS setup steps, keep the contact-sheet artifact per OS, add the `render` smoke step, and give each OS its own cache key. Delete the temporary probe job.

### Step 6: Per-adapter golden references

Restructure the reference directory per decision 5, add the `workflow_dispatch` regeneration job, commit the WARP and lavapipe sets (and Metal if the probe allows).

### Step 7: Docs

Update CLAUDE.md (build commands, CI section, environment table if new knobs appear, testing table), `docs/roadmap.md`, and the Results section of this plan with measured runtimes per OS.

## Risks and Mitigations

- **The macOS paravirtual GPU is unproven for our pipeline.** Mitigation: the temporary probe job in Step 4 runs before the matrix depends on it; if the render layer fails there, macOS ships as build-plus-unit-plus-engine coverage and the golden set waits. That outcome still satisfies success criterion 5.
- **Soak-test memory bounds were calibrated on Windows.** Linux and macOS allocators and the different software rasterizers may shift the warm-up size or the growth profile. Mitigation: land the snapshots first, print the numbers from a few CI runs, then set per-OS bounds if the Windows ones do not hold. Do not weaken the Windows bounds.
- **lavapipe or Metal precision breaks a shader-test invariant.** The GPU tests assert invariants rather than exact pixels, so this should be rare; if an invariant genuinely does not hold on a conformant adapter, the test is wrong, and the fix is a documented tolerance adjustment, not a per-OS skip.
- **Slint build dependencies on the Ubuntu runner.** The runner image may lack a library Slint's winit backend needs at build or test time. Mitigation: Slint documents its Linux dependencies; install them explicitly rather than relying on the image.
- **CI cost triples.** Accepted by the plan; the Windows warm-cache baseline (criterion 7) guards the inner loop, and macOS/Linux queue time is tolerable for a stacked draft PR.
- **The reformat commit conflicts with the stacked PRs below.** It lives at the top of the stack, so it rebases mechanically when #21 and #22 move; nothing below reformats.

## Rollback Strategy

Each step is a separate commit or small commit series on `feat/phase2-cross-platform`; reverting a step reverts its commits. The matrix change is one commit that can be reverted to restore the single Windows job without touching the code gates. Nothing in this phase migrates data or changes persisted config.

## Status

In progress on `feat/phase2-cross-platform` (PR #24).

- Step 1, reformat and the `fmt` gate: done.
- Step 2, warning cleanup and `-D warnings`: not started.
- Step 3, Linux: not started.
- Step 4, macOS probe: not started.
- Step 5, the matrix: not started.
- Step 6, per-adapter goldens: not started.
- Step 7, docs: not started.

## Deviations

(To be filled during implementation.)

## Results

(To be filled during implementation: per-OS CI runtimes, soak numbers per OS, the macOS probe outcome, and golden reference status per adapter.)
