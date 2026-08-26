# sunlit-core

The headless heart of Sunlit Earth: the engine thread, the wgpu renderer, the scene model, and the configuration. No Slint, no window, no event loop. The settings window is one optional client of this crate, not its owner, which is what makes every test below possible without a desktop.

The engine owns the GPU device and drives everything through injected boundaries: a `Clock` (so the soak test simulates 14 days in seconds), a `CloudSource` (so tests serve fixtures instead of hitting the network), and a `WallpaperSink` (so nothing touches the real desktop unless asked). Clients talk to it with `EngineCommand` and hear back through `EngineEvent`.

Layout: `src/engine/` is the thread and its schedule, `src/renderer/` the wgpu pipeline and offscreen render, `src/scene/` camera and sun position, `src/assets/` texture loading and the cloud pipeline, `shaders/` the WGSL. `src/params.rs` holds `SceneParams`, the single description of what to draw.

## Tests

```bash
cargo test -p sunlit-core                 # everything
cargo test -p sunlit-core --test engine   # real engine, real GPU, headless
cargo test -p sunlit-core --test soak     # mock clock, 14 simulated days
cargo test -p sunlit-core --test golden   # golden images, per-adapter references
```

The GPU tests run on the software adapter (WARP, lavapipe, or Metal) and assert invariants rather than exact pixels. See the workspace [CLAUDE.md](../../CLAUDE.md) and [docs/](../../docs/README.md) for the full testing conventions and the architecture rationale.
