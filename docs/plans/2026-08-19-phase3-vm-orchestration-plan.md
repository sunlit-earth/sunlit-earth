# Plan: Phase 3, VM Orchestration for Desktop E2E

## Summary

Build the `cargo xtask` layer that runs the desktop e2e suite in local virtual machines, taking it off the developer desktop. Scope follows retrospective section 10, phase 3: the xtask crate with the provider trait (Hyper-V and QEMU), the golden VM images, and the guest contract; then the e2e suite runs inside the VMs. On top of the retrospective's design, this plan adds the host-preparation tooling settled on 2026-08-19: an unelevated, read-only `vm doctor` command, an elevated `vm setup` command that reports but never performs reboots, and expiry detection for the Windows evaluation image with an explicit override flag. Deliverables include a one-page human guide (`docs/vm-setup.md`). macOS stays on hosted runners only; Linux tray e2e, wallpaper setters, and any CI execution of VMs are out of scope.

## Stakes Classification

Low for the product, medium for the schedule. Everything new is developer tooling: an xtask crate, Packer templates, and documentation. The only changes inside the existing crates are two test-harness adjustments (a binary-path override and per-capability gating of e2e cases), neither of which ships in the binary. The real risks are external dependencies (evaluation ISO availability, package manager contents) and time, not user-facing regressions.

## Research

Settled in `docs/retrospective-2026-08.md` sections 8.3 and 8.4, and in the 2026-08-19 planning discussion:

- Hypervisor matrix: Hyper-V for the Windows guest on a Windows host, QEMU everywhere else (WHPX acceleration on Windows, KVM on Linux). The two providers coexist because WHPX runs on top of the Hyper-V hypervisor; the "Windows Hypervisor Platform" optional feature must be enabled alongside Hyper-V.
- Packer stays for image building. Checked 2026-08-19: no community fork of Packer exists (Terraform got OpenTofu and Vault got OpenBao; Packer got nothing), and the alternatives are Linux-only (KVMage targets KVM/libvirt, virt-builder is libguestfs). Packer is BUSL-licensed since 2023, the same license that counted against Vagrant in section 8.3; the difference is that Packer here is a build-time tool invoked only by `vm build-image`, with templates in the repo and nothing at e2e runtime depending on it, and the BUSL restriction only bites when embedding it in a competing commercial product. The recorded fallback if Packer ever becomes a problem: move image building into the xtask on the same QEMU/QMP provider layer, reimplementing the boot orchestration Packer currently supplies.
- The doctor command must work unelevated. `Get-WindowsOptionalFeature -Online` requires admin; the CIM classes (`Win32_OptionalFeature`, `Win32_ComputerSystem`, `Win32_OperatingSystem`) do not.
- The Windows evaluation clock starts during the image build and never resets, because the golden image is read-only and every run boots a throwaway overlay. A build timestamp in the image manifest is therefore an accurate proxy for eval age, with no VM boot needed to check it.
- An expired evaluation does not refuse to boot; Windows starts shutting down hourly. Undetected, that surfaces as flaky e2e runs rather than a clear error, which is why the doctor and the VM start path both check age up front.
- The e2e harness already has the right shape (spawn binary, IPC commands, `SIGNAL:` lines, log assertions). Its two portability gaps: `crates/sunlit-app/tests/e2e.rs:27` resolves the app binary via `env!("CARGO_BIN_EXE_sunlit-earth")`, a compile-time host path that is wrong inside a guest, and some cases assume Windows capabilities (wallpaper set, tray).

To verify at implementation time rather than trusted from this plan: the current direct-download URL for the Windows 11 Enterprise evaluation ISO, the winget package identifiers for QEMU and Packer, the exact autounattend bypass key names for the ISO revision in use, and the non-interactive initialization path for a fresh WSL distro.

## Key Design Decisions

