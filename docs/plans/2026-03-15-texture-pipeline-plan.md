# Plan: Texture Pre-Processing Pipeline (2026-03-15)

## Summary

Build a standalone Python CLI tool at `tools/texture-pipeline/` that converts NASA Blue Marble source textures (JPEG, PNG, TIFF) into JPEG XL files at one or more target resolutions. The tool uses Pillow for image processing, pillow-jxl-plugin for JXL encoding, Typer for the CLI interface, and Rich for progress reporting. It is managed with `uv` and is fully independent of the Rust build system. The pipeline outputs textures in standard geographic convention (north-up, prime-meridian-left); coordinate transforms remain in the Rust loader.

## Stakes Classification

**Level**: Medium
**Rationale**: This is a greenfield addition in a new language (Python) within the existing Rust repository. It touches no existing Rust code, so there is zero risk of breaking the running application. However, it establishes the project structure, dependency choices, and CLI conventions that future Python tooling will follow, making correctness of the initial design moderately important. Rollback is trivial (delete the `tools/` directory).

## Context

**Research**: [`docs/plans/2026-03-15-texture-pipeline-research.md`](2026-03-15-texture-pipeline-research.md)
**Affected Areas**: New `tools/texture-pipeline/` directory. `.gitignore` updated. No changes to Rust source code.

## Success Criteria

- [ ] `uv sync` in `tools/texture-pipeline/` creates a working virtualenv with all dependencies on Python 3.14
- [ ] `uv run texture-pipeline --help` prints a well-formatted help message
- [ ] Given a folder of JPEG source textures, the tool produces `.jxl` files at each requested resolution, preserving directory structure
- [ ] Output files are valid JPEG XL (openable by any JXL-capable viewer)
- [ ] Output dimensions maintain exact 2:1 aspect ratio
- [ ] The `lossless_jpeg=False` flag is always passed when saving JXL, so quality/effort parameters take effect
- [ ] Default settings: width 8192, quality 85, effort 7
- [ ] LANCZOS downscaling is used; UnsharpMask sharpening is applied when enabled
- [ ] All automated tests pass via `uv run pytest`
- [ ] `uv run ruff check` and `uv run ruff format --check` pass with no errors

## Implementation Steps

### Phase 1: Project Scaffolding

#### Step 1.1: Create `pyproject.toml` and project layout

- **Files**: `tools/texture-pipeline/pyproject.toml` (new), `tools/texture-pipeline/src/texture_pipeline/__init__.py` (new), `tools/texture-pipeline/.python-version` (new)
- **Action**: Create the uv project structure:
  - `pyproject.toml` with hatchling build backend, `requires-python = ">=3.14"`, dependencies on `pillow>=11.0`, `pillow-jxl-plugin>=1.3.7`, `typer>=0.12`, `rich>=13.0`. Dev dependencies: `pytest>=8.0`, `ruff>=0.11`. Define `[project.scripts] texture-pipeline = "texture_pipeline.main:app"`.
  - `src/texture_pipeline/__init__.py` empty file.
  - `.python-version` containing `3.14`.
- **Verify**: `uv sync` succeeds in `tools/texture-pipeline/` and creates `.venv/`. `uv run python -c "import texture_pipeline"` exits without error.
- **Complexity**: Small

#### Step 1.2: Update `.gitignore` for Python tooling

- **Files**: `.gitignore` (existing, root of repo)
- **Action**: Add entries for Python virtualenvs and caches, scoped to the tools directory:
  - `.venv/`
  - `__pycache__/`
  - `*.pyc`
- **Verify**: `git status` shows `.gitignore` modified. The `.venv/` directory created by `uv sync` is not tracked.
- **Complexity**: Small

#### Step 1.3: Run `uv sync` and verify lockfile

- **Files**: `tools/texture-pipeline/uv.lock` (generated)
- **Action**: Run `uv sync` to generate `uv.lock`. Confirm the lockfile is created and includes all four runtime dependencies plus their transitive deps.
- **Verify**: `uv.lock` exists, is non-empty, and contains entries for `pillow`, `pillow-jxl-plugin`, `typer`, and `rich`.
- **Complexity**: Small

### Phase 2: Core Image Processing Module

