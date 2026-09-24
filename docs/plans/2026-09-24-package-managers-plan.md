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

#### What the spike found

Runs 36047584120, 36048250176 and 36048919200 of the temporary `spike-package-managers.yml` on 2026-09-24 (departure 1), against the published `v0.2.0-beta.1`. The runners had Homebrew 6.0.22 (macOS 26.6.2, arm64), 6.0.18 (macOS 26.6.1, Intel) and 7.0.4 (Ubuntu 24.04 on both architectures), and Scoop 0.5.3 on Windows Server 2025.

- **The quarantine removal works.** Inside `postflight_steps` Ruby interpolation is not available; the install-time token is `{{appdir}}`, so the step is `run "/usr/bin/xattr", args: ["-dr", "com.apple.quarantine", "{{appdir}}/Sunlit Earth.app"]` (Cask Cookbook; `Casks/p/parallels.rb` in homebrew/cask uses the same form). On both architectures the cached download carries `com.apple.quarantine` with the agent `Homebrew Cask`, a control install of the same cask without the step leaves the attribute on `/Applications/Sunlit Earth.app`, and with the step `xattr -p` reports no such attribute. No prompt appeared and nothing blocked the call on the runner. `open -a "Sunlit Earth"` exits 0 and starts `Contents/MacOS/sunlit-earth`; `spctl --assess` still says `rejected`, which is the notarization it lacks and not a quarantine check. `brew uninstall --cask` quits the app through `uninstall quit:`, removes the app, the `bin` link and the Caskroom entry.
- **`current_exe()` reports the `binary` symlink on macOS.** Through `/opt/homebrew/bin/sunlit-earth` (Intel: `/usr/local/bin`) the released binary rendered the grid: its PNG is 150553 bytes on arm64, exactly the size of the render against an empty textures directory, and 151744 against 152071 on Intel, while the app's own `Contents/MacOS/sunlit-earth` rendered the globe at about 280 KB on both. Decision 4 is needed. On Linux the same kind of symlink found the textures already, since `current_exe()` there reads `/proc/self/exe`.
- **Cask style and audit.** `brew style` asks for an empty line before `postflight_steps` (`Cask/StanzaGrouping`) and nothing else; `brew audit --cask --strict` passes. `--online` adds two failures that only a prerelease has: the version differs from what `livecheck` reads (`:github_latest` skips prereleases), and the release is a GitHub pre-release. The string form `depends_on macos: ">= :big_sur"` is deprecated in favor of the symbol, and the spike used `depends_on macos: :big_sur`; departure 4 is why the cask ended with no minimum at all. The legacy `postflight do ... end` also removed the attribute and drew only the grouping offense from Homebrew 6.0.22, but the cookbook deprecates it, so it is not used.
- **The formula without `--cask` on macOS** prints `Treating sunlit-earth/verify/sunlit-earth as a formula. For the cask, use sunlit-earth/verify/sunlit-earth or specify the --cask flag.`, then `sunlit-earth: Linux is required for this software.` and `An unsatisfied requirement failed this build.`
- **Linux.** Homebrew is preinstalled under `/home/linuxbrew/.linuxbrew` on `ubuntu-24.04` and `ubuntu-24.04-arm` and only needs its `bin` on `PATH`. The installed binary is byte-identical to the one in the tarball: same SHA-256, same `INTERP`, same `NEEDED`, no `RPATH` either way, so Homebrew does not relocate it. Every `NEEDED` library resolves on a bare runner; the render fails with `no graphics adapter is available` until `mesa-vulkan-drivers` is installed, and that is the only apt package it needs. `install-user.sh --exec $(brew --prefix)/opt/sunlit-earth/bin/sunlit-earth` from `opt/sunlit-earth/libexec` writes the entry with that `Exec` line and eight icons, and `--uninstall` removes all of them. `brew uninstall` removes the Cellar, `opt` and `bin` entries.
- **Formula style.** `FormulaAudit/ComponentsOrder` refuses `url` and `sha256` inside `on_arm` and `on_intel` outside a `resource` block, and `FormulaAudit/OnSystemConditionals` refuses `if Hardware::CPU.arm?` in their place, so a formula that downloads a different archive per architecture has no form `brew style` accepts; see departure 2. Homebrew 6.0.22 also calls `version` redundant with the version it scans from the URL, and 7.0.4 does not.
- **Windows.** `scoop install <path>\sunlit-earth.json` works and records the path, so `scoop update` rereads that file. The shim is `~\scoop\shims\sunlit-earth.exe`, made a GUI binary to match the target, so PowerShell does not wait for it and its output lands after the prompt; `sunlit-earth --version` prints `sunlit-earth 0.2.0-beta.1` and `displays` prints the plan, and a caller that waits (bash, or a captured pipe) sees them in order. A render through the shim from another directory found the textures. Neither the installed exe nor the cached zip has a `Zone.Identifier` stream. The shortcut is `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Scoop Apps\Sunlit Earth.lnk`, with target `~\scoop\apps\sunlit-earth\current\sunlit-earth.exe` and that directory as working directory. With the tray app running (the runner has no OpenGL, so it needs `SLINT_BACKEND=winit-software` to stay up), `scoop update` prints `ERROR The following instances of "sunlit-earth" are still running. Close them and try again.` and `Running process detected, skip updating.`; after the app quits, the same command updates. So the manifest carries a `notes` line saying to quit the app first. `scoop uninstall` removes the app directory, both shim files and the shortcut.

