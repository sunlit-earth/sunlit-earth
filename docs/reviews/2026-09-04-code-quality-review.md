# Code Quality and Resilience Review

Date: 2026-09-04. Tree: commit `3046327` (0.1.0-beta.1). Scope: `crates/sunlit-core` and `crates/sunlit-app`, source and tests, plus the shaders and `ui/main.slint`. Status: findings and recommendations only, nothing implemented.

The maintainer asked for a pre-release look at code quality and resilience with four concerns named up front: verbose and redundant commentary, source files over a thousand lines, inconsistent file structure, and unit tests that take long for the value they bring. This document is the synthesis. The per-area notes it draws on, with every finding at `file:line`, are in [2026-09-04-code-quality/](2026-09-04-code-quality/) and are referenced below as "the notes".

## 1. Summary

The code is correct and defensive. Ten reviewers read every line of both crates and found no data race, no unbounded growth, no leak on a normal path, and only a handful of panics reachable in production, all of them on startup or on an operator-supplied argument. The memory rules from the August retrospective hold (section 6.1), the engine's scheduling contract holds, and 49 of the 50 `#[allow(unsafe_code)]` sites carry a real `SAFETY` argument. The problems are volume, duplication, public surface, and a test suite whose run time is dominated by one avoidable cost.

The findings that matter most, in order:

1. **`tests/engine.rs` starts 91 wgpu devices for 78 tests.** Each start costs about 1.2 s (adapter, device, seven pipelines compiled from `sphere.wgsl`, grid mips, star catalog), which is 109 s of the target's 161.6 s. `golden.rs` in the same directory already shares one engine across 20 cases and runs in 4 s. Sharing an engine per configuration group takes the target to roughly 80 s with no assertion removed. `soak.rs` adds 65.6 s to every `cargo test` and has drifted 47% past the figure its own comment records. The 1215 unit tests together take 4.2 s; they are not the problem.
2. **The `Uniforms` layout is not tested against the shader.** The offset test in `render_pipeline.rs` checks a hand-written copy of the struct, not the production one, because `renderer::uniforms` is `pub(crate)`. Swapping two adjacent `f32` fields in production passes every test. One visibility change fixes it and deletes about 150 lines of test-only copies.
3. **Startup failures reach the user as Rust panics.** No usable GPU adapter panics the engine thread and then `engine::start`, with no message and no panic hook. `--ipc-socket` with a bad or busy name panics. A missing tray host panics (plausible, not verified). `MainWindow::new`, `show` and the event loop each `expect` on a `PlatformError`. Each is a small change to return `Result` and print one sentence.
4. **About a thousand comment lines are a second copy of `docs/`, a bug story, a plan reference, or a restatement of the next line.** The density is not uniform: `engine/mod.rs` and `main.slint` mostly earn their comments, `display/`, `desktop.rs`, `assets/` and the integration tests mostly do not. Sixteen comments are factually wrong or attached to the wrong item (section 4.1).
5. **Duplication that will drift.** The ten render pipelines are built twice, verbatim (`gpu_setup.rs`); the preview and export passes repeat 45 lines; the Arc-dedupe wallpaper write loop exists on both platforms; the app data directory is spelled four times; the gamma slider curve is re-implemented in Slint; `layout::Framing` is built from `SceneParams` in both crates; the `SIGNAL:` line formats are written on both sides of the process boundary with nothing linking them. Section 4.4 has the full table.
6. **Adding a shader parameter touches 16 production sites, not the six CLAUDE.md lists**, and the "table-driven" digest test cannot catch a parameter that was never added to its table.
7. **About 90 `pub` items are used only inside their own crate**, and nine are dead. `Renderer` alone exposes 13 methods only the engine calls. This is cheap to fix before a first release and a breaking change after it.
8. **33 tests pin defaults or presets**, against the project's own rule. `config.rs` holds 16 of them. About 210 of the 1407 tests are redundant with a neighbor and can be merged or deleted with no coverage lost.
9. **Three per-OS code conventions coexist** in one crate, and `display.rs` is the only multi-file module without `mod.rs`.
10. **Of the nine Rust source files over 1000 lines, only two are long in production code.** `engine/mod.rs` (1395) and `wallpaper.rs` (1106, of which 670 are Windows-only). The other seven are under 850 once the test module is set aside, and five are under 700.

Everything here is fixable in roughly two weeks of focused work (section 5). The first two phases, about four days, carry most of the value.

## 2. Method

- Ten parallel reviewers, each assigned an area and told to read every file in full, classify every comment block over three lines (keep, move to docs, delete), propose split seams for long files with line ranges, check visibility against the other crate and the test targets by grep, audit every `expect` and `unsafe`, and classify every test. They were forbidden from running cargo, so every runtime claim in the notes is reasoned from the code and marked as such.
- One timed run of the whole workspace: `cargo test --workspace -- --report-time --test-threads=1`, debug profile, Windows host. Wall time 8m49s from a cold target directory including compilation. The per-test figures are in [2026-09-04-code-quality/test-timing.md](2026-09-04-code-quality/test-timing.md).
- `cargo clippy --all-targets`: zero warnings.
- A line-classification pass over every source file (code, doc comment, line comment, blank, test module).
- The Apollo Rust best-practices handbook was read for its chapters on idioms, errors, tests and comments and used as a checklist, not a standard.

Every finding below cites the notes it comes from. Claims the reviewers could not confirm are marked unverified there and are repeated here only where they matter.

## 3. Metrics

### 3.1 Size

| | Lines |
|---|---|
| Whole tree under review | 43,522 |
| Production Rust, `sunlit-core/src` | ~13,500 |
| Production Rust, `sunlit-app/src` | ~3,300 |
| Unit-test modules inside source files | ~10,000 |
| Integration tests (`tests/` in both crates) | 13,772 |
| WGSL | 1,188 |
| Slint | 1,644 |

Tests are 55% of the tree. Inside `sunlit-core/src`, 40% of the lines are `mod tests`.

### 3.2 Files over 1000 lines

| File | Total | Production | Test module | Long because |
|---|---|---|---|---|
| `sunlit-core/tests/engine.rs` | 4765 | | | 78 scenarios, 91 engine starts, shared helpers stranded 4400 lines from their type |
| `sunlit-app/tests/e2e.rs` | 2862 | | | 15 cases, harness only half shared, 55 numbered step comments |
| `sunlit-core/tests/render_pipeline.rs` | 2506 | | | one 464-line test plus a 173-line copy of the WGSL uniform struct |
| `sunlit-app/ui/main.slint` | 1644 | | | one `MainWindow`; 832 lines are the Advanced section, which Slint cannot factor without adding lines |
| `sunlit-core/src/config.rs` | 1616 | 686 | 930 | four concerns; 73 tests |
| `sunlit-core/src/wallpaper.rs` | 1609 | 1106 | 502 | 670 production lines are Windows-only |
| `sunlit-core/src/engine/mod.rs` | 1485 | 1395 | 90 | six separable concerns; the loop itself is 43 + 69 lines |
| `sunlit-core/src/scene/sun_occlusion.rs` | 1449 | 732 | 717 | five concerns under one name |
| `sunlit-core/src/assets/cloud_fetcher.rs` | 1391 | 538 | 853 | test module larger than the code |
| `sunlit-core/src/desktop.rs` | 1262 | 672 | 590 | XFCE and KDE machinery inline |
| `sunlit-app/tests/slint_ui.rs` | 1165 | | | 45 tests, about 16 redundant |
| `sunlit-core/src/renderer/mod.rs` | 1147 | 841 | 306 | sizing and slot policy beside the `Renderer` |
| `sunlit-core/shaders/sphere.wgsl` | 1144 | | | 30% comment lines; no function over 63 lines |
| `sunlit-core/src/params.rs` | 1112 | 449 | 663 | one 445-line hand-written mutation table |
| `sunlit-core/src/display/layout.rs` | 1066 | 459 | 607 | 33 tests |
| `sunlit-core/tests/golden.rs` | 1037 | | | 31% comment lines, mostly measurements |

