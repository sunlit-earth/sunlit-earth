"""Monthly sea ice concentration caps from the OSI SAF climate data record.

The source is EUMETSAT OSI SAF's OSI-450-a1, the monthly mean sea ice
concentration of each hemisphere on the 25 km EASE2 grid, a Lambert azimuthal
equal-area projection of the WGS84 ellipsoid. The bake averages each calendar
month over a window of years and resamples the mean into the caps the app
samples: one per hemisphere and month, each a square in the polar aspect of
the spherical Lambert azimuthal equal-area projection, in units of the
sphere's radius.

The caps live in the app's world frame, +Y north, +Z longitude 0 and +X
longitude 90 E (``cube.py``). With ``n`` the unit normal, the north cap's plane
point is ``(k * n.x, -k * n.z)`` with ``k = sqrt(2 / (1 + n.y))`` and the south
cap's is ``(k * n.x, k * n.z)`` with ``k = sqrt(2 / (1 - n.y))``, which puts
longitude 0 straight down in the north and straight up in the south, as EASE2
has it. A cap spans ``-RHO_MAX`` to ``RHO_MAX`` on both axes, with its first
row at ``+RHO_MAX``, so a cap reads like the source file it came from.
"""

import hashlib
import math
import urllib.request
from collections.abc import Callable, Iterable
from dataclasses import dataclass
from pathlib import Path

import netCDF4
import numpy as np
from rasterio.warp import transform
from scipy.ndimage import distance_transform_edt, map_coordinates

PRODUCT = "OSI-450-a1"
"""The product the bake reads."""

YEARS = range(2010, 2021)
"""The years averaged: the last eleven of the record, which ends in 2020."""

MONTHS = range(1, 13)

HEMISPHERES = ("nh", "sh")
"""The source's hemisphere names, in the atlas's row order."""

THREDDS = (
    "https://thredds.met.no/thredds/fileServer/osisaf/met.no/reprocessed/ice/"
    "conc_450a1_files/monthly"
)
"""Where ``--fetch`` downloads the monthly files from, one per request."""

MANIFEST = "SHA256SUMS"
"""The checksum file ``--fetch`` writes beside the monthly files."""

CAP_SIZE = 512
"""Texels along each side of a cap."""

RHO_MAX = 2 * math.sin(math.radians(27.5))
"""Half the side of a cap in sphere radii: 35 degrees of latitude on its axes."""

LAND_REACH = 3
"""Source cells a land cell may be from the ocean and still take its value."""

EXTENT_THRESHOLD = 15.0
"""The concentration in percent at and above which a cell counts as ice."""

AUTHALIC_RADIUS_KM = 6371.0072
"""The radius of the sphere with the WGS84 ellipsoid's area."""

SOURCE_CELLS = 432
SOURCE_CELL_KM = 25.0
SOURCE_HALF_KM = SOURCE_CELLS * SOURCE_CELL_KM / 2

_EASE2 = {"nh": "EPSG:6931", "sh": "EPSG:6932"}


def file_name(hemisphere: str, year: int, month: int) -> str:
    """Return the name of one monthly file of the record.

    :param hemisphere: ``nh`` or ``sh``.
    :param year: The year.
    :param month: The month, 1 to 12.
    :returns: The file's name as the server and the record give it.
    """
    return f"ice_conc_{hemisphere}_ease2-250_cdr-v3p1_{year}{month:02d}.nc"


def file_url(hemisphere: str, year: int, month: int) -> str:
    """Return the HTTPS address of one monthly file.

    :param hemisphere: ``nh`` or ``sh``.
    :param year: The year.
    :param month: The month, 1 to 12.
    :returns: Its address on the THREDDS file server.
    """
    return f"{THREDDS}/{year}/{file_name(hemisphere, year, month)}"


def window_files(years: Iterable[int] = YEARS) -> list[tuple[str, int, int]]:
    """Return every ``(hemisphere, year, month)`` of a window of years.

    :param years: The years of the window.
    :returns: The files in hemisphere, year and month order.
    """
    return [(h, y, m) for h in HEMISPHERES for y in years for m in MONTHS]


