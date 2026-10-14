"""Ocean masking: rasterize shapefiles to pixel masks and apply to images."""

from __future__ import annotations

import os
from pathlib import Path

import geopandas as gpd
import numpy as np
from PIL import Image
from rasterio.features import rasterize
from rasterio.transform import from_bounds


def rasterize_ocean_mask(
    shapefile_path: Path,
    width: int,
    height: int,
    supersample: int = 2,
) -> np.ndarray:
    """Rasterize an ocean shapefile into a pixel mask.

    :param shapefile_path: Path to the .shp file (must be in EPSG:4326).
    :param width: Output mask width in pixels.
    :param height: Output mask height in pixels.
    :param supersample: Supersampling factor for anti-aliased edges.
    :returns: uint8 array of shape (height, width). 0 = land, 255 = ocean,
        intermediate values at anti-aliased edges when supersample > 1.
    """
    os.environ["GDAL_CACHEMAX"] = "512"

    gdf = gpd.read_file(shapefile_path)
    if gdf.crs is None or gdf.crs.to_epsg() != 4326:
        gdf = gdf.to_crs(epsg=4326)

    ss_w = width * supersample
    ss_h = height * supersample
    transform = from_bounds(-180, -90, 180, 90, ss_w, ss_h)

    shapes = ((geom, 255) for geom in gdf.geometry)
    mask = rasterize(
        shapes,
        out_shape=(ss_h, ss_w),
        fill=0,
        dtype="uint8",
        transform=transform,
        all_touched=False,
    )

    if supersample > 1:
        img = Image.fromarray(mask, mode="L")
        img = img.resize((width, height), resample=Image.Resampling.LANCZOS)
        mask = np.array(img, dtype=np.uint8)

    return mask


def apply_ocean_mask(
    image: Image.Image,
    mask: np.ndarray,
    color: tuple[int, int, int] = (10, 30, 60),
) -> Image.Image:
    """Replace ocean pixels in an image with a uniform fill color.

    Memory-optimized: directly assigns fill color for fully-ocean pixels
    (mask=255) and fully preserves land pixels (mask=0) using uint8
    operations. Float blending is only used for the small fraction of
    anti-aliased edge pixels with intermediate mask values.

    :param image: Source RGB image.
    :param mask: uint8 array of shape (H, W). 0 = land, 255 = ocean.
    :param color: RGB fill color for ocean regions.
    :returns: New RGB image with ocean pixels replaced.
    """
    result = np.array(image)  # uint8 copy

    # Fully ocean pixels: direct fill (no float math needed)
    ocean = mask == 255
    result[ocean] = color

    # Anti-aliased edge pixels: float blend (typically <0.1% of pixels)
    edges = (mask > 0) & (mask < 255)
    if np.any(edges):
        alpha = mask[edges].astype(np.float32) / 255.0
        src_px = result[edges].astype(np.float32)
        fill = np.array(color, dtype=np.float32)
        blended = src_px * (1.0 - alpha[:, np.newaxis]) + fill * alpha[:, np.newaxis]
        result[edges] = blended.clip(0, 255).astype(np.uint8)

    return Image.fromarray(result, mode="RGB")


def apply_coast_offset(
    mask: np.ndarray,
    offset_pixels: int,
) -> np.ndarray:
    """Shift the coastline boundary by the given pixel offset.

    Positive values expand the ocean into land (Gaussian blur transition).
    Negative values erode the ocean away from the coast (pull fill color
    back, preserving more of the original texture at the shoreline).
    Zero is a no-op.

    :param mask: uint8 array of shape (H, W). 0 = land, 255 = ocean.
    :param offset_pixels: Signed coastline shift in pixels.
    :returns: uint8 mask with the same shape.
    """
    if offset_pixels == 0:
        return mask

    from PIL import ImageFilter

    if offset_pixels > 0:
        # Expand ocean into land via Gaussian blur
        img = Image.fromarray(mask, mode="L")
        blurred = img.filter(ImageFilter.GaussianBlur(radius=offset_pixels))
        blurred_arr = np.array(blurred, dtype=np.uint8)
        # Preserve deep-ocean pixels at 255
        result = np.where(mask == 255, np.uint8(255), blurred_arr)
        return result.astype(np.uint8)

    # Negative: erode the ocean boundary inward so the fill color pulls
    # back from the coast. The eroded zone stays at 255 (full fill) —
    # we never reveal original ocean pixels, which would create a visible
    # dark band between land and fill. Only the new boundary gets a soft
    # Gaussian gradient toward land.
    radius = abs(offset_pixels)
    img = Image.fromarray(mask, mode="L")
    # MinFilter kernel must be odd and >= 3
    kernel = max(3, 2 * radius + 1)
    eroded = img.filter(ImageFilter.MinFilter(size=kernel))
    # Blur the new (inward-shifted) boundary for a soft transition
    eroded = eroded.filter(ImageFilter.GaussianBlur(radius=radius))
    eroded_arr = np.array(eroded, dtype=np.uint8)
    # Take the minimum of original and eroded: the eroded mask is
    # smaller, so min() keeps the eroded boundary. But where the
    # eroded mask is 255 (deep ocean, untouched by erosion), the
    # original is also 255 — no change. Where the eroded mask is 0
    # (land side of the new boundary), the original might still be
    # 255 (was ocean) — min picks 0, hiding the original ocean.
    return np.minimum(mask, eroded_arr)


