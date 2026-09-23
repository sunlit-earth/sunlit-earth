#!/usr/bin/env bash
# Four desktops on Xorg, two of them on Wayland as well, sway and i3, one
# display manager, and a per-boot choice between them. Runs as root through sudo.
#
# Phase 5 decision 2: KDE Plasma (the default), GNOME, XFCE and Cinnamon, each
# from the smallest package set that gives a working session, never the
# kitchen-sink metapackages. Together they are roughly 80-90% of measured Linux
# desktop usage, which is what makes the wallpaper backends in the app worth
# verifying live rather than only reviewing.
#
# Phase 5 decision 3: nothing in the image decides which one a boot uses. The
# host writes a session name into QEMU's fw_cfg and the selector unit installed
# at the bottom of this script puts it in sddm's autologin configuration before
# the display manager starts. So switching desktops costs a reboot rather than
# an image rebuild.
set -euxo pipefail

cloud-init status --wait || true
export DEBIAN_FRONTEND=noninteractive

# Unattended upgrades hold the apt lock at unpredictable moments, which turns
# both this build and a later test run into a coin flip.
systemctl disable --now unattended-upgrades.service 2>/dev/null || true
systemctl disable --now apt-daily.timer apt-daily-upgrade.timer 2>/dev/null || true

# The one property of the base image that decides whether any of this can work,
# checked before six gigabytes of desktops are installed on top of it. sddm waits
# for logind to report a graphical seat, logind calls a seat graphical when it has
# a DRM device, and the run-time display device is virtio-vga, so what has to
# exist is a `virtio_gpu` module for this kernel. Debian's `genericcloud` image
# has none: its `linux-image-cloud-amd64` is built without drivers for physical
# hardware and DRM goes with them, and the symptom an hour later is a guest
# sitting on the text console with sddm running and no X server, which is not a
# symptom that names its cause. `generic` carries `linux-image-amd64`.
if ! modinfo virtio_gpu >/dev/null 2>&1; then
  echo "this kernel ($(uname -r)) has no virtio_gpu module, so the guest will" \
    "have no /dev/dri, no graphical seat, and no X session. The base image is" \
    "wrong: it needs to be debian-13-generic, not debian-13-genericcloud." >&2
  exit 1
fi
ls -l /dev/dri || echo "no /dev/dri under the build's own display device"

apt-get update

# sddm on its own and first, for two reasons. It answers the shared debconf
# question about which display manager owns the session, so no later package can
# put that prompt in front of a non-interactive build; and `cinnamon-core`
# depends on `slick-greeter | lightdm | x-display-manager`, which apt satisfies
# with the first alternative unless something already provides the last one.
echo "sddm shared/default-x-display-manager select sddm" | debconf-set-selections
apt-get install -y --no-install-recommends sddm
# The debconf answer above is what writes this file, and the whole point of
# preseeding it is that nothing later gets to ask. If it says anything else, the
# session would come up under a display manager none of this configures, so the
# build stops here rather than forty minutes later at a greeter.
if ! grep -q sddm /etc/X11/default-display-manager 2>/dev/null; then
  echo "the session's display manager is '$(cat /etc/X11/default-display-manager 2>/dev/null)'," \
    "not sddm, so the preseed above did not take" >&2
  exit 1
fi

# The X server, the software GL and Vulkan stacks the tests render on, the
# libraries the app links against, and the tools the guest contract, the monitor
# query and the wallpaper setters use. Each of the last four is a package away
# from being missing, and none of them arrives on its own with
# --no-install-recommends: `xdpyinfo` from x11-utils, `xrandr` from
# x11-xserver-utils, `gsettings` from libglib2.0-bin, which is the wallpaper
# backend for two of the four desktops, and `dconf` from dconf-cli, which is what
# compiles the system-wide defaults below. `xdotool` is here to answer questions
# about the guest rather than to run the suite: it is what can say where a
# pointer actually is.
#
# The last two are what a guest with more than one screen needs. `xinput` is how
# the boot maps the single absolute pointer onto the primary output; without it
# QEMU scales that head's coordinates across the whole desktop and a click lands
# at twice the x it was aimed at. `arandr` is a display settings UI that works in
# every session: this minimal Plasma install ships none, while GNOME, XFCE and
# Cinnamon each carry their own, and Plasma's `kscreen` plus `systemsettings`
# costs tens of megabytes against about one.
apt-get install -y --no-install-recommends \
  xserver-xorg xserver-xorg-core xinit dbus-x11 xauth \
  x11-xserver-utils x11-utils xdotool libglib2.0-bin dconf-cli \
  xinput arandr \
  mesa-vulkan-drivers libgl1-mesa-dri vulkan-tools \
  libfontconfig1 libxkbcommon0 libxkbcommon-x11-0 libxcb-shape0 libxcb-xfixes0 \
  fonts-dejavu-core openssh-server ca-certificates

