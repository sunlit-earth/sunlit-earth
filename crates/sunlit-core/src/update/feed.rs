//! The release file, `releases/v1.json`, and what the app makes of it.
//!
//! Every release of the app follows the same seven rules from the first one,
//! which is what lets the file grow without a `v2`: unknown fields are ignored,
//! an entry that cannot be read is skipped, a stable build ignores prereleases,
//! the version on offer is the greatest considered one above the app's own,
//! what changed runs from there down to the app's own, a URL outside the
//! project's release pages counts as missing, and a body over [`BODY_LIMIT`] is
//! a failed check. `docs/plans/2026-11-22-update-check-plan.md` has the format.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use semver::Version;
use serde_json::Value;
use time::Date;
use time::macros::format_description;

/// The largest body a check accepts.
pub const BODY_LIMIT: u64 = 256 * 1024;

/// What a release page's URL starts with.
pub const NOTES_PREFIX: &str = "https://github.com/sunlit-earth/sunlit-earth/releases/";

/// What an archive's URL starts with.
pub const ASSET_PREFIX: &str = "https://github.com/sunlit-earth/sunlit-earth/releases/download/";

/// Whether `url` is `prefix` followed by a plain path.
///
/// Every segment of that path is ASCII letters, digits, `.`, `_`, `-` and `+`,
/// and none is empty, `.` or `..`. So nothing a browser or a downloader
/// resolves can climb out of the prefix: no dot segment, no `%` to encode one
/// or a slash, no `\` that a browser reads as `/`, and no query, fragment,
/// userinfo or port.
fn is_under(url: &str, prefix: &str) -> bool {
    url.strip_prefix(prefix).is_some_and(|path| {
        path.split('/').all(|segment| {
            !matches!(segment, "" | "." | "..")
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
        })
    })
}

/// One archive of a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// `None` when the file had none, or one outside [`ASSET_PREFIX`].
    pub url: Option<String>,
    /// In bytes.
    pub size: Option<u64>,
}

/// One entry of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub date: Option<Date>,
    /// The release's page, `None` when the file had none, or one outside
    /// [`NOTES_PREFIX`].
    pub notes: Option<String>,
    pub highlights: Vec<String>,
    /// By the keys [`asset_key`] makes.
    pub assets: BTreeMap<String, Asset>,
}

/// Every readable entry of the file, in the file's order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Feed {
    pub releases: Vec<Release>,
}

/// A newer version than the running one, and every release on the way to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// Newest first: the version on offer, then each considered release above
    /// the running one. Never empty.
    pub releases: Vec<Release>,
}

impl Offer {
    /// The version on offer.
    pub fn latest(&self) -> &Release {
        &self.releases[0]
    }
}

/// What the file says about the running version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    UpToDate,
    Available(Offer),
}

/// Read a body, skipping what cannot be read.
///
/// Fails only for a body over [`BODY_LIMIT`], one that is not JSON, or one
/// without a `releases` list: there is nothing to skip to in those.
pub fn parse(body: &[u8]) -> Result<Feed, String> {
    if body.len() as u64 > BODY_LIMIT {
        return Err(format!(
            "the release file is {} bytes, more than the {BODY_LIMIT} a check accepts",
            body.len()
        ));
    }
    let value: Value = serde_json::from_slice(body)
        .map_err(|e| format!("the release file is not valid JSON: {e}"))?;
    let entries = value
        .get("releases")
        .and_then(Value::as_array)
        .ok_or_else(|| "the release file has no list of releases".to_owned())?;
    Ok(Feed {
        releases: entries.iter().filter_map(read_release).collect(),
    })
}

fn read_release(entry: &Value) -> Option<Release> {
    let version = Version::parse(entry.get("version")?.as_str()?).ok()?;
    let date = entry
        .get("date")
        .and_then(Value::as_str)
        .and_then(|text| Date::parse(text, format_description!("[year]-[month]-[day]")).ok());
    let notes = entry
        .get("notes")
        .and_then(Value::as_str)
        .filter(|url| is_under(url, NOTES_PREFIX))
        .map(ToOwned::to_owned);
    let highlights = entry
        .get("highlights")
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |lines| {
            lines
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        });
    let assets =
        entry
            .get("assets")
            .and_then(Value::as_object)
            .map_or_else(BTreeMap::new, |assets| {
                assets
                    .iter()
                    .filter(|(_, asset)| asset.is_object())
                    .map(|(key, asset)| (key.clone(), read_asset(asset)))
                    .collect()
            });
    Some(Release {
        version,
        date,
        notes,
        highlights,
        assets,
    })
}

fn read_asset(asset: &Value) -> Asset {
    Asset {
        url: asset
            .get("url")
            .and_then(Value::as_str)
            .filter(|url| is_under(url, ASSET_PREFIX))
            .map(ToOwned::to_owned),
        size: asset.get("size").and_then(Value::as_u64),
    }
}