def sha256(path: Path) -> str:
    """Return the SHA-256 of a file as lowercase hex."""
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


type Opener = Callable[[str], bytes]


def _download(url: str) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": "texture-pipeline"})
    with urllib.request.urlopen(request, timeout=120) as response:
        return response.read()


def fetch(
    directory: Path,
    years: Iterable[int] = YEARS,
    opener: Opener = _download,
    report: Callable[[str], None] = lambda _: None,
) -> int:
    """Download the monthly files missing from ``directory``, one at a time.

    The THREDDS server asks its users not to run parallel sessions, so the
    files are requested in sequence. Each lands under a temporary name first,
    so an interrupted run leaves no truncated file behind. The manifest is
    rewritten afterwards with the checksum of every file of the window.

    :param directory: Where the files are kept. Created if missing.
    :param years: The window of years.
    :param opener: Returns the body at a URL.
    :param report: Told the name of each file as it is downloaded.
    :returns: How many files were downloaded.
    """
    directory.mkdir(parents=True, exist_ok=True)
    files = window_files(years)
    downloaded = 0
    for hemisphere, year, month in files:
        path = directory / file_name(hemisphere, year, month)
        if path.exists():
            continue
        report(path.name)
        partial = path.with_name(path.name + ".part")
        partial.write_bytes(opener(file_url(hemisphere, year, month)))
        partial.replace(path)
        downloaded += 1
    write_manifest(directory, [file_name(*f) for f in files])
    return downloaded


def write_manifest(directory: Path, names: Iterable[str]) -> Path:
    """Write the checksums of ``names`` in ``directory`` to its manifest.

    :param directory: The directory holding the files.
    :param names: The files to list, all of which must exist.
    :returns: The manifest's path.
    """
    lines = [f"{sha256(directory / name)}  {name}\n" for name in sorted(names)]
    path = directory / MANIFEST
    path.write_text("".join(lines), encoding="utf-8", newline="\n")
    return path


def read_month(path: Path) -> np.ndarray:
    """Read the concentration of one monthly file in percent.

    :param path: A monthly OSI-450-a1 file.
    :returns: float32 array of shape ``(432, 432)``, NaN over land, row 0 at
        the grid's top.
    :raises ValueError: If the file's grid is not the 25 km EASE2 grid.
    """
    with netCDF4.Dataset(path) as dataset:
        xc = np.asarray(dataset["xc"][:])
        yc = np.asarray(dataset["yc"][:])
        expected = (np.arange(SOURCE_CELLS) + 0.5) * SOURCE_CELL_KM - SOURCE_HALF_KM
        if not (np.allclose(xc, expected) and np.allclose(yc, expected[::-1])):
            msg = f"{path.name} is not on the 25 km EASE2 grid"
            raise ValueError(msg)
        conc = dataset["ice_conc"][0]
    return np.ma.filled(conc.astype(np.float32), np.nan)


@dataclass(frozen=True)
class Climatology:
    """The monthly means of one hemisphere."""

    means: np.ndarray
    """float32 array of shape ``(12, 432, 432)`` in percent, January first,
    NaN over land."""

    land: np.ndarray
    """bool array of shape ``(432, 432)``, the source's land cells."""


def climatology(
    directory: Path,
    hemisphere: str,
    years: Iterable[int] = YEARS,
    read: Callable[[Path], np.ndarray] = read_month,
) -> Climatology:
    """Average each calendar month of a hemisphere over a window of years.

    :param directory: Where the monthly files are.
    :param hemisphere: ``nh`` or ``sh``.
    :param years: The window of years.
    :param read: Reads one file, as :func:`read_month` does.
    :returns: The means and the land cells they share.
    :raises ValueError: If a file's land cells differ from the first file's.
    """
    years = list(years)
    land: np.ndarray | None = None
    means = []
    for month in MONTHS:
        total = None
        for year in years:
            path = directory / file_name(hemisphere, year, month)
            conc = read(path)
            here = np.isnan(conc)
            if land is None:
                land = here
            elif not np.array_equal(here, land):
                msg = f"{path.name} has other land cells than the rest of the window"
                raise ValueError(msg)
            total = conc.astype(np.float64) if total is None else total + conc
        assert total is not None
        means.append((total / len(years)).astype(np.float32))
    assert land is not None
    return Climatology(np.stack(means), land)


