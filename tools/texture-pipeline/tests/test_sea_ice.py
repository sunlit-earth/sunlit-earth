"""Tests for the sea ice bake.

The real bake needs the 264 OSI-450-a1 monthly files, which are not in the
repository. Point ``TEXTURE_PIPELINE_OSI_DIR`` at a directory holding them
(``texture-pipeline sea-ice --fetch`` fills one) to run the cases that check
the shipped atlas against them; without it those cases skip.
"""

import csv
import os
from pathlib import Path

import numpy as np
import pillow_jxl  # noqa: F401 - registers the JPEG XL plugin with Pillow
import pytest
from conftest import SEA_ICE_FIXTURE_YEARS, synthetic_concentration
from PIL import Image
from typer.testing import CliRunner

from texture_pipeline import sea_ice
from texture_pipeline.main import app
from texture_pipeline.processing import encode_jxl

REPOSITORY = Path(__file__).resolve().parents[3]
SHIPPED = REPOSITORY / "textures" / "sea_ice_2010_2020.jxl"
CORE_FIXTURES = REPOSITORY / "crates" / "sunlit-core" / "tests" / "fixtures"
POINTS = CORE_FIXTURES / "sea_ice_points.csv"
EARTH_FIXTURE = CORE_FIXTURES / "earth" / "sea_ice.jxl"
EARTH_CAP = 64
FIXTURE = Path(__file__).parent / "fixtures" / "sea_ice" / "atlas.jxl"
FIXTURE_CAP = 64
LFS_POINTER = b"version https://git-lfs"
EXTENT_TOLERANCE = 0.03


def decode(path: Path) -> np.ndarray:
    """Decode a JPEG XL file into an array."""
    with Image.open(path) as image:
        return np.asarray(image)


def is_lfs_pointer(path: Path) -> bool:
    """Return whether ``path`` is a Git LFS pointer rather than its object."""
    with path.open("rb") as file:
        return file.read(len(LFS_POINTER)) == LFS_POINTER


def osi_directory() -> Path:
    """Return the directory of the real monthly files, or skip the test."""
    value = os.environ.get("TEXTURE_PIPELINE_OSI_DIR", "").strip()
    if not value:
        pytest.skip("TEXTURE_PIPELINE_OSI_DIR names no directory of OSI-450-a1 files")
    directory = Path(value)
    missing = [
        sea_ice.file_name(*f)
        for f in sea_ice.window_files()
        if not (directory / sea_ice.file_name(*f)).exists()
    ]
    if missing:
        pytest.skip(f"{len(missing)} monthly files missing from {directory}")
    return directory


@pytest.fixture(scope="module")
def fixture_bake(osi_monthly_dir: Path) -> sea_ice.Bake:
    """Bake the synthetic window at the fixture's cap size."""
    return sea_ice.bake(osi_monthly_dir, years=SEA_ICE_FIXTURE_YEARS, size=FIXTURE_CAP)


@pytest.fixture(scope="module")
def real_bake() -> sea_ice.Bake:
    """Bake the real window, when its files are there."""
    return sea_ice.bake(osi_directory())


class TestProjection:
    def test_the_cap_and_its_inverse_round_trip(self) -> None:
        latitude, longitude = np.meshgrid(
            np.linspace(36, 89.5, 40), np.linspace(-179, 179, 60), indexing="ij"
        )
        for sign in (1, -1):
            cap, x, y = sea_ice.latlon_to_cap(sign * latitude, longitude)
            assert np.all(cap == (0 if sign > 0 else 1))
            back_lat, back_lon = sea_ice.cap_to_latlon(x, y, "nh" if sign > 0 else "sh")
            np.testing.assert_allclose(back_lat, sign * latitude, atol=1e-9)
            np.testing.assert_allclose(back_lon, longitude, atol=1e-9)

    def test_rho_max_reaches_35_degrees_on_the_axes(self) -> None:
        latitude, _ = sea_ice.cap_to_latlon(
            np.array(sea_ice.RHO_MAX), np.array(0.0), "nh"
        )
        assert float(latitude) == pytest.approx(35.0)

    def test_the_pinned_points_are_where_the_cap_puts_them(self) -> None:
        with POINTS.open(encoding="utf-8") as file:
            rows = list(csv.DictReader(file))
        assert len(rows) >= 8
        for row in rows:
            latitude = np.array(float(row["latitude"]))
            longitude = np.array(float(row["longitude"]))
            cap, x, y = sea_ice.latlon_to_cap(latitude, longitude)
            assert int(cap) == int(row["cap"])
            u = 0.5 + x / (2 * sea_ice.RHO_MAX)
            v = 0.5 - y / (2 * sea_ice.RHO_MAX)
            assert float(u) == pytest.approx(float(row["u"]), abs=1e-6)
            assert float(v) == pytest.approx(float(row["v"]), abs=1e-6)

    def test_the_pinned_points_reach_their_source_cells_through_proj(self) -> None:
        """Each point is the center of a cell of a real file, its latitude and
        longitude read from that file's ``lat`` and ``lon``: the cap's inverse
        and PROJ's EASE2 put it back on that cell."""
        with POINTS.open(encoding="utf-8") as file:
            rows = list(csv.DictReader(file))
        for row in rows:
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


