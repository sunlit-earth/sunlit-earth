<!-- Reviewer notes for docs/reviews/2026-09-04-code-quality-review.md. Line numbers refer to commit 3046327. "The brief" is the shared review instruction; "the maintainer's rules" are the project's comment conventions. Runtime claims here are reasoned from the code; the measured figures are in test-timing.md. -->

# Review: renderer, params, geometry, shaders (`crates/sunlit-core`)

## 1. Summary

The code is correct and carefully reasoned; almost every finding below is about cost of change, not about bugs. Three things stand out. First, the production `Uniforms` struct is kept in sync with the WGSL only by eye: the GPU test that checks field offsets writes a *copy* of the struct that lives in the test file, so a reorder in `renderer/uniforms.rs` alone passes every test. Second, the ten pipeline descriptors in `gpu_setup.rs` are near-copies of one another and are then constructed a second time, verbatim, in `rebuild_msaa_resources`; that is where the file's 934 lines come from, not from one giant function. Third, `params.rs` is not really a 1112-line file: 665 of those lines are its test module and 445 are a single hand-written mutation table. The digest is hand-written per field, and the "table-driven test" cannot catch a parameter that was never added to the table, so the guarantee CLAUDE.md claims for it is weaker than stated.

The maintainer's own concerns land: commentary is heavy (25 comment blocks over three lines in `renderer/mod.rs`, 30 in `sphere.wgsl`), several long blocks are design history that belongs in `docs/`, and three are now factually stale. Unit tests are cheap, not slow: the 28 tests in `renderer/mod.rs` create no GPU device at all, contrary to the premise in my brief. Six of them are redundant.

## 2. Metrics

| File | Total | Test mod | Doc `///`/`//!` | Line `//` | Blocks > 3 lines (keep/move/delete) | Fns > 80 lines | `pub` only used in-crate |
|---|---|---|---|---|---|---|---|
| `renderer/mod.rs` | 1147 | 308 (28 tests) | 243 | 22 | 25 (16/6/3) | `export_image_with` 88 | `preview_texture` (unused anywhere), `resolve_sample_count`, `quantize_to_granularity`, `RenderOutcome`, `RendererConfig`, and 13 `Renderer` methods |
| `renderer/gpu_setup.rs` | 934 | 0 | 36 | 13 | 5 (2/2/1) | `create_renderer` 299 | n/a (all `pub(super)`) |
| `renderer/render_pass.rs` | 660 | 0 | 67 | 36 | 8 (4/3/1) | `write_uniforms` 146, `encode_and_submit` 148 | `read_texture_rgba8` (tests use it, keep) |
| `renderer/textures.rs` | 412 | 34 (4 tests) | 59 | 30 | 10 (8/2/0) | none | n/a |
| `renderer/frame.rs` | 114 | 76 (7 tests) | 9 | 0 | 0 | none | n/a |
| `renderer/uniforms.rs` | 109 | 0 | 34 | 0 | 0 | none | n/a (`pub(crate)`, and that is the problem, see A/E) |
| `renderer/texture_routing.rs` | 66 | 0 | 9 | 4 | 0 | none | n/a |
| `params.rs` | 1112 | 665 (16 tests) | 71 | 11 | 9 (5/3/1) | test `every_shader_parameter_triggers_dirty` 445 | `quantize_direction`, `ParamsDigest`, `CLOUD_SPHERE_RADIUS`, `RAYLEIGH_RADIUS`, `NIGHTGLOW_ORANGE_RADIUS`, `NIGHTGLOW_GREEN_RADIUS` |
| `geometry/sphere.rs` | 157 | 63 (6 tests) | 5 | 8 | 0 | none | `SphereMesh`, `generate_uv_sphere`, `Vertex`, `Vertex::buffer_layout` |
| `geometry/grid_texture.rs` | 161 | 88 (7 tests) | 6 | 23 | 1 (0/0/1) | none | `generate` |
| `shaders/sphere.wgsl` | 1144 | n/a | 159 | 149 | 30 (18/9/3) | none (longest `fs_main` 63) | n/a |
| `shaders/blend.wgsl` | 44 | n/a | 9 | 0 | 1 (1/0/0) | none | n/a |

Struct sizes: `Renderer` 47 fields, `SceneParams` 51, `ParamsDigest` 49, `Uniforms` 69.

Comment density in `sphere.wgsl` is 308 comment lines against 725 code lines (30%). In `gpu_setup.rs` it is 49 against 843 (5%).

## 3. Findings

### A. Commentary

