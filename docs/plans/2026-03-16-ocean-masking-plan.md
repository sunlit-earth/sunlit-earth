# Plan: Ocean Masking for Texture Pipeline (2026-03-16)

## Summary

Add an ocean masking stage to the texture pipeline that rasterizes the Natural Earth 10m ocean shapefile into a pixel mask using rasterio and geopandas, then replaces ocean pixels with a configurable uniform fill color (default RGB 10, 40, 80). The masking runs once per source image at full resolution before the existing downscale/sharpen/encode loop, so all output width variants inherit the masked result. Anti-aliased coastlines are achieved via supersampled rasterization with Lanczos downscale. A configurable coastal buffer/transition zone blends the fill color into the original texture at the land-ocean boundary. The feature is opt-in via `--ocean-mask` CLI flag and requires no changes to existing pipeline behavior when the flag is absent.

## Stakes Classification

**Level**: Medium
**Rationale**: This adds a new processing stage and two new required dependencies (rasterio, geopandas) to an existing, working pipeline. The new module is self-contained and the masking step is entirely opt-in, so there is no risk of breaking existing functionality when the flag is omitted. However, the geospatial dependencies are substantial (GDAL, numpy, shapely, pyproj transitively), and the rasterization must produce geometrically correct masks at resolutions up to 21600x10800. Incorrect masks would silently corrupt output textures. The memory budget (~3.5 GB peak at 2x supersampling) requires careful implementation.

## Context

**Research**: [`docs/plans/2026-03-16-ocean-masking-research.md`](2026-03-16-ocean-masking-research.md)
**Affected Areas**: `tools/texture-pipeline/` -- new `ocean_masking.py` module, modified `main.py` (CLI flags and pipeline insertion), modified `processing.py` (new function), modified `pyproject.toml` (new dependencies), new tests.

## Success Criteria

- [ ] `uv sync` installs rasterio and geopandas alongside existing dependencies on Python 3.14
- [ ] Running the pipeline without `--ocean-mask` behaves identically to the current pipeline
- [ ] Running with `--ocean-mask path/to/ne_10m_ocean.shp` replaces ocean pixels with the default fill color
- [ ] `--ocean-color "R,G,B"` overrides the fill color
- [ ] `--ocean-supersample N` controls anti-aliasing quality (default 2, minimum 1)
- [ ] `--ocean-buffer N` applies a coastal transition zone of N pixels width (default 0, meaning disabled)
- [ ] The ocean mask is computed once per unique (width, height) pair and reused across all source images of that resolution within a single run
- [ ] At 2x supersampling, coastline edges have smooth anti-aliased transitions (not binary staircase)
- [ ] With `--ocean-buffer` > 0, the boundary between fill color and land texture blends smoothly over the specified pixel width
- [ ] All automated tests pass via `uv run pytest`
- [ ] `uv run ruff check src/ tests/` passes with no errors

## Implementation Steps

### Phase 1: Add Dependencies

#### Step 1.1: Add rasterio and geopandas to pyproject.toml

- **Files**: `tools/texture-pipeline/pyproject.toml`
- **Action**: Add `rasterio>=1.4` and `geopandas>=1.0` to the `dependencies` list. These are required dependencies, not optional. Also add `numpy>=2.0` as an explicit dependency since ocean masking uses NumPy arrays directly for mask manipulation.
- **Verify**: `uv sync` succeeds. `uv run python -c "import rasterio; import geopandas; import numpy; print('OK')"` exits without error.
- **Complexity**: Small

### Phase 2: Ocean Masking Module -- Core Functions

#### Step 2.1: Write tests for `rasterize_ocean_mask` (RED)

