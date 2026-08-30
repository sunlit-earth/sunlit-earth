# Plan: the Milky Way panorama through the texture pipeline (2026-08-30)

## Summary

`tools/texture-pipeline` gains a second subcommand, `milky-way`, that takes one NASA SVS Deep Star Maps 2020 `milkyway_2020_*.exr` at any of its published resolutions and writes the denoised 8-bit sRGB panorama as a JPEG XL at a chosen width. The existing `convert` subcommand, which is about the Earth's surface maps, is renamed `earth`. No Rust changes; the shipped asset in `textures/` is not replaced by this work.

The algorithm is the one worked out in `tmp/milky way/experiments.md` (version 3 of `tmp/milky way/denoise_experiment.py`), which the user has judged on renders. The pipeline reproduces that result from the EXR alone.

## Stakes

Low. A new command in an offline Python tool, one renamed command, no runtime code touched. Rollback is a revert.

## Background the implementer needs

The SVS layer is every Gaia DR2 star fainter than magnitude 11.5, each rendered with a small PSF (about 7 px across at 8k) and summed into pixels. Its "noise" is the sky itself at too fine a scale: a grain of magnitude 12 to 16 stars, and isolated stars past the Tycho limit peaking at 5 to 20 times their surroundings. The eye resolves neither. A plain blur cannot fix it because the radius that removes the grain leaves every isolated star as a blob.

The EXRs hold flux per pixel, so each resolution has its own scale: the shipped 4k asset's linear mean is 16.00 times that of the 16k EXR box-averaged to 8k. The texture's brightness on screen is its texel value, and `milky_way_intensity` was tuned against the 4k asset, so every output has to land on the 4k grid's scale whatever the source or target width. With `REFERENCE_WIDTH = 4096`, a source of width `W` box-averaged to any target width needs a gain of `(W / REFERENCE_WIDTH)^2`: 16 for the 16k file, 4 for the 8k, 1 for the 4k.

## The algorithm

All in linear light, float32, three channels. Steps 3 to 5 operate on the target grid, with their lengths given in arcminutes and converted to pixels of the target width (`arcmin_per_px = 360 * 60 / target_width`, so 2.637 at 8k). The defaults below are the arcminute equivalents of the pixel values validated at 8k.

1. Read the EXR as float32 RGB (drop alpha if present), clip negatives to zero. Refuse anything that is not 2:1.
2. Multiply by `(source_width / 4096)^2`.
3. Box-average to the target width. The factor must be an integer; refuse upscaling and non-integer factors.
4. Despeckle:
   - Background: Gaussian mean of the luminance (mean of the three channels) with sigma `bg_sigma` (15.8 arcmin, 6 px at 8k), re-estimated `bg_iterations = 2` times from `min(lum, k * bg + eps)` so the stars do not lift it.
   - Cap: `cap = k * bg + eps` with `k = 3.0`, `eps = 0.0005`; scale all three channels of a pixel by `min(1, cap / lum)`, so a trimmed pixel keeps its color.
   - Strong stars: `lum > strong * bg + eps` with `strong = 9.0`, dilated by `dilate` (7.9 arcmin, 3 px at 8k); those pixels are replaced per channel by a normalized convolution over the unmasked pixels, Gaussian sigma `fill_sigma` (7.9 arcmin, 3 px at 8k), weight floored at 1e-3.
5. Gaussian blur, sigma `blur` (7.9 arcmin, 3 px at 8k).
6. sRGB transfer curve, clip to [0, 1], triangular dither of one code value (sum of two uniforms minus one, mean zero) from `numpy.random.default_rng(seed)` with `seed = 7` by default, round, `uint8`. Dither is not optional: without it the smoothed dark sky posterizes.
7. Encode JPEG XL, lossy, `--quality` default 90 (see amendment 1), `--effort` as in the existing command, default 7. Quality 100 means lossless (`lossless=True` in pillow-jxl).

Boundary handling for every Gaussian and the dilation: wrap along columns (longitude is periodic) and nearest along rows (the poles are not).

