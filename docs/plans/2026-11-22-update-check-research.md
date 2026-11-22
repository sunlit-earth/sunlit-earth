# Research: Checking for updates

## Question

The roadmap has Help > Check for Updates… open, and the web hosting plan sketched an update check as its phase 3: a `latest.json` on `api.sunlit.earth` that the app reads once a day. This document collects what the app and the website already have, which feed formats exist, where the file can live if it is to be a static file on the main domain, how the app can tell how it was installed, and how much of the update itself the app can take off the user. The plan is [2026-11-22-update-check-plan.md](2026-11-22-update-check-plan.md).

The code was read on `feature/dependency-upgrade` at `cef3f61` (Slint 1.19.0, ureq 3.5.2, Rust 1.100.0), the website in `sunlit-earth/website` at `bfa0b29`. The format and hosting facts were read from the primary sources listed at the end on 2026-11-22. Nothing was run except the two checks on the development desktop that say so. What could not be confirmed is collected under Open questions.

## What the app has today

- No update check, no menu entry for one and no setting. The roadmap entry is the only trace, and `docs/platforms.md` already says the entry has no menu role and goes in Help on every platform.
- One HTTP client, `ureq` 3 with rustls and bundled webpki roots, used only by `sunlit-core`'s `assets/cloud_source.rs`. `HttpCloudSource` builds one agent with the user agent `sunlit.earth/<version>` and a two minute timeout, asks with `HEAD` and `If-None-Match`, and downloads with `GET`. Errors are strings. Only `sunlit-core` depends on `ureq`, `serde` and `toml`; `sunlit-app` reaches them through it. `serde_json` is a workspace dependency that only the xtask uses, and no `semver` crate is a direct dependency.
- The cloud map is polled once an hour by default from a third party host, so the app already makes background requests that carry its version. `clouds_enabled` turns the clouds off; there is no general network switch.
- The cloud fetcher keeps its ETag in a TOML sidecar beside the cached image, written through `files::write_toml`, which writes a `~` file and renames it. `files.rs` is private to `sunlit-core`. `cloud_fetcher::cache_dir()` is the folder for such files: `SUNLIT_EARTH_CACHE_DIR` when set, the app data folder otherwise.
- `engine::clock::Clock` (`elapsed`, `now_utc`) with `SystemClock` and `MockClock` is public; `engine::schedule::Schedule` is private to the engine.
- `AppConfig` is `#[serde(default)]`, so a new field with a default needs no migration step. Saving from the window goes through `read_config_from_window`, which starts from the stored file, so a field the window does not carry survives a save.
- Secondary windows: `AboutWindow` and `OlderVersionWindow` are `Window` components of their own, created lazily and reused, with no position set from Rust. `window_state::register_other_window` keeps the macOS Dock icon up while one is open. Slint 1.19 has no modal windows.
- The Help menu is Mouse and Keyboard, Website, Report an Issue…, Open Data Folder, About Sunlit Earth. The tray menu is Show / Hide, Refresh Now, Auto-refresh, Start on login, Website, About, Quit, and its first title already follows a property.
- `open::url` opens only what `is_web_url` accepts: `http` or `https` and nothing outside ASCII letters, digits and `-._~:/?#[]@+='`. A unit test walks every `Links` entry through it.
- Background threads reach the UI with `slint::invoke_from_event_loop` and a `Weak` handle; modules that need the tray from elsewhere keep a thread-local weak handle to it (`autostart::ui`).
- The e2e harness starts the app with `SUNLIT_EARTH_NO_CLOUDS` unless a case asks for clouds, and talks to it through one-line IPC commands answered by `SIGNAL:` lines on stdout. `tests/common/cloud_stub.rs` is a loopback HTTP stub.
- The version is `env!("CARGO_PKG_VERSION")`, today `0.6.0-beta.1`, so prerelease versions are in use.
- `CHANGELOG.md` is hand-written for users; the GitHub release bodies are generated lists of pull requests.

## What the website has today

- A single page built with Zola 0.23.6 and served by a Cloudflare Worker with static assets and no script, on `sunlit.earth`. `deploy.yml` runs on `ubuntu-latest` on a push to `main`, on `workflow_dispatch` and on a `repository_dispatch` of type `release-published`.
- `scripts/fetch-releases.sh` runs before every build and writes `data/releases.json` from the GitHub API: the published stable releases, newest first, each with its version, date, release page and seven archives (`windows-x86_64`, `linux-x86_64`, `linux-aarch64`, and `macos-<arch>-app` and `macos-<arch>-terminal` for both architectures) with name, URL and a size text. It fails the build when the newest release lacks one of the seven. Prereleases are dropped.
- `changes.toml` holds hand-written highlights per version, a few short lines with one emoji each, for the page's "What's new" section. The template looks an entry up by version, so an entry for a version that is not published yet is ignored.
- `static/` is copied to the site root as it is. `static/_headers` sets security headers on `/*` and a year of immutable caching on three paths; everything else keeps the default.
- The dispatch that `deploy.yml` listens for is not sent by anything yet (open item 1 of the hosting plan), so a release reaches the site only after a push or a manual run.
- The 0.5.0 archives are between 33 and 38 MB each.

