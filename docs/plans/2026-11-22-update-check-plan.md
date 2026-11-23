# Checking for updates

## Problem

Nothing tells a user that a newer version exists. The project is in public beta with breaking changes still expected, the app sits in the tray for weeks at a time, and the only way to learn of a release is to look at the website or at GitHub. Scoop and Homebrew users have to remember to ask their package manager, and everybody else has to find the right one of seven archives.

The research is in [2026-11-22-update-check-research.md](2026-11-22-update-check-research.md). This plan replaces phase 3 of [2026-10-22-web-hosting-plan.md](2026-10-22-web-hosting-plan.md) and takes over that plan's open item 1, the release dispatch. It builds on `feature/dependency-upgrade` (Slint 1.19).

## What exists today

- No update check, no menu entry and no setting. `docs/roadmap.md` has Check for Updates… open, in Help above About.
- `sunlit-core` has the only HTTP client (`ureq` 3 with rustls), `files::write_toml`, `cloud_fetcher::cache_dir()`, `env_override` and the injectable `Clock`. `sunlit-app` has `open::url` with its URL check, the `Links` global, the pattern for a secondary window (`about.rs`, `older-version.slint`), a tray menu whose first title follows a property, and IPC commands answered by `SIGNAL:` lines.
- The website's build already lists every stable release with its seven archives (`scripts/fetch-releases.sh`) and has hand-written highlights per version (`changes.toml`). Its deploy workflow listens for a `release-published` dispatch that nothing sends.
- `autostart::linux::homebrew_alias` recognizes the Homebrew Cellar path; nothing recognizes a Scoop or a cask install.

## Decisions

Decisions 5, 8, 9 and 10 were settled with the user on 2026-11-22, as was the file's place on the main domain. Decision 3 is this plan's proposal and waits for the user's word.

### 1. One static file on the main domain

`https://sunlit.earth/releases/v1.json`, served by the site's own Worker like every other static file, with the default headers: an `ETag` and `Cache-Control: public, max-age=0, must-revalidate`, so a daily check is usually a 304. The `v1` is the format's version: a format that an older release could not read gets a new file beside this one, and this one stays served for as long as anyone runs a release that asks for it.

`api.sunlit.earth` is not needed and is dropped from the hosting plan.

### 2. The format

```json
{
  "releases": [
    {
      "version": "0.6.0",
      "date": "2026-12-01",
      "notes": "https://github.com/sunlit-earth/sunlit-earth/releases/tag/v0.6.0",
      "highlights": [
        "Export the current view as an image",
        "A menu bar, and full screen with F11"
      ],
      "assets": {
        "windows-x86_64": {
          "url": "https://github.com/sunlit-earth/sunlit-earth/releases/download/v0.6.0/sunlit-earth-0.6.0-windows-x86_64.zip",
          "size": 35800000
        }
      }
    }
  ]
}
```

- `releases` is newest first and holds at most the 20 newest. `version` is a SemVer version without a leading `v`. `date` is the day of publication. `notes` is the release page. `highlights` are plain text, one line each, and may be empty or missing. `assets` is keyed as the website's data already is: `windows-x86_64`, `linux-x86_64`, `linux-aarch64`, `macos-aarch64-app`, `macos-x86_64-app`, `macos-aarch64-terminal`, `macos-x86_64-terminal`; `size` is in bytes.
- Every release carries its `assets`, although the app reads only those of the version on offer. An entry without them is still an ordinary case for the app (decision 10).
- An asset is an object so that a later release can add a checksum or a signature to it (decision 15).
- Size, measured on the seven releases published so far and written without whitespace: the seven archives of one release take 1.1 KB, and everything else in an entry about 0.3 KB with three highlights. The file is about 9 KB today and comes to about 25 KB at the cap of 20 releases, which the user accepted on 2026-11-22; with archives on the newest release only it would be about 8 KB at the cap. The size matters only on the day the file changes, since every other check is a 304 without a body, and `ureq` is built without its gzip feature, so the body travels as it is.

The rules every release of the app follows from the first one, which are what lets the file grow without a `v2`:

