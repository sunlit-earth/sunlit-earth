//! The build cache: what an earlier build in this image left behind.
//!
//! Amendment decisions 19 to 21, 24 and 26. A builder guest is a throwaway
//! overlay of a golden disk, so nothing a build learns survives the guest that
//! learned it: every run downloads and compiles all 519 crates. The cache is
//! that work, packed by the guest that did it, parked on the host, and copied
//! back into the next guest.
//!
//! What keeps it a cache rather than a shortcut is that it can never decide
//! what the binary is. The source tree is extracted with `-m`, so no committed
//! file can look older than an artifact built from it; `--locked` and the
//! lockfile's checksums mean a restored registry can only hold what the network
//! would have handed over; and a cache is discarded whole rather than merged
//! when the channel or the builder image changes. A damaged one costs a slower
//! build or a loud link failure, never a wrong binary.
//!
//! Two archives rather than one, because the two halves change at different
//! rates: the registry moves only when `Cargo.lock` does, and the build
//! directory moves on every build. One archive would send a gigabyte back over
//! the wire on every run to say nothing new.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The two halves of a cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    /// `~/.cargo/registry` and `~/.cargo/git`: what the network handed over.
    Registry,
    /// The build directory: what the compiler produced.
    Target,
}

impl Kind {
    pub const ALL: [Self; 2] = [Self::Registry, Self::Target];

    /// The slug the archive, the sidecar and every message use.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Registry => "registry",
            Self::Target => "target",
        }
    }

    /// What it holds, for a line somebody reads.
    pub fn label(self) -> &'static str {
        match self {
            Self::Registry => "the crate registry",
            Self::Target => "the build directory",
        }
    }

    /// The archive's file name, in both the store and the guest.
    ///
    /// zstd on both builders, decided rather than left to what each guest
    /// happens to have: rlibs carrying LLVM bitcode compress several fold and
    /// zstd compresses fast enough that the pack does not become the new
    /// bottleneck. There is deliberately no uncompressed fallback, because a
    /// fallback would mean two formats to test and a build whose transfer cost
    /// depends on which image it happened to run in.
    pub fn archive(self) -> String {
        format!("{}.tar.zst", self.slug())
    }

    /// The sidecar beside it, which is what a restore reads before it trusts
    /// the archive.
    pub fn sidecar(self) -> String {
        format!("{}.json", self.slug())
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.slug())
    }
}

/// The version of the sidecar's own layout.
pub const FORMAT_VERSION: u32 = 1;

/// What one archive was made from.
///
/// Decision 24: a cache belongs to one channel and one build of one image, and
/// anything else is discarded whole rather than merged. `image_built_utc` is in
/// here beside the template hash because an image rebuilt from an unchanged
/// template is still a different MSVC, a different libclang and a different set
/// of paths baked into cargo's fingerprints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sidecar {
    pub format_version: u32,
    /// The file this describes, so a sidecar that was moved says what it is.
    pub archive: String,
    pub bytes: u64,
    /// The channel `rust-toolchain.toml` pinned when it was written.
    pub channel: String,
    /// The builder image's slug.
    pub image: String,
    /// Its manifest's template hash and build time, which together are what
    /// says this is the same build of the same image.
    pub template_hash: String,
    pub image_built_utc: String,
    /// The `Cargo.lock` the registry was made from. Not part of what a restore
    /// refuses on: a lockfile that moved means some crates are missing, which
    /// cargo downloads. It is what decides whether a *new* registry archive is
    /// worth packing (decision 26).
    pub lockfile_hash: String,
    /// The commit and the moment that wrote it, so a cache can be dated.
    pub commit: String,
    pub written_utc: String,
    pub written_unix: u64,
}

impl Sidecar {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("malformed cache sidecar: {e}"))
    }
}

/// What this build is, for a sidecar to be compared against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub channel: String,
    pub image: String,
    pub template_hash: String,
    pub image_built_utc: String,
    pub lockfile_hash: String,
}

