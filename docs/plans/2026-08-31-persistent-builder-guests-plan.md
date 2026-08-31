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
