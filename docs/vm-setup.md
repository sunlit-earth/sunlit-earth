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

`vm setup` is the only command that changes the machine. It enables Hyper-V and the Windows Hypervisor Platform, installs QEMU and Packer, adds you to the Hyper-V Administrators group, registers the WSL distribution the Linux guest's binaries are built in, and generates the SSH key pair both guests trust. It never reboots and never signs you out: it reports what needs one and stops. Running it twice is a no-op the second time.

`vm doctor` is the opposite: unelevated, read-only, and the single place that answers "can this host run the suite". It prints a line per check and exits nonzero if any of them failed. Warnings are things that block one target or one convenience; failures block everything.

`vm build-image <target>` builds a golden image with Packer. Expect twenty minutes or so for Linux and the better part of an hour for Windows, plus several gigabytes of download. It is a one-time cost, repeated only when a template changes or a Windows evaluation expires.

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

There is no stop or pause, and that is deliberate. A guest holds no state worth keeping, so ending one and discarding it are the same act: `vm destroy` frees the memory and the overlay, leaves the golden image untouched, and the next `vm up` boots something pristine. Until you destroy it, a running guest holds its RAM allocation. Nothing ever runs in the background unasked: a VM exists only during a run, after `--keep`, or after `vm up`.

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
cargo xtask vm destroy <windows|linux|all>            # run state only
cargo xtask vm destroy <windows|linux|all> --purge    # images and media too
```

A plain destroy stops the VM, deletes the overlay and the state file, and leaves the golden image alone. It is cheap and costs nothing to undo: the next run boots a fresh overlay of the same image.

`--purge` additionally deletes the golden image, the converted VHDX, the cached ISO, and the manifest. That is the disk-space recovery path. It prints what it deleted and how much it freed, and the next `vm status` reports the images as missing. Getting them back means another `vm build-image`, so purge when you need the space rather than as a matter of routine.

Neither command touches anything that is not the xtask's own. Every VM it creates is named `sunlit-e2e-<target>`, every file it writes lives under the image store, and a state file naming anything else is reported and left alone.

The images live outside the repository, in `%LOCALAPPDATA%\SunlitEarth\vm` on Windows and `~/.local/share/SunlitEarth/vm` on Linux. Set `SUNLIT_EARTH_VM_DIR` to put them somewhere else, on a bigger disk for instance. Budget 40 to 60 GB for both images plus their overlays and the Windows installation media.

## Troubleshooting

**The doctor says a feature is enabled but no hypervisor is running.** That is what a pending restart looks like. Restart and run it again.

**The doctor says you are not in the Hyper-V Administrators group, but setup added you.** Group membership reaches your token at logon, not before. Sign out and back in.

**QEMU is installed but the doctor cannot find it.** The winget package installs to `C:\Program Files\qemu` and does not touch `PATH`. The doctor looks there anyway, so this should not happen; if it does, add that directory to `PATH` yourself or re-run `vm setup`, which has a step for exactly this.

**The ISO download fails.** Microsoft publishes the evaluation behind a registration form and documents no direct link, so this is expected to break from time to time. The command prints the Evaluation Center page and the exact path to save the file at; download it by hand and run `vm build-image windows` again.

**A Windows build stops at "This PC can't run Windows 11".** The unattend file's LabConfig bypass keys are undocumented and a revision can stop honoring them. `cargo xtask vm view windows` shows the installer's screen while a build is running, which is the fastest way to see where it stopped.

**A guest boots but never becomes reachable.** `vm view` shows its console. For a QEMU guest the console is a VNC server on `127.0.0.1:5900` that is always running, so a viewer can attach at any moment, including in the middle of a wedged boot.

**A run leaves a VM behind.** `vm status` finds it; `vm destroy <target>` removes it. The orchestrator writes its state file as soon as the VM exists, so a crash mid-run leaves something to clean up rather than an orphan nothing knows about, and any failure after a boot either destroys the guest or prints exactly what is still running and how to reach it.

**`vm destroy` says it could not stop something and deleted nothing.** That is deliberate. A VM whose stop failed keeps its files, because unlinking the disk of a running guest destroys it mid-write and, with `--purge`, takes the golden image too. Fix whatever stopped it, then run the destroy again: nothing was half-done, so it is safe to repeat.

**`vm destroy` says a process id was reused.** The VM's process is gone and something unrelated now has its id, so nothing was stopped and nothing was deleted. Delete the state file it names once you are sure nothing of yours is running.

**A test fails in the guest but passes on the desktop.** The results are pulled back to the image store and the path is printed at the end of the run: `output.log` is the suite's own output and `artifacts/` is whatever it wrote. `e2e --target <t> --keep` leaves the VM up so you can look at it from the inside.
