<!-- Reviewer notes for docs/reviews/2026-09-04-code-quality-review.md. Line numbers refer to commit 3046327. "The brief" is the shared review instruction; "the maintainer's rules" are the project's comment conventions. Runtime claims here are reasoned from the code; the measured figures are in test-timing.md. -->

# Review: the GPU-facing integration tests of `sunlit-core`

Area: `crates/sunlit-core/tests/render_pipeline.rs`, `golden.rs`, `shading.rs`, and the parts of
`tests/support/mod.rs` and `tests/common/mod.rs` these three use. Read in full. No file was modified
and no cargo command was run.

## 1. Summary

The three suites are in better shape than their line counts suggest, but the counts are misleading in
different directions in each file. `render_pipeline.rs` is 2506 lines for 21 tests, and 660 of those
lines are two items: one test with 61 hand-written assertions and a 173-line WGSL struct that is a
hand-maintained fourth copy of a field list production already owns. Fourteen of its tests are lean
and share a real helper layer; the repetition that does exist is the clear-color predicate, written
out five times. `golden.rs` is 31 percent comment lines, and most of that is measurements and history
that the maintainer's own rules put in `docs/` or a commit message. `shading.rs` has four tests that
are implied by other tests in the same file.

On runtime, the measurements settle the question: these three suites cost 7.6 s of a run that spends
161 s in `tests/engine.rs` and 66 s in `tests/soak.rs`. The maintainer's "long runtime" concern does
not point here, and there is almost nothing to reclaim. What the per-test numbers *do* show is an
artifact worth knowing about: in all three files the most expensive test is the alphabetically first
one, because it pays for the file's `LazyLock` device or engine initialization. So
`cloud_pipeline_renders_with_alpha` is not a 1.4-second test, and optimizing it would achieve nothing.
This report is therefore weighted toward structure, duplication and commentary.

Two things outside the "slow tests" brief are worth the maintainer's attention before a release: the
`metal` golden set has 4 of the 18 references the suite requires, and the roadmap entry describing
that gap is stale by a factor of two; and four `render_pipeline.rs` tests do not assert what their
names claim.

## 2. Metrics

| File | Total | Comment lines | Blank | Code | Tests | Blocks > 3 lines (keep / move / delete) | Fns > 80 lines |
|---|---|---|---|---|---|---|---|
| `render_pipeline.rs` | 2506 | 195 (7.8%) | 198 | 2113 | 21 | 5 (1 / 3 / 1), plus ~20 short blocks of which ~12 delete | 6 |
| `golden.rs` | 1037 | 319 (30.8%) | 64 | 654 | 20 | 31 (9 / 17 / 5) | 2 |
| `shading.rs` | 613 | 54 (8.8%) | 74 | 485 | 12 | 5 (2 / 1 / 2) | 0 |
| `support/mod.rs` | 374 | 118 (31.6%) | 31 | 225 | 0 | 12 (8 / 4 / 0) | 0 |
| `common/mod.rs` | 124 | 24 (19.4%) | 14 | 86 | 0 | 3 (3 / 0 / 0) | 0 |

Functions over 80 lines:

- `render_pipeline.rs:177` `create_render_context`, 165 lines, `#[allow(too_many_lines)]`. Justified: wgpu descriptors.
- `render_pipeline.rs:401` `render_frame`, 105 lines, no allow. Justified for the same reason.
- `render_pipeline.rs:912` `uniform_buffer_field_offsets_match_wgsl`, **464 lines**, allow. Not justified; see D1.
- `render_pipeline.rs:1419` `the_shader_and_the_cpu_agree_on_the_three_shared_rules`, 146 lines, allow.
- `render_pipeline.rs:1599` `the_panoramas_reconstruction_inverts_the_projection_it_sits_under`, 192 lines, allow.
- `render_pipeline.rs:2042` `cloud_pipeline_renders_with_alpha`, 194 lines, allow. ~150 of those duplicate `create_render_context` and `render_frame`; see D2.
- `golden.rs:259` `check_golden_in`, 99 lines. Reasonable: it is the whole comparison policy in one place.
- `golden.rs:948` `every_golden_case_is_distinguishable`, 90 lines. Reasonable.

`pub` items used only inside their own module: `support/mod.rs:64` `PANORAMA_FIXTURE_WIDTH`,
`support/mod.rs:76` `PANORAMA_BANDS`, `support/mod.rs:178` `SURFACE_FIXTURE_WIDTH`,
`support/mod.rs:266` `CLOUD_FIXTURE_WIDTH`. Checked against both including binaries (`engine.rs`,
`golden.rs`); no external reference. The file's `#![allow(dead_code)]` at line 3 is why nothing warns.
`PANORAMA_BANDS` carries a 9-line doc comment (68-76) for a value no test binary reads.

Dead across the whole area: `common/mod.rs:85` `pub static GPU: LazyLock<Mutex<GpuContext>>`. Both
binaries that include `common` define their own context static (`shading.rs:131`, `render_pipeline.rs:343`),
and `shading.rs:139`'s `GPU.lock()` resolves to its own local, not this one. Confirmed by grep.

## 3. Timing

**Measured, and the headline is that these three suites are not the problem.** From
`test-timing.md` (full workspace run, `--test-threads=1`, debug, Windows, whatever adapter the
tests picked):

| Suite | Sum | Tests | Over 1 s | Over 0.1 s |
|---|---|---|---|---|
| `golden.rs` | 4.00 s | 20 | 1 | 4 |
| `render_pipeline.rs` | 2.08 s | 21 | 1 | 2 |
| `shading.rs` | 1.55 s | 12 | 0 | 2 |
| **area total** | **7.6 s** | 53 | 2 | 8 |

