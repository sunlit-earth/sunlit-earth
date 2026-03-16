# Plan: Ice Preservation for Ocean Masking (2026-03-16)

## Summary

Add an ice-preservation stage to the ocean masking pipeline that detects contiguous ice/snow regions in polar ocean areas and reduces the ocean mask there so the original texture is preserved. Detection uses a two-tier geodesic dilation algorithm: strict luminance+saturation seeds identify high-confidence ice cores, connected-component labeling removes small noise regions, then the surviving seeds are dilated into a relaxed candidate mask to capture marginal ice (thin ice, melt zones, cyan-tinted edges). The result is Gaussian-blurred for smooth transitions and subtracted from the ocean mask before the fill color is applied. The feature is on by default when `--ocean-mask` is used and can be disabled with `--no-ocean-preserve-ice`.

## Stakes Classification

**Level**: Medium
**Rationale**: Modifies an existing module (`ocean_masking.py`) and its CLI/pipeline integration (`main.py`), adds one new dependency (`scipy`). The feature is additive and opt-out, so disabling it restores previous behavior. The detection algorithm is well-validated by empirical analysis of the actual Blue Marble textures, but edge cases (thin ice, JPEG artifacts) require careful threshold tuning. Incorrect detection silently alters output textures, making tests important.

## Context

**Research**: Empirical pixel analysis of the 12 monthly Blue Marble `world.topo.bathy` textures at 21600x10800, plus web research on classical ice detection in RGB satellite composites.
**Prior plan**: [`2026-03-16-ocean-masking-plan.md`](2026-03-16-ocean-masking-plan.md)
**Affected Areas**: `tools/texture-pipeline/` — modified `ocean_masking.py`, `main.py`, `pyproject.toml`, `conftest.py`, `test_ocean_masking.py`, `test_cli.py`, `test_integration.py`.

### Empirical Findings

Analysis of actual Blue Marble polar pixel distributions (full resolution, 21600x10800):

| Property | Ice (seed) | Open ocean | Edge case (thin ice) |
| --- | --- | --- | --- |
| Mean RGB | (238, 241, 243) | (11, 31, 67) | (163, 187, 214) |
| Luminance | >200 | <50 | 80--160 |
| HSV saturation | <0.15 | moderate | 0.15--0.3 |
| Prevalence in polar ocean | ~1% of ocean mask | ~75% | ~0.5% |

- 97.7% of ice-like bright pixels above 60N are on land (Greenland, Arctic islands), not in the ocean mask. Only ~295K pixels at full resolution are ocean ice that needs preserving.
- Connected-component analysis at full resolution: 15,715 raw ice regions, median size 3px (JPEG fragmentation). After morphological closing (3 iterations), regions merge; filtering at >50px keeps 82% of ice signal.
- Arctic seasonal variation: January ~25% ice vs July ~11% (all pixels above 66.5N including land). The algorithm adapts automatically.
- Tropical false-positive risk: 0.05% of 30N--30S pixels pass the ice threshold. The latitude gate eliminates these entirely.
- Blue Marble composites are cloud-free (MODIS cloud masking during compositing), so no cloud false positives.

## Success Criteria

- [ ] `uv sync` installs scipy alongside existing dependencies
- [ ] Running with `--ocean-mask` (no extra flags) preserves ice regions in polar textures by default
- [ ] Running with `--no-ocean-preserve-ice` disables ice detection and produces the same output as the previous implementation
- [ ] `--ocean-ice-luminance N` overrides the strict seed luminance threshold (default 200)
- [ ] `--ocean-ice-latitude N` overrides the polar latitude gate (default 60)
- [ ] Ice detection only processes polar rows (|latitude| >= threshold), not the full image
- [ ] Contiguous ice regions are preserved; isolated bright noise pixels are filtered out
- [ ] Ice edges have smooth transitions (Gaussian blur), not hard staircase boundaries
- [ ] All automated tests pass via `uv run pytest`
- [ ] `uv run ruff check src/ tests/` passes with no errors

## Implementation Steps

### Phase 1: Add scipy Dependency

#### Step 1.1: Add scipy to pyproject.toml

- **Files**: `tools/texture-pipeline/pyproject.toml`
- **Action**: Add `scipy>=1.14` to the `dependencies` list.
- **Verify**: `uv sync` succeeds. `uv run python -c "from scipy.ndimage import label; print('OK')"` exits without error.
- **Complexity**: Small

### Phase 2: Ice Detection Module -- Core Function

#### Step 2.1: Write tests for `detect_ice_regions` (RED)

