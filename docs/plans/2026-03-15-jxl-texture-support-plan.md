# Plan: JXL Texture Support with Lazy Loading (2026-03-15)

## Summary

Replace the JPEG earth texture with JPEG-XL textures (day and night), loaded lazily on demand when the user selects them from the UI combobox. The `jxl-oxide` crate provides pure-Rust JXL decoding via the `image` crate's external hook system. The existing `earth_texture.rs` module is repurposed into a generic texture loader, the JPEG dependency is removed, bind group storage is generalized from named fields to a `Vec<Option<wgpu::BindGroup>>` to support lazy population, and the UI texture list becomes Grid / Day / Night. The existing `earth_4k.jpg` loading code is removed entirely.

## Stakes Classification

**Level**: Medium
**Rationale**: This changes the texture loading pipeline, the GPU resource management in `renderer.rs`, and the startup flow in `main.rs`. However, the changes are well-scoped to the texture subsystem, do not touch the shader or camera modules, and rollback is straightforward (revert the commits). The bind group storage refactor from named fields to a Vec is the highest-risk piece, but the renderer already handles `Option<wgpu::BindGroup>` for the earth texture, so the pattern is established.

## Context

**Research**: [`docs/plans/2026-03-15-jxl-texture-support-research.md`](2026-03-15-jxl-texture-support-research.md)
**Affected Areas**: `Cargo.toml`, `src/main.rs`, `src/renderer.rs`, `src/earth_texture.rs` (renamed/repurposed), `ui/main.slint` (default values only)

## Success Criteria

- [ ] `jxl-oxide` is added as a dependency with `image` and `rayon` features, `default-features = false`
- [ ] The `image` crate dependency no longer includes the `jpeg` feature
- [ ] `jxl_oxide::integration::register_decoding_hook()` is called once at startup
- [ ] All references to `earth_4k.jpg` are removed from the codebase
- [ ] The texture combobox lists "Grid", "Day", "Night" (always all three, regardless of which JXL files exist on disk)
- [ ] Selecting "Day" or "Night" triggers lazy loading: the JXL file is decoded and the GPU bind group is created on first selection
- [ ] While a texture is loading, the previously rendered texture remains visible (no flash to grid)
- [ ] If a JXL file is missing or fails to decode, the previously rendered texture remains as fallback and an error is printed to stderr
- [ ] The grid (procedural) texture continues to work unchanged at index 0
- [ ] `cargo build` succeeds with no warnings
- [ ] `cargo clippy` passes
- [ ] `cargo test` passes (existing tests unbroken)
- [ ] The application runs and renders correctly with the JXL textures

## Implementation Steps

### Phase 1: Dependency Changes

#### Step 1.1: Update `Cargo.toml` dependencies

- **Files**: `Cargo.toml`
- **Action**: Make three changes:
  1. Bump `image` version floor from `"0.25"` to `"0.25.8"` (required for the external decoder hook API). Remove the `jpeg` feature. Since `default-features = false` is already set and no feature is needed, keep `default-features = false` with an empty features list.
  2. Add `jxl-oxide` dependency: `jxl-oxide = { version = "0.12", default-features = false, features = ["image", "rayon"] }`
  3. Remove the comment about `jpeg` feature if present.
- **Verify**: `cargo check` succeeds. `cargo tree -i image` shows image 0.25.8+. `cargo tree -i jxl-oxide` shows jxl-oxide 0.12.x with `image` and `rayon` features.
- **Complexity**: Small

#### Step 1.2: Verify the build compiles cleanly

- **Files**: N/A
- **Action**: Run `cargo build` and `cargo clippy` to ensure the new dependencies integrate without issues. The jxl-oxide crate requires Rust 1.85+ (edition 2024); confirm the project's Rust toolchain meets this requirement.
- **Verify**: `cargo build` and `cargo clippy` succeed with zero errors. No new warnings.
- **Complexity**: Small

### Phase 2: Repurpose the Texture Loader

#### Step 2.1: Rename and generalize `earth_texture.rs` to `texture_loader.rs`

