"""End-to-end integration tests for the texture pipeline CLI."""

from pathlib import Path

import numpy as np
import pillow_jxl  # noqa: F401 - registers JXL plugin
from PIL import Image
from typer.testing import CliRunner

from texture_pipeline.main import app

runner = CliRunner()


def _create_synthetic_jpeg(path: Path, width: int, height: int) -> None:
    """Create a synthetic JPEG image at the given path.

    :param path: Destination file path.
    :param width: Image width in pixels.
    :param height: Image height in pixels.
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    img = Image.new("RGB", (width, height), color=(100, 150, 200))
    # Add some variation so compression has something to work with
    for x in range(0, width, 4):
        for y in range(0, height, 4):
            img.putpixel((x, y), (x % 256, y % 256, 128))
    img.save(path, format="JPEG", quality=95)


def _create_synthetic_png(path: Path, width: int, height: int) -> None:
    """Create a synthetic PNG image at the given path.

    :param path: Destination file path.
    :param width: Image width in pixels.
    :param height: Image height in pixels.
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    img = Image.new("RGB", (width, height), color=(50, 100, 200))
    for x in range(0, width, 4):
        for y in range(0, height, 4):
            img.putpixel((x, y), (x % 256, y % 256, 64))
    img.save(path, format="PNG")


class TestEndToEndSingleWidth:
    def test_end_to_end_single_width(self, tmp_path: Path) -> None:
        input_dir = tmp_path / "input"
        output_dir = tmp_path / "output"
        input_dir.mkdir()

        _create_synthetic_jpeg(input_dir / "earth.jpg", 128, 64)

        result = runner.invoke(
            app,
            [
                "convert",
                "--input",
                str(input_dir),
                "--output",
                str(output_dir),
                "--width",
                "64",
                "--quality",
                "85",
                "--effort",
                "1",
            ],
        )
        assert result.exit_code == 0, f"CLI failed: {result.output}"

        # Check output structure
        jxl_path = output_dir / "64" / "earth.jxl"
        assert jxl_path.exists(), f"Expected {jxl_path} to exist"

        # Verify the file is a valid image with correct dimensions
        reopened = Image.open(jxl_path)
        assert reopened.size == (64, 32)


class TestEndToEndMultipleWidths:
    def test_end_to_end_multiple_widths(self, tmp_path: Path) -> None:
        input_dir = tmp_path / "input"
        output_dir = tmp_path / "output"
        input_dir.mkdir()

        _create_synthetic_png(input_dir / "world.png", 256, 128)

        result = runner.invoke(
            app,
            [
                "convert",
                "--input",
                str(input_dir),
                "--output",
                str(output_dir),
                "--width",
                "128",
                "--width",
                "64",
                "--effort",
                "1",
            ],
        )
        assert result.exit_code == 0, f"CLI failed: {result.output}"

        # Check both width subdirectories
        jxl_128 = output_dir / "128" / "world.jxl"
        jxl_64 = output_dir / "64" / "world.jxl"
        assert jxl_128.exists()
        assert jxl_64.exists()

        # Verify dimensions
        assert Image.open(jxl_128).size == (128, 64)
        assert Image.open(jxl_64).size == (64, 32)


class TestEndToEndPreservesSubdirectoryStructure:
    def test_end_to_end_preserves_subdirectory_structure(self, tmp_path: Path) -> None:
        input_dir = tmp_path / "input"
        output_dir = tmp_path / "output"

        _create_synthetic_jpeg(input_dir / "land" / "earth.jpg", 128, 64)
        _create_synthetic_png(input_dir / "ocean" / "water.png", 128, 64)

        result = runner.invoke(
            app,
            [
                "convert",
                "--input",
                str(input_dir),
                "--output",
                str(output_dir),
                "--width",
                "64",
                "--effort",
                "1",
            ],
        )
        assert result.exit_code == 0, f"CLI failed: {result.output}"

        assert (output_dir / "64" / "land" / "earth.jxl").exists()
        assert (output_dir / "64" / "ocean" / "water.jxl").exists()


