"""Tests for shared image processing functions."""

import numpy as np
import pytest
from numpy.testing import assert_allclose

from cloud_fetch.processing import apply_blur, apply_levels, float_to_uint8


class TestApplyLevels:
    def test_identity_with_full_range_gamma_one(self) -> None:
        """floor=0, ceiling=255, gamma=1.0 is an identity transform."""
        data = np.array([0.0, 0.25, 0.5, 0.75, 1.0], dtype=np.float32)
        result = apply_levels(data, floor=0, ceiling=255, gamma=1.0)
        assert_allclose(result, data, atol=1e-6)

    def test_floor_clips_low_values(self) -> None:
        """Values below floor/255 are clipped to zero."""
        data = np.array([0.0, 0.1, 0.2, 0.5], dtype=np.float32)
        result = apply_levels(data, floor=51, ceiling=255, gamma=1.0)
        # floor=51 -> floor_f = 0.2, so 0.0 and 0.1 should become 0
        assert result[0] == 0.0
        assert result[1] == 0.0
        assert_allclose(result[2], 0.0, atol=1e-6)

    def test_ceiling_clips_high_values(self) -> None:
        """Values above ceiling/255 are clipped to one."""
        data = np.array([0.5, 0.8, 1.0], dtype=np.float32)
        result = apply_levels(data, floor=0, ceiling=128, gamma=1.0)
        # ceiling=128 -> ceiling_f ~= 0.502, so 0.8 and 1.0 should clip to 1.0
        assert_allclose(result[1], 1.0, atol=1e-6)
        assert_allclose(result[2], 1.0, atol=1e-6)

    def test_gamma_less_than_one_darkens(self) -> None:
        """gamma < 1 raises to exponent > 1, darkening midtones."""
        data = np.array([0.5], dtype=np.float32)
        result = apply_levels(data, floor=0, ceiling=255, gamma=0.5)
        # exponent = 1/0.5 = 2.0, so 0.5^2 = 0.25
        assert_allclose(result, [0.25], atol=1e-6)

    def test_gamma_greater_than_one_brightens(self) -> None:
        """gamma > 1 raises to exponent < 1, brightening midtones."""
        data = np.array([0.25], dtype=np.float32)
        result = apply_levels(data, floor=0, ceiling=255, gamma=2.0)
        # exponent = 1/2.0 = 0.5, so 0.25^0.5 = 0.5
        assert_allclose(result, [0.5], atol=1e-6)

    def test_output_always_in_zero_one(self) -> None:
        """Output is always clipped to [0, 1] regardless of inputs."""
        data = np.linspace(-0.5, 1.5, 100, dtype=np.float32)
        result = apply_levels(data, floor=30, ceiling=200, gamma=0.3)
        assert np.all(result >= 0.0)
        assert np.all(result <= 1.0)

    def test_floor_equals_ceiling_raises(self) -> None:
        """floor == ceiling raises ValueError (division by zero)."""
        data = np.array([0.5], dtype=np.float32)
        with pytest.raises(ValueError, match=r"floor.*must be less than ceiling"):
            apply_levels(data, floor=128, ceiling=128, gamma=1.0)

    def test_floor_greater_than_ceiling_raises(self) -> None:
        """floor > ceiling raises ValueError."""
        data = np.array([0.5], dtype=np.float32)
        with pytest.raises(ValueError, match=r"floor.*must be less than ceiling"):
            apply_levels(data, floor=200, ceiling=50, gamma=1.0)

    def test_2d_array(self) -> None:
        """Works with 2D arrays (images)."""
        data = np.random.default_rng(42).random((64, 64)).astype(np.float32)
        result = apply_levels(data, floor=50, ceiling=255, gamma=0.3)
        assert result.shape == (64, 64)
        assert np.all(result >= 0.0)
        assert np.all(result <= 1.0)


class TestApplyBlur:
    def test_no_op_at_zero_sigma(self) -> None:
        """sigma=0 returns the input unchanged."""
        data = np.eye(5, dtype=np.float32)
        result = apply_blur(data, sigma=0.0)
        assert_allclose(result, data)

    def test_no_op_at_negative_sigma(self) -> None:
        """Negative sigma returns the input unchanged."""
        data = np.eye(5, dtype=np.float32)
        result = apply_blur(data, sigma=-1.0)
        assert_allclose(result, data)

    def test_blur_spreads_single_pixel(self) -> None:
        """A single bright pixel is spread by the blur."""
        data = np.zeros((11, 11), dtype=np.float32)
        data[5, 5] = 1.0
        result = apply_blur(data, sigma=2.0)
        # Center should decrease, neighbors should increase
        assert result[5, 5] < 1.0
        assert result[5, 6] > 0.0
        assert result[6, 5] > 0.0

    def test_blur_preserves_total_energy(self) -> None:
        """Gaussian blur approximately preserves the sum of pixel values."""
        data = np.zeros((21, 21), dtype=np.float32)
        data[10, 10] = 1.0
        result = apply_blur(data, sigma=2.0)
        assert_allclose(result.sum(), data.sum(), atol=1e-4)


class TestFloatToUint8:
    def test_roundtrip(self) -> None:
        """Values at uint8 boundaries survive conversion."""
        data = np.array([0.0, 1.0], dtype=np.float32)
        result = float_to_uint8(data)
        assert result.dtype == np.uint8
        assert result[0] == 0
        assert result[1] == 255

    def test_midpoint(self) -> None:
        """0.5 maps to 127 or 128 (floor of 127.5)."""
        data = np.array([0.5], dtype=np.float32)
        result = float_to_uint8(data)
        assert result[0] in (127, 128)

    def test_clips_below_zero(self) -> None:
        """Negative values clip to 0."""
        data = np.array([-0.5], dtype=np.float32)
        result = float_to_uint8(data)
        assert result[0] == 0

    def test_clips_above_one(self) -> None:
        """Values above 1 clip to 255."""
        data = np.array([1.5], dtype=np.float32)
        result = float_to_uint8(data)
        assert result[0] == 255

    def test_preserves_shape(self) -> None:
        """Shape is preserved through conversion."""
        data = np.zeros((4, 8), dtype=np.float32)
        result = float_to_uint8(data)
        assert result.shape == (4, 8)