1. **Command surface.**

   ```
   cargo xtask vm doctor
   cargo xtask vm setup
   cargo xtask vm build-image <windows|linux>
   cargo xtask vm up <windows|linux>
   cargo xtask vm ssh <windows|linux>
   cargo xtask vm view <windows|linux>
   cargo xtask vm status
   cargo xtask vm destroy <windows|linux|all> [--purge]
   cargo xtask e2e --target <host|windows|linux> [--keep] [--allow-expired-image]
   ```

   `--target host` runs the suite on the real desktop exactly as `cargo e2e` does today (the alias stays), so the manual real-GPU run from section 8.4 is the same command as the VM runs and can be automated later. `--keep` leaves the VM running after the tests for inspection; `vm up` boots an interactive guest without running any tests, `vm ssh` connects to a running guest, and `vm view` opens its desktop (decision 14). `vm status` and `vm destroy` are the inventory and cleanup pair (decision 13).

2. **The doctor is unelevated, read-only, and the single verification path.** It prints a per-check PASS/WARN/FAIL report and exits nonzero if anything FAILs. It changes nothing about the system. On Windows it reads through CIM: `Win32_OptionalFeature` for whether Hyper-V and the Windows Hypervisor Platform are enabled, `Win32_ComputerSystem.HypervisorPresent` for whether a hypervisor is actually running, `Win32_OperatingSystem` for the edition (Home has no Hyper-V). The firmware virtualization flag (`Win32_Processor.VirtualizationFirmwareEnabled`) is consulted only when no hypervisor is running, because it can read false while Hyper-V owns VT-x. The full check lists:

   | Windows host | Linux host |
   |---|---|
   | Edition supports Hyper-V | |
   | Hyper-V and WHPX features enabled; `vmms` service present | `/dev/kvm` exists and is writable (kvm group) |
   | Hypervisor present, else firmware virtualization enabled | |
   | Current user in the Hyper-V Administrators group | |
   | `qemu-system-x86_64`, `packer`, `ssh` on PATH | same |
   | WSL distro (Ubuntu 22.04) present with build deps | |
   | Disk space for images and overlays | same |
   | Golden images: present, checksum valid, template current, eval age (Windows image) | same |
   | VNC viewer on PATH (WARN only; needed for `vm view` of QEMU guests) | same |

3. **The setup command is elevated, idempotent, and never reboots.** On Windows it requires an elevated shell and exits with a clear message otherwise; it enables the two features with `-NoRestart`, aggregates the `RestartNeeded` flags, installs QEMU and Packer via winget, adds the current user to the Hyper-V Administrators group (a relogin item; this is what lets `e2e`, `vm status`, and `vm destroy` manage Hyper-V VMs unelevated afterward), and initializes the WSL distro and its build dependencies (running inside the distro as root, which needs no password). On Linux it runs unelevated and shells through `sudo` for the specific commands (apt, `usermod -aG kvm`), so `target/` never becomes root-owned. Either way it ends with the verdict: what needs a reboot or relogin, and "then run `cargo xtask vm doctor` to confirm". It performs neither.

4. **Image store and currency model.** The multi-GB images live outside the repo in a platform data directory, overridable via `SUNLIT_EARTH_VM_DIR`. The repo carries the Packer templates under `vm/`. At build time the xtask writes a manifest next to the images recording the content hash of the template the image was built from, the image checksum, and the build-completion timestamp (UTC). The doctor can then distinguish four states: missing, corrupt (checksum mismatch), stale (the repo template changed since the image was built), and expired (Windows image only, see next decision). A repo-pinned image checksum is impossible because every user builds their own image and Windows installs are not reproducible; template-hash currency is the honest version of the retrospective's "checksum manifest" sentence.

5. **Eval expiry is detected, explained, and overridable, never a hard refusal.** The Windows 11 Enterprise evaluation runs 90 days from install. The doctor warns from day 75 and reports expired from day 90, computed from the manifest timestamp. When `e2e --target windows` (or any VM start) sees an expired image, it prints a help text explaining what expired means (the guest shuts itself down hourly, so runs get flaky rather than failing cleanly), that the fix is `cargo xtask vm build-image windows`, and that `--allow-expired-image` boots anyway; then it exits. With the flag, it proceeds. The deep check (`slmgr /xpr` over SSH) is not the primary mechanism; it is emitted as a diagnostic when a Windows VM run fails.