_mask_cache: dict[tuple[str, int, int, int, int], np.ndarray] = {}


def get_or_create_mask(
    shapefile_path: Path,
    width: int,
    height: int,
    supersample: int = 2,
    coast_offset: int = 0,
) -> np.ndarray:
    """Return a cached ocean mask, computing it if not already cached.

    :param shapefile_path: Path to the .shp file.
    :param width: Mask width in pixels.
    :param height: Mask height in pixels.
    :param supersample: Supersampling factor for anti-aliasing.
    :param coast_offset: Coastal transition zone width.
    :returns: uint8 mask array.
    """
    key = (str(shapefile_path), width, height, supersample, coast_offset)
    if key in _mask_cache:
        return _mask_cache[key]

    mask = rasterize_ocean_mask(shapefile_path, width, height, supersample)
    if coast_offset != 0:
        mask = apply_coast_offset(mask, coast_offset)

    _mask_cache[key] = mask
    return mask


def clear_mask_cache() -> None:
    """Clear the in-memory mask cache."""
    _mask_cache.clear()


def detect_ice_regions(
    image: Image.Image,
    ocean_mask: np.ndarray,
    latitude_threshold: float = 60.0,
    south_only: bool = False,
    seed_luminance: int = 200,
    seed_saturation: float = 0.15,
    relax_luminance: int = 120,
    relax_saturation: float = 0.3,
    min_region_size: int = 50,
    dilation_iterations: int = 5,
    blur_radius: int = 3,
) -> np.ndarray:
    """Detect ice/snow regions in polar ocean areas.

    Uses a two-tier geodesic dilation approach: strict seeds identify
    high-confidence ice cores, connected-component filtering removes noise,
    then surviving seeds expand into a relaxed candidate mask to capture
    marginal ice (thin ice, melt zones, cyan-tinted edges).

    :param image: Source RGB image (equirectangular projection).
    :param ocean_mask: uint8 array (H, W). 0 = land, 255 = ocean.
    :param latitude_threshold: Minimum absolute latitude for polar gate.
    :param south_only: Search south of ``-latitude_threshold`` alone, where
        the ocean side of the coastline holds the Antarctic ice shelves.
    :param seed_luminance: Strict seed luminance threshold (0--255).
    :param seed_saturation: Strict seed max saturation (0.0--1.0).
    :param relax_luminance: Relaxed expansion luminance threshold.
    :param relax_saturation: Relaxed expansion max saturation.
    :param min_region_size: Minimum connected-component size in pixels.
    :param dilation_iterations: Geodesic dilation iteration count.
    :param blur_radius: Gaussian blur radius for soft edges.
    :returns: uint8 array (H, W). 0 = no ice, 255 = ice, intermediate
        at blurred edges.
    """
    from PIL import ImageFilter
    from scipy.ndimage import (
        binary_closing,
        binary_dilation,
        generate_binary_structure,
        label,
    )

    h, w = ocean_mask.shape

    # 1. Latitude gate — identify polar row ranges
    row_latitudes = 90.0 - np.arange(h) * 180.0 / h
    polar_rows = (
        row_latitudes <= -latitude_threshold
        if south_only
        else np.abs(row_latitudes) >= latitude_threshold
    )

    # Find contiguous polar row ranges to avoid processing the full image.
    # In equirectangular, polar rows are at the top (Arctic) and bottom
    # (Antarctic) of the image.
    polar_indices = np.where(polar_rows)[0]
    if len(polar_indices) == 0:
        return np.zeros((h, w), dtype=np.uint8)

    # Collect slices for each contiguous polar band
    bands: list[tuple[int, int]] = []
    band_start = polar_indices[0]
    for i in range(1, len(polar_indices)):
        if polar_indices[i] != polar_indices[i - 1] + 1:
            bands.append((band_start, polar_indices[i - 1] + 1))
            band_start = polar_indices[i]
    bands.append((band_start, polar_indices[-1] + 1))

    result = np.zeros((h, w), dtype=np.uint8)
    struct = generate_binary_structure(2, 2)  # 8-connectivity

    for r_start, r_end in bands:
        # Extract only the polar band from the image (avoids full float32)
        band_arr = np.array(image.crop((0, r_start, w, r_end)), dtype=np.float32)
        band_mask = ocean_mask[r_start:r_end, :]
        domain = band_mask > 0

        if not np.any(domain):
            continue

        # Luminance and saturation on the band only
        lum = (
            0.299 * band_arr[:, :, 0]
            + 0.587 * band_arr[:, :, 1]
            + 0.114 * band_arr[:, :, 2]
        )
        ch_max = band_arr.max(axis=2)
        ch_min = band_arr.min(axis=2)
        sat = np.divide(
            ch_max - ch_min,
            ch_max,
            out=np.zeros_like(ch_max),
            where=ch_max > 0,
        )

        # Free the float32 image band early
        del band_arr

        # Strict seed detection
        seeds = domain & (lum >= seed_luminance) & (sat <= seed_saturation)
        if not np.any(seeds):
            continue

        # Connected-component labeling — remove small regions
        labeled, _ = label(seeds)
        sizes = np.bincount(labeled.ravel())
        keep = sizes >= min_region_size
        keep[0] = False  # background
        seeds_filtered = keep[labeled]
        del labeled, sizes, keep

        if not np.any(seeds_filtered):
            continue

        # Relaxed candidate mask
        relaxed = domain & (lum >= relax_luminance) & (sat <= relax_saturation)
        del lum, ch_max, ch_min, sat

        # Geodesic dilation: grow seeds into relaxed mask
        expanded = binary_dilation(
            seeds_filtered,
            structure=struct,
            iterations=dilation_iterations,
            mask=relaxed,
        )

        # Morphological closing to fill small internal gaps
        closed = binary_closing(expanded, structure=struct, iterations=2)
        closed = closed & domain

        # Gaussian blur for soft transitions at ice/water boundary.
        # Take max with the pre-blur mask so the blur can only expand
        # ice coverage (soft gradient into open water), never erode it
        # (prevents fill-color bleed at the ice/coast boundary).
        band_ice = closed.astype(np.uint8) * 255
        if blur_radius > 0:
            ice_img = Image.fromarray(band_ice, mode="L")
            ice_img = ice_img.filter(ImageFilter.GaussianBlur(radius=blur_radius))
            blurred = np.array(ice_img, dtype=np.uint8)
            band_ice = np.maximum(band_ice, blurred)
        result[r_start:r_end, :] = band_ice

    return result