def extend_into_land(
    means: np.ndarray, land: np.ndarray, reach: int = LAND_REACH
) -> np.ndarray:
    """Give the land cells near the ocean the value of their nearest ocean cell.

    A land cell within ``reach`` cells of the ocean takes the value of the
    ocean cell nearest to it, and every other land cell becomes 0, so a smooth
    sampler does not pull the land's zero into the ice along a coast.

    :param means: float array of shape ``(..., H, W)``, NaN over land.
    :param land: bool array of shape ``(H, W)``.
    :param reach: The largest distance in cells that takes a value.
    :returns: A float32 array of the shape of ``means`` without NaN.
    """
    distance, (rows, cols) = distance_transform_edt(land, return_indices=True)
    extended = means[..., rows, cols].astype(np.float32)
    extended[..., land & (distance > reach)] = 0.0
    return extended


def cap_texel_centers(size: int = CAP_SIZE) -> tuple[np.ndarray, np.ndarray]:
    """Return the plane coordinates of a cap's texel centers.

    :param size: Texels along each side.
    :returns: ``x`` and ``y`` arrays of shape ``(size, size)`` in sphere
        radii, row 0 at ``y`` near ``RHO_MAX``.
    """
    centers = ((np.arange(size) + 0.5) / size * 2 - 1) * RHO_MAX
    x = np.broadcast_to(centers[None, :], (size, size))
    y = np.broadcast_to(-centers[:, None], (size, size))
    return x, y


def cap_to_latlon(
    x: np.ndarray, y: np.ndarray, hemisphere: str
) -> tuple[np.ndarray, np.ndarray]:
    """Invert the spherical polar projection of a cap.

    :param x: Plane coordinates in sphere radii.
    :param y: Plane coordinates of the same shape.
    :param hemisphere: ``nh`` or ``sh``.
    :returns: Latitude and longitude in degrees.
    """
    rho = np.hypot(x, y)
    colatitude = 2 * np.arcsin(np.clip(rho / 2, 0, 1))
    if hemisphere == "nh":
        latitude = 90 - np.degrees(colatitude)
        longitude = np.degrees(np.arctan2(x, -y))
    else:
        latitude = np.degrees(colatitude) - 90
        longitude = np.degrees(np.arctan2(x, y))
    return latitude, longitude