class TestFieldSteps:
    def test_land_takes_its_nearest_ocean_value_up_to_three_cells(self) -> None:
        land = np.zeros((12, 12), dtype=bool)
        land[:, 4:] = True
        means = np.where(land, np.nan, 50.0)[None].astype(np.float32)
        means[0, :, 3] = 80.0
        extended = sea_ice.extend_into_land(means, land)
        assert not np.isnan(extended).any()
        np.testing.assert_array_equal(extended[0, :, 4:7], 80.0)
        np.testing.assert_array_equal(extended[0, :, 7:], 0.0)
        np.testing.assert_array_equal(extended[0, :, :3], 50.0)

    def test_quantization_rounds_percent_to_bytes_and_zeroes_the_ring(self) -> None:
        cap = np.full((6, 6), 100.0)
        cap[2, 2] = 15.0
        cap[2, 3] = 0.0
        out = sea_ice.quantize(cap)
        assert out.dtype == np.uint8
        assert out[3, 3] == 255
        assert out[2, 2] == round(0.15 * 255)
        assert out[2, 3] == 0
        for ring in (out[0], out[-1], out[:, 0], out[:, -1]):
            np.testing.assert_array_equal(ring, 0)

    def test_the_atlas_holds_the_months_across_and_the_north_above(self) -> None:
        caps = np.zeros((2, 12, 4, 4), dtype=np.uint8)
        for hemisphere in range(2):
            for month in range(12):
                caps[hemisphere, month] = hemisphere * 12 + month
        atlas = sea_ice.assemble(caps)
        assert atlas.shape == (8, 48)
        assert atlas[0, 0] == 0
        assert atlas[0, 4 * 11] == 11
        assert atlas[4, 4 * 2] == 14
        np.testing.assert_array_equal(sea_ice.disassemble(atlas, 4), caps)

    def test_a_file_with_other_land_fails_the_climatology(self) -> None:
        def read(path: Path) -> np.ndarray:
            field = synthetic_concentration("nh", 2015, 1)
            if path.name.endswith("201507.nc"):
                field[0, 0] = np.nan
            return field

        with pytest.raises(ValueError, match="other land cells"):
            sea_ice.climatology(Path("."), "nh", years=[2015], read=read)

    def test_the_climatology_is_the_mean_of_the_years(
        self, osi_monthly_dir: Path
    ) -> None:
        climate = sea_ice.climatology(osi_monthly_dir, "sh", SEA_ICE_FIXTURE_YEARS)
        expected = np.mean(
            [synthetic_concentration("sh", y, 4) for y in SEA_ICE_FIXTURE_YEARS],
            axis=0,
        )
        np.testing.assert_allclose(climate.means[3], expected, atol=0.006)
        np.testing.assert_array_equal(climate.land, np.isnan(expected))


class TestFetch:
    def test_only_the_missing_files_are_fetched_one_by_one(
        self, tmp_path: Path
    ) -> None:
        years = [2015]
        present = sea_ice.file_name("nh", 2015, 1)
        (tmp_path / present).write_bytes(b"kept")
        asked: list[str] = []

        def opener(url: str) -> bytes:
            asked.append(url)
            return url.rsplit("/", 1)[-1].encode()

        count = sea_ice.fetch(tmp_path, years=years, opener=opener)
        assert count == 23
        assert len(asked) == len(set(asked)) == 23
        assert all(url.startswith(sea_ice.THREDDS + "/2015/") for url in asked)
        assert (tmp_path / present).read_bytes() == b"kept"
        assert not list(tmp_path.glob("*.part"))
        manifest = (tmp_path / sea_ice.MANIFEST).read_text(encoding="utf-8")
        manifest = manifest.splitlines()
        assert len(manifest) == 24
        digest, name = manifest[0].split("  ")
        assert digest == sea_ice.sha256(tmp_path / name)

    def test_a_complete_directory_fetches_nothing(self, tmp_path: Path) -> None:
        for f in sea_ice.window_files([2016]):
            (tmp_path / sea_ice.file_name(*f)).write_bytes(b"x")

        def opener(url: str) -> bytes:
            raise AssertionError(url)

        assert sea_ice.fetch(tmp_path, years=[2016], opener=opener) == 0