def reduce_mask_for_ice(
    ocean_mask: np.ndarray,
    ice_mask: np.ndarray,
) -> np.ndarray:
    """Subtract ice regions from the ocean mask.

    Where ice is detected, the ocean mask is reduced so original texture
    pixels are preserved instead of being replaced with the fill color.

    :param ocean_mask: uint8 array (H, W). 0 = land, 255 = ocean.
    :param ice_mask: uint8 array (H, W). 0 = no ice, 255 = full ice.
    :returns: uint8 ocean mask with ice regions reduced toward zero.
    """
    return np.clip(
        ocean_mask.astype(np.int16) - ice_mask.astype(np.int16), 0, 255
    ).astype(np.uint8)


REC709 = np.array([0.2126, 0.7152, 0.0722], dtype=np.float32)
"""The luminance weights the water test reads the imagery with."""


class YearlyWater:
    """Each source pixel's brightest luminance and least blue over the year.

    A pixel is water in every month exactly when its brightest luminance is
    below a threshold and its least blue, blue less red, above one, so these
    two arrays decide the test for any thresholds.
    """

    def __init__(self, max_luma: np.ndarray, min_blue: np.ndarray) -> None:
        """Hold the two arrays.

        :param max_luma: uint8 array ``(H, W)``, the brightest Rec. 709
            luminance over the months added, rounded.
        :param min_blue: int16 array ``(H, W)``, the least blue less red.
        """
        self.max_luma = max_luma
        self.min_blue = min_blue

    @classmethod
    def empty(cls, height: int, width: int) -> YearlyWater:
        """Return the statistics of no month at all."""
        return cls(
            np.zeros((height, width), dtype=np.uint8),
            np.full((height, width), 255, dtype=np.int16),
        )

    def add(self, rgb: np.ndarray, band_rows: int = 540) -> None:
        """Take one month into the statistics.

        :param rgb: uint8 array ``(H, W, 3)``.
        :param band_rows: Rows worked at once, which bounds the float
            intermediates.
        """
        for start in range(0, rgb.shape[0], band_rows):
            band = rgb[start : start + band_rows]
            luma = np.rint(band.astype(np.float32) @ REC709).astype(np.uint8)
            np.maximum(self.max_luma[start : start + band_rows], luma, out=luma)
            self.max_luma[start : start + band_rows] = luma
            blue = band[..., 2].astype(np.int16) - band[..., 0].astype(np.int16)
            np.minimum(self.min_blue[start : start + band_rows], blue, out=blue)
            self.min_blue[start : start + band_rows] = blue


