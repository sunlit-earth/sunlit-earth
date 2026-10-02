"""Tests for the equi-angular cube bake."""

import os
import shutil
from concurrent.futures import ThreadPoolExecutor
from math import atan, pi
from pathlib import Path

import geopandas as gpd
import numpy as np
import pillow_jxl  # noqa: F401 - registers the JPEG XL plugin with Pillow
import pytest
from PIL import Image
from shapely.geometry import box
from shapely.ops import unary_union
from typer.testing import CliRunner

from texture_pipeline.cube import (
    BAND_ROWS,
    FACES,
    area_average,
    equirect_coordinates,
    face_directions,
    reproject_face,
    sample_bilinear,
    working_size,
)
from texture_pipeline.main import app

runner = CliRunner()

FIXTURE = Path(__file__).parent / "fixtures" / "cube"
FIXTURE_FACE = 16
FIXTURE_MONTHS = (1, 7)
OCEAN_FILL = (10, 30, 60)


def texel_of(direction: tuple[float, float, float], size: int) -> tuple[str, int, int]:
    """Select a face and texel the way the OpenGL cube map table does.

    :param direction: Any nonzero direction in the world frame.
    :param size: Face edge length in texels.
    :returns: Face name, row and column.
    """
    x, y, z = direction
    ax, ay, az = abs(x), abs(y), abs(z)
    if ax >= ay and ax >= az:
        face, sc, tc, ma = ("px", -z, -y, ax) if x > 0 else ("nx", z, -y, ax)
    elif ay >= az:
        face, sc, tc, ma = ("py", x, z, ay) if y > 0 else ("ny", x, -z, ay)
    else:
        face, sc, tc, ma = ("pz", x, -y, az) if z > 0 else ("nz", -x, -y, az)
    s = atan(sc / ma) * 4 / pi
    t = atan(tc / ma) * 4 / pi
    col = min(int((s + 1) / 2 * size), size - 1)
    row = min(int((t + 1) / 2 * size), size - 1)
    return face, row, col


def direction_of(
    face: str, size: int, row: int, col: int
) -> tuple[float, float, float]:
    """Return the direction through one texel center."""
    x, y, z = face_directions(face, size, row, row + 1)
    return float(x[0, col]), float(y[0, col]), float(z[0, col])


def lon_lat(face: str, size: int, row: int, col: int) -> tuple[float, float]:
    """Return the longitude and latitude in degrees of one texel center."""
    x, y, z = face_directions(face, size, row, row + 1)
    src_row, src_col = equirect_coordinates(x, y, z, 360, 180)
    return float(src_col[0, col]) + 0.5 - 180, 90 - (float(src_row[0, col]) + 0.5)


def write_fixture_sources(root: Path) -> tuple[Path, Path, Path]:
    """Write two tiny monthly maps, a night map and an ocean shapefile.

    The land is one box on each face around the equator, one in the north and
    a cap south of 70 S, so a face swapped or turned changes the bake. January
    carries a bright block in the Arctic ocean for the ice detector to find.

    :param root: Directory to write into.
    :returns: The day directory, the night map and the shapefile.
    """
    days = root / "day"
    days.mkdir(parents=True)
    rows, cols = np.mgrid[0:64, 0:128]
    for month in FIXTURE_MONTHS:
        blue = np.full_like(rows, 40 + 10 * month)
        rgb = np.stack([cols * 2, rows * 4, blue], axis=-1).astype(np.uint8)
        if month == 1:
            rgb[1:9, 20:36] = 240
        Image.fromarray(rgb).save(days / f"world.topo.2004{month:02d}.png")

    night_rows, night_cols = np.mgrid[0:32, 0:64]
    night = np.stack(
        [night_cols * 4, night_rows * 8, np.full_like(night_rows, 128)], axis=-1
    ).astype(np.uint8)
    night_path = root / "night.png"
    Image.fromarray(night).save(night_path)

    land = unary_union(
        [
            box(-20, -40, 30, 30),
            box(70, 0, 120, 40),
            box(-110, -30, -70, 10),
            box(150, -35, 175, -10),
            box(-50, 60, -20, 80),
            box(-180, -90, 180, -70),
        ]
    )
    ocean = box(-180, -90, 180, 90).difference(land)
    shapefile = root / "ocean.shp"
    gpd.GeoDataFrame(geometry=[ocean], crs="EPSG:4326").to_file(shapefile)
    return days, night_path, shapefile


