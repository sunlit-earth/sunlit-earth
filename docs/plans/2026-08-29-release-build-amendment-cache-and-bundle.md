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

3. **The bundle is assembled in the run directory of the guest it is staged into, and `dist` removes it when a target is done, but it is not on `vm::run_state_paths`.** The plan says the bundle is built as a directory first and does not say where. It is run state by every test the store already applies: it is made per run out of what that run produced, it is copied into a guest that is about to be destroyed, and it is worth nothing afterwards. So `Store::bundle_scratch` sits beside `job_scratch` and `handover_scratch` in the run directory, `dist` deletes it at the end of each target whichever way the target went, and `vm down` and `vm purge` sweep one that a dead run left behind, because both of them take the run directory whole rather than reading a list.

    The list it is off is `vm::run_state_paths`, and it was on it for the first two live runs, which is what those runs found. That list is the shorter one a run's *own* teardown deletes as it ends a guest, and the bundle is the one thing in the run directory that outlives the guest it was staged into: departure 2 writes the final record into it after the verification boot, so that boot's teardown deleted the bundle whose textures it had just proved. Taking it off the list is `3efb649`, with a test that walks a teardown over a bundle scratch and fails on the old code. The cache is outside `run_state_paths` too, and decision 28 puts it there for the same reason read the other way round: a teardown must never take minutes of compiling that nothing recreates.

4. **Decision 26's registry rule asks about a cache that was restored, not about any sidecar on disk.** The decision says the registry is skipped when its recorded lockfile hash matches this build's, and that is right for the case it was written for: a warm build whose registry came off the host has nothing new to send back. It is wrong for a refused one. A sidecar refused for a moved channel or a rebuilt image still records the lockfile its archive was made from, so on the literal rule a run that had just declined to restore the registry also declined to replace it, and the archive stayed there being refused by every later build while none of them ever wrote a fresh one. The registry would have been cold until `Cargo.lock` happened to move, which on a settled dependency tree is not soon. The same shape reached further through the sidecar reader, where an unreadable sidecar left the loop before the save decision was reached at all, so one corrupt file froze both archives rather than one. So `plan_cache` now asks `registry_worth_saving` about the sidecar this run actually restored and about no other, and a sidecar that cannot be read is a reason to pack rather than a reason to stop. Decision 26's sentence stands with its scope named: a recorded hash is evidence only while the cache it describes is the one the host will hand over next time. Both halves have an assertion that fails on the code as it was, and it was found by building acceptance criterion 4's tamper, which is the only situation on this branch where a sidecar is refused and the run then carries on.

5. **Decision 23's `-m` is no longer the only thing standing between a warm build and a binary of the previous commit.** The decision's whole argument is that the extracted source is newer than the restored artifacts, so cargo rebuilds this workspace's crates. That is an argument about one clock: a builder guest whose time ran behind the modification times inside `target.tar.zst` would present cargo with sources that look older than the artifacts, every unit would be judged fresh, and the job would copy out the previous commit's binary under this commit's `build-info.json`. Nothing has been seen to do this and every live run rebuilds as expected, so it is a residual hole rather than a defect, and it is the one failure this design must never have. So a restored build directory now gives up two things before the build starts: every `.fingerprint/sunlit-*` directory, which makes `sunlit-core` and `sunlit-earth` units cargo compiles again whatever the times say, and the release binary itself. Deleting the binary alone, which is what validator round 1 suggested, does not close it: cargo notices a missing output and relinks it out of whatever rlibs it still believes in, so the result would be a binary of the previous commit reached by a slightly longer road. The fingerprints are the half that decides it, and the binary goes with them so that a link which did not happen cannot be copied out as this commit's. It costs nothing: those crates are recompiled and relinked on every warm build already, which decision 23 says in its last sentence, and what the cache is for is the dependency tree below them. `a_restored_build_directory_gives_up_this_workspace` holds both halves in both jobs, and their absence from a job that restored nothing.