- **Files**: `tools/texture-pipeline/tests/test_ocean_masking.py` (new), `tools/texture-pipeline/tests/conftest.py` (modified to add shapefile fixtures)
- **Action**: Create test fixtures that generate minimal synthetic shapefiles using Shapely and fiona in `conftest.py`. Create a fixture that writes a simple rectangular polygon (covering the "western half" of the globe, i.e., longitude -180 to 0) as a shapefile in a temp directory. Write failing tests for `rasterize_ocean_mask()`.
- **Test cases**:
  - `test_rasterize_ocean_mask_shape`: Output array shape is `(H, W)` matching the requested dimensions.
  - `test_rasterize_ocean_mask_dtype`: Output array dtype is `uint8`.
  - `test_rasterize_ocean_mask_binary_at_supersample_1`: With `supersample=1`, all values are exactly 0 or 255 (no intermediate values).
  - `test_rasterize_ocean_mask_antialiased_at_supersample_2`: With `supersample=2`, at least some values are between 1 and 254 (fractional coverage at edges).
  - `test_rasterize_ocean_mask_coverage`: With the western-half rectangle fixture at `supersample=1`, approximately half the pixels are 255 (ocean) and half are 0 (land). Allow a small tolerance for edge pixels.
  - `test_rasterize_ocean_mask_full_ocean`: With a shapefile containing a polygon covering the entire globe (-180 to 180, -90 to 90), all mask pixels are 255.
  - `test_rasterize_ocean_mask_empty`: With a shapefile containing a tiny polygon far from the test region (or a very small image), the mask is predominantly 0.
- **Verify**: Tests exist and fail (no implementation yet). `uv run pytest tests/test_ocean_masking.py` shows `ImportError` or `ModuleNotFoundError`.
- **Complexity**: Medium

#### Step 2.2: Implement `rasterize_ocean_mask` (GREEN)

- **Files**: `tools/texture-pipeline/src/texture_pipeline/ocean_masking.py` (new)
- **Action**: Create the `ocean_masking.py` module with:

  ```python
  def rasterize_ocean_mask(
      shapefile_path: Path,
      width: int,
      height: int,
      supersample: int = 2,
  ) -> np.ndarray:
  ```

  Implementation:
  1. Set `os.environ["GDAL_CACHEMAX"] = "512"` for single-pass rasterization.
  2. Read the shapefile with `gpd.read_file()` and ensure CRS is EPSG:4326 via `to_crs()`.
  3. Compute the Affine transform with `from_bounds(-180, -90, 180, 90, W*supersample, H*supersample)`.
  4. Call `rasterize()` with `shapes=((geom, 255) for geom in gdf.geometry)`, `out_shape=(H*supersample, W*supersample)`, `fill=0`, `dtype="uint8"`, `all_touched=False`.
  5. If `supersample > 1`, convert to a Pillow `Image` (mode `"L"`), resize to `(W, H)` with `LANCZOS`, and convert back to a NumPy array.
  6. Return the uint8 mask array (0 = land, 255 = ocean, intermediate values at anti-aliased edges).
- **Verify**: All tests from Step 2.1 pass. `uv run pytest tests/test_ocean_masking.py` is green.
- **Complexity**: Medium

#### Step 2.3: Write tests for `apply_ocean_mask` (RED)

- **Files**: `tools/texture-pipeline/tests/test_ocean_masking.py` (append)
- **Action**: Write failing tests for `apply_ocean_mask()`. These tests use synthetic NumPy arrays and PIL Images directly, not shapefiles.
- **Test cases**:
  - `test_apply_ocean_mask_all_land`: With a mask of all zeros, the output image is pixel-identical to the input.
  - `test_apply_ocean_mask_all_ocean`: With a mask of all 255s, every pixel in the output equals the fill color.
  - `test_apply_ocean_mask_blend_128`: With a uniform mask value of 128, the output pixel values are approximately the midpoint between the source pixel and the fill color (within +/-1 for rounding).
  - `test_apply_ocean_mask_blend_gradient`: With a mask that varies from 0 to 255 across columns, the output transitions smoothly from source to fill color.
  - `test_apply_ocean_mask_custom_color`: With a non-default fill color (e.g., 0, 0, 255), the ocean region matches the custom color.
  - `test_apply_ocean_mask_output_is_pil_image`: The return type is a `PIL.Image.Image` in RGB mode.
  - `test_apply_ocean_mask_preserves_dimensions`: Output image has the same dimensions as the input.
- **Verify**: Tests exist and fail. `uv run pytest tests/test_ocean_masking.py -k "apply"` shows failures.
- **Complexity**: Small

#### Step 2.4: Implement `apply_ocean_mask` (GREEN)

