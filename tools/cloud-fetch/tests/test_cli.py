"""Tests for the CLI interface."""

from pathlib import Path
from unittest.mock import patch

from typer.testing import CliRunner

from cloud_fetch.main import app

runner = CliRunner()


class TestHelpOutput:
    def test_main_help(self) -> None:
        result = runner.invoke(app, ["--help"])
        assert result.exit_code == 0
        assert "matteason" in result.output
        assert "gmgsi" in result.output

    def test_matteason_help(self) -> None:
        result = runner.invoke(app, ["matteason", "--help"])
        assert result.exit_code == 0
        for word in ["output-dir", "floor", "ceiling", "gamma", "blur", "raw"]:
            assert word in result.output

    def test_gmgsi_help(self) -> None:
        result = runner.invoke(app, ["gmgsi", "--help"])
        assert result.exit_code == 0
        for word in [
            "output-dir",
            "date",
            "hour",
            "floor",
            "ceiling",
            "gamma",
            "blur",
            "raw",
        ]:
            assert word in result.output


class TestMatteason:
    def test_default_values(self, tmp_path: Path) -> None:
        with patch("cloud_fetch.main.fetch_matteason") as mock:
            result = runner.invoke(app, ["matteason", "--output-dir", str(tmp_path)])
            assert result.exit_code == 0
            mock.assert_called_once()
            kwargs = mock.call_args.kwargs
            assert kwargs["floor"] == 50
            assert kwargs["ceiling"] == 255
            assert kwargs["gamma"] == 0.3
            assert kwargs["blur"] == 0.0
            assert kwargs["raw"] is False

    def test_custom_values(self, tmp_path: Path) -> None:
        with patch("cloud_fetch.main.fetch_matteason") as mock:
            result = runner.invoke(
                app,
                [
                    "matteason",
                    "--output-dir",
                    str(tmp_path),
                    "--floor",
                    "30",
                    "--ceiling",
                    "200",
                    "--gamma",
                    "0.5",
                    "--blur",
                    "2.0",
                    "--raw",
                ],
            )
            assert result.exit_code == 0
            kwargs = mock.call_args.kwargs
            assert kwargs["floor"] == 30
            assert kwargs["ceiling"] == 200
            assert kwargs["gamma"] == 0.5
            assert kwargs["blur"] == 2.0
            assert kwargs["raw"] is True

    def test_pipeline_error_exits_nonzero(self, tmp_path: Path) -> None:
        with patch(
            "cloud_fetch.main.fetch_matteason",
            side_effect=RuntimeError("download failed"),
        ):
            result = runner.invoke(app, ["matteason", "--output-dir", str(tmp_path)])
            assert result.exit_code != 0


class TestGmgsi:
    def test_default_values(self, tmp_path: Path) -> None:
        with patch("cloud_fetch.main.fetch_gmgsi") as mock:
            result = runner.invoke(app, ["gmgsi", "--output-dir", str(tmp_path)])
            assert result.exit_code == 0
            mock.assert_called_once()
            kwargs = mock.call_args.kwargs
            assert kwargs["date"] is None
            assert kwargs["hour"] is None
            assert kwargs["floor"] == 60
            assert kwargs["ceiling"] == 215
            assert kwargs["gamma"] == 0.7
            assert kwargs["blur"] == 3.0
            assert kwargs["raw"] is False

    def test_custom_values(self, tmp_path: Path) -> None:
        with patch("cloud_fetch.main.fetch_gmgsi") as mock:
            result = runner.invoke(
                app,
                [
                    "gmgsi",
                    "--output-dir",
                    str(tmp_path),
                    "--date",
                    "20260319",
                    "--hour",
                    "06",
                    "--floor",
                    "40",
                    "--ceiling",
                    "180",
                    "--gamma",
                    "0.5",
                    "--blur",
                    "1.5",
                    "--raw",
                ],
            )
            assert result.exit_code == 0
            kwargs = mock.call_args.kwargs
            assert kwargs["date"] == "20260319"
            assert kwargs["hour"] == "06"
            assert kwargs["floor"] == 40
            assert kwargs["ceiling"] == 180
            assert kwargs["gamma"] == 0.5
            assert kwargs["blur"] == 1.5
            assert kwargs["raw"] is True

    def test_pipeline_error_exits_nonzero(self, tmp_path: Path) -> None:
        with patch(
            "cloud_fetch.main.fetch_gmgsi",
            side_effect=RuntimeError("S3 error"),
        ):
            result = runner.invoke(app, ["gmgsi", "--output-dir", str(tmp_path)])
            assert result.exit_code != 0


class TestValidation:
    def test_floor_too_low(self) -> None:
        result = runner.invoke(app, ["matteason", "--floor", "-1"])
        assert result.exit_code != 0

    def test_floor_too_high(self) -> None:
        result = runner.invoke(app, ["matteason", "--floor", "256"])
        assert result.exit_code != 0

    def test_ceiling_too_low(self) -> None:
        result = runner.invoke(app, ["matteason", "--ceiling", "-1"])
        assert result.exit_code != 0

    def test_ceiling_too_high(self) -> None:
        result = runner.invoke(app, ["matteason", "--ceiling", "256"])
        assert result.exit_code != 0

    def test_gamma_zero(self) -> None:
        result = runner.invoke(app, ["matteason", "--gamma", "0"])
        assert result.exit_code != 0

    def test_gamma_negative(self) -> None:
        result = runner.invoke(app, ["matteason", "--gamma", "-0.5"])
        assert result.exit_code != 0

    def test_blur_negative(self) -> None:
        result = runner.invoke(app, ["matteason", "--blur", "-1"])
        assert result.exit_code != 0

    def test_gmgsi_floor_too_high(self) -> None:
        result = runner.invoke(app, ["gmgsi", "--floor", "300"])
        assert result.exit_code != 0

    def test_gmgsi_gamma_zero(self) -> None:
        result = runner.invoke(app, ["gmgsi", "--gamma", "0"])
        assert result.exit_code != 0

    def test_floor_equals_ceiling_errors(self, tmp_path: Path) -> None:
        """floor == ceiling triggers a ValueError from apply_levels."""
        with patch("cloud_fetch.main.fetch_matteason", side_effect=ValueError("floor")):
            result = runner.invoke(
                app,
                [
                    "matteason",
                    "--output-dir",
                    str(tmp_path),
                    "--floor",
                    "128",
                    "--ceiling",
                    "128",
                ],
            )
            assert result.exit_code != 0
