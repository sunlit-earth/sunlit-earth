# Command line and display reference

The [README](../README.md#using-the-app) covers everyday use. This reference contains additional launch options, display diagnostics, the camera fixed toward the Sun, and file locations.

## Installing with a package manager

Stable releases are published to a Scoop bucket and a Homebrew tap of this project's own, [sunlit-earth/scoop-bucket](https://github.com/sunlit-earth/scoop-bucket) and [sunlit-earth/homebrew-tap](https://github.com/sunlit-earth/homebrew-tap). Both are written by `package-managers.yml` when a release is published, and only after the new manifest has been installed and run on every platform it serves ([testing.md](testing.md#cicd)). Prereleases are not published there. The two repositories get their first manifests with the stable 0.2.0 release, so until then they are empty.

**Windows, with Scoop.**

```powershell
scoop bucket add sunlit-earth https://github.com/sunlit-earth/scoop-bucket
scoop install sunlit-earth/sunlit-earth
```

This gives a Sunlit Earth entry in the Start Menu and a `sunlit-earth` command in the terminal. Scoop downloads without marking the file as coming from the internet, so SmartScreen does not ask about it. `scoop update sunlit-earth` skips the app while it is running, so quit it from its tray icon first; Scoop says so after the install too.

**macOS, with Homebrew.**

```bash
brew install --cask sunlit-earth/tap/sunlit-earth
```

This puts `Sunlit Earth.app` in `/Applications` and a `sunlit-earth` command on `PATH`. The cask removes the quarantine attribute Homebrew sets on the download, so the app should open without the System Settings step described below; a hosted runner shows the attribute gone and the app starting, and whether a real Mac shows no dialog is still a tester question ([platforms.md](platforms.md)). That is a choice with a cost you should know about: the app is ad-hoc signed and not notarized, and without the attribute Gatekeeper never assesses it. Homebrew deprecated its own `--no-quarantine` flag in 5.0.0, alongside requiring casks in its official repository to pass Gatekeeper. Installing through this tap means trusting this project's release pipeline in place of that check. `brew uninstall --cask sunlit-earth` quits and removes the app, and `brew uninstall --zap --cask sunlit-earth` also deletes `~/Library/Application Support/SunlitEarth`. Leaving out `--cask` on a Mac gives `Linux is required for this software.`, because the tap's formula of the same name is the Linux one.

**Linux, with Homebrew.**

```bash
brew install sunlit-earth/tap/sunlit-earth
```

This installs the binary and its textures under Homebrew's prefix and a `sunlit-earth` command on `PATH`. A formula cannot write into your home directory, so the desktop entry and icons are one more command, which the install prints as a caveat:

```bash
$(brew --prefix)/opt/sunlit-earth/libexec/assets/linux/install-user.sh --exec $(brew --prefix)/opt/sunlit-earth/bin/sunlit-earth
```

`--exec` matters because a desktop session's launcher usually does not have Homebrew's `bin` on its `PATH`, and the `opt` path stays the same across upgrades. Before `brew uninstall sunlit-earth`, remove the entry again with `$(brew --prefix)/opt/sunlit-earth/libexec/assets/linux/install-user.sh --uninstall`. The render needs a Vulkan driver, as it does for the tarball.

## Installing on macOS

The macOS build has never been run on a Mac. Nobody working on this project owns Apple hardware, so it is written against Apple's documentation, compiled and unit-tested on a hosted macOS runner, and that is the whole of the evidence behind it. [macos-testing.md](macos-testing.md) says what a tester with a Mac could report back.

Two archives are published for each Mac architecture, `<arch>` being `aarch64` for Apple Silicon and `x86_64` for Intel: `sunlit-earth-<version>-macos-<arch>-app.zip` holds `Sunlit Earth.app`, and `sunlit-earth-<version>-macos-<arch>-terminal.tar.gz` holds the same binary with its textures and no bundle. Both are signed ad-hoc, because notarization needs an Apple Developer Program membership this project does not have, and Gatekeeper treats an ad-hoc signed download as unverified. Since macOS 15.1 there is no Control-click "Open" shortcut around that.

**The app bundle.** Unzip the archive, which gives a folder holding `Sunlit Earth.app` and a `README.txt`, and move the app to Applications. Double-clicking it the first time gives a dialog saying macOS could not verify that the app is free of malware. Open System Settings, go to Privacy & Security, scroll to the bottom, and choose Open Anyway for Sunlit Earth; on macOS 26 that step asks for an administrator password. `xattr -dr com.apple.quarantine "/Applications/Sunlit Earth.app"` does the same thing in one line, by removing the quarantine attribute the browser set.

**The plain binary.** Downloading the tarball with `curl` and unpacking it with `tar` in Terminal avoids Gatekeeper entirely, because quarantine is an extended attribute that browsers and Archive Utility set and `tar` does not:

```bash
curl -L -O https://github.com/sunlit-earth/sunlit-earth/releases/latest/download/sunlit-earth-0.1.0-macos-aarch64-terminal.tar.gz
tar xzf sunlit-earth-0.1.0-macos-aarch64-terminal.tar.gz
cd sunlit-earth-0.1.0-macos-aarch64-terminal
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

Run `sunlit-earth --help` for the full list. Use `sunlit-earth.exe` on Windows or `./sunlit-earth` from the extracted folder on Linux. From a source checkout, prefix app arguments with `cargo app --`.

```bash
cargo app -- render --output earth.png --width 1920 --height 1080
cargo app -- displays
cargo app -- displays --out ./plan
```

`render` produces a single PNG without a window. `displays` prints the detected monitors, anchor, and wallpaper plan without creating a GPU device or changing the desktop. Adding `--out` renders the planned images into the chosen directory for inspection but does not apply them as wallpaper. The text output is useful in display bug reports.

## Several screens

The Displays group appears when the session has more than one monitor. Its modes are:

- One screen applies the view to the selected monitor and leaves the others alone where the desktop supports it.
- Mirror, the default, renders the same view for each monitor's resolution and aspect ratio.
- Extend renders one continuous scene over the desktop and divides it among the monitors.

The anchor is the monitor that receives the wallpaper in One screen mode and the monitor on which the view is centered in the other modes. Primary (automatic) follows the system's primary monitor. The layout diagram highlights the anchor. The configuration stores these choices as `display_mode` and `anchor_monitor`.

Windows, XFCE, and KDE Plasma support applying images to individual monitors. GNOME, Cinnamon, MATE, and Budgie use one wallpaper image and support spanning. LXQt uses one image for all monitors. When a desktop cannot implement the selected mode, the status line explains the fallback. See [platforms.md](platforms.md) for exact capabilities and verification status, including Wayland and mixed display scaling limitations.

## A camera fixed toward the Sun

By default the camera hangs over one place on the Earth and turns with it, so the lighting moves across the view over the day. "Fixed toward the Sun", the check at the top of Camera Position, keeps the camera where it is relative to the Sun instead: the Earth turns beneath it once a day, west to east, and a view of the day side stays a view of the day side, a sunrise at the limb stays a sunrise. Over the year the terminator still slides north and south with the seasons, at most about 0.4 degrees a day.

With the check on, the Longitude row is replaced by a Time of day row, the local solar time beneath the camera: 12:00 looks down on the noon side, 18:00 on the evening terminator, 00:00 on the middle of the night. Latitude, zoom, orientation and framing keep their meaning. Switching the check either way keeps the view where it is. A preset keeps the mode, centers its place as of the moment it is clicked, and lets it turn away from there. A left drag across the globe moves the time of day instead of the longitude. With Override date/time on, the Time slider turns the globe under a Sun that stays put, the Day slider moves the terminator north and south, and the Earth stops turning; turning the override off turns the globe to the present.

The preview is redrawn about once a pixel of the turn while it is shown, at most every 5 seconds. The wallpaper follows the auto-refresh interval, and between two refreshes the surface moves by about this many pixels on a screen 1080 pixels tall at the default 20 degree Earth field of view, twice that on a 2160 pixel one:

| Auto-refresh | Default zoom (8 radii) | Europe preset (3.2 radii) | Closest zoom (1.5 radii) |
|---|---|---|---|
| 1 minute | 2 px | 6 px | 27 px |
| 5 minutes (the default) | 10 px | 30 px | 134 px |
| 15 minutes | 29 px | 91 px | 401 px |
| 30 minutes | 57 px | 182 px | 802 px |

The configuration stores the mode as `sun_fixed` and the time of day as `solar_time`, in hours. While the mode is on, `longitude` keeps the value it had when the mode was last switched on, which is also what an older build that does not know the mode shows.

## The controls on the preview

The camera panel in the preview's top right corner moves the camera the way the mouse does on the globe: drag around the compass ring, or drag its N around it, to roll the picture, and click the N to put north back up, push the orbit joystick inside it and hold to travel over the globe or click its arrows to step 10 degrees, use + and − or the slider beside them to zoom, and push the look and frame joysticks to aim the camera away from the Earth's center or to move the frame. A ↺ beside a joystick puts its values back. Over the ring, the N and the joysticks the pointer becomes an open hand that closes while you drag; on Windows it stays the pointing hand, since Windows draws the grabbing hand as a four-way arrow. Every control's hover text names the mouse gesture that does the same, and a gesture on the globe lights its control up. While the pointer is elsewhere the panel fades.

The time bar at the bottom center shows the time on screen, in local time unless it says UTC; hovering it opens the full bar. The Time and Date tracks set the date and time the wallpaper uses, a click on the time or the date enters them exactly (a click outside the picker cancels), and Now returns to live time. Play runs the preview through time without touching the wallpaper: the Time label makes it run the clock, from real time to 6 hours a second, the Date label the date at the same time of day, from a day to 3 months a second, and the dots on either side of play pick the speed, backward on the left and forward on the right. While it plays or rests away from the setting the bar says Preview, and a click there goes back. Closing the window pauses it. The zone chip switches between local time and UTC, and the pin keeps the bar open. In a window too narrow for the whole bar it keeps its width, centered, and its two ends are cut off and fade out at the edges of the preview.

Keys act on the preview, which has the keyboard when the window opens and takes it back with a click on the globe after a sidebar control had it: the arrows or W, A, S and D orbit, with Shift they look and with Ctrl (⌘ on macOS) they move the frame; + and − zoom, Q and E roll, N puts north up; Space plays and pauses, J and L shuttle backward and forward, a step for each press or, held, a step every 0.2 seconds after the first 0.4 until K, Space, 0 or R ends the hold, K pauses, R reverses, T and G make play run the time or the date, comma and period step back and forward (with Shift by more), 0 returns to now; H hides and shows the panel and the bar. F1 or Ctrl + ? (⌘? on macOS) opens a sheet with every gesture and key.

## App data and environment overrides

By default, `config.toml`, the cloud cache, the tile packs the Earth is drawn from (`tile_cache/`, about 416 MiB for the year, rebuilt on first launch and when the faces change), generated wallpaper directories, and memory metrics CSV files live under the platform's local app data directory:

- Windows uses `%LOCALAPPDATA%\SunlitEarth`.
- Linux uses `~/.local/share/SunlitEarth`.
- macOS uses `~/Library/Application Support/SunlitEarth`.

When a release changes what `config.toml` holds, its first start updates the file and keeps the one it found beside it, named after the version that file was at, such as `config.v0.toml`. Starting 0.3.0 for the first time moves a texture resolution of 4096 to 8192 and sets anti-aliasing to MSAA 2x; both are in the Rendering group.

Each wallpaper publish writes images into a fresh `gen-<id>/` directory, with one image per monitor and a canvas where required. Fresh paths let desktop shells notice updates without reading a partially rewritten image. The app manages retention and cleanup; [platforms.md](platforms.md) explains the lifecycle and the older alternating file scheme it replaced.

Started from a terminal, the app writes its log there and nowhere else. Started any other way, which is a launcher, a shortcut, or a macOS app bundle, it also writes `sunlit-earth.<date>.log` into the directory above: one file per day and a week of them kept. The newest is what to attach to a bug report, and the app's own first log line names the file it is writing.

The [environment variable reference](architecture.md#environment-knobs) covers config, textures, caches, metrics, cloud polling, and test and VM overrides. Variables that carry values generally treat blank as unset; switches documented as presence only are enabled by being present, regardless of their value.