Measured with these parameters at 8k from the 16k EXR (`tmp/milky way/exp8k_v3_linear_stats.txt`): at the galactic center 0.0% of pixels capped and 0.02% strong-masked with the mean preserved to 0.1%; in Cygnus 1.3% capped and 0.05% masked with the mean preserved to 0.5%; at the north galactic pole 64.5% masked and the mean halved, which is correct, because that sky is only discrete stars. Block-to-block scatter falls from 26% to 6% at the center. Not validated at other target widths: the thresholds are relative to a local mean whose grain depends on how many stars a pixel holds, so 4k or 16k output is supported but unjudged.

Deliberately not included: the floor variant from the experiments (subtracting the pole level), which the user judged to look noisier than plain g3.

## Implementation steps

### Step 1: rename `convert` to `earth`

- Files: `tools/texture-pipeline/src/texture_pipeline/main.py`, `tests/test_cli.py`, `tests/test_integration.py` (wherever `"convert"` is invoked), `README.md`.
- The command's options and behavior are unchanged. Its help text should say what it is for now that there are two commands: the Earth's surface maps (Blue Marble, Black Marble) at one or more widths. `app.help` becomes a one-line description of the tool as a whole.
- Add a test that `convert` is no longer a command (exit code 2 from Typer).
- Verify: `uv run pytest`, `uv run ruff check src/ tests/`, `uv run ruff format --check src/ tests/`.

### Step 2: EXR input