### Step 2: The texture lookup fallback

Decision 4 in `texture_loader.rs`, with a `#[cfg(unix)]` unit test that symlinks an executable path into another directory and checks that the lookup through the symlink finds the textures beside the target. Windows gets no symlink test, because creating one needs a privilege CI runners are not guaranteed to have.

### Step 3: `cargo xtask manifests` and `cargo xtask verify-install`

The generator from decision 5 and the verification from decision 7, with `verify_here`'s render comparison factored so both it and `verify-install` call the same function. Unit tests: every URL in every manifest is `https://github.com/sunlit-earth/sunlit-earth/releases/download/v<version>/<name>` with the name from the bundle's own naming functions; the cask's `app` path is `app_bundle_name` joined with `Sunlit Earth.app`; each hash is the SHA-256 of the file named; the Scoop manifest parses as JSON with the field order Extras requires and its `extract_dir` is the Windows bundle name; a prerelease version such as `0.2.0-beta.1` gives manifests Scoop and Homebrew accept; a missing archive is refused by name. Formatting the Ruby files is plain string assembly, checked against `brew style` in Step 4, not reimplemented.

### Step 4: The workflow

`package-managers.yml` per decisions 6, 7, 8 and 10, with the checks Step 1 settled. The dispatch has a second input, `push`, off by default, so a dispatch is a dry run unless asked; the `release` event always pushes, and on a prerelease stops per decision 8. A dispatch needs the workflow on the default branch, so while it is on the feature branch it also carries a temporary `push: branches: [feat/package-managers]` trigger that runs the dry run for `v0.2.0-beta.1`: all five verify jobs, no `push` job. That trigger is removed before the pull request leaves draft. After the merge, which is the user's call: dispatch from `main` for `v0.2.0` with `push` off, then with it on to bootstrap the repositories, then once more to see criterion 3's no-op. If 0.2.0 is not published by then, those three dispatches wait for it and are handed to the user with the exact commands.

#### State before the merge, and what is left for after it

The dry run through the temporary trigger went green on all five verify jobs in <https://github.com/sunlit-earth/sunlit-earth/actions/runs/36051180250>, with the `push` job skipped, as it is for every `push` event. The trigger was then removed, so from here the workflow runs only on `release` and `workflow_dispatch`, and a dispatch needs the file on `main`. The rest of Steps 4, 5 and 7 needs the merge, which is the user's call, and for Step 4 also a published stable 0.2.0. In order, from a checkout of `main` after the merge; `gh workflow run` prints the new run's URL, and `<id>` is the number at its end:

```bash
# Step 4, once v0.2.0 is published: a dry run, then the bootstrap, then the no-op.
gh workflow run package-managers.yml --ref main -f tag=v0.2.0 -f push=false
gh run watch <id> --exit-status
gh workflow run package-managers.yml --ref main -f tag=v0.2.0 -f push=true
gh run watch <id> --exit-status
gh api repos/sunlit-earth/scoop-bucket/commits --jq '.[0].commit.message'   # sunlit-earth 0.2.0
gh api repos/sunlit-earth/homebrew-tap/commits --jq '.[0].commit.message'   # sunlit-earth 0.2.0
gh workflow run package-managers.yml --ref main -f tag=v0.2.0 -f push=true
gh run watch <id> --exit-status
# criterion 3: the Push job's log says "already at 0.2.0, nothing to commit" for both repositories

# Step 5, needs only the merge: a prerelease with push on stops at the first job.
gh workflow run package-managers.yml --ref main -f tag=v0.2.0-beta.1 -f push=true
gh run watch <id>
# expected: Manifests fails with "v0.2.0-beta.1 is a prerelease, and this run would push", Verify and Push never run

# Step 7, after asking the user, once the bucket holds the verified 0.2.0 manifest.
gh repo edit sunlit-earth/scoop-bucket --add-topic scoop-bucket
```

**v0.2.0 has to be tagged from `main` after this pull request is merged.** The first dispatch for v0.2.0 is the first run in which the macOS verify jobs render through the cask's `bin/` symlink (departure 3), and only a binary that contains the texture lookup fallback of decision 4 (commit 39fb348) finds its textures from there. A v0.2.0 tagged before the merge would fail both macOS verify jobs on "The installed command finds its textures", and since `push` needs every verify job, the bootstrap could not happen for it at all. The same failure on a release tagged after the merge means decision 4 did not reach the release binary, not that the cask is wrong.

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
- **A push moves users backwards.** A dispatch for an older tag with `push` on, or a patch release of an older line published after a newer release, would otherwise overwrite newer manifests. The push job reads the version each of the three manifests holds and refuses when it is newer than the one being pushed; an equal version is the no-op. Only stable versions are ever written, so the comparison is on `X.Y.Z`, read as base-10 numbers. A manifest that exists and holds no version of that form is refused too, so an unreadable file stops the push instead of letting it through; a missing file is the bootstrap.
- **Two runs race to the same branches.** The workflow's concurrency group is per tag, so runs for two releases can overlap. The push job has its own fixed concurrency group, `package-managers-push`, so two pushes never run at the same time. It does not queue them all: a group holds one running and one pending job, and a newer pending job cancels the older pending one even with `cancel-in-progress` off, so with three releases' pushes arriving together the middle one is cancelled and needs a rerun. The downgrade check keeps any order of reruns from moving users backwards. If a push still meets a moved branch, `git push` fails as not a fast-forward and rerunning the job is the fix.

## Rollback Strategy

A bad manifest: revert its commit in the repository that holds it, and users' next `scoop update` or `brew upgrade` goes back to the previous version. The pipeline: disable or delete `package-managers.yml`, and publishing a release goes back to doing nothing beyond the release. The app change stands on its own and needs no rollback.

## Departures