1. Unknown fields are ignored, at every level.
2. An entry that cannot be read (no version, a version that does not parse) is skipped, and the rest of the file still counts.
3. A stable build ignores every entry whose version has a prerelease part. A prerelease build considers all entries.
4. The version on offer is the greatest considered version above the app's own, by `semver`'s `cmp_precedence`. With none, the app is up to date.
5. What changed is every considered entry above the app's own version up to the one on offer, newest first.
6. A `notes` URL is used only when it starts with `https://github.com/sunlit-earth/sunlit-earth/releases/` and an asset URL only when it starts with `https://github.com/sunlit-earth/sunlit-earth/releases/download/`, and both only when `open::is_web_url` accepts them. What follows the prefix must be a plain path, so the prefix cannot be climbed out of: every segment is ASCII letters, digits, `.`, `_`, `-` and `+`, and none is empty, `.` or `..`, which also leaves out percent escapes, backslashes, a query, a fragment and userinfo. Anything else is treated as missing. With that, a tampered file can show wrong text and cannot send anyone anywhere else. *Dropped on 2026-11-23:* the prefixes and the plain path rule are gone, and a `notes` or asset URL on any host is kept, with `open::is_web_url` still applied in the app; see [2026-11-23-feed-urls-selectable-text-guides-plan.md](2026-11-23-feed-urls-selectable-text-guides-plan.md).
7. A body over 256 KiB is a failed check.

### 3. Stable releases only, for now

The file lists stable releases. A prerelease build is offered the next stable release, which rule 4 already gives (`0.6.0-beta.1 < 0.6.0`). Listing prereleases later needs no change in any released app, because rule 3 is in from the start; it needs the website's script to keep them and the dispatch to fire for them, which is what the hosting plan would have had to do anyway.

### 4. The website writes the file, and a release triggers it

- `scripts/fetch-releases.sh` keeps writing `data/releases.json` and gains each archive's size in bytes beside the size text.
- A new `scripts/write-update-feed.py`, standard library only (`tomllib`, `json`), reads `data/releases.json` and `changes.toml` and writes `static/releases/v1.json`, which Zola copies to the site. The emoji of `changes.toml` stay on the website; the file carries the text, without whitespace between its tokens. It refuses to write when a version is not `X.Y.Z`, when a URL does not carry the prefix of rule 6, or when the newest release lacks one of the seven archives. `static/releases/` is ignored by git like `data/`.
- `deploy.yml` runs it with `python3` between the fetch and `zola build`. Locally it is `uv run scripts/write-update-feed.py`.
- The dispatch is sent by `package-managers.yml` in the app's repository, as a last step of its `push` job, after the Scoop bucket and the Homebrew tap have been pushed: the app must not show a `scoop update` or `brew upgrade` command before the package manager has the release. The step uses the packaging app's token with `website` added to its `repositories`, which needs the app installed on that repository with write access to contents, a change the user makes on GitHub.
- The highlights of a release are added to `changes.toml` before the draft release is published. An entry for a version that is not published yet is ignored by the page and by the script, so it can be pushed ahead of time. A release without an entry is listed without highlights, and the app shows it that way.

### 5. When the app checks

- Automatically, on by default. `AppConfig` gains `update_check_enabled`, `true` by default, with no control in the UI until the settings window exists; until then it is a line in `config.toml`. It is read at startup.
- The first check runs 30 seconds after the app starts, if the last successful check is 24 hours old or older, was never made, or lies in the future. After that the check runs every 24 hours for as long as the app runs. The age is measured on the wall clock from a time kept on disk, so a restart does not check again and a laptop that slept for three days checks soon after it wakes.
- A check is one `GET` with `If-None-Match` when an ETag is known. A 200 replaces the kept copy of the file and the ETag; a 304 reads the kept copy.
- A scheduled check that fails is logged and says nothing to the user. The next attempt is an hour later, and the hour is not kept across a restart.
- Help > Check for Updates… checks at once whatever the schedule and the setting say, and shows a failure with its reason.
- With `update_check_enabled` off, nothing is asked on a schedule, and what an earlier check found is still shown.

### 6. What is sent

One request for one static file, with the user agent `sunlit.earth/<version> (<os>; <arch>)`, the form decision 7 of the hosting plan chose for counting. No identifier, no cookie, nothing about the installation. `docs/usage.md` says so beside the setting. Whether and how the requests are counted stays with the hosting plan.

### 7. What is kept on disk

Two files in `cloud_fetcher::cache_dir()`, the folder the cloud cache and the tile packs already use, so `SUNLIT_EARTH_CACHE_DIR` moves them with the rest:

- `update_feed.json`, the last body received, as it came.
- `update_state.toml`: `etag`, `last_checked` (UTC, RFC 3339), `skipped_version` and `dismissed_version`.

Only the update thread writes them. At startup it reads the kept file and reports what it says before any request, so the tray shows a known update the moment the app is up. A kept ETag without a readable kept file is dropped, and the next check is unconditional. After the app itself has been updated, the same kept file compares as up to date.

### 8. Where the user sees it

- **Help > Check for Updates…**, above About Sunlit Earth and below the separator, with a fixed title and no shortcut. It opens the update window and checks. The entry never follows a property, so nothing here depends on how Slint updates the native menu.
- **The tray menu** gains one entry above Website. It reads "Check for Updates…" and, while a version is on offer that the user has not skipped, "Update Available: 0.7.0…". Either way it opens the update window; with nothing on offer it also checks. The entry is always there and only its title changes, which is what Show / Hide already relies on in all three tray backends.
- **A banner in the main window**, at the foot of the panel above the About button: "Version 0.7.0 is available", a Details button and a close button. It shows while a version is on offer that is neither skipped nor dismissed. Details opens the update window. Opening the update window by any route, or the close button, dismisses the banner for that version, and the dismissal is kept across restarts. This is the one passive sign in a session without a tray.
- **No notification from the operating system**, and nothing in the About window.

Skip This Version in the update window turns the tray title and the banner off for that version and nothing else: Help > Check for Updates… still shows it, and a later version is offered as usual.

### 9. The update window

A window of its own like the About window: `UpdateWindow` in `update.slint`, created on first use and reused, closed with Escape, registered with `window_state::register_other_window`. It is 480 px wide and has four states.

- **Checking.** "Checking for updates…". When something is already known from the kept file, that is shown underneath meanwhile.
- **Up to date.** "Sunlit Earth 0.6.0 is the latest version." Check Again and Close.
- **Failed.** "Could not check for updates." with the reason on a second line. Try Again and Close.
- **Available.** "Sunlit Earth 0.7.0 is available. You have 0.6.0." Below it, in a scrolling list, every release since the installed one, newest first (rule 5): the version and its date as a heading ("0.7.0, December 1, 2026"), then its highlights as plain lines, or nothing but the heading for a release without any. A Release Notes link opens the newest release's page. Then how to update (decision 10), and Skip This Version and Close.

### 10. Helping with the update

What the window offers depends on how the app was installed (decision 11).

| Installed with | The window shows |
|---|---|
| Scoop | "Quit Sunlit Earth, then run:" and `scoop update sunlit-earth` in a field with a Copy button. Below it: "If Scoop reports the old version as current, run `scoop update` first." |
| Homebrew, formula (Linux) | The same with `brew upgrade sunlit-earth` and `brew update` |
| Homebrew, cask (macOS) | The same with `brew upgrade --cask sunlit-earth` and `brew update` |
| An archive | A Download button, "Download 0.7.0 (35.8 MB)", that opens the archive for this OS, architecture and kind of macOS build in the browser, and two lines on what to do with it |

- Quitting first is asked of all three package managers. Scoop skips an app that is running; for Homebrew it is the safe order, not something that was tested.
- The command sits in a read-only `LineEdit`, and Copy calls its `select-all()` and `copy()`, so the clipboard needs no Rust and no new dependency.
- The archive's key is `windows-x86_64` on Windows, `linux-<arch>` on Linux, and on macOS `macos-<arch>-app` when the executable is inside a `.app` bundle and `macos-<arch>-terminal` otherwise. With no usable asset for the key (an architecture without an archive, or a URL rule 6 refuses), the button reads "Open Download Page" and opens `https://sunlit.earth/#download`, a new entry in `Links`.
- The two lines for an archive say to quit Sunlit Earth, unpack the archive and replace the old folder (on macOS, the old app) with the new one, and that the settings are kept. The exact words follow each platform's release README and are written in step 5.

The app downloads nothing itself and runs nothing it did not ship with.

### 11. Telling how the app was installed

`install::kind()` in the app looks at `std::env::current_exe()` and answers with one of four:

- **Scoop**, on Windows: the executable's folder holds `install.json` and `manifest.json`, the folder above it is named `sunlit-earth`, and the one above that `apps`. The word `scoop` is not looked for, since the root can be anywhere.
- **Homebrew formula**, on Linux: `autostart::linux::homebrew_alias` answers for the path.
- **Homebrew cask**, on macOS: the executable is inside a `.app` bundle and `/opt/homebrew/Caskroom/sunlit-earth` or `/usr/local/Caskroom/sunlit-earth` is a folder.
- **Archive**: everything else.

The function takes the path and a "does this exist" closure, so each rule is tested on every platform against made-up trees. A wrong answer costs little: the user sees a command where a button would have fit, or the other way round, and the Release Notes link leads to the archives in both cases. The cask rule is Homebrew's convention read from its documentation and has not been seen on a Mac.

### 12. Where the code lives

- **`sunlit_core::update`**, headless, no Slint: `feed` (the format and rules 1 to 7), `state` (the two files), `source` (the `UpdateSource` trait with `fetch(known_etag) -> Result<Fetched, String>`, `Fetched` being `Unchanged` or a body with its ETag, and `HttpUpdateSource` on a `ureq` agent of its own with a 30 second timeout), and `checker` (when a check is due and what one does, on an injected `Clock` and `UpdateSource`). `spawn` starts the thread `sunlit-update` and returns an `UpdateHandle`.
- The thread waits on its request channel with a one minute timeout and asks the `Clock` whether a check is due each time it wakes; it does not sleep on wall time. Requests are `CheckNow`, `Skip(version)` and `Dismiss(version)` on a bounded channel of 8, sent with `try_send`: they are clicks, a handful in a session, and a full channel drops the click with a warning. A `CheckNow` that arrives while a check runs is answered by that check. Results go to one callback, `Fn(UpdateEvent) + Send`, whose consumer is the app's controller, and the thread runs for as long as the app does, with scheduled checks on or off, because the manual check uses it.
- Dropping the handle ends the thread at its next wake; a request in flight is left to its timeout and the thread is not joined, as with the cloud worker.
- **The engine is not touched.** The check has no part in rendering, and the headless `render` and `displays` subcommands do not start it.
- **`sunlit-app`**: `update.rs` with `UpdateController` (the latest status, the window, weak handles to the main window and the tray, the `UpdateHandle`), reached from the menu, the tray and IPC through a thread-local as `autostart::ui` is; `install.rs` with decision 11; and the wiring in `app.rs` after `init_ui`, with the handle dropped in `tear_down`. The callback crosses to the UI thread with `slint::invoke_from_event_loop`.
- New dependencies: `semver = "1"` in the workspace and in `sunlit-core`, and `serde_json` in `sunlit-core`. Both are in `Cargo.lock` already as dependencies of other crates.

### 13. Switches for tests

- `SUNLIT_EARTH_UPDATE_URL` replaces the file's URL, through `env_override`. An `http` URL is accepted, which is what a loopback stub serves.
- `SUNLIT_EARTH_NO_UPDATE_CHECK`, presence only like `SUNLIT_EARTH_NO_CLOUDS`, turns scheduled checks off whatever the config says. The e2e harness sets it for every case that does not ask for a feed.
- IPC `check-update` asks for a check at once, and every answer to a manual check prints `SIGNAL:update state=<up_to_date|available|failed> version=<version or ->`.

### 14. The macOS version keys

`CFBundleShortVersionString` and `CFBundleVersion` take the workspace version without its prerelease part, so `0.6.0-beta.1` writes `0.6.0` into both. Apple allows digits and periods only, and nothing compares the keys: the check compares the version compiled into the binary with the file. This closes the question the roadmap left to this plan.

### 15. Updating in place, later and tentative

Not part of this plan. A later one would have the app download the archive, verify it, replace the installed files and restart. It needs a signing key whose public half is compiled into the app and a signing step in `release.yml`, a way to replace the files on each platform, and somebody with a Mac. What this plan does for it is leave room: an asset is an object that can gain `sha256` and `signature`, and rule 1 lets today's releases ignore them.

### Not chosen