6. **`--no-verify` writes the bundle, and the bundle says nothing ran it.** `docs/vm-setup.md` claimed the flag skipped the archive as well as the boot, "since a bundle is sealed only once the verification has passed", and the code never read the flag there: `one_target` seals whatever was assembled. The doc was the wrong half. Departure 2 makes the archive the last thing a target writes, which is what made "sealed after the verification" sound like "sealed only if verified", but withholding the artifact would make a deliberately quick build useless, and CLAUDE.md already described the flag correctly as skipping the boot. So the sentence is corrected rather than the behaviour. What the flag really costs is the only thing that boot bought, so an unverified bundle now says so in three places instead of one: `bundle::summary` carries the caveat on the line that names the archive, the closing summary already said "not verified", and `verified_in` and `BundleInfo::texture_lookup_delta` are serialized as `null` rather than skipped. Those two are the only optional fields in the record written when they are empty, and the reason is that the record travels inside the bundle: the plan's own validation record defended leaving `verified_in` out as the way a Linux record leaves out a Windows field, which was right while the record only ever sat in the dist directory beside the command that wrote it. A person unpacking an archive is not that person, and a field that is not there reads as one the writer had no answer for.

7. **The pack, unpack and transfer times are instrumented rather than taken once.** Step 5 lists them beside the archive sizes and the build durations, and the first pass measured everything but these: the job said `cache: unpacking ...` and nothing about how long it took, and the host timed neither of its copies, so the figures could not have been quoted from a log without inventing them. The whole-target totals already settle the risk they were for, since a warm Windows target is 5m 59s against 8m 29s cold and 9m 49s with `--no-cache`, but the plan's discipline is that an unmeasured figure is recorded as unmeasured, so the cheaper of the two honest endings was not the one taken. Every cache step now says what it cost as it happens, and `cache::Report` carries `copied_in_secs` and `copied_out_secs` into `build-info.json`, which is where decision 27 already puts what the cache did. The two jobs say it differently and deliberately: bash has `SECONDS`, so the Linux job prints a duration, and `cmd.exe` has no arithmetic on its own clock that is not either a process spawn per reading or a bet on the locale's time format, so the Windows job prints `%TIME%` on either side of each step and leaves the subtraction to the reader. Two readings that cannot be wrong beat one number that can, and a release build is the wrong place to spend eight process spawns on instrumentation. `every_cache_step_says_what_it_cost` holds both shapes and their absence from a cold job.

8. **The Windows drop of this workspace's fingerprints may fail the build, and nothing else in the cache region may.** Departure 5 put that drop in both jobs. On Linux it is one `rm -rf` under `set -euo pipefail`, so a directory that will not go ends the build; on Windows it was a `for /d ... do rmdir /s /q` loop with nothing reading the result, which validator round 2 named as the asymmetry it is. Everything else in that region is a convenience whose failure costs a slower build and must not stop it, which is why nothing there may `exit /b`, but this drop is the guarantee departure 5 exists to give, and a guarantee that quietly did not happen is worse than a build that stopped. The trigger is narrow and was not reproduced: a handle held on one `release\.fingerprint\sunlit-core-<hash>` in the moments after `tar.exe` created it, by Defender or the indexer, while the rest of the loop deletes cleanly. Its consequence is the one thing this design must never do, since in the skewed-clock case departure 5 was written for cargo would then call `sunlit-core` fresh and link the previous commit's rlib into this commit's binary and record. So the job looks again after the loop and refuses to build while a `sunlit-*` directory or the cached binary is still there, naming what it found. It looks at the directories rather than at an errorlevel because `rmdir` runs inside the loop and what the loop leaves behind is its last iteration's verdict, so a failure on the first of two would be masked by the second succeeding. Both halves of the clause were run against a real `cmd.exe` before they were written into the job: with nothing holding the files it drops the two `sunlit-*` directories and the exe, leaves `wgpu-*` alone and exits 0, and with a handle open on a file inside one of them it prints that path and exits 1. `a_drop_that_did_not_happen_ends_the_build` holds both jobs and fails on the code as it was.

