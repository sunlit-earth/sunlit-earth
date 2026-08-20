# Running the desktop e2e suite in a VM

The desktop end-to-end suite opens real windows, uses a real tray icon, and sets a real wallpaper. On a development machine that means it takes the desktop over for a minute; anywhere without an interactive desktop, including every hosted CI runner, it cannot run at all. This is how to run it in a local virtual machine instead.

Everything goes through `cargo xtask`. The design is in [plans/2026-08-19-phase3-vm-orchestration-plan.md](plans/2026-08-19-phase3-vm-orchestration-plan.md).

## The four steps

```
cargo xtask vm setup            # in an elevated shell, once per machine
                                # then restart, and sign out and back in
cargo xtask vm doctor           # confirms the host is ready
cargo xtask vm build-image linux
cargo xtask e2e --target linux
```

`vm setup` is the only command that changes the machine. It enables Hyper-V and the Windows Hypervisor Platform, installs QEMU, Packer, and an ISO builder, adds you to the Hyper-V Administrators group, registers the WSL distribution the Linux guest's binaries are built in, and generates the SSH key pair both guests trust. It never reboots and never signs you out: it reports what needs one and stops. Running it twice is a no-op the second time.

`vm doctor` is the opposite: unelevated, read-only, and the single place that answers "can this host run the suite". It prints a line per check and exits nonzero if any of them failed. Warnings are things that block one target or one convenience; failures block everything.

`vm build-image <target>` builds a golden image with Packer. Expect twenty minutes or so for Linux and the better part of an hour for Windows, plus several gigabytes of download. It is a one-time cost, repeated only when a template changes or a Windows evaluation expires.

> **The Windows image cannot currently be built on a Windows host.** Packer installs it in QEMU, QEMU on Windows uses the WHPX accelerator, and a WHPX guest with more than one vCPU does not survive the reboot the Windows installer performs after copying its files: the machine stops with `WHPX: Unexpected VP exit code 4` and cannot be restarted. Every workaround is blocked by another constraint, and the three of them are mutually exclusive; the measurements are under Troubleshooting below. `build-image windows` warns about this before it starts and ends the attempt after about four minutes instead of waiting out Packer's two-hour SSH timeout. A Linux host builds the image normally, because KVM does not have the fault. The Linux image builds and runs on a Windows host today.

`e2e --target <host|windows|linux>` runs the suite. `--target host` is what `cargo e2e` does: this desktop, with its real GPU. The other two boot a pristine VM, copy the current binaries in, run the suite in the guest's console session, pull the results back, and destroy the VM.

## What runs where

| | Windows guest | Linux guest | This desktop |
|---|---|---|---|
| Hypervisor | Hyper-V | QEMU | none |
| Host it runs from | Windows only | Windows or Linux | any |
| Cases | all 9 | 6 of 9 | 8 of 9 |
| GPU | WARP | lavapipe | the real one |

The Windows guest needs a Windows host, because its binaries have to be built somewhere and a Linux host has no toolchain for Windows executables. `e2e --target windows` says so and stops before creating anything.

The Windows guest runs a ninth case the others do not: it sets a real desktop wallpaper. That case is opt-in through `SUNLIT_EARTH_E2E_WALLPAPER`, which only the guest job sets, so running the suite on your own desktop leaves your wallpaper alone and says so.

The Linux guest skips the two cases that need a tray icon. One of them is the tray-start-hidden lifecycle; the other is single-instance enforcement, which the app performs in tray mode only, so on a platform without a tray there is nothing for it to enforce. Both print why they skipped. Linux tray support waits on the StatusNotifier work that comes after this phase.

## Interactive access

`cargo xtask vm up <target>` boots a guest and copies the current binaries in without running anything. `cargo xtask vm view <target>` opens its desktop, and `cargo xtask vm ssh <target>` opens a shell in it. To look at the aftermath of a test run instead, use `cargo xtask e2e --target <target> --keep` and then the same two commands.

