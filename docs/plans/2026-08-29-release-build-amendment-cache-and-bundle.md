# Plan Amendment: A Build Cache on the Host, and a Release Bundle

Amends `2026-08-28-vm-release-build-plan.md`. Written 2026-08-29, after the first days of live `dist` runs recorded in that plan's validation record. Two things the finished command does not do. It downloads and compiles all 519 crates on every run, because a builder guest is a throwaway overlay of a golden disk and nothing a build learns survives the guest that learned it; a cold release build with fat LTO is 4m51s on Linux and 6m45s on Windows, which is most of the 5m40s and 7m47s a whole target costs. And it produces a bare binary, which is not something anyone can be handed: the app reads its four surface textures from disk, and those are the Git LFS assets the source archive deliberately leaves out, so the artifact in `target/dist/<target>/` renders the procedural grid on any machine but one with a checkout beside it.

This amendment adds a cache that lives on the host between builds, and a release bundle that carries the textures with the binary.

## What changes, and what does not

The image model, the layer semantics, the provider matrix, the guest contract, the linkage checks, the dirty-tree refusal, the `--keep` rule and the two builder images are all untouched. `e2e` is untouched. What changes is inside one command:

- `dist` copies a cache archive into the builder before the build and pulls a fresh one out after it, so the second build of a commit pays for the compiler and not for the download and the dependency graph.
- `dist` writes a release bundle beside the loose binary, a zip for Windows and a `.tar.gz` for Linux, holding the binary, the textures, the record and the license, and the verification boot runs *that* rather than the binary and the repository's `textures/` staged separately.

Two statements in the plan are revised rather than extended, and both are called out here because a reader of the plan alone would be misled.

**Non-goal "Publishing" is narrowed.** The plan says "No tags, no zips, no GitHub release". The bundle is now in scope, because the open question it was standing on has been answered. Tags, `release.yml` and GitHub releases stay out.

**Open question 1 is answered: textures travel with the binary.** The plan's default was that `target/dist/<target>/` holds the executable and nothing else, with the textures staged into the verification guest and nowhere else. That default made the output not a runnable install, and the question of how textures are distributed is now settled for the one case this command produces: they go in the bundle, at the layout `assets::texture_loader::resolve_textures_dir` already looks for, which is a `textures/` directory beside the executable.

### What this does to goal 4

Goal 4 says nothing of the host reaches the build except the source archive and the pinned toolchain name: no host `target/`, no host `~/.cargo`, no host environment. A cache is host-held build state, so the literal sentence no longer holds, and the goal has to be restated rather than quietly weakened.

What the goal was written against is a developer's `target/` directory that has seen every branch this checkout was ever on, and a `~/.cargo` shared with every other project on the machine. Neither of those is what the cache is. The cache is the output of an earlier run of this same command, in the same builder image, with the same pinned channel, parked on the host only because the guest that produced it was destroyed. No compiler, header, library, or environment variable of the host is in it, and the moment the image or the channel changes it is discarded whole rather than merged.

So goal 4 becomes, settled 2026-08-29: the host's own Rust cache is never copied into a guest, and the only thing a build may inherit beyond the source archive and the channel's name is what an earlier build in the same builder image, on the same channel, wrote after it succeeded. That is a direction rather than a qualification, and it is the property to hold on to when reading the rest of this document: everything under `<store>/cache/` arrives there from a builder guest and from nowhere else. There is no flag that seeds it from the host's `~/.cargo` or `target/`, and adding one would be the thing this rule forbids. `--no-cache` makes the original goal reachable on demand, and decision 27 is what keeps the difference visible in the record rather than a thing to remember.

## Facts measured for this amendment

