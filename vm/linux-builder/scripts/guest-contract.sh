#!/usr/bin/env bash
# The guest contract, builder half: the same protocol the desktop guests speak,
# with the one thing a builder cannot have supplied differently.
#
# The job writes `output.log` and `artifacts/`, and `exit_code.txt` last, so the
# file appearing is what tells the orchestrator the run is over. The root is the
# same absolute path outside any home directory, and it has to match
# `GUEST_ROOT_LINUX` in crates/xtask/src/provider/mod.rs, which a unit test pins.
#
# What differs is the readiness marker. In a desktop guest the session writes it
# at logon, which is also how the session's environment reaches processes the
# orchestrator starts over SSH. There is no session here and never will be, so a
# oneshot unit writes the marker at boot and there is no `session.env` at all:
# the runner sources one when there is one, finds none, and leaves `DISPLAY` at
# its harmless default. That is what lets `bring_up` wait for the marker on every
# image rather than branching on which kind of guest it booted.
set -euxo pipefail

root=/var/lib/sunlit-e2e
install -d -o "${TEST_USER}" -g "${TEST_USER}" -m 0755 \
  "${root}" "${root}/bin" "${root}/fixtures" "${root}/results" "${root}/results/artifacts"

# The marker, at boot. `date +%s` in it for the same reason the desktop one
# carries it: a marker with a timestamp says when the guest became usable, and a
# stale one from a previous boot is impossible because the file is removed first.
cat > /usr/local/bin/sunlit-e2e-boot-ready <<'EOF'
#!/bin/sh
set -e
root=/var/lib/sunlit-e2e
mkdir -p "${root}/results/artifacts" "${root}/bin"
rm -f "${root}/ready"
date +%s > "${root}/ready"
EOF
chmod 0755 /usr/local/bin/sunlit-e2e-boot-ready

cat > /etc/systemd/system/sunlit-e2e-ready.service <<'EOF'
[Unit]
Description=Sunlit e2e readiness marker
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
ExecStart=/usr/local/bin/sunlit-e2e-boot-ready
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
EOF
systemctl enable sunlit-e2e-ready.service

# The runner itself always exits 0: its exit code answers "did the job get
# started", and the job's own answer is in exit_code.txt. Conflating the two is
# how a failed launch gets reported as a failed build.
#
# Byte for byte the desktop guest's runner, including the `session.env` it will
# never find, so that one job script runs unchanged in either kind of guest.
cat > /usr/local/bin/sunlit-e2e-run-job <<'EOF'
#!/usr/bin/env bash
set -uo pipefail
root="${SUNLIT_E2E_ROOT:-/var/lib/sunlit-e2e}"
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
export SUNLIT_E2E_ROOT="${root}"
export SUNLIT_E2E_RESULTS="${results}"
export SUNLIT_E2E_ARTIFACTS="${results}/artifacts"

if [ ! -f "${root}/job.sh" ]; then
  echo "no job.sh in ${root}" > "${results}/output.log"
  echo 127 > "${results}/exit_code.txt"
  exit 0
fi

chmod +x "${root}/job.sh" || true
( cd "${root}" && "${root}/job.sh" ) > "${results}/output.log" 2>&1
code=$?
printf '%s\n' "${code}" > "${results}/exit_code.txt"
exit 0
EOF
chmod 0755 /usr/local/bin/sunlit-e2e-run-job
