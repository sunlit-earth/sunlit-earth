"""Tests for the ocean masking module."""

from pathlib import Path

import numpy as np
from PIL import Image

from texture_pipeline.ocean_masking import (
    apply_coastal_buffer,
    apply_ocean_mask,
    clear_mask_cache,
    get_or_create_mask,
    rasterize_ocean_mask,
)


class TestRasterizeOceanMaskShape:
    def test_rasterize_ocean_mask_shape(self, western_half_shapefile: Path) -> None:
        mask = rasterize_ocean_mask(western_half_shapefile, width=100, height=50)
        assert mask.shape == (50, 100)

    def test_rasterize_ocean_mask_shape_large(
        self, western_half_shapefile: Path
    ) -> None:
        mask = rasterize_ocean_mask(western_half_shapefile, width=200, height=100)
        assert mask.shape == (100, 200)


class TestRasterizeOceanMaskDtype:
    def test_rasterize_ocean_mask_dtype(self, western_half_shapefile: Path) -> None:
        mask = rasterize_ocean_mask(western_half_shapefile, width=100, height=50)
        assert mask.dtype == np.uint8


class TestRasterizeOceanMaskBinary:
    def test_binary_at_supersample_1(self, western_half_shapefile: Path) -> None:
        mask = rasterize_ocean_mask(
            western_half_shapefile, width=100, height=50, supersample=1
        )
        unique_values = set(np.unique(mask))
        assert unique_values <= {0, 255}


class TestRasterizeOceanMaskAntialiased:
    def test_antialiased_at_supersample_2(
        self, western_half_shapefile: Path
    ) -> None:
        mask = rasterize_ocean_mask(
            western_half_shapefile, width=100, height=50, supersample=2
        )
        intermediate = (mask > 0) & (mask < 255)
        assert np.any(intermediate), "Expected some anti-aliased intermediate values"


class TestRasterizeOceanMaskCoverage:
    def test_coverage_western_half(self, western_half_shapefile: Path) -> None:
        mask = rasterize_ocean_mask(
            western_half_shapefile, width=100, height=50, supersample=1
        )
        ocean_fraction = np.count_nonzero(mask == 255) / mask.size
        assert 0.45 <= ocean_fraction <= 0.55, (
            f"Expected ~50% ocean, got {ocean_fraction:.2%}"
        )


class TestRasterizeOceanMaskFullOcean:
    def test_full_ocean(self, full_globe_shapefile: Path) -> None:
        mask = rasterize_ocean_mask(
            full_globe_shapefile, width=100, height=50, supersample=1
        )
        assert np.all(mask == 255)


class TestRasterizeOceanMaskEmpty:
    def test_mostly_empty(self, tiny_polygon_shapefile: Path) -> None:
        mask = rasterize_ocean_mask(
            tiny_polygon_shapefile, width=100, height=50, supersample=1
        )
        ocean_fraction = np.count_nonzero(mask == 255) / mask.size
        assert ocean_fraction < 0.05, (
            f"Expected mostly land, got {ocean_fraction:.2%} ocean"
        )


class TestApplyOceanMaskAllLand:
    def test_all_land_preserves_image(self) -> None:
        img = Image.new("RGB", (64, 32), color=(200, 100, 50))
        mask = np.zeros((32, 64), dtype=np.uint8)
        result = apply_ocean_mask(img, mask)
        assert np.array_equal(np.array(result), np.array(img))


class TestApplyOceanMaskAllOcean:
    def test_all_ocean_fills_color(self) -> None:
        img = Image.new("RGB", (64, 32), color=(200, 100, 50))
        mask = np.full((32, 64), 255, dtype=np.uint8)
        result = apply_ocean_mask(img, mask)
        result_arr = np.array(result)
        expected = np.array([10, 40, 80], dtype=np.uint8)
        assert np.all(result_arr == expected)


class TestApplyOceanMaskBlend128:
    def test_blend_midpoint(self) -> None:
        src_color = (200, 100, 50)
        fill_color = (10, 40, 80)
        img = Image.new("RGB", (64, 32), color=src_color)
        mask = np.full((32, 64), 128, dtype=np.uint8)
        result = apply_ocean_mask(img, mask, color=fill_color)
        result_arr = np.array(result)
        # Check that all pixels are roughly the midpoint
        for ch in range(3):
            expected_val = (src_color[ch] + fill_color[ch]) / 2
            actual_val = float(result_arr[0, 0, ch])
            assert abs(actual_val - expected_val) <= 2, (
                f"Channel {ch}: expected ~{expected_val}, got {actual_val}"
            )


class TestApplyOceanMaskBlendGradient:
    def test_gradient_mask_smooth_transition(self) -> None:
        img = Image.new("RGB", (256, 1), color=(200, 200, 200))
        mask = np.arange(256, dtype=np.uint8).reshape(1, 256)
        result = apply_ocean_mask(img, mask, color=(0, 0, 0))
        result_arr = np.array(result)
        # Red channel should decrease from left (land) to right (ocean)
        red_row = result_arr[0, :, 0].astype(float)
        assert red_row[0] > red_row[-1], "Expected decreasing values left to right"
        # Check monotonicity (allowing +-1 for rounding)
        diffs = np.diff(red_row)
        assert np.all(diffs <= 1), "Expected non-increasing red channel"


