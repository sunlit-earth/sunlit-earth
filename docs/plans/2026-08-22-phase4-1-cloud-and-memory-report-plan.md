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
