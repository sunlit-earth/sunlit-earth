# Testing

The test layers, the conventions every layer follows, and the hosted CI that runs them. What each platform can and cannot assert, and how the desktop e2e cases gate themselves, is in [platforms.md](platforms.md); running the e2e suite in a local VM is in [vm-setup.md](vm-setup.md).

## Layers

| Layer | Where | What | Runs on |
|---|---|---|---|
| Unit + property | both crates | pure functions, `proptest` invariants | all three |
| Engine integration | `sunlit-core/tests/engine/` | real engine, real GPU, headless | all three |
| Soak | `sunlit-core/tests/soak.rs` | mock clock, fixture cloud, 14 simulated days | all three |
| Golden images | `sunlit-core/tests/golden.rs` | fixed scenes, software adapter, perceptual tolerance | all three, per-adapter references |
| GPU shader | `sunlit-core/tests/{shading,render_pipeline}.rs` | real WGSL on the GPU | all three |
| UI logic | `sunlit-app/tests/slint_ui.rs` | `i-slint-backend-testing` | all three |
| Desktop e2e | `sunlit-app/tests/e2e.rs` | the real binary over IPC, `#[ignore]`d | built everywhere; `cargo e2e` on the desktop, `cargo xtask e2e --target <windows\|linux> [--screens <n>]` in a VM |
| VM orchestration | `crates/xtask/src/**` | pure decision logic against fabricated hosts, no VM | all three |

## What a display-less machine can prove about several displays

Almost all of the multi-monitor work is decided on a machine with one screen, and that is a property of how the code is split rather than a happy accident. `display::layout` is a pure function from a list of rectangles to a canvas size, a modified framing and a set of crops, so every layout question is answered against fabricated monitor lists: side by side, stacked, unequal heights, a portrait secondary, negative origins, a gap, two identical rectangles, one monitor, no monitors, a monitor with a zero dimension. `tests/engine/`'s `RecordingSink` reports a fabricated layout to a real engine and keeps what it was handed, which is what proves the engine asks for the right images in each mode with no display anywhere; `desktop.rs`'s table is asserted per row against fabricated two-monitor sessions; and the two GPU cases render a real canvas and compare the anchor's crop against its standalone render. All of that runs in CI on all three operating systems.

What is left is exactly three things, and each has one place it can be checked. That a real session's monitors come back with the rectangles it actually has is answered by `test_displays_reports_the_session_layout` in the guest, which compares what the running app reports over IPC against what the test process gets from the same platform query. That a real desktop holds what the mode said it should is answered by `test_set_wallpaper` and `test_across_screens_writes_what_this_desktop_can_hold`, which read the setting back out of the desktop rather than trusting an exit code. That a layout which really changes reaches the app is answered by `test_a_layout_change_republishes_the_wallpaper`, which switches an output off with `xrandr` from the test process and waits for the app's own account of what happened.

The display-change path is the same split. The settle, the comparison and the two edges of the republish rule are five cases in `tests/engine/` on a `MockClock`, with a `RecordingSink` whose monitor list moves under the running engine and whose query count is what says a settled burst was one query. `display::watch` itself is start-and-stop on each platform: on Linux, against whatever X server the run has, which proves the connection, the extension check, the `select_input` and the wake, and skips with a printed reason where `DISPLAY` is unset; on Windows, by sending `WM_DISPLAYCHANGE` to the listener's own window the way `session_end`'s test does, which is the only way to produce it in a guest whose video is a single fixed mode. `tests/slint_ui.rs` takes the window from two screens to one and back and asserts the combo rows, the tile count, the hidden group and the anchor returning to its own row.

`test_a_layout_change_republishes_the_wallpaper` is what none of that reaches: a real RandR change waking a real watcher, the settle collapsing a real burst, and a real desktop being handed images for the layout it has now. It asks the session which change it can make. With two outputs it switches the non-primary one off, which is the case the feature was written for and the only one that moves the screen count. With one it switches that output to another of its modes, which is the same event and the same comparison, and is what runs on a host whose QEMU cannot give the guest two heads. Either way it waits for `SIGNAL:displays_changed` and then for the publish, checks that every path changed and that the sizes match the layout the session now has, and puts the layout back from a guard so a failing assertion cannot leave the guest one-screened, or at the wrong mode, for the cases after it. It checks that its own change took before waiting on anything, and skips with a printed reason where the session put its layout straight back. What is left after that is a Windows-only hand check on real hardware, because no guest can undock a notebook; `docs/roadmap.md` carries it beside the other Windows questions.

