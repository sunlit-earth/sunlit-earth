"""File discovery and output path construction for the texture pipeline."""

from pathlib import Path

SUPPORTED_EXTENSIONS: set[str] = {".jpg", ".jpeg", ".png", ".tif", ".tiff"}


def discover_images(input_dir: Path) -> list[Path]:
    """Recursively find all supported image files under *input_dir*.

    Matching is case-insensitive on the file extension.

    :param input_dir: Root directory to search.
    :returns: Sorted list of paths to image files.
    """
    results: list[Path] = []
    for path in input_dir.rglob("*"):
        if path.is_file() and path.suffix.lower() in SUPPORTED_EXTENSIONS:
            results.append(path)
    return sorted(results)


def compute_output_path(
    source_path: Path,
    input_root: Path,
    output_root: Path,
    width: int,
) -> Path:
    """Compute the output path for a processed image.

    The output mirrors the input directory structure under a width-specific
    subdirectory, with the extension changed to ``.jxl``.

    :param source_path: Path to the source image file.
    :param input_root: Root of the input directory tree.
    :param output_root: Root of the output directory tree.
    :param width: Target width (used as subdirectory name).
    :returns: Output path like ``output_root / str(width) / relative / name.jxl``.
    """
    relative = source_path.relative_to(input_root)
    return output_root / str(width) / relative.with_suffix(".jxl")