#### Step 2.1: Write tests for image processing functions (RED)

- **Files**: `tools/texture-pipeline/tests/test_processing.py` (new), `tools/texture-pipeline/tests/conftest.py` (new)
- **Action**: Write failing tests for the core processing functions. Create a conftest with fixtures that generate small synthetic test images (e.g., 64x32 solid-color JPEG, 128x64 PNG with a gradient).
- **Test cases**:
  - `test_downscale_maintains_aspect_ratio`: 128x64 source, target width 64 produces 64x32 output
  - `test_downscale_maintains_aspect_ratio_from_larger`: 256x128 source, target width 64 produces 64x32 output
  - `test_downscale_uses_lanczos`: confirm `Image.LANCZOS` is the resampling filter (mock or inspect call)
  - `test_downscale_rejects_non_2_to_1_aspect_ratio`: 100x100 source raises `ValueError`
  - `test_downscale_rejects_upscale`: target width 256 from 128-wide source raises `ValueError`
  - `test_sharpen_applies_unsharp_mask`: sharpened image differs from unsharpened (pixel comparison)
  - `test_sharpen_disabled_returns_identical`: with sharpening disabled, output matches input exactly
  - `test_encode_jxl_creates_file`: output file exists and has `.jxl` extension
  - `test_encode_jxl_respects_quality`: quality 50 produces smaller file than quality 95 (same source)
  - `test_encode_jxl_passes_lossless_jpeg_false`: when source is a JPEG loaded at original size, the output is NOT a lossless JPEG reconstruction (file size differs from lossless encode)
  - `test_encode_jxl_from_jpeg_source_at_original_size`: a JPEG saved to JXL at its original dimensions still applies lossy encoding (not silently lossless)
- **Verify**: All tests exist and fail (no implementation yet). `uv run pytest tests/test_processing.py` shows failures.
- **Complexity**: Medium

#### Step 2.2: Implement image processing functions (GREEN)

- **Files**: `tools/texture-pipeline/src/texture_pipeline/processing.py` (new)
- **Action**: Implement three functions:
  - `downscale(img: Image.Image, target_width: int) -> Image.Image`: Validate 2:1 aspect ratio and that target is not larger than source. Compute target height as `target_width // 2`. Use `img.resize((target_width, target_height), Image.LANCZOS)`.
  - `sharpen(img: Image.Image, radius: float = 1.0, percent: int = 80, threshold: int = 3) -> Image.Image`: Apply `ImageFilter.UnsharpMask(radius, percent, threshold)`. Return the filtered image.
  - `encode_jxl(img: Image.Image, output_path: Path, quality: int = 85, effort: int = 7) -> None`: Import `pillow_jxl` to register the plugin. Call `img.save(output_path, quality=quality, effort=effort, lossless_jpeg=False)`.
- **Verify**: All tests from Step 2.1 pass. `uv run pytest tests/test_processing.py` is green.
- **Complexity**: Medium

### Phase 3: File Discovery and Directory Structure

#### Step 3.1: Write tests for file discovery and output path logic (RED)

- **Files**: `tools/texture-pipeline/tests/test_discovery.py` (new)
- **Action**: Write failing tests for file discovery and output path construction. Use `tmp_path` fixture to create temporary directory trees.
- **Test cases**:
  - `test_discover_jpeg_files`: directory with `a.jpg`, `b.jpeg`, `c.txt` returns only JPEG files
  - `test_discover_png_files`: directory with `a.png` returns it
  - `test_discover_tiff_files`: directory with `a.tiff`, `b.tif` returns both
  - `test_discover_case_insensitive`: `A.JPG` and `b.Png` are both discovered
  - `test_discover_recursive`: files in subdirectories are found
  - `test_discover_empty_directory`: returns empty list (no error)
  - `test_output_path_preserves_structure`: input `src/land/earth.jpg` with input root `src/` and output root `out/` and width 4096 produces `out/4096/land/earth.jxl`
  - `test_output_path_replaces_extension`: `.jpg` becomes `.jxl`, `.png` becomes `.jxl`, `.tiff` becomes `.jxl`
  - `test_output_path_multiple_widths`: each width gets its own subdirectory under output root
- **Verify**: All tests exist and fail. `uv run pytest tests/test_discovery.py` shows failures.
- **Complexity**: Small