- **Files**: `src/earth_texture.rs` (rename to `src/texture_loader.rs`), `src/main.rs` (module declaration)
- **Action**:
  1. Rename the file from `earth_texture.rs` to `texture_loader.rs`.
  2. Update the module declaration in `main.rs` from `mod earth_texture;` to `mod texture_loader;`.
  3. Update the `use` statement in `renderer.rs` from `use crate::earth_texture::DecodedImage;` to `use crate::texture_loader::DecodedImage;`.
  4. Update the module-level doc comment from "Load the Earth texture from a JPG file on disk" to "Load equirectangular texture images from disk" (or similar).
  5. Update the `load()` function doc comment to remove "JPG" — it now handles any format the `image` crate supports (including JXL via the hook).
- **Verify**: `cargo build` succeeds. All existing `use` paths resolve.
- **Complexity**: Small

#### Step 2.2: Add JXL hook registration function

- **Files**: `src/texture_loader.rs`
- **Action**: Add a public function `register_jxl_hook()` that calls `jxl_oxide::integration::register_decoding_hook()`. This is idempotent and must be called before any `image::open()` call that might encounter a JXL file. The function wraps the jxl-oxide call so the rest of the codebase does not need to depend on `jxl-oxide` directly.
- **Verify**: `cargo build` succeeds. The function exists and is callable.
- **Complexity**: Small

### Phase 3: Lazy Loading Infrastructure in the Renderer

This is the core architectural change. The renderer must support bind groups that are not yet loaded (`None`) and must be able to create them on demand during `BeforeRendering`.

#### Step 3.1: Define a texture registry data structure

- **Files**: `src/renderer.rs`
- **Action**: Replace the named bind group fields in `GpuResources` with a lazy-loading structure. Define a new struct and update `GpuResources`:

  ```rust
  /// Descriptor for a texture that can be loaded on demand.
  struct TextureSlot {
      /// The GPU bind group, populated on first use.
      bind_group: Option<wgpu::BindGroup>,
      /// Filesystem path to the texture file (None for procedural textures).
      source_path: Option<std::path::PathBuf>,
  }
  ```

  Update `GpuResources`:
  - Remove `grid_bind_group: wgpu::BindGroup` and `earth_bind_group: Option<wgpu::BindGroup>`.
  - Add `texture_slots: Vec<TextureSlot>` — index 0 is Grid (always loaded), index 1 is Day, index 2 is Night.
  - Add `last_rendered_index: usize` (defaults to 0) — tracks the most recently successfully rendered texture slot index. Used as fallback when the requested slot isn't loaded yet or failed to load, so the user sees the current texture rather than a flash to grid.
  - Store the `bind_group_layout`, `sampler`, and `uniform_buffer` as fields on `GpuResources` (the layout and sampler are needed to create bind groups lazily; the uniform buffer is already stored). The bind group layout is currently a local variable in `create_gpu_resources()` — it must be promoted to a field.

- **Verify**: `cargo build` succeeds (with temporary compilation errors in rendering callback fixed in the next step).
- **Complexity**: Medium

#### Step 3.2: Update `create_gpu_resources()` to initialize the texture slots

- **Files**: `src/renderer.rs`
- **Action**: Modify `create_gpu_resources()`:
  1. Change the signature: replace `earth_pixels: Option<&DecodedImage>` with `texture_paths: Vec<Option<std::path::PathBuf>>` (a Vec of optional paths, one per non-grid texture slot).
  2. Create the grid bind group as before (always loaded at slot 0).
  3. Initialize the texture slots Vec:
     - Slot 0: `TextureSlot { bind_group: Some(grid_bind_group), source_path: None }`
     - Slot 1: `TextureSlot { bind_group: None, source_path: texture_paths.get(0).cloned().flatten() }`
     - Slot 2: `TextureSlot { bind_group: None, source_path: texture_paths.get(1).cloned().flatten() }`
  4. Store `bind_group_layout` and `sampler` on `GpuResources`.
  5. Remove all earth-texture-specific loading code from this function.

- **Verify**: `cargo build` succeeds. The grid texture still renders at index 0.
- **Complexity**: Medium

#### Step 3.3: Implement lazy bind group creation in the rendering callback

