# Research: Ocean Masking for Texture Pipeline (2026-03-16)

## Problem Statement

The Sunlit Earth desktop app renders a 3D globe using NASA Blue Marble equirectangular textures. The roadmap notes that "ocean areas are too dark" in the Blue Marble day textures and calls for pre-processing to correct water brightness. The proposed solution is to rasterize a vector ocean boundary (Natural Earth 10m ocean shapefile) into a pixel mask, then replace ocean pixels with a uniform deep-ocean fill color before the existing downscale/sharpen/encode pipeline runs.

This document consolidates findings from external research on geospatial rasterization libraries and ocean color selection with codebase analysis of the existing texture pipeline architecture, ocean data assets, and integration constraints.

## Requirements

1. **Ocean mask generation**: Rasterize `ne_10m_ocean.shp` (already present at `tmp/ocean/`) to produce a binary or alpha mask at the source texture's resolution (up to 21600x10800).
2. **Ocean pixel replacement**: Replace masked ocean pixels with a configurable uniform RGB color, defaulting to a value that matches NASA Blue Marble deep-ocean fill.
3. **Anti-aliased coastlines**: Coastline edges should blend smoothly between the fill color and the original land pixels to avoid hard staircasing artifacts.
4. **Pipeline integration**: The masking step must slot into the existing texture pipeline (`tools/texture-pipeline/`) as an optional stage before downscaling, controlled by CLI flags.
5. **Performance**: Must handle 21600x10800 source images without exceeding reasonable memory bounds (under ~4 GB peak).
6. **Minimal disruption**: The existing `downscale` -> `sharpen` -> `encode_jxl` pipeline must continue to work unchanged when ocean masking is not requested.

## Findings

### Current Pipeline Architecture

The texture pipeline is a Python 3.14+ CLI tool at `tools/texture-pipeline/` built with Pillow, Typer, Rich, and pillow-jxl-plugin. The processing flow in `run_pipeline()` (`main.py`) is:

1. `discover_images()` finds source files recursively
2. For each source image, for each target width:
   - `downscale()` resizes to target width (LANCZOS, strict 2:1 aspect ratio)
   - Optional `sharpen()` applies UnsharpMask
   - `encode_jxl()` writes the output file

The pipeline loads each source image once per file via `Image.open()`, converts to RGB mode, then iterates over target widths. The Pillow decompression bomb limit is disabled to support 21600x10800 sources (~233M pixels).

Ocean masking must operate on the full-resolution source image before the per-width downscale loop begins. This means the masking step runs once per source file, and all subsequent width variants inherit the masked result.

### Ocean Data Assets

The Natural Earth 10m ocean shapefile is already present in the repository at `tmp/ocean/ne_10m_ocean.shp` with companion files (.shx, .dbf, .prj, .cpg). Key properties:

- **CRS**: WGS84 (EPSG:4326) -- longitude/latitude coordinates that map directly to equirectangular pixel space without reprojection.
- **Version**: 5.1.1 (May 2022).
- **Feature count**: Under 200 polygons. The modest count means rasterization is fast even at high resolution.
- **Antimeridian handling**: Natural Earth 10m vectors are clipped to the +/-180 degree bounding box and topologically repaired, so GDAL/rasterio handle them cleanly when the transform covers the full -180 to +180 extent.

### Rasterization Approach

#### Library selection: rasterio + geopandas

The de-facto standard Python workflow for shapefile rasterization is `rasterio.features.rasterize()` fed from a GeoPandas GeoDataFrame. This combination is well-documented and actively maintained.

- **geopandas** reads the shapefile: `gpd.read_file("ne_10m_ocean.shp")` returns a GeoDataFrame whose `.geometry` column is directly consumable by `rasterize()`.
- **rasterio** performs the rasterization: it wraps GDAL's polygon-burn algorithm and returns a NumPy ndarray in-memory -- no intermediate output file is needed.
- **fiona** (used internally by geopandas) can alternatively be used directly to iterate geometries one at a time if memory is tight.

Alternatives considered:

| Library | Verdict | Reason |
|---|---|---|
| `gdal_rasterize` CLI | Viable for one-time offline mask generation | Avoids Python overhead, but less suitable for in-pipeline use |
| Shapely alone | Not suitable | No built-in rasterization; useful only for geometry pre-processing |
| Pillow `ImageDraw.polygon()` | Not recommended | No coordinate-system awareness, aliased output only, requires manual vertex projection |

