"""Tests for the sea ice source, its caps and its look.

The real monthly files are not in the repository; ``conftest`` writes a
synthetic year of them on the 25 km EASE2 grid. Point
``TEXTURE_PIPELINE_OSI_DIR`` at a directory holding the 24 files of 2004
(``texture-pipeline cube --sea-ice <dir> --fetch`` fills one) to run the case
that holds the real bake's area to the area the floes are asked to draw; run
it with ``-s`` to see the area against the record too. Without the directory
that case skips.
"""

import csv
import os
from pathlib import Path

import numpy as np
import pillow_jxl  # noqa: F401 - registers the JPEG XL plugin with Pillow
import pytest
from conftest import synthetic_concentration
from PIL import Image

from texture_pipeline import sea_ice, sea_ice_look
from texture_pipeline.cube import FACES, face_directions

POINTS = Path(__file__).parent / "fixtures" / "sea_ice_points.csv"
SHIPPED_MASK = Path(__file__).resolve().parents[3] / "textures" / "mask"
LFS_POINTER = b"version https://git-lfs"
AREA_TOLERANCE = 0.10
FILL = (10, 30, 60)
SMALL_CAP = 256


@pytest.fixture(scope="module")
def north(osi_monthly_dir: Path) -> sea_ice.Hemisphere:
    """The synthetic north."""
    return sea_ice.read_hemisphere(osi_monthly_dir, "nh")


def no_land(size: int) -> np.ndarray:
    """A cap without land."""
    return np.zeros((size, size), dtype=bool)


@pytest.fixture(scope="module")
def north_fields(north: sea_ice.Hemisphere) -> sea_ice_look.Fields:
    """The synthetic north's noise fields on a small cap."""
    return sea_ice_look.prepare(
        sea_ice.cap_concentration(north, 128), "nh", SMALL_CAP, land=no_land(SMALL_CAP)
    )


def pinned_points() -> list[dict[str, str]]:
    with POINTS.open(encoding="utf-8") as file:
        return list(csv.DictReader(file))


class TestProjection:
    def test_the_cap_and_its_inverse_round_trip(self) -> None:
        latitude, longitude = np.meshgrid(
            np.linspace(36, 89.5, 40), np.linspace(-179, 179, 60), indexing="ij"
        )
        for sign in (1, -1):
            south, x, y = sea_ice.latlon_to_cap(sign * latitude, longitude)
            assert np.all(south == (sign < 0))
            back_lat, back_lon = sea_ice.cap_to_latlon(x, y, "nh" if sign > 0 else "sh")
            np.testing.assert_allclose(back_lat, sign * latitude, atol=1e-9)
            np.testing.assert_allclose(back_lon, longitude, atol=1e-9)

    def test_rho_max_reaches_35_degrees_on_the_axes(self) -> None:
        latitude, _ = sea_ice.cap_to_latlon(
            np.array(sea_ice.RHO_MAX), np.array(0.0), "nh"
        )
        assert float(latitude) == pytest.approx(35.0)

    def test_the_pinned_points_are_where_the_cap_puts_them(self) -> None:
        rows = pinned_points()
        assert len(rows) >= 8
        for row in rows:
            south, x, y = sea_ice.latlon_to_cap(
                np.array(float(row["latitude"])), np.array(float(row["longitude"]))
            )
            assert bool(south) == (row["cap"] == "1")
            assert float(0.5 + x / (2 * sea_ice.RHO_MAX)) == pytest.approx(
                float(row["u"]), abs=1e-6
            )
            assert float(0.5 - y / (2 * sea_ice.RHO_MAX)) == pytest.approx(
                float(row["v"]), abs=1e-6
            )

    def test_the_pinned_points_reach_their_source_cells_through_proj(self) -> None:
        """Each point is the center of a cell of a real file, its latitude and
        longitude read from that file's ``lat`` and ``lon``: the cap's inverse
        and PROJ's EASE2 put it back on that cell."""
        for row in pinned_points():
            hemisphere = sea_ice.HEMISPHERES[int(row["cap"])]
            u, v = float(row["u"]), float(row["v"])
            x = np.array((u - 0.5) * 2 * sea_ice.RHO_MAX)
            y = np.array((0.5 - v) * 2 * sea_ice.RHO_MAX)
            latitude, longitude = sea_ice.cap_to_latlon(x, y, hemisphere)
            source_row, source_col = sea_ice.ease2_cells(
                latitude, longitude, hemisphere
            )
            expected_col = (float(row["x_km"]) + sea_ice.SOURCE_HALF_KM) / 25 - 0.5
            expected_row = (sea_ice.SOURCE_HALF_KM - float(row["y_km"])) / 25 - 0.5
            assert float(source_col) == pytest.approx(expected_col, abs=0.01)
            assert float(source_row) == pytest.approx(expected_row, abs=0.01)

    def test_a_face_texel_lands_on_the_cap_where_its_latitude_says(self) -> None:
        x, y, z = face_directions("py", 64, 0, 64)
        south, px, py = sea_ice.directions_to_cap(x, y, z)
        assert not south.any()
        latitude, _ = sea_ice.cap_to_latlon(px, py, "nh")
        expected = np.degrees(np.arctan2(y, np.hypot(x, z)))
        np.testing.assert_allclose(latitude, expected, atol=1e-9)


