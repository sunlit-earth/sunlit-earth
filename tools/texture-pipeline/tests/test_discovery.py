"""Tests for the file discovery and output path module."""

from pathlib import Path

from texture_pipeline.discovery import compute_output_path, discover_images


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
