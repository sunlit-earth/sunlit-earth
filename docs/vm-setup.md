# Running the desktop e2e suite in a VM

The desktop end-to-end suite opens real windows, uses a real tray icon, and sets a real wallpaper. On a development machine that means it takes the desktop over for a minute; anywhere without an interactive desktop, including every hosted CI runner, it cannot run at all. This is how to run it in a local virtual machine instead.

Everything goes through `cargo xtask`. The design is in [plans/2026-08-19-phase3-vm-orchestration-plan.md](plans/2026-08-19-phase3-vm-orchestration-plan.md), and the Linux guest's overhaul in [plans/2026-08-21-phase5-linux-vm-and-parity-plan.md](plans/2026-08-21-phase5-linux-vm-and-parity-plan.md).

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

One convenience it warns about rather than installs is a VNC viewer, which only `vm view` of a QEMU guest needs. It looks for `vncviewer`, `tigervnc`, `tvnviewer`, `remmina` and `vinagre`, by name on `PATH` and then in the places an installer is known to leave a program without putting it there: TightVNC's own directory under Program Files, winget's links directory, and scoop's shims directory. `scoop install tightvnc` on Windows and `apt install tigervnc-viewer` on Linux both satisfy it. A viewer installed in the shell that is running `vm view` still counts, which is the point of not asking `PATH` alone: both winget and scoop append to the user `PATH`, and a shell that started earlier never sees it.

`vm build-image <target>` builds a golden image. Expect the better part of an hour either way, plus several gigabytes of download: the Linux image installs four desktops, and the Windows one installs Windows. It is a one-time cost, repeated only when a template changes or a Windows evaluation expires.

Which mechanism performs the install depends on the host, and it mirrors the hypervisor the finished guest runs on:

| | Windows host | Linux host |
|---|---|---|
| Windows image | Hyper-V, driven by the xtask | Packer and QEMU on KVM |
| Linux image | Packer and QEMU on WHPX | Packer and QEMU on KVM |

The one cell that is not Packer is the one QEMU cannot do: on a Windows host QEMU runs on the WHPX accelerator, and a WHPX guest with more than one vCPU does not survive the reset Windows Setup performs after copying its files. The measurements are under Troubleshooting below. So on a Windows host the xtask installs Windows itself, on the hypervisor the guest will run on anyway: it repacks the installation media without its boot prompt, builds the unattend CD with oscdimg, creates a generation 2 VM, watches the install with a line a minute, runs the same finalize script over SSH, shuts the guest down, keeps its disk and drops the VM, and converts that disk to qcow2 for the QEMU provider. No Packer, no QEMU process, no keypress, and no network on the host after the ISO download. The guest itself fetches one thing at first logon, the Visual C++ runtime, because Windows does not ship it and every Rust binary the suite runs needs it.

Either way the result is one canonical install in two formats, so the Hyper-V provider and the QEMU provider boot the same Windows. What differs is only which format is derived from which.

`e2e --target <host|windows|linux>` runs the suite. `--target host` is what `cargo e2e` does: this desktop, with its real GPU. The other two boot a pristine VM, copy the current binaries in, run the suite in the guest's console session, pull the results back, and destroy the VM.

What goes into a guest is the app, the test harness, the fixtures, and the `textures/` directory if it holds the assets. The textures are Git LFS, so a checkout without the objects has files that exist and cannot be decoded; the run checks their size, says what it decided, and leaves the variable that points at them unset when there is nothing to point it at. One case samples the globe by color and needs them; a guest without them renders the procedural grid, exactly as this desktop would in the same state.

## What runs where

| | Windows guest | Linux guest | This desktop |
|---|---|---|---|
| Hypervisor | Hyper-V | QEMU | none |
| Guest OS | Windows 11 Enterprise evaluation | Debian 13, four desktops | whatever you are on |
| Host it runs from | Windows only | Windows or Linux | any |
| Cases | all 11 | 10 of 10 under KDE and XFCE, 8 of 10 under GNOME and Cinnamon | 10 of 11 |
| GPU | WARP | lavapipe | the real one |

The Windows guest needs a Windows host, because its binaries have to be built somewhere and a Linux host has no toolchain for Windows executables. `e2e --target windows` says so and stops before creating anything.