class TestSource:
    def test_land_takes_its_nearest_ocean_value_up_to_three_cells(self) -> None:
        land = np.zeros((12, 12), dtype=bool)
        land[:, 4:] = True
        means = np.where(land, np.nan, 0.5)[None].astype(np.float32)
        means[0, :, 3] = 0.8
        extended = sea_ice.extend_into_land(means, land)
        assert not np.isnan(extended).any()
        np.testing.assert_allclose(extended[0, :, 4:7], 0.8)
        np.testing.assert_array_equal(extended[0, :, 7:], 0.0)
        np.testing.assert_allclose(extended[0, :, :3], 0.5)

    def test_a_month_with_other_land_is_refused(self) -> None:
        def read(path: Path) -> np.ndarray:
            field = synthetic_concentration("nh", 1) / 100
            if path.name.endswith("200407.nc"):
                field[0, 0] = np.nan
            return field

        with pytest.raises(ValueError, match="other land cells"):
            sea_ice.read_hemisphere(Path("."), "nh", read=read)

    def test_the_files_read_back_as_fractions(self, north: sea_ice.Hemisphere) -> None:
        expected = synthetic_concentration("nh", 4) / 100
        np.testing.assert_allclose(north.means[3], expected, atol=1e-4)
        np.testing.assert_array_equal(north.land, np.isnan(expected))

    def test_the_source_area_follows_the_season(
        self, north: sea_ice.Hemisphere
    ) -> None:
        area = sea_ice.source_area_km2(north)
        assert int(np.argmax(area)) == 2
        assert int(np.argmin(area)) == 8

    def test_the_cap_is_bounded_and_zero_far_from_the_ice(
        self, north: sea_ice.Hemisphere
    ) -> None:
        caps = sea_ice.cap_concentration(north, 64)
        assert caps.shape == (12, 64, 64)
        assert caps.min() >= 0
        assert caps.max() <= 1
        assert caps[2, 32, 32] == pytest.approx(1.0, abs=1e-3)
        assert caps[2, 0, 0] == 0


class TestFetch:
    def test_only_the_missing_files_are_fetched_one_by_one(
        self, tmp_path: Path
    ) -> None:
        present = sea_ice.file_name("nh", 1)
        (tmp_path / present).write_bytes(b"kept")
        asked: list[str] = []

        def opener(url: str) -> bytes:
            asked.append(url)
            return url.rsplit("/", 1)[-1].encode()

        assert sea_ice.fetch(tmp_path, opener=opener) == 23
        assert len(asked) == len(set(asked)) == 23
        assert all(url.startswith(f"{sea_ice.THREDDS}/2004/") for url in asked)
        assert (tmp_path / present).read_bytes() == b"kept"
        assert not list(tmp_path.glob("*.part"))
        manifest = (tmp_path / sea_ice.MANIFEST).read_text(encoding="utf-8")
        lines = manifest.splitlines()
        assert len(lines) == 24
        digest, name = lines[0].split("  ")
        assert digest == sea_ice.sha256(tmp_path / name)
        assert sea_ice.missing_files(tmp_path) == []

    def test_a_complete_directory_fetches_nothing(self, tmp_path: Path) -> None:
        for name in sea_ice.year_files():
            (tmp_path / name).write_bytes(b"x")

        def opener(url: str) -> bytes:
            raise AssertionError(url)

        assert sea_ice.fetch(tmp_path, opener=opener) == 0


