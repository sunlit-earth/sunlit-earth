# Changelog

Notable changes to Sunlit Earth, for people who use it. The GitHub release pages list every merged pull request.

## [0.5.0] - 2026-10-25

- Camera and time controls directly in the scene preview. It's now easier to discover how camera controls work and all keyboard shortcuts are documented. Time controls allow you to control time. Try it, it's fun! ([#82](https://github.com/sunlit-earth/sunlit-earth/pull/82))
- Autostart option. ([#88](https://github.com/sunlit-earth/sunlit-earth/pull/88))
- The scene shown in the preview and the actual wallpaper now match exactly. ([#87](https://github.com/sunlit-earth/sunlit-earth/pull/87))
- New camera mode: fixed toward the Sun, so the Earth turns below the camera. ([#80](https://github.com/sunlit-earth/sunlit-earth/pull/80))
- Launching the app while it is already running now opens the window of the running app. ([#88](https://github.com/sunlit-earth/sunlit-earth/pull/88))
- Other changes: Adjustable ocean colors ([#81](https://github.com/sunlit-earth/sunlit-earth/pull/81)), no more black bars around the preview window ([#86](https://github.com/sunlit-earth/sunlit-earth/pull/86)), on macOS the Dock icon only shows while the window is open ([#88](https://github.com/sunlit-earth/sunlit-earth/pull/88)), Bugfix for the first wallpaper after starting the app, which sometimes had no clouds ([#85](https://github.com/sunlit-earth/sunlit-earth/pull/85))

## [0.4.0] - 2026-10-16

- Sea ice in the Arctic and Antarctic, changing with the month, based on satellite data from EUMETSAT. ([#76](https://github.com/sunlit-earth/sunlit-earth/pull/76), [#78](https://github.com/sunlit-earth/sunlit-earth/pull/78))
- Less grey haze in the clouds over cold regions, and an option to turn the clouds off. ([#74](https://github.com/sunlit-earth/sunlit-earth/pull/74))
- Settings under Advanced can be reset to their defaults one by one or by group. ([#77](https://github.com/sunlit-earth/sunlit-earth/pull/77))
- Bugfix for lighting in the Day and Night only modes. ([#72](https://github.com/sunlit-earth/sunlit-earth/pull/72))

## [0.3.0] - 2026-10-10

- Seasonal textures: the Earth's surface changes each month, from NASA's Blue Marble Next Generation, with sharper detail and no distortion at the poles. ([#69](https://github.com/sunlit-earth/sunlit-earth/pull/69))
- Lower memory use: Lots of optimizations in the way textures are handled; sunlit earth needs less RAM but delivers the same image quality. ([#69](https://github.com/sunlit-earth/sunlit-earth/pull/69), [#70](https://github.com/sunlit-earth/sunlit-earth/pull/70))

## [0.2.2] - 2026-09-29

- Bugfix for an issue in Slint 1.18 that prevents users from scrolling to the end of the page in the About window. ([#67](https://github.com/sunlit-earth/sunlit-earth/pull/67))

## [0.2.1] - 2026-09-29

- macOS: the settings window has an opaque title bar and a frame again. ([#64](https://github.com/sunlit-earth/sunlit-earth/issues/64), [#65](https://github.com/sunlit-earth/sunlit-earth/pull/65))

## [0.2.0] - 2026-09-24

- macOS support for Apple silicon and Intel, as an app bundle or a plain binary for Terminal. ([#53](https://github.com/sunlit-earth/sunlit-earth/pull/53))
- Linux builds for aarch64. ([#56](https://github.com/sunlit-earth/sunlit-earth/pull/56))
- Install and upgrade through Scoop on Windows and Homebrew on macOS and Linux. ([#62](https://github.com/sunlit-earth/sunlit-earth/pull/62))
- Wallpapers on many more Linux desktops and window managers, including sway, i3, Hyprland and Wayland compositors. ([#54](https://github.com/sunlit-earth/sunlit-earth/issues/54), [#60](https://github.com/sunlit-earth/sunlit-earth/pull/60))
- Release archives name their platform and architecture and include a short README. ([#56](https://github.com/sunlit-earth/sunlit-earth/pull/56), [#61](https://github.com/sunlit-earth/sunlit-earth/pull/61))
- Linux on Wayland: task managers show the app's icon. ([#57](https://github.com/sunlit-earth/sunlit-earth/pull/57))

## [0.1.0] - 2026-09-18

- First public release, for Windows and Linux.

[0.5.0]: https://github.com/sunlit-earth/sunlit-earth/releases/tag/v0.5.0
[0.4.0]: https://github.com/sunlit-earth/sunlit-earth/releases/tag/v0.4.0
[0.3.0]: https://github.com/sunlit-earth/sunlit-earth/releases/tag/v0.3.0
[0.2.2]: https://github.com/sunlit-earth/sunlit-earth/releases/tag/v0.2.2
[0.2.1]: https://github.com/sunlit-earth/sunlit-earth/releases/tag/v0.2.1
[0.2.0]: https://github.com/sunlit-earth/sunlit-earth/releases/tag/v0.2.0
[0.1.0]: https://github.com/sunlit-earth/sunlit-earth/releases/tag/v0.1.0
