# Plan: Phase 4.1, Clouds Follow the Resolution, and a Memory Report

## Summary

Two closures on phase 4. The cloud texture variant follows the texture resolution setting instead of the quality tier, so lowering the resolution lowers the biggest texture the setting previously did not touch. And the app can report where its memory actually is: a `memory-report` IPC command that prints the top memory holders, expected next to measured, instead of process-wide totals nobody can act on. The report is deliberately short: top-N entries, everything else rolled up, because a report nobody reads is telemetry, not a tool.

## Stakes Classification

Low-medium. The cloud change alters which URL is fetched and when a fetch is triggered; a mistake shows up as stale or missing clouds, recoverable by the next poll. The report is read-only diagnostics. Nothing touches persisted state: the config schema is unchanged, the cloud variant is derived, not stored.

## Research

Recorded 2026-08-22 from the wgpu 28 sources in the local registry and the tree at this date.

- `wgpu::Device::generate_allocator_report()` exists (`wgpu-28.0.0/src/api/device.rs:533`) and returns `Option<wgt::AllocatorReport>`: `allocations` (label, offset, size per live allocation), `blocks`, `total_allocated_bytes`, `total_reserved_bytes` (`wgpu-types-28.0.0/src/counters.rs:155-186`). Implemented for D3D12 (`dx12/device.rs:2569`) and Vulkan (`vulkan/device.rs:2608`); every other backend inherits `None` (`wgpu-hal-28.0.1/src/lib.rs:1145`). D3D12 routes buffers and textures through the suballocator whenever placed resources are supported, committed resources are only the fallback (`dx12/suballocation.rs:186-214`), so the report sees effectively everything wgpu owns, including its internal staging buffers. Our adapters: host GPU and WARP are D3D12, lavapipe is Vulkan, all covered; Metal is the one `None`.
- `wgpu::Device::get_internal_counters()` exists (`api/device.rs:523`); `HalCounters` carries `buffer_memory`, `texture_memory`, `memory_allocations` as running byte totals (`wgpu-types-28.0.0/src/counters.rs:124-132`). The `counters` cargo feature is not in wgpu's defaults (`wgpu-28.0.0/Cargo.toml:50`), so today the counters read zero; the documented contract is that unset counters return zero, so the report can print them unconditionally.
- The cloud URL is one function: `cloud_fetcher::cloud_url(tier)` builds `https://clouds.matteason.co.uk/images/{size}/clouds.jpg` from `QualityTier::cloud_size()` (`cloud_fetcher.rs:102-114`, `config.rs:56-62`); `SUNLIT_EARTH_CLOUD_URL` wins over it. The three variants (2048x1024, 4096x2048, 8192x4096) map one to one onto the three texture resolutions. The on-disk cache is a JPEG plus an etag sidecar (`CacheMeta`, `cloud_fetcher.rs:94-97`); whether its file name keys on the variant is checked in Step 2.
- `query-memory` (`ipc.rs:138-149`) prints exactly `SIGNAL:memory rss_bytes=... peak_rss_bytes=... private_bytes=...`; the e2e suite parses that line and it must not change.
- `memory.rs:37-47`: the 3 GiB `PRIVATE_BYTES_BUDGET` is a single constant with a comment saying a budget derived from the texture tier belongs with exactly this work. The documented startup peak is ~2.43 GiB, from decoding the two 8K JXL sources, which still happens once at any resolution while the downscale cache is cold.
- Steady-state sizes for the report to be checked against: surface pair 341/85/21 MiB at 8192/4096/2048 (RGBA8 plus full mip chain), cloud 171/43/11 MiB per variant, grid ~11 MiB, preview render targets color plus Depth32Float plus an MSAA pair at the sample count (`gpu_setup.rs:217-248`, `create_render_textures`).

## Key Design Decisions

1. **The cloud variant is keyed on `texture_resolution`, one to one, and the quality tier keeps everything else.** `cloud_url` takes the resolution instead of the tier; 8192 fetches 8192x4096 and so on down. The tier keeps its MSAA and preview caps. `SUNLIT_EARTH_CLOUD_URL` still wins over everything, and `SUNLIT_EARTH_NO_CLOUDS` is untouched. Nothing is persisted: the variant is derived from the same config field phase 4 added.

