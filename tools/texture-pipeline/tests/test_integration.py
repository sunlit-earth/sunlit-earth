"""End-to-end integration tests for the texture pipeline CLI."""

from pathlib import Path

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
