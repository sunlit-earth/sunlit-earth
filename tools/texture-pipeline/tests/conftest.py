"""Shared test fixtures for texture-pipeline tests."""

from io import BytesIO
from pathlib import Path

import geopandas as gpd
import numpy as np
import OpenEXR
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


@pytest.fixture
def polar_ice_image_200x100() -> Image.Image:
    """200x100 image with ice-like and ocean-like pixels for ice detection tests.

    Equirectangular: row 0 = +90, row 100 = -90.
    Rows where |lat| >= 60 are rows 0..16 (Arctic) and 83..99 (Antarctic).
    Arctic zone (rows 0..16): dark blue ocean (10, 30, 65) everywhere,
    EXCEPT a 20x10 bright white block at (col=10..30, row=3..13) = ice,
    and 3 isolated bright pixels at (50,2), (52,2), (54,2) = noise.
    Tropical/mid-lat zone (rows 17..82): uniform mid-grey (128, 128, 128).
    Antarctic zone (rows 83..99): dark blue ocean (10, 30, 65).
    """
    arr = np.zeros((100, 200, 3), dtype=np.uint8)

    # Arctic ocean (rows 0..16)
    arr[:17, :] = [10, 30, 65]
    # Large contiguous ice block
    arr[3:13, 10:30] = [240, 240, 240]
    # 3 isolated noise pixels
    arr[2, 50] = [240, 240, 240]
    arr[2, 52] = [240, 240, 240]
    arr[2, 54] = [240, 240, 240]

    # Tropical zone (rows 17..82)
    arr[17:83, :] = [128, 128, 128]

    # Antarctic ocean (rows 83..99)
    arr[83:, :] = [10, 30, 65]

    return Image.fromarray(arr)


@pytest.fixture
def polar_ice_ocean_mask_200x100() -> np.ndarray:
    """200x100 ocean mask: ocean in polar zones, land in tropics.

    Matches polar_ice_image_200x100 layout.
    """
    mask = np.zeros((100, 200), dtype=np.uint8)
    mask[:17, :] = 255  # Arctic ocean
    mask[83:, :] = 255  # Antarctic ocean
    return mask


def write_exr(path: Path, channels: dict[str, np.ndarray]) -> Path:
    """Write a scanline EXR from a mapping of channel name to 2D array.

    The bindings read each plane's buffer as if it were contiguous, so a
    strided view of an interleaved array would be written interleaved.

    :param path: Destination file path.
    :param channels: Channel name to ``(H, W)`` array, half or float.
    :returns: The path written.
    """
    header = {"compression": OpenEXR.ZIP_COMPRESSION, "type": OpenEXR.scanlineimage}
    planes = {name: np.ascontiguousarray(a) for name, a in channels.items()}
    with OpenEXR.File(header, planes) as out:
        out.write(str(path))
    return path


def synthetic_sky(width: int, height: int) -> np.ndarray:
    """Build a linear ``(H, W, 3)`` sky with a smooth band and a few stars.

    :param width: Image width in pixels.
    :param height: Image height in pixels.
    :returns: A float32 array of small positive values.
    """
    rows = np.arange(height, dtype=np.float32)[:, None]
    cols = np.arange(width, dtype=np.float32)[None, :]
    band = 40.0 * np.exp(-(((rows - height / 2) / (height / 12)) ** 2))
    grain = 2.0 + np.sin(cols / 7.0) * np.cos(rows / 5.0)
    sky = np.repeat((band + grain)[..., None], 3, axis=2).astype(np.float32)
    sky[..., 1] *= 0.9
    sky[..., 2] *= 0.75
    for row, col in ((height // 4, 3), (height // 3, width // 2), (height - 5, 17)):
        sky[row, col] += 400.0
    return sky


@pytest.fixture
def rgb_exr_64x32(tmp_path: Path) -> tuple[Path, np.ndarray]:
    """A 64x32 float EXR with distinct R, G and B ramps, and its pixel array."""
    height, width = 32, 64
    values = np.arange(height * width, dtype=np.float32).reshape(height, width) / 1000
    rgb = np.stack([values, values * 2.0, values * 3.0], axis=-1).astype(np.float32)
    path = write_exr(
        tmp_path / "rgb_64x32.exr",
        {"R": rgb[..., 0], "G": rgb[..., 1], "B": rgb[..., 2]},
    )
    return path, rgb


@pytest.fixture
def rgba_exr_64x32(tmp_path: Path) -> Path:
    """A 64x32 float EXR carrying an alpha channel beside R, G and B."""
    height, width = 32, 64
    plane = np.full((height, width), 0.25, dtype=np.float32)
    return write_exr(
        tmp_path / "rgba_64x32.exr",
        {"R": plane, "G": plane * 2, "B": plane * 3, "A": np.ones_like(plane)},
    )


@pytest.fixture
def sky_exr_512x256(tmp_path: Path) -> Path:
    """A 512x256 half-float EXR holding a synthetic sky."""
    sky = synthetic_sky(512, 256)
    return write_exr(
        tmp_path / "sky_512x256.exr",
        {
            "R": sky[..., 0].astype(np.float16),
            "G": sky[..., 1].astype(np.float16),
            "B": sky[..., 2].astype(np.float16),
        },
    )


@pytest.fixture
def square_exr_64x64(tmp_path: Path) -> Path:
    """A 64x64 float EXR, which is not the 2:1 a panorama must be."""
    plane = np.full((64, 64), 0.5, dtype=np.float32)
    return write_exr(
        tmp_path / "square_64x64.exr",
        {"R": plane, "G": plane, "B": plane},
    )
