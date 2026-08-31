# VM orchestration internals

How `cargo xtask` is built, from the code's side: which module decides what, and why each decision came out the way it did. [vm-setup.md](vm-setup.md) is the guide for using it; the design documents are [plans/2026-08-19-phase3-vm-orchestration-plan.md](plans/2026-08-19-phase3-vm-orchestration-plan.md), [plans/2026-08-20-phase3-amendment-hyperv-windows-build.md](plans/2026-08-20-phase3-amendment-hyperv-windows-build.md), [plans/2026-08-21-phase5-linux-vm-and-parity-plan.md](plans/2026-08-21-phase5-linux-vm-and-parity-plan.md), [plans/2026-08-28-vm-release-build-plan.md](plans/2026-08-28-vm-release-build-plan.md) and [plans/2026-08-29-release-build-amendment-cache-and-bundle.md](plans/2026-08-29-release-build-amendment-cache-and-bundle.md).

## Commands

The desktop e2e suite runs in local VMs instead of taking over the developer's desktop. `docs/vm-setup.md` is the human guide; the design is in `docs/plans/2026-08-19-phase3-vm-orchestration-plan.md`, and the Linux guest's overhaul in `docs/plans/2026-08-21-phase5-linux-vm-and-parity-plan.md`.

```bash
cargo xtask vm doctor              # Unelevated, read-only: can this host run the VM suite?
cargo xtask vm setup               # The one command that changes the host. Elevated on Windows.
cargo xtask vm build-image <image>  # An install or a layer, then a manifest. Minutes to an hour.
cargo xtask vm up <image> [--desktop <kde|gnome|xfce|cinnamon>]   # An interactive guest, with the current binaries in it
cargo xtask vm stop <builder>      # End a builder and keep its overlay; vm start resumes it
cargo xtask vm ssh <image>         # A shell in the running guest
cargo xtask vm view <image>        # Its desktop (vmconnect for Hyper-V, VNC for QEMU)
cargo xtask vm smoke <image>       # Boot, run a trivial job through the guest contract, take it down
cargo xtask vm status              # Images, media, overlays, running VMs, disk footprint
cargo xtask vm down <image|all>    # End the guest, keep the image
cargo xtask vm purge <image|all> [--vm] [--image] [--iso] [--cache] [-f]
cargo xtask e2e --target <host|windows|linux> [--keep] [--allow-expired-image] [--desktop <d>]
cargo xtask dist [--target <windows|linux|all>] [--keep] [--no-verify] [--no-cache] [--allow-expired-image] [--allow-dirty]
```

An `<image>` is one of four slugs: `windows` and `linux` are the desktop guests the e2e
suite runs in, and `windows-builder` and `linux-builder` are where `dist` builds a
release binary. A `<target>` is an operating system, which is what `e2e` and `dist` take,
because each of them picks the image it needs itself.

## Host tool lookup

The host tools the VM commands run go through `host::facts::resolve_tool`, which asks `PATH` and then the places an installer is known to leave a program without putting it on `PATH`: QEMU's and TightVNC's own directories under Program Files, winget's links directory, and scoop's shims directory under `%SCOOP%` or `~\scoop` and `%SCOOP_GLOBAL%` or `%ProgramData%\scoop`. Both package managers append to the *user* `PATH`, so a tool installed in the shell that is now running the xtask is installed and invisible; a lookup that missed it would make `vm setup` plan an install that winget then refuses as redundant, and `vm view` claim a viewer is absent. The VNC viewers are in `facts::VNC_VIEWERS`, executable names rather than package identifiers, and `vm view` of a QEMU guest resolves them the same way `vm doctor` reports them; what to hand the one that was found is `facts::vnc_viewer_argument`, because KRDC takes a `vnc://` URL where the rest take the bare address. Two lookups stay on bare `PATH` deliberately and say so where they sit: the Packer ISO tools, because Packer resolves them itself and a fallback location would not help it, and `store::windows_media`'s choice between `curl` and `wget`.

## Setup, teardown and run state

