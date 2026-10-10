# Plan: Config File Versions and Migrations

## Summary

`config.toml` gains a schema version, `version`, as a top-level integer. A file without one is version 0, which is every file any release up to and including 0.2.2 wrote. On load, the `[sunlit.earth]` table is parsed as a raw `toml::Table`, passed through every migration step between the file's version and the build's, and only then deserialized into `AppConfig` and sanitized. A migration step is a plain function over the table, so it can rename a key, change a type or move a default; the build's version is the number of steps, so adding a step cannot leave the version behind.

Moving existing users to a new value comes in two kinds, each a helper a step calls. `change_default` replaces a value only when it is the one the old default wrote, so a choice the user made survives. `override_value` sets the value whatever the file held. The first step, 0 to 1, ships with 0.3.0: it moves `texture_resolution` from 4096 to 8192 with `change_default`, since the seasonal textures made the surface cheap enough that the widest setting is the better default, and sets `sample_count` to 2 with `override_value`, since MSAA did not get cheaper and every install should start from the new value.

The migrated file is written back once, by the first load from the standard path, with the original kept beside it as `config.v0.toml`.

## Stakes Classification

Medium. Every existing install runs the first migration on its first start of 0.3.0, and a mistake there either resets settings people chose or misses the ones the change is for. The code is one new module of perhaps 150 lines plus tests, and the format change is backward compatible: a 0.2.x build reading a migrated file drops the unknown `version` key and reads valid values.

## What the file holds today

These facts decide the design, and each was checked against the code on `feature/seasonal-textures`:

- Every save writes every field. `save_config_to` serializes the whole `AppConfig`, and `read_config_from_window_onto` builds it from the window over `..stored.clone()`. A file that has been saved once holds a value for every key, whether or not anyone touched it. The file therefore cannot tell "the user picked 4096" from "4096 was the default when this file was written".
- The file is written on Set as Wallpaper, on auto-refresh changes, on display plan changes and with the window geometry, so practically every install that ran has a full file.
- The stored `sample_count` is the combo box's entry, not the configured value: `read_params_from_window` reads `aa_counts[aa_index]`. In 0.2.x the default of 8 was shown as the highest count the adapter and tier allowed, so an untouched install wrote 8 on an adapter with 8x at the high tier, 4 on one whose highest is 4 or at the medium tier, and 1 at the low tier.
- Every release so far (0.1.0 through 0.2.2) shipped `DEFAULT_TEXTURE_RESOLUTION = 4096` and `sample_count: 8`. This branch changes them to 8192 and 2.
- A key this build does not know is dropped on read and gone after the next save. A file that does not parse as `ConfigFile` loads as `AppConfig::default()` and is overwritten by the next save. A value out of range is repaired by `AppConfig::sanitize`.
- `docs/architecture.md` says "the default is 8192, so an install whose config predates the setting moves to the full width". That holds only for a file without the key, which no saved 0.2.x file is. This plan makes the sentence true for saved files too, and the sentence gets corrected with it.

## Key Design Decisions

### 1. An integer schema version at the top of the file

```toml
version = 1

[sunlit.earth]
longitude = 10.0
# ...
```

An integer that counts schema changes, not the app's semver. Most releases do not change the config, prereleases would make version comparison awkward, and an integer is what an ordered list of steps indexes. It sits at the top level rather than inside `[sunlit.earth]` because it describes the file, not a setting: `AppConfig` stays the set of things the window round-trips, and `read_config_from_window_onto` never has to know about it. `ConfigFile` gains the field first, before the `sunlit` table, so the serializer emits it above the table.

The name is plain `version`, as in `Cargo.lock`'s top-level format version and the per-section `version = 2` of cargo-deny's `deny.toml`. It counts the file's format, not the app's releases, and `architecture.md` says so. A hand edit that sets it to the app's version, such as `"0.3.0"`, is a damaged number under decision 2.

### 2. Which version a file is

- No file: a fresh install. Defaults, no migration, nothing written, as today.
- A file without `version`: version 0.
- A `version` that is not a non-negative integer: warn and treat it as 0. Every step is written to be harmless on a file it does not apply to (decision 4), so running them all is the safe reading of a damaged number.
- A file at the build's version: no migration.
- A file above the build's version: decision 7.

### 3. Migrations run on the raw table, before serde