**A1 (medium). Three comments are stale and one is factually wrong.**
- `gpu_setup.rs:215` `// Slot 3 = cloud overlay (populated by the cloud fetcher thread, not file-based)`. In production the cloud slot is 5: `SlotLayout::clouds()` is `file_backed + 1` and there are four file-backed paths. The comment was true when there were two. Delete it, or write `// The cloud overlay, always last; SlotLayout::clouds names its index.`
- `gpu_setup.rs:780-782` `Preview passes use RENDER_ATTACHMENT | TEXTURE_BINDING`. `PREVIEW_USAGE` at `gpu_setup.rs:15-17` also carries `COPY_SRC`, which is what `read_preview_pixels` depends on. Fix or delete the sentence.
- `params.rs:1-8` names `renderer::uniforms` as the second translation point. It is `renderer::render_pass::write_uniforms`; `docs/architecture.md:65` and CLAUDE.md both say so. It also says "the same thirty values"; there are 51 fields now. This whole block is history ("Before it existed... adding one shader knob meant touching eight files") and belongs in `docs/` or a commit message. Replace with two lines naming the two translation points.
- `mod.rs:340-342` says the 1x1 texture is used "in single-texture bind groups (Grid, Day, Night modes)". The cloud bind group uses it too (`textures.rs:194`). Minor.

**A2 (medium). Design history in source.** These read as records of decisions rather than as things the reader of the code needs:
- `mod.rs:51-57` `TextureMode` doc, last sentence: "Those facts agreed numerically while the day/night blend index and the cloud slot were both three, which is what let one integer stand for both." That is the same sentence as `docs/architecture.md:77`. Delete here.
- `render_pass.rs:264-270`, the clear color: "Near black rather than the faint blue this was while it was the whole sky." History. The last sentence ("Unconditional, so a checkout without the panorama's Git LFS object does not change color the day it arrives") is a keep. Trim to that.
- `gpu_setup.rs:521-527`, the moon pipeline doc, is a near-verbatim third copy of `docs/rendering.md:52` and `sphere.wgsl:270-281`. Keep one sentence here ("Opaque so it covers the Sun's additive disk; back-face culled; no depth write") and let the docs carry the argument.
- `mod.rs:562-570` `textures_pending`: keep the first five lines, delete "The question this answers is 'will this get better on its own', which is the only sound reason to hold something back."
- `render_pass.rs:436-460` and `520-538`: the second paragraph of each `select` doc argues by comparison with the other draws. Keep the first paragraph, move the comparison to `docs/rendering.md`, which already has it.

**A3 (low). Comments that restate the next line.** `gpu_setup.rs:49` `// Generate sphere mesh`, `:84` `// Shared sampler for all textures`; `mod.rs:733` `// Look up the bind group that was used for the last rendered frame`, `:745` `// Create temporary render textures with COPY_SRC for readback`; `render_pass.rs:353` `// Nightglow orange overlay (additive, sodium D + FeO, ~1.014 radius)` and `:360` for green, both of which the pipeline name and the doc comment 130 lines above already say. Delete.

**A4 (low). Test comments that narrate why a test exists.** The brief calls these out explicitly. `mod.rs:1102-1103` ("which is what kept the old identity mapping from landing on the cloud slot"), `mod.rs:1068-1072`, `params.rs:520-526` (a 7-line doc on `the_display_plan_is_not_a_shader_parameter` that argues a design decision), `grid_texture.rs:289-293` (four lines of stream of thought: "Actually lat_deg=45 is a minor grid line too. We just need lon or lat on a 15-degree multiple that isn't major."). Delete.