`vm setup` never reboots or signs anyone out; it reports what needs one. `vm doctor` changes nothing. `vm down` is the cheap teardown: it ends the guest and deletes its run state, which the next boot recreates. `vm stop` is the other one, for the builder images alone: `Provider::stop` asks the guest to shut itself down (QMP `system_powerdown`, or `Stop-VM` without `-TurnOff`) and after `provider::SHUTDOWN_GRACE` insists the way `destroy` does, reporting which of the two happened as `Stopped::ShutDown` or `Stopped::Killed` and printing a line at the moment it stops asking, since a kill leaves a filesystem the guest never closed and the next boot of that overlay opens with a repair pass; a teardown collapses the two, because it deletes the overlay they differ over. `RunState::stopped` records which of the two a not-running guest is, and the overlay and the record both stay where they are. `vm start` resumes one through `vm::resume`, which is `boot` without the overlay creation plus `Provider::readdress`, since a builder may have been stopped for hours and QEMU's loopback ports may belong to another guest by now. Both refuse a desktop image and quote the reason (`vm::persistence_refusal`): a resumed overlay is not pristine, which is the same argument that keeps a compiler out of those images. A resume that fails takes the opposite branch from a boot that fails (`vm::after_failed_resume` against `vm::after_failure`): a boot tears its guest down because it created that overlay three lines earlier, and a resume was called to preserve one, so it leaves the guest and the overlay alone, puts `stopped` back if the guest never came up, and prints the way back and the way out. The failure it is written for is the one decision 2's kill makes likelier, a guest that has not answered because it is running a repair pass, which is the moment the build directory is worth the most. What reads `stopped` besides those two is everything that would otherwise treat a kept build directory as a crash: `vm status`, `RunState::cost_of_ending`, the line `vm::clearing_line` prints when a boot discards one anyway, and `artifacts::found_from`, which is the staging build's reuse decision. A command that boots a guest of its own tears it down the same way when it is finished with it, and `vm::run_state_paths` is the one list of what that removes: the record, the overlay, and the three things a boot writes beside them, each named by the `Store` method the writer uses, which are the scratch a job's script was written into (`job_scratch`), the scratch the Windows hand-over launcher is staged in (`handover_scratch`), and the per-VM copy of the firmware's variables a QEMU boot makes (`firmware_vars`). So a green run leaves `vm status` nothing to report. Two things in the run directory are deliberately outside that list: `vm.log`, because a failed boot's message quotes its tail and names its path, and the bundle a release build assembles (`bundle_scratch`), because it outlives the guest it was staged into by the seconds it takes to write the final record into it and archive it, and a teardown that took it deleted the bundle whose textures that boot had just proved. `dist` removes its own, and `vm down` and `vm purge` sweep one a dead run left behind, because both take the run directory whole rather than reading the list. `vm purge` is the disk-space one: everything a target has on disk unless `--vm`, `--image`, `--iso`, or `--cache` narrows it, and it asks before deleting unless `-f` is given. `e2e --target host` is what `cargo e2e` does, kept as one command so the manual real-GPU run and the VM runs are the same thing. The one-VM-at-a-time rule is one *desktop* guest at a time for the same reason (`vm::may_run_beside`): a builder is exempt in both directions, because what the rule protects is a host oversubscribed by two guests each sized for the whole of it and a compiler beside the thing it compiles for is not that pair. The exemption comes with the number, so a boot that proceeds beside another guest prints `vm::beside_line`, which names it and adds up what the two hold.

## Images: bases and layers

**The store holds an image per slug, not an image per target** (`provider::target::Image`).
An image is either a *base*, installed from media, or a *layer*, provisioned over a named
parent:

| slug | kind | what it is for |
|---|---|---|
| `windows` | base | Windows 11 Enterprise evaluation: the e2e guest |
| `linux` | base | Debian 13 with four desktops: the e2e guest |
| `windows-builder` | layer over `windows` | release builds |
| `linux-builder` | base, Ubuntu 22.04 | release builds |

What keys on the image is what belongs to one disk: every path under `images/`, `run/`,
`build/` and `results/`, the VM name, the manifest, the run record, and how much of the
host a guest of it gets (`provider::resources_for`, which both hypervisors read, so a
builder's memory and its one vCPU per host core cannot come apart between the two;
`provider::builder_cpus` is that count, `available_parallelism` with eight as the fallback,
because affinity masks and cgroup quotas are what a core count misses). What keys on the
target is everything about the operating system: the provider matrix, the guest root, the
job scripts, the SSH account, and the device models the Packer templates and the QEMU
command line have to agree about. `Image::target()` is the bridge, and the two desktop
images keep their original slugs so every path they had is the path they have.
The third question is two questions with two names. `Image::has_desktop()` is whether a
session logs on, which the Linux builder alone has none of: it is built from a cloud image
with no desktop in it and writes its readiness marker from a oneshot unit at boot, while the
Windows builder is a differencing child of the desktop image and inherits its autologon.
Every text that offers a console asks it through `Image::console_label()`, so the Linux
builder's `vm view` line says `console:` and every other image's says `desktop:`.
`Image::is_builder()` is the other question, what an image is for, and it is what the texts
about which command uses an image, which command leaves one of its guests behind, and what a
smoke test asks it to prove all read. Asking the first where the second was meant was what
once offered the Windows builder a console four lines above two sentences about its desktop.

