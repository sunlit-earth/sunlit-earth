"""A preview harness for the look of the sea ice.

A variant is a set of options of the bake. Each one starts from the shipped
textures and rebakes what four views read: the day face ``py`` in the north's
two months and ``ny`` in the south's, with the production settings, and the
water mask, from all twelve months. It assembles a texture directory with those
faces in place, renders the four views through the app's headless ``render``
with a tile cache of its own, and measures the faces. Whatever a variant does
not change is cached under ``<root>/cache``: the shapefile's raster, the masks,
the resampled sources and the noise fields.

``<root>/sources.toml`` names the inputs, relative to the root or absolute::

    blue_marble = "..."   # the twelve monthly JPEGs of 2004
    ocean_mask = "..."    # ne_10m_ocean.shp
    sea_ice = "..."       # the 24 OSI-450-a1 files
    textures = "..."      # the shipped textures
    app = "..."           # a release build of sunlit-earth
"""

import ast
import dataclasses
import datetime
import functools
import hashlib
import inspect
import json
import math
import os
import shutil
import subprocess
import sys
import textwrap
import threading
import time
import tomllib
import tracemalloc
import types
import typing
from collections.abc import Callable, Iterable, Iterator, Mapping
from concurrent.futures import Executor, ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np
import scipy
from PIL import Image
from scipy.ndimage import (
    binary_dilation,
    binary_erosion,
    distance_transform_edt,
    gaussian_filter,
    label,
    map_coordinates,
    zoom,
)

from texture_pipeline import ocean_masking, parallel, sea_ice, sea_ice_look
from texture_pipeline.cube import FACES, area_average, face_directions, reproject_face
from texture_pipeline.discovery import discover_months
from texture_pipeline.processing import encode_jxl

Image.MAX_IMAGE_PIXELS = None

FACE_SIZE = 2048
QUALITY = 85
EFFORT = 7
FILL: sea_ice_look.Rgb = (10, 30, 60)
SUPERSAMPLE = 2
ICE_LUMINANCE = 200
ICE_LATITUDE = 60.0

RENDER_SIZE = 2048
FOV_DEG = 20.0
FRAME_LATITUDE = 55.0
CAMERA_LATITUDE = 89.9
"""The app clamps the camera's latitude to 89.9 degrees, short of the pole
where its view matrix has no up direction."""

ZOOM_DISTANCE_MIN = 1.5
ZOOM_DISTANCE_MAX = 80.0
CONFIG_VERSION = 2

MONTHS_AT_ONCE = 2
"""Views whose month a bake makes at once: a month holds about 2 GiB while it
is made."""

DRAFT_HALF = 512
"""Half the side of a draft's region, in texels of the working size: about
390 texels of the 2048 face, more than a detail crop shows."""

POOL = 8
"""Source pixels along each side of a pixel of the imagery water's overview."""
DAY_MODE = 1
YEAR = 2004

REC709 = np.array([0.2126, 0.7152, 0.0722], dtype=np.float32)

type Report = Callable[[str], None]


@dataclass(frozen=True)
class Sources:
    """Where the harness reads its inputs."""

    blue_marble: Path
    ocean_mask: Path
    sea_ice: Path
    textures: Path
    app: Path

    @classmethod
    def load(cls, root: Path) -> Sources:
        """Read ``sources.toml`` under the root.

        :param root: The harness's output directory.
        :returns: The paths, relative ones resolved against the root.
        """
        values = tomllib.loads((root / "sources.toml").read_text(encoding="utf-8"))
        paths = {
            name: (root / values[name]).resolve()
            for name in ("blue_marble", "ocean_mask", "sea_ice", "textures", "app")
        }
        return cls(**paths)


@dataclass(frozen=True)
class View:
    """One of the four renders."""

    key: str
    hemisphere: str
    season: str
    month: int
    longitude: float

    @property
    def face(self) -> str:
        """The face that holds the pole."""
        return "py" if self.hemisphere == "nh" else "ny"

    @property
    def latitude(self) -> float:
        """The camera's latitude."""
        return CAMERA_LATITUDE if self.hemisphere == "nh" else -CAMERA_LATITUDE

    @property
    def stamp(self) -> str:
        """The month's ``YYYYMM``."""
        return f"{YEAR}{self.month:02d}"

    @property
    def label(self) -> str:
        """What the page calls the render."""
        pole = "North" if self.hemisphere == "nh" else "South"
        name = datetime.date(YEAR, self.month, 1).strftime("%B")
        return f"{pole}, {self.season}, {name} {YEAR}"


def default_views(
    north_winter: int = 3,
    north_summer: int = 8,
    south_winter: int = 9,
    south_summer: int = 2,
    north_longitude: float = -40.0,
    south_longitude: float = 0.0,
) -> tuple[View, ...]:
    """Return the four views.

    :param north_winter: The month of the north's winter view.
    :param north_summer: The month of the north's summer view.
    :param south_winter: The month of the south's winter view.
    :param south_summer: The month of the south's summer view.
    :param north_longitude: The longitude at the bottom of the north's views,
        where Greenland is in the references.
    :param south_longitude: The longitude at the top of the south's views.
    :returns: North winter, north summer, south winter, south summer.
    """
    return (
        View("north-winter", "nh", "winter", north_winter, north_longitude),
        View("north-summer", "nh", "summer", north_summer, north_longitude),
        View("south-winter", "sh", "winter", south_winter, south_longitude),
        View("south-summer", "sh", "summer", south_summer, south_longitude),
    )


@dataclass(frozen=True)
class Variant:
    """A set of bake options the user compares against the baseline."""

    name: str
    summary: str
    """What the variant changes, for its notes."""

    imagery_water: bool = True
    """Turn the Blue Marble's water on the shapefile's land side into water,
    as the production bake does."""

    imagery_water_latitude: float = ocean_masking.IMAGERY_WATER_LATITUDE
    """Poleward of this latitude only."""

    snow_colors: bool = False
    """Measure each hemisphere's winter ice color from its coastal snow
    again, in place of the colors of :data:`sea_ice_look.LOOK`, and move
    every month by the same ratio."""

    south_toward_north: float = 0.0
    """With :attr:`snow_colors`, how far the south's winter color moves from
    its coastal snow's toward the north's: 0.5 is the midpoint."""

    north_darkening: float = 1.0
    """With :attr:`snow_colors`, the share of the north's darkening against
    its winter color that each month keeps, per channel."""

    look: dict[str, object] = field(default_factory=dict)
    """Tunables of :class:`sea_ice_look.Look` that differ from the bake's."""

    tests: tuple[str, ...] = ()
    """Tests whose pinned behavior the variant changes, and why."""

    parent: str | None = None
    """The variant this one builds on, which its faces are compared with too."""

    remarks: tuple[str, ...] = ()
    """What inspecting the variant's results found, for its notes."""

    def derive(
        self,
        name: str,
        summary: str,
        *,
        look: dict[str, object] | None = None,
        tests: tuple[str, ...] = (),
        remarks: tuple[str, ...] = (),
        **options: object,
    ) -> Variant:
        """Return a variant that builds on this one.

        It takes every option of this one, its tunables merged with ``look``
        and its tests followed by ``tests``, unless ``options`` names another
        value; the summary and the remarks are its own.

        :param name: The new variant's name.
        :param summary: What it changes against this one.
        :param look: Tunables that differ from this one's.
        :param tests: Tests it changes beyond this one's.
        :param remarks: Its own findings.
        :param options: Other fields that differ from this one's.
        :returns: The new variant.
        """
        return dataclasses.replace(
            self,
            name=name,
            summary=summary,
            look={**self.look, **(look or {})},
            tests=self.tests + tests,
            parent=self.name,
            remarks=remarks,
            **options,
        )


BASELINE = Variant(
    "baseline",
    "The production bake, unchanged: the shapefile's mask joined by the Blue "
    "Marble's own water poleward of 50 degrees, less the Antarctic ice "
    "shelves, and the sea ice of `cube --sea-ice`.",
)

VARIANTS: dict[str, Variant] = {v.name: v for v in (BASELINE,)}


_NETCDF = threading.Lock()
"""netCDF4's HDF5 is not thread-safe: two threads reading the record at once
crash the process."""


def read_record(directory: Path, hemisphere: str) -> sea_ice.Hemisphere:
    """Read a hemisphere's record, one thread at a time."""
    with _NETCDF:
        return sea_ice.read_hemisphere(directory, hemisphere)


def _key(*parts: object) -> str:
    return hashlib.sha256(repr(parts).encode()).hexdigest()[:12]


def _timed[T](times: dict[str, float], name: str, run: Callable[[], T]) -> T:
    started = time.monotonic()
    result = run()
    times[name] = round(times.get(name, 0.0) + time.monotonic() - started, 1)
    return result


def _open_rgb(path: Path) -> np.ndarray:
    with Image.open(path) as image:
        return np.asarray(image.convert("RGB"))


def _open_gray(path: Path) -> np.ndarray:
    with Image.open(path) as image:
        return np.asarray(image.convert("L"))


def luma(rgb: np.ndarray) -> np.ndarray:
    """Return Rec. 709 luminance of RGB values.

    :param rgb: Array of any shape ending in 3.
    :returns: float32 array without the last axis.
    """
    return rgb.astype(np.float32) @ REC709


def face_latitude(face: str, size: int) -> np.ndarray:
    """Return the latitude in degrees of every texel of a face."""
    x, y, z = face_directions(face, size, 0, size)
    return np.degrees(np.arctan2(y, np.hypot(x, z))).astype(np.float32)


class Cache:
    """Arrays kept between runs, under ``<root>/cache``."""

    def __init__(self, root: Path) -> None:
        self.dir = root / "cache"
        self.dir.mkdir(parents=True, exist_ok=True)

    def array(self, name: str, make: Callable[[], np.ndarray]) -> np.ndarray:
        """Return a cached array, making and saving it when missing.

        :param name: The file name without suffix.
        :param make: Makes the array.
        :returns: The array.
        """
        path = self.dir / f"{name}.npy"
        if path.exists():
            return np.load(path)
        value = make()
        partial = path.with_suffix(".partial.npy")
        np.save(partial, value)
        partial.replace(path)
        return value

    def arrays(
        self, name: str, make: Callable[[], dict[str, np.ndarray]]
    ) -> dict[str, np.ndarray]:
        """Return cached named arrays, making and saving them when missing."""
        path = self.dir / f"{name}.npz"
        if path.exists():
            with np.load(path) as data:
                return {k: data[k] for k in data.files}
        value = make()
        partial = path.with_suffix(".partial.npz")
        np.savez(partial, **typing.cast("dict[str, typing.Any]", value))
        partial.replace(path)
        return value

    def record(self, name: str, make: Callable[[], dict]) -> dict:
        """Return a cached JSON record, making and saving it when missing."""
        path = self.dir / f"{name}.json"
        if path.exists():
            return json.loads(path.read_text(encoding="utf-8"))
        value = make()
        path.write_text(json.dumps(value, indent=2), encoding="utf-8", newline="\n")
        return value