/// Whether a cache written under `sidecar` may be restored into a build with
/// `facts`, and if not, which field moved.
///
/// One field at a time and named, because "the cache was not used" with no
/// reason is indistinguishable from a cache that is not being written at all.
pub fn restorable(sidecar: &Sidecar, facts: &Facts) -> Result<(), String> {
    if sidecar.format_version != FORMAT_VERSION {
        return Err(format!(
            "it was written in format {} and this is format {FORMAT_VERSION}",
            sidecar.format_version
        ));
    }
    let moved = [
        ("the builder image", &sidecar.image, &facts.image),
        (
            "the pinned toolchain channel",
            &sidecar.channel,
            &facts.channel,
        ),
        (
            "the builder image's template",
            &sidecar.template_hash,
            &facts.template_hash,
        ),
        (
            "the builder image's build time",
            &sidecar.image_built_utc,
            &facts.image_built_utc,
        ),
    ];
    for (what, was, now) in moved {
        if was != now {
            return Err(format!("{what} was {was} and is {now} now"));
        }
    }
    Ok(())
}

/// Whether the registry is worth packing again.
///
/// Decision 26: `--locked` means an unchanged lockfile is an unchanged
/// registry, so a build that did not move `Cargo.lock` has nothing new to send
/// back and the several hundred megabytes stay where they are.
///
/// `existing` is the sidecar of a cache that was *restored*, and passing one
/// that was refused instead is the bug this sentence exists to prevent: its
/// hash describes an archive no later build will read either, so the answer
/// would be no forever and the registry would stay cold until the lockfile
/// happened to move.
pub fn registry_worth_saving(existing: Option<&Sidecar>, lockfile_hash: &str) -> bool {
    existing.is_none_or(|sidecar| sidecar.lockfile_hash != lockfile_hash)
}

/// What a run did with one archive, for the record and for the terminal.
///
/// Decision 27: the build record says whether the build was warm, per archive,
/// which is what keeps decision 19's argument checkable after the fact rather
/// than a claim in a document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// `registry` or `target`.
    pub archive: String,
    /// Whether it was copied into the guest and unpacked there.
    pub restored: bool,
    /// How large the archive was, when one was restored or written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// When the restored cache was written, so its age is readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_utc: Option<String>,
    /// Why it was not restored, when it was not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Whether this run wrote a fresh one back.
    pub saved: bool,
    /// How long the archive took to reach the guest, and how long the fresh one
    /// took to come back.
    ///
    /// The two halves of the risk this whole design was written against: an
    /// archive is up to a gigabyte and it crosses an SSH boundary twice, so the
    /// question is whether that costs more than the compiling it saves. The
    /// answer is only readable if somebody wrote it down, and a build that is
    /// over is the only thing that knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copied_in_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copied_out_secs: Option<u64>,
}

impl Report {
    /// One line about what happened to this archive.
    pub fn line(&self) -> String {
        let mut text = format!("  cache:   {}: ", self.archive);
        if self.restored {
            text.push_str("restored");
            if let Some(bytes) = self.bytes {
                let _ = write!(text, " from {}", crate::util::format_bytes(bytes));
            }
            if let Some(secs) = self.copied_in_secs {
                let _ = write!(
                    text,
                    ", {} to reach the guest",
                    crate::util::format_duration(std::time::Duration::from_secs(secs))
                );
            }
            if let Some(written) = &self.written_utc {
                let _ = write!(text, ", written {written}");
            }
        } else {
            text.push_str("cold");
            if let Some(reason) = &self.reason {
                let _ = write!(text, " ({reason})");
            }
        }
        if self.saved {
            text.push_str("; saved");
        }
        text
    }
}

/// A temporary name for a file being written, carrying this process's id.
///
/// The discipline `assets::texture_cache` already uses: an interrupted pull
/// cannot leave a truncated archive for the next build to read, and two writers
/// of one entry cannot truncate each other.
pub fn temp_path(final_path: &Path) -> PathBuf {
    let name = final_path
        .file_name()
        .map_or_else(|| "cache".to_owned(), |n| n.to_string_lossy().into_owned());
    final_path.with_file_name(format!("{name}.{}.tmp", std::process::id()))
}