Both guests run the case that sets a real desktop wallpaper. It is opt-in through `SUNLIT_EARTH_E2E_WALLPAPER`, which only the guest jobs set, so running the suite on your own desktop leaves your wallpaper alone and says so. In the Linux guest that case is the one that proves a desktop's wallpaper backend, which is why the guest carries four desktops: `--desktop <kde|gnome|xfce|cinnamon>` runs the suite under each of them in turn, from one image, and each run exercises a different setter.

The Linux guest skips the two cases that need a tray icon. One of them is the tray-start-hidden lifecycle; the other is single-instance enforcement, which the app performs in tray mode only, so on a platform without a tray there is nothing for it to enforce. Both print why they skipped.

## Choosing the Linux guest's desktop

The Linux image carries KDE Plasma, GNOME, XFCE and Cinnamon, installed minimally and side by side, and the boot decides which one it logs into:

```
cargo xtask vm up linux --desktop gnome
cargo xtask e2e --target linux --desktop xfce
cargo xtask vm smoke linux --desktop cinnamon
```

With no flag it is KDE Plasma, which is the image's default. Every one of them is that desktop's X11 session and not its Wayland one, deliberately: the guest contract starts windowed processes over SSH through `DISPLAY`, and a Wayland session has none to hand out.

Nothing in the image decides this, so switching desktops costs a boot rather than a rebuild. The host adds `-fw_cfg name=opt/sunlit/desktop,string=<session>` to QEMU's command line; in the guest, a oneshot unit ordered before the display manager reads the value out of `/sys/firmware/qemu_fw_cfg`, checks it against its own allowlist of four names, and writes sddm's `[Autologin] Session=`. Which desktop a guest is running goes into its run record, so `cargo xtask vm status` names it and a run's results say which desktop they came from.

`--desktop` is a Linux guest option and the Windows image has one desktop, so asking for one there is refused rather than ignored: a run whose flag did nothing is a run whose results are about a desktop nobody chose.

Only Windows has one setter for every session. Linux has one per desktop, so `check_supported` there reads `XDG_CURRENT_DESKTOP` and looks for that desktop's own tool before anything is rendered. All four in the image have been run and the wallpaper looked at afterwards in each; MATE, LXQt and Budgie have table rows written from their documented setters and have never run, which `docs/roadmap.md` says rather than the docs claiming support nothing produced.

Looking is not optional, and XFCE is why. Its setter exited zero, the case passed, and the desktop went on showing xfdesktop's default image, because the property xfdesktop reads is named after the connected monitor and does not exist until something creates it. A run that only reads test output would have called that a pass.

The case count differs by desktop because the tray does. `tests/e2e.rs` asks `gdbus` who owns `org.kde.StatusNotifierWatcher`, and Plasma and xfce4-panel answer while GNOME and Cinnamon do not, so the two cases that need a tray icon run under the first two and skip under the other two, saying so.

Cinnamon's session has a defect worth knowing before looking at one: the shell starts, is signalled a second later, and `cinnamon-launcher` falls back to `metacity`, which this image does not install, so what is left on screen is a modal "Cinnamon just crashed" dialog over a black desktop. The suite is unaffected, since it runs over SSH through `DISPLAY`, and `cinnamon --replace` over `vm ssh` brings the shell and the wallpaper back. `docs/roadmap.md` carries it.

## Interactive access

`cargo xtask vm up <target>` boots a guest and copies the current binaries in without running anything. `cargo xtask vm view <target>` opens its desktop, and `cargo xtask vm ssh <target>` opens a shell in it. To look at the aftermath of a test run instead, use `cargo xtask e2e --target <target> --keep` and then the same two commands.

There is no stop or pause, and that is deliberate. A guest holds no state worth keeping, so ending one and discarding it are the same act: `vm down` frees the memory and the overlay, leaves the golden image untouched, and the next `vm up` boots something pristine. Until you take it down, a running guest holds its RAM allocation. Nothing ever runs in the background unasked: a VM exists only during a run, after `--keep`, or after `vm up`.

A Windows guest's desktop has two shortcuts on it, written per boot by whatever staged the binaries:

- `Sunlit Earth` starts the app through `C:\sunlit-e2e\run-app.cmd`, which sets `SLINT_BACKEND=winit-software` and, when the textures were staged, `SUNLIT_EARTH_TEXTURES`. That is the same backend the generated job sets and for the same reason: the guest has no OpenGL, and the app started without it dies before its window appears. The launcher keeps its console window, so the app's log is on screen while it runs.
- `sunlit-e2e` opens the directory the binaries, fixtures and results are in.