6. **One guest contract regardless of provider.** A fixed test user with autologon (the plaintext password in the unattend file is acceptable for a throwaway local test VM), an OpenSSH server, and a results-directory protocol: the job writes `output.log`, `artifacts/`, and finally `exit_code.txt`; the orchestrator polls for `exit_code.txt` with a timeout. On Windows, processes started over SSH cannot touch the interactive desktop, so the golden image pre-registers a scheduled task ("run only when user is logged on") that executes the job in the console session; the image also writes a ready-marker file at logon so the orchestrator knows when the desktop exists. On Linux, the image autologs into GNOME on Xorg (`WaylandEnable=false` in gdm3, so `DISPLAY=:0` reaches the session and input tooling works), with animations disabled and mesa's Vulkan drivers installed. Tests inside guests run the app with `--software-rendering`; no guest has a real GPU.

7. **Provider trait, two implementations, one VM at a time.** `create_from_golden`, `start`, `wait_ssh`, `exec`, `copy_in`, `collect_results`, `destroy`. QEMU: process spawn plus QMP over a local socket, user-mode networking with an SSH hostfwd on a fixed localhost port, WHPX on Windows hosts and KVM on Linux hosts; every QEMU guest also starts with `-display none -vnc 127.0.0.1:0`, so the console framebuffer is attachable at any moment and costs nothing when nobody connects (localhost-only, no password, a throwaway local test VM). Hyper-V: PowerShell cmdlets, a differencing VHDX child, the Default Switch, guest IP via `Get-VMNetworkAdapter`. Clean state comes from throwaway overlays against read-only golden images, exactly as section 8.3 specifies. No concurrency in this phase; the fixed port and the single-VM rule keep the orchestrator simple.

8. **Build on the host, copy artifacts in.** `cargo test --no-run --message-format=json` yields the test executable paths. The Windows host builds Windows binaries natively and Linux binaries via WSL; the guest and the WSL distro are pinned to the same Ubuntu 22.04 base so glibc matches. The harness gains a `SUNLIT_EARTH_BIN` environment override with the compile-time `CARGO_BIN_EXE` value as fallback, the one change the retrospective already named.

9. **The xtask crate stays thin and shells out.** It lives at the repo root (`xtask/`, added to workspace members, with the `xtask = "run -p xtask --"` alias in `.cargo/config.toml`), inherits the workspace lints, and contains no unsafe code. It links almost nothing: `clap` and `serde_json` are already in the dependency tree (QMP is JSON over a socket, cargo messages are JSON lines), plus a hashing crate for checksums. SSH and SCP are the OpenSSH client binaries Windows ships; Hyper-V is driven through `powershell.exe`; Packer and QEMU are processes. The pure logic (manifest parsing, doctor evaluation, expiry math, cargo-message parsing) is unit-tested without any VM.

10. **Windows 11 Enterprise evaluation, not Windows Server.** Server's 180-day eval and TPM-free install are tempting, but the product ships on client Windows and the e2e suite exercises exactly the places where Server differs (tray, wallpaper, shell behavior). The autounattend file carries the TPM/Secure Boot bypass registry keys, which is the standard route for QEMU guests without a vTPM and avoids a swtpm dependency. One canonical image is built with Packer's QEMU builder and converted between qcow2 and VHDX with `qemu-img convert`, so both providers boot the same install.

11. **E2e portability by capability, not by cfg.** Phase 2's deviation 13 established that compiling the suite everywhere is free coverage, so the wallpaper and tray cases are gated at runtime the way `software_adapter_produces_correct_results` is: they assert the capability is present on Windows (where its absence would be a regression) and skip only where absence is expected. The Linux guest runs the windowed-mode subset: launch, single-instance, IPC lifecycle (show, hide, quit), export, and the hidden-window memory-growth regression test. Linux tray e2e waits for the StatusNotifier decision that the retrospective explicitly defers until after this phase.