- A `target/release` of this workspace on this host is **4.42 GiB in 7,752 files**. That is an upper bound on what a builder's would be, since `dist` builds `--release --locked -p sunlit-earth` rather than the whole workspace with its test targets. Fat LTO is why it is that large: `lto = true` makes every rlib carry LLVM bitcode.
- The host's own `~/.cargo/registry` is 1.5 GiB, but that serves every project on this machine. A builder's holds one lockfile's 519 crates, which is the figure to measure on the first run.
- `tar -m`, which extracts without restoring the archived modification time, works on **both** tools this needs: GNU tar 1.34, which is the Linux builder's userland, and bsdtar, which is Windows' own `tar.exe`. Verified on this host by extracting a file stamped 2001 with each and reading back the extraction time.
- Windows 11's `tar.exe` on **this host** is bsdtar 3.8.4 with libzstd 1.5.7 linked in. What the *guest* image's `tar.exe` is has not been asked. That question decides whether the Windows layer needs a zstd of its own, not what format the archives are in, which decision 22 settles either way.
- The four textures the app names come to **14.1 MB** together (`world.topo.200405.jxl` 2.5 MB, `BlackMarble_2016.jxl` 1.4 MB, `lroc_color_poles_1k.jxl` 0.3 MB, `milkyway_2020_4k.jxl` 9.9 MB), with `PROVENANCE.md` another 6.6 KB. JXL is why a bundle with every asset in it is smaller than the binary.
- `flate2` is already in `Cargo.lock`, through `image`'s PNG support, so the deflate half of the zip and the gzip half of the tarball both cost the workspace no new transitive tree. The two writers themselves, the `zip` and `tar` crates, are the new dependencies, and both are xtask-only.
- The host never reads a cache archive, only stores and transfers it, so no zstd tool is required on the host and `vm doctor`'s tool list is unchanged.
- The repository has **no license text**, though the workspace manifest declares `GPL-3.0-or-later`, and **no tags**, so `git describe --tags --always` is a bare hash today. Both matter to a bundle: one is a file it should carry, the other rules out naming it after `describe`.

## Decisions (continuing the plan's numbering)

19. **The cache is host-side state per builder image, and it can never decide what the binary is.** `<store>/cache/<builder slug>/` holds the archives and a sidecar per archive. Three guards make the second half true, and they are the reason this is a cache and not a shortcut. The extraction of the source tree gains `-m` (decision 23), so no committed file can ever look older than an artifact built from it. `--locked` and the lockfile's checksums mean a restored registry can only ever hold what the network would have handed over. And a cache is discarded whole rather than merged when the channel or the builder image changes (decision 24). What a damaged cache can cost is a slower build or a loud link failure, never a wrong binary: cargo rebuilds an output that is missing, and the linker refuses one that is truncated.

20. **Two archives, because the two halves change at different rates.** `registry.tar.zst` is `~/.cargo/registry` and `~/.cargo/git`, which change only when `Cargo.lock` does. `target.tar.zst` is the build directory, which changes on every build. Keeping them apart is what lets the registry stay on the host through a run that did not move the lockfile, and that is half the outbound transfer on an ordinary build. One archive would have been simpler and would have sent a gigabyte back over the wire every time to say nothing new.

21. **One file crosses the boundary per archive, and the guest does the packing.** `scp -r` of a directory is a round trip per file, and a restored registry is tens of thousands of small files: this is the same reason the source goes in as a tar rather than as a tree. So the host holds an archive, copies it in as one file, and the job unpacks it inside the guest's own file system, where `toolchain.ps1` has already put `C:\sunlit-e2e`, `.cargo` and `.rustup` on Defender's exclusion list, which is what stops a Windows guest from scanning every object file a build writes.

22. **The archives are zstd, and an image that cannot make one is fixed rather than worked around.** `registry.tar.zst` and `target.tar.zst`, one format on both builders, decided 2026-08-29 rather than left to what each guest happens to have. rlibs carrying LLVM bitcode compress several fold and zstd compresses fast enough that the pack does not become the new bottleneck, which is what makes the round trip worth making at all; gzip over four gigabytes would cost more than it saves, and uncompressed is the case this is meant to avoid. `zstd` joins the Linux builder's package list, one line and a 1m12s rebuild. The Windows side is the open measurement: this host's `tar.exe` is bsdtar 3.8.4 with libzstd linked in, and the guest image's has not been asked, so step 3 asks it with one `vm ssh windows-builder "tar --version"`. If it has none, `toolchain.ps1` gains the zstd release archive the way it already gains libclang, and the layer is rebuilt in about four minutes. There is deliberately no uncompressed fallback: a fallback would mean two formats to test and a build whose transfer cost depends on which image it happened to run in.