/// Put a file written under its temporary name in place.
pub fn commit(temp: &Path, final_path: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(final_path);
    std::fs::rename(temp, final_path).map_err(|e| {
        format!(
            "cannot put {} in place as {}: {e}",
            temp.display(),
            final_path.display()
        )
    })
}

/// Read a sidecar, distinguishing "there is none" from "there is one and it
/// will not parse".
pub fn read_sidecar(path: &Path) -> Result<Option<Sidecar>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Sidecar::from_json(&text).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// Write one, temp-then-rename.
pub fn write_sidecar(path: &Path, sidecar: &Sidecar) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let temp = temp_path(path);
    std::fs::write(&temp, sidecar.to_json())
        .map_err(|e| format!("cannot write {}: {e}", temp.display()))?;
    commit(&temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sidecar() -> Sidecar {
        Sidecar {
            format_version: FORMAT_VERSION,
            archive: Kind::Target.archive(),
            bytes: 1_234_567,
            channel: "1.94.0".to_owned(),
            image: "linux-builder".to_owned(),
            template_hash: "crc32:1a2b3c4d".to_owned(),
            image_built_utc: "2026-08-28T09:00:00Z".to_owned(),
            lockfile_hash: "crc32:deadbeef".to_owned(),
            commit: "0123456789abcdef".to_owned(),
            written_utc: "2026-08-29T10:00:00Z".to_owned(),
            written_unix: 1_787_000_000,
        }
    }

    fn facts() -> Facts {
        Facts {
            channel: "1.94.0".to_owned(),
            image: "linux-builder".to_owned(),
            template_hash: "crc32:1a2b3c4d".to_owned(),
            image_built_utc: "2026-08-28T09:00:00Z".to_owned(),
            lockfile_hash: "crc32:deadbeef".to_owned(),
        }
    }

    #[test]
    fn a_sidecar_round_trips() {
        let one = sidecar();
        assert_eq!(Sidecar::from_json(&one.to_json()), Ok(one.clone()));
        // Every field a restore reads is in the file, spelled the way the
        // reader looks for it.
        let json = one.to_json();
        for field in [
            "format_version",
            "channel",
            "image",
            "template_hash",
            "image_built_utc",
            "lockfile_hash",
            "commit",
        ] {
            assert!(json.contains(field), "{json}");
        }
        assert!(Sidecar::from_json("{").is_err());
    }

    /// Decision 24: a cache belongs to one channel and one build of one image.
    /// Each of the four is refused on its own, and the refusal names the field
    /// that moved, because "not used" with no reason reads like a cache that is
    /// not being written at all.
    #[test]
    fn a_cache_is_refused_when_the_channel_or_the_image_moves() {
        assert_eq!(restorable(&sidecar(), &facts()), Ok(()));

        let cases = [
            ("channel", "1.95.0", "toolchain channel"),
            ("image", "windows-builder", "builder image"),
            ("template_hash", "crc32:99999999", "template"),
            ("image_built_utc", "2026-09-01T00:00:00Z", "build time"),
        ];
        for (field, moved, expected) in cases {
            let mut now = facts();
            match field {
                "channel" => now.channel = moved.to_owned(),
                "image" => now.image = moved.to_owned(),
                "template_hash" => now.template_hash = moved.to_owned(),
                _ => now.image_built_utc = moved.to_owned(),
            }
            let err = restorable(&sidecar(), &now).unwrap_err();
            assert!(err.contains(expected), "{field}: {err}");
            assert!(err.contains(moved), "{field}: {err}");
        }

        // A sidecar from another layout is not something to read fields out of.
        let mut old = sidecar();
        old.format_version = FORMAT_VERSION + 1;
        assert!(restorable(&old, &facts()).unwrap_err().contains("format"));

        // A lockfile that moved is not a reason to refuse: it means some crates
        // are missing, and cargo downloads those. It is what decides whether a
        // fresh registry is worth sending back.
        let mut moved_lock = facts();
        moved_lock.lockfile_hash = "crc32:00000000".to_owned();
        assert_eq!(restorable(&sidecar(), &moved_lock), Ok(()));
    }

    /// Decision 26: `--locked` means an unchanged lockfile is an unchanged
    /// registry, so an ordinary build sends nothing back for it.
    #[test]
    fn the_registry_is_repacked_only_when_the_lockfile_moved() {
        let existing = sidecar();
        assert!(!registry_worth_saving(
            Some(&existing),
            &existing.lockfile_hash
        ));
        assert!(registry_worth_saving(Some(&existing), "crc32:00000000"));
        // Nothing on disk means everything to save.
        assert!(registry_worth_saving(None, &existing.lockfile_hash));
    }

    /// A pull that was interrupted must not leave a truncated archive for the
    /// next build to read, and two writers of one entry must not truncate each
    /// other.
    #[test]
    fn an_archive_is_written_under_a_name_of_its_own_and_then_moved() {
        let final_path = Path::new("/srv/vm/cache/linux-builder/target.tar.zst");
        let temp = temp_path(final_path);
        assert_eq!(temp.parent(), final_path.parent());
        let name = temp.file_name().expect("a name").to_string_lossy();
        assert!(name.starts_with("target.tar.zst."), "{name}");
        assert!(name.ends_with(".tmp"), "{name}");
        assert!(name.contains(&std::process::id().to_string()), "{name}");
        assert_ne!(temp, final_path.to_path_buf());
    }

    #[test]
    fn a_sidecar_survives_a_round_trip_through_the_disk() {
        let dir = std::env::temp_dir().join("sunlit_xtask_cache_sidecar");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("target.json");
        assert_eq!(read_sidecar(&path), Ok(None));
        write_sidecar(&path, &sidecar()).expect("written");
        assert_eq!(read_sidecar(&path), Ok(Some(sidecar())));
        // Nothing of the temporary name is left lying beside it.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("readable")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| Path::new(name).extension().is_some_and(|e| e == "tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        std::fs::write(&path, "not json").expect("write");
        assert!(read_sidecar(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The two archives are named the same way in the store and in the guest,
    /// because one name is copied between them.
    #[test]
    fn each_half_names_its_archive_and_its_sidecar_once() {
        assert_eq!(Kind::Registry.archive(), "registry.tar.zst");
        assert_eq!(Kind::Target.archive(), "target.tar.zst");
        assert_eq!(Kind::Registry.sidecar(), "registry.json");
        assert_eq!(Kind::Target.sidecar(), "target.json");
        for kind in Kind::ALL {
            assert!(kind.archive().starts_with(kind.slug()), "{kind}");
            assert!(kind.sidecar().starts_with(kind.slug()), "{kind}");
            assert!(!kind.label().is_empty(), "{kind}");
        }
        assert_ne!(Kind::Registry.archive(), Kind::Target.archive());
    }

    /// The line a run prints has to say cold from warm at a glance, and say why
    /// when it is cold.
    #[test]
    fn the_report_line_says_which_of_the_two_it_was() {
        let warm = Report {
            archive: Kind::Target.slug().to_owned(),
            restored: true,
            bytes: Some(1_048_576),
            written_utc: Some("2026-08-29T10:00:00Z".to_owned()),
            reason: None,
            saved: true,
            copied_in_secs: Some(41),
            copied_out_secs: Some(40),
        };
        let line = warm.line();
        assert!(line.contains("restored"), "{line}");
        assert!(line.contains("2026-08-29T10:00:00Z"), "{line}");
        assert!(line.contains("saved"), "{line}");
        // The transfer is the cost this whole design was weighed against, so a
        // warm line says what it was rather than leaving it to the totals.
        assert!(line.contains("41s"), "{line}");

        let cold = Report {
            archive: Kind::Registry.slug().to_owned(),
            restored: false,
            bytes: None,
            written_utc: None,
            reason: Some("the pinned toolchain channel was 1.93.0 and is 1.94.0 now".to_owned()),
            saved: true,
            copied_in_secs: None,
            copied_out_secs: None,
        };
        let line = cold.line();
        assert!(line.contains("cold"), "{line}");
        assert!(line.contains("1.93.0"), "{line}");
    }
}
