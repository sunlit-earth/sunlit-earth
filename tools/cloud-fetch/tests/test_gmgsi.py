"""Tests for the GMGSI cloud fetcher."""

import contextlib
from pathlib import Path
from unittest.mock import MagicMock, patch

import numpy as np
from numpy.testing import assert_allclose

from cloud_fetch.gmgsi import find_latest_key, remap_to_equirectangular, upscale


class TestRemapToEquirectangular:
    def _make_regular_grid(
        self, nlat: int = 50, nlon: int = 100
    ) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
        """Create a synthetic regular grid for testing.

        Covers roughly -60S to 60N, -180 to 180.
        """
        lats = np.linspace(60, -60, nlat)
        lons = np.linspace(-179, 179, nlon)
        lon_2d, lat_2d = np.meshgrid(lons, lats)
        data = np.full((nlat, nlon), 0.5, dtype=np.float32)
        return data, lat_2d, lon_2d

    def test_output_shape(self) -> None:
        """Output grid has the expected dimensions."""
        data, lat_2d, lon_2d = self._make_regular_grid()
        result, counts = remap_to_equirectangular(data, lat_2d, lon_2d)
        assert result.ndim == 2
        assert counts.ndim == 2
        assert result.shape == counts.shape

    def test_uniform_data_produces_uniform_output(self) -> None:
        """Uniform input produces uniform output in the covered region."""
        data, lat_2d, lon_2d = self._make_regular_grid()
        result, counts = remap_to_equirectangular(data, lat_2d, lon_2d)
        covered = counts > 0
        assert covered.any()
        covered_values = result[covered]
        assert_allclose(covered_values, 0.5, atol=0.01)

    def test_uncovered_regions_are_zero(self) -> None:
        """Polar regions outside data extent are zero."""
        data, lat_2d, lon_2d = self._make_regular_grid()
        result, counts = remap_to_equirectangular(data, lat_2d, lon_2d)
        uncovered = counts == 0
        if uncovered.any():
            assert_allclose(result[uncovered], 0.0, atol=1e-6)

    def test_known_coordinate_binning(self) -> None:
        """A bright pixel at the equator/prime meridian lands near grid center."""
        # Small grid centered on equator, prime meridian with known spacing
        nlat, nlon = 5, 5
        lats = np.linspace(2, -2, nlat)
        lons = np.linspace(-2, 2, nlon)
        lon_2d, lat_2d = np.meshgrid(lons, lats)
        data = np.zeros((nlat, nlon), dtype=np.float32)
        # Place a bright pixel at exactly (0, 0) -- the center
        data[2, 2] = 0.8

        result, _counts = remap_to_equirectangular(data, lat_2d, lon_2d)

        # The bright pixel should land near the center of the output grid
        # and have a nonzero value (it may be averaged with other pixels in
        # the same bin, so we just check it's positive)
        nlat_out, nlon_out = result.shape
        mid_row = nlat_out // 2
        mid_col = nlon_out // 2
        neighborhood = result[
            max(0, mid_row - 2) : mid_row + 3,
            max(0, mid_col - 2) : mid_col + 3,
        ]
        assert np.any(neighborhood > 0.0)

    def test_gap_fill_reduces_gaps(self) -> None:
        """Gap-fill reduces the number of zero-count pixels near data."""
        # Create a grid with some intentional gaps
        nlat, nlon = 30, 60
        lats = np.linspace(30, -30, nlat)
        lons = np.linspace(-90, 90, nlon)
        lon_2d, lat_2d = np.meshgrid(lons, lats)
        data = np.full((nlat, nlon), 0.5, dtype=np.float32)
        # Remove every other row to create gaps
        data[1::2, :] = np.nan

        _result, counts = remap_to_equirectangular(data, lat_2d, lon_2d)
        # After gap-fill, many of the gaps should be filled
        total_filled = np.count_nonzero(counts > 0)
        assert total_filled > nlat * nlon // 2

    def test_nan_values_filtered(self) -> None:
        """NaN values in data are excluded from binning."""
        data = np.array([[np.nan, 0.5], [0.5, np.nan]], dtype=np.float32)
        lat_2d = np.array([[10.0, 10.0], [9.0, 9.0]], dtype=np.float64)
        lon_2d = np.array([[0.0, 1.0], [0.0, 1.0]], dtype=np.float64)

        result, counts = remap_to_equirectangular(data, lat_2d, lon_2d)
        # Should not crash and should produce some valid output
        assert result.shape == counts.shape
        assert np.all(np.isfinite(result))

    def test_returns_float32(self) -> None:
        """Result array is float32."""
        data, lat_2d, lon_2d = self._make_regular_grid()
        result, _counts = remap_to_equirectangular(data, lat_2d, lon_2d)
        assert result.dtype == np.float32


