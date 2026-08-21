# Plan: Phase 4, Texture Resolution Selection

## Summary

A user-facing texture resolution setting: the Rendering group offers 8192, 4096, and 2048; lower resolutions are downscaled once from the 8K JXL sources and cached on disk; 4096 is the new default; switching resolutions fully purges the previous textures from memory, so going down actually lowers the process footprint, and the choice is persisted in the config. Pure app-side work, independent of phase 5 (the Linux plan), and reviewable on its own.

## Stakes Classification

Medium-low. The work touches persisted user config and GPU resource lifetimes; a mistake shows up as a stale frame, a crash on switch, or memory that never comes back, all recoverable and none destructive. Nothing outside the app's own data directory is touched.

## Research

Recorded from code reconnaissance on 2026-08-21, references to the tree at that date.

The day and night sources are `textures/world.topo.200405.jxl` and `textures/BlackMarble_2016.jxl`, 8K equirectangular; no smaller variants exist in the repo. Decoding runs `image::ImageReader` with the jxl-oxide hook (`assets/texture_loader.rs:9-11,31-50`), which is decode-only; nothing in the tree can encode JXL, but `engine::save_png` already encodes PNG. Each texture decodes on its own background thread (`renderer/textures.rs:114-153`) and posts to the latest-value mailbox; mip generation is a CPU box filter, `downsample_2x` (`renderer/textures.rs:284-309`), running on the engine thread inside `create_mipmapped_texture`. The renderer keeps only `TextureView`s and bind groups, never the `wgpu::Texture` itself (`renderer/mod.rs:144,187,190-205`; `textures.rs:9-16`), so freeing an old texture means dropping every derived view and bind group; `gpu_setup::replace_render_textures` (`gpu_setup.rs:634-643`) is the existing nil-before-recreate pattern. `SceneParams::texture_index` is the display mode (grid/day/night/blend), not a resolution. Surface textures are not tier-dependent: `resolve_texture_paths` (`sunlit-app/src/main.rs:200-206`) always names the same two files; `QualityTier` caps MSAA, preview width, and the cloud variant only (`config.rs:36-62`). There is no engine command for swapping textures at runtime. Config fields without a widget survive saves through the read-modify-write in `read_config_from_window_onto` (`ui_callbacks.rs:359-399`); fields with a widget need explicit wiring in `apply_config_to_window` and that same function, plus `defer_combobox_indices` (`ui_callbacks.rs:218-232`) for the combo index. The cloud cache lives in the same `SunlitEarth` data directory as the config, keyed by HTTP freshness only; nothing reusable validates a derived file against a local source. `memory.rs:39-46` anticipates this feature by name, and the soak test's `private_bytes` pattern (`tests/soak.rs:135-154`) is the template for a measured before/after assertion.

## Key Design Decisions

1. **The resolution is an `AppConfig` field with a widget, and a change is an engine command, not a params push.** New field `texture_resolution: u32` (8192, 4096, or 2048), default 4096, wired explicitly through `apply_config_to_window` and `read_config_from_window_onto` like every widgeted config value, with its combo index in `defer_combobox_indices` and its three call sites. It does not join `SceneParams` or the digest: it does not describe what to draw, it describes which pixels to load, and acting on it means re-decoding files and swapping GPU textures, which `push_params` cannot express. The combo's callback sends a new `EngineCommand::SetTextureResolution(u32)`. A `--texture-resolution` CLI flag overrides for one run without being written back, following the `--quality` precedent.

2. **Missing config fields take the new default, so existing installs move to 4096.** That is the requested behavior: 4096 is the default, and whatever the user picks is what gets saved. An existing config written before this feature has no `texture_resolution` key and lands on 4096; anyone who wants the old look picks 8192 once and it persists.

3. **Downscale once on the loader thread, cache as PNG beside the cloud cache.** At 8192 the source is decoded directly and nothing is cached. At 4096 or 2048 the loader first looks for `texture_cache/<source-stem>.<width>.png` under the same resolved cache directory the cloud cache uses (so `SUNLIT_EARTH_CACHE_DIR` covers both); a sidecar TOML records the source file's size and mtime, and a mismatch invalidates. On a miss the 8K source is decoded, halved with the existing box-filter logic (8192 to 4096 to 2048 are exact 2x steps), encoded to PNG with the atomic temp-then-rename pattern the config save uses, and posted. All of it runs on the per-texture background thread that already does the decode; the engine thread never blocks on it. PNG because the image crate encodes it, losslessly, and the decode cost at startup is far below JXL's. First run at 4096 still pays one 8K decode; every later startup decodes only the cached 4096 and is faster than today.