- **Files**: `src/renderer.rs`
- **Action**: In the `BeforeRendering` branch of `rendering_callback()`, replace the existing bind group selection logic with lazy loading:
  1. Read `texture_index` from the UI.
  2. Clamp the index to valid range: `0..texture_slots.len()`.
  3. If `texture_slots[index].bind_group` is `Some`, use it and update `last_rendered_index` to this index.
  4. If `texture_slots[index].bind_group` is `None` and `source_path` is `Some`:
     - Call `texture_loader::register_jxl_hook()` (idempotent, safe to call multiple times).
     - Call `texture_loader::load(&path)`.
     - On success: call `create_mipmapped_texture()` + `create_bind_group()` to create the bind group, store it in the slot, use it for this frame, and update `last_rendered_index`.
     - On failure: print error to stderr, set `source_path` to `None` (so we don't retry), and fall back to the previously rendered texture (see step 5).
  5. If the requested slot has no bind group and no source path (failed or unavailable): render using `last_rendered_index` — the most recently successfully rendered texture. This keeps the current texture visible while a new one loads or if the new one fails, rather than flashing back to the grid. `last_rendered_index` defaults to 0 (grid) so on first launch the grid is the initial fallback.

  The decode happens synchronously on the UI thread. For 8K textures, this may cause a brief UI freeze (a few seconds). This is acceptable for the initial implementation; background decoding can be added later if needed.

  Extract the bind group selection into a helper function like `ensure_bind_group_loaded()` to keep the rendering callback clean.

- **Verify**: `cargo build` succeeds. Selecting a texture from the combobox triggers loading.
- **Complexity**: Medium

#### Step 3.4: Add timing instrumentation for texture decode

- **Files**: `src/renderer.rs` (or `src/texture_loader.rs`)
- **Action**: Wrap the JXL decode + GPU upload in `std::time::Instant` timing. Print the elapsed time to stderr, e.g.: `eprintln!("Loaded Day texture (8192x4096) in 1.23s")`. This resolves the research document's open question about decode latency for 8K textures.
- **Verify**: When selecting a JXL texture, timing output appears in stderr.
- **Complexity**: Small

### Phase 4: Update `main.rs` Startup Flow

#### Step 4.1: Remove JPEG loading, add JXL hook registration and texture path resolution

- **Files**: `src/main.rs`
- **Action**:
  1. Call `texture_loader::register_jxl_hook()` early in `main()`, before any image loading.
  2. Remove the `earth_pixels` loading block (the `textures_dir.and_then(|dir| { ... earth_4k.jpg ... })` block).
  3. Resolve texture paths for the two JXL files:
     - `day_path`: `textures_dir.as_ref().map(|d| d.join("world.topo.200405.jxl")).filter(|p| p.exists())`
     - `night_path`: `textures_dir.as_ref().map(|d| d.join("BlackMarble_2016.jxl")).filter(|p| p.exists())`
  4. Build the `texture_paths: Vec<Option<PathBuf>>` to pass to the renderer: `vec![day_path, night_path]`.
  5. Update the texture options to always be `["Grid", "Day", "Night"]` — all three are always listed. If a JXL file doesn't exist on disk, the renderer will show the grid fallback when that option is selected (and print an error).
  6. Remove the conditional `has_earth` logic. The combobox always shows all three options.
  7. Set the default `texture_index` to 1 (Day) unconditionally in the deferred `invoke_from_event_loop` block.
  8. Update the call to `renderer::setup_rendering_notifier()` to pass `texture_paths` instead of `earth_pixels`.

- **Verify**: `cargo build` succeeds. The application starts without loading any textures eagerly. The combobox shows "Grid", "Day", "Night".
- **Complexity**: Medium

#### Step 4.2: Update `setup_rendering_notifier()` signature and plumbing

- **Files**: `src/renderer.rs`
- **Action**: Change the `setup_rendering_notifier()` function:
  1. Replace the `earth_pixels: Option<DecodedImage>` parameter with `texture_paths: Vec<Option<std::path::PathBuf>>`.
  2. Pass `texture_paths` through to `create_gpu_resources()` in the `RenderingSetup` handler (wrapped in a `RefCell` for the closure, or cloned — the paths are small).
  3. Remove the `earth_pixels` `RefCell` and its `.take()` pattern.

- **Verify**: `cargo build` succeeds. The rendering notifier correctly receives texture paths.
- **Complexity**: Small

### Phase 5: Update Slint UI Defaults

#### Step 5.1: Update default texture options in `main.slint`

- **Files**: `ui/main.slint`
- **Action**: Change the default `texture-options` property from `["Grid", "Earth"]` to `["Grid", "Day", "Night"]`. This is only the default value — Rust overrides it at startup, but the Slint file should reflect the intended design.
- **Verify**: The Slint file compiles (`cargo build`). The combobox shows three options.
- **Complexity**: Small

### Phase 6: Cleanup

#### Step 6.1: Remove all `earth_4k.jpg` references

- **Files**: `src/texture_loader.rs` (formerly `earth_texture.rs`), any other files
- **Action**: Search the entire codebase for references to `earth_4k.jpg` and remove them. This includes:
  - Any comments mentioning `earth_4k.jpg`
  - Any path construction for `earth_4k.jpg`
  - The file `textures/earth_4k.jpg` itself (if it exists in the working tree)
  - References in documentation (CLAUDE.md if applicable)
- **Verify**: `grep -r "earth_4k" src/` returns no results. `cargo build` succeeds.
- **Complexity**: Small

#### Step 6.2: Remove dead code from `texture_loader.rs`

- **Files**: `src/texture_loader.rs`
- **Action**: Review the module for any code that is now dead after the transition. The `load()` function and `DecodedImage` struct should remain (they are used by the lazy loader). The `resolve_textures_dir()` function should remain (used by `main.rs`). The `shift_horizontal()` helper should remain (called by `load()`). Verify no unused imports remain.
- **Verify**: `cargo clippy` reports no dead code warnings in this module.
- **Complexity**: Small

#### Step 6.3: Update `CLAUDE.md` module descriptions

- **Files**: `CLAUDE.md`
- **Action**: Update the "Key modules" section:
  - Rename `earth_texture.rs` entry to `texture_loader.rs` with updated description: "generic equirectangular texture loading (JXL via jxl-oxide hook, with coordinate transforms)"
  - Update any mentions of JPEG loading to reflect JXL
  - Mention lazy loading in the architecture section if appropriate
- **Verify**: `CLAUDE.md` accurately reflects the new module structure.
- **Complexity**: Small

### Phase 7: Testing and Verification

#### Step 7.1: Run existing test suite

- **Files**: N/A
- **Action**: Run `cargo test` to confirm all existing tests pass. The camera, sphere, and grid_texture tests should be entirely unaffected. The `earth_texture` module had no tests, so no test migration is needed.
- **Verify**: `cargo test` passes with zero failures.
- **Complexity**: Small

#### Step 7.2: Run clippy and verify no warnings

- **Files**: N/A
- **Action**: Run `cargo clippy` and verify there are no new warnings or errors from the changes.
- **Verify**: `cargo clippy` exits with zero warnings.
- **Complexity**: Small

#### Step 7.3: Manual end-to-end verification

- **Files**: N/A (manual verification)
- **Action**: Run the application and test all texture switching scenarios.
- **Manual test cases**:
  - Application starts: grid renders initially, then Day texture loads and replaces it (grid is the initial `last_rendered_index` fallback)
  - Select "Grid" from combobox: grid texture renders immediately
  - Select "Day" from combobox: after a brief pause (JXL decode), `world.topo.200405.jxl` renders correctly with proper orientation (north up, prime meridian aligned)
  - Select "Night" from combobox: after a brief pause, `BlackMarble_2016.jxl` renders correctly
  - Switch back to "Day": renders immediately (already cached in the bind group)
  - Switch back to "Night": renders immediately (already cached)
  - Switch to "Grid": renders immediately
  - Verify stderr output shows timing messages for each first-time texture load
  - Remove one JXL file from textures dir, restart, select that texture: previously shown texture remains, error printed to stderr
  - Remove both JXL files, restart: application runs with grid texture only (initial fallback), errors printed for Day and Night when selected
  - Camera controls (longitude, latitude, zoom) work correctly with all three texture types
  - MSAA switching works correctly with all three texture types
  - Window resize works correctly with loaded textures
- **Verify**: All manual test cases pass.
- **Complexity**: Medium

## Test Strategy

### Automated Tests

No new automated tests are added in this plan. The texture loading involves GPU resources and file I/O that require integration test infrastructure not yet present in the project. The existing unit tests (camera, sphere, grid_texture) verify that unrelated modules are not broken.

| Test Case | Type | Input | Expected Output |
|---|---|---|---|
| Sphere vertex count | Unit (existing) | `generate_uv_sphere(4, 8)` | 45 vertices |
| Sphere index count | Unit (existing) | `generate_uv_sphere(4, 8)` | 192 indices |
| Camera eye position | Unit (existing) | `OrbitalCamera::new(0, 0, 5)` | (0, 0, 5) |
| Grid texture size | Unit (existing) | `generate(64, 32)` | 8192 bytes |
| Grid pixels opaque | Unit (existing) | `generate(64, 32)` | All alpha = 255 |

### Manual Verification

- [ ] Application starts without errors when JXL files are present in `textures/`
- [ ] Selecting "Day" loads and renders `world.topo.200405.jxl` with correct orientation
- [ ] Selecting "Night" loads and renders `BlackMarble_2016.jxl` with correct orientation
- [ ] Subsequent selections of already-loaded textures are instantaneous (cached)
- [ ] Grid texture continues to render correctly
- [ ] Missing JXL files produce graceful fallback to previous texture, not a crash
- [ ] Timing output in stderr shows decode duration for each first load
- [ ] Camera, MSAA, and resize all work correctly with all texture types
- [ ] `cargo build --release` succeeds
- [ ] `cargo clippy` passes with no warnings

## Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| 8K JXL decode causes multi-second UI freeze on first texture selection | Poor UX during first load | Accept for initial implementation. Timing instrumentation (Step 3.4) measures the impact. Background thread decoding can be added in a follow-up if the freeze exceeds ~3 seconds. |
| jxl-oxide's `image` feature hook fails silently for certain JXL variants | Texture loads fail with opaque error | The `load()` function already returns `Result` with descriptive errors. Log the error and fall back to grid. The NASA textures are standard sRGB, unlikely to hit edge cases. |
| `image` crate 0.25.8 introduces breaking API changes vs 0.25.x | Build failure | The image crate follows semver; 0.25.8 is compatible with 0.25.x. The `image::open()` API is stable. |
| 8K textures with full mipmaps consume ~256 MB GPU memory each (~512 MB total for two) | Out-of-memory on low-VRAM GPUs | This is inherent to 8K textures and not specific to JXL. The lazy loading approach means only selected textures consume GPU memory. If both are loaded and the user switches back to grid, the bind groups remain allocated. A future optimization could evict unused textures. |
| `register_decoding_hook()` is called multiple times (once at startup, potentially again during lazy load) | Unexpected behavior | The function is documented as idempotent in jxl-oxide. Safe to call multiple times. |
| The `bind_group_layout` and `sampler` must outlive individual bind groups | Use-after-free or borrow issues | Both are stored as fields on `GpuResources`, which owns all GPU resources. Lifetimes are tied to the struct. |

## Rollback Strategy

Revert the commits from this plan. Restore `earth_texture.rs` (from git history), restore the `image` crate's `jpeg` feature, and remove the `jxl-oxide` dependency. The Slint UI defaults can be reverted from `["Grid", "Day", "Night"]` to `["Grid", "Earth"]`. No data files are modified (the JXL files are untracked and remain in `textures/`).

## File Inventory

```text
Cargo.toml                              (modified: add jxl-oxide, bump image, remove jpeg feature)
src/main.rs                             (modified: remove JPEG loading, add JXL hook, pass texture paths)
src/renderer.rs                         (modified: TextureSlot struct, lazy bind group loading, Vec storage)
src/earth_texture.rs                    (renamed to src/texture_loader.rs, doc updates)
src/texture_loader.rs                   (new name for earth_texture.rs, add register_jxl_hook())
ui/main.slint                           (modified: default texture-options updated)
CLAUDE.md                               (modified: update module descriptions)
```

## Status

- [x] Plan approved
- [x] Implementation started
- [x] Implementation complete