# The four desktops. `plasma-workspace` is named alongside `plasma-desktop`,
# which depends on it anyway, because it is the package that ships both the
# `plasmax11` session and `plasma-apply-wallpaperimage`. `kwin-x11` is named
# because Debian has it only as one half of a Recommends alternative
# (`kwin-wayland | kwin-x11`), so nothing pulls it in and a Plasma X11 session
# without it has no window manager at all.
apt-get install -y --no-install-recommends \
  plasma-desktop plasma-workspace kwin-x11 \
  gnome-session gnome-session-xsession gnome-shell gnome-settings-daemon \
  gnome-terminal \
  xfce4 \
  cinnamon-core

# Two sessions with no desktop of their own, where the wallpaper setters that
# are not a desktop's are exercised: sway on Wayland, with `swaybg` both for
# sway's own `output bg` and for the app's owned one, and i3 on X11, where the
# app paints the root window itself. Neither runs XDG autostart, so each gets
# the session marker from its own configuration below.
apt-get install -y --no-install-recommends \
  sway swaybg \
  i3-wm

# Every session name the host may ask for has to exist, and the moment to find
# out is the build rather than a boot forty minutes of image later: a missing
# session file makes sddm fall back to its own idea of a session, which is not
# the desktop whose results anybody is about to read.
for session in plasmax11 gnome-xorg xfce cinnamon i3; do
  test -f "/usr/share/xsessions/${session}.desktop"
done

# The Wayland sessions, from the same packages. Each name has to exist in the
# Wayland directory and must not exist in the X11 one: sddm's autologin resolves
# a name by basename and looks in /usr/share/xsessions first (sddm issue #837), so
# a name in both starts the X11 session while every record says Wayland. That is
# why GNOME's is `gnome-wayland` and not `gnome`, which both directories have.
for session in plasma gnome-wayland sway; do
  test -f "/usr/share/wayland-sessions/${session}.desktop"
  if [ -e "/usr/share/xsessions/${session}.desktop" ]; then
    echo "${session} is an X11 session name as well as a Wayland one, so sddm would start the X11 session for it" >&2
    exit 1
  fi
done

# The greeter's display server, which is all this setting decides: autologin
# never shows the greeter, and the session type comes from the session file the
# selector below names.
install -d -m 0755 /etc/sddm.conf.d
cat > /etc/sddm.conf.d/00-sunlit-general.conf <<'EOF'
[General]
DisplayServer=x11
EOF

# System-wide dconf defaults rather than gsettings: these have to hold for a
# session that does not exist yet at build time. GNOME and Cinnamon both read
# dconf, under their own schema paths, so both sets live here.
install -d -m 0755 /etc/dconf/profile /etc/dconf/db/local.d
cat > /etc/dconf/profile/user <<'EOF'
user-db:user
system-db:local
EOF
cat > /etc/dconf/db/local.d/00-sunlit-e2e <<'EOF'
# GNOME Shell and Muffin both composite through llvmpipe in here, so animations
# cost real time and add real jitter to anything a test measures.
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

[org/cinnamon]
desktop-effects=false

[org/cinnamon/desktop/interface]
enable-animations=false

[org/cinnamon/desktop/session]
idle-delay=uint32 0

[org/cinnamon/desktop/screensaver]
lock-enabled=false
idle-activation-enabled=false

[org/cinnamon/settings-daemon/plugins/power]
sleep-inactive-ac-type='nothing'
sleep-inactive-battery-type='nothing'

[org/cinnamon/desktop/notifications]
display-notifications=false
EOF
dconf update

# Plasma's defaults, as KConfig files in the system default configuration
# directory: KDE reads $XDG_CONFIG_DIRS, which is /etc/xdg, for anything the
# user's own configuration does not set.
install -d -m 0755 /etc/xdg
cat > /etc/xdg/kwinrc <<'EOF'
# KWin composites through llvmpipe here, which is the one setting of the four
# desktops' compositors that can be turned off outright.
[Compositing]
Enabled=false
EOF
cat > /etc/xdg/kscreenlockerrc <<'EOF'
[Daemon]
Autolock=false
LockOnResume=false
Timeout=0
EOF
cat > /etc/xdg/baloofilerc <<'EOF'
# File indexing on a throwaway guest indexes a test suite's output.
[Basic Settings]
Indexing-Enabled=false
EOF
cat > /etc/xdg/powermanagementprofilesrc <<'EOF'
[AC][DPMSControl]
idleTime=0