#### Step 3.2: Implement file discovery and output path logic (GREEN)

- **Files**: `tools/texture-pipeline/src/texture_pipeline/discovery.py` (new)
- **Action**: Implement:
  - `SUPPORTED_EXTENSIONS: set[str]` containing `.jpg`, `.jpeg`, `.png`, `.tif`, `.tiff`
  - `discover_images(input_dir: Path) -> list[Path]`: Recursively walk `input_dir`, collect files whose lowercased suffix is in `SUPPORTED_EXTENSIONS`, return sorted list.
  - `compute_output_path(source_path: Path, input_root: Path, output_root: Path, width: int) -> Path`: Compute the relative path from `input_root` to `source_path`, change extension to `.jxl`, join under `output_root / str(width) / relative_path`.
- **Verify**: All tests from Step 3.1 pass. `uv run pytest tests/test_discovery.py` is green.
- **Complexity**: Small

### Phase 4: CLI Interface

#### Step 4.1: Write tests for CLI argument parsing and validation (RED)

- **Files**: `tools/texture-pipeline/tests/test_cli.py` (new)
- **Action**: Write failing tests using `typer.testing.CliRunner` to test the CLI interface.
- **Test cases**:
  - `test_help_output`: `--help` exits 0 and contains "input", "output", "quality", "effort", "width"
  - `test_default_values`: invoking with only `--input` and `--output` uses width=8192, quality=85, effort=7
  - `test_multiple_widths`: `--width 4096 --width 2048` passes both widths to the pipeline
  - `test_invalid_quality_too_low`: quality=0 exits with error
  - `test_invalid_quality_too_high`: quality=101 exits with error
  - `test_invalid_effort_too_low`: effort=0 exits with error
  - `test_invalid_effort_too_high`: effort=10 exits with error
  - `test_invalid_width_odd`: width=4097 exits with error (not divisible produces non-integer height)
  - `test_nonexistent_input_dir`: exits with error message
  - `test_sharpening_flag_default_off`: sharpening is disabled by default
  - `test_sharpening_flag_enabled`: `--sharpen` enables sharpening
- **Verify**: All tests exist and fail. `uv run pytest tests/test_cli.py` shows failures.
- **Complexity**: Small

#### Step 4.2: Implement CLI with Typer (GREEN)

- **Files**: `tools/texture-pipeline/src/texture_pipeline/main.py` (new)
- **Action**: Implement the Typer application:
  - Create `app = typer.Typer()` with a helpful program name and description.
  - Define a `convert` command with parameters:
    - `--input` / `-i`: `Path`, required, must exist and be a directory
    - `--output` / `-o`: `Path`, required (created if it does not exist)
    - `--width` / `-w`: `list[int]`, default `[8192]`
    - `--quality` / `-q`: `int`, default 85, validated 1-100
    - `--effort` / `-e`: `int`, default 7, validated 1-9
    - `--sharpen`: `bool`, default `False`, flag to enable post-downscale UnsharpMask
  - Validate inputs (quality range, effort range, widths are even and positive).
  - Call discovery to find images, then for each (image, width) pair: load with Pillow, downscale, optionally sharpen, encode to JXL in the correct output path.
  - Use `rich.progress.Progress` to display a progress bar showing files processed.
  - Print a summary at the end (files processed, total input size, total output size, compression ratio).
- **Verify**: All tests from Step 4.1 pass. `uv run pytest tests/test_cli.py` is green.
- **Complexity**: Medium

### Phase 5: Integration Test and End-to-End Verification

#### Step 5.1: Write integration test (RED)