- **A file on `api.sunlit.earth`**, as the hosting plan had it: a second Worker and host name for one file the site's own Worker can serve.
- **A Sparkle appcast, an Atom feed, Tauri's updater format or the GitHub API**; the research has the table.
- **Checking only when asked.** Few people ever open that menu entry.
- **Asking at first launch whether to check.** A question before the first globe, for a request that tells the server no more than the hourly cloud download already tells a third party.
- **Only the newest release's notes.** Wrong for anyone more than one version behind.
- **The full changelog as markdown.** Slint's `StyledText` cannot lay a document out, and the About window's block parser would be needed a second time for text that the website already has in a shorter form.
- **Building the archive's URL in the app from the naming scheme.** A release that renames its archives would break every older copy.
- **Downloading inside the app** (level 2 in the research): progress, cancelling and checksums for a file the user then has to unpack by hand anyway.
- **Running the package manager from the app.** The app has to quit before Scoop will update it, so it would need a helper that outlives it and a terminal to show the output in.
- **A system notification, a changing tooltip on the tray icon, and a line in the About window.** One more surface each; the tray title and the banner are enough until somebody misses an update.
- **A conditional tray entry** that exists only while an update is on offer. An entry that changes its title relies on what the tray backends are already known to do.
- **`yq` for reading `changes.toml`** in the website's script: its TOML reader is the less complete one, and Python is on the runner.

## Implementation steps

Steps 1 and 2 are in `sunlit-earth/website`, the rest in this repository.

0. **Checks before any code.**
   - The runner: `ubuntu-latest` has Python 3.11 or newer (the `ubuntu-24.04` image lists 3.12.3).
   - The archives' top-level layout and what the release README files tell a user to do, for the words of decision 10.
   - Whether the testing backend lets a test read the clipboard after `LineEdit.copy()`.
1. **The website's script.** `fetch-releases.sh` adds `bytes` to each file. `write-update-feed.py` as in decision 4, with the 20 release cap. `.gitignore` gains `static/releases/`. The website's README describes the file and the script.
2. **The website's deploy.** `deploy.yml` runs the script before `zola build`. After the deploy: `curl -sI https://sunlit.earth/releases/v1.json` shows `application/json` and an `ETag`, and a second request with `If-None-Match` gets a 304. If the content type is something else, a rule for `/releases/*` in `static/_headers` sets it.
3. **The format in core.** `update/feed.rs`: the serde types, lenient reading (rules 1, 2, 7), the choice (rules 3 to 5), the URL prefixes (rule 6, with the `is_web_url` half applied in the app, where that function lives), and `asset_key(os, arch, bundled)`. A fixture `crates/sunlit-core/tests/fixtures/update_feed_v1.json` is the format's reference copy. Add `semver` and `serde_json`.
4. **State, source, checker, thread.** `update/state.rs` on `files::write_toml` and `cache_dir()`; `update/source.rs`; `update/checker.rs` with the constants (24 hours, 30 seconds, 1 hour, 1 minute, 30 seconds, 256 KiB) and the `resolve_*` and public wrapper pair for `SUNLIT_EARTH_UPDATE_URL`; `update::spawn`. `AppConfig::update_check_enabled` with its default.
5. **The install kind and the texts.** `install.rs` with decision 11, and the command, hint and archive steps of decision 10 as one function from kind and platform to text.
6. **The window.** `update.slint` with the four states, the release list as a model of version, date text and highlight lines, the command field with Copy, and the callbacks `check-again()`, `skip()`, `open-url(string)` and `dismiss()`. `Links.download`, added to the list the opener's test walks.
7. **The controller and the surfaces.** `update.rs`; the Help entry and `check-for-updates()` in `main.slint`; the tray's `update-version` property, its entry and `show-update()`; the banner in the panel with `update-banner-version`; the wiring in `app.rs`, `menu.rs` and `tray.rs`; IPC `check-update` and the signal.
8. **The harness.** `Spawn` sets `SUNLIT_EARTH_NO_UPDATE_CHECK` by default and gains `with_update_feed(url)`; `tests/common/feed_stub.rs` beside the cloud stub, answering 200 with an ETag and 304 to a matching `If-None-Match`.
9. **The dispatch.** In `package-managers.yml`, `website` in the token's `repositories` and a last step in `push` that posts `release-published` to `sunlit-earth/website`. Before it can work, the user installs the packaging app on the website repository.
10. **The version keys.** `bundle.rs` cuts the prerelease part before filling `@VERSION@`, and the plist test asserts it for a prerelease version.
11. **Licenses.** `cargo xtask bake licenses`, since the direct dependencies changed.
12. **Docs** (below).
13. **Checks on the platforms.**
    - Windows desktop, `cargo app` with `SUNLIT_EARTH_UPDATE_URL` on a local file server holding a feed with a higher version: the tray title changes without the preview flashing, the banner shows and stays dismissed after a restart, the window lists the releases, Download opens the browser on the archive, Skip This Version clears the tray title, and Help > Check for Updates… still offers the skipped version. Then from a Scoop install of a release build: the window shows the Scoop command, and Copy puts it on the clipboard.
    - Linux guest, KDE on X11 and GNOME on Wayland: the same through the drawn menu bar, the tray title in both tray hosts, and the archive button opening the browser.
    - macOS runner: the e2e case, and `install::kind()` printed for the bundle the runner built. The cask rule and the window itself are left for a tester with a Mac and noted in `docs/platforms.md` as to be observed.
    - The real file: a release build without the override reports up to date against the deployed `v1.json`.