A Windows guest offers one of two consoles, and which one depends on whether it was handed over. `vm up` and `e2e --keep` hand a guest over and record that they did; nothing else does, so `vm smoke --keep` leaves a guest with the second kind of console and an empty desktop, and says so.

**A guest handed to you**, by `vm up` or by `e2e --keep`, offers an enhanced session. That is the only one that can be resized: drag the window and the guest's desktop follows it. It is RDP underneath, so `vmconnect` asks for credentials, and the hand-over makes that a dialog to dismiss rather than fill in. It blanks the `tester` account's password, clears the LSA policy that otherwise confines a blank-password account to the physical console, and starts Remote Desktop Services. So: username `tester`, password field empty, connect. Nothing is weakened that was not already public: the password was in `Autounattend.xml` in plain text, and the guest is reachable from its own host and nowhere else.

The display-configuration dialog in front of that is answered for you. `vmconnect` files those settings per VM identifier, and every `vm up` creates a VM with a new one, so ticking "save my settings for future connections to this virtual machine" lasts exactly until the next boot. `vm view` writes the file itself before starting `vmconnect`, with the same size the console would have had, and deletes the ones earlier guests of ours left behind; taking a guest down deletes its own, since the identifier it is filed under means nothing once the VM is gone.

**A guest nobody handed over**, which includes every guest with a run in it, offers no enhanced session at all, and `vmconnect` opens a basic session that asks for nothing. That is deliberate rather than an oversight: connecting over RDP moves the console session into the RDP one, and the console session is where the windowed tests keep their desktop. The golden image ships with Remote Desktop Services disabled for exactly this reason, and only a hand-over turns it on.

A credential dialog can appear for such a guest anyway, and the answer is the same whatever produced it: cancel it rather than signing in, for the reason above. Usually the image is one built before that change, which still has the service enabled, since a stale image only warns at boot rather than refusing to run; `cargo xtask vm build-image windows` replaces it. Two other guests present the same dialog with no stale image behind them: one whose hand-over started the service and then failed before the guest confirmed it, and one that is still being installed by `vm build-image`, which has no golden image yet at all. Every text that says a guest offers no enhanced session says this too, `vm view`'s and the closing lines of `vm smoke --keep` alike, from one constant.

A basic session shows the framebuffer as it is, so its window is the guest's resolution and cannot be dragged. The resolution is therefore chosen before the guest boots and reported on the line that creates it: the largest of 1024x768, 1280x800, 1440x900, 1600x900, 1680x1050 and 1920x1080 that fits this host's screen with room for the window's own frame. `SUNLIT_EARTH_VM_RESOLUTION=2560x1440` asks for something specific, including sizes larger than any on that list, which the guest's synthetic adapter honors. Changing it means taking the guest down and bringing it up again, because `Set-VMVideo` refuses to run against a VM that is on; and the guest is given exactly the one mode, so its own display settings offer nothing else to pick.

`SUNLIT_EARTH_VM_RESOLUTION` applies to the Linux guest too, where it overrides a fixed 1920x1080 rather than a size derived from this host's screen: a VNC viewer scales, so there is nothing there that has to fit. It reaches the guest as `xres` and `yres` on the `virtio-vga` device, which is what sets virtio-gpu's preferred mode, and Xorg takes that mode when it starts. A guest already running keeps the size it booted with, the same as on Hyper-V.

Clicks in the Linux guest's VNC console land where the cursor is, which they did not before this phase: the guest has a `virtio-tablet-pci`, an absolute pointer. VNC's own protocol carries absolute coordinates and QEMU's implicit PS/2 mouse is a relative device, so without an absolute pointer in the guest every click went through a translation layer and landed somewhere else.

Two more things either way:

- Watching a run is harmless. Clicking, typing, or moving the mouse during one perturbs the tests, which is the whole point of them having a desktop to themselves.
- A basic session carries no clipboard, and neither does the VNC console of a Linux guest. An enhanced session is RDP, so it does: text can be pasted straight into a guest that was handed over. Files go in through `vm ssh` and `scp` either way.

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