class TestEndToEndWithSharpening:
    def test_end_to_end_with_sharpening(self, tmp_path: Path) -> None:
        input_dir = tmp_path / "input"
        output_dir_sharp = tmp_path / "output_sharp"
        output_dir_nosharp = tmp_path / "output_nosharp"
        input_dir.mkdir()

        _create_synthetic_jpeg(input_dir / "earth.jpg", 128, 64)

        # Run without sharpening
        result1 = runner.invoke(
            app,
            [
                "convert",
                "--input",
                str(input_dir),
                "--output",
                str(output_dir_nosharp),
                "--width",
                "64",
                "--effort",
                "1",
            ],
        )
        assert result1.exit_code == 0

        # Run with sharpening
        result2 = runner.invoke(
            app,
            [
                "convert",
                "--input",
                str(input_dir),
                "--output",
                str(output_dir_sharp),
                "--width",
                "64",
                "--sharpen",
                "--effort",
                "1",
            ],
        )
        assert result2.exit_code == 0

        # Both should produce output
        nosharp_path = output_dir_nosharp / "64" / "earth.jxl"
        sharp_path = output_dir_sharp / "64" / "earth.jxl"
        assert nosharp_path.exists()
        assert sharp_path.exists()

        # File sizes should differ (sharpened image has different content)
        nosharp_size = nosharp_path.stat().st_size
        sharp_size = sharp_path.stat().st_size
        assert nosharp_size != sharp_size


class TestEndToEndSkipNonImageFiles:
    def test_end_to_end_skip_non_image_files(self, tmp_path: Path) -> None:
        input_dir = tmp_path / "input"
        output_dir = tmp_path / "output"
        input_dir.mkdir()

        _create_synthetic_jpeg(input_dir / "earth.jpg", 128, 64)
        (input_dir / "readme.txt").write_text("not an image")

        result = runner.invoke(
            app,
            [
                "convert",
                "--input",
                str(input_dir),
                "--output",
                str(output_dir),
                "--width",
                "64",
                "--effort",
                "1",
            ],
        )
        assert result.exit_code == 0

        # Only the JPEG should produce output
        assert (output_dir / "64" / "earth.jxl").exists()
        assert not (output_dir / "64" / "readme.txt").exists()
        assert not (output_dir / "64" / "readme.jxl").exists()


class TestEndToEndEmptyInput:
    def test_end_to_end_empty_input(self, tmp_path: Path) -> None:
        input_dir = tmp_path / "input"
        output_dir = tmp_path / "output"
        input_dir.mkdir()

        result = runner.invoke(
            app,
            [
                "convert",
                "--input",
                str(input_dir),
                "--output",
                str(output_dir),
                "--width",
                "64",
                "--effort",
                "1",
            ],
        )
        assert result.exit_code == 0
        assert "No supported image files" in result.output


class TestEndToEndOceanMask:
    def test_ocean_mask_replaces_pixels(
        self, tmp_path: Path, eastern_half_shapefile: Path
    ) -> None:
        input_dir = tmp_path / "input"
        output_dir = tmp_path / "output"
        input_dir.mkdir()

        # Solid green image — ocean masking should replace eastern half
        img = Image.new("RGB", (128, 64), color=(0, 200, 0))
        (input_dir / "earth.jpg").parent.mkdir(parents=True, exist_ok=True)
        img.save(input_dir / "earth.jpg", format="JPEG", quality=95)

        result = runner.invoke(
            app,
            [
                "convert",
                "--input", str(input_dir),
                "--output", str(output_dir),
                "--width", "64",
                "--effort", "1",
                "--ocean-mask", str(eastern_half_shapefile),
            ],
        )
        assert result.exit_code == 0, f"CLI failed: {result.output}"

        jxl_path = output_dir / "64" / "earth.jxl"
        assert jxl_path.exists()
        out = np.array(Image.open(jxl_path))

        # Right half (eastern ocean) should be close to default fill (10, 40, 80)
        right_quarter = out[:, -8:, :]
        for ch, expected in enumerate([10, 40, 80]):
            mean_val = right_quarter[:, :, ch].mean()
            assert abs(mean_val - expected) < 15, (
                f"Channel {ch}: expected ~{expected}, got {mean_val:.1f}"
            )

        # Left half (land) should still be greenish
        left_quarter = out[:, :8, :]
        assert left_quarter[:, :, 1].mean() > 100, "Expected green land pixels"

    def test_ocean_mask_custom_color(
        self, tmp_path: Path, eastern_half_shapefile: Path
    ) -> None:
        input_dir = tmp_path / "input"
        output_dir = tmp_path / "output"
        input_dir.mkdir()

        img = Image.new("RGB", (128, 64), color=(0, 200, 0))
        img.save(input_dir / "earth.jpg", format="JPEG", quality=95)

        result = runner.invoke(
            app,
            [
                "convert",
                "--input", str(input_dir),
                "--output", str(output_dir),
                "--width", "64",
                "--effort", "1",
                "--ocean-mask", str(eastern_half_shapefile),
                "--ocean-color", "0,0,255",
            ],
        )
        assert result.exit_code == 0, f"CLI failed: {result.output}"

        out = np.array(Image.open(output_dir / "64" / "earth.jxl"))
        right_quarter = out[:, -8:, :]
        # Should be blue
        assert right_quarter[:, :, 2].mean() > 200, "Expected blue ocean"

    def test_ocean_mask_with_buffer(
        self, tmp_path: Path, eastern_half_shapefile: Path
    ) -> None:
        input_dir = tmp_path / "input"
        output_dir = tmp_path / "output"
        input_dir.mkdir()

        img = Image.new("RGB", (128, 64), color=(0, 200, 0))
        img.save(input_dir / "earth.jpg", format="JPEG", quality=95)

        result = runner.invoke(
            app,
            [
                "convert",
                "--input", str(input_dir),
                "--output", str(output_dir),
                "--width", "64",
                "--effort", "1",
                "--ocean-mask", str(eastern_half_shapefile),
                "--ocean-coast-offset", "3",
            ],
        )
        assert result.exit_code == 0, f"CLI failed: {result.output}"

        out = np.array(Image.open(output_dir / "64" / "earth.jxl"))
        # The boundary region should have intermediate blended values
        mid_col = out.shape[1] // 2
        boundary = out[:, mid_col - 2 : mid_col + 2, :]
        # Not all pixels should be pure green or pure fill
        green_ch = boundary[:, :, 1].flatten()
        assert np.any((green_ch > 20) & (green_ch < 180)), (
            "Expected blended boundary pixels"
        )


