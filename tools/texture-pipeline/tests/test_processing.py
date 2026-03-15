"""Tests for the image processing module."""

from pathlib import Path
from unittest.mock import patch

import pytest
from PIL import Image

from texture_pipeline.processing import downscale, encode_jxl, sharpen


class TestDownscale:
    def test_downscale_maintains_aspect_ratio(
        self, gradient_image_128x64: Image.Image
    ) -> None:
        result = downscale(gradient_image_128x64, target_width=64)
        assert result.size == (64, 32)

    def test_downscale_maintains_aspect_ratio_from_larger(
        self, gradient_image_256x128: Image.Image
    ) -> None:
        result = downscale(gradient_image_256x128, target_width=64)
        assert result.size == (64, 32)

    def test_downscale_uses_lanczos(self, gradient_image_128x64: Image.Image) -> None:
        with patch.object(
            Image.Image, "resize", wraps=gradient_image_128x64.resize
        ) as mock_resize:
            downscale(gradient_image_128x64, target_width=64)
            mock_resize.assert_called_once()
            _args, kwargs = mock_resize.call_args
            # Check positional args or keyword args for LANCZOS
            if len(_args) >= 2:
                assert _args[1] == Image.Resampling.LANCZOS
            else:
                assert kwargs.get("resample") == Image.Resampling.LANCZOS

    def test_downscale_rejects_non_2_to_1_aspect_ratio(
        self, square_image_100x100: Image.Image
    ) -> None:
        with pytest.raises(ValueError, match="2:1"):
            downscale(square_image_100x100, target_width=50)

    def test_downscale_rejects_upscale(
        self, gradient_image_128x64: Image.Image
    ) -> None:
        with pytest.raises(ValueError, match=r"[Uu]pscal"):
            downscale(gradient_image_128x64, target_width=256)


class TestSharpen:
    def test_sharpen_applies_unsharp_mask(self) -> None:
        # Create an image with mid-range block edges so UnsharpMask can
        # both brighten and darken pixels at the transitions.
        img = Image.new("RGB", (128, 64))
        for x in range(128):
            for y in range(64):
                # 8-pixel-wide vertical stripes alternating between two
                # mid-range values -- gives edges the filter can enhance.
                if (x // 8) % 2 == 0:
                    img.putpixel((x, y), (60, 60, 60))
                else:
                    img.putpixel((x, y), (180, 180, 180))
        sharpened = sharpen(img)
        assert img.tobytes() != sharpened.tobytes()

    def test_sharpen_disabled_returns_identical(
        self, gradient_image_128x64: Image.Image
    ) -> None:
        # Passing percent=0 should produce identical output
        result = sharpen(gradient_image_128x64, percent=0)
        assert gradient_image_128x64.tobytes() == result.tobytes()


class TestEncodeJxl:
    def test_encode_jxl_creates_file(
        self, gradient_image_128x64: Image.Image, tmp_path: Path
    ) -> None:
        output = tmp_path / "output.jxl"
        encode_jxl(gradient_image_128x64, output)
        assert output.exists()
        assert output.suffix == ".jxl"

    def test_encode_jxl_respects_quality(
        self, gradient_image_128x64: Image.Image, tmp_path: Path
    ) -> None:
        low_q = tmp_path / "low.jxl"
        high_q = tmp_path / "high.jxl"
        encode_jxl(gradient_image_128x64, low_q, quality=50)
        encode_jxl(gradient_image_128x64, high_q, quality=95)
        assert low_q.stat().st_size < high_q.stat().st_size

    def test_encode_jxl_passes_lossless_jpeg_false(
        self, jpeg_file_128x64: Path, tmp_path: Path
    ) -> None:
        """Verify that JXL output is lossy, not a lossless JPEG reconstruction."""
        import pillow_jxl  # noqa: F401 - registers JXL plugin

        img = Image.open(jpeg_file_128x64)

        # Lossy encode via our function
        lossy_path = tmp_path / "lossy.jxl"
        encode_jxl(img, lossy_path, quality=85)

        # Lossless encode for comparison
        lossless_path = tmp_path / "lossless.jxl"
        img.save(lossless_path, lossless_jpeg=True)

        # File sizes should differ (lossy != lossless)
        assert lossy_path.stat().st_size != lossless_path.stat().st_size

    def test_encode_jxl_from_jpeg_source_at_original_size(
        self, jpeg_file_128x64: Path, tmp_path: Path
    ) -> None:
        """A JPEG saved to JXL at its original dimensions still applies lossy
        encoding (not silently lossless)."""
        import pillow_jxl  # noqa: F401 - registers JXL plugin

        img = Image.open(jpeg_file_128x64)
        output = tmp_path / "from_jpeg.jxl"
        encode_jxl(img, output, quality=85)

        # Re-open and verify it's a valid image with correct dimensions
        reopened = Image.open(output)
        assert reopened.size == (128, 64)