There is no stop or pause, and that is deliberate. A guest holds no state worth keeping, so ending one and discarding it are the same act: `vm down` frees the memory and the overlay, leaves the golden image untouched, and the next `vm up` boots something pristine. Until you take it down, a running guest holds its RAM allocation. Nothing ever runs in the background unasked: a VM exists only during a run, after `--keep`, or after `vm up`.

Three things to know before you connect:

- For a Hyper-V guest, use the basic session that `vmconnect` opens by default. Enhanced session mode is RDP underneath and logs into a session of its own, which locks the console session out from under a running job. Plain `mstsc` does the same thing and should be avoided for the same reason.
- Watching a run is harmless. Clicking, typing, or moving the mouse during one perturbs the tests, which is the whole point of them having a desktop to themselves.
- The hypervisor console has essentially no clipboard integration, which is the price of it not being RDP. Text and files go in through `vm ssh` and `scp`.

## The Windows evaluation expires

The Windows guest is built from the Windows 11 Enterprise evaluation, which runs for 90 days from installation. The clock starts during the image build and never resets, because the image is read-only and every run boots a throwaway overlay of it.

An expired evaluation does not refuse to boot. Windows starts shutting itself down about once an hour, so a run inside it fails in the middle of whatever it was doing rather than failing cleanly. That is why it is checked before booting rather than diagnosed afterwards:

- `vm doctor` and `vm status` report the age from day one, warn from day 75, and report expired from day 90.
- `e2e --target windows` on an expired image prints an explanation and stops.
- `--allow-expired-image` proceeds anyway, for when the run is short enough not to care.

The fix is `cargo xtask vm build-image windows`, which is also the only way to get a fresh 90 days.

## Disk usage and cleanup

`cargo xtask vm status` lists what exists: the golden images with their sizes and build dates, the cached installation media, any overlays including ones a crashed run left behind, any VM that is registered or running and how to reach it, and what all of it costs. It prints the command to reclaim each part next to the numbers.

```
cargo xtask vm down <windows|linux|all>      # the guest and its run state
cargo xtask vm purge <windows|linux|all>     # that, the golden image, and the media
cargo xtask vm purge windows --iso           # only the 6.6 GB download
cargo xtask vm purge linux --image           # only the golden image and its leftovers
```

`vm down` stops the VM, deletes the overlay and the state file, and leaves the golden image alone. It is cheap and costs nothing to undo: the next run boots a fresh overlay of the same image.

`vm purge` deletes what took time to get: the golden image, its manifest, Packer's leftovers, and the cached installation media. It lists every file first and then asks, because rebuilding an image is tens of minutes and the Windows media is a 6.6 GB download; `-f` answers in advance, and so does a closed stdin answering no. The three flags are additive, and none of them means all of it.

`--purge` additionally deletes the golden image, the converted VHDX, the cached ISO, and the manifest. That is the disk-space recovery path. It prints what it deleted and how much it freed, and the next `vm status` reports the images as missing. Getting them back means another `vm build-image`, so purge when you need the space rather than as a matter of routine.

Neither command touches anything that is not the xtask's own. Every VM it creates is named `sunlit-e2e-<target>`, every file it writes lives under the image store, and a state file naming anything else is reported and left alone.

The images live outside the repository, in `%LOCALAPPDATA%\SunlitEarth\vm` on Windows and `~/.local/share/SunlitEarth/vm` on Linux. Set `SUNLIT_EARTH_VM_DIR` to put them somewhere else, on a bigger disk for instance. Budget 40 to 60 GB for both images plus their overlays and the Windows installation media.

## Troubleshooting

**The doctor says a feature is enabled but no hypervisor is running.** That is what a pending restart looks like. Restart and run it again.

**The doctor says you are not in the Hyper-V Administrators group, but setup added you.** Group membership reaches your token at logon, not before. Sign out and back in.

**QEMU is installed but the doctor cannot find it.** The winget package installs to `C:\Program Files\qemu` and does not touch `PATH`. The doctor looks there anyway, so this should not happen; if it does, add that directory to `PATH` yourself or re-run `vm setup`, which has a step for exactly this.