A layer is a differencing child, which is what makes the Windows builder five minutes
and a few gigabytes rather than an hour and another fifteen: `commands::build_layer`
creates the child, boots it, runs `toolchain.ps1` and then `finalize.ps1` over SSH, shuts
it down, drops the VM keeping the disk, and moves the disk into the store. The cost is
that the file cannot be read without its parent: Hyper-V refuses to attach a child whose
parent's identifier changed and qcow2 reads garbage silently, so the layer's manifest
records the parent's checksum as the parent's own manifest states it and the inventory
calls a mismatch `ImageCondition::Detached` and blocks the boot. The evaluation clock is
the parent's too, recorded in the same place: provisioning a layer over an eighty-day-old
install does not reset the licence. A purge of a base lists its layers with it, and
`SUNLIT_EARTH_VM_PROVIDER` is refused for a layer, because converting a differencing
child between formats means flattening it through a full copy of its parent.

The other cost is that a differencing child records every block the guest wrote,
including the ones it freed again, so a layer is much larger than what it
installed: the Windows one holds 4.8 GiB of toolchain and came out at 16.5 GiB.
Two steps take that to 13.5, one on each side of the boundary. `finalize.ps1`
deletes the installer caches a build never reads and runs `Optimize-Volume
-ReTrim`, which is the only thing that tells a virtual disk a block is free, and
`build_layer::compact` runs `Optimize-VHD -Mode Full` on the host between the
move into the store and the manifest, so the manifest measures the file as it
will be read. Both are best effort, since a layer that could not be trimmed is a
larger layer rather than a failed build, and the trim runs before `finalize`'s
toolchain checks so that anything it breaks fails that build. The compaction is
the larger half: 2.3 GiB of the 3.0 is blocks the guest had freed and nothing had
told the disk about.

## The builder matrix

**The builder matrix mirrors the runtime provider matrix**, and `commands::build_image::builder_for` is the one place that decides it:

| | Windows host | Linux host |
|---|---|---|
| Windows image | native Hyper-V (`commands::build_hyperv`) | Packer and QEMU on KVM |
| Linux image | Packer and QEMU on WHPX | Packer and QEMU on KVM |

The one cell that is not Packer is the one QEMU cannot install. QEMU on a Windows host uses WHPX, and a WHPX guest with more than one vCPU does not survive the reboot Windows Setup performs after copying its files: `WHPX: Unexpected VP exit code 4`, unrecoverable. One vCPU survives it and Windows 11 Setup refuses to install on one core; `kernel-irqchip=off` keeps the vCPUs and stops the guest booting at all. So on a Windows host the xtask installs Windows itself, on the hypervisor the guest runs on anyway: it repacks the media without its boot prompt (`store::windows_media::ensure_install_media`, so there is no keypress to inject, keeping a repack only until the build that needed it succeeded, and reusing one a failed build left behind while the record beside it still matches the download it came from), builds the unattend CD with oscdimg from `build_hyperv::CD_FILES`, creates a generation 2 VM, watches the install, runs `finalize.ps1` over SSH, shuts the guest down, keeps its disk and drops the VM, and converts the disk to qcow2 for the QEMU override cell. No Packer, no QEMU process, and no network on the host after the ISO download; the guest's own first logon fetches the Visual C++ runtime, which Windows does not ship and every Rust MSVC binary the suite runs links dynamically, and `finalize.ps1` fails the build if it is not there. Decision 10's intent stands either way: one canonical install in two formats, with only the conversion direction flipping. `docs/vm-setup.md` has the WHPX measurements and a twenty-second recipe for rechecking them against a newer QEMU. The dispatch is deliberately not overridable by `SUNLIT_EARTH_VM_PROVIDER`, which moves a guest rather than a build.

