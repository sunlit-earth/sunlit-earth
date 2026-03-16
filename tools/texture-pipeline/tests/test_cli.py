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


def _base_args(tmp_path: Path) -> tuple[Path, list[str]]:
    """Create input dir and return (input_dir, base CLI args)."""
    input_dir = tmp_path / "in"
    input_dir.mkdir()
    output_dir = tmp_path / "out"
    args = ["convert", "--input", str(input_dir), "--output", str(output_dir)]
    return input_dir, args


class TestOceanMaskDefaults:
    def test_ocean_mask_default_none(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        with patch("texture_pipeline.main.run_pipeline") as mock:
            result = runner.invoke(app, args)
            assert result.exit_code == 0
            assert mock.call_args.kwargs["ocean_shapefile"] is None

    def test_ocean_color_default(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        with patch("texture_pipeline.main.run_pipeline") as mock:
            result = runner.invoke(app, args)
            assert result.exit_code == 0
            assert mock.call_args.kwargs["ocean_color"] == (10, 40, 80)

    def test_ocean_supersample_default(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        with patch("texture_pipeline.main.run_pipeline") as mock:
            result = runner.invoke(app, args)
            assert result.exit_code == 0
            assert mock.call_args.kwargs["ocean_supersample"] == 2

    def test_ocean_buffer_default(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        with patch("texture_pipeline.main.run_pipeline") as mock:
            result = runner.invoke(app, args)
            assert result.exit_code == 0
            assert mock.call_args.kwargs["ocean_buffer"] == 0


class TestOceanMaskCustomValues:
    def test_ocean_mask_passes_path(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        shp = tmp_path / "ocean.shp"
        shp.touch()
        args += ["--ocean-mask", str(shp)]
        with patch("texture_pipeline.main.run_pipeline") as mock:
            result = runner.invoke(app, args)
            assert result.exit_code == 0
            assert mock.call_args.kwargs["ocean_shapefile"] == shp

    def test_ocean_color_custom(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        args += ["--ocean-color", "0,100,200"]
        with patch("texture_pipeline.main.run_pipeline") as mock:
            result = runner.invoke(app, args)
            assert result.exit_code == 0
            assert mock.call_args.kwargs["ocean_color"] == (0, 100, 200)

    def test_ocean_supersample_custom(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        args += ["--ocean-supersample", "4"]
        with patch("texture_pipeline.main.run_pipeline") as mock:
            result = runner.invoke(app, args)
            assert result.exit_code == 0
            assert mock.call_args.kwargs["ocean_supersample"] == 4

    def test_ocean_buffer_custom(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        args += ["--ocean-buffer", "3"]
        with patch("texture_pipeline.main.run_pipeline") as mock:
            result = runner.invoke(app, args)
            assert result.exit_code == 0
            assert mock.call_args.kwargs["ocean_buffer"] == 3


class TestOceanMaskValidation:
    def test_ocean_color_invalid_format(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        args += ["--ocean-color", "not,a,color"]
        result = runner.invoke(app, args)
        assert result.exit_code != 0

    def test_ocean_color_out_of_range(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        args += ["--ocean-color", "256,0,0"]
        result = runner.invoke(app, args)
        assert result.exit_code != 0

    def test_ocean_supersample_too_low(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        args += ["--ocean-supersample", "0"]
        result = runner.invoke(app, args)
        assert result.exit_code != 0

    def test_ocean_buffer_negative(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        args += ["--ocean-buffer", "-1"]
        result = runner.invoke(app, args)
        assert result.exit_code != 0


class TestOceanFlagsInHelp:
    def test_ocean_flags_in_help(self) -> None:
        result = runner.invoke(app, ["convert", "--help"])
        assert result.exit_code == 0
        for flag in ["ocean-mask", "ocean-color", "ocean-supersam", "ocean-buffer"]:
            assert flag in result.output, f"Expected '{flag}' in help output"


class TestIcePreservationDefaults:
    def test_preserve_ice_default_true(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        with patch("texture_pipeline.main.run_pipeline") as mock:
            result = runner.invoke(app, args)
            assert result.exit_code == 0
            assert mock.call_args.kwargs["ocean_preserve_ice"] is True

    def test_ice_luminance_default(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        with patch("texture_pipeline.main.run_pipeline") as mock:
            result = runner.invoke(app, args)
            assert result.exit_code == 0
            assert mock.call_args.kwargs["ocean_ice_luminance"] == 200

    def test_ice_latitude_default(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        with patch("texture_pipeline.main.run_pipeline") as mock:
            result = runner.invoke(app, args)
            assert result.exit_code == 0
            assert mock.call_args.kwargs["ocean_ice_latitude"] == 60.0


class TestIcePreservationCustomValues:
    def test_no_preserve_ice_flag(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        args += ["--no-ocean-preserve-ice"]
        with patch("texture_pipeline.main.run_pipeline") as mock:
            result = runner.invoke(app, args)
            assert result.exit_code == 0
            assert mock.call_args.kwargs["ocean_preserve_ice"] is False

    def test_ice_luminance_custom(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        args += ["--ocean-ice-luminance", "180"]
        with patch("texture_pipeline.main.run_pipeline") as mock:
            result = runner.invoke(app, args)
            assert result.exit_code == 0
            assert mock.call_args.kwargs["ocean_ice_luminance"] == 180

    def test_ice_latitude_custom(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        args += ["--ocean-ice-latitude", "55"]
        with patch("texture_pipeline.main.run_pipeline") as mock:
            result = runner.invoke(app, args)
            assert result.exit_code == 0
            assert mock.call_args.kwargs["ocean_ice_latitude"] == 55.0


class TestIcePreservationValidation:
    def test_ice_latitude_out_of_range(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        args += ["--ocean-ice-latitude", "100"]
        result = runner.invoke(app, args)
        assert result.exit_code != 0

    def test_ice_luminance_out_of_range(self, tmp_path: Path) -> None:
        _, args = _base_args(tmp_path)
        args += ["--ocean-ice-luminance", "300"]
        result = runner.invoke(app, args)
        assert result.exit_code != 0


class TestIceFlagsInHelp:
    def test_ice_flags_in_help(self) -> None:
        result = runner.invoke(app, ["convert", "--help"])
        assert result.exit_code == 0
        for flag in [
            "ocean-preserve-i",
            "ocean-ice-lumi",
            "ocean-ice-lati",
        ]:
            assert flag in result.output, f"Expected '{flag}' in help output"
