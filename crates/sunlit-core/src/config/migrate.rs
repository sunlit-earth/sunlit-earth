//! The config file's schema version and the steps that bring an older file up
//! to it.
//!
//! The steps run on the parsed `[sunlit.earth]` table before serde sees it, so
//! a step can rename a key or change its type as well as move a value: serde
//! would have dropped the first and failed the whole file on the second.

use toml::{Table, Value};
use tracing::{info, warn};

/// One step from the version at its index in [`MIGRATIONS`] to the next.
type Migration = fn(&mut Table, &mut ChangeLog);

const MIGRATIONS: [Migration; 1] = [v0_to_v1];

/// The version this build writes, and the one a file reaches after loading.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the number of steps, which a u32 holds"
)]
pub(crate) const CONFIG_VERSION: u32 = MIGRATIONS.len() as u32;

/// 0.3.0: the seasonal textures made 8192 cheap enough to be the default, and
/// every install starts again from MSAA 2x.
fn v0_to_v1(earth: &mut Table, log: &mut ChangeLog) {
    change_default(
        earth,
        "texture_resolution",
        Value::Integer(4096),
        Value::Integer(8192),
        log,
    );
    override_value(earth, "sample_count", Value::Integer(2), log);
}

/// One value a step replaced or inserted.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Change {
    /// The version of the step that made the change.
    pub from_version: u32,
    pub key: String,
    /// `None` when the file did not hold the key.
    pub from: Option<Value>,
    pub to: Value,
}

/// Where the steps record what they change, stamped with the running step's
/// version.
pub(super) struct ChangeLog<'a> {
    from_version: u32,
    changes: &'a mut Vec<Change>,
}

impl ChangeLog<'_> {
    fn record(&mut self, key: &str, from: Option<Value>, to: Value) {
        self.changes.push(Change {
            from_version: self.from_version,
            key: key.to_owned(),
            from,
            to,
        });
    }
}

/// Move `key` from the default it had to the one it has now, for a file that
/// still holds the old one.
///
/// A file cannot tell an untouched default from the same value chosen on
/// purpose, so both move; any other value stays, one of another TOML type
/// included. A missing key stays missing, since serde fills it with the
/// current default.
fn change_default(earth: &mut Table, key: &str, old: Value, new: Value, log: &mut ChangeLog) {
    if earth.get(key) == Some(&old) {
        earth.insert(key.to_owned(), new.clone());
        log.record(key, Some(old), new);
    }
}

/// Set `key` to `new` whatever the file holds, inserting it when it is missing.
fn override_value(earth: &mut Table, key: &str, new: Value, log: &mut ChangeLog) {
    let previous = earth.insert(key.to_owned(), new.clone());
    if previous.as_ref() != Some(&new) {
        log.record(key, previous, new);
    }
}

/// What a file's top-level `version` says.
#[derive(Debug, PartialEq)]
enum FileVersion {
    /// No `version`: every file written before versions existed.
    Unstamped,
    Stamped(u32),
    /// Anything that is not a non-negative integer.
    Damaged(Value),
}

fn read_version(doc: &Table) -> FileVersion {
    match doc.get("version") {
        None => FileVersion::Unstamped,
        Some(Value::Integer(n)) if *n >= 0 => {
            FileVersion::Stamped(u32::try_from(*n).unwrap_or(u32::MAX))
        }
        Some(other) => FileVersion::Damaged(other.clone()),
    }
}

/// What [`migrate`] did to a file.
#[derive(Debug)]
pub(super) struct Migrated {
    /// The version the file was at before the steps ran.
    pub from_version: u32,
    pub changes: Vec<Change>,
}

impl Migrated {
    /// Whether the file was below this build's version and is now at it.
    pub fn upgraded(&self) -> bool {
        self.from_version < CONFIG_VERSION
    }