2. **A resolution switch re-targets the cloud pipeline and requests an immediate poll; the old cloud texture stays until the new variant lands.** The switch itself must not depend on the network, and a cloudless gap would be a regression, so `SetTextureResolution` does not purge the cloud slot. It updates the URL the worker fetches and pokes a poll; when the new image arrives, the existing update path replaces texture, view, and bind group together, which the soak test already proves frees the old one. Offline, the guest keeps the old variant until a poll succeeds, and the log says so. Step 2 verifies the cache cannot serve a stale variant across the switch (the cache entry must be keyed by variant or invalidated when the URL changes).

3. **The report is an engine command with a reply, surfaced as a new `memory-report` IPC command; `query-memory` does not change.** The device is owned by the engine thread, so assembly happens there, following the `ExportPixels` reply-channel precedent. The e2e suite parses `query-memory`'s single line, so that command stays byte-identical and the new command gets its own name.

4. **Top-N discipline.** The report holds four short sections: the process snapshot (three numbers); the wgpu internal counters (texture bytes, buffer bytes, allocation count); the allocator report reduced to its two totals plus the top 10 allocations of at least 1 MiB, aggregated by label, with everything smaller rolled into one line ("n more, x MiB"); and the renderer's expected table, every texture and render target it owns that is at least 1 MiB, with dimensions, format, mips, and computed bytes. Expected next to measured is the point: the day the columns disagree is the day there is a leak. A CPU-side byte ledger and a counting global allocator are deliberately out of scope: steady-state CPU pixel memory is near zero by the resource-flow rules, transients are visible in `peak_rss_bytes`, and per-category counters would be rows nobody reads.

5. **The wgpu `counters` feature goes on workspace-wide.** Two honest totals for the cost of relaxed atomic adds on resource create and destroy. Backends that do not maintain a counter report zero, and the report prints them as such rather than hiding the section.

6. **Backends without an allocator report degrade section by section.** Metal returns `None`; the report prints the sections it has and names the adapter (`wgpu_init::adapter_key`) so the reader knows whether GPU bytes overlap `private_bytes` (they do on WARP and lavapipe, they mostly do not on a discrete GPU).

7. **The private-bytes budget derives from the resolution.** The 3 GiB constant becomes a function of `texture_resolution`, sized from the measured steady states plus headroom for the one 8K decode a cold cache still performs at any resolution. The rule from the existing comment stands: a warning that fires during normal operation is a warning nobody reads, and a test pins that the cold-cache first run at the default resolution does not warn.

## Success Criteria

1. The fetched cloud variant follows the resolution: unit tests replace the tier-keyed ones, and an engine test with a fixture cloud source proves a switch triggers a refetch and the replacement frees the old texture.
2. `memory-report` prints the four sections and nothing else, on WARP and on a real GPU; on a backend without an allocator report the section says so instead of vanishing silently.
3. After an 8192 to 2048 switch, the report shows no allocation at the old width, and the measured totals agree with the expected table within a stated tolerance; pool slack is visible as reserved minus allocated.
4. `query-memory` output is byte-identical to today; the e2e memory test passes unchanged.
5. The budget warns on a genuinely exceeded budget and stays silent through a cold-cache first run at every resolution, pinned by tests on the budget function.
6. `cargo test`, `cargo clippy --all-targets`, `cargo fmt --check` green on Windows and in WSL; CLAUDE.md (cloud variant, IPC command, counters feature), README, and `docs/roadmap.md` updated.

## Implementation Steps

### Step 1: The memory report

Enable the `counters` feature; audit that every texture and buffer we create carries a distinctive label; the renderer's expected table; report assembly on the engine thread (`EngineCommand::ReportMemory` with a reply channel); the `memory-report` IPC command printing it; a debug-level dump once after `TexturesReady`. Tests: the top-N cap and the rollup line against a fabricated report, the expected table against known texture sets, the IPC command end to end where a device exists.