**A build fails immediately with "could not find a supported CD ISO creation command".** Each template hands its guest a small CD, the Linux one carrying its cloud-init seed and the Windows one its unattend file, and Packer builds that CD by shelling out. It looks for xorriso, mkisofs, hdiutil, or oscdimg, in that order, and nothing else. `vm setup` installs one (the winget package `Microsoft.OSCDIMG` on Windows, `xorriso` on Linux) and `vm doctor` reports which one Packer will pick. Both `vm doctor` and `vm build-image` check for it before anything starts.

**A build dies seconds after the ISO download, on `packer init`.** `packer init` fetches the plugins the template requires over HTTPS from `api.github.com`, and it is the first thing in the build that needs the network for itself rather than for a download the xtask ran. It is retried three times, and if all three fail the error names the usual causes. The one seen in practice was a per-application firewall that had not started after a reboot and was refusing sockets to every program it did not already know: `curl` had downloaded 6.6 GB happily a moment earlier, because it was on the allowed list and `packer.exe` was not. Packer reports that as `dial tcp ...: connectex: An attempt was made to access a socket in a way forbidden by its access permissions`, which reads like an outage. Nothing is built before this point, so re-running the command once the network is sorted costs nothing.

**A second `vm setup` in the same shell reports failed steps.** Fixed, and worth knowing why: winget adds the directory it links `packer` and `oscdimg` into to your user `PATH`, and a shell that was already running does not see it, so setup planned installs for packages that were already there and winget refused them. Detection now looks in that directory, and `vm doctor` calls a tool it finds there but not on `PATH` a warning rather than a failure. Opening a new shell clears it either way.

**The ISO download fails.** Microsoft publishes the evaluation behind a registration form and documents no direct link, so this is expected to break from time to time. The command prints the Evaluation Center page and the exact path to save the file at; download it by hand and run `vm build-image windows` again.

**A Windows build sits at the firmware logo and nothing happens.** The tell is that the output disk stays a few hundred kilobytes and the `progress:` lines say the guest is stopped. QEMU says why on its own stderr, which Packer captures rather than prints, so look in the `packer.log` the build names in its header: `WHPX: Unexpected VP exit code 4` means the hypervisor gave up on the vCPU, which QEMU's default guest CPU model provokes as soon as the Windows boot manager runs. The Windows template asks for `-cpu max` for exactly this reason; if you are looking at this on a modified template, that is the first thing to check.

**A build sits at "Waiting for SSH to become available" and Packer says nothing else.** That is Packer's normal state for most of a Windows install. Between the boot prompt and the guest's first SSH answer nothing on the host is driving the install, so Packer has nothing to report; SSH does not exist in the guest until `bootstrap.ps1` installs it at first logon. What the build adds is a `progress:` line a minute, from the signals that do exist: how much the output disk holds and how fast it is growing, whether the guest's screen is changing, and what QEMU's monitor says the machine is doing. A Windows install writes most of its gigabytes in the first few minutes and then spends a long stretch where the disk barely moves and the screen keeps changing; both of those are healthy. The build also leaves the guest's last screen at `<store>\build\<target>\screen.png` and Packer's own log at `<store>\build\<target>\packer.log`.

**A Windows build dies at the installer's first reboot, about four minutes and 11 GB in.** This is the one above, and this is what was measured on 2026-08-20 on an AMD Ryzen 7 5800X, Windows 11, QEMU 11.1.0. Three builds died identically; the rest was narrowed with a throwaway QEMU and a QMP `system_reset` rather than by repeating hour-long builds.

- A WHPX guest with more than one vCPU does not survive a guest reset. `WHPX: Unexpected VP exit code 4` is `WHvRunVpExitReasonUnrecoverableException`, a triple fault, and QEMU 11.1 pauses the machine rather than aborting, which is why it sits there looking like a slow install. Four vCPUs die, two die, one survives three resets in a row. The CPU model makes no difference (`max`, `host`, `qemu64`, `Skylake-Client`, `EPYC`), nor does the machine type or the firmware. Once stopped it is unrecoverable: `cont`, `system_reset`, and both together all leave it stopped or frozen.
- `kernel-irqchip=off`, which upstream reports as the workaround that keeps multiple vCPUs, is worse here: with QEMU's own APIC instead of the hypervisor's, this guest never leaves the TianoCore splash, at one vCPU or four. That is QEMU issue 3178 in a form 11.1 still has.
- One vCPU, which does survive resets, is refused by the product: Windows 11 Setup stops at "The processor needs to have two or more cores", and the unattend file's `BypassCPUCheck` key does not cover that check.

