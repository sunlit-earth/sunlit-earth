# Research: Codebase Restructuring (2026-03-15)

## Problem Statement

Sunlit Earth is a ~2,540-line Rust desktop app (11 source files, excluding tests and shaders) that renders a 3D Earth with wgpu and displays it via Slint. The codebase has two structural problems:

1. **`renderer/mod.rs` is 748 lines** and mixes callback dispatch, state management, dirty-checking, MSAA/resize rebuild, texture loading coordination, uniform construction, render pass encoding, and UI updates -- all in a single `rendering_callback()` function of ~250 lines.
2. **Seven top-level files lack logical grouping** -- geometry (`sphere.rs`, `grid_texture.rs`), astronomy (`sun.rs`), GPU infrastructure (`wgpu_init.rs`), texture I/O (`texture_loader.rs`), camera math (`camera.rs`), and `main.rs` are all flat siblings in `src/`.

The goal is to improve module organization and extract independent functionality into smaller, more focused files without changing behavior.

## Requirements

- No behavioral changes -- this is a pure structural refactor
- Preserve all existing tests and maintain current coverage levels
- Keep `pub` API surface unchanged (external callers of `renderer::build_aa_options`, `renderer::setup_rendering_notifier` unaffected)
- Maintain the `thread_local! { RefCell<Option<GpuResources>> }` pattern required by Slint's `'static` callback constraint
- Respect `unsafe_code = "deny"` policy
- All files should stay under ~400 lines (the threshold established by the prior renderer split)

## Findings

### Current File Sizes and Responsibilities

| File | Lines | Primary Concern |
| ---- | ----: | --------------- |
| `renderer/mod.rs` | 748 | Callback dispatch, state, dirty-checking, frame rendering, UI updates |
| `renderer/gpu_setup.rs` | 383 | GPU resource creation, pipeline, shader loading |
| `renderer/textures.rs` | 354 | Texture slots, background decoding, bind groups, mipmapping |
| `texture_loader.rs` | 181 | File I/O, JXL hook, texture directory resolution |
| `grid_texture.rs` | 158 | Procedural equirectangular grid texture |
| `sphere.rs` | 157 | Procedural UV sphere mesh generation |
| `sun.rs` | 156 | Astronomy Engine FFI wrapper |
| `wgpu_init.rs` | 141 | Adapter selection, device creation |
| `main.rs` | 133 | CLI, window setup, callbacks, timer |
| `camera.rs` | 113 | Orbital camera: spherical coords to MVP matrix |
| `renderer/uniforms.rs` | 16 | Pure data struct (96 bytes, std140 aligned) |

### The `renderer/mod.rs` Problem

The `rendering_callback()` function (lines 219-471) is the core concern. It handles all three `RenderingState` arms in a single match, with the `BeforeRendering` arm alone spanning ~210 lines. Within that arm, at least seven distinct phases are interleaved:

1. **Texture decode polling** -- calls `process_decoded_textures()`
2. **MSAA/viewport rebuild** -- checks `lookup_sample_count()`, `quantized_viewport_size()`, triggers `rebuild_msaa_resources()` or `rebuild_render_textures()`
3. **Frame state construction** -- reads UI properties, calls `build_frame_state()`, computes `sun_direction_now()`
4. **Texture slot resolution** -- determines blend mode, spawns background loads, resolves bind groups
5. **Loading indicator UI** -- constructs loading text, calls `win.set_loading_text()`
6. **Dirty-check gate** -- compares against `last_state`, early-returns if clean
7. **Render pass execution** -- builds camera/MVP, writes uniforms, encodes render pass, submits, converts texture to Slint Image

These phases have a linear data-flow (each feeds the next) but represent fundamentally different concerns: framework integration (phases 1-2), decision logic (phases 3-5), UI feedback (phase 5), and GPU submission (phase 7).

The `GpuResources` struct (lines 70-115) also has 22 fields, mixing GPU pipeline state, texture management state, channel endpoints, and window references.

### Module Coupling and Independence

Both research documents agree on a clear separation between loosely coupled and tightly coupled modules:

**Independent pure-function modules** (no inter-module or framework dependencies):

- `camera.rs` -- depends only on `glam`
- `sphere.rs` -- zero crate imports
- `grid_texture.rs` -- zero crate imports
- `sun.rs` -- only FFI, standalone

These four modules are already well-structured. They share a common trait: no GPU, no UI, no state -- just inputs to outputs. Both researchers identified this as confirming that the core architectural problem is the failure to separate pure logic from framework-dependent code elsewhere.

**Tightly coupled renderer internals:**

- `renderer/mod.rs` <-> `renderer/gpu_setup.rs` <-> `renderer/textures.rs` share `GpuResources` and have deep interdependencies via `pub(super)` visibility
- `renderer/gpu_setup.rs` -> `sphere.rs`, `grid_texture.rs` is one-way (used only during setup)

**Mixed-concern modules:**

- `renderer/mod.rs` -- mixes callback dispatch with rendering logic with UI updates
- `main.rs` -- mixes CLI parsing, window setup, timer management, texture path resolution, and slider callbacks

### Insights from the Prior Renderer Split

The `renderer/` submodule structure was created during the test coverage initiative. The original `renderer.rs` was a 1,104-line monolith that was split along four seams into `mod.rs`, `gpu_setup.rs`, `textures.rs`, and `uniforms.rs`. Key lessons from that split:

- **The split unlocked testability**: pure functions like `build_aa_options()`, `quantize_to_granularity()`, and `build_frame_state()` were previously trapped in GPU-dependent contexts. Extracting them made them testable, and `mod.rs` now has ~30 unit tests (the most in any single file).
- **`pub(super)` works well**: cross-module visibility within `renderer/` uses `pub(super)`, keeping the internal API invisible to the rest of the crate while allowing the submodules to collaborate.
- **The split was incomplete**: `mod.rs` absorbed everything that didn't clearly belong elsewhere, resulting in 748 lines -- nearly double the ~400-line ceiling established for the other submodules.

### MVVM Alignment

The test coverage research noted that the codebase partially aligns with MVVM:

- **Model layer**: `camera.rs`, `sun.rs`, `sphere.rs`, `grid_texture.rs` are already pure
- **ViewModel layer**: dirty-checking, MSAA selection, texture directory resolution -- currently mixed into the View layer
- **View layer**: the rendering callback, Slint window setup

The restructuring extracted some ViewModel logic (e.g., `build_frame_state()`), but loading-indicator text construction, texture slot resolution, and blend-mode logic remain embedded in the callback.

### Naming Confusion

The codebase analysis identified a naming problem: `texture_loader.rs` (top-level, file I/O) and `renderer/textures.rs` (GPU texture management) perform related but distinct roles. Their names don't clearly communicate the boundary. A reader unfamiliar with the codebase would struggle to predict which file handles which concern.

### Test Infrastructure

Tests relevant to the restructuring:

- **Unit tests in `renderer/mod.rs`** (~30 tests): cover `build_aa_options`, `quantize_to_granularity`, `build_frame_state`. These are the most likely to be affected by moving code.
- **Integration tests** (`tests/shading.rs`, `tests/render_pipeline.rs`): use `tests/common/mod.rs` for shared `GpuContext`. These test the GPU pipeline end-to-end and should be unaffected by internal reorganization.
- **Pure module tests**: `camera.rs` (5), `sphere.rs` (6), `grid_texture.rs` (7), `texture_loader.rs` (7), `wgpu_init.rs` (3) -- all self-contained, unaffected.

## Technical Constraints

- **Slint's `'static` callback requirement**: the `set_rendering_notifier()` callback must own all its data or use `thread_local!`. This is why `GpuResources` lives in a thread-local and cannot be restructured into a more conventional ownership pattern.
- **Slint's one-initialization-per-process constraint**: UI tests cannot coexist with GPU tests in the same process. This limits test organization options if Slint testing backend support is added later.
- **WGSL layout coupling**: `Uniforms` must match WGSL `vec3<f32>` alignment (16-byte with explicit `_pad: f32`). Moving `Uniforms` is safe as long as `#[repr(C)]` and the size assertion are preserved.
- **Shader concatenation**: `gpu_setup.rs` concatenates `blend.wgsl` + `sphere.wgsl` at load time. This coupling point should remain in `gpu_setup.rs`.
- **No `lib.rs`**: everything is binary-only (`main.rs` is the crate root). This limits reuse and makes it impossible to write integration tests that import internal types without `pub(crate)` visibility. Adding `lib.rs` would be a prerequisite for deeper restructuring.
- **Windows-specific concerns**: per-test GPU device creation crashes on Windows; the `std::process::exit(0)` workaround in `main.rs` addresses thread-local destruction ordering.