- **Files**: `tools/texture-pipeline/src/texture_pipeline/ocean_masking.py` (append)
- **Action**: Add the function:

  ```python
  def apply_ocean_mask(
      image: Image.Image,
      mask: np.ndarray,
      color: tuple[int, int, int] = (10, 40, 80),
  ) -> Image.Image:
  ```

  Implementation:
  1. Convert the PIL image to a NumPy float32 array: `src = np.array(image, dtype=np.float32)`.
  2. Compute alpha: `alpha = mask.astype(np.float32) / 255.0`.
  3. Create the fill array: `fill = np.array(color, dtype=np.float32)`.
  4. Blend: `result = src * (1.0 - alpha[..., None]) + fill * alpha[..., None]`.
  5. Clip and convert: `return Image.fromarray(result.clip(0, 255).astype(np.uint8), mode="RGB")`.
- **Verify**: All tests from Step 2.3 pass. `uv run pytest tests/test_ocean_masking.py -k "apply"` is green.
- **Complexity**: Small

### Phase 3: Coastal Buffer/Transition Zone

#### Step 3.1: Write tests for `apply_coastal_buffer` (RED)

- **Files**: `tools/texture-pipeline/tests/test_ocean_masking.py` (append)
- **Action**: Write failing tests for a function that modifies a binary/anti-aliased ocean mask to add a transition zone at the coastline. The transition zone is a configurable pixel-width region where the mask value ramps from 255 (full ocean) down to 0 (full land) over the specified distance from the coastline edge.
- **Test cases**:
  - `test_coastal_buffer_zero_is_identity`: With `buffer_pixels=0`, the output mask is identical to the input mask.
  - `test_coastal_buffer_expands_transition`: With a binary mask (sharp edge at column 50 in a 100x50 image) and `buffer_pixels=5`, pixels in columns 50--54 on the land side have intermediate values (not 0, not 255), while pixels far from the edge remain unchanged.
  - `test_coastal_buffer_preserves_deep_ocean`: Pixels that are far from any coastline (fully interior to the ocean) remain 255 after applying the buffer.
  - `test_coastal_buffer_preserves_deep_land`: Pixels that are far from any coastline on the land side remain 0 after applying the buffer.
  - `test_coastal_buffer_monotonic_decay`: Along a transect perpendicular to the coastline, the mask values are monotonically non-decreasing from land into ocean.
  - `test_coastal_buffer_dtype`: Output dtype is `uint8`.
- **Verify**: Tests exist and fail.
- **Complexity**: Small

#### Step 3.2: Implement `apply_coastal_buffer` (GREEN)

- **Files**: `tools/texture-pipeline/src/texture_pipeline/ocean_masking.py` (append)
- **Action**: Add the function:

  ```python
  def apply_coastal_buffer(
      mask: np.ndarray,
      buffer_pixels: int,
  ) -> np.ndarray:
  ```

  Implementation approach -- use distance-based blending:
  1. If `buffer_pixels == 0`, return the mask unchanged.
  2. Compute the land region as a binary array: `land = mask < 128`.
  3. Use `scipy.ndimage.distance_transform_edt` on the inverted land mask (i.e., distance from each land pixel to the nearest ocean pixel). Alternatively, to avoid adding scipy as a dependency, use a Gaussian blur approach: apply `PIL.ImageFilter.GaussianBlur` with `radius=buffer_pixels` to the mask image, then composite the result so that only the land-side boundary region is affected.

  Preferred approach (avoids scipy): Use morphological dilation via Pillow's `ImageFilter.MaxFilter` iterated or a single Gaussian blur of the mask:
  1. Convert mask to a Pillow `Image` (mode `"L"`).
  2. Apply `GaussianBlur(radius=buffer_pixels)` to get a blurred version.
  3. Take the element-wise maximum of the blurred mask and... no, the goal is to erode the ocean boundary into land. The correct approach:
     - The blurred mask naturally creates a gradient at the boundary. Pixels that were 0 (land) near the coast get small positive values from the blur. Pixels that were 255 (ocean) near the coast get values slightly below 255.
     - Use `np.where(original_mask == 255, original_mask, blurred_mask)` -- this preserves fully-ocean pixels and only applies the gradient on the land side.
     - Actually, for a clean result: take the Gaussian-blurred version of the mask as the new mask. This extends the transition zone into land by `buffer_pixels` while softening the ocean edge by the same amount. To keep deep-ocean pixels fully at 255, composite: `np.maximum(original_mask, blurred)` does not work because blur reduces 255 values near edges. Instead: `np.where(original_mask >= 255, 255, blurred)` preserves deep ocean, applies gradient at the boundary.

  Simplest correct implementation:
  1. Convert mask to Pillow `Image` mode `"L"`.
  2. Apply `GaussianBlur(radius=buffer_pixels)`.
  3. Convert back to NumPy uint8.
  4. Composite: for any pixel where the original mask was 255 (fully ocean), keep 255. For any pixel where the original mask was 0 (land) and the blurred value is > 0, use the blurred value. This is `np.where(mask == 255, 255, blurred)`.
  5. Return the result as uint8.