23. **The two trees are extracted with opposite treatment of modification times, and that is the correctness argument.** The source tree is extracted with `-m`, so every file in it is stamped at extraction: `git archive` stamps its entries with the commit's own time, and rebuilding an older commit over a newer cache would otherwise present cargo with sources older than the artifacts and produce a binary of the previous commit under this commit's `build-info.json`. That is the failure the dirty-tree refusal exists to prevent, reached by a different road. The cached trees are extracted **without** `-m`, keeping the times they were archived with, because their whole value is that nothing in them looks newer than what was built from it. What `-m` costs on the source is that the workspace's own crates recompile on every build, which they would anyway and which the fat LTO link makes unavoidable regardless.

24. **A cache belongs to one channel and one build of one image, and anything else is discarded.** The sidecar records the format version, the archive's name and size, the pinned channel, the builder image's slug, its manifest's `template_hash` and its `built_utc`, the `Cargo.lock` hash the registry was made from, and the commit and time that wrote it. A restore compares channel and image identity and refuses on any difference, with one line naming the field that moved. `built_utc` is in there beside `template_hash` because an image rebuilt from an unchanged template is still a different MSVC, a different libclang and a different set of paths baked into cargo's fingerprints.

25. **The build directory moves out of the source tree.** `CARGO_TARGET_DIR` becomes `<guest root>/cargo-target`, so the `rm -rf src` and the fresh extraction at the top of every job cannot touch it, the archive has one fixed path on both operating systems, and the executable is copied from a path that does not move with the source layout. `kept_builder_note` promises a kept builder the source tree with its `target/release` beside it, which this makes literally true.

26. **The cache is written after a success, and the registry only when the lockfile moved.** A failed build's tree is not saved, because a build that failed because of what was in its cache would otherwise make that failure stick, and a `dist` build is of a committed tree where a failure usually means something real. The registry is skipped when its recorded lockfile hash matches this build's, since `--locked` means an unchanged lockfile is an unchanged registry. Both are written temp-then-rename under a name carrying the process id, the discipline `assets::texture_cache` already uses, so an interrupted pull cannot leave a truncated archive for the next build to read.

27. **The build record says whether the build was warm.** `BuildInfo` gains a `cache` section: per archive, whether it was restored, and if so the key it matched and how old it was, or the one-line reason it was not. `--no-cache` skips restore and save both and the record says so. That is what keeps decision 19's argument checkable after the fact rather than a claim in a document, and it is what a release that is actually shipped can be pinned on.

28. **The cache is inventory, not run state.** Run state is what the next boot recreates and what `vm down` removes; a cache is the opposite of both, so it stays outside `vm::run_state_paths` and no teardown touches it. It is a real part of the store's disk footprint, so `vm status` counts it per image and names the command that reclaims it, `teardown::Scope` gains a fourth flag, and `vm purge <image> --cache` takes the cache alone while a purge with no flags takes it with everything else. The flag joins the additive three the same way, and `the_docs_spell_out_every_flag_dist_takes` covers the new `dist` flag the same way it covers the rest.

29. **The bundle is each platform's own archive format, beside the loose binary, named for the package version.** `sunlit-earth-<version>-windows.zip` and `sunlit-earth-<version>-linux.tar.gz`, both in `<target dir>/dist/<target>/`, each holding one top-level directory of the same name so that unpacking anywhere produces one folder rather than a scattering. Decided 2026-08-29: a zip is what Windows opens with no tool at all, and a tarball is what a Linux user expects and what carries a file mode without a convention on top of it, so the format follows the target the way the job scripts and the guest roots already do. The version comes from the workspace `Cargo.toml`, read with the `toml` crate `guest::toolchain` already uses, rather than from `env!("CARGO_PKG_VERSION")` in the xtask, which is the version of the tree the xtask was compiled from and not necessarily the one being built. Not `git describe`, because this repository has no tags and a bundle called `sunlit-earth-4b4cb2e-windows.zip` tells a user nothing; `describe` and the commit are inside it, in the record.

