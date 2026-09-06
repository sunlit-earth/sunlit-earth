# Documentation

## Building and using the app

- [building.md](building.md) covers native dependencies, WSL builds, launch examples, and developer tooling.
- [usage.md](usage.md) covers command line options, display modes and diagnostics, app data, and environment overrides.
- [assets.md](assets.md) covers Git LFS textures, asset discovery and caching, preparation tools, and license notices.

## How the code works

- [architecture.md](architecture.md): the crate split, the headless-first engine and its clients, `SceneParams`, the renderer's resources, the app and the settings window, quality tiers, texture resolution, memory reporting, environment knobs, dependencies, constraints
- [rendering.md](rendering.md): the shaders and the draw order, then each layer of the sky (stars and planets, the Sun, the Moon, the Milky Way) and the clouds on the night side, with the measurements behind each
- [testing.md](testing.md): the test layers, the conventions every layer follows, golden images, and the hosted CI workflows
- [platforms.md](platforms.md): what each OS does today, the per-OS implementations, setting a wallpaper on Linux, and how the desktop e2e cases gate themselves
- [app-icon.md](app-icon.md): the mark, the bake, and which surface consumes which raster
- [vm-setup.md](vm-setup.md): running the desktop e2e suite and the release builds in a local VM: setup, images, interactive access, cleanup, troubleshooting
- [vm-internals.md](vm-internals.md): how `cargo xtask` is built, module by module, and why each decision came out the way it did

## Vision and history

- [project.md](project.md): project vision, goals, motivation, and technology stack
- [tech.md](tech.md): the technology survey the stack was chosen from: constraints, wallpaper APIs, astronomy libraries, the framework options
- [related.md](related.md): competitive analysis of existing satellite imagery and rendered globe apps
- [roadmap.md](roadmap.md): planned features and improvements, roughly ordered by priority, and the known defects
- [notes.md](notes.md): open issues and research topics
- [retrospective-2026-08.md](reviews/2026-08-15-retrospective.md): prototype retrospective: what worked, what failed (including the tray-mode memory leak analysis), and the plan for the next iteration
- [2026-09-04-code-quality-review.md](reviews/2026-09-04-code-quality-review.md): pre-release code quality and resilience review of `sunlit-core` and `sunlit-app`: commentary, file length, structure, duplication, test cost and value, with the per-area reviewer notes under `reviews/2026-09-04-code-quality/`

## Plans

The `plans/` folder contains research documents, implementation plans, and investigation notes produced during development. Files are prefixed with the date they were created (e.g. `2026-03-15-day-night-plan.md`).

These documents follow the Research-Plan-Implement (RPI) framework and were created with [rpikit](https://github.com/bostonaholic/rpikit), a structured methodology for Claude Code that separates work into distinct research, planning, and implementation phases.
