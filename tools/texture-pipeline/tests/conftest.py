"""Shared test fixtures for texture-pipeline tests."""

from io import BytesIO
from pathlib import Path

import geopandas as gpd
import pytest
from PIL import Image
from shapely.geometry import box


@pytest.fixture
def solid_color_jpeg_64x32(tmp_path: Path) -> Path:
    """Create a 64x32 solid red JPEG image."""
    img = Image.new("RGB", (64, 32), color=(255, 0, 0))
    path = tmp_path / "solid_64x32.jpg"
    img.save(path, format="JPEG")
    return path


@pytest.fixture
def gradient_png_128x64(tmp_path: Path) -> Path:
    """Create a 128x64 PNG image with a horizontal gradient."""
    img = Image.new("RGB", (128, 64))
    for x in range(128):
        for y in range(64):
            img.putpixel((x, y), (x * 2, y * 4, 128))
    path = tmp_path / "gradient_128x64.png"
    img.save(path, format="PNG")
    return path


@pytest.fixture
def gradient_image_128x64() -> Image.Image:
    """Create a 128x64 in-memory PIL image with a gradient."""
    img = Image.new("RGB", (128, 64))
    for x in range(128):
        for y in range(64):
            img.putpixel((x, y), (x * 2, y * 4, 128))
    return img


@pytest.fixture
def gradient_image_256x128() -> Image.Image:
    """Create a 256x128 in-memory PIL image with a gradient."""
    img = Image.new("RGB", (256, 128))
    for x in range(256):
        for y in range(128):
            img.putpixel((x, y), (x, y * 2, 128))
    return img


@pytest.fixture
def square_image_100x100() -> Image.Image:
    """Create a 100x100 square image (non-2:1 aspect ratio)."""
    return Image.new("RGB", (100, 100), color=(0, 128, 255))


@pytest.fixture
def jpeg_bytes_128x64() -> bytes:
    """Return raw JPEG bytes for a 128x64 image."""
    img = Image.new("RGB", (128, 64), color=(100, 150, 200))
    buf = BytesIO()
    img.save(buf, format="JPEG", quality=95)
    return buf.getvalue()


@pytest.fixture
def jpeg_file_128x64(tmp_path: Path, jpeg_bytes_128x64: bytes) -> Path:
    """Create a 128x64 JPEG file on disk."""
    path = tmp_path / "source_128x64.jpg"
    path.write_bytes(jpeg_bytes_128x64)
    return path


def _write_shapefile(tmp_path: Path, name: str, polygons: list) -> Path:
    """Write a list of Shapely polygons as a shapefile in EPSG:4326.

    :param tmp_path: Directory to write to.
    :param name: Base name for the shapefile.
    :param polygons: List of Shapely geometry objects.
    :returns: Path to the .shp file.
    """
    gdf = gpd.GeoDataFrame(geometry=polygons, crs="EPSG:4326")
    path = tmp_path / f"{name}.shp"
    gdf.to_file(path)
    return path


@pytest.fixture
def western_half_shapefile(tmp_path: Path) -> Path:
    """Shapefile with a rectangle covering the western half of the globe.

    Longitude -180 to 0, latitude -90 to 90.
    """
    return _write_shapefile(tmp_path, "western_half", [box(-180, -90, 0, 90)])


@pytest.fixture
def eastern_half_shapefile(tmp_path: Path) -> Path:
    """Shapefile with a rectangle covering the eastern half of the globe.

    Longitude 0 to 180, latitude -90 to 90.
    """
    return _write_shapefile(tmp_path, "eastern_half", [box(0, -90, 180, 90)])


@pytest.fixture
def full_globe_shapefile(tmp_path: Path) -> Path:
    """Shapefile with a rectangle covering the entire globe."""
    return _write_shapefile(tmp_path, "full_globe", [box(-180, -90, 180, 90)])


@pytest.fixture
def tiny_polygon_shapefile(tmp_path: Path) -> Path:
    """Shapefile with a tiny polygon near the south pole."""
    return _write_shapefile(tmp_path, "tiny", [box(-180, -90, -179, -89)])
