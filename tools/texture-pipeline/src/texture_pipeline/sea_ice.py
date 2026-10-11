"""Monthly sea ice concentration from the OSI SAF climate data record.

The source is EUMETSAT OSI SAF's OSI-450-a1, the monthly mean sea ice
concentration of each hemisphere on the 25 km EASE2 grid, a Lambert azimuthal
equal-area projection of the WGS84 ellipsoid. The bake reads the twelve months
of one year, the Blue Marble's 2004, and resamples each month into a cap: a
square in the polar aspect of the spherical Lambert azimuthal equal-area
projection, in units of the sphere's radius, where the ice is generated
(``sea_ice_look``) before it is composited into the day faces.

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
from pyproj import Transformer
from scipy.ndimage import distance_transform_edt, map_coordinates

PRODUCT = "OSI-450-a1"
"""The product the bake reads."""

DOI = "10.15770/EUM_SAF_OSI_0023"
"""The product's DOI, which the credit names."""

YEAR = 2004
"""The year read: the year of the Blue Marble the day faces are baked from."""

MONTHS = range(1, 13)

HEMISPHERES = ("nh", "sh")
"""The source's hemisphere names, north first."""

THREDDS = (
    "https://thredds.met.no/thredds/fileServer/osisaf/met.no/reprocessed/ice/"
    "conc_450a1_files/monthly"
)
"""Where ``--fetch`` downloads the monthly files from, one per request."""

MANIFEST = "SHA256SUMS"
"""The checksum file ``--fetch`` writes beside the monthly files."""

CAP_SIZE = 4096
"""Texels along each side of a cap the ice is generated on: 2.87 km a texel."""

FIELD_SIZE = 1024
"""Texels along each side of the cap the concentration is resampled to: 11.5 km
a texel, against the source's 25 km."""

RHO_MAX = 2 * math.sin(math.radians(27.5))
"""Half the side of a cap in sphere radii: 35 degrees of latitude on its axes."""

LAND_REACH = 3
"""Source cells a land cell may be from the ocean and still take its value."""

AUTHALIC_RADIUS_KM = 6371.0072
"""The radius of the sphere with the WGS84 ellipsoid's area."""

SOURCE_CELLS = 432
SOURCE_CELL_KM = 25.0
SOURCE_HALF_KM = SOURCE_CELLS * SOURCE_CELL_KM / 2

BAND_ROWS = 512
"""Cap rows projected at once, which bounds the float64 intermediates."""

_EASE2 = {"nh": "EPSG:6931", "sh": "EPSG:6932"}


def file_name(hemisphere: str, month: int, year: int = YEAR) -> str:
    """Return the name of one monthly file of the record.

    :param hemisphere: ``nh`` or ``sh``.
    :param month: The month, 1 to 12.
    :param year: The year.
    :returns: The file's name as the server and the record give it.
    """
    return f"ice_conc_{hemisphere}_ease2-250_cdr-v3p1_{year}{month:02d}.nc"


def file_url(hemisphere: str, month: int, year: int = YEAR) -> str:
    """Return the HTTPS address of one monthly file.

    :param hemisphere: ``nh`` or ``sh``.
    :param month: The month, 1 to 12.
    :param year: The year.
    :returns: Its address on the THREDDS file server.
    """
    return f"{THREDDS}/{year}/{file_name(hemisphere, month, year)}"


def year_files(year: int = YEAR) -> list[str]:
    """Return the names of every monthly file of a year, north first.

    :param year: The year.
    :returns: The 24 names in hemisphere and month order.
    """
    return [file_name(h, m, year) for h in HEMISPHERES for m in MONTHS]


def missing_files(directory: Path, year: int = YEAR) -> list[str]:
    """Return the monthly files of a year that ``directory`` lacks.

    :param directory: Where the files are kept.
    :param year: The year.
    :returns: Their names, in :func:`year_files` order.
    """
    return [name for name in year_files(year) if not (directory / name).exists()]


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
    year: int = YEAR,
    opener: Opener = _download,
    report: Callable[[str], None] = lambda _: None,
) -> int:
    """Download the monthly files missing from ``directory``, one at a time.

    The THREDDS server asks its users not to run parallel sessions, so the
    files are requested in sequence. Each lands under a temporary name first,
    so an interrupted run leaves no truncated file behind. The manifest is
    rewritten afterwards with the checksum of every file of the year.

    :param directory: Where the files are kept. Created if missing.
    :param year: The year.
    :param opener: Returns the body at a URL.
    :param report: Told the name of each file as it is downloaded.
    :returns: How many files were downloaded.
    """
    directory.mkdir(parents=True, exist_ok=True)
    downloaded = 0
    for hemisphere in HEMISPHERES:
        for month in MONTHS:
            path = directory / file_name(hemisphere, month, year)
            if path.exists():
                continue
            report(path.name)
            partial = path.with_name(path.name + ".part")
            partial.write_bytes(opener(file_url(hemisphere, month, year)))
            partial.replace(path)
            downloaded += 1
    write_manifest(directory, year_files(year))
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
    """Read the concentration of one monthly file as a fraction.

    :param path: A monthly OSI-450-a1 file.
    :returns: float32 array of shape ``(432, 432)`` from 0 to 1, NaN over land,
        row 0 at the grid's top.
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
    return np.ma.filled(conc.astype(np.float32) / 100, np.nan)


@dataclass(frozen=True)
class Hemisphere:
    """The twelve monthly means of one hemisphere."""

    name: str
    """``nh`` or ``sh``."""

    means: np.ndarray
    """float32 array of shape ``(12, 432, 432)`` from 0 to 1, January first,
    NaN over land."""

    land: np.ndarray
    """bool array of shape ``(432, 432)``, the source's land cells."""


