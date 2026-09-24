# Plan: Scoop and Homebrew From Our Own Repositories

## Summary

Two repositories in the `sunlit-earth` organization, `scoop-bucket` and `homebrew-tap`, let users install and upgrade Sunlit Earth with `scoop` on Windows and `brew` on macOS and Linux. A version tag push already builds every platform and creates the GitHub release as a draft, and making it public stays a manual step. Publishing a stable release starts a new workflow that writes the three manifests from the published archives, installs each one on a runner of every platform and architecture it serves, proves the installed binary finds its textures, and only then commits the manifests to the two repositories. On macOS the cask installs `Sunlit Earth.app` and removes the quarantine attribute Homebrew puts on it, so the app starts without the System Settings detour `usage.md` describes today. The app gains one change: the texture lookup also tries the executable's resolved path, so a command reached through a package manager's symlink still finds the textures.

`v0.2.0-beta.1`, published as a prerelease on 2026-09-24, is the first release with every archive under the current names, and it is what the spike and the dry runs use. The repositories are bootstrapped from the stable 0.2.0 once that is published, which may come after the rest of this plan is done.

The research is in [2026-09-24-package-managers-research.md](2026-09-24-package-managers-research.md).

## Stakes Classification

Medium. The pipeline gains write access to two repositories outside this one and publishes what ends up on users' machines, and the macOS cask deliberately bypasses a Gatekeeper check that Homebrew itself no longer offers a flag for. The app change is a few lines in one function. Everything else is new files: an xtask command, a workflow, and three manifests. A bad manifest is reverted by one commit in the repository that holds it.

## What a release holds

Read from `release.yml`, `bundle.rs` and the `v0.2.0-beta.1` draft on 2026-09-24. A `v<semver>` tag push builds five bundles and one `publish` job creates a draft release, marked prerelease when the tag has a `-`. Its assets:

| Asset | Used by |
|---|---|
| `sunlit-earth-<version>-windows-x86_64.zip` | Scoop |
| `sunlit-earth-<version>-linux-x86_64.tar.gz`, `-linux-aarch64.tar.gz` | the formula |
| `sunlit-earth-<version>-macos-x86_64-app.zip`, `-macos-aarch64-app.zip` | the cask |
| `sunlit-earth-<version>-macos-<arch>-terminal.tar.gz` | nothing here |
| `build-info.json`, one record for every build | nothing here |

Each archive unpacks to one directory named like the archive without its extension, with a `README.txt` at its top level. The `-app.zip` directory holds `Sunlit Earth.app` and the readme; the Linux one holds the binary, `textures/`, the licenses and `assets/linux/` with `install-user.sh`, which also takes `--uninstall`. The names come from `bundle::archive_name`, `bundle::app_archive_name` and `bundle::app_bundle_name`. A draft's assets are not at the public `releases/download/<tag>/` URL until the release is published.

## Key Design Decisions

1. **Two repositories, one manifest each per platform.** `sunlit-earth/scoop-bucket` holds `bucket/sunlit-earth.json`. `sunlit-earth/homebrew-tap` holds `Casks/sunlit-earth.rb` for macOS and `Formula/sunlit-earth.rb` for Linux. The `homebrew-` prefix is what makes the short form `sunlit-earth/tap` work (research 3.1); the Scoop name is free, and `scoop-bucket` says what it is. Users run:

   ```
   scoop bucket add sunlit-earth https://github.com/sunlit-earth/scoop-bucket
   scoop install sunlit-earth/sunlit-earth
   brew install --cask sunlit-earth/tap/sunlit-earth    # macOS
   brew install sunlit-earth/tap/sunlit-earth           # Linux
   ```

2. **macOS is the cask, and the cask removes quarantine.** This follows the user's decision and replaces the research's recommendation of a formula on macOS. The cask downloads `sunlit-earth-<version>-macos-<arch>-app.zip` and installs `app "sunlit-earth-<version>-macos-<arch>-app/Sunlit Earth.app"`, the path inside the zip's one directory. A `postflight_steps` block runs `/usr/bin/xattr -dr com.apple.quarantine` on the installed bundle. Homebrew applies the attribute to every cask download (research 3.2), and without it Gatekeeper does not assess the app at all, which is the same reason the terminal tarball and `curl` route in `usage.md` meets no dialog today. `postflight_steps` is the structured form Homebrew now expects; the legacy Ruby `postflight` block is deprecated. The exact `run` syntax is unverified, which is the first thing Step 1 settles. The cask also carries a `binary` stanza for the CLI, `uninstall quit: "earth.sunlit.SunlitEarth"`, a `depends_on macos:` minimum for the 11.0 deployment target (the symbol form the Cask Cookbook documents, checked with `brew audit` in Step 1, since Homebrew itself may no longer run on 11), and `zap trash: "~/Library/Application Support/SunlitEarth"`. The archive's `README.txt` stays in Homebrew's staging directory and is not installed.