## Open Questions

1. **Should `lib.rs` be introduced?** Both researchers noted the lack of a library crate. Adding one would enable integration tests to import internal types directly, but it changes the crate structure (binary becomes a thin wrapper around a library). This needs a decision before proceeding.

2. **How far should `rendering_callback` be decomposed?** The seven phases identified above could each become a separate function, but some share mutable access to `GpuResources`. Rust's borrow checker makes splitting a function that takes `&mut GpuResources` into multiple sequential calls straightforward, but splitting into concurrent or interleaved access requires careful design.

3. **Should `GpuResources` be split?** Its 22 fields span GPU pipeline state, texture management, channel endpoints, and window references. Splitting it into sub-structs (e.g., `PipelineState`, `TextureState`, `Channels`) would reduce the surface area each function touches, but adds indirection.

4. **Should top-level files be reorganized into directories?** Grouping `camera.rs` + `sun.rs` into `scene/` or `sphere.rs` + `grid_texture.rs` into `geometry/` would add logical structure, but for only 2-3 files per group the overhead of `mod.rs` files may not be justified.

5. **Should `main.rs` be split?** At 133 lines it is not large, but it mixes several concerns. Extracting a `window_setup()` function was identified as a prerequisite for future Slint testing. The benefit is marginal until Slint testing is actually pursued.