The steps operate on the `[sunlit.earth]` `toml::Table` after `toml::from_str::<toml::Table>` and before `Table::try_into::<ConfigFile>()` (present in the `toml` 0.9.12 the workspace uses). A typed migration on `AppConfig` would be simpler for a value change but cannot handle a renamed key, which serde has already dropped, or a changed type, which fails the whole file and resets every setting. The raw table handles all three the same way, and each step is pinned by a test that loads a fixture file, which is where the type safety comes back.

```rust
// crates/sunlit-core/src/config/migrate.rs

/// One step from the version at its index in `MIGRATIONS` to the next.
type Migration = fn(&mut toml::Table, &mut Vec<Change>);

const MIGRATIONS: [Migration; 1] = [v0_to_v1];

/// The version this build writes, and the one a file reaches after loading.
pub(crate) const CONFIG_VERSION: u32 = MIGRATIONS.len() as u32;

fn v0_to_v1(earth: &mut toml::Table, changes: &mut Vec<Change>) {
    change_default(earth, "texture_resolution", Value::Integer(4096), Value::Integer(8192), changes);
    override_value(earth, "sample_count", Value::Integer(2), changes);
}
```

`CONFIG_VERSION` derived from the array length means that adding a step bumps the version and nothing else has to be remembered. The cast needs an `#[expect(clippy::cast_possible_truncation)]` with a reason, as `slint_index` has.

A step receives the `[sunlit.earth]` table, since that is all the file holds. A file without that table, or with something other than a table under it, skips the steps and fails or succeeds in deserialization exactly as it does today.

### 4. Two kinds of value migration

A step moves values with one of two helpers, and which one is a decision made per key when the step is written.

```rust
/// Move `key` from the default it had to the one it has now, for a file that
/// still holds the old one.
fn change_default(earth: &mut toml::Table, key: &str, old: Value, new: Value, changes: &mut Vec<Change>);

/// Set `key` to `new` whatever the file holds.
fn override_value(earth: &mut toml::Table, key: &str, new: Value, changes: &mut Vec<Change>);
```

`change_default` replaces the value when it equals `old` and leaves it alone otherwise, a value of another TOML type included. A missing key needs nothing, because serde fills it with the current default. It is for a default that is better for most people but where a different value is a legitimate choice someone may have made. Since the file cannot distinguish an untouched default from a deliberate choice of the same value (see above), it moves everyone who has the old default, including the few who picked it on purpose, and leaves everyone who picked something else.

`override_value` writes `new` whatever the key held, and inserts it when the key is missing, so after the step the file holds `new` in every case. It is for a value the project wants every install to start from again: a setting whose cost or meaning changed so that the earlier choice, whoever made it, was made on information that no longer holds. It overrides deliberate choices by design, so the release notes name every key a step overrides.

Each replaced value is recorded as a `Change { from_version, key, from: Option<Value>, to }`, with `from` empty for an inserted key and no entry when `override_value` finds the value already there, and logged at `info` when the load finishes, so a log from a user's first start of 0.3.0 says what moved.

### 5. Migration 0 to 1

| Key | Kind | Old default | New value | What happens to other values |
|---|---|---|---|---|
| `texture_resolution` | `change_default` | 4096 | 8192 | 8192 and 2048 stay |
| `sample_count` | `override_value` | 8 | 2 | every value becomes 2 |

The texture resolution is a memory budget, and a user who chose 2048 to save memory chose it for a reason the new textures do not remove, so only the old default moves. MSAA is overridden because the stored value does not say what anyone chose: an untouched 0.2.x install wrote 8, 4 or 1 depending on the adapter and the tier (see above), and a chosen 4x or 8x was chosen when MSAA was a smaller share of the app's memory than it is now that the textures shrank. Everyone starts at 2x and can raise it in the Rendering group.

### 6. Write back once, from the standard path, with a backup

`load_config_from(path)` stays pure: it migrates in memory and writes nothing. That is what `render --config <path>` uses, and a headless render must not rewrite a file somebody passed it.

`load_and_upgrade_config()`, the startup load of the standard path in `app.rs` (departure 4; the plan first gave this to `load_config()`), writes back when the file it read was below `CONFIG_VERSION`:

1. Copy the original bytes to `config.v{from}.toml` beside it, unless that file already exists, so the backup is always the first file of that version this install had.
2. Write the migrated, sanitized config through `save_config_to`, which stamps `CONFIG_VERSION`.

It runs once, at startup, before the window and the IPC listener exist. Later loads in the same run (`read_config_from_window`, `ipc.rs`, `engine_client.rs`, Reset) go through `load_config()`, which writes nothing, and find a current file. If the write fails, every later load migrates the same file again in memory and gets the same answer, so a read-only config directory costs a warning per load and nothing else. Two processes migrating at once each write the same content through `write_toml`'s rename.

