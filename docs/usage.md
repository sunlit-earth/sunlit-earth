# Command line and display reference

The [README](../README.md#using-the-app) covers everyday use. This reference contains additional launch options, display diagnostics, and file locations.

## Installing on macOS

The macOS build has never been run on a Mac. Nobody working on this project owns Apple hardware, so it is written against Apple's documentation, compiled and unit-tested on a hosted macOS runner, and that is the whole of the evidence behind it. [macos-testing.md](macos-testing.md) says what a tester with a Mac could report back.

Two archives are published for each Mac architecture, `<arch>` being `aarch64` for Apple Silicon and `x86_64` for Intel: `sunlit-earth-<version>-macos-<arch>.zip` holds `Sunlit Earth.app`, and `sunlit-earth-<version>-macos-<arch>.tar.gz` holds the same binary with its textures and no bundle. Both are signed ad-hoc, because notarization needs an Apple Developer Program membership this project does not have, and Gatekeeper treats an ad-hoc signed download as unverified. Since macOS 15.1 there is no Control-click "Open" shortcut around that.

**The app bundle.** Unzip the archive and move `Sunlit Earth.app` to Applications. Double-clicking it the first time gives a dialog saying macOS could not verify that the app is free of malware. Open System Settings, go to Privacy & Security, scroll to the bottom, and choose Open Anyway for Sunlit Earth; on macOS 26 that step asks for an administrator password. `xattr -dr com.apple.quarantine "/Applications/Sunlit Earth.app"` does the same thing in one line, by removing the quarantine attribute the browser set.

**The plain binary.** Downloading the tarball with `curl` and unpacking it with `tar` in Terminal avoids Gatekeeper entirely, because quarantine is an extended attribute that browsers and Archive Utility set and `tar` does not:

```bash
curl -L -O https://github.com/sunlit-earth/sunlit-earth/releases/latest/download/sunlit-earth-0.1.0-macos-aarch64.tar.gz
tar xzf sunlit-earth-0.1.0-macos-aarch64.tar.gz
cd sunlit-earth-0.1.0-macos-aarch64
./sunlit-earth
```

Started this way the app prints its log to the terminal it came from, which is the shape a bug report wants. Downloading that same tarball in Safari and unpacking it by double-clicking does not avoid Gatekeeper: Archive Utility propagates the quarantine attribute to what it extracts.

To render an image from the bundle, the executable is at `"/Applications/Sunlit Earth.app/Contents/MacOS/sunlit-earth"`.

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

Started from a terminal, the app writes its log there and nowhere else. Started any other way, which is a launcher, a shortcut, or a macOS app bundle, it also writes `sunlit-earth.<date>.log` into the directory above: one file per day and a week of them kept. The newest is what to attach to a bug report, and the app's own first log line names the file it is writing.

The [environment variable reference](architecture.md#environment-knobs) covers config, textures, caches, metrics, cloud polling, and test and VM overrides. Variables that carry values generally treat blank as unset; switches documented as presence only are enabled by being present, regardless of their value.
