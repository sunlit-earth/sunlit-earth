# Plan: The Preview Fills Its Panel

## Summary

The 3D preview should always cover the whole of `image-container`, edge to edge. Today it can leave a black band of up to 63 physical pixels along the right or bottom edge, because the engine rounds the preview's render size down to a multiple of 64 px and the window shows the frame with `image-fit: contain`. The rounding was introduced to stop the render targets from being rebuilt on every pixel of a resize. This plan removes it, has the window show the frame with `image-fit: cover`, and replaces the protection the rounding gave with a settle rule in the window: a new preview size goes to the engine only once the viewport has stopped changing.

## Stakes Classification

Low. No setting, config key, IPC line or shader changes. The wallpaper and export paths have their own sizes and are untouched. What changes is the preview's render size, one `image-fit`, and when the window tells the engine about a new size.

## What the code does today

Checked against `main` at 20b3276:

- `start_viewport_timer` (`crates/sunlit-app/src/app.rs`) polls `viewport-width` and `viewport-height` (bound to `image-container`'s size in `main.slint`) every `VIEWPORT_POLL`, 200 ms, multiplies by the scale factor and sends `EngineCommand::SetPreviewSize` whenever the physical size differs from the last one sent. Nothing waits for a resize to finish: during a splitter drag every tick with a new size reaches the engine.
- The engine's `SetPreviewSize` handler calls `preview_target_size` (`crates/sunlit-core/src/engine/mod.rs`), which caps the width at `QualityTier::max_preview_width` (1280 Low, 1920 Medium, unbounded High), scales the height by the same factor, and passes both through `quantize_to_granularity` (`crates/sunlit-core/src/renderer/sizing.rs`), which floors each dimension to a multiple of `SIZE_GRANULARITY`, 64, never below 64. A size that differs from `Renderer::size()` calls `Renderer::resize`, which rebuilds the render texture, the depth texture and the MSAA views (`rebuild_render_textures` in `gpu_setup.rs`), drops the preview readback buffer, and marks the engine dirty, so the next loop iteration redraws. The initial size from `EngineConfig::preview_size` goes through the same function.
- `quantize_to_granularity` has no other caller. The export and wallpaper renders size their own targets.
- The camera's aspect ratio is the render target's (`render_pass.rs`, `viewport_width as f32 / viewport_height as f32`), so a floored size is a different aspect from the panel's, and `image-fit: contain` (`main.slint`, the `Image` in `image-container`) letterboxes it.
- Readback already handles any width: `padded_row_bytes` rounds a row up to the 256 byte copy alignment and `read_texture_rgba8_into` strips the padding. Nothing else in the renderer needs a size that is a multiple of anything.
- Nothing in the window computes where the image sits inside the container. `mouse_math::fine_drag_gain` reads `viewport-width`, the container's width.
- The rounding came in with e57b061 (March 2026), when the frame was rendered inside Slint's own render loop on every resize step. The engine thread and the 200 ms poll came later.
- Tests that lean on the rounding: `sizing.rs`'s `quantization_floors_each_dimension_to_a_whole_granule`; `engine/mod.rs`'s four `preview_size_*` unit tests; `tests/engine/lifecycle.rs`'s `preview_size_changes_are_quantized_and_applied`, which expects (300, 200) to become (256, 192) and (512, 288) to become (512, 256). The engine harness (`tests/engine/harness.rs`) and the soak (`tests/soak.rs`) start at (512, 288), which today renders 512 x 256; `tests/engine/months.rs` says so in a comment and builds a 512 x 256 screen to match, and `tests/engine/groups.rs` documents `FRAME` (512, 256) as "the size the preview quantizes to". The golden target starts at 512 x 256, already a multiple of 64.
- Docs that state the rounding: `docs/architecture.md` (the line "Render texture size is quantized to 64px boundaries...") and the Key constraints bullet in `CLAUDE.md` ("Render texture size is quantized to 64 px and capped by the quality tier"). The doc comment on `start_viewport_timer` says "The engine quantizes the value".

## Key Design Decisions

### 1. The preview renders at the size it is asked for

`preview_target_size` keeps the tier cap and drops the quantization. The capped height is rounded to the nearest pixel rather than truncated, so the aspect is as close to the panel's as whole pixels allow. Each dimension is held at one pixel at least, since wgpu refuses a zero-sized texture. `quantize_to_granularity` and `SIZE_GRANULARITY` are deleted, and the module doc of `sizing.rs` stops mentioning a granularity.

### 2. The window shows the frame with `cover`

`image-fit: cover` scales the frame until it covers the container and crops what overhangs, keeping the aspect. Once the engine has drawn at the panel's size this is an exact fit at High and a crop of under a pixel where the tier cap scales the frame. While a resize is in progress, the old frame stays on screen at the old aspect: `cover` shows it cropped with no band and no distortion, where `contain` would show a band and `fill` would stretch the globe. This was proposed as `fill` in the conversation that led to this plan and changed to `cover` for that reason.

### 3. The window sends a size once it has settled

`start_viewport_timer` keeps its 200 ms poll and sends `SetPreviewSize` only when the size read on this tick equals the size read on the previous tick and differs from the last size sent. A drag therefore costs no rebuild and no redraw until the pointer has rested for one poll interval, and then exactly one. The decision is a small pure type in `sunlit-app` (for example `ViewportSettle` with `fn observe(&mut self, size: (u32, u32)) -> Option<(u32, u32)>`), so it is unit tested without a window. Zero sizes are never sent, as today.

Startup is unchanged: the engine starts from `EngineConfig::preview_size`, and the first settled reading that differs from what was sent (nothing has been sent yet) goes out after two ticks.

Whether Slint's timers fire while Windows holds the event loop in its modal resize loop has not been checked. If they do not, a window drag on Windows already sends nothing until it ends; either way the rule above gives one size per resize.

### 4. Not chosen

- Keeping the quantization and switching to `cover` alone: the frame would be cropped by up to 63 px instead of letterboxed, which moves the globe off center in the panel by up to 31 px.
- Rounding up instead of down: renders up to 63 px beyond the panel for nothing, and still needs `cover`.
- A coarser engine-side debounce: the engine sees commands, not pointer state, and the window already polls; the settle rule belongs where the poll is.

## Success Criteria

1. At every window size and splitter position, once the size has settled, the preview covers `image-container` with no black band on any edge, on every quality tier.
2. A splitter drag or a window resize leads to one `SetPreviewSize`, and so one target rebuild and one redraw, after the movement stops, not one per poll tick while it moves. Shown by the settle type's unit tests and by the engine's `viewport size changed` debug log during a manual drag.
3. Every engine, golden and render pipeline test that drew a frame before draws the same frame after; no golden reference changes.
4. The gates below pass.

## Implementation Steps

### Step 1: Engine sizing

In `crates/sunlit-core/src/engine/mod.rs`, rewrite `preview_target_size` per decision 1 and replace its unit tests with behavior tests: a size below the cap comes back as asked; a size above the cap comes back at the cap's width with the aspect within one pixel of the request's; zero in either dimension comes back as one; the cap grows with the tier. Delete `quantize_to_granularity`, `SIZE_GRANULARITY` and their test from `renderer/sizing.rs`, and the re-export in `renderer/mod.rs`. Update the `Renderer::resize` doc comment, which says the caller has quantized the size.

### Step 2: Test fixtures keep their frames

Change the engine harness's and the soak's starting size from (512, 288) to (512, 256), the size they rendered at, so no case's frame changes. Rewrite `lifecycle.rs`'s `preview_size_changes_are_quantized_and_applied` into `preview_size_changes_are_applied` with sizes that are not multiples of 64, asserting the renderer reports what was asked. Fix the comments in `months.rs` and `groups.rs` that say "quantized". Search the test trees for any other size that is not a multiple of 64 and would now render differently, and keep its old frame the same way.

### Step 3: The window

In `crates/sunlit-app/ui/main.slint`, set the preview `Image` to `image-fit: cover`. In `crates/sunlit-app/src/app.rs`, add the settle type per decision 3, use it in `start_viewport_timer`, rewrite that function's doc comment, and add unit tests: a steady size is sent once; a size that changes on every tick is never sent; a size that changes, then holds for one tick, is sent once; returning to the last sent size sends nothing; zero sizes are never sent.

### Step 4: Documentation

Update `docs/architecture.md` (the quantization line, and wherever the preview's sizing or the viewport poll is described), the Key constraints bullet in `CLAUDE.md` (the render texture size is the panel's, capped by the quality tier), and add a done entry to `docs/roadmap.md`. Search `docs/` (outside `plans/` and `reviews/`) for other mentions of the 64 px granularity. `README.md` is not edited.

### Step 5: VM runs

Run `cargo xtask e2e --target windows` and `cargo xtask e2e --target linux` (KDE) and record the outcome in the Validation Record. Builds for the Linux guest go through WSL as the xtask arranges them. Run `vm down all` afterwards. If a VM command fails for host reasons, run `cargo xtask vm doctor`, record what it says, and carry on; do not run `vm setup` or `vm build-image`.

### Step 6: Validation with the user

The user resizes the window and drags the splitter on their desktop and confirms the preview fills the panel. The orchestrator handles this after the validation loop.

## Gates

- `cargo fmt --check`
- `cargo test` (the whole workspace)
- `cargo clippy --all-targets` with no warnings (CI builds with `-D warnings`)
- Step 5's e2e runs, recorded

## Out of Scope

- The tier caps themselves.
- The wallpaper and export sizes.
- Clamping the preview to the adapter's `max_texture_dimension_2d`, which the preview never did before either.

## Risks and Mitigations

- A test that relied on the floor silently draws a different frame: step 2 keeps every fixture at the size it rendered at, and the goldens are the backstop.
- The settle rule delays the first redraw after a resize by 200 to 400 ms. That is the point of the rule; `cover` keeps the old frame filling the panel meanwhile.
- More distinct target sizes over a session: each rebuild frees the old targets first (`replace_render_textures`), and with the settle rule there is one rebuild per resize.

## Rollback Strategy

Revert the branch's commits. Nothing persisted changes shape.

## Departures

None yet.

## Open items

None yet.

## Validation Record

Steps 1 to 5, on the tree of 473f076, 2026-10-22, on the Windows 11 host:

- `cargo fmt --check`: clean.
- `cargo clippy --all-targets`: no warnings.
- `cargo test`: every target passed, among them the engine target's 138 cases, the golden target's 25 with no reference changed, render_pipeline's 51, the soak, `sunlit-earth`'s 122 unit tests with the five `ViewportSettle` cases, and `slint_ui`'s 132.
- `cargo xtask e2e --target windows`: 18 passed, 0 failed, in 532.80 s, 638 s for the guest. The layout change, the setter line and the plasmashell cases skipped themselves, as they do off Linux and off KDE.
- `cargo xtask e2e --target linux` (KDE Plasma, X11): 17 passed, 0 failed, in 468.79 s, 535 s for the guest; the session end case is Windows only. The hidden window memory case grew by 16.5 MiB against its 40 MiB limit.
- `cargo xtask vm down all` afterwards; `vm status` shows no overlays and no run state.

Validation round 1, on e6071d1..a811b9d: no majors, four minors, all fixed. The doc comments on `EngineConfig::preview_size` and `EngineCommand::SetPreviewSize` still said quantized; `docs/testing.md` still named the harness's old 512 x 288 preview; the `preview_target_size` tests wrote the tier caps down as numbers, and now derive their sizes from `max_preview_width`; and `start_viewport_timer`'s doc comment left out the tier cap.

Step 6, 2026-10-24: the user resized the window and dragged the splitter on their desktop and signed off. Merged `main` at 7bc987c (the wallpaper's wait for the cloud layer, #85) into the branch; the one conflict was the two new entries at the end of `docs/roadmap.md`, both kept. On the merge, `cargo fmt --check` and `cargo clippy --all-targets` are clean and `cargo test` passes, the engine target's 147 cases among them.
