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

Implemented on `feat/phase2-cross-platform` (PR #24). All seven steps are done and every success criterion is met.

- Step 1, reformat and the `fmt` gate: done, green in CI (run 31911197718).
- Step 2, warning cleanup and `-D warnings`: done, green in CI (run 31911787852).
- Step 3, Linux: done. Full suite green in WSL against lavapipe, under `xvfb-run -a` and with `-D warnings`, and green on the Ubuntu runner. The Windows suite (380 tests) and the desktop e2e suite (8 tests) were re-run after the changes and stay green.
- Step 4, macOS probe: done. See the probe section below; the paravirtual Metal GPU runs the whole pipeline.
- Step 5, the matrix: done, green on all three OSes (run 31914066981).
- Step 6, per-adapter goldens: done. Directory restructure, the `workflow_dispatch` regeneration workflow, and all three reference sets (`warp`, `lavapipe`, `metal`).
- Step 7, docs: done. CLAUDE.md, `docs/roadmap.md`, and this document.

Against the success criteria:

1. CI green on all three OSes with both gates enabled: yes.
2. `cargo test` passes on Linux and macOS with the soak assertions live: yes, and no per-OS calibration was needed.
3. `render` produces a PNG on all three in CI: yes, from the matrix run, 70452 bytes on Windows (WARP), 70590 on Linux (lavapipe), 69639 on macOS (Metal), each logging "no texture files found, rendering the procedural grid".
4. `memory::snapshot()` returns `Some` everywhere: yes, asserted by `snapshot_returns_some_on_every_supported_platform`.
5. References for at least WARP and lavapipe: exceeded, all three exist.
6. No new unconditional `unsafe`: yes, the one new FFI site is a scoped allow with a `// SAFETY:` comment.
7. Windows runtime close to baseline: yes, 20 m 16 s against a 44 m 18 s cold first run.

## Deviations

1. **`mach2` is a dependency, where decision 2 said "instead of a crate dependency".** That phrase rules out a memory-measurement crate such as `sysinfo`, and the same decision asks the macOS path to match "the existing Windows FFI pattern", which is `windows-sys` (raw declarations) plus our own scoped `unsafe` and `// SAFETY:` comment. `mach2` is the macOS equivalent of exactly that: declarations only, no logic, and libc's own deprecation notice on `mach_task_self` points at it. Hand-rolling `task_vm_info` was the alternative and was rejected: it is a 30-field struct whose layout would have to be transcribed blind, with no macOS hardware to test the transcription on.

2. **Step 6's directory restructure landed with Step 3 rather than after Step 5.** The lavapipe references had to be generated during the WSL session that was already open, and generating them requires the per-adapter layout to exist first. The rest of Step 6 (the `workflow_dispatch` job, the Metal set) stayed in place.

3. **Decision 4's "primary monitor size reported by the windowing layer" is not implemented; the documented 2560x1440 default is used everywhere off Windows.** Slint 1.17's public `Window` API exposes the window's own size, position and scale factor and nothing about the display behind it, so there is no windowing layer to ask. Adding a second windowing dependency to serve a code path that immediately returns "wallpaper setting is not supported on this platform yet" would be the wrong trade. Recorded on the roadmap: the real Linux and macOS wallpaper setters each bring a native display query with them, and that is where this placeholder goes away.

4. **`is_position_on_screen` off Windows became a coordinate-range check rather than staying a no-op.** Not planned, but running the suite on Linux turned `validated_geometry_rejects_off_screen` red, and the honest reading was that the test was right and the code was wrong: the non-Windows branch accepted every coordinate, so config carried from a multi-monitor desk to a laptop restored a window nobody could reach. The fix asserts the portable half of the question (X11 carries window coordinates as `INT16`, and the Windows virtual screen is bounded the same way) so the test now passes on all three platforms instead of being weakened to Windows-only.

5. **`wgpu::Instance` moved into a process-wide `OnceLock`.** Also not planned, and the largest single finding of the phase; see Results. It is a behavior change on every platform, though an invisible one on Windows.

6. **One test now skips on macOS.** `software_adapter_produces_correct_results` asks for a software adapter, and Metal has none to give. The risk this plan warns about is a per-OS skip papering over a real precision difference, so the skip is written to make that impossible: it is conditional on querying the adapter rather than on `cfg!(target_os)`, and it asserts that the adapter *is* present on everything except macOS, so a missing WARP or lavapipe fails the test rather than quietly skipping it. Windows and Linux run the full assertions unchanged, verified at 12 passed on each. No tolerance was touched anywhere.

7. **The `textures_ready` defect is recorded rather than fixed.** Finding it was a side effect of the phase (CI checks out Git LFS pointers, which decode-fail and leave the slot in the same terminal state an unconfigured slot is in), but it is an asset-lifecycle bug that behaves identically on Windows, and fixing it properly means reasoning about the blend-mode composite bind group and the settings window together. Phase 2 takes the two scoped pieces it needs: `run_render` no longer waits when there is no texture file at all, and the CI smoke step points at an empty textures directory so it is deterministic and does not spend two minutes per OS waiting for an event that cannot arrive. The general fix is on the roadmap with the diagnosis written out.

8. **The Metal references came from a temporary step in `ci.yml`, not from `golden.yml`.** GitHub only registers a `workflow_dispatch` workflow once it is on the default branch, so the regeneration workflow this phase adds cannot be dispatched from the branch that adds it. The set was produced by a temporary matrix step that ran the golden test with `SUNLIT_EARTH_UPDATE_GOLDEN=1` and uploaded the directory as an artifact, which was then reviewed, committed, and the step removed. The step deliberately ran *after* the normal test step so the committed references were still compared first and a genuine mismatch could not hide behind the regeneration; the artifact confirmed this by shipping the `warp` and `lavapipe` directories byte-identical to the committed ones.

## Results

### What running on a second OS actually found

Three defects, none of which a Windows-only suite could have shown. This is the return on the phase and worth stating plainly.

**Dropping the wgpu instance unloads the Vulkan loader.** `wgpu_init::init` built a `wgpu::Instance` per engine and let it go at the end of the function, on the stated reasoning that "wgpu's own handles keep whatever they need alive". They do, which is precisely the problem: keeping it alive only defers the unload to whenever the device dies. When the engine thread ended and dropped the device, the last reference went with it, `libvulkan.so.1` was `dlclose`d, and Mesa's pthread TLS destructors were left pointing into an unmapped page. All 14 engine integration tests died with SIGSEGV in `__nptl_deallocate_tsd` while joining the engine thread. The instance now lives in a `OnceLock` for the process. Windows never showed this because unloading the D3D12 runtime is safe; the defect was always in the code, only the platform was forgiving. The test harness in `tests/common` carried the same latent pattern and now shares the one instance.

**`is_position_on_screen` accepted every coordinate off Windows.** Saved geometry from a large multi-monitor desk would restore a window nowhere reachable on a laptop. Now a coordinate-range check; see Deviations 4.

**The memory assertions were silently inert everywhere but Windows.** Not a discovery so much as the thing this phase was for, but worth noting that the soak test printed "no memory counters on this platform, skipping the growth assertion" and passed, which from the outside looks identical to passing for the right reason.

### Soak test, per OS

Same test, same limits: growth under 16 MiB, warm-up under 192 MiB. No per-OS calibration was needed.

| | Windows (WARP) | Linux (lavapipe, WSL) | macOS (Metal) |
|---|---|---|---|
| Wall clock for 14 simulated days | 44.2 s | 14.2 s | 17.4 s |
| Exports / cloud fetches | 336 / 113 | 336 / 113 | 336 / 113 |
| Private at startup | 531.1 MiB | 195.5 MiB | 67.9 MiB |
| Warm-up allocation | +87.6 MiB | +12.2 MiB | +0.0 MiB |
| Growth over 12 simulated days | +2.1 MiB | +0.0 MiB | +1.8 MiB |

The absolute figures differ by up to a factor of eight, which is the counters rather than the program: Windows `PrivateUsage` charges committed-but-not-resident pages, Linux `Private_Clean + Private_Dirty` counts only resident private pages, and macOS `phys_footprint` is a ledger that compression and reclaim can move downward. macOS in fact ends warm-up *below* its startup figure, which is why its warm-up column reads +0.0: the saturating subtraction floors it. The growth row is what the test asserts on, and it is flat on all three.

### Golden references, per adapter

| Adapter | Key | Status |
|---|---|---|
| WARP (Windows, D3D12) | `warp` | committed, unchanged from before the restructure |
| lavapipe (Linux, Vulkan) | `lavapipe` | committed, generated in WSL on Mesa 23.2.1 / LLVM 15.0.7 and confirmed by the matrix run against the runner's LLVM 20.1.2 |
| Metal (macOS) | `metal` | committed, generated on `macos-latest` and uploaded as a CI artifact (see Deviations 8) |

Measured difference between the reference sets, both against WARP, with a tolerance of mean 2.0 and 1% outliers:

| Scene | lavapipe, mean | lavapipe, outliers | Metal, mean | Metal, outliers |
|---|---|---|---|---|
| default | 0.707 | 0.417% | 0.137 | 0.008% |
| nightglow | 0.860 | 0.381% | 0.180 | 0.007% |
| rayleigh | 0.558 | 0.182% | 0.120 | 0.000% |
| close_up | 0.188 | 0.001% | 0.008 | 0.000% |

The paravirtual Metal GPU turns out to agree with WARP roughly five times more closely than lavapipe does, which was not the expected ordering: the two CPU rasterizers are the pair that disagree most.

That the WSL-generated lavapipe set passed unmodified on a runner five LLVM major versions ahead is itself a useful data point: the tolerance absorbs a large shader-compiler difference within one rasterizer family, which is the case it was designed for.

Worth recording because it contradicts the assumption behind decision 5. Every adapter pair agrees well inside the existing tolerance, so a single shared reference set would in fact have passed on all three today, and the case for per-adapter directories is about margin rather than compatibility: one shared set would spend up to 43% of the mean budget (0.860 of 2.0) on the difference between two correct implementations, leaving a regression that size able to hide on one platform while failing on another. Per-adapter references give every platform the whole tolerance to spend on detecting real change, and the numbers above are the evidence for how much that is worth rather than an assertion that it was necessary.

### CI runtimes

Run 31914066981 was the first run of the three-OS matrix and run 31915731976 the first with every cache warm; both green on all four jobs.

| Job | Cold total | Warm total | Warm `cargo test` | Warm smoke |
|---|---|---|---|---|
| Windows | 20 m 16 s (partly warm) | 5 m 48 s | 4 m 11 s | 2 s |
| macOS | 18 m 23 s | 1 m 28 s | 53 s | 1 s |
| Linux | 30 m 25 s | 2 m 59 s | 1 m 53 s | under 1 s |
| Format (Ubuntu) | 12 s | 14 s | n/a | n/a |

Criterion 7 is met: Windows is under six minutes warm. The fully cold first run of the PR took 44 m 18 s on Windows alone, which is what the first run on any new PR costs because Actions caches are scoped per merge ref; three runs on this branch paid it only because they were pushed inside the first one's build window.

The warm figures above are the second measurement, and the first one is why measuring mattered. Warm Windows initially came out at 13 m 35 s, of which the render smoke step was 7 m 17 s and none of that was rendering: `cargo test` builds binaries under the test profile (the e2e suite needs one through `CARGO_BIN_EXE`) and `cargo run` then rebuilt and relinked the same executable under `dev`. Running the binary the test step already produced took the step to 2 seconds and the job to 5 m 48 s. It is also a slightly better test, since it exercises the artifact the suite built rather than a second copy of it.

Green in CI: run 31911197718 (step 1), run 31911787852 (step 2), run 31914066981 (the matrix).

### macOS probe

Run 31912759965, job 95080200216, on `macos-latest`. Adapter: **`Apple Paravirtual device (Metal, IntegratedGpu)`**.

This answers retrospective section 11, question 1 for our pipeline, and the answer is yes. MSAA, mipmapped texture upload, offscreen rendering, and buffer readback all work on the paravirtual device: the engine integration tests and the render-pipeline tests pass there unmodified, and the headless `render` subcommand produced a valid 69672-byte PNG.

| Step | Result |
|---|---|
| Build | success |
| Unit tests | 266 + 38 passed |
| Render smoke test | success, 640x360 PNG |
| Engine integration | 14 passed |
| `render_pipeline` | 19 passed |
| `shading` | 11 passed, 1 failed |
| Soak | passed |
| Golden | 6 passed, comparisons skipped (no `metal` set) |
| `slint_ui` | 17 passed |

The single failure was `software_adapter_produces_correct_results`, and it was not a shading or precision problem. wgpu reports `no_fallback_backends: Backends(METAL)`: the Metal backend exposes no software adapter, so a case that explicitly asks for one has nothing to run. `docs/tech.md` had already written this down during the technology selection ("macOS: No software rendering path. Apple deprecated OpenGL and there's no Mesa equivalent"), so the probe confirmed an existing note rather than discovering something; it was simply never connected to the test that depends on it. The test now checks for the adapter and skips only when it is genuinely absent, and asserts that it is present anywhere other than macOS, so a missing WARP or lavapipe still fails rather than quietly skipping. The other eleven cases in that file already cover the adapter macOS actually uses.

Also found here, though not a macOS issue: `textures/**` is Git LFS, and `actions/checkout` leaves pointer files behind. They resolve as texture paths and then fail to decode as JXL, which clears the slot's path and leaves `textures_ready` permanently false, so `run_render` waited the full 120-second timeout on every OS and logged an error about a problem that did not exist. The smoke step now points `SUNLIT_EARTH_TEXTURES` at an empty directory, which is both faster and an honest statement of what it tests; `run_render` additionally skips the wait when no texture file was found at all. The general defect in `Renderer::textures_ready` is on the roadmap rather than patched here, because it needs the blend-mode composite state and the settings-window path considered together.