- **Verify**: All tests from Step 3.1 pass. `uv run pytest tests/test_ocean_masking.py -k "coastal"` is green.
- **Complexity**: Medium

### Phase 4: In-Memory Mask Cache

#### Step 4.1: Write tests for mask caching (RED)

- **Files**: `tools/texture-pipeline/tests/test_ocean_masking.py` (append)
- **Action**: Write failing tests for a cache function that returns a previously computed mask when called with the same dimensions, or computes a new one when dimensions differ.
- **Test cases**:
  - `test_mask_cache_returns_same_object`: Calling `get_or_create_mask()` twice with the same `(shapefile, width, height, supersample, buffer_pixels)` returns the exact same array object (identity check with `is`).
  - `test_mask_cache_recomputes_for_different_size`: Calling with `(100, 50)` then `(200, 100)` returns arrays with different shapes.
  - `test_mask_cache_clear`: After calling `clear_mask_cache()`, the next call recomputes the mask (does not return the previous object).
- **Verify**: Tests exist and fail.
- **Complexity**: Small

#### Step 4.2: Implement mask cache (GREEN)

- **Files**: `tools/texture-pipeline/src/texture_pipeline/ocean_masking.py` (append)
- **Action**: Implement a simple module-level dictionary cache:

  ```python
  _mask_cache: dict[tuple[str, int, int, int, int], np.ndarray] = {}

  def get_or_create_mask(
      shapefile_path: Path,
      width: int,
      height: int,
      supersample: int = 2,
      buffer_pixels: int = 0,
  ) -> np.ndarray:
  ```

  Implementation:
  1. Build a cache key from `(str(shapefile_path), width, height, supersample, buffer_pixels)`.
  2. If the key exists in `_mask_cache`, return the cached array.
  3. Otherwise, call `rasterize_ocean_mask()`, then if `buffer_pixels > 0` call `apply_coastal_buffer()`, store the result in `_mask_cache`, and return it.

  Also implement `clear_mask_cache()` to reset the dict (useful for testing and end-of-run cleanup).
- **Verify**: All tests from Step 4.1 pass.
- **Complexity**: Small

### Phase 5: CLI Integration

#### Step 5.1: Write tests for new CLI flags (RED)

- **Files**: `tools/texture-pipeline/tests/test_cli.py` (append)
- **Action**: Write failing tests for the new ocean masking CLI flags using `typer.testing.CliRunner` with a mocked `run_pipeline`.
- **Test cases**:
  - `test_ocean_mask_flag_default_none`: Without `--ocean-mask`, `run_pipeline` is called with `ocean_shapefile=None`.
  - `test_ocean_mask_flag_passes_path`: With `--ocean-mask path/to/ocean.shp`, `run_pipeline` is called with `ocean_shapefile=Path("path/to/ocean.shp")`.
  - `test_ocean_color_flag_default`: Without `--ocean-color`, `run_pipeline` is called with `ocean_color=(10, 40, 80)`.
  - `test_ocean_color_flag_custom`: With `--ocean-color "0,100,200"`, `run_pipeline` is called with `ocean_color=(0, 100, 200)`.
  - `test_ocean_color_flag_invalid_format`: With `--ocean-color "not,a,color"`, the CLI exits with an error.
  - `test_ocean_color_flag_out_of_range`: With `--ocean-color "256,0,0"`, the CLI exits with an error.
  - `test_ocean_supersample_flag_default`: Without `--ocean-supersample`, `run_pipeline` is called with `ocean_supersample=2`.
  - `test_ocean_supersample_flag_custom`: With `--ocean-supersample 4`, `run_pipeline` is called with `ocean_supersample=4`.
  - `test_ocean_supersample_flag_too_low`: With `--ocean-supersample 0`, the CLI exits with an error.
  - `test_ocean_buffer_flag_default`: Without `--ocean-buffer`, `run_pipeline` is called with `ocean_buffer=0`.
  - `test_ocean_buffer_flag_custom`: With `--ocean-buffer 3`, `run_pipeline` is called with `ocean_buffer=3`.
  - `test_ocean_buffer_flag_negative`: With `--ocean-buffer -1`, the CLI exits with an error.
  - `test_ocean_flags_in_help`: `--help` output contains "ocean-mask", "ocean-color", "ocean-supersample", "ocean-buffer".
