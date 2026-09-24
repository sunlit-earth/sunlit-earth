# Research: Installing Through Scoop and Homebrew From Our Own Repositories

Code reconnaissance and one web research pass, both on 2026-09-24. The goal is `scoop install` on Windows and `brew install` on macOS and Linux from repositories this project owns, without waiting for the official Scoop buckets or Homebrew repositories to accept the app. The plan that uses this is [2026-09-24-package-managers-plan.md](2026-09-24-package-managers-plan.md); it departs from section 5 by putting macOS on a cask that removes quarantine. Every claim says how far it was verified: read from this repository or the GitHub API, stated by upstream documentation or source, or inferred.

## 1. What a package manager gets from us today

Read from `release.yml`, `crates/xtask/src/commands/bundle.rs`, `texture_loader.rs` and the GitHub API.

- A `v*` tag is meant to publish, per release, `sunlit-earth-<version>-windows-x86_64.zip`, `sunlit-earth-<version>-linux-{x86_64,aarch64}.tar.gz`, `sunlit-earth-<version>-macos-{x86_64,aarch64}.tar.gz` (plain layout), `sunlit-earth-<version>-macos-{x86_64,aarch64}.zip` (`Sunlit Earth.app`) and one `build-info.json` holding every build's record, keyed by `<platform>-<arch>`. Each archive holds one top-level directory named like the archive. The tag trigger is dormant: `release.yml` is `workflow_dispatch` only, and the `publish` job only runs on a tag ref.
- The only published release, v0.1.0, predates the architecture suffix: its assets are `sunlit-earth-0.1.0-windows.zip` and `sunlit-earth-0.1.0-linux.tar.gz`, about 18 and 20 MB. No macOS asset has been published yet. Manifests written for the new names can only point at the next release.
- There is no checksums file, but the GitHub API returns a `digest` of the form `sha256:<hex>` for every release asset (seen on both v0.1.0 assets). The release workflow could equally write the hashes itself, since it has the archives in hand.
- The binary finds its textures by walking up from `std::env::current_exe()` looking for `textures/` or `Resources/textures`, after `--textures-dir`, `SUNLIT_EARTH_TEXTURES` and `./textures`. Whatever a package manager puts on `PATH` must therefore lead `current_exe()` to the real install directory, not to a symlink somewhere else.
- Configuration, caches and wallpapers live in `%LOCALAPPDATA%\SunlitEarth`, `~/.local/share/SunlitEarth` and `~/Library/Application Support/SunlitEarth`, outside any install directory, so an upgrade or uninstall through either tool leaves them alone.
- The Windows release binary is GUI subsystem and calls `AttachConsole(ATTACH_PARENT_PROCESS)` so its subcommands print to the terminal it was started from.
- The macOS builds are ad-hoc signed and not notarized. There is no autostart on any platform yet; `roadmap.md` lists a macOS LaunchAgent as future work.

## 2. Scoop

### 2.1 A personal bucket