**A5. Comments worth keeping, so the pass is fair.** `textures.rs:42-48` (why a discarded post must not clear `loading`), `textures.rs:82-86` (drop versus destroy), `render_pass.rs:102-107` (why a Moon with no disc is not drawn), `mod.rs:211-219` (`resolve_sample_count`: an unsupported count is a validation error, not a warning), `sphere.wgsl:207-215` (`star_halo_profile`), `sphere.wgsl:860-863` (why the hash is a PCG one and not `fract(sin(...))`), `sphere.wgsl:453-455` (`pow(0,0)` is NaN through WGSL's `exp2(y*log2(x))`), `sphere.wgsl:1104-1113` (the `atan2` branch cut and the gradient), `gpu_setup.rs:690-694` (the premultiplied Rayleigh blend). All of these carry an invariant or a workaround that the code cannot state itself.

### B. Length and structure

**B1 (high). `gpu_setup.rs` is not one giant function; it is one 299-line function plus ten near-identical pipeline builders, built twice.**
`create_renderer` is `gpu_setup.rs:34-332`. The ten builders are `create_sky_quad_pipeline` (344-400), `create_star_pipeline` (402-471), `create_pipeline` (473-519), `create_moon_pipeline` (528-574), `create_cloud_pipeline` (576-622), `create_atmo_shell_pipeline` (626-682), `create_rayleigh_pipeline` (684-742), and the two nightglow wrappers (744-776). Every one is the same 45-line `RenderPipelineDescriptor` differing only in label, entry points, vertex buffer layout, blend state, topology, cull mode, and depth compare. Then `rebuild_msaa_resources` (861-913) reconstructs all ten with the same arguments, string literals included: `"milky_way_pipeline"`, `"vs_milky_way"`, `"fs_milky_way"` appear at 249-251 and again at 872-874, and so on for the sun disk, the sun glare, and the rest. Adding an eleventh pipeline and forgetting the second site leaves a stale pipeline at the old sample count after an MSAA change, which is a wgpu validation error on the engine thread.

Proposed split, in this order:
1. `renderer/gpu_setup/pipelines.rs`: a `Pipelines` struct holding the ten handles, and one `Pipelines::build(device, layout, shader, sample_count) -> Pipelines` driven by a small table of `(label, vs, fs, kind)` where `kind` is `SkyQuad | StarSprite | Mesh { blend, depth_write, depth_compare }`. `create_renderer` and `rebuild_msaa_resources` both call it, and `Renderer` holds `pipelines: Pipelines` instead of ten fields. Source range absorbed: 334-776 and 861-913, about 495 lines collapsing to roughly 130.
2. `renderer/gpu_setup/targets.rs`: `PREVIEW_USAGE`, `COLOR_FORMAT`, `DEPTH_FORMAT` (12-30) plus `create_render_textures`, `rebuild_render_textures`, `replace_render_textures` (778-934). About 175 lines.
3. What is left in `gpu_setup.rs` is the mesh and star buffers, the sampler, the bind group layout, the dummy texture, the grid texture, the slot vector, the shader module, and the struct literal. About 260 lines, and the `#[allow(clippy::too_many_lines)]` at line 32 can then go.

**B2 (medium). `renderer/mod.rs` splits cleanly along two seams and needs no behavior change.**
- `renderer/sizing.rs`: `SIZE_GRANULARITY` (36), `build_aa_options` (190-209), `resolve_sample_count` (220-236), `quantize_to_granularity` (240-244), plus their 22 tests (850-1005). These are engine and combo box policy with no GPU content, and they account for 306 of the file's 308 test lines.
- `renderer/slots.rs`: `DAY_SLOT`..`MILKY_WAY_SLOT` (38-45), `TEXTURE_LABELS` (49), `TextureMode` (58-91), `SlotLayout` (100-167), `SLOT_LABELS` (176-182), plus the 8 layout tests (1011-1121).
What remains is the `Renderer` struct, its impl, and `planet_instance_bytes`: roughly 360 lines.

**B3 (medium). `Renderer` has 47 fields in append order rather than grouped.** Six pipelines are at 276-285 and four more at 352-359, with the composite bind group and two texture views between them. Grouping into `pipelines: Pipelines`, `targets: Targets` (render/depth/msaa views, size, sample count), `textures: TextureState` (slots, generation, cache dir, views, composite and cloud bind groups), and `last: LastFrame` (state, params, inputs, resolved) would make the struct readable and would make B1's rebuild a single assignment.

**B4 (medium). `encode_and_submit` takes 20 arguments, eight of which are already a struct at both call sites.** `render_pass.rs:230-250`. Both callers build an `Overlays` (`mod.rs:773`, `render_pass.rs:408`) and then destructure it into eight positional arguments. Pass `Overlays` itself, and group `pipeline`/`bind_group`/`vertex_buffer`/`index_buffer`/`index_count` into an `Earth<'a>`. That takes the signature to nine parameters and may retire the `#[allow(clippy::too_many_arguments)]` at line 229.

**B5 (low). `write_uniforms` is 146 lines, of which 79 are an irreducible struct literal.** The seam is at `render_pass.rs:77-131`: the camera assembly, `place_moon`, and `place_sun` are a placement step that could be `fn place(params, inputs, viewport) -> Placement`. Then `write_uniforms` is only the encoder and the `too_many_lines` allow is honest rather than covering two jobs.

**B6 (low). `params.rs` is 447 lines of code and 665 of tests.** The code half does not need splitting. See F1 for the test half.

**B7 (low). `sphere.wgsl` has no function over 63 lines and its sections are half delimited.** Three banner headers (`// ---...` at 270, 461, 1063) and four `// --- X ---` headers (522, 605, 645, 684). The whole prologue, lines 1-268 (uniform struct, bindings, vertex structs, star constants, the sky lens, the star sprite), carries no header, and the Earth and the clouds (324-459) sit between the Moon banner and the light-path banner with no heading of their own, so the shader's primary subject is the one layer that is not announced. Adding three banners in the existing style would cost nothing and would make the file navigable.

### C. Consistency

**C1 (medium). Module doc headers are present in four files and absent in seven.** Present: `lib.rs`, `renderer/mod.rs`, `params.rs`, `geometry/grid_texture.rs`. Absent: `gpu_setup.rs`, `render_pass.rs`, `textures.rs`, `frame.rs`, `uniforms.rs`, `texture_routing.rs`, `geometry/sphere.rs`. `uniforms.rs` is the odd one: it opens with a `///` doc on the struct at line 1 where every other file that documents itself uses `//!`.

**C2 (medium). Two idioms for "is this draw on this frame".** `MilkyWay`, `Stars`, `Sun`, and `Moon` are `struct X<'a>` with `X::select(...) -> Option<X>` (`render_pass.rs:438-538`). `Overlays` (543-591) is four `(Option<&RenderPipeline>, Option<&BindGroup>)` tuples that are always both `Some` or both `None`, checked with `if let (Some(a), Some(b))` at four places in `encode_and_submit`. Make the four fields `Option<Overlay<'a>>` and the two idioms become one.

**C3 (medium). `#[allow]` versus `#[expect]`, and five stale allows to prove the point.** The crate uses `#[allow]` in 22 places across these files and `#[expect]` in exactly two (`assets/stars.rs:35`, `tests/render_pipeline.rs:546`). These five allows are, by reading, no longer earning anything:
- `mod.rs:724` `#[allow(clippy::cast_precision_loss)]` on `export_image_with`: the body (725-812) contains no numeric cast.
- `render_pass.rs:380` `#[allow(clippy::cast_precision_loss)]` on `execute_render_pass`: same, no cast in 382-434.
- `params.rs:300` `#[allow(clippy::cast_possible_truncation)]` on `digest`: every cast is inside `q`, which carries its own allow at line 357.
- `params.rs:205` `#[allow(clippy::cast_precision_loss)]` on `write_to_config`: the only conversion is `f32::from(u16)`, which is lossless.
- `params.rs:363` `#[allow(clippy::cast_possible_truncation)]` on `quantize_direction`: it only calls `q`.
- Also `params.rs:37` `#[allow(clippy::struct_excessive_bools)]` on `SceneParams`, which has two bools; clippy's threshold is three.
Switching to `#[expect]` makes each of these a warning under `cargo clippy --all-targets`, which is the command CLAUDE.md says is on the developer because CI does not run it. It would have caught all six.

**C4 (low). Error style is consistent but coarse.** `export_image` and `export_image_with` return `Result<Vec<u8>, String>` with three distinct `&'static str` messages, two of which are the same text (`"No frame rendered yet"` at `mod.rs:700`, `:731`, `:734`). After a resolution switch, `purge_file_backed_slots` clears `last_resolved` but leaves `last_inputs`, so an export in that window reports "No frame rendered yet" when a frame has in fact been rendered. A two-variant enum (`NoFrame`, `TexturesReloading`) would say the true thing and cost nothing.

**C5 (low). `quantize_to_granularity`'s doc says "nearest" and the code truncates.** `mod.rs:238-244`: `(w / 64).max(1) * 64` floors. `quantize_typical_display` at `:1003` asserts 1080 becomes 1024, where the nearest multiple is 1088. Truncating is almost certainly the intent (never allocate more than asked). Fix the word.

**C6 (low). `pub` items that nothing outside the crate uses.** Verified by grepping `crates/sunlit-app`, `crates/sunlit-core/tests`, and `crates/xtask`:
- `Renderer::preview_texture` (`mod.rs:374`) is used nowhere in the workspace at all. Its doc and the `TEXTURE_BINDING` flag in `PREVIEW_USAGE` describe a client that binds the texture directly, which CLAUDE.md says does not exist ("Preview frames cross to the UI as RGBA pixel buffers... Slint has no wgpu feature and shares no device"). It survives only because `pub` suppresses `dead_code`. Delete it, and decide whether `TEXTURE_BINDING` is still wanted.
- Used only by `engine/mod.rs`: `resolve_sample_count`, `quantize_to_granularity`, `RenderOutcome`, `RendererConfig`, and `Renderer::{size, has_frame, set_sky_state, drain_texture_updates, set_texture_resolution, texture_resolution, memory_report, textures_ready, textures_pending, loading_text, read_preview_pixels, export_image, export_image_with, export_limits}`.
- `params::{quantize_direction, ParamsDigest, CLOUD_SPHERE_RADIUS, RAYLEIGH_RADIUS, NIGHTGLOW_ORANGE_RADIUS, NIGHTGLOW_GREEN_RADIUS}`, `geometry::sphere::{SphereMesh, generate_uv_sphere, Vertex}`, `geometry::grid_texture::generate`.
- Truly external and should stay `pub`: `renderer::{TEXTURE_LABELS, SlotLayout, build_aa_options, read_texture_rgba8}`, `params::{SceneParams, gamma_slider_to_value, gamma_value_to_slider, CLOUD_TERMINATOR_WIDTH}`.
Narrowing the rest to `pub(crate)` before the first release is cheap and would shrink the published API by about 25 items.

### D. Duplication

**D1 (high). The preview pass and the export pass repeat 45 lines.** `render_pass::execute_render_pass` (382-434) and `Renderer::export_image_with` (`mod.rs:725-812`) both call `write_uniforms`, build a `RenderTarget`, call `Overlays::select`, `MilkyWay::select`, `Stars::select`, `Sun::select`, and then `encode_and_submit` with the same twenty arguments. `Overlays`' own doc says it is "Shared by the preview pass and the wallpaper export so the two cannot drift apart", but only the *selection* is shared; the call site is not. Extract `fn draw_scene(res, params, bind_group, inputs, width, height, target)` and have both callers build only their target. The two `log_memory_usage` calls stay at the export call site.

**D2 (high). The `Uniforms` struct exists three times.** `renderer/uniforms.rs:6-107` (69 fields), `tests/render_pipeline.rs:20-89` (a byte-identical hand copy, its header comment claiming "integration tests can't import from a bin crate", which is not true: `sunlit-core` is a library and the same test file already imports `read_texture_rgba8` from it), and `shaders/sphere.wgsl:1-71`. See E1 for why the copy is dangerous and not merely wasteful.

**D3 (medium). Duplicated shader bodies.**
- `vs_rayleigh` (525), `vs_nightglow_orange` (608), `vs_nightglow_green` (648), and `vs_cloud` (410) are the same five statements with a different radius uniform. One `fn shell_vertex(in: VertexInput, radius: f32) -> VertexOutput` and four one-line entry points.
- `fs_nightglow_orange` (618-643) and `fs_nightglow_green` (658-682) are 26 lines each differing only in `time_mod`, `color`, and which side of `nightglow_balance` they take. The `lat_mod` expression is verbatim at 636 and 675, and the `rim` expression at 626-627 and 665-666.
- `fs_sun_disk:993` inlines the body of `limb_hue_km` (`sphere.wgsl:495-498`) instead of calling it. The two can drift.

**D4 (medium). `generate_uv_sphere` exists twice.** `geometry/sphere.rs:44` and `tests/render_pipeline.rs:103`, the latter labeled "simplified version of production code". `generate_uv_sphere` and `Vertex` are both already `pub`, so the copy is unnecessary and can be deleted outright.

**D5 (low). The "adding a shader parameter means" checklist exists three times**: `CLAUDE.md`, `docs/rendering.md:60`, and `params.rs:1-8` in prose form. Keep one.

### E. Correctness and resilience

**E1 (high, confirmed by reading). Nothing checks the production `Uniforms` layout against the WGSL.** The chain is: `renderer/uniforms.rs` `Uniforms` (69 fields, `const _: () = assert!(size_of == 544)` at line 109) is written to the buffer by `write_uniforms`. The guard test `uniform_buffer_field_offsets_match_wgsl` (`tests/render_pipeline.rs:912`) writes the *test file's own copy* of the struct (line 20, with its own 544-byte assert at line 91) and reads it back through a compute shader that uses the WGSL struct. So it proves *test copy == WGSL*, and *production == test copy* is held only by eye plus a shared byte count. Swapping any two adjacent `f32` fields in `renderer/uniforms.rs` alone keeps the size at 544, passes every test in the workspace, and silently feeds the shader two swapped values. With 69 fields, 26 of which are consecutive scalar floats, this is a realistic edit. Fix: make `renderer::uniforms` and `Uniforms` `pub` (or `#[doc(hidden)] pub`), import it in `render_pipeline.rs`, and delete the 70-line copy. The existing test then guards the real struct.

**E2 (medium, confirmed). `read_texture_rgba8` panics on device loss.** `render_pass.rs:644-647`, three `unwrap`s. Taking them one at a time:
- `:646` `device.poll(wgpu::PollType::wait_indefinitely()).unwrap()` fails on a lost device, which on Windows is a real event (a TDR or a driver update resets the adapter). This is on the wallpaper export path and on `read_preview_pixels`, so a GPU reset panics the engine thread rather than failing the export.
- `:647` `rx.recv().unwrap().expect("buffer mapping failed")`. The `recv` cannot fail while the callback is alive; the `expect` on the map result fails for the same device-level reasons as above.
- `:644` `tx.send(result).unwrap()` inside the map callback. It cannot fail on the happy path. It can fail during unwinding: if `:646` panics, `rx` is dropped while wgpu still holds the closure, and a later invocation of that callback panics inside a panic, which aborts. That is a second-order concern, but it is free to fix by ignoring the send result.
Recommendation: return `Result<Vec<u8>, String>` (the export path already returns `Result<_, String>` and can propagate), `let _ = tx.send(result);`, and map the poll error into that result. So: no, these three are not on values that cannot fail.

**E3 (low, confirmed). `Renderer::render`'s two `expect`s do hold.** `mod.rs:639` "composite bind group must exist when Composite is returned": `texture_routing::resolve_textures` returns `Composite` only inside `if res.composite_bind_group.is_some()` (`texture_routing.rs:193`), and nothing clears it in between. `mod.rs:643` "render_index must always point to a loaded slot": `resolve_render_index` (`textures.rs:254`) returns the requested slot only when its bind group is `Some`, otherwise `last_rendered_index`, which starts at 0, is only ever assigned a loaded slot, and is reset to 0 by `purge_file_backed_slots`; slot 0 is the grid, whose bind group is created eagerly and never cleared (the purge skips slots with no `source_path`, and the decode-failure path clears `source_path` but not `bind_group`). Both are sound. `mod.rs:658` `expect("just assigned")` is trivially sound but would disappear if the assignment were bound to a local first.

**E4 (low, unverified beyond reading). Integer arithmetic in `read_texture_rgba8` is `u32`.** `render_pass.rs:606` `width * 4`, `:650` `(width * height * 4) as usize`, `:652` `(row * bytes_per_row) as usize`. At the largest `max_texture_dimension_2d` I would expect (16384 square) these reach 1.07e9 and still fit, and `Renderer::export_limits` exists precisely so the engine can cap the canvas. I did not read the engine's capping logic, so I mark this unverified. Widening to `u64`/`usize` before the multiply costs nothing.

**E5 (low, confirmed). `Renderer::layout()` underflows on a renderer with fewer than two slots.** `mod.rs:511` `SlotLayout::new(self.texture_slots.len() - 2)`. `create_renderer` always pushes the grid and the cloud slot, so the invariant holds today; a `debug_assert!` or `saturating_sub` documents it for free.

**E6 (low, confirmed). `Renderer::render` trusts `params.sample_count`.** `mod.rs:604-610` calls `rebuild_msaa_resources` on any change, and an unsupported count is a wgpu validation error that kills the thread, exactly as `resolve_sample_count`'s own doc warns. The engine does funnel every path through `resolve_requested_sample_count` (`engine/mod.rs:900-915`), so nothing in the workspace can hit this. Since `Renderer` is `pub`, one line calling `resolve_sample_count` inside `render` (or narrowing `Renderer` to `pub(crate)`, see C6) closes it.

**E7 (low, confirmed). `create_mipmapped_texture` calls `ilog2()` on a decoded width.** `textures.rs:276` `width.max(height).ilog2() + 1` panics if both are zero. Whether the JXL loader can yield a zero-dimension image is outside my area, so I did not verify reachability.

I found no races, no unbounded growth, no leaks, and no lock-ordering problems in this area. The texture generation and `loading` flag protocol in `textures.rs` is carefully reasoned and its comments earn their place.

### F. Unit tests in `mod tests`

**F1 (high). `params.rs`'s mutation table is not table-driven and does not give the guarantee that is claimed for it.** `every_shader_parameter_triggers_dirty` (`params.rs:595-1039`) is a 445-line hand-written `vec![(name, SceneParams { field: value, ..base })]` of 58 entries. Nothing enumerates the fields of `SceneParams`, so a parameter added to the struct, to `from_config`, to `write_to_config`, to `Uniforms`, and to the WGSL, but not to `digest()` and not to this vector, compiles and passes. CLAUDE.md and `docs/rendering.md:60` both promise that "forgetting the dirty check is a test failure rather than a stale-frame bug"; what actually holds is "forgetting the dirty check *and remembering the table* is a test failure". Similarly, `digest()` (301-353) and `ParamsDigest` (373-423) are hand-written restatements of the same 48 field names.

The fix that makes the promise true: declare the quantized parameters once in a `macro_rules!` list of `(field, type)` and generate the `SceneParams` field block, the `ParamsDigest` field block, the `digest()` body, the `from_config`/`write_to_config` bodies, and the test's mutation list from it. That collapses `params.rs` from 1112 lines to roughly 350 and reduces "add a parameter" inside core to one edit. A cheaper half-measure that captures most of the value: keep `SceneParams` hand-written, but generate `ParamsDigest`, `digest()`, and the mutation table from one list.

**F2 (medium). Adding a parameter touches far more places than CLAUDE.md lists.** Traced `sun_reddening` across the workspace: `ui/main.slint:274` (property) and `:1027-1034` (row), `ui_callbacks.rs:486` and `:563` (both bridge directions, hand-written, not derived), `config.rs:272` (field) and `:412` (default), `params.rs:102` (field), `:183` (`from_config`), `:255` (`write_to_config`), `:342` (digest), `:413` (`ParamsDigest`), `:948` (the table), `uniforms.rs:97`, `render_pass.rs:204` and `:126`, `sphere.wgsl:65` (field plus a hand-maintained offset comment) and `:485` (the use), `tests/render_pipeline.rs:83` (the struct copy) and its initializer. That is 16 production sites plus 2 test sites, against the six the CLAUDE.md list names. `day_gamma` and `atmo_sunrise_glow` land in the same places. Either the doc should say sixteen, or the macro in F1 should make it fewer.

**F3 (medium). The 28 tests in `renderer/mod.rs` need no GPU and are fast; six can go.** Correcting the premise in my brief: none of them creates a device. They cover four pure functions. Redundancy:
- `aa_options_full_range`, `aa_options_no_8x_falls_back_to_highest`, `aa_options_skip_intermediates`, `aa_options_single_sample`, `aa_options_empty_input` (850-891) assert one rule against five inputs. Merge into one table test; keep `aa_options_respect_the_tier_cap` separate since it tests a second rule.
- `quantize_zero_gives_one_granularity`, `quantize_below_granularity`, `quantize_exact_boundary`, `quantize_just_above_boundary`, `quantize_double_boundary`, `quantize_mixed_dimensions`, `quantize_typical_display` (972-1005) are seven one-assert tests over a two-line function. One table test.
- `supported_request_is_honored`, `unsupported_request_falls_back_to_the_next_lower_option`, `adapter_without_8x_never_yields_8x`, and `a_request_below_everything_offered_takes_the_lowest_option` are partly subsumed by `every_resolution_is_actually_supported` (952-966), which sweeps 4 x 10 x 3 combinations. Keep the sweep and the two rule-specific ones (`tier_cap_applies_on_top_of_adapter_support`, `an_empty_or_fully_filtered_list_still_yields_a_valid_count`), drop the rest.
Net: 28 down to about 12, with no loss of coverage. No duplication with `tests/render_pipeline.rs` or `tests/golden.rs`, which test shaders and frames, not these helpers.

**F4 (low). `grid_texture.rs` has three tests asserting the same fact.** `contains_grid_lines` (250), `prime_meridian_is_major_yellow` (279), and `equator_is_major_yellow` (269) all assert that a major line is `MAJOR_YELLOW`; the first two both probe the prime meridian. Merge to one. `pixel_between_grid_lines_is_base_color` (305) ends by asserting alpha 255, which `all_pixels_are_opaque` (242) already covers. 7 tests down to 4.

**F5 (low). `gamma_slider_endpoints` pins constants.** `params.rs:1074-1077` asserts `0.2` and `3.0` literally, which are `GAMMA_MIN` and `GAMMA_MAX` (30-31). Changing the slider range breaks the test, which the project's own rule ("changing a preset or a default must not break a test") argues against. Use the constants, or assert the property (endpoints map to the ends, midpoint is 1.0, monotonic, round trip) that `gamma_slider_monotonic` and `gamma_roundtrip` already assert.

**F6 (low). `every_shader_parameter_triggers_dirty`'s `atmo_enabled` row only works by accident of the fixture.** `atmo_enabled` is not a `ParamsDigest` field; it reaches the digest only through `effective_rayleigh_intensity` and `effective_nightglow_intensity` (`params.rs:319`, `:322`). The row passes because `params()` (455-481) happens to set both intensities nonzero. If the fixture ever zeroed them, the row would silently stop testing anything. One assertion in `disabling_the_atmosphere_zeroes_both_shells` (1042) already covers the real behavior; a comment on the fixture or a nonzero assertion in the row would make the coupling explicit.

No slow tests, no sleeps, no real I/O, no large allocations, and no proptest in any `mod tests` in this area. The maintainer's "long runtime of unit tests" concern does not apply here; if unit tests are slow it is elsewhere in the crate.

### G. Best practices with a real effect

**G1 (low). `Option<Option<usize>>` in the overlay load loop.** `mod.rs:622-629` builds `[(cond).then(|| self.layout().moon()), ...]` and then matches `Some(Some(slot))`. `(cond).then(...).flatten()` or `.and_then(|_| self.layout().moon())` says the same thing in one level.

**G2 (low). `Renderer::slot_label` allocates a `String` per call.** `mod.rs:515-522`, called once per slot in `expected_textures` and once per decoded texture. Neither is hot, so this is only a note; returning `Cow<'static, str>` would remove the allocation in the common case where the answer is a `SLOT_LABELS` entry.

**G3. Things I checked and found fine.** `Stars::select` calls `visible_count` every frame (`render_pass.rs:482`); that is a binary search over the catalog (`assets/stars.rs:40-56`), not a scan. `create_mipmapped_texture` takes the pixel buffer by value and reuses it in place, with a comment saying why (`textures.rs:265-266`); that is the right call for a 128 MB 8K decode. `SceneParams` is `Copy` and 51 fields wide, so `*params` at `mod.rs:646` is a memcpy of a few hundred bytes once per rendered frame, which is nothing. No needless `collect`, no `String` parameters that want `&str`, no large enum variants.

## 4. Recommended refactors, by value over effort

| # | Refactor | Effort | Risk | Why first |
|---|---|---|---|---|
| 1 | Make `renderer::uniforms::Uniforms` `pub`, import it in `tests/render_pipeline.rs`, delete the 70-line copy and the `generate_uv_sphere` copy (E1, D2, D4) | 1 h | Low | Turns an eye-only invariant into a tested one and deletes ~150 lines of test duplication. Nothing else in the report protects the uniform layout. |
| 2 | Delete `Renderer::preview_texture`, fix the four stale comments, delete the ~10 restating and history comments named in A2 to A4 (C6, A1 to A4) | 1 h | None | Pure deletion, no behavior change, and it removes the one comment that is actively wrong about slot numbering. |
| 3 | Extract `draw_scene` so the preview and export paths share one call site (D1) | 1.5 h | Low | Removes 45 duplicated lines and the risk that a new draw lands in the preview and not in the wallpaper. Covered by the golden suite and by the engine's export cases. |
| 4 | `Pipelines` struct + one table-driven builder; `Renderer` holds it; `rebuild_msaa_resources` becomes one assignment (B1, B3 in part) | 4 h | Medium | The largest single win: ~495 lines to ~130, and it closes the "add a pipeline, forget the MSAA rebuild" hole. Medium risk because a wrong blend or depth state per row is a silent visual change; the golden suite is the safety net, so run it on the software adapter before and after. |
| 5 | Split `renderer/mod.rs` into `sizing.rs` and `slots.rs`; split `gpu_setup.rs` into `pipelines.rs` and `targets.rs` (B2, B1) | 2 h | Low | Mechanical moves once #4 has landed. 1147 and 934 become roughly 360, 230, 250, 130, 175. |
| 6 | Convert the six stale `#[allow]`s to `#[expect]` and adopt `#[expect]` as the convention in this area (C3) | 0.5 h | None | Cheap, and it stops the next stale allow from accumulating. Note the benefit only materializes under `cargo clippy --all-targets`, which CI does not run. |
| 7 | Generate `ParamsDigest`, `digest()`, and the mutation table from one macro list (F1) | 4 h | Medium | Makes the guarantee CLAUDE.md already claims actually true, and cuts `params.rs` from 1112 to roughly 350. Medium risk: a macro that changes the digest's field set changes dirty-checking behavior, so pair it with the existing `sub_threshold_change_compares_equal` and `at_threshold_change_compares_different` tests. |
| 8 | Merge the redundant unit tests: `mod.rs` 28 to ~12, `grid_texture.rs` 7 to 4 (F3, F4) | 1.5 h | Low | Not a speed win (these are microseconds), but it removes 150 lines of near-identical assertions. |
| 9 | `Overlays` to `Option<Overlay>`; `encode_and_submit` from 20 arguments to 9 (B4, C2) | 2 h | Low | Best done together with #3, which touches the same call sites. |
| 10 | `read_texture_rgba8` returns `Result`; `let _ = tx.send(...)` (E2) | 1.5 h | Low | Turns a GPU reset from an engine-thread panic into a failed export. Touches the export path in `engine/mod.rs`, so it reaches outside this area. |
| 11 | Shader deduplication: one `shell_vertex`, one shared nightglow fragment, `fs_sun_disk` calling `limb_hue` (D3); three section banners in `sphere.wgsl` (B7) | 2 h | Medium | Every edit here moves pixels, so it needs a golden run. Worth doing before the release only if #4 has already proved the golden loop is comfortable; otherwise defer. |
| 12 | Narrow the ~25 `pub` items that only `engine/mod.rs` uses to `pub(crate)` (C6) | 1 h | Low | Best done before the first public release, since it is a breaking change afterward. |

Two notes outside my file set, for whoever owns the docs: `docs/architecture.md:77` lists the file-backed slots as "day 1, night 2, moon 3" and omits the Milky Way at slot 4; and the "adding a shader parameter" checklist in CLAUDE.md, `docs/rendering.md:60`, and `params.rs:1-8` should become one copy, with the count corrected per F2.