- **Files**: `tools/texture-pipeline/tests/test_ocean_masking.py` (append), `tools/texture-pipeline/tests/conftest.py` (add fixtures)
- **Action**: Add test fixtures that create synthetic images with known ice-like and ocean-like pixel patterns. Write failing tests for `detect_ice_regions()`.
- **Fixtures** (in `conftest.py`):
  - `polar_ice_image_200x100`: 200x100 RGB image. Top 1/3 of rows (polar zone, latitude > 60 in a 200x100 equirectangular image) has a bright white 20x10 block at (10, 5) simulating a contiguous ice region (RGB 240, 240, 240), a cluster of 3 scattered bright pixels at (50, 2) simulating noise, and dark blue ocean everywhere else (RGB 10, 30, 65). Bottom 2/3 is uniform mid-grey (128, 128, 128) simulating non-polar land/ocean.
  - `polar_ice_ocean_mask_200x100`: 200x100 uint8 mask that is 255 (ocean) in the top 1/3 and 0 (land) in the bottom 2/3 to ensure ice detection is constrained to ocean.
- **Test cases**:
  - `test_detect_ice_shape`: Output is uint8 array with shape (H, W) matching input.
  - `test_detect_ice_dtype`: Output dtype is uint8.
  - `test_detect_ice_preserves_large_region`: The 20x10 bright block in the polar zone produces mask values > 0 in that region.
  - `test_detect_ice_filters_small_noise`: The 3 scattered pixels do NOT produce mask values > 0 (filtered by min region size).
  - `test_detect_ice_respects_latitude_gate`: Bright pixels in the non-polar bottom 2/3 produce zero mask values, even if they would otherwise pass the luminance/saturation thresholds.
  - `test_detect_ice_respects_ocean_mask`: Bright pixels on land (mask=0) produce zero ice mask values.
  - `test_detect_ice_zero_when_no_ice`: An all-dark-blue image produces an all-zero mask.
  - `test_detect_ice_soft_edges`: The ice mask has intermediate values (not just 0/255) at the boundary of the large ice region, due to Gaussian blur.
  - `test_detect_ice_disabled_returns_none`: When `preserve_ice=False` is passed, the function returns None (fast path for `--no-ocean-preserve-ice`).
- **Verify**: Tests exist and fail with `ImportError`.
- **Complexity**: Medium

#### Step 2.2: Implement `detect_ice_regions` (GREEN)

- **Files**: `tools/texture-pipeline/src/texture_pipeline/ocean_masking.py` (append)
- **Action**: Add the function:

  ```python
  def detect_ice_regions(
      image: Image.Image,
      ocean_mask: np.ndarray,
      latitude_threshold: float = 60.0,
      seed_luminance: int = 200,
      seed_saturation: float = 0.15,
      relax_luminance: int = 120,
      relax_saturation: float = 0.3,
      min_region_size: int = 50,
      dilation_iterations: int = 5,
      blur_radius: int = 3,
  ) -> np.ndarray:
  ```

  Implementation:
  1. Convert image to numpy float32 array.
  2. Compute luminance: `lum = 0.299*R + 0.587*G + 0.114*B`.
  3. Compute saturation: `sat = (max(R,G,B) - min(R,G,B)) / max(R,G,B)` (with zero-division guard).
  4. Create latitude mask from row indices: `row_latitudes = 90.0 - np.arange(H) * 180.0 / H`; `polar = np.abs(row_latitudes) >= latitude_threshold`; broadcast to (H, W).
  5. Create polar ocean mask: `domain = (ocean_mask > 128) & polar_rows`.
  6. Strict seed detection: `seeds = domain & (lum >= seed_luminance) & (sat <= seed_saturation)`.
  7. Connected-component labeling: `from scipy.ndimage import label`; `labeled, n = label(seeds)`; compute sizes via `np.bincount`; zero out regions smaller than `min_region_size`.
  8. Relaxed candidate mask: `relaxed = domain & (lum >= relax_luminance) & (sat <= relax_saturation)`.
  9. Geodesic dilation: `from scipy.ndimage import binary_dilation, generate_binary_structure`; `struct = generate_binary_structure(2, 2)` (8-connectivity); iteratively dilate filtered seeds constrained by relaxed mask for `dilation_iterations` iterations: `expanded = binary_dilation(seeds_filtered, structure=struct, iterations=dilation_iterations, mask=relaxed)`.
  10. Morphological closing: `from scipy.ndimage import binary_closing`; `closed = binary_closing(expanded, structure=struct, iterations=2)` to fill small internal gaps; re-intersect with `domain`.
  11. Gaussian blur: convert boolean mask to uint8*255, wrap in Pillow `Image("L")`, apply `GaussianBlur(radius=blur_radius)`, convert back to uint8 numpy array.
  12. Return the uint8 ice mask.