12. **CI stays untouched.** Hosted runners cannot nest these VMs, and rewriting a green three-OS `ci.yml` to route through xtask tiers buys uniformity at the cost of churn right after phase 2 stabilized it. The retrospective's "workflows call the same xtask commands" idea is deferred until a workflow actually needs orchestration knowledge. This phase defines only the desktop-e2e tier (`cargo xtask e2e`); unit and engine tiers keep their existing cargo aliases.

13. **Inventory and cleanup: `vm status` and `vm destroy`.** Everything the xtask creates is recognizable as its own: VMs carry a `sunlit-e2e-` name prefix, overlays live in a dedicated directory inside the image store, and each running or kept VM has a small state file next to its overlay (VM name or QEMU pid, SSH port), so status and destroy find what a crashed orchestrator left behind without scanning the host, and neither command ever touches a VM or file that is not the xtask's. `vm status` is unelevated and read-only like the doctor: per target it lists the golden image (path, size, build date, template currency, eval age for the Windows image), the cached ISO, overlays including orphans from crashed runs, `--keep` runs, or `vm up`, and any running or registered VM with how to reach it (`vm ssh`, `vm view`) and the `vm destroy` hint next to it, plus per-target and total disk footprint. `vm destroy <windows|linux|all>` tears down run state only: it stops and unregisters the Hyper-V VM or terminates the QEMU process, then deletes overlays and state files; cheap, and never costs an image rebuild. `vm destroy <target> --purge` additionally deletes the golden image, the converted VHDX, the cached ISO, and the manifest entry: this is the disk-space recovery path, it prints what it deleted and how much space was freed, and `vm status` prints the matching purge command next to the footprint numbers so reclaiming space is one copy-paste. Rebuilding after a purge is `vm build-image <target>`. Reverting `vm setup` itself (uninstalling QEMU or Packer, disabling the Windows features, removing the group membership) is explicitly out of scope.

14. **Interactive desktop access via `vm view`, at the hypervisor level.** Both hypervisors expose the guest's console without any software, configuration, or firewall rule inside the guest, and both attach to the same console session the e2e jobs run in. For a Hyper-V guest, `vm view` launches `vmconnect.exe localhost <vm-name>`; the guide instructs using its basic session, because enhanced session mode is RDP underneath and logs into a separate session, locking the console session out from under a running job (plain `mstsc` RDP is avoided for the same reason). For a QEMU guest, `vm view` connects a VNC viewer to the always-on localhost VNC from decision 7, or prints the address if no viewer is on PATH. `vm view` attaches to a running VM only; if none is running it says so and points at `vm up`. `vm up <target>` is the direct route to an interactive guest: it boots a fresh overlay and copies the current binaries in exactly as a test run would, then prints the connect and destroy hints; `e2e --target <t> --keep` remains the way to inspect the aftermath of an actual test run. The name pairs with `vm destroy` the way `vagrant up` pairs with `vagrant destroy`, and it is deliberately not `start`, because there is no stop: the guests hold no state worth preserving, so ending one and discarding it are the same act. `vm up`'s closing output says exactly that (destroy frees the memory and the overlay, the golden image is untouched, the next boot is pristine, there is no stop or pause), and `vm status` repeats the destroy hint next to any running VM, so the explanation lives where the question arises. Nothing ever runs in the background unasked: a VM exists only during a run, after `--keep`, or after `vm up`, and holds its RAM allocation until destroyed. Two caveats belong in the guide: watching a test run is harmless but injecting input during one perturbs the tests, and the hypervisor console has essentially no clipboard integration (the tradeoff for not being RDP), so text and files go in via `vm ssh` and scp.

## Success Criteria

