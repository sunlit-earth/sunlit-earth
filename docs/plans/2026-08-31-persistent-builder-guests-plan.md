# Persistent builder guests: `vm start` and `vm stop`, the one-VM rule, and what a builder holds

Written 2026-08-31. Follows the release-build plan (`2026-08-28-vm-release-build-plan.md`) and its cache amendment (`2026-08-29-release-build-amendment-cache-and-bundle.md`), and picks up where the commit that moved the Windows guest's e2e build into the builder guest left off.

## Context

A Linux host used to have no way to put anything into the Windows guest, so `vm up windows` there booted an image with an empty `C:\sunlit-e2e`. That is now fixed: the suite is compiled in the `windows-builder` guest, which is the same image `dist` builds a release binary in, because nothing here is cross-compiled and a guest is the only Windows userland a Linux host has. The release-build plan named this as a non-goal at the time and said exactly what would go wrong with it: it "would turn every `vm up` into a cold release-grade compile". It does.

Measured on this host today, one `cargo xtask vm up windows` from a clean start:

| | |
|---|---|
| working tree archived | 9.5 MiB |
| builder boot to SSH | 22s |
| `cargo test --no-run`, 469 crates, cold | 9m 01s |
| whole command, two boots included | 12m 09s (14:54:32 to 15:04:41) |

The reason it is cold every time is the guest lifecycle rather than anything about cargo: a builder guest is a throwaway overlay that is destroyed when the build ends, so the build directory dies with it. The equivalent sidecar on a Windows host is WSL, which is a persistent environment with `CARGO_TARGET_DIR=$HOME/sunlit-target` in it, so that host has never paid this twice. A builder guest should be the same kind of thing: a compiler that persists, beside a guest under test that does not.

Facts this plan rests on, measured or read off the tree today:

- `provider::resources_for` gives both builders 8192 MiB and 8 vCPUs, the Windows desktop guest 6144 and 4, the Linux one 4096 and 4. This host has 16 cores and 60 GiB.
- QEMU maps guest memory anonymously: there is no `-mem-prealloc` and no hugepage backing in `Launch::args`, so `-m 8192` is a ceiling the host commits lazily rather than a reservation. There is also no `virtio-balloon`, so nothing takes pages back once the guest has touched them, and a Windows guest touches them all eventually because its file cache fills free RAM. A long-lived Windows builder therefore trends to its ceiling.
- A vCPU is a host thread. Nothing is pinned and nothing is reserved, so a guest with more vCPUs than the host has cores is merely pointless rather than harmful, and two guests at once oversubscribe rather than fail.
- Ports are already picked by walking from 2222, 4444 and 5900 (`pick_port`, `PORT_WALK` 250), and the walks are disjoint by test. Records, `vm status` and `vm down all` are per image already. So two guests coexist mechanically today; what refuses them is one gate, `check_no_other_vm`.
- `dist`'s build cache is per builder image and its validity is keyed on the image, the pinned channel, the image's template hash and the image's build time. None of those is the profile, and both builds share one `cargo-target` directory. Measured for the release build in the docs: cold 6m 58s against warm 4m 04s for the Windows builder, and the archives here are 122.3 MiB registry and 613.0 MiB target for `linux-builder`.
- The staging job now extracts with `tar.exe -xf` rather than `-xmf`, so the archive's modification times survive. That change is what makes a warm build directory worth anything at all: cargo's freshness check for the crates in this workspace is mtime-based, and files stamped with the extraction time are files it rebuilds. It is committed separately from this plan and is a precondition for it.

## Goals

1. A repeat staging build in a builder that already exists is a link rather than a compile.
2. A stopped builder holds no memory, and a running one holds as little as a release build in it reliably needs.
3. A builder guest may run beside a desktop guest.
4. `vm start` and `vm stop` exist for builder images: a stop keeps the overlay and the record, a start resumes both.
5. The desktop guests' contract is untouched. Every boot of one is still a pristine overlay, because that is what makes a result from one worth having.
6. Nothing runs in the background unasked. A builder this xtask booted for a build is left stopped, not running; one a person left running stays running.