- **Verify**: Tests exist and fail. `uv run pytest tests/test_cli.py -k "ocean"` shows failures.
- **Complexity**: Small

#### Step 5.2: Add CLI flags to the `convert` command (GREEN)

- **Files**: `tools/texture-pipeline/src/texture_pipeline/main.py`
- **Action**: Add four new parameters to the `convert()` command function:
  - `--ocean-mask`: `Path | None`, default `None`. When provided, enables ocean masking. Use `typer.Option(exists=True, dir_okay=False)` to validate the file exists.
  - `--ocean-color`: `str`, default `"10,40,80"`. Parse as comma-separated RGB integers. Add a validation callback `_validate_ocean_color` that splits on commas, converts to ints, validates each is 0--255, and returns the tuple.
  - `--ocean-supersample`: `int`, default `2`. Add a validation callback `_validate_ocean_supersample` that ensures the value is >= 1.
  - `--ocean-buffer`: `int`, default `0`. Add a validation callback `_validate_ocean_buffer` that ensures the value is >= 0.

  Update the `run_pipeline()` function signature to accept the four new keyword arguments: `ocean_shapefile: Path | None`, `ocean_color: tuple[int, int, int]`, `ocean_supersample: int`, `ocean_buffer: int`. Pass them through from `convert()`.
- **Verify**: All tests from Step 5.1 pass. All existing CLI tests still pass.
- **Complexity**: Small

### Phase 6: Pipeline Integration

#### Step 6.1: Write tests for pipeline with ocean masking (RED)

- **Files**: `tools/texture-pipeline/tests/test_integration.py` (append)
- **Action**: Write failing integration tests that exercise the full pipeline with ocean masking enabled. Use the synthetic shapefile fixtures from `conftest.py`.
- **Test cases**:
  - `test_end_to_end_ocean_mask_replaces_pixels`: Create a 128x64 solid-color JPEG. Create a synthetic shapefile with a polygon covering the eastern half of the globe. Run the CLI with `--ocean-mask`. Open the output JXL and verify:
    - Pixels in the eastern half (right side of image) match the default ocean color (10, 40, 80) within a tolerance of +/-5 per channel (LANCZOS downscale introduces minor blending).
    - Pixels in the western half (left side) retain the original color.
  - `test_end_to_end_ocean_mask_custom_color`: Same as above but with `--ocean-color "0,0,255"`. Verify the eastern half is blue.
  - `test_end_to_end_ocean_mask_with_buffer`: Run with `--ocean-buffer 3`. Verify that pixels near the boundary have intermediate values (not a hard edge between fill and source).
  - `test_end_to_end_without_ocean_mask_unchanged`: Run the pipeline without `--ocean-mask` on the same input. Verify the output is identical to the current pipeline behavior (no ocean processing).
  - `test_end_to_end_ocean_mask_multiple_widths`: Run with `--ocean-mask` and `--width 64 --width 32`. Verify both output files have ocean pixels replaced.
- **Verify**: Tests exist and fail.
- **Complexity**: Medium

#### Step 6.2: Integrate ocean masking into `run_pipeline` (GREEN)

- **Files**: `tools/texture-pipeline/src/texture_pipeline/main.py`
- **Action**: In `run_pipeline()`, after `img = Image.open(source_path)` and the RGB mode conversion, add:

  ```python
  if ocean_shapefile is not None:
      from texture_pipeline.ocean_masking import (
          apply_ocean_mask,
          get_or_create_mask,
      )
      mask = get_or_create_mask(
          ocean_shapefile,
          img.size[0],
          img.size[1],
          supersample=ocean_supersample,
          buffer_pixels=ocean_buffer,
      )
      img = apply_ocean_mask(img, mask, color=ocean_color)
  ```

  This insertion point is after the image is loaded and converted to RGB but before the `for width in widths` loop. The lazy import keeps the geospatial imports from running when ocean masking is not used. Add a call to `clear_mask_cache()` after the main processing loop completes (at the end of `run_pipeline`), to free memory.

  Also update the import at the top of `main.py` to not import `ocean_masking` at module level -- the lazy import inside the `if` block is sufficient.
