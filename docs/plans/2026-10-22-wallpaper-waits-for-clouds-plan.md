# The wallpaper waits for the cloud layer

## Problem

With auto-refresh on, the first wallpaper of a run is sometimes made without clouds. It is put right on the next cycle, but it is wrong for as long as that cycle is.

The first update is armed in `start_engine` (`crates/sunlit-app/src/app.rs`) and released by `TexturesReady`. That event covers the cube faces, the floors, the night side and the mask. The Moon, the Milky Way and the clouds are excluded on purpose (`Renderer::textures_ready`), and `Engine::publish` (`crates/sunlit-core/src/engine/publish.rs`) holds back on the same set. The clouds come from a worker thread (cache read or network fetch, decode, then an upload on a later tick), so with a warm tile cache the globe is ready before the cloud map is resident, and the publish goes out without it.

`EngineEvent::CloudsSettled { drawn }` already says when the layer has its first answer. The one-shot commands (`render`, `displays --out`) wait for it. The running app ignores it (`engine_client.rs`), by decision 10 of [2026-10-10-cloud-haze-plan.md](2026-10-10-cloud-haze-plan.md): a session publishes again within the hour, and a session offline would otherwise never publish. The interval refresh and the display-change republish go through the same gate and have the same race, which matters when the interval is shorter than the startup load.

## Decisions

### 1. The engine's publish gate holds for the cloud layer

The hold lives in `Engine::publish`, so the startup update, the interval refresh, the tray, IPC, "Set as Wallpaper" and the display-change republish all get it. It reuses `wallpaper_owed`: a publish that finds the clouds pending sets the debt, returns, and `tick` asks again every tick, as it does for cubes and tiles.

The clouds are pending when all of these hold:

- the engine has a cloud worker (`self.cloud.is_some()`),
- `self.params.draws_clouds()`,
- `CloudsSettled` has not gone out (`!self.clouds_settled`).

The last condition makes the hold a first-answer wait. After the first answer, a later poll that is slow, failing or in flight never holds a publish, so a session offline for hours still publishes on schedule and a manual update never hangs on the network.

The first two conditions are the ones `waits_for_clouds` in `headless.rs` uses, for the same reason: with the layer off, or both opacities at zero, or no source, the event never comes and waiting would only time out. Params are read at each check, so switching the layer off during the wait releases the publish, and switching it on while `clouds_settled` is still false makes the next publish wait for that first answer.

An offline first run with no cached map is not slowed: the worker reports `drawn: false` right after the first failed poll (`FirstAnswer::Unavailable`), which sets `clouds_settled` and releases the hold well before the worker's first retry.

### 2. The wait is bounded

`pub const CLOUD_WAIT: Duration = Duration::from_secs(60)` beside `TILE_WAIT` in `engine/mod.rs`, public for the same reason (the tests step the mock clock over it), documented the same way. It is the figure `headless.rs` already uses for a slow 8192 px download and its decode, so a map that is merely slow still makes it in.

It is counted on the injected clock from the first time a publish is asked for while the clouds are pending, in a new field of `Engine` beside `tiles_awaited_since` and cleared wherever that one is (`report_wallpaper`). A publish that waits it out goes out without clouds, logs a `warn!` saying so and naming the wait, and the next cycle fixes it as today. Making that wallpaper again once the clouds land, the way `reexport` does for tiles, is not part of this change.

The wait does not apply on the exit path: `publish_wallpaper_at_exit` has no tick left to wait in, and it publishes with what is resident, as it does for tiles.

### 3. Placement inside `publish`

After `check_supported` and `plan_wallpaper`, beside the `cubes_pending` hold, so a platform that cannot set a wallpaper still reports that at once and does not wait. The early return at the top of `publish`, which `tick` comes back through every 50 ms, learns the clouds as a third reason to wait, so the debt stands without re-asking the sink. The log line "wallpaper update deferred until the cloud layer has settled" is an `info!` emitted from `clouds_waited_out` at the moment it starts the count, so it fires once per cloud wait whichever hold took the debt first (departure 1).

The cube hold stays first: the cloud wait is judged only when no cube is pending, and its clock starts then, so a first run that spends minutes building packs does not spend the cloud budget on them.

### 4. The explicit waits in the app and the headless commands

`run_displays` already waits for `CloudsSettled` before sending `RenderWallpaperNow`; `run_render` waits before `render_to_file`, which is an export and not a publish, so it keeps its wait. Both stay as they are. Their comments, and the comment on the `CloudsSettled` arm in `engine_client.rs`, are stale after this change and are rewritten to say what is now true. No behavior changes in the app crate.

## Tests

In `crates/sunlit-core/tests/engine/clouds.rs`, using `HeldClouds`, `CountingClouds`, `MockClock`, `RecordingSink` and the existing helpers. Assert behavior, not constants: step over `CLOUD_WAIT` by name.

