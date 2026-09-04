<!-- Reviewer notes for docs/reviews/2026-09-04-code-quality-review.md. Line numbers refer to commit 3046327. "The brief" is the shared review instruction; "the maintainer's rules" are the project's comment conventions. Runtime claims here are reasoned from the code; the measured figures are in test-timing.md. -->

# Review: configuration and memory instrumentation (`sunlit-core`)

Files: `crates/sunlit-core/src/config.rs`, `memory.rs`, `memory_report.rs`. All three read in
full. No file in the repository was modified and no cargo command was run, so every clippy claim
below is marked as unverified where it depends on the linter actually firing.

## 1. Summary

The code in these three files is correct and defensive; I found no reachable panic, no unbounded
growth and no leak. The problems are structural and in the tests.

The single largest finding is that **16 of config.rs's 73 tests pin literal defaults**, which the
project's own rule forbids, and that **8 more re-test "a missing field falls back to its default"
one field group at a time** when a single `assert_eq!` against `AppConfig::default()` would cover
all of them with more coverage. config.rs would go from 73 tests to about 45 while asserting
strictly more. memory.rs has one test whose body is algebraically identical to another's
(`memory.rs:806` vs `memory.rs:786`) and one that pins the budget to exactly 3 GiB.

memory_report.rs's module doc says the rendered numbers are "free to change"; four of its tests pin
whole rendered sentences including the numbers. That contradiction should be settled in the tests'
favor or the doc's, not left as is.

Structurally: window-geometry validation (config.rs:561-671, a Win32 `unsafe` block included) has
nothing to do with TOML and duplicates a monitor query `display.rs` already has on both platforms.
Four places in the crate independently spell `dirs::data_local_dir()?.join("SunlitEarth")`.

Comment density is not the outlier the maintainer's complaint suggests (`wgpu_init.rs` is denser
than any of these). The problem is kind, not volume: measurements, bug history and "why this test
exists" notes in doc comments.

## 2. Metrics

| | config.rs | memory.rs | memory_report.rs |
|---|---|---|---|
| Total lines | 1616 | 859 | 641 |
| `mod tests` lines | 932 (685-1616) | 359 (501-859) | 291 (351-641) |
| Non-test lines | 684 | 500 | 350 |
| Non-test comment lines / code lines | 213 / 418 (0.51) | 190 / 262 (**0.73**) | 78 / 246 (0.32) |
| Comment blocks >= 4 lines | 22 | 27 | 7 |
| of those: keep / move / delete | 11 / 4 / 7 | 16 / 5 / 6 | 6 / 1 / 0 |
| Tests | 73 | 23 | 17 |
| Tests carrying a doc comment | 8 | 10 | 10 |
| Functions over 80 lines (non-test) | 0 | 0 | 1 (`fmt`, 93) |
| `#[allow]` sites | 7 | 4 | 2 |
| `pub` items used only inside `sunlit-core` | 3 | 4 | 0 |

Crate baseline for the comment ratio (non-test comment lines / code lines), for fairness:
`params.rs` 0.19, `scene/camera.rs` 0.33, `display.rs` 0.52, `wgpu_init.rs` 0.97. memory.rs at 0.73
is high but not the crate's worst.

`pub` items only used in-crate, by name (verified by grepping `crates/sunlit-app`,
`crates/sunlit-core/tests`, `crates/xtask`):

- `config.rs`: `SUN_FLARE_MAX` (:75), `resolve_texture_resolution` (:99), `config_path` (:447).
  All three have zero callers outside `config.rs` itself.
- `memory.rs`: `metrics_path` (:392), `append_metrics_sample` (:469), `private_bytes_budget`
  (:109) have zero callers outside `memory.rs`; `record_metrics_sample` (:480) is called only from
  `engine/mod.rs:683,856`.
- `memory_report.rs`: none can be tightened. `ProcessSection`, `CounterSection`, `AllocationGroup`
  and `AllocatorSection` are named nowhere outside the module but are reachable through
  `MemoryReport`'s public fields, so they must stay `pub`. Only `MemoryReport` and
  `ExpectedTexture` are named externally (`sunlit-app/src/engine_client.rs:18`,
  `sunlit-core/src/renderer/mod.rs:23`, `tests/engine.rs:2462`).

## 3. Findings

### A. Commentary

