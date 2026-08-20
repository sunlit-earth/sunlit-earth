# The Windows golden image: Windows 11 Enterprise evaluation, autologon,
# OpenSSH, and a scheduled task that runs the e2e job in the console session.
#
# This is the Linux-host half of the builder matrix. `cargo xtask vm build-image
# windows` drives it there, on KVM; on a Windows host the same command installs
# Windows natively on Hyper-V instead, because QEMU there runs on WHPX and a WHPX
# guest does not survive the reset Windows Setup performs after copying its files
# (deviation 25 of the phase 3 plan, and the amendment
# docs/plans/2026-08-20-phase3-amendment-hyperv-windows-build.md). Nothing in
# this file changed with that: it is the path KVM still takes.
#
# One canonical install either way, converted between qcow2 and VHDX so the
# Hyper-V provider and the QEMU provider boot the same Windows (plan decision
# 10). Which format is derived depends on where the install happened.
#
# The devices are the ones a stock Windows install has in-box drivers for: an
# IDE disk and an e1000 NIC. That is slower than virtio and it is deliberate.
# virtio would mean shipping the virtio-win driver ISO and injecting storage
# drivers during setup, and a driver injection that silently does not take
# leaves the installer sitting at "no drives were found" with no way to ask it
# why. The cost is paid on a Linux host running the Windows guest, which is the
# one cell of the provider matrix this does not otherwise touch.

packer {
  required_plugins {
    qemu = {
      version = ">= 1.1.0"
      source  = "github.com/hashicorp/qemu"
    }
  }
}

variable "output_dir" {
  type        = string
  description = "Packer's output directory. Must not exist when the build starts."
}

variable "iso_path" {
  type        = string
  description = "The evaluation ISO, downloaded or placed by the xtask."
}

variable "iso_checksum" {
  type    = string
  default = "none"

  # Microsoft publishes the hashes as a PDF linked from the Evaluation Center
  # rather than as a checksum file, so there is nothing to point Packer at. The
  # xtask checks the download is plausibly an ISO rather than an error page; a
  # deliberate hash can be passed here when one is at hand.
  description = "SHA256 of the ISO, or \"none\"."
}

variable "ssh_public_key" {
  type        = string
  description = "The key the guest will trust, generated once by `vm setup`."
}

variable "ssh_private_key_file" {
  type        = string
  description = "Its private half, which Packer authenticates with."
}

variable "efi_firmware_code" {
  type        = string
  description = "OVMF code half. Located by the xtask, which knows where QEMU put it."
}

variable "efi_firmware_vars" {
  type        = string
  description = "OVMF variables half."
}

variable "accelerator" {
  type    = string
  default = "whpx"
}

variable "vm_name" {
  type    = string
  default = "golden.qcow2"
}

# Two is the floor, not a choice: Windows 11 Setup refuses to install on a
# single-core processor ("The processor needs to have two or more cores"), and
# the LabConfig BypassCPUCheck key below does not cover that check. It is worth
# writing down because one vCPU is the workaround for the WHPX reset fault, which
# is why that workaround was never available and why a Windows host installs this
# image on Hyper-V instead. Deviation 25 has the measurements.
variable "cpus" {
  type    = string
  default = "4"
}

# The QMP port the xtask presses the boot key on. Its own, not the 4444 a
# running guest uses, so a build and a guest cannot collide. The default and the
# xtask's constant are kept in step by a test.
variable "qmp_port" {
  type    = string
  default = "4445"
}

variable "memory" {
  type    = string
  default = "6144"
}

variable "disk_size" {
  type    = string
  default = "64G"
}

variable "test_user" {
  type    = string
  default = "tester"
}

# In the unattend file this password is plaintext, which plan decision 6
# accepts: the guest is a throwaway local VM reachable only from this host's
# loopback, it holds nothing, and autologon needs it.
variable "test_password" {
  type    = string
  default = "tester"
}

# The edition inside the evaluation media. `dism /Get-WimInfo` against the
# ISO's install.wim lists what is actually there if a future revision renames
# it.
variable "image_name" {
  type    = string
  default = "Windows 11 Enterprise Evaluation"
}