9. **A job's last output is read once more after its exit code, and the defect that made it necessary predates this branch.** `wait_for_exit_code` reads the guest's log before it asks whether the job is over, and the comment on that order claimed the last of the output was therefore printed "even when both land in the same poll". The two are separate SSH round trips, so a job that finished between them returns with its tail never printed. It happened live in this range: `dist-windows-timed.log` stops at `cache: packing the build directory at 18:18:02.31` where `target\dist\windows\build.log`, the same file pulled out of the guest afterwards, carries the `ended at` reading and the listing after it. The record stayed honest and only the console lost anything, which is why it is a minor. The code is the parent branch's rather than this range's, but this range is what made that console the place `docs/vm-setup.md` tells a person to watch a build, and departure 7's step timings are printed into it, so it is fixed here rather than left for the next branch to inherit. What the extra read costs is one round trip per job, at the end of one. It cannot print anything twice, because `OutputTail` measures what it has already printed against the whole log it is handed on every read and answers nothing when there is nothing new; the one thing it still holds back is a trailing replacement character, which after the exit code exists is a byte sequence that was never going to become a character, since the runner writes that file once the job's own output is closed. `the_last_of_the_output_survives_a_job_that_ends_mid_poll` is the case, and it needs a fake guest that changes under a wait, which is `FakeRunner::on_each`: a fixed table can only describe a guest that answers the same thing twice, and the whole of this is about the two answers differing.

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

### `cargo xtask dist --target windows`, cold and warm

Both green end to end at the first attempt, of `3efb649` and `a2e6407`.

| | cold | warm |
|---|---|---|
| the cache going in | nothing on this host yet | registry 118.5 MiB, target 619.1 MiB |
| the build itself | **6m 58s** | **4m 04s** |
| the whole target | **8m 29s** | **5m 59s** |
| the cache coming out | registry 118.5 MiB, target 619.1 MiB | target only |
| the binary | 29,183,488 bytes, 26 imports, none of them the Visual C++ runtime | the same |
| the bundle | 9 files | 9 files, **25.4 MiB** as a zip |
| the two renders | **19.2** of a channel step apart | 19.1 |

The saving is nearly three minutes on a seven-minute build, and it is the same shape as
the Linux one: what a warm build still pays for is the workspace's own crates and the fat
LTO link. The Windows registry archive is 118.5 MiB against Linux's 122.3 for the same
lockfile, and the build directories are within a percent of each other at 619.1 and 612.6
MiB, which is the two toolchains producing much the same bitcode from the same crates.

`bsdtar --zstd` in the guest did what the probe said it would, with no zstd binary
anywhere and no change to the layer.

The zip, read back with a reader that had nothing to do with writing it, is decision 31's
compression rule as a measurement rather than a claim:

| entry | on disk | in the zip | method |
|---|---|---|---|
| `sunlit-earth.exe` | 29,183,488 | 12,548,176 | deflated |
| the four `.jxl` textures | 14,117,036 | 14,117,036 | **stored** |
| `LICENSE` | 35,149 | 12,122 | deflated |
| `PROVENANCE.md`, `build-info.json`, `ATTRIBUTION.md` | 8,742 | 4,042 | deflated |

So the binary compresses to 43% and the imagery to 100%, which is what storing already
compressed data is for: deflating the four JXL files would have spent time to make the
archive very slightly larger. Nine entries, all under one `sunlit-earth-0.1.0-windows/`
directory, and no `assets/` at all, which is decision 30's deliberate asymmetry: there is
nothing to install on Windows, because the icon is a resource inside the exe.

### `cargo xtask dist --target all`, both targets warm