1. `cargo xtask vm doctor` runs unelevated on both host OSes, changes nothing, and reports every check from decision 2 accurately (verified against at least: a host missing a feature, a host with everything, a missing image, a stale image).
2. `cargo xtask vm setup` takes a fresh supported host to doctor-green, modulo the reboot or relogin it reports at the end but does not perform. Running it twice is a no-op the second time.
3. `cargo xtask vm build-image linux` and `build-image windows` produce golden images unattended from the templates in `vm/`, writing the manifest (template hash, checksum, timestamp). The Windows build may pause once for a manual ISO download if the automated fetch fails, and says exactly where to put the file.
4. `cargo xtask e2e --target windows` runs the full 8-test desktop e2e suite, including the wallpaper set, green inside the Windows VM from a single command: pristine overlay, boot, copy in, run in the console session, results pulled back, VM destroyed. `--keep` keeps it and `vm ssh` reaches it.
5. `cargo xtask e2e --target linux` runs the windowed-mode subset green inside the GNOME guest.
6. With the Windows image's manifest timestamp pushed past 90 days, the doctor reports expired, a plain `e2e --target windows` prints the help text and exits, and `--allow-expired-image` proceeds to boot.
7. `cargo e2e` and `cargo xtask e2e --target host` still pass on the development desktop, unchanged in behavior.
8. `docs/vm-setup.md` exists: one page, the four-step flow (setup, reboot, build-image, e2e), plus the expiry, interactive-access, disk-usage-and-cleanup, and troubleshooting notes, and it matches what the commands actually do.
9. `cargo xtask vm view` opens the console desktop of a running guest on both providers (vmconnect for Hyper-V, VNC for QEMU). Both interactive routes work: `vm up` then `vm view` reaches a desktop with the current binaries inside without running any tests, and `e2e --keep` then `vm view` reaches the aftermath of a test run. `vm up`'s output ends with the lifecycle explainer from decision 14.
10. `cargo xtask vm status` reports images, ISO cache, overlays, running VMs, and disk footprint accurately in at least three states: nothing built, images built, and a `--keep` VM left running. `vm destroy` removes the run state without touching images; `vm destroy --purge` frees the image-store space, reports how much it freed, and the next `vm status` and `vm doctor` report the images as missing rather than anything worse.

## Implementation Steps

### Step 1: xtask scaffold, doctor, setup

Create `xtask/` with the alias, the `vm/` directory skeleton, and the manifest format. Implement `vm doctor` and `vm setup` for both host OSes per decisions 2 and 3, including the expiry evaluation (against a manifest that no image writes yet). Also the file-backed halves of `vm status` and `vm destroy --purge` (decision 13): the image, ISO, and manifest inventory with sizes, and their deletion; the run-state halves land with each provider. Unit-test the evaluation and inventory logic with fabricated CIM/file inputs.

### Step 2: Linux golden image

Packer template: Ubuntu 22.04 cloud image, cloud-init installs the desktop, GNOME on Xorg with autologin and animations off, OpenSSH, mesa Vulkan drivers, the test user, the results directory. `vm build-image linux` drives it and writes the manifest.

### Step 3: QEMU provider and the guest-contract smoke

Implement the provider trait for QEMU (spawn, QMP, hostfwd, overlay lifecycle, the state file, the always-on localhost VNC) plus `vm ssh` and the QEMU halves of `vm up`, `vm view`, `vm status`, and `vm destroy` (liveness from the state file, process teardown, overlay cleanup). Prove the contract end to end with a trivial job: boot an overlay of the Linux image, run a command, write results, pull them back, destroy. This step, not the e2e step, is where boot timing and polling get calibrated.

### Step 4: E2e harness portability

Add the `SUNLIT_EARTH_BIN` override at `e2e.rs:27` with the compile-time fallback. Port the environment assumptions (paths off `%LOCALAPPDATA%`) and gate the Windows-only cases per decision 11. Re-run `cargo e2e` on the dev desktop to confirm nothing regressed (criterion 7).