**A1 (medium) `config.rs:588-604`: 17-line doc comment, about 5 lines of it load-bearing.**
The X11 `INT16` argument ("X11's core protocol carries window coordinates as `INT16`, so on that
display server -32768..=32767 is the whole of what a position can express") is a real, non-obvious
unit convention and earns its place. "That is the precise check the roadmap paired with the
wallpaper work, and it arrives with it" is roadmap history and belongs in a commit message. The
middle paragraph re-derives why the coarse fallback exists, which `plausible_coordinates`'
own doc at :628-634 then says again. Recommendation: cut to the `INT16` sentence plus one line
saying real outputs are preferred, and delete the duplicate half of :628-634.

**A2 (medium) `config.rs:463-466` and `config.rs:475-478`: the same four-line doc, verbatim.**
`load_config` and `load_config_from` carry byte-identical text. Both also say "Parse errors are
logged to stderr", which is wrong: they go through `tracing::warn!` (:486, :497), which in a
release build is the only level that survives `release_max_level_warn` and does not necessarily
reach stderr. Recommendation: one doc on `load_config_from`, a one-line `load_config` pointing at
it, and fix "stderr" to "logged at `warn`".

**A3 (medium) `config.rs:151-155`: restates the attribute above it, and the second half is wrong.**
"Fields use `#[serde(default)]` at the struct level so that missing fields ... are filled from
`Default::default()`" restates the `#[serde(default)]` on line 157. "and unknown fields are
silently ignored (forward compatibility)" attributes to `default` a behavior that actually comes
from the *absence* of `deny_unknown_fields`, and the forward-compatibility claim is not true on the
write path (see E2). Delete or replace with the truthful one-liner.

**A4 (medium) `memory.rs:26-29`: bug history in a module doc.** "so the tray-mode memory leak
produced no telemetry at all across ten days of uptime" is exactly the "history of what a bug was"
the maintainer's rules exclude. The half that matters ("release builds compile out `debug!` and
`info!`, so the CSV and the budget `warn!` are what survive") is one sentence. Move the rest.

**A5 (medium) `memory.rs:356-365`: a census that will rot.** "This is called from about seventeen
places, three of them per wallpaper export and one per cloud decode" is a count of call sites in a
doc comment; I count 13 `log_memory_usage` call sites in the crate today plus 6 in the app.
The reason for the early `enabled!` check ("`debug!` compiling out does not compile out the
measurement behind it") is real and should stay. Delete the census.

**A6 (medium) `memory.rs:211-216` duplicates `memory.rs:17-20`.** The `smaps_rollup`-missing
fallback is explained once in the module doc and again, at the same length, at the call site.
Keep the call-site one (it is where the `Once` lives) and cut the module-doc copy to a row in the
table already at :11-15.

**A7 (low) `memory.rs:508-518`, `716-720`, `743-753`, `654-659`: 33 lines of prose on four test
items.** `:654-659` ("This used to be Windows-only, and everything that asserts on memory ...
quietly became a no-op elsewhere") is history. `:716-720` ("3 GiB is the number the original
single-constant budget was measured against ... Nothing since has been allowed to move it") is
history and describes a test I recommend deleting anyway (F5). `:743-753` carries exact clearance
figures ("2048 clears the measurement by 121 MiB where 8192 clears it by 633") that go stale the
moment any constant moves. `:508-518` makes a real methodological point about not deriving the
expected value from the code under test, which belongs in `docs/testing.md`, not above a `const`.

**A8 (low) `config.rs:769-772` and `config.rs:1071-1075`: "why this test exists" doc comments.**
Both explain the reasoning behind a test rather than what it asserts. `:1071` at least has a usable
first line; `:769` is entirely taste narrative for a test that pins seven literals.

**A9 (keep, cited for fairness).** These earn their place and should not be touched:
`config.rs:77-82` (the halving invariant and the fact that the array is also the combo box model),
`config.rs:93-98` (why the loader must be the guard: downstream treats the value as an exact
halving), `config.rs:335-339` (`sanitize`'s invariant: serde covers missing, this covers present
and out of range), `memory.rs:11-15` (the per-OS counter table, which is the single most useful
thing in these three files), `memory.rs:238-246` (why the parser is `cfg(any(linux, test))`),
`memory.rs:204-208` (why degrading `VmHWM` to `VmRSS` keeps the column honest),
`memory_report.rs:12-14` (what is and is not a parsing contract),
`memory_report.rs:201-204` (wgpu does not re-export `AllocationReport`, so a test cannot build one),
`memory_report.rs:310-313` (why a zero texture counter prints no comparison), and all three
`// SAFETY:` blocks.

**A10 (low) `memory.rs:94-100` under-explains.** The doc justifies "2.67 MiB on the GPU ... about
the same again on the CPU" for a constant declared as `6 * 1024 * 1024`. The reader has to do the
addition and the rounding themselves. One clause ("2.67 + 2.67, rounded up") would fix it. This is
the opposite problem to the rest of the file and is worth fixing in the same pass.

### B. Length and structure

**B1 (high) `config.rs` is 1616 lines and holds four separable concerns.** Proposed split into a
`config/` directory:

| New module | Items | Lines today |
|---|---|---|
| `config/quality.rs` | `QualityTier` + impls, `TEXTURE_RESOLUTIONS`, `DEFAULT_TEXTURE_RESOLUTION`, `resolve_texture_resolution`, `find_texture_resolution_index`, `texture_resolution_at`, `find_sample_count_index` | 11-136, 673-683 |
| `config/model.rs` | `AppConfig`, `SUN_FLARE_MAX`, `default_custom_year`, `sanitize`, `anchor`, `Default` | 69-75, 138-436 |
| `config/persist.rs` | `ConfigFile`, `SunlitSection`, `ENV_CONFIG`, `config_path*`, `load_config*`, `save_config*` | 138-149, 438-547 |
| `config/window_geometry.rs` | `save_window_geometry`, `is_position_on_screen` (both cfgs), `plausible_coordinates`, `validated_window_geometry` | 549-671 |

The last one is the split worth doing even if nothing else is: 123 lines of per-OS window
placement, including the file's only `unsafe` block, sitting inside a TOML module. See B2 and D2 for
why it should probably not exist at all in its current form.

`AppConfig::default()` (359-435, 77 lines) is a flat literal and should stay one function.
No function in the non-test half exceeds 80 lines.

**B2 (medium) `config.rs:566-626`: two `is_position_on_screen` bodies where one would do.**
The non-Windows branch already delegates to `crate::display::outputs()` and `Output::overlaps`.
`display.rs` has a Windows monitor enumeration too (`display::monitors()` at :215, via
`wallpaper::enumerate_monitors`), and `Monitor` carries `x`, `y`, `width`, `height` and a `rect()`
(`display.rs:152-176`). One implementation over `display::monitors()` with the
`plausible_coordinates` fallback for `None` would replace both branches, delete the `MonitorFromRect`
`unsafe` block at `config.rs:584`, and make the rule identical on every OS. `Monitor` would need
the `overlaps` method `Output` already has (`display.rs:47-58`), which is four lines.
Risk: `MonitorFromRect` and an explicit rectangle test are not bit-identical at monitor edges, and
`enumerate_monitors` is a heavier call than `MonitorFromRect`. It runs once per launch.

**B3 (medium) `memory_report.rs:256-348`: `fmt` is 93 lines with four sections inline.**
The module doc describes the report as exactly four sections; the code does not say so structurally.
Four private `fn write_process(&self, f) -> fmt::Result` style helpers would drop `fmt` to about a
dozen lines and make the "four sections and nothing else" invariant (which
`the_report_has_exactly_four_sections` at :561 tests) visible in the code. Low risk, mechanical.

**B4 (low) `memory.rs` at 859 lines is on the edge.** Non-test is 500, which is fine. If it is
split, the seam is the budget model (`COLD_START_BYTES` through `private_bytes_budget`, :37-113,
plus its 8 tests) and the metrics CSV (:115-126 and :385-499, plus its 6 tests) as
`memory/budget.rs` and `memory/metrics.rs`, leaving the per-OS snapshot in `memory/mod.rs`. Lower
value than B1, and only worth doing alongside C1.

### C. Consistency

**C1 (medium) The crate has two conventions for per-OS code and memory.rs uses the weaker one.**

- `display/watch.rs`: a thin cfg'd dispatch at the top (:28-46) and per-OS **submodules**
  (`mod x11` at :67, `mod win32` at :255) with per-OS test modules (:489, :531).
- `wallpaper.rs`: inline `#[cfg(windows)]` on roughly 15 free functions plus one `mod shell` (:673).
- `desktop.rs`: no cfg at all; pure functions over an environment, tested everywhere.
- `memory.rs`: four separate top-level `snapshot()` definitions (:150, :201, :286, :352) with the
  Linux parse helpers interleaved between them (:248, :273).

memory.rs is the worst of the three cfg'd files to read because the four bodies of one function are
900 lines apart from each other with unrelated helpers in between. Recommendation: adopt
`display/watch.rs`'s convention crate-wide, and apply it to memory.rs first: `mod windows_impl`,
`mod linux_impl`, `mod macos_impl`, each with its `snapshot()`, and a single cfg'd `pub use` at the
top. The `#[cfg(any(target_os = "linux", test))]` on the parsers (which is deliberate and
documented at :244-246, so the parsing is unit-tested on the Windows dev machine) moves to the
`linux_impl` module declaration with only `snapshot` gated to Linux inside it.

**C2 (medium) Two `#[allow(clippy::cast_possible_truncation)]` that appear to have nothing to
allow.** `config.rs:567` and `config.rs:606` sit on the two `is_position_on_screen` bodies. Neither
body contains an `as` expression: the Windows one uses `u32::cast_signed()` (:572, :576, :577) and
the other uses `i32::try_from` (:608, :611). `cast_possible_truncation` fires on `as` casts only,
so both allows look dead, probably left behind when the casts were rewritten. Confidence: high from
reading, **unverified** because I did not run clippy. Related: the repo has 224 `#[allow]` and 2
`#[expect]`. `#[expect]` would have caught both of these automatically, since an unfulfilled
expectation is itself a warning and CI builds with `-D warnings`. Recommendation: convert the cast
allows in this area to `#[expect]` and let the compiler tell you which are dead.

**C3 (low) Three cast allows could be one helper.** `config.rs:113` and `config.rs:677` both allow
`cast_possible_wrap, cast_possible_truncation` for the same `usize as i32` at the end of a
`position()` lookup (:123, :682). A `fn slint_index(i: usize) -> i32 { i32::try_from(i).unwrap_or(0) }`
removes both allows and both casts. `config.rs:127`'s `cast_sign_loss` is already guarded by an
explicit `if index < 0` and is fine as is.

**C4 (low) `config.rs:319`: a redundant serde attribute.** `#[serde(default = "default_custom_year")]`
on `custom_year` is dead: the struct-level `#[serde(default)]` at :157 already fills every missing
field from `AppConfig::default()`, which calls `default_custom_year()` at :429. The two paths are
identical, so this is dead code rather than a bug, but the test comment at `config.rs:1060`
("custom_year uses its own serde default function") documents a mechanism that is not the one in
play.

**C5 (low) Error style is consistent within the area and different from its neighbors.** All three
files use log-and-swallow: `warn!` plus a default value or an early return, never `Result`. That is
the right policy for config and telemetry and is stated at `memory.rs:426-427`. Neighbouring
`wallpaper.rs` uses `Result<_, String>` and `display.rs` uses `Option`. No change recommended
inside the area, but see E1 for the one place where swallowing costs something.

**C6 (low) Item order is consistent and conventional in all three files** (module doc, imports,
consts, types, impls, free functions, tests). `config.rs` is the exception: it has no module doc
(`//!`) at all, where `memory.rs`, `memory_report.rs`, `desktop.rs`, `display.rs` and
`display/layout.rs` all do. `config.rs` also declares `SUN_FLARE_MAX` (:75) and the texture-resolution
constants (:83, :91) between `QualityTier` and `AppConfig` rather than with the field they clamp.

### D. Duplication

**D1 (medium) Four independent spellings of the app data directory.**
`config.rs:456`, `memory.rs:401`, `assets/cloud_fetcher.rs:167` and `wallpaper.rs:58` each write
`dirs::data_local_dir()?.join("SunlitEarth")`. The first three additionally share the identical
shape `match env_override { Some(p) => PathBuf::from(p), None => data_local_dir()?.join(...) }`
(`config.rs:452-461`, `memory.rs:397-406`, `cloud_fetcher.rs:164-170`). One
`pub fn app_data_dir() -> Option<PathBuf>` in `lib.rs` next to `env_override` collapses all four,
and the four doc comments that each explain the same `%LOCALAPPDATA%` resolution collapse to one.
`wallpaper.rs` needs a `Result` wrapper over it, which is two lines.

**D2 (medium) The MiB conversion is written three ways.** `memory_report.rs:245` (`mib`),
`memory_report.rs:251` (`mib_signed`), and `memory.rs:372-374` inline three times inside
`log_memory_usage`. Each carries its own `#[allow(clippy::cast_precision_loss)]`
(`memory.rs:366`, `memory_report.rs:244`, `:250`). One shared helper removes two allow sites.

**D3 (low) Two identically shaped path tests.** `config.rs:702` and `memory.rs:544` are the same
test written twice for two paths that D1 would merge. Both use the `if let Some(path) = ...` shape,
which means they pass silently on a platform with no local data directory. If D1 lands, one test
against the shared helper replaces both.

**D4 (low) Test scaffolding repeated ten times.** `let dir = std::env::temp_dir().join("sunlit_earth_test_<name>"); let _ = fs::remove_dir_all(&dir); ... let _ = fs::remove_dir_all(&dir);`
appears at `config.rs:789, 873, 1121, 1132, 1221, 1236, 1254, 1269, 1472, 1541, 1557` and, in a
helper, at `memory.rs:531`. `memory.rs` already has the right idea (`fn metrics_test_dir(name)`);
`config.rs` does not. The crate has no `tempfile` dev-dependency
(`crates/sunlit-core/Cargo.toml` dev-deps are `approx` and `proptest` only). Adding it, or copying
`memory.rs`'s helper into `config.rs`, removes about 30 lines and the cleanup-on-panic hole: a test
that panics leaves its directory behind, and every one of these names is fixed, so two concurrent
cargo invocations over the same checkout share them.

### E. Correctness and resilience

**E1 (low) `config.rs:508-547`: `save_config` cannot fail visibly.** Every failure path in
`save_config_to` warns and returns `()`. A user whose disk is full, whose config file is read-only,
or whose profile directory is on a disconnected network share loses their settings with no feedback
in the UI. `warn!` does survive `release_max_level_warn`, so it reaches the log file, but nothing
reaches the person. For a first public release, making `save_config_to` return
`Result<(), String>` and having the app surface a failure on the explicit "save" path (not on the
window-geometry path) is cheap. Confidence: confirmed by reading.

**E2 (low-medium) `config.rs:552-559`: closing the window downgrades a config written by a newer
build.** `save_window_geometry` does load-modify-save. `load_config_from` deserializes into
`AppConfig`, which has no `#[serde(flatten)]` catch-all, so any key a future build wrote is dropped
on read; `save_config_to` then serializes only the known fields and renames over the file. So a user
who downgrades, or who runs an older build once, silently loses every setting the newer build
added. The doc at `config.rs:154-155` claims forward compatibility, which holds for reads and not
for writes. Fix, if wanted: a `#[serde(flatten)] extra: toml::Table` on `AppConfig` (needs a
`PartialEq`-compatible type, and it changes the `AppConfig { .. }` literal in about six tests), or
simply correct the doc. Confidence: confirmed by reading; I did not test a downgrade.

**E3 (low) `config.rs:538`: the atomic write is not durable and the temp path is fixed.**
`fs::write(tmp)` then `fs::rename(tmp, path)` with no `sync_all` between them. On a crash the
rename can land before the data on some filesystems, leaving a zero-length config; the next load
falls back to defaults, which is the safe direction, so this is a durability nit rather than a
correctness bug. The temp path (`config.toml~`) is also fixed, so two processes saving at once
interleave into it. In practice the app is single-instance and every `save_config` call I found
(`sunlit-app/src/main.rs:579`, `ui_callbacks.rs:238,345`) is on the UI thread of the primary
instance, so this is not currently reachable. Confidence: confirmed by reading; marked low for that
reason.

**E4 (low) `memory_report.rs:113`: an unguarded shift.** `width >> level` inside
`texture_bytes` panics in a debug build if `mip_levels > 32` (shift amount >= the width of `u32`).
The only production caller is `renderer/mod.rs:445-485`, where `mip_levels` comes from
`wgpu::Texture::mip_level_count()` and from literal `1`s, so it is bounded by the texture's own
dimensions and cannot reach 33. But `ExpectedTexture` is `pub` with `pub` fields and `bytes()` is
`pub`, so an external caller can reach it. `width.checked_shr(level).unwrap_or(0).max(1)` is a
one-line fix. Confidence: shift confirmed by reading; unreachability confirmed for the one caller I
checked.

**E5 (low, not a bug, worth a note) `config.rs:572`: an inverted RECT is doing the rejecting.**
`30i32.min(height.cast_signed())` with a `height` above `i32::MAX` yields a negative title-bar
height, so `bottom < top` and `MonitorFromRect` returns NULL under `MONITOR_DEFAULTTONULL`, which
rejects the geometry. That is the outcome you want, but it is reached by accident rather than by a
check. `validated_window_geometry` already rejects zero sizes at :659; rejecting sizes that do not
fit in `i32` there too would make the intent explicit and would let B2's unified implementation
share the same guard.

**E6 (no issue found, stated because it is the kind of thing that goes wrong here).**
`memory.rs:428-461`'s rotation keeps exactly one `.old` file and rewrites the header after a
rotation (`needs_header` is recomputed after the rename at :443); the CSV is therefore bounded at
`2 * METRICS_MAX_BYTES`. `memory_report.rs:225-231`'s `take(TOP_N).take_while(>= FLOOR)` is correct
because the vector is sorted descending first (:223), and listed plus rolled-up is exhaustive.
`resident_texture_bytes` and `milky_way_texture_bytes` use `saturating_*` throughout so
`private_bytes_budget(u32::MAX)` cannot overflow. All three `unsafe` blocks
(`config.rs:584`, `memory.rs:178`, `memory.rs:313`) carry a scoped `#[allow(unsafe_code)]` and a
`// SAFETY:` comment that makes a real argument about handle validity, buffer size and struct
layout; the macOS one at :326-340 additionally checks the kernel's written-back `count` before
reading the fields, which is the right kind of paranoia. No changes recommended.

### F. Tests

#### config.rs: 73 tests, of which I would remove or merge 28, ending at about 45

Classification: 40 behavior, 16 default-pinning, 9 serde round-trip variations, 8 forward
compatibility / migration.

**F1 (high) 16 tests pin literal defaults, which the project rule forbids.** Named:
`default_auto_refresh_is_disabled_with_5_min_interval` (:734), `default_star_brightness_is_two_times_gain`
(:741), `default_sky_fov_is_reviewed_wide_angle` (:746), `default_star_tuning_uses_balanced_profile`
(:751), `default_sun_shows_the_glare_and_a_trace_of_the_camera` (:760),
`default_horizon_is_the_measured_atmosphere_with_a_peak_on_top` (:774),
`default_moon_is_enlarged_two_and_a_half_times` (:801), `the_milky_way_defaults_to_a_fifth` (:812),
`deserialize_missing_the_milky_way_fills_the_default` (:817),
`deserialize_missing_moon_fields_fills_defaults` (:823), `deserialize_missing_sun_fields_fills_defaults`
(:831), `deserialize_missing_sunrise_band_fields_fills_defaults` (:846),
`deserialize_missing_star_brightness_uses_two_times_gain` (:853),
`deserialize_missing_sky_fov_uses_reviewed_wide_angle` (:888),
`deserialize_missing_star_tuning_uses_balanced_profile` (:894),
`deserialize_missing_auto_refresh_fields_fills_defaults` (:903).
Between them they assert about 40 float literals. Retuning any slider default breaks them, which is
the exact failure mode `CLAUDE.md` names. Recommendation: delete all 16 and replace with one
property test that every default lies inside the range the UI offers for that parameter, which is
the invariant that actually matters and which none of these currently checks.

The right pattern is already in the file: `default_values_match_camera_params` (:720) compares
against `CameraParams::default()` rather than literals, `deserialize_missing_atmo_fields_fills_defaults`
(:910) and `deserialize_missing_cloud_fields_fills_defaults` (:922) compare against
`AppConfig::default()`, `the_earth_lens_defaults_to_the_narrow_one_...` (:859) compares against
`DEFAULT_CAMERA_FOV`. Those four should stay and are the template.

**F2 (high) 8 "missing field group falls back" tests collapse to 1.** `AppConfig` derives
`PartialEq`, so `assert_eq!(toml::from_str::<AppConfig>("longitude = 42.0").unwrap(),
AppConfig { longitude: 42.0, ..AppConfig::default() })` covers every field at once, including ones
nobody remembered to list. Replaces `deserialize_missing_the_milky_way_fills_the_default` (:817),
`..._moon_fields_...` (:823), `..._sun_fields_...` (:831), `..._sunrise_band_...` (:846),
`..._star_brightness_...` (:853), `..._sky_fov_...` (:888), `..._star_tuning_...` (:894),
`..._auto_refresh_...` (:903), `..._atmo_fields_...` (:910), `..._cloud_fields_...` (:922),
`deserialize_missing_fields` (:1036), `deserialize_missing_datetime_fields_fills_defaults` (:1054)
and `load_partial_file_fills_defaults` (:1268). Thirteen tests, one assertion, strictly more
coverage. `deserialize_explicit_zero_year_stays_zero` (:1066) must stay: it is the one that checks
an explicit value beats the default function, which the merged test cannot express.

**F3 (medium) Tests to delete outright, beyond F1 and F2 (6).**
- `deserialize_invalid_toml` (:1112) tests that the `toml` crate rejects `{{{invalid`. That is the
  dependency's test. `load_corrupt_file_returns_default` (:1253) is the version that tests this
  crate.
- `build_default_is_low_in_debug_and_high_in_release` (:1426) reimplements
  `QualityTier::default_for_build`'s body (:33-39) in the test and compares the two. It is
  tautological. The half worth keeping is
  `assert_eq!(AppConfig::default().quality_tier, QualityTier::default_for_build())`.
- `quality_tier_survives_a_file_round_trip` (:1464) does through a file what
  `quality_tier_round_trips_for_every_variant` (:1451) does in memory, and the file layer is already
  covered by `save_and_load_round_trip` (:1131). Three temp-directory operations for no new
  coverage.
- `every_offered_resolution_survives_a_file_round_trip` (:1556) is the same argument, three times
  over: 3 save-and-load cycles covering what `every_offered_width_is_accepted` (:1515) and
  `loading_a_config_with_an_impossible_resolution_repairs_it` (:1540) already cover.
- `a_config_missing_the_resolution_lands_on_the_default` (:1532) is subsumed by F2's merged test
  and by `default_texture_resolution_is_one_of_the_offered_widths` (:1495).
- `find_sample_count_exact_match_8x` (:1613) is `find_sample_count_exact_match` (:1598) with a
  different index. Fold the four `find_sample_count_*` tests into one table.

**F4 (medium) Two tests are environment-dependent and can fail on a machine that is fine.**
- `validated_geometry_accepts_on_screen` (:1319) asserts `is_some()` for position (100, 100). On
  Windows that is a live `MonitorFromRect` against the real desktop; in a session with no monitor
  (a Windows CI container, a headless VM before the display driver attaches) it returns NULL and the
  test fails. The test's own comment ("On a machine with at least one monitor") acknowledges this.
  Recommendation: skip with a printed reason when `display::monitors()` is empty or `None`, the way
  the project already handles LFS-dependent tests.
- `default_custom_year_is_current` (:1047) reads the clock twice
  (`AppConfig::default()` at :1048 calls `default_custom_year`, then :1049 reads it again) and
  compares. It fails if the year rolls over between the two reads. Same pattern in `serde_round_trip`
  (:932) and `deserialize_empty_string` (:1030), which compare two separately built
  `AppConfig::default()` values. Negligible probability; worth one line of awareness rather than a fix.

**F5 (medium) memory.rs: 2 tests to delete, 1 to merge, 23 -> 20.**
- `the_moons_term_is_the_same_at_every_resolution` (:806) has a body algebraically identical to the
  first loop of `the_resident_half_is_the_textures_the_renderer_keeps` (:786). Compare :787-792
  against :807-812: both assert
  `resident_texture_bytes(w) - MOON - milky_way(w) == 3 * one_base_level * 4 / 3`. Delete one.
- `the_widest_resolution_keeps_the_budget_it_had` (:722) asserts
  `private_bytes_budget(8192) == 3 * 1024 * MIB + MOON + milky_way(8192)`, which pins
  `COLD_START_BYTES + BUDGET_HEADROOM_BYTES` to exactly 3 GiB. Retuning either constant breaks it.
  The three property tests that follow (`the_budget_grows_with_the_resolution` :730,
  `..._stays_above_a_cold_cache_first_run_...` :755, `..._is_low_enough_to_catch_a_runaway` :771)
  bound the budget from both sides without pinning it, and they are excellent. Delete :722.
- `snapshot_values_are_reasonable` (:839) uses `if let Some(snap)`, so it passes silently where
  there is no snapshot, and its "< 10 GB" assertions belong in
  `snapshot_returns_some_on_every_supported_platform` (:661), which already asserts the platform is
  supported. Merge.
- The first loop of `the_resident_half_is_the_textures_the_renderer_keeps` (:787-792) re-derives the
  function under test; the three concrete totals that follow (:797-799, 512 / 128 / 32 MiB) are the
  real assertion. Keep the tail, drop the loop.

The remaining memory.rs tests are good. The four `parse_status_bytes` tests (:677, :684, :691, :698)
each check a distinct failure mode (missing key, prefix collision, wrong unit), which is exactly
right, and they run on Windows because of the `cfg(any(linux, test))` at :247.

**F6 (medium) memory_report.rs: 6 of 17 tests assert on rendered text, and 4 of those pin whole
sentences the module doc says are free to change.** The doc at :12-14 states "Only the section names
are a contract. The numbers, their order within a line, and the row layout are free to change."
Against that:
- `the_rollup_line_is_always_there` (:575) pins `"  rest x1: 0.0 MiB"`, which is a label, a count, a
  number, a precision and the indentation.
- `a_backend_without_an_allocator_report_still_prints_the_section` (:599) pins
  `"gpu allocations: no allocator report on this backend (metal)"`, a whole sentence with its
  parenthetical.
- `a_platform_without_a_process_snapshot_still_prints_the_section` (:617) pins
  `"process: not measurable on this platform"`.
- `a_backend_without_a_texture_counter_makes_no_comparison` (:630) pins `"against 90.0 MiB measured"`.

The structural half of each is the part worth keeping: for :599 it is that four unindented section
headers are still printed (which it does assert, at :608-613) and that the third starts with
`"gpu allocations:"`. For :630 it is the *absence* of `"measured"`, which is a real structural
assertion and should stay. Recommendation: assert section prefixes and presence/absence, not full
sentences with numbers in them. `the_report_has_exactly_four_sections` (:561) is the model: it
counts unindented lines and checks four prefixes, and nothing about the numbers.

The other 11 tests (7 on `group_allocations`, 4 on `texture_bytes`) are structural, cheap and
well chosen; `a_group_exactly_at_the_floor_is_listed` (:435) and
`listed_plus_rolled_up_is_everything` (:452) in particular are the right kind of test.

**F7 (low) Runtime is not the problem in this area.** 10 config.rs tests and 4 memory.rs tests do
real disk I/O, all of it a handful of syscalls in the system temp directory; nothing sleeps, nothing
allocates large buffers, no proptest, no GPU. `CLAUDE.md` says `cargo unit` is about 1 second, which
these are consistent with. Deleting the 28 tests above is worth doing for the maintenance cost and
the default-pinning rule, not for wall time. The one real hygiene issue is D4: fixed directory names
under the shared system temp directory, not cleaned up on panic.

### G. Best practices with a real effect

**G1 (low) `config.rs:353-355`: `anchor()` clones on every call.**
`(!self.anchor_monitor.trim().is_empty()).then(|| self.anchor_monitor.clone())`. Callers
(`engine_client.rs:210` reads `.anchor_monitor` directly; the display-plan path calls `anchor()`)
are not hot, so this is correctness-neutral. An `anchor(&self) -> Option<&str>` would be the
idiomatic signature and lets the one caller that needs an owned `String` do the clone. Small.

**G2 (low) `memory.rs:436` and `:443`: two `fs::metadata` calls for one question.**
`file_len(path)` is called before the rotation check and again for `needs_header`. The second is
required only because the first may have renamed the file, so this is correct as written; caching
the pre-rotation length and setting `needs_header = true` on a successful rename would halve the
syscalls and read more clearly. Once every ten minutes, so this is a clarity point.

**G3 (low) `memory_report.rs:216-217`: `entry.count += 1; entry.bytes += size;`** wrap silently in
release. `saturating_add` costs nothing and matches the discipline `memory.rs`'s budget arithmetic
already follows. Not reachable with real allocator data.

## 4. Recommended refactors, by value over effort

| # | Refactor | Effort | Risk | Value |
|---|---|---|---|---|
| 1 | Delete the 16 default-pinning tests (F1) and merge the 13 missing-field tests into one struct equality (F2). config.rs goes 73 -> ~48 tests with more coverage. | 1.5 h | Low. `AppConfig: PartialEq` makes the merged assertion mechanical. | Highest. Removes the project's own stated anti-pattern from the file most exposed to it. |
| 2 | Delete the 6 redundant config.rs tests (F3) and the 2 duplicate memory.rs tests (F5). | 0.5 h | None. | High. |
| 3 | Loosen the 4 string-pinning tests in memory_report.rs to section prefixes and presence/absence (F6), matching what the module doc already promises. | 0.5 h | None. | High. Removes a contradiction between the doc and the tests. |
| 4 | Extract `app_data_dir()` into `lib.rs` and route config.rs, memory.rs, cloud_fetcher.rs and wallpaper.rs through it (D1). Fold `mib()` into it as a second shared helper (D2). | 1 h | Low. Four call sites, all covered by existing tests. | High. Removes 4 copies of a path and 2 `#[allow]` sites. |
| 5 | Comment pass: A1 through A8 and A10. About 60 lines out, about 4 lines in. | 1.5 h | None. | High. This is the maintainer's stated complaint and these are the concrete instances. |
| 6 | Split `config.rs` into `config/{quality,model,persist,window_geometry}.rs` (B1), starting with `window_geometry.rs`. | 2 h | Low. Pure moves plus `pub(crate)`/`pub use` bookkeeping; the tests move with their items. | High. 1616 -> four files of 200 to 500 lines. |
| 7 | Reorganize `memory.rs` per-OS code into submodules following `display/watch.rs` (C1), and adopt that as the crate convention. | 1.5 h | Low, but touches all three `snapshot` bodies. | Medium-high. Biggest readability win in the file. |
| 8 | Unify `is_position_on_screen` over `display::monitors()` and delete the `MonitorFromRect` `unsafe` block (B2), adding `Monitor::overlaps`. | 2 h | Medium. Changes real Windows behavior at monitor edges; wants an e2e pass on a multi-monitor desk before release. | Medium. One `unsafe` block fewer and one rule instead of two. |
| 9 | Split `MemoryReport::fmt` into four per-section writers (B3). | 0.5 h | None. Tests assert on the output, which does not change. | Medium. |
| 10 | Add a `tempdir` test helper (or the `tempfile` dev-dependency) and route the 11 config.rs temp-directory tests through it (D4). | 1 h | None. | Medium. |
| 11 | Tighten the 7 `pub` items with no external caller to `pub(crate)`, and delete the redundant `#[serde(default = ...)]` at config.rs:319 (C4). | 0.5 h | Low. The compiler proves it. | Medium. |
| 12 | Convert the cast `#[allow]`s in the area to `#[expect]` and remove the two that turn out to be dead (C2, C3). | 0.5 h | None. `-D warnings` in CI turns an unfulfilled expectation into a build failure, which is the point. | Medium. |
| 13 | Skip `validated_geometry_accepts_on_screen` when there is no monitor (F4). | 0.25 h | None. | Medium, if the release is ever built in a headless Windows container. |
| 14 | Return `Result` from `save_config_to` and surface an explicit-save failure in the UI (E1); guard the `i32` size cast in `validated_window_geometry` (E5); `checked_shr` in `texture_bytes` (E4). | 1.5 h | Low. E1 touches three app call sites. | Medium. Resilience polish for a public release. |
| 15 | Decide E2: either add a `#[serde(flatten)]` catch-all to `AppConfig` or correct the forward-compatibility claim at config.rs:154. | 0.5 h (doc) / 2 h (catch-all) | The catch-all touches every `AppConfig { .. }` literal in the tests. | Low-medium. Matters more after the second release than before the first. |

### Things I could not verify

- Whether `#[allow(clippy::cast_possible_truncation)]` at `config.rs:567` and `:606` is actually
  dead. I read both bodies and neither contains an `as` cast, but I did not run clippy, per the
  brief.
- Whether `Monitor::overlaps` over `display::monitors()` would classify a window identically to
  `MonitorFromRect` at monitor boundaries on Windows. `Output::overlaps` is documented as half-open
  on the far edges (`display.rs:44-46`); Win32's rule for an empty or inverted rect is not something
  I confirmed from documentation.
- Whether `validated_geometry_accepts_on_screen` actually fails in the project's Windows CI. I
  reasoned from the Win32 call and the test's own comment; I did not check the workflow files.