[AC][DimDisplay]
idleTime=0

[AC][SuspendSession]
idleTime=0
suspendType=0
EOF

# XFCE's defaults, as xfconf channel files in the same system directory.
#
# None of the three packages these configure is installed: the screensaver, the
# power manager and the notification daemon are all Recommends of the `xfce4`
# metapackage, and this build takes none of its recommends, so this session has
# nothing that blanks a screen or pops a banner. The files are written anyway,
# dormant, so that a Debian which promotes any of the three to a dependency does
# not quietly bring its behaviour with it.
#
# xfwm4's own channel is deliberately not written here, though it is the one
# package of the four that is installed. xfconf reads one file per channel rather
# than merging across the search path, so a file of ours would replace the one
# xfwm4 ships and take its theme and keybindings with it. Compositing is turned
# off from inside the session instead, further down.
install -d -m 0755 /etc/xdg/xfce4/xfconf/xfce-perchannel-xml
cat > /etc/xdg/xfce4/xfconf/xfce-perchannel-xml/xfce4-screensaver.xml <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<channel name="xfce4-screensaver" version="1.0">
  <property name="saver" type="empty">
    <property name="enabled" type="bool" value="false"/>
    <property name="idle-activation" type="empty">
      <property name="enabled" type="bool" value="false"/>
    </property>
  </property>
  <property name="lock" type="empty">
    <property name="enabled" type="bool" value="false"/>
  </property>
</channel>
EOF
cat > /etc/xdg/xfce4/xfconf/xfce-perchannel-xml/xfce4-power-manager.xml <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<channel name="xfce4-power-manager" version="1.0">
  <property name="xfce4-power-manager" type="empty">
    <property name="blank-on-ac" type="uint" value="0"/>
    <property name="dpms-enabled" type="bool" value="false"/>
    <property name="dpms-on-ac-sleep" type="uint" value="0"/>
    <property name="dpms-on-ac-off" type="uint" value="0"/>
  </property>
</channel>
EOF
cat > /etc/xdg/xfce4/xfconf/xfce-perchannel-xml/xfce4-notifyd.xml <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<channel name="xfce4-notifyd" version="1.0">
  <property name="do-not-disturb" type="bool" value="true"/>
</channel>
EOF

# xfwm4's compositing, turned off from inside the session rather than from a
# channel file, for the reason given above: this talks to a running xfconfd,
# which is a per-property write and leaves everything else xfwm4 shipped alone.
# `OnlyShowIn` keeps it out of the other three sessions, where the channel does
# not exist and the call would be a stray error in a log.
cat > /usr/local/bin/sunlit-e2e-xfce-quiet <<'EOF'
#!/bin/sh
# xfwm4 composites through llvmpipe in here, which is the one compositor of the
# four desktops that can be turned off at all.
exec xfconf-query -c xfwm4 -p /general/use_compositing -s false
EOF
chmod 0755 /usr/local/bin/sunlit-e2e-xfce-quiet
install -d -m 0755 /etc/xdg/autostart
cat > /etc/xdg/autostart/sunlit-e2e-xfce-quiet.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=Sunlit e2e XFCE compositing off
Exec=/usr/local/bin/sunlit-e2e-xfce-quiet
NoDisplay=true
OnlyShowIn=XFCE;
EOF

# The one first-run dialog among the four. xfce4-panel with no configuration at
# all asks whether to use the default layout or an empty panel, and it asks in a
# modal window over the session, which is fatal to a test that clicks anything.
# Giving the account the layout the dialog offers is what stops it being asked.
home="$(getent passwd "${TEST_USER}" | cut -d: -f6)"
install -d -o "${TEST_USER}" -g "${TEST_USER}" -m 0755 "${home}/.config"
if [ -f /etc/xdg/xfce4/panel/default.xml ]; then
  install -D -o "${TEST_USER}" -g "${TEST_USER}" -m 0644 \
    /etc/xdg/xfce4/panel/default.xml \
    "${home}/.config/xfce4/xfconf/xfce-perchannel-xml/xfce4-panel.xml"
  chown -R "${TEST_USER}:${TEST_USER}" "${home}/.config/xfce4"
fi