Both builders report on the install while it runs, because neither Packer nor Windows Setup says anything for most of an hour (`commands::build_watch`, whose `Trend` both share). The Packer path holds one QMP connection, presses the installer's boot key with it, and prints a line a minute: what the output disk holds and how fast it is growing, whether the guest's screen is changing, and what QEMU says the machine is doing. A guest QEMU has stopped is reported rather than waited out: the build starts it again once, and if that changes nothing it ends the guest so the build fails now instead of at Packer's two-hour SSH timeout. The build directory keeps `screen.png`, the guest's last screen as QEMU encoded it, and `packer.log`, which is where QEMU's stderr ends up. The native path prints the same shape of line from the signals Hyper-V has: `Get-VM`'s state, the growing VHDX, and the firmware's first boot entry, which is what makes the one unmeasured assumption in it visible (that Setup's own boot entry survives the mid-install reboots). A guest that is not `Running` on two readings in a row ends the build, since a Hyper-V guest stays running through the reboots an install performs; one reading is not enough, because `Start-VM` returns before the guest is `Running` and `Get-VM` says `Starting` in between.

The build VM carries the same `sunlit-e2e-windows` name a runtime guest does and writes a `RunState` with a build reason before it exists, so `vm status`, `vm view`, `vm ssh`, `vm down`, and the one-VM-at-a-time rule all apply to a build. Every failure from the point the create script could have run asks Hyper-V whether the VM is there and says which of the three answers it got, because that script is one process that stops at its first error and most of its statements leave a registered VM behind. A failed build keeps its guest and prints how to reach and remove it; `vm down windows` takes the VM and the unfinished disk together, a `--iso` purge ends a build first because a build holds both DVDs for the whole install, and a build that is still running is refused as something to clear away. Anything that ends a build says so before it does: `StartReason::cost_of_ending` is the one clause, read by the one-VM-at-a-time refusal and by a teardown's listing and question alike, and a purge that ends a build without taking its run state names the record and the disk it leaves.

## Staging a guest for e2e

`e2e --target <guest>` copies three things in: the app, the test harness, and the fixtures, plus the repository's `textures/` when it holds the assets rather than Git LFS pointers (checked by size, since a pointer file exists and cannot be decoded). `test_render_and_exit` samples the globe by color, so without them a guest renders the procedural grid; the job then omits `SUNLIT_EARTH_TEXTURES` rather than naming a directory that is not there, which is the same thing that happens to `cargo e2e` on a host in that state. The generated Windows job also sets `SLINT_BACKEND=winit-software` (`commands::e2e::WINDOWS_SLINT_BACKEND`): a Hyper-V guest's synthetic display adapter offers no OpenGL and Windows ships no software implementation of it, so Slint's default renderer cannot start at all and the app dies with "Could not locate glCreateShader symbol" before its event loop. WARP does not cover that, because WARP is Direct3D and Slint asks for GL. The Linux job sets no backend, because Mesa is a software GL implementation and llvmpipe answers there. Both details live in the generated job, which is per run: neither needs an image rebuild.

## Release builds

`cargo xtask dist` is the release path, and it is the one place decision 8's
"build on the host and copy the binaries in" does not apply. That rule is right for a
debug build of a test harness, where the host's toolchain is the fast path for iterating
on a test; it is wrong for a release, where the point is that the host is not in the
build. So `commands::dist` boots a pristine overlay of the target's builder image, copies
in a `git archive` of `HEAD` without `textures/`, and runs `cargo build --release
--locked -p sunlit-earth` in there with `cargo` named by absolute path. Nothing else of
the host reaches it: no `target/`, no `~/.cargo`, no environment, and the toolchain is
installed by the name `rust-toolchain.toml` pins (`guest::toolchain`). The one thing a
build may inherit is what an earlier build of the same image on the same channel left
behind, which is the build cache, and never anything of the host's own.

