//! The image manifest: what an image was built from, and when.
//!
//! Plan decision 4. A repo-pinned image checksum is impossible, because every
//! developer builds their own image and a Windows install is not reproducible.
//! What the repo can pin is the template, so the manifest records the hash of
//! the template tree the image came from alongside the image's own checksum and
//! the build-completion time. From those three the doctor distinguishes
//! missing, corrupt, stale, and, for the Windows evaluation image, expired.

use serde::{Deserialize, Serialize};

use crate::provider::target::Image;
use crate::util;

/// Bumped when a field changes meaning rather than merely appearing.
pub const MANIFEST_VERSION: u32 = 1;

/// The Windows 11 Enterprise evaluation runs 90 days from installation.
pub const EVAL_TOTAL_DAYS: u64 = 90;

/// Warn from day 75, which leaves a fortnight to schedule a rebuild that takes
/// the better part of an hour.
pub const EVAL_WARN_DAYS: u64 = 75;

/// One file recorded in a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRecord {
    /// File name relative to the image directory.
    pub file: String,
    pub bytes: u64,
    /// Checksum in `crc32:xxxxxxxx` form; see `hash.rs` for why it is a CRC.
    pub checksum: String,
}

/// The image a layer was provisioned over, as it stood at the time.
///
/// A layer is a differencing child, so its parent is part of its identity and
/// must not change under it: `Hyper-V` refuses to attach a child whose parent's
/// identifier changed, and qcow2 reads garbage silently rather than refusing.
/// Recording the parent's own checksum for the exact file the layer backs onto
/// is what lets the inventory answer "is this layer still attached to the disk
/// it was built over" without reading either disk (plan decision 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParentRecord {
    /// The parent image's slug.
    pub image: String,
    /// The backing file inside the parent's image directory, which is the disk
    /// format of whichever host built the layer.
    pub file: String,
    /// What the parent's own manifest recorded for that file. Compared against
    /// the parent's manifest rather than against the disk, so the check costs
    /// nothing and stays honest about where the number came from.
    pub checksum: String,
    /// The parent's build time, which is the instant the evaluation clock
    /// started running. A layer's own timestamp says when it was provisioned and
    /// nothing about the licence it inherited.
    #[serde(default)]
    pub built_unix: u64,
}

/// What `vm build-image` writes next to the images it produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub format_version: u32,
    /// The image's slug. Defaulted, because manifests written when there was
    /// one image per target carry only `target`, and those two images are the
    /// ones whose slugs are the same either way.
    #[serde(default)]
    pub image: String,
    /// The operating system inside it.
    #[serde(default)]
    pub target: String,
    /// The image this one is a differencing child of, for a layer.
    #[serde(default)]
    pub parent: Option<ParentRecord>,
    /// Hash of `vm/<slug>/` at build time, from `hash::template_hash`.
    #[serde(default)]
    pub template_hash: String,
    /// Build completion, in seconds since the Unix epoch. For a base this is
    /// the number the evaluation clock is measured from: the Windows licence
    /// starts running during the build and never resets, because every run
    /// boots a throwaway overlay of a read-only image. For a layer it is when
    /// the layer was provisioned, which the template-currency check uses and
    /// the expiry check must not: the clock belongs to the installation, and
    /// that is the parent's.
    #[serde(default)]
    pub built_unix: u64,
    /// The same instant in RFC 3339, so the file is readable without tooling.
    #[serde(default)]
    pub built_utc: String,
    /// The installation media or base image the build consumed.
    #[serde(default)]
    pub source: String,
    /// What built it, for example `packer 1.11.2`.
    #[serde(default)]
    pub builder: String,
    #[serde(default)]
    pub images: Vec<ImageRecord>,
}

impl Manifest {
    /// A manifest for a build that just finished.
    pub fn new(
        image: Image,
        template_hash: String,
        built_unix: u64,
        source: String,
        builder: String,
        images: Vec<ImageRecord>,
    ) -> Self {
        Self {
            format_version: MANIFEST_VERSION,
            image: image.slug().to_owned(),
            target: image.target().slug().to_owned(),
            parent: None,
            template_hash,
            built_unix,
            built_utc: util::format_unix_utc(built_unix),
            source,
            builder,
            images,
        }
    }