def bake_arguments(days: Path, night: Path, shapefile: Path, out: Path) -> list[str]:
    """Return the command line of the fixture bake."""
    return [
        "cube",
        "--input",
        str(days),
        "--night",
        str(night),
        "--ocean-mask",
        str(shapefile),
        "--output",
        str(out),
        "--face-size",
        str(FIXTURE_FACE),
        "--quality",
        "100",
    ]


def decode(path: Path) -> np.ndarray:
    """Decode a JPEG XL file into an array."""
    with Image.open(path) as image:
        return np.asarray(image)


@pytest.fixture(scope="module")
def fixture_bake(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """Bake the fixture sources once and return the output directory."""
    root = tmp_path_factory.mktemp("cube")
    days, night, shapefile = write_fixture_sources(root / "src")
    out = root / "out"
    result = runner.invoke(app, bake_arguments(days, night, shapefile, out))
    assert result.exit_code == 0, f"CLI failed: {result.output}"
    return out


class TestGeometry:
    def test_face_centers_point_along_their_axes(self) -> None:
        axes = {
            "px": (1, 0, 0),
            "nx": (-1, 0, 0),
            "py": (0, 1, 0),
            "ny": (0, -1, 0),
            "pz": (0, 0, 1),
            "nz": (0, 0, -1),
        }
        for face, axis in axes.items():
            np.testing.assert_allclose(direction_of(face, 5, 2, 2), axis, atol=1e-12)

    def test_every_texel_center_selects_its_own_texel(self) -> None:
        size = 8
        for face in FACES:
            for row in range(size):
                for col in range(size):
                    direction = direction_of(face, size, row, col)
                    assert texel_of(direction, size) == (face, row, col)

    def test_the_frame_puts_longitude_zero_on_pz_and_north_on_py(self) -> None:
        centers = {
            "pz": (0, 0),
            "px": (90, 0),
            "nx": (-90, 0),
            "py": (None, 90),
            "ny": (None, -90),
        }
        for face, (lon, lat) in centers.items():
            got_lon, got_lat = lon_lat(face, 5, 2, 2)
            assert got_lat == pytest.approx(lat, abs=1e-9)
            if lon is not None:
                assert got_lon == pytest.approx(lon, abs=1e-9)
        assert abs(lon_lat("nz", 5, 2, 2)[0]) == pytest.approx(180, abs=1e-9)

    def test_rows_run_north_to_south_and_columns_west_to_east_on_pz(self) -> None:
        assert lon_lat("pz", 5, 0, 2)[1] > 0 > lon_lat("pz", 5, 4, 2)[1]
        assert lon_lat("pz", 5, 2, 0)[0] < 0 < lon_lat("pz", 5, 2, 4)[0]

    def test_the_polar_faces_turn_their_edges_as_the_table_says(self) -> None:
        assert lon_lat("py", 5, 4, 2)[0] == pytest.approx(0, abs=1e-9)
        assert lon_lat("py", 5, 2, 4)[0] == pytest.approx(90, abs=1e-9)
        assert lon_lat("ny", 5, 0, 2)[0] == pytest.approx(0, abs=1e-9)
        assert lon_lat("ny", 5, 2, 4)[0] == pytest.approx(90, abs=1e-9)

    def test_texels_span_equal_angles_along_a_face_axis(self) -> None:
        size = 64
        x, _, z = face_directions("pz", size, size // 2, size // 2 + 1)
        angles = np.degrees(np.arctan2(x[0], z[0]))
        np.testing.assert_allclose(np.diff(angles), 90 / size, rtol=1e-3)


class TestSampling:
    def test_texel_centers_return_the_texel(self) -> None:
        source = np.arange(24, dtype=np.uint8).reshape(4, 6)
        row, col = np.mgrid[0:4, 0:6].astype(np.float64)
        np.testing.assert_array_equal(sample_bilinear(source, row, col), source)

    def test_columns_wrap_across_the_antimeridian(self) -> None:
        source = np.zeros((2, 4), dtype=np.uint8)
        source[:, 0] = 100
        source[:, 3] = 200
        edge = sample_bilinear(source, np.array([0.0, 0.0]), np.array([-0.5, 3.5]))
        np.testing.assert_array_equal(edge, [150, 150])

    def test_rows_clamp_at_the_poles(self) -> None:
        source = np.array([[10, 10], [90, 90]], dtype=np.uint8)
        pole = sample_bilinear(source, np.array([-0.5, 1.5]), np.array([0.0, 0.0]))
        np.testing.assert_array_equal(pole, [10, 90])

    def test_a_marker_at_longitude_zero_lands_on_pz_only(self) -> None:
        source = np.zeros((180, 360, 3), dtype=np.uint8)
        source[80:100, 170:190] = (255, 0, 0)
        for face in FACES:
            red = reproject_face(source, face, 32)[..., 0]
            if face == "pz":
                assert red[16, 16] == 255
            else:
                assert red.max() == 0

    def test_bands_in_a_pool_match_a_sequential_resample(self) -> None:
        rng = np.random.default_rng(3)
        source = rng.integers(0, 256, size=(90, 180), dtype=np.uint8)
        size = BAND_ROWS + 44
        with ThreadPoolExecutor(max_workers=3) as pool:
            pooled = reproject_face(source, "py", size, pool)
        np.testing.assert_array_equal(pooled, reproject_face(source, "py", size))


class TestSizes:
    def test_working_size_keeps_the_equator_density(self) -> None:
        assert working_size(21600, 2048) == 5400
        assert working_size(13500, 2048) == 3375

    def test_working_size_never_falls_below_the_face(self) -> None:
        assert working_size(8192, 2048) == 2048
        assert working_size(4000, 2048) == 2048

    def test_area_average_keeps_a_constant_face(self) -> None:
        face = np.full((27, 27, 3), (10, 30, 60), dtype=np.uint8)
        np.testing.assert_array_equal(area_average(face, 8), face[:8, :8])

    def test_area_average_takes_the_mean_of_each_footprint(self) -> None:
        face = np.array([[0, 100], [200, 100]], dtype=np.uint8)
        assert area_average(face, 1)[0, 0] == 100


class TestCubeCommand:
    def test_help_lists_the_options(self) -> None:
        result = runner.invoke(app, ["cube", "--help"])
        assert result.exit_code == 0
        for word in ["--input", "--night", "--ocean-mask", "--output", "--face-size"]:
            assert word in result.output

    def test_an_input_without_months_is_refused(self, tmp_path: Path) -> None:
        _, night, shapefile = write_fixture_sources(tmp_path / "src")
        empty = tmp_path / "empty"
        empty.mkdir()
        result = runner.invoke(
            app, bake_arguments(empty, night, shapefile, tmp_path / "out")
        )
        assert result.exit_code == 2
        assert "No monthly maps" in result.output

    def test_months_of_different_sizes_are_refused(self, tmp_path: Path) -> None:
        days, night, shapefile = write_fixture_sources(tmp_path / "src")
        Image.new("RGB", (256, 128)).save(days / "world.topo.200403.png")
        result = runner.invoke(
            app, bake_arguments(days, night, shapefile, tmp_path / "out")
        )
        assert result.exit_code == 2
        assert "differ in size" in result.output

    def test_a_face_larger_than_the_source_allows_is_refused(
        self, tmp_path: Path
    ) -> None:
        days, night, shapefile = write_fixture_sources(tmp_path / "src")
        arguments = bake_arguments(days, night, shapefile, tmp_path / "out")
        arguments[arguments.index("--face-size") + 1] = "64"
        result = runner.invoke(app, arguments)
        assert result.exit_code == 2
        assert "at least 256 wide" in result.output


class TestFixtureBake:
    def test_the_committed_fixture_matches_a_fresh_bake(
        self, fixture_bake: Path
    ) -> None:
        if os.environ.get("TEXTURE_PIPELINE_UPDATE_FIXTURES"):
            shutil.rmtree(FIXTURE, ignore_errors=True)
            shutil.copytree(fixture_bake, FIXTURE)
        fresh = sorted(p.relative_to(fixture_bake) for p in fixture_bake.rglob("*"))
        committed = sorted(p.relative_to(FIXTURE) for p in FIXTURE.rglob("*"))
        assert fresh == committed
        for relative in fresh:
            if (fixture_bake / relative).is_dir():
                continue
            np.testing.assert_array_equal(
                decode(fixture_bake / relative),
                decode(FIXTURE / relative),
                err_msg=f"{relative} differs from a fresh bake; rerun the test "
                "with TEXTURE_PIPELINE_UPDATE_FIXTURES=1",
            )

    def test_the_layout_holds_every_face_of_every_set(self, fixture_bake: Path) -> None:
        sets = [f"day/2004{month:02d}" for month in FIXTURE_MONTHS]
        for directory in [*sets, "night", "mask"]:
            names = sorted(p.name for p in (fixture_bake / directory).iterdir())
            assert names == sorted(f"{face}.jxl" for face in FACES)

    def test_day_and_night_faces_are_rgb_and_the_mask_is_one_channel(
        self, fixture_bake: Path
    ) -> None:
        shape = (FIXTURE_FACE, FIXTURE_FACE)
        for face in FACES:
            assert decode(fixture_bake / "day" / "200401" / f"{face}.jxl").shape == (
                *shape,
                3,
            )
            assert decode(fixture_bake / "night" / f"{face}.jxl").shape == (*shape, 3)
            assert decode(fixture_bake / "mask" / f"{face}.jxl").shape == shape

    def test_the_mask_is_open_water_at_sea_and_zero_on_land(
        self, fixture_bake: Path
    ) -> None:
        def mask_at(lon: float, lat: float) -> int:
            face, row, col = texel_of(direction_at(lon, lat), FIXTURE_FACE)
            return int(decode(fixture_bake / "mask" / f"{face}.jxl")[row, col])

        assert mask_at(-150, 0) == 255
        assert mask_at(15, 0) == 0
        assert mask_at(0, -85) == 0
        assert mask_at(-100, 76) == 0

    def test_open_water_is_the_fill_in_every_month(self, fixture_bake: Path) -> None:
        face, row, col = texel_of(direction_at(-150, 0), FIXTURE_FACE)
        for month in FIXTURE_MONTHS:
            day = decode(fixture_bake / "day" / f"2004{month:02d}" / f"{face}.jxl")
            assert tuple(day[row, col]) == OCEAN_FILL

    def test_ice_found_in_one_month_stays_unflattened_in_every_month(
        self, fixture_bake: Path
    ) -> None:
        face, row, col = texel_of(direction_at(-100, 76), FIXTURE_FACE)
        for month in FIXTURE_MONTHS:
            day = decode(fixture_bake / "day" / f"2004{month:02d}" / f"{face}.jxl")
            assert tuple(day[row, col]) != OCEAN_FILL


def direction_at(lon: float, lat: float) -> tuple[float, float, float]:
    """Return the world direction of a longitude and latitude in degrees."""
    lo, la = np.radians(lon), np.radians(lat)
    return (
        float(np.cos(la) * np.sin(lo)),
        float(np.sin(la)),
        float(np.cos(la) * np.cos(lo)),
    )