Just under the line: `gpu_setup.rs` (934, no tests, ten near-identical pipeline builders constructed twice), `main.rs` (969, a 200-line `run_app`), `memory.rs` (859), `wallpaper_sink.rs` (755), `camera.rs` (742, of which 279 are production), `mouse_math.rs` (739, of which 243 are production).

### 3.3 Tests and their cost

| Target | Tests | Time (serial) | Notes |
|---|---|---|---|
| `sunlit-core` unit | 537 | 2.3 s | slowest 0.93 s (a proptest in `texture_loader`) |
| `sunlit-earth` unit | 76 | 0.03 s | |
| `xtask` unit | 604 | 2.2 s | out of scope |
| `tests/engine.rs` | 78 | **161.6 s** | 76 tests over 1 s; floor 1.2 s per engine start |
| `tests/soak.rs` | 1 | **65.6 s** | runs in every `cargo test`; own comment says 44.6 s |
| `tests/golden.rs` | 20 | 4.0 s | one shared engine, 27 software renders |
| `tests/render_pipeline.rs` | 21 | 2.1 s | one shared device |
| `tests/shading.rs` | 12 | 1.6 s | |
| `tests/slint_ui.rs` | 45 | 0.6 s | |
| `tests/e2e.rs` | 15 | ignored | worst-case internal budget 53 min on Linux, above xtask's 30 min job timeout |

Two targets are 96% of the test time. Every other target, unit tests included, is under 5 s.

### 3.4 Other counts

| | Count |
|---|---|
| `#[allow(...)]` in the two crates (src and tests) | 207 |
| `#[expect(...)]` | 2 |
| `#[allow]`s that appear to suppress nothing (by reading; clippy cannot report these) | 10 |
| Files without a `//!` module doc | 15 |
| `pub` items with no caller outside their crate | ~90 |
| `pub` items with no caller at all | 9 |
| Tests that pin a default or preset | 33 |
| Tests recommended for merge or deletion | ~210 of 1407 |
| Comments that are factually wrong or attached to the wrong item | 16 |
| Comment lines that duplicate `docs/`, narrate history, or restate the next line | ~1000 |

## 4. Findings

### 4.1 Commentary

The problem is kind, not volume. Comment density ranges from 5% (`gpu_setup.rs`, which is under-documented) to 97% of code lines (`wgpu_init.rs`), and the files the reviewers rated as well commented (`engine/mod.rs`, `textures.rs`, `main.slint`, `support/mod.rs`) are among the denser ones. What the maintainer's rules exclude, and what the reviewers found in every area, falls into five kinds.

**A second copy of `docs/`.** The display reviewer sampled 18 long comment blocks in `desktop.rs`, `display/` and `layout.rs` and grepped a distinctive phrase from each against `docs/`: 17 were already in `docs/platforms.md` or `docs/rendering.md`, mostly near verbatim and in more detail. The same holds for `mailbox.rs:24-31` and `:60-74`, `cloud_fetcher.rs:526-530`, `texture_cache.rs:1-13` (all in `architecture.md`), `session_end.rs:1-35` (`architecture.md:84`), the moon pipeline doc at `gpu_setup.rs:521-527` (a third copy after `rendering.md:52` and `sphere.wgsl:270-281`), and the missing-globe-texture argument written at `main.rs:275`, `:436`, `:942` and `roadmap.md:99`. About 220 lines in the display area alone. Recommendation: keep the one sentence a reader of that function needs, and let the document carry the argument.

**History, incidents and plan references.** "phase 5 decision 8" (`session_end.rs:321`), "phase 5 decision 9" (`desktop.rs:536-541`), "departure 9 in the plan" (`tests/engine.rs:1259`), "the amendment's criterion 4" (`:404`), "Step 3" (`:1892`), six `(Step X.Y)` banners in `render_pipeline.rs` and three in `slint_ui.rs`, "(from Step 4 on)" in `lib.rs:4`, "Phase 0" in `engine_client.rs:8` and `mailbox.rs`. Bug narratives: `memory.rs:26-29` (the ten-day leak), `wallpaper.rs:1305-1309` and `:1409-1411`, `desktop.rs:216-219` and `:965-969` (the XFCE guest), `main.rs:782-786` and `:802-806`, `engine.rs:1614-1618`, `:4301-4309`, `:4345-4357`, `:4441-4453`, `golden.rs:64, 184, 275, 777, 945`, `e2e.rs:968-977`, `:1004-1012`, `:1618-1626`, `:2768-2784`. All of these state a real property in their first sentence and then tell the story; the story belongs in the commit.

**Measurements in source.** `wgpu_init.rs:113-146` (34 lines of per-adapter channel differences), eleven blocks in `golden.rs` ("losing the warm shift entirely comes to a mean of 2.33 against a tolerance of 2.00"), `memory.rs:743-753` (exact clearance figures per resolution), `wallpaper_sink.rs:138-145`, `sun_occlusion.rs:794-796` and `:967-972`, `sky.rs:458`, `mouse_math.rs:463-466`, `e2e.rs:968-977` (timings on a named host on a named date). These are useful facts in the wrong place: they justify a tolerance, so they belong in a table in `docs/testing.md` or `docs/rendering.md` where they can be compared, with a one-line pointer left behind.

**Test doc comments explaining why the test exists.** This is a house style rather than an accident, and the largest single category by line count: about 13 blocks in `cloud_fetcher.rs`'s test module, a dozen across `wallpaper.rs` and `wallpaper_sink.rs`, four in `mouse_math.rs`, 28 across `config.rs`, `memory.rs` and `memory_report.rs`, and the 55 numbered step comments in `e2e.rs` (`// 4. Assert exit code is 0`). The maintainer's rule names this category explicitly. The exceptions worth keeping are the ones that explain a non-obvious setup mechanism (`texture_cache.rs:555-557`, `cloud_fetcher.rs:1234-1237`, `slint_ui.rs:1001-1009`).

