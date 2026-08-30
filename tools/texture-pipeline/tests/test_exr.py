"""Tests for reading OpenEXR panoramas."""

from pathlib import Path

import numpy as np
import pytest
from conftest import write_exr

from texture_pipeline.exr import read_exr_rgb


class TestReadExrRgb:
    def test_round_trip_is_exact(self, rgb_exr_64x32: tuple[Path, np.ndarray]) -> None:
        path, expected = rgb_exr_64x32
        actual = read_exr_rgb(path)
        assert actual.shape == (32, 64, 3)
        assert actual.dtype == np.float32
        np.testing.assert_array_equal(actual, expected)

    def test_alpha_is_dropped(self, rgba_exr_64x32: Path) -> None:
        actual = read_exr_rgb(rgba_exr_64x32)
        assert actual.shape == (32, 64, 3)
        np.testing.assert_allclose(actual[..., 0], 0.25)
        np.testing.assert_allclose(actual[..., 2], 0.75)

    def test_half_float_source_reads_as_float32(self, sky_exr_512x256: Path) -> None:
        actual = read_exr_rgb(sky_exr_512x256)
        assert actual.shape == (256, 512, 3)
        assert actual.dtype == np.float32

    def test_missing_channel_is_refused(self, tmp_path: Path) -> None:
        plane = np.zeros((8, 16), dtype=np.float32)
        path = write_exr(tmp_path / "rg.exr", {"R": plane, "G": plane})
        with pytest.raises(ValueError, match="has no B channel"):
            read_exr_rgb(path)