That cache is `<store>/cache/<builder slug>/`, two zstd archives with a JSON sidecar each
(`store::cache`): `registry.tar.zst` is the guest's `~/.cargo/registry` and `~/.cargo/git`,
and `target.tar.zst` is its build directory, which `CARGO_TARGET_DIR` puts at
`<guest root>/cargo-target` so the `rm -rf src` at the top of every job cannot reach it.
The guest packs and unpacks both, because a restored registry is tens of thousands of small
files and `scp -r` is a round trip per file; the host only stores and transfers them, so it
needs no zstd of its own and `vm doctor`'s tool list is unchanged. What keeps it a cache
rather than a shortcut is that it cannot decide what the binary is: the source tree is
extracted with `-m`, so no committed file can look older than an artifact built from it,
and because that is an argument about the guest's clock a restored build directory also
gives up this workspace's own fingerprints and the binary linked from them, which makes
`sunlit-core` and `sunlit-earth` units cargo compiles again whatever the times say;
`--locked` and the lockfile's checksums mean a restored registry holds only what the
network would have handed over; and `cache::restorable` discards a cache whole rather than
merging it when the channel or the builder image moves, in a line naming the field that
moved. A failed build saves nothing, and the registry is not packed again while a
*restored* one's `Cargo.lock` has not moved: a sidecar that was refused describes an
archive nothing will read again, so reading its hash as "the host already has this" would
leave the registry cold until the lockfile happened to move. `--no-cache` skips restore and save both, and
`build-info.json`'s `cache` section says per archive which of those happened and what each
transfer cost, so what a release inherited is readable afterwards rather than taken on
trust. Measured warm against cold: 4m 04s against 6m 58s on Windows and 3m 49s against
5m 28s on Linux, for archives of about 120 MiB and 615 MiB. The cache's own round trip is
44 to 64 seconds on Linux and 1m 42s on Windows, and 62 of those Windows seconds are one
step, unpacking the crate registry, which is the small-files cost that put the unpacking in
the guest to begin with; every step prints what it took, a duration on Linux and a reading
of the clock on either side of it on Windows, where computing the difference would cost a
process spawn or a bet on the locale's time format. The cache is inventory rather than run state, so `vm status`
counts it per image, `vm down` never takes it, and `vm purge <image> --cache` is what frees
it.

Two claims a release binary makes cannot be checked by running it, so the builder reads
its own output with the tools it has and the host parses that: `dumpbin /dependents` must
name neither `vcruntime140.dll` nor `msvcp140.dll`, which is `crt-static` proven on the
artifact, and `readelf -d` with `objdump -T` must show a glibc floor of at most 2.35 and
the four libraries the Linux port links. A guest with the Visual C++ redistributable
installed runs a dynamically linked binary perfectly well, which is exactly why running
it proves nothing about that. What running it does prove is the other half, and what is run
is the release bundle rather than the loose binary. `commands::bundle` assembles one
directory, named `sunlit-earth-<version>-<target>` after the version in the workspace
manifest rather than after `git describe`, which this repository has no tags for: the
binary, `textures/` with the four JXL assets and their `PROVENANCE.md`, `build-info.json`,
`LICENSE`, and the star catalog's `ATTRIBUTION.md`, plus `assets/` on Linux, where
`install-user.sh` is the whole install story and on Windows the icon is a resource inside
the exe. It is then written as the archive its target expects, a zip that stores the JXL
entries and deflates the rest (the `zip` crate), or a `.tar.gz` where 0755 on the binary is
an ordinary header field (`tar` over the `flate2` that `image` already pulls in), and read
back with the same crate that wrote it. Both writers are xtask-only. The *directory* is what the
desktop guest is staged with, and the render is asked for with no `SUNLIT_EARTH_TEXTURES`
at all, so what is under test is the lookup a user's machine does: `resolve_textures_dir`
walking up from the executable to the `textures/` beside it. A render that failed to find
them still writes a 640x360 PNG of the procedural grid, so its header cannot tell the two
apart and the job renders twice, once against an empty textures directory it creates and
once from the bundle, and the host decodes both and refuses a mean difference under
`dist::TEXTURE_LOOKUP_FLOOR`, which is 8.0 channel steps against the 19 to 23 the live runs
measure. The record is written into the bundle after that boot rather than before it, so
the copy inside the archive and the copy beside it are one file, which also means a bundle
that failed its own texture check is never archived. Where the host's `textures/` is still
Git LFS pointers there is no bundle at all: one line says so and the loose binary is
verified as before. `--no-verify` skips the boot and nothing else: the archive is written
like any other run's, and what says otherwise is the line naming it, the closing summary,
and the two fields the record writes as `null` rather than leaving out, `verified_in` and
the bundle's `texture_lookup_delta`, since that record travels inside the archive to
somebody who did not watch the command run. The output is
`<target dir>/dist/<target>/`, replaced wholesale on success and untouched on failure,
holding the binary, the bundle archive, `build-info.json`, the builder's `output.log` as
`build.log`, and the verification render. A dirty working tree is refused before anything boots, because the
archive is of `HEAD` and a record whose commit does not describe the binary is the one
thing it must not be. `--keep` leaves one guest of the whole run up, not one per target:
the next boot is refused while another guest is registered, so `dist::keeps_guest` narrows
the flag to the last boot the run makes and the closing summary names what is still there.
Which guest that is follows from the verification, so a text offering the builder says
`--keep --no-verify` (`vm::keep_command`): with the verification on, the last boot is the
desktop guest and the flag alone hands back a desktop with no source tree in it.