**Restating the next line.** `gpu_setup.rs:49, 84`, `renderer/mod.rs:733, 745`, `render_pass.rs:353, 360`, `wallpaper.rs:1052-1079` (five in a row), `texture_loader.rs:111, 143`, `camera.rs:247-260`, `sun.rs:26-27`, about twelve in `render_pipeline.rs`. Delete.

**Comments that are wrong.** These are the ones to fix first, because a wrong comment is worse than none:

| Where | What it says | What is true |
|---|---|---|
| `renderer/gpu_setup.rs:215` | the cloud overlay is slot 3 | it is slot 5; `SlotLayout::clouds` names it |
| `renderer/gpu_setup.rs:780` | preview usage is `RENDER_ATTACHMENT \| TEXTURE_BINDING` | `PREVIEW_USAGE` also carries `COPY_SRC`, which readback depends on |
| `params.rs:1-8` | the second translation point is `renderer::uniforms`; "the same thirty values" | it is `render_pass::write_uniforms`; there are 51 fields |
| `scene/sun.rs:22-24` | "+X = prime meridian, -Z = 90° East" | the code and the comment at `:80-82` put the prime meridian on +Z and 90° East on +X |
| `scene/datetime.rs:43-45` | "adjust the day-of-year down" | the code adds one to the cumulative month offset; behavior is correct |
| `scene/moon.rs:10-16` | the Moon's pixel floor is "one rule with one spelling" shared with `place_sun` | the two floor differently; only the constant is shared |
| `assets/texture_loader.rs:22-23` | the format is auto-detected | `ImageReader::open` uses the extension; `with_guessed_format` is never called |
| `config.rs:151-155` | unknown fields survive (forward compatibility) | true on read, false on write: `save_window_geometry` drops keys a newer build wrote |
| `config.rs:463-466, 475-478` | parse errors are logged to stderr | they go through `tracing::warn!` |
| `memory.rs:356-365` | "called from about seventeen places" | 13 in the crate today |
| `mouse_math.rs:3-4` | "extracted from the mouse callback closures in `main.rs`" | the callers are in `ui_callbacks.rs` |
| `tests/render_pipeline.rs:13` | "integration tests can't import from a bin crate" | `sunlit-core` is a library; the obstacle is `pub(crate) mod uniforms` |
| `tests/engine.rs:1725` | names `a_stale_decode_arriving_after_a_switch_is_never_applied` | no such test exists; the intended one is at `:1780` |
| `tests/soak.rs:256` | 44.6 s, margin 2.7x | 65.6 s measured, margin 1.8x |
| `desktop.rs:448-454` | doc for `file_uri` | attached to `plasma_command`; `file_uri` at `:514` has none |
| `tests/e2e.rs:1864-1892` | doc for `assert_the_desktop_holds_the_wallpaper` | attached to `placement_of` because a blank line is missing at `:1890` |

Documentation drift in `docs/` itself: `architecture.md:148` still describes the two-slot wallpaper naming that `sweep_legacy_files` exists to delete; `architecture.md:77` lists the file-backed slots without the Milky Way; `roadmap.md:83` says seven of nine golden cases fail on Metal when it is fourteen of eighteen; `testing.md:35` says the golden suite holds `GPU_SERIAL` when it uses its own `ENGINE` mutex; `xtask/src/commands/e2e.rs:44` says the desktop e2e run is "well under a minute"; and the "adding a shader parameter" checklist exists in `CLAUDE.md`, `rendering.md:60` and `params.rs:1-8` and undercounts (section 4.4).