class TestEndToEndWithoutOceanMaskUnchanged:
    def test_no_ocean_mask_produces_same_output(self, tmp_path: Path) -> None:
        input_dir = tmp_path / "input"
        output_a = tmp_path / "output_a"
        output_b = tmp_path / "output_b"
        input_dir.mkdir()

        _create_synthetic_jpeg(input_dir / "earth.jpg", 128, 64)

        # Run without ocean mask
        result1 = runner.invoke(
            app,
            [
                "convert",
                "--input", str(input_dir),
                "--output", str(output_a),
                "--width", "64",
                "--effort", "1",
            ],
        )
        assert result1.exit_code == 0

        # Run again without ocean mask
        result2 = runner.invoke(
            app,
            [
                "convert",
                "--input", str(input_dir),
                "--output", str(output_b),
                "--width", "64",
                "--effort", "1",
            ],
        )
        assert result2.exit_code == 0

        img_a = np.array(Image.open(output_a / "64" / "earth.jxl"))
        img_b = np.array(Image.open(output_b / "64" / "earth.jxl"))
        assert np.array_equal(img_a, img_b)


class TestEndToEndOceanMaskMultipleWidths:
    def test_multiple_widths_both_masked(
        self, tmp_path: Path, eastern_half_shapefile: Path
    ) -> None:
        input_dir = tmp_path / "input"
        output_dir = tmp_path / "output"
        input_dir.mkdir()

        img = Image.new("RGB", (128, 64), color=(0, 200, 0))
        img.save(input_dir / "earth.jpg", format="JPEG", quality=95)

        result = runner.invoke(
            app,
            [
                "convert",
                "--input", str(input_dir),
                "--output", str(output_dir),
                "--width", "64",
                "--width", "32",
                "--effort", "1",
                "--ocean-mask", str(eastern_half_shapefile),
            ],
        )
        assert result.exit_code == 0, f"CLI failed: {result.output}"

        for w in [64, 32]:
            jxl_path = output_dir / str(w) / "earth.jxl"
            assert jxl_path.exists()
            out = np.array(Image.open(jxl_path))
            # Right edge should be close to ocean fill color
            right_col = out[:, -1, :]
            assert right_col[:, 0].mean() < 50, (
                f"Width {w}: expected dark red channel in ocean"
            )