`vm purge` deletes what took time to get: the golden image, its manifest, the build directory's leftovers, and the cached installation media, which for Windows is the download, the prompt-free copy made from it, and the small record saying which download that copy came from. It lists every file first and then asks, because rebuilding an image is tens of minutes and the Windows media is a 6.6 GB download; `-f` answers in advance, and so does a closed stdin answering no. The three flags are additive, and none of them means all of it.

A purge that has to stop a guest says what stopping it costs, on the line that says it is being stopped and in the question, and `-f` skips the question rather than the warning. That matters for one guest only: an image build. A purge that ends a build stops there and does not also clear the build's record and the disk its install had written, since those are run state and no flag asked for them; it names both and points at `vm down <target>`, which is what a `vm status` full of a build that is not running is telling you afterwards.

Neither command touches anything that is not the xtask's own. Every VM it creates is named `sunlit-e2e-<target>`, every file it writes lives under the image store, and a state file naming anything else is reported and left alone.

The images live outside the repository, in `%LOCALAPPDATA%\SunlitEarth\vm` on Windows and `~/.local/share/SunlitEarth/vm` on Linux. Set `SUNLIT_EARTH_VM_DIR` to put them somewhere else, on a bigger disk for instance. Budget 40 to 60 GB for both images plus their overlays and the 6.6 GB Windows download.

The prompt-free copy of that download does not stay in that budget. A native Windows build repacks the media to take the boot prompt out of it, and a build that produced an image deletes the copy and the record beside it on the way out: it is 6.6 GB of derived data, and remaking it costs minutes against a build that costs an hour. A build that failed keeps it and says so, since the retry is the one occasion when that saving is worth having.

## Troubleshooting

**The doctor says a feature is enabled but no hypervisor is running.** That is what a pending restart looks like. Restart and run it again.

**The doctor says you are not in the Hyper-V Administrators group, but setup added you.** Group membership reaches your token at logon, not before. Sign out and back in.

**QEMU is installed but the doctor cannot find it.** The winget package installs to `C:\Program Files\qemu` and does not touch `PATH`. The doctor looks there anyway, so this should not happen; if it does, add that directory to `PATH` yourself or re-run `vm setup`, which has a step for exactly this.

**A build fails immediately with "could not find a supported CD ISO creation command".** Every guest is handed a small CD, the Linux one carrying its cloud-init seed and the Windows one its unattend file, and building that CD means shelling out to an ISO builder. Packer looks for xorriso, mkisofs, hdiutil, or oscdimg, in that order, and nothing else; a native Hyper-V build uses oscdimg directly, both for the unattend CD and for the media repack. `vm setup` installs one (the winget package `Microsoft.OSCDIMG` on Windows, `xorriso` on Linux) and `vm doctor` reports which one Packer will pick. Both `vm doctor` and `vm build-image` check for it before anything starts.

**A build dies seconds after the ISO download, on `packer init`.** `packer init` fetches the plugins the template requires over HTTPS from `api.github.com`, and it is the first thing in the build that needs the network for itself rather than for a download the xtask ran. It is retried three times, and if all three fail the error names the usual causes. The one seen in practice was a per-application firewall that had not started after a reboot and was refusing sockets to every program it did not already know: `curl` had downloaded 6.6 GB happily a moment earlier, because it was on the allowed list and `packer.exe` was not. Packer reports that as `dial tcp ...: connectex: An attempt was made to access a socket in a way forbidden by its access permissions`, which reads like an outage. Nothing is built before this point, so re-running the command once the network is sorted costs nothing.

**A second `vm setup` in the same shell reports failed steps.** Fixed, and worth knowing why: winget adds the directory it links `packer` and `oscdimg` into to your user `PATH`, and a shell that was already running does not see it, so setup planned installs for packages that were already there and winget refused them. Detection now looks in that directory, and `vm doctor` calls a tool it finds there but not on `PATH` a warning rather than a failure. Opening a new shell clears it either way.

**The ISO download fails.** Microsoft publishes the evaluation behind a registration form and documents no direct link, so this is expected to break from time to time. The command prints the Evaluation Center page and the exact path to save the file at; download it by hand and run `vm build-image windows` again.