30. **What is in it.** The binary; `textures/` with the four files `artifacts::TEXTURE_FILES` names and `PROVENANCE.md`, which is the attribution for the imagery; `build-info.json`, so the record travels with the thing it describes; and `LICENSE`. There is no license text in the tree today and the workspace declares `GPL-3.0-or-later`, so a `LICENSE` at the repository root is part of this work: a binary handed to somebody should carry the text of the license it is under. The star catalog's `ATTRIBUTION.md` goes in beside it, since that data is baked into the binary and its attribution cannot travel any other way. A Linux bundle carries one thing more, `assets/` with the desktop entry, the hicolor icon set, the master SVG and `install-user.sh`, which exists exactly for a user holding a binary and no package and which reads those files by a path relative to itself; that is a few hundred kilobytes, and it is what makes the Linux bundle installable rather than only runnable. A Windows bundle needs no equivalent, because the icon is a resource inside the exe.

31. **One assembled directory, two writers, and the binary keeps its mode.** The bundle is built as a directory first and then written out by whichever writer the target names, so the layout exists in one place and the format question is the last thing that happens to it. The `zip` crate writes the Windows one, storing the `.jxl` entries rather than deflating what is already compressed and deflating the rest; the `tar` crate over `flate2` writes the Linux one, where mode 0755 on the binary is an ordinary field of the header rather than an external attribute anyone has to remember to set, and a test asserts it, because without it the first thing a Linux user does is `chmod +x`. Splitting the formats this way is what makes the mode question disappear from the Windows path entirely rather than being answered there and ignored. The host then reads the finished archive back with the same crate that wrote it and compares its entries against the directory, which is the cheap half of proving the bundle. The expensive half is decision 32.

32. **The verification boot runs the bundle, and proves the textures were found rather than assuming it.** The desktop guest is staged with the bundle directory rather than with the binary and the repository's `textures/` separately, and the render is asked for with no `SUNLIT_EARTH_TEXTURES` at all, so what is under test is the lookup a user's machine will do: `resolve_textures_dir` walking up from the executable to the `textures/` beside it. A render that failed to find them still produces a 640x360 PNG of the procedural grid, so the header check the plan already makes cannot tell the two apart. So the job renders twice, once with `SUNLIT_EARTH_TEXTURES` pointed at an empty directory it creates, which is the grid by construction, and once from the bundle; the host decodes both with the `image` crate it already carries and requires a mean channel difference far above what a few seconds of the Earth turning between two renders can account for. The threshold is calibrated on the first live run and recorded, the way the plan treats every first run. It is the same lesson the XFCE wallpaper row left behind: an exit code is not evidence that the thing happened.

33. **No textures on the host means no bundle, and one line saying so.** `artifacts::host_textures` already tells a Git LFS pointer from an asset by size and prints what it decided. Where it finds pointers there is no bundle to write and none to verify, so the run falls back to the plan's own verification, the loose binary is produced as before, and the summary line says the bundle was skipped and why. A bundle without the textures would be a bundle that renders a grid under a name that promises a release, which is worse than not writing one.

## The command

```
cargo xtask dist [--target <windows|linux|all>] [--keep] [--no-verify] [--no-cache]
                 [--allow-expired-image] [--allow-dirty]
cargo xtask vm purge <image|all> [--vm] [--image] [--iso] [--cache] [-f]
```