A file that was stamped but whose steps changed nothing is still written back, so the steps do not run again, and still backed up, which costs one small file and keeps the rule simple.

### 7. A file from a newer build

A file whose `version` is above `CONFIG_VERSION` was written by a newer build that the user then downgraded from. It loads best effort, exactly as today: no steps run, keys this build does not know are dropped, missing ones take this build's defaults, and a warning names both versions. Nothing is written at load.

When this build saves, it writes its own `CONFIG_VERSION`, not the higher one. The version describes the keys that are in the file, and the file this build writes holds this build's keys. If the newer build's next step renamed a key, the newer build then finds the old name again and migrates it, which keeps the downgraded build's edits. The cost is that the steps above the older build's version run again after a downgrade and upgrade round trip: a `change_default` moves a file that holds the old default again, and an `override_value` resets its key even if the user set it after the first migration. Downgrades are rare enough in a beta that this is accepted rather than designed around.

Builds before this change (0.2.x and earlier) drop the key on save, so a round trip through one of them leaves a version 0 file, which migrates from 0 again. Its values are valid in both directions: 8192 and 2 are on offer in 0.2.x.

### 8. `sanitize` stays the last word

The order on load is parse, migrate, deserialize, sanitize. Migrations bring a file to the current shape; `sanitize` repairs values that are out of range whatever version wrote them, which is a different job. A step never needs to clamp, and `sanitize` never needs to know about versions.

### 9. Not chosen

- Writing only the keys that differ from the defaults, so a changed default reaches every user who never touched the setting without a migration. It would make the full-file heuristic unnecessary in the future, but it changes what users see when they open the file (a handful of keys instead of all of them), every default change would then move users silently whether or not anyone decided that it should, `custom_year`'s default is the current year and would drift, and `quality_tier`'s default depends on the build profile. The existing files would still need this plan's migration first.
- Tracking per key whether the user set it, in a second table. Same benefit, a second source of truth in the file, and the window has no notion of "touched" to feed it.
- The app's semver as the version (decision 1).
- A snapshot test of `AppConfig::default()` that fails whenever a default changes without a migration. It would catch the forgotten migration, but it is exactly a test of constants, which `CLAUDE.md` rules out: changing a default must not break a test. The safeguard is the documentation in Step 4 instead.
- Preserving unknown keys across a downgrade (a flattened `toml::Table` of extras on `AppConfig`). Useful, separate from versioning, and not needed for this change.
- Backing up a file that does not parse before the next save overwrites it. Also useful and also separate.

## Success Criteria

1. A version 0 file holding `texture_resolution = 4096` and `sample_count = 8` loads as 8192 and 2, and after the startup `load_and_upgrade_config()` the file on disk holds `version = 1` with those values and `config.v0.toml` holds the original bytes.
2. A version 0 file holding 2048 and 4 loads as 2048 and 2, a version 0 file without `sample_count` loads as 2, and every other key in a full 0.2.2 file loads with the value the file holds.
3. A version 1 file holding 4096 and 8 loads as 4096 and 8: a choice made after the migration is kept by both kinds.
4. A file with `version` above the build's loads without migration, logs a warning and is not written at load.
5. `load_config_from` never writes, and `render --config <path>` leaves the file's bytes unchanged.
6. Every save writes `version = CONFIG_VERSION` above `[sunlit.earth]`.
7. A file written by this branch loads in v0.2.2's code with every value intact (checked by reasoning and by the existing `deserialize_unknown_fields_ignored`, since 0.2.2's `ConfigFile` has no `deny_unknown_fields`).
8. `cargo test`, `cargo clippy --all-targets` and `cargo fmt --check` pass.

## Implementation Steps

### Step 1: The version and the step list

New file `crates/sunlit-core/src/config/migrate.rs` with `Migration`, `MIGRATIONS`, `CONFIG_VERSION`, `Change`, `change_default`, `override_value`, `v0_to_v1`, and one entry point:

```rust
/// Bring a parsed file up to `CONFIG_VERSION`. Returns the version it was at
/// and what changed.
pub(super) fn migrate(doc: &mut toml::Table) -> Migrated;
```

`migrate` reads `version` (decision 2), returns early above `CONFIG_VERSION` (decision 7), otherwise runs `MIGRATIONS[from..]` over `doc["sunlit"]["earth"]` when that path is a table, and sets `doc["version"]` to `CONFIG_VERSION`.