class TestNoise:
    def test_a_seeded_noise_is_the_same_every_time(self) -> None:
        first = sea_ice_look.band_noise(128, (100, 400), np.random.default_rng(7))
        again = sea_ice_look.band_noise(128, (100, 400), np.random.default_rng(7))
        other = sea_ice_look.band_noise(128, (100, 400), np.random.default_rng(8))
        np.testing.assert_array_equal(first, again)
        assert not np.array_equal(first, other)
        assert float(first.std()) == pytest.approx(1.0, rel=1e-3)
        assert abs(float(first.mean())) < 1e-3

    @pytest.mark.parametrize("size", [16, 256])
    def test_a_band_finer_than_the_texels_is_zero(self, size: int) -> None:
        assert sea_ice.texel_km(size) > 20
        noise = sea_ice_look.band_noise(size, (5, 20), np.random.default_rng(0))
        assert not np.any(noise)

    def test_a_band_the_texels_reach_is_drawn(self) -> None:
        assert sea_ice.texel_km(256) < 120
        noise = sea_ice_look.band_noise(256, (10, 120), np.random.default_rng(0))
        assert float(noise.std()) == pytest.approx(1.0, rel=1e-3)

    def test_a_band_left_out_leaves_the_generator_where_a_drawn_one_does(
        self,
    ) -> None:
        left_out, drawn = np.random.default_rng(3), np.random.default_rng(3)
        sea_ice_look.band_noise(16, (5, 20), left_out)
        sea_ice_look.band_noise(16, (500, 2000), drawn)
        assert left_out.random() == drawn.random()

    def test_the_fields_do_not_depend_on_the_month(
        self, north: sea_ice.Hemisphere, north_fields: sea_ice_look.Fields
    ) -> None:
        again = sea_ice_look.prepare(
            sea_ice.cap_concentration(north, 128),
            "nh",
            SMALL_CAP,
            land=no_land(SMALL_CAP),
        )
        np.testing.assert_array_equal(again.fractures.open, north_fields.fractures.open)
        np.testing.assert_array_equal(again.surface.relief, north_fields.surface.relief)
        np.testing.assert_array_equal(again.cascade.ids, north_fields.cascade.ids)


def uniform_fields(conc: float, size: int = 512) -> sea_ice_look.Fields:
    stack = np.full((12, 32, 32), conc, dtype=np.float32)
    return sea_ice_look.prepare(stack, "sh", size, land=no_land(size))