What one target does, with the two additions in place: read the pinned toolchain and the git facts and refuse a dirty tree; check the builder image and, unless `--no-verify`, the desktop image; boot a pristine overlay of the builder; probe for the toolchain; write and copy in the source archive; copy in whichever cache archives match this channel and this image; run the build job, which restores what it was given, builds, and packs what it produced, printing cargo's output as it arrives; collect the results, pull the cache archives the job says it wrote, and tear the builder down; check `deps.txt` on the host; assemble the bundle directory and write the archive its target names; boot the desktop image, stage the bundle, render twice, collect both, tear it down; write `target/dist/<target>/`; print where it is, what it was built from, whether it was warm, and what it needs to run.

## Steps

1. The bundle: `LICENSE` at the root, the `zip` and `tar` dependencies, a `bundle` module with the layout, the per-target name and format, the version read out of `Cargo.toml`, the two writers and the read-back check, all as pure functions with tests over a fabricated tree. Commit.
2. The cache: the store paths and the sidecar with its round-trip test, the match rule as one pure function over sidecar and facts, temp-then-rename, `Scope`'s fourth flag, and the inventory, status and purge accounting. No guest work yet. Commit.
3. The guest half: `zstd` in the Linux builder's package list, one `vm ssh windows-builder "tar --version"` to settle whether the Windows layer needs a zstd of its own and `toolchain.ps1` plus a layer rebuild if it does, then `CARGO_TARGET_DIR`, `-m` on the source extraction, the restore and pack clauses in `build_job`, the `cache.txt` the job writes to say what it packed, `Provider::copy_out` for one file, and the host's copy-in and copy-out around the job. The two generated scripts stay inside `script_syntax`'s generated list, and the batch half gets the same reading the `%%i` bug earned it. Commit.
4. `dist` itself: the bundle written after the linkage check and before verification, the verification staging the bundle and rendering twice, the grid comparison on the host, `--no-cache`, and `BuildInfo`'s two new sections. Commit.
5. Live, in this order, recording every figure: `dist --target linux` cold, then again at the same commit warm; the same two for Windows; `--target all`; one `--no-cache`. Record the archive sizes, the pack and unpack times, the transfer times, the build time warm against cold, and whether the warm and cold binaries of one commit are byte-identical, which is worth knowing either way and is not a criterion.
6. Docs: CLAUDE.md's dist paragraph and command list, `docs/vm-setup.md` (the release builds section's file table and two new paragraphs, the disk usage section's cache footprint and `--cache`), and `docs/roadmap.md`, where the release-builds item gains the bundle and the cache. Commit.
7. Gates at the tip: `cargo test`, `cargo clippy --all-targets` with zero warnings, `cargo fmt --check`, and the WSL leg with the known `tests/shading.rs` flake treated as it always is.

## Acceptance criteria

1. A second `dist --target linux` at the same commit restores both archives and is faster than the first by at least the dependency compile; the same for Windows; both figures recorded here beside the cold ones the plan already carries.
2. Both binaries pass the linkage checks the plan defines, warm and cold, and `build-info.json` says which of the two each was.
3. `--no-cache` neither restores nor saves, and the record says so.
4. A cache is refused after the channel in `rust-toolchain.toml` moves, and after the builder image is rebuilt, each with a line naming the field that moved. The second half can be shown by editing the sidecar, the way validator round 1 showed a detached layer by editing a manifest.
5. `vm status` counts the cache per image and names `vm purge <image> --cache`; `vm down <image>` leaves it alone; `vm purge <image>` with no flags takes it with the rest and lists it before it asks.
6. `target/dist/windows/` holds `sunlit-earth-<version>-windows.zip` and `target/dist/linux/` holds `sunlit-earth-<version>-linux.tar.gz`; each archive's entries are exactly its bundle directory's, and the Linux binary's mode in the tarball is 0755.
7. Unpacking the bundle on a machine that did not build it and running the binary with no environment set renders the Earth and not the grid, which is the two-render comparison in the desktop guest, and the smoke render in the dist directory is the one from the bundle.
8. With `textures/**` as Git LFS pointers there is no bundle, one line says why, and the loose binary and its record are produced as before.
9. Gates as in step 7 above.

## Risks