The two-head guest is `cargo xtask e2e --target linux --screens 2`, and what it costs the image is two packages: `xinput`, without which QEMU's single absolute pointer covers the whole desktop and clicks at twice the x it was aimed at, and `arandr`, a display settings UI that works in every session where the minimal Plasma install has none. Measured in the Debian guest on 2026-08-31: both connectors come up connected at the rectangles the boot placed them at, `display::monitors()` reports exactly those, and the pointer maps to the primary output. What the guest cannot show is Windows' own behavior with more than one monitor: the Hyper-V synthetic adapter has no multi-monitor mode, so the Windows guest exercises the whole new path, COM plumbing and id mapping and read-back included, at one monitor only. Mixed DPI is a Windows question that no guest of either kind answers today; see [platforms.md](platforms.md).

## Conventions

- **Test behavior, not constants.** Changing a preset or a default must not break a test.
- **Float comparisons**: `approx::assert_relative_eq!`. `tests/shading.rs` predates this and keeps its own GPU tolerance pattern.
- **GPU tests assert invariants** (monotonicity, bounds, visibility), not exact pixels, because of cross-adapter float variance.
- **One GPU device at a time.** Per-test device creation crashes on Windows. Shader tests share a device through `LazyLock<Mutex<GpuContext>>`; the engine and soak targets each hold a `GPU_SERIAL` mutex for the lifetime of their engine, and the golden target serializes on the `LazyLock<Mutex<EngineHandle>>` it shares one engine through.
- **One wgpu instance per process, ever.** `wgpu_init::instance()` holds it in a `OnceLock` and nothing else may call `wgpu::Instance::new`; `clippy.toml` enforces that through `disallowed-methods`, so a second call site has to allow the lint by name. An instance owns the loaded driver libraries, and dropping the last one `dlclose`s the Vulkan loader while Mesa's pthread TLS destructors still point into it, so the next thread to exit dies in `__nptl_deallocate_tsd`. That is not theoretical: it killed all 14 engine tests on lavapipe.
- **Golden images** force the software adapter where the platform has one, so a developer machine and a CI runner compare against the same references. References are per adapter (`tests/golden/warp/`, `lavapipe/`, `metal/`), keyed by `wgpu_init::adapter_key`, which carries the rule; the measured cross-adapter deltas that justify the split are under "Measurements behind the thresholds" below. Which adapters have a set is listed in the test's `GENERATED_ADAPTERS`, not inferred from the filesystem: an adapter on the list whose directory is missing fails, and only an adapter that has genuinely never been generated skips. A missing single case fails every run, with its render written under `CARGO_TARGET_TMPDIR` for review rather than into the tracked tree. Tolerance: mean channel difference under 2/255 and at most 1% of pixels differing by more than 24. A companion test asserts every pair of references is distinguishable, which is what stops the others from becoming vacuous.
- **Every committed bake is compared against a fresh one.** `the_committed_bake_matches_a_fresh_one` in `bake_icon` and `bake_licenses`, and `the_committed_fixture_matches_a_fresh_bake` in `bake_stars`: the outputs of `cargo xtask bake ...` are data in the tree rather than something a build produces, so a source edited without rerunning the bake has to fail the suite. The licence one is the only bake test that can skip: it needs `cargo metadata`, `cargo tree` and the unpacked registry sources the manifests point at, so on a checkout that cannot resolve the tree it prints why and returns, the way the tests that need the real 8K assets do.
- **Scratch directories come from `ScratchDir`** in `crates/sunlit-core/src/test_support.rs`: the name carries the process id and a counter, and `Drop` removes the tree, so two `cargo test` runs over one checkout never share a directory and a test that panics leaves nothing behind. The library reaches it as a `cfg(test)` module; the integration targets take the same file by `#[path]`, which keeps `tests/common`'s GPU context out of a binary that must not create a second device. It roots under `CARGO_TARGET_TMPDIR` where cargo sets one, which is every integration target, and under the system temporary directory for a library's own unit tests. Note that it creates its directory, so a test asserting that production code creates a parent must name a path below the scratch root that does not exist yet, or the assertion is vacuous.
- **`cargo doc --no-deps` does not see inside private items.** A doc link in a private or `pub(super)` item is
  unchecked by the plain form, so it can rot invisibly, and every module split manufactures more such items: an item
  that moves behind a private module keeps a link that pointed at a sibling in its old file.
  `cargo doc --no-deps -p sunlit-core --document-private-items` is the form that sees them. Run 4 found three that way,
  one of them written during the run itself.
