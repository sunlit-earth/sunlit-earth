# Testing

The test layers, the conventions every layer follows, and the hosted CI that runs them. What each platform can and cannot assert, and how the desktop e2e cases gate themselves, is in [platforms.md](platforms.md); running the e2e suite in a local VM is in [vm-setup.md](vm-setup.md).

## Layers

| Layer | Where | What | Runs on |
|---|---|---|---|
| Unit + property | both crates | pure functions, `proptest` invariants | all three |
| Engine integration | `sunlit-core/tests/engine.rs` | real engine, real GPU, headless | all three |
| Soak | `sunlit-core/tests/soak.rs` | mock clock, fixture cloud, 14 simulated days | all three |
| Golden images | `sunlit-core/tests/golden.rs` | fixed scenes, software adapter, perceptual tolerance | all three, per-adapter references |
| GPU shader | `sunlit-core/tests/{shading,render_pipeline}.rs` | real WGSL on the GPU | all three |
| UI logic | `sunlit-app/tests/slint_ui.rs` | `i-slint-backend-testing` | all three |
| Desktop e2e | `sunlit-app/tests/e2e.rs` | the real binary over IPC, `#[ignore]`d | built everywhere; `cargo e2e` on the desktop, `cargo xtask e2e --target <windows\|linux> [--screens <n>]` in a VM |
| VM orchestration | `crates/xtask/src/**` | pure decision logic against fabricated hosts, no VM | all three |

## What a display-less machine can prove about several displays

Almost all of the multi-monitor work is decided on a machine with one screen, and that is a property of how the code is split rather than a happy accident. `display::layout` is a pure function from a list of rectangles to a canvas size, a modified framing and a set of crops, so every layout question is answered against fabricated monitor lists: side by side, stacked, unequal heights, a portrait secondary, negative origins, a gap, two identical rectangles, one monitor, no monitors, a monitor with a zero dimension. `tests/engine.rs`'s `RecordingSink` reports a fabricated layout to a real engine and keeps what it was handed, which is what proves the engine asks for the right images in each mode with no display anywhere; `desktop.rs`'s table is asserted per row against fabricated two-monitor sessions; and the two GPU cases render a real canvas and compare the anchor's crop against its standalone render. All of that runs in CI on all three operating systems.

What is left is exactly two things, and each has one place it can be checked. That a real session's monitors come back with the rectangles it actually has is answered by `test_displays_reports_the_session_layout` in the guest, which compares what the running app reports over IPC against what the test process gets from the same platform query. That a real desktop holds what the mode said it should is answered by `test_set_wallpaper` and `test_across_screens_writes_what_this_desktop_can_hold`, which read the setting back out of the desktop rather than trusting an exit code.

The two-head guest is `cargo xtask e2e --target linux --screens 2`, and what it costs the image is two packages: `xinput`, without which QEMU's single absolute pointer covers the whole desktop and clicks at twice the x it was aimed at, and `arandr`, a display settings UI that works in every session where the minimal Plasma install has none. Measured in the Debian guest on 2026-08-31: both connectors come up connected at the rectangles the boot placed them at, `display::monitors()` reports exactly those, and the pointer maps to the primary output. What the guest cannot show is Windows' own behaviour with more than one monitor: the Hyper-V synthetic adapter has no multi-monitor mode, so the Windows guest exercises the whole new path, COM plumbing and id mapping and read-back included, at one monitor only. Mixed DPI is a Windows question that no guest of either kind answers today; see [platforms.md](platforms.md).

## Conventions

