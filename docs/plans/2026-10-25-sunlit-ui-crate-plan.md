# Plan: The Slint UI in Its Own Crate

## Summary

Every edit to any Rust file in `sunlit-app`, and every edit to `sunlit-core`, recompiles about 10 MB of Rust that `slint-build` generates from `ui/`. That generated code is the bulk of an incremental rebuild of the `sunlit-earth` library. This plan moves `ui/` and the `slint-build` step into a new workspace crate, `sunlit-ui`, whose library is nothing but `slint::include_modules!()`. `sunlit-earth` depends on it and re-exports it, so every path that names a generated type stays the same. A probe of exactly this change cut a debug rebuild after a one-line app edit from 14.5 s to 5.5 s.

## Stakes Classification

Low. No behavior, setting, config key, IPC line or shader changes, and the binary is built from the same sources. What changes is where the `.slint` files live, which crate runs `slint-build`, two manifests, two build scripts, four paths in one test file, and the docs that name those paths.

## What the code does today

Checked against `main` at 8024c11:

- `crates/sunlit-app/build.rs` calls `slint_build::compile_with_config("ui/main.slint", ...)` with `with_debug_info(true)`, then embeds the Windows icon resource, sets the 4 MB main-thread stack on Windows, and runs `refuse_a_foreign_target_directory`.
- `crates/sunlit-app/src/lib.rs` has `slint::include_modules!();` at the crate root. The macro includes the file the same crate's build script wrote to `OUT_DIR`, which is why it can only live in the crate that runs `slint-build`.
- The generated `main.rs` under `target/debug/build/sunlit-earth-*/out/` is 10.7 MB.
- No code in `sunlit-app` implements a trait or an inherent method on a generated type, so nothing breaks the orphan rule when the types move to another crate.
- `.slint` files reference the icon as `@image-url("../../../assets/icon/sunlit-earth.svg")`. `crates/sunlit-ui/ui/` is as deep as `crates/sunlit-app/ui/`, so these paths still resolve.
- `tests/slint_ui.rs` reads three `.slint` files by path, `include_str!("../ui/overlays/camera-panel.slint")`, `"../ui/overlays/time-bar.slint"` and `"../ui/main.slint"` (around line 5134), and one inline `slint!` block imports `KeyCap` from `"ui/overlays/common.slint"` (around line 6050). That import resolves against the crate's manifest directory.
- `crates/sunlit-app/src`, the xtask, `.github/` and the release scripts have no path into `crates/sunlit-app/ui`. Only historical documents under `docs/plans/` and `docs/reviews/` name it, and those stay as written.
- `cargo xtask bake licenses` skips workspace members and walks the shipping tree through them, so `slint` and its dependencies are reached the same way through `sunlit-ui`.
- Two unmerged branches touch `ui/`: `exp/macos-vnc` (one file, 2026-09-23) and `feature/sea-ice` (one file, 2026-10-10).

### Measurements

A throwaway worktree at f60e753 with the change below, on the Windows desktop (16 threads), debug profile, sharing one target directory so the registry dependencies were already built. Each edit appended an unused private function to `crates/sunlit-app/src/displays.rs` or `crates/sunlit-core/src/scene/sun.rs`, and each figure is from a run that followed a settled build.

| After a one-line edit in | Today | With `sunlit-ui` |
|---|---|---|
| the app, `cargo build` | 14.5 s (the `sunlit-earth` lib 11.5 s) | 5.5 s (the lib 2.1 s) |
| core, `cargo build` | 18.9 s (`sunlit-core` 3.9 s, the lib 13.4 s) | 7.8 to 8.5 s (`sunlit-core` 2.6 to 3.1 s, the lib 2.4 to 2.6 s) |
| the app, `cargo test --no-run` | 24.5 to 24.9 s | 11.0 to 13.2 s |
| core, `cargo test --no-run` | 29.7 s | 18.6 s |

The first core run of `cargo test --no-run` took 38 s on both sides and is left out as an outlier. Editing a `.slint` file still regenerates and recompiles everything, as it does today.

In the split layout, a core edit's test build is bounded by the `sunlit-earth` lib in test mode (11 s), `slint_ui` (8.8 s) and the `sunlit-core` lib in test mode (10.9 s), all running in parallel. That is why the core crate itself stays whole here (Not chosen, below).

## Key Design Decisions

### 1. `sunlit-ui` holds the `.slint` sources and nothing else

```
crates/sunlit-ui/
  Cargo.toml
  build.rs        # slint_build::compile_with_config("ui/main.slint", with_debug_info(true))
  src/lib.rs      # slint::include_modules!();
  ui/             # moved with `git mv` from crates/sunlit-app/ui, unchanged
```

`ui/` moves with `git mv` so history and rename detection follow it. No Rust logic goes into `sunlit-ui`: callbacks, models and the window bridge stay in `sunlit-app`. The crate's only dependency is `slint`, with the same `raw-window-handle-06` feature `sunlit-earth` asks for, so `cargo build -p sunlit-ui` on its own resolves the same `slint` artifact rather than a second one with a different feature set. `slint-build` moves from `sunlit-earth`'s build dependencies to `sunlit-ui`'s. The manifest takes `version`, `edition`, `license` and `repository` from the workspace, sets `publish = false` and a one-line `description`, and has `[lints] workspace = true`.