def latlon_to_cap(
    latitude: np.ndarray, longitude: np.ndarray
) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Project points the way the shader does: from the unit normal.

    :param latitude: Latitude in degrees.
    :param longitude: Longitude in degrees.
    :returns: The hemisphere index (0 north, 1 south) and the ``x`` and ``y``
        plane coordinates in that hemisphere's cap.
    """
    phi = np.radians(latitude)
    lam = np.radians(longitude)
    nx = np.cos(phi) * np.sin(lam)
    ny = np.sin(phi)
    nz = np.cos(phi) * np.cos(lam)
    south = ny < 0
    k = np.sqrt(2 / (1 + np.abs(ny)))
    return south.astype(np.intp), k * nx, np.where(south, k * nz, -k * nz)


def plane_to_texel(
    x: np.ndarray, y: np.ndarray, size: int = CAP_SIZE
) -> tuple[np.ndarray, np.ndarray]:
    """Return continuous texel coordinates of plane points, centers at integers.

    :param x: Plane coordinates in sphere radii.
    :param y: Plane coordinates of the same shape.
    :param size: Texels along each side of the cap.
    :returns: ``row`` and ``col`` arrays.
    """
    col = (x / RHO_MAX + 1) / 2 * size - 0.5
    row = (1 - y / RHO_MAX) / 2 * size - 0.5
    return row, col


def ease2_cells(
    latitude: np.ndarray, longitude: np.ndarray, hemisphere: str
) -> tuple[np.ndarray, np.ndarray]:
    """Return the continuous source cell coordinates of geodetic points.

    :param latitude: Geodetic latitude in degrees.
    :param longitude: Longitude in degrees.
    :param hemisphere: ``nh`` or ``sh``.
    :returns: ``row`` and ``col`` arrays, cell centers at whole numbers.
    """
    xs, ys = transform(
        "EPSG:4326",
        _EASE2[hemisphere],
        np.ravel(longitude).tolist(),
        np.ravel(latitude).tolist(),
    )
    x_km = np.asarray(xs).reshape(np.shape(latitude)) / 1000
    y_km = np.asarray(ys).reshape(np.shape(latitude)) / 1000
    col = (x_km + SOURCE_HALF_KM) / SOURCE_CELL_KM - 0.5
    row = (SOURCE_HALF_KM - y_km) / SOURCE_CELL_KM - 0.5
    return row, col


def cap_source_cells(
    hemisphere: str, size: int = CAP_SIZE
) -> tuple[np.ndarray, np.ndarray]:
    """Return where each texel center of a cap falls on the source grid.

    :param hemisphere: ``nh`` or ``sh``.
    :param size: Texels along each side of the cap.
    :returns: ``row`` and ``col`` arrays of shape ``(size, size)``.
    """
    x, y = cap_texel_centers(size)
    latitude, longitude = cap_to_latlon(x, y, hemisphere)
    return ease2_cells(latitude, longitude, hemisphere)


def resample(field: np.ndarray, row: np.ndarray, col: np.ndarray) -> np.ndarray:
    """Sample a source field bilinearly, 0 outside the grid.

    :param field: float array of shape ``(H, W)`` without NaN.
    :param row: Source rows, cell centers at whole numbers.
    :param col: Source columns of the same shape.
    :returns: float32 array of the coordinates' shape.
    """
    return map_coordinates(
        field, [row, col], order=1, mode="constant", cval=0.0, prefilter=False
    ).astype(np.float32)


def quantize(cap: np.ndarray) -> np.ndarray:
    """Turn a cap in percent into bytes, with its outermost ring zero.

    The ring is what a clamped sample past the edge of the cap reads.

    :param cap: float array in percent.
    :returns: uint8 array of the same shape.
    """
    out = np.clip(np.round(cap / 100 * 255), 0, 255).astype(np.uint8)
    out[0, :] = out[-1, :] = 0
    out[:, 0] = out[:, -1] = 0
    return out


def assemble(caps: np.ndarray) -> np.ndarray:
    """Lay out the caps as the atlas: months across, the north above the south.

    :param caps: uint8 array of shape ``(2, 12, size, size)``.
    :returns: uint8 array of shape ``(2 * size, 12 * size)``.
    """
    hemispheres, months, size, _ = caps.shape
    return caps.transpose(0, 2, 1, 3).reshape(hemispheres * size, months * size)


def disassemble(atlas: np.ndarray, size: int) -> np.ndarray:
    """Cut an atlas back into its caps, the inverse of :func:`assemble`.

    :param atlas: uint8 array of shape ``(2 * size, 12 * size)``.
    :param size: Texels along each side of a cap.
    :returns: uint8 array of shape ``(2, 12, size, size)``.
    """
    return atlas.reshape(2, size, 12, size).transpose(0, 2, 1, 3)


def bake_hemisphere(
    climate: Climatology, hemisphere: str, size: int = CAP_SIZE
) -> np.ndarray:
    """Resample a hemisphere's twelve monthly means into caps.

    :param climate: The hemisphere's monthly means.
    :param hemisphere: ``nh`` or ``sh``.
    :param size: Texels along each side of a cap.
    :returns: float32 array of shape ``(12, size, size)`` in percent.
    """
    extended = extend_into_land(climate.means, climate.land)
    row, col = cap_source_cells(hemisphere, size)
    return np.stack([resample(month, row, col) for month in extended])


@dataclass(frozen=True)
class Extent:
    """The ice extent of one month of one hemisphere in square kilometers."""

    source: float
    """The ocean cells of the mean at or above the threshold."""

    baked: float
    """The atlas texels at or above it whose center is over a source ocean
    cell."""

    @property
    def ratio(self) -> float:
        """The baked extent over the source's, 1 when both are empty."""
        return self.baked / self.source if self.source else float(self.baked == 0)