- A bucket is any Git repository of JSON manifests. The name in `scoop bucket add <name> <url>` is a local alias, so there is no naming rule. Users then run `scoop install <name>/sunlit-earth`. Source: https://github.com/ScoopInstaller/Scoop/wiki/Buckets
- Scoop looks for a `bucket/` subdirectory and falls back to the repository root (`Find-BucketDirectory` in https://github.com/ScoopInstaller/Scoop/blob/master/lib/buckets.ps1). `bucket/` is the convention of the official template, https://github.com/ScoopInstaller/BucketTemplate, which also ships the CI and update workflows.
- The GitHub topic `scoop-bucket` gets a bucket indexed on scoop.sh (Buckets wiki).

### 2.2 The manifest

Stated by https://github.com/ScoopInstaller/Scoop/wiki/App-Manifests unless noted.

- `version`, `homepage` and `license` are required, `description` recommended. `GPL-3.0-or-later` is a valid SPDX identifier.
- `architecture.64bit.url` and `.hash` carry the download; `extract_dir` set to `sunlit-earth-<version>-windows-x86_64` puts the exe and `textures/` directly in the install directory.
- `bin: "sunlit-earth.exe"` creates a shim on `PATH` for the CLI subcommands. `shortcuts` creates a Start Menu entry, which is how most users will start a tray app.
- `persist` is not needed, since nothing the app writes lives in its install directory. `notes` can carry a one-line hint shown after install.
- `apps/<app>/current` is an NTFS junction to the version directory, and shims and shortcuts point through it (https://github.com/ScoopInstaller/Scoop/wiki/The-'Current'-Version-Alias, `link_current` in `lib/install.ps1`).
- Scoop reads the target exe's PE subsystem and makes the shim match. The native shim (default since Scoop 0.5.3) calls `FreeConsole()` for a GUI target launched without arguments and `AttachConsole(ATTACH_PARENT_PROCESS)` when launched with arguments, which fits what the app itself does. Source: `shim()` in https://github.com/ScoopInstaller/Scoop/blob/master/lib/core.ps1, https://raw.githubusercontent.com/ScoopInstaller/Shim/main/cpp/shim.cpp
- Whether `current_exe()` reports the junction path or the versioned path is unverified, and it should not matter: `textures/` sits beside the exe in both.

A manifest for the next release would look like this. It is assembled from the documented fields, not copied from an example, and has not been run:

```json
{
    "version": "0.1.1",
    "description": "A view of Earth as seen from space as your wallpaper",
    "homepage": "https://github.com/sunlit-earth/sunlit-earth",
    "license": "GPL-3.0-or-later",
    "architecture": {
        "64bit": {
            "url": "https://github.com/sunlit-earth/sunlit-earth/releases/download/v0.1.1/sunlit-earth-0.1.1-windows-x86_64.zip",
            "hash": "<sha256>",
            "extract_dir": "sunlit-earth-0.1.1-windows-x86_64"
        }
    },
    "bin": "sunlit-earth.exe",
    "shortcuts": [["sunlit-earth.exe", "Sunlit Earth"]],
    "checkver": "github",
    "autoupdate": {
        "architecture": {
            "64bit": {
                "url": "https://github.com/sunlit-earth/sunlit-earth/releases/download/v$version/sunlit-earth-$version-windows-x86_64.zip",
                "extract_dir": "sunlit-earth-$version-windows-x86_64"
            }
        }
    }
}
```

### 2.3 Updates and prereleases

- `"checkver": "github"` reads `/releases/latest` and ignores prereleases, by documentation and in `bin/checkver.ps1`. To follow `v0.1.1-beta.1` style tags, `checkver` can point at `https://api.github.com/repos/sunlit-earth/sunlit-earth/releases` with `jsonpath: "$[0].tag_name"` and a version regex. That recipe is built from documented primitives, not a published example. Source: https://github.com/ScoopInstaller/Scoop/wiki/App-Manifest-Autoupdate, https://docs.github.com/en/rest/releases/releases
- With no hash source configured, `autoupdate` downloads the archive and hashes it (same wiki page).
- Scoop has no prerelease switch; the official convention is a separate manifest such as `7zip-beta.json`, as in https://github.com/ScoopInstaller/Versions. A feature request for a flag was closed: https://github.com/ScoopInstaller/Scoop/issues/2951
- The template's `excavator.yml` runs every four hours, runs `checkver` with `autoupdate` over every manifest and commits the results, using only the bucket's own `GITHUB_TOKEN` with `contents: write`. Source: https://github.com/ScoopInstaller/BucketTemplate/blob/master/.github/workflows/excavator.yml
- The alternative is to write the manifest from this repository's release workflow and push it to the bucket, which is what GoReleaser's Scoop publisher does (https://goreleaser.com/customization/publish/scoop/). That needs a token that can write to the bucket repository.

### 2.4 SmartScreen

No Scoop document says what SmartScreen does with a Scoop install. What is verified: the Mark of the Web is written by programs that use `IAttachmentExecute`, mainly browsers and mail clients, and ordinary HTTP clients do not write it (https://textslashplain.com/2016/04/04/downloads-and-the-mark-of-the-web/). Scoop downloads with .NET `WebRequest` or aria2 (`lib/download.ps1`). So a Scoop-installed exe is expected to carry no `Zone.Identifier` stream and to meet no SmartScreen prompt. Inferred, to be checked once in the Windows guest.

### 2.5 The official Extras bucket later

- Main takes non-GUI tools only, with at least 500 stars and 150 forks and a stable version. Extras is defined as the bucket for what does not fit Main, so Sunlit Earth would go there. Source: https://github.com/ScoopInstaller/Scoop/wiki/Criteria-for-including-apps-in-the-main-bucket, https://github.com/ScoopInstaller/Extras
- No popularity threshold specific to Extras was found. Contributions need an issue first, strict field order, and a working `checkver` with `autoupdate`. Source: https://github.com/ScoopInstaller/.github/blob/main/.github/CONTRIBUTING.md
- Whether a project with only prerelease tags is accepted is unverified; the separate Versions bucket suggests a stable release is expected.

## 3. Homebrew

### 3.1 A personal tap

- The repository must be named `homebrew-<name>` for the short form to work: `sunlit-earth/homebrew-tap` becomes `brew tap sunlit-earth/tap`, and `brew install sunlit-earth/tap/sunlit-earth` taps it implicitly. Formulae go in `Formula/`, casks in `Casks/`. Source: https://github.com/Homebrew/brew/blob/master/docs/How-to-Create-and-Maintain-a-Tap.md
- `brew tap-new` scaffolds the layout plus `tests.yml` (`brew test-bot` on macOS and Linux, builds bottles), `publish.yml` (a manually dispatched `brew pr-pull`) and `autobump.yml` (scheduled `brew bump` driven by `livecheck`), all on the tap's own `GITHUB_TOKEN`. Source: https://github.com/Homebrew/brew/blob/master/Library/Homebrew/dev-cmd/tap-new.rb. The bottle machinery is for formulae built from source and is not needed for ours.

### 3.2 Gatekeeper decides the macOS shape

- Homebrew 5.0.0 (2025-11-12): "Casks without codesigning are deprecated. We will disable all Homebrew/homebrew-cask casks that fail Gatekeeper checks in September 2026", and "--no-quarantine and --quarantine flags have been deprecated". Verified from https://brew.sh/2025/11/12/homebrew-5.0.0/. The issue behind it gives 2026-09-01: https://github.com/Homebrew/brew/issues/20755
- The rule is an acceptance rule for homebrew/cask. Third-party taps choose their own audits, and maintainers say unsigned software may live in one's own tap. Source: https://docs.brew.sh/Acceptable-Casks, https://github.com/orgs/Homebrew/discussions/6482, https://github.com/orgs/Homebrew/discussions/7050
- Casks from any tap are still quarantined: `Cask::Quarantine` sets `com.apple.quarantine` on cask downloads because curl does not. Source: https://github.com/Homebrew/brew/blob/master/Library/Homebrew/cask/quarantine.rb. So an ad-hoc signed `.app` installed as a cask meets exactly the Gatekeeper dialog `usage.md` describes for a browser download, with the System Settings route on 15.1 and later.
- Formula downloads are not quarantined. The quarantine code lives only under `Library/Homebrew/cask/`, and https://docs.brew.sh/Homebrew-Security-and-Supply-Chain scopes it to casks. A formula installing the plain macOS tarball therefore meets no Gatekeeper dialog, for the same reason `curl` plus `tar` does not.
- A cask can strip the attribute itself. Homebrew now rejects legacy Ruby `postflight` blocks in official taps and deprecates them elsewhere in favor of `postflight_steps`, whose `run` step executes a command, so `run "/usr/bin/xattr", args: ["-dr", "com.apple.quarantine", "#{appdir}/Sunlit Earth.app"]` or similar is possible. Supported steps: https://docs.brew.sh/Cask-Cookbook. Third-party taps do this in practice (https://github.com/panarch/caffold/pull/308), no audit forbids it in a third-party tap, and no document endorses it. It is also exactly the circumvention Homebrew says it does not want to make easy. The exact `run` argument syntax is unverified.

### 3.3 A formula over the plain tarballs, for macOS and Linux

- Prebuilt binaries are not accepted in homebrew/core, and nothing enforces that in a third-party tap. cargo-dist generates exactly such formulae for taps their users own. Source: https://docs.brew.sh/Acceptable-Formulae, https://axodotdev.github.io/cargo-dist/book/installers/homebrew.html
- Per OS and architecture `url` and `sha256` go in nested `on_macos`/`on_linux`/`on_arm`/`on_intel` blocks (https://docs.brew.sh/Formula-Cookbook).
- The texture lookup rules out `bin.install_symlink`: Homebrew's `bin/` entries are symlinks, and on macOS `current_exe()` can return the symlink's path (https://doc.rust-lang.org/std/env/fn.current_exe.html). `bin.write_exec_script` writes a small script into `bin/` that `exec`s the binary at its real path, and the Formula Cookbook documents it for this case. So the formula installs the unpacked directory into `libexec` and writes the script.
- Linux ARM64 is Tier 1 since Homebrew 5.0.0 (https://docs.brew.sh/Support-Tiers). Homebrew uses the host's glibc when it is new enough (https://docs.brew.sh/Homebrew-on-Linux). Our Linux binaries are built on Ubuntu 22.04 and link against the host's glibc, fontconfig and xkbcommon, as they do when unpacked by hand. Whether Homebrew rewrites the ELF interpreter or RPATH of a non-bottle binary it installs is unverified and needs one test install.

A formula would look like this. Assembled from the documented primitives, not run:

```ruby
class SunlitEarth < Formula
  desc "View of Earth as seen from space as your wallpaper"
  homepage "https://github.com/sunlit-earth/sunlit-earth"
  version "0.1.1"
  license "GPL-3.0-or-later"

  on_macos do
    on_arm do
      url "https://github.com/sunlit-earth/sunlit-earth/releases/download/v0.1.1/sunlit-earth-0.1.1-macos-aarch64.tar.gz"
      sha256 "<sha256>"
    end
    on_intel do
      url "https://github.com/sunlit-earth/sunlit-earth/releases/download/v0.1.1/sunlit-earth-0.1.1-macos-x86_64.tar.gz"
      sha256 "<sha256>"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/sunlit-earth/sunlit-earth/releases/download/v0.1.1/sunlit-earth-0.1.1-linux-aarch64.tar.gz"
      sha256 "<sha256>"
    end
    on_intel do
      url "https://github.com/sunlit-earth/sunlit-earth/releases/download/v0.1.1/sunlit-earth-0.1.1-linux-x86_64.tar.gz"
      sha256 "<sha256>"
    end
  end

  def install
    libexec.install Dir["*"]
    bin.write_exec_script libexec/"sunlit-earth"
  end

  test do
    system bin/"sunlit-earth", "--version"
  end
end
```

`--version` comes from clap's `#[command(version)]` in `cli.rs`; that it returns before any window or GPU code runs is expected from clap and not tested here.

### 3.4 A cask for the `.app`

A cask is the documented shape for a macOS `.app` (https://docs.brew.sh/Adding-Software-to-Homebrew) and gives Launchpad and Spotlight an entry, which a formula does not. Its stanzas, per https://docs.brew.sh/Cask-Cookbook: `arch arm: "aarch64", intel: "x86_64"` feeding the `url`, `sha256 arm: "...", intel: "..."`, `app "Sunlit Earth.app"`, `binary "#{appdir}/Sunlit Earth.app/Contents/MacOS/sunlit-earth"` for the CLI, `depends_on macos:` for the 11.0 deployment target, `livecheck`, and `zap trash: "~/Library/Application Support/SunlitEarth"`. Since Homebrew 4.5.0 casks can install on Linux, but `app` is macOS only, so the Linux side stays a formula (https://brew.sh/2025/04/29/homebrew-4.5.0/).

The cost is the Gatekeeper dialog from 3.2 on first launch, or a `postflight_steps` that removes quarantine. Whether a formula and a cask of the same name can coexist in one tap, and which `brew install sunlit-earth` then picks, is unverified.

### 3.5 Updating the tap

- `livecheck` with `autobump.yml` in the tap polls for new releases and opens bump PRs on the tap's own token.
- Pushing from this repository's release workflow instead needs a token that can write to the tap: https://github.com/mislav/bump-homebrew-formula-action asks for a classic PAT with `repo` and `workflow` scopes, `Homebrew/actions/bump-packages` documents `public_repo` as enough for public repositories (https://github.com/Homebrew/actions/blob/master/bump-packages/README.md). A fine-grained token limited to the tap and bucket repositories with contents write should also work; not checked against either action.
- cargo-dist writes Homebrew formulae but has no Scoop support (https://github.com/axodotdev/cargo-dist/issues/521). Our archives, names and verification already exist, so adopting it would replace working machinery for one of the two targets.

### 3.6 homebrew/core and homebrew/cask later

- homebrew/core: needs a stable release, builds from source, and is not for software whose main output is a `.app`. Self-submitted software must be notable and used by others. Source: https://docs.brew.sh/Acceptable-Formulae. The star and fork thresholds often quoted are not on that page today and are unverified.
- homebrew/cask: must pass Gatekeeper without being bypassed (https://docs.brew.sh/Acceptable-Casks), which means a Developer ID and notarization. Ad-hoc signed, Sunlit Earth is not eligible, whatever its popularity.

## 4. Linux alternatives to Homebrew

A second web research pass on 2026-09-24 looked for a cross-distro, user-level tool on Linux where a developer publishes without central approval and users install and upgrade with one command. Snap, Flatpak, AUR and distribution repositories were excluded by decision, not evaluated. Flatpak and Snap would also conflict with the setter, which runs host programs (`gsettings`, `dbus-send`, `xfconf-query`) and deliberately avoids the portal (`platforms.md`).

The finding: no tool on Linux fills the role Scoop and Homebrew fill for end-user GUI apps. Every candidate's adoption is command-line developer tools; the research found no example of a desktop app shipped through any of them as its main route.

The closest fits, each for different reasons:

- **mise**, `github:` backend (about 34,000 stars, the most used of the group). It installs straight from a repository's GitHub releases with no manifest at all: `mise use -g github:sunlit-earth/sunlit-earth`, and `mise upgrade` updates everything. Archives are extracted whole into the install directory, a single top-level directory is stripped automatically, and prereleases are excluded unless `prerelease = true`, all verified from https://mise.jdx.dev/dev-tools/backends/github.html. So `textures/` stays beside the binary. That page documents no post-install step, so installing the desktop entry through mise is unverified. mise is a version manager for development tools, which is not what an end user reaches for to install a wallpaper app.
- **soar**, pkgforge (about 870 stars, young). Verified from https://github.com/pkgforge/soar: it is not tied to its default repository ("Add a third-party one, or run your own"), `soar update` updates everything, and desktop entries and icons are installed "where your system already looks". It targets self-contained formats (static binaries, AppImages and similar), so whether our tarball with a sibling `textures/` fits its metadata model is unverified. An independent review says few projects use it as their main distribution channel (https://itsfoss.com/pkgforge/).
- **Nix**, a flake in our own repository. It keeps a directory tree intact and upgrades cleanly, but a prebuilt GPU application on a distribution other than NixOS commonly fails to load the host's OpenGL and Vulkan drivers and needs nixGL wrapped around every launch (https://github.com/nix-community/nixGL, https://wiki.nixos.org/wiki/Packaging/Binaries). For a wgpu renderer that is disqualifying without further work.
- **Homebrew on Linux** stays the baseline: a tap needs no approval, a formula keeps the layout, `brew upgrade` updates everything, and Linux ARM64 is Tier 1 since 5.0.0 (section 3.3). It has no desktop entry mechanism, hence the plan's caveat.

Ruled out:

- pkgx: one central pantry, no third-party repositories (https://github.com/pkgxdev/pkgx/issues/1124).
- webinstall.dev: pull requests only, and by its own FAQ "not a package manager".
- eget, bin, stew and cargo-binstall: each installs one binary and drops the files beside it (eget, bin and stew by their docs and issues, cargo-binstall into `~/.cargo/bin`).
- ubi: its `--extract-all` keeps the whole archive, but it has no update-everything command.
- aqua: needs a per-user `aqua policy allow` for any registry but its own, and has no post-install step.
- Zero Install: its desktop integration is documented as Windows only (https://docs.0install.net/details/desktop-integration/).
- AM/AppMan: a curated database built around AppImages.
- huber: stale, and its self-hosting is undocumented.

Star counts are from the GitHub API on the day of the pass.

## 5. What this suggests

These are recommendations for the plan, not decisions.

1. Two repositories in the `sunlit-earth` organization: `scoop-bucket` holding `bucket/sunlit-earth.json`, and `homebrew-tap` holding `Formula/sunlit-earth.rb`. Users run `scoop bucket add sunlit-earth https://github.com/sunlit-earth/scoop-bucket` and `brew install sunlit-earth/tap/sunlit-earth`.
2. The Homebrew formula covers macOS and Linux from the plain tarballs, installed into `libexec` behind `write_exec_script`. On macOS that avoids quarantine entirely, matches what `usage.md` already recommends to testers, and is the same binary as the `.app`'s.
3. A cask for the `.app` is optional and worth adding only if a Launchpad entry matters more than the Gatekeeper dialog, or once the project has a Developer ID. Stripping quarantine in `postflight_steps` would work but is the circumvention Homebrew moved against; that call is the user's.
4. Update both from `release.yml`'s `publish` job: it knows the version and the archives, so it can write both manifests with their hashes and push them with one token scoped to the two repositories. That avoids the prerelease blind spot of `checkver: github` and the polling delay. It depends on the tag trigger coming back, which is its own decision. Whether prereleases go into the same manifest or into `sunlit-earth-beta` files is a choice to make in the plan.
5. Things to prove once, before announcing either: a Scoop install in the Windows guest (shortcut, shim output from `sunlit-earth displays`, no SmartScreen, `scoop update` while the tray app runs), and a Homebrew install on the Linux guest and on the hosted macOS runner (textures found through the wrapper, the binary starts, `brew uninstall` leaves the data directory).
6. A future autostart entry should point at the stable paths, Scoop's `current` junction and Homebrew's `opt/sunlit-earth`, not the versioned directories, or it breaks on the first upgrade.