- **Test behavior, not constants.** Changing a preset or a default must not break a test.
- **Float comparisons**: `approx::assert_relative_eq!`. `tests/shading.rs` predates this and keeps its own GPU tolerance pattern.
- **GPU tests assert invariants** (monotonicity, bounds, visibility), not exact pixels, because of cross-adapter float variance.
- **One GPU device at a time.** Per-test device creation crashes on Windows. Shader tests share a device through `LazyLock<Mutex<GpuContext>>`; engine, soak, and golden tests each hold a `GPU_SERIAL` mutex for the lifetime of their engine.
- **One wgpu instance per process, ever.** `wgpu_init::instance()` holds it in a `OnceLock` and nothing else may call `wgpu::Instance::new`; `clippy.toml` enforces that through `disallowed-methods`, so a second call site has to allow the lint by name. An instance owns the loaded driver libraries, and dropping the last one `dlclose`s the Vulkan loader while Mesa's pthread TLS destructors still point into it, so the next thread to exit dies in `__nptl_deallocate_tsd`. That is not theoretical: it killed all 14 engine tests on lavapipe.
- **Golden images** force the software adapter where the platform has one, so a developer machine and a CI runner compare against the same references. References are per adapter (`tests/golden/warp/`, `lavapipe/`, `metal/`), keyed by `wgpu_init::adapter_key`; the reasoning and the measured cross-adapter deltas are on that function. Which adapters have a set is listed in the test's `GENERATED_ADAPTERS`, not inferred from the filesystem: an adapter on the list whose directory is missing fails, and only an adapter that has genuinely never been generated skips. A missing single case fails every run, with its render written under `CARGO_TARGET_TMPDIR` for review rather than into the tracked tree. Tolerance: mean channel difference under 2/255 and at most 1% of pixels differing by more than 24. A companion test asserts every pair of references is distinguishable, which is what stops the others from becoming vacuous.
- **Soak measurements** take their baseline after warm-up (the first cloud texture and wgpu's allocator pools are a one-off ~85 MiB); the assertion is on the remaining simulated days.
- **A test that needs the real 8K assets skips with a printed reason without them**, rather than failing or passing vacuously: `textures/**` is Git LFS, and a checkout without the objects holds pointer files that exist as far as anything that only asks about existence is concerned, so the check is on size. `lowering_the_resolution_lowers_the_process_footprint` in `tests/engine.rs` is the one such case, and it costs about 25 seconds where the assets are present. Everything else that needs a texture, including every Moon case, the golden suite's Moon and every panorama case but the two that are about the real asset, uses a generated fixture instead.

## CI/CD

Three GitHub Actions workflows in `.github/workflows/`:

- **`ci.yml`**: `workflow_dispatch` only. It ran on every push and PR until the hosted minutes ran out; this repository is private, so they are billed, and the local VM suite covers what the runners were for. What a manual run still buys is the other two operating systems, so dispatch it before anything that has to hold on all three. A `fmt` job runs `cargo fmt --check` once on Ubuntu, and a `test` matrix runs `cargo test --locked -- --show-output` plus a headless `render` smoke test on `ubuntu-latest`, `windows-latest`, and `macos-latest`, uploading the contact sheet and the smoke render per OS.
- **`golden.yml`**: `workflow_dispatch` only. Pick an OS, run it, download the `golden-<adapter>` artifact, review the images, commit them. It regenerates, reads the adapter key from the test's marker line, runs the suite again without `SUNLIT_EARTH_UPDATE_GOLDEN` so the job verifies what it produced, and uploads only that adapter's directory. GitHub only registers dispatchable workflows from the default branch, so it is usable once merged; before that, regenerate locally on the adapter in question.
- **`release.yml`**: on semver tag pushes (`v[0-9]+.[0-9]+.[0-9]+`). Builds `cargo build --release --locked`, zips `target/release/sunlit-earth.exe`, and creates a GitHub Release. Windows only; cross-platform release artifacts are still a roadmap item.

Key details:

- `fail-fast: false` on the matrix. All three results, every time: cancelling macOS because Linux failed costs a round trip to learn something the same run already knew.
- Per-OS setup, all of it explicit rather than relied on from the runner image: LLVM 19 pinned on Windows via `KyleMayes/install-llvm-action@v2` with `LIBCLANG_PATH`; Slint's build dependencies plus `mesa-vulkan-drivers` and `xvfb` via apt on Ubuntu; Xcode's `libclang.dylib` located defensively on macOS.
- Linux tests run under `xvfb-run -a`. Nothing opens a window today, so this is for the first windowed test to arrive. It does propagate the exit status, so a failing suite still fails the job.
- The render smoke test is deliberately **not** wrapped in xvfb: `run_render` claims to need no window, and running it with no `DISPLAY` is what makes that a tested claim. It asserts the output is a 640x360 PNG by reading the IHDR header, not that the file is merely non-trivial in size.
- `--show-output` on the test step, because the numbers worth having from a CI run come from tests that pass: the golden suite's adapter key and the soak test's per-OS memory profile. libtest discards those otherwise.
- Both `ci.yml` and `golden.yml` declare `permissions: contents: read` at the top level. `release.yml` needs write and says so itself.
- Cache keys are `ci-linux` / `ci-windows` / `ci-macos`. The Windows one is spelled out rather than derived from the runner label so it kept the key it had before the matrix.
- The first run on a new PR builds cold (roughly 45 minutes on Windows) because Actions caches are scoped per merge ref. Later runs on the same PR restore it. That is expected, not a regression.
- All `cargo` commands use `--locked`. `Cargo.lock` lives at the workspace root.
- GPU tests run on the software adapter where the platform has one: WARP on Windows, lavapipe on Linux. macOS has no CPU adapter, so it falls back to the runner's paravirtual Metal GPU, which is a real one.
- Clippy runs locally only (its artifacts are incompatible with the test cache and force full recompilation). `-D warnings` in CI covers rustc's own lints.