def extents(climate: Climatology, caps: np.ndarray, hemisphere: str) -> list[Extent]:
    """Compare the ice extent of each baked cap with its source mean.

    A texel's area is the cap's equal share of the plane, which the projection
    makes the same on the sphere; a source cell's is 625 square kilometers.
    The texels are counted over the source's ocean alone, since the land
    extension gives values to coastal land the app's water mask removes.

    :param climate: The hemisphere's monthly means.
    :param caps: uint8 array of shape ``(12, size, size)``.
    :param hemisphere: ``nh`` or ``sh``.
    :returns: One extent per month, January first.
    """
    size = caps.shape[-1]
    texel_km2 = (2 * RHO_MAX / size * AUTHALIC_RADIUS_KM) ** 2
    row, col = cap_source_cells(hemisphere, size)
    r = np.rint(row).astype(np.intp)
    c = np.rint(col).astype(np.intp)
    inside = (r >= 0) & (r < SOURCE_CELLS) & (c >= 0) & (c < SOURCE_CELLS)
    ocean = np.zeros_like(inside)
    ocean[inside] = ~climate.land[r[inside], c[inside]]
    threshold = EXTENT_THRESHOLD / 100 * 255
    result = []
    for mean, cap in zip(climate.means, caps, strict=True):
        source = float(np.count_nonzero(mean >= EXTENT_THRESHOLD)) * SOURCE_CELL_KM**2
        baked = float(np.count_nonzero((cap >= threshold) & ocean)) * texel_km2
        result.append(Extent(source, baked))
    return result


@dataclass(frozen=True)
class Bake:
    """What one bake made."""

    atlas: np.ndarray
    """uint8 array of shape ``(2 * size, 12 * size)``."""

    extents: dict[str, list[Extent]]
    """Per hemisphere, the twelve months' extents, January first."""


def bake(
    directory: Path,
    years: Iterable[int] = YEARS,
    size: int = CAP_SIZE,
    read: Callable[[Path], np.ndarray] = read_month,
) -> Bake:
    """Bake the atlas from the monthly files of a window of years.

    :param directory: Where the monthly files are.
    :param years: The window of years.
    :param size: Texels along each side of a cap.
    :param read: Reads one file, as :func:`read_month` does.
    :returns: The atlas and the extent of every month.
    """
    years = list(years)
    caps = []
    found: dict[str, list[Extent]] = {}
    for hemisphere in HEMISPHERES:
        climate = climatology(directory, hemisphere, years, read)
        quantized = np.stack(
            [quantize(cap) for cap in bake_hemisphere(climate, hemisphere, size)]
        )
        found[hemisphere] = extents(climate, quantized, hemisphere)
        caps.append(quantized)
    return Bake(assemble(np.stack(caps)), found)


def shrink(atlas: np.ndarray, size: int) -> np.ndarray:
    """Average every cap of an atlas down to ``size`` texels a side.

    :param atlas: uint8 atlas of caps of a multiple of ``size``.
    :param size: Texels along each side of a shrunk cap.
    :returns: The atlas of shrunk caps.
    """
    caps = disassemble(atlas, atlas.shape[0] // 2)
    factor = caps.shape[-1] // size
    blocks = caps.reshape(2, 12, size, factor, size, factor).astype(np.float64)
    shrunk = np.rint(blocks.mean(axis=(3, 5))).astype(np.uint8)
    return assemble(shrunk)
