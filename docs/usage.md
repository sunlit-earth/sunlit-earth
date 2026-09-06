# Command line and display reference

The [README](../README.md#using-the-app) covers everyday use. This reference contains additional launch options, display diagnostics, and file locations.

## Launch options

The main options are:

- `--mode <tray|window>` selects tray or window mode.
- `--tray-start <visible|hidden>` controls the initial settings window visibility in tray mode.
- `--quality <low|medium|high>` overrides the saved quality tier for one run.
- `--texture-resolution <8192|4096|2048>` overrides the texture width for one run.
- `--software-rendering` requests a software adapter.
- `--textures-dir PATH` selects a texture directory.
- `--log-level LEVEL` controls logging within the levels compiled into the binary.
- `--ipc-socket NAME` enables the optional local control socket used by the desktop tests.

Run `sunlit-earth --help` for the full list. Use `sunlit-earth.exe` on Windows or `./sunlit-earth` from the extracted folder on Linux. From a source checkout, prefix app arguments with `cargo run --`.

```bash
cargo run -- render --output earth.png --width 1920 --height 1080
cargo run -- displays
cargo run -- displays --out ./plan
```

`render` produces a single PNG without a window. `displays` prints the detected monitors, anchor, and wallpaper plan without creating a GPU device or changing the desktop. Adding `--out` renders the planned images into the chosen directory for inspection but does not apply them as wallpaper. The text output is useful in display bug reports.

## Several screens

The Displays group appears when the session has more than one monitor. Its modes are:

- One screen applies the view to the selected monitor and leaves the others alone where the desktop supports it.
- Mirror, the default, renders the same view for each monitor's resolution and aspect ratio.
- Extend renders one continuous scene over the desktop and divides it among the monitors.

The anchor is the monitor that receives the wallpaper in One screen mode and the monitor on which the view is centered in the other modes. Primary (automatic) follows the system's primary monitor. The layout diagram highlights the anchor. The configuration stores these choices as `display_mode` and `anchor_monitor`.

Windows, XFCE, and KDE Plasma support applying images to individual monitors. GNOME, Cinnamon, MATE, and Budgie use one wallpaper image and support spanning. LXQt uses one image for all monitors. When a desktop cannot implement the selected mode, the status line explains the fallback. See [platforms.md](platforms.md) for exact capabilities and verification status, including Wayland and mixed display scaling limitations.

## App data and environment overrides

By default, `config.toml`, the cloud cache, downscaled textures in `texture_cache/`, generated wallpaper directories, and memory metrics CSV files live under the platform's local app data directory:

- Windows uses `%LOCALAPPDATA%\SunlitEarth`.
- Linux uses `~/.local/share/SunlitEarth`.
- macOS uses `~/Library/Application Support/SunlitEarth`.

Each wallpaper publish writes images into a fresh `gen-<id>/` directory, with one image per monitor and a canvas where required. Fresh paths let desktop shells notice updates without reading a partially rewritten image. The app manages retention and cleanup; [platforms.md](platforms.md) explains the lifecycle and the older alternating file scheme it replaced.

The [environment variable reference](architecture.md#environment-knobs) covers config, textures, caches, metrics, cloud polling, and test and VM overrides. Variables that carry values generally treat blank as unset; switches documented as presence only are enabled by being present, regardless of their value.