### Step 2: Clouds follow the resolution

`cloud_url` keyed on the resolution; the worker re-targets and polls immediately on a switch; verify the cache keys per variant (or invalidate on URL change) so a switch cannot be answered with the old variant's bytes; the engine test from criterion 1. The tier's `cloud_size` and its ordering test go or shrink to whatever still has a caller.

### Step 3: The budget

`PRIVATE_BYTES_BUDGET` becomes a function of the resolution with the cold-cache headroom; the metrics sampler calls it with the current resolution; tests pin the no-warn-in-normal-operation rule.

### Step 4: Documentation

CLAUDE.md: the cloud variant row in the env table ("wins over the quality tier" becomes the resolution), the quality tier section, the IPC command list, the counters feature and where the report lives. README and roadmap.

## Risks and Mitigations

- The allocator report takes an allocator lock; it runs on demand on the engine thread between frames, not on a schedule, so contention is bounded by how often someone asks.
- The `counters` feature adds atomic bookkeeping to every resource create and destroy. Expected to be unmeasurable next to texture uploads; if profiling disagrees, the feature is one line to revert.
- A switch while offline keeps the old cloud variant indefinitely. Intended and logged; the alternative, purging, trades a stale-resolution cloud for no cloud.
- Report format churn: only the `query-memory` line is a parsing contract; the new report promises stable section names, not stable line layouts, and says so where the format lives.
- Metal has no allocator report and possibly zero counters; the macOS leg is verifiable only through a `ci.yml` dispatch, which criterion 2's degrade behavior makes safe to defer.

## Rollback Strategy

Reverts cleanly by commit. No config schema change, no cache format change the old binary cannot ignore: the texture cache is untouched, and a cloud cache entry for a variant the old code never fetches is dead weight it never reads.

## Departures

None. Every design decision held as written. The two places the plan left a choice open were closed rather than departed from: decision 2's "keyed by variant or invalidated when the URL changes" was settled as variant-keyed, which is what the rollback note above already describes, and `cloud_variant`'s behavior for a width outside the three the config offers (take the widest variant that does not exceed it) is an extension the plan does not speak to rather than a contradiction of it.

## Validation

### Round 1 (2026-08-22)

Scope: the range b77b191..c38a9d9, a fresh validator, all three Windows gates re-run green by it (873 passed, 11 ignored, clippy clean after a fresh `cargo clean -p`), criterion 3 reproduced live on the real assets. Verdict: 1 major, 3 minors; the round blocks.

- M1: a switch back to a variant whose cache entry was still fresh never reached the screen. `set_resolution` adopted the new entry's `CacheMeta`, so the poll it asked for revalidated, was answered with a 304, and posted nothing; since the switch deliberately does not purge the cloud slot, the previous variant's overlay stayed until upstream published again, possibly hours. Nothing in production calls `post_cached` after startup, so the bytes were on disk and unreachable. Breaks decision 2 and criterion 1.
- m1: the comment on the cloud mailbox post still justified `generation: None` with the pre-change reason, that the resolution does not govern the overlay. It does now.
- m2: the variant-keyed cache name was a promise nothing enforced. `CloudUpdater::new` derived the file name from the resolution but never pointed the source anywhere, so a run under `SUNLIT_EARTH_CLOUD_URL` filed whatever the override served under a name claiming a variant, and the next run without the override would show it.
- m3: the per-resolution budget tests derived their peak from the budget's own decomposition, so `budget(w) - peak(w)` was a constant and the loop over the three widths asserted one inequality three times. Dropping `COLD_START_BYTES` from 2 GiB to 1.5 GiB passed everything except the exact-3-GiB test.

Disposition: all four fixed, none declined. M1, m1 and m2 in 86f48d0; m3 in 71d629f.