def latitude_rows(height: int, min_latitude: float) -> tuple[int, int]:
    """Return the rows that bound the band within ``min_latitude`` of the equator.

    :param height: Rows of an equirectangular source.
    :param min_latitude: The latitude in degrees.
    :returns: The first row whose center is equatorward of ``min_latitude``
        north, and the first row whose center is at or poleward of it south.
    """
    centers = 90.0 - (np.arange(height) + 0.5) * 180.0 / height
    north = int(np.count_nonzero(centers >= min_latitude))
    south = height - int(np.count_nonzero(centers <= -min_latitude))
    return north, south


def water_from_imagery(
    mask: np.ndarray,
    water: np.ndarray,
) -> tuple[np.ndarray, int]:
    """Turn the imagery's water that is connected to the ocean into ocean.

    A pixel short of open ocean in the mask whose imagery is water joins the
    ocean when it is connected to the mask's open ocean through such pixels,
    four-connected, so that a lake stays land.

    :param mask: uint8 array ``(H, W)``, 255 for open ocean. Changed in place.
    :param water: bool array ``(H, W)``, where the imagery is water.
    :returns: The mask, and how many pixels became ocean.
    """
    from scipy.ndimage import binary_dilation, label

    ocean = mask == 255
    candidate = water & ~ocean
    labels, count = label(candidate)
    if count == 0:
        return mask, 0
    touching = binary_dilation(ocean) & candidate
    keep = np.zeros(count + 1, dtype=bool)
    keep[np.unique(labels[touching])] = True
    keep[0] = False
    joined = keep[labels]
    mask[joined] = 255
    return mask, int(np.count_nonzero(joined))


IMAGERY_WATER_LATITUDE = 50.0
"""The imagery's water joins the ocean poleward of this latitude."""

IMAGERY_WATER = (14, 7)
"""The brightest Rec. 709 luminance over the year, at most, and the least
blue above red in every month, at least, of a pixel the imagery draws as
water: the 99th and the 1st percentile of the Blue Marble of 2004 over the
open ocean poleward of 50 degrees, at least 10 pixels from anything else and
where OSI-450-a1's concentration stays below 15% all year."""


def join_imagery_water(
    mask: np.ndarray,
    stats: YearlyWater,
    min_latitude: float = IMAGERY_WATER_LATITUDE,
    thresholds: tuple[int, int] = IMAGERY_WATER,
) -> dict[str, int]:
    """Join the imagery's water to the ocean poleward of a latitude, in place.

    A pixel is the imagery's water in every month when its brightest
    luminance is at most the first threshold and its least blue above red at
    least the second. In the north's band and in the south's, such water
    joins the ocean where :func:`water_from_imagery` connects it, so a lake
    stays land.

    :param mask: uint8 array ``(H, W)``, 255 for open ocean, equirectangular.
        Changed in place.
    :param stats: The imagery's statistics over the year.
    :param min_latitude: The bands' equatorward edge, in degrees.
    :param thresholds: The luminance and the blue above red.
    :returns: The pixels joined in the ``north`` and in the ``south``.
    """
    height = mask.shape[0]
    north, south = latitude_rows(height, min_latitude)
    max_luma, min_blue = thresholds
    joined = {}
    for name, start, stop in (("north", 0, north), ("south", south, height)):
        water = (stats.max_luma[start:stop] <= max_luma) & (
            stats.min_blue[start:stop] >= min_blue
        )
        _, joined[name] = water_from_imagery(mask[start:stop], water)
    return joined