In `config/mod.rs`, `ConfigFile` gains `version: u32` as its first field, `save_config_to` sets it to `CONFIG_VERSION`, and `load_config_from` becomes parse to `toml::Table`, `migrate`, `try_into::<ConfigFile>()`, `sanitize`. The parse and deserialize failures keep their current warnings and fall back to defaults as today. `ConfigFile::default()` should carry `CONFIG_VERSION`, so it gets a hand-written `Default` instead of the derive.

### Step 2: Write-back and backup

A private `load_and_upgrade(path) -> AppConfig` in `config/mod.rs` does what `load_config_from` does and then decision 6's backup and write when the file was below `CONFIG_VERSION` and parsed. `load_and_upgrade_config()` calls it (departure 4); `load_config()` and `load_config_from` do not. The backup name comes from one function, `backup_path(path, version)`, which turns `config.toml` into `config.v0.toml` and, for an override path from `SUNLIT_EARTH_CONFIG` such as `custom.toml`, into `custom.v0.toml`.

### Step 3: Tests

In `config/migrate.rs` and `config/mod.rs`, using `ScratchDir` as the existing file tests do:

- A fixture `crates/sunlit-core/src/config/fixtures/v0.2.2.toml`: the file a 0.2.2 release build writes on its first Set as Wallpaper, built from v0.2.2's `AppConfig::default()` with `quality_tier = "high"` and window geometry, so it holds every key a 0.2.2 file holds. Loading it gives `AppConfig::default()`'s `texture_resolution` and `sample_count`, and every other key the fixture's own value. The assertion compares against the current defaults rather than 8192 and 2, so it keeps passing when a later step moves those defaults again, as long as that step exists; that is the behavior being tested.
- Success criteria 1 to 6, one test each.
- `change_default` leaves a missing key missing, a different value alone, and a value of another TOML type (`4096.0`) alone.
- `override_value` replaces a different value, a value of another TOML type and a missing key, and records no `Change` when the file already holds the new value.
- A `version` of `-1`, `"one"` and `1.5` each load as version 0 with a warning, and a value above `CONFIG_VERSION` loads without migration.
- The backup is not overwritten by a second upgrade of a file at the same version, which is what a downgrade round trip produces.
- `serde_round_trip`, `save_and_load_round_trip` and `config_file_contains_sunlit_earth_table` keep passing; the last gains the assertion that `version` precedes the table.

The e2e case `test_across_screens_writes_what_this_desktop_can_hold` writes a version 0 config with only `display_mode`. It now gets migrated and stamped at startup and a `config.v0.toml` appears beside it in the isolated state directory. Neither affects what the case checks, but the implementer runs `cargo e2e` once (or `cargo xtask e2e --target linux`) to confirm, since `process.rs` changes on this branch touch the isolated config path.

### Step 4: Documentation

- `docs/architecture.md`, in the config paragraph around `QualityTier`: a short section "Config file versions" with decisions 1, 3, 4, 6 and 7 in two paragraphs, and the checklist for changing a default: decide whether existing users should follow it and how. Users who kept the old default follow: a step with `change_default` from the old value to the new. Every user follows, whatever they set: a step with `override_value`, named in the release notes. Nobody follows: change the default alone, and only new installs and files without the key get it. The same section explains that a rename or a type change needs a step too, since serde drops the old key.
- `docs/architecture.md`, line 130's sentence on the 8192 default: say that saved files at the old 4096 move through migration 0 to 1.
- `docs/roadmap.md`: an entry for the versioned config with the link to this plan.
- `CLAUDE.md`, under Key constraints, one line: changing an `AppConfig` default that existing users should follow, renaming a key or changing its type takes a step in `config/migrate.rs`, with `change_default` or `override_value` for a default; `docs/architecture.md` has the rule. Agreed with the user on 2026-10-10.
- README is not edited. Its "Back it up" sentence under the settings file could mention `config.v0.toml`; that is the user's call.
- The 0.3.0 release notes say that a texture resolution of 4096 moves to 8192, that anti-aliasing is reset to MSAA 2x for everyone, and that both are in the Rendering group.

## Out of Scope

- A notice in the settings window listing what a migration changed. The `Change` list exists, so a later change can show it; this plan logs it.
- Preserving unknown keys across a downgrade, and backing up an unparsable file (decision 9).
- Versioning the cloud cache's `meta.toml`, which is a cache and is rebuilt when it does not match.

## Risks and Mitigations

