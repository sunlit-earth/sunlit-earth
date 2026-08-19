# The Windows golden image: Windows 11 Enterprise evaluation, autologon,
# OpenSSH, and a scheduled task that runs the e2e job in the console session.
# `cargo xtask vm build-image windows` drives this.
#
# One canonical install, built with Packer's QEMU builder and converted to VHDX
# afterwards, so the Hyper-V provider and the QEMU provider boot the same
# Windows (plan decision 10).
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

variable "cpus" {
  type    = string
  default = "4"
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

  # Windows Setup searches removable media for Autounattend.xml, and a virtual
  # floppy is the one medium it has always searched. The same floppy carries
  # everything the first-logon script needs, including the public key.
  floppy_files = [
    "${path.root}/autounattend/Autounattend.xml",
    "${path.root}/scripts/bootstrap.ps1",
    "${path.root}/scripts/run-job.cmd",
    "${path.root}/scripts/session-ready.cmd",
  ]
  floppy_content = {
    "authorized_keys" = var.ssh_public_key
  }

  # "Press any key to boot from CD or DVD" waits about five seconds and then
  # falls through to the empty disk. One keypress is all it needs, and a stray
  # one at the setup screen that follows is harmless because setup is
  # unattended from there on.
  boot_wait    = "2s"
  boot_command = ["<spacebar><wait1><spacebar>"]

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
  ]
}

build {
  name    = "sunlit-e2e-windows"
  sources = ["source.qemu.windows"]

  provisioner "powershell" {
    scripts = ["${path.root}/scripts/finalize.ps1"]
  }
}