Of `a2e6407`, clean tree, verification on: **11 minutes 17 seconds for both targets
through four boots**, exit 0, two summary lines. Windows built in 6m 53s and Linux in
4m 23s, each guest torn down before the next one booted.

Against the plan's own `--target all` figures on the same host before there was a cache,
which were 7m 46s and 5m 21s for thirteen minutes in total:

| | plan, cold | here, warm |
|---|---|---|
| Windows | 7m 46s | **6m 53s** |
| Linux | 5m 21s | **4m 23s** |
| both, wall clock | ~13m | **11m 17s** |

The per-target saving is smaller here than in the single-target runs above, and the
reason is the transfer rather than the compile: each target now copies 740 MiB in and
620 MiB back out on top of its build, and `--target all` pays that twice in one wall
clock. The build times themselves are the ones to read: 4m 35s and 2m 50s inside the
guests, against 6m 58s and 5m 28s cold.

**Three Linux binaries of three different runs are byte for byte identical**, sha256
`fc89d33af112ceb8c271cba81ae8cad66684c483d84934f36653adb2ccbf5167` on all of them: the
cold one, the warm one, and this one. The Windows binary of this run is
`89d8718c3270bfe60b9d0fdcc0b4e9e91af2ac00038b8f8caec8271cb332863d`; the cold Windows
binary was overwritten by the warm run before it was hashed, so the Windows half of that
comparison is still to make, and the `--no-cache` run is where it belongs.

The two-render difference moved from 23.3 to **22.6** on Linux between two runs eleven
minutes apart, which is the Earth turning. The floor of 8.0 has three times that of
headroom and the drift is a tenth of the margin.

### `cargo xtask dist --target windows --no-cache`

Of `c1ca9af`, clean tree, verification on, green end to end. The last run of step 5, and
the one that makes acceptance criterion 3 a measurement.

| | |
|---|---|
| the cache going in | nothing, and the reason is in the record |
| the build itself | **8m 10s** |
| the whole target | **9m 49s** |
| the cache coming out | nothing |
| the binary | 29,183,488 bytes, 26 imports, none of them the Visual C++ runtime |
| the bundle | 9 files, 25.4 MiB as a zip |
| the two renders | **22.7** of a channel step apart |

`build-info.json`'s `cache` section, which is criterion 3 in the file rather than in a
sentence: `{"archive": "registry", "restored": false, "reason": "--no-cache was given",
"saved": false}` and the same for `target`. Neither half ran, and both said why.

**Cold is not one number.** This build compiled everything from nothing, as the cold run of
`3efb649` did, and took 8m 10s against that one's 6m 58s. So the cold figure carries more
than a minute of spread on this host, and the warm saving is a saving of minutes rather
than a figure good to the second. What the four Windows runs put a range on is the
distance between the build and the whole target, which is 1m 31s to 2m 18s of two boots and,
where there was a cache, two transfers.

**The Windows half of the byte-identity question cannot be answered by hashing, and this
run is what settled that.** The binary is
`78752bcb50bc7a6f6cf0458792c837b0e9fc04c48cb54217dc10589ee49ebba5` against the warm run's
`89d8718c3270bfe60b9d0fdcc0b4e9e91af2ac00038b8f8caec8271cb332863d`, and the cause is not
the cache: the PE header's `TimeDateStamp` in this binary reads 2026-08-29T16:58:54Z, which
is the minute the link finished. Every MSVC link stamps its own wall clock into the file
unless `/Brepro` is passed, which nothing here passes, so no two Windows builds of one
commit can ever hash the same however they were built. What that does not prove is that
nothing *else* differs, and proving it would take two more Windows runs compared with the
timestamp and the debug directory normalized away, which is not worth twenty minutes for a
question the Linux side already answers: three Linux binaries of three runs, one cold and
two warm, are byte for byte identical, and that is the comparison the "a cache makes one
build depend on an earlier one" risk actually needed.