class Harness:
    """Runs a variant end to end.

    The noise fields are kept per part and per hemisphere, each under a key
    of the tunables and the code that part reads, so a change to one tunable
    makes only the parts that read it again. The two hemispheres' fields, the
    four views' bakes, renders and measurements run at once, each step on up
    to ``workers`` threads; none of it changes a byte of the results, and
    ``workers=1`` runs everything in turn.
    """

    def __init__(
        self,
        root: Path,
        variant: Variant,
        views: tuple[View, ...],
        report: Report = print,
        workers: int | None = None,
        warm_check: bool = False,
    ) -> None:
        """Open a harness.

        :param root: The harness's directory, holding ``sources.toml``.
        :param variant: The variant to run.
        :param views: The four views.
        :param report: Told what is happening.
        :param workers: Threads; the process's setting when ``None``.
        :param warm_check: Render in turn on one tile cache and once more
            warm, to measure what the tile packs cost; otherwise the views
            render at once, each on a tile cache of its own.
        """
        self.workers = parallel.workers(workers)
        self.warm_check = warm_check
        self.root = root
        self.variant = variant
        self.views = views
        self.report = report
        self.sources = Sources.load(root)
        self.cache = Cache(root)
        self.out = root / variant.name
        self.times: dict[str, float] = {}
        self.months = discover_months(self.sources.blue_marble)
        self.notes: dict[str, typing.Any] = {}
        self._fields: dict[str, tuple[sea_ice_look.Look, sea_ice_look.Fields]] = {}
        self._fields_notes: dict[str, dict] = {}
        self._locks = {h: threading.Lock() for h in sea_ice.HEMISPHERES}
        self._coast_gap_lock = threading.Lock()
        self._months = threading.Semaphore(MONTHS_AT_ONCE)

    # The source raster and the masks.

    def raster(self) -> np.ndarray:
        """The shapefile rasterized at the source's size, as the bake does."""

        def make() -> np.ndarray:
            self.report("Rasterizing the shapefile")
            width, height = self.source_size
            return ocean_masking.rasterize_ocean_mask(
                self.sources.ocean_mask, width, height, supersample=SUPERSAMPLE
            )

        return self.cache.array("ocean-raster", make)

    @property
    def source_size(self) -> tuple[int, int]:
        """The monthly sources' width and height."""
        with Image.open(next(iter(self.months.values()))) as image:
            return image.size

    def yearly_water(self) -> dict[str, np.ndarray]:
        """Each source pixel's brightest luminance and least blue over the year."""

        def make() -> dict[str, np.ndarray]:
            stats = ocean_masking.YearlyWater.empty(*reversed(self.source_size))
            for stamp, path in self.months.items():
                self.report(f"Yearly water: {stamp}")
                stats.add(_open_rgb(path))
            return {"max_luma": stats.max_luma, "min_blue": stats.min_blue}

        return self.cache.arrays("yearly-water", make)

    def mask_key(self, imagery_water: bool | None = None) -> str:
        """The key of a source mask, the variant's unless told otherwise.

        :param imagery_water: Whether the mask joins the imagery's water; the
            variant's choice when ``None``.
        """
        v = self.variant
        joined = v.imagery_water if imagery_water is None else imagery_water
        imagery = (
            (
                v.imagery_water_latitude,
                ocean_masking.IMAGERY_WATER,
                code_digest(ocean_masking.join_imagery_water),
            )
            if joined
            else None
        )
        return _key("mask", SUPERSAMPLE, ICE_LUMINANCE, ICE_LATITUDE, imagery)

    def add_imagery_water(self, mask: np.ndarray, key: str) -> None:
        """Join the imagery's water to the ocean poleward of the variant's
        latitude, in place, as the production bake does, and count it."""
        yearly = self.yearly_water()
        stats = ocean_masking.YearlyWater(yearly["max_luma"], yearly["min_blue"])
        height, width = mask.shape
        before = mask.copy()
        joined = ocean_masking.join_imagery_water(
            mask, stats, self.variant.imagery_water_latitude
        )
        rows, cols = np.nonzero(mask != before)
        del before, stats, yearly
        pixel_km2 = (2 * np.pi * sea_ice.AUTHALIC_RADIUS_KM / width) * (
            np.pi * sea_ice.AUTHALIC_RADIUS_KM / height
        )
        flags = np.zeros((height // POOL, width // POOL), dtype=np.uint8)
        flags[
            np.minimum(rows // POOL, flags.shape[0] - 1),
            np.minimum(cols // POOL, flags.shape[1] - 1),
        ] = 1
        counts = {}
        for name, here in (
            ("north", rows < height // 2),
            ("south", rows >= height // 2),
        ):
            lat = 90.0 - (rows[here] + 0.5) * 180.0 / height
            counts[name] = {
                "source_pixels": joined[name],
                "km2": round(float(np.cos(np.radians(lat)).sum() * pixel_km2)),
            }
        self.cache.array(f"imagery-water-flags-{key}", lambda: flags)
        self.cache.record(
            f"imagery-water-{key}",
            lambda: {
                "thresholds": dict(
                    zip(
                        ("max_luma", "min_blue"),
                        ocean_masking.IMAGERY_WATER,
                        strict=True,
                    )
                ),
                "joined": counts,
            },
        )

    def source_mask(self, imagery_water: bool | None = None) -> np.ndarray:
        """A water mask at the source's size, the variant's unless told otherwise.

        :param imagery_water: Whether the mask joins the imagery's water; the
            variant's choice when ``None``.
        """
        key = self.mask_key(imagery_water)
        joined = self.variant.imagery_water if imagery_water is None else imagery_water

        def make() -> np.ndarray:
            mask = self.raster().copy()
            if joined:
                self.add_imagery_water(mask, key)
            ice = np.zeros_like(mask)
            for stamp, path in self.months.items():
                self.report(f"Ice pass: {stamp}")
                found = ocean_masking.detect_ice_regions(
                    Image.open(path).convert("RGB"),
                    mask,
                    latitude_threshold=ICE_LATITUDE,
                    south_only=True,
                    seed_luminance=ICE_LUMINANCE,
                )
                np.maximum(ice, found, out=ice)
            return ocean_masking.reduce_mask_for_ice(mask, ice)

        return self.cache.array(f"mask-{key}", make)

    def water_faces(self, pool: Executor) -> dict[str, np.ndarray]:
        """The variant's mask resampled to every face at the working size."""
        key = self.mask_key()
        work = self.work_size

        def make() -> dict[str, np.ndarray]:
            mask = self.source_mask()
            self.report("Resampling the mask")
            return {face: reproject_face(mask, face, work, pool) for face in FACES}

        return self.cache.arrays(f"water-{key}-{work}", make)

    def water_face(self, face: str, pool: Executor) -> np.ndarray:
        """One face of the variant's mask, without reading the others."""
        path = self.cache.dir / f"water-{self.mask_key()}-{self.work_size}.npz"
        if path.exists():
            with np.load(path) as data:
                return data[face]
        return self.water_faces(pool)[face]

    @property
    def work_size(self) -> int:
        """The size the faces are resampled at before the average."""
        width, _ = self.source_size
        return max(width // 4, FACE_SIZE)

    # The sources and the ice.

    def source_face(self, stamp: str, face: str, pool: Executor) -> np.ndarray:
        """One month's face at the working size."""

        def make() -> np.ndarray:
            self.report(f"Resampling {stamp} {face}")
            source = _open_rgb(self.months[stamp])
            return reproject_face(source, face, self.work_size, pool)

        return self.cache.array(f"source-{stamp}-{face}-{self.work_size}", make)

    def source_face_small(self, stamp: str, face: str, pool: Executor) -> np.ndarray:
        """One month's face averaged to the face size, without the flatten.

        The working size face is cached only when a view's bake made it.
        """

        def make() -> np.ndarray:
            cached = self.cache.dir / f"source-{stamp}-{face}-{self.work_size}.npy"
            if cached.exists():
                return area_average(np.load(cached), FACE_SIZE)
            self.report(f"Resampling {stamp} {face}")
            source = _open_rgb(self.months[stamp])
            return area_average(
                reproject_face(source, face, self.work_size, pool), FACE_SIZE
            )

        return self.cache.array(f"source-{stamp}-{face}-{FACE_SIZE}", make)

    def look(
        self, colors: dict[str, tuple[sea_ice_look.Rgb, ...]] | None
    ) -> sea_ice_look.Look:
        """The variant's tunables."""
        look = dataclasses.replace(sea_ice_look.LOOK, **self.variant.look)
        if colors is not None:
            look = dataclasses.replace(look, colors=colors)
        return look

    def record(
        self, hemisphere: str, look: sea_ice_look.Look
    ) -> tuple[np.ndarray, str]:
        """A hemisphere's concentration on the coarse cap, with the fast ice
        band, and the key it is kept under."""
        key = record_key(self.sources.sea_ice, hemisphere, look)

        def make() -> np.ndarray:
            read = read_record(self.sources.sea_ice, hemisphere)
            return sea_ice.cap_concentration(sea_ice_look.fast_ice(read, look))

        return self.cache.array(f"record-{hemisphere}-{key}", make), key

    def fields(self, hemisphere: str, look: sea_ice_look.Look) -> sea_ice_look.Fields:
        """A hemisphere's noise fields, each part of them cached by the
        tunables and the code that make it."""
        with self._locks[hemisphere]:
            kept = self._fields.get(hemisphere)
            if kept is not None and kept[0] == look:
                return kept[1]
            concentration, record = self.record(hemisphere, look)
            land, land_key = self.cap_land(hemisphere)
            keys = part_keys(look, record, sea_ice.CAP_SIZE, land_key)
            tracing = self.workers == 1

            def cached(
                part: sea_ice_look.Part, make: Callable[[], dict[str, object]]
            ) -> dict[str, object]:
                name = f"fields-{hemisphere}-{part.name}-{keys[part.name]}"
                made: dict[str, typing.Any] = {}

                def build() -> dict[str, np.ndarray]:
                    self.report(f"Noise fields: {hemisphere}, {part.name}")
                    started = time.monotonic()
                    if tracing:
                        tracemalloc.start()
                    made["part"] = make()
                    made["seconds"] = round(time.monotonic() - started, 1)
                    if tracing:
                        made["peak"] = tracemalloc.get_traced_memory()[1]
                        tracemalloc.stop()
                    return part_to_arrays(made["part"])

                arrays = self.cache.arrays(name, build)
                if "part" not in made:
                    return parts_from_arrays(arrays)
                self.cache.record(
                    f"{name}-made",
                    lambda: {
                        "seconds": made["seconds"],
                        "peak_gib": (
                            round(made["peak"] / 2**30, 2) if "peak" in made else None
                        ),
                        "mib": {
                            k: round(v.nbytes / 2**20, 1) for k, v in arrays.items()
                        },
                    },
                )
                return made["part"]

            fields = sea_ice_look.prepare(
                concentration,
                hemisphere,
                sea_ice.CAP_SIZE,
                look,
                workers=self.workers,
                cache=cached,
                land=land,
            )
            self._fields_notes[hemisphere] = self.fields_note(
                hemisphere, look, keys, fields
            )
            self._fields[hemisphere] = (look, fields)
            return fields

    def cap_land(self, hemisphere: str) -> tuple[np.ndarray, str]:
        """The land on a hemisphere's cap from the variant's mask faces, and
        the key it is kept under."""
        key = _key(
            "cap-land",
            self.mask_key(),
            self.work_size,
            sea_ice.CAP_SIZE,
            code_digest(sea_ice_look.cap_land),
        )

        def make() -> np.ndarray:
            with ThreadPoolExecutor(max_workers=self.workers) as pool:
                water = self.water_faces(pool)
            self.report(f"Land on the cap: {hemisphere}")
            return sea_ice_look.cap_land(
                water, hemisphere, sea_ice.CAP_SIZE, self.workers
            )

        return self.cache.array(f"cap-land-{hemisphere}-{key}", make), key

    def fields_note(
        self,
        hemisphere: str,
        look: sea_ice_look.Look,
        keys: dict[str, str],
        fields: sea_ice_look.Fields,
    ) -> dict:
        """What the parts of a hemisphere's fields cost when they were made."""
        seconds: dict[str, float] = {}
        peaks: list[float] = []
        for part in sea_ice_look.PARTS:
            path = (
                self.cache.dir
                / f"fields-{hemisphere}-{part.name}-{keys[part.name]}-made.json"
            )
            if not path.exists():
                continue
            made = json.loads(path.read_text(encoding="utf-8"))
            seconds[part.name] = made["seconds"]
            if made["peak_gib"] is not None:
                peaks.append(made["peak_gib"])
        return {
            "seconds": seconds,
            "peak_gib": max(peaks) if peaks else None,
            "mib": {
                k: round(v.nbytes / 2**20, 1)
                for k, v in fields_to_arrays(fields).items()
            },
        }

    def fields_both(self, look: sea_ice_look.Look) -> None:
        """Make or load the fields of the views' hemispheres, at once."""
        names = [
            h for h in sea_ice.HEMISPHERES if any(v.hemisphere == h for v in self.views)
        ]
        parallel.pmap(lambda h: self.fields(h, look), names, self.workers)
        self.notes["fields"] = {h: self._fields_notes[h] for h in names}

    # The run.

    def run(self) -> dict:
        """Bake, assemble, render and measure the variant.

        :returns: The metrics, as written to ``metrics.json``.
        """
        started = time.monotonic()
        if self.out.exists():
            shutil.rmtree(self.out)
        (self.out / "faces").mkdir(parents=True)
        with parallel.configured(self.workers):
            with ThreadPoolExecutor(max_workers=self.workers) as pool:
                water = _timed(self.times, "mask", lambda: self.water_faces(pool))
                colors = _timed(self.times, "colors", lambda: self.colors(water, pool))
                look = self.look(colors)
                _timed(self.times, "fields", lambda: self.fields_both(look))
                baked = _timed(self.times, "bake", lambda: self.bake(water, look, pool))
                masks = _timed(self.times, "mask faces", lambda: self.mask_faces(water))
            self._fields.clear()
            textures = _timed(
                self.times, "assemble", lambda: self.assemble(baked, masks)
            )
            renders = self.render_all(textures)
            metrics = _timed(
                self.times, "metrics", lambda: self.measure(textures, baked, renders)
            )
        metrics["time_s"] = dict(self.times, total=round(time.monotonic() - started, 1))
        (self.out / "metrics.json").write_text(
            json.dumps(metrics, indent=2), encoding="utf-8", newline="\n"
        )
        self.write_notes(metrics)
        return metrics

    def colors(
        self, water: dict[str, np.ndarray] | None, pool: Executor
    ) -> dict[str, tuple[sea_ice_look.Rgb, ...]] | None:
        """The ice's colors when the variant takes them from the snow, kept by
        the mask, the table and the code that measures the snow."""
        if not self.variant.snow_colors:
            return None
        table = dataclasses.replace(sea_ice_look.LOOK, **self.variant.look).colors
        sources = [(m, p.name, p.stat().st_size) for m, p in self.months.items()]
        moved = (self.variant.south_toward_north, self.variant.north_darkening)
        key = _key(
            "snow-colors",
            self.mask_key(),
            self.work_size,
            FACE_SIZE,
            table,
            sources,
            moved,
            code_digest(
                coastal_snow,
                sea_ice_look.winter_months,
                sea_ice_look.rescaled_colors,
                snow_tables,
                area_average,
                luma,
                face_latitude,
            ),
        )

        def make() -> dict:
            faces = water if water is not None else self.water_faces(pool)
            colors, snow = self.snow_colors(table, faces, pool)
            return json.loads(json.dumps({"colors": colors, "snow": snow}))

        stored = self.cache.record(f"snow-colors-{key}", make)
        self.notes["snow_colors"] = stored["snow"]
        return {
            hemisphere: tuple((r, g, b) for r, g, b in rgb)
            for hemisphere, rgb in stored["colors"].items()
        }

    def snow_colors(
        self,
        table: dict[str, tuple[sea_ice_look.Rgb, ...]],
        water: dict[str, np.ndarray],
        pool: Executor,
    ) -> tuple[dict, dict]:
        """Measure the coastal snow of each hemisphere's winter months, and
        move the winter colors and the north's darkening as the variant
        says."""
        snow = {}
        for hemisphere, face, min_lat in (("nh", "py", 70.0), ("sh", "ny", 60.0)):
            months = sea_ice_look.winter_months(table[hemisphere])
            mask = area_average(water[face], FACE_SIZE)
            pooled = []
            per_month = {}
            for month in months:
                rgb = self.source_face_small(f"{YEAR}{month:02d}", face, pool)
                found = coastal_snow(rgb, mask, face, min_lat)
                per_month[month] = _rgb_list(np.median(found, axis=0))
                pooled.append(found)
            median = np.median(np.concatenate(pooled), axis=0)
            red, green, blue = (round(float(c)) for c in median)
            snow[hemisphere] = {
                "face": face,
                "min_latitude": min_lat,
                "months": months,
                "per_month_median": per_month,
                "pooled_median": (red, green, blue),
                "texels": int(sum(len(p) for p in pooled)),
                "table_before": [list(c) for c in table[hemisphere]],
            }
        colors, winters = snow_tables(
            table,
            {h: s["pooled_median"] for h, s in snow.items()},
            self.variant.south_toward_north,
            self.variant.north_darkening,
        )
        for hemisphere, s in snow.items():
            s["winter"] = winters[hemisphere]
            s["darkening"] = self.variant.north_darkening if hemisphere == "nh" else 1
            s["table_after"] = [list(c) for c in colors[hemisphere]]
        return colors, snow

    def bake(
        self, water: dict[str, np.ndarray], look: sea_ice_look.Look, pool: Executor
    ) -> dict[str, dict]:
        """Rebake each view's face, and measure the ice's area on every face.

        The views run at once; what each one notes is put together in the
        views' order, so the notes do not depend on which finished first.
        """
        self.fields_both(look)
        results = parallel.pmap(
            lambda view: self.bake_view(view, water, look, pool),
            self.views,
            self.workers,
        )
        baked = {}
        for view, (entry, notes) in zip(self.views, results, strict=True):
            baked[view.key] = entry
            for name, value in notes.items():
                section = self.notes.setdefault(name, {})
                section[view.hemisphere if name == "fractures" else view.key] = value
        return baked

    def bake_view(
        self,
        view: View,
        water: dict[str, np.ndarray],
        look: sea_ice_look.Look,
        pool: Executor,
    ) -> tuple[dict, dict]:
        """Rebake one view's face.

        :returns: The baked face's paths and ice areas, and what the bake
            noted about the view.
        """
        stage = time.monotonic()
        notes: dict[str, object] = {}
        fields = self.fields(view.hemisphere, look)
        with self._months:
            cap = sea_ice_look.month_cap(fields, view.month, look, workers=self.workers)
        notes["fractures"] = fracture_stats(fields.fractures)
        assert cap.floes is not None
        pack = (cap.floes > 0.5) & ~fields.coast.land
        leads = fields.fractures.open[pack].astype(np.float32)
        floes = cap.floes[pack]
        share = float((leads * floes).sum()) / max(float(floes.sum()), 1e-9)
        notes["lead_area"] = round(share, 5)
        notes["activity"] = activity_shares(cap)
        notes["pack_streaks"] = pack_streaks(fields, cap, view.month, self.workers)
        cap = dataclasses.replace(cap, activity=None, floes=None)
        ice = sea_ice_look.MonthIce({view.hemisphere: cap})
        source = self.source_face(view.stamp, view.face, pool)
        flat = np.array(
            ocean_masking.apply_ocean_mask(
                Image.fromarray(source), water[view.face], color=FILL
            )
        )
        notes["coast_blend"] = self.blend_coast(
            view, flat, source, water[view.face], ice, look, pool
        )
        area = sea_ice_look.composite_face(
            flat, water[view.face], view.face, ice, FILL, pool
        )[view.hemisphere]
        with self._coast_gap_lock:
            notes["coast_gaps"] = coast_gaps(flat, water[view.face], view.face)
        for face in FACES:
            if face != view.face:
                area += ice_area(water[face], face, ice, view.hemisphere, pool)
        del fields, cap
        small = area_average(flat, FACE_SIZE)
        pre = self.out / "faces" / f"{view.stamp}-{view.face}.png"
        Image.fromarray(small).save(pre)
        encoded = self.out / "faces" / f"{view.stamp}-{view.face}.jxl"
        encode_jxl(Image.fromarray(small), encoded, quality=QUALITY, effort=EFFORT)
        record = read_record(self.sources.sea_ice, view.hemisphere)
        source_km2 = float(sea_ice.source_area_km2(record)[view.month - 1])
        toned_km2 = float(sea_ice_look.toned_area_km2(record, look)[view.month - 1])
        self.report(
            f"Baked {view.key}: {time.monotonic() - stage:.1f}s, ice "
            f"{area / 1e6:.2f} million km2 against {source_km2 / 1e6:.2f} in the "
            f"record and {toned_km2 / 1e6:.2f} toned"
        )
        entry = {
            "preencode": pre,
            "encoded": encoded,
            "area_km2": area,
            "source_km2": source_km2,
            "toned_km2": toned_km2,
        }
        return entry, notes

    def coast_weight(
        self,
        source: np.ndarray,
        water: np.ndarray,
        look: sea_ice_look.Look,
        spacing_km: float | None = None,
    ) -> np.ndarray:
        """The coast's weight for a face, kept by a hash of the pixels it is
        made from, the tunables it reads and the code that makes it."""
        digest = hashlib.blake2b(digest_size=16)
        for array in (source, water):
            digest.update(np.ascontiguousarray(array).data)
        read = tunables_read(sea_ice_look.coast_weight)
        tunables = tuple(
            (f.name, getattr(look, f.name))
            for f in dataclasses.fields(look)
            if f.name in read
        )
        key = _key(
            "coast-weight",
            digest.hexdigest(),
            source.shape,
            tunables,
            spacing_km,
            code_digest(sea_ice_look.coast_weight),
        )
        return self.cache.array(
            f"coast-weight-{key}",
            lambda: sea_ice_look.coast_weight(
                source, water, look, spacing_km, workers=self.workers
            ),
        )

    def blend_coast(
        self,
        view: View,
        flat: np.ndarray,
        source: np.ndarray,
        water: np.ndarray,
        ice: sea_ice_look.MonthIce,
        look: sea_ice_look.Look,
        pool: Executor,
    ) -> dict:
        """Blend the coast's land toward the ice, and draw and count the blend.

        :returns: What the blend did, for the view's notes.
        """
        started = time.monotonic()
        weight = self.coast_weight(source, water, look)
        applied = sea_ice_look.blend_coast(
            flat, source, water, view.face, ice, look, pool, fill=FILL, weight=weight
        )
        seconds = time.monotonic() - started
        small = area_average(np.rint(applied * 255).astype(np.uint8), FACE_SIZE)
        name = f"{view.key}-coast-blend.png"
        Image.fromarray(small).save(self.out / name)
        return {
            "seconds": round(seconds, 1),
            "texels_blended": int((applied > 0.01).sum()),
            "texels_mostly_ice": int((applied > 0.5).sum()),
            "working_size": int(water.shape[0]),
            "image": name,
        }

    def mask_faces(self, water: dict[str, np.ndarray]) -> dict[str, Path]:
        """Write the variant's mask faces, lossless, or copy them from the
        cache when the mask is one the harness has encoded."""
        key = _key(
            "mask-faces",
            self.mask_key(),
            self.work_size,
            FACE_SIZE,
            EFFORT,
            code_digest(encode_jxl, area_average),
        )
        kept = self.cache.dir / f"mask-faces-{key}"
        paths = {face: self.out / "faces" / f"mask-{face}.jxl" for face in FACES}
        if all((kept / path.name).exists() for path in paths.values()):
            for path in paths.values():
                shutil.copyfile(kept / path.name, path)
            return paths

        def encode(face: str) -> None:
            encode_jxl(
                Image.fromarray(area_average(water[face], FACE_SIZE)),
                paths[face],
                quality=100,
                effort=EFFORT,
            )

        parallel.pmap(encode, FACES, self.workers)
        kept.mkdir(exist_ok=True)
        for path in paths.values():
            shutil.copyfile(path, kept / path.name)
        return paths

    def assemble(self, baked: dict[str, dict], masks: dict[str, Path]) -> Path:
        """Copy the shipped textures and put the variant's faces in place."""
        textures = self.out / "textures"
        shutil.copytree(self.sources.textures, textures)
        for view in self.views:
            target = textures / "day" / view.stamp / f"{view.face}.jxl"
            target.unlink()
            shutil.copyfile(baked[view.key]["encoded"], target)
        for face, path in masks.items():
            target = textures / "mask" / f"{face}.jxl"
            target.unlink()
            shutil.copyfile(path, target)
        return textures

    # The renders.

    def render_all(self, textures: Path) -> dict[str, dict]:
        """Render every view, at once on tile caches of their own.

        With ``warm_check`` the views render in turn on one tile cache, and
        the first once more warm, which is how the packs' cost is measured.
        """
        if not self.warm_check:
            started = time.monotonic()

            def one(view: View) -> tuple[str, dict]:
                cache = self.out / f"tile-cache-{view.key}"
                return view.key, self.render(view, textures, cache, view.key)

            renders = dict(parallel.pmap(one, self.views, self.workers))
            self.times["renders"] = round(time.monotonic() - started, 1)
            return renders
        cache = self.out / "tile-cache"
        renders = {}
        for view in self.views:
            renders[view.key] = self.render(view, textures, cache, view.key)
        warm = self.render(self.views[0], textures, cache, "warm-check")
        (self.out / "warm-check.png").unlink()
        cold = sum(r["seconds"] for r in renders.values())
        self.times["renders"] = round(cold + warm["seconds"], 1)
        self.notes["pack_build_s"] = round(cold - len(self.views) * warm["seconds"], 1)
        self.notes["warm_render_s"] = warm["seconds"]
        return renders

    def render(self, view: View, textures: Path, cache: Path, name: str) -> dict:
        """Render one view through the app."""
        work = self.out / "render"
        work.mkdir(exist_ok=True)
        config = work / f"{name}.toml"
        config.write_text(render_config(view), encoding="utf-8", newline="\n")
        output = self.out / f"{name}.png"
        env = dict(
            os.environ,
            SUNLIT_EARTH_TEXTURES=str(textures),
            SUNLIT_EARTH_CACHE_DIR=str(cache),
            SUNLIT_EARTH_CONFIG=str(work / "unused-config.toml"),
            SUNLIT_EARTH_METRICS_DIR=str(work / "metrics"),
            SUNLIT_EARTH_NO_CLOUDS="1",
        )
        args = [
            str(self.sources.app),
            "--quality",
            "high",
            "render",
            "--output",
            str(output),
            "--width",
            str(RENDER_SIZE),
            "--height",
            str(RENDER_SIZE),
            "--config",
            str(config),
        ]
        started = time.monotonic()
        code = run_in_own_console(args, env)
        seconds = round(time.monotonic() - started, 1)
        if code != 0 or not output.exists():
            msg = f"The render of {name} failed with exit code {code}."
            raise RuntimeError(msg)
        self.report(f"Rendered {name}: {seconds}s")
        return {"path": output, "seconds": seconds}

    # The draft.

    def draft(
        self,
        view: View,
        place: tuple[str, float, float],
        half: int = DRAFT_HALF,
        out: Path | None = None,
    ) -> dict:
        """Draw the region around a place of one view's face, in seconds.

        Nothing is encoded, rendered or measured. The fields come from the
        cache, the month's ice is made for the cap rows the region reads, and
        the region of the face at the working size is blended with the coast
        and composited as the bake does it, so a texel is the one the bake
        would write there.

        :param view: The view, whose face and month are drawn.
        :param place: A name, a latitude and a longitude, as :data:`PLACES`
            lists them.
        :param half: Half the region's side in texels of the working size.
        :param out: Where to write the PNG; under the variant's directory by
            default.
        :returns: The PNG's path, the region's window, and the seconds each
            stage took.
        """
        stages: dict[str, float] = {}
        mark = [time.monotonic()]

        def tick(name: str) -> None:
            now = time.monotonic()
            stages[name] = round(now - mark[0], 2)
            mark[0] = now

        name, latitude, longitude = place
        if (latitude > 0) != (view.hemisphere == "nh"):
            msg = f"{name} is not in the {view.hemisphere} hemisphere of {view.key}."
            raise ValueError(msg)
        size = self.work_size
        window = region_window(view.face, latitude, longitude, size, half)
        with (
            parallel.configured(self.workers),
            ThreadPoolExecutor(max_workers=self.workers) as pool,
        ):
            look = self.look(self.colors(None, pool))
            tick("colors")
            fields = self.fields(view.hemisphere, look)
            tick("fields")
            cap = sea_ice_look.month_cap(
                fields,
                view.month,
                look,
                row_range=cap_rows(view.face, window, fields.size),
                workers=self.workers,
            )
            tick("month")
            kept = self.cache.dir / f"source-{view.stamp}-{view.face}-{size}.npy"
            if not kept.exists():
                self.source_face(view.stamp, view.face, pool)
            full_source = np.load(kept, mmap_mode="r")
            full_water = self.water_face(view.face, pool)
            tick("sources")
            ice = sea_ice_look.MonthIce({view.hemisphere: cap})
            rows, columns = window.rows, window.columns
            spacing = np.pi / 2 * sea_ice.AUTHALIC_RADIUS_KM / size
            reach = 4 * look.coast_window_km + max(
                look.coast_reach_km[1], look.coast_inner_km[1]
            )
            margin = max(int(np.ceil(reach / spacing)) + 8, 64)
            top, bottom = max(rows[0] - margin, 0), min(rows[1] + margin, size)
            left, right = max(columns[0] - margin, 0), min(columns[1] + margin, size)
            inner = (
                slice(rows[0] - top, rows[1] - top),
                slice(columns[0] - left, columns[1] - left),
            )
            slab_source = np.ascontiguousarray(full_source[top:bottom, left:right])
            slab_water = np.ascontiguousarray(full_water[top:bottom, left:right])
            source = np.ascontiguousarray(slab_source[inner])
            water = np.ascontiguousarray(slab_water[inner])
            flat = np.array(
                ocean_masking.apply_ocean_mask(
                    Image.fromarray(source), water, color=FILL
                )
            )
            tick("slice")
            weight = sea_ice_look.coast_weight(
                slab_source, slab_water, look, spacing, workers=self.workers
            )
            tick("coast weight")
            sea_ice_look.blend_coast(
                flat,
                source,
                water,
                view.face,
                ice,
                look,
                pool,
                fill=FILL,
                weight=np.ascontiguousarray(weight[inner]),
                window=window,
            )
            tick("blend")
            sea_ice_look.composite_face(
                flat, water, view.face, ice, FILL, pool, window=window
            )
            tick("composite")
        target = out or self.out / f"draft-{view.key}-{name}.png"
        target.parent.mkdir(parents=True, exist_ok=True)
        Image.fromarray(flat).save(target)
        tick("save")
        return {"path": target, "window": window, "seconds": stages}

    # The measurements.

    def measure(
        self, textures: Path, baked: dict[str, dict], renders: dict[str, dict]
    ) -> dict:
        """Measure the variant's faces and renders, the views and the global
        checks at once."""
        baseline = self.root / "baseline"
        metrics: dict = {
            "variant": self.variant.name,
            "views": {},
            "dark_land": {},
        }
        compared = baseline != self.out and (baseline / "faces").exists()
        parent = self.variant.parent
        crop_parent = self.root / parent if parent not in (None, "baseline") else None
        with ThreadPoolExecutor(max_workers=self.workers) as pool:
            dark = {
                face: pool.submit(dark_land, textures, face) for face in ("py", "ny")
            }
            views = [
                pool.submit(self.measure_view, view, textures, baked, renders)
                for view in self.views
            ]
            changed = pool.submit(mask_change, baseline, self.out) if compared else None
            shipped = pool.submit(
                against_shipped, self.sources.textures, textures, self.views
            )
            thumbs = pool.submit(thumbnails, self.out, self.views)
            crops = pool.submit(
                detail_crops, baseline, self.out, self.views, crop_parent
            )
            for face, future in dark.items():
                metrics["dark_land"][face] = future.result()
            for view, future in zip(self.views, views, strict=True):
                metrics["views"][view.key] = future.result()
            if changed is not None:
                metrics["mask_change"] = changed.result()
            metrics["against_shipped"] = shipped.result()
            thumbs.result()
            metrics["pack_build_s"] = self.notes.get("pack_build_s")
            metrics["warm_render_s"] = self.notes.get("warm_render_s")
            if self.variant.imagery_water:
                key = self.mask_key()
                metrics["imagery_water"] = self.cache.record(
                    f"imagery-water-{key}", dict
                )
                flags = np.load(self.cache.dir / f"imagery-water-flags-{key}.npy")
                imagery_water_overview(self.raster(), flags, self.out)
            if "snow_colors" in self.notes:
                metrics["snow_colors"] = self.notes["snow_colors"]
            if "coast_blend" in self.notes:
                metrics["coast_blend"] = self.notes["coast_blend"]
            for key in (
                "activity",
                "coast_gaps",
                "fields",
                "fractures",
                "lead_area",
                "pack_streaks",
            ):
                if key in self.notes:
                    metrics[key] = self.notes[key]
            metrics["crops"] = crops.result()
        jsonable = _jsonable(metrics)
        assert isinstance(jsonable, dict)
        return jsonable

    def measure_view(
        self,
        view: View,
        textures: Path,
        baked: dict[str, dict],
        renders: dict[str, dict],
    ) -> dict:
        """Measure one view's face and render."""
        baseline = self.root / "baseline"
        entry = baked[view.key]
        decoded = _open_rgb(entry["encoded"])
        pre = _open_rgb(entry["preencode"])
        mask = _open_gray(textures / "mask" / f"{view.face}.jxl")
        min_lat = 70.0 if view.face == "py" else 60.0
        m = {
            "label": view.label,
            "face": view.face,
            "month": view.month,
            "render": renders[view.key]["path"].name,
            "render_s": renders[view.key]["seconds"],
            "month_check": month_check(view, renders[view.key]["path"], textures),
            "coast_snow": coast_against_ice(decoded, mask, view.face, min_lat),
            "ice_area": {
                "faces_km2": round(entry["area_km2"]),
                "source_km2": round(entry["source_km2"]),
                "difference_pct": round(
                    (entry["area_km2"] / entry["source_km2"] - 1) * 100, 1
                ),
                "toned_km2": round(entry["toned_km2"]),
                "toned_difference_pct": round(
                    (entry["area_km2"] / entry["toned_km2"] - 1) * 100, 1
                ),
            },
            "fine_detail": fine_detail(pre, decoded, mask, view.face),
            "coast_steps": coast_steps(decoded, mask, view.face),
        }
        parent_dir = self.root / (self.variant.parent or "baseline")
        parent_face = parent_dir / "textures" / "day" / view.stamp
        if self.variant.parent and (parent_face / f"{view.face}.jxl").exists():
            m["parent_coast_steps"] = coast_steps(
                _open_rgb(parent_face / f"{view.face}.jxl"),
                _open_gray(parent_dir / "textures" / "mask" / f"{view.face}.jxl"),
                view.face,
            )
        if baseline != self.out and (baseline / "faces").exists():
            m["against_baseline"] = changes_against(
                baseline,
                self.out,
                view,
                textures,
                self.out / f"{view.key}-changes.png",
            )
        parent = self.variant.parent
        if parent not in (None, "baseline") and (self.root / parent / "faces").exists():
            m["against_parent"] = changes_against(
                self.root / parent,
                self.out,
                view,
                textures,
                self.out / f"{view.key}-changes-against-{parent}.png",
            )
        return m

    def write_notes(self, metrics: dict) -> None:
        """Write ``notes.md``: what the variant changes and what it measured."""
        lines = [f"# {self.variant.name}", "", self.variant.summary, ""]
        lines += notes_body(self.variant, metrics)
        (self.out / "notes.md").write_text(
            "\n".join(lines) + "\n", encoding="utf-8", newline="\n"
        )


def snow_tables(
    table: Mapping[str, tuple[sea_ice_look.Rgb, ...]],
    measured: Mapping[str, sea_ice_look.Rgb],
    south_toward_north: float = 0.0,
    north_darkening: float = 1.0,
) -> tuple[dict[str, tuple[sea_ice_look.Rgb, ...]], dict[str, sea_ice_look.Rgb]]:
    """Return each hemisphere's colors from its coastal snow, as B2 makes
    them, with the moves of :attr:`Variant.south_toward_north` and
    :attr:`Variant.north_darkening`.

    :param table: The colors the months had, twelve per hemisphere.
    :param measured: Each hemisphere's coastal snow, its new winter color.
    :param south_toward_north: How far the south's winter color moves toward
        the north's.
    :param north_darkening: The share of the north's darkening its months keep.
    :returns: The colors, and the winter color each hemisphere's took.
    """
    winters = {h: _rgb(rgb) for h, rgb in measured.items()}
    if south_toward_north:
        north, south = (np.asarray(winters[h], np.float64) for h in ("nh", "sh"))
        winters["sh"] = _rgb(np.rint(south + (north - south) * south_toward_north))
    colors = {
        h: sea_ice_look.rescaled_colors(
            table[h], winters[h], north_darkening if h == "nh" else 1.0
        )
        for h in winters
    }
    return colors, winters


PACKAGE = "texture_pipeline"

FIELD_ROOTS: tuple[Callable[..., object], ...] = (
    sea_ice.read_hemisphere,
    sea_ice_look.fast_ice,
    sea_ice.cap_concentration,
    sea_ice_look.prepare,
)
"""The functions that make a hemisphere's noise fields from the record."""


def _ours(value: object) -> bool:
    return (
        (inspect.isfunction(value) or inspect.isclass(value))
        and value.__module__.startswith(PACKAGE)
        and value is not sea_ice_look.Look
    )


def _code_objects(value: object) -> Iterator[types.CodeType]:
    if inspect.isfunction(value):
        todo = [value.__code__]
    else:
        todo = []
        for member in vars(value).values():
            if isinstance(member, staticmethod | classmethod):
                member = member.__func__
            if isinstance(member, property):
                member = member.fget
            if inspect.isfunction(member):
                todo.append(member.__code__)
    while todo:
        code = todo.pop()
        yield code
        todo += [c for c in code.co_consts if isinstance(c, types.CodeType)]


@functools.cache
def _syntax(value: typing.Any) -> ast.AST:
    return ast.parse(textwrap.dedent(inspect.getsource(value)))


def _stable(value: object) -> str:
    """Return a constant's text without the addresses a function's repr has."""
    if inspect.isfunction(value):
        return f"{value.__qualname__}:{inspect.getsource(value)}"
    if isinstance(value, dict):
        items = ", ".join(f"{k!r}: {_stable(v)}" for k, v in value.items())
        return "{" + items + "}"
    if isinstance(value, tuple | list | frozenset):
        parts = [_stable(v) for v in value]
        return (
            "("
            + ", ".join(sorted(parts) if isinstance(value, frozenset) else parts)
            + ")"
        )
    if dataclasses.is_dataclass(value) and not isinstance(value, type):
        fields_ = ", ".join(
            f"{f.name}={_stable(getattr(value, f.name))}"
            for f in dataclasses.fields(value)
        )
        return f"{type(value).__qualname__}({fields_})"
    return repr(value)


def code_closure(*roots: object) -> tuple[list[typing.Any], list[str]]:
    """Return what functions and classes of this package reach by name.

    A name a function's code reads is looked up in its module; a function or
    class of this package found there is followed in turn, and a module of
    this package is followed through every other name the code reads. Plain
    values, the module's constants, are kept as ``module.name=repr``. The
    tunables' class is not followed: a cache key holds their values.

    :param roots: Functions or classes of this package.
    :returns: The functions and classes reached, the roots among them, and
        the constants they read.
    """
    reached: dict[int, typing.Any] = {}
    constants: set[str] = set()
    todo = list(roots)
    while todo:
        value = todo.pop()
        if id(value) in reached:
            continue
        reached[id(value)] = value
        namespace = vars(sys.modules[value.__module__])
        names = {n for code in _code_objects(value) for n in code.co_names}
        names |= {
            node.id for node in ast.walk(_syntax(value)) if isinstance(node, ast.Name)
        }
        for name in names:
            found = namespace.get(name)
            if isinstance(found, types.ModuleType):
                if found.__name__.startswith(PACKAGE):
                    todo += [
                        getattr(found, n) for n in names if _ours(getattr(found, n, 0))
                    ]
            elif _ours(found):
                todo.append(found)
            elif isinstance(
                found, int | float | str | bytes | tuple | frozenset | dict
            ):
                constants.add(f"{value.__module__}.{name}={_stable(found)}")
    return list(reached.values()), sorted(constants)


@functools.cache
def code_digest(*roots: object) -> str:
    """Return a digest of the source code roots run, constants and libraries
    included, so that a cache made by older code is never read.

    :param roots: Functions or classes of this package.
    :returns: Twelve hex digits.
    """
    reached, constants = code_closure(*roots)
    sources = sorted(
        f"{v.__module__}.{v.__qualname__}\n{inspect.getsource(v)}" for v in reached
    )
    return _key(sources, constants, np.__version__, scipy.__version__)


@functools.cache
def tunables_read(*roots: object) -> frozenset[str]:
    """Return the names of :class:`sea_ice_look.Look`'s tunables the code that
    roots reach reads as attributes."""
    reached, _ = code_closure(*roots)
    names = {f.name for f in dataclasses.fields(sea_ice_look.Look)}
    return frozenset(
        node.attr
        for value in reached
        for node in ast.walk(_syntax(value))
        if isinstance(node, ast.Attribute) and node.attr in names
    )


def field_tunables(look: sea_ice_look.Look) -> tuple[tuple[str, object], ...]:
    """Return the tunables the noise fields depend on, by name: those the code
    that makes them reads."""
    read = tunables_read(*FIELD_ROOTS)
    return tuple(
        (f.name, getattr(look, f.name))
        for f in dataclasses.fields(look)
        if f.name in read
    )


def fields_key(look: sea_ice_look.Look) -> str:
    """The cache key of a hemisphere's noise fields: the cap's size, the
    tunables they read and the code that makes them."""
    return _key(
        "fields", sea_ice.CAP_SIZE, field_tunables(look), code_digest(*FIELD_ROOTS)
    )


RECORD_ROOTS: tuple[Callable[..., object], ...] = (
    sea_ice.read_hemisphere,
    sea_ice_look.fast_ice,
    sea_ice.cap_concentration,
)
"""The functions that make a hemisphere's concentration, with the fast ice
band, from the record."""


def record_key(directory: Path, hemisphere: str, look: sea_ice_look.Look) -> str:
    """The cache key of a hemisphere's concentration: the record's files, the
    tunables that shape it and the code that reads it."""
    files = []
    for month in sea_ice.MONTHS:
        stat = (directory / sea_ice.file_name(hemisphere, month)).stat()
        files.append((month, stat.st_size, stat.st_mtime_ns))
    read = tunables_read(*RECORD_ROOTS)
    tunables = tuple(
        (f.name, getattr(look, f.name))
        for f in dataclasses.fields(look)
        if f.name in read
    )
    return _key(
        "record", sea_ice.FIELD_SIZE, files, tunables, code_digest(*RECORD_ROOTS)
    )


def part_keys(
    look: sea_ice_look.Look, record: str, size: int, land: str
) -> dict[str, str]:
    """Return the cache key of each part of a hemisphere's noise fields.

    A key holds the cap's size, the tunables the part's code reads, that code,
    the record's key when the part reads the concentration, the land's key
    when it reads the land, and the keys of the parts it reads, so that a
    change reaches exactly the parts it affects.

    :param look: The tunables.
    :param record: The record's key, as :func:`record_key` gives it.
    :param size: Texels along each side of the cap.
    :param land: The key of the land on the cap.
    :returns: The key of each part, by the part's name.
    """
    keys: dict[str, str] = {}
    for part in sea_ice_look.PARTS:
        read = tunables_read(part.make)
        tunables = tuple(
            (f.name, getattr(look, f.name))
            for f in dataclasses.fields(look)
            if f.name in read
        )
        keys[part.name] = _key(
            "part",
            part.name,
            size,
            tunables,
            code_digest(part.make),
            record if part.reads_record else None,
            tuple(keys[name] for name in part.needs),
            *((land,) if part.reads_land else ()),
        )
    return keys


def part_to_arrays(part: Mapping[str, object]) -> dict[str, np.ndarray]:
    """Return the attributes of a part, or of the fields, as named arrays.

    An array is named by its attribute, one of a nested dataclass by both
    names joined with a dot; an attribute that is neither is left out.
    """
    out = {}
    for name, value in part.items():
        if isinstance(value, np.ndarray):
            out[name] = value
        elif dataclasses.is_dataclass(value):
            for g in dataclasses.fields(value):
                if getattr(value, g.name) is not None:
                    out[f"{name}.{g.name}"] = getattr(value, g.name)
    return out


def parts_from_arrays(arrays: dict[str, np.ndarray]) -> dict[str, typing.Any]:
    """Return the attributes of the fields that :func:`part_to_arrays` took
    apart."""
    hints = typing.get_type_hints(sea_ice_look.Fields)
    values: dict[str, typing.Any] = {}
    for f in dataclasses.fields(sea_ice_look.Fields):
        nested = {
            key.split(".", 1)[1]: value
            for key, value in arrays.items()
            if key.startswith(f"{f.name}.")
        }
        if nested:
            kind = next(
                t
                for t in (hints[f.name], *typing.get_args(hints[f.name]))
                if dataclasses.is_dataclass(t)
            )
            values[f.name] = kind(
                **{g.name: nested.get(g.name) for g in dataclasses.fields(kind)}
            )
        elif f.name in arrays:
            values[f.name] = arrays[f.name]
    return values


def fields_to_arrays(fields: sea_ice_look.Fields) -> dict[str, np.ndarray]:
    """Return a hemisphere's noise fields as named arrays, for the cache.

    An array is named by its field, one of a nested dataclass by both names
    joined with a dot; a field that is ``None`` is left out.
    """
    return part_to_arrays(
        {f.name: getattr(fields, f.name) for f in dataclasses.fields(fields)}
    )


def fields_from_arrays(
    hemisphere: str, arrays: dict[str, np.ndarray]
) -> sea_ice_look.Fields:
    """Return the noise fields :func:`fields_to_arrays` took apart."""
    values = parts_from_arrays(arrays)
    return sea_ice_look.Fields(hemisphere=hemisphere, **values)


ACTIVITY_CLASSES = (0.25, 0.6)
"""Activity below the first is flat, from the first to the second moderate,
above it structured."""


def activity_shares(cap: sea_ice_look.Cap) -> dict[str, float]:
    """Return the shares of the pack, coverage above a half, by activity.

    The cap's projection keeps area, so a share of texels is a share of area.
    """
    if cap.activity is None:
        return {}
    pack = cap.coverage > 0.5
    active = cap.activity[pack]
    flat, structured = ACTIVITY_CLASSES
    total = max(len(active), 1)
    return {
        "flat": round(float((active < flat).sum()) / total, 3),
        "moderate": round(
            float(((active >= flat) & (active < structured)).sum()) / total, 3
        ),
        "structured": round(float((active >= structured).sum()) / total, 3),
    }


def _rgb(values: Iterable[float]) -> sea_ice_look.Rgb:
    red, green, blue = (int(v) for v in values)
    return red, green, blue


def _rgb_list(values: Iterable[float]) -> list[int]:
    return [round(float(v)) for v in values]


def _jsonable(value: object) -> object:
    if isinstance(value, dict):
        return {str(k): _jsonable(v) for k, v in value.items()}
    if isinstance(value, list | tuple):
        return [_jsonable(v) for v in value]
    if isinstance(value, Path):
        return value.name
    if isinstance(value, np.integer):
        return int(value)
    if isinstance(value, np.floating):
        return float(value)
    return value


def run_in_own_console(args: list[str], env: dict[str, str]) -> int:
    """Run the app with a console of its own, so it writes no log file.

    The app logs to a file in the user's data directory when its standard
    error is not a terminal. On Windows a hidden console of its own is one; on
    other systems it inherits the harness's terminal.

    :param args: The command line.
    :param env: The environment.
    :returns: The exit code.
    """
    if sys.platform != "win32":
        return subprocess.run(args, env=env, check=False).returncode
    startup = subprocess.STARTUPINFO()
    startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    startup.wShowWindow = 0
    return subprocess.run(
        args,
        env=env,
        check=False,
        creationflags=subprocess.CREATE_NEW_CONSOLE,
        startupinfo=startup,
    ).returncode


def camera_distance(
    frame_latitude: float = FRAME_LATITUDE, fov_deg: float = FOV_DEG
) -> float:
    """Return the distance at which a parallel touches the frame's edges.

    :param frame_latitude: The latitude at the middle of each edge.
    :param fov_deg: The vertical field of view.
    :returns: The distance from the Earth's center in its radii.
    """
    colatitude = math.radians(90 - frame_latitude)
    return math.cos(colatitude) + math.sin(colatitude) / math.tan(
        math.radians(fov_deg / 2)
    )


def camera_zoom(distance: float) -> float:
    """Return the app's normalized zoom for a camera distance."""
    return math.log(distance / ZOOM_DISTANCE_MIN) / math.log(
        ZOOM_DISTANCE_MAX / ZOOM_DISTANCE_MIN
    )


def day_of_year(month: int) -> int:
    """Return the day of the year of the first of a month of 2004."""
    return datetime.date(YEAR, month, 1).timetuple().tm_yday


def render_config(view: View) -> str:
    """Return the app config of a view: the day surface unlit, nothing else."""
    return "\n".join(
        [
            f"version = {CONFIG_VERSION}",
            "",
            "[sunlit.earth]",
            f"latitude = {view.latitude}",
            f"longitude = {view.longitude}",
            f"zoom = {camera_zoom(camera_distance()):.6f}",
            f"camera_fov = {FOV_DEG}",
            f"texture_index = {DAY_MODE}",
            "texture_resolution = 8192",
            "atmo_enabled = false",
            "star_intensity = 0.0",
            "milky_way_intensity = 0.0",
            "clouds_enabled = false",
            "cloud_opacity = 0.0",
            "cloud_opacity_night = 0.0",
            "sun_flare = 0.0",
            "use_custom_datetime = true",
            "custom_hour = 12.0",
            f"custom_day_of_year = {day_of_year(view.month)}",
            f"custom_year = {YEAR}",
            "",
        ]
    )


def view_directions(
    view: View, size: int
) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    """Return the surface direction under every pixel of a view's render.

    The camera is the app's: at the view's latitude and longitude, looking at
    the Earth's center with +Y up, with the vertical field of view and distance
    the render config gives it.

    :param view: The view.
    :param size: The render's edge in pixels.
    :returns: ``x``, ``y``, ``z`` and whether the pixel shows the Earth.
    """
    lat, lon = math.radians(view.latitude), math.radians(view.longitude)
    distance = camera_distance()
    eye = distance * np.array(
        [math.cos(lat) * math.sin(lon), math.sin(lat), math.cos(lat) * math.cos(lon)]
    )
    forward = -eye / np.linalg.norm(eye)
    right = np.cross(forward, [0.0, 1.0, 0.0])
    right /= np.linalg.norm(right)
    up = np.cross(right, forward)
    half = math.tan(math.radians(FOV_DEG / 2))
    ndc = (np.arange(size) + 0.5) / size * 2 - 1
    sx = ndc[None, :, None] * half
    sy = -ndc[:, None, None] * half
    ray = forward + sx * right + sy * up
    a = np.sum(ray * ray, axis=-1)
    b = 2 * ray @ eye
    c = float(eye @ eye) - 1
    disc = b * b - 4 * a * c
    hit = disc > 0
    t = (-b - np.sqrt(np.maximum(disc, 0))) / (2 * a)
    point = eye + t[..., None] * ray
    return point[..., 0], point[..., 1], point[..., 2], hit


def polar_face_texels(
    face: str, x: np.ndarray, y: np.ndarray, z: np.ndarray, size: int
) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Return where directions land on ``py`` or ``ny``, and whether they do.

    :param face: ``py`` or ``ny``.
    :param x: Direction components.
    :param y: Direction components.
    :param z: Direction components.
    :param size: The face's edge in texels.
    :returns: Continuous row and column, texel centers at integers, and
        whether the direction lies on the face.
    """
    north = face == "py"
    on = (y > 0 if north else y < 0) & (np.abs(y) >= np.maximum(np.abs(x), np.abs(z)))
    scale = np.where(on, np.abs(y), 1.0)
    s = x / scale
    t = z / scale if north else -z / scale
    col = (np.arctan(s) / (np.pi / 4) + 1) / 2 * size - 0.5
    row = (np.arctan(t) / (np.pi / 4) + 1) / 2 * size - 0.5
    return row, col, on


def month_check(view: View, render: Path, textures: Path) -> dict:
    """Check that a render shows its month's face rather than a neighbor's.

    The render's pixels poleward of 50 degrees on the polar face are compared
    with the face of the month and of the months either side, each sampled
    through the app's camera; the month's own face has to be the nearest.

    :param view: The view.
    :param render: The render.
    :param textures: The variant's texture directory.
    :returns: The mean absolute difference per month, and whether the view's
        month is the nearest.
    """
    image = _open_rgb(render).astype(np.float32)
    x, y, z, hit = view_directions(view, image.shape[0])
    row, col, on = polar_face_texels(view.face, x, y, z, FACE_SIZE)
    lat = np.degrees(np.arctan2(y, np.hypot(x, z)))
    use = hit & on & (np.abs(lat) >= 50)
    coords = [row[use], col[use]]
    seen = image[use]
    errors = {}
    for month in (view.month - 1, view.month, view.month + 1):
        month = (month - 1) % 12 + 1
        face = _open_rgb(textures / "day" / f"{YEAR}{month:02d}" / f"{view.face}.jxl")
        predicted = np.stack(
            [
                map_coordinates(face[..., ch].astype(np.float32), coords, order=1)
                for ch in range(3)
            ],
            axis=-1,
        )
        errors[month] = round(float(np.abs(predicted - seen).mean()), 2)
    nearest = min(errors, key=errors.__getitem__)
    return {
        "mean_abs_diff": errors,
        "pixels": int(use.sum()),
        "ok": nearest == view.month,
    }


def dark_land(textures: Path, face: str) -> dict:
    """Count the mask's land texels that are dark water in every month.

    The evidence's measure: a texel the mask calls land (below 128) whose
    brightest luminance over the twelve months is below 60 and whose blue is
    above its red by more than 8 in every month.

    :param textures: A texture directory.
    :param face: The face.
    :returns: The counts on the face and poleward of 60 degrees, those
        poleward split by whether the texel is connected to the mask's water
        through dark texels, four-connected: the coast's mismatch, or a lake.
    """
    mask = _open_gray(textures / "mask" / f"{face}.jxl")
    brightest = np.zeros(mask.shape, np.float32)
    bluest = np.full(mask.shape, 255.0, np.float32)
    for month in sorted(p.name for p in (textures / "day").iterdir()):
        rgb = _open_rgb(textures / "day" / month / f"{face}.jxl").astype(np.float32)
        brightest = np.maximum(brightest, rgb @ REC709)
        bluest = np.minimum(bluest, rgb[..., 2] - rgb[..., 0])
    dark = (mask < 128) & (brightest < 60) & (bluest > 8)
    polar = np.abs(face_latitude(face, mask.shape[0])) >= 60
    water = mask >= 128
    labels, _ = label(dark | water)
    touching = np.isin(labels, np.unique(labels[water]))
    return {
        "dark_land": int(dark.sum()),
        "poleward_of_60": int((dark & polar).sum()),
        "connected_to_water": int((dark & touching).sum()),
        "poleward_of_60_connected": int((dark & polar & touching).sum()),
        "poleward_of_60_unconnected": int((dark & polar & ~touching).sum()),
    }


def coastal_snow(
    rgb: np.ndarray, mask: np.ndarray, face: str, min_lat: float
) -> np.ndarray:
    """Return the colors of bright land within 8 texels of the mask's water.

    :param rgb: A face, uint8 ``(n, n, 3)``.
    :param mask: The mask face.
    :param face: The face's name.
    :param min_lat: The least absolute latitude measured.
    :returns: uint8 array ``(k, 3)``.
    """
    polar = np.abs(face_latitude(face, mask.shape[0])) >= min_lat
    coast = (mask == 0) & binary_dilation(mask == 255, iterations=8) & polar
    return rgb[coast & (luma(rgb) > 180)]


def coast_against_ice(rgb: np.ndarray, mask: np.ndarray, face: str, min_lat: float):
    """The evidence's measure: coastal snow against the ice beyond 8 texels out.

    :returns: The medians and counts of both, and the ice's difference.
    """
    snow = coastal_snow(rgb, mask, face, min_lat)
    polar = np.abs(face_latitude(face, mask.shape[0])) >= min_lat
    water = mask == 255
    open_sea = water & ~binary_dilation(~water, iterations=8) & polar
    ice = rgb[open_sea & (luma(rgb) > 180)]
    out: dict = {
        "min_latitude": min_lat,
        "snow_texels": len(snow),
        "ice_texels": len(ice),
    }
    if len(snow):
        out["snow_median"] = _rgb_list(np.median(snow, axis=0))
    if len(ice):
        out["ice_median"] = _rgb_list(np.median(ice, axis=0))
    if len(snow) and len(ice):
        out["ice_minus_snow"] = [
            i - s for i, s in zip(out["ice_median"], out["snow_median"], strict=True)
        ]
    return out


def fine_detail(pre: np.ndarray, decoded: np.ndarray, mask: np.ndarray, face: str):
    """Measure the ice interior's fine detail before and after the encode.

    The interior is the mask's open water at least 8 texels from anything
    else, poleward of 60 degrees, where the face before the encode is brighter
    than 150, eroded by 6 texels so that no edge reaches the filters. The
    detail is the luminance's root mean square in two bands: finer than a
    Gaussian of 1 texel (about 5 km), and between Gaussians of 1 and 4 texels.

    :returns: Both bands before and after the encode, and the interior's size.
    """
    water = mask == 255
    polar = np.abs(face_latitude(face, mask.shape[0])) >= 60
    bright = luma(pre) > 150
    interior = water & ~binary_dilation(~water, iterations=8) & polar & bright
    interior = binary_erosion(interior, iterations=6)
    out: dict = {"texels": int(interior.sum())}
    if not interior.any():
        return out
    for name, rgb in (("before", pre), ("after", decoded)):
        y = luma(rgb)
        g1 = gaussian_filter(y, 1.0)
        g4 = gaussian_filter(y, 4.0)
        out[name] = {
            "fine_rms": round(float(np.sqrt(np.mean((y - g1)[interior] ** 2))), 3),
            "mid_rms": round(float(np.sqrt(np.mean((g1 - g4)[interior] ** 2))), 3),
        }
    out["fine_kept"] = round(out["after"]["fine_rms"] / out["before"]["fine_rms"], 3)
    out["mid_kept"] = round(out["after"]["mid_rms"] / out["before"]["mid_rms"], 3)
    return out


COASTS: dict[str, dict[str, tuple[str, tuple[float, float], tuple[float, float]]]] = {
    "ny": {
        "ross-ice-shelf-front": ("flat", (-78.8, -77.3), (172.0, -160.0)),
        "ronne-filchner-front": ("flat", (-78.5, -74.5), (-62.0, -30.0)),
        "amery-front": ("flat", (-70.0, -68.5), (70.0, 75.0)),
        "wilkes-land": ("flat", (-67.5, -65.5), (100.0, 135.0)),
        "queen-maud-land-shelves": ("flat", (-71.0, -69.0), (-10.0, 30.0)),
        "larsen-c": ("flat", (-68.5, -66.0), (-63.0, -59.0)),
        "victoria-land": ("mountain", (-77.5, -71.0), (160.0, 171.0)),
        "antarctic-peninsula-west": ("mountain", (-68.0, -63.0), (-68.0, -60.5)),
    },
    "py": {
        "north-greenland": ("flat", (81.5, 83.7), (-50.0, -20.0)),
        "canadian-arctic-west": ("flat", (71.0, 78.0), (-126.0, -105.0)),
        "east-greenland": ("mountain", (66.0, 72.5), (-35.0, -20.0)),
        "ellesmere-north": ("mountain", (81.5, 83.2), (-90.0, -62.0)),
    },
}
"""Coasts metric (e) measures, per polar face: flat snow coasts, ice shelf
fronts and low islands, against mountain coasts. Each is a box of latitude and
longitude; a longitude range from east to west crosses the date line."""

COAST_OFFSETS = np.arange(-4, 5)
"""Texels along a coast's normal metric (e) samples, positive toward the sea."""


def face_longitude(face: str, size: int) -> np.ndarray:
    """Return the longitude in degrees of every texel of a face."""
    x, _, z = face_directions(face, size, 0, size)
    return np.degrees(np.arctan2(x, z)).astype(np.float32)


def in_box(
    lat: np.ndarray,
    lon: np.ndarray,
    lats: tuple[float, float],
    lons: tuple[float, float],
) -> np.ndarray:
    """Return where points lie in a box of latitude and longitude.

    :param lats: The latitude range, either order.
    :param lons: West and east; a west above the east crosses the date line.
    """
    inside = (lat >= min(lats)) & (lat <= max(lats))
    west, east = lons
    if west <= east:
        return inside & (lon >= west) & (lon <= east)
    return inside & ((lon >= west) | (lon <= east))


def seaward(mask: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """Return the gradient of the smoothed signed distance from the coast.

    :param mask: The mask face.
    :returns: Its row and column components; they point toward the sea.
    """
    land = mask < 128
    signed = np.where(
        land,
        0.5 - distance_transform_edt(land),
        distance_transform_edt(~land) - 0.5,
    ).astype(np.float32)
    gr, gc = np.gradient(gaussian_filter(signed, 1.0))
    return gr, gc


def coast_profiles(
    planes: tuple[np.ndarray, ...],
    gradient: tuple[np.ndarray, np.ndarray],
    rows: np.ndarray,
    cols: np.ndarray,
) -> list[np.ndarray]:
    """Sample planes along the normals of coast texels at :data:`COAST_OFFSETS`.

    :param planes: float32 arrays ``(n, n)`` to sample.
    :param gradient: :func:`seaward` of the mask.
    :param rows: Rows of the coast texels.
    :param cols: Their columns.
    :returns: One float32 array ``(len(rows), len(COAST_OFFSETS))`` per plane.
    """
    gr, gc = gradient[0][rows, cols], gradient[1][rows, cols]
    norm = np.hypot(gr, gc) + 1e-6
    rr = rows[:, None] + COAST_OFFSETS[None, :] * (gr / norm)[:, None]
    cc = cols[:, None] + COAST_OFFSETS[None, :] * (gc / norm)[:, None]
    return [map_coordinates(p, [rr, cc], order=1, mode="nearest") for p in planes]


def coast_steps(rgb: np.ndarray, mask: np.ndarray, face: str) -> dict:
    """Metric (e): how the coasts of :data:`COASTS` show under closed ice.

    At every land texel next to the mask's water in a coast's box, luminance
    is sampled along the coast's normal. A texel counts where the ice is
    closed, brighter than 200 and the mask's open water 2 to 4 texels out,
    and the land is snow, brighter than 200 2 to 4 texels in. The step is the
    land's mean 2 and 3 texels in, 10 to 15 km, less the ice's 2 and 3 texels
    out; the dip is the mean of the two less the darkest texel within one of
    the coast.

    :param rgb: A face, uint8 ``(n, n, 3)``.
    :param mask: The mask face.
    :param face: ``py`` or ``ny``.
    :returns: Per coast and pooled per kind: the texels counted, the median
        step, and the dip's median and 90th percentile.
    """
    coasts = COASTS.get(face, {})
    if not coasts:
        return {}
    size = mask.shape[0]
    lat, lon = face_latitude(face, size), face_longitude(face, size)
    land = mask < 128
    rim = land & binary_dilation(~land)
    gradient = seaward(mask)
    planes = (luma(rgb), mask.astype(np.float32) / 255)
    o = {int(k): i for i, k in enumerate(COAST_OFFSETS)}
    out: dict = {}
    pooled: dict[str, list[tuple[np.ndarray, np.ndarray]]] = {}
    for name, (kind, lats, lons) in coasts.items():
        rows, cols = np.nonzero(rim & in_box(lat, lon, lats, lons))
        lum, share = coast_profiles(planes, gradient, rows, cols)
        sea = [o[2], o[3], o[4]]
        inland = [o[-4], o[-3], o[-2]]
        keep = (
            (lum[:, sea] > 200).all(axis=1)
            & (share[:, sea] > 0.99).all(axis=1)
            & (lum[:, inland] > 200).all(axis=1)
        )
        lum = lum[keep]
        land_side = lum[:, [o[-3], o[-2]]].mean(axis=1)
        ice_side = lum[:, [o[2], o[3]]].mean(axis=1)
        step = land_side - ice_side
        dip = (land_side + ice_side) / 2 - lum[:, [o[-1], o[0], o[1]]].min(axis=1)
        out[name] = {"kind": kind, "texels": int(keep.sum()), **_steps(step, dip)}
        pooled.setdefault(kind, []).append((step, dip))
    for kind, parts in pooled.items():
        step = np.concatenate([p[0] for p in parts])
        dip = np.concatenate([p[1] for p in parts])
        out[f"all-{kind}"] = {"kind": kind, "texels": len(step), **_steps(step, dip)}
    return out


def _steps(step: np.ndarray, dip: np.ndarray) -> dict:
    if not len(step):
        return {}
    return {
        "step_median": round(float(np.median(step)), 1),
        "dip_median": round(float(np.median(dip)), 1),
        "dip_p90": round(float(np.percentile(dip, 90)), 1),
    }


def ice_area(
    water: np.ndarray,
    face: str,
    ice: sea_ice_look.MonthIce,
    hemisphere: str,
    pool: Executor,
    band_rows: int = 256,
) -> float:
    """Return a hemisphere's ice area on a face, as the composite counts it.

    :returns: Coverage times the mask's share of water times the texel's area,
        summed, in square kilometers.
    """
    size = water.shape[0]

    def band(start: int) -> float:
        stop = min(start + band_rows, size)
        share = water[start:stop]
        x, y, z = face_directions(face, size, start, stop)
        here = (share > 0) & ((y > 0) if hemisphere == "nh" else (y < 0))
        here &= np.abs(np.arctan2(y, np.hypot(x, z))) > np.radians(30)
        if not here.any():
            return 0.0
        x, y, z = x[here], y[here], z[here]
        coverage, _ = ice.sample(x, y, z)
        weight = share[here].astype(np.float32) / 255 * coverage
        area = weight * sea_ice_look.texel_solid_angles(x, y, z, size)
        return float(area.sum()) * sea_ice.AUTHALIC_RADIUS_KM**2

    return sum(pool.map(band, range(0, size, band_rows)))


def changes_against(
    baseline: Path, variant: Path, view: View, textures: Path, image: Path
) -> dict:
    """Compare a view's face before the encode with the baseline's.

    :returns: Changed texels in all, on the land of both masks, where the masks
        differ, and on the water of both.
    """
    name = f"{view.stamp}-{view.face}.png"
    before = _open_rgb(baseline / "faces" / name)
    after = _open_rgb(variant / "faces" / name)
    mask_before = _open_gray(baseline / "textures" / "mask" / f"{view.face}.jxl")
    mask_after = _open_gray(textures / "mask" / f"{view.face}.jxl")
    changed = np.any(before != after, axis=-1)
    land = (mask_before == 0) & (mask_after == 0)
    differ = mask_before != mask_after
    delta = np.abs(before.astype(np.int16) - after.astype(np.int16)).max(axis=-1)
    view_image = np.repeat((luma(before) * 0.5).astype(np.uint8)[..., None], 3, -1)
    view_image[changed] = (255, 60, 0)
    view_image[changed & land] = (255, 255, 0)
    Image.fromarray(view_image).save(image)
    return {
        "changed": int(changed.sum()),
        "changed_on_land": int((changed & land).sum()),
        "changed_where_mask_differs": int((changed & differ).sum()),
        "changed_on_water": int((changed & ~land & ~differ).sum()),
        "largest_change": int(delta.max()),
        "largest_change_on_land": int(delta[land].max()) if land.any() else 0,
        "image": image.name,
    }


def mask_change(baseline: Path, variant: Path) -> dict:
    """Draw and count the mask faces' change against the baseline."""
    out = {}
    for face in ("py", "ny"):
        before = _open_gray(baseline / "textures" / "mask" / f"{face}.jxl").astype(
            np.int16
        )
        after = _open_gray(variant / "textures" / "mask" / f"{face}.jxl").astype(
            np.int16
        )
        delta = after - before
        image = np.zeros((*before.shape, 3), dtype=np.uint8)
        image[...] = np.where(before[..., None] >= 128, (20, 40, 80), (110, 110, 110))
        gained = delta > 0
        lost = delta < 0
        image[gained] = (255, 40, 0)
        image[lost] = (0, 220, 255)
        name = f"mask-change-{face}.png"
        Image.fromarray(image).save(variant / name)
        out[face] = {
            "texels_gaining_water": int(gained.sum()),
            "texels_losing_water": int(lost.sum()),
            "water_gained_texel_units": round(float(delta[gained].sum()) / 255, 1),
            "image": name,
        }
    return out


PLACES: tuple[tuple[str, float, float], ...] = (
    ("north-greenland", 82.3, -38.0),
    ("ellesmere-north", 82.3, -75.0),
    ("svalbard", 78.8, 17.0),
    ("franz-josef-land", 80.8, 54.0),
    ("canadian-arctic-west", 76.5, -120.0),
    ("east-greenland", 71.0, -21.5),
    ("beaufort-sea", 75.0, -145.0),
    ("antarctic-peninsula", -66.0, -63.0),
    ("ross-sea-coast", -72.5, 168.0),
    ("east-antarctica", -66.5, 110.0),
    ("ronne-ice-shelf-front", -77.6, -45.0),
    ("ross-ice-shelf-front", -78.3, -175.0),
    ("amery-ice-shelf", -69.5, 72.5),
    ("victoria-land", -75.5, 163.0),
    ("weddell-sea", -68.0, -40.0),
)
"""Places the detail crops are cut around: name, latitude, longitude."""

CROP = 256
"""Render pixels along each side of a detail crop, drawn at twice the size."""


def find_place(name: str) -> tuple[str, float, float]:
    """Return the place of :data:`PLACES` with a name.

    :raises ValueError: If there is none; the message lists the names.
    """
    for place in PLACES:
        if place[0] == name:
            return place
    msg = f"No place {name!r}; one of {', '.join(p[0] for p in PLACES)}."
    raise ValueError(msg)


def region_window(
    face: str, latitude: float, longitude: float, size: int, half: int
) -> sea_ice_look.Window:
    """Return the square of a polar face around a point, kept on the face.

    :param face: ``py`` or ``ny``.
    :param latitude: The point's latitude in degrees.
    :param longitude: The point's longitude in degrees.
    :param size: The face's edge in texels.
    :param half: Half the square's side in texels.
    :returns: The window.
    :raises ValueError: If the point is not on the face.
    """
    phi, lam = math.radians(latitude), math.radians(longitude)
    x, y, z = (
        np.array(v)
        for v in (
            math.cos(phi) * math.sin(lam),
            math.sin(phi),
            math.cos(phi) * math.cos(lam),
        )
    )
    row, col, on = polar_face_texels(face, x, y, z, size)
    if not on:
        msg = f"{latitude}, {longitude} is not on the {face} face."
        raise ValueError(msg)
    return sea_ice_look.Window(
        size,
        (max(round(float(row)) - half, 0), min(round(float(row)) + half, size)),
        (max(round(float(col)) - half, 0), min(round(float(col)) + half, size)),
    )


def cap_rows(face: str, window: sea_ice_look.Window, cap_size: int) -> tuple[int, int]:
    """Return the cap rows a region of a polar face reads.

    :param face: ``py`` or ``ny``.
    :param window: The region.
    :param cap_size: Texels along each side of the cap.
    :returns: The first row and the one after the last, with a margin for the
        interpolation.
    """
    x, y, z = face_directions(face, window.size, *window.rows, window.columns)
    _, px, py = sea_ice.directions_to_cap(x, y, z)
    rows_, _ = sea_ice.plane_to_texel(px.ravel(), py.ravel(), cap_size)
    return (
        max(int(np.floor(rows_.min())) - 3, 0),
        min(int(np.ceil(rows_.max())) + 4, cap_size),
    )


def view_pixel(view: View, latitude: float, longitude: float, size: int):
    """Return the render pixel a point on the surface is drawn at.

    :returns: Column and row, or ``None`` when the point is outside the frame
        or on the far side.
    """
    lat, lon = math.radians(view.latitude), math.radians(view.longitude)
    eye = camera_distance() * np.array(
        [math.cos(lat) * math.sin(lon), math.sin(lat), math.cos(lat) * math.cos(lon)]
    )
    forward = -eye / np.linalg.norm(eye)
    right = np.cross(forward, [0.0, 1.0, 0.0])
    right /= np.linalg.norm(right)
    up = np.cross(right, forward)
    phi, lam = math.radians(latitude), math.radians(longitude)
    point = np.array(
        [math.cos(phi) * math.sin(lam), math.sin(phi), math.cos(phi) * math.cos(lam)]
    )
    if point @ (eye - point) <= 0:
        return None
    ray = point - eye
    depth = ray @ forward
    half = math.tan(math.radians(FOV_DEG / 2))
    x = (ray @ right) / depth / half
    y = (ray @ up) / depth / half
    col, row = (x + 1) / 2 * size, (1 - y) / 2 * size
    if not (0 <= col < size and 0 <= row < size):
        return None
    return col, row


def detail_crops(
    baseline: Path, out: Path, views: tuple[View, ...], parent: Path | None = None
) -> list[str]:
    """Cut the renders around :data:`PLACES`: the baseline's, the parent's when
    it is not the baseline, and the variant's, left to right.

    :returns: The names of the images written.
    """
    names = []
    for view in views:
        render = out / f"{view.key}.png"
        with Image.open(render) as image:
            size = image.width
            after_image = image.convert("RGB")
        others = []
        for directory in (baseline, parent):
            before = None if directory is None else directory / f"{view.key}.png"
            if before is not None and directory != out and before.exists():
                with Image.open(before) as image:
                    others.append(image.convert("RGB"))
        for name, latitude, longitude in PLACES:
            if (latitude > 0) != (view.hemisphere == "nh"):
                continue
            at = view_pixel(view, latitude, longitude, size)
            if at is None:
                continue
            left = int(min(max(at[0] - CROP / 2, 0), size - CROP))
            top = int(min(max(at[1] - CROP / 2, 0), size - CROP))
            box = (left, top, left + CROP, top + CROP)
            panels = [*others, after_image]
            sheet = Image.new("RGB", (len(panels) * (2 * CROP + 8) - 8, 2 * CROP))
            for k, panel in enumerate(panels):
                crop = panel.crop(box).resize(
                    (2 * CROP, 2 * CROP), Image.Resampling.NEAREST
                )
                sheet.paste(crop, (k * (2 * CROP + 8), 0))
            file = f"crop-{view.key}-{name}.png"
            sheet.save(out / file)
            names.append(file)
    return names


def thumbnails(out: Path, views: tuple[View, ...]) -> None:
    """Write half size JPEGs of the renders for the page's grid."""
    for view in views:
        with Image.open(out / f"{view.key}.png") as image:
            small = image.convert("RGB").resize(
                (image.width // 2, image.height // 2), Image.Resampling.LANCZOS
            )
        small.save(out / f"{view.key}-small.jpg", quality=92)


def notes_body(variant: Variant, metrics: dict) -> list[str]:
    """The measured part of a variant's notes."""
    lines = ["## Metrics", ""]
    dark = metrics["dark_land"]
    lines.append(
        "- (a) Dark texels of the mask's land poleward of 60 degrees: "
        f"py {dark['py']['poleward_of_60']:,}, ny {dark['ny']['poleward_of_60']:,} "
        "(baseline 1,202 and 668 in the plan's evidence). Of them connected to the "
        f"mask's water, the coast's mismatch: py "
        f"{dark['py']['poleward_of_60_connected']:,}, ny "
        f"{dark['ny']['poleward_of_60_connected']:,}; not connected, lakes: py "
        f"{dark['py']['poleward_of_60_unconnected']:,}, ny "
        f"{dark['ny']['poleward_of_60_unconnected']:,}."
    )
    for key, view in metrics["views"].items():
        snow = view["coast_snow"]
        area = view["ice_area"]
        detail = view["fine_detail"]
        check = view["month_check"]
        lines.append(f"- {view['label']} (`{key}`):")
        lines.append(
            f"  - (b) coastal snow {snow.get('snow_median')}, ice "
            f"{snow.get('ice_median')}, ice minus snow {snow.get('ice_minus_snow')}"
            f" (poleward of {snow['min_latitude']:.0f})"
        )
        lines.append(
            f"  - (c) ice area {area['faces_km2'] / 1e6:.3f} million km2 on the "
            f"faces against {area['toned_km2'] / 1e6:.3f} in the source read "
            f"through the tone curve, {area['toned_difference_pct']:+.1f}%, and "
            f"{area['source_km2'] / 1e6:.3f} in the record, "
            f"{area['difference_pct']:+.1f}%"
        )
        if "after" in detail:
            lines.append(
                f"  - (d) interior detail after the encode: fine "
                f"{detail['after']['fine_rms']} ({detail['fine_kept']:.0%} of "
                f"before), mid {detail['after']['mid_rms']} ({detail['mid_kept']:.0%}"
                f" of before), over {detail['texels']:,} texels"
            )
        lines += coast_step_notes(view, variant.parent)
        lines.append(
            f"  - month check: {'ok' if check['ok'] else 'FAILED'}, mean "
            f"differences {check['mean_abs_diff']}"
        )
        for key, name in (
            ("against_baseline", "the baseline"),
            ("against_parent", f"`{variant.parent}`"),
        ):
            if key in view:
                c = view[key]
                lines.append(
                    f"  - against {name} before the encode: {c['changed']:,} "
                    f"texels changed, {c['changed_on_land']:,} on the land of both "
                    f"masks (by at most {c['largest_change_on_land']}), "
                    f"{c['changed_where_mask_differs']:,} where the mask changed, "
                    f"{c['changed_on_water']:,} on water; image `{c['image']}`"
                )
    times = metrics["time_s"]
    lines += [
        "",
        "## Time",
        "",
        f"- Total {times['total']} s: "
        + ", ".join(f"{k} {v} s" for k, v in times.items() if k != "total"),
    ]
    if metrics["pack_build_s"] is not None:
        lines.append(
            f"- Tile packs built during the cold renders: about "
            f"{metrics['pack_build_s']} s (cold renders less four warm ones of "
            f"{metrics['warm_render_s']} s)."
        )
    if "imagery_water" in metrics:
        lines += imagery_water_notes(metrics)
    if "snow_colors" in metrics:
        lines += snow_color_notes(metrics["snow_colors"])
    if "coast_blend" in metrics:
        lines += coast_blend_notes(metrics["coast_blend"])
    if "activity" in metrics:
        lines += activity_notes(metrics["activity"])
    if "coast_gaps" in metrics:
        lines += coast_gap_notes(metrics["coast_gaps"])
    if "pack_streaks" in metrics:
        lines += pack_streak_notes(metrics["pack_streaks"])
    if "fractures" in metrics:
        lines += fracture_notes(metrics["fractures"], metrics.get("lead_area", {}))
    if "fields" in metrics:
        lines += fields_notes(metrics["fields"])
    if variant.remarks:
        lines += ["", "## Remarks", ""] + [f"- {r}" for r in variant.remarks]
    if variant.tests:
        lines += ["", "## Tests", ""] + [f"- {t}" for t in variant.tests]
    return lines


def coast_step_notes(view: dict, parent: str | None) -> list[str]:
    """Metric (e) of a view, pooled per kind of coast, against the parent's."""
    lines = []
    for kind in ("flat", "mountain"):
        here = view.get("coast_steps", {}).get(f"all-{kind}", {})
        if "dip_median" not in here:
            continue
        line = (
            f"  - (e) {kind} coasts under closed ice: step land less ice "
            f"{here['step_median']}, dip {here['dip_median']} (p90 "
            f"{here['dip_p90']}) over {here['texels']:,} texels"
        )
        before = view.get("parent_coast_steps", {}).get(f"all-{kind}", {})
        if "dip_median" in before:
            line += (
                f"; `{parent}` step {before['step_median']}, dip "
                f"{before['dip_median']} (p90 {before['dip_p90']})"
            )
        lines.append(line)
    return lines


def activity_notes(activity: dict) -> list[str]:
    """The pack's shares by activity, per view."""
    flat, structured = ACTIVITY_CLASSES
    lines = [
        "",
        "## Activity",
        "",
        f"Shares of the pack, coverage above a half: flat below {flat}, moderate "
        f"to {structured}, structured above.",
        "",
    ]
    for key, a in activity.items():
        lines.append(
            f"- `{key}`: flat {a['flat']:.0%}, moderate {a['moderate']:.0%}, "
            f"structured {a['structured']:.0%}"
        )
    return lines


PACK_STREAKS = (0.85, 0.8)
"""The pack's least concentration, and the floes' coverage below which a
texel of it counts as water in the pack."""


def pack_streaks(
    fields: sea_ice_look.Fields,
    cap: sea_ice_look.Cap,
    month: int,
    workers: int | None = None,
) -> dict[str, float]:
    """Measure the floes' water inside the pack.

    :param fields: The hemisphere's noise fields.
    :param cap: The month's cap, with the floes' own coverage.
    :param month: The month, 1 to 12.
    :param workers: Threads for reading the concentration through the eddy
        shift; the process's setting when ``None``.
    :returns: Of the cap's texels where the record's concentration is at
        least the pack's, the share where the floes' coverage is below the
        streak's, and their area in square kilometers; the same for the band
        from 0.6 to the pack, and, as ``read_pack_share`` and
        ``read_pack_km2``, where the concentration the floes read, through
        the eddy shift, is at least the pack's.
    """
    cover = cap.floes
    assert cover is not None
    coarse = fields.concentration[month - 1]
    conc = zoom(
        coarse, fields.size / coarse.shape[-1], order=1, mode="nearest", grid_mode=True
    )
    conc[fields.coast.land] = 0
    texel = sea_ice.texel_km(fields.size) ** 2
    pack, below = PACK_STREAKS
    insides = [
        ("pack", conc >= pack),
        ("inner_margin", (conc >= 0.6) & (conc < pack)),
    ]
    del conc
    read = np.empty((fields.size, fields.size), np.float32)

    def band(span: tuple[int, int]) -> None:
        _, _, conc = sea_ice_look.read_concentration(fields, month, *span)
        read[span[0] : span[1]] = conc

    parallel.pmap(band, sea_ice_look.rows(fields.size), workers)
    read[fields.coast.land] = 0
    insides.append(("read_pack", read >= pack))
    out: dict[str, float] = {}
    for name, inside in insides:
        streak = inside & (cover < below)
        out[f"{name}_share"] = round(float(streak.sum() / max(inside.sum(), 1)), 4)
        out[f"{name}_km2"] = round(float(streak.sum()) * texel)
    return out


COAST_GAP_OFFSETS = np.arange(0, 17)
"""Working face texels seaward of a coast texel that :func:`coast_gaps`
reads, about 1.85 km apart."""


def coast_gaps(rgb: np.ndarray, water: np.ndarray, face: str) -> dict:
    """Measure B4's question: open water along coasts closed offshore.

    From each texel of land at the coastline poleward of 60 degrees, the
    working face is read along the coast's normal, the gradient of the land's
    share smoothed over two texels: where it is closed ice
    15 to 30 km out (luminance above 200 on full water), the coast counts as
    closed, and as open where it is darker than 100 within 2 to 11 km, gray
    where it is darker than 180 there and not open.

    :param rgb: uint8 array ``(N, N, 3)``, the composited working face.
    :param water: uint8 array ``(N, N)``, its mask.
    :param face: ``py`` or ``ny``.
    :returns: The closed coast texels and the shares of them open and gray.
    """
    size = water.shape[0]
    cut = slice(size // 6, size - size // 6)
    lum = luma(rgb[cut, cut])
    wet = water[cut, cut].astype(np.float32) / 255
    latitude = face_latitude(face, size)[cut, cut]
    land = wet < 0.5
    rim = land & binary_dilation(~land) & (np.abs(latitude) >= 60)
    del latitude
    gr, gc = np.gradient(-gaussian_filter(land.astype(np.float32), 2.0))
    rows, cols = np.nonzero(rim)
    del rim
    norm = np.hypot(gr[rows, cols], gc[rows, cols]) + 1e-6
    offsets = COAST_GAP_OFFSETS[None, :]
    rr = rows[:, None] + offsets * (gr[rows, cols] / norm)[:, None]
    cc = cols[:, None] + offsets * (gc[rows, cols] / norm)[:, None]
    del gr, gc
    seen = map_coordinates(lum, [rr, cc], order=1, mode="nearest")
    sea = map_coordinates(wet, [rr, cc], order=1, mode="nearest")
    far = (COAST_GAP_OFFSETS >= 8) & (COAST_GAP_OFFSETS <= 16)
    near = (COAST_GAP_OFFSETS >= 1) & (COAST_GAP_OFFSETS <= 6)
    closed = (seen[:, far] > 200).all(1) & (sea[:, far] > 0.99).all(1)
    gap = closed & (seen[:, near] < 100).any(1)
    gray = closed & ~gap & (seen[:, near] < 180).any(1)
    total = max(int(closed.sum()), 1)
    return {
        "closed": int(closed.sum()),
        "open_share": round(float(gap.sum()) / total, 4),
        "gray_share": round(float(gray.sum()) / total, 4),
    }


def coast_gap_notes(gaps: dict) -> list[str]:
    """B4's measure per view."""
    lines = [
        "",
        "## Open water along closed coasts",
        "",
        "Of the coast texels poleward of 60 degrees with closed ice 15 to 30 km "
        "out, on the working face, the share with open water (luminance below "
        "100) within 2 to 11 km of the coast, and the share only gray (100 to "
        "180) there.",
        "",
    ]
    for key, g in gaps.items():
        lines.append(
            f"- `{key}`: open {g['open_share']:.1%}, gray {g['gray_share']:.1%} "
            f"of {g['closed']:,} closed coast texels"
        )
    return lines


def pack_streak_notes(streaks: dict) -> list[str]:
    """The floes' water inside the pack, per view."""
    pack, below = PACK_STREAKS
    lines = [
        "",
        "## Water in the pack",
        "",
        f"Where the record's concentration is at least {pack}, the share of the "
        f"cap the floes' water leaves less than {below} of; the band from 0.6 "
        f"to {pack} in brackets. Then, after the semicolon, the same where the "
        f"concentration the floes read through the eddy shift is at least "
        f"{pack}.",
        "",
    ]
    for key, s in streaks.items():
        read = ""
        if "read_pack_share" in s:
            read = (
                f"; {s['read_pack_share']:.1%}, {s['read_pack_km2'] / 1e3:,.0f} "
                "thousand km2"
            )
        lines.append(
            f"- `{key}`: {s['pack_share']:.1%}, {s['pack_km2'] / 1e3:,.0f} thousand "
            f"km2 ({s['inner_margin_share']:.1%}){read}"
        )
    return lines


def fracture_stats(fractures: sea_ice_look.Fractures) -> dict:
    """Count a hemisphere's fracture systems and measure their lengths."""
    length, branches, darkness = fractures.systems.T
    return {
        "systems": len(length),
        "branches": int(branches.sum()),
        "total_km": round(float(length.sum())),
        "length_km_p10_50_90": [
            round(float(v)) for v in np.percentile(length, (10, 50, 90))
        ],
        "darkness_p10_50_90": [
            round(float(v), 2) for v in np.percentile(darkness, (10, 50, 90))
        ],
    }


def fracture_notes(fractures: dict, lead_area: dict) -> list[str]:
    """The fracture systems and the share of the pack their open water takes."""
    lines = ["", "## The fractures", ""]
    for hemisphere, f in fractures.items():
        lines.append(
            f"- `{hemisphere}`: {f['systems']} systems with {f['branches']} "
            f"branches, {f['total_km']:,} km in all; a system's length, branches "
            f"included, {f['length_km_p10_50_90']} km at the 10th, 50th and 90th "
            f"percentile; darkness {f['darkness_p10_50_90']}"
        )
    for key, share in lead_area.items():
        lines.append(
            f"- `{key}`: the leads' share of the fill takes {share:.2%} of the "
            f"pack's ice ({share * 1000:.2f} per mille)"
        )
    return lines


def fields_notes(fields: dict) -> list[str]:
    """What making the noise fields cost, when this code made them."""
    lines = ["", "## The noise fields", ""]
    for hemisphere, f in fields.items():
        steps = ", ".join(f"{k} {v} s" for k, v in f["seconds"].items())
        total = round(sum(f["seconds"].values()), 1)
        size = round(sum(f["mib"].values()))
        peak = (
            f"; peak {f['peak_gib']} GiB of traced allocations"
            if f["peak_gib"] is not None
            else ""
        )
        lines.append(
            f"- `{hemisphere}`: {total} s of work ({steps}){peak}; {size} MiB kept: "
            + ", ".join(f"{k} {v}" for k, v in f["mib"].items())
        )
    return lines


def coast_blend_notes(blends: dict) -> list[str]:
    """Where the coast's land took the ice, per view."""
    lines = ["", "## The coast's land", ""]
    for key, b in blends.items():
        lines.append(
            f"- `{key}`: {b['texels_blended']:,} texels of the {b['working_size']}"
            f" working face took more than 1% of the ice, "
            f"{b['texels_mostly_ice']:,} more than half; {b['seconds']} s; "
            f"image `{b['image']}` (white: the share of the texel the ice took)"
        )
    return lines


def imagery_water_notes(metrics: dict) -> list[str]:
    """What B1 joined."""
    water = metrics["imagery_water"]
    t = water["thresholds"]
    lines = [
        "",
        "## The water from the Blue Marble",
        "",
        f"A pixel is the imagery's water in every month when its brightest Rec. "
        f"709 luminance over the year is at most {t['max_luma']} and its blue is "
        f"at least {t['min_blue']} above its red in every month; where it is "
        "connected to the ocean it joins it.",
        "",
    ]
    for name, count in water["joined"].items():
        lines.append(
            f"- Joined in the {name}: {count['source_pixels']:,} source pixels, "
            f"about {count['km2']:,} km2"
        )
    change = metrics.get("mask_change", {})
    for face, c in change.items():
        lines.append(
            f"- Mask face `{face}` against the baseline: "
            f"{c['texels_gaining_water']:,} texels gain water "
            f"({c['water_gained_texel_units']:,} texels' worth), "
            f"{c['texels_losing_water']:,} lose it; image `{c['image']}`"
        )
    lines.append(
        "- `imagery-water-world.png`: where the join applies (red), each pixel 8 "
        "source pixels square."
    )
    return lines


def snow_color_notes(snow: dict) -> list[str]:
    """What B2 measured and the colors it gave the months."""
    lines = ["", "## The ice's color from the coastal snow", ""]
    for hemisphere, s in snow.items():
        name = "North" if hemisphere == "nh" else "South"
        months = ", ".join(f"{m}: {tuple(c)}" for m, c in s["per_month_median"].items())
        lines.append(
            f"- {name} (`{s['face']}` poleward of {s['min_latitude']:.0f}, months "
            f"{s['months']}): coastal snow median {tuple(s['pooled_median'])} over "
            f"{s['texels']:,} texels; per month {months}"
        )
        winter = tuple(s.get("winter", s["pooled_median"]))
        if winter != tuple(s["pooled_median"]):
            lines.append(f"  - winter color moved to {winter}")
        if s.get("darkening", 1.0) != 1.0:
            lines.append(
                f"  - each month keeps {s['darkening']:.3g} of its darkening "
                "against winter, per channel"
            )
        lines += [
            "  - colors by month, before: "
            + ", ".join(str(tuple(c)) for c in s["table_before"]),
            "  - after: " + ", ".join(str(tuple(c)) for c in s["table_after"]),
        ]
    return lines


def imagery_water_overview(raster: np.ndarray, flags: np.ndarray, out: Path) -> None:
    """Draw where B1 joined water on the world."""
    height, width = flags.shape
    pooled = raster[: height * POOL, : width * POOL].reshape(height, POOL, width, POOL)
    ocean = pooled.mean(axis=(1, 3)) >= 128
    image = np.where(ocean[..., None], (20, 40, 80), (110, 110, 110)).astype(np.uint8)
    image[flags == 1] = (255, 40, 0)
    Image.fromarray(image).save(out / "imagery-water-world.png")


def run_variant(
    root: Path,
    name: str,
    views: tuple[View, ...],
    report: Report = print,
    workers: int | None = None,
    warm_check: bool = False,
) -> dict:
    """Run a variant by name.

    :param root: The harness's directory.
    :param name: One of :data:`VARIANTS`.
    :param views: The four views.
    :param report: Told what is happening.
    :param workers: Threads; the process's setting when ``None``.
    :param warm_check: Render in turn and once warm, to measure the packs.
    :returns: The variant's metrics.
    """
    return Harness(root, VARIANTS[name], views, report, workers, warm_check).run()


def against_shipped(shipped: Path, textures: Path, views: tuple[View, ...]) -> dict:
    """Compare the variant's rebaked faces, decoded, with the shipped ones.

    For the baseline this is the check that the harness bakes what the
    production bake did.

    :returns: Per mask face and per view's day face, the texels that differ and
        the largest difference.
    """
    pairs: dict[str, Path] = {
        f"mask-{face}": Path("mask") / f"{face}.jxl" for face in ("py", "ny")
    }
    for view in views:
        pairs[view.key] = Path("day") / view.stamp / f"{view.face}.jxl"
    out = {}
    for name, relative in pairs.items():
        a = _open_rgb(shipped / relative).astype(np.int16)
        b = _open_rgb(textures / relative).astype(np.int16)
        delta = np.abs(a - b).max(axis=-1)
        out[name] = {
            "texels_differing": int((delta > 0).sum()),
            "largest_difference": int(delta.max()),
            "bytes_identical": (shipped / relative).read_bytes()
            == (textures / relative).read_bytes(),
        }
    return out