### 2. `sunlit-earth` re-exports it

`slint::include_modules!();` in `crates/sunlit-app/src/lib.rs` becomes `pub use sunlit_ui::*;`. `sunlit_earth::MainWindow` and every other generated name stay where they are, so `src/`, `tests/slint_ui.rs` and `tests/e2e.rs` need no `use` changes. `sunlit-earth` keeps its own `slint` dependency for the runtime API (`Timer`, `Weak`, `ModelRc`, the window handle).

### 3. `sunlit-earth`'s build script keeps everything but the Slint step

`refuse_a_foreign_target_directory`, the Windows icon resource and the stack size stay in `crates/sunlit-app/build.rs`. Only the two `slint_build` lines go. The guard's doc comment still holds: it runs before the binaries link, which is where it says the weight is.

### 4. The test file follows the move

The three `include_str!` paths in `tests/slint_ui.rs` become `"../../sunlit-ui/ui/..."` and the inline import becomes `"../sunlit-ui/ui/overlays/common.slint"`. Both forms compiled in the probe.

### 5. Not chosen

- **Splitting `sunlit-core`.** Its modules have cycles today (`scene` and `display::layout`, `params` and `config`, `renderer` and `assets`, `memory` and `renderer`, `wallpaper` and `engine::wallpaper_sink`, `desktop` and `wallpaper`). Most are a constant or one type and could be broken, but the measurements above show core is not the critical path, and most core edits are in modules everything else depends on, so they would rebuild nearly everything anyway. If a split is ever wanted for separation's sake, the natural seam is a platform crate (`wallpaper`, `desktop`, the OS half of `display`, the OS probes in `memory`, `thread_priority`). That is a separate plan.
- **Generating into `sunlit-app` with fewer codegen units or other compiler settings.** These would tune the cost of the 10 MB instead of keeping it out of the edit loop, and would change the profile for every crate.
- **Moving app logic into the UI crate.** The point of the crate is that nobody edits it while working on Rust code.

## Success Criteria

1. `cargo build`, `cargo test`, `cargo clippy --all-targets` and `cargo fmt --check` pass, with no new warnings under `RUSTFLAGS=-D warnings`.
2. `cargo xtask bake licenses` leaves `assets/third-party.md` and `assets/THIRD-PARTY-LICENSES.md` unchanged, and the xtask's freshness test passes.
3. A one-line app edit followed by `cargo build` takes roughly what the probe measured, well under today's 14.5 s on this desktop.
4. The app starts, the settings window, the About window (with its icon) and the tray behave as before.
5. The Linux build through WSL works with the worktree's own `CARGO_TARGET_DIR`.

## Implementation Steps

### Step 1: The crate

Create `crates/sunlit-ui/` as in decision 1, `git mv crates/sunlit-app/ui crates/sunlit-ui/ui`, and add `sunlit-ui = { path = "../sunlit-ui" }` to `sunlit-earth`'s dependencies. `members = ["crates/*"]` picks the crate up with no change to the root manifest. Commit `Cargo.lock` with the new entry, since `cargo xtask dist` and `release.yml` build with `--locked`.

### Step 2: The app

Replace the macro with the re-export (decision 2), drop the `slint_build` lines and the `slint-build` build dependency from `sunlit-earth` (decision 3), and fix the four paths in `tests/slint_ui.rs` (decision 4).

### Step 3: Checks

Run Success Criteria 1 and 2, then repeat the probe's two `cargo build` measurements and record them in this plan's Validation Record.

### Step 4: Documentation

- `docs/architecture.md`: the workspace tree gets `sunlit-ui/` with `ui/main.slint` and `ui/` under it, removed from `sunlit-app/`. The `## UI (ui/)` and `## The overlays (ui/overlays/)` headings name `crates/sunlit-ui/ui/`. One short paragraph says why the crate exists, with the before and after figures.
- `CLAUDE.md`: the Workspace block gets a `crates/sunlit-ui/` line. The shader-parameter recipe ("fourteen edits in seven files") names `main.slint` without a path and stays correct.
- `docs/app-icon.md`: `ui/main.slint` and `ui/about.slint` become `crates/sunlit-ui/ui/...`.
- `docs/roadmap.md`: one checked entry. Which section it belongs in is an open item.
- `README.md` is not touched without the user's permission. It names no `ui/` path today.

### Step 5: Other platforms

Build and run the unit tests on Linux through WSL with `CARGO_TARGET_DIR=$HOME/sunlit-target-<worktree>`, and run `cargo xtask e2e --target linux` once. macOS is only built on GitHub's runners: with the user's go-ahead, a `workflow_dispatch` dry run of `release.yml` on the branch builds and verifies all five targets and publishes nothing.