- **Verify**: All tests from Step 2.1 pass.
- **Complexity**: Medium

### Phase 3: Mask Reduction Helper

#### Step 3.1: Write tests for `reduce_mask_for_ice` (RED)

- **Files**: `tools/texture-pipeline/tests/test_ocean_masking.py` (append)
- **Action**: Write failing tests for a helper that subtracts the ice mask from the ocean mask.
- **Test cases**:
  - `test_reduce_mask_no_ice`: With an all-zero ice mask, the ocean mask is unchanged.
  - `test_reduce_mask_full_ice`: With ice mask = 255 everywhere, the result is all zeros.
  - `test_reduce_mask_partial_ice`: With ice mask = 128 at some pixels, those pixels in the result are approximately `ocean_mask - 128`, clamped to 0.
  - `test_reduce_mask_dtype`: Output dtype is uint8.
  - `test_reduce_mask_clamps_to_zero`: When ice > ocean_mask at a pixel, the result is 0 (not negative wrap-around).
- **Verify**: Tests exist and fail.
- **Complexity**: Small

#### Step 3.2: Implement `reduce_mask_for_ice` (GREEN)

- **Files**: `tools/texture-pipeline/src/texture_pipeline/ocean_masking.py` (append)
- **Action**: Add:

  ```python
  def reduce_mask_for_ice(
      ocean_mask: np.ndarray,
      ice_mask: np.ndarray,
  ) -> np.ndarray:
  ```

  Implementation: `np.clip(ocean_mask.astype(np.int16) - ice_mask.astype(np.int16), 0, 255).astype(np.uint8)`.
- **Verify**: All tests from Step 3.1 pass.
- **Complexity**: Small

### Phase 4: CLI Integration

#### Step 4.1: Write tests for new CLI flags (RED)

- **Files**: `tools/texture-pipeline/tests/test_cli.py` (append)
- **Action**: Write failing tests for the ice preservation CLI flags.
- **Test cases**:
  - `test_preserve_ice_default_true`: Without any ice flag, `run_pipeline` is called with `ocean_preserve_ice=True`.
  - `test_no_preserve_ice_flag`: With `--no-ocean-preserve-ice`, `run_pipeline` is called with `ocean_preserve_ice=False`.
  - `test_ice_luminance_default`: Without `--ocean-ice-luminance`, `run_pipeline` is called with `ocean_ice_luminance=200`.
  - `test_ice_luminance_custom`: With `--ocean-ice-luminance 180`, the value is passed through.
  - `test_ice_latitude_default`: Without `--ocean-ice-latitude`, `run_pipeline` is called with `ocean_ice_latitude=60.0`.
  - `test_ice_latitude_custom`: With `--ocean-ice-latitude 55`, the value is passed through.
  - `test_ice_latitude_validation_out_of_range`: With `--ocean-ice-latitude 100`, the CLI exits with an error.
  - `test_ice_luminance_validation_out_of_range`: With `--ocean-ice-luminance 300`, the CLI exits with an error.
  - `test_ice_flags_in_help`: `--help` output contains "ocean-preserve-ice", "ocean-ice-luminance", "ocean-ice-latitude".
- **Verify**: Tests exist and fail.
- **Complexity**: Small

#### Step 4.2: Add CLI flags to the `convert` command (GREEN)

- **Files**: `tools/texture-pipeline/src/texture_pipeline/main.py`
- **Action**: Add three new parameters to the `convert()` command:
  - `--ocean-preserve-ice / --no-ocean-preserve-ice`: `bool`, default `True`. Controls whether ice regions in polar ocean areas are detected and preserved.
  - `--ocean-ice-luminance`: `int`, default `200`. Strict seed luminance threshold (0--255). Add a validation callback.
  - `--ocean-ice-latitude`: `float`, default `60.0`. Minimum absolute latitude for polar region gate (0--90). Add a validation callback.

  Update `run_pipeline()` signature to accept: `ocean_preserve_ice: bool`, `ocean_ice_luminance: int`, `ocean_ice_latitude: float`. Pass them through from `convert()`.
- **Verify**: All tests from Step 4.1 pass. All existing CLI tests still pass.
- **Complexity**: Small

### Phase 5: Pipeline Integration