## Tests

### Unit, `sunlit-core`

- The fixture reads, and so does a copy with an unknown field at each level.
- An entry without a version or with one that does not parse is skipped, and its neighbors still count.
- A stable build is not offered a prerelease, a prerelease build is offered a newer prerelease and a newer stable release, and `0.6.0-beta.1` is offered `0.6.0`.
- What changed runs from above the installed version to the version on offer, newest first, whatever the order in the file.
- A `notes` or asset URL outside its prefix is missing; a body over the limit fails the check.
- `asset_key` for each of the seven archives.
- The state file survives a round trip, a missing or broken one reads as empty, and an ETag without a kept body is dropped.
- With a `MockClock` and a recording source: no check before the startup delay; one check after it when the last one is a day old, never made, or in the future; none when it is an hour old; the next one 24 hours later; a failure tries again after an hour and leaves the last status in place; a 304 reads the kept body; `CheckNow` checks at once with the schedule off; a disabled schedule asks nothing.
- `Skip` and `Dismiss` reach the state file, and a newer version on offer is not affected by either.

### Unit, `sunlit-app`

- `install::kind` for a Scoop tree, a Scoop tree without its two files, a Cellar path, a bundle with and without a Caskroom folder, and a plain folder.
- The command and the hint for each package manager, and the archive steps for each platform.
- Every `Links` entry, `download` included, passes the opener's check.

### `slint_ui`

- Help > Check for Updates… reaches `check-for-updates`, as a row of the table in `test_each_menu_entry_reaches_its_command`.
- The tray's entry reads "Check for Updates…" with nothing on offer and names the version when one is, read from a title property since the testing backend shows no tray menu.
- The banner appears with a version, Details and the close button each hide it, and it stays hidden for that version.
- The update window in each state shows that state's texts and buttons; in the available state it lists the releases in order with their highlights, and one without highlights as a heading alone.
- Download reaches `open-url` with the asset's URL, and with no asset the button reads "Open Download Page" and reaches it with `Links.download`.
- For a package manager the command is in the field, and Copy leaves it on the clipboard if step 0 found a way to read that.

### e2e

One case, `test_update_check_over_http`, against the feed stub with `SUNLIT_EARTH_CACHE_DIR` in a scratch folder: `check-update` with a higher version in the feed answers `state=available` with that version; a second `check-update` makes the stub see `If-None-Match`, answer 304, and the app answer the same; a feed that lists only the running version answers `state=up_to_date`; and with the stub stopped, `state=failed`. Every other case keeps running with the check off.

## Docs

- `docs/architecture.md`: `sunlit_core::update` and its thread, the two files, `update.rs` and `install.rs` in the app's module list, `check-update` in the IPC list, the Help and tray entries, and the two variables in the environment table.
- `docs/usage.md`: the menu and tray entries, the update window, `update_check_enabled` and what a check sends, and the two files under the app data.
- `docs/platforms.md`: how each install is told apart, and what is still to be observed on macOS.
- `docs/testing.md`: the feed stub, the e2e case, the harness default, and the dispatch step of `package-managers.yml`.
- `docs/roadmap.md`: Check for Updates… done; open entries for updating in place (decision 15), prereleases in the file, and the control for `update_check_enabled` in the settings window. The sentence about the version keys goes with decision 14.
- `docs/plans/2026-10-22-web-hosting-plan.md`: a pointer to this plan in its summary, phase 3 and `api.sunlit.earth` marked as replaced by it, and open item 1 closed by step 9.
- `CLAUDE.md`: under "Making a release", the highlights go into the website's `changes.toml` before the draft is published.
- `README.md` and `CHANGELOG.md` wait for the user's permission.