4. **A purge destroys the texture explicitly instead of trusting drop order.** `TextureSlot` gains the `wgpu::Texture` it currently lets go out of scope, so a resolution switch can nil the slot's bind group, the day and night texture views, and the composite bind group, then call `Texture::destroy()`, which releases the allocation deterministically rather than whenever the last view drops. This mirrors `replace_render_textures`, which already nils before recreating for exactly this reason. Only then are the new loads spawned.

5. **A generation counter keeps stale decodes out of the mailbox.** A decode of the old resolution may still be in flight when the user switches. Texture load posts carry the generation current when they were spawned, and `process_decoded_textures` discards posts from an older generation, so a slow 8K decode can never overwrite its 2048 replacement.

6. **Clouds and the grid are out of scope.** The cloud variant stays tier-driven and fetched; the procedural grid stays 2048. The setting governs the two local surface textures only, and its name, `texture_resolution`, keeps clear of the existing `texture_index`, which selects the display mode.

## Success Criteria

1. The Rendering group offers 8192, 4096, and 2048; a fresh config starts at 4096; the choice survives a restart and a wallpaper save.
2. Selecting a lower resolution for the first time creates the cache files; subsequent startups at that resolution never decode the 8K sources (verifiable from the logs) and are faster than an 8K startup.
3. Switching 8192 to 2048 lowers `private_bytes`, asserted by a test that runs where the real textures are present and prints why it skipped where they are not.
4. After a switch in either direction the very next frame renders from the new textures: no stale frame, no crash, engine tests stay green.
5. `cargo test`, `cargo clippy --all-targets`, `cargo fmt --check` green on Windows and in WSL.
6. CLAUDE.md (the setting, the CLI flag, the cache) and `docs/roadmap.md` reflect the feature.

## Implementation Steps

### Step 1: Config, CLI, and UI for the resolution setting

`AppConfig::texture_resolution` with default 4096 and validation to the three allowed values on load; the `--texture-resolution` flag; the combo box in the Rendering group; `defer_combobox_indices` and its call sites; the config bridge in both directions. No engine behavior yet: the value reaches the engine at startup only.

### Step 2: Downscale-and-cache loader

The cache key (source size and mtime in a sidecar TOML), the halving pipeline on the loader thread, PNG encode with atomic rename, cache hit path, and unit tests for the key logic and invalidation against fabricated files. Startup honors the configured resolution.

### Step 3: The switch command and the purge

`EngineCommand::SetTextureResolution`, the generation counter, `TextureSlot` keeping its `wgpu::Texture`, the explicit destroy-then-reload sequence, and engine integration tests: a frame arrives after a switch, a switch mid-load never leaves a stale texture, and the command is cheap on the channel (the payload test).

### Step 4: The memory assertion

A test in the engine or soak layer that loads at 8192, snapshots `private_bytes`, switches to 2048, and asserts a real decrease with tolerance for allocator noise. Gated at runtime on the real textures being present (the same size-not-pointer check the e2e staging uses) and prints why it skips otherwise.

### Step 5: Documentation

CLAUDE.md (the new setting and flag, the texture cache, the adding-a-parameter recipe if it needs a caveat for non-shader settings) and `docs/roadmap.md`.

## Risks and Mitigations

- wgpu's allocator may not return every byte to the OS on destroy. The assertion in Step 4 measures a decrease with tolerance rather than an exact figure; if it proves adapter-flaky, it pins the platforms where it is reliable and documents the rest.
- An in-flight decode racing the switch is the one real concurrency hazard; the generation counter in Decision 5 exists for it and gets a dedicated test.
- The first run at 4096 still decodes the 8K sources once before the cache warms. Acceptable, and visible in logs.
- Existing users are silently moved from 8192 to 4096 by Decision 2. Intended, but worth one line in the README or release notes when this ships.

## Rollback Strategy

Pure app code on a feature branch, reverts cleanly by commit. Nothing migrates persisted state: a config with `texture_resolution` read by an older binary is ignored by serde, and the texture cache directory can be deleted at any time without consequence.