The two-render difference is 22.7 here against 19.2 and 19.1 on the two Windows runs two
and a half hours earlier, which is the Earth having turned thirty-seven degrees between
them. Both ends of that are three times the floor of 8.0.

### `vm down`, `vm status` and a purge: acceptance criterion 5

Three commands, no boot, in that order on 2026-08-29 with a warm Linux cache on disk.

`cargo xtask vm down linux-builder` deleted one file, `vm.log` at 607 B, and said "the
golden images are untouched". `vm status` afterwards read `no overlays or run state` and,
on the next line, `build cache: 4 files (734.8 MiB); cargo xtask vm purge linux-builder
--cache frees it`, with the footprint line naming the cache as a third figure beside the
image and the run state. So the cheap teardown leaves the expensive thing alone, which is
decision 28 as a measurement.

`cargo xtask vm purge linux-builder` with no flags listed all four cache files first, ahead
of the golden image and Packer's leftovers, and asked to "delete the VM and its run state,
the golden image, the installation media, and the build cache for linux-builder, freeing
4.5 GiB?". Answered `n`: "nothing was deleted", and the four files were still there
afterwards.

### A cache refused twice over: acceptance criterion 4

Both halves in one run, because the two archives can be refused for different reasons at
once. Before `dist --target linux` of `77858d5`, `registry.json`'s `channel` was edited from
`1.94.0` to `1.93.0` and `target.json`'s `image_built_utc` from the image's real
`2026-08-29T11:19:05Z` to `2026-08-28T09:00:00Z`, both saved first and put back afterwards.
The run's own two lines:

```
  cache:   registry: cold (the pinned toolchain channel was 1.93.0 and is 1.94.0 now)
  cache:   target: cold (the builder image's build time was 2026-08-28T09:00:00Z and is 2026-08-29T11:19:05Z now)
```

The criterion names `rust-toolchain.toml` for the channel half and the sidecar for the
image half. Both were done on the sidecar, and it is the same experiment: `restorable`
compares the sidecar's field against the fact, so moving either side exercises the one
comparison and produces the one message. Moving the pin would additionally have made the
guest install a second toolchain, which tests rustup rather than the cache.

**What the tamper found is departure 4**, and it is a defect rather than a curiosity: the
refused registry was then not packed again either, because its recorded lockfile hash still
matched this build's. On the code as it stood, a real channel bump or a real image rebuild
would have left the registry archive refused by every later build and replaced by none of
them, and every release build would have downloaded 519 crates until `Cargo.lock` moved for
some other reason. The live run is where that is visible from both ends. The record says
`"archive": "registry", "restored": false, "saved": false` beside the target's
`"restored": false, "saved": true`, and on disk `target.json` had healed itself, carrying
this run's commit and the image's real build time, while `registry.json` still said
`1.93.0` and still carried the write time from four hours earlier. Fixed in `plan_cache`,
with the two assertions departure 4 names, and the tampered sidecar put back from its
backup afterwards so the store is consistent again.

### A checkout with no textures in it: acceptance criterion 8

The same run, from a second checkout rather than from the worktree. `SUNLIT_EARTH_REPO`
pointed at a `GIT_LFS_SKIP_SMUDGE=1` clone of the same commit, whose four `textures/*.jxl`
are the 131 and 132 byte pointer files a checkout without the LFS objects holds, and whose
tree is otherwise clean, so the dirty-tree refusal never came into it and nothing had to be
moved aside in the worktree. That is the criterion's own condition rather than an
imitation of it, and it costs one `git clone` of 40 MB.

What the run said, in the two lines decision 33 asks for, the first naming the file it
measured and the second saying what follows:

```
  no bundle: world.topo.200405.jxl is 132 bytes, which is a Git LFS pointer rather than the asset; `git lfs pull` fetches it
  no bundle: this checkout holds Git LFS pointers rather than the texture assets, and a bundle without them would render the procedural grid under a name that promises a release. `git lfs pull` fetches them; the loose binary and its record are in the dist directory either way.
```

