# cloud-fetch

Fetch and process satellite cloud imagery for Sunlit Earth.

Two data sources are supported:

- **matteason** -- Pre-composited 8K equirectangular cloud JPEG from [clouds.matteason.co.uk](https://clouds.matteason.co.uk)
- **gmgsi** -- NOAA GMGSI Longwave IR band from the public `noaa-gmgsi-pds` S3 bucket, remapped to equirectangular and upscaled to 8K

Both sources apply Photoshop-style Levels adjustment (floor, ceiling, gamma) and optional Gaussian blur.

## Usage

```bash
# Matteason source (simplest, pre-composited)
uv run cloud-fetch matteason --output-dir ./output

# GMGSI source (latest available data)
uv run cloud-fetch gmgsi --output-dir ./output

# GMGSI with specific date/time
uv run cloud-fetch gmgsi --output-dir ./output --date 20260319 --hour 06

# Custom levels processing
uv run cloud-fetch matteason -o ./output --floor 30 --ceiling 200 --gamma 0.5

# Also save the unprocessed image
uv run cloud-fetch matteason -o ./output --raw
```

## Options

### `cloud-fetch matteason`

| Option | Default | Description |
|---|---|---|
| `--output-dir`, `-o` | cwd | Output directory |
| `--floor` | 50 | Levels black point (0--255) |
| `--ceiling` | 255 | Levels white point (0--255) |
| `--gamma` | 0.3 | Levels midtone gamma |
| `--blur` | 0.0 | Gaussian blur sigma (pixels) |
| `--raw` | off | Also save unprocessed image |

### `cloud-fetch gmgsi`

| Option | Default | Description |
|---|---|---|
| `--output-dir`, `-o` | cwd | Output directory |
| `--date` | auto | Force date (YYYYMMDD) |
| `--hour` | auto | Force UTC hour (HH) |
| `--floor` | 60 | Levels black point (0--255) |
| `--ceiling` | 215 | Levels white point (0--255) |
| `--gamma` | 0.7 | Levels midtone gamma |
| `--blur` | 3.0 | Gaussian blur sigma at 8K (pixels) |
| `--raw` | off | Also save unprocessed image |

## Development

```bash
uv sync                          # Install dependencies
uv run pytest -v                 # Run tests
uv run ruff check src/ tests/   # Lint
uv run ruff format src/ tests/  # Format
```