#### Step 5.1: Write integration tests for ice preservation (RED)

- **Files**: `tools/texture-pipeline/tests/test_integration.py` (append)
- **Action**: Write failing end-to-end tests.
- **Test cases**:
  - `test_end_to_end_ice_preserved_by_default`: Create a 128x64 image where the top 16 rows (polar zone) have a bright white block (240, 240, 240) on the right half (which is ocean per the eastern-half shapefile). Run with `--ocean-mask`. The bright block pixels in the output should NOT be replaced with fill color — they should retain high luminance (>150).
  - `test_end_to_end_ice_disabled`: Same input but with `--no-ocean-preserve-ice`. The bright block should be replaced with fill color (luminance <100).
  - `test_end_to_end_tropical_bright_not_preserved`: Create a 128x64 image where the middle rows (tropical zone) have a bright white block on the right half. Run with `--ocean-mask`. The bright block should be replaced with fill color (latitude gate excludes tropics).
- **Verify**: Tests exist and fail.
- **Complexity**: Medium

#### Step 5.2: Integrate ice detection into `run_pipeline` (GREEN)

- **Files**: `tools/texture-pipeline/src/texture_pipeline/main.py`
- **Action**: In `run_pipeline()`, after `mask = get_or_create_mask(...)` and before `img = apply_ocean_mask(...)`, add:

  ```python
  if ocean_preserve_ice:
      from texture_pipeline.ocean_masking import (
          detect_ice_regions,
          reduce_mask_for_ice,
      )
      ice = detect_ice_regions(
          img,
          mask,
          latitude_threshold=ocean_ice_latitude,
          seed_luminance=ocean_ice_luminance,
      )
      mask = reduce_mask_for_ice(mask, ice)
  ```

  Note: the `mask` variable here is a local copy per image (returned from the cache), so we must `.copy()` before modifying it to avoid corrupting the cache. Update the code to `mask = get_or_create_mask(...).copy()` when ice preservation is enabled.
- **Verify**: All tests from Step 5.1 pass. All existing tests still pass. Full `uv run pytest` is green.
- **Complexity**: Small

### Phase 6: Lint and Final Verification

#### Step 6.1: Lint check

- **Files**: All modified files.
- **Action**: Run `uv run ruff check src/ tests/` and fix any violations.
- **Verify**: Zero ruff errors.
- **Complexity**: Small

### Phase 7: Manual End-to-End Verification

#### Step 7.1: Test with real Blue Marble textures

- **Files**: N/A (manual verification)
- **Action**: Run the pipeline against the January and July Blue Marble textures with the real `ne_10m_ocean.shp`:

  ```bash
  cd tools/texture-pipeline
  uv run texture-pipeline convert \
      --input ../../tmp/texture-source/world.topo.bathy/ \
      --output ../../tmp/texture-target-masked/ \
      --ocean-mask ../../tmp/ocean/ne_10m_ocean.shp \
      --ocean-color "10,40,80" \
      --ocean-supersample 2 \
      --ocean-buffer 2 \
      --width 4096 --width 2048 \
      --quality 85 --effort 1
  ```

- **Manual test cases**:
  - January output: Arctic ice regions (north of Canada/Russia) are preserved with original texture pixels. Deep Arctic Ocean (if any without ice) is replaced with fill color.
  - July output: Smaller Arctic ice extent is preserved. More ocean area is filled.
  - Antarctic: Ice shelf edges are preserved in both months.
  - Coastlines: Smooth anti-aliased transitions at ice boundaries (no hard staircase).
  - Tropical and mid-latitude ocean: Uniformly filled with (10, 40, 80). No bright patches preserved.
  - Compare with `--no-ocean-preserve-ice`: ice regions are filled over, confirming the flag works.
- **Verify**: All manual checks pass.
- **Complexity**: Small

## Test Strategy

### Automated Tests