    /// The same, for a layer, carrying what its parent looked like.
    pub fn with_parent(mut self, parent: ParentRecord) -> Self {
        self.parent = Some(parent);
        self
    }

    /// The image this manifest is about, if it names one this xtask knows.
    ///
    /// A manifest with no image field is one written when there was one image
    /// per target, so it can only be about that target's desktop image.
    pub fn image(&self) -> Option<Image> {
        if self.image.trim().is_empty() {
            return crate::provider::target::Target::parse(&self.target).map(Image::desktop);
        }
        Image::parse(&self.image)
    }

    /// The record for one of the parent's files, by name.
    pub fn record_for(&self, file: &str) -> Option<&ImageRecord> {
        self.images.iter().find(|r| r.file == file)
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("malformed manifest: {e}"))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Whether the image was built from the template tree currently in the
    /// repo. An unreadable template tree (`None`) is not evidence of staleness.
    pub fn currency(&self, current_template_hash: Option<&str>) -> Currency {
        match current_template_hash {
            None => Currency::Unknown,
            Some(current) if current == self.template_hash => Currency::Current,
            Some(current) => Currency::Stale {
                built_from: self.template_hash.clone(),
                repo_has: current.to_owned(),
            },
        }
    }

    /// The instant this image's evaluation licence started running.
    ///
    /// A layer inherits its parent's installation and therefore its parent's
    /// clock, so it reads the timestamp it recorded for the parent rather than
    /// its own. A layer with no parent record is one whose manifest predates the
    /// field or was hand-edited, and the inventory calls that detached before
    /// anything asks this.
    pub fn eval_epoch(&self) -> u64 {
        match &self.parent {
            Some(parent) => parent.built_unix,
            None => self.built_unix,
        }
    }

    /// Where this image sits on its evaluation clock, for images that have one.
    pub fn eval_state(&self, image: Image, now_unix: u64) -> Option<EvalState> {
        image
            .has_eval_expiry()
            .then(|| eval_state(self.eval_epoch(), now_unix))
    }
}

/// Whether an image still matches the repo templates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Currency {
    Current,
    Stale {
        built_from: String,
        repo_has: String,
    },
    /// The repo templates could not be read, so the question was not asked.
    Unknown,
}

/// Where an evaluation image sits in its 90 days.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalState {
    /// Comfortably inside the window.
    Fresh { days_used: u64, days_left: u64 },
    /// Inside the window but close enough to plan a rebuild.
    Expiring { days_used: u64, days_left: u64 },
    /// Past 90 days. Windows does not refuse to boot; it starts shutting itself
    /// down about once an hour, which shows up as flaky runs rather than as a
    /// clear failure. That is the whole reason this is checked up front.
    Expired { days_used: u64, days_over: u64 },
}

impl EvalState {
    pub fn is_expired(self) -> bool {
        matches!(self, Self::Expired { .. })
    }

    /// A one-line summary for reports.
    pub fn summary(self) -> String {
        match self {
            Self::Fresh {
                days_used,
                days_left,
            } => format!("evaluation day {days_used} of {EVAL_TOTAL_DAYS}, {days_left} left"),
            Self::Expiring {
                days_used,
                days_left,
            } => format!(
                "evaluation day {days_used} of {EVAL_TOTAL_DAYS}, only {days_left} left; \
                 rebuild with `cargo xtask vm build-image windows`"
            ),
            Self::Expired {
                days_used,
                days_over,
            } => format!(
                "evaluation expired {days_over} days ago (day {days_used} of \
                 {EVAL_TOTAL_DAYS}); the guest now shuts itself down hourly"
            ),
        }
    }
}