- **The transfer may cost more than the compile it saves.** Up to four gigabytes of build directory in and out of a guest per build, against four to six minutes of compiling. zstd is what is expected to settle it, and step 5 measures it rather than assuming. If it does not settle it, the fallback is the registry archive alone, which is the small one and the one that is usually not re-sent, and the target archive becomes a departure recorded with its numbers.
- **Fat LTO puts a floor under any cache.** `lto = true` and `codegen-units = 1` mean the final link reads every dependency's bitcode on every build, so a warm build is not a fast build, it is a build without the download and the dependency compile. Anyone reading the measurements should expect minutes rather than seconds.
- **zstd in the Windows guest is unmeasured.** With no uncompressed fallback, the worst case is that step 3 has to put a zstd binary in the layer and rebuild it, which is four minutes and a script that already downloads and unpacks a release archive for libclang. The measurement is one `tar --version` over SSH and it comes before any of the work that depends on it.
- **A cache makes one build depend on an earlier one**, which is the thing goal 4 was written against. The three guards in decision 19, the record in decision 27 and `--no-cache` are the whole of the answer, and the first comparison of a warm binary against a cold one at the same commit is where it is put to the test.
- **The bundle's texture lookup is a behavior of the app, not of the xtask.** If `resolve_textures_dir` ever stops walking up from the executable, the bundle silently ships a grid. Decision 32's second render is what makes that a failed run rather than a shipped defect, and it is why the check is a comparison rather than a header read.
- **The store grows.** Two archives per builder, so four in all, on top of 64.8 GiB. `vm status` is where that shows and `--cache` is what reclaims it, and the figure goes into `docs/vm-setup.md`'s budget once step 5 has measured it.
- **`LICENSE` is a repository-level change** rather than something this command needs, and it is here because the bundle is the first artifact this project produces that leaves the machine. The text is the GPL 3.0 as published; nothing about the declared license changes.

## Questions put, and what they came to

Four, all answered on 2026-08-29, and all written up above as decisions rather than left here. Nothing about this amendment is waiting on anybody.

1. **How the cache archives are stored.** Compressed with zstd, with no uncompressed fallback, and an image that cannot make one is fixed rather than worked around. Decision 22.
2. **What format the bundle takes.** Each platform's own: a zip for Windows and a `.tar.gz` for Linux, rather than one format for both. Decisions 29 and 31.
3. **Whether the loose binary stays in `target/dist/<target>/` now that a bundle exists.** It stays, with the bundle beside it. It is what the plan's acceptance criteria name, it is the easiest thing to run a host-side `render` against, and it is one file.
4. **Whether a Windows bundle should carry an install kit too.** It does not. There is nothing to install: the icon is a resource in the exe and there is no menu entry to place, so the `assets/` half of decision 30 is Linux only and the asymmetry is deliberate.

## Alternatives considered

**A persistent cache disk attached to the builder guest**, a second qcow2 or VHDX holding the cargo home and the build directory, mounted over them in the guest and left attached across boots. It is the faster answer in the steady state, because nothing crosses the SSH boundary at all, and it satisfies the same "no directories of small files on a Windows host" argument the archive does, since a disk image is one file too. It is not taken here because it reaches much further: both providers' VM creation, both guest images (a mount unit on Linux, a drive letter and junctions over `%USERPROFILE%\.cargo` on Windows), the one-overlay-per-boot model, and the teardown and purge rules all have to learn about a disk that is neither the golden image nor a throwaway. A read-write disk attached to a guest is also corruptible by a crash mid-build in a way a temp-then-rename archive on the host is not. This is what to reach for if step 5 measures the transfer as the dominant cost and zstd does not bring it down.

**Warming the registry into the builder images at build time**, which the plan already declined as its decision 18 and departure 2. The reasons stand and the cache subsumes it: an image that carried a registry would carry the lockfile of the day it was built, and would have to be rebuilt to follow the tree, where a cache follows it by itself.

## Departures and validation record

Recorded in this document the way the plan records its own, appended as the work goes: a departure for anything built differently from what is decided above, with the reason, and a validation record with the live figures, each round's findings and what each came to.