- **Verify**: All tests from Step 6.1 pass. All existing tests still pass. `uv run pytest` is fully green.
- **Complexity**: Small

### Phase 7: Manual End-to-End Verification

#### Step 7.1: Test with the real Natural Earth ocean shapefile

- **Files**: N/A (manual verification)
- **Action**: Run the pipeline against a real Blue Marble texture with the `ne_10m_ocean.shp` shapefile:

  ```bash
  cd tools/texture-pipeline
  uv run texture-pipeline convert \
      --input ../../tmp/texture-source/ \
      --output ../../tmp/texture-output-ocean/ \
      --ocean-mask ../../tmp/ocean/ne_10m_ocean.shp \
      --ocean-color "10,40,80" \
      --ocean-supersample 2 \
      --ocean-buffer 2 \
      --width 4096 --width 2048 \
      --quality 85 --effort 1
  ```

- **Manual test cases**:
  - Pipeline completes without errors; progress bar displays normally.
  - Output JXL files exist at expected paths with correct dimensions.
  - Visual inspection: open output textures. Ocean regions should be a uniform dark blue (10, 40, 80). Land regions should be unchanged from the source texture. Coastlines should show smooth anti-aliased transitions, not hard staircases.
  - With `--ocean-buffer 2`, the boundary between the fill color and land texture should have a gentle gradient, not a hard edge.
  - Compare with output from a run without `--ocean-mask` to confirm land pixels are unchanged.
  - Run a second time to verify the in-memory mask cache works (mask rasterization log message should appear once per resolution, not once per file).
- **Verify**: All manual checks pass.
- **Complexity**: Small

#### Step 7.2: Verify memory usage at high resolution

- **Files**: N/A (manual verification)
- **Action**: Run the pipeline with the full 21600x10800 Blue Marble source at 2x supersampling. Monitor peak memory usage via Task Manager.
- **Manual test cases**:
  - Peak memory stays below 4 GB.
  - Pipeline completes without OOM errors.
  - If peak memory exceeds 4 GB, document the actual usage and consider reducing the default supersample factor.
- **Verify**: Memory within budget.
- **Complexity**: Small

## Test Strategy

### Automated Tests

| Test Case | Type | Input | Expected Output |
| --- | --- | --- | --- |
| Mask shape matches requested dimensions | Unit | Shapefile, 100x50 | `(50, 100)` ndarray |
| Mask dtype is uint8 | Unit | Shapefile, 100x50 | `dtype == np.uint8` |
| Binary mask at supersample=1 | Unit | Shapefile, supersample=1 | All values 0 or 255 |
| Anti-aliased mask at supersample=2 | Unit | Shapefile, supersample=2 | Some intermediate values |
| Mask coverage matches polygon area | Unit | Western-half polygon | ~50% of pixels are 255 |
| Full-ocean mask | Unit | Globe-covering polygon | All pixels 255 |
| Apply mask with all-zero mask | Unit | Mask of zeros | Output == input |
| Apply mask with all-255 mask | Unit | Mask of 255s | Output == fill color |
| Apply mask with 128 mask | Unit | Uniform 128 mask | Midpoint of source and fill |
| Apply mask custom color | Unit | Fill (0, 0, 255) | Ocean pixels are blue |
| Apply mask preserves dimensions | Unit | 128x64 image | 128x64 output |
| Coastal buffer zero is identity | Unit | buffer_pixels=0 | Mask unchanged |
| Coastal buffer expands transition | Unit | buffer_pixels=5 | Transition zone at boundary |
| Coastal buffer preserves deep ocean | Unit | buffer_pixels=5 | Interior ocean stays 255 |
| Coastal buffer preserves deep land | Unit | buffer_pixels=5 | Interior land stays 0 |
| Coastal buffer monotonic decay | Unit | Perpendicular transect | Non-decreasing land-to-ocean |
| Cache returns same object | Unit | Same args twice | `mask1 is mask2` |
| Cache recomputes for different size | Unit | (100, 50) then (200, 100) | Different shapes |
| Cache clear forces recompute | Unit | Call, clear, call | Different objects |
| CLI --ocean-mask default None | CLI | No flag | `ocean_shapefile=None` |
| CLI --ocean-mask passes path | CLI | `--ocean-mask path` | `ocean_shapefile=Path(...)` |
| CLI --ocean-color default | CLI | No flag | `ocean_color=(10, 40, 80)` |
| CLI --ocean-color custom | CLI | `--ocean-color "0,100,200"` | `ocean_color=(0, 100, 200)` |
| CLI --ocean-color invalid | CLI | `--ocean-color "bad"` | Error exit |
| CLI --ocean-supersample default | CLI | No flag | `ocean_supersample=2` |
| CLI --ocean-supersample custom | CLI | `--ocean-supersample 4` | `ocean_supersample=4` |
| CLI --ocean-buffer default | CLI | No flag | `ocean_buffer=0` |
| CLI --ocean-buffer custom | CLI | `--ocean-buffer 3` | `ocean_buffer=3` |
| End-to-end ocean mask replaces pixels | Integration | Shapefile + JPEG | Ocean region matches fill |
| End-to-end custom color | Integration | `--ocean-color "0,0,255"` | Ocean region is blue |
| End-to-end with buffer | Integration | `--ocean-buffer 3` | Smooth boundary |
| End-to-end without flag unchanged | Integration | No `--ocean-mask` | Output identical to baseline |
| End-to-end multiple widths | Integration | Two widths | Both have ocean replaced |