### Step 6: Validation with the user

The user runs the app and confirms Success Criterion 4.

## Gates

- After Step 2: `cargo build` and `cargo test -p sunlit-earth --test slint_ui` pass.
- After Step 3: Success Criteria 1 to 3.
- After Step 5: WSL build and the Linux e2e run pass, and the release dry run if the user approved it.
- Nothing is committed or pushed without the user's approval.

## Out of Scope

Splitting `sunlit-core` or creating a platform crate; any change to the `.slint` sources themselves; Slint version changes; build profile changes.

## Risks and Mitigations

- **Unmerged branches that edit `ui/`.** `exp/macos-vnc` and `feature/sea-ice` each change one file there. Git's rename detection should carry those edits across on merge or rebase, because the move changes no file content. If it does not, the edit is reapplied by hand under the new path.
- **Clippy on the generated code in a new crate.** The generated code already passes the pedantic workspace lints inside `sunlit-earth`, so `[lints] workspace = true` should not add warnings. If it does, the fix is a crate-level `#![allow(...)]` in `sunlit-ui/src/lib.rs` for the lint in question, not a lower workspace level.
- **A glob re-export hiding a name clash.** `pub use sunlit_ui::*;` gives way silently to an item of the same name defined in `sunlit-earth`'s root. The root defines only modules and `run` today, and none of them shares a name with a generated type, so the compile in Step 2 is the check.

## Rollback Strategy

Revert the commit. Nothing outside the repository depends on the crate layout: the package name, binary name and archive contents are unchanged.

## Departures

1. **`docs/rendering.md` and the `ui/...` mentions inside the renamed sections keep their crate-relative paths.** Step 4 lists the documents that name `ui/` paths, and `docs/rendering.md`'s table of the fourteen edits for a shader parameter has a `ui/main.slint` row it does not mention. Every other row of that table is relative to its crate (`renderer/uniforms.rs`, `shaders/sphere.wgsl`), so `ui/main.slint` is still correct relative to `crates/sunlit-ui` and stays as written. The same holds for `ui/main.slint`, `ui/tray.slint`, `ui/icons/` and `ui/overlays/common.slint` inside `docs/architecture.md`'s UI and overlay sections, whose headings now name `crates/sunlit-ui/ui/`, and for `ui/tray.slint` in an open roadmap entry. Only the workspace tree, the two headings, `docs/app-icon.md` and `CLAUDE.md` name the crate, as Step 4 says.

## Open items

- Resolved: the roadmap entry is under "CI and distribution", right after the entry about the dev target directory, since both are about what the development build costs rather than about a user-facing defect.

## Validation Record

Step 3, on the Windows desktop in this worktree at ccdcbfe, debug profile:

- `cargo fmt --check`: clean.
- `cargo build`: finished, no warnings. `cargo test -p sunlit-earth --test slint_ui`: 140 passed.
- `cargo test --no-fail-fast`: 1942 passed, 0 failed, 20 ignored (the e2e cases). The first whole-workspace run failed five `assets::cloud_fetcher` unit tests in `sunlit-core`, because the shell the run used had `SUNLIT_EARTH_CLOUD_URL=http://127.0.0.1:8765/clouds_processed.jpg` set, which those tests read through `env_override`. That is the environment, not this change: `sunlit-core` does not depend on `sunlit-ui`. The run above had the variable unset.
- `RUSTFLAGS="-D warnings" cargo clippy --all-targets`: finished with no warnings or errors.
- `cargo xtask bake licenses`: 528 third-party crates, 17 license texts; `git status` shows `assets/third-party.md` and `assets/THIRD-PARTY-LICENSES.md` unchanged. `the_committed_bake_matches_a_fresh_one` and `the_committed_list_names_the_shipping_crates_and_no_others` passed in the run above.
- Rebuild after appending an unused private function, each from a settled build and reverted with `git checkout` of the file: in `crates/sunlit-app/src/displays.rs`, `cargo build` took 5.33 s and 5.14 s (the probe measured 5.5 s, today's layout 14.5 s); in `crates/sunlit-core/src/scene/sun.rs`, 7.48 s and 8.16 s (probe 7.8 to 8.5 s, today 18.9 s).

Step 5:

- WSL, Ubuntu 22.04, `CARGO_TARGET_DIR=$HOME/sunlit-target-sunlit-ui-crate`: `cargo build` finished in 2 min 53 s from a fresh directory; `cargo unit` passed 684 (`sunlit-core`), 180 (`sunlit-earth`), 0 (`sunlit-ui`) and 683 (xtask); `cargo test -p sunlit-earth --test slint_ui` passed 140. `cargo sweep --maxsize 8GB` then ran over every `sunlit-target-*` directory, and `--maxsize 10GB` over `~/sunlit-target`.
- `cargo xtask e2e --target linux`: KDE Plasma on X11, 19 passed, 0 failed, the guest exiting 0 after 516 s; `cargo xtask vm down all` afterwards.
- The `release.yml` dry run for macOS was not run: it needs the user's go-ahead.