6. **What naming convention should replace `texture_loader.rs` / `renderer/textures.rs`?** Options include `texture_io.rs` / `texture_gpu.rs`, or moving `texture_loader.rs` into the `renderer/` module as `renderer/texture_loader.rs`.

## Recommendations

### Priority 1: Split `renderer/mod.rs` (high impact, low risk)

Extract three new submodules from `renderer/mod.rs`:

- **`renderer/frame.rs`** (~100 lines): `FrameState` struct, `build_frame_state()`, and the dirty-checking comparison logic. These are already pure and have 30 tests -- moving them is mechanical.
- **`renderer/render_pass.rs`** (~120 lines): the render pass encoding logic (camera/MVP computation, uniform construction, encoder creation, pass recording, queue submission, texture-to-image conversion). This is the "phase 7" block from `rendering_callback`.
- **`renderer/texture_routing.rs`** (~80 lines): blend mode detection, `maybe_spawn_texture_load` calls, bind group resolution, loading indicator text. This is "phases 4-5" from the callback.

After extraction, `renderer/mod.rs` would drop to ~350-400 lines containing only the public API (`build_aa_options`, `setup_rendering_notifier`), the rendering callback skeleton (delegating to submodules), `GpuResources` struct, `quantize_to_granularity`, and the thread-local.

### Priority 2: Introduce `lib.rs` (medium impact, low risk)

Move all `mod` declarations from `main.rs` into a new `lib.rs`. `main.rs` becomes a thin binary entry point that calls `lib::run()` or similar. Benefits:

- Integration tests can `use sunlit_earth::*` to access internal types
- Enables future `pub` API for embedding
- Makes the crate structure more conventional

### Priority 3: Logical grouping of top-level files (low impact, optional)

This is the lowest priority because the current flat structure with 7 files is not yet unwieldy. If pursued:

- Group `camera.rs` + `sun.rs` under `scene/` (both compute scene-level parameters)
- Group `sphere.rs` + `grid_texture.rs` under `geometry/` (both generate mesh/texture data)
- Keep `wgpu_init.rs` and `texture_loader.rs` top-level (they're singletons with unique roles)

This adds 2 new `mod.rs` files and changes import paths. The benefit is primarily navigational -- it signals intent to new contributors. Defer until the module count grows larger.

### Not Recommended (yet)

- **Splitting `GpuResources`**: adds complexity without clear testability gains. Revisit if the struct grows beyond its current 22 fields.
- **Trait-based dependency injection for sun direction**: useful for deterministic testing of the callback, but overkill until the callback is actually tested end-to-end.
- **Splitting `main.rs`**: at 133 lines it is not a maintenance burden. Extract `window_setup()` only when Slint UI testing is implemented.

## Sources

| Document                                               | Focus Area                                                         |
| ------------------------------------------------------ | ------------------------------------------------------------------ |
| `docs/plans/2026-03-15-restructuring-codebase.md`      | Codebase structure analysis: file sizes, dependency graph, coupling|
| `docs/plans/2026-03-15-restructuring-test-insights.md` | Structural insights distilled from test coverage research          |