**Confidence: High.** This is the standard approach in the Python geospatial ecosystem.

#### Coordinate system and Affine transform

An equirectangular grid covering the full globe at W x H pixels has the Affine transform:

```python
from rasterio.transform import from_bounds
transform = from_bounds(west=-180.0, south=-90.0, east=180.0, north=90.0, width=W, height=H)
```

Pixel (0, 0) is the top-left corner at (-180, +90), matching the north-up convention of NASA Blue Marble TIFFs. The Y-scale in the Affine matrix is negative to handle the north-up convention automatically. No reprojection is needed because WGS84 geographic coordinates are used directly as Cartesian pixel coordinates in the equirectangular projection.

**Confidence: High.** The math is straightforward and well-documented.

#### Core rasterization code

```python
import numpy as np
import geopandas as gpd
from rasterio.transform import from_bounds
from rasterio.features import rasterize
import os

os.environ["GDAL_CACHEMAX"] = "512"  # MB; ensures single-pass burn

gdf = gpd.read_file("ne_10m_ocean.shp")
gdf = gdf.to_crs("EPSG:4326")  # guard against stale .prj

transform = from_bounds(-180.0, -90.0, 180.0, 90.0, W, H)

ocean_mask = rasterize(
    shapes=((geom, 1) for geom in gdf.geometry),
    out_shape=(H, W),
    transform=transform,
    fill=0,
    dtype="uint8",
    all_touched=False,
)
```

### Performance at 21600x10800

**Memory for the mask array**: A uint8 array at 21600x10800 is 233 MB. Using `dtype="uint8"` is important; the float64 default would require 1.9 GB.

**GDAL cache**: `rasterio.features.rasterize()` iterates shapes multiple times if `GDAL_CACHEMAX` is smaller than the output data. Setting `GDAL_CACHEMAX=512` (MB) before calling `rasterize()` ensures a single-pass burn and typical wall-clock time of a few seconds.

**Total pipeline memory**: The source Blue Marble image in Pillow uses approximately 1.75 GB (21600 x 10800 x 3 bytes as RGB, plus Pillow overhead). The ocean mask adds 233 MB (or ~932 MB at 2x supersample). The alpha-blended result adds another ~700 MB as float32. Peak memory at 2x supersampling is roughly 3.5 GB, which fits within a 4 GB budget. At 4x supersampling, the mask alone would require ~3.7 GB, making total peak memory impractical on most workstations.

**Confidence: High.** Memory figures are derived from array dimensions. GDAL cache guidance is from rasterio documentation.

### Ocean Color Selection

The Blue Marble: Next Generation dataset uses a uniform fill color for deep ocean regions where MODIS ocean color data is unavailable. This fill is not derived from real sensor data -- it is a hand-chosen flat color.

Recommended value: **RGB (10, 40, 80)** -- a dark, slightly greenish navy that matches the BMNG deep ocean fill. This was determined by cross-referencing Blue Marble pixel samples, professional cartographic palettes, and NASA Ocean Color documentation.

Other credible options for reference:

| Use case | RGB | Hex | Notes |
|---|---|---|---|
| Faithful Blue Marble match | (10, 40, 80) | #0A2850 | Best match for BMNG deep ocean fill |
| Slightly brighter | (28, 107, 160) | #1C6BA0 | More readable but deviates from source |
| Cartographic deep water | (0, 60, 95) | #003C5F | Named "Deep Ocean Blue" in color references |

To verify empirically: open any BMNG tile and sample a pixel in the open South Pacific, far from land and plankton blooms. Typical range: R 8-15, G 38-50, B 75-95.

**Confidence: Medium.** The specific RGB value is a best-effort estimate. A deviation of +/-15 per channel from (10, 40, 80) is plausible. The default should be configurable via CLI.

### Anti-Aliasing at Polygon Edges

`rasterio.features.rasterize()` is a hard binary rasterizer. Every pixel is either 0 or 1 with no partial coverage. The `all_touched` parameter controls the coverage rule:

- `all_touched=False` (default): pixel is ocean if its center falls inside the polygon. Slightly inward-biased coastlines.
- `all_touched=True`: pixel is ocean if any part of the polygon touches it. Slightly outward-biased, thicker coastlines.

For a clean ocean mask, `all_touched=False` is preferred because it avoids inland bleeding along narrow coastal features.

#### Supersampling for smooth edges

To get anti-aliased edges, rasterize at 2x the target resolution and downsample with Lanczos:

```python
SCALE = 2
W_ss, H_ss = W * SCALE, H * SCALE
transform_ss = from_bounds(-180.0, -90.0, 180.0, 90.0, W_ss, H_ss)

ocean_mask_ss = rasterize(
    shapes=((geom, 255) for geom in gdf.geometry),
    out_shape=(H_ss, W_ss),
    transform=transform_ss,
    fill=0,
    dtype="uint8",
    all_touched=False,
)

mask_img = Image.fromarray(ocean_mask_ss, mode="L")
mask_img_ds = mask_img.resize((W, H), Image.LANCZOS)
ocean_mask_aa = np.array(mask_img_ds)  # values 0-255, fractional coverage
```

The resulting grayscale mask (0 = fully land, 255 = fully ocean) enables alpha-blending at coastlines:

```python
alpha = ocean_mask_aa.astype(np.float32) / 255.0
result = (source * (1.0 - alpha[..., None]) +
          DEEP_OCEAN_RGB * alpha[..., None]).astype(np.uint8)
```

#### When to skip anti-aliasing

At 21600x10800 (~1 km/pixel), natural coastline generalization in the 10m shapefile already exceeds the pixel resolution, so anti-aliasing is cosmetically beneficial but not critical. For preview textures at 4096x2048 or 2048x1024, anti-aliasing is more visibly important -- but those are produced by the subsequent LANCZOS downscale, which itself acts as an anti-aliasing filter on the full-resolution masked result. For the full-resolution mask, 2x supersampling is the practical choice.

**Confidence: High.** Supersampling with Lanczos downsample is a well-established cartographic technique.

### Integration Design

#### New module: `ocean_masking.py`

A new module at `src/texture_pipeline/ocean_masking.py` should encapsulate:

1. **`rasterize_ocean_mask(shapefile_path, width, height, supersample=2)`** -- Loads the shapefile, rasterizes at `supersample` times the target resolution, downsamples to produce a float or uint8 alpha mask.
2. **`apply_ocean_mask(image, mask, color)`** -- Alpha-blends the ocean fill color onto the image using the mask.

These two functions separate mask generation (expensive, cacheable) from mask application (cheap, per-image).

#### Mask caching

Rasterizing the shapefile at 21600x10800 (or 43200x21600 at 2x supersample) takes several seconds and significant memory. Since the mask depends only on the shapefile and the source dimensions, it can be computed once and reused across all source images of the same resolution. The pipeline should cache the mask array in memory for the duration of a run and optionally save it to disk (e.g., as a NumPy `.npy` file or grayscale PNG) for reuse across runs.

#### CLI flags

New options on the `convert` command:

| Flag | Type | Default | Description |
|---|---|---|---|
| `--ocean-mask` | `Path` (optional) | None | Path to ocean shapefile. When provided, enables ocean masking. |
| `--ocean-color` | `str` | `"10,40,80"` | RGB fill color as comma-separated integers. |
| `--ocean-supersample` | `int` | `2` | Supersampling factor for anti-aliased coastlines (1 = no AA). |

When `--ocean-mask` is not provided, the pipeline behaves exactly as it does today. This preserves backward compatibility.

#### Pipeline insertion point

In `run_pipeline()`, ocean masking runs after `Image.open()` and RGB conversion but before the per-width downscale loop:

```python
img = Image.open(source_path)
if img.mode != "RGB":
    img = img.convert("RGB")

# NEW: apply ocean mask if configured
if ocean_shapefile is not None:
    mask = get_or_create_mask(ocean_shapefile, img.size, supersample)
    img = apply_ocean_mask(img, mask, ocean_color)

for width in widths:
    scaled = downscale(img, target_width=width)
    # ... sharpen, encode as before
```

This means masking happens once per source image at full resolution, and all width variants inherit the result.

#### Testing strategy

**Unit tests** for `ocean_masking.py`:

- `test_rasterize_ocean_mask_shape`: Verify output array shape matches requested dimensions.
- `test_rasterize_ocean_mask_values`: Verify mask contains only 0 and 255 at supersample=1, or intermediate values at supersample>1.
- `test_apply_ocean_mask_land_unchanged`: With a mask of all zeros, the output image must equal the input.
- `test_apply_ocean_mask_ocean_replaced`: With a mask of all 255s, the output must equal the fill color everywhere.
- `test_apply_ocean_mask_blending`: With an intermediate mask value (e.g., 128), the output must be a blend of source and fill color.

**Integration test**: Run the full pipeline with a small synthetic shapefile (a simple rectangle) and a small test image, verify the output JXL contains the expected fill color in the masked region.

**Test fixtures**: Create minimal test shapefiles using Shapely + fiona in conftest.py rather than depending on the full Natural Earth dataset for tests.

The new `ocean_masking.py` functions are pure (input array in, output array out) which makes them straightforward to test without mocking.

#### New dependencies

```toml
dependencies = [
    "pillow>=11.0",
    "pillow-jxl-plugin>=1.3.7",
    "typer>=0.12",
    "rich>=13.0",
    "rasterio>=1.4",
    "geopandas>=1.0",
]
```

Adding rasterio and geopandas brings in substantial transitive dependencies (GDAL, fiona/pyogrio, numpy, shapely, pyproj). This is the main cost of the approach.

Alternatively, rasterio and geopandas could be declared as optional dependencies:

```toml
[project.optional-dependencies]
ocean = ["rasterio>=1.4", "geopandas>=1.0"]
```

This keeps the base installation lightweight for users who do not need ocean masking. The `--ocean-mask` CLI flag would raise a clear error if the optional dependencies are not installed.

## External Research

All external findings are sourced from the document at `docs/plans/2026-03-16-ocean-masking-external.md`, which includes full citations and per-section confidence assessments.

Key external sources:

- rasterio documentation (v1.4.4) for `features.rasterize()`, `transform.from_bounds()`, and GDAL cache guidance
- GDAL documentation for `gdal_rasterize` command-line alternative
- Natural Earth Data (naturalearthdata.com) for shapefile specification and antimeridian handling
- NASA Earth Observatory for Blue Marble: Next Generation color characteristics
- Wikipedia (Spatial anti-aliasing) for supersampling theory

**Confidence summary from external research:**

| Topic | Confidence |
|---|---|
| Library selection (rasterio + geopandas) | High |
| Affine transform construction | High |
| GDAL_CACHEMAX performance guidance | High |
| Antimeridian handling in ne_10m_ocean | High |
| Anti-aliasing via supersampling | High |
| Deep ocean color RGB (10, 40, 80) | Medium |

## Technical Constraints

1. **Python 3.14 requirement**: The existing pipeline requires Python >= 3.14. Both rasterio and geopandas publish wheels for recent Python versions, but 3.14 wheel availability should be verified before implementation (3.14 is very new).
2. **Dependency weight**: rasterio and geopandas bring GDAL, numpy, shapely, pyproj, and fiona as transitive dependencies. This significantly increases install size and potential for platform-specific build issues compared to the current Pillow-only stack.
3. **Memory budget**: At 2x supersampling for a 21600x10800 source, peak memory is approximately 3.5 GB. This is within bounds for modern workstations but should be documented.
4. **GDAL on Windows**: rasterio wheels for Windows bundle GDAL, so no separate GDAL installation is needed. However, GDAL version compatibility issues occasionally surface with new Python versions.
5. **Shapefile path**: The ocean shapefile at `tmp/ocean/ne_10m_ocean.shp` is in a `tmp/` directory that may be gitignored or not distributed. The CLI requires an explicit `--ocean-mask` path rather than assuming a default location.
6. **2:1 aspect ratio**: Ocean masking does not affect the aspect ratio -- it replaces pixel colors in place. The existing pipeline's strict 2:1 validation remains unchanged.
7. **Coordinate convention**: The mask rasterization uses the same north-up, prime-meridian-centered convention as the source Blue Marble textures. No coordinate transform is needed between the mask and the source image.

## Open Questions