**What to keep.** The reviewers were asked to be fair and named the comments that earn their place in every area. The pattern is consistent: `SAFETY` blocks, coordinate and unit conventions (`texture_loader.rs:54-60`, `sky.rs:113-119`, `layout.rs:9-26`), external quirks (`wgpu_init.rs:5-12` on Mesa's TLS destructors, `session_end.rs:22-23` on `HWND_MESSAGE` windows, `desktop.rs:50-55` on MATE and URIs, `watch.rs:120-124` on X11 empty event masks), invariants the code cannot state (`textures.rs:42-48`, `engine/mod.rs:368-389`, `ui_callbacks.rs:593-606`), and Slint layout gotchas throughout `main.slint`. Nothing in this section argues for removing those.

### 4.2 Length and structure

Once the test module is set aside, the real long files are `engine/mod.rs`, `wallpaper.rs`, `renderer/mod.rs` and `gpu_setup.rs` together, `sun_occlusion.rs`, `desktop.rs`, `config.rs`, `main.rs`, and the three big test files. The reviewers proposed seams for each with line ranges; the notes have them in full. In brief:

| File | Proposed shape | Notes |
|---|---|---|
| `engine/mod.rs` | `engine/{schedule,protocol,handle,cloud_worker,publish}.rs`, leaving `Engine`, `new`, `run`, `handle`, `tick` at ~500 lines. Split `Engine::new` (144 lines) at three marked seams and `handle` (102) at two, which removes both `too_many_lines` allows | engine B3-B5 |
| `wallpaper.rs` | `wallpaper/mod.rs` (portable, ~610 with tests) + `wallpaper/windows.rs` (~800), optionally `windows/shell.rs`. Move the Linux publish path (`write_placement`, `which`, `run`) out of `wallpaper_sink.rs` into `wallpaper/linux.rs` so both platforms delegate the same way | engine B1-B2 |
| `renderer/gpu_setup.rs` | a `Pipelines` struct built once by a table-driven builder from `(label, vs, fs, kind)`, called from both `create_renderer` and `rebuild_msaa_resources`; ~495 lines become ~130. Then `pipelines.rs` and `targets.rs` | renderer B1 |
| `renderer/mod.rs` | `sizing.rs` (AA options, sample count, quantization, 22 tests) and `slots.rs` (`TextureMode`, `SlotLayout`, labels, 8 tests); `Renderer` keeps ~360 lines. Group its 47 fields into `pipelines`, `targets`, `textures`, `last` | renderer B2-B3 |
| `scene/sun_occlusion.rs` | `sky_lens.rs` first (fixes the wrong-module imports in `moon.rs:20` and `mouse_math.rs:7`), then `limb_extinction.rs`; optionally `horizon_band.rs` and `disk_occlusion.rs`. Split `place_sun` (123 lines) at two marked seams | scene B1-B2 |
| `desktop.rs` | `desktop/{mod,xfce,kde}.rs`, lifting the arms of the 115-line `Backend::commands` to per-family functions | display B1-B2 |
| `config.rs` | `config/{quality,model,persist,window_geometry}.rs`; `window_geometry` first, since 123 lines of per-OS window placement including the file's only `unsafe` do not belong in a TOML module, and `display::monitors()` can replace the `MonitorFromRect` call | config B1-B2 |
| `assets/cloud_fetcher.rs` | `cloud_fetcher/{mod,cache,variant}.rs`; optional once the test module is trimmed | assets B1 |
| `sunlit-app/src/main.rs` | `cli.rs`, `logging.rs`, `startup.rs`, `headless.rs`, `app.rs` in the library; `run_app` (200 lines) into five helpers at seams already marked by blank lines | app B1 |
| `ui/main.slint` | `widgets.slint` (lines 3-195) and `tray.slint`; leave the 832-line Advanced section where it is, because Slint has no way to hand a component a group of two-way bindings and the split would add lines | app B5 |
| `tests/engine.rs` | one target, `tests/engine/main.rs` plus 13 modules (`harness`, `sinks`, `sun`, `lifecycle`, `display_plan`, `textures`, `memory`, `moon`, `panorama`, `clouds`, ...). Not several targets: each is a separate binary linking wgpu with `+crt-static` | tests-engine B1 |
| `tests/e2e.rs` | `tests/common/{process,pixels,cloud_stub,desktop_linux}.rs`, leaving the 15 cases at ~1500 lines | tests-app B1 |
| `tests/render_pipeline.rs` | table-drive the 61 remaining assertions in the offset test (the same test already has a 14-entry table at `:1353`); rebuild `UNIFORM_READBACK_SHADER` on top of `sphere.wgsl` the way the two newer probes do; put `cloud_pipeline_renders_with_alpha` on the shared context. 2506 to ~2150 | tests-gpu B1-B2 |

Files that should not be split, and why: `layout.rs` (456 production lines, one model), `camera.rs` (279), `texture_cache.rs` (348), `params.rs` (the fix is the macro in 4.4, not a split), `golden.rs` (one policy), `ipc::dispatch_command` (a flat match), `main.slint`'s Advanced section (above).

### 4.3 Consistency

**Module layout.** `display.rs` + `display/` is the only multi-file module in `sunlit-core` that does not use `mod.rs`; `assets`, `engine`, `geometry`, `renderer` and `scene` all do. A `git mv display.rs display/mod.rs` fixes it with no code change.

**Per-OS code, three ways.** `desktop.rs` uses no `cfg` at all (per-OS behavior is data in `BACKENDS`, testable everywhere). `display.rs`, `memory.rs` and `wallpaper.rs` use inline `#[cfg]` on individual items: `display.rs` writes `monitors()` three times, `memory.rs` writes `snapshot()` four times 900 lines apart with unrelated parsers between them, `wallpaper.rs` has fifteen scattered `cfg(windows)` items and `cfg`-annotated `use` lines in the shared header. `display/watch.rs` and `session_end.rs` use one private submodule per OS with a top-level `pub use`. Recommendation: the submodule pattern whenever a module has more than about two per-OS functions, with `watch.rs`'s `x11` / `win32` naming rather than `session_end.rs`'s `platform` / `unix`. `memory.rs` and `wallpaper.rs` benefit most.

**Layering.** `display::monitors()` on Windows calls `wallpaper::enumerate_monitors()` (`display.rs:217`), so the module that answers "what screens are there" depends on the one that puts a picture on them. `docs/platforms.md` describes the opposite shape. Fixing it drags `mod shell` along, so it is flagged rather than urged.

**Module docs.** 15 files open without `//!`: `config.rs`, `wgpu_init.rs`, `geometry/{mod,sphere}.rs`, `renderer/{frame,gpu_setup,render_pass,texture_routing,textures,uniforms}.rs`, `scene/{camera,datetime,mod}.rs`, and the app's `lib.rs` and `main.rs`. `uniforms.rs` opens with a `///` on the struct where the others use `//!`. `scene/mod.rs` and `geometry/mod.rs` are bare `pub mod` lists.

**`#[allow]` versus `#[expect]`.** 207 allows against 2 expects. Ten allows appear, by reading, to suppress nothing: `params.rs:37` (`struct_excessive_bools` on a struct with two bools; the threshold is three), `:205`, `:300`, `:363`; `renderer/mod.rs:724`; `render_pass.rs:380`; `config.rs:567`, `:606`; `engine/mod.rs:1456` (on `f64::from`, which is not a cast); `ui_callbacks.rs:607` (on a function with no cast). `#[expect]` would have reported all ten under `cargo clippy --all-targets`, which CLAUDE.md says is the developer's gate because CI does not run it. Several groups of cast allows could be one helper each: `fn px(u32) -> f32` in `layout.rs` (three sites), `vec3_of(astro_vector_t)` in `sky.rs` (five sites, plus a `checked(status, what)` helper for the six identical FFI epilogues), a shared `mib()` in `memory.rs` and `memory_report.rs` (three sites), a `slint_index` helper in `config.rs` (two sites), and `slider_index`/`slider_u32` in the app (four sites).

**Error style.** Consistent and appropriate within areas: `Result<_, String>` across the engine, sink and app boundaries; log-and-swallow in config and telemetry; one real error enum (`stars`). Two things to align: error message capitalization in `wallpaper.rs` (the older half is sentence case, the newer half and every other module lowercase; these reach the status line), and poison handling (`mailbox.rs` and `wallpaper.rs` production code `expect` on a poisoned lock while `cloud_source.rs` and `wallpaper.rs`'s own tests use `into_inner`; the mailbox is on the render path, where a poison panic is a hard stop).

**Public surface.** About 90 `pub` items have no caller outside their crate, verified by grepping the other crate, both test directories and `xtask`. The largest groups: 13 `Renderer` methods plus `RendererConfig`, `RenderOutcome`, `resolve_sample_count`, `quantize_to_granularity` (engine-only); eight items in `wallpaper.rs`; nine in `assets/` including the whole `CloudUpdater` API; 16 in `scene/`; seven in `config.rs`, `memory.rs`; 14 in `desktop.rs`, `display/`; eleven in the app's `displays.rs` and `mouse_math.rs`. Nine have no caller anywhere: `Renderer::preview_texture` (described as a client that binds the texture directly, which CLAUDE.md says does not exist), `wallpaper::wallpaper_on_monitor`, `DragSpeed::speed`, `StarCatalog::is_empty`, `SunVisibility::transit_fraction` (computed every frame), `cloud_fetcher::no_notify` (test-only), `tests/common::GPU`, `watch::CLASS_NAME`'s re-export, and `mouse_math::apply_globe_drag` (test-only). `pub` is what keeps `dead_code` from reporting them. `CountingSink` and `MockClock` are test doubles in the library's public API because integration tests cannot see `cfg(test)` items; `#[doc(hidden)]` would keep them out of the documented surface.

**Smaller items.** Test modules named `windows_tests` and `linux_tests` (`watch.rs`, `session_end.rs`) where every other file uses `mod tests`; `use super::*` before or after `use crate::` varies between test modules; `#[must_use]` on 8 of 17 `pub` functions in `sun_occlusion.rs` and nowhere else in `scene/`; constants placed next to their first user in `sun_occlusion.rs` (`DISK_FADE_GAIN` after the function that reads it) versus at the top in `camera.rs`; item order in `cloud_fetcher.rs`, `desktop.rs` and `displays.rs` interleaves consts, types, impls and free functions where every other file follows module doc, imports, consts, types, impls, functions, tests; `display.rs` has no `use` block and spells paths inline; two spellings of the same raw-pointer cast between `sun.rs` and `sky.rs`; two test-directory idioms in `engine.rs` and two temp roots (`temp_dir()` in two tests, `CARGO_TARGET_TMPDIR` in eighteen).

### 4.4 Duplication

| What | Where | Cost |
|---|---|---|
| The `Uniforms` struct, four times | `renderer/uniforms.rs`, `sphere.wgsl`, `tests/render_pipeline.rs:20-89` (Rust copy), `:735-805` (WGSL copy) | the offset test guards the copies, not production (section 6.2) |
| Ten pipeline descriptors, each built twice | `gpu_setup.rs:344-776` and again in `rebuild_msaa_resources:861-913`, string literals included | an eleventh pipeline added to one site is a wgpu validation error after an MSAA change |
| Preview pass and export pass | `render_pass.rs:382-434` vs `renderer/mod.rs:725-812`, 45 lines, same 20-argument `encode_and_submit` | a new draw can land in one and not the other |
| Arc-dedupe wallpaper write loop, comment included | `wallpaper_sink.rs:359-389` (Linux) and `wallpaper.rs:927-951` (Windows) | consequence of the Linux and Windows publish paths living in different modules |
| App data directory | `config.rs:456`, `memory.rs:401`, `cloud_fetcher.rs:167`, `wallpaper.rs:58`, three with the identical env-override wrapper | one `app_data_dir()` beside `env_override` |
| PNG encoder | `engine::save_png` (copies the frame), `wallpaper::encode_png`, `texture_cache::save_png` | |
| `unfinished` temp-name helper | `texture_cache.rs:315-321`, `wallpaper.rs:411-417` | byte-identical apart from the suffix |
| Atomic TOML write | `cloud_fetcher::save_cache_meta`, `config::save_config_to` | |
| `layout::Framing` from `SceneParams` | `main.rs:367-372` and `engine/mod.rs:1100-1105`, across crates | `sunlit-earth displays` prints a plan the engine does not build if a field is added |
| Gamma slider curve | `params.rs:430` and inlined twice in `main.slint:1384, 1410` with the constants baked in | change `GAMMA_MIN` and the labels lie with no test failing |
| Sky FOV clamp bounds | `layout.rs:260`, literals in `sun_occlusion.rs:238` (with a comment pointing at the constant), `sphere.wgsl:136` | the Rust copy is a one-line fix |
| `decode_cloud_jpeg` vs `texture_loader::orient` | `cloud_fetcher.rs:251-259` uses `fliph()` and allocates a second 100 MB copy at 8K; a test exists only to keep the two spellings in sync | |
| FFI epilogue | six functions in `sky.rs`: one `unsafe` call, one status assert, `as f32` per field, one allow each | |
| Shader shell vertex and nightglow fragment | `vs_rayleigh`, `vs_nightglow_orange`, `vs_nightglow_green`, `vs_cloud` are the same five statements; the two nightglow fragments differ in three values; `fs_sun_disk:993` inlines `limb_hue_km` | needs a golden run |
| `SIGNAL:memory` and `SIGNAL:displays` | produced by inline `format!` in `ipc.rs:149` and `displays.rs:360`, parsed by string literals in `e2e.rs:706` and `:2238` | a renamed field is a panic in a guest, not a compile error |
| MSAA fallback warning | `engine/mod.rs:607-614` and `:912-919` | |
| Linux backend lookup | `wallpaper_sink.rs:217-221` and `:282-286` | |
| `on_load_defaults` / `on_reset` | `ui_callbacks.rs:248-278` and `:284-314`, 25 identical lines in the crate's least tested function | |
| Test helpers | `Monitor` builder in `layout.rs`, `displays.rs`, `wallpaper_sink.rs`; the "drain until `WallpaperSet`" loop seven times in `engine.rs`; the spawn-and-watch block nine times and the quit-and-assert tail twelve times in `e2e.rs`; the clear-color predicate five times (one with a different rule) and luminance three times in the GPU tests; `memory_counters_supported`/`private_bytes`/`mib` in both `engine.rs` and `soak.rs`; `generate_uv_sphere` copied into `render_pipeline.rs` | |

**The parameter plumbing.** CLAUDE.md says adding a shader parameter means the `.slint` property and row, the `AppConfig` field, `SceneParams` and `ParamsDigest`, `Uniforms`, and the WGSL. Tracing `sun_reddening` finds 16 production sites plus two test sites: two in `main.slint`, two in `ui_callbacks.rs`, two in `config.rs`, five in `params.rs` (field, `from_config`, `write_to_config`, `digest`, `ParamsDigest`) plus the test table, `uniforms.rs`, two in `render_pass.rs`, two in `sphere.wgsl` (field with a hand-maintained offset comment, and the use), and the struct copy in `render_pipeline.rs`. The digest and `ParamsDigest` are hand-written per field, and the "table-driven" test at `params.rs:595-1039` is a 445-line hand-written `vec!` of 58 entries: a parameter never added to `digest()` and never added to the table compiles and passes, so the guarantee CLAUDE.md and `rendering.md:60` claim ("forgetting the dirty check is a test failure") does not hold. Recommendation: generate `ParamsDigest`, `digest()` and the mutation table from one `macro_rules!` list (the renderer notes estimate `params.rs` from 1112 to about 350 lines), or at least correct the count in the documentation. The app reviewer argues, convincingly, against extending a macro into `ui_callbacks.rs`: the two flat `set_*`/`get_*` lists are the crate's stated translation point, greppable, and the four fields that are not plain pass-throughs would each need an escape hatch.

### 4.5 Tests

**Run time.** The 1215 unit tests take 4.2 s. The concern about slow unit tests is really about two integration targets that run in every `cargo test`:

- `tests/engine.rs`, 161.6 s. The cause is exact: 91 engine starts at about 1.2 s each is 109 s. The cheapest engine-starting test (start, wait for `TexturesReady`, drop) takes 1.189 s; the only two tests under 1 s are the only two that create no device; every multi-engine test is a clean multiple (three engines, 3.7 s; nine two-engine tests all between 2.4 and 2.8 s). The remainder is two decodes of the real 8K assets with 341 MiB of mip chains (19 s in one test), 9.9 s of unconditional `thread::sleep` across 22 sites including a bare 2 s at `:2436`, a 244,800-point brute-force camera search called eight times at `opt-level = 0`, and fixture generation. The sun family starts 15 engines that differ only in `SceneParams`, which `UpdateParams` sets on a live engine; the display-plan family starts 16 that differ only in mode, anchor and monitor list, all mutable on a live engine, as one of its own tests proves. Sharing an engine per group, the pattern `golden.rs:96` already uses, takes 91 starts to about 28 and the target to roughly 80 s.
- `tests/soak.rs`, 65.6 s, not `#[ignore]`d. Halving `STEPS` to seven simulated days keeps 56 cloud updates behind the leak assertion (a one-frame-per-update leak would still be 450 MiB against a 16 MiB limit) and saves 33 s.

Together with the 2 s sleep and the camera search, those four changes take the two targets from 227 s to about 110 s without deleting an assertion. Elsewhere the wins are small but free: `render_pipeline.rs` issues 144 blocking GPU round trips for three shader functions that each read one uniform (8 probes cover every axis value), and compiles the 53 KB `sphere.wgsl` four times where once would do; `sky.rs`'s two moon sweeps evaluate 5840 sky states of 15 FFI calls each when four calls per state are read (about 175,000 ephemeris calls, a 7.5x cut available); `sun_occlusion.rs:973` runs 3.2 million inner iterations with the unit-disk mask recomputed 20 times; `texture_loader.rs:314` is a proptest with heavy rejection sampling asserting a length already checked five lines above; three Windows tests in `wallpaper.rs` each run a full COM monitor enumeration where one would do. `[profile.test] opt-level = 1` for the workspace crates is worth measuring (dependencies are already at `opt-level = 2`, the workspace is at 0, and every pixel-scanning loop pays for it).

**Tests that pin defaults or presets** (33, against the project's own rule): 16 in `config.rs` (asserting about 40 float literals between them, with `default_values_match_camera_params` showing the right shape three lines away), three in `camera.rs`, four in `slint_ui.rs` (`test_default_horizon_matches_the_config` shows the right shape), two in `cloud_fetcher.rs` (hardcoding `TEXTURE_RESOLUTIONS`), one each in `layout.rs` (`DisplayMode::default()`), `memory.rs` (the budget at exactly 3 GiB, beside three good property tests that bound it), `stars.rs` (exactly 15,597 stars), `params.rs` (the gamma endpoints), `datetime.rs` (the literal 21-year range), `renderer/mod.rs` and two in `engine.rs` (the 64-pixel quantization, pinned three times). The fix for `config.rs` is one assertion: `AppConfig` derives `PartialEq`, so `assert_eq!(from_str("longitude = 42.0"), AppConfig { longitude: 42.0, ..default() })` replaces thirteen missing-field tests with strictly more coverage.

**Redundancy.** About 210 of 1407 tests assert something a neighbor already asserts. By file, the reviewers' counts: `config.rs` 73 to ~45, `datetime.rs` 49 to ~14 (43 non-proptest tests covering about 10 behaviors), `camera.rs` 31 to ~14, `mouse_math.rs` 48 to 36, `slint_ui.rs` 45 to ~29, `layout.rs` 33 to 24, `renderer/mod.rs` 28 to ~12, `cloud_fetcher.rs` 42 to ~31, `engine.rs` 78 to 57, `shading.rs` 12 to 7, `desktop.rs` 26 to 21, `display.rs` 12 to 8, `texture_cache.rs` 19 to 13, `texture_loader.rs` 13 to 8, `render_pipeline.rs` 21 to 18, and a handful elsewhere. The common shapes are one-assertion tests over a pure function that belong in a table (`datetime`, `camera`, `layout::bounds_of`, `renderer::quantize`), pairs testing both clamp ends separately when a proptest already covers the range (`mouse_math`), and sequential steps of one lifecycle each rebuilding the fixture (`cloud_fetcher`). The runtime saving is nil for all of these; the reading cost is about 1500 lines.

**Tests that do not test what they say.** `re_enabling_the_preview_resends_the_current_frame_unchanged` never captures the earlier frame (`engine.rs:893`). `fresnel_specular_zero_intensity_unchanged` and `fresnel_diffuse_shift_zero_is_noop` render once and compare nothing; `fresnel_specular_brighter_at_grazing` passes when grazing is 20% darker; `gamma_identity_unchanged` and `saturation_identity_unchanged` never vary their parameter and are two copies of a determinism check (`render_pipeline.rs`). `test_preset_changes_camera_properties` registers a handler with hardcoded values and asserts them back, never reading `PRESETS` (`slint_ui.rs:319`). `nyc_day_side_not_dominated_by_city_lights` reduces to arithmetic on two fixture constants at its sample point (`shading.rs:451`). `deserialize_invalid_toml` tests the `toml` crate; `build_default_is_low_in_debug_and_high_in_release` reimplements the function it tests (`config.rs`). `system_wallpaper_accepts_frames_on_windows` asserts a trait default method (`wallpaper_sink.rs:686`). `mock_clock_is_shareable_across_threads` asserts what the `Send + Sync` bound proves at compile time.

**Test-only copies of production code.** `Uniforms` (Rust and WGSL), `generate_uv_sphere`, `DISPLAY_SETTLE` (`engine.rs:4596`, with a test whose advance pattern is only correct while the engine's private constant is between 1.5 and 2.5 s), `WINDOW_MIN_WIDTH = 520` (`slint_ui.rs:994`, restating `min-width: 520px`), and `test_the_sky_slider_stops_where_one_screen_stops`, which reads `main.slint` from disk and string-matches `minimum: 60.0;`. Each is one visibility change or one `out property` away from reading the real value.

**Hygiene.** Fixed-name scratch directories under the system temp dir with no cleanup on panic in `config.rs` (11 sites), `cloud_fetcher.rs` (11), `texture_cache.rs` (removes only at start, so every run leaves directories behind), `main.rs:920`, and `e2e.rs`'s `isolated_state_dir()`, which is never cleaned and accumulates one config file per spawned process forever; two concurrent cargo runs over the same checkout share all of them. `engine.rs` has the right RAII guard in two groups and `let _ = remove_dir_all` in twelve tests. `about.rs:87` calls `init_no_event_loop()` with no guard, so the second unit test in the lib target to do so will panic. `shading.rs:569` creates a second device in a process that may hold one, against the one-device rule, unmarked. `watch.rs:562, 569` use `SendMessageW` with no timeout in a unit test, the one place that can stall `cargo unit`. `validated_geometry_accepts_on_screen` needs a live monitor and fails in a headless Windows session. `render_to_file_*` write to `temp_dir()` where 18 other tests use `CARGO_TARGET_TMPDIR`. Four groups of engine tests keep the engine ticking by polling `memory_report()` for its side effect; if `ReportMemory` ever stopped implying a tick they would hang for 60 s each.

**The e2e harness.** `spawn_for_ipc` exists and four cases use it; eight repeat the same 30-line spawn-and-watch block inline. The same wait has different budgets with no stated reason (readiness 30 s or 60 s, shutdown 10 s or 15 s). The suite's worst-case internal budget is 53 minutes on Linux against xtask's 30-minute job timeout, so a slow guest is killed by the orchestrator before a per-wait panic can name the missing signal. `test_render_and_exit` pipes both streams and drains neither for up to 60 s, the one place the file's own pipe-congestion rule is not followed. `test_binary_exists` is `#[ignore]`d and `#[serial]` for reasons that do not apply to it.

**Golden.** `tests/golden/metal/` holds 4 of the 18 references the suite requires, so 14 cases plus the distinguishability guard fail on macOS; the suite is behaving as designed and `roadmap.md:83` is stale by a factor of two. Two cases are near-duplicates (`panorama_at_a_narrow_sky`, `clouds_across_the_terminator`); dropping them is the maintainer's call. The tolerance logic is in one place and is good.

### 4.6 Resilience and correctness

Confirmed by reading unless marked otherwise. Ordered by what matters for a first release.

**Reachable panics on startup or on an argument.**

- `wgpu_init.rs:73` `assert!(!adapters.is_empty())` and `:97` `.expect("Failed to create wgpu device")` run inside `Engine::new` before `ready` is sent; `engine/mod.rs:405` then panics on the main thread. `main.rs` has no panic hook. A user with a broken D3D12 runtime or Vulkan loader sees two backtraces and no window; `--software-rendering` does not help because `init` still runs. Return `Result` from `init` and `start`, print one sentence.
- `ipc.rs:42, 47` and `tray.rs:47` `expect` on an IPC socket name that is invalid or already taken, reachable from the documented `--ipc-socket` flag (a stale socket on Linux is the ordinary case).
- `tray.rs:57, 117` `expect` on tray creation and show; on a Linux desktop with no `StatusNotifierItem` host this may return `Err` (plausible, not verified). The resilient shape is to warn and fall back to windowed mode.
- `main.rs:632, 764, 798` `expect` on `PlatformError` from `MainWindow::new`, `show` and the event loop; three `let Ok(..) else` blocks turn crashes into messages.
- `render_pass.rs:644-647`: three `unwrap`s in `read_texture_rgba8`. `device.poll().unwrap()` panics on device loss, which on Windows is a real event (a TDR or a driver update); this is the wallpaper export and preview readback path, so a GPU reset panics the engine thread instead of failing one export. The `tx.send().unwrap()` inside the map callback can panic during unwinding and abort.

**Untested invariants that a routine edit would break silently.** The `Uniforms` layout (section 4.4). The `ParamsDigest` field set (section 4.4). `DisplayMode::index` (`layout.rs:66-71`) is the only non-test `expect` in its area and nothing links `ALL` to the variant list: a fourth variant compiles and panics on the UI path; an exhaustive `match` fixes it.

**Behavior defects.**

- Dragging the auto-refresh interval slider does a config-file read, serialize, write and rename per tick on the UI thread (`main.slint:416-424`, `main.rs:553-580`), plus one `SetAutoRefresh` command per pixel.
- While a wallpaper publish is deferred on pending textures, `tick` re-enters `publish_wallpaper` every 50 ms and `check_supported` runs a full `PATH` walk with a stat per directory each time on Linux (`engine/mod.rs:893-895, 1000-1017`). Check `textures_pending` first.
- `RenderWallpaperNow` is not coalesced: two in one drain batch (a double-click, or tray plus IPC together) are two full native-resolution renders, readbacks and encodes (`engine/mod.rs:744`).
- `save_config_to` cannot report failure; a full disk or read-only profile loses settings with a `warn!` in the log and nothing in the UI (`config.rs:508-547`). `save_window_geometry`'s load-modify-save drops keys a newer build wrote, so one run of an older build silently discards every newer setting (`config.rs:552-559`), against the comment at `:154`.
- `decode_cloud_jpeg` allocates a second full-size copy via `fliph()` before converting to RGBA, peaking near 334 MB at 8K where 234 MB would do (`cloud_fetcher.rs:251-259`).
- `SunVisibility::transit_fraction` is computed every frame (an `overlap_area` call) and read by nothing in either crate, any shader or any test except its own six assertions (`sun_occlusion.rs:677`).
- `texture_loader::decode` calls `no_limits()`, removing the `image` crate's 512 MB allocation cap on files the user can point the app at through `--textures-dir`; a malformed header becomes an allocation the process cannot refuse rather than an `Err` (`texture_loader.rs:40`). An explicit generous cap keeps the 8K assets decodable.
- Neither `Watcher` implements `Drop`; on Windows a leaked one leaves the `NOTIFY` slot filled so every later `start` returns `None` for the life of the process (`watch.rs`). `main.rs:810` does call `stop()`, so not live today.
- `wallpaper.rs:214, 326` `expect` on a poisoned `PUBLISHED` lock that `commit` holds across `remove_dir_all`; a panic under it would make every later publish panic on the engine thread while the app looks alive. The file's own tests use `into_inner`.
- `session_end.rs:127-153` leaves `HANDLER` set when installation fails after the `OnceLock` is populated, so a retry logs "already installed" forever (one caller today).
- `ipc.rs:58-60` reads an unbounded line from a local client.
- `Frame` derives `Debug` over a multi-megabyte pixel buffer (`wallpaper_sink.rs:18`); `engine::save_png` copies the whole framebuffer before encoding (`engine/mod.rs:1388`); `memory_report.rs:113` shifts by an unchecked mip count; `downsample_2x` underflows on a zero-width source (not reachable today).

**`unsafe` audit.** 49 of the 50 `#[allow(unsafe_code)]` sites have a `// SAFETY:` comment stating a real argument (`wallpaper.rs:511` is a redundant inner allow under the one at `:501`). The exception is `watch.rs:568-572`, whose comment describes the assertion rather than the safety argument. Two generic `fn zeroed<T>()` wrappers (`watch.rs:377`, `session_end.rs:192`) argue safety for the two types they are called with while their signature admits any `T`; make them `unsafe fn` or non-generic. `sky.rs:152-153` covers two raw pointers from one `&mut` and could say why that is sound.

**Verified sound, for the record.** The memory rules: no decoded pixel buffer rests anywhere but the fixed-length latest-value mailbox, both background producers name their consumer, and the reasoning is at the declaration (assets E1, with line references). The scheduling contract: the engine loop never sleeps on wall time, every deadline comes off the injected `Clock`, all five cross-thread queues are bounded or unbounded-with-reasoning at the declaration (engine E7). The preview mailbox protocol in `engine_client.rs` has no lost wakeup. The X11 stop path, the display-hint debounce, `crop`'s bounds, `parse_outputs`' negative offsets, `refract`'s Newton loop, `overlap_area`'s guards, `Renderer::render`'s two `expect`s, the CSV rotation bound, and every `assert_eq!` on an astronomy status code were each traced and hold.

## 5. Recommended plan

Ordered by value over effort. Estimates are the reviewers' and assume one person who knows the code; the notes have per-item estimates and risk.

**Phase A: before the release, about two days, low risk.**

1. Return `Result` from `wgpu_init::init` and `engine::start`; handle the IPC, tray and `PlatformError` `expect`s; `read_texture_rgba8` returns `Result`. Four startup crashes and one export crash become messages. (~8 h)
2. Make `renderer::uniforms::Uniforms` `pub`, import it in `render_pipeline.rs`, delete the Rust and WGSL copies and the `generate_uv_sphere` copy. The offset test then guards production. (~2 h)
3. Narrow the ~90 crate-internal `pub` items to `pub(crate)` or private; delete the nine dead ones; `#[doc(hidden)]` on the two test doubles. The compiler proves every step. (~3 h)
4. Fix the sixteen wrong comments and the six documentation drifts in section 4.1; correct the parameter-checklist count. (~2 h)
5. Stop saving the config on every slider tick; check `textures_pending` before `check_supported`; coalesce `RenderWallpaperNow`; route `decode_cloud_jpeg` through `orient`; delete `transit_fraction`; `into_inner` on the wallpaper lock; the missing `SAFETY` at `watch.rs:568`. (~4 h)
6. Share one engine per configuration group in `tests/engine.rs` (sun, display-plan, then memory, resolution, cloud, moon, preview); halve `soak.rs`; delete the 2 s sleep; coarse-to-fine `camera_showing`. 227 s to about 110 s, nothing deleted. (~10 h)

**Phase B: the comment pass, about two days, no risk.** File by file per the notes' section A: delete the restatements and the "why this test exists" blocks (the 55 step comments in `e2e.rs` first), move the measurements into tables in `docs/testing.md` and `docs/rendering.md` with one-line pointers, cut the second copies of `docs/` to the sentence the function needs, strip the plan and phase references. About a thousand lines. Decide once whether test doc comments are a house style the project keeps; today the rule and the code disagree.

**Phase C: the tests, about three days, low risk.**

1. Replace the 33 default-pinning tests with property or agreement tests (`config.rs` first: 13 missing-field tests become one struct equality). (~3 h)
2. Table-drive the one-assertion families (`datetime`, `camera`, `layout::bounds_of`, `renderer::quantize`, `mouse_math` clamps, `find_sample_count`); merge the sequential-lifecycle tests in `cloud_fetcher`; merge the redundant `slint_ui`, `shading` and `render_pipeline` tests. About 210 tests and 1500 lines. (~10 h)
3. Fix the tests that do not test their name (section 4.5). (~3 h)
4. One scratch-directory helper with a pid suffix and a `Drop` guard, used everywhere; the `about.rs` init guard; a `LazyLock` for `shading.rs`'s software context; `SendMessageTimeoutW` in the watch test; skip the on-screen geometry test without a monitor. (~3 h)
5. `render_pipeline.rs`: 8 probes instead of 144, reuse the compiled shader module, table-drive the offset test, rebuild the readback shader on `sphere.wgsl`. `sky.rs`: a moon-only helper and a twice-daily grid. (~5 h)
6. `e2e.rs`: widen `spawn_for_ipc`, extract `assert_no_error_lines` and `quit_and_expect_clean_exit`, four named timeout constants, attach the watchers in `test_render_and_exit`, one parser per `SIGNAL:` line shared with the producer, reconcile the Linux job timeout. (~6 h)

**Phase D: structure, about four days, low to medium risk, after A to C so less code moves.**

1. `git mv display.rs display/mod.rs`. Adopt the per-OS submodule pattern in `memory.rs`, then `wallpaper.rs` as part of its split. (~4 h)
2. `gpu_setup.rs`: the `Pipelines` table. Run the golden suite before and after. (~4 h)
3. Extract `draw_scene` for the preview and export passes; `Overlays` to `Option<Overlay>`; `encode_and_submit` from 20 arguments to 9. (~4 h)
4. The splits in section 4.2, in this order: `wallpaper/{mod,windows,linux}.rs` with `Publication::write_job` shared; `engine/{schedule,protocol,handle,cloud_worker,publish}.rs` and `Engine::new`/`handle`; `renderer/{sizing,slots}.rs`; `scene/sky_lens.rs` and `limb_extinction.rs`; `config/window_geometry.rs` over `display::monitors()`; `desktop/{xfce,kde}.rs`; `main.rs` into the library; `tests/engine/`; `tests/common/`; `ui/widgets.slint`. (~20 h)
5. Generate `ParamsDigest`, `digest()` and the mutation table from one macro list. (~4 h)
6. Convert the cast allows to `#[expect(..., reason = "...")]` and add the six small cast helpers; adopt `#[expect]` as the convention. (~3 h)
7. The shared helpers in section 4.4: `app_data_dir`, one PNG writer, one `unfinished`, one atomic TOML write, `From<&SceneParams> for Framing`, the gamma display value pushed from Rust, `SKY_FOV_*` in `sun_occlusion.rs`, the `sky.rs` FFI helpers, the shader `shell_vertex` (golden run). (~6 h)

Total about eleven working days if everything is done. Phases A and B are the release-relevant half and take about four.

## 6. What this review did not do

- The reviewers did not run cargo. Every runtime estimate in the notes is derived from the code; the measured figures in section 3.3 are the one timed run, on one Windows host, debug profile, serial. The engine-start floor was confirmed from the measured data, not assumed.
- Clippy cannot report an `#[allow]` that suppresses nothing, so the ten listed in 4.3 are by reading. Converting them to `#[expect]` is the check.
- Not verified: whether Slint's tray returns `Err` on a Linux desktop without a tray host; whether `interprocess::incoming()` can spin on a persistent error; whether `jxl-oxide` validates declared dimensions before allocating; whether x11rb's `RustConnection` tolerates the two-thread use `watch.rs` documents (the module's own claim was read, not x11rb's source); whether `Monitor::overlaps` would classify a window identically to `MonitorFromRect` at monitor edges; whether the `SendMessageW` stall in the watch test has ever occurred; the size of the gain from `[profile.test] opt-level = 1`.
- `crates/xtask`, `vm/`, `assets/` and the workflows were out of scope except where a source finding pointed at them.
- Nothing about the rendering itself, the astronomy, or the visual output was assessed. The golden suite passed on this host.

## Appendix: the notes

Each file is one reviewer's full report for its area, with the classification of every long comment block, the split seams with line ranges, the visibility grep results, and the per-test verdicts. Line numbers refer to commit `3046327`.

| Area | Files covered | Notes |
|---|---|---|
| Engine and wallpaper | `engine/`, `wallpaper.rs`, `wgpu_init.rs`, `lib.rs` | [engine.md](2026-09-04-code-quality/engine.md) |
| Desktop and display | `desktop.rs`, `display.rs`, `display/` | [display.md](2026-09-04-code-quality/display.md) |
| Renderer, params, shaders | `renderer/`, `params.rs`, `geometry/`, `shaders/` | [renderer.md](2026-09-04-code-quality/renderer.md) |
| Scene | `scene/` | [scene.md](2026-09-04-code-quality/scene.md) |
| Config and memory | `config.rs`, `memory.rs`, `memory_report.rs` | [config-memory.md](2026-09-04-code-quality/config-memory.md) |
| Assets | `assets/` | [assets.md](2026-09-04-code-quality/assets.md) |
| The app crate | `sunlit-app/src`, `ui/main.slint` | [app.md](2026-09-04-code-quality/app.md) |
| Engine and soak tests | `tests/engine.rs`, `soak.rs`, `support/`, `common/` | [tests-engine.md](2026-09-04-code-quality/tests-engine.md) |
| GPU tests | `tests/render_pipeline.rs`, `golden.rs`, `shading.rs` | [tests-gpu.md](2026-09-04-code-quality/tests-gpu.md) |
| App tests | `tests/slint_ui.rs`, `e2e.rs` | [tests-app.md](2026-09-04-code-quality/tests-app.md) |
| Timing | the per-test figures from the timed run | [test-timing.md](2026-09-04-code-quality/test-timing.md) |