impl Feed {
    /// What the file offers a build of `current`.
    ///
    /// A stable build considers stable releases only and a prerelease build
    /// every release. Versions compare by precedence, so build metadata never
    /// makes one release newer than another, and the first of two entries
    /// with the same precedence is the one kept.
    pub fn verdict(&self, current: &Version) -> Verdict {
        let considers_prereleases = !current.pre.is_empty();
        let mut newer: Vec<&Release> = self
            .releases
            .iter()
            .filter(|release| considers_prereleases || release.version.pre.is_empty())
            .filter(|release| release.version.cmp_precedence(current) == Ordering::Greater)
            .collect();
        newer.sort_by(|a, b| b.version.cmp_precedence(&a.version));
        newer.dedup_by(|a, b| a.version.cmp_precedence(&b.version) == Ordering::Equal);
        if newer.is_empty() {
            Verdict::UpToDate
        } else {
            Verdict::Available(Offer {
                releases: newer.into_iter().cloned().collect(),
            })
        }
    }
}

/// The key of the archive a build for `os` and `arch` is updated from.
///
/// `os` and `arch` as `std::env::consts` spell them; `bundled` is whether the
/// executable runs from inside a macOS `.app`. `None` for a platform no
/// archive is made for.
pub fn asset_key(os: &str, arch: &str, bundled: bool) -> Option<String> {
    match os {
        "windows" | "linux" => Some(format!("{os}-{arch}")),
        "macos" => Some(format!(
            "macos-{arch}-{}",
            if bundled { "app" } else { "terminal" }
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/update_feed_v1.json");

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn entry(version: &str) -> Value {
        serde_json::json!({ "version": version })
    }

    fn feed_of(entries: &[Value]) -> Feed {
        let body = serde_json::json!({ "releases": entries }).to_string();
        parse(body.as_bytes()).unwrap()
    }

    fn versions(verdict: &Verdict) -> Vec<String> {
        match verdict {
            Verdict::UpToDate => Vec::new(),
            Verdict::Available(offer) => offer
                .releases
                .iter()
                .map(|release| release.version.to_string())
                .collect(),
        }
    }

    #[test]
    fn the_reference_copy_reads_in_full() {
        let feed = parse(FIXTURE.as_bytes()).unwrap();
        let raw: Value = serde_json::from_str(FIXTURE).unwrap();
        let entries = raw["releases"].as_array().unwrap();
        assert_eq!(feed.releases.len(), entries.len());
        for (release, entry) in feed.releases.iter().zip(entries) {
            assert_eq!(release.version.to_string(), entry["version"]);
            assert!(release.date.is_some());
            assert_eq!(release.notes.as_deref(), entry["notes"].as_str());
            assert_eq!(
                release.highlights.len(),
                entry["highlights"].as_array().map_or(0, Vec::len)
            );
            assert_eq!(release.assets.len(), 7);
            for asset in release.assets.values() {
                assert!(asset.url.is_some());
                assert!(asset.size.is_some());
            }
        }
    }

    #[test]
    fn every_archive_key_the_app_asks_for_is_in_the_reference_copy() {
        let feed = parse(FIXTURE.as_bytes()).unwrap();
        let keys = [
            asset_key("windows", "x86_64", false),
            asset_key("linux", "x86_64", false),
            asset_key("linux", "aarch64", false),
            asset_key("macos", "aarch64", true),
            asset_key("macos", "x86_64", true),
            asset_key("macos", "aarch64", false),
            asset_key("macos", "x86_64", false),
        ];
        let expected: Vec<&str> = feed.releases[0].assets.keys().map(String::as_str).collect();
        let mut asked: Vec<String> = keys.into_iter().map(Option::unwrap).collect();
        asked.sort();
        assert_eq!(asked, expected);
    }

    #[test]
    fn asset_keys_name_the_platform_the_architecture_and_the_kind_of_mac_build() {
        assert_eq!(
            asset_key("windows", "x86_64", false).unwrap(),
            "windows-x86_64"
        );
        assert_eq!(
            asset_key("linux", "aarch64", false).unwrap(),
            "linux-aarch64"
        );
        assert_eq!(
            asset_key("macos", "aarch64", true).unwrap(),
            "macos-aarch64-app"
        );
        assert_eq!(
            asset_key("macos", "x86_64", false).unwrap(),
            "macos-x86_64-terminal"
        );
        assert_eq!(asset_key("freebsd", "x86_64", false), None);
    }

    #[test]
    fn unknown_fields_are_ignored_at_every_level() {
        let mut raw: Value = serde_json::from_str(FIXTURE).unwrap();
        raw["format_note"] = Value::from("later");
        let first = &mut raw["releases"][0];
        first["signature"] = Value::from("abc");
        first["assets"]["windows-x86_64"]["sha256"] = Value::from("00ff");
        first["assets"]["windows-x86_64"]["mirrors"] = serde_json::json!([1, 2]);

        let extended = parse(raw.to_string().as_bytes()).unwrap();
        assert_eq!(extended, parse(FIXTURE.as_bytes()).unwrap());
    }

    #[test]
    fn an_entry_that_cannot_be_read_is_skipped_and_its_neighbors_still_count() {
        let feed = feed_of(&[
            entry("0.9.0"),
            serde_json::json!({ "date": "2027-01-01" }),
            entry("v0.8.0"),
            entry("eight"),
            serde_json::json!({ "version": 8 }),
            Value::from("0.8.5"),
            entry("0.7.0"),
        ]);
        let read: Vec<String> = feed
            .releases
            .iter()
            .map(|r| r.version.to_string())
            .collect();
        assert_eq!(read, ["0.9.0", "0.7.0"]);
    }

    #[test]
    fn a_field_of_the_wrong_kind_reads_as_missing() {
        let feed = feed_of(&[serde_json::json!({
            "version": "1.0.0",
            "date": "first of may",
            "notes": 3,
            "highlights": ["one", 2, "  ", " three "],
            "assets": { "linux-x86_64": "not an object", "windows-x86_64": { "url": 1, "size": "big" } },
        })]);
        let release = &feed.releases[0];
        assert_eq!(release.date, None);
        assert_eq!(release.notes, None);
        assert_eq!(release.highlights, ["one", "three"]);
        assert_eq!(release.assets.len(), 1);
        assert_eq!(
            release.assets["windows-x86_64"],
            Asset {
                url: None,
                size: None
            }
        );
    }

    #[test]
    fn a_body_that_is_not_a_release_file_fails() {
        assert!(parse(b"<html>").is_err());
        assert!(parse(b"{}").is_err());
        assert!(parse(br#"{"releases": {}}"#).is_err());
        assert!(parse(br#"{"releases": []}"#).is_ok());
    }

    #[test]
    fn a_body_over_the_limit_fails_and_one_at_it_is_read() {
        let limit = usize::try_from(BODY_LIMIT).unwrap();
        let mut at_limit = br#"{"releases": []}"#.to_vec();
        at_limit.resize(limit, b' ');
        assert!(parse(&at_limit).is_ok());
        at_limit.push(b' ');
        assert!(parse(&at_limit).is_err());
    }

    #[test]
    fn a_url_outside_its_prefix_is_missing() {
        let feed = feed_of(&[serde_json::json!({
            "version": "1.0.0",
            "assets": {
                "a": { "url": "https://github.com/sunlit-earth/sunlit-earth/releases/tag/v1.0.0" },
                "b": { "url": "http://github.com/sunlit-earth/sunlit-earth/releases/download/v1.0.0/x.zip" },
                "c": { "url": "https://github.com/sunlit-earth/sunlit-earth-fork/releases/download/v1.0.0/x.zip" },
                "d": { "url": "https://github.com/sunlit-earth/sunlit-earth/releases/download/v1.0.0/x.zip" },
            },
        })]);
        let release = &feed.releases[0];
        let usable: Vec<&str> = release
            .assets
            .iter()
            .filter(|(_, asset)| asset.url.is_some())
            .map(|(key, _)| key.as_str())
            .collect();
        assert_eq!(usable, ["d"]);
    }

    #[test]
    fn a_release_page_counts_only_on_this_repositorys_releases() {
        let page = "https://github.com/sunlit-earth/sunlit-earth/releases/tag/v1.0.0";
        let notes = |url: &str| {
            feed_of(&[serde_json::json!({ "version": "1.0.0", "notes": url })]).releases[0]
                .notes
                .clone()
        };
        assert_eq!(notes(page).as_deref(), Some(page));
        for foreign in [
            "https://example.com/sunlit-earth/sunlit-earth/releases/tag/v1.0.0",
            "https://github.com/sunlit-earth/sunlit-earth-fork/releases/tag/v1.0.0",
            "https://github.com/someone/sunlit-earth/releases/tag/v1.0.0",
            "http://github.com/sunlit-earth/sunlit-earth/releases/tag/v1.0.0",
        ] {
            assert_eq!(notes(foreign), None, "{foreign}");
        }
    }

    /// Paths after a prefix that a browser or a downloader could resolve to
    /// somewhere outside it, or that carry more than a path.
    const ESCAPES: [&str; 17] = [
        "../../../attacker/repo",
        "tag/../../../attacker/repo",
        "tag/..",
        "./tag/v1.0.0",
        "tag/./v1.0.0",
        "%2e%2e/%2e%2e/attacker/repo",
        "tag%2Fv1.0.0",
        "..%2F..%2Fattacker",
        "..\\..\\attacker\\repo",
        "tag\\v1.0.0",
        "tag//v1.0.0",
        "tag/v1.0.0/",
        "tag/v1.0.0?redirect=https://example.com",
        "tag/v1.0.0#top",
        "tag/v1.0.0@example.com",
        "tag/v1.0.0:443",
        "tag/v1.0.0 x",
    ];

    fn read_both(notes: &str, asset: &str) -> (Option<String>, Option<String>) {
        let feed = feed_of(&[serde_json::json!({
            "version": "1.0.0",
            "notes": notes,
            "assets": { "windows-x86_64": { "url": asset } },
        })]);
        let release = &feed.releases[0];
        (
            release.notes.clone(),
            release.assets["windows-x86_64"].url.clone(),
        )
    }

    #[test]
    fn the_shapes_the_website_writes_are_kept() {
        let page = format!("{NOTES_PREFIX}tag/v0.5.0");
        let archive = format!("{ASSET_PREFIX}v0.5.0/sunlit-earth-0.5.0-windows-x86_64.zip");
        assert_eq!(read_both(&page, &archive), (Some(page), Some(archive)));
        let beta = format!(
            "{ASSET_PREFIX}v0.6.0-beta.1/sunlit-earth-0.6.0-beta.1+b2-linux-aarch64.tar.gz"
        );
        assert_eq!(read_both("", &beta).1, Some(beta));
    }

    #[test]
    fn a_url_that_could_climb_out_of_its_prefix_is_missing() {
        for escape in ESCAPES {
            let page = format!("{NOTES_PREFIX}{escape}");
            let archive = format!("{ASSET_PREFIX}{escape}");
            assert_eq!(read_both(&page, &archive), (None, None), "{escape}");
            let nested = format!("{ASSET_PREFIX}v1.0.0/{escape}");
            assert_eq!(read_both(&page, &nested), (None, None), "{escape}");
        }
        assert_eq!(read_both(NOTES_PREFIX, ASSET_PREFIX), (None, None));
    }

    #[test]
    fn a_stable_build_is_not_offered_a_prerelease() {
        let feed = feed_of(&[entry("0.8.0-beta.1"), entry("0.7.0")]);
        assert_eq!(feed.verdict(&version("0.7.0")), Verdict::UpToDate);
        assert_eq!(versions(&feed.verdict(&version("0.6.0"))), ["0.7.0"]);
    }

    #[test]
    fn a_prerelease_build_is_offered_a_newer_prerelease_and_a_newer_stable_release() {
        let feed = feed_of(&[entry("0.8.0-beta.2"), entry("0.7.0")]);
        assert_eq!(
            versions(&feed.verdict(&version("0.7.0-beta.1"))),
            ["0.8.0-beta.2", "0.7.0"]
        );
        assert_eq!(
            versions(&feed.verdict(&version("0.8.0-beta.1"))),
            ["0.8.0-beta.2"]
        );
    }

    #[test]
    fn a_beta_is_offered_its_own_stable_release() {
        let feed = feed_of(&[entry("0.6.0"), entry("0.5.0")]);
        assert_eq!(versions(&feed.verdict(&version("0.6.0-beta.1"))), ["0.6.0"]);
    }

    #[test]
    fn the_running_version_and_older_ones_are_up_to_date() {
        let feed = feed_of(&[entry("0.6.0"), entry("0.5.0")]);
        assert_eq!(feed.verdict(&version("0.6.0")), Verdict::UpToDate);
        assert_eq!(feed.verdict(&version("0.6.1")), Verdict::UpToDate);
        assert_eq!(feed_of(&[]).verdict(&version("0.6.0")), Verdict::UpToDate);
    }

    #[test]
    fn what_changed_runs_from_the_offer_down_to_the_running_version_whatever_the_order() {
        let feed = feed_of(&[
            entry("0.6.1"),
            entry("0.5.0"),
            entry("0.8.0"),
            entry("0.6.0"),
            entry("0.7.0"),
        ]);
        assert_eq!(
            versions(&feed.verdict(&version("0.6.0"))),
            ["0.8.0", "0.7.0", "0.6.1"]
        );
    }

    #[test]
    fn build_metadata_does_not_make_a_release_newer() {
        let feed = feed_of(&[entry("0.6.0+rebuild.2"), entry("0.6.0")]);
        assert_eq!(feed.verdict(&version("0.6.0")), Verdict::UpToDate);
        assert_eq!(
            versions(&feed.verdict(&version("0.5.0"))),
            ["0.6.0+rebuild.2"]
        );
    }
}
