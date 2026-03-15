"""Tests for the CLI interface."""

from pathlib import Path
from unittest.mock import patch

from typer.testing import CliRunner

from texture_pipeline.main import app

runner = CliRunner()


class TestHelpOutput:
    def test_help_output(self) -> None:
        result = runner.invoke(app, ["convert", "--help"])
        assert result.exit_code == 0
        for word in ["input", "output", "quality", "effort", "width"]:
            assert word in result.output.lower()


class TestDefaultValues:
    def test_default_values(self, tmp_path: Path) -> None:
        input_dir = tmp_path / "in"
        input_dir.mkdir()
        output_dir = tmp_path / "out"
        # Patch the pipeline to capture the arguments without doing real work
        with patch("texture_pipeline.main.run_pipeline") as mock_pipeline:
            result = runner.invoke(
                app,
                ["convert", "--input", str(input_dir), "--output", str(output_dir)],
            )
            assert result.exit_code == 0
            mock_pipeline.assert_called_once()
            call_kwargs = mock_pipeline.call_args
            # Check defaults: width=[8192], quality=85, effort=7
            assert call_kwargs.kwargs["widths"] == [8192]
            assert call_kwargs.kwargs["quality"] == 85
            assert call_kwargs.kwargs["effort"] == 7
            assert call_kwargs.kwargs["do_sharpen"] is False


class TestMultipleWidths:
    def test_multiple_widths(self, tmp_path: Path) -> None:
        input_dir = tmp_path / "in"
        input_dir.mkdir()
        output_dir = tmp_path / "out"
        with patch("texture_pipeline.main.run_pipeline") as mock_pipeline:
            result = runner.invoke(
                app,
                [
                    "convert",
                    "--input",
                    str(input_dir),
                    "--output",
                    str(output_dir),
                    "--width",
                    "4096",
                    "--width",
                    "2048",
                ],
            )
            assert result.exit_code == 0
            call_kwargs = mock_pipeline.call_args
            assert call_kwargs.kwargs["widths"] == [4096, 2048]


class TestValidation:
    def test_invalid_quality_too_low(self, tmp_path: Path) -> None:
        input_dir = tmp_path / "in"
        input_dir.mkdir()
        result = runner.invoke(
            app,
            [
                "convert",
                "--input",
                str(input_dir),
                "--output",
                str(tmp_path / "out"),
                "--quality",
                "0",
            ],
        )
        assert result.exit_code != 0

    def test_invalid_quality_too_high(self, tmp_path: Path) -> None:
        input_dir = tmp_path / "in"
        input_dir.mkdir()
        result = runner.invoke(
            app,
            [
                "convert",
                "--input",
                str(input_dir),
                "--output",
                str(tmp_path / "out"),
                "--quality",
                "101",
            ],
        )
        assert result.exit_code != 0

    def test_invalid_effort_too_low(self, tmp_path: Path) -> None:
        input_dir = tmp_path / "in"
        input_dir.mkdir()
        result = runner.invoke(
            app,
            [
                "convert",
                "--input",
                str(input_dir),
                "--output",
                str(tmp_path / "out"),
                "--effort",
                "0",
            ],
        )
        assert result.exit_code != 0

    def test_invalid_effort_too_high(self, tmp_path: Path) -> None:
        input_dir = tmp_path / "in"
        input_dir.mkdir()
        result = runner.invoke(
            app,
            [
                "convert",
                "--input",
                str(input_dir),
                "--output",
                str(tmp_path / "out"),
                "--effort",
                "10",
            ],
        )
        assert result.exit_code != 0

    def test_invalid_width_odd(self, tmp_path: Path) -> None:
        input_dir = tmp_path / "in"
        input_dir.mkdir()
        result = runner.invoke(
            app,
            [
                "convert",
                "--input",
                str(input_dir),
                "--output",
                str(tmp_path / "out"),
                "--width",
                "4097",
            ],
        )
        assert result.exit_code != 0

    def test_nonexistent_input_dir(self, tmp_path: Path) -> None:
        result = runner.invoke(
            app,
            [
                "convert",
                "--input",
                str(tmp_path / "nonexistent"),
                "--output",
                str(tmp_path / "out"),
            ],
        )
        assert result.exit_code != 0


class TestSharpenFlag:
    def test_sharpening_flag_default_off(self, tmp_path: Path) -> None:
        input_dir = tmp_path / "in"
        input_dir.mkdir()
        output_dir = tmp_path / "out"
        with patch("texture_pipeline.main.run_pipeline") as mock_pipeline:
            result = runner.invoke(
                app,
                ["convert", "--input", str(input_dir), "--output", str(output_dir)],
            )
            assert result.exit_code == 0
            call_kwargs = mock_pipeline.call_args
            assert call_kwargs.kwargs["do_sharpen"] is False

    def test_sharpening_flag_enabled(self, tmp_path: Path) -> None:
        input_dir = tmp_path / "in"
        input_dir.mkdir()
        output_dir = tmp_path / "out"
        with patch("texture_pipeline.main.run_pipeline") as mock_pipeline:
            result = runner.invoke(
                app,
                [
                    "convert",
                    "--input",
                    str(input_dir),
                    "--output",
                    str(output_dir),
                    "--sharpen",
                ],
            )
            assert result.exit_code == 0
            call_kwargs = mock_pipeline.call_args
            assert call_kwargs.kwargs["do_sharpen"] is True
