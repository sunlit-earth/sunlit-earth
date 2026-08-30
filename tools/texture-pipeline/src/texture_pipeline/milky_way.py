"""The Milky Way panorama pipeline: flux gain, despeckle, blur, 8-bit sRGB.

Every step works in linear light on float32 ``(H, W, 3)`` arrays. The lengths
that describe the filters are given in arcminutes and converted to pixels of
the target grid, so one set of parameters means the same thing on the sky at
any output width.

The source is a NASA SVS Deep Star Maps panorama, whose pixels hold flux
rather than radiance, so each published resolution has its own scale. Values
are lifted onto the 4096 grid's scale before anything else, because that is
the scale the renderer's brightness was tuned against.
"""

from dataclasses import dataclass
from typing import NamedTuple

import numpy as np
from scipy import ndimage

REFERENCE_WIDTH = 4096
"""The panorama width whose pixel scale the renderer was tuned against."""

ARCMIN_PER_TURN = 360 * 60

# Longitude is periodic and the poles are not, so rows and columns get
# different boundary handling. scipy orders these as (rows, columns).
_AXIS_MODES = ["nearest", "wrap"]

_LUMINANCE_FLOOR = 1e-9
_FILL_WEIGHT_FLOOR = 1e-3


@dataclass(frozen=True)
class MilkyWayParams:
    """The denoise thresholds and lengths, lengths in arcminutes.

    :ivar k: Luminance is capped at this multiple of the local background.
    :ivar strong: Above this multiple a star's whole footprint is replaced.
    :ivar eps: Absolute margin, so a near-black sky is not all stars.
    :ivar bg_sigma: Gaussian that estimates the local background.
    :ivar bg_iterations: Times the background is re-estimated from the capped
        image, so the stars do not lift it.
    :ivar dilate: Radius grown around a strong star for its rendered wings.
    :ivar fill_sigma: Normalized convolution that fills a strong star's hole.
    :ivar blur: The final blur.
    :ivar seed: Seed of the dither the 8-bit quantization draws from.
    """

    k: float = 3.0
    strong: float = 9.0
    eps: float = 0.0005
    bg_sigma: float = 15.8
    bg_iterations: int = 2
    dilate: float = 7.9
    fill_sigma: float = 7.9
    blur: float = 7.9
    seed: int = 7


class DespeckleResult(NamedTuple):
    """What :func:`despeckle` produced and how much of the frame it touched.

    :ivar image: The despeckled ``(H, W, 3)`` float32 array.
    :ivar capped: Fraction of pixels whose luminance was trimmed.
    :ivar strong: Fraction of pixels replaced as part of a strong star.
    """

    image: np.ndarray
    capped: float
    strong: float


class ProcessResult(NamedTuple):
    """The finished panorama and the same two fractions.

    :ivar image: The ``(H, W, 3)`` uint8 sRGB array.
    :ivar capped: Fraction of pixels whose luminance was trimmed.
    :ivar strong: Fraction of pixels replaced as part of a strong star.
    """

    image: np.ndarray
    capped: float
    strong: float


def flux_gain(source_width: int, reference_width: int = REFERENCE_WIDTH) -> float:
    """Return the factor that lifts a source's flux onto the reference grid.

    A pixel of a panorama twice as wide covers a quarter of the sky, and the
    source holds flux per pixel, so the factor is the square of the ratio.

    :param source_width: Width of the source panorama in pixels.
    :param reference_width: Width whose scale the output must land on.
    :returns: The multiplier to apply before anything else.
    """
    return (source_width / reference_width) ** 2


def arcmin_to_px(arcmin: float, width: int) -> float:
    """Convert a length on the sky to pixels of an equirectangular grid.

    :param arcmin: The length in arcminutes.
    :param width: Grid width in pixels, covering the full 360 degrees.
    :returns: The same length in pixels.
    """
    return arcmin * width / ARCMIN_PER_TURN


