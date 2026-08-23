#!/usr/bin/env bash
# Last pass over the golden image: stop cloud-init from looking for a datasource
# that will not be there, and shrink what the qcow2 has to carry.
#
# The first-run wizard suppression that used to be here is gone with the base:
# `gnome-initial-setup` is a package this image does not install, and the one
# first-run dialog among the four desktops is xfce4-panel's, which `desktop.sh`
# answers by giving the account the layout the dialog offers.
set -euxo pipefail

# Every run boots a throwaway overlay of this image with no seed CD attached.
# Left enabled, cloud-init would spend its datasource timeout looking for one
# on every single boot.
touch /etc/cloud/cloud-init.disabled

# The marker is written at logon. Shipping one inside the image would tell the
# orchestrator a desktop exists before the guest had finished booting.
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
# Both, because Debian is moving from the service to socket activation and this
# has to come first whichever of the two this image ended up with.
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

# Zero the free space so the qcow2 compresses down to what it actually holds.
dd if=/dev/zero of=/zero.fill bs=1M 2>/dev/null || true
rm -f /zero.fill
sync