Upstream it is QEMU issues 858, 2042 and 2402, all open. The maintainer's fix, moving `WHvResetPartition` into the boot CPU's reset, was still unmerged on 2026-08-18, so no QEMU version has it yet and downgrading does not help either: the fault predates 11.1. The way to a Windows image today is a Linux host. Building it with Packer's `hyperv-iso` builder on a Windows host and converting the VHDX to qcow2 would also work and is not implemented.

**A build says the guest stopped, and ends itself.** QEMU has the machine halted, which is not something an installer can do to itself. The build starts it again once, and if it stops a second time at the same instruction with nothing written in between, it stops waiting: it says so, ends the guest, and lets the build fail now instead of at Packer's two-hour SSH timeout. That is worth the paragraph because of how it looked before the build watched for it. On 2026-08-20 two Windows builds each sat at `Waiting for SSH` for an hour and a quarter with a machine that had been stopped since minute four, paused in the firmware at the same instruction after the installer's first reboot; resuming it by hand over QMP got two seconds of firmware and another stop in the same place. Nothing in Packer's output distinguished that from a slow install. When it happens, `packer.log` has whatever QEMU said and `screen.png` has the screen it stopped on.

**A Windows build reaches "Start PXE over IPv4" and then hangs.** The keypress that answers "Press any key to boot from CD or DVD" was missed, so the firmware fell through every other boot option. That key does not come from Packer: its `boot_command` is empty, because its keystrokes were measured not to reach the guest, and `build-image` presses the key over QMP instead. It presses until the output disk shows the installer writing, and says which happened. If it could not reach the monitor, something else is on port 4445; if it reports presses without the installer starting, the boot is slower than `BOOT_KEY_MAX_PRESSES` in `build_watch.rs` allows.

**A Windows build stops at "This PC can't run Windows 11".** The unattend file's LabConfig bypass keys are undocumented and a revision can stop honoring them. `cargo xtask vm view windows` shows the installer's screen while a build is running, which is the fastest way to see where it stopped.

**`vm up` looks stuck after the guest boots.** It is building the e2e suite for that guest, which is a cold cargo build the first time and takes minutes; for the Linux guest it runs inside WSL against its own target directory. Cargo's progress goes to the terminal while its machine-readable output is parsed, so there is something to watch. `vm ssh` works while that build is still running: the guest is already up.

**A guest boots but never becomes reachable.** `vm view` shows its console. For a QEMU guest the console is a VNC server on `127.0.0.1:5900` that is always running, so a viewer can attach at any moment, including in the middle of a wedged boot.

**A run leaves a VM behind.** `vm status` finds it; `vm down <target>` removes it. The orchestrator writes its state file as soon as the VM exists, so a crash mid-run leaves something to clean up rather than an orphan nothing knows about, and any failure after a boot either destroys the guest or prints exactly what is still running and how to reach it.

**A teardown says it could not stop something and deleted nothing.** That is deliberate. A VM whose stop failed keeps its files, because unlinking the disk of a running guest destroys it mid-write and, in a purge, takes the golden image too. Fix whatever stopped it, then run it again: nothing was half-done, so it is safe to repeat.

**A teardown says a process id was reused.** The VM's process is gone and something unrelated now has its id, so nothing was stopped and nothing was deleted. Delete the state file it names once you are sure nothing of yours is running.

**A test fails in the guest but passes on the desktop.** The results are pulled back to the image store and the path is printed at the end of the run: `output.log` is the suite's own output and `artifacts/` is whatever it wrote. `e2e --target <t> --keep` leaves the VM up so you can look at it from the inside.