1. **rasterio/geopandas Python 3.14 wheels**: Do rasterio >= 1.4 and geopandas >= 1.0 have pre-built wheels for Python 3.14 on Windows? If not, building from source requires a C compiler and GDAL development libraries, which is a significant installation burden. This should be verified before committing to these dependencies.
2. **Optional vs. required dependencies**: Should rasterio/geopandas be required dependencies (simpler code, heavier install) or optional dependencies (lighter install, requires import-time guards and a clear error message)? The optional approach is recommended but adds complexity.
3. **Mask caching strategy**: Should the rasterized mask be cached to disk between runs? A 21600x10800 uint8 mask is 233 MB uncompressed but compresses well (LZW PNG or .npy.gz). Disk caching avoids re-rasterizing on every pipeline invocation but requires cache invalidation logic.
4. **Empirical ocean color verification**: The recommended RGB (10, 40, 80) should be verified by sampling actual BMNG source pixels in the deep South Pacific. The confidence is Medium and a +/-15 per-channel deviation is plausible.
5. **Coastal transition zone**: Should there be a configurable buffer zone around coastlines where the fill color blends into the original texture, independent of anti-aliasing? This could help hide the boundary between the flat fill and the original shallow-water texture data.
6. **Night texture**: The roadmap item mentions "ocean areas are too dark" which may apply to both day and night textures. Should ocean masking be applied to night textures as well, and if so, with what color?
7. **Pre-rasterized mask distribution**: Instead of requiring rasterio/geopandas at all, the mask could be pre-rasterized once (via `gdal_rasterize` or a one-time script) and distributed as a compressed image file. This would eliminate the heavy geospatial dependencies entirely, at the cost of flexibility.

## Recommendations

1. **Use rasterio + geopandas as optional dependencies.** Declare them under `[project.optional-dependencies] ocean = [...]` to keep the base installation lightweight. The `--ocean-mask` CLI flag should raise `typer.BadParameter` with installation instructions if the imports fail.

2. **Create a dedicated `ocean_masking.py` module** with two pure functions: `rasterize_ocean_mask()` and `apply_ocean_mask()`. Keep the module self-contained with lazy imports of rasterio/geopandas so the rest of the pipeline is unaffected when the dependencies are absent.

3. **Default to 2x supersampling** for anti-aliased coastlines. This adds ~700 MB of memory overhead at full resolution but produces visibly smoother edges. Make it configurable via `--ocean-supersample` so users on memory-constrained machines can set it to 1.

4. **Default ocean color: RGB (10, 40, 80).** Make it configurable via `--ocean-color`. Before finalizing the default, sample actual BMNG pixels in the deep ocean to verify.

5. **Insert masking before the per-width downscale loop** in `run_pipeline()`. This ensures masking runs once per source image and all width variants benefit.

6. **Cache the mask in memory** for the duration of a pipeline run (all source images at the same resolution share the mask). Defer disk caching to a future enhancement -- the rasterization takes only seconds and the added complexity of cache invalidation is not justified yet.

7. **Verify Python 3.14 wheel availability** for rasterio and geopandas on Windows before implementation. If wheels are not available, consider the pre-rasterized mask alternative (Open Question 7) as a fallback that eliminates the geospatial dependency entirely.

8. **Test with minimal synthetic shapefiles** created in conftest.py via Shapely + fiona, not the full Natural Earth dataset. This keeps tests fast and self-contained.

## Sources

| Document | Focus Area |
|---|---|
| `docs/plans/2026-03-16-ocean-masking-external.md` | Rasterization libraries, Affine transforms, GDAL performance, ocean color selection, anti-aliasing techniques |
| `tools/texture-pipeline/src/texture_pipeline/main.py` | Pipeline architecture, CLI structure, processing flow |
| `tools/texture-pipeline/src/texture_pipeline/processing.py` | Downscale, sharpen, encode functions |
| `tools/texture-pipeline/src/texture_pipeline/discovery.py` | File discovery and output path construction |
| `tools/texture-pipeline/pyproject.toml` | Current dependencies, Python version, tooling |
| `tools/texture-pipeline/tests/conftest.py` | Existing test fixture patterns |
| `docs/roadmap.md` | "Blue Marble water brightness" roadmap item |
| `tmp/ocean/ne_10m_ocean.shp` | Natural Earth 10m ocean shapefile (WGS84, v5.1.1) |
