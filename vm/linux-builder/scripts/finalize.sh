#!/usr/bin/env bash
# Last pass over the builder image: prove the toolchain is there, stop cloud-init
# from looking for a datasource that will not be, and shrink what the qcow2 has
# to carry.
#
# The verification is the point of this running as a provisioner rather than as a
# comment. A builder without a compiler in it is an image that boots, answers
# SSH, accepts a source archive, and fails twenty seconds into its job with
# "cargo: not found"; a build that fails here says so while there is still a
# Packer log to read it in.
set -euxo pipefail

home=$(getent passwd "${TEST_USER}" | cut -d: -f6)
cargo="${home}/.cargo/bin/cargo"
rustc="${home}/.cargo/bin/rustc"

for tool in "${cargo}" "${rustc}"; do
  test -x "${tool}" || {
    echo "no ${tool} in the image; the toolchain install did not take" >&2
    exit 1
  }
done

# By name, not just the default: the build job installs the pinned channel and
# then calls it by name, so a toolchain that answers only as `default` would fail
# there instead of here.
su - "${TEST_USER}" -c "set -eux
  '${rustc}' '+${RUST_CHANNEL}' -vV
  '${cargo}' '+${RUST_CHANNEL}' -V"

# What bindgen needs, which is the one build dependency of this workspace that is
# not a compiler or a header: libclang, found through the same mechanism
# `astronomy-engine-bindings` uses.
test -n "$(ls /usr/lib/llvm-*/lib/libclang.so* 2>/dev/null || true)" || {
  echo "no libclang in the image; bindgen cannot run here" >&2
  exit 1
}
# And what the build job reads its own output back with, which is how the glibc
# floor and the NEEDED set reach the host as facts rather than as claims.
for tool in readelf objdump; do
  command -v "${tool}" >/dev/null || {
    echo "no ${tool} in the image; the build could not report its own linkage" >&2
    exit 1
  }
done

# Recorded in the image so a guest can be asked what it is without a toolchain
# query: the floor a binary linked here carries is this libc's version.
ldd --version | head -1 > /var/lib/sunlit-e2e/glibc.txt
chmod 0644 /var/lib/sunlit-e2e/glibc.txt

# Every run boots a throwaway overlay of this image with no seed CD attached.
# Left enabled, cloud-init would spend its datasource timeout looking for one on
# every single boot.
touch /etc/cloud/cloud-init.disabled

# The marker is written at boot by the oneshot unit. Shipping one inside the
# image would tell the orchestrator the guest was ready before it had booted.
rm -f /var/lib/sunlit-e2e/ready /var/lib/sunlit-e2e/job.sh
rm -rf /var/lib/sunlit-e2e/results
install -d -o "${TEST_USER}" -g "${TEST_USER}" -m 0755 \
  /var/lib/sunlit-e2e/results /var/lib/sunlit-e2e/results/artifacts

apt-get clean
rm -rf /var/lib/apt/lists/*
rm -f /etc/ssh/ssh_host_*
# Regenerated on first boot, so the image does not ship one identity that every
# overlay then shares.
cat > /etc/systemd/system/sunlit-e2e-sshkeys.service <<'EOF'
[Unit]
Description=Generate SSH host keys on first boot
Before=ssh.service ssh.socket
ConditionPathExistsGlob=!/etc/ssh/ssh_host_*_key

[Service]
Type=oneshot
ExecStart=/usr/bin/ssh-keygen -A
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
EOF
systemctl enable sunlit-e2e-sshkeys.service

# Hand the free space back to the qcow2 rather than zeroing it, for the reason
# the desktop image's finalize gives: the disk is attached with `discard=unmap`,
# so a trim punches holes in the image file, and this build skips Packer's
# convert pass, so a zero-fill would allocate every free cluster instead.
sync
fstrim -av