- Dependency: `OpenEXR>=3.3` (the official bindings; the 3.4.15 wheel installs and imports under this tool's Python 3.14.5 on Windows). Add with `uv add OpenEXR` so `uv.lock` moves with it.
- New module `src/texture_pipeline/exr.py` with `read_exr_rgb(path) -> numpy.ndarray` (float32, `(H, W, 3)`), reading the `R`, `G`, `B` channels whatever their stored type (half or float) and ignoring others. The 16k file is 16384x8192 half floats, 805 MB as float32 per channel set; read it straight into one array and free nothing prematurely, but do not make copies.
- Also a `write_exr_rgb(path, array)` for tests only if the library makes reading back simpler than writing; otherwise tests write their fixture EXR through the library directly in `conftest.py`.
- Tests: write a small EXR (say 64x32) with known values, read it back, compare exactly; a file with an alpha channel reads as three channels; a non-2:1 file is refused with a clear message (the refusal itself may live in the pipeline function, tested there).

### Step 3: the algorithm as pure functions

- New module `src/texture_pipeline/milky_way.py`. Functions, each small and separately testable:
  - `flux_gain(source_width, reference_width=4096) -> float`
  - `box_downsample(a, factor) -> ndarray` (mean over `factor x factor` blocks; `ValueError` for a non-integer factor or factor below 1)
  - `arcmin_to_px(arcmin, width) -> float`
  - `estimate_background(lum, sigma_px, k, eps, iterations) -> ndarray`
  - `despeckle(a, *, k, strong, eps, bg_sigma_px, bg_iterations, dilate_px, fill_sigma_px) -> ndarray`
  - `smooth(a, sigma_px) -> ndarray`
  - `to_srgb8(a, seed) -> ndarray[uint8]` (transfer curve, dither, quantize)
  - `process(a, *, source_width, target_width, params) -> ndarray[uint8]` composing the above, where `params` is a small dataclass `MilkyWayParams` holding the arcminute defaults and thresholds.
- Use `scipy.ndimage` (already a dependency) for the Gaussians, the dilation and the wrap/nearest modes: `gaussian_filter` takes a per-axis `mode` sequence, `binary_dilation` does not, so dilate on an array padded by `dilate_px` columns from the opposite edge and crop, or document why wrap on rows is harmless there.
- Tests on synthetic data, no real asset:
  - gains: 16384 gives 16, 8192 gives 4, 4096 gives 1.
  - `box_downsample` of a 4x4 with known blocks gives their means; factor 3 on width 8 raises.
  - `despeckle` on a flat field of 0.01 with one Gaussian star of peak 0.5 (sigma 1.5 px) placed at a column edge (to exercise the wrap): the peak comes down to within 20% of the background, the mean over the field moves by under 1%, and a pixel that was capped keeps its channel ratios (use a colored star, say 3:2:1).
  - `despeckle` leaves a smooth gradient (no star) unchanged to 1e-6.
  - `smooth` of a field wraps: a bright column at column 0 spreads into the last columns.
  - `to_srgb8`: deterministic for the same seed; on a flat field at 0.002 linear the mean of the output, decoded back through the inverse curve, is within half a code value of the input; on a gradient from 0 to 0.01 the number of distinct output values is at least that of the undithered quantization.
  - `process` on a 512x256 synthetic sky produces `(128, 256, 3)` uint8 for target width 256 (factor 2) and refuses target width 1024.

### Step 4: the `milky-way` command

- In `main.py`: `milky-way` with `--input/-i FILE` (must exist), `--output/-o FILE` (parent created), `--width/-w INT` (default 8192; must be even), `--quality/-q` (default 90; 1 to 100, where 100 is lossless), `--effort/-e` (1 to 9, default 7), `--seed` (default 7), and the algorithm's parameters as options with the defaults above (`--k`, `--strong`, `--eps`, `--bg-sigma`, `--dilate`, `--fill-sigma`, `--blur`, all in arcminutes where they are lengths). Reuse the existing validators where they fit.
- A `run_milky_way(...)` function in `main.py` mirroring `run_pipeline`'s shape (keyword-only, patched by the CLI tests), printing what it did: source size, gain, target size, the percentage of pixels capped and strong-masked, output size in MB, elapsed time.
- `encode_jxl` in `processing.py` treats quality 100 as lossless (passes `lossless=True`); below 100 it encodes as it does today. `earth` is unchanged at its default of 85.
- Tests: `milky-way --help` lists the options; defaults reach `run_milky_way` (patched); `--width 8191` is refused; an end-to-end run on a synthetic 512x256 EXR fixture at the default quality produces a JXL that Pillow opens at 256x128 RGB whose decode differs from the array `process` returns by a mean of under 1.0 of 255 and a maximum of at most 8; the same run with `--quality 100` decodes to exactly that array (lossless round trip).

### Step 5: documentation

- `README.md` in the tool: both commands, the `milky-way` options, and a short paragraph on the flux-per-pixel gain, why the output is dithered, and the lossy measurement in amendment 1.
- `docs/roadmap.md`: one item for the follow-up this plan stops short of, replacing `textures/milkyway_2020_4k.jxl` with the tool's 8k output, which needs `memory::milky_way_texture_bytes` and its tests moved off the 4096 assumption, the loader's file name, `textures/PROVENANCE.md`, and a rerun of the two engine tests that read the real asset.
- `docs/README.md` if it indexes plans.

### Step 6: run it on the real file

- `cd tools/texture-pipeline && uv run texture-pipeline milky-way -i "../../tmp/milky way/milkyway_2020_16k.exr" -o "../../tmp/milky way/milkyway_2020_8k_pipeline.jxl" -w 8192`.
- Compare against `tmp/milky way/exp8k_v3_linear_despeck_g3.png`: decode both, mean absolute channel difference over the whole image should be under 1.0 of 255 (the two differ in the dither draw and in ImageMagick's float TIFF against numpy's box filter) and the per-region means from the stats file should agree to 1%. Write the numbers into the status log. Do not put the output in `textures/`.
- Report elapsed time and peak memory if easily read.

## Gates

From the tool's README, run in `tools/texture-pipeline`:

```
uv run pytest
uv run ruff check src/ tests/
uv run ruff format --check src/ tests/
uv run ty check src/
```

No Rust is touched, so no cargo gate applies; do not run `cargo test`.

## Constraints

- No commits, no branches, no PR: the user reviews the working tree and the output texture. Work in place in the repository.
- Do not modify anything under `textures/`, `crates/`, or `tmp/milky way/` except for writing the one output file named in step 6.
- Keep `uv.lock` consistent: dependencies change only through `uv add`.
- Follow the tool's existing style: Sphinx-style docstrings (`:param:`, `:returns:`, `:raises:`), Typer `Annotated` options with validators, tests in classes as in `tests/test_cli.py`. Comments only where the code cannot say it; never comments about history or this plan.
- Writing style for docs: no em dashes, no dashes as parentheticals, American English, plain prose.

## Amendments

Changes to the plan made by the user or the orchestrator while the work runs.

1. **Lossy by default, quality 90.** The plan first asked for lossless, carrying over the reasoning behind the current asset in `textures/PROVENANCE.md`: Gaia photon noise over a smooth gradient is the worst content for a lossy codec. After denoising that reason is gone, and the remaining worry, that a lossy codec would flatten the dither and reintroduce banding in the near-black sky, was measured and does not hold. Encoding `exp8k_v3_linear_despeck_g3.png` with libjxl through ImageMagick: quality 85 is 0.7 MB, 90 is 0.9 MB, 95 is 1.3 MB, 99 is 4.3 MB, lossless is 21.5 MB. Against the source, the mean absolute channel difference is 0.55, 0.50, 0.45 and 0.40 of 255, the maximum 6, 5, 5 and 3, no pixel off by more than 4 at any quality, per-region means shifted by under 0.03 of a code value, and the north galactic pole tile keeps its count of distinct levels (20 against 21) with the same fine texture at 8x gain and no blocking or posterization even at 85. Cygnus at 1:1 is indistinguishable at 85, 95 and lossless. The comparison images are `tmp/milky way/stage/compare_lossy_pole_x8.png` and `compare_lossy_cygnus_1to1.png`. Default 90 rather than the Earth maps' 85 for a little headroom in the toe; the difference is 0.2 MB.

## Departures

Numbered, appended here by the implementer when reality disagrees with the plan.

1. **The format gate was already red, so three files were reformatted that this plan does not otherwise touch.** `uv run ruff format --check src/ tests/` failed on `src/texture_pipeline/ocean_masking.py`, `tests/test_integration.py` and `tests/test_ocean_masking.py` before any of this work started, under ruff 0.15.6. Two of those three are outside the plan's file list. Since the gate is one of the four that must pass, `uv run ruff format src/ tests/` was run once over the whole tool and the result is in the working tree. The changes are the formatter's own: a few calls joined onto one line and a few argument lists exploded one per line. Nothing about their behavior moves, but the reviewer should expect diff noise in those two files that has nothing to do with the Milky Way.

2. **`despeckle` and `process` return a named tuple rather than a bare array.** Step 4 asks the command to print the percentage of pixels capped and strong-masked, and nothing but the despeckle knows those. Rather than compute the masks twice or hand back the two full boolean planes, both functions return a `NamedTuple` whose first field is the image and whose other two are the fractions, so `image, capped, strong = despeckle(...)` still reads as the plan's signature does.

3. **The step 3 test measures the star's own pixel, not the frame's maximum.** The plan asks that a Gaussian star of peak 0.5 on a background of 0.01 comes down "to within 20% of the background". At the star's position it does: 0.0119 against 0.0100, down from 0.51. The frame's maximum after the despeckle is 0.0192, three pixels diagonally out, and that is not a defect: a true Gaussian has wings at every radius, while the rendered point spread function the `dilate` of 3 px is sized for stops at about 3.5 px, so the synthetic star leaves a residual the real data does not have. The test therefore asserts the plan's 20% at the star and bounds the wing separately at 2.5 times the background, which is the honest statement of what the step removes.

4. **Two of the four gates were red before this work, and both are now fixed in place.** `ruff format --check` is departure 1. `ty check src/` failed on `src/texture_pipeline/ocean_masking.py` using `Image.LANCZOS`, the deprecated alias of `Image.Resampling.LANCZOS` that `processing.py` already spells correctly; that one character-for-character equivalent line is changed. The new EXR import failed the same gate for a different reason: the OpenEXR wheel is a bare extension module with no type information at all, so `stubs/OpenEXR.pyi` declares the handful of names this tool touches and `[tool.ty.environment] extra-paths` points at it. A suppression comment would have been the smaller edit and a worse one, since it would also hide a real mistake in the same import.