## Gates

`cargo fmt --check`, `cargo test`, `cargo clippy --all-targets`, the e2e suite in the Windows and Linux guests, and step 13.

## Departures

1. **The Windows archive key names the architecture.** Decision 10 says the key is `windows-x86_64` on Windows; `asset_key` makes it `windows-<arch>`, as it does on Linux. Every Windows build the project ships is x86_64, so for those the two read the same. A build from source on an ARM64 Windows machine would have been offered the x86_64 archive, which replaces a native build with an emulated one; with the architecture in the key it finds no asset and the button opens the download page instead, which is the plan's own answer for an architecture without an archive.
2. **The macOS runner shows the e2e binary's install kind, not the bundle's.** Step 13 asks for `install::kind()` printed for the bundle the runner built. A release build logs nothing below warn, and `bundle --verify` only renders from the archive, so neither prints the answer, and adding a command or a warn line only for this would put a test hook into the shipped binary. Instead `Install::detect` logs the kind and the asset key at debug level, and `test_update_check_over_http` prints that line for the suite's own binary and asserts it is an archive; on the runner that binary sits outside a bundle, so it answers `macos-<arch>-terminal`. The bundle's answer, `macos-<arch>-app` and an archive unless a Caskroom folder exists, rests on the unit tests of `install::kind_of` against made-up trees and on a tester with a Mac, as the cask rule already did.
3. **The feed's sizes come from the new `bytes` field.** `write-update-feed.py` takes each asset's `size` from the `bytes` that step 1 adds to `data/releases.json`, and refuses a `data/releases.json` written before that change with "run scripts/fetch-releases.sh again". It follows from step 1 and is recorded so that the list of refusals in decision 4 is complete.
4. **The website's script refuses more than decision 4 names.** Beyond a version that is not `X.Y.Z`, a URL outside the prefixes of rule 6 and a newest release short of an archive, it also refuses a date that is not `YYYY-MM-DD` and an asset without an integer size in bytes, and it checks every release it writes, up to 20, rather than only the newest. The app trusts these fields, and a failed deploy is easier to notice and fix than a half valid file. Prereleases never reach the script, since `fetch-releases.sh` already drops drafts and prereleases, so the version rule cannot stop a deploy over a beta. Since 2026-11-23 the app keeps a URL on any host, so the script's prefix check has to go as well, or the first release hosted elsewhere fails the deploy; see [2026-11-23-feed-urls-selectable-text-guides-plan.md](2026-11-23-feed-urls-selectable-text-guides-plan.md).
5. **A release without highlights has no `highlights` key.** Decision 2 allows the key to be empty or missing; the script leaves it out, which the app reads the same way and which saves a few bytes per entry.
6. **The website's README says more than the plan asks.** Besides describing the file and the script, its "After a release" section now says to add the highlights to `changes.toml` before publishing the draft, matching step 4 of "Making a release" in `CLAUDE.md`, and its prerequisites name Python 3.11 for `tomllib`.
7. **The website has a token of its own.** Step 9 adds `website` to the token's `repositories`. `create-github-app-token` fails when the app is not installed on every repository it names, and that step runs before the bucket and the tap are pushed, so a website the packaging app cannot reach yet would have kept the release from both package managers. The `push` job keeps its token for `scoop-bucket` and `homebrew-tap` alone and mints a second one for `website` right before the dispatch, after both pushes; a failure there fails only the dispatch, and the job shows it.
8. **An archive gets one line: quit, unpack and replace.** Decision 10 had the lines for an archive mention that the settings are kept, and on Windows and in a macOS bundle add that the system may ask once more before the new version starts. On 2026-11-22, after trying the window, the user asked to drop the settings line, and later the same day the whole line on the system asking once more, every sentence of it. What remains on every platform is the one line that says to quit Sunlit Earth, unpack the download and replace the files, or in a macOS bundle the app, so `UpdateHelp::Archive` holds one string and the window's `archive-step` is one text.
9. **Release Notes opens the changelog, and each version opens its release's page.** Decision 9 had the Release Notes link open the newest release's page from the file's `notes`. On 2026-11-22 the user asked for it to open `https://github.com/sunlit-earth/sunlit-earth/blob/main/CHANGELOG.md` instead, which is now `Links.changelog`, in the list the opener's test walks, and the link shows whenever a version is on offer. Later the same day the user asked for every release's page as well: in the release list the version number of each heading ("0.7.0, December 1, 2026") is a link in the accent color with a pointer cursor, the same as Release Notes, and opens that release's `notes` URL. Rule 6 applies to `notes` as written: the URL counts only when it starts with `https://github.com/sunlit-earth/sunlit-earth/releases/` (`NOTES_PREFIX` in `feed.rs`) and `open::is_web_url` accepts it, and anything else reads as missing, which shows that version as plain text. The argument in decision 11 that the Release Notes link leads to the archives no longer holds; a release's page and the website's download section still do.
10. **There is no Skip This Version.** On 2026-11-22 the user asked to remove the button and everything that exists only for it: the banner shows once per version and is closed with one click, so skipping a version adds nothing. This overrides decision 7 in that `update_state.toml` no longer has `skipped_version`; a file an earlier build wrote with it still reads, since unknown fields are ignored, and keeps its `ETag`, `last_checked` and dismissal. It overrides decision 8 in that the tray's entry names any version on offer and the banner hides only for a dismissed version, and the paragraph on Skip This Version no longer applies. It overrides decision 9 in that the Available state ends with Close alone besides the update help. With it go `Request::Skip` and `UpdateEvent::skipped_version` from decision 12, the `skip()` callback from step 6, the Skip half of the Windows desktop check in step 13, and the unit test for `Skip` reaching the state file; the slint case that tested skipping now tests that Close leaves the tray's entry naming the version and that Check for Updates… checks and offers it again.
11. **One website script instead of two.** Decision 4, step 1 and departure 3 name `scripts/fetch-releases.sh` and `scripts/write-update-feed.py`. At the user's request, sunlit-earth/website PR #2 merged the two into one standard library Python script, `scripts/write-release-data.py`, which `deploy.yml` now runs. It fetches the releases with `urllib`, builds `data/releases.json` and `static/releases/v1.json` from the same list in memory, and writes neither file when it refuses, so a deploy can no longer end with the site data of one run and the feed of another. Its output is identical to what the two scripts wrote. The refusals of decision 4 and departure 4 hold unchanged; the refusal departure 3 adds for a `data/releases.json` without `bytes` has no case left, since the feed is no longer read from a file an earlier run wrote. Wherever the decisions name either old script, read `write-release-data.py`.
12. **The window grows for an offer, and a failure names the URL.** On 2026-11-22 the user found the window too short to read the releases. The window took its preferred height of 520 px only when first shown, so one opened on "Checking…" (160 px) kept that height when the offer arrived, and `open` showed the window before filling it. The offer height is now 460 px at the user's request, the `offer-height` property, and `update.rs` grows a shown window to it when the answer is an offer, leaving a taller window alone. The user also asked for the URL in a network failure, after a check failed against a loopback `SUNLIT_EARTH_UPDATE_URL` left over from testing: `HttpUpdateSource` now reports "could not reach <url>: <error>", "<url> answered <status>" and "could not read <url>: <error>".