| Risk | Mitigation |
|---|---|
| A user who chose 4096 on purpose is moved, and every user's MSAA choice is reset. | The first is indistinguishable from the old default (see "What the file holds today"), the second is the intent of `override_value`; release notes name both changes, the log records them, the backup holds the old file. |
| A step writes a value of the wrong TOML type and the whole file then fails to deserialize, resetting every setting. | Steps write `toml::Value`s built from literals of the field's type; the fixture test deserializes the result of every step against the real `ConfigFile`. |
| Someone changes a default and forgets the step. | The architecture checklist and the `CLAUDE.md` line; a test cannot catch it without testing constants (decision 9). |
| The write-back fails on a read-only directory. | Each load migrates in memory again with the same result; `write_toml` already warns rather than fails. |
| A stale backup from an earlier version confuses someone restoring by hand. | The version is in the name, and only the first file of each version is kept. |

## Rollback Strategy

Reverting the change leaves stamped files on users' machines. A build without this code reads them as it reads any file: `version` is an unknown key and is dropped on the next save, and the migrated values are valid in every release since 0.1.0. Nothing else is needed.

## Departures

1. A step receives a `ChangeLog` rather than a `&mut Vec<Change>`. `Change` carries `from_version`, and the helpers have the signature the plan gives them with no version in it, so with a bare `Vec` either the helpers would push a placeholder version that `migrate` patches afterwards, or every helper would grow a version parameter that every step passes by hand. `ChangeLog` is the `Vec` plus the running step's version, which `migrate` sets once per step; a step and its helpers still see one `&mut` recorder, and `Migration` is `fn(&mut toml::Table, &mut ChangeLog)`.

2. `ConfigFile::version` is `#[serde(skip_deserializing)]`. `migrate` is the one reader of the version, from the raw table, and a file from a newer build is left as it is, so a `version` too large for a `u32` would otherwise fail deserialization and reset every setting, which decision 7 says must not happen. Skipped on the way in, the field takes `ConfigFile::default()`'s `CONFIG_VERSION`, and nothing reads it after loading.

3. When the backup cannot be written, the migrated file is not written either. Decision 6 orders the two but does not say what a failed backup means. Skipping the write-back does not protect the original for long: the first ordinary save, of a setting, of the display plan or of the window geometry at close, writes the stamped file over it with no backup anywhere. What it buys is that the loss is not the startup's doing and is no worse than writing anyway; the run loads the migrated values in memory either way, and a later start tries the backup again if no save has happened since.

4. The write-back moved out of `load_config()` into a separate public entry point, `config::load_and_upgrade_config()`, which `app.rs` calls once for its startup load (every mode except `render --config`, which stays on `load_config_from`). `load_config()` is a pure load again, and every other caller (`ipc.rs`, `engine_client.rs`, `ui_callbacks.rs`, `window_geometry.rs`) stays on it. With the write-back in `load_config()`, any later load could write: after a failed startup backup, a `displays` query on the IPC listener thread could back up and write the earlier migrated values over a setting the UI thread had just saved, through the same `config.toml~` temporary name. At startup the window and the IPC listener do not exist yet, so the only write a load makes happens before anything else can write the file, and nothing races the UI's saves.

## Open items

- The 0.3.0 release notes must say that a texture resolution of 4096 moves to 8192, that anti-aliasing is reset to MSAA 2x for everyone, and that both are in the Rendering group. `CHANGELOG.md` has no Unreleased section and its entries are written at release time, so this item is where the reminder lives until then.

## Validation Record

Round 1: 0 major, 5 minor.

1. Departure 3 overstated what skipping the write-back after a failed backup protects. Reworded: the first ordinary save still replaces the original.
2. `load_config()` wrote from whatever thread called it, the IPC listener included, unserialized against the UI thread's saves. Fixed by departure 4: the write-back is `load_and_upgrade_config()`, called once at startup in `app.rs`, and `load_config()` writes nothing.
3. The release notes item of Step 4 was neither done nor recorded. Recorded under Open items.
4. `a_version_that_is_not_a_non_negative_integer_migrates_from_0` asserted the literal value step 0 writes. It now asserts the version migrated from and that step 0 recorded changes.
5. (a) Departure 2's reason was tested only at the `migrate` level. Added `a_version_too_large_for_the_build_does_not_reset_the_settings`, which loads a file with a version above `u32::MAX` through `load_config_from` and checks a non-default value survives. (b) The warnings for a damaged or newer version are not asserted: the repository has no log-capture helper, and the tests assert the classification (`FileVersion::Damaged`, `from_version` above `CONFIG_VERSION`) that decides the warning instead. Declined.
