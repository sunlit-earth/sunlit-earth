"""Tests for the Matteason cloud fetcher."""

import io
from pathlib import Path
from unittest.mock import MagicMock, patch

import numpy as np
from numpy.testing import assert_allclose
from PIL import Image

from cloud_fetch.matteason import decode_grayscale, fetch_matteason

_URLOPEN = "cloud_fetch.matteason.urllib.request.urlopen"


def _make_jpeg(width: int = 64, height: int = 32, value: int = 128) -> bytes:
    """Create a synthetic grayscale JPEG for testing."""
    img = Image.new("L", (width, height), value)
    buf = io.BytesIO()
    img.save(buf, format="JPEG", quality=95)
    return buf.getvalue()


class TestDecodeGrayscale:
    def test_shape_and_dtype(self) -> None:
        """Decoded array has correct shape and float32 dtype."""
        jpeg = _make_jpeg(64, 32)
        result = decode_grayscale(jpeg)
        assert result.shape == (32, 64)
        assert result.dtype == np.float32

    def test_value_range(self) -> None:
        """Decoded values are in [0, 1]."""
        jpeg = _make_jpeg(64, 32, value=200)
        result = decode_grayscale(jpeg)
        assert np.all(result >= 0.0)
        assert np.all(result <= 1.0)

    def test_white_image(self) -> None:
        """A white JPEG decodes to values near 1.0."""
        jpeg = _make_jpeg(16, 16, value=255)
        result = decode_grayscale(jpeg)
        assert_allclose(result, 1.0, atol=0.05)  # JPEG lossy tolerance

    def test_black_image(self) -> None:
        """A black JPEG decodes to values near 0.0."""
        jpeg = _make_jpeg(16, 16, value=0)
        result = decode_grayscale(jpeg)
        assert_allclose(result, 0.0, atol=0.05)

    def test_rgb_input_converted_to_grayscale(self) -> None:
        """An RGB JPEG is automatically converted to grayscale."""
        img = Image.new("RGB", (16, 16), (100, 100, 100))
        buf = io.BytesIO()
        img.save(buf, format="JPEG", quality=95)
        result = decode_grayscale(buf.getvalue())
        assert result.ndim == 2


class TestFetchMatteason:
    def _mock_urlopen(self, jpeg_data: bytes) -> MagicMock:
        """Create a mock for urllib.request.urlopen."""
        mock_resp = MagicMock()
        mock_resp.read.return_value = jpeg_data
        mock_resp.__enter__ = lambda s: s
        mock_resp.__exit__ = MagicMock(return_value=False)
        return mock_resp

    def test_basic_pipeline(self, tmp_path: Path) -> None:
        """Fetch pipeline produces an output PNG."""
        jpeg = _make_jpeg(64, 32)
        mock_resp = self._mock_urlopen(jpeg)

        with patch(_URLOPEN, return_value=mock_resp):
            result = fetch_matteason(tmp_path)

        assert result.exists()
        assert result.name == "clouds_matteason.png"
        img = Image.open(result)
        assert img.mode == "L"
        assert img.size == (64, 32)

    def test_raw_flag_saves_extra_file(self, tmp_path: Path) -> None:
        """raw=True saves both raw and processed images."""
        jpeg = _make_jpeg(64, 32)
        mock_resp = self._mock_urlopen(jpeg)

        with patch(_URLOPEN, return_value=mock_resp):
            fetch_matteason(tmp_path, raw=True)

        assert (tmp_path / "clouds_matteason.png").exists()
        assert (tmp_path / "clouds_matteason_raw.png").exists()

    def test_raw_flag_false_no_extra_file(self, tmp_path: Path) -> None:
        """raw=False does not save the raw image."""
        jpeg = _make_jpeg(64, 32)
        mock_resp = self._mock_urlopen(jpeg)

        with patch(_URLOPEN, return_value=mock_resp):
            fetch_matteason(tmp_path, raw=False)

        assert (tmp_path / "clouds_matteason.png").exists()
        assert not (tmp_path / "clouds_matteason_raw.png").exists()

    def test_creates_output_dir(self, tmp_path: Path) -> None:
        """Output directory is created if it doesn't exist."""
        jpeg = _make_jpeg(64, 32)
        mock_resp = self._mock_urlopen(jpeg)
        out = tmp_path / "sub" / "dir"

        with patch(_URLOPEN, return_value=mock_resp):
            result = fetch_matteason(out)

        assert out.is_dir()
        assert result.exists()

    def test_custom_levels(self, tmp_path: Path) -> None:
        """Custom levels parameters are applied."""
        jpeg = _make_jpeg(64, 32, value=128)
        mock_resp = self._mock_urlopen(jpeg)

        with patch(_URLOPEN, return_value=mock_resp):
            result = fetch_matteason(
                tmp_path, floor=0, ceiling=255, gamma=1.0, blur=0.0
            )

        assert result.exists()
        img = Image.open(result)
        arr = np.asarray(img)
        # With identity levels (floor=0, ceiling=255, gamma=1.0),
        # output should be close to the input value (~128)
        mean_val = arr.mean()
        assert 100 < mean_val < 160  # JPEG lossy tolerance