### Manual Verification

- [ ] Run against a real Blue Marble texture with `ne_10m_ocean.shp`; verify ocean regions are uniformly colored and coastlines are smooth
- [ ] Compare output with and without `--ocean-mask` to confirm land pixels are unchanged
- [ ] Test `--ocean-buffer 2` produces a visible gradual transition at coastlines
- [ ] Verify peak memory at 21600x10800 with 2x supersampling stays under 4 GB

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| rasterio/geopandas GDAL bundling issues on Windows with Python 3.14 | Install fails; pipeline unusable | Python 3.14 wheels confirmed available for rasterio 1.5.0, geopandas 1.1.3, shapely 2.1.2 on Windows. Pin minimum versions to these known-good releases. |
| Memory exceeds 4 GB at 2x supersampling for 21600x10800 | OS kills process | Research estimates ~3.5 GB peak. Document the requirement. If exceeded, user can set `--ocean-supersample 1` to halve mask memory. |
| Gaussian blur for coastal buffer is too slow at full resolution | Pipeline latency | Pillow's `GaussianBlur` is implemented in C and runs in milliseconds even at high resolution. If needed, reduce buffer_pixels or skip at full-res (the downscale itself blends). |
| Incorrect Affine transform produces misaligned mask | Ocean/land swapped or shifted | The transform math is straightforward and validated by the research. Integration tests with synthetic shapefiles verify alignment by checking that the correct half of the image is masked. |
| Adding required geospatial dependencies bloats install for users who never use ocean masking | Slower `uv sync`, larger venv | Acceptable for an internal tool per user decision. The lazy import in `run_pipeline()` means the imports do not slow down non-ocean invocations. |
| Natural Earth shapefile has topology issues at antimeridian | Artifacts at date line | Natural Earth 10m vectors are clipped to +/-180 and topologically repaired (per research). The `from_bounds(-180, -90, 180, 90, ...)` transform covers the full extent correctly. |

## Rollback Strategy

Delete `src/texture_pipeline/ocean_masking.py` and revert modifications to `main.py`, `pyproject.toml`, `conftest.py`, and test files. The ocean masking feature is entirely additive: no existing function signatures are changed (only extended with new keyword arguments that have backward-compatible defaults). Reverting the `run_pipeline` signature to its original form and removing the ocean masking call path restores the previous behavior exactly. Run `uv sync` after reverting `pyproject.toml` to remove the geospatial dependencies.

## File Inventory

```text
tools/texture-pipeline/
  pyproject.toml                              (modified: add rasterio, geopandas, numpy deps)
  src/texture_pipeline/
    ocean_masking.py                          (new: rasterize_ocean_mask, apply_ocean_mask,
                                               apply_coastal_buffer, get_or_create_mask,
                                               clear_mask_cache)
    main.py                                   (modified: new CLI flags, run_pipeline params,
                                               ocean masking insertion in processing loop)
  tests/
    conftest.py                               (modified: add synthetic shapefile fixtures)
    test_ocean_masking.py                     (new: unit tests for ocean masking module)
    test_cli.py                               (modified: tests for new CLI flags)
    test_integration.py                       (modified: end-to-end tests with ocean masking)
```

## Status

- [x] Plan approved
- [x] Implementation started
- [x] Implementation complete