class TestUpscale:
    def test_output_dimensions(self) -> None:
        """Upscaled image has the requested dimensions."""
        data = np.random.default_rng(42).random((50, 100)).astype(np.float32)
        result = upscale(data, 200, 100)
        assert result.shape == (100, 200)

    def test_preserves_value_range(self) -> None:
        """Upscaled values stay within [0, 1]."""
        data = np.random.default_rng(42).random((50, 100)).astype(np.float32)
        result = upscale(data, 200, 100)
        assert np.all(result >= 0.0)
        assert np.all(result <= 1.0)

    def test_uniform_image_stays_uniform(self) -> None:
        """A uniform image stays uniform after upscale."""
        data = np.full((20, 40), 0.5, dtype=np.float32)
        result = upscale(data, 80, 40)
        # uint8 quantization: 0.5 * 255 = 127.5 -> 127, 127/255 ~= 0.498
        assert_allclose(result, result.mean(), atol=0.01)

    def test_returns_float32(self) -> None:
        """Result is float32."""
        data = np.zeros((10, 20), dtype=np.float32)
        result = upscale(data, 40, 20)
        assert result.dtype == np.float32


class TestFindLatestKey:
    def _mock_s3(self, responses: dict[str, list[dict]]) -> MagicMock:
        """Create a mock S3 client that returns predefined responses.

        :param responses: Mapping from S3 prefix to list of S3 Contents objects.
        """
        mock = MagicMock()

        def list_objects_v2(Bucket, Prefix, MaxKeys=5):
            contents = responses.get(Prefix, [])
            return {"Contents": contents} if contents else {}

        mock.list_objects_v2 = MagicMock(side_effect=list_objects_v2)
        return mock

    def test_finds_with_explicit_date_and_hour(self) -> None:
        """Returns the key when both date and hour are given."""
        prefix = "GMGSI_LW/2026/03/19/06/"
        s3 = self._mock_s3({prefix: [{"Key": "GMGSI_LW/2026/03/19/06/data.nc"}]})
        key, date, hour = find_latest_key(s3, "20260319", "06")
        assert key == "GMGSI_LW/2026/03/19/06/data.nc"
        assert date == "20260319"
        assert hour == "06"

    def test_finds_latest_hour_for_date(self) -> None:
        """Searches hours in descending order when only date is given."""
        prefix_12 = "GMGSI_LW/2026/03/19/12/"
        s3 = self._mock_s3({prefix_12: [{"Key": "GMGSI_LW/2026/03/19/12/data.nc"}]})
        _key, date, hour = find_latest_key(s3, "20260319", None)
        assert hour == "12"
        assert date == "20260319"

    def test_raises_when_nothing_found(self) -> None:
        """Raises RuntimeError when no data is available."""
        s3 = self._mock_s3({})  # empty responses
        try:
            find_latest_key(s3, "20260319", "06")
            raise AssertionError("Expected RuntimeError")
        except RuntimeError as e:
            assert "No GMGSI_LW data found" in str(e)

    def test_skips_non_nc_files(self) -> None:
        """Non-.nc files are ignored."""
        prefix = "GMGSI_LW/2026/03/19/06/"
        s3 = self._mock_s3({prefix: [{"Key": "GMGSI_LW/2026/03/19/06/readme.txt"}]})
        try:
            find_latest_key(s3, "20260319", "06")
            raise AssertionError("Expected RuntimeError")
        except RuntimeError:
            pass

    def test_auto_date_searches_backwards(self) -> None:
        """With no date or hour, searches backwards from now."""
        # We just verify it calls list_objects_v2 multiple times
        s3 = self._mock_s3({})
        with contextlib.suppress(RuntimeError):
            find_latest_key(s3, None, None)
        # Should have tried all 25 prefixes before giving up
        assert s3.list_objects_v2.call_count == 25


class TestFetchGmgsiRaw:
    """Test the raw flag in the GMGSI pipeline."""

    def _make_fake_netcdf(self, path: Path) -> None:
        """Create a minimal fake NetCDF file for testing."""
        from netCDF4 import Dataset

        ds = Dataset(str(path), "w", format="NETCDF4")
        nlat, nlon = 20, 40
        ds.createDimension("y", nlat)
        ds.createDimension("x", nlon)

        var_data = ds.createVariable("data", "f4", ("y", "x"))
        var_data[:] = np.full((nlat, nlon), 128.0, dtype=np.float32)

        var_lat = ds.createVariable("lat", "f8", ("y", "x"))
        lats = np.linspace(30, -30, nlat)
        lons = np.linspace(-60, 60, nlon)
        lon_2d, lat_2d = np.meshgrid(lons, lats)
        var_lat[:] = lat_2d
        var_lon = ds.createVariable("lon", "f8", ("y", "x"))
        var_lon[:] = lon_2d

        ds.close()

    def test_raw_flag_saves_extra_file(self, tmp_path: Path) -> None:
        """raw=True saves an additional raw PNG before levels processing."""
        from cloud_fetch.gmgsi import fetch_gmgsi

        nc_path = tmp_path / "fake.nc"
        self._make_fake_netcdf(nc_path)

        mock_s3 = MagicMock()
        mock_s3.list_objects_v2.return_value = {
            "Contents": [{"Key": "GMGSI_LW/2026/03/19/06/data.nc"}]
        }
        mock_s3.head_object.return_value = {"ContentLength": 1000}

        def fake_download(bucket, key, dest):
            import shutil

            shutil.copy(str(nc_path), dest)

        mock_s3.download_file.side_effect = fake_download

        with patch("cloud_fetch.gmgsi.create_s3_client", return_value=mock_s3):
            result = fetch_gmgsi(
                tmp_path / "out",
                date="20260319",
                hour="06",
                raw=True,
            )

        assert result.exists()
        assert (tmp_path / "out" / "clouds_lw_raw.png").exists()