class TestApplyOceanMaskCustomColor:
    def test_custom_color(self) -> None:
        img = Image.new("RGB", (64, 32), color=(200, 100, 50))
        mask = np.full((32, 64), 255, dtype=np.uint8)
        result = apply_ocean_mask(img, mask, color=(0, 0, 255))
        result_arr = np.array(result)
        expected = np.array([0, 0, 255], dtype=np.uint8)
        assert np.all(result_arr == expected)


class TestApplyOceanMaskOutputType:
    def test_output_is_pil_image(self) -> None:
        img = Image.new("RGB", (64, 32), color=(200, 100, 50))
        mask = np.zeros((32, 64), dtype=np.uint8)
        result = apply_ocean_mask(img, mask)
        assert isinstance(result, Image.Image)
        assert result.mode == "RGB"


class TestApplyOceanMaskPreservesDimensions:
    def test_preserves_dimensions(self) -> None:
        img = Image.new("RGB", (128, 64), color=(200, 100, 50))
        mask = np.zeros((64, 128), dtype=np.uint8)
        result = apply_ocean_mask(img, mask)
        assert result.size == (128, 64)


def _make_binary_mask_with_edge(width: int, height: int, edge_col: int) -> np.ndarray:
    """Create a binary mask: 0 for columns < edge_col, 255 for columns >= edge_col."""
    mask = np.zeros((height, width), dtype=np.uint8)
    mask[:, edge_col:] = 255
    return mask


class TestCoastalBufferZeroIsIdentity:
    def test_zero_buffer_unchanged(self) -> None:
        mask = _make_binary_mask_with_edge(100, 50, 50)
        result = apply_coastal_buffer(mask, buffer_pixels=0)
        assert np.array_equal(result, mask)


class TestCoastalBufferExpandsTransition:
    def test_transition_zone_at_boundary(self) -> None:
        mask = _make_binary_mask_with_edge(100, 50, 50)
        result = apply_coastal_buffer(mask, buffer_pixels=5)
        # Pixels on the land side near the edge (columns 45-49) should have
        # intermediate values (not 0, not 255)
        land_near_edge = result[25, 45:50]
        assert np.any((land_near_edge > 0) & (land_near_edge < 255)), (
            f"Expected intermediate values near coastline, got {land_near_edge}"
        )


class TestCoastalBufferPreservesDeepOcean:
    def test_deep_ocean_stays_255(self) -> None:
        mask = _make_binary_mask_with_edge(100, 50, 50)
        result = apply_coastal_buffer(mask, buffer_pixels=5)
        # Far into ocean (column 90) should still be 255
        assert np.all(result[:, 90] == 255)


class TestCoastalBufferPreservesDeepLand:
    def test_deep_land_stays_0(self) -> None:
        mask = _make_binary_mask_with_edge(100, 50, 50)
        result = apply_coastal_buffer(mask, buffer_pixels=5)
        # Far into land (column 10) should still be 0
        assert np.all(result[:, 10] == 0)


class TestCoastalBufferMonotonicDecay:
    def test_monotonic_from_land_to_ocean(self) -> None:
        mask = _make_binary_mask_with_edge(100, 50, 50)
        result = apply_coastal_buffer(mask, buffer_pixels=5)
        # Take a horizontal transect through the middle row
        transect = result[25, :].astype(float)
        diffs = np.diff(transect)
        # Should be monotonically non-decreasing from land into ocean
        assert np.all(diffs >= 0), (
            f"Expected non-decreasing transect, found negative diffs at "
            f"{np.where(diffs < 0)[0]}"
        )


class TestCoastalBufferDtype:
    def test_output_dtype(self) -> None:
        mask = _make_binary_mask_with_edge(100, 50, 50)
        result = apply_coastal_buffer(mask, buffer_pixels=5)
        assert result.dtype == np.uint8


class TestMaskCacheReturnsSameObject:
    def test_cache_hit(self, western_half_shapefile: Path) -> None:
        clear_mask_cache()
        m1 = get_or_create_mask(western_half_shapefile, 100, 50)
        m2 = get_or_create_mask(western_half_shapefile, 100, 50)
        assert m1 is m2


class TestMaskCacheRecomputesForDifferentSize:
    def test_cache_miss_on_size(self, western_half_shapefile: Path) -> None:
        clear_mask_cache()
        m1 = get_or_create_mask(western_half_shapefile, 100, 50)
        m2 = get_or_create_mask(western_half_shapefile, 200, 100)
        assert m1.shape != m2.shape


class TestMaskCacheClear:
    def test_clear_forces_recompute(self, western_half_shapefile: Path) -> None:
        clear_mask_cache()
        m1 = get_or_create_mask(western_half_shapefile, 100, 50)
        clear_mask_cache()
        m2 = get_or_create_mask(western_half_shapefile, 100, 50)
        assert m1 is not m2
