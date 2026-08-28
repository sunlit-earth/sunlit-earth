#!/usr/bin/env bash
# What makes this image a builder: the workspace's build dependencies and a
# pinned Rust toolchain, and nothing else.
#
# The package list is the Ubuntu list from CLAUDE.md minus everything graphical.
# `mesa-vulkan-drivers` and `xvfb` are deliberately absent: no test runs here, so
# there is nothing for a software adapter to render, and a builder with a GPU
# stack in it is a builder that can accidentally be asked to run a test.
# `binutils` is there for `readelf` and `objdump`, which is how the build job
# reads its own output back and proves the glibc floor. There is no `git`: the
# source arrives as a tar the host made, the lockfile names no git dependency,
# and cargo's registry protocol needs none.
#
# `RUST_CHANNEL` comes from the Packer template, which the xtask fills in from
# `rust-toolchain.toml`. A release build installs the pinned channel by name
# again in its own job, so this is a warm cache rather than the contract.
set -euxo pipefail

export DEBIAN_FRONTEND=noninteractive

# Unattended upgrades and snapd both wake up on their own schedule and hold the
# apt lock while they do, which in a throwaway guest is a build that fails on a
# lock nobody asked for. The desktop image disables its apt timers for the same
# reason; here snapd goes entirely, because nothing in a Rust build is a snap.
systemctl disable --now apt-daily.timer apt-daily-upgrade.timer \
  unattended-upgrades.service 2>/dev/null || true
apt-get purge -y snapd unattended-upgrades || true
apt-get autoremove -y --purge || true

apt-get update
apt-get install -y --no-install-recommends \
  build-essential \
  pkg-config \
  clang \
  libclang-dev \
  libfontconfig-dev \
  libxcb-shape0-dev \
  libxcb-xfixes0-dev \
  libxkbcommon-dev \
  binutils \
  curl \
  ca-certificates

# rustup as the account the build runs as, not as root: the job names
# `$HOME/.cargo/bin/cargo` by absolute path, and root's home is not that account's.
# `--profile minimal` for the same reason the pin says so: rustfmt and clippy are
# the gates' business and a release build needs neither.
su - "${TEST_USER}" -c "set -eux
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    -o /tmp/rustup-init.sh
  sh /tmp/rustup-init.sh -y --default-toolchain '${RUST_CHANNEL}' --profile minimal
  rm -f /tmp/rustup-init.sh
  \"\${HOME}/.cargo/bin/rustc\" -vV
  \"\${HOME}/.cargo/bin/cargo\" -V"
