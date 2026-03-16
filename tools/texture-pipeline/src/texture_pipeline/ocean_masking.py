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