## Progress output

A build says what it is doing while it does it: `job::OutputTail` reads the guest's
`output.log` on every poll and prints what is new, tracked by how much has been printed
already, because a forty-minute compile that says nothing is indistinguishable from a
wedged one. The whole log is decoded afresh every poll, so a multi-byte character the job
was in the middle of writing arrives as a replacement character and becomes itself a poll
later, which moves everything after it: what a decode had to replace is held back for the
poll that has the whole of it. The e2e path does not take the hook; its suite finishes in
under a minute and its log is printed once at the end.

## Hand-over: the guest as a desktop

Where those binaries are compiled is `artifacts::builder_for`, and it has three answers: this host's cargo, the WSL distribution (a Windows host building the Linux guest's), and a builder guest of the target's own operating system (a Linux host building the Windows guest's). The third uses `windows-builder`, copies in a tar of the working tree, runs the ordinary `cargo test --no-run --message-format=json` in it, reads the JSON that comes back in the results directory to learn which two executables to fetch, since the harness carries a hash in its name, and pulls them out with `copy_out`. Which guest it runs in and what becomes of it afterwards is `artifacts::found_from` and `artifacts::stop_after_build`: a builder that is up keeps running and is built in as it stands, a stopped one is resumed and stopped again, and anything else, including the guest a crash left registered, is booted and then left stopped rather than destroyed. The source archive is still extracted over a wiped `src/` on every build, so what persists in a builder is the build directory and the crate registry and never the sources, and the extraction keeps the archive's modification times, because cargo's freshness check for this workspace is mtime-based and a file stamped with the extraction time is a file it rebuilds. `usable_builder` adds the one thing that can be absent, that image, and its refusal is a sentence rather than an error because the caller decides what to do with it: `vm up` prints it and boots a guest with nothing in it, `e2e` stops. Building happens before the boot in both commands rather than inside `stage`, which used to be what the one-VM-at-a-time rule required of the third answer and is now a matter of order: the binaries have to exist before there is a guest to put them in, and a compile error costs no boot at all.

A guest is also something a person looks at, so staging writes two more things into either one (`guest::handover`): a launcher that starts the app with the environment this boot gave it, and two shortcuts, one for that launcher and one for the guest root. In a Windows guest they are `C:\sunlit-e2e\run-app.cmd`, which sets the same `SLINT_BACKEND` the job sets and `SUNLIT_EARTH_TEXTURES` under the same condition, and two `.lnk` files on the console user's desktop. Per boot for the same reason the job script is per run, and generated rather than shipped in the image because the launcher has to know what this boot staged. A Windows guest also offers one of two consoles, and `handover::enable_enhanced_session` is what decides which. The image ships with Remote Desktop Services disabled, so a guest running a suite offers no enhanced session (`EnhancedSessionModeState` 6 rather than 2) and `vmconnect` opens a basic session that asks for nothing: an enhanced session is RDP, and connecting moves the console session into it, which is where the windowed tests keep their desktop. A guest being handed to a person has nothing of ours running in it, so `vm up` and `e2e --keep` turn the service back on, blank the account's password and clear `LimitBlankPasswordUse`, which is what makes the credential dialog a thing to dismiss rather than fill in. `handover::offers_enhanced_session` asks the hypervisor before the guest: the same Windows image under QEMU, which is how a Linux host boots it, is looked at over VNC, so there is no session to turn on and nothing in the guest is changed for one. That buys the one thing a basic session cannot do at all: a window that resizes, with the guest's desktop following it. What happened is recorded rather than inferred: `vm::hand_over` writes `RunState::handed_over` from the marker the guest itself printed, and that field, not the start reason, is what `vm view` reads to decide whether to answer the connection dialog and which of the two consoles to describe. The reason is chosen before the boot, so it can answer neither question; `Keep` is written at the same moment, which is what lets `vm status` tell a run in progress from one that is over. Each command's closing text comes from the same facts (`vm::Prepared`): what was staged, which hypervisor is showing the console, and whether an enhanced session is on offer. So `vm smoke --keep`, which stages nothing and hands nothing over, is described as the empty desktop it is, and the Windows guest under QEMU is described as the VNC console it has. Staging is three states rather than two (`vm::Staging`), because "nothing was staged" is the command's answer on a host that could have staged something and the host's answer on one that could not: a Linux host cannot build Windows binaries, so `vm up windows` there boots the image bare and says that, where `e2e --target windows` refuses instead. A guest whose image predates the Remote Desktop Services disable still offers an enhanced session, and staleness only warns at boot, so the note for a guest nobody handed over says to cancel a credential dialog that appears anyway rather than sign in. The display-configuration dialog in front of an enhanced session is answered by `vm view`, which writes `vmconnect`'s own per-VM settings file (`hyperv::vmconnect_settings`) before starting it, sweeps the ones earlier guests left, and takes the file with the VM when one is destroyed: those settings are filed under the VM's identifier, which is new on every boot, so the dialog's own remember-me checkbox lasts exactly one guest. Runtime is enough for all three, so none of it needs an image rebuild; the disable is an image property only because the service refuses to stop once started, and `finalize.ps1` fails a build whose start type is anything else.