source "qemu" "windows" {
  iso_url      = var.iso_path
  iso_checksum = var.iso_checksum

  disk_size      = var.disk_size
  format         = "qcow2"
  accelerator    = var.accelerator
  disk_interface = "ide"
  net_device     = "e1000"
  machine_type   = "q35"

  # Windows 11 requires UEFI. The TPM and Secure Boot requirements are what the
  # LabConfig keys in the unattend file bypass; the firmware itself is not
  # optional.
  efi_boot          = true
  efi_firmware_code = var.efi_firmware_code
  efi_firmware_vars = var.efi_firmware_vars

  # Windows Setup searches the root of every removable drive for
  # Autounattend.xml, CD-ROMs included, so a secondary CD carries it along with
  # everything the first-logon script needs.
  #
  # Not a floppy: q35 has no floppy controller, and Packer's -fda would make
  # QEMU exit at startup rather than boot. The marker file is how the bootstrap
  # command finds this CD without knowing which letter it was given.
  cd_files = [
    "${path.root}/autounattend/Autounattend.xml",
    "${path.root}/scripts/bootstrap.ps1",
    "${path.root}/scripts/run-job.cmd",
    "${path.root}/scripts/session-ready.cmd",
  ]
  cd_content = {
    "authorized_keys"       = var.ssh_public_key
    "sunlit-e2e-media.marker" = "sunlit-e2e"
  }

  # Empty on purpose. "Press any key to boot from CD or DVD" still has to be
  # answered, and the xtask does it over QMP while this build runs, because
  # Packer's own keystrokes do not arrive. It presses until the disk shows the
  # installer writing, and then stops: a spacebar arriving after setup is up
  # presses whatever has focus, and on the "Installing Windows" screen that is
  # Cancel.
  #
  # Measured on 2026-08-20: Packer logs each `<spacebar>` it sends over VNC,
  # QEMU's log shows nothing wrong, and the prompt times out anyway. The same
  # key, on the same schedule, from a small VNC client of our own reached the
  # guest every time, and so does `send-key` over QMP, which injects at the
  # input device and skips the question entirely. `-qmp` below is what the
  # xtask connects to.
  boot_wait    = "1s"
  boot_command = []

  # SSH does not exist until bootstrap.ps1 installs it at first logon, which is
  # on the far side of the whole Windows install.
  communicator         = "ssh"
  ssh_username         = var.test_user
  ssh_private_key_file = var.ssh_private_key_file
  ssh_timeout          = "2h"

  shutdown_command = "shutdown /s /t 5 /f /d p:4:1"
  shutdown_timeout = "30m"

  headless         = true
  vnc_bind_address = "127.0.0.1"

  output_directory = var.output_dir
  vm_name          = var.vm_name

  qemuargs = [
    ["-m", "${var.memory}"],
    ["-smp", "${var.cpus}"],
    ["-rtc", "base=utc"],

    # Not optional, and not a performance choice. With QEMU's default guest CPU,
    # `qemu64`, WHPX dies the moment the Windows boot manager runs: `WHPX:
    # Unexpected VP exit code 4`, after which the vCPU is gone and the screen
    # stays on the firmware logo. `-cpu max` boots the same media to an
    # unattended install on the same host, measured on an AMD Ryzen 7 5800X on
    # 2026-08-20 and reproduced four times.
    #
    # It stays even though this template is now a Linux host's, because the same
    # guest is booted through the QEMU provider override on a Windows host, where
    # WHPX is what runs it. It does not make WHPX survive the installer's reboot:
    # that is a separate fault with the same exit code and no CPU model avoids
    # it, which is deviation 25 and why a Windows host does not use this path.
    ["-cpu", "max"],

    # The firmware has to try the CD first, or the "Press any key" prompt never
    # comes up and no keypress can help. `order=` rather than `once=`: the
    # install restarts several times, and each of those restarts then waits out
    # the prompt before reaching the disk, which costs about five seconds and
    # is understood, where `once=` depends on how QEMU rewrites the boot order
    # across a guest-initiated reset.
    ["-boot", "order=d"],

    # The monitor the xtask presses the boot key on. Loopback only, and it
    # answers as soon as QEMU is up rather than making QEMU wait for a client.
    ["-qmp", "tcp:127.0.0.1:${var.qmp_port},server=on,wait=off"],
  ]
}

build {
  name    = "sunlit-e2e-windows"
  sources = ["source.qemu.windows"]

  provisioner "powershell" {
    scripts = ["${path.root}/scripts/finalize.ps1"]
  }
}