def _create_polar_ice_jpeg(
    path: Path, width: int, height: int
) -> None:
    """Create a JPEG with bright ice-like pixels in the top rows (polar zone).

    Equirectangular: row 0 = +90, top rows = high Arctic.
    Rows 0..height//6: bright white (240, 240, 240) on the right half,
    dark blue (10, 30, 65) on the left half.
    Remaining rows: uniform mid-green (0, 150, 0).
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    arr = np.zeros((height, width, 3), dtype=np.uint8)
    polar_rows = height // 6
    # Left half: dark ocean
    arr[:polar_rows, : width // 2] = [10, 30, 65]
    # Right half: bright ice
    arr[:polar_rows, width // 2 :] = [240, 240, 240]
    # Rest: green land
    arr[polar_rows:, :] = [0, 150, 0]
    Image.fromarray(arr).save(path, format="JPEG", quality=98)


class TestEndToEndIcePreservedByDefault:
    def test_ice_preserved(
        self, tmp_path: Path, full_globe_shapefile: Path
    ) -> None:
        input_dir = tmp_path / "input"
        output_dir = tmp_path / "output"
        input_dir.mkdir()

        _create_polar_ice_jpeg(input_dir / "earth.jpg", 128, 64)

        result = runner.invoke(
            app,
            [
                "convert",
                "--input", str(input_dir),
                "--output", str(output_dir),
                "--width", "64",
                "--effort", "1",
                "--ocean-mask", str(full_globe_shapefile),
            ],
        )
        assert result.exit_code == 0, f"CLI failed: {result.output}"

        out = np.array(Image.open(output_dir / "64" / "earth.jxl"))
        # Top-right area (polar ice) should retain high luminance
        polar_rows = out.shape[0] // 6
        ice_region = out[1 : max(2, polar_rows - 1), -8:, :]
        lum = (
            0.299 * ice_region[:, :, 0].astype(float)
            + 0.587 * ice_region[:, :, 1].astype(float)
            + 0.114 * ice_region[:, :, 2].astype(float)
        )
        assert lum.mean() > 100, (
            f"Expected bright ice preserved, got mean luminance {lum.mean():.1f}"
        )


class TestEndToEndIceDisabled:
    def test_ice_replaced_when_disabled(
        self, tmp_path: Path, full_globe_shapefile: Path
    ) -> None:
        input_dir = tmp_path / "input"
        output_dir = tmp_path / "output"
        input_dir.mkdir()

        _create_polar_ice_jpeg(input_dir / "earth.jpg", 128, 64)

        result = runner.invoke(
            app,
            [
                "convert",
                "--input", str(input_dir),
                "--output", str(output_dir),
                "--width", "64",
                "--effort", "1",
                "--ocean-mask", str(full_globe_shapefile),
                "--no-ocean-preserve-ice",
            ],
        )
        assert result.exit_code == 0, f"CLI failed: {result.output}"

        out = np.array(Image.open(output_dir / "64" / "earth.jxl"))
        # Top-right area should be replaced with fill color (low luminance)
        polar_rows = out.shape[0] // 6
        ice_region = out[1 : max(2, polar_rows - 1), -8:, :]
        lum = (
            0.299 * ice_region[:, :, 0].astype(float)
            + 0.587 * ice_region[:, :, 1].astype(float)
            + 0.114 * ice_region[:, :, 2].astype(float)
        )
        assert lum.mean() < 80, (
            f"Expected ice replaced with fill, got mean luminance {lum.mean():.1f}"
        )


class TestEndToEndTropicalBrightNotPreserved:
    def test_tropical_bright_replaced(
        self, tmp_path: Path, full_globe_shapefile: Path
    ) -> None:
        input_dir = tmp_path / "input"
        output_dir = tmp_path / "output"
        input_dir.mkdir()

        # Image with bright block only in the tropical zone (middle rows)
        arr = np.full((64, 128, 3), 10, dtype=np.uint8)
        arr[25:40, 80:120] = [240, 240, 240]  # bright tropical block
        Image.fromarray(arr).save(
            input_dir / "earth.jpg", format="JPEG", quality=98
        )

        result = runner.invoke(
            app,
            [
                "convert",
                "--input", str(input_dir),
                "--output", str(output_dir),
                "--width", "64",
                "--effort", "1",
                "--ocean-mask", str(full_globe_shapefile),
            ],
        )
        assert result.exit_code == 0, f"CLI failed: {result.output}"

        out = np.array(Image.open(output_dir / "64" / "earth.jxl"))
        # The tropical bright block should be replaced with fill color
        tropical_block = out[12:20, 40:60, :]
        lum = (
            0.299 * tropical_block[:, :, 0].astype(float)
            + 0.587 * tropical_block[:, :, 1].astype(float)
            + 0.114 * tropical_block[:, :, 2].astype(float)
        )
        assert lum.mean() < 80, (
            f"Expected tropical bright replaced, got luminance {lum.mean():.1f}"
        )
