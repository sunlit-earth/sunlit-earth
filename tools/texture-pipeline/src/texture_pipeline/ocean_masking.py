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
        img = img.resize((width, height), resample=Image.LANCZOS)
        mask = np.array(img, dtype=np.uint8)

    return mask


def apply_ocean_mask(
    image: Image.Image,
    mask: np.ndarray,
    color: tuple[int, int, int] = (10, 40, 80),
) -> Image.Image:
    """Replace ocean pixels in an image with a uniform fill color.

    :param image: Source RGB image.
    :param mask: uint8 array of shape (H, W). 0 = land, 255 = ocean.
    :param color: RGB fill color for ocean regions.
    :returns: New RGB image with ocean pixels replaced.
    """
    src = np.array(image, dtype=np.float32)
    alpha = mask.astype(np.float32) / 255.0
    fill = np.array(color, dtype=np.float32)
    result = src * (1.0 - alpha[..., None]) + fill * alpha[..., None]
    return Image.fromarray(result.clip(0, 255).astype(np.uint8), mode="RGB")


def apply_coastal_buffer(
    mask: np.ndarray,
    buffer_pixels: int,
) -> np.ndarray:
    """Add a transition zone at the coastline by blurring the mask boundary.

    Pixels deep in the ocean stay 255; pixels deep inland stay 0. Only the
    boundary region gains intermediate values, extending the transition into
    the land side.

    :param mask: uint8 array of shape (H, W). 0 = land, 255 = ocean.
    :param buffer_pixels: Width of the transition zone in pixels. 0 = no-op.
    :returns: uint8 mask with the same shape, with a gradient at the coast.
    """
    if buffer_pixels == 0:
        return mask

    from PIL import ImageFilter

    img = Image.fromarray(mask, mode="L")
    blurred = img.filter(ImageFilter.GaussianBlur(radius=buffer_pixels))
    blurred_arr = np.array(blurred, dtype=np.uint8)

    # Preserve deep-ocean pixels: where the original mask was 255, keep 255.
    # Elsewhere, use the blurred value (which introduces a gradient at the
    # coast, extending into land).
    result = np.where(mask == 255, np.uint8(255), blurred_arr)
    return result.astype(np.uint8)


_mask_cache: dict[tuple[str, int, int, int, int], np.ndarray] = {}


def get_or_create_mask(
    shapefile_path: Path,
    width: int,
    height: int,
    supersample: int = 2,
    buffer_pixels: int = 0,
) -> np.ndarray:
    """Return a cached ocean mask, computing it if not already cached.

    :param shapefile_path: Path to the .shp file.
    :param width: Mask width in pixels.
    :param height: Mask height in pixels.
    :param supersample: Supersampling factor for anti-aliasing.
    :param buffer_pixels: Coastal transition zone width.
    :returns: uint8 mask array.
    """
    key = (str(shapefile_path), width, height, supersample, buffer_pixels)
    if key in _mask_cache:
        return _mask_cache[key]

    mask = rasterize_ocean_mask(shapefile_path, width, height, supersample)
    if buffer_pixels > 0:
        mask = apply_coastal_buffer(mask, buffer_pixels)

    _mask_cache[key] = mask
    return mask


def clear_mask_cache() -> None:
    """Clear the in-memory mask cache."""
    _mask_cache.clear()


def detect_ice_regions(
    image: Image.Image,
    ocean_mask: np.ndarray,
    latitude_threshold: float = 60.0,
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

    arr = np.array(image, dtype=np.float32)
    h, w = arr.shape[:2]

    # 1. Latitude gate — only polar rows
    row_latitudes = 90.0 - np.arange(h) * 180.0 / h
    polar_rows = np.abs(row_latitudes) >= latitude_threshold
    polar_mask_2d = polar_rows[:, np.newaxis]  # (H, 1) for broadcasting

    # 2. Domain: polar ocean pixels
    domain = (ocean_mask > 128) & polar_mask_2d

    # Early exit if no polar ocean pixels
    if not np.any(domain):
        return np.zeros((h, w), dtype=np.uint8)

    # 3. Luminance and saturation
    lum = 0.299 * arr[:, :, 0] + 0.587 * arr[:, :, 1] + 0.114 * arr[:, :, 2]
    ch_max = arr.max(axis=2)
    ch_min = arr.min(axis=2)
    sat = np.where(ch_max > 0, (ch_max - ch_min) / ch_max, 0.0)

    # 4. Strict seed detection
    seeds = domain & (lum >= seed_luminance) & (sat <= seed_saturation)

    if not np.any(seeds):
        return np.zeros((h, w), dtype=np.uint8)

    # 5. Connected-component labeling — remove small regions
    labeled, _ = label(seeds)
    sizes = np.bincount(labeled.ravel())
    keep = sizes >= min_region_size
    keep[0] = False  # background
    seeds_filtered = keep[labeled]

    if not np.any(seeds_filtered):
        return np.zeros((h, w), dtype=np.uint8)

    # 6. Relaxed candidate mask
    relaxed = domain & (lum >= relax_luminance) & (sat <= relax_saturation)

    # 7. Geodesic dilation: grow seeds into relaxed mask
    struct = generate_binary_structure(2, 2)  # 8-connectivity
    expanded = binary_dilation(
        seeds_filtered,
        structure=struct,
        iterations=dilation_iterations,
        mask=relaxed,
    )

    # 8. Morphological closing to fill small internal gaps
    closed = binary_closing(expanded, structure=struct, iterations=2)
    closed = closed & domain  # re-intersect with polar ocean

    # 9. Gaussian blur for soft transitions
    ice_uint8 = (closed.astype(np.uint8) * 255)
    ice_img = Image.fromarray(ice_uint8, mode="L")
    if blur_radius > 0:
        ice_img = ice_img.filter(ImageFilter.GaussianBlur(radius=blur_radius))
    result = np.array(ice_img, dtype=np.uint8)

    # Ensure non-polar rows are zero (blur may have bled across boundary)
    result[~polar_rows, :] = 0

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