**A native Windows build leaves a VM behind.** That is deliberate: a failed install is the one thing worth looking inside. The record is written before the VM exists, so `vm status` names it as an image build whether it is still running or a crash left it registered, `vm view windows` shows its console and `vm ssh windows` reaches it once Windows is up, and `vm down windows` removes it along with the unfinished disk. A half-built image is worth nothing, so nothing tries to keep one. The failure asks the hypervisor whether the VM is actually there before saying any of this: a build that fell over before creating one says so instead of pointing at a console that does not exist, and one that could not get an answer says that too.

**A native Windows build fails before it creates anything.** Three preflight failures are worth naming. A missing `oscdimg` is the entry above. "this session is not a member of the Hyper-V Administrators group" means the membership `vm setup` granted has not reached your token yet: sign out and back in, then run `vm doctor`. And the media repack copies the installation media out of a mounted image and back, so it wants about 13 GB free under the build directory while it runs; the message says so, and the extraction tree is deleted on the way out either way.

**A native Windows build repacks the media when you expected it to reuse the cached copy.** The prompt-free copy records which download it was made from, as a size and a CRC-32 in `windows11-enterprise-eval-noprompt.source.json` beside it, and a build checks that record before reusing the copy. A re-downloaded original, or a copy from before that record existed, is repacked, which costs about a minute. If the original is no longer on disk the copy is used as it stands and the build says nothing checked it: the copy is what the install boots, and re-fetching 6.6 GB to remake a file that is already there would cost more than the check is worth. `vm purge windows --iso` clears both files and the record together, which is how you ask for a fresh download.

**`ssh` warns that the remote host identification has changed.** A guest's host keys live on the golden image, so they change when the image is rebuilt and stay the same across every run in between. The xtask keeps its own known-hosts file at `<store>/ssh/known_hosts` rather than touching yours, and a build deletes it for that reason whether it produced an image or failed: a build that failed after its guest answered on SSH has already written an entry that later runs would warn about. If the warning appears anyway, for instance after an image was replaced by hand, delete that file; nothing else needs it, and public-key authentication was never affected.

**A native Windows install reboots back into Setup instead of continuing.** The install starts from media repacked without its boot prompt, so a boot from that DVD goes straight into Setup with no key to press. What stops the mid-install reboots from looping is the firmware: Windows Setup's first phase writes a Windows Boot Manager entry into the generation 2 VM's UEFI variable store and puts it first in the boot order. The build prints that entry when it changes, as a `firmware:` line, so the moment it stops being the DVD is visible minutes in. If a future Windows revision does not write it, the fix is to detach the install DVD once the disk shows the installer writing, which Hyper-V permits on a running generation 2 VM.

**A native build is running and you want to see the installer's screen.** `cargo xtask vm view windows` opens it, and the build does not mind being watched. Where a window is not wanted, Hyper-V will hand over the screen as pixels instead: `Msvm_VirtualSystemManagementService.GetVirtualSystemThumbnailImage` in the `root\virtualization\v2` WMI namespace returns the console as RGB565, which is what answered "is the bootstrap still installing OpenSSH or has it died" during the first live build. The build does not take screenshots itself; the QEMU path does, because QEMU encodes the PNG for it.

**A native build's `progress:` lines say the guest is Off.** A Hyper-V guest stays running through the reboots an install performs, so anything else partway through means the install ended: the machine powered itself off rather than rebooting. The build says which state it saw and stops there rather than waiting out its two-hour deadline, and the disk it wrote is left where it is. `vm view windows` before you take it down shows what was on screen.

**A Windows build under QEMU dies at the installer's first reboot, about four minutes and 11 GB in.** This is why a Windows host does not build this image with Packer any more. Measured on 2026-08-20 on an AMD Ryzen 7 5800X, Windows 11, QEMU 11.1.0: three builds died identically, and the rest was narrowed with a throwaway QEMU and a `system_reset` rather than by repeating hour-long builds.

- A WHPX guest with more than one vCPU does not survive a guest reset. `WHPX: Unexpected VP exit code 4` is `WHvRunVpExitReasonUnrecoverableException`, a triple fault, and QEMU 11.1 pauses the machine rather than aborting, which is why it sits there looking like a slow install. Four vCPUs die, two die, one survives three resets in a row. The CPU model makes no difference (`max`, `host`, `qemu64`, `Skylake-Client`, `EPYC`), nor does the machine type or the firmware. Once stopped it is unrecoverable: `cont`, `system_reset`, and both together all leave it stopped or frozen.
- `kernel-irqchip=off`, which upstream reports as the workaround that keeps multiple vCPUs, is worse here: with QEMU's own APIC instead of the hypervisor's, this guest never leaves the TianoCore splash, at one vCPU or four. That is QEMU issue 3178 in a form 11.1 still has.
- One vCPU, which does survive resets, is refused by the product: Windows 11 Setup stops at "The processor needs to have two or more cores", and the unattend file's `BypassCPUCheck` key does not cover that check.