# sway and i3 start no XDG autostart entries, so the session marker every other
# desktop gets from /etc/xdg/autostart is started from their own configuration.
# Debian's /etc/sway/config ends by including /etc/sway/config.d; the snippet also
# replaces the default background, whose image is in sway-backgrounds, a
# Recommends this build leaves out. i3 reads the account's own config before
# /etc/i3's, and having one is also what keeps i3-config-wizard from asking in a
# window over the session at the first login.
install -d -m 0755 /etc/sway/config.d
cat > /etc/sway/config.d/50-sunlit-e2e.conf <<'EOF'
output * bg #203040 solid_color
exec /usr/local/bin/sunlit-e2e-session-ready
EOF
install -d -o "${TEST_USER}" -g "${TEST_USER}" -m 0755 "${home}/.config/i3"
grep -v 'i3-config-wizard' /etc/i3/config > "${home}/.config/i3/config"
echo 'exec --no-startup-id /usr/local/bin/sunlit-e2e-session-ready' >> "${home}/.config/i3/config"
chown "${TEST_USER}:${TEST_USER}" "${home}/.config/i3/config"

# The per-boot desktop choice (phase 5 decision 3). The host adds
# `-fw_cfg name=opt/sunlit/desktop,string=<session>` to QEMU's command line and
# this reads it back out. The device is ACPI-enumerated as QEMU0002, so
# `qemu_fw_cfg` loads itself long before the display manager starts; the modprobe
# is there for a kernel where it did not and costs nothing where it did.
#
# The accepted names are an allowlist, not whatever the host said. A value from
# outside the machine ends up in a configuration file read by a process running
# as root, and `crates/xtask/src/provider/desktop.rs` holds the same six names
# on the other side with a test that compares them against this file.
cat > /usr/local/bin/sunlit-e2e-select-desktop <<'SELECTOR'
#!/bin/sh
set -eu

default_session=plasmax11
conf=/etc/sddm.conf.d/10-sunlit-autologin.conf
asked_path=/sys/firmware/qemu_fw_cfg/by_name/opt/sunlit/desktop/raw

modprobe qemu_fw_cfg 2>/dev/null || true

session="${default_session}"
if [ -r "${asked_path}" ]; then
  asked="$(tr -d '\000\r\n' < "${asked_path}")"
  case "${asked}" in
    plasmax11|gnome-xorg|xfce|cinnamon|i3|plasma|gnome-wayland|sway) session="${asked}" ;;
    "") ;;
    *) echo "sunlit-e2e: '${asked}' is not a desktop this image offers" >&2 ;;
  esac
fi

# A name that passed the allowlist and still has no session file means the image
# was built without that desktop, which is worth saying rather than handing sddm
# a session it will silently replace with one of its own choosing.
case "${session}" in
  plasma|gnome-wayland|sway) sessions_dir=/usr/share/wayland-sessions ;;
  *) sessions_dir=/usr/share/xsessions ;;
esac
if [ ! -f "${sessions_dir}/${session}.desktop" ]; then
  echo "sunlit-e2e: no ${session} session in this image" >&2
  session="${default_session}"
fi

mkdir -p /etc/sddm.conf.d
cat > "${conf}" <<EOF
# Written on every boot by sunlit-e2e-select-desktop. Editing it lasts until the
# next one.
[Autologin]
User=@TEST_USER@
Session=${session}
Relogin=false
EOF
echo "sunlit-e2e: autologin session is ${session}"
SELECTOR
sed -i "s/@TEST_USER@/${TEST_USER}/" /usr/local/bin/sunlit-e2e-select-desktop
chmod 0755 /usr/local/bin/sunlit-e2e-select-desktop

cat > /etc/systemd/system/sunlit-e2e-desktop.service <<'EOF'
[Unit]
Description=Choose the desktop session this boot logs into
Before=display-manager.service

[Service]
Type=oneshot
ExecStart=/usr/local/bin/sunlit-e2e-select-desktop
RemainAfterExit=yes

[Install]
WantedBy=graphical.target
EOF
systemctl enable sunlit-e2e-desktop.service

# Run once here as well as on every boot, so that an image whose selector unit
# never ran still autologs in somewhere rather than sitting at a greeter. The
# account name is baked into the script by the substitution above, so this needs
# nothing from the build's environment.
/usr/local/bin/sunlit-e2e-select-desktop

systemctl set-default graphical.target
systemctl enable sddm

# SSH is left as openssh-server set it up rather than enabled again. Debian is
# in the middle of moving from `ssh.service` to socket activation, and the two
# conflict: enabling the one the package did not choose is how a guest ends up
# with no SSH at all. Packer is talking to this guest over SSH right now, so what
# is worth doing is confirming the enablement survives a reboot.
systemctl is-enabled ssh.service || systemctl is-enabled ssh.socket
