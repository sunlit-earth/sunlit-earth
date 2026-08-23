# The Linux golden image: Debian 13 "trixie" with four desktops installed side
# by side, sddm autologin, and an OpenSSH server. `cargo xtask vm build-image
# linux` drives this; the variables it passes are the ones without a default.
#
# The base is the Debian cloud image rather than the installer ISO, so there is
# no installer to automate: the image boots, cloud-init reads a NoCloud seed
# from an attached CD and creates the test user with our key, and Packer takes
# it from there over SSH. cloud-init is kept to the user account on purpose.
# Everything else runs in provisioner scripts, where a failure names itself in
# Packer's output instead of disappearing into a guest log.
#
# Debian 13 rather than a current Ubuntu (phase 5 decision 1). It is the only
# base where Plasma, GNOME, XFCE and Cinnamon all have a first-class X11 session
# at once: GNOME 50 deleted X11 upstream in March 2026 and Ubuntu 25.10 had
# already dropped the GNOME Xorg session, while trixie froze on GNOME 48 and
# Plasma 6.3, both of which predate every X11 removal. That insulates this image
# for trixie's whole support window (full support to 2028-08, LTS to 2030-06);
# the exposure returns with Debian 14, which is what the roadmap's Wayland guest
# item is about.

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
  default     = "32G"
  description = "The virtual size. A qcow2 only occupies what it holds."
}

# Debian publishes the current trixie cloud image behind a stable `latest` path
# with a SHA512SUMS file beside it, so the checksum follows the image instead of
# pinning a hash that goes stale on every respin.
variable "base_image_url" {
  type    = string
  default = "https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2"
}

variable "base_image_checksum" {
  type    = string
  default = "file:https://cloud.debian.org/images/cloud/trixie/latest/SHA512SUMS"
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

source "qemu" "debian" {
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

  # Packer's compaction pass fails on this host, reproducibly and at the very
  # end: it converts the finished disk to `golden.qcow2.convert` and then renames
  # that over `golden.qcow2`, and on Windows the rename is refused with "Access
  # is denied" because something still holds the original open. Twice in a row,
  # after a build that had otherwise run every provisioner to completion, and
  # Packer deletes its output directory on failure, so there is nothing to retry
  # but the whole hour. What compaction buys is a smaller file; what it costs
  # here is the build. `finalize.sh` already zeroes the free space, which is the
  # half that makes a qcow2 hold only what it holds.
  skip_compaction = true

  qemuargs = [
    ["-m", "${var.memory}"],
    ["-smp", "${var.cpus}"],
  ]
}

build {
  name    = "sunlit-e2e-linux"
  sources = ["source.qemu.debian"]

  provisioner "shell" {
    execute_command = "chmod +x {{ .Path }}; sudo -E env TEST_USER='${var.test_user}' {{ .Vars }} {{ .Path }}"
    scripts = [
      "${path.root}/scripts/desktop.sh",
      "${path.root}/scripts/guest-contract.sh",
      "${path.root}/scripts/finalize.sh",
    ]
  }
}
