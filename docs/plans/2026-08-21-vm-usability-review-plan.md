# Review plan: Windows VM usability fixes

## Background

Five commits were made in an interactive session on `feat/phase3-vm-orchestration`, fixing usability problems in the Windows VM automation: guest handover to a person, tool discovery, console sizing, media cleanup, and CI cost. Each was verified by hand on this host, but none has had adversarial review. This plan drives that review through the orchestrate workflow, validator first.

## Scope

The fixed commit range `d9875e2..6cad5d7`:

- `ce97b24` Make a guest usable by hand, and stop paying for CI: desktop launcher and shortcuts staged per boot, Remote Desktop Services disabled in the image, shell-script parse check fed by stdin, CI moved to dispatch-only.
- `cccbbbc` Find tools scoop installed, and TightVNC's viewer among them: scoop shims and TightVNC's install directory join `resolve_tool`, viewer list carries executable names, `vm view` resolves viewers the same way `vm doctor` does.
- `452fab5` Take the repacked media back out, and size the guest console: a successful build deletes the repacked install media, `console_resolution` sizes the guest framebuffer from the host's work area with `SUNLIT_EARTH_VM_RESOLUTION` as override.
- `b35a555` Hand a guest over with an enhanced session and nothing to type: `handover::enable_enhanced_session` blanks the tester password, clears `LimitBlankPasswordUse`, starts Remote Desktop Services on `vm up` and `e2e --keep` only.
- `6cad5d7` Answer vmconnect's connection dialog before it opens: `vm view` writes vmconnect's per-VM settings file and sweeps the ones earlier guests left.

18 files, about 1500 insertions: `crates/xtask/src/**`, `vm/windows/scripts/`, `.github/workflows/ci.yml`, `CLAUDE.md`, `docs/vm-setup.md`, `docs/roadmap.md`.

## What review means here

1. A validator with fresh context reviews the diff against this plan, the project conventions in CLAUDE.md, and the claims each commit message makes.
2. The updated documentation (CLAUDE.md, docs/vm-setup.md) must match the code it describes; a doc claim the code does not implement is a finding.
3. Claims about guest behavior cannot be re-verified without an hour-long image build. The validator verifies them by reading the code path end to end, and says explicitly when a claim rests only on the commit message.
4. Hunt for what the changes should have touched but did not: callers of changed functions, tests for new decision logic, teardown paths for new on-disk state (the vmconnect settings files, the desktop shortcuts, the deleted repack).
5. Gates: `cargo test`, `cargo clippy --all-targets`, `cargo fmt --check`, run by the validator itself.
6. Findings use the orchestrate taxonomy. MAJOR findings go to an implementer for fixes on this branch; rounds iterate until one reports zero majors. MINOR findings are fixed or declined with reasoning recorded here.

## Constraints

- No VM is booted, built, or torn down during review. No `cargo xtask vm setup`, `vm up`, `vm build-image`, `vm smoke`, `vm down`, `vm purge`, and no `cargo xtask e2e`. `cargo xtask vm doctor` is allowed; it is read-only and unelevated.
- No `cargo e2e` on the host: it takes over the desktop. The desktop wallpaper is never set.
- Nothing is pushed. CI is dispatch-only and not a gate for this run. No PR exists and none is created.
- Fixes are committed on `feat/phase3-vm-orchestration` in reasonable chunks.

## Departures

None yet.

## Validation rounds

Recorded here per round.