Upstream it is QEMU issues 858, 2042 and 2402, all open. The maintainer's fix, moving `WHvResetPartition` into the boot CPU's reset, was still unmerged on 2026-08-18, so no released QEMU had it, and downgrading does not help either: the fault predates 11.1.

What still runs into WHPX on a Windows host is booting the *finished* Windows guest through the provider override, `SUNLIT_EARTH_VM_PROVIDER=qemu`. There `-cpu max` remains load-bearing: with QEMU's default guest CPU the hypervisor gives up on the vCPU as soon as the Windows boot manager runs, and the guest sits on the firmware logo with nothing written. The Windows template and the runtime QEMU provider both ask for it, and two tests pin that they do, one per side. A third pins the two-core floor Setup insists on, which is the other half of the same measurement.

Rechecking a newer QEMU takes twenty seconds and needs no build. Boot a throwaway guest off the Windows media, reset it by hand, and ask what state it is in. Copy `edk2-i386-vars.fd` beside your working directory as `vars.fd` first, because the firmware writes to it:

```
qemu-system-x86_64 -accel whpx -machine q35 -cpu max -smp 4 -m 2048 ^
  -drive if=pflash,format=raw,readonly=on,file="C:\Program Files\qemu\share\edk2-x86_64-code.fd" ^
  -drive if=pflash,format=raw,file=vars.fd ^
  -cdrom "%LOCALAPPDATA%\SunlitEarth\vm\iso\windows11-enterprise-eval.iso" ^
  -boot order=d -display none -monitor stdio
```

Then, at the `(qemu)` prompt, which is the human form of the QMP `system_reset` and `query-status` the narrowing used:

```
(qemu) system_reset
(qemu) info status
```

`VM status: running` after two or three resets with four vCPUs means the fault is gone and a Windows host could use the Packer path again. `VM status: paused`, with `WHPX: Unexpected VP exit code 4` on stderr, means it is not. Answering the media's boot prompt is not necessary: what is being tested is whether the machine survives a reset at all.

**A Packer build sits at "Waiting for SSH to become available" and says nothing else.** That is Packer's normal state for most of an install: the Linux image on either host, and the Windows image on a Linux host. Between the boot prompt and the guest's first SSH answer nothing on the host is driving the install, so Packer has nothing to report; SSH does not exist in a Windows guest until `bootstrap.ps1` installs it at first logon. What the build adds is a `progress:` line a minute, from the signals that do exist: how much the output disk holds and how fast it is growing, whether the guest's screen is changing, and what QEMU's monitor says the machine is doing. A Windows install writes most of its gigabytes in the first few minutes and then spends a long stretch where the disk barely moves and the screen keeps changing; both of those are healthy. The build also leaves the guest's last screen at `<store>\build\<target>\screen.png` and Packer's own log at `<store>\build\<target>\packer.log`. A native Hyper-V build prints the same kind of line from the signals Hyper-V has: the VM's state, the growing VHDX, and the firmware's first boot entry.

**A Packer build says the guest stopped, and ends itself.** QEMU has the machine halted, which is not something an installer can do to itself. The build starts it again once, and if it stops a second time at the same instruction with nothing written in between, it stops waiting: it says so, ends the guest, and lets the build fail now instead of at Packer's two-hour SSH timeout. That is worth the paragraph because of how it looked before the build watched for it. On 2026-08-20 two Windows builds each sat at `Waiting for SSH` for an hour and a quarter with a machine that had been stopped since minute four, paused in the firmware at the same instruction after the installer's first reboot; resuming it by hand over QMP got two seconds of firmware and another stop in the same place. Nothing in Packer's output distinguished that from a slow install. When it happens, `packer.log` has whatever QEMU said and `screen.png` has the screen it stopped on.