1. A publish asked for while the first fetch is held does not go out; once the fetch is released and the map is resident it goes out, and the picture handed to the sink has clouds in it (compare against the same scene with the layer switched off).
2. With no cached map and a source that fails, a publish goes out on the first failed poll, well before `FIRST_RETRY`, and the event stream shows `CloudsSettled { drawn: false }` before `WallpaperSet`.
3. With the fetch held for good, a publish asked for goes out once the mock clock has advanced `CLOUD_WAIT` and not before, and carries no clouds.
4. No hold when the layer is off (`clouds_enabled` false), when both opacities are zero, and when the engine has no cloud source. Each publishes without waiting.
5. After the first `CloudsSettled`, a publish is not held by a later fetch that is in flight and held.
6. Switching the layer off while a publish waits for it releases the publish.
7. The wallpaper owed because of the clouds is published, with what is resident, when the engine shuts down.
8. The existing cases that publish with a cloud source configured keep passing. A case that now has to wait for the clouds is changed to say so, and the change is noted in the status log, not made silently.

The soak target (`tests/soak.rs`) and the app's `slint_ui` target must keep passing unchanged. If either needs an edit, that edit is a numbered departure.

## Docs

- [2026-10-10-cloud-haze-plan.md](2026-10-10-cloud-haze-plan.md), decision 10: the last two paragraphs ("The app's own publishes do not change" and "Not chosen") describe what this change reverses. Rewrite them to say what the engine now does and point here. Do not rewrite history elsewhere in that file.
- `docs/architecture.md`: "The publish" bullet (line 233 at the time of writing) gains the cloud hold and `CLOUD_WAIT`; the `CloudsSettled` description in the events bullet (line 64) gains that the engine's own publish gate reads it.
- `docs/roadmap.md`: the entry about `render` and `displays --out` (line 149) ends with "The running app's own publishes still do not wait." Update it, and add a done entry for this fix.
- `docs/testing.md`: one line on the new cases where it describes the cloud and publish cases, if a place for it exists.
- `README.md` is not edited.

## Gates

Run from the worktree root, in this order, and report the real output.

```bash
cargo fmt --check
cargo clippy --all-targets
cargo unit
cargo test -p sunlit-core --test engine
cargo test -p sunlit-core --test soak
cargo test -p sunlit-earth --test slint_ui
cargo test
```

`cargo clippy --all-targets` must be clean (pedantic is on; CI builds with `-D warnings`). `cargo test` at the end is the full workspace and is the last word. A test that skips because an asset is an LFS pointer is reported as skipped, not as passed.

## Constraints for every agent

- Work only in `C:\Workspace\rustrover\sunlit-earth\.worktrees\cloud-wait-publish` on branch `bugfix/wallpaper-waits-for-clouds`. Never touch the main checkout or the other worktrees under `.worktrees/`.
- Never run `cargo xtask vm`, `cargo xtask e2e`, `cargo xtask dist`, `cargo e2e`, anything elevated, anything through WSL, or any golden regeneration (`SUNLIT_EARTH_UPDATE_GOLDEN`). The e2e suite takes the screen over and is not a gate here.
- Do not edit `README.md`. Do not push. Do not create or touch PRs. Do not amend or rewrite commits.
- Commits: on the run branch, in reasonable chunks, with plain messages. No `Co-Authored-By` trailer or any other co-author line. Do not override the git identity.
- Code comments only where the code cannot be made clear without them, and never about history, tickets, or why a test exists. Match the surrounding comment density, which is high in this crate: the neighbors document their reasoning.
- Writing style in docs, comments, logs and commit messages: American English, no em dashes, no en dashes, avoid dashes in sentences generally, no "genuinely", "delve", "crucial", "honestly". Bold and italics only for real callouts.
- Departures from this plan are numbered, sequentially from 1, and recorded in the section below with a paragraph of reasoning.

## Departures

1. **The deferral is logged where the cloud wait starts, not where the debt is first taken.** Decision 3 had the log line follow the cube and tile holds, emitted when `wallpaper_owed` goes from false to true. When the cube hold takes the debt first, `wallpaper_owed` is already true by the time the cubes are resident and the cloud hold is first judged, so the line was never emitted for a wallpaper that waited for cubes and then for clouds, which is exactly the startup case. The count of the wait is the event the line announces and it starts once per wait, so `clouds_waited_out` now logs when its `get_or_insert_with` inserts, and the conditional `info!` on the main path is gone.

## Validation rounds

Round 1, range a056129..109c76c: 0 major, 5 minor, all fixed in 4d88093 (the code and the tests: the two resets of the wait's start now have cases that fail without them, the failed first poll case fails without the hold, the warning is judged after the export is prepared, the deferral is logged where the wait starts) and 1046fc6 (rendering.md no longer says the clouds never delay a wallpaper, and testing.md describes the new cases). The mutation checks were run against each: removing the clear in `report_wallpaper` fails `a_second_wallpaper_waits_for_the_clouds_as_long_as_the_first`, removing the clear in the cube hold fails `a_cube_that_is_not_resident_starts_the_cloud_wait_over`, forcing `clouds_pending` false and removing `!self.clouds_settled` each fail `a_wallpaper_does_not_wait_on_a_first_poll_that_failed`.

Open item the validator observed: `run_displays` waits for `CloudsSettled` in `headless.rs` and the engine's publish can then wait up to `CLOUD_WAIT` again, so the worst case for `displays --out` with a map that never arrives is about 120 s, inside `RENDER_TEXTURE_TIMEOUT`. Left as is on purpose.