- M1: `set_resolution` posts the entry as it adopts it, which is what the worker already does at startup with the entry it finds. A variant with nothing on disk still posts nothing, which is the clause that keeps the old clouds up while a download runs, so the offline behavior decision 2 asks for is unchanged. Two tests, both verified by removing the line: a unit test on the updater, and `a_switch_back_to_a_cached_variant_shows_it_again`, the only engine test with a cache directory and a cloud source together, which is what it takes to reach the disk-cache branch at all.
- m1: replaced with the true reason. The slot is never purged, so there is nothing for a stamp to protect; a fetch of the old variant landing after a switch is one poll of exactly the picture the switch deliberately leaves up.
- m2: made true by construction rather than documented away. `CloudUpdater::new` points the source at the URL its entry is named after, and an override run caches under `clouds_cache_override` instead of a name claiming a size nobody checked. `set_resolution` now compares URLs rather than variants, so under the override a switch moves no entry and keeps its `ETag`. `SUNLIT_EARTH_CLOUD_URL` still wins: `cloud_url` applies it before any of this runs, so both retargets resolve to it.
- m3: the derived peak is gone. Every resolution is held to the one peak that was measured, 2.43 GiB at 8192 in a release build, because the peak is dominated by the two 8K decodes that happen at every width and how much of the resident saving reaches the peak is exactly what nobody checked. That makes 2048 the binding case, at 104 MiB of margin against 584 at 8192, and the same mutation now fails at 4096 and 2048 while the widest, where the 3 GiB total is anchored, still passes.

### Round 2 (2026-08-22)

Scope: the fix range c38a9d9..71d629f, all three Windows gates re-run green. Verdict: 0 majors, 2 minors; the round does not block. All four round 1 fixes verified, M1 and m3 by mutation.

- m4: `poll_once` saved the `CacheMeta` sidecar whether or not the image write succeeded, so a failed `fs::write` left an `ETag` with no image behind it. The code predates this range and is identical at 0304514, but the M1 fix made the invariant load-bearing: `post_cached` finds nothing, the poll sends the saved `ETag`, gets a 304, and the M1 symptom is back, with no clouds at all on a fresh process.
- m5: the doc comment on the new engine test said the other engine tests run with no cache directory. Six of them do have one.

Disposition: both fixed in 4f7f63b.

- m4: the image write now reports whether the entry holds the image and takes any partial file with it on the way out, and a failure discards the sidecar rather than refreshing it, including one an earlier poll left there. The in-memory copy is kept either way, because the frame is about to be on the GPU and with no cache directory it is the only copy there is, which is also the case every engine test runs in. The test puts a directory where the JPEG goes, which fails the write on both platforms while leaving the sidecar beside it writable, so the two halves are separable; it fails on both of its assertions against the old clause.
- m5: reworded. What makes that test the only route to the cloud disk cache is having a cache directory and a cloud source together, not the absence of directories elsewhere.

One observation from the round is recorded rather than fixed: `CloudUpdater::new` reads `SUNLIT_EARTH_CLOUD_URL`, both through `cloud_url` and to decide what the cache entry is named after, so the tests that assert on the variant-keyed URL or the variant-keyed entry fail in a shell that exports the variable. Measured rather than reasoned: five of the forty-one `cloud_fetcher` cases fail under `SUNLIT_EARTH_CLOUD_URL=http://127.0.0.1:9/never-fetched.jpg`, namely `construction_points_the_source_at_the_variant_it_will_cache_under`, `set_resolution_points_the_source_at_the_new_variant`, `each_variant_keeps_its_own_cached_image`, `a_switch_back_to_a_fresh_cache_entry_puts_its_pixels_on_screen`, and `an_image_that_could_not_be_cached_leaves_no_freshness_claim`. All five are new in this range, and they fail because the override is doing exactly what it promises: replacing the variant with one fixed URL. Nothing in CI, the VM jobs, or the cargo aliases exports the variable into a `cargo test` run; the e2e suite sets it per spawned child, not in the shell. The two ways out are both worse than the exposure: setting and clearing a process-wide variable from a test binary that runs its cases in parallel, or threading the resolved URL in from the caller on both `new` and `set_resolution`, which moves the env read to the worker and rebuilds a seam that is otherwise sound. If a third test ever wants the same guarantee, the second option is the one to take.
