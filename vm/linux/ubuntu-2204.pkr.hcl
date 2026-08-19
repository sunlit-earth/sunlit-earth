# The Linux golden image: Ubuntu 22.04 with GNOME on Xorg, autologin, and an
# OpenSSH server. `cargo xtask vm build-image linux` drives this; the variables
# it passes are the ones without a default.
#
# The base is the Ubuntu cloud image rather than the installer ISO, so there is
# no installer to automate: the image boots, cloud-init reads a NoCloud seed
# from an attached CD and creates the test user with our key, and Packer takes
# it from there over SSH. cloud-init is kept to the user account on purpose.
# Everything else runs in provisioner scripts, where a failure names itself in
# Packer's output instead of disappearing into a guest log.

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

variable "ssh_public_key" {
  type        = string
  description = "The key the guest will trust, generated once by `vm setup`."
}

variable "ssh_private_key_file" {
  type        = string
  description = "Its private half, which Packer authenticates with."
}

variable "accelerator" {
  type        = string
  default     = "kvm"
  description = "kvm on a Linux host, whpx on a Windows one, none as a fallback."
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
  default = "4096"
}

variable "disk_size" {
  type        = string
  default     = "24G"
  description = "The virtual size. A qcow2 only occupies what it holds."
}

# Ubuntu publishes the current 22.04 cloud image behind a stable path and a
# SHA256SUMS file beside it, so the checksum follows the image instead of
# pinning a hash that goes stale on every respin.
variable "base_image_url" {
  type    = string
  default = "https://cloud-images.ubuntu.com/jammy/current/jammy-server-cloudimg-amd64.img"
}

variable "base_image_checksum" {
  type    = string
  default = "file:https://cloud-images.ubuntu.com/jammy/current/SHA256SUMS"
}

variable "test_user" {
  type    = string
  default = "tester"
}

# A throwaway local test VM with no data on it and no route off the host. The
# password exists so that someone at the console via `vm view` can log in when
# autologin is what broke.
variable "test_password" {
  type    = string
  default = "tester"
}

source "qemu" "ubuntu" {
  iso_url          = var.base_image_url
  iso_checksum     = var.base_image_checksum
  disk_image       = true
  use_backing_file = false
  disk_size        = var.disk_size
  format           = "qcow2"
  accelerator      = var.accelerator

  # The NoCloud datasource: cloud-init looks for a filesystem labelled cidata.
  cd_label = "cidata"
  cd_content = {
    "user-data" = templatefile("${path.root}/cloud-init/user-data.pkrtpl.yaml", {
      ssh_public_key = var.ssh_public_key
      test_user      = var.test_user
      test_password  = var.test_password
    })
    "meta-data" = "instance-id: sunlit-e2e-linux\nlocal-hostname: sunlit-e2e-linux\n"
  }

  ssh_username         = var.test_user
  ssh_private_key_file = var.ssh_private_key_file
  ssh_timeout          = "30m"
  shutdown_command     = "sudo shutdown -P now"
  shutdown_timeout     = "10m"

  # Headless with the console on localhost VNC, which is the same arrangement
  # the QEMU provider uses at run time: nothing to look at unless someone
  # attaches, and something to look at the moment a build wedges.
  headless         = true
  vnc_bind_address = "127.0.0.1"

  # Spelled the way `qemu.rs` spells it at run time rather than as the
  # `virtio-net` alias, so that the two can be compared literally: an image
  # installed with one network card and booted with another has no driver for
  # what it finds, and the symptom is a ten-minute SSH timeout.
  net_device     = "virtio-net-pci"
  disk_interface = "virtio"
  machine_type   = "q35"

  output_directory = var.output_dir
  vm_name          = var.vm_name

  qemuargs = [
    ["-m", "${var.memory}"],
    ["-smp", "${var.cpus}"],
  ]
}

build {
  name    = "sunlit-e2e-linux"
  sources = ["source.qemu.ubuntu"]

  provisioner "shell" {
    execute_command = "chmod +x {{ .Path }}; sudo -E env TEST_USER='${var.test_user}' {{ .Vars }} {{ .Path }}"
    scripts = [
      "${path.root}/scripts/desktop.sh",
      "${path.root}/scripts/guest-contract.sh",
      "${path.root}/scripts/finalize.sh",
    ]
  }
}