class TestFixtureBake:
    def test_the_committed_fixture_matches_a_fresh_bake(
        self, fixture_bake: sea_ice.Bake
    ) -> None:
        if os.environ.get("TEXTURE_PIPELINE_UPDATE_FIXTURES"):
            encode_jxl(Image.fromarray(fixture_bake.atlas), FIXTURE, quality=100)
        np.testing.assert_array_equal(
            decode(FIXTURE),
            fixture_bake.atlas,
            err_msg="the sea ice fixture differs from a fresh bake; rerun with "
            "TEXTURE_PIPELINE_UPDATE_FIXTURES=1",
        )

    def test_the_extent_survives_the_bake(self, fixture_bake: sea_ice.Bake) -> None:
        """A cap of the fixture's size has texels of 180 km, so only the months
        whose ice is wide against that are held to the tolerance."""
        for hemisphere, months in fixture_bake.extents.items():
            for month, extent in enumerate(months, start=1):
                assert extent.source > 0, (hemisphere, month)
                if extent.source > 2e6:
                    assert abs(extent.ratio - 1) < 0.08, (hemisphere, month, extent)

    def test_the_ice_follows_the_season_in_each_hemisphere(
        self, fixture_bake: sea_ice.Bake
    ) -> None:
        north = [e.baked for e in fixture_bake.extents["nh"]]
        south = [e.baked for e in fixture_bake.extents["sh"]]
        assert north.index(max(north)) == 2
        assert south.index(max(south)) == 8

    def test_the_land_disk_reads_the_coast_and_not_zero(
        self, fixture_bake: sea_ice.Bake
    ) -> None:
        """Around the south pole the synthetic land is a disk under full ice: its
        rim takes the ice's value, and only its middle, past three cells from
        the coast, is zero."""
        caps = sea_ice.disassemble(fixture_bake.atlas, FIXTURE_CAP)
        september = caps[1, 8]
        middle = FIXTURE_CAP // 2
        assert september[middle, middle] == 0
        rim = round((1500 - 40) / 6371 / (2 * sea_ice.RHO_MAX) * FIXTURE_CAP)
        assert september[middle, middle + rim] == 255

    def test_the_cli_bakes_nothing_while_files_are_missing(
        self, tmp_path: Path
    ) -> None:
        result = CliRunner().invoke(
            app,
            ["sea-ice", "--input", str(tmp_path), "--output", str(tmp_path / "a.jxl")],
        )
        assert result.exit_code == 1
        assert "--fetch" in result.output
        assert not (tmp_path / "a.jxl").exists()


class TestShippedAtlas:
    def test_the_shipped_atlas_is_a_fresh_bake(self, real_bake: sea_ice.Bake) -> None:
        if is_lfs_pointer(SHIPPED):
            pytest.skip("the shipped atlas is a Git LFS pointer in this checkout")
        np.testing.assert_array_equal(decode(SHIPPED), real_bake.atlas)

    def test_every_month_keeps_its_extent(self, real_bake: sea_ice.Bake) -> None:
        for hemisphere, months in real_bake.extents.items():
            for month, extent in enumerate(months, start=1):
                assert abs(extent.ratio - 1) <= EXTENT_TOLERANCE, (
                    f"{hemisphere} {month:02d}: baked {extent.baked / 1e6:.2f} "
                    f"against {extent.source / 1e6:.2f} million km2"
                )

    def test_the_earth_fixture_is_the_shipped_atlas_shrunk(self) -> None:
        if is_lfs_pointer(SHIPPED):
            pytest.skip("the shipped atlas is a Git LFS pointer in this checkout")
        shrunk = sea_ice.shrink(decode(SHIPPED), EARTH_CAP)
        if os.environ.get("TEXTURE_PIPELINE_UPDATE_FIXTURES"):
            encode_jxl(Image.fromarray(shrunk), EARTH_FIXTURE, quality=100)
        np.testing.assert_array_equal(
            decode(EARTH_FIXTURE),
            shrunk,
            err_msg="the earth fixture's sea ice is not the shipped atlas shrunk; "
            "rerun with TEXTURE_PIPELINE_UPDATE_FIXTURES=1",
        )