## Validation

Three rounds, each by a fresh reviewer that ran the gates itself.

- **Round 1** (the plan through departure 6, and the website's first PR): no major, three minor. The website's token shared a step with the bucket's and the tap's, so a missing install on the website repository would have stopped the package managers too (departure 7). A manual check with the update thread gone stayed on "Checking…" for good; it now fails with a reason. The website's README left out two refusals; fixed there.
- **Between rounds 1 and 2**, from the user's look at the window: the buttons grew with the window's height, since every row of its layout had stretch 0 and Slint shares spare height among all of them; the release list sat unframed and spread out as the window grew, and now sits in a framed panel laid out from the top as the About window's tabs are; departures 8 to 11.
- **Round 2**: one major, three minor. The URL rules tested only the prefix, so `.../releases/../../../attacker/repo` passed for `notes` and for assets and resolves to another repository; `feed::is_under` now accepts only a plain path after the prefix, as rule 6 says. The resize test did not pin the list's top; architecture.md still described two archive lines and left `changelog` out of `Links`. All fixed. At the user's request the older version notice got the same layout as the update window, with the rows above the buttons in an inner layout so the gap above them stays the layout's spacing.
- **Round 3**: no major, one minor. The resize test did not measure where spare height goes outside the release panel; `test_the_update_window_leaves_its_rows_in_place_as_it_grows` does, and fails for each of the reviewer's mutations.
