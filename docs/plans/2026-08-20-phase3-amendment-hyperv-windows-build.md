# Plan Amendment: The Windows Image Builds Natively on Hyper-V

Amends `2026-08-19-phase3-vm-orchestration-plan.md`. Written 2026-08-20, after the events deviation 25 of that plan records: on a Windows host QEMU runs on WHPX, a WHPX guest with more than one vCPU does not survive the guest reset Windows Setup performs after copying its files, and each of the three workarounds is blocked by another constraint. The host decision that deviation left open is now made: on a Windows host, `cargo xtask vm build-image windows` installs Windows on Hyper-V, driven by the xtask directly.

Packer's `hyperv-iso` builder was considered for this and declined. Third-party keyboard boot orchestration is the mechanism that already failed once on this host and cost the most to diagnose (deviation 22); the plugin would add a `packer init` network dependency to a path that otherwise sheds one (deviation 20's failure class); and agreement between an HCL file and the Rust orchestrator is harder to test than agreement inside one crate, which is where the second validation round's defect class lived. What Packer still contributed on this target had already shrunk to CD creation (shelled out to oscdimg), the SSH wait, one provisioner, and a shutdown command, each of which the xtask already performs elsewhere. The research section's recorded fallback, moving image building into the xtask and reimplementing the boot orchestration Packer supplies, is what this amendment does, on the hypervisor where that orchestration is smallest.

## What changes, and what does not

The builder matrix now mirrors the runtime provider matrix:

| | Windows host | Linux host |
|---|---|---|
| Windows image | native Hyper-V (this amendment) | Packer + QEMU on KVM (unchanged) |
| Linux image | Packer + QEMU on WHPX (unchanged) | Packer + QEMU on KVM (unchanged) |

Decision 10's intent, one canonical install that both providers boot, stands. What flips on a Windows host is the conversion direction: the install produces a VHDX natively and `qemu-img convert` derives the qcow2 for the QEMU provider, instead of the reverse. On this host the primary runtime provider is Hyper-V, so the common path now boots the install in its native format, and the derived qcow2 serves only the `SUNLIT_EARTH_VM_PROVIDER=qemu` override cell.

The guest-side files are provider-agnostic and carry over unchanged. `Autounattend.xml` names no devices, and Hyper-V's synthetic SCSI disk and NIC have in-box Windows 11 drivers, which is exactly the property deviation 4 chose the QEMU devices for. `bootstrap.ps1`, `run-job.cmd`, `session-ready.cmd`, and `finalize.ps1` run inside the guest; `finalize.ps1`'s fallback-loader work (deviation 16) remains necessary on this path too, because the runtime VMs are created fresh with blank NVRAM. The runtime providers, the guest contract, the manifest with its expiry and currency model, and the Linux image are untouched. The Packer QEMU template stays what a Linux host uses to build the Windows image, unchanged in behavior.

## Key design decisions (continuing the plan's numbering)

15. **The xtask drives the install; no builder in the loop.** The build creates the VM, boots it, waits for SSH the way the guest contract already does, runs `finalize.ps1` over the existing SSH machinery, shuts the guest down with the same `shutdown /s /t 5 /f /d p:4:1` the template used, waits for the Off state, removes the VM keeping its disk, moves the VHDX into the image store, converts, and writes the manifest. The unattend CD is built by the xtask with oscdimg from the same files the HCL template's `cd_files` block names, plus the two generated entries (`authorized_keys` and the media marker); the file list lives in one Rust constant, and an agreement test reads the HCL and asserts both name the same files, the technique that caught the e1000/virtio divergence in the second validation round.

