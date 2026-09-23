# Testing Sunlit Earth on a Mac

The macOS build has never been run on a Mac. Nobody working on this project owns Apple hardware, so the platform code is written against Apple's documentation, compiled and unit-tested on a hosted macOS runner, and that is the whole of the evidence behind it. [platforms.md](platforms.md) gives every macOS row the tier of evidence it is at.

This is the checklist for somebody with a Mac who is willing to find out. Nothing here needs developer tools, and none of it changes anything a normal app cannot change: the app writes only under `~/Library/Application Support/SunlitEarth` and sets the desktop picture.

## Getting it open

Two archives are published for each Mac architecture, `<arch>` being `aarch64` for Apple Silicon and `x86_64` for Intel. `sunlit-earth-<version>-macos-<arch>.zip` holds `Sunlit Earth.app`, and `sunlit-earth-<version>-macos-<arch>.tar.gz` holds the same binary with its textures and no bundle. Both are signed ad-hoc: notarization needs an Apple Developer Program membership this project does not have, so Gatekeeper treats either download as unverified, and since macOS 15.1 there is no Control-click "Open" shortcut around that.

**The bundle.** Unzip, move `Sunlit Earth.app` to Applications, double-click it. The first attempt gives a dialog saying macOS could not verify that the app is free of malware. Open System Settings, go to Privacy & Security, scroll to the bottom, and choose Open Anyway for Sunlit Earth. On macOS 26 that asks for an administrator password; on 15 it does not. `xattr -dr com.apple.quarantine "/Applications/Sunlit Earth.app"` in Terminal does the same thing by removing the attribute the browser set, and is worth trying if the System Settings route does not appear.

**The tarball.** Downloaded with `curl` and unpacked with `tar`, it meets no Gatekeeper dialog at all, because quarantine is an extended attribute that browsers and Archive Utility set and `tar` does not:

```bash
curl -L -O https://github.com/sunlit-earth/sunlit-earth/releases/latest/download/sunlit-earth-0.1.0-macos-aarch64.tar.gz
tar xzf sunlit-earth-0.1.0-macos-aarch64.tar.gz
cd sunlit-earth-0.1.0-macos-aarch64
./sunlit-earth
```

Started this way the app prints its log to the terminal, which is the most useful shape a report can take. Downloading the same tarball in Safari and unpacking it by double-clicking does not avoid Gatekeeper: Archive Utility propagates quarantine to what it extracts.

Which of the two routes worked, and what each one said, is itself a result worth reporting.

## What to look at

- **The status item.** It should appear in the menu bar. Is it legible against a light menu bar and against a dark one? Does its Open entry bring the settings window to the front, including when the window was already open behind something else? Does Quit end the app rather than leaving the item behind?
- **The wallpaper.** Change something in the settings window and let it publish. Does the desktop picture actually change? Check each screen if there is more than one, and each of the three display modes: One screen, Mirror, Extend.
- **A second Space.** Make another Space and switch to it. Does it get the wallpaper too, or does it keep the old picture until something else touches it?
- **Logging out and back in.** Does the wallpaper survive it? Does anything reset?
- **`sunlit-earth displays`.** In Terminal, from the tarball or from `"/Applications/Sunlit Earth.app/Contents/MacOS/sunlit-earth" displays`. It prints the monitors the session has and the plan they come to, and the whole output is useful whether or not it looks right.
- **The window itself.** Does it open at a sensible size, does the preview render, do the sliders move the picture, and does a Retina display make it blurry or sharp?

## What to send back

- The log file. Started from a terminal the log goes there and nowhere else; started from Finder or the Dock it also goes to `~/Library/Application Support/SunlitEarth/sunlit-earth.<date>.log`. The date is the rotation's: there is a file per day and a week of them, so send the newest, and the app's own first log line names the one it is writing.
- The output of `sunlit-earth displays`.
- The macOS version and the machine, from the Apple menu, About This Mac.
- Screenshots of anything that looks wrong. A screenshot is worth more than a description.

A report that only says "the wallpaper did not change" is still worth having, but the log file beside it is what makes it actionable.