    /// Log every change at `info`, so a log from the first start of a new
    /// release says what moved.
    pub fn log(&self, path: &std::path::Path) {
        if !self.upgraded() {
            return;
        }
        info!(
            path = %path.display(),
            from = self.from_version,
            to = CONFIG_VERSION,
            "migrated the config file"
        );
        for change in &self.changes {
            let from = change
                .from
                .as_ref()
                .map_or_else(|| "missing".to_owned(), ToString::to_string);
            info!(
                step = change.from_version,
                key = change.key,
                from,
                to = %change.to,
                "config value migrated"
            );
        }
    }
}

/// Bring a parsed file up to [`CONFIG_VERSION`]. Returns the version it was at
/// and what changed.
///
/// A file from a newer build is left alone and loads best effort. A damaged
/// `version` counts as 0, since every step is harmless on a file it does not
/// apply to.
pub(super) fn migrate(doc: &mut Table) -> Migrated {
    let from_version = match read_version(doc) {
        FileVersion::Unstamped => 0,
        FileVersion::Stamped(version) => version,
        FileVersion::Damaged(value) => {
            warn!(version = %value, "the config file's version is not a number, migrating from 0");
            0
        }
    };
    if from_version > CONFIG_VERSION {
        warn!(
            file_version = from_version,
            build_version = CONFIG_VERSION,
            "the config file was written by a newer build, loading what this build knows of it"
        );
        return Migrated {
            from_version,
            changes: Vec::new(),
        };
    }

    let mut changes = Vec::new();
    if let Some(Value::Table(sunlit)) = doc.get_mut("sunlit")
        && let Some(Value::Table(earth)) = sunlit.get_mut("earth")
    {
        for (version, step) in (0..).zip(MIGRATIONS).skip_while(|(v, _)| *v < from_version) {
            step(
                earth,
                &mut ChangeLog {
                    from_version: version,
                    changes: &mut changes,
                },
            );
        }
    }
    doc.insert(
        "version".to_owned(),
        Value::Integer(i64::from(CONFIG_VERSION)),
    );
    Migrated {
        from_version,
        changes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn earth(source: &str) -> Table {
        toml::from_str(source).unwrap()
    }

    fn apply(step: impl FnOnce(&mut Table, &mut ChangeLog), table: &mut Table) -> Vec<Change> {
        let mut changes = Vec::new();
        step(
            table,
            &mut ChangeLog {
                from_version: 0,
                changes: &mut changes,
            },
        );
        changes
    }

    fn default_4096_to_8192(table: &mut Table, log: &mut ChangeLog) {
        change_default(
            table,
            "texture_resolution",
            Value::Integer(4096),
            Value::Integer(8192),
            log,
        );
    }

    fn override_to_2(table: &mut Table, log: &mut ChangeLog) {
        override_value(table, "sample_count", Value::Integer(2), log);
    }

    // --- change_default ---

    #[test]
    fn change_default_moves_the_old_default_and_records_it() {
        let mut table = earth("texture_resolution = 4096");
        let changes = apply(default_4096_to_8192, &mut table);
        assert_eq!(table, earth("texture_resolution = 8192"));
        assert_eq!(
            changes,
            [Change {
                from_version: 0,
                key: "texture_resolution".to_owned(),
                from: Some(Value::Integer(4096)),
                to: Value::Integer(8192),
            }]
        );
    }

    #[test]
    fn change_default_leaves_a_missing_key_missing() {
        let mut table = earth("sample_count = 4");
        let changes = apply(default_4096_to_8192, &mut table);
        assert_eq!(table, earth("sample_count = 4"));
        assert!(changes.is_empty());
    }

    #[test]
    fn change_default_leaves_another_value_alone() {
        for source in ["texture_resolution = 2048", "texture_resolution = 4096.0"] {
            let mut table = earth(source);
            let changes = apply(default_4096_to_8192, &mut table);
            assert_eq!(table, earth(source));
            assert!(changes.is_empty(), "{source}");
        }
    }

    // --- override_value ---

    #[test]
    fn override_value_replaces_any_value_and_inserts_a_missing_one() {
        for (source, from) in [
            ("sample_count = 8", Some(Value::Integer(8))),
            (
                "sample_count = \"eight\"",
                Some(Value::String("eight".to_owned())),
            ),
            ("", None),
        ] {
            let mut table = earth(source);
            let changes = apply(override_to_2, &mut table);
            assert_eq!(table, earth("sample_count = 2"), "{source}");
            assert_eq!(
                changes,
                [Change {
                    from_version: 0,
                    key: "sample_count".to_owned(),
                    from,
                    to: Value::Integer(2),
                }],
                "{source}"
            );
        }
    }

    #[test]
    fn override_value_records_nothing_when_the_file_already_holds_the_value() {
        let mut table = earth("sample_count = 2");
        assert!(apply(override_to_2, &mut table).is_empty());
        assert_eq!(table, earth("sample_count = 2"));
    }

    // --- versions ---

    #[test]
    fn a_file_without_a_version_is_version_0() {
        assert_eq!(
            read_version(&earth("[sunlit.earth]")),
            FileVersion::Unstamped
        );
        let migrated = migrate(&mut earth("[sunlit.earth]"));
        assert_eq!(migrated.from_version, 0);
        assert!(migrated.upgraded());
    }

    #[test]
    fn a_version_that_is_not_a_non_negative_integer_migrates_from_0() {
        for version in ["-1", "\"one\"", "1.5"] {
            let source = format!("version = {version}\n[sunlit.earth]\nsample_count = 8\n");
            assert!(
                matches!(read_version(&earth(&source)), FileVersion::Damaged(_)),
                "{version}"
            );
            let mut doc = earth(&source);
            let migrated = migrate(&mut doc);
            assert_eq!(migrated.from_version, 0, "{version}");
            assert!(
                migrated.changes.iter().any(|c| c.from_version == 0),
                "step 0 ran: {version}"
            );
            assert_eq!(doc["version"], Value::Integer(i64::from(CONFIG_VERSION)));
        }
    }

    #[test]
    fn migrating_stamps_the_build_version() {
        let mut doc = earth("[sunlit.earth]\n");
        migrate(&mut doc);
        assert_eq!(doc["version"], Value::Integer(i64::from(CONFIG_VERSION)));
    }

    #[test]
    fn a_file_without_the_earth_table_is_stamped_and_otherwise_untouched() {
        for source in ["", "[sunlit]\nearth = 5\n", "sunlit = \"x\"\n"] {
            let mut doc = earth(source);
            let migrated = migrate(&mut doc);
            assert!(migrated.changes.is_empty(), "{source}");
            doc.remove("version");
            assert_eq!(doc, earth(source), "{source}");
        }
    }

    #[test]
    fn a_file_at_the_build_version_runs_no_step() {
        let source = format!(
            "version = {CONFIG_VERSION}\n[sunlit.earth]\ntexture_resolution = 4096\nsample_count = 8\n"
        );
        let mut doc = earth(&source);
        let migrated = migrate(&mut doc);
        assert!(!migrated.upgraded());
        assert!(migrated.changes.is_empty());
        assert_eq!(doc, earth(&source));
    }

    #[test]
    fn a_file_from_a_newer_build_runs_no_step_and_keeps_its_version() {
        for newer in [i64::from(CONFIG_VERSION) + 1, i64::MAX] {
            let source = format!(
                "version = {newer}\n[sunlit.earth]\ntexture_resolution = 4096\nsample_count = 8\n"
            );
            let mut doc = earth(&source);
            let migrated = migrate(&mut doc);
            assert!(migrated.from_version > CONFIG_VERSION);
            assert!(!migrated.upgraded());
            assert!(migrated.changes.is_empty());
            assert_eq!(doc, earth(&source));
        }
    }

    #[test]
    fn every_change_names_the_step_that_made_it() {
        let mut doc = earth("[sunlit.earth]\ntexture_resolution = 4096\nsample_count = 8\n");
        let migrated = migrate(&mut doc);
        assert!(
            migrated
                .changes
                .iter()
                .any(|c| c.from_version == 0 && c.key == "sample_count")
        );
        assert!(
            migrated
                .changes
                .iter()
                .all(|c| c.from_version < CONFIG_VERSION)
        );
    }
}