**A Packer build of the Windows image reaches "Start PXE over IPv4" and then hangs.** The keypress that answers "Press any key to boot from CD or DVD" was missed, so the firmware fell through every other boot option. That key does not come from Packer: its `boot_command` is empty, because its keystrokes were measured not to reach the guest, and `build-image` presses the key over QMP instead. It presses until the output disk shows the installer writing, and says which happened. If it could not reach the monitor, something else is on port 4445; if it reports presses without the installer starting, the boot is slower than `BOOT_KEY_MAX_PRESSES` in `build_watch.rs` allows. A native Hyper-V build has none of this, because the media it boots has no prompt to answer.

**A Windows build stops at "This PC can't run Windows 11".** The unattend file's LabConfig bypass keys are undocumented and a revision can stop honoring them, and that is the same on both builders because the unattend file is shared. `cargo xtask vm view windows` shows the installer's screen while a build is running, which is the fastest way to see where it stopped.

**`vm up` looks stuck after the guest boots.** It is building the e2e suite for that guest, which is a cold cargo build the first time and takes minutes; for the Linux guest it runs inside WSL against its own target directory. Cargo's progress goes to the terminal while its machine-readable output is parsed, so there is something to watch. `vm ssh` works while that build is still running: the guest is already up.

**A guest boots but never becomes reachable.** `vm view` shows its console. For a QEMU guest the console is a VNC server on `127.0.0.1:5900` that is always running, so a viewer can attach at any moment, including in the middle of a wedged boot.

**A run leaves a VM behind.** `vm status` finds it; `vm down <target>` removes it. The orchestrator writes its state file as soon as the VM exists, so a crash mid-run leaves something to clean up rather than an orphan nothing knows about, and any failure after a boot either destroys the guest or prints exactly what is still running and how to reach it.

**A teardown says it could not stop something and deleted nothing.** That is deliberate. A VM whose stop failed keeps its files, because unlinking the disk of a running guest destroys it mid-write and, in a purge, takes the golden image too. Fix whatever stopped it, then run it again: nothing was half-done, so it is safe to repeat.

**A teardown says a process id was reused.** The VM's process is gone and something unrelated now has its id, so nothing was stopped and nothing was deleted. Delete the state file it names once you are sure nothing of yours is running.

**Every case in the guest fails with exit code -1073741515 and an empty log.** That is `0xC0000135`, `STATUS_DLL_NOT_FOUND`: the binary could not start, and nothing prints when that happens. A fresh Windows 11 has only the `_clr0400` copies of `vcruntime140.dll` and `msvcp140.dll`, which belong to .NET, and every Rust MSVC binary links the plain ones. The golden image installs the Visual C++ runtime at first logon and `finalize.ps1` refuses to finish an image without it, so this means an image built before that was added: rebuild it. `cargo xtask vm ssh windows "dir /b C:\Windows\System32\vcruntime140.dll"` answers the question in one line.

**The app in a Windows guest says "Could not locate glCreateShader symbol" and every windowed case times out.** A Hyper-V guest's display adapter has no OpenGL, and Windows ships no software implementation of it, so Slint's default renderer cannot start. The guest job sets `SLINT_BACKEND=winit-software` for exactly this, which keeps the real window and swaps the renderer for the CPU one. Seeing this means something ran the app in the guest without that variable: a hand-typed command will, and so will an older job script. Start it through the desktop shortcut or `C:\sunlit-e2e\run-app.cmd`, which sets it.

**The app in a Windows guest logs `windows_read_data_files_in_registry: Registry lookup failed to get ICD manifest files. Possibly missing Vulkan driver?`** This one is noise, however it reads: wgpu logs it at error level while probing its Vulkan backend, and the guest has no Vulkan driver because a synthetic display adapter does not come with one. The next line says which adapter was actually selected, which in a guest is `Microsoft Basic Render Driver (Dx12, Cpu)`, WARP, the same software rasterizer the golden tests run on. A `render` in a guest exits 0 and produces a correct PNG with these lines in its log.

**The render case fails on a color, such as "Sahara: expected yellowish/sandy".** The guest has no textures, so it rendered the procedural grid and the sampled points are whatever the grid has there. The run says at the top whether it staged them and why not; `git lfs pull` is the usual answer.

**A test fails in the guest but passes on the desktop.** The results are pulled back to the image store and the path is printed at the end of the run: `output.log` is the suite's own output and `artifacts/` is whatever it wrote. `e2e --target <t> --keep` leaves the VM up so you can look at it from the inside.
