#!/usr/bin/env bash
# The guest contract (plan decision 6), Linux half: a results directory the
# orchestrator polls, and a job runner that executes in the autologin session.
#
# The protocol is the same on both guests. The job writes `output.log` and
# `artifacts/`, and `exit_code.txt` last, so the file appearing is what tells
# the orchestrator the run is over.
set -euxo pipefail

home="$(getent passwd "${TEST_USER}" | cut -d: -f6)"
root="${home}/sunlit-e2e"
install -d -o "${TEST_USER}" -g "${TEST_USER}" -m 0755 \
  "${root}" "${root}/bin" "${root}/results" "${root}/results/artifacts"

# Written by the desktop session at logon. Two jobs: it is the marker that says
# a desktop now exists, and it carries the session's environment out to
# processes the orchestrator starts over SSH, which inherit none of it.
cat > /usr/local/bin/sunlit-e2e-session-ready <<'EOF'
#!/bin/sh
set -e
root="${HOME}/sunlit-e2e"
mkdir -p "${root}/results/artifacts" "${root}/bin"
# An SSH-launched process runs as the same user but with no X credentials of
# its own; this is what lets it open a window on the running session.
xhost "+SI:localuser:$(id -un)" >/dev/null 2>&1 || true
{
  echo "DISPLAY=${DISPLAY}"
  echo "XAUTHORITY=${XAUTHORITY}"
  echo "DBUS_SESSION_BUS_ADDRESS=${DBUS_SESSION_BUS_ADDRESS}"
  echo "XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR}"
} > "${root}/session.env"
rm -f "${root}/ready"
date +%s > "${root}/ready"
EOF
chmod 0755 /usr/local/bin/sunlit-e2e-session-ready

install -d -m 0755 /etc/xdg/autostart
cat > /etc/xdg/autostart/sunlit-e2e-session-ready.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=Sunlit e2e session marker
Exec=/usr/local/bin/sunlit-e2e-session-ready
NoDisplay=true
X-GNOME-Autostart-enabled=true
EOF

# The runner itself always exits 0: its exit code answers "did the job get
# started", and the job's own answer is in exit_code.txt. Conflating the two
# is how a failed launch gets reported as a failed test suite.
cat > /usr/local/bin/sunlit-e2e-run-job <<'EOF'
#!/usr/bin/env bash
set -uo pipefail
root="${SUNLIT_E2E_ROOT:-${HOME}/sunlit-e2e}"
results="${root}/results"

rm -rf "${results}"
mkdir -p "${results}/artifacts"

if [ -f "${root}/session.env" ]; then
  set -a
  # shellcheck disable=SC1091
  . "${root}/session.env"
  set +a
fi
export DISPLAY="${DISPLAY:-:0}"
export SUNLIT_E2E_RESULTS="${results}"
export SUNLIT_E2E_ARTIFACTS="${results}/artifacts"

if [ ! -f "${root}/job.sh" ]; then
  echo "no job.sh in ${root}" > "${results}/output.log"
  echo 127 > "${results}/exit_code.txt"
  exit 0
fi

chmod +x "${root}/job.sh" || true
( cd "${root}" && ./job.sh ) > "${results}/output.log" 2>&1
code=$?
printf '%s\n' "${code}" > "${results}/exit_code.txt"
exit 0
EOF
chmod 0755 /usr/local/bin/sunlit-e2e-run-job
