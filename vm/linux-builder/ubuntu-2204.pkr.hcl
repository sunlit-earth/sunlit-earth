# The Linux release builder: Ubuntu 22.04 with a Rust toolchain and no graphics
# stack at all. `cargo xtask vm build-image linux-builder` drives this; the
# variables it passes are the ones without a default.
#
# Ubuntu 22.04 and not the Debian 13 the desktop guest runs, because the floor a
# binary carries is the glibc of the machine that linked it. Debian 13 is glibc
# 2.41, and a binary linked there refuses to start on anything older, Ubuntu
# 24.04 LTS included. 22.04 is glibc 2.35, which is the oldest still-supported
# userland where clang 14 builds this workspace through bindgen, and it is the
# same userland the WSL build the e2e suite uses already is. That gives a
# release binary a floor of 2.35: Ubuntu 22.04 and everything since.
#
# It has a shelf life. Standard support for 22.04 ends in April 2027, and the
# successor is 24.04 at glibc 2.39, which is this file's `base_image_url` and a
# line in the roadmap.
#
# The base is the cloud image rather than the installer ISO, the same way the
# desktop template works: the image boots, cloud-init reads a NoCloud seed from
# an attached CD and creates the build account with our key, and Packer takes it
# from there over SSH. Everything else runs in provisioner scripts, where a
# failure names itself in Packer's output instead of disappearing into a guest
# log.
#
# What this image deliberately does not have is a desktop, an X server, Mesa, or
# a display manager. A builder is not something the e2e suite can run in, and
# that is the point: a compiler in the images the suite runs in would cost the
# fidelity that found the missing Visual C++ runtime, because the guest that
# found it was a stock Windows.

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

variable "rust_channel" {
  type        = string
  default     = "stable"
  description = <<-EOT
    The toolchain to install, which the xtask reads out of `rust-toolchain.toml`
    and passes in. The default is only what a bare `packer build` would use: a
    release build installs the pinned channel by name in its own job anyway, so
    an image built without this variable costs a download rather than a wrong
    compiler.
  EOT
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

# Larger than the desktop image's 32G, because a release build's `target/`
# directory with fat LTO is the biggest thing that ever lands in one of these
# guests, and it lands in a throwaway overlay of this disk. A qcow2 only
# occupies what it holds, so the virtual size costs nothing until it is used.
variable "disk_size" {
  type    = string
  default = "48G"
}

# Ubuntu publishes the current 22.04 point release behind a stable `current`
# path with one SHA256SUMS file covering everything under it, so the checksum
# follows the image instead of pinning a hash that goes stale on every respin.
variable "base_image_url" {
  type    = string
  default = "https://cloud-images.ubuntu.com/releases/22.04/release/ubuntu-22.04-server-cloudimg-amd64.img"
}

variable "base_image_checksum" {
  type    = string
  default = "file:https://cloud-images.ubuntu.com/releases/22.04/release/SHA256SUMS"
}

variable "test_user" {
  type    = string
  default = "tester"
}

# A throwaway local build VM with no data on it and no route off the host. The
# password exists so that someone at the console via `vm view` can log in when
# the network is what broke.
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
    "meta-data" = "instance-id: sunlit-e2e-linux-builder\nlocal-hostname: sunlit-e2e-linux-builder\n"
  }

  ssh_username         = var.test_user
  ssh_private_key_file = var.ssh_private_key_file
  ssh_timeout          = "30m"
  shutdown_command     = "sudo shutdown -P now"
  shutdown_timeout     = "10m"

  headless         = true
  vnc_bind_address = "127.0.0.1"

  # Spelled the way `qemu.rs` spells it at run time rather than as the
  # `virtio-net` alias, so that the two can be compared literally:
  # `the_runtime_devices_match_the_templates` reads both sides. An image
  # installed with one network card and booted with another has no driver for
  # what it finds, and the symptom is a ten-minute SSH timeout.
  net_device     = "virtio-net-pci"
  disk_interface = "virtio"
  machine_type   = "q35"

  # Discards from the guest reach the qcow2 and free the clusters behind them,
  # which is what makes `finalize.sh`'s `fstrim` the whole of what compaction
  # would do. apt's caches are most of it.
  disk_discard = "unmap"

  output_directory = var.output_dir
  vm_name          = var.vm_name

  # For the reason the Debian template gives: Packer's compaction pass renames
  # its converted copy over the original, and on a Windows host that rename is
  # refused while anything still holds the file. `fstrim` inside the guest
  # reaches the same end state without a second copy of the disk.
  skip_compaction = true

  qemuargs = [
    ["-m", "${var.memory}"],
    ["-smp", "${var.cpus}"],
    # For the reason the other two templates carry it: QEMU's default `qemu64`
    # is what makes WHPX abort a guest, and it costs nothing under KVM.
    ["-cpu", "max"],
  ]
}

build {
  name    = "sunlit-e2e-linux-builder"
  sources = ["source.qemu.ubuntu"]

  provisioner "shell" {
    execute_command = "chmod +x {{ .Path }}; sudo -E env TEST_USER='${var.test_user}' RUST_CHANNEL='${var.rust_channel}' {{ .Vars }} {{ .Path }}"
    scripts = [
      "${path.root}/scripts/toolchain.sh",
      "${path.root}/scripts/guest-contract.sh",
      "${path.root}/scripts/finalize.sh",
    ]
  }
}