The Linux half of that hand-over is `/var/lib/sunlit-e2e/run-app.sh` and two XDG desktop entries, and what a person there is missing is not an environment variable but the root itself: it sits outside any home directory on purpose, so the closing text names it, the launcher and the shell command, and `vm smoke --keep` promises none of the three. The launcher names the staged textures directory when there is one and sets no backend at all, because Mesa answers GL in the guest; it redirects its own output to `run-app.log` beside it when stdout is not a terminal, since started from an icon there is nowhere else a failure could be read and started from a shell a log would hide it. The entries are written twice, into `~/.local/share/applications` and onto the desktop directory `xdg-user-dir DESKTOP` names, because GNOME draws no desktop icons at all and the menu is the whole hand-over there; both are `Type=Application`, the folder one running `xdg-open`, since a menu shows nothing else and one `xdg-open` finds a file manager in all four sessions (not necessarily that desktop's own; the KDE session opens Thunar). The install script runs over SSH as the console account, so `HOME` is the right home and nothing needs root, and it sources the session's own `session.env` first: the one blessing it writes goes through the session's metadata daemon over the session bus, and without one `gio set` answers that the attribute is not supported. That blessing is `metadata::xfce-exe-checksum`, the file's sha256, which is what xfdesktop's "Mark As Secure And Launch" button writes and the only thing that will make it run a launcher on what it calls an insecure location. Plasma and Nemo were both seen to run an executable entry without asking, which is why there is one attribute and not three. All of it measured one boot per desktop, with the desktop screenshotted each time.

## The Linux guest

The Linux guest is Debian 13 with four desktops installed side by side, and nothing in the image decides which one a boot logs into. `provider::desktop` is the host half: `Desktop::session` maps the `--desktop` flag onto the `.desktop` basename sddm's `[Autologin] Session=` wants (`plasmax11`, `gnome-xorg`, `xfce`, `cinnamon`, none of which are the names one would guess), and `fw_cfg_args` puts it on QEMU's command line as `-fw_cfg name=opt/sunlit/desktop,string=<session>`. The `opt/` prefix is required; QEMU refuses anything outside it. In the guest, a oneshot unit ordered before `display-manager.service` reads `/sys/firmware/qemu_fw_cfg/by_name/opt/sunlit/desktop/raw`, checks the value against its own allowlist of the same four names, and writes sddm's autologin drop-in. `the_guest_accepts_exactly_the_sessions_the_host_can_ask_for` reads both lists and compares them, since nothing else connects the two: a name on one side and not the other is a boot that silently falls back to Plasma while `vm status` says otherwise. fw_cfg rather than SMBIOS OEM strings, which was the other candidate: the fw_cfg device is ACPI-enumerated so its module loads itself early and the value is a file in sysfs, while mainline exports no per-string sysfs interface for SMBIOS type 11 at all. The chosen desktop goes into `RunState`, so it survives the command that chose it and `vm status` can name it; `--desktop` against the Windows guest is refused rather than ignored, because a run whose flag did nothing is a run whose results are about a desktop nobody chose.

Debian 13 rather than a current Ubuntu, and that choice has a shelf life. It is the only current base where Plasma, GNOME and XFCE all have a first-class X11 session at once: GNOME 50 removed X11 upstream in March 2026 and Ubuntu 25.10 had already dropped the GNOME Xorg session, while trixie froze on GNOME 48 and Plasma 6.3. That insulates the image for trixie's support window (full to 2028-08, LTS to 2030-06) and no longer. The guest stays on X11 because the guest contract needs `DISPLAY` for a process the orchestrator starts over SSH, so a Wayland guest story is real work rather than polish and is on the roadmap as such.

Of Debian's two cloud images the base is `generic`, not the smaller `genericcloud`, and the difference is the only thing that makes a graphical guest possible: `genericcloud` ships `linux-image-cloud-amd64`, built without drivers for physical hardware, and DRM goes with them. No `CONFIG_DRM`, no `virtio_gpu` module, so no `/dev/dri` whatever the display device is, so logind's seat0 is not graphical and sddm waits for a display forever. The guest boots, answers SSH, autologs in as far as its configuration goes, and sits on the text console: a failure that says nothing about its cause, which is why `desktop.sh` checks `modinfo virtio_gpu` before it installs anything and names the image to use.

The disk is attached with `discard=unmap` and `finalize.sh` ends in `fstrim -av`. That is the whole of the size management, because the template also sets `skip_compaction`: Packer's own compaction converts the finished disk and renames the copy over the original, and on this host the rename is refused as long as something still holds the file. Trimming from inside the guest reaches the same end state without a second copy. Zeroing the free space, which is what a build does when compaction *will* run, is the one thing not to do here: it allocates every cluster it writes and there is no convert pass left to drop them.

Two more things about the QEMU command line, both in `qemu::Launch::args`. Every guest gets an absolute pointer (`qemu::pointer_args`), which is what makes a VNC viewer's cursor and the guest's the same cursor: VNC's `PointerEvent` carries absolute coordinates, QEMU's implicit PS/2 mouse is relative, and the translation between the two is the textbook cause of an offset cursor and of clicks that follow it. The Linux guest takes `virtio-tablet-pci`, driven in-kernel and consistent with its all-virtio device set; the Windows guest has no virtio driver at all and takes a `usb-tablet` on a `qemu-xhci`, which Windows binds to its inbox HID driver. Both halves are cold-plugged because `pcie.0` supports no hot-plug: `device_add qemu-xhci` on a running guest is refused, which is how the Windows guest's missing pointer was diagnosed in the first place, along with a `query-mice` that answered `QEMU PS/2 Mouse` with `"absolute": false` and an `input-send-event` with an `abs` axis that came back "Input handler not found for event type abs". And the display is `-device virtio-vga` carrying `xres`/`yres` rather than `-vga virtio`, because only the device form takes properties, and those properties are what set virtio-gpu's preferred mode. `SUNLIT_EARTH_VM_RESOLUTION` therefore applies to both providers now, through the shared `provider::console`; on QEMU it overrides `qemu::DEFAULT_CONSOLE` rather than a size fitted to the host's screen, since a VNC viewer scales and has nothing to fit.

## Ports and consoles

A QEMU guest's three loopback ports (the SSH forward, QMP, VNC) are preferred rather than fixed: `create_from_golden` records the first bindable port at or after 2222, 4444 and 5900, and `start` builds the command line from the record, because on a Windows host WinNAT reserves 100-port blocks for Hyper-V and WSL at moments of its own choosing and QEMU exits over a reserved port before it has built the machine. The walk is bounded by `qemu::PORT_WALK`, chosen so the three ranges can never meet. The waits that follow a boot (`wait_ssh`, the session wait, the job poll) consult `Provider::defunct` after every unanswered probe, so a QEMU that exited fails the command within a poll and quotes the tail of `vm.log` rather than sitting out the ten-minute SSH timeout; on Hyper-V only `Off` and an unregistered VM count as a verdict, because a guest still `Starting` and a failed state query are unknowns, not deaths.

A Hyper-V guest is looked at through a basic `vmconnect` session, which shows the framebuffer as it is: the window is the resolution, and nothing about it can be dragged larger. So the resolution is decided before the guest boots. `hyperv::console_resolution` takes the largest mode from `CONSOLE_MODES` that fits the host's work area, or whatever `SUNLIT_EARTH_VM_RESOLUTION` names, and `hyperv::video_script` puts it in both create scripts as `Set-VMVideo -ResolutionType Single`. Between `New-VM` and `Start-VM` is the only place it can go, because the cmdlet refuses to run against a VM that is on. `Single` rather than `Maximum`: `Maximum` advertises a list and leaves the guest to pick, which it does at 1024x768, the very default this exists to replace. What it costs is the guest's own display settings, which are then offered one mode; what it buys is a console that is the right size from the firmware's first frame, sized from the host, where the screen it has to fit on is.