def read_hemisphere(
    directory: Path,
    hemisphere: str,
    year: int = YEAR,
    read: Callable[[Path], np.ndarray] = read_month,
) -> Hemisphere:
    """Read the twelve monthly means of a hemisphere.

    :param directory: Where the monthly files are.
    :param hemisphere: ``nh`` or ``sh``.
    :param year: The year.
    :param read: Reads one file, as :func:`read_month` does.
    :returns: The means and the land cells they share.
    :raises ValueError: If a file's land cells differ from January's.
    """
    land: np.ndarray | None = None
    means = []
    for month in MONTHS:
        path = directory / file_name(hemisphere, month, year)
        conc = read(path)
        here = np.isnan(conc)
        if land is None:
            land = here
        elif not np.array_equal(here, land):
            msg = f"{path.name} has other land cells than January"
            raise ValueError(msg)
        means.append(conc.astype(np.float32))
    assert land is not None
    return Hemisphere(hemisphere, np.stack(means), land)


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


def cap_texel_centers(
    size: int, start: int = 0, stop: int | None = None
) -> tuple[np.ndarray, np.ndarray]:
    """Return the plane coordinates of a band of a cap's texel centers.

    :param size: Texels along each side.
    :param start: First row of the band.
    :param stop: Row after the last row of the band, ``size`` when omitted.
    :returns: ``x`` and ``y`` arrays of shape ``(stop - start, size)`` in
        sphere radii, row 0 at ``y`` near ``RHO_MAX``.
    """
    stop = size if stop is None else stop
    centers = ((np.arange(size) + 0.5) / size * 2 - 1) * RHO_MAX
    shape = (stop - start, size)
    x = np.broadcast_to(centers[None, :], shape)
    y = np.broadcast_to(-centers[start:stop, None], shape)
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


def directions_to_cap(
    x: np.ndarray, y: np.ndarray, z: np.ndarray
) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Project directions of the world frame onto their hemisphere's cap.

    :param x: Direction components toward longitude 90 E, not normalized.
    :param y: Direction components toward the north pole.
    :param z: Direction components toward longitude 0.
    :returns: Whether each point is south, and its ``x`` and ``y`` on that
        hemisphere's cap in sphere radii.
    """
    length = np.sqrt(x * x + y * y + z * z)
    nx, ny, nz = x / length, y / length, z / length
    south = ny < 0
    k = np.sqrt(2 / (1 + np.abs(ny)))
    return south, k * nx, np.where(south, k * nz, -k * nz)


def latlon_to_cap(
    latitude: np.ndarray, longitude: np.ndarray
) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Project points onto their hemisphere's cap, as the faces do.

    :param latitude: Latitude in degrees.
    :param longitude: Longitude in degrees.
    :returns: Whether each point is south, and its ``x`` and ``y`` on that
        hemisphere's cap.
    """
    phi = np.radians(latitude)
    lam = np.radians(longitude)
    return directions_to_cap(
        np.cos(phi) * np.sin(lam), np.sin(phi), np.cos(phi) * np.cos(lam)
    )


def plane_to_texel(
    x: np.ndarray, y: np.ndarray, size: int
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
    transformer = Transformer.from_crs("EPSG:4326", _EASE2[hemisphere], always_xy=True)
    xs, ys = transformer.transform(np.asarray(longitude), np.asarray(latitude))
    col = (np.asarray(xs) / 1000 + SOURCE_HALF_KM) / SOURCE_CELL_KM - 0.5
    row = (SOURCE_HALF_KM - np.asarray(ys) / 1000) / SOURCE_CELL_KM - 0.5
    return row, col


def cap_concentration(hemisphere: Hemisphere, size: int = FIELD_SIZE) -> np.ndarray:
    """Resample a hemisphere's monthly means onto its cap.

    The 25 km field, extended into land, is read through a cubic B-spline, so
    no cell's diamond survives and nothing overshoots: every texel is a
    weighted mean of the cells around it. Outside the source grid it is 0.

    :param hemisphere: The hemisphere's monthly means.
    :param size: Texels along each side of the cap.
    :returns: float32 array of shape ``(12, size, size)`` from 0 to 1.
    """
    extended = extend_into_land(hemisphere.means, hemisphere.land)
    out = np.empty((len(extended), size, size), dtype=np.float32)
    for start in range(0, size, BAND_ROWS):
        stop = min(start + BAND_ROWS, size)
        x, y = cap_texel_centers(size, start, stop)
        latitude, longitude = cap_to_latlon(x, y, hemisphere.name)
        row, col = ease2_cells(latitude, longitude, hemisphere.name)
        for month, field in enumerate(extended):
            out[month, start:stop] = map_coordinates(
                field,
                [row, col],
                order=3,
                mode="constant",
                cval=0.0,
                prefilter=False,
            )
    np.clip(out, 0.0, 1.0, out=out)
    return out


def texel_km(size: int) -> float:
    """Return the side of a cap texel in kilometers.

    :param size: Texels along each side of the cap.
    :returns: The plane's spacing times the sphere's radius.
    """
    return 2 * RHO_MAX * AUTHALIC_RADIUS_KM / size


def source_area_km2(hemisphere: Hemisphere) -> np.ndarray:
    """Return each month's ice area in the source: concentration times area.

    :param hemisphere: The hemisphere's monthly means.
    :returns: float64 array of 12 areas in square kilometers, January first.
    """
    return np.nansum(hemisphere.means, axis=(1, 2), dtype=np.float64) * (
        SOURCE_CELL_KM**2
    )