/// Classify an evaluation image's age.
///
/// A build timestamp in the future, which clock skew or a restored backup can
/// produce, reads as day zero rather than as a wildly expired image.
pub fn eval_state(built_unix: u64, now_unix: u64) -> EvalState {
    let days_used = util::days_between(built_unix, now_unix);
    if days_used >= EVAL_TOTAL_DAYS {
        EvalState::Expired {
            days_used,
            days_over: days_used - EVAL_TOTAL_DAYS,
        }
    } else if days_used >= EVAL_WARN_DAYS {
        EvalState::Expiring {
            days_used,
            days_left: EVAL_TOTAL_DAYS - days_used,
        }
    } else {
        EvalState::Fresh {
            days_used,
            days_left: EVAL_TOTAL_DAYS - days_used,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::SECS_PER_DAY;

    fn sample() -> Manifest {
        Manifest::new(
            Image::Windows,
            "crc32:deadbeef".to_owned(),
            1_755_600_000,
            "windows11-enterprise-eval.iso".to_owned(),
            "packer 1.11.2".to_owned(),
            vec![ImageRecord {
                file: "golden.qcow2".to_owned(),
                bytes: 21_474_836_480,
                checksum: "crc32:12345678".to_owned(),
            }],
        )
    }

    #[test]
    fn a_manifest_round_trips_through_json() {
        let manifest = sample();
        let parsed = Manifest::from_json(&manifest.to_json()).expect("round trip");
        assert_eq!(parsed, manifest);
        assert_eq!(parsed.built_utc, "2025-08-19T10:40:00Z");
        assert_eq!(parsed.format_version, MANIFEST_VERSION);
    }

    #[test]
    fn a_manifest_missing_optional_fields_still_parses() {
        let parsed = Manifest::from_json(r#"{"target":"linux","built_unix":10}"#)
            .expect("defaults fill the rest");
        assert_eq!(parsed.target, "linux");
        assert_eq!(parsed.built_unix, 10);
        assert!(parsed.images.is_empty());
        assert_eq!(parsed.template_hash, "");
    }

    #[test]
    fn a_manifest_with_a_field_from_the_future_still_parses() {
        let parsed = Manifest::from_json(r#"{"target":"linux","something_new":true}"#)
            .expect("unknown fields are ignored");
        assert_eq!(parsed.target, "linux");
    }

    #[test]
    fn garbage_is_reported_rather_than_defaulted() {
        let err = Manifest::from_json("not json at all").unwrap_err();
        assert!(err.contains("malformed manifest"), "{err}");
    }

    #[test]
    fn records_are_found_by_file_name() {
        let manifest = sample();
        assert_eq!(
            manifest.record_for("golden.qcow2").map(|r| r.bytes),
            Some(21_474_836_480)
        );
        assert!(manifest.record_for("golden.vhdx").is_none());
    }

    #[test]
    fn a_manifest_from_before_there_were_four_images_reads_as_a_desktop_one() {
        let parsed = Manifest::from_json(r#"{"target":"windows","built_unix":10}"#)
            .expect("an older manifest parses");
        assert_eq!(parsed.image(), Some(Image::Windows));
        assert_eq!(sample().image(), Some(Image::Windows));
        // And an image nothing knows is not guessed at.
        let odd = Manifest::from_json(r#"{"image":"freebsd","target":"freebsd"}"#).expect("parses");
        assert_eq!(odd.image(), None);
    }

    /// A layer's licence is its parent's, so its expiry is measured from the
    /// parent's build and not from its own. Provisioning a layer over a
    /// two-month-old install must not reset the clock to today.
    #[test]
    fn a_layer_reads_its_evaluation_clock_off_its_parent() {
        let parent_built = 1_000 * SECS_PER_DAY;
        let layer = Manifest::new(
            Image::WindowsBuilder,
            "crc32:aaaa".to_owned(),
            parent_built + 60 * SECS_PER_DAY,
            "images/windows/golden.vhdx".to_owned(),
            "xtask".to_owned(),
            Vec::new(),
        )
        .with_parent(ParentRecord {
            image: Image::Windows.slug().to_owned(),
            file: "golden.vhdx".to_owned(),
            checksum: "crc32:12345678".to_owned(),
            built_unix: parent_built,
        });

        assert_eq!(layer.eval_epoch(), parent_built);
        let now = parent_built + 80 * SECS_PER_DAY;
        assert_eq!(
            layer.eval_state(Image::WindowsBuilder, now),
            Some(EvalState::Expiring {
                days_used: 80,
                days_left: 10
            }),
            "a layer provisioned 20 days ago over an 80-day-old install is 80 days in"
        );
        // Its own timestamp is what the template-currency check uses, so it is
        // still recorded and still readable.
        assert_eq!(layer.built_unix, parent_built + 60 * SECS_PER_DAY);
        let parsed = Manifest::from_json(&layer.to_json()).expect("round trip");
        assert_eq!(parsed, layer);
        assert_eq!(parsed.image(), Some(Image::WindowsBuilder));
        assert_eq!(parsed.target, "windows");
    }

    #[test]
    fn currency_compares_against_the_repo_templates() {
        let manifest = sample();
        assert_eq!(manifest.currency(Some("crc32:deadbeef")), Currency::Current);
        assert_eq!(manifest.currency(None), Currency::Unknown);
        assert_eq!(
            manifest.currency(Some("crc32:0000ffff")),
            Currency::Stale {
                built_from: "crc32:deadbeef".to_owned(),
                repo_has: "crc32:0000ffff".to_owned(),
            }
        );
    }

    #[test]
    fn the_evaluation_clock_has_three_bands_with_exact_boundaries() {
        let built = 1_000 * SECS_PER_DAY;
        let at = |day: u64| eval_state(built, built + day * SECS_PER_DAY);

        assert_eq!(
            at(0),
            EvalState::Fresh {
                days_used: 0,
                days_left: 90
            }
        );
        assert_eq!(
            at(74),
            EvalState::Fresh {
                days_used: 74,
                days_left: 16
            }
        );
        assert_eq!(
            at(75),
            EvalState::Expiring {
                days_used: 75,
                days_left: 15
            }
        );
        assert_eq!(
            at(89),
            EvalState::Expiring {
                days_used: 89,
                days_left: 1
            }
        );
        assert_eq!(
            at(90),
            EvalState::Expired {
                days_used: 90,
                days_over: 0
            }
        );
        assert_eq!(
            at(120),
            EvalState::Expired {
                days_used: 120,
                days_over: 30
            }
        );
        assert!(at(90).is_expired());
        assert!(!at(89).is_expired());
    }

    #[test]
    fn a_partial_day_does_not_advance_the_clock() {
        let built = 1_000 * SECS_PER_DAY;
        let almost = built + 90 * SECS_PER_DAY - 1;
        assert!(!eval_state(built, almost).is_expired());
        assert!(eval_state(built, almost + 1).is_expired());
    }

    #[test]
    fn a_build_stamped_in_the_future_reads_as_brand_new() {
        let now = 1_000 * SECS_PER_DAY;
        let built = now + 30 * SECS_PER_DAY;
        assert_eq!(
            eval_state(built, now),
            EvalState::Fresh {
                days_used: 0,
                days_left: 90
            }
        );
    }

    #[test]
    fn only_the_windows_installation_carries_an_evaluation_state() {
        let manifest = sample();
        let now = manifest.built_unix + 100 * SECS_PER_DAY;
        assert!(manifest.eval_state(Image::Windows, now).is_some());
        assert!(manifest.eval_state(Image::WindowsBuilder, now).is_some());
        assert!(manifest.eval_state(Image::Linux, now).is_none());
        assert!(manifest.eval_state(Image::LinuxBuilder, now).is_none());
    }

    #[test]
    fn the_expiry_summaries_say_what_to_do_about_it() {
        let built = 0;
        assert!(
            eval_state(built, 80 * SECS_PER_DAY)
                .summary()
                .contains("vm build-image windows")
        );
        assert!(
            eval_state(built, 95 * SECS_PER_DAY)
                .summary()
                .contains("shuts itself down hourly")
        );
    }
}