It then did exactly what the plan did before there were bundles: staged the loose binary
into the Linux desktop guest, rendered 640x360 at 108.2 KiB, and wrote
`<repo>/target/dist/linux/` with the binary, `build-info.json`, `build.log` and
`smoke.png` and no archive. The record carries no `bundle` section at all rather than an
empty one, and `verified_in` still says `linux`. Exit 0, the whole target 7m 17s with a
build of 6m 06s, which is a third cold Linux figure beside 5m 28s and the warm ones.

The smoke render is 108.2 KiB against the 398.3 KiB of the Windows bundle's, which is the
same fact from the other side: a procedural grid compresses to a quarter of what the Earth
does, and that is exactly why the header check the plan started with could not tell the two
apart and decision 32 measures the pixels instead.

**A fourth Linux binary, and the strongest form of the comparison.** This build hashed
`fc89d33af112ceb8c271cba81ae8cad66684c483d84934f36653adb2ccbf5167`, the same sha256 as the
cold one, the warm one and the `--target all` one. It was built with *both* archives
refused, from a different checkout on the host, at a commit three ahead of the first: four
builds, two of them warm and two cold, one binary. Whatever a cache can do to a Linux build,
this says it did not do it.

### The soak test in a whole-workspace run

`cargo test -p sunlit-core --test soak` passes alone and `cargo test -p sunlit-core`
passes with it in the middle, both repeatedly. `cargo test` over the whole workspace
failed it twice and then passed it twice, on an unchanged tree. Nothing on this branch
touches `sunlit-core`: what it changes in the workspace manifest is three dependencies
`xtask` alone consumes, and `dist` compiles neither.

Left as an observation rather than chased, and it belongs to the project rather than to
this amendment. The shape matches the "one GPU device at a time" constraint CLAUDE.md
already records: `GPU_SERIAL` serializes within a process and `cargo test` runs separate
test binaries as separate processes, so a target added anywhere in the workspace can move
that timing without being the cause. What is not yet known is whether it fails the same
way at `13fbb88`, which is the question a successor should settle before anyone spends
more on it.

The observation now has a home outside this plan: `docs/roadmap.md` carries it under Bugs
and polish, beside the WSL `tests/shading.rs` flake it resembles, with the question about
`13fbb88` written down as the first thing to establish.

### The acceptance criteria, and where each was shown

| | shown by | what it came to |
|---|---|---|
| 1 | the four cold and warm runs above | Linux 3m 49s against 5m 28s, Windows 4m 04s against 6m 58s, both archives restored |
| 2 | every run's own linkage line, and `build-info.json` | 26 imports and none of them the Visual C++ runtime; glibc 2.35 and the four Linux libraries; the `cache` section says per archive which of the two the build was |
| 3 | `dist --target windows --no-cache` | `"restored": false, "reason": "--no-cache was given", "saved": false` for both archives |
| 4 | the tampered sidecars | two refusals in one run, each naming the field that moved, and departure 4, which the tamper is what found |
| 5 | `vm down`, `vm status` and a no-flag purge | 607 B of run state taken, 734.8 MiB of cache left, and all four cache files listed before the question |
| 6 | reading both archives back | 19 entries under one directory with 0755 on the Linux binary, 9 under one directory on Windows with the JXL stored |
| 7 | the two renders in the desktop guest | 19 to 23 channel steps against a floor of 8.0, and `smoke.png` is the bundle's own render |
| 8 | the pointer-only checkout | no bundle, two lines saying which file and why, and the loose binary verified and published as before |
| 9 | step 7's gates at the tip | below |

### The gates at the tip

Step 7 and acceptance criterion 9, run at `0c55fc2`. Only `CLAUDE.md` and this document
moved after that, and the one test that reads either is
`the_docs_spell_out_every_flag_dist_takes`, which was re-run green at the tip along with
`cargo fmt --check`, `cargo clippy --all-targets` and the whole of `cargo test -p xtask`.

