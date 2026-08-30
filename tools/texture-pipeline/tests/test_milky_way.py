"""Tests for the Milky Way denoise pipeline."""

import numpy as np
import pytest
from conftest import synthetic_sky

from texture_pipeline.milky_way import (
    MilkyWayParams,
    arcmin_to_px,
    box_downsample,
    despeckle,
    flux_gain,
    linear_to_srgb,
    process,
    smooth,
    to_srgb8,
)

DESPECKLE_KWARGS = {
    "k": 3.0,
    "strong": 9.0,
    "eps": 0.0005,
    "bg_sigma_px": 6.0,
    "bg_iterations": 2,
    "dilate_px": 3.0,
    "fill_sigma_px": 3.0,
}

STAR_COLOR = np.array([1.5, 1.0, 0.5], dtype=np.float32)


def field_with_star(peak: float, sigma: float = 1.5, background: float = 0.01):
    """Build a flat field carrying one colored Gaussian star at column zero.

    :param peak: Peak luminance of the star above the background.
    :param sigma: Star width in pixels.
    :param background: The flat level the star sits on.
    :returns: The ``(256, 512, 3)`` float32 field and the star's row.
    """
    height, width = 256, 512
    row = height // 2
    rows = np.arange(height, dtype=np.float32)[:, None] - row
    cols = np.arange(width, dtype=np.float32)[None, :]
    cols = np.minimum(cols, width - cols)
    profile = peak * np.exp(-(rows**2 + cols**2) / (2 * sigma**2))
    field = np.full((height, width, 3), background, dtype=np.float32)
    return (field + profile[..., None] * STAR_COLOR).astype(np.float32), row


class TestFluxGain:
    def test_gain_at_each_published_width(self) -> None:
        assert flux_gain(16384) == pytest.approx(16.0)
        assert flux_gain(8192) == pytest.approx(4.0)
        assert flux_gain(4096) == pytest.approx(1.0)


class TestArcminToPx:
    def test_eight_k_pixel_scale(self) -> None:
        assert arcmin_to_px(15.8, 8192) == pytest.approx(5.99, abs=0.01)
        assert arcmin_to_px(7.9, 8192) == pytest.approx(3.0, abs=0.01)

    def test_a_full_turn_is_the_width(self) -> None:
        assert arcmin_to_px(360 * 60, 4096) == pytest.approx(4096.0)


class TestBoxDownsample:
    def test_block_means(self) -> None:
        a = np.array(
            [
                [0.0, 2.0, 10.0, 10.0],
                [4.0, 6.0, 10.0, 10.0],
                [1.0, 1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0, 8.0],
            ],
            dtype=np.float32,
        )
        expected = np.array([[3.0, 10.0], [1.0, 2.0]], dtype=np.float32)
        np.testing.assert_allclose(box_downsample(a, 2), expected)

    def test_three_channels_are_kept(self) -> None:
        a = np.ones((4, 8, 3), dtype=np.float32)
        assert box_downsample(a, 2).shape == (2, 4, 3)

    def test_factor_one_is_the_identity(self) -> None:
        a = np.arange(8, dtype=np.float32).reshape(2, 4)
        np.testing.assert_array_equal(box_downsample(a, 1), a)

    def test_non_dividing_factor_is_refused(self) -> None:
        a = np.ones((4, 8), dtype=np.float32)
        with pytest.raises(ValueError, match="does not divide"):
            box_downsample(a, 3)

    def test_factor_below_one_is_refused(self) -> None:
        a = np.ones((4, 8), dtype=np.float32)
        with pytest.raises(ValueError, match="at least 1"):
            box_downsample(a, 0)


class TestDespeckle:
    def test_a_strong_star_comes_down_to_the_background(self) -> None:
        field, row = field_with_star(peak=0.5)
        result = despeckle(field, **DESPECKLE_KWARGS)

        before = float(field[row, 0].mean())
        after = float(result.image[row, 0].mean())
        assert before / 0.01 > 50
        assert after == pytest.approx(0.01, rel=0.2)
        assert result.strong > 0.0

    def test_a_strong_stars_wings_are_brought_close_to_the_background(self) -> None:
        field, _ = field_with_star(peak=0.5)
        result = despeckle(field, **DESPECKLE_KWARGS)

        assert float(result.image.mean(axis=2).max()) < 0.025

    def test_a_strong_star_barely_moves_the_mean(self) -> None:
        field, _ = field_with_star(peak=0.5)
        result = despeckle(field, **DESPECKLE_KWARGS)

        before = float(field.mean())
        after = float(result.image.mean())
        assert abs(after - before) / before < 0.01

    def test_a_capped_pixel_keeps_its_channel_ratios(self) -> None:
        field, row = field_with_star(peak=0.05)
        result = despeckle(field, **DESPECKLE_KWARGS)

        assert result.capped > 0.0
        assert result.strong == 0.0
        pixel = result.image[row, 0]
        assert pixel[0] / pixel[1] == pytest.approx(field[row, 0][0] / field[row, 0][1])
        assert pixel[2] / pixel[1] == pytest.approx(field[row, 0][2] / field[row, 0][1])
        assert pixel.mean() < field[row, 0].mean()

    def test_a_smooth_gradient_is_left_alone(self) -> None:
        rows = np.arange(256, dtype=np.float32)[:, None]
        gradient = 0.01 + 0.02 * rows / 255
        field = np.repeat(np.broadcast_to(gradient, (256, 512))[..., None], 3, axis=2)
        field = np.ascontiguousarray(field, dtype=np.float32)

        result = despeckle(field, **DESPECKLE_KWARGS)

        np.testing.assert_allclose(result.image, field, atol=1e-6)
        assert result.capped == 0.0
        assert result.strong == 0.0