class TestCoverage:
    @pytest.mark.parametrize("conc", [0.3, 0.5, 0.78])
    def test_the_coverage_follows_the_toned_concentration(self, conc: float) -> None:
        """Over a uniform concentration the floes and the leads together draw
        the concentration the tone curve reads, which is the concentration
        itself below the curve's start and more above it."""
        cap = sea_ice_look.month_cap(uniform_fields(conc), 9)
        expected = float(
            sea_ice_look.toned(np.array(conc, np.float32), sea_ice_look.LOOK.floe_tone)
        )
        assert float(cap.coverage.mean()) == pytest.approx(expected, abs=0.04)

    def test_full_ice_has_no_floes_water_and_open_water_no_ice(self) -> None:
        full = sea_ice_look.month_cap(uniform_fields(1.0, 128), 9)
        assert full.floes is not None
        assert full.floes.min() == 1
        assert full.coverage.mean() > 0.99
        assert sea_ice_look.month_cap(uniform_fields(0.0, 128), 9).coverage.max() == 0

    def test_a_month_moves_the_edge_and_keeps_the_interior(
        self, north_fields: sea_ice_look.Fields
    ) -> None:
        march = sea_ice_look.month_cap(north_fields, 3)
        november = sea_ice_look.month_cap(north_fields, 11)
        assert march.coverage.sum() > november.coverage.sum()
        pole = slice(SMALL_CAP // 2 - 16, SMALL_CAP // 2 + 16)
        assert march.floes is not None
        np.testing.assert_array_equal(march.floes[pole, pole], 1)
        np.testing.assert_array_equal(
            march.brightness[pole, pole], november.brightness[pole, pole]
        )


def month_ice(coverage: float, brightness: float = 1.0) -> sea_ice_look.MonthIce:
    caps = {
        name: sea_ice_look.Cap(
            np.full((8, 8), coverage, np.float32),
            np.full((8, 8), brightness, np.float32),
            (230, 235, 240),
        )
        for name in sea_ice.HEMISPHERES
    }
    return sea_ice_look.MonthIce(caps)


def flattened_face(size: int, land: tuple[int, int, int]) -> tuple:
    water = np.full((size, size), 255, dtype=np.uint8)
    water[:, : size // 4] = 0
    water[:, size // 4] = 128
    share = water.astype(np.float32)[..., None] / 255
    flat = np.clip(
        np.asarray(land, np.float32) * (1 - share)
        + np.asarray(FILL, np.float32) * share,
        0,
        255,
    ).astype(np.uint8)
    return flat, water


class TestComposite:
    def test_land_is_untouched_and_water_takes_the_ice(self) -> None:
        flat, water = flattened_face(32, (90, 120, 40))
        before = flat.copy()
        sea_ice_look.composite_face(flat, water, "py", month_ice(1.0), FILL)
        land = water == 0
        np.testing.assert_array_equal(flat[land], before[land])
        np.testing.assert_array_equal(flat[water == 255][0], (230, 235, 240))

    def test_a_coast_texel_keeps_its_land_share(self) -> None:
        flat, water = flattened_face(32, (90, 120, 40))
        sea_ice_look.composite_face(flat, water, "py", month_ice(1.0), FILL)
        share = 128 / 255
        expected = (
            np.array((90, 120, 40)) * (1 - share) + np.array((230, 235, 240)) * share
        )
        np.testing.assert_allclose(flat[0, 8], expected, atol=1.5)

    def test_open_water_without_ice_stays_the_fill(self) -> None:
        flat, water = flattened_face(32, (90, 120, 40))
        before = flat.copy()
        sea_ice_look.composite_face(flat, water, "py", month_ice(0.0), FILL)
        np.testing.assert_array_equal(flat, before)

    def test_nothing_is_sampled_below_the_caps_reach(self) -> None:
        flat, water = flattened_face(32, (90, 120, 40))
        before = flat.copy()
        area = sea_ice_look.composite_face(flat, water, "pz", month_ice(1.0), FILL)
        rows = slice(12, 20)
        np.testing.assert_array_equal(flat[rows], before[rows])
        assert area["nh"] > 0
        assert area["sh"] > 0

    def test_the_area_of_a_fully_covered_cube_is_the_reach_of_the_caps(
        self,
    ) -> None:
        total = {"nh": 0.0, "sh": 0.0}
        for face in FACES:
            water = np.full((64, 64), 255, dtype=np.uint8)
            flat = np.zeros((64, 64, 3), dtype=np.uint8)
            for name, km2 in sea_ice_look.composite_face(
                flat, water, face, month_ice(1.0), FILL
            ).items():
                total[name] += km2
        reach = (
            2 * np.pi * (1 - np.sin(np.radians(35.0))) * sea_ice.AUTHALIC_RADIUS_KM**2
        )
        for km2 in total.values():
            assert km2 == pytest.approx(reach, rel=0.02)


def osi_directory() -> Path:
    """Return the directory of the real monthly files, or skip the test."""
    value = os.environ.get("TEXTURE_PIPELINE_OSI_DIR", "").strip()
    if not value:
        pytest.skip("TEXTURE_PIPELINE_OSI_DIR names no directory of OSI-450-a1 files")
    directory = Path(value)
    missing = sea_ice.missing_files(directory)
    if missing:
        pytest.skip(f"{len(missing)} monthly files missing from {directory}")
    return directory


def shipped_mask() -> dict[str, np.ndarray]:
    """Return the shipped mask faces, or skip the test."""
    masks = {}
    for face in FACES:
        path = SHIPPED_MASK / f"{face}.jxl"
        with path.open("rb") as file:
            if file.read(len(LFS_POINTER)) == LFS_POINTER:
                pytest.skip("the shipped mask is a Git LFS pointer in this checkout")
        with Image.open(path) as image:
            masks[face] = np.asarray(image)
    return masks


class TestRealIce:
    def test_the_drawn_area_follows_the_toned_source_in_every_month(self) -> None:
        """The coverage over the shipped mask's water, weighted by its share
        of water and the texels' area, against the area the floes are asked to
        draw: the source's concentration read through the tone curve, times
        its cells' area. The curve adds ice against the record itself by
        design, so that comparison is printed rather than held."""
        directory = osi_directory()
        masks = shipped_mask()
        fields = {}
        record = {}
        toned = {}
        for name in sea_ice.HEMISPHERES:
            hemisphere = sea_ice.read_hemisphere(directory, name)
            record[name] = sea_ice.source_area_km2(hemisphere)
            toned[name] = sea_ice_look.toned_area_km2(hemisphere)
            fields[name] = sea_ice_look.prepare(
                sea_ice.cap_concentration(sea_ice_look.fast_ice(hemisphere)),
                name,
                sea_ice.CAP_SIZE,
                land=sea_ice_look.cap_land(masks, name, sea_ice.CAP_SIZE),
            )
        failures = []
        for month in sea_ice.MONTHS:
            ice = sea_ice_look.MonthIce(
                {n: sea_ice_look.month_cap(f, month) for n, f in fields.items()}
            )
            drawn = dict.fromkeys(sea_ice.HEMISPHERES, 0.0)
            for face, water in masks.items():
                flat = np.zeros((*water.shape, 3), dtype=np.uint8)
                found = sea_ice_look.composite_face(flat, water, face, ice, FILL)
                for name, km2 in found.items():
                    drawn[name] += km2
            for name in sea_ice.HEMISPHERES:
                asked = float(toned[name][month - 1])
                source = float(record[name][month - 1])
                line = (
                    f"{name} {month:02d}: drawn {drawn[name] / 1e6:.3f} million km2, "
                    f"toned source {asked / 1e6:.3f} "
                    f"({(drawn[name] / asked - 1) * 100:+.1f}%), record "
                    f"{source / 1e6:.3f} ({(drawn[name] / source - 1) * 100:+.1f}%)"
                )
                print(line)
                if abs(drawn[name] / asked - 1) > AREA_TOLERANCE:
                    failures.append(line)
        assert not failures, failures