### Departures

1. **The commits do not line up one to one with the steps.** The steps put the bundle module in commit 1 and the `dist` command that uses it in commit 4, and the same for the cache across commits 2, 3 and 4. A module nobody calls yet is a wall of dead-code warnings, and `cargo clippy --all-targets` with none of those is one of the gates, so committing in that order means three commits whose gates are red on purpose. The work is therefore grouped by what can stand on its own: step 1 with the bundle half of step 4, then step 2 with the accounting that already has callers, then step 3 with the cache half of step 4. Nothing about what is built changes, only where the commit boundaries fall.

2. **The record goes into the bundle after the verification boot rather than before it.** The command's own description assembles the bundle directory and writes the archive, and only then boots the desktop image. Done that way, the `build-info.json` inside the bundle stops short of `verified_in` and of what the two renders measured, while the copy beside it in the dist directory carries both: two files of the same name with different contents, and the one a user actually receives is the poorer of the two. So the directory is assembled with the record as it stands, staged and verified, and then that one file is rewritten in place before the archive is written. The archive is therefore made after the verification rather than before it, which also means a bundle that failed its own texture-lookup check is never written at all. What it cannot carry either way is the archive's own size, since that is a number the archive would have to contain about itself; the bundle's section of the record names what the bundle is instead.

3. **The bundle is assembled in the run directory of the guest it is staged into, and `dist` removes it when a target is done.** The plan says the bundle is built as a directory first and does not say where. It is run state by every test the store already applies: it is made per run out of what that run produced, it is copied into a guest that is about to be destroyed, and it is worth nothing afterwards. So `Store::bundle_scratch` sits beside `job_scratch` and `handover_scratch`, `vm::run_state_paths` takes it with the rest, and `dist` deletes it at the end of each target whichever way the target went. That is the opposite of the cache, which decision 28 keeps outside `run_state_paths` for exactly the same reason read the other way round: a teardown must never take it.

## Validation record

### The zstd probe in the Windows builder, 2026-08-29

