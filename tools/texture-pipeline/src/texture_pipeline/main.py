"""CLI interface for the texture pipeline."""

from pathlib import Path
from typing import Annotated

import typer
from PIL import Image
from rich.progress import (
    BarColumn,
    Progress,
    SpinnerColumn,
    TaskProgressColumn,
    TextColumn,
)

from texture_pipeline.discovery import compute_output_path, discover_images
from texture_pipeline.processing import downscale, encode_jxl, sharpen

# Disable Pillow's decompression bomb limit. NASA Blue Marble textures
# (e.g. 21600x10800 = 233M pixels) exceed the default ~178M pixel threshold.
# This is safe because this tool only processes known-safe local files.
Image.MAX_IMAGE_PIXELS = None

app = typer.Typer(
    name="texture-pipeline",
    help="Convert NASA Blue Marble textures to JPEG XL at multiple resolutions.",
    invoke_without_command=True,
)


@app.callback()
def main() -> None:
    """Convert NASA Blue Marble textures to JPEG XL at multiple resolutions."""


def run_pipeline(
    *,
    input_dir: Path,
    output_dir: Path,
    widths: list[int],
    quality: int,
    effort: int,
    do_sharpen: bool,
) -> None:
    """Execute the texture conversion pipeline.

    :param input_dir: Directory containing source textures.
    :param output_dir: Root directory for converted output.
    :param widths: List of target widths in pixels.
    :param quality: JPEG XL encoding quality (1--100).
    :param effort: JPEG XL encoding effort (1--9).
    :param do_sharpen: Whether to apply UnsharpMask after downscaling.
    """
    images = discover_images(input_dir)

    if not images:
        typer.echo("No supported image files found in input directory.")
        return

    total_tasks = len(images) * len(widths)
    total_input_size = 0
    total_output_size = 0
    files_processed = 0

    with Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        BarColumn(),
        TaskProgressColumn(),
        TextColumn("{task.fields[current_file]}"),
    ) as progress:
        task = progress.add_task(
            "Processing textures...", total=total_tasks, current_file=""
        )

        for source_path in images:
            total_input_size += source_path.stat().st_size
            img = Image.open(source_path)

            # Ensure RGB mode for consistent processing
            if img.mode != "RGB":
                img = img.convert("RGB")

            for width in widths:
                progress.update(task, current_file=f"{source_path.name} → {width}px")

                output_path = compute_output_path(
                    source_path, input_dir, output_dir, width
                )

                scaled = downscale(img, target_width=width)

                if do_sharpen:
                    scaled = sharpen(scaled)

                encode_jxl(scaled, output_path, quality=quality, effort=effort)

                total_output_size += output_path.stat().st_size
                files_processed += 1
                progress.advance(task)

    typer.echo(f"\nProcessed {files_processed} file(s).")
    typer.echo(f"Total input size:  {total_input_size / 1024 / 1024:.2f} MB")
    typer.echo(f"Total output size: {total_output_size / 1024 / 1024:.2f} MB")
    if total_input_size > 0:
        ratio = total_output_size / total_input_size
        typer.echo(f"Compression ratio: {ratio:.2f}x")


def _validate_quality(value: int) -> int:
    if value < 1 or value > 100:
        raise typer.BadParameter("Quality must be between 1 and 100.")
    return value


def _validate_effort(value: int) -> int:
    if value < 1 or value > 9:
        raise typer.BadParameter("Effort must be between 1 and 9.")
    return value


def _validate_width(value: int) -> int:
    if value <= 0:
        raise typer.BadParameter("Width must be a positive integer.")
    if value % 2 != 0:
        raise typer.BadParameter("Width must be even (to maintain 2:1 aspect ratio).")
    return value


@app.command()
def convert(
    input_dir: Annotated[
        Path,
        typer.Option(
            "--input",
            "-i",
            help="Input directory containing source textures.",
            exists=True,
            file_okay=False,
            dir_okay=True,
            resolve_path=True,
        ),
    ],
    output_dir: Annotated[
        Path,
        typer.Option(
            "--output",
            "-o",
            help="Output directory for converted textures.",
            resolve_path=True,
        ),
    ],
    width: Annotated[
        list[int] | None,
        typer.Option(
            "--width",
            "-w",
            help="Target width(s) in pixels. Can be specified multiple times.",
        ),
    ] = None,
    quality: Annotated[
        int,
        typer.Option(
            "--quality",
            "-q",
            help="JPEG XL encoding quality (1-100).",
            callback=_validate_quality,
        ),
    ] = 85,
    effort: Annotated[
        int,
        typer.Option(
            "--effort",
            "-e",
            help="JPEG XL encoding effort (1-9). Higher = smaller file, slower.",
            callback=_validate_effort,
        ),
    ] = 7,
    do_sharpen: Annotated[
        bool,
        typer.Option(
            "--sharpen",
            help="Apply UnsharpMask sharpening after downscale.",
        ),
    ] = False,
) -> None:
    """Convert source textures to JPEG XL at one or more target resolutions."""
    widths = width if width else [8192]

    # Validate each width
    for w in widths:
        _validate_width(w)

    output_dir.mkdir(parents=True, exist_ok=True)

    run_pipeline(
        input_dir=input_dir,
        output_dir=output_dir,
        widths=widths,
        quality=quality,
        effort=effort,
        do_sharpen=do_sharpen,
    )