3. **Linux is a formula over the plain tarballs, and macOS has no formula.** `depends_on :linux`, one `url` and `sha256` per architecture, `libexec.install Dir["*"]`, and `bin.install_symlink libexec/"sunlit-earth"`. A formula of the same name on macOS as well would give macOS two install routes with different Gatekeeper behavior, which is exactly what decision 2 is meant to avoid. On macOS, `brew install sunlit-earth/tap/sunlit-earth` without `--cask` then fails with the formula's Linux requirement instead of installing the other thing, and Step 1 records what Homebrew prints so `usage.md` can quote it. The formula's `caveats` names `#{opt_libexec}/assets/linux/install-user.sh --exec #{opt_bin}/sunlit-earth`, which puts the desktop entry and icons where the session finds them, and the same script with `--uninstall` to run before `brew uninstall`. A caveat because formulae have no uninstall hook (https://github.com/Homebrew/legacy-homebrew/issues/33329). A Linux cask could run the script on install and on uninstall, but Homebrew calls Linux casks preliminary and runs flight steps without home-directory access where it has a sandbox, so that was set aside on 2026-09-24. A launcher usually does not have Homebrew's `bin` on its `PATH`, hence `--exec`, and `opt_bin` is the path that survives upgrades.

4. **The texture lookup also tries the resolved executable.** `resolve_textures_dir` tries `textures_near(current_exe())` as it does today and, if that finds nothing, `textures_near` on `std::fs::canonicalize(current_exe())`. Homebrew's `bin/` entries are symlinks, the cask's `binary` stanza included, and on macOS `current_exe()` can report the symlink's own path (research 3.3), from where the walk up never reaches `Contents/Resources/textures`. A wrapper script inside the `.app` is not an option, because adding a file to the bundle breaks its signature. The resolved path is the fallback, not the first attempt: on Windows canonicalization turns Scoop's `current` junction into a `\\?\` versioned path, and there is no reason to put that in logs when the plain path already works. This also makes `bin.install_symlink` correct for the formula, so it needs no `write_exec_script`.

5. **The manifests are generated by the xtask, from the archives.** `cargo xtask manifests --version <version> --assets <dir> --out <dir>` reads the five archives it needs from `<dir>`, hashes each one, and writes `bucket/sunlit-earth.json`, `Casks/sunlit-earth.rb` and `Formula/sunlit-earth.rb` under `--out`, laid out as the two repositories are. Every file name and the cask's `app` path come from the bundle's own naming functions (the table above), so a rename in the bundle cannot leave the manifests pointing at assets that do not exist. A missing archive is refused by name. A prerelease version is accepted, because the dry runs verify `v0.2.0-beta.1`; keeping prereleases out of the repositories is the workflow's job (decision 8). Hashes are computed from the files and not read from the GitHub API, so the command runs the same on a laptop against `gh release download` as it does in the workflow. It is Rust rather than shell templates because it is unit-testable offline, as `bundle` is.

6. **A separate workflow runs when a release is published.** Since the release workflow only creates a draft and publishing is done by hand, `release.yml` has no moment at which the archives are downloadable, and it stays as it is. `package-managers.yml` runs on `release: types: [published]` and on `workflow_dispatch` with a `tag` input. `published` rather than `released`, because GitHub documents that `prereleased` does not fire for a prerelease published from a draft, and whether `released` fires for a draft that becomes a stable release is not stated; `published` fires in every case and decision 8 filters. The event is raised because a person publishes the release: GitHub raises no workflow events for release actions taken with `GITHUB_TOKEN`, so if publishing is ever automated with that token, this workflow stops running. For the `release` event `GITHUB_SHA` is the tagged commit, and a workflow runs from the file at `GITHUB_SHA` (inferred from GitHub's event table, not stated there for this event), so the first automatic run is the first stable release tagged after this workflow is merged; everything before it goes through the dispatch, which is also what bootstraps the repositories from 0.2.0 and repairs a release whose push failed. The workflow downloads the assets of the published release rather than build artifacts, so what it hashes and installs is exactly what users download. Its jobs: `manifests` generates and uploads; `verify` installs on five runners; `push` commits. `push` needs every `verify` job, so a manifest that fails to install is never published. The release is public by then, which is acceptable: a failed verify means a release without package manager updates, and the fix is to rerun the job.

7. **Verification installs through the package manager, on every platform and architecture the manifest serves.** Windows x86_64 installs `scoop install <path>/bucket/sunlit-earth.json`. Linux x86_64 and aarch64, and macOS aarch64 and x86_64, copy the formula or the cask into a local tap made by `brew tap-new --no-git sunlit-earth/verify` and install from it. Each job then runs `sunlit-earth --version` through `PATH` and `cargo xtask verify-install --exe <resolved command>`, which renders twice from a working directory outside the install, once against an empty textures directory, and refuses if the two are closer than `TEXTURE_LOOKUP_FLOOR`. That is the comparison `bundle --verify` already makes, factored out of `verify_here` to take an installed executable instead of an unpacked archive. The macOS jobs also require that `xattr -p com.apple.quarantine` fails on the installed `.app`, and that `open -a "Sunlit Earth"` starts a process. The Windows job requires that the installed exe has no `Zone.Identifier` stream and that the Start Menu shortcut exists. Each job ends with an uninstall and checks that it left no install directory behind.

8. **Only stable releases update the repositories.** The first job refuses, and the workflow stops, unless the release exists and is published. It also refuses a release that is marked prerelease or whose tag has a `-` whenever the run would push, which is every `release` event and a dispatch with `push` on. Both the flag and the tag are checked, because the flag can be changed by hand on GitHub after the fact. A dry run, a dispatch with `push` off, may generate and verify a prerelease, which is how the pipeline is proven before a stable release with the current names exists. A beta channel (`sunlit-earth-beta` manifests) is out of scope. Since the pipeline is the only writer and runs at release time, `checkver: "github"` seeing only stable releases is correct, and the manifests carry `checkver`, `autoupdate` and `livecheck` only because Extras and homebrew/cask expect them and they cost nothing. Neither repository runs Excavator or `autobump`, because two writers would race.

9. **`release.yml` does not change.** The tag trigger it gained on 2026-09-24 (`ea732b9`) and the draft it creates (`8ea047a`) are what this plan builds on. It neither calls nor waits for `package-managers.yml`.

10. **The pipeline writes through a GitHub App.** The `sunlit-earth-packaging` app, owned by the organization, with Contents read and write, installed on `scoop-bucket` and `homebrew-tap` only. The `push` job mints a token with `actions/create-github-app-token`, scoped to those two repositories. A token like that expires within the hour and belongs to no person, unlike a fine-grained PAT that expires within a year and leaves with its owner. The private key is a secret in the `package-managers` environment, which only the `push` job uses. The environment allows tags matching `v*`, which is the ref a `release` event runs on, and `main`, which is the ref the dispatch runs on, because a dispatch runs the workflow file of the branch it is started from and the tags before this workflow have none. No other branch can read the key. Commits are made as the app's bot identity, `sunlit-earth-packaging[bot]` with its `users.noreply.github.com` address, which the workflow sets for its own checkout. A commit is made only when a manifest changed, so a rerun is harmless.

## Success Criteria

1. Publishing a stable release on GitHub commits updated manifests to both repositories, with no further manual step, after all five verify jobs pass.
2. Publishing a prerelease, and dispatching the workflow with a prerelease tag, leave both repositories untouched, each with a run that says why it stopped.
3. Dispatching `package-managers.yml` from `main` with `v0.2.0` bootstraps both repositories. Dispatching it again changes nothing.
4. On a Windows machine that has never seen the app, the Scoop commands in decision 1 give a Start Menu entry and a `sunlit-earth` command whose `render` draws the textured Earth, with no SmartScreen prompt.
5. On macOS, `brew install --cask sunlit-earth/tap/sunlit-earth` puts `Sunlit Earth.app` in `/Applications` with no quarantine attribute, and the `sunlit-earth` command draws the textured Earth. The runner can show both of those. That double-clicking the app shows no Gatekeeper dialog is a tester question, recorded at the tier `platforms.md` gives it.
6. On Linux x86_64 and aarch64, `brew install sunlit-earth/tap/sunlit-earth` gives a `sunlit-earth` command that draws the textured Earth, and the caveat's script installs a desktop entry that starts it and removes it again with `--uninstall`.
7. `cargo xtask manifests` refuses an asset directory missing any of the five archives, naming the missing one, and its output passes `brew style` and `brew audit` for the checks Step 1 selects.
8. The first stable release after 0.2.0 upgrades an existing install through `scoop update sunlit-earth` and `brew upgrade`, and is the first one the `release` event handles on its own. Checked by hand at that release, since no second release with these names exists before.

## Implementation Steps

### Step 0: What the user sets up

Outward-facing and in the organization's settings, so not the implementer's: create `sunlit-earth/scoop-bucket` and `sunlit-earth/homebrew-tap`, each public with a `main` branch and a README; create the GitHub App per decision 10 and install it on the two repositories; create the `package-managers` environment in `sunlit-earth/sunlit-earth`, holding the `PACKAGING_APP_PRIVATE_KEY` secret and the `PACKAGING_APP_ID` variable.

State on 2026-09-24: the app is `sunlit-earth-packaging` (ID 5064236) with Contents write and Metadata read, no webhook, installable only on the organization. Both repositories are public and hold a README and nothing else. The environment exists with the secret, the variable, and a deployment rule for tags matching `v*`. The environment also allows the `main` branch (decision 10), and the user confirmed the installation covers exactly the two repositories, which the `gh` token cannot read. Step 0 is complete.

### Step 1: A spike against a published release on hosted runners

Before any xtask or pipeline code, prove the risky parts by hand with manifests written directly, on a `spike/package-managers` branch with a workflow triggered by pushes to that branch (a `workflow_dispatch` workflow must exist on `main` first). Delete the branch and the workflow afterwards. The install paths need public download URLs, so this runs against `v0.2.0-beta.1`; a prerelease is fine here, since these manifests are written by hand. Record for each:

- macOS, both architectures: the working `postflight_steps` syntax for the `xattr` call; the attribute present on the staged download and absent after install; whether App Management or any other prompt appears; what `open -a` does; whether `current_exe()` through the `binary` symlink reports the symlink (which is what decision 4 assumes); the output of `brew style` and `brew audit --cask --strict`, and which audits fail for reasons this plan accepts, such as the signature. Also what `brew install sunlit-earth/verify/sunlit-earth` without `--cask` prints on macOS when the Linux-only formula is in the same tap.
- Linux, both architectures: whether Homebrew is on the runner or needs `Homebrew/actions/setup-homebrew`; whether the installed binary's ELF interpreter and RPATH are what the tarball shipped (`readelf -l`, `readelf -d`), since Homebrew might rewrite them; `brew style` and `brew audit --formula --strict`; which apt packages the render needs, starting from the release workflow's list; `install-user.sh` run from `opt_libexec` with and without `--uninstall`.
- Windows: installing from a local manifest path; the shim's output for `sunlit-earth --version` and `sunlit-earth displays`; no `Zone.Identifier`; the Start Menu shortcut's target; and what `scoop update` does while the tray app is running, by installing a copy of the manifest with a different version string and the same archive.

If the quarantine removal cannot be done by any flight step, stop and report, since that is the premise of decision 2.

### Step 2: The texture lookup fallback

Decision 4 in `texture_loader.rs`, with a `#[cfg(unix)]` unit test that symlinks an executable path into another directory and checks that the lookup through the symlink finds the textures beside the target. Windows gets no symlink test, because creating one needs a privilege CI runners are not guaranteed to have.

### Step 3: `cargo xtask manifests` and `cargo xtask verify-install`

The generator from decision 5 and the verification from decision 7, with `verify_here`'s render comparison factored so both it and `verify-install` call the same function. Unit tests: every URL in every manifest is `https://github.com/sunlit-earth/sunlit-earth/releases/download/v<version>/<name>` with the name from the bundle's own naming functions; the cask's `app` path is `app_bundle_name` joined with `Sunlit Earth.app`; each hash is the SHA-256 of the file named; the Scoop manifest parses as JSON with the field order Extras requires and its `extract_dir` is the Windows bundle name; a prerelease version such as `0.2.0-beta.1` gives manifests Scoop and Homebrew accept; a missing archive is refused by name. Formatting the Ruby files is plain string assembly, checked against `brew style` in Step 4, not reimplemented.

### Step 4: The workflow

`package-managers.yml` per decisions 6, 7, 8 and 10, with the checks Step 1 settled. The dispatch has a second input, `push`, off by default, so a dispatch is a dry run unless asked; the `release` event always pushes, and on a prerelease stops per decision 8. A dispatch needs the workflow on the default branch, so while it is on the feature branch it also carries a temporary `push: branches: [feat/package-managers]` trigger that runs the dry run for `v0.2.0-beta.1`: all five verify jobs, no `push` job. That trigger is removed before the pull request leaves draft. After the merge, which is the user's call: dispatch from `main` for `v0.2.0` with `push` off, then with it on to bootstrap the repositories, then once more to see criterion 3's no-op. If 0.2.0 is not published by then, those three dispatches wait for it and are handed to the user with the exact commands.

### Step 5: The prerelease check

After the merge, dispatch with `v0.2.0-beta.1` and `push` on, and confirm the run stops at the first job with the reason. The `release` event half of criterion 2 is seen the first time a prerelease is published after the merge, since this workflow does not exist at any earlier tag; that needs no throwaway release.

### Step 6: Documentation

- `usage.md`: an install section per platform with the commands from decision 1, and the macOS section saying that the cask removes the quarantine attribute, and what that means: the app is not notarized, and a user who installs through Homebrew skips the check Gatekeeper would otherwise make. The Linux caveat's desktop entry script, with `--uninstall`.
- `README.md`: propose the install commands to the user and edit only with permission.
- `assets/readme/*.txt`: whether the three per-platform readmes should name the package manager route, proposed to the user rather than assumed, since they ship in every archive.
- `platforms.md`: the package manager rows with their evidence tiers.
- `testing.md`: `package-managers.yml`, what its verify jobs prove and what they do not, and that it runs on publishing rather than on the tag.
- `roadmap.md`: close the item, and add the beta channel, the Extras and homebrew submissions with their preconditions from research 2.5 and 3.6, and the note from the research that a future autostart must point at `current` and `opt/sunlit-earth`.
- `CLAUDE.md`: the two xtask commands in the Release builds section.

### Step 7: List the bucket

Once the bucket holds a verified manifest, add the `scoop-bucket` topic to `sunlit-earth/scoop-bucket` (`gh repo edit sunlit-earth/scoop-bucket --add-topic scoop-bucket`), which is what gets it indexed on scoop.sh (research 2.1). Last, because a listing that points at an empty bucket helps nobody. Ask the user first, since it is outward facing.

## Out of Scope

- A beta channel.
- Submission to Scoop Extras, homebrew/cask or homebrew/core, and notarization, which the last two depend on.
- winget, AUR, Flatpak and other package managers.
- A Homebrew formula on macOS, and any use of the `-terminal.tar.gz` archives.
- Installing the Linux desktop entry automatically. A formula cannot write to the user's home directory, so this stays a caveat.
- Publishing releases automatically. It stays a manual step, and decision 6 depends on it being done by a person or a token other than `GITHUB_TOKEN`.

## Risks and Mitigations

- **Homebrew tightens against quarantine removal in third-party taps.** No rule forbids it today (research 3.2), but it is the circumvention Homebrew removed the flag for. Mitigation: the verify job fails loudly if the attribute is still present after install, and the fallback is a macOS formula over the terminal tarball, which research 3.3 lays out and decision 4 already makes work.
- **A prompt or permission blocks the `xattr` call.** macOS App Management protects apps in `/Applications` from modification by other processes. Step 1 observes this on the runner; a real Mac with stricter settings may differ, which is a tester question.
- **Homebrew rewrites the Linux binary.** If Step 1 shows the interpreter or RPATH changed, the render check catches a broken result, and the formula can opt out of relocation or the finding goes into the departures.
- **`scoop update` fails while the tray app runs.** If Step 1 shows it, the manifest's `notes` tells users to quit the app first, and `usage.md` says so.
- **Publishing stops raising the event.** If a release is ever published with `GITHUB_TOKEN`, nothing runs (decision 6). The dispatch covers it, and `testing.md` says so.
- **The app's credentials leak.** The token is scoped to two repositories and expires within the hour, and the private key is readable only from runs on `v*` tags or `main` through the environment rule. Rotating the key is one click in the app's settings.
- **A release is published and its manifests are not.** Intended by decision 6. Rerun the failed jobs, or dispatch `package-managers.yml` with the tag.

## Rollback Strategy

A bad manifest: revert its commit in the repository that holds it, and users' next `scoop update` or `brew upgrade` goes back to the previous version. The pipeline: disable or delete `package-managers.yml`, and publishing a release goes back to doing nothing beyond the release. The app change stands on its own and needs no rollback.

## Departures

1. **The Step 1 spike ran on the run branch, not on a `spike/package-managers` branch.** The run that implements this plan may push only `feat/package-managers`, so the spike workflow was a temporary file on that branch, triggered by `push` to it, and was removed in a later commit once its findings were recorded under Step 1 below. The branch's history keeps both commits, which costs nothing, since the pull request is merged as a whole and the file does not exist at its head.

## Validation Record

None yet.
