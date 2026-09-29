"""Equi-angular cube faces resampled from equirectangular maps.

The cube is the app's world frame: +Y is north, +Z is longitude 0 and +X is
longitude 90 E. Faces follow the OpenGL and Direct3D cube map table and are
named ``px``, ``nx``, ``py``, ``ny``, ``pz`` and ``nz`` in that order, which is
also the layer order of a cube texture. A face texel at row ``r`` and column
``c`` of ``n`` sits at the warped coordinates ``t = (r + 0.5) / n * 2 - 1`` and
``s = (c + 0.5) / n * 2 - 1``; the tangent warp ``tan(s * pi / 4)`` turns them
into the coordinates on the plain cube face that the table combines into a
direction, so every texel spans the same angle along a face's axes.

An equirectangular source has its left edge at longitude -180 and its top edge
at latitude 90, with pixel centers half a pixel in from both.
"""

from collections.abc import Callable
from concurrent.futures import Executor

import numpy as np
from PIL import Image

type Directions = tuple[np.ndarray, np.ndarray, np.ndarray]

_DIRECTIONS: dict[str, Callable[[np.ndarray, np.ndarray, np.ndarray], Directions]] = {
    "px": lambda s, t, one: (one, -t, -s),
    "nx": lambda s, t, one: (-one, -t, s),
    "py": lambda s, t, one: (s, one, t),
    "ny": lambda s, t, one: (s, -one, -t),
    "pz": lambda s, t, one: (s, -t, one),
    "nz": lambda s, t, one: (-s, -t, -one),
}

FACES: tuple[str, ...] = tuple(_DIRECTIONS)
"""The face names in cube texture layer order: +X, -X, +Y, -Y, +Z, -Z."""

BAND_ROWS = 256
"""Face rows resampled at once, which bounds the float intermediates."""


def working_size(source_width: int, face_size: int) -> int:
    """Return the face size that keeps the source's density at the equator.

    A face spans a quarter of the equator, so a face of ``source_width / 4``
    texels takes every source pixel there. Resampling at that size and
    area-averaging down to ``face_size`` keeps the detail a direct resample
    to the smaller face would alias away.

    :param source_width: Width of the equirectangular source in pixels.
    :param face_size: Edge length of the face to be written.
    :returns: The edge length to resample at, never below ``face_size``.
    """
    return max(source_width // 4, face_size)


def face_directions(face: str, size: int, start: int, stop: int) -> Directions:
    """Return the directions through the centers of a band of face texels.

    The directions are not normalized; only their angles matter.

    :param face: One of :data:`FACES`.
    :param size: Edge length of the face in texels.
    :param start: First row of the band.
    :param stop: Row after the last row of the band.
    :returns: ``x``, ``y`` and ``z`` arrays of shape ``(stop - start, size)``.
    :raises KeyError: If ``face`` is not a face name.
    """
    warped = np.tan(((np.arange(size) + 0.5) / size * 2 - 1) * (np.pi / 4))
    shape = (stop - start, size)
    s = np.broadcast_to(warped[None, :], shape)
    t = np.broadcast_to(warped[start:stop, None], shape)
    return _DIRECTIONS[face](s, t, np.ones(shape))


def equirect_coordinates(
    x: np.ndarray,
    y: np.ndarray,
    z: np.ndarray,
    width: int,
    height: int,
) -> tuple[np.ndarray, np.ndarray]:
    """Return the continuous source pixel coordinates of directions.

    :param x: Direction components toward longitude 90 E.
    :param y: Direction components toward the north pole.
    :param z: Direction components toward longitude 0.
    :param width: Source width in pixels.
    :param height: Source height in pixels.
    :returns: ``row`` and ``col`` arrays, with pixel centers at whole numbers.
    """
    longitude = np.arctan2(x, z)
    latitude = np.arctan2(y, np.hypot(x, z))
    col = (longitude + np.pi) * (width / (2 * np.pi)) - 0.5
    row = (np.pi / 2 - latitude) * (height / np.pi) - 0.5
    return row, col


def sample_bilinear(source: np.ndarray, row: np.ndarray, col: np.ndarray) -> np.ndarray:
    """Sample an equirectangular array bilinearly at continuous coordinates.

    Columns wrap around the antimeridian and rows clamp at the poles.

    :param source: uint8 array of shape ``(H, W)`` or ``(H, W, C)``.
    :param row: Row coordinates, pixel centers at whole numbers.
    :param col: Column coordinates of the same shape.
    :returns: uint8 array of the coordinates' shape, plus ``C`` if present.
    """
    height, width = source.shape[:2]
    row0 = np.floor(row)
    col0 = np.floor(col)
    frow = (row - row0).astype(np.float32)
    fcol = (col - col0).astype(np.float32)
    row0 = row0.astype(np.intp)
    col0 = col0.astype(np.intp)
    above = np.clip(row0, 0, height - 1)
    below = np.clip(row0 + 1, 0, height - 1)
    left = col0 % width
    right = (col0 + 1) % width
    if source.ndim == 3:
        frow = frow[..., None]
        fcol = fcol[..., None]
    top = source[above, left] * (1 - fcol) + source[above, right] * fcol
    bottom = source[below, left] * (1 - fcol) + source[below, right] * fcol
    value = top * (1 - frow) + bottom * frow
    return np.clip(value + 0.5, 0, 255).astype(np.uint8)


def reproject_face(
    source: np.ndarray,
    face: str,
    size: int,
    pool: Executor | None = None,
) -> np.ndarray:
    """Resample one cube face from an equirectangular array.

    Each texel center is mapped back into the source and sampled bilinearly,
    one band of :data:`BAND_ROWS` rows at a time.

    :param source: uint8 array of shape ``(H, W)`` or ``(H, W, C)``, 2:1.
    :param face: One of :data:`FACES`.
    :param size: Edge length of the face in texels.
    :param pool: Runs the bands concurrently when given.
    :returns: uint8 array of shape ``(size, size)``, plus ``C`` if present.
    """
    height, width = source.shape[:2]
    out = np.empty((size, size, *source.shape[2:]), dtype=np.uint8)

    def fill(start: int) -> None:
        stop = min(start + BAND_ROWS, size)
        x, y, z = face_directions(face, size, start, stop)
        row, col = equirect_coordinates(x, y, z, width, height)
        out[start:stop] = sample_bilinear(source, row, col)

    starts = range(0, size, BAND_ROWS)
    if pool is None:
        for start in starts:
            fill(start)
    else:
        list(pool.map(fill, starts))
    return out


def area_average(face: np.ndarray, size: int) -> np.ndarray:
    """Shrink a square face by averaging over each output texel's footprint.

    :param face: uint8 array of shape ``(N, N)`` or ``(N, N, 3)``.
    :param size: Edge length of the result, at most ``N``.
    :returns: uint8 array of shape ``(size, size)``, plus the channels.
    """
    if face.shape[0] == size:
        return face
    image = Image.fromarray(face).resize((size, size), Image.Resampling.BOX)
    return np.asarray(image)