| gate | result |
|---|---|
| `cargo test` on Windows | green, thirteen targets, no warnings: 450 unit, 54 engine, 14 golden, 21 render pipeline, 12 shading, the soak in 60.2s, 46 and 2 and 27 in the app, 536 in xtask, 11 e2e cases ignored as they always are |
| `cargo clippy --all-targets` | **zero warnings**, workspace-wide |
| `cargo fmt --check` | clean |
| the WSL leg | green on the second run, thirteen targets, 525 in xtask, which is 536 less the Windows-only cases |

The WSL leg's first run failed the way `docs/roadmap.md` says it does: `tests/shading.rs`'s
`software_adapter_produces_correct_results` panicking on `Result::unwrap()` of `BadAccess`
at `wgpu-hal-28.0.1/src/gles/egl.rs:308`, which took the remaining targets with it because
`cargo test` stops at the first failing one. The second run passed that target in 0.16s and
everything after it. That is the documented flake at its documented rate and not something
this branch touched: nothing here compiles `sunlit-core`.

The soak test passed inside the whole-workspace `cargo test` on Windows here, which is the
other half of the observation above rather than a contradiction of it: two failures and
three passes now, on trees that differ by nothing it compiles.

### What the cache itself costs, and validator round 1's three minors

Round 1 found no majors and three minors. Two were things the code did not do that a
document said it did, and the third was step 5's pack, unpack and transfer times, which
were never measured because nothing was instrumented to measure them. All three are closed
here, in departures 5, 6 and 7, and three live runs at `f8a1d52` are what says so.

**The times, which is the ending that was taken.** Step 5's figures could have been
recorded as not itemized, since the whole-target totals already settle the risk they were
for, but the plan's discipline is that an unmeasured figure is measured or recorded as
unmeasured, and instrumenting it is cheap. So each cache step now says what it cost as it
happens and the host puts its own two transfers in the record.

| | Linux, two runs | Windows |
|---|---|---|
| registry, 122.3 / 118.5 MiB, into the guest | under 1s, 1s | under 1s |
| registry, unpacked in the guest | 5s, 5s | 62.2s |
| build directory, 612.6 / 619.1 MiB, into the guest | 4s, 4s | 3s |
| build directory, unpacked in the guest | 11s, 13s | 21.3s |
| build directory, packed in the guest | 9s, 23s | 12.4s |
| build directory, back out to the host | 15s, 18s | 3s |
| the whole cache round trip | 44s, 64s | 1m 42s |
| cargo's own time in the same run | 3m 54s, 5m 00s | 5m 40s |

The registry is restored on every build and packed only when `Cargo.lock` has moved, so its
two figures are a restore cost with no matching save, which is decision 26 working.

Two things in that table are worth reading twice. **A registry unpacks twelve times slower
in the Windows guest than in the Linux one**, 62.2s against 5s for an archive of the same
size, which is exactly the tens-of-thousands-of-small-files cost decision 21 named as the
reason the guest unpacks rather than the host copying a tree in. And **the Windows cache's
own round trip is 1m 42s against a cold build's 6m 58s and a warm one's 5m 40s here**, so
on that target the cache is a smaller win than the build times alone suggest: it is still a
win, because a cold registry is a full index and crate download rather than a 62 second
unpack, but the margin is real and it is now written down rather than assumed. The Linux
side is not close: 44s to 64s against the same saving. Nothing is changed in response,
because the numbers carry more than a minute of spread (cargo took 3m 54s and 5m 00s on two
runs of the same sources), and a design decision taken on figures that noisy would be a
guess wearing a table.