- **Files**: `tools/texture-pipeline/tests/test_integration.py` (new)
- **Action**: Write an end-to-end integration test that exercises the full pipeline through the CLI.
- **Test cases**:
  - `test_end_to_end_single_width`: Create a temp dir with a 128x64 synthetic JPEG. Run the CLI with `--width 64`. Verify:
    - Output directory contains `64/` subdirectory
    - The `.jxl` file exists inside it
    - The file is a valid image (can be reopened with Pillow+pillow_jxl)
    - Reopened image dimensions are 64x32
  - `test_end_to_end_multiple_widths`: Create a temp dir with a 256x128 synthetic PNG. Run with `--width 128 --width 64`. Verify:
    - Both `128/` and `64/` subdirectories exist
    - Both contain `.jxl` files with correct dimensions (128x64 and 64x32)
  - `test_end_to_end_preserves_subdirectory_structure`: Create `input/land/earth.jpg` and `input/ocean/water.png`. Run the CLI. Verify that `output/8192/land/earth.jxl` and `output/8192/ocean/water.jxl` would exist (use smaller widths matching source sizes for the test).
  - `test_end_to_end_with_sharpening`: Run with `--sharpen`, verify output file is produced (sharpened output differs from non-sharpened at pixel level).
  - `test_end_to_end_skip_non_image_files`: Place a `.txt` file in input; verify it is not in output.
  - `test_end_to_end_empty_input`: Empty input directory produces empty output directory, no crash.
- **Verify**: All tests exist and fail. `uv run pytest tests/test_integration.py` shows failures.
- **Complexity**: Medium

#### Step 5.2: Make integration tests pass (GREEN)

- **Files**: Any files from Phases 2-4 that need adjustment
- **Action**: Fix any issues found by the integration tests. This step may require minor adjustments to the processing, discovery, or CLI modules to handle edge cases revealed by end-to-end testing. Common issues: output directory creation, path separator handling on Windows, Pillow mode conversions (e.g., palette PNG to RGB before JXL encode).
- **Verify**: All integration tests pass. Full test suite is green: `uv run pytest` passes all tests.
- **Complexity**: Small

### Phase 6: Code Quality and Linting

#### Step 6.1: Add ruff configuration

- **Files**: `tools/texture-pipeline/pyproject.toml` (existing, add `[tool.ruff]` section)
- **Action**: Add ruff configuration to `pyproject.toml`:
  - `[tool.ruff]` with `target-version = "py314"`, `line-length = 88`
  - `[tool.ruff.lint]` with `select = ["E", "F", "W", "I", "UP", "B", "SIM", "RUF"]` (pyflakes, pycodestyle, isort, pyupgrade, bugbear, simplify, ruff-specific)
  - `[tool.ruff.lint.isort]` with `known-first-party = ["texture_pipeline"]`
- **Verify**: `uv run ruff check src/ tests/` passes with no errors. `uv run ruff format --check src/ tests/` passes.
- **Complexity**: Small

#### Step 6.2: Add pytest configuration

- **Files**: `tools/texture-pipeline/pyproject.toml` (existing, add `[tool.pytest.ini_options]` section)
- **Action**: Add pytest configuration:
  - `testpaths = ["tests"]`
  - `pythonpath = ["src"]`
- **Verify**: `uv run pytest` discovers and runs all tests from the `tests/` directory.
- **Complexity**: Small

### Phase 7: Manual End-to-End Verification

#### Step 7.1: Test with a real NASA Blue Marble texture