Decision 22's one open measurement, and the risk it carried, asked with `vm smoke
windows-builder --keep` and three `vm ssh` lines rather than a `vm up`, which would have
cost a host build of the e2e suite for nothing.

| | |
|---|---|
| the guest's `tar.exe` | **bsdtar 3.8.1, libarchive 3.8.1, `libzstd/1.5.5`** linked in |
| a standalone `zstd.exe` | absent, and not needed |
| `tar --zstd -cf` | packs; `tar -xf` unpacks it again and `tar -tvf` lists it |
| `tar -xmf` | stamped the extraction at 2026-08-29, where the same archive without `-m` restored the archived 2001-01-01 |

So the Windows layer needs no zstd of its own, `toolchain.ps1` is untouched, and there is
no layer rebuild. The host's own `tar.exe` is bsdtar 3.8.4 and the guest's is 3.8.1, which
is the same libarchive line with the same zstd support; decision 23's mechanism was
measured on both rather than assumed to carry from one to the other. The guest was torn
down afterwards and the store went back to 64.9 GiB.

### `cargo xtask dist --target linux`, cold and warm

Three runs on 2026-08-29, the first two of which failed at the last step for two
different reasons, both recorded below because the first was a real defect and the
second is worth knowing about the tooling.

| | cold, of `f273bd2` | warm, of `3efb649` | warm again |
|---|---|---|---|
| the cache going in | nothing on this host yet | registry 122.3 MiB, target 612.6 MiB | the same, one build newer |
| the build itself | **5m 28s** | **3m 49s** | **2m 59s** |
| the whole target | did not finish | did not finish | **4m 35s**, two boots included |
| the cache coming out | registry 122.3 MiB, target 612.6 MiB | target only | target only |
| the binary | 31,852,608 bytes | the same | the same |
| the bundle | 19 files | 19 files | 19 files, **27.5 MiB** as `.tar.gz` |
| the two renders | **23.5** of a channel step apart | 23.4 | 23.3 |

**The warm build is the headline: 3m 49s against 5m 28s, and 2m 59s on the third run**,
where the cold figure includes downloading 519 crates and compiling every one of them.
What is left is the workspace's own crates and the fat-LTO link, which is the floor the
risks section predicted and which no cache can take away. The third run is faster than
the second because the second's cache was made by a build whose own `target/` was cold:
the second run compiled the workspace crates into a directory that already held every
dependency, and the third restored *that*.

**Decision 26 held on the second run and every one after it**: the registry was restored
and not re-saved, because `Cargo.lock` had not moved, so 122 MiB stayed on the host
instead of crossing the wire twice. `build-info.json` says so per archive, which is
decision 27 in the file rather than in a sentence: `"restored": true, "saved": false` for
the registry and `true`/`true` for the build directory.

**The warm binary and the cold binary are byte for byte the same**, sha256
`fc89d33af112ceb8c271cba81ae8cad66684c483d84934f36653adb2ccbf5167` on both. That is
worth knowing rather than a criterion, and it is slightly stronger than the question
asked: the two runs were of different commits, which differ only in files under
`crates/xtask/`, and `cargo build -p sunlit-earth` compiles none of those.

**Decision 32 measured, which is the number the threshold was waiting for.** The bundle's
own render and one made against an empty textures directory are **23.3 to 23.5 of a
channel step apart**, against a floor of 8.0 in `dist::TEXTURE_LOOKUP_FLOOR`. The three
runs agree to two tenths, which is the Earth turning between them, so the floor sits
roughly three times above the signal and a hundred times above the noise.

The tarball, read back on the host with a tool that had nothing to do with writing it:
19 entries, all under one `sunlit-earth-0.1.0-linux/` directory, `sunlit-earth` and
`assets/linux/install-user.sh` at **0755** and everything else at 0644, which is
acceptance criterion 6. `target/dist/linux/` holds the binary, `build-info.json`,
`build.log`, `smoke.png` and the 27.5 MiB tarball.

**The first failure was a real defect, and the live pass is what found it.** Both the cold
run and the first warm run got as far as the two-render comparison, proved the textures
were found, and then failed to write the final record into the bundle: the verification
guest's own teardown had deleted the bundle directory, because departure 3 had put it on
`vm::run_state_paths`. That list is what a run's own teardown removes as it ends a guest,
and the bundle outlives the guest it is staged into by the few seconds it takes to seal
it. Fixed in `3efb649` by taking it off that list and leaving it in the run directory,
where `vm status` still counts it and `vm down` still sweeps it, because those take the
directory whole rather than reading the list. The test walks a teardown over a bundle
scratch and fails on the old code.

**The second failure was not a defect and cost a run anyway.** `target/debug/xtask.exe`
was still the binary from before the fix, because `cargo test -p xtask` builds the test
binary and not the bin, so the run that was meant to prove the fix exercised the bug
again. Worth writing down for anyone driving live runs from a built binary rather than
through `cargo xtask`.

`vm status` after the cold run, which is acceptance criterion 5's first half:

```
linux-builder: ok (current)
  build cache: 4 files (734.8 MiB); `cargo xtask vm purge linux-builder --cache` frees it
  footprint: 3.1 GiB image, 607 B run state, 734.8 MiB build cache
total: 65.5 GiB
  64.8 GiB in golden images and media, 1.2 KiB in run state, 734.8 MiB in build caches
  `cargo xtask vm purge all --cache` frees the build caches; the next release build is then a cold one
```

### The Linux builder with `zstd` in it

`zstd` joined the package list (decision 22) because GNU tar's `--zstd` shells out to that
program rather than linking a library, which is the one thing the Windows guest did not
need. Rebuilt in **1 minute 17 seconds** to **3,305,504,768 bytes (3.08 GiB)**, which is
44 MB *smaller* than the image without it: the difference is what a fresh `apt` run left
behind rather than anything zstd added. `cargo xtask vm smoke linux-builder` passes on it
with `cargo 1.94.0` from the probe.