## Where the file can live

The hosting plan put `latest.json` on a second Worker at `api.sunlit.earth`. A file on the main domain needs no second Worker:

- Workers static assets answer with `Cache-Control: public, max-age=0, must-revalidate` and an `ETag` that is a hash of the file. The hosting research tested on 2026-10-22 that a matching `If-None-Match` gets a 304 there. Cloudflare's own page describes the ETag and `If-None-Match` but does not name the 304.
- Static asset requests do not invoke a Worker, so the free plan's daily request limit does not apply to them (hosting plan, decision 4).
- Zola cannot write the file itself. `load_data` reads TOML and JSON, but the only templates it renders to files that are not HTML are the feeds, the sitemap and `robots.txt`, and a page whose path ends in `.json` becomes a folder with an `index.html` in it. A script has to write the file into `static/` before `zola build`.
- `jq` cannot read `changes.toml`. The `ubuntu-24.04` runner image carries Python 3.12.3, whose standard library has `tomllib` since 3.11, and `yq` 4.54.3. `tomllib` is the one whose TOML support is documented as complete.
- A URL compiled into a release has to stay served for as long as anyone runs that release (hosting plan, decision 5), so the path carries a format version.

Counting the checks (hosting plan, decision 7) is not affected by the move: the daily query would filter on the path instead of the host name, and still depends on that plan's step 2, which has no recorded result.

## Feed formats

| Format | What it is | Why it does not fit |
|---|---|---|
| Sparkle appcast | RSS 2.0 with a `sparkle:` namespace; `sparkle:version` and `sparkle:shortVersionString` per item, release notes as HTML in `description` or behind `sparkle:releaseNotesLink`, `sparkle:os` on the enclosure, `sparkle:channel`, and an EdDSA signature that is mandatory. WinSparkle reads the same format | It is the format of two updater libraries the app does not use. It needs an XML parser, its notes are HTML, which Slint cannot lay out, and Sparkle's own documentation advises against mixing operating systems in one feed |
| Atom or RSS | GitHub already serves `releases.atom` for every repository | The same parser, the version buried in a title, the notes as HTML, and no assets by platform. Feed readers already have GitHub's |
| Tauri's updater JSON | One object with `version`, `notes`, `pub_date` and `platforms`, each platform with a `url` and a `signature` that cannot be left out | It describes the newest release only, so it cannot say what changed across several versions, and it exists to feed a signed download |
| The GitHub API | `releases/latest` | 60 requests an hour for each IP address without a token, a large answer, and every installed copy tied to GitHub's API. Nothing could be counted |
| A JSON file of our own | A list of releases with what the app needs | Chosen; see the plan |

`semver` 1.0.28 orders versions by SemVer precedence: a prerelease sorts below its release, numeric identifiers compare as numbers, and `1.0.0-beta.2 < 1.0.0-beta.11 < 1.0.0-rc.1 < 1.0.0`. `Version::cmp_precedence` ignores build metadata.

## How the app was installed

Four ways to install exist, and three of them can be told apart:

- **Scoop.** The manifest extracts the bundle folder, so the executable sits directly in `<root>\apps\sunlit-earth\<version>\`, reached through the `current` junction. Scoop writes `install.json` and `manifest.json` into that folder; both were seen in an installed app's folder on the development desktop. The root is `~\scoop` by default and can be anywhere, so the two files and the two folder names above them are the marker, not the word `scoop`.
- **Homebrew formula (Linux).** The formula installs everything under `libexec` and links the binary, so the running executable is `<prefix>/Cellar/sunlit-earth/<version>/libexec/sunlit-earth`. `autostart::linux::homebrew_alias` already recognizes that path.
- **Homebrew cask (macOS).** The cask moves `Sunlit Earth.app` to the applications folder, links the binary and strips the quarantine attribute. The executable's path is therefore the same as after a manual install. What a cask leaves behind is `<prefix>/Caskroom/sunlit-earth/`, with `/opt/homebrew` and `/usr/local` as the two default prefixes. This is Homebrew's convention as documented, not seen on a Mac.
- **An archive unpacked by hand.** Everything else.

What each package manager needs from the user, read in Scoop's source on the development desktop (`libexec/scoop-update.ps1`, `lib/core.ps1`): `scoop update <app>` refreshes the buckets first only when the last refresh is three hours old or older, and skips an app with a running process ("Running process detected, skip updating"), which is what the manifest's own note says.

## How much of the update the app can do

1. **Open the right archive in the browser.** The app knows its OS, its architecture and, on macOS, whether it runs from a bundle, which is exactly the key of one of the seven archives. The browser brings progress, resuming and the downloads folder. The user still quits, unpacks and replaces.
2. **Download inside the app and show the file.** Progress, cancelling, partial files and a checksum become the app's job, and the user still installs by hand. More code for the same amount of work on the user's side.
3. **Update in place.** Download, verify, replace the installed files, restart. This is the only step that takes the work away, and it changes what the feed is trusted with: the app would run what it downloaded, so the archives need a signature checked against a key compiled into the app, which is why Sparkle and Tauri both refuse to work without one. Replacing the files differs on each platform (a running executable on Windows can be renamed but not overwritten, an app bundle on macOS, a folder that may not be writable on Linux), and the macOS half cannot be tested without a Mac.

With levels 1 and 2 the app executes nothing it downloaded. If it opens only URLs under the project's own release downloads, a tampered feed can show wrong text and nothing more.

## Slint 1.19

- The winit backend diffs the native menu instead of rebuilding it (slint-ui/slint#13119), which is why the View menu's checks are back. That the flicker stays gone on the Windows desktop is still to be confirmed (step 9 of the dependency upgrade plan), and `main.slint` still carries the comment that no menu property may follow the export sheet.
- The tray menu is rebuilt when a property it reads changes; Show / Hide relies on it.
- `TextInput` has `select-all()` and `copy()`, and `LineEdit` exposes both as public functions (`i-slint-compiler` 1.19.0, `builtin_elements.rs` and `widgets/common/lineedit-base.slint`), so a button can put a command on the clipboard without any Rust. Slint's Rust API has no clipboard function for application code.
- `StyledText` renders inline markdown and cannot lay out a document; the About window's tabs work around that block by block.

## The macOS version keys

The roadmap leaves the format of `CFBundleShortVersionString` and `CFBundleVersion` to this plan, "since updaters on macOS compare them". Apple documents the first as three period-separated integers and the second as one to three, both with digits and periods only. The bundle template writes the workspace version into both, so a prerelease puts `-beta.1` there. An update check that compares `CARGO_PKG_VERSION` with a feed never reads either key.

## Recommendation

A JSON file of our own on the main domain under a path that names its format version, listing the stable releases with their highlights and archives; a check once a day in a thread of its own in `sunlit-core`, beside the engine and not in it; a small window that shows what changed since the installed version and then either the package manager's command or a button that opens the right archive; and updating in place left for a later plan, with room for it in the format.

## Open questions

- The Caskroom marker and what `brew upgrade --cask` does to a running app have not been seen on a Mac.
- Whether `brew upgrade` refreshes a third party tap before it compares versions was not looked up. The window's hint to run `brew update` first costs nothing either way.
- Whether a `.json` static asset is served as `application/json`, and the 304 for this file, are to be seen on the deployed file.
- Whether the testing backend has a clipboard a test could read after `copy()`.
- The archives' top-level layout, which the install steps in the window have to describe, is to be read from the release README files.

## Sources

- [Sparkle, publishing an update](https://sparkle-project.org/documentation/publishing/)
- [WinSparkle, publishing updates](https://winsparkle.org/guides/publishing-updates/)
- [Tauri v2, the updater plugin](https://v2.tauri.app/plugin/updater/)
- [GitHub, rate limits for the REST API](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api)
- [GitHub, create a repository dispatch event](https://docs.github.com/en/rest/repos/repos#create-a-repository-dispatch-event) and [permissions required for GitHub Apps](https://docs.github.com/en/rest/authentication/permissions-required-for-github-apps)
- [semver, `Version`](https://docs.rs/semver/latest/semver/struct.Version.html)
- [The `ubuntu-24.04` runner image](https://github.com/actions/runner-images/blob/main/images/ubuntu/Ubuntu2404-Readme.md)
- [Python, `tomllib`](https://docs.python.org/3/library/tomllib.html)
- [Zola, templates overview](https://www.getzola.org/documentation/templates/overview/) and [feeds](https://www.getzola.org/documentation/templates/feeds/)
- [Cloudflare, headers for Workers static assets](https://developers.cloudflare.com/workers/static-assets/headers/)
- [Apple, `CFBundleShortVersionString`](https://developer.apple.com/documentation/bundleresources/information-property-list/cfbundleshortversionstring) and [`CFBundleVersion`](https://developer.apple.com/documentation/bundleresources/information-property-list/cfbundleversion)