## Non-goals

- `start`/`stop` for the desktop images. See decision 5.
- Suspending RAM to disk (`savevm`, or Hyper-V's Saved state). It would make a resume seconds rather than a boot, and it ties the overlay to a QEMU version and a machine model, which is a worse thing to own than a 20 second boot.
- Wiring `dist`'s cache into the staging build. Once a builder persists, a warm overlay covers the same ground; the cache stays what it is, which is the cold-start path for a builder that does not exist yet.
- Concurrency beyond one builder beside one desktop guest. Two desktop guests stay refused.
- Changing the Linux builder's memory. Its fat-LTO link is where the docs already name OOM as the failure shape, and nothing here measures it.
- macOS, which has no guest.

## Decisions

1. **Builders are exempt from the one-VM-at-a-time rule, in both directions.** `check_no_other_vm` ignores entries whose image `is_builder()`, so a builder may boot beside a desktop guest and a desktop guest may boot beside a builder. What the rule protects is a host oversubscribed by two guests each sized for the whole of it, which is a number rather than a principle, so the exemption comes with the number: a boot that proceeds beside a running builder names it and says what the two hold together. Two desktop guests are still refused, with the message unchanged.

2. **A stop asks first and then insists.** QMP `system_powerdown`, or `Stop-VM` without `-TurnOff` on Hyper-V, and 60 seconds to comply; after that the process is killed the way `destroy` kills it. Asking first is worth doing because what is being kept is a cargo build directory inside an NTFS volume, and a guest cut off mid-write costs the next boot a repair pass and possibly the last minutes of what it just built. Insisting afterwards is worth more: a Windows shutdown that has not finished inside a minute is not slow, it is waiting for something that will not arrive, and a `vm stop` that hangs is worse than a cold build. The cost of the kill is bounded on purpose, because everything a builder keeps is a cache: the worst case is a repair pass and a build that starts from nothing, which is where every build started before this plan.

3. **The record says which of the two happened.** `RunState` gains `stopped: bool`, defaulted so older records read as false. `is_running` answers no for a stopped guest and for a crashed one, and the two need opposite treatment: one is resumed and its build directory is the reason it exists, the other is cleared away.

4. **A resume re-picks its ports.** A new trait method, `readdress`, implemented by QEMU as the existing `fill_in_address` and left as a no-op default for Hyper-V, whose `start` already waits for the guest's own address. The reason is decision 1: while the builder was stopped, 2222 may have gone to a desktop guest, and a resume that trusted its old record would fail at `-netdev` bind time with a message about a socket.

5. **`start` and `stop` refuse a non-builder image, and say why.** A resumed overlay is not pristine, and the desktop guests' results are worth having precisely because theirs are. This is the same argument that keeps a compiler out of those images, and the refusal quotes it rather than hiding behind "unsupported".

6. **A staging build leaves the builder the way it found it, except that one it booted itself is left stopped rather than destroyed.** Running stays running, stopped goes back to stopped, absent is booted and then stopped. That satisfies goal 6 in both directions: nothing starts running behind a person's back, and nothing throws away ten minutes of compile either. The overlay it leaves is counted by `vm status` and reclaimed by `vm down windows-builder`, and the command says so when it leaves one.

7. **The Windows builder's memory comes down to a measured number, or does not come down.** The candidate is 6144 MiB, and the measurement is one full `cargo xtask dist --target windows` at that number with the vCPU count decision 8 ships, sampling the guest's own available memory over SSH every 15 seconds and keeping the minimum, against the 6m 58s cold that the docs record for that build at 8192. The candidate ships only if the build passes with a margin that is written into this document; if the minimum available memory goes near zero, or the wall time shows the guest paging, the number goes back up. A build that fails at 6144 is not a bug to fix, it is the measurement answering.

8. **Builder vCPUs come from the host rather than a constant.** `std::thread::available_parallelism`, which respects affinity and cgroup limits, with the existing 8 as the fallback. The dependency compile is most of the wall time and nothing about a vCPU is reserved, so a 16-core host giving a builder 8 is leaving half the machine idle for no reason. This interacts with decision 7 in one direction only: more parallel rustc raises the peak, which is why the memory measurement is taken at the shipping core count and not before it. If memory turns out to be the binding constraint, the lever is cargo's `--jobs` in the job script, not fewer vCPUs, because the two are not the same trade.

9. **What this adds to what a run prints is one line per command.** `vm up`'s closing text is already long, and a builder needs less of it than a desktop guest does rather than more: it has no shortcuts on a desktop, no choice of console, and nothing to say about a clipboard. So the builder's closing text is trimmed to what is true of a builder, `stop` and `start` print one line each, and the boot beside a running builder adds one. The documentation changes are measured in sentences, in the two places that already describe the lifecycle, and not in a new section.

10. **A resumed builder still compiles the tree it was given.** The source archive is extracted over a wiped `src/` on every build, as it is today, with the modification times preserved. So what persists in a builder is the build directory and the crate registry, never the sources, and a build in a resumed guest cannot pick up a leftover file from the run before it.

## What changes where

- `provider/mod.rs`: `stop` and `readdress` on the trait; `resources_for` for the builder's memory and vCPUs.
- `provider/qemu.rs`: `stop` through QMP with `SHUTDOWN_GRACE` at 60s and the same `terminate` `destroy` falls back to; `readdress`.
- `provider/hyperv.rs`: `stop` through `Stop-VM` and a poll to `Off`.
- `store/state.rs`: `stopped`.
- `commands/vm.rs`: the `check_no_other_vm` exemption and its memory line; `start` and `stop`; what a stopped guest looks like to a teardown.
- `commands/status.rs`: a stopped guest reads as stopped rather than as a guest that is recorded and not running.
- `guest/artifacts.rs`: reuse, resume and stop around `build_in_guest`.
- `main.rs`: two subcommands.
- Docs: `vm-setup.md` for the commands and what persists, `vm-internals.md` for the lifecycle, `CLAUDE.md`'s command list, and the measurements in this document.

## Acceptance criteria

1. `vm up windows-builder`, then `vm stop windows-builder`: `vm status` shows it stopped with its overlay, the QEMU process is gone, and the host's committed memory drops by the guest's share. `vm start windows-builder` brings it back and `vm ssh windows-builder` answers.
2. With a stopped builder present, `vm up windows` resumes it, builds, stops it again, and boots the desktop guest with the binaries in it. The second build's compile is under two minutes, and the number goes in this document.
3. A running builder does not block `vm up windows`, and the boot says which guest is up beside it and what the two hold.
4. `vm start windows` and `vm stop windows` are refused, naming the pristine-overlay reason.
5. `dist --target windows` passes at the reduced memory, and the guest's minimum available memory during it is recorded here.
6. `vm down windows-builder` on a stopped guest deletes the overlay and the record and says what it freed.
7. The output grows by at most one line per command, and a builder's closing text is shorter than it is today. A test reads the builder's text and fails on the parts of the desktop guest's that do not apply to it.
8. `cargo test`, `cargo clippy --all-targets` and `cargo fmt --check` are clean, and the new behaviour is unit-tested where it is a pure function: the exemption, the refusals, the reuse decision, the memory and vCPU figures.

## Risks

- **The Hyper-V half cannot be tested on this host.** `stop` there is written to the same shape as the existing scripts and stays unverified until someone runs it on Windows. The docs have to say that rather than imply otherwise.
- **A kept overlay is gigabytes.** The Windows builder's `cargo-target` plus its registry is most of a release build's disk. `vm status` already counts overlays, and the stop message names the reclaim command, but a person who never runs it will find the store grown.
- **A resumed Windows guest may want to do something first**: a repair pass, an update, a pending reboot. Asking it to shut down is what keeps the repair pass away in the ordinary case, and decision 2 accepts it in the case where the guest ignored the request. A resume that takes materially longer than a fresh boot is the signal that something in there is not settled, and the boot already prints its own timings.
- **Less memory turns a build failure into an OOM**, whose shape is a killed rustc and a cargo that reports a signal, which the docs already describe for the Linux builder. Decision 7's answer to a marginal measurement is to keep 8192.
- **Two guests, one host.** 6144 plus a builder's share is well inside 60 GiB, but the numbers are printed rather than assumed because a smaller host is the case that breaks, and the exemption makes it possible to ask for both.

## Open questions

1. Does a Windows builder's cargo build directory survive an ACPI shutdown and a boot intact enough for cargo to call it fresh? Expected yes, and criterion 2 is the answer either way. If it is not, the fallback is dist's cache, which is a fresh guest every time by construction.
2. Does deleting and re-extracting 9.5 MiB of source dominate a warm build? Criterion 2 measures it. If it does, the next step is extracting over the tree rather than wiping it, which costs the guarantee in decision 9 and would need its own argument.
3. Answered while this was being reviewed: `vm up windows-builder`'s closing text was written for a guest to look at, and a builder is now a thing with a lifetime. Decision 9 trims it rather than explaining more.

## Departures

1. **`vm up <builder>` stages nothing into the guest it boots.** Not in the plan, and forced by decision 6 meeting a command that predates it. On a Linux host the Windows suite is compiled in the `windows-builder` guest, and `vm up` built it before every boot whatever image it was booting: `vm up windows-builder` therefore compiled the suite inside the image it was about to boot, which cost nine minutes to put a test harness where nothing runs it, and with the builder now persistent it also meant the build left a stopped guest that the same command discarded to make its pristine overlay. A builder is not a guest the suite runs in, so `vm::stages_binaries` answers no for one, and criterion 1's `vm up windows-builder` is the 22 second boot the plan describes rather than a twelve minute one.

## Validation record

Measured on this host on 2026-08-31, on the branch's own commits. One thing about the host differs from the plan's context section and bears on every memory number below: **an unrelated Windows VM of the user's was running throughout, holding about 40 GiB**, so the 60 GiB the plan counts on was 20 GiB of available memory in practice, with the host's swap already full. Nothing here needed more than that, but a figure that reads as headroom on this host is headroom beside that guest and not beside an idle machine.

### Criterion 1 and 4: the stop, start, status and refusal cycle

`cargo xtask vm up windows-builder`, then the cycle, all against `windows-builder` under QEMU on a Linux host. The Hyper-V half of decision 2 is still unrun, as the plan's risk section says it would be.

| | |
|---|---|
| `vm up windows-builder`, cold | **22s** total, SSH at 21s, the desktop marker already in place |
| `vm stop windows-builder` | **16.7s**, then **15.3s** on the second stop |
| host memory after the stop | back to the pre-boot figure: 41.4 GiB used against 41.9 before the boot, 20.7 GiB available against 20.1 |
| the QEMU process | gone; `pgrep -f sunlit-e2e-` finds nothing |
| `vm start windows-builder` | **12.4s**, SSH at 11s, which is half the cold boot's 21s because the guest's disk is in the host's page cache |
| `vm ssh windows-builder` on the resumed guest | answers: `hostname` is `SUNLIT-E2E`, and PowerShell reports 3,269,820 KiB free of the guest's 6 GiB while it idles |
| `vm status` on the stopped guest | "sunlit-e2e-windows-builder is stopped, holding its overlay and no memory", with the `vm start` and `vm down` lines under it and the overlay counted at 1.5 GiB |
| `vm stop windows` and `vm start linux` | both refused, exit 1, naming the pristine overlay and pointing at `vm down` and at the builders |

Two things worth writing down. **A stop of a freshly booted Windows guest takes fifteen to seventeen seconds**, which is well inside the 60 second grace, so decision 2's kill has not been exercised on a healthy guest. And the memory reading immediately after the process exits is not the reading to trust: at three seconds after the stop the host still showed only 1.1 GiB returned, and at thirty seconds it showed all 6. `free` was sampled twice for that reason.

### The cold path, with a stopped builder resumed for it

`cargo xtask vm up windows` with a stopped `windows-builder` present, holding nothing but a boot (the guest from criterion 1 had never compiled anything), so this is the cold-compile case reached through a resume.

| | |
|---|---|
| whole command | **9m 07s** (16:30:07 to 16:39:14), two guests, one of them resumed |
| the builder's resume | SSH at **6s**, faster again than the 11s resume above |
| `cargo test --no-run`, 469 crates, cold | **8m 10s** by cargo's own count |
| the desktop guest | SSH at 11s, the session at 9s, binaries and textures staged |
| what the run left | the builder **stopped** with its build directory, and the desktop guest up |

Against the plan's context table, which measured 12m 09s for the whole command and 9m 01s for the same cold compile, this run was **three minutes faster end to end and a minute faster on the compile**. Most of the three minutes is the boot the builder no longer needs (a resume at 6s where a cold boot is 21s) and the overlay it no longer creates; the minute on the compile is the vCPU count decision 8 raised from 8 to this host's 16, measured at the same 469 crates. Neither number is a like-for-like against a warm build, which is what criterion 2 still owes.

### Criterion 2: the warm compile, and open question 2 with it

The same `cargo xtask vm up windows` again, against the builder the run above left stopped with 469 crates in it. Nothing in the tree had changed between the two runs.

| | |
|---|---|
| whole command | **60.9s** (17:49:17 to 17:50:18) against **9m 07s** cold |
| `cargo test --no-run`, by cargo's own count | **6.80s** against **8m 10s** cold |
| the builder's resume | SSH at **7s** |
| everything the builder did, resume to binaries back on the host | **23s** (17:49:17 to 17:49:40) |
| the builder's stop after it | **7s** (17:49:40 to 17:49:47) |
| the desktop guest | SSH at 11s, the session at 14s, binaries and textures staged |
| what the run left | the builder **stopped**, the desktop guest up |

**A warm compile is seven seconds where a cold one is eight minutes**, which is goal 1 with two orders of magnitude on it and comfortably inside the two minutes criterion 2 asks for. Cargo printed no `Compiling` line at all: every one of the 469 crates came back fresh, which is the answer to open question 1. A Windows build directory survives an ACPI shutdown and a boot intact, and the `tar.exe -xf` that preserves the archive's modification times is what makes cargo believe it.

Open question 2 asked whether re-extracting 9.6 MiB of source dominates a warm build, and the answer is **yes and it does not matter**. The compile is 6.8s of a 23 second in-guest phase, so the archive, the wipe, the extraction, rustup's channel check and copying 45.6 MiB of binaries back over scp together cost more than twice what the compile does. All of it is seconds. Extracting over the tree rather than wiping it would buy a fraction of sixteen seconds at the cost of decision 10's guarantee, so it stays unbought.

What is left of the minute is the desktop guest: a pristine overlay of an 18.2 GiB image, an 11 second boot, a 14 second wait for the session, and the textures. That is the floor for this command now, and the builder is no longer any part of it.

### Criterion 3: a builder running beside a desktop guest

`vm start windows-builder` by hand, then `vm up windows` into a host that already had a builder on it. The desktop guest from the run above was taken down first, so this is one builder and one desktop guest, which is the concurrency the non-goals cap it at.

| | |
|---|---|
| `vm start windows-builder` | **7.5s**, SSH at 7s, "is up again, with what it was holding" |
| what the build said about it | "the windows-builder guest is already up; building in it as it stands" |
| `cargo test --no-run`, warm again | **3.07s**, half of the 6.80s above, in a guest that was already awake |
| the whole `vm up windows` | **48.5s** (17:51:40 to 17:52:28), no resume and no stop in it |
| the desktop guest | SSH at **21s** against 11s alone, the session at 9s |
| the builder afterwards | still **running**, because it was running when the build found it |
| `vm status` with both up | `sunlit-e2e-windows is running` and `sunlit-e2e-windows-builder is running`, both as `an interactive guest (vm up)` |

The line criterion 3 asks for, verbatim:

```
sunlit-e2e-windows-builder is up as well, which a builder may be: the two hold 12.0 GiB of this host's memory between them
```

**Decision 4's port walk earned itself here, in the direction the decision did not name.** The builder holds 2222 while it is up, so it is the desktop guest that walks, and the boot said so in three lines: `the usual ssh port 2222 is in use or reserved; using 2223`, and the same for 4444 to 4445 and 5900 to 5901. The decision was written for a resume finding its own port taken; what actually happens more often is the other guest finding it taken, and `pick_port` covered both without being asked to.

Host memory, which is the number decision 1 says to print rather than assume. This host had 20.6 GiB available with nothing of ours up, beside the user's own 40 GiB guest. The builder alone took it to 14.6, and both guests together to **11.9 GiB, falling to a low of 8.8 GiB over the two minutes they were both up** as the Windows file caches filled toward their ceilings, which is exactly the trend the plan's context section predicts of a guest with no balloon. Swap was already full at 7.8 GiB before any of this and did not move, so nothing here forced a page out. `vm down windows` then took 0.27s and left the builder alone, and `vm stop windows-builder` took 11.8s and returned the host to 21.2 GiB available.

Two guests on this host is comfortable and would not be on a smaller one: 12.0 GiB is the figure the boot prints, and the ceiling the two would reach if left up all day is that same 12.0.

### Criterion 6: `vm down` on a stopped builder

`vm status` first, then the teardown, on the builder the runs above left stopped with one cold compile and two warm ones in it.

The listing named the overlay and the two ways out of it:

```
  run state: 5 files (8.1 GiB)
    overlay.qcow2  8.1 GiB
  sunlit-e2e-windows-builder is stopped, holding its overlay and no memory
    start:   `cargo xtask vm start windows-builder` resumes it with what is in it
    down:    `cargo xtask vm down windows-builder` frees the overlay instead
  footprint: 9.3 GiB image, 8.1 GiB run state, 738.0 MiB build cache
```

The teardown took **0.22 seconds**, listed the same five files, and opened with the clause the `stopped` field exists for:

```
stop sunlit-e2e-windows-builder (qemu), which throws away the build directory and crate registry it is
holding, so the next build in it starts from nothing
...
that frees 8.1 GiB across 5 files
stopped 0 VMs, deleted 5 files, freed 8.1 GiB
```

**`stopped 0 VMs` is the sentence being honest**: there was no process to stop, and the cost clause still had to be said, which is `RunState::cost_of_ending` reading the record rather than the start reason. A teardown that took the reason at its word would have offered to end an interactive guest that was not there and said nothing about the eight gigabytes it was about to delete.

Afterwards `windows-builder` reads `no overlays or run state` and the store is back from 72.8 GiB to **64.8 GiB**. Two things fall out of the numbers. The overlay grew from 8.0 GiB to 8.1 across two more warm builds, so what a kept builder costs on disk is set by the first cold compile and creeps afterwards. And `dist`'s 738.0 MiB build cache is untouched by any of this, as the non-goals intend: it is the cold-start path for a builder that does not exist, and this is the command that makes one not exist.

### Criterion 5: the Windows builder's memory, measured three times

Decision 7 asks for one full `dist --target windows` at 6144 MiB with the vCPU count decision 8 ships, the guest's own free physical memory sampled every 15 seconds, and the wall time read against the 6m 58s cold that `docs/vm-setup.md` records at 8192. **The docs' figure turned out to be the wrong baseline**: it was measured on the Windows host under Hyper-V, and this is a Linux host under QEMU with different cores and a 40 GiB guest of the user's beside it. A number from another machine cannot say whether this one is paging, so the run at 6144 was repeated at 8192 on this host, and then at 6144 again once the first pair disagreed by more than the answer could carry.

All three are `--no-cache`, so each is a cold build with its own crate download, and all three archive the same commit `d60199c`: `dist` archives `HEAD` whatever the working tree holds, so the one-line memory edit the control needed changed the builder and not a byte of what it compiled.

| | guest | cargo's own time | whole target | guest's minimum free physical memory |
|---|---|---|---|---|
| run A | 6144 MiB, 16 vCPU | 7m 52s | 9m 06s | **731 MiB** |
| control | 8192 MiB, 16 vCPU | 7m 10s | 8m 24s | **1,411 MiB** |
| run B | 6144 MiB, 16 vCPU | 7m 18s | 8m 40s | **692 MiB** |

Every one of the three passed: exit 0, 26 imports with none of them the Visual C++ runtime, and a 640x360 render out of the bundle in the Windows desktop guest afterwards, at 320.8, 320.7 and 321.3 KiB.

**6144 ships.** Run A against the control looks like a 42 second penalty and run B says that is the host talking: two builds at the same 6144 are 34 seconds apart, so the 8 seconds between the control and the better of them is noise on a machine that is also running someone else's 40 GiB guest with its swap full. What is repeatable is the memory, and 692 and 731 MiB is a floor rather than a cliff: **11 to 12 percent of the guest, twice**, with the curve free to move above it the whole time rather than pinned to it.

The shape of the curve is worth as much as the floor, because it says which part of the build is the peak.

```
        6144 MiB run B, free physical memory in the guest, MiB
18:16:49 3134   18:17:38 3584   18:18:14 2447   18:18:54 1106
18:19:14  721   18:19:52  917   18:20:13  692   18:20:33 2811
18:21:10 3670   18:21:45 2255   18:22:38 2337   18:23:47 2477
```

The floor is **the parallel dependency compile**, about eighty seconds of sixteen rustc processes at once, and it is over by the time the fat-LTO link starts. The link then runs for three minutes with 2.3 to 2.7 GiB free, which is the opposite of what the plan's risk section expected from the shape the Linux builder's OOM note describes: on this workspace the last few crates holding a lot at once is not the peak, sixteen of them holding a little each is. The control's curve is the same curve shifted up by about 1.7 GiB, which is the 2 GiB of extra ceiling arriving as free memory rather than as speed.

So decision 8's escape hatch stays unused. `--jobs` is the lever if the floor ever becomes a cliff, and nothing here needed it.

`docs/vm-setup.md`'s OOM note gained a sentence saying the Windows builder's 6 GiB is this measurement rather than an assumption, since that note is where someone whose build died for want of memory will look.

### What is not measured here

Criterion 7 and criterion 8 are code rather than a run. The builder's closing text is pinned by a test that reads it and fails on the desktop guest's two shortcuts, its `SLINT_BACKEND` note, its choice of console session, its clipboard caveat, its warning about clicking during a run, and its claim that there is no way to save or pause a guest, while insisting the builder's own `stop`, `start` and `down` lines are there. The exemption, the two refusals, the reuse decision and the memory and vCPU figures each have their own unit test over supplied data rather than over this host. The gates at the end of the work: `cargo test -p xtask` 560 passed, `cargo clippy --all-targets` silent, `cargo fmt --check` clean, and `cargo test --no-fail-fast` green but for `golden_sun_rising_through_the_band`, a stale lavapipe golden this branch did not touch.

One thing that turned up while running those gates and belongs to nobody here: **`tests/shading.rs` flakes on this host**, panicking inside `wgpu-hal`'s `gles/egl.rs` with `BadAccess` while it takes the shared GPU context, which then poisons the `LazyLock` and takes the other eleven cases with it. It ran eight times tonight against a tree in which nothing but Markdown moved, and failed four of them, alone and inside a full `cargo test` alike, so it is the host's GL stack rather than a regression. Worth someone's attention, not this branch's.

**The Hyper-V half of decision 2 is still unrun**, as the risk section said it would be. `shutdown_script`, `turn_off_script` and `wait_for_off` have never executed; nothing in this record covers them.

Two other things nothing here measures, both of them deliberate. **Decision 2's kill has never fired**: every stop of a healthy Windows guest in this session complied in 7 to 17 seconds, well inside the 60 second grace, so what the record proves is the asking and not the insisting. And **the Linux builder's memory is untouched at 8192**, which the non-goals reserve; the measurement above is about a Windows guest's file cache and says nothing about a fat-LTO link on Ubuntu.
