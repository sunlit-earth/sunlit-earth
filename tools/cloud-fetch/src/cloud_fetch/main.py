"""CLI interface for the cloud-fetch tool."""

from pathlib import Path
from typing import Annotated

import typer

from cloud_fetch.gmgsi import (
    DEFAULT_BLUR_SIGMA as GMGSI_DEFAULT_BLUR,
)
from cloud_fetch.gmgsi import (
    DEFAULT_CEILING as GMGSI_DEFAULT_CEILING,
)
from cloud_fetch.gmgsi import (
    DEFAULT_FLOOR as GMGSI_DEFAULT_FLOOR,
)
from cloud_fetch.gmgsi import (
    DEFAULT_GAMMA as GMGSI_DEFAULT_GAMMA,
)
from cloud_fetch.gmgsi import (
    fetch_gmgsi,
)
from cloud_fetch.matteason import (
    DEFAULT_BLUR_SIGMA as MATT_DEFAULT_BLUR,
)
from cloud_fetch.matteason import (
    DEFAULT_CEILING as MATT_DEFAULT_CEILING,
)
from cloud_fetch.matteason import (
    DEFAULT_FLOOR as MATT_DEFAULT_FLOOR,
)
from cloud_fetch.matteason import (
    DEFAULT_GAMMA as MATT_DEFAULT_GAMMA,
)
from cloud_fetch.matteason import (
    fetch_matteason,
)

app = typer.Typer(
    name="cloud-fetch",
    help="Fetch and process satellite cloud imagery for Sunlit Earth.",
)

_DEFAULT_OUTPUT_DIR = Path.cwd()


def _validate_floor(value: int) -> int:
    if value < 0 or value > 255:
        raise typer.BadParameter("Floor must be between 0 and 255.")
    return value


def _validate_ceiling(value: int) -> int:
    if value < 0 or value > 255:
        raise typer.BadParameter("Ceiling must be between 0 and 255.")
    return value


def _validate_gamma(value: float) -> float:
    if value <= 0:
        raise typer.BadParameter("Gamma must be positive.")
    return value


def _validate_blur(value: float) -> float:
    if value < 0:
        raise typer.BadParameter("Blur sigma must be non-negative.")
    return value


@app.command()
def matteason(
    output_dir: Annotated[
        Path,
        typer.Option(
            "--output-dir",
            "-o",
            help="Directory for output PNG (default: current directory).",
            resolve_path=True,
        ),
    ] = _DEFAULT_OUTPUT_DIR,
    floor: Annotated[
        int,
        typer.Option(
            "--floor",
            help="Levels black point, 0-255.",
            callback=_validate_floor,
        ),
    ] = MATT_DEFAULT_FLOOR,
    ceiling: Annotated[
        int,
        typer.Option(
            "--ceiling",
            help="Levels white point, 0-255.",
            callback=_validate_ceiling,
        ),
    ] = MATT_DEFAULT_CEILING,
    gamma: Annotated[
        float,
        typer.Option(
            "--gamma",
            help="Levels midtone gamma.",
            callback=_validate_gamma,
        ),
    ] = MATT_DEFAULT_GAMMA,
    blur: Annotated[
        float,
        typer.Option(
            "--blur",
            help="Gaussian blur sigma in pixels.",
            callback=_validate_blur,
        ),
    ] = MATT_DEFAULT_BLUR,
    raw: Annotated[
        bool,
        typer.Option(
            "--raw",
            help="Also save the unprocessed image.",
        ),
    ] = False,
) -> None:
    """Fetch the matteason 8K cloud image and apply levels processing."""
    try:
        fetch_matteason(
            output_dir=output_dir,
            floor=floor,
            ceiling=ceiling,
            gamma=gamma,
            blur=blur,
            raw=raw,
        )
    except Exception as e:
        typer.echo(f"Error: {e}", err=True)
        raise typer.Exit(code=1) from e


@app.command()
def gmgsi(
    output_dir: Annotated[
        Path,
        typer.Option(
            "--output-dir",
            "-o",
            help="Directory for output PNG (default: current directory).",
            resolve_path=True,
        ),
    ] = _DEFAULT_OUTPUT_DIR,
    date: Annotated[
        str | None,
        typer.Option(
            "--date",
            help="Force date YYYYMMDD.",
        ),
    ] = None,
    hour: Annotated[
        str | None,
        typer.Option(
            "--hour",
            help="Force UTC hour HH.",
        ),
    ] = None,
    floor: Annotated[
        int,
        typer.Option(
            "--floor",
            help="Levels black point, 0-255.",
            callback=_validate_floor,
        ),
    ] = GMGSI_DEFAULT_FLOOR,
    ceiling: Annotated[
        int,
        typer.Option(
            "--ceiling",
            help="Levels white point, 0-255.",
            callback=_validate_ceiling,
        ),
    ] = GMGSI_DEFAULT_CEILING,
    gamma: Annotated[
        float,
        typer.Option(
            "--gamma",
            help="Levels midtone gamma.",
            callback=_validate_gamma,
        ),
    ] = GMGSI_DEFAULT_GAMMA,
    blur: Annotated[
        float,
        typer.Option(
            "--blur",
            help="Gaussian blur sigma in pixels at 8K output.",
            callback=_validate_blur,
        ),
    ] = GMGSI_DEFAULT_BLUR,
    raw: Annotated[
        bool,
        typer.Option(
            "--raw",
            help="Also save the unprocessed image.",
        ),
    ] = False,
) -> None:
    """Fetch NOAA GMGSI LW satellite imagery and produce a cloud texture."""
    try:
        fetch_gmgsi(
            output_dir=output_dir,
            date=date,
            hour=hour,
            floor=floor,
            ceiling=ceiling,
            gamma=gamma,
            blur=blur,
            raw=raw,
        )
    except Exception as e:
        typer.echo(f"Error: {e}", err=True)
        raise typer.Exit(code=1) from e