def box_downsample(a: np.ndarray, factor: int) -> np.ndarray:
    """Average an image over square blocks.

    :param a: An array whose first two axes are rows and columns.
    :param factor: Side of the block, in pixels.
    :returns: A float32 array with both spatial axes divided by ``factor``.
    :raises ValueError: If ``factor`` is below 1.
    :raises ValueError: If either spatial axis is not a multiple of ``factor``.
    """
    if factor < 1:
        msg = f"Box factor must be at least 1, got {factor}"
        raise ValueError(msg)

    height, width = a.shape[:2]
    if height % factor or width % factor:
        msg = (
            f"Box factor {factor} does not divide the image size "
            f"{width}x{height} evenly"
        )
        raise ValueError(msg)

    if factor == 1:
        return a.astype(np.float32, copy=False)

    blocks = a.reshape(height // factor, factor, width // factor, factor, *a.shape[2:])
    return blocks.mean(axis=(1, 3), dtype=np.float32)


def linear_to_srgb(a: np.ndarray) -> np.ndarray:
    """Apply the sRGB transfer curve to linear values already in [0, 1].

    :param a: Linear values.
    :returns: The encoded values, same shape and dtype.
    """
    return np.where(a <= 0.0031308, a * 12.92, 1.055 * np.power(a, 1 / 2.4) - 0.055)


def estimate_background(
    lum: np.ndarray,
    sigma_px: float,
    k: float,
    eps: float,
    iterations: int,
) -> np.ndarray:
    """Estimate the local background of a luminance plane.

    The first estimate is a plain Gaussian mean, which the stars sitting on
    the sky lift. Each further pass takes the mean of the image capped against
    the previous estimate, which they do not.

    :param lum: The luminance plane.
    :param sigma_px: Gaussian sigma in pixels.
    :param k: Multiple of the estimate at which the image is capped.
    :param eps: Absolute margin added to the cap.
    :param iterations: Number of re-estimation passes.
    :returns: The background, same shape as ``lum``.
    """
    background = ndimage.gaussian_filter(lum, sigma_px, mode=_AXIS_MODES)
    for _ in range(iterations):
        capped = np.minimum(lum, k * background + eps)
        background = ndimage.gaussian_filter(capped, sigma_px, mode=_AXIS_MODES)
    return background


def _dilate(mask: np.ndarray, iterations: int) -> np.ndarray:
    """Grow a boolean mask, wrapping across columns and holding rows.

    :param mask: The mask to grow.
    :param iterations: Radius in pixels. Below 1 the mask is returned as is.
    :returns: The grown mask.
    """
    if iterations < 1:
        return mask

    padded = np.pad(mask, ((iterations, iterations), (0, 0)), mode="edge")
    padded = np.pad(padded, ((0, 0), (iterations, iterations)), mode="wrap")
    grown = ndimage.binary_dilation(padded, iterations=iterations)
    return grown[iterations:-iterations, iterations:-iterations]


def despeckle(
    a: np.ndarray,
    *,
    k: float,
    strong: float,
    eps: float,
    bg_sigma_px: float,
    bg_iterations: int,
    dilate_px: float,
    fill_sigma_px: float,
) -> DespeckleResult:
    """Cap what stands above the local background and remove strong stars.

    Capping scales all three channels of a pixel by one factor, so a trimmed
    pixel keeps its color. A strong star's rendered point spread function has
    wings below the cap over several pixels, which a blur would turn into a
    soft blob, so its footprint is grown and then filled by normalized
    convolution from the pixels around it.

    :param a: A linear ``(H, W, 3)`` float32 image.
    :param k: Multiple of the background at which luminance is capped.
    :param strong: Multiple of the background above which a star is replaced.
    :param eps: Absolute margin added to both thresholds.
    :param bg_sigma_px: Background Gaussian sigma in pixels.
    :param bg_iterations: Background re-estimation passes.
    :param dilate_px: Radius grown around a strong star, in pixels.
    :param fill_sigma_px: Sigma of the convolution that fills the holes.
    :returns: The image and the fractions of pixels capped and replaced.
    """
    lum = a.mean(axis=2)
    background = estimate_background(lum, bg_sigma_px, k, eps, bg_iterations)

    cap = k * background + eps
    scale = np.minimum(1.0, cap / np.maximum(lum, _LUMINANCE_FLOOR)).astype(np.float32)
    out = a * scale[..., None]
    capped = scale < 1.0

    footprint = _dilate(lum > strong * background + eps, round(dilate_px))
    keep = (~footprint).astype(np.float32)
    weight = ndimage.gaussian_filter(keep, fill_sigma_px, mode=_AXIS_MODES)
    for index in range(out.shape[2]):
        lit = ndimage.gaussian_filter(
            out[..., index] * keep, fill_sigma_px, mode=_AXIS_MODES
        )
        fill = lit / np.maximum(weight, _FILL_WEIGHT_FLOOR)
        out[..., index][footprint] = fill[footprint]

    return DespeckleResult(out, float(capped.mean()), float(footprint.mean()))


def smooth(a: np.ndarray, sigma_px: float) -> np.ndarray:
    """Blur each channel of an image with a Gaussian.

    :param a: An ``(H, W, C)`` float32 image.
    :param sigma_px: Gaussian sigma in pixels.
    :returns: The blurred image, same shape and dtype.
    """
    out = np.empty_like(a)
    for index in range(a.shape[2]):
        ndimage.gaussian_filter(
            a[..., index], sigma_px, output=out[..., index], mode=_AXIS_MODES
        )
    return out


def to_srgb8(a: np.ndarray, seed: int) -> np.ndarray:
    """Encode a linear image as 8-bit sRGB with a triangular dither.

    The dither is a sum of two uniform draws minus one, so it has mean zero
    and a width of one code value. Without it the smoothed dark sky, which is
    most of the panorama, posterizes into blotches.

    :param a: A linear ``(H, W, C)`` float32 image.
    :param seed: Seed of the random generator the dither is drawn from.
    :returns: The encoded image as uint8.
    """
    encoded = linear_to_srgb(np.clip(a, 0.0, 1.0)) * 255.0
    rng = np.random.default_rng(seed)
    dithered = (
        encoded
        + rng.random(encoded.shape, dtype=np.float32)
        + rng.random(encoded.shape, dtype=np.float32)
        - 1.0
    )
    return np.clip(np.round(dithered), 0, 255).astype(np.uint8)


def process(
    a: np.ndarray,
    *,
    source_width: int,
    target_width: int,
    params: MilkyWayParams,
) -> ProcessResult:
    """Run the whole pipeline, from a linear source panorama to 8-bit sRGB.

    :param a: The source as a linear ``(H, W, 3)`` float32 array.
    :param source_width: The source width, which sets the flux gain.
    :param target_width: The output width. Must divide the source width.
    :param params: The denoise thresholds and lengths.
    :returns: The uint8 panorama and the fractions of pixels capped and
        replaced by the despeckle.
    :raises ValueError: If the source is not 2:1, if ``source_width`` is not
        its width, if the target is wider than the source, or if the target
        does not divide the source.
    """
    height, width = a.shape[:2]
    if width != 2 * height:
        msg = f"The panorama must have a 2:1 aspect ratio, got {width}x{height}"
        raise ValueError(msg)
    if width != source_width:
        msg = f"Source width {source_width} does not match the image width {width}"
        raise ValueError(msg)
    if target_width > source_width:
        msg = (
            f"Upscaling is not supported: target width {target_width} "
            f"exceeds source width {source_width}"
        )
        raise ValueError(msg)
    if source_width % target_width:
        msg = (
            f"Source width {source_width} is not an integer multiple of "
            f"target width {target_width}"
        )
        raise ValueError(msg)

    linear = np.clip(a, 0.0, None)
    linear *= flux_gain(source_width)
    linear = box_downsample(linear, source_width // target_width)

    despeckled = despeckle(
        linear,
        k=params.k,
        strong=params.strong,
        eps=params.eps,
        bg_sigma_px=arcmin_to_px(params.bg_sigma, target_width),
        bg_iterations=params.bg_iterations,
        dilate_px=arcmin_to_px(params.dilate, target_width),
        fill_sigma_px=arcmin_to_px(params.fill_sigma, target_width),
    )
    blurred = smooth(despeckled.image, arcmin_to_px(params.blur, target_width))
    return ProcessResult(
        to_srgb8(blurred, params.seed), despeckled.capped, despeckled.strong
    )