For scale, the same run spends 161.6 s in `tests/engine.rs` (78 tests, 76 of them over a second) and
65.6 s in `tests/soak.rs`. The area under review is 3 percent of the suite's run time. **The
maintainer's "long runtime of unit tests" concern does not point here**, and I have reweighted the
rest of this report toward structure, redundancy, compile cost and readability accordingly. Where a
finding below is phrased as a runtime win, treat the win as tenths of a second and take the change for
the clarity instead.

**The per-test numbers are misleading, and how they are misleading is itself a finding.** In each of
the three files the single most expensive test is the **alphabetically first** one, which is the one
libtest runs first and therefore the one that pays for the file's `LazyLock` initialization:

| Test charged | Suite | Time | What it is actually paying for |
|---|---|---|---|
| `cloud_pipeline_renders_with_alpha` | `render_pipeline.rs` | 1.414 s | `RENDER_CTX` init at 343: adapter request, device creation, the 53 KB shader compile, the main render pipeline. 68% of the file's total, charged to a test whose own body is one 64x64 frame |
| `contact_sheet_of_every_preset` | `golden.rs` | 2.141 s | `ENGINE` init at 96: engine start on the forced software adapter, plus generating five fixture images (two 1024x512 surfaces, a 512x256 panorama, a 256x128 moon, a cloud PNG), plus its own 9 renders. 54% of the file's total |
| `diffuse_disabled_matches_pure_blend` | `shading.rs` | 0.897 s | `GPU` init at 131: device creation and the `blend.wgsl` compute pipeline. 58% of the file's total, charged to a test that dispatches 101 cases |

Verified: I sorted the test names in each file and the alphabetically first is `cloud_pipeline_renders_with_alpha`,
`contact_sheet_of_every_preset` and `diffuse_disabled_matches_pure_blend` respectively, matching the
three most expensive entries exactly. In `golden.rs` the *second* alphabetically
(`every_golden_case_is_distinguishable`, 0.750 s) is also the second most expensive, and that one is
its own work: 18 PNG decodes and 91 pairwise full-frame comparisons.

Two consequences the maintainer should know:

- Nobody should optimize these three tests. Their marginal cost is near zero. The eighteen real golden
  cases sum to about 1.1 s between them, roughly 60 ms each, and nineteen of the twenty-one
  `render_pipeline.rs` tests sum to 0.14 s, roughly 7 ms each.
- The one number in the area that *is* real per-test work and *is* avoidable is
  `shading.rs`'s `software_adapter_produces_correct_results` at **0.649 s**, 42 percent of that file,
  which is a second `wgpu::Device` being created (E2). It is last alphabetically, so this is not an
  init artifact.

Derived per-suite work, kept because it explains the numbers above:

| Suite | Devices created | Full-shader compiles (53 KB `sphere.wgsl`) | Render passes | Compute dispatch + blocking readback round trips | Pixels read back |
|---|---|---|---|---|---|
| `render_pipeline.rs` | 1 | 4 (1 needed) | 29 (28 at 128x128, 1 at 64x64) | 170 | ~460 K |
| `golden.rs` | 1 (engine-owned, forced software) | engine's own | 27 (18 at 512x256, 9 at 256x128) | n/a, exports through the engine | ~2.7 M |
| `shading.rs` | 2 | 0 (`blend.wgsl` only, 44 lines) | 0 | 14 | 0 |

Per-test detail for `render_pipeline.rs`, in render passes unless noted:

| Test | Line | Work |
|---|---|---|
| `sphere_renders_visible_pixels` | 648 | 1 @128 |
| `day_side_brighter_than_night_side` | 667 | 1 @128 |
| `single_texture_mode_ignores_night` | 702 | 1 @128 |
| `uniform_buffer_field_offsets_match_wgsl` | 912 | 1 shader compile (173-line WGSL) + 1 pipeline + 1 dispatch |
| `the_shader_and_the_cpu_agree_on_the_three_shared_rules` | 1419 | 1 full-shader compile + **144 dispatch/readback round trips** |
| `the_panoramas_reconstruction_inverts_the_projection_it_sits_under` | 1599 | 1 full-shader compile + 24 dispatch/readback round trips (62 workgroups each) |
| `fresnel_specular_zero_intensity_unchanged` | 1829 | 1 @128 |
| `fresnel_specular_brighter_at_grazing` | 1857 | 2 @128 |
| `fresnel_diffuse_shift_zero_is_noop` | 1904 | 1 @128 |
| `fresnel_diffuse_shift_brightens_grazing_water` | 1930 | 2 @128 |
| `fresnel_diffuse_shift_absent_on_land` | 1964 | 2 @128 |
| `fresnel_diffuse_shift_absent_at_night` | 1995 | 2 @128 |
| `cloud_pipeline_renders_with_alpha` | 2042 | 1 full-shader compile (**exact duplicate of the context's**) + 1 render pipeline + 1 @64 |
| `gamma_above_one_brightens` | 2242 | 2 @128 |
| `gamma_below_one_darkens` | 2267 | 2 @128 |
| `gamma_identity_unchanged` | 2292 | 2 @128 |
| `saturation_zero_produces_greyscale` | 2316 | 1 @128 |
| `saturation_identity_unchanged` | 2360 | 2 @128 |
| `saturation_above_one_increases_chroma` | 2379 | 2 @128 |
| `night_gamma_does_not_affect_single_texture_mode` | 2440 | 2 @128 |
| `day_and_night_corrections_independent` | 2464 | 2 @128 |

Every test in the file takes `RENDER_CTX.lock()` for its whole body, so the 21 run strictly serially
regardless of `--test-threads`. Same for `golden.rs` and its `ENGINE` mutex, and `shading.rs` and its
`GPU` mutex. That is the correct choice given the one-device rule; it just means the suite's wall time
is the sum of its parts, so per-test savings are not absorbed by parallelism.

Golden fixture sizes, for the cost model: moon 256x128, panorama bands 512x256, day and night surfaces
1024x512 each, clouds 512x256 PNG. All are generated at engine start, once, behind the `ENGINE`
`LazyLock`. Every case waits on `moon_texture` (the default `moon_brightness` is 1.0, config.rs:414),
which after the first case is one channel round trip; the first case pays a 20 ms poll loop.

Committed reference sizes: `warp/` 18 files, 1.60 MB; `lavapipe/` 18 files, 1.72 MB; `metal/` 4 files,
273 KB. Largest single reference is `panorama_at_a_wide_sky.png` at 206 KB (lavapipe).

## 4. Findings

### A. Commentary

**A1 (medium, `render_pipeline.rs:731, 1793, 1900, 2238, 2312, 2436`).** Six section banners carry a
plan-step identifier: `// Uniform buffer field offset test (Step 4.3)`, `// Fresnel specular tests
(Step 2.1)`, `// Fresnel diffuse shift tests (Step 2.2)`, `// Color correction: gamma tests (Step 3.2)`,
`// Color correction: saturation tests (Step 3.3)`, `// Color correction: independence tests (Step 3.4)`.
These are exactly the "ticket or issue IDs" the maintainer's rule excludes, and the step numbers no
longer point anywhere a reader can follow. **Delete the parenthetical, keep the banner.**

**A2 (medium, `golden.rs:64, 184, 777`).** Three references to project history inside source:
"that already happened during Phase 2, when the macOS probe's golden leg was green while comparing
nothing" (64-65), "which is precisely the failure phase B's goldens taught" (184-185), and "for the
reason decision 5 cares about" (777). The first two sentences before them carry the actual invariant
and should stay; the history belongs in the commit that introduced `GENERATED_ADAPTERS`. **Delete the
history clauses.** `golden.rs:275` ("That is what happened to the first pair of cloud references.") and
`golden.rs:945-946` ("which is what happened when the two panorama references were added") are the
same pattern.

**A3 (medium, `golden.rs`, ~11 blocks).** The measurement-carrying doc comments. Representative:

- `176-187` (`Window`): "Removing the Moon entirely comes to a mean channel difference of 0.22 against
  a tolerance of 2.00 ... over the window it is 3.12 with 1.71 percent of pixels outliers."
- `512-522` (`SUN_CASE_SKY_FOV`): "a reference that lost the Sun entirely would still pass at a mean of
  1.11 and half a percent of outliers."
- `541-553` (`golden_sun_grazing_the_limb`): "losing the warm shift entirely comes to a mean of 2.33
  against a tolerance of 2.00 and 1.04 percent outliers against a limit of 1.00."
- `583-593` (`RISING_SUN_WINDOW`): six numbers in five lines.
- `625-632` (`SUNRISE_BAND_WINDOW`): "deleting the lobe entirely comes to a mean of 0.21 ... over this
  strip it is 1.52 and 2.83 percent."

Classification: **move**. Each records a real experiment and none of them is verifiable from the code,
which is precisely the brief's definition of something that belongs in `docs/`. The *rule* each one
justifies is one sentence and should stay ("this case compares a window because the Moon is too small
to move a full frame past the tolerance"); the arithmetic should move to a table in `docs/testing.md`.
Doing this to the eleven measurement blocks removes roughly 90 lines from a 1037-line file without
losing a fact, because the facts land somewhere a reader can find all of them at once.

**A4 (low, `render_pipeline.rs:1406-1416, 1586-1596`).** Two 11-line doc comments explaining why their
test exists ("Three rules exist once in WGSL and once in `scene::sun_occlusion`, and every pairing
matters at the pixel...", "`milky_way_direction` has to be the exact inverse of `sky_lens_project`...").
The first sentence of each is the invariant and is worth keeping. The rest is design rationale.
**Move to `docs/rendering.md`, keep one sentence each.**

**A5 (low, `render_pipeline.rs:1600-1607`).** The `TOLERANCE` doc records "exact to 6.5e-7 on warp and
to 1.5e-4 on lavapipe ... three below the faults it exists to catch, which are 1.229 and 0.546." A
magic tolerance constant does deserve a justification, so **keep one line** ("an order of magnitude
above the worse adapter's transcendental noise") and move the four numbers.

**A6 (low, `render_pipeline.rs:13`).** `// Local copies of production types (integration tests can't
import from a bin crate)`. The stated reason is wrong: `sunlit-core` is a library crate
(`crates/sunlit-core/src/lib.rs:16` has `pub mod renderer`), and the actual obstacle is that
`renderer/mod.rs:13` declares `pub(crate) mod uniforms`. This matters because the wrong reason makes
the duplication look unavoidable when it is one visibility change away. See D1.

**A7 (delete list, `render_pipeline.rs`).** Comments that restate the next line, roughly twelve of them.
Concrete: `674` ("Sun pointing along +Z (toward the camera at lon=0)" above `flags: 1, // diffuse
enabled`), `706` ("Red day texture, green night texture" above two `create_solid_texture` calls named
`red` and `green`), `714`/`717` ("Check all non-clear pixels" / "Skip clear-color pixels"), `2046`
("Create a cloud pipeline using vs_cloud / fs_cloud entry points" above a pipeline whose entry points
are literally `vs_cloud` and `fs_cloud`), `2125` ("1x1 white cloud texture" above
`create_solid_texture(..., [255,255,255,255])`), `2301` ("Render twice with identical identity
settings" above two identical `render_frame` calls), `2330`, `2344`, `2395`. **Delete.**

**A8 (keep, for fairness).** These earn their place and should not be touched: `render_pipeline.rs:600-602`
and `613-615` and `619-621` (why individual uniforms are zeroed in a shared fixture, which is not
derivable), `render_pipeline.rs:908-909` ("One assertion per uniform field: splitting it would only
hide which field moved"), `render_pipeline.rs:1545-1546` (why the comparison is relative), `1757-1759`
and `1765-1769` (single-precision limits and why straight-line distance beats `acos`),
`golden.rs:104-113` (why the Moon slot points at a fixture rather than the LFS asset),
`golden.rs:313-318` (why a missing reference is written outside the tracked tree), `golden.rs:496-502`
and `564-573` (the aspect-ratio geometry that decides the two shared cameras),
`common/mod.rs:16-21` (Metal has no fallback adapter), and most of `support/mod.rs`, whose comments
describe non-obvious properties of generated fixtures that a reader cannot see in the pixel loops.

### B. Length and structure

**B1 (high, `render_pipeline.rs:734-1375`, 642 lines).** One WGSL string constant plus one test make up
26 percent of the file. Split seam, in order of value:

1. The 173-line `UNIFORM_READBACK_SHADER` (734-906) restates `sphere.wgsl`'s own `Uniforms` struct
   field for field. `sphere.wgsl` already declares `@group(0) @binding(0) var<uniform> uniforms`
   (line 74 there), and the two newer probes in this same file (`SHARED_RULE_PROBE:1383`,
   `ROUND_TRIP_PROBE:1572`) already demonstrate the pattern of concatenating the production shaders
   and putting the probe's own storage buffers on `@group(1)`. Rewriting this constant the same way
   deletes 71 lines of struct declaration and, more importantly, makes the test check production's
   WGSL instead of a copy of it. As written, reordering a field in `sphere.wgsl` and not in this
   constant leaves the test green.
2. The 61 hand-written `assert!` blocks (1070-1352) are followed at 1353-1374 by a 14-entry
   table-driven loop doing the same job in 22 lines. Converting the first 61 to the same table removes
   roughly 280 lines and makes the two halves consistent. The `#[allow(clippy::too_many_lines)]` at
   910 and its comment ("splitting it would only hide which field moved") argue against splitting the
   *test*, which is right, but the table does not split the test.

Together these two take the file from 2506 to about 2150 lines and the test from 464 to about 110.

**B2 (medium, `render_pipeline.rs:2042-2235`).** `cloud_pipeline_renders_with_alpha` is 194 lines, of
which about 150 are a second copy of `create_render_context` (the shader module, the pipeline layout,
the vertex layout) and `render_frame` (the color and depth textures, the pass, the readback). The
only new thing is the pipeline's `vs_cloud`/`fs_cloud` entry points, its
`ALPHA_BLENDING` target and `depth_write_enabled: false`. Seam: give `RenderContext` a
`cloud_pipeline` field built alongside `pipeline` in `create_render_context`, and give `render_frame`
a `&wgpu::RenderPipeline` parameter. The test then becomes about ten lines.

**B3 (low, `golden.rs`).** At 1037 lines it is over the threshold, but 319 of those are comments and
654 are eighteen 15-line test bodies plus a 100-line comparison policy. After A3 moves the measurement
blocks it lands around 940. No structural split is warranted; the file is one coherent thing.

### C. Consistency

**C1 (low).** `render_pipeline.rs` and `shading.rs` use `// ---...` banner dividers (26 and 16 of them
respectively); `golden.rs` uses none and organizes by doc comment on each item. Both work. Worth
picking one for new files.

**C2 (low, `render_pipeline.rs:546-549` vs everywhere else).** One cast-lint suppression uses
`#[expect(clippy::cast_precision_loss, reason = "...")]`; the other fourteen in the same file use bare
`#[allow(...)]` with no reason. The `#[expect]` form is better (it fails when the lint stops firing).
More usefully, the three sites at `1798-1803`, `2331-2336` and `2396-2401` each carry the identical
three-attribute stanza to convert `CLEAR_COLOR` into three `u8`s. A single
`const CLEAR_RGB: [u8; 3] = [5, 5, 12];` next to `CLEAR_COLOR` at 351 removes all nine attributes and
the arithmetic. See D3.

**C3 (low, `render_pipeline.rs:1641`).**
`f32::from(u8::try_from(step).expect("a small index")) * 30.0` for a loop counter `step` in `0..12`.
This is cast-lint avoidance written as a runtime fallible conversion. `support/mod.rs:117` does the
same thing with `u16`. Both would be clearer as an `#[allow(clippy::cast_precision_loss)]` on the
loop, which is what the rest of both files already do.

**C4 (low, `docs/testing.md:35`).** The doc says "engine, soak, and golden tests each hold a
`GPU_SERIAL` mutex for the lifetime of their engine." `golden.rs` has no `GPU_SERIAL`; it serializes
through `static ENGINE: LazyLock<Mutex<EngineHandle>>` (line 96), which achieves the same thing under
a different name. Grep confirms `GPU_SERIAL` exists only in `engine.rs:33` and `soak.rs:23`. Small doc
correction.

**C5 (low, `render_pipeline.rs:1776` and `golden.rs:1030`).** Two assertion messages have a run of
about twenty spaces in the middle, an artifact of a multi-line format string that lost its `\`
continuation: `"...viewport {viewport:?}:                          {sent:?} came back as..."` and
`"...compare equal (mean {mean:.2}, outliers {:.2}%): either the                  tolerance is too
loose..."`. Cosmetic, but these are the strings a failing run prints.

### D. Duplication

**D1 (high, `render_pipeline.rs:19-91`).** The `Uniforms` struct is declared four times in this
repository: `renderer/uniforms.rs` (production Rust), `sphere.wgsl:~76-146` (production WGSL),
`render_pipeline.rs:19-89` (test Rust copy) and `render_pipeline.rs:735-805` (test WGSL copy). The
`const _: () = assert!(size_of::<Uniforms>() == 544)` at `render_pipeline.rs:91` is a copy of the one
at `renderer/uniforms.rs:109` and, as written, proves only that the test's own copy is 544 bytes,
which is circular. Making `renderer::uniforms` `pub` (or adding a `#[doc(hidden)] pub use`) lets the
test import the production struct and deletes two of the four copies at once.

**D2 (medium, `render_pipeline.rs:261-265` vs `2047-2051`).** Byte-identical construction of the same
WGSL source string:

```rust
let wgsl_source = format!(
    "{}\n{}",
    include_str!("../shaders/blend.wgsl"),
    include_str!("../shaders/sphere.wgsl"),
);
```

`sphere.wgsl` is 53 KB and 1144 lines, so this is a second full naga parse and validate of it for no
gain. Storing the `wgpu::ShaderModule` from `create_render_context` in `RenderContext` and reusing it
removes the duplicate source construction and the second module creation.

**How much that saves is smaller than it looks, and the timing data says why.** The two probe tests at
`1422-1427` and `1612-1617` compile the same 53 KB source and cost 0.523 s and under 0.1 s
respectively, so module creation is not where the time goes: drivers specialize from the entry point,
and a compute probe reaches almost none of `sphere.wgsl`. The expensive part is
`create_render_pipeline` with a full vertex and fragment path, which reusing a module does not avoid.
So take D2 as a duplication fix worth about 40 lines, not a speed fix. The two probes could also share
one module if their `@group(1)` bindings were given distinct indices (both currently claim
`@binding(0)` and `@binding(1)`); *unverified* whether naga accepts two resource variables at the
same binding point in one module, and given the measurements it is not worth finding out.

**D3 (medium, `render_pipeline.rs`, five sites).** The "is this pixel the clear color" predicate,
`dr > 1 || dg > 1 || db > 1` against `CLEAR_COLOR` scaled to `u8`, is written five times:
`509-523` (`count_non_clear_pixels`), `1808-1811` (`avg_luminance_non_clear`), `2340-2343`
(inside `saturation_zero_produces_greyscale`), `2407-2410` (the `avg_chroma` closure), and
`718` uses a *different* rule for the same job (`px[0] <= 6 && px[1] <= 6 && px[2] <= 14`). One
`fn is_clear(px: &[u8]) -> bool` next to `CLEAR_COLOR` collapses all five and makes the odd one out
either consistent or deliberately different. This is the answer to "is sampling logic repeated": the
`render_frame`/`create_solid_texture`/`default_test_uniforms` helper layer is good and most tests use
it well, but the *pixel-sampling* layer was never factored.

**D4 (low, `render_pipeline.rs:535` and `1815`, `shading.rs:277`).** Three copies of
`0.2126*r + 0.7152*g + 0.0722*b`. Two are in the same file and should be one. The third is in a
different test binary, so it is only shareable by moving it into `common/mod.rs`, which is worth
doing since `common` exists for exactly this.

**D5 (low, `render_pipeline.rs:526-543`).** `avg_luminance_region` is used by exactly one test
(`day_side_brighter_than_night_side:685`); every other test uses `avg_luminance_non_clear`. Either
inline it or express it in terms of the shared predicate.

### E. Correctness and resilience

**E1 (high, `crates/sunlit-core/tests/golden/metal/`).** The directory holds 4 references
(`close_up`, `default`, `nightglow`, `rayleigh`) while `every_golden_case_is_distinguishable:962-981`
names 18 and asserts each exists (1003-1008), and `check_golden_in:312` panics on a missing reference.
`metal` is on `GENERATED_ADAPTERS` (line 70), so it does not skip. On macOS, 14 golden cases plus the
distinguishability case fail. **Confirmed by directory listing and by reading the two code paths.**
This is a known item, but `docs/roadmap.md:83` describes it as "seven of the suite's nine cases fail
there" and names three missing star cases: the suite has since grown to eighteen and the entry is
stale by roughly a factor of two. The suite is behaving as designed; the documentation is not. Fix the
roadmap entry before release, and decide explicitly whether macOS ships with a red golden suite.

**E2 (medium, and now the area's one real runtime item, `shading.rs:569`).** Measured at **0.649 s**,
42 percent of `shading.rs`'s entire 1.55 s, and unlike the other large per-test numbers in this area it
is not a `LazyLock` artifact: this test is last alphabetically, so the time is its own. It is a second
`wgpu::Device`. `software_adapter_produces_correct_results` calls
`create_blend_context(true)`, which creates a **second** `wgpu::Device` in a process where the
`GPU` `LazyLock` (line 131) may already hold one, and libtest runs the file's twelve tests on parallel
threads by default. CLAUDE.md's constraint is "One GPU device at a time in tests: shader tests share a
`LazyLock<Mutex<GpuContext>>` ... Per-test device creation crashes on Windows." This test is the one
exception in the area and it is not marked as one. It has not been observed failing (*unverified*
whether it ever has), and the second device is at least not per-test, but the risk is real and
undocumented. Either give the software context its own `LazyLock` so it is created at most once, or
put a comment at 569 stating why one extra device is safe here. To be clear, I am **not** recommending
deleting the test to reclaim the 0.65 s: a second independent implementation checking the same shader
is the only cross-adapter evidence the suite has, and 0.65 s is a fair price. The finding is that the
project's own one-device rule has an undocumented exception, not that the exception is too slow.

**E3 (medium, four tests in `render_pipeline.rs` do not assert what they are named).**

- `fresnel_specular_zero_intensity_unchanged:1829`. Name says "unchanged"; the body renders **once**
  and asserts `lum < 100.0`. There is no baseline and nothing is compared. The comment at 1848-1849
  even calls it a "Sanity check."
- `fresnel_diffuse_shift_zero_is_noop:1904`. Renders once, asserts `lum_base > 0.0`. That is
  `sphere_renders_visible_pixels` with a different name. The 1918-1921 comment describes the property
  the test would need two renders to check, and then does not check it.
- `fresnel_specular_brighter_at_grazing:1857`. Asserts `lum_grazing > lum_head_on * 0.8`, that is,
  grazing may be up to 20 percent *darker* than head-on and the test still passes. The message calls
  it "comparable to or brighter than", which is honest, but the name is not.
- `gamma_identity_unchanged:2292` and `saturation_identity_unchanged:2360`. Both render the same
  uniforms twice with `day_gamma`/`day_saturation` at the defaults `default_test_uniforms` already
  sets (1.0 each) and assert byte equality. Neither varies the parameter it is named for. They are two
  copies of a GPU-determinism check.

Recommendation: turn the first two into real two-render comparisons (they are one `render_frame` call
away from being what their names claim), tighten the third to `>=`, and merge the last two into one
`consecutive_renders_are_identical`.

**E4 (low, `shading.rs:451-463`).** `nyc_day_side_not_dominated_by_city_lights` asserts
`lum < night_lum` at `n_dot_l = 1.0`, where the shader's output equals `NYC_DAY` exactly (that is what
`nyc_fully_lit_matches_day_color:427` proves). So at that sample the assertion reduces to
`luminance(NYC_DAY) < luminance(NYC_NIGHT)`, which is 0.2456 < 0.4962 and true by arithmetic on the
two fixture constants regardless of what the shader does. This is the "test behavior, not constants"
rule being bent: the constants are test-local, so a preset change cannot break it, but the assertion
at that sample point tests nothing.

**E5 (informational, `golden.rs:239-251`).** `wait_for_slot_texture` has a 60-second deadline and
panics past it, which is the right shape. Worth noting it is called up to five times per case and
issues a fresh `memory_report()` channel round trip on every 20 ms poll; after the first case each
call returns on its first iteration. No change needed.

### F. Test-level findings (redundancy, cost, and what to remove)

**F1 (low on runtime, medium on clarity, `render_pipeline.rs:1516-1522`). The 144-probe cross product
is 18x larger than the property it checks.** Measured at **0.523 s**, so the ceiling on the saving is
about 0.45 s and this is a readability change with a small speed bonus, not a performance fix. The test loops 6 viewport heights x 8 sky FOVs x 3 reddening values and issues a
full write-buffer, dispatch, submit, staging-copy, `map_async`, blocking `device.poll`, receive cycle
for each of the 144 combinations. The three functions it measures are, in the shader source:

- `output_pixel_scale()` (`sphere.wgsl:122-124`) reads only `uniforms.viewport_size.y`.
- `sky_lens_edge_radius()` (`sphere.wgsl:135-137`) reads only `uniforms.sky_fov`.
- `limb_transmission_km(h)` via `limb_air_mass` (`sphere.wgsl:482-490`) reads only its `height_km`
  argument and `uniforms.sun_reddening`.

The three axes are provably independent, so the cross product measures each function 144 times when
6, 8 and 3 probes respectively would cover every value. Varying all three per probe covers all 17 axis
values in **8 dispatches**. That is an 18x cut in the test's GPU round trips with no loss of coverage,
and it is a five-line change to the loop. This is the single largest recoverable cost in the area.
The 9 heights stay as they are, since they are a buffer the shader walks in one dispatch and cost
nothing.

By contrast the sibling at `1741-1750` (4 FOVs x 3 pans x 2 viewports = 24 probes) is *not* reducible
the same way: `sky_lens_project` and its inverse read all three, so the cross product is the point.
Leave it.

**F2 (medium, `render_pipeline.rs`). Redundant tests, 4 of 21.**

| Test | Line | Verdict |
|---|---|---|
| `gamma_identity_unchanged` | 2292 | Merge with the next; neither varies its named parameter |
| `saturation_identity_unchanged` | 2360 | Merge into one `consecutive_renders_are_identical` |
| `gamma_below_one_darkens` | 2267 | Merge into `gamma_above_one_brightens`: identical setup, identical baseline render, one value and one comparison direction different. Saves 1 of 4 renders and 22 lines |
| `night_gamma_does_not_affect_single_texture_mode` | 2440 | Same claim as `single_texture_mode_ignores_night:702` (the night path is inert in single-texture mode), reached by a different route. Keep one; the pixel-equality one is the stronger |

Net: 21 tests to 18, and 28 render passes to about 24.

**F3 (medium, `shading.rs`). Redundant tests, 4 of 12.**

| Test | Line | Verdict |
|---|---|---|
| `land_monotonic` | 320 | Body is byte-identical to `ocean_monotonic:304` apart from the two constants and the message. Merge into one loop over both fixtures |
| `ocean_never_below_night` | 336 | Strictly implied by `per_channel_never_below_night:353`: luminance weights are all positive, so per-channel `>= night[ch]` gives luminance `>= luminance(night)`. Remove |
| `per_channel_never_below_night` | 353 | In turn a strict subset of `never_below_min_of_night_and_day:467`'s ocean leg, since `OCEAN_NIGHT[ch] < OCEAN_DAY[ch]` for all three channels so `min` is `night`. Remove |
| `never_below_min_of_night_and_day` | 467 | Covered by `never_below_min_across_color_range:494` for the ocean and NYC pairs; only the LAND pair is new, and `LAND_NIGHT` is one entry away from being in that test's `nights` list. Add it and remove this |
| `nyc_fully_lit_matches_day_color` | 428 | Same test as `fully_lit_matches_day_color:368` with different constants. Merge |

Net: 12 tests to 7 or 8. The runtime saving is small (these dispatches are microseconds of GPU work
each; the file's cost is two device creations), so this is a clarity change, not a speed one. Worth
saying plainly, because "delete slow tests" is the wrong framing here.

**F4 (medium, `golden.rs`). Two of eighteen cases are near-duplicates.**

Cases covering a visual feature nothing else covers: `default` (the baseline), `nightglow` (the
airglow shells, which is the only sun-dependent thing visible in single-texture mode per its own
comment at 388-393), `rayleigh` (day-side scattering at haze 1.0), `close_up` (near camera with tilt),
`large_crisp_stars` (sprite size with glow and contrast at zero), `bright_star_halos` (the halo at
maximum, which is the quad-edge artifact its comment at 473-477 names), `sun_over_the_night_side`
(unoccluded glare), `sun_grazing_the_limb` (the partial-occlusion branch, which its comment at 546-547
correctly claims nothing else reaches), `sun_rising_through_the_band` (the disk's own gradient with
the atmosphere off), `sunrise_band_close` (the forward lobe at the shipped 140-degree lens),
`moon_crescent` (phase orientation), `panorama_at_a_wide_sky` (above the 180-degree slider stop, three
distinct behaviors per 800-809), `cloud_terminator_close_up` (the cloud terminator offset against the
ground's).

Near-duplicates, same scene with one parameter nudged:

- `panorama_at_a_narrow_sky:780` versus `panorama_behind_the_stars:754`. Identical camera, identical
  every other field, `sky_fov` 60 against the 140 default. The wide case at 300 covers the clamp
  branch; the narrow one is an interpolation of code the other two already run, and `moon_crescent`
  already renders at `sky_fov: 60`. **Weakest case in the suite.** Its comment (773-778) justifies it
  as making the pair distinguishable, but distinguishability is a property the guard test checks, not
  a reason for a case to exist.
- `clouds_across_the_terminator:857` versus `cloud_terminator_close_up:872`. Same params, camera
  latitude and zoom differ. `cloud_terminator_close_up`'s own comment (861-870) says it is "the case a
  revert of either the shift or the width fails", which concedes that the wide one is the weaker of
  the two.
- `night_side_with_stars:433`, `large_crisp_stars:451` and `bright_star_halos:472` share one camera
  and one scene and differ only in star sliders. All three do hit distinct code, so keep all three,
  but note that this is where a fourth star case would be redundant.
- `sunrise_band:641` runs at `atmo_sunrise_glow: 3.0`, three times the shipped value, while
  `sunrise_band_close:691` runs the same phenomenon at defaults. Both have a claim; the close one is
  the stronger test and the boosted one exists because the tolerance cannot see the shipped value,
  which is an argument for a window, not for a second case.

Dropping `panorama_at_a_narrow_sky` and `clouds_across_the_terminator` removes 4 committed PNGs
(248 KB warp, 200 KB lavapipe) and about 120 ms. It is the maintainer's call whether the coverage is
worth it; I would drop the panorama one and keep the cloud one, on coverage grounds alone.

The timing data confirms the init-attribution pattern a third time here, and it is worth noting before
anyone reads those two numbers as case costs. The only two golden cases over 0.1 s are
`golden_cloud_terminator_close_up` (0.352 s) and `golden_panorama_at_a_narrow_sky` (0.156 s), and each
is the alphabetically first case of its kind: the former is the first to request blend mode and so
pays the `wait_for_slot_texture` polls for the cloud, day and night decodes (`check_golden_in:269-279`),
the latter is the first to request `milky_way_intensity > 0` and pays the panorama decode. Deleting
`golden_panorama_at_a_narrow_sky` would simply move its 0.156 s onto `golden_panorama_at_a_wide_sky`.

**F5 (answer to "is the tolerance logic in one place").** Yes, and cleanly. `MEAN_TOLERANCE`,
`OUTLIER_FRACTION` and `OUTLIER_THRESHOLD` at `golden.rs:53-57`, one `compare` function at `361-379`,
used by both `check_golden_in:344` and `every_golden_case_is_distinguishable:1021`. No second copy
anywhere in the area. This part of the design is good and should not be touched.

**F6 (answer to golden resolution and per-case cost).** Every case renders the full 512x256 frame
through the real engine; the four windowed cases (`sun_rising_through_the_band` 80x80, `sunrise_band`
72x256, `sunrise_band_close` 72x256, `moon_crescent` 96x96) crop *after* export at `check_golden_in:280`,
so the window saves comparison work, not render work. The contact sheet renders 9 more at 256x128
(`PRESETS` has 9 entries, `scene/camera.rs:54`). Total 27 full-scene software renders per run. The
engine is forced to the software adapter (`golden.rs:101`) even on a machine with a GPU, which is
correct for reference stability and is also why this suite is the slowest of the three by construction.

**F7 (overlap between the three suites and with `renderer/mod.rs`).** I read the test names in
`crates/sunlit-core/src/renderer/mod.rs:841-1147`: 28 tests covering MSAA option selection, resolution
quantization, texture-mode indexing, `SlotLayout` ordering and planet instance coordinates. All pure
CPU, none touching a shader. **No overlap with any of the three GPU suites.** Between the suites
themselves the overlap is small and mostly justified: `shading.rs` runs `blend_fragment` in isolation
through a compute harness while `render_pipeline.rs` runs it as part of the fragment shader, which is
a real difference in what a failure localizes to. The one real three-way overlap is the day/night
terminator: `shading.rs`'s sweeps, `render_pipeline.rs:667` `day_side_brighter_than_night_side`, and
every `golden.rs` case that shows a terminator. That is defensible layering rather than duplication,
since only the first localizes a fault to `blend_fragment`.

Against the CPU unit tests, `the_shader_and_the_cpu_agree_on_the_three_shared_rules` is *not*
duplicative of `scene/sun_occlusion.rs`'s 30-odd unit tests even though it calls the same three
functions: the unit tests pin the CPU behavior, and this one pins the WGSL against it. What is
excessive is only the sample count (F1), because an agreement failure is structural, not point-dependent.

### G. Best practices with a real effect

**G1 (low, `render_pipeline.rs:435-463`).** `render_frame` creates a fresh color texture and a fresh
depth texture on every call, 29 pairs over the run. At 128x128 this is 64 KB and 64 KB per pair, so
the allocation cost is negligible, but the texture creation itself is a driver round trip on a
software adapter. Since every caller but one uses the same 128x128, caching a single pair on
`RenderContext` and creating a new one only when the size changes is a two-line change. Low value;
list it only because it is free.

**G2 (low, `shading.rs:513-523`).** `never_below_min_across_color_range` builds a `meta` vector with
one `(day, night)` tuple per *case* (18036 entries, 432 KB) purely so error messages can name the
pair. The pair is recoverable from the index arithmetic (`i / 501` gives the combination). Not worth
changing on its own, but it is the one place in the area that allocates for a message that almost
never prints.

**G3 (low, `common/mod.rs:85`).** Delete the unused `GPU` static (see metrics). It is currently held
alive by `#[allow(dead_code)]` at line 84 and reads as though the two suites share a device with each
other, which they do not; each has its own.

## 5. Recommended refactors, by value over effort

Reordered after the timing data: nothing here is sold as a speed fix, because there is no speed to
recover. The ordering is by lines and confusion removed per hour.

| # | Change | Files | Effort | Risk | Value |
|---|---|---|---|---|---|
| 1 | Table-drive the remaining 61 assertions in `uniform_buffer_field_offsets_match_wgsl`, matching the loop the same test already has at 1353 (B1.2) | `render_pipeline.rs:1070-1352` | 1 h | Low, mechanical | **-280 lines** from the longest file in the area, and its two halves stop disagreeing |
| 2 | Update `docs/roadmap.md:83` to the real size of the `metal` gap, and decide whether macOS ships with a red golden suite (E1) | `docs/roadmap.md` | 0.5 h plus a decision | None to code | Removes a stale claim right before a release; the gap is 14 cases, not the 3 the entry names |
| 3 | Rebuild `UNIFORM_READBACK_SHADER` on top of `sphere.wgsl` with its probe on `@group(1)`, the way the two newer probes already do (B1.1) | `render_pipeline.rs:734-906` | 1.5 h | Low: the pattern exists twice in the same file | -71 lines, and the test starts checking production's WGSL instead of a copy that can drift silently |
| 4 | Fix the four mis-named tests: two need a second render, one needs `>=`, two merge into a determinism check (E3) | `render_pipeline.rs:1829, 1857, 1904, 2292, 2360` | 1 h | Low | Four tests start testing what they say; costs nothing in run time |
| 5 | Move the eleven measurement blocks from `golden.rs` into a table in `docs/testing.md`, keeping one sentence each (A3) | `golden.rs`, `docs/testing.md` | 1.5 h | None to behavior | -90 lines from a file that is 31% comments; the numbers land where they can be compared |
| 6 | Extract `is_clear`, `CLEAR_RGB` and one `luminance` into shared helpers (D3, D4, C2) | `render_pipeline.rs` five sites, `common/mod.rs` | 1 h | Very low | -60 lines, removes nine cast-lint attributes, makes the odd predicate at 718 visible |
| 7 | Delete the six `(Step X.Y)` markers, the three history clauses in `golden.rs`, and the ~12 restating comments (A1, A2, A7) | all three | 0.5 h | None | Brings the area in line with the maintainer's stated commentary rule |
| 8 | Refactor `cloud_pipeline_renders_with_alpha` onto the shared context and `render_frame` (B2, D2) | `render_pipeline.rs:2042-2235` | 1.5 h | Medium: the cloud pipeline's blend and depth-write differ, so `render_frame` gains a parameter | -150 lines. Note this will *not* speed the test up; its 1.414 s is `RENDER_CTX` init that moves to whichever test then runs first |
| 9 | Merge the five redundant `shading.rs` tests (F3) | `shading.rs:320, 336, 353, 428, 467` | 1 h | Low; verify `LAND_NIGHT` joins the `nights` list before deleting `never_below_min_of_night_and_day` | 12 tests to 7. Clarity only; these dispatches are microseconds |
| 10 | Merge the four redundant `render_pipeline.rs` tests (F2) | `render_pipeline.rs:2267, 2292, 2360, 2440` | 0.5 h | Low | 21 tests to 18 |
| 11 | Collapse the 6x8x3 probe grid to 8 probes covering all 17 axis values (F1) | `render_pipeline.rs:1516-1522` | 0.5 h | Very low: each shader function reads one uniform, verified in `sphere.wgsl` | -0.45 s and a loop a reader can follow. Do it for the second reason |
| 12 | Document why `shading.rs:569` may create a second device, or give it a `LazyLock` (E2) | `shading.rs:569` | 0.5 h | Low | Closes the one place in the area that bends the one-device rule silently. Do not delete the test for its 0.649 s |
| 13 | Drop `pub` from the four support consts and delete `common::GPU` (metrics, G3) | `support/mod.rs`, `common/mod.rs` | 0.25 h | None | Removes items that exist only because `allow(dead_code)` hides them |
| 14 | Fix the two mangled assertion messages (C5) | `render_pipeline.rs:1776`, `golden.rs:1030` | 0.1 h | None | These are the strings a failing run prints |

Items 1, 3, 5 and 8 are the whole of the line-count answer and take five and a half hours between
them; they remove about 590 lines. Items 2 and 4 are the two correctness items and should go first if
only two things get done. Nothing in this list changes what any suite covers, except items 9 and 10,
which remove assertions implied by assertions that stay, and F4's optional golden drop, which is a
judgment call for the maintainer.