### Step 5: `e2e --target linux` end to end

Build the Linux test binaries via WSL, copy them and the app binary in, run the subset against the autologin session, collect results. Optional validation of the Linux-host provider path: QEMU with KVM inside WSL2 (nested virtualization), which exercises the Linux side of the provider matrix without separate hardware.

### Step 6: Windows golden image

Packer QEMU-builder template: evaluation ISO, autounattend with the bypass keys, virtio drivers, autologon, OpenSSH, the scheduled task and ready marker, the results directory. `qemu-img convert` produces the VHDX. The manifest timestamp from this build is what the expiry check consumes. The ISO fetch tries the known URL and falls back to printed instructions.

### Step 7: Hyper-V provider and `e2e --target windows`

Implement the Hyper-V provider (differencing VHDX, Default Switch, `Get-VMNetworkAdapter` for the address), the scheduled-task job dispatch, the Hyper-V halves of `vm up`, `vm view`, `vm status`, and `vm destroy` (vmconnect, `Get-VM` on the name prefix, stop and unregister plus VHDX cleanup), and the boot-time expiry gate with `--allow-expired-image`. The full suite, wallpaper set included, runs in the VM: the phase's headline deliverable.

### Step 8: Docs

Write `docs/vm-setup.md` (the one-page guide), including the interactive-testing section from decision 14 (the `vm up` and `e2e --keep` workflows, the no-stop lifecycle, the basic-session note for vmconnect, and the input and clipboard caveats) and the disk-usage-and-cleanup section from decision 13 (`vm status` to see what occupies space, `vm destroy` versus `--purge`, and what a rebuild costs). Update CLAUDE.md (xtask commands, the new directory, the testing table's e2e row), `docs/roadmap.md`, and this plan's Results section with the measured image build times, warm VM-run times, and pass counts per guest.

## Risks and Mitigations

- **The evaluation ISO download rots.** Microsoft's page sits behind a form; direct URLs change. Mitigation: the fetch is best-effort with a clear manual fallback (criterion 3), and the URL lives in one place in the template.
- **Autounattend bypass keys drift with ISO revisions.** Mitigation: the template pins the ISO version it was written against; a new ISO is a deliberate template change, which the currency model then surfaces as "stale" on old images.
- **Scheduled-task and autologon races.** A job fired before the desktop exists fails confusingly. Mitigation: the ready marker from decision 6, polling with generous timeouts, and `--keep` plus `vm ssh` for post-mortems.
- **Guest performance.** QEMU under WHPX is slower than KVM, and GNOME composites on llvmpipe. Mitigation: animations off, Xorg session, software rendering forced, e2e timeouts calibrated in step 3 rather than assumed from desktop runs. First VM runs are treated as calibration, not regressions.
- **glibc mismatch between WSL-built binaries and the guest.** Mitigation: both pinned to Ubuntu 22.04; the doctor checks the distro version.
- **Eval expiry as silent flakiness.** Mitigation is the whole of decision 5; the failure diagnostic additionally runs `slmgr /xpr` so an expired guest names itself.
- **xtask scope creep.** An orchestrator can absorb unlimited complexity. Mitigation: the shell-out rule, the seven-method trait, one VM at a time, and CI explicitly out of scope (decision 12).
- **Disk and time costs.** 40 to 60 GB and a 45-to-90-minute Windows image build. Mitigation: the doctor checks free space before anything runs, the build is one-time per quarter (expiry) or per template change, and `vm status` plus `vm destroy --purge` make what the space is spent on visible and reclaimable with one command each (decision 13).

## Rollback Strategy

Everything is additive: the `xtask/` crate, the `vm/` directory, and the docs can be deleted without touching the product. The two harness changes (binary override, capability gating) are small standalone commits, revertible independently; neither alters what ships. No CI workflow, config format, or persisted data changes in this phase.

## Status

Planned 2026-08-19. Not started. Results (image build times, VM run times, per-guest pass counts) to be recorded here during implementation.