- **Files**: N/A (manual verification)
- **Action**: Download or use an existing NASA Blue Marble JPEG (e.g., the 4096x2048 file already in the project's `textures/` directory or `tmp/` directory). Run the full pipeline:

  ```bash
  cd tools/texture-pipeline
  uv run texture-pipeline convert \
      --input ../../tmp/texture-source/ \
      --output ../../tmp/texture-output/ \
      --quality 85 \
      --effort 7 \
      --width 4096 --width 2048 --width 1024
  ```

- **Manual test cases**:
  - Progress bar displays during processing and completes
  - Output directories `4096/`, `2048/`, `1024/` are created under the output path
  - Each contains a `.jxl` file with the correct dimensions (verify with an image viewer or `uv run python -c "from PIL import Image; import pillow_jxl; img = Image.open('path.jxl'); print(img.size)"`)
  - File sizes are reasonable (smaller than equivalent JPEG at similar quality)
  - Visual inspection: open the JXL files in a viewer; confirm no corruption, correct orientation (north-up, prime-meridian-left), and acceptable quality
- **Verify**: All manual checks pass.
- **Complexity**: Small

## Test Strategy

### Automated Tests

| Test Case | Type | Input | Expected Output |
|---|---|---|---|
| Downscale maintains 2:1 aspect ratio | Unit | 128x64 image, width=64 | 64x32 image |
| Downscale rejects non-2:1 source | Unit | 100x100 image | `ValueError` |
| Downscale rejects upscale request | Unit | 128x64 image, width=256 | `ValueError` |
| Sharpen modifies pixels | Unit | Synthetic image | Pixel values differ from input |
| Sharpen disabled is identity | Unit | Synthetic image, disabled | Identical pixel values |
| JXL encode creates valid file | Unit | Synthetic image | `.jxl` file on disk |
| JXL respects quality parameter | Unit | Same image, q=50 vs q=95 | q=50 file is smaller |
| JXL passes lossless_jpeg=False | Unit | JPEG source at original size | Output is lossy (not lossless reconstruction) |
| Discover finds JPEG/PNG/TIFF | Unit | Mixed file types | Only image files returned |
| Discover is case-insensitive | Unit | `.JPG`, `.Png` | Both found |
| Discover handles empty dir | Unit | Empty directory | Empty list |
| Output path preserves structure | Unit | Nested input paths | Correct nested output paths |
| Output path per width | Unit | Multiple widths | Separate subdirectories |
| CLI help text | Integration | `--help` | Exit 0, contains param names |
| CLI default values | Integration | Minimal args | width=8192, q=85, e=7 |
| CLI validates quality range | Integration | quality=0 | Error exit |
| CLI validates effort range | Integration | effort=10 | Error exit |
| End-to-end single width | Integration | 128x64 JPEG, width=64 | 64x32 JXL file |
| End-to-end multiple widths | Integration | 256x128 PNG, widths 128+64 | Two JXL files, correct sizes |
| End-to-end preserves subdirs | Integration | Nested input structure | Nested output structure |
| End-to-end skips non-images | Integration | `.txt` in input | Not in output |

### Manual Verification

- [ ] Run against a real 4096x2048 NASA Blue Marble JPEG; verify output quality visually
- [ ] Run with `--width 4096 --width 2048 --width 1024`; confirm all three output directories and files
- [ ] Confirm `uv sync` works from a clean state (delete `.venv/`, re-run)
- [ ] Confirm `uv run texture-pipeline --help` produces clean, readable output

## Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| pillow-jxl-plugin drops Python 3.14 support or changes API | Pipeline breaks on dependency update | Pin `>=1.3.7` (tested version), lock with `uv.lock`, re-test on upgrade |
| Large source images (21600x10800) exhaust RAM under Pillow | Process killed by OS | Document the memory requirement (~1.75 GB for full Blue Marble). This is acceptable for an offline tool. If needed later, add chunked processing. |
| JXL viewer availability for manual verification | Cannot visually verify output | Use Pillow itself to reopen and verify dimensions/mode; Windows 11 has native JXL support in Photos app |
| Windows path separator issues in tests | Tests fail on Windows | Use `pathlib.Path` throughout; avoid string path manipulation. Test on Windows (the development platform). |
| Typer version incompatibility with Python 3.14 | Import errors | Tested empirically in research; pin `>=0.12` which has 3.14 support |

## Rollback Strategy

Delete the `tools/texture-pipeline/` directory and revert the `.gitignore` additions. No Rust code is modified by this plan, so rollback has zero impact on the application.

## File Inventory

All files created by this plan (no existing files are modified except `.gitignore`):

```text
.gitignore                                              (modified: add Python ignores)
tools/texture-pipeline/
  pyproject.toml                                        (new: project metadata, deps, tool config)
  .python-version                                       (new: "3.14")
  uv.lock                                               (new: generated by uv sync)
  src/texture_pipeline/
    __init__.py                                         (new: empty package init)
    main.py                                             (new: Typer CLI app, convert command)
    processing.py                                       (new: downscale, sharpen, encode_jxl)
    discovery.py                                        (new: discover_images, compute_output_path)
  tests/
    conftest.py                                         (new: shared fixtures, synthetic images)
    test_processing.py                                  (new: unit tests for processing module)
    test_discovery.py                                   (new: unit tests for discovery module)
    test_cli.py                                         (new: CLI argument and validation tests)
    test_integration.py                                 (new: end-to-end pipeline tests)
```

## Status

- [x] Plan approved
- [x] Implementation started
- [x] Implementation complete