16. **The boot prompt is removed from the media rather than answered.** Windows installation media carries two UEFI boot images: `efi\microsoft\boot\efisys.bin`, which prints "Press any key to boot from CD or DVD", and `efisys_noprompt.bin`, which boots the installer immediately. The build repacks the downloaded ISO once with the noprompt image: mount with `Mount-DiskImage`, copy the tree out, dismount, then `oscdimg -m -o -u2 -udfver102 -bootdata:2#p0,e,b<tree>\boot\etfsboot.com#pEF,e,b<tree>\efi\microsoft\boot\efisys_noprompt.bin <tree> <out>`. A generation 2 VM boots the EFI entry; the BIOS entry is kept because it costs nothing. No prompt means no keypress, no keyboard injection, and none of deviation 22's machinery on this path. The repacked ISO is cached next to the original in the media directory and both are inventory: `vm status` lists them and `vm purge windows --iso` deletes them. The transient extraction tree lives in the build directory and is deleted on the way out.

    The mid-install reboots do not loop back into Setup because a generation 2 VM keeps its UEFI variable store: Setup's first phase writes a Windows Boot Manager entry and puts it first in the boot order, so every later boot goes to the disk. That persistence is exactly what the QEMU path lacked; deviation 16 exists because Packer's throwaway OVMF store discards what Setup writes. This is the one assumption in this amendment without a measurement behind it. It is observable minutes into the first live build (`Get-VMFirmware` on the build VM after the installer's first reboot), and the contingency if it proves wrong is to detach the install DVD once the output disk crosses the installer-writing threshold `build_watch` already defines, which Hyper-V permits on a running generation 2 VM.

    The Linux-host QEMU path keeps the prompting original ISO and the QMP keypress. How OVMF weighs QEMU's `-boot order=d` against NVRAM entries written mid-install is unmeasured firmware behavior, and this amendment does not touch a path it does not need to.

17. **The build VM is inventory from the moment it exists.** It carries the same per-target name the runtime guest uses (`sunlit-e2e-windows`), so `vm view`, `vm ssh`, `vm down`, and the one-VM-at-a-time rule all apply to a build without new plumbing, and a `RunState` with a new build start reason is written before `Start-VM` runs, so `vm status` shows a build in progress or a leftover from a crash and labels it as a build. The second validation round's rule, no failure may leave a guest running silently, applies to every path inside the build, not only after boot returns: every failure either destroys the build VM or prints exactly what is still running and how to reach it. Tearing down a build VM deletes the VM and its unfinished disk, since a half-built image is worth nothing, and never touches the image directory or the media.

18. **The watcher slims down to the signals Hyper-V has.** The heartbeat keeps `Trend` and swaps signal sources: `Get-VM` state instead of QMP status, the growing VHDX's file size instead of the qcow2's. The resume and give-up arms stay unwired on this path; they exist because WHPX guests wedge unrecoverably, which Hyper-V guests do not, and a build that goes nowhere is bounded by the install deadline (one constant, two hours to match the old `ssh_timeout`, calibrated by the first live run) plus a heartbeat that reports a machine that turned itself off. No keypress, no QMP, no screenshots in the first pass; the WMI thumbnail (`GetVirtualSystemThumbnailImage`) is recorded as the option if a live failure shows screens are worth having, and `vm view windows` reaches a running build VM meanwhile.

19. **The build stays unelevated and needs less than before.** Hyper-V Administrators membership (which `vm setup` grants) covers `New-VHD`, `New-VM`, `Start-VM`, and `Stop-VM`; `Mount-DiskImage` of an ISO and oscdimg need nothing. This path requires Hyper-V with the membership effective, oscdimg, qemu-img (it ships in the QEMU winget package setup already installs), and ssh with ssh-keygen. It does not require packer or qemu-system-x86_64, and it uses no network after the ISO download, so `packer init` and its firewall failure class (deviation 20) are gone from this target. `build-image`'s preflight asks only for what the chosen path uses; the doctor's tool list stays as it is, because the same host still builds the Linux image with Packer and QEMU.

20. **Secure Boot and the TPM are unchanged in this pass.** The unattend keeps the LabConfig bypasses and the runtime VMs keep Secure Boot off, because the QEMU override cell boots the same image without a vTPM, and changing the install's security posture in the same change as the installer would confound the first live run. A generation 2 build VM can supply a real vTPM (`Set-VMKeyProtector`, `Enable-VMTPM`) and Secure Boot with the Microsoft Windows template, which would let the undocumented bypass keys be dropped and retire the "bypass keys drift with ISO revisions" risk. Recorded as a follow-up with its own template-currency consequences, not done here.

## Cleanup

What the WHPX debugging left behind, and what happens to each piece. The rule: a workaround survives only if a reachable path still exercises it.

- **C1. `commands::build_image::qemu_cannot_install_windows` is removed**, with its call site and its test. Its trigger, the Windows target on a Windows host under WHPX, becomes unreachable once the dispatch routes that cell to Hyper-V. The measurements stay recorded in deviation 25 and in the condensed `vm-setup.md` note (C2); nothing in code keeps warning about a path that no longer exists.

- **C2. `docs/vm-setup.md` is rewritten where the WHPX story lives.** The callout "The Windows image cannot currently be built on a Windows host" is replaced by the builder matrix. The WHPX troubleshooting entries collapse into one, scoped to what can still hit the fault (booting the Windows guest through the QEMU provider override on a Windows host, where `-cpu max` remains load-bearing), and that entry gains the twenty-second recheck recipe: the exact throwaway-QEMU-plus-QMP `system_reset` commands, so a future QEMU release can be re-evaluated without an hour-long build and without taking this document's word for it.

- **C3. `CLAUDE.md`:** the bold "cannot be built on a Windows host" paragraph and the build-watch paragraph are rewritten for the new matrix; the command list and testing table stay accurate.

- **C4. `vm/windows/windows11.pkr.hcl`:** comments rescoped to the path that still uses it, a Linux host building the Windows image; the WHPX narrative trims to a pointer at deviation 25. No behavioral change: `-cpu max`, the QMP port, and the empty `boot_command` all stay, because the prompting ISO still needs the keypress under KVM and `-cpu max` is load-bearing wherever WHPX boots this guest.

- **C5. `vm/windows/scripts/finalize.ps1` has a latent path typo, found while preparing this amendment.** The fallback boot manager source reads `\EFI\Microsoft\Bootootmgfw.efi` where `\EFI\Microsoft\Boot\bootmgfw.efi` is meant. Latent rather than shipped, since no Windows image build has ever reached this script; if Setup leaves no fallback loader, the copy would look for a file that cannot exist and fail the build with "no boot manager on the EFI system partition". Fixed.

- **C6. What deliberately stays, each because a live path still needs it:** the QMP boot-key machinery and `build_watch`'s resume and give-up arms (the Linux-host QEMU build of the Windows image), `BUILD_QMP_PORT` and its template-agreement test (same path), `-cpu max` in the template and the runtime QEMU provider (any WHPX boot of this guest), and the OVMF firmware location (QEMU builds and the QEMU runtime provider). Anything only the dead WHPX install path used goes.

## Success criteria

1. On a Windows host, `cargo xtask vm build-image windows` produces the golden VHDX, the derived qcow2, and the manifest, entirely on Hyper-V: no Packer, no QEMU process, no network after the ISO download, no keypress, unattended start to finish, with a heartbeat line a minute.
2. Every failure after the build VM exists either removes it or names it. A leftover build VM shows in `vm status` and `vm down windows` removes it. No failure path touches an existing golden image or the cached media.
3. `cargo xtask vm smoke windows` and `cargo xtask e2e --target windows` pass against the built image, which makes the original plan's criterion 4 reachable on this host for the first time.
4. The Linux image build and the Linux-host build of the Windows image are unchanged: the existing command-line and template tests pass untouched.
5. The cleanup list is executed in full. No document still claims a Windows host cannot build the Windows image, and the WHPX knowledge with its recheck recipe is retained where C2 puts it.
6. New logic is tested in the crate's established style: every generated PowerShell script parses under PowerShell's own parser, the CD file list agrees with the HCL by a test that reads both, the oscdimg, VM-creation, and conversion command lines are pinned, and the build state file's lifecycle is covered including the crashed-build case.

## Implementation steps

- **Step A: media.** The noprompt repack in `store::windows_media` (mount, copy, dismount, oscdimg), cached and inventoried; `vm status` and `vm purge --iso` learn the second file; failures name the tool and the disk budget (the extraction is transient but peaks at roughly the ISO's size again).
- **Step B: the unattend CD.** An oscdimg-built ISO from the shared file list plus the generated `authorized_keys` and marker; the list is one Rust constant; the agreement test reads the HCL.
- **Step C: the build VM.** Creation script (generation 2, 4 vCPUs, 6 GB static memory, Default Switch, both DVDs attached, install DVD first in boot order, Secure Boot off, no checkpoints, no automatic start), `RunState` with a build reason written before `Start-VM`, teardown and status integration, and the no-silent-guest rule on every path.
- **Step D: orchestration.** Start, slim watcher, SSH wait against the install deadline, `finalize.ps1` over SSH, shutdown and wait for Off, remove the VM keeping the disk, move the VHDX into the store, convert to qcow2, write the manifest. The manifest's builder field records the mechanism, since `packer <version>` no longer applies on this path.
- **Step E: dispatch.** `build-image` picks the path from host and target; the preflight checks per path; C1 lands here.
- **Step F: cleanup and docs.** C2 to C5, `docs/roadmap.md`, and this amendment's Status / Results section.
- **Step G: gates and the live run.** `cargo test`, `cargo clippy --all-targets`, `cargo fmt --check`; then, if `vm doctor` says the host is ready, `vm build-image windows`, `vm smoke windows`, and `e2e --target windows`, recording times and pass counts in Status / Results. The first live run is calibration, not regression, per the plan's own rule.

## Risks and mitigations

- **The generation 2 boot-order assumption (decision 16).** Observable minutes into the first live build; the contingency is named there.
- **oscdimg repack subtleties** (UDF version, long names). The flags in decision 16 are the documented ones for Windows media; the first live run validates that the repacked ISO boots before anything downstream is trusted.
- **The install deadline is a guess until a live run calibrates it**, the same stance the plan took for boot timing in step 3.
- **The Hyper-V Administrators membership may not be in the running session's token** (it is logon-time, per the setup design). The doctor already reports this; the build must fail with that pointer rather than a raw access-denied.
- **Two ISOs double the media footprint** to roughly 13 GB; `vm status` shows it and `vm purge windows --iso` reclaims it.

## Rollback

Additive within the xtask: the QEMU build path stays intact behind the dispatch, so reverting is removing the new module and the dispatch arm. The guest files change only by comments and the C5 typo fix.

## Status / Results

To be filled by the implementation: what was built, gate results, live build, smoke, and e2e results with times, and any departures from this amendment recorded in the plan's own deviation style.
