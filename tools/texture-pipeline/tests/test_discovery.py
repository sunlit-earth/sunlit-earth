"""Tests for the file discovery and output path module."""

from pathlib import Path

import pytest

from texture_pipeline.discovery import (
    compute_output_path,
    discover_images,
    discover_months,
)


class TestDiscoverImages:
    def test_discover_jpeg_files(self, tmp_path: Path) -> None:
        (tmp_path / "a.jpg").write_bytes(b"fake")
        (tmp_path / "b.jpeg").write_bytes(b"fake")
        (tmp_path / "c.txt").write_bytes(b"fake")
        result = discover_images(tmp_path)
        names = {p.name for p in result}
        assert names == {"a.jpg", "b.jpeg"}

    def test_discover_png_files(self, tmp_path: Path) -> None:
        (tmp_path / "a.png").write_bytes(b"fake")
        result = discover_images(tmp_path)
        assert len(result) == 1
        assert result[0].name == "a.png"

    def test_discover_tiff_files(self, tmp_path: Path) -> None:
        (tmp_path / "a.tiff").write_bytes(b"fake")
        (tmp_path / "b.tif").write_bytes(b"fake")
        result = discover_images(tmp_path)
        names = {p.name for p in result}
        assert names == {"a.tiff", "b.tif"}

    def test_discover_case_insensitive(self, tmp_path: Path) -> None:
        (tmp_path / "A.JPG").write_bytes(b"fake")
        (tmp_path / "b.Png").write_bytes(b"fake")
        result = discover_images(tmp_path)
        assert len(result) == 2

    def test_discover_recursive(self, tmp_path: Path) -> None:
        sub = tmp_path / "subdir"
        sub.mkdir()
        (sub / "deep.jpg").write_bytes(b"fake")
        (tmp_path / "top.png").write_bytes(b"fake")
        result = discover_images(tmp_path)
        assert len(result) == 2

    def test_discover_empty_directory(self, tmp_path: Path) -> None:
        result = discover_images(tmp_path)
        assert result == []


class TestComputeOutputPath:
    def test_output_path_preserves_structure(self, tmp_path: Path) -> None:
        input_root = tmp_path / "src"
        source = input_root / "land" / "earth.jpg"
        output_root = tmp_path / "out"
        result = compute_output_path(source, input_root, output_root, width=4096)
        expected = output_root / "4096" / "land" / "earth.jxl"
        assert result == expected

    def test_output_path_replaces_extension(self, tmp_path: Path) -> None:
        input_root = tmp_path / "in"
        output_root = tmp_path / "out"

        for ext in [".jpg", ".png", ".tiff"]:
            source = input_root / f"file{ext}"
            result = compute_output_path(source, input_root, output_root, width=1024)
            assert result.suffix == ".jxl"

    def test_output_path_multiple_widths(self, tmp_path: Path) -> None:
        input_root = tmp_path / "in"
        source = input_root / "earth.jpg"
        output_root = tmp_path / "out"

        for width in [4096, 2048, 1024]:
            result = compute_output_path(source, input_root, output_root, width=width)
            assert result == output_root / str(width) / "earth.jxl"


class TestDiscoverMonths:
    def test_months_are_keyed_by_their_stamp_in_order(self, tmp_path: Path) -> None:
        (tmp_path / "world.topo.200412.3x21600x10800.jpg").write_bytes(b"fake")
        (tmp_path / "world.topo.200401.3x21600x10800.jpg").write_bytes(b"fake")
        (tmp_path / "SHA256SUMS").write_bytes(b"fake")
        result = discover_months(tmp_path)
        assert list(result) == ["200401", "200412"]
        assert result["200412"].name == "world.topo.200412.3x21600x10800.jpg"

    def test_an_image_without_a_stamp_is_refused(self, tmp_path: Path) -> None:
        (tmp_path / "earth.jpg").write_bytes(b"fake")
        with pytest.raises(ValueError, match="no month stamp"):
            discover_months(tmp_path)

    def test_a_stamp_that_names_no_month_is_refused(self, tmp_path: Path) -> None:
        (tmp_path / "world.topo.200413.jpg").write_bytes(b"fake")
        with pytest.raises(ValueError, match="13 is not a month"):
            discover_months(tmp_path)

    def test_two_images_of_one_month_are_refused(self, tmp_path: Path) -> None:
        (tmp_path / "world.topo.200405.jpg").write_bytes(b"fake")
        (tmp_path / "world.topo.bathy.200405.png").write_bytes(b"fake")
        with pytest.raises(ValueError, match="both carry 200405"):
            discover_months(tmp_path)

    def test_subdirectories_are_not_searched(self, tmp_path: Path) -> None:
        (tmp_path / "old").mkdir()
        (tmp_path / "old" / "world.topo.200405.jpg").write_bytes(b"fake")
        assert discover_months(tmp_path) == {}
