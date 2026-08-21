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

1. **Major 1 was fixed with a new recorded fact rather than by assigning `StartReason::Keep`.** The finding names the missing assignment, and making `e2e --keep` write `Keep` would have made `vm view` take the hand-over branch. It would also have kept one field answering two questions, which is what produced the finding: the reason is chosen before the boot, and whether a guest offers an enhanced session is something the guest confirms afterwards and can fail to. So `RunState` gained `handed_over`, written from the marker `enable_enhanced_session` reads, and `vm view` keys the settings write and the console note on that; `Keep` is assigned as well, because a kept guest reporting itself as a run in progress was the other half of the same wrong record and `StartReason::Keep` was otherwise dead. The same reasoning covers major 2: `vm smoke --keep` now prints from what was done to the guest (`vm::Prepared`) rather than being taught to stage and hand over, which would have added an unexercisable guest interaction to a command whose purpose is to prove the plumbing.

2. **Minor 4 was resolved toward `localhost`, changing a value `6cad5d7` verified by hand.** The settings file said `%COMPUTERNAME%` while `vmconnect` was started against `localhost`, and the file with that mismatch demonstrably suppressed the dialog, which is what proves `vmconnect` keys the settings on the file name. Both spellings name this same machine, and each has been observed working in one of the two roles, so one constant in both places cannot connect anywhere new; the alternative, leaving two spellings with a comment, keeps a discrepancy that only looks deliberate. This cannot be re-verified without a running guest, and the cost if it is wrong is one dialog on a best-effort path.

## Validation rounds

### Round 1

Validator over `d9875e2..6cad5d7`, gates all green (fmt, clippy, full workspace tests). Verdict: 3 majors, 8 minors. The majors are one shape: text and behavior keyed on a hand-over the code does not confirm happened. (1) `StartReason::Keep` is never assigned, so a guest kept by `e2e --keep` gets `vm view`'s run-in-progress branch: no settings file, wrong note. (2) `vm smoke --keep` prints hand-over instructions for a guest that was never staged or handed over. (3) The stale-image warning `ce97b24` put in `vm view` was deleted by `b35a555`, and the image on this host is stale, so the note claims no credentials will be asked for on a guest that will ask. Minors: vmconnect settings name a different server than vmconnect is launched with; CLAUDE.md's "every host tool goes through resolve_tool" overclaims; `enhanced_session_script` and `id_script` sit outside the parse check the range extended; `sweep_stale_settings` deletes files with no test; the settings file is in no teardown path; the clipboard claims in `vm up`'s output and vm-setup.md were made wrong by `ClipboardRedirection = True`; a stale trigger comment in ci.yml; an unparseable sentence in CLAUDE.md. Disposition of each is recorded below. The validator also listed the guest-side claims it could verify only by reading, noting the current image predates the `TermService` disable so that path has never run in a real build.

#### Round 1 disposition

All eleven were fixed; nothing was declined. Four commits on `feat/phase3-vm-orchestration`, all three gates green on the Windows host (`cargo fmt --check` clean, `cargo clippy --all-targets` clean, `cargo test` all suites passing with 391 xtask tests among them), no VM booted, built, or torn down.

1. Fixed. `RunState::handed_over`, written by `vm::hand_over` from the guest's own marker, is what `vm view` reads; `StartReason::Keep` is assigned at the same moment (departure 1).
2. Fixed. `lifecycle_explainer` takes `vm::Prepared`, so `vm smoke --keep` describes an empty desktop and a basic session instead of shortcuts and a passwordless enhanced one.
3. Fixed. The not-handed-over note says a credential dialog means the image predates the Remote Desktop Services disable, to cancel it, and which command rebuilds the image; `docs/vm-setup.md` says the same.
4. Fixed. `hyperv::VMCONNECT_SERVER` is the one value in the settings file and in the `vmconnect` invocation (departure 2).
5. Fixed. CLAUDE.md no longer claims every host tool goes through `resolve_tool`, and names the two lookups that deliberately stay on bare `PATH`: the Packer ISO tools and `windows_media`'s downloader.
6. Fixed. `handover::enhanced_session_script` and `hyperv::id_script` joined `all_scripts`.
7. Fixed. The sweep is now `hyperv::forget_settings` over the pure `is_our_settings` predicate, with a unit test for the predicate against a foreign VM's file, `vmconnect.config` and a truncated document, and a temp-directory test for both call paths. Writing that test found one real fault in the code it covers: the file to keep was recognized by comparing paths, which is case-sensitive against names from a file system that is not, so a settings file already present under another spelling would have been swept as an earlier guest's. It is recognized by name, without case, now.
8. Fixed. `HypervProvider::destroy` forgets the settings file, so `vm down`, `vm purge` and a stale-state clear all take it with the VM.
9. Fixed. The enhanced session's clipboard is stated as a clipboard in both the printed text and the guide; the redirection setting stays on, since a guest handed to a person is the one place it is wanted.
10. Fixed. The comment in `ci.yml` gives the reason that still applies.
11. Fixed. The sentence parses.

### Round 2

Validator over `be95772..051eddd` with fresh context, gates all green (run by the validator, clippy re-verified uncached). Verdict: 0 majors, 5 minors. All eleven round 1 findings verified fixed in the code, nothing cosmetically closed; both departures judged sound, with departure 1 rated strictly better than the fix round 1 named, since assigning `Keep` alone would have promised an enhanced session after a failed hand-over. Of the fixer's flagged open items, the unbumped `STATE_VERSION` and the text-only stale-image coverage were judged acceptable and are not findings. The five minors: (A) `commands::vm::view_note`'s basic-session text lost the stale-image caveat its `hyperv` sibling gained, so `vm smoke --keep` on a stale image says "nothing to type" about a console that will ask; (B) the stale-image sentence is unconditional and single-cause, so it misdiagnoses a build guest and a half-completed hand-over; (C) `Prepared.staged` means staging ran, not that the shortcuts arrived, so a failed `handover::prepare` still yields closing text naming them; (D) `hand_over` records `handed_over: true` for a Linux guest, wrong under the `SUNLIT_EARTH_VM_PROVIDER=hyperv` override; (E) `windows_media`'s downloader does not say at the site why it skips `resolve_tool`, which CLAUDE.md claims both bare-`PATH` lookups do. Disposition below.

#### Round 2 disposition

Recorded by the round 2 fixer.