- **Soak measurements** take their baseline after warm-up (the first cloud texture and wgpu's allocator pools are a one-off ~85 MiB); the assertion is on the remaining simulated days.
- **A test that needs the real 8K assets skips with a printed reason without them**, rather than failing or passing vacuously: `textures/**` is Git LFS, and a checkout without the objects holds pointer files that exist as far as anything that only asks about existence is concerned, so the check is on size. `lowering_the_resolution_lowers_the_process_footprint` in `tests/engine/` is the one such case, and it costs about 25 seconds where the assets are present. Everything else that needs a texture, including every Moon case, the golden suite's Moon and every panorama case but the two that are about the real asset, uses a generated fixture instead.

## Measurements behind the thresholds

Numbers that justify a tolerance or a budget live here rather than in the source, so that they can be compared and so
that the code says what it does instead of how it was tuned. Each one names the test or the constant it stands behind.

**The cross-adapter deltas behind the per-adapter golden sets.** Measured on the four reference scenes, each against
WARP, at the tolerance above: lavapipe agrees to a mean channel difference of 0.19 to 0.86 with 0.001% to 0.42%
outliers, and the paravirtual Metal device on a `macos-latest` runner to 0.008 to 0.18 with at most 0.008% outliers. One
shared reference set would therefore pass on all three today, but it would spend up to 43% of the mean budget on the
difference between two correct implementations, leaving a regression that size able to hide on one platform while
failing on another. The ordering in those numbers is not the expected one: the two CPU rasterizers are the pair that
disagree most, and Metal, which is both a different shader translation target and an actual GPU, lands about five times
closer to WARP than lavapipe does.

**The memory budget's cold-start figure.** `MEASURED_COLD_START_PEAK` in `memory.rs`'s test module is 2488 MiB, the one
cold-cache startup peak anyone has measured: private bytes, at 8192, in a release build. Every resolution is held to
that one figure rather than to a smaller one derived from it, and that is the point. The peak is dominated by the two 8K
JXL decodes, which a cold downscale cache performs whatever width it was asked for; how much of the resident saving at a
lower width also shows up in the peak is exactly what nobody has measured. Deriving a per-resolution peak from the
budget's own decomposition would make the two move together and assert nothing, and it would let the budget rest on a
saving that may not be there.

The two bounding tests clear it from both sides, and by these margins: 2048 clears the measurement by 121 MiB where 8192
clears it by 633, so a cold-start figure set too low fails at the two lower widths while the widest, which is where the
3 GiB total is anchored, still passes. The narrow end is the binding case rather than a restatement of the wide one: it
gets the smallest resident allowance and has the same decode to pay for.

**The e2e render case's thresholds and its budget.** `test_render_and_exit` gives itself an empty cache directory so
that it pays the surface texture decode instead of inheriting a warm cache from whichever case ran first. Measured in
the Linux guest on 2026-09-01, the same 800x800 render peaked at 2088 MB and settled to 444 MB with a cold cache, and
sat flat at 381 MB with a warm one, where the peak is the last sample and "settled" means nothing. Its one-minute budget
comes from the same date on the development host, debug build, empty cache: 5.9 s on the GPU and 8.0 s on the software
adapter, against 2.5 s warm. A guest is a software rasterizer on a slower CPU and has never been timed, which is what
the margin is for.

**The e2e suite's own budget**, defined as every wait consuming its full timeout and then succeeding, is about 67
minutes on Linux and 45 on Windows, dominated by the two cases that publish a wallpaper five and six times. A real
Windows guest run is about 97 seconds. `xtask`'s `job_timeout` sits above the budget rather than near the real runtime,
so that a stuck case reports the signal it was waiting for instead of the job reporting that it ran out of time.

**Why three golden cases compare a window, and why two turn a parameter up.** The suite's tolerance is a mean channel
difference under 2.00 with at most 1 percent of pixels off by more than 24, measured over whatever region the case
compares. A feature a few dozen pixels across cannot move a 512 by 256 frame past that, so three cases compare a window
instead, and two more raise a parameter until the effect is larger than the tolerance. Each row below compares a
reference against the same scene with the named feature deleted.

| Case | Feature deleted | Over the whole frame | Over the case's own window |
|---|---|---|---|
| `moon_crescent` | the Moon | mean 0.22, passes | mean 3.12, 1.71 percent outliers |
| `sun_rising_through_the_band` | the refraction tint | mean 10.04, 16.80 percent | mean 55.33, 83.09 percent |
| `sun_rising_through_the_band` | the exposure gain | mean 7.27, 11.00 percent | mean 56.03, 98.58 percent |
| `sun_rising_through_the_band` | the lift and the squash | mean 0.91, 0.22 percent, passes | mean 6.19, 4.58 percent |
| `sunrise_band` | the forward lobe | mean 0.21, 0.40 percent, passes | mean 1.52, 2.83 percent |

The two turned-up parameters are the same argument without a window. `sun_grazing_the_limb` runs `sun_glow` at 1.6
because at the default strength losing the warm shift entirely comes to a mean of 2.33 against a tolerance of 2.00 and
1.04 percent outliers against a limit of 1.00, which is a test that passes or fails on rounding. `sunrise_band` runs
`atmo_sunrise_glow` at 3.0 for the same reason. Both sun cases also run at a 60 degree sky rather than the 140 degree
default, for the reason `docs/rendering.md` gives beside the four goldens.

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