| Test Case | Type | Input | Expected Output |
| --- | --- | --- | --- |
| Ice detection output shape | Unit | 200x100 image + mask | `(100, 200)` ndarray |
| Ice detection output dtype | Unit | 200x100 image + mask | `dtype == np.uint8` |
| Large ice region preserved | Unit | Image with 20x10 bright block | Mask > 0 in that region |
| Small noise filtered | Unit | Image with 3 isolated bright pixels | Mask == 0 at those pixels |
| Latitude gate respected | Unit | Bright pixels at lat < 60 | Mask == 0 outside polar zone |
| Ocean mask respected | Unit | Bright pixels on land (mask=0) | Ice mask == 0 on land |
| All-dark image produces zero | Unit | Uniform dark blue image | All-zero mask |
| Soft edges via blur | Unit | Ice region boundary | Intermediate values present |
| Reduce mask no ice | Unit | Ice mask = 0 | Ocean mask unchanged |
| Reduce mask full ice | Unit | Ice mask = 255 | Result = 0 |
| Reduce mask partial ice | Unit | Ice mask = 128 | Result ≈ ocean - 128 |
| Reduce mask clamps to zero | Unit | Ice = 255, ocean = 100 | Result = 0 |
| CLI preserve ice default true | CLI | No flag | `ocean_preserve_ice=True` |
| CLI no-preserve-ice flag | CLI | `--no-ocean-preserve-ice` | `ocean_preserve_ice=False` |
| CLI ice luminance default | CLI | No flag | `ocean_ice_luminance=200` |
| CLI ice luminance custom | CLI | `--ocean-ice-luminance 180` | `ocean_ice_luminance=180` |
| CLI ice latitude default | CLI | No flag | `ocean_ice_latitude=60.0` |
| CLI ice latitude custom | CLI | `--ocean-ice-latitude 55` | `ocean_ice_latitude=55.0` |
| CLI ice latitude out of range | CLI | `--ocean-ice-latitude 100` | Error exit |
| CLI ice luminance out of range | CLI | `--ocean-ice-luminance 300` | Error exit |
| CLI ice flags in help | CLI | `--help` | Flags appear in output |
| E2E ice preserved by default | Integration | Bright polar block + shapefile | High luminance retained |
| E2E ice disabled | Integration | `--no-ocean-preserve-ice` | Bright block replaced |
| E2E tropical bright not preserved | Integration | Bright tropical block | Block replaced (latitude gate) |

### Manual Verification

- [ ] Run January + July Blue Marble textures with `--ocean-mask`; confirm Arctic/Antarctic ice regions preserved
- [ ] Compare output with and without `--no-ocean-preserve-ice`
- [ ] Verify ice edges have smooth gradient transitions
- [ ] Confirm tropical/mid-latitude ocean is uniformly filled

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| scipy adds ~15 MB to install and transitive deps | Larger venv, slower `uv sync` | Acceptable for an internal tool. scipy is well-maintained and provides fast C-implemented ndimage ops that have no pure-Python alternative. |
| JPEG compression fragments ice boundaries, creating many tiny connected components | Threshold too aggressive → ice lost; too lenient → noise preserved | Two-tier approach handles this: strict seeds identify cores, geodesic dilation recovers fragments adjacent to confirmed ice. Morphological closing bridges small gaps. Empirically validated at full resolution. |
| Threshold defaults may not work perfectly for all 12 monthly textures | Some months lose ice or gain false positives | Defaults calibrated from January (max ice) and July (min ice). Users can tune `--ocean-ice-luminance` for edge cases. The latitude gate eliminates non-polar false positives entirely. |
| Ice detection runs per-image (not cacheable like the ocean mask) | Slower pipeline when processing many files | At 21600x10800, ice detection processes only polar rows (~20% of image). scipy ndimage ops are C-implemented and fast. Expected overhead: <1 second per image. |
| Modifying the cached ocean mask in-place would corrupt the cache | Subsequent images get wrong mask | Plan specifies `.copy()` before modifying mask. Test verifies cache integrity across multiple images. |

## Rollback Strategy

Remove the `detect_ice_regions()` and `reduce_mask_for_ice()` functions from `ocean_masking.py`. Remove the ice-related CLI flags from `main.py` and the `run_pipeline` signature. Remove scipy from `pyproject.toml` and run `uv sync`. The ocean masking pipeline reverts to its previous behavior (no ice preservation). All changes are additive — existing function signatures use default parameters that produce identical behavior when ice preservation is disabled.

## File Inventory

```text
tools/texture-pipeline/
  pyproject.toml                              (modified: add scipy dep)
  src/texture_pipeline/
    ocean_masking.py                          (modified: add detect_ice_regions,
                                               reduce_mask_for_ice)
    main.py                                   (modified: new CLI flags,
                                               run_pipeline params,
                                               ice detection insertion)
  tests/
    conftest.py                               (modified: add synthetic polar
                                               ice image fixtures)
    test_ocean_masking.py                     (modified: add ice detection and
                                               mask reduction tests)
    test_cli.py                               (modified: tests for new CLI flags)
    test_integration.py                       (modified: end-to-end ice tests)
```

## Status

- [x] Plan approved
- [x] Implementation started
- [x] Implementation complete