**A restored build directory now gives up this workspace, and the runs show it.** Both jobs
printed `cache: dropping this workspace out of the restored build directory` and then
compiled exactly `sunlit-earth` and `sunlit-core` and nothing else, which is what departure
5 asks for: the two crates of this tree are rebuilt from the extracted source whatever the
guest's clock says, and the dependency tree below them comes out of the cache. Both binaries
verified in their desktop guests afterwards, at 21.5 and 23.9 channel steps from the grid
against the floor of 8.0.

**`--no-verify` writes a bundle that says nothing ran it.** The third run was
`dist --target linux --no-verify`, which had neither a test nor a live run before. It wrote
`target/dist/linux/` with the archive in it, printed the caveat on the line naming the
archive, and closed with "linux: built in 6m35s and not verified". Both copies of the
record, the one beside the archive and the one inside it, carry `"verified_in": null` and a
null `texture_lookup_delta`. That is the correction departure 6 makes to
`docs/vm-setup.md`, shown from both ends.

### The gates at the tip of the minors

Run on Windows over the whole of this work: `cargo test -p xtask` with `clippy` and `fmt`
at each of the three code commits, and all three gates whole on the tree that became the
tip, which is `f8a1d52`'s code with this document and the two guides on top of it.

| gate | result |
|---|---|
| `cargo test` | green, thirteen targets, 538 in xtask against 536 before these three |
| `cargo clippy --all-targets` | **zero warnings**, workspace-wide |
| `cargo fmt --check` | clean |

The WSL leg was not re-run. Nothing here compiles differently there: the three changes are
xtask's generated job text, one serde attribute pair and two prose files, and `cargo test -p
xtask` is the whole of what covers them.

### Validator round 2's two minors, and the run that shows both, 2026-08-29

Round 2 found no majors and two minors, and both are about a Windows build saying what it
did: the fingerprint drop that could fail without anyone hearing it (departure 8) and the
console that could lose a job's last lines (departure 9). One run proves both, because the
Windows job is where each of them lives: `cargo xtask dist --target windows` at `9afae7c`,
over the warm cache the earlier runs left behind, **built in 7m54s and verified in the
desktop guest**, with the bundle's own render 23.9 channel steps from the grid and the
25.4 MiB zip read back and matched.

**The drop is loud and it passed.** The console shows `cache: dropping this workspace out of
the restored build directory` and then exactly `sunlit-earth` and `sunlit-core` compiling
and nothing else, with no refusal line between them: on a real restore of a 619.1 MiB build
directory every `sunlit-*` fingerprint and the cached binary went, which is what the new
check asks about. That the check can also fail was measured where it would happen rather
than in the job text alone, against a real `cmd.exe`: with nothing holding the files it drops
the two `sunlit-*` directories and the exe, leaves a `wgpu-*` fingerprint alone and exits 0,
and with a handle open on a file inside one of them it prints that path and exits 1. The
line it exits from leans on `if COND cmd & exit /b 1` binding both halves to the condition
inside a batch file, which is the opposite of what a command prompt does with the same text,
so that was probed too, twice and independently.

**The last of the guest's output is on the console now.** The run before this one stopped at
`cache: packing the build directory at 18:18:02.31` and said nothing more from inside the
guest; this one carries `cache: packing the build directory ended at 19:15:39.83` and the
artifacts listing after it. The whole of it, in fact: the console's copy of the guest's log
is line for line what `target\dist\windows\build.log` holds, which is that same file pulled
out of the guest after the job ended. What the two readings say about this host is that the
pack took 12.9s, and that the registry unpack, the one step decision 26's warning is about,
took 27.2s here against the 62.2s the earlier run measured, so that cost is variable as well
as large.

Gate at this point: `cargo test -p xtask`, 540 cases green, which is the 538 of the previous
tip plus `a_drop_that_did_not_happen_ends_the_build` and
`the_last_of_the_output_survives_a_job_that_ends_mid_poll`. Both fail on the code as it was,
each with the symptom it is about. The whole-workspace gates were left to be run once at the
tip rather than twice here.