1. **The Step 1 spike ran on the run branch, not on a `spike/package-managers` branch.** The run that implements this plan may push only `feat/package-managers`, so the spike workflow was a temporary file on that branch, triggered by `push` to it, and was removed in a later commit once its findings were recorded under Step 1 below. The branch's history keeps both commits, which costs nothing, since the pull request is merged as a whole and the file does not exist at its head.
2. **The formula is checked with `FormulaAudit/ComponentsOrder` excluded.** Step 1 showed that Homebrew's style rules leave no accepted way for a formula to download a different archive per architecture: `url` and `sha256` are refused inside `on_arm` and `on_intel` (the cop allows them only in a `resource`), and `if Hardware::CPU.arm?` is refused in their place. homebrew/core builds from source and never needs it. So the formula keeps `on_arm` and `on_intel`, orders every other stanza the way that cop wants (in the spike its only other complaint was the position of `depends_on`, which the generator now puts before the blocks), and the workflow runs `brew style --except-cops FormulaAudit/ComponentsOrder` on it. `brew audit` refuses `--except-cops` beside `--strict`, so the audit runs `--strict --skip-style` and leaves its style half to `brew style`. The dry run found one more: the audit calls `version` redundant on aarch64, where Homebrew reads `0.2.0-beta.1` out of the URL, and not on x86_64, where the URL does not give it that, so the stanza is needed and the audit also runs with `--except version`, which skips only that one method. Criterion 7's "for the checks Step 1 selects" is these three exclusions. The cask is checked with nothing excluded.
3. **The dry run for `v0.2.0-beta.1` verifies the macOS app's own binary, not the `binary` symlink.** Step 1 showed that the beta.1 binary, which predates decision 4, renders the grid through the symlink, so a verify job pointed at the command on `PATH` cannot pass for that release, and no later release exists yet. The macOS verify jobs therefore check `sunlit-earth --version` through `PATH` for every tag, and for `v0.2.0-beta.1` alone run `verify-install` on `/Applications/Sunlit Earth.app/Contents/MacOS/sunlit-earth` with a notice saying why; every other tag, 0.2.0 included, is verified through the symlink. The exception is keyed on that one tag and cannot reach a push, since the workflow refuses to push a prerelease. The symlink half of decision 4 is proven by the unit test from Step 2 until the first dispatch for 0.2.0, which is the first run that sees a binary built with it.
4. **The cask has no `depends_on macos:` minimum.** Decision 2 asked for one at 11.0 and left it to `brew audit` to say whether Homebrew still runs there. The spike's runners, on Homebrew 6.0.22 and 6.0.18, accepted `depends_on macos: :big_sur`; the first dry run updated Homebrew first, and `brew style` there refused it with `Homebrew/OSDependsOn: Use depends_on :macos instead of a redundant minimum macOS version`, which says Homebrew's own minimum is above 11. The cask therefore says `depends_on :macos`. A Mac too old for the app is one Homebrew itself does not install on, so nothing is lost, and `LSMinimumSystemVersion` in `Info.plist` still states 11.0 for anyone who unpacks the zip by hand.

## Validation Record

**Round 1** (validator on `e34e6ec..1b6492e`): 1 major and 5 minors.

1. Major: `verify-install` removed and recreated `--work` before any check, so `--work` naming an install, a textures directory or `.` deleted user data. Fixed: it makes a fresh, uniquely named directory under `--work` and deletes only that one, after a passing run; the guard against a working directory inside the install was dropped as dead logic, since the lookup reads only `./textures` from the working directory and a directory made empty there cannot answer it. Tests cover a refusal that touches nothing, the named directory and its contents surviving a run, and a fresh directory never reusing one that exists.
2. Minor: the workflow passes `app-id`, which `actions/create-github-app-token` v3 deprecates in favor of `client-id`. Deferred to the user, since it needs a new variable in the `package-managers` environment.
3. Minor: nothing prevented a downgrade. Fixed with the push job's version check and its fixed concurrency group, both under Risks.
4. Minor: the plan did not say that v0.2.0 must be tagged after the merge. Fixed in Step 4's after-merge section.
5. Minor: `testing.md` named only the `FormulaAudit/ComponentsOrder` exclusion. Fixed: it names the audit's `--skip-style --except version` too.
6. Minor: the README and `assets/readme/*.txt` proposals. Deferred to the user, who decides both.

**Round 2** (validator on `1b6492e..e3aac86`): every round 1 fix verified, 0 major and 3 minors, all fixed.

1. A relative `--work` always failed: the path was handed to renders whose working directory was already that directory, so it was resolved twice and the grid render found the real textures. The parent is now made absolute with `std::path::absolute` first, and a test checks that both renders receive a path under its absolute form.
2. The downgrade check failed open on a held version it could not read (a null `version`, a missing or indented `version` line, leading zeros read as octal, four parts). The held value now has to match `X.Y.Z` with an optional suffix, a present file without one is refused, and the parts are compared as base-10. Checked locally against sixteen fixture cases.
3. The Risks entry and the push job's comment claimed the concurrency group serializes every push. They now say it holds one running and one pending job, that a newer pending job cancels the older pending one, and that a cancelled push is rerun by hand.
