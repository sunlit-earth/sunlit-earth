#!/usr/bin/env bash
# GNOME on Xorg, autologin, animations off. Runs as root through sudo.
set -euxo pipefail

cloud-init status --wait || true
export DEBIAN_FRONTEND=noninteractive

# Unattended upgrades hold the apt lock at unpredictable moments, which turns
# both this build and a later test run into a coin flip.
systemctl disable --now unattended-upgrades.service 2>/dev/null || true
systemctl disable --now apt-daily.timer apt-daily-upgrade.timer 2>/dev/null || true

apt-get update
apt-get install -y --no-install-recommends \
  gdm3 gnome-session gnome-shell gnome-terminal gnome-settings-daemon \
  xserver-xorg xserver-xorg-core xinit dbus-x11 x11-xserver-utils xauth \
  mesa-vulkan-drivers libgl1-mesa-dri vulkan-tools \
  libfontconfig1 libxkbcommon0 libxcb-shape0 libxcb-xfixes0 \
  fonts-dejavu-core openssh-server ca-certificates

# Xorg, not Wayland. An X session is what makes DISPLAY=:0 reachable from a
# process the orchestrator starts over SSH, which is how the job runs at all.
install -d -m 0755 /etc/gdm3
cat > /etc/gdm3/custom.conf <<EOF
[daemon]
WaylandEnable=false
AutomaticLoginEnable=true
AutomaticLogin=${TEST_USER}

[security]

[xdmcp]

[chooser]

[debug]
EOF

# System-wide dconf defaults rather than gsettings: these have to hold for a
# session that does not exist yet at build time.
install -d -m 0755 /etc/dconf/profile /etc/dconf/db/local.d
cat > /etc/dconf/profile/user <<'EOF'
user-db:user
system-db:local
EOF
cat > /etc/dconf/db/local.d/00-sunlit-e2e <<'EOF'
# GNOME Shell composites through llvmpipe in here, so animations cost real
# time and add real jitter to anything a test measures.
[org/gnome/desktop/interface]
enable-animations=false

# Nothing may blank, lock, or suspend the session under a long test run.
[org/gnome/desktop/session]
idle-delay=uint32 0

[org/gnome/desktop/screensaver]
lock-enabled=false
idle-activation-enabled=false

[org/gnome/settings-daemon/plugins/power]
sleep-inactive-ac-type='nothing'
sleep-inactive-battery-type='nothing'

[org/gnome/desktop/notifications]
show-banners=false
EOF
dconf update

systemctl set-default graphical.target
systemctl enable gdm3
systemctl enable ssh