class TestSmooth:
    def test_a_bright_column_wraps_around(self) -> None:
        field = np.zeros((32, 64, 3), dtype=np.float32)
        field[:, 0] = 1.0

        blurred = smooth(field, 2.0)

        assert blurred[16, -1, 0] > 0.1
        assert blurred[16, -2, 0] > 0.01
        assert blurred[16, 32, 0] == pytest.approx(0.0, abs=1e-6)

    def test_rows_do_not_wrap(self) -> None:
        field = np.zeros((32, 64, 3), dtype=np.float32)
        field[0, :] = 1.0

        blurred = smooth(field, 2.0)

        assert blurred[-1, 32, 0] == pytest.approx(0.0, abs=1e-6)


class TestToSrgb8:
    def test_the_same_seed_gives_the_same_bytes(self) -> None:
        field = np.full((16, 32, 3), 0.004, dtype=np.float32)
        np.testing.assert_array_equal(to_srgb8(field, 7), to_srgb8(field, 7))

    def test_another_seed_gives_another_draw(self) -> None:
        field = np.full((16, 32, 3), 0.004, dtype=np.float32)
        assert not np.array_equal(to_srgb8(field, 7), to_srgb8(field, 8))

    def test_the_dither_preserves_the_mean(self) -> None:
        field = np.full((256, 256, 3), 0.002, dtype=np.float32)
        exact = float(linear_to_srgb(np.float32(0.002)) * 255.0)

        assert float(to_srgb8(field, 7).mean()) == pytest.approx(exact, abs=0.5)

    def test_the_dither_keeps_more_levels_than_plain_rounding(self) -> None:
        ramp = np.linspace(0.0, 0.01, 128, dtype=np.float32)[None, :, None]
        field = np.ascontiguousarray(np.broadcast_to(ramp, (64, 128, 3)))
        plain = np.clip(np.round(linear_to_srgb(field) * 255.0), 0, 255).astype(
            np.uint8
        )

        dithered = to_srgb8(field, 7)

        assert len(np.unique(dithered)) >= len(np.unique(plain))


class TestProcess:
    def test_it_halves_a_synthetic_sky(self) -> None:
        sky = synthetic_sky(512, 256)

        result = process(
            sky, source_width=512, target_width=256, params=MilkyWayParams()
        )

        assert result.image.shape == (128, 256, 3)
        assert result.image.dtype == np.uint8
        assert 0.0 <= result.capped <= 1.0
        assert 0.0 <= result.strong <= 1.0

    def test_upscaling_is_refused(self) -> None:
        sky = synthetic_sky(512, 256)
        with pytest.raises(ValueError, match="Upscaling is not supported"):
            process(sky, source_width=512, target_width=1024, params=MilkyWayParams())

    def test_a_non_integer_factor_is_refused(self) -> None:
        sky = synthetic_sky(512, 256)
        with pytest.raises(ValueError, match="not an integer multiple"):
            process(sky, source_width=512, target_width=192, params=MilkyWayParams())

    def test_a_non_2_to_1_source_is_refused(self) -> None:
        square = np.zeros((256, 256, 3), dtype=np.float32)
        with pytest.raises(ValueError, match="2:1 aspect ratio"):
            process(square, source_width=256, target_width=128, params=MilkyWayParams())

    def test_a_wrong_source_width_is_refused(self) -> None:
        sky = synthetic_sky(512, 256)
        with pytest.raises(ValueError, match="does not match the image width"):
            process(sky, source_width=8192, target_width=256, params=MilkyWayParams())

    def test_negative_values_do_not_reach_the_output(self) -> None:
        sky = synthetic_sky(512, 256)
        sky[10:20, 10:20] = -5.0

        result = process(
            sky, source_width=512, target_width=256, params=MilkyWayParams()
        )

        assert result.image.min() >= 0
