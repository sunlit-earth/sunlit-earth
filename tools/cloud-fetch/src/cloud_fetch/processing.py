"""Shared image processing functions for cloud imagery."""

import numpy as np
from scipy.ndimage import gaussian_filter


def apply_levels(
    data: np.ndarray, floor: int, ceiling: int, gamma: float
) -> np.ndarray:
    """Apply Photoshop-style Levels adjustment.

    Remaps input values from ``[floor/255, ceiling/255]`` to ``[0, 1]``,
    clips, and raises to the power ``1/gamma``.

    :param data: 0.0--1.0 float array.
    :param floor: Input black point (0--255).
    :param ceiling: Input white point (0--255).
    :param gamma: Photoshop midtone slider (< 1 darkens midtones).
    :returns: 0.0--1.0 float array with levels applied.
    """
    if floor >= ceiling:
        raise ValueError(f"floor ({floor}) must be less than ceiling ({ceiling})")

    floor_f = floor / 255.0
    ceiling_f = ceiling / 255.0

    result = (data - floor_f) / (ceiling_f - floor_f)
    result = np.clip(result, 0.0, 1.0)

    exponent = 1.0 / gamma
    result = np.power(result, exponent)

    return result


def apply_blur(data: np.ndarray, sigma: float) -> np.ndarray:
    """Apply Gaussian blur to a 2D array.

    No-op when *sigma* is zero or negative.

    :param data: 2D float array.
    :param sigma: Gaussian blur sigma in pixels.
    :returns: Blurred array (same shape and dtype).
    """
    if sigma <= 0:
        return data
    return gaussian_filter(data, sigma=sigma)


def float_to_uint8(data: np.ndarray) -> np.ndarray:
    """Convert a [0, 1] float array to [0, 255] uint8.

    Values are clipped before conversion.

    :param data: Float array with values nominally in [0, 1].
    :returns: uint8 array.
    """
    return (np.clip(data, 0.0, 1.0) * 255.0).astype(np.uint8)
