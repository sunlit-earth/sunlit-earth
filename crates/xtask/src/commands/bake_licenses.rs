//! `cargo xtask bake licenses`: the dependency tree to the two notices it owes.
//!
//! Every license in the tree except the handful that ask for nothing wants its
//! notice carried by whoever distributes the binary, and until this existed
//! nothing in the tree carried one. Two files come out of here. `assets/
//! third-party.md` is the compact list the About window's third tab renders,
//! one line per crate with each license identifier linked to its SPDX page.
//! `THIRD-PARTY-LICENSES.md` is the notice itself: every license and notice
//! file every shipping package carries, verbatim, with the per-crate copyright
//! lines that are the part MIT and BSD actually require and that no canonical
//! text can supply.
//!
//! Both are committed rather than generated during `cargo build`, following
//! `bake_icon` and `bake_stars`: a build gains no metadata walk and nothing
//! runs one on a user's machine. [`render_list`] and [`render_notices`] are
//! pure functions of a collected tree, which is what lets
//! `the_committed_bake_matches_a_fresh_one` compare the files on disk against a
//! fresh walk and fail when a dependency was added without rerunning the bake.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::runner::{Cmd, Runner};
use crate::store;
use crate::util;

/// The triples this project ships, which are what the tree is resolved for.
///
/// Decision 8: one list for every platform, and it is the union over these
/// three rather than an unfiltered walk. An unfiltered walk drags in the wasm,
/// Android and Redox branches, which ship nowhere; a per-target list would
/// mean three files, three `include_str!`s and a `cfg` in the app for no gain.
/// Where a crate is in the list but not in one particular build, that is the
/// direction to err.
pub const SHIPPING_TARGETS: [&str; 3] = [
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "aarch64-apple-darwin",
];

/// The compact list the About window renders, relative to the repository root.
pub const LIST_PATH: &str = "assets/third-party.md";

/// The notice that travels in the release bundle, relative to the same root.
///
/// It lives under `assets/` because it is a generated document a megabyte
/// long, and the repository root is not where that belongs.
pub const NOTICES_PATH: &str = "assets/THIRD-PARTY-LICENSES.md";

/// What the same file is called inside the bundle, where it sits at the top
/// level next to `LICENSE`: the first place someone unpacking a release looks
/// for a notice, and no reason to make them open a directory for it.
pub const NOTICES_NAME: &str = "THIRD-PARTY-LICENSES.md";

/// Where an unrecognized `LicenseRef-` operand sends a reader instead of a
/// link, since SPDX has no page for one.
const NO_SPDX_PAGE: &str = "no SPDX page";

/// What a package's line says when the package carries no notice of its own.
///
/// Named rather than spelled twice: a test looking for this phrase against a
/// generator that emits a different one is a test that cannot fail.
const SILENT: &str = "vendors no license text";

/// File-name prefixes that mean "this file is part of the grant".
///
/// Prefixes rather than exact names because the tree spells them every way
/// there is: `LICENSE-MIT`, `LICENSE.APACHE`, `license-mit`, `COPYING.LESSER`,
/// `LICENSE-Apache-2.0_WITH_LLVM-exception`. `PATENTS` is here because the
/// grants derived from Go's carry the patent grant in a separate file, and
/// `NOTICE` because Apache-2.0 section 4(d) asks for it by name.
const NOTICE_PREFIXES: [&str; 7] = [
    "LICENSE",
    "LICENCE",
    "COPYING",
    "COPYRIGHT",
    "NOTICE",
    "UNLICENSE",
    "PATENTS",
];

/// A directory whose whole contents are the grant, which is the layout
/// REUSE-compliant crates use. Slint's is the one in this tree.
const NOTICE_DIR: &str = "LICENSES";

/// Packages that carry a notice somewhere other than a file named like one.
///
/// A notice is found by file name, which is how every package in this tree but
/// one carries it. `astronomy-engine-bindings` vendors Don Cross's C sources
/// and their MIT notice lives in the leading comment of `astronomy.h` and
/// nowhere else, so a name-only search reports the package as shipping no
/// notice when in fact it ships one this program owes. The entry names the
/// file; a bake that cannot find the notice in it fails rather than quietly
/// writing a gap into the committed file.
const VENDORED_NOTICES: [(&str, &str); 1] = [(
    "astronomy-engine-bindings",
    "astronomy/source/c/astronomy.h",
)];

/// One license or notice file a package ships.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Its name inside the package, with a forward slash for a file inside
    /// `LICENSES/`.
    pub file: String,
    /// Its contents, with line endings normalized to LF and a UTF-8 byte order
    /// mark removed. Otherwise verbatim: this is the notice.
    pub text: String,
}

/// One third-party crate that reaches the binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub version: String,
    /// The SPDX expression from the manifest, normalized by
    /// [`normalize_expression`]. Empty when the manifest declares none, which
    /// is what a `license-file`-only package looks like.
    pub license: String,
    pub repository: Option<String>,
    pub authors: Vec<String>,
    pub notices: Vec<Notice>,
}

/// What one `cargo metadata` run says about one package, before its notice
/// files are read.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Resolved {
    name: String,
    version: String,
    license: String,
    repository: Option<String>,
    authors: Vec<String>,
    manifest_path: PathBuf,
}

mod metadata {
    //! The subset of `cargo metadata --format-version 1` this walk reads.

    use serde::Deserialize;

    #[derive(Deserialize)]
    pub struct Metadata {
        pub packages: Vec<Package>,
        pub workspace_members: Vec<String>,
    }

    #[derive(Deserialize)]
    pub struct Package {
        pub id: String,
        pub name: String,
        pub version: String,
        pub license: Option<String>,
        pub repository: Option<String>,
        #[serde(default)]
        pub authors: Vec<String>,
        pub manifest_path: String,
    }
}

/// The command that names the crates one triple's release build links.
///
/// `cargo tree` rather than `cargo metadata --filter-platform`, which is what
/// decision 8 named. The resolve `cargo metadata` prints is feature-unified
/// across dependency kinds, so an optional dependency that only a
/// dev-dependency turns on appears in it as an ordinary normal edge: the app's
/// `i-slint-backend-testing` switches on `i-slint-backend-selector`'s feature
/// of the same name, and a normal-edge walk of that resolve therefore reaches
/// the testing backend, which no release build links. `cargo tree --edges
/// normal` resolves features per kind and does not.
///
/// `--locked` so a bake can never be the thing that updates `Cargo.lock`: the
/// committed files are a function of the committed lockfile and nothing else.
pub fn tree_cmd(cargo: &str, repo: &Path, target: &str) -> Cmd {
    Cmd::new(cargo)
        .args([
            "tree",
            "--locked",
            "--package",
            "sunlit-earth",
            "--edges",
            "normal",
            "--target",
            target,
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .cwd(repo)
}

/// The command that describes every package the lockfile holds.
///
/// Unfiltered, because the three trees between them name crates from all three
/// platforms and this is only the lookup table the tree walks are resolved
/// against: license, repository and the source directory the notices are read
/// from.
pub fn metadata_cmd(cargo: &str, repo: &Path) -> Cmd {
    Cmd::new(cargo)
        .args(["metadata", "--format-version", "1", "--locked"])
        .cwd(repo)
}

/// The name and version of every crate one `cargo tree` answer names.
///
/// A `{p}` line is `name vX.Y.Z` and then, for some crates, ` (proc-macro)`,
/// ` (*)` for a subtree printed elsewhere, or a path for a workspace member.
/// A crate name cannot contain a space, so the first two words are the pair
/// and the rest is decoration.
fn parse_tree(stdout: &str) -> Result<Vec<(String, String)>, String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (name, rest) = line
                .split_once(' ')
                .ok_or_else(|| format!("cargo tree printed {line:?}, which names no version"))?;
            let version = rest
                .split(' ')
                .next()
                .and_then(|word| word.strip_prefix('v'))
                .ok_or_else(|| format!("cargo tree printed {line:?}, which names no version"))?;
            Ok((name.to_owned(), version.to_owned()))
        })
        .collect()
}

/// The cargo to call, which is the one running this xtask when there is one.
fn cargo_program() -> String {
    util::env_var("CARGO").unwrap_or_else(|| "cargo".to_owned())
}

/// Every package the lockfile holds, keyed by name and version, and which of
/// them are this workspace's own.
///
/// A workspace member is code this repository wrote and licenses itself, so it
/// is traversed by `cargo tree` and then dropped here: `sunlit-core` is in the
/// binary, but it is not third-party.
type Catalog = (BTreeMap<(String, String), Resolved>, Vec<(String, String)>);

fn parse_catalog(json: &str) -> Result<Catalog, String> {
    let meta: metadata::Metadata =
        serde_json::from_str(json).map_err(|e| format!("cargo metadata did not parse: {e}"))?;
    let mut catalog = BTreeMap::new();
    let mut members = Vec::new();
    for package in &meta.packages {
        let key = (package.name.clone(), package.version.clone());
        if meta.workspace_members.contains(&package.id) {
            members.push(key.clone());
        }
        catalog.insert(
            key,
            Resolved {
                name: package.name.clone(),
                version: package.version.clone(),
                license: package
                    .license
                    .as_deref()
                    .map(normalize_expression)
                    .unwrap_or_default(),
                repository: package.repository.clone(),
                authors: package.authors.clone(),
                manifest_path: PathBuf::from(&package.manifest_path),
            },
        );
    }
    if catalog.is_empty() {
        return Err("cargo metadata described no packages at all".to_owned());
    }
    Ok((catalog, members))
}

/// A legacy license expression in the spelling SPDX actually defines.
///
/// The tree carries four spellings of one thing (`MIT/Apache-2.0`,
/// `MIT / Apache-2.0`, `Apache-2.0/MIT`, `BSD-3-Clause/MIT`), which cargo
/// accepted before it wanted SPDX. No identifier contains a slash, so a slash
/// is always the disjunction operator.
pub fn normalize_expression(license: &str) -> String {
    license
        .replace('/', " OR ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where the canonical text of one identifier lives, or `None` when nothing
/// derivable does.
///
/// Decision 9: one rule for every identifier rather than a table to maintain.
/// SPDX has a page per identifier by construction, so the URL is derived from
/// the identifier and a license nobody anticipated gets a working link with no
/// code change. `LicenseRef-Slint-*` is the exception the derivation gets
/// wrong: it is not an SPDX identifier and has no page, so those operands point
/// at Slint's own license files, which are named after the identifiers. Any
/// other `LicenseRef-` gets no link at all rather than a guess.
pub fn license_url(identifier: &str) -> Option<String> {
    if identifier.starts_with("LicenseRef-Slint-") {
        Some(format!(
            "https://github.com/slint-ui/slint/blob/master/LICENSES/{identifier}.md"
        ))
    } else if identifier.starts_with("LicenseRef-") {
        None
    } else {
        Some(format!("https://spdx.org/licenses/{identifier}.html"))
    }
}

/// The expression with every operand turned into a markdown link.
///
/// The operators are kept where they are, so `MIT OR Apache-2.0` reads as two
/// links joined by `OR` rather than as one link that would have to lie about
/// one of them, and `Apache-2.0 WITH LLVM-exception` links the exception too,
/// which has an SPDX page of its own.
pub fn link_expression(license: &str) -> String {
    if license.is_empty() {
        return "license not declared in the manifest".to_owned();
    }
    let mut out = String::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.is_empty() {
            return;
        }
        if matches!(word.to_ascii_uppercase().as_str(), "OR" | "AND" | "WITH") {
            out.push_str(word);
        } else if let Some(url) = license_url(word) {
            out.push('[');
            out.push_str(word);
            out.push_str("](");
            out.push_str(&url);
            out.push(')');
        } else {
            out.push_str(word);
            out.push_str(" (");
            out.push_str(NO_SPDX_PAGE);
            out.push(')');
        }
        word.clear();
    };
    for character in license.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '+' | '_') {
            word.push(character);
        } else {
            flush(&mut word, &mut out);
            out.push(character);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// The leading `/* ... */` of a C source, markers and indentation included.
///
/// Verbatim rather than dedented and unwrapped: this is a copyright notice,
/// and the fewer transformations between the file and the committed bytes the
/// better.
fn leading_block_comment(text: &str) -> Option<&str> {
    let start = text.find("/*")?;
    if !text[..start].trim().is_empty() {
        return None;
    }
    let end = text[start..].find("*/")? + start + 2;
    Some(&text[start..end])
}

/// The notice files one package directory carries, in name order.
fn read_notices(name: &str, manifest_path: &Path) -> Result<Vec<Notice>, String> {
    let dir = manifest_path.parent().ok_or_else(|| {
        format!(
            "{} has no parent directory to read notices from",
            manifest_path.display()
        )
    })?;
    let mut paths: Vec<(String, PathBuf)> = Vec::new();
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot list {}: {e}", dir.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let upper = name.to_ascii_uppercase();
        let is_dir = entry.path().is_dir();
        if is_dir && upper == NOTICE_DIR {
            let nested = std::fs::read_dir(entry.path())
                .map_err(|e| format!("cannot list {}: {e}", entry.path().display()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("cannot list {}: {e}", entry.path().display()))?;
            for inner in nested {
                if inner.path().is_file() {
                    paths.push((
                        format!("{name}/{}", inner.file_name().to_string_lossy()),
                        inner.path(),
                    ));
                }
            }
        } else if !is_dir && NOTICE_PREFIXES.iter().any(|p| upper.starts_with(p)) {
            paths.push((name, entry.path()));
        }
    }
    paths.sort();
    let mut notices: Vec<Notice> = paths
        .into_iter()
        .map(|(file, path)| {
            let bytes =
                std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            Ok::<Notice, String>(Notice {
                file,
                text: normalize_text(&decode(&bytes)),
            })
        })
        .collect::<Result<_, _>>()?;

    for (package, relative) in VENDORED_NOTICES {
        if package != name {
            continue;
        }
        let path = dir.join(relative);
        let bytes = std::fs::read(&path).map_err(|e| {
            format!(
                "{package} is expected to carry its notice in {}: {e}",
                path.display()
            )
        })?;
        let text = decode(&bytes);
        let comment = leading_block_comment(&text).ok_or_else(|| {
            format!(
                "{} does not start with the comment {package}'s notice is in",
                path.display()
            )
        })?;
        notices.push(Notice {
            file: format!("{relative} (leading comment)"),
            text: normalize_text(comment),
        });
    }
    Ok(notices)
}

/// One notice file's bytes as text.
///
/// Almost every file in the tree is UTF-8, and `rav1e`'s `PATENTS` is not: it
/// carries typographic quotes as single Windows-1252 bytes. A lossy UTF-8
/// decode would put replacement characters where the quotes are, which is a
/// corrupted legal text, so the fallback decodes the whole file as
/// Windows-1252. That mapping is total, so it cannot fail, and for a file that
/// really is UTF-8 it never runs.
fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => bytes.iter().map(|byte| cp1252(*byte)).collect(),
    }
}

/// Windows-1252 for one byte. Identical to Latin-1 except for `0x80` to
/// `0x9F`, where Latin-1 has unprintable controls and Windows-1252 has the
/// punctuation this fallback exists for.
fn cp1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{81}', '\u{201A}', '\u{192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{2C6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8D}', '\u{17D}',
        '\u{8F}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}',
        '\u{2014}', '\u{2DC}', '\u{2122}', '\u{161}', '\u{203A}', '\u{153}', '\u{9D}', '\u{17E}',
        '\u{178}',
    ];
    if (0x80..=0x9F).contains(&byte) {
        HIGH[usize::from(byte) - 0x80]
    } else {
        char::from(byte)
    }
}

/// A notice's bytes as they go into a committed LF file.
fn normalize_text(text: &str) -> String {
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut normalized = body.replace("\r\n", "\n").replace('\r', "\n");
    while normalized.ends_with('\n') {
        normalized.pop();
    }
    normalized.push('\n');
    normalized
}

/// Sort key that orders `0.9.4` before `0.10.1`.
///
/// A string sort on versions puts a two-digit minor before a one-digit one,
/// and this list carries several crates at two majors at once, so the numeric
/// segments are compared as numbers. The trailing string keeps prereleases
/// deterministic without pretending to implement semver precedence.
fn version_key(version: &str) -> (Vec<u64>, String) {
    let numbers = version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect();
    (numbers, version.to_owned())
}

/// Walk the three trees, union them, and read every notice file.
pub fn collect(runner: &dyn Runner, repo: &Path) -> Result<Vec<Package>, String> {
    let cargo = cargo_program();
    let metadata = runner
        .capture(&metadata_cmd(&cargo, repo))
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !metadata.success() {
        return Err(format!("cargo metadata failed: {}", metadata.stderr.trim()));
    }
    let (catalog, members) = parse_catalog(&metadata.stdout)?;

    let mut shipping: BTreeMap<(String, String), Resolved> = BTreeMap::new();
    for target in SHIPPING_TARGETS {
        let output = runner
            .capture(&tree_cmd(&cargo, repo, target))
            .map_err(|e| format!("cannot run cargo tree for {target}: {e}"))?;
        if !output.success() {
            return Err(format!(
                "cargo tree for {target} failed: {}",
                output.stderr.trim()
            ));
        }
        for key in parse_tree(&output.stdout)? {
            if members.contains(&key) {
                continue;
            }
            let resolved = catalog.get(&key).ok_or_else(|| {
                format!(
                    "cargo tree named {} {} for {target}, which cargo metadata does not describe",
                    key.0, key.1
                )
            })?;
            shipping.insert(key, resolved.clone());
        }
    }
    if shipping.is_empty() {
        return Err("the shipping tree resolved to no third-party crates at all".to_owned());
    }

    let mut packages: Vec<Package> = shipping
        .into_values()
        .map(|package| {
            let notices = read_notices(&package.name, &package.manifest_path)?;
            Ok(Package {
                name: package.name,
                version: package.version,
                license: package.license,
                repository: package.repository,
                authors: package.authors,
                notices,
            })
        })
        .collect::<Result<_, String>>()?;
    packages.sort_by(|a, b| {
        (a.name.to_ascii_lowercase(), version_key(&a.version))
            .cmp(&(b.name.to_ascii_lowercase(), version_key(&b.version)))
    });
    Ok(packages)
}

/// The compact list the About window's third tab renders.
///
/// Markdown that the app's own preprocessor and `StyledText::from_markdown`
/// both accept: a heading, two paragraphs and one list item per crate. No
/// table, no code block, no horizontal rule, because the parser rejects all
/// three.
pub fn render_list(packages: &[Package]) -> String {
    let mut out = String::new();
    out.push_str("# Third-party crates\n\n");
    out.push_str(
        "These are the crates Sunlit Earth depends on. The list is the union over the three \
         platforms this project builds for, and it names every normal dependency edge, so a \
         crate here may be absent from the build you are running or may be one a procedural \
         macro used at build time. It errs that way on purpose: a notice for a crate that is \
         not in your copy misleads nobody, a missing notice for one that is does.\n\n",
    );
    out.push_str(
        "Each license name links to its SPDX page, which carries the canonical text. The \
         copyright lines, which differ crate by crate and are the part the MIT and BSD licenses \
         actually require, are in `THIRD-PARTY-LICENSES.md` beside the program.\n\n",
    );
    for package in packages {
        let _ = writeln!(
            out,
            "- `{} {}`: {}",
            package.name,
            package.version,
            link_expression(&package.license)
        );
    }
    out
}

/// A fence long enough that nothing inside the text can close it.
fn fence_for(text: &str) -> String {
    let longest = text
        .lines()
        .map(|line| line.trim_start().chars().take_while(|c| *c == '`').count())
        .max()
        .unwrap_or(0);
    "`".repeat(longest.max(2) + 1)
}

/// The notice that travels with the binary.
///
/// Identical texts are carried once and referenced by number, which is what
/// takes the file from five megabytes to under one: the Apache-2.0 text is
/// byte for byte the same in nearly three hundred packages. Nothing is
/// summarized or substituted; a package that ships no notice file is said to
/// ship none rather than being given somebody else's copyright line.
pub fn render_notices(packages: &[Package]) -> String {
    let mut texts: Vec<(&str, Vec<String>)> = Vec::new();
    let mut references: Vec<Vec<(usize, &str)>> = Vec::new();
    for package in packages {
        let mut mine = Vec::new();
        for notice in &package.notices {
            let index = texts
                .iter()
                .position(|(text, _)| *text == notice.text.as_str())
                .unwrap_or_else(|| {
                    texts.push((notice.text.as_str(), Vec::new()));
                    texts.len() - 1
                });
            texts[index]
                .1
                .push(format!("{} {}", package.name, package.version));
            mine.push((index + 1, notice.file.as_str()));
        }
        references.push(mine);
    }
    let silent = packages
        .iter()
        .filter(|package| package.notices.is_empty())
        .count();

    let mut out = String::new();
    out.push_str("# Third-party licenses\n\n");
    out.push_str(
        "Sunlit Earth is licensed under the GPL-3.0-or-later, whose text is in `LICENSE` beside \
         this file. This file is the other half of the obligation: the licenses and copyright \
         notices of the crates it depends on.\n\n",
    );
    let _ = write!(
        out,
        "It covers {} crates. {} of them vendor no license text of their own, so no copyright \
         line of theirs can be reproduced here; they have their own section below, carrying what \
         their manifests do say. Nothing in this file is summarized, paraphrased, or supplied \
         from a canonical text on a package's behalf.\n\n",
        packages.len(),
        silent
    );
    let _ = write!(
        out,
        "It is generated by `cargo xtask bake licenses` from the committed `Cargo.lock`, as the \
         union over {} of `cargo tree --edges normal`, which is the set of crates a release \
         build of each platform links. Build-dependencies, dev-dependencies and this \
         repository's own crates are not in it, and neither is the effect of feature unification \
         across dependency kinds, which is why the walk is `cargo tree` rather than the resolve \
         graph `cargo metadata` prints. What the set does include is the crates a procedural \
         macro used at build time, which are reachable by a normal edge and are not in the \
         binary: it errs toward carrying a notice nobody needs rather than omitting one somebody \
         does.\n\n",
        SHIPPING_TARGETS.join(", ")
    );
    out.push_str(
        "The Packages section names each crate, its license expression, its repository and the \
         numbers of the license texts it ships. The License texts section carries those texts \
         verbatim, each once, naming the packages that ship it byte for byte.\n\n",
    );

    out.push_str("## Packages\n\n");
    for (package, mine) in packages.iter().zip(&references) {
        if mine.is_empty() {
            continue;
        }
        out.push_str(&describe(package));
        let listed: Vec<String> = mine
            .iter()
            .map(|(number, file)| format!("{file} is text {number}"))
            .collect();
        let _ = writeln!(out, ", {}", listed.join(", "));
    }

    out.push_str("\n## Packages that vendor no license text\n\n");
    out.push_str(
        "These crates declare a license in their manifest and ship no copy of it and no \
         copyright line. What follows is what their manifests say and nothing more: the \
         identifier, the authors field where there is one, and the repository. The terms are the \
         canonical text of the identifier, at `https://spdx.org/licenses/<identifier>.html`, and \
         in most cases that same text is already reproduced below from another package. The \
         copyright holder is not reproduced, because the package states none and a guess would \
         put a fabricated attribution in a legal notice.\n\n",
    );
    for package in packages.iter().filter(|p| p.notices.is_empty()) {
        out.push_str(&describe(package));
        if package.authors.is_empty() {
            out.push_str(", no authors field");
        } else {
            let _ = write!(out, ", authors: {}", package.authors.join("; "));
        }
        let _ = writeln!(out, ", {SILENT}");
    }

    out.push_str("\n## License texts\n\n");
    for (index, (text, holders)) in texts.iter().enumerate() {
        let _ = write!(out, "### Text {}\n\n", index + 1);
        let _ = write!(out, "Shipped by: {}\n\n", holders.join(", "));
        let fence = fence_for(text);
        out.push_str(&fence);
        out.push('\n');
        out.push_str(text);
        out.push_str(&fence);
        out.push_str("\n\n");
    }
    out
}

/// The half of a package's line that is the same in both sections.
fn describe(package: &Package) -> String {
    let mut line = format!("- `{} {}`", package.name, package.version);
    if package.license.is_empty() {
        line.push_str(", license not declared in the manifest");
    } else {
        let _ = write!(line, ", {}", package.license);
    }
    if let Some(repository) = &package.repository {
        let _ = write!(line, ", {repository}");
    }
    line
}

/// Write both files into the repository.
pub fn run(runner: &dyn Runner) -> Result<u8, String> {
    let repo = store::repo_root();
    let packages = collect(runner, &repo)?;
    let list = render_list(&packages);
    let notices = render_notices(&packages);
    for (relative, contents) in [(LIST_PATH, &list), (NOTICES_PATH, &notices)] {
        let path = repo.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, contents)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        println!("wrote {} ({} bytes)", path.display(), contents.len());
    }
    println!(
        "{} third-party crates across {}",
        packages.len(),
        SHIPPING_TARGETS.join(", ")
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::RealRunner;

    fn package(name: &str, version: &str, license: &str) -> Package {
        Package {
            name: name.to_owned(),
            version: version.to_owned(),
            license: normalize_expression(license),
            repository: None,
            authors: Vec::new(),
            notices: Vec::new(),
        }
    }

    /// A fabricated `cargo metadata` answer: the two workspace members and
    /// three registry crates, one of them with a legacy license spelling.
    fn fabricated_metadata() -> String {
        serde_json::json!({
            "packages": [
                {"id": "app", "name": "sunlit-earth", "version": "0.1.0",
                 "license": "GPL-3.0-or-later", "repository": null, "authors": [],
                 "manifest_path": "/repo/crates/sunlit-app/Cargo.toml"},
                {"id": "core", "name": "sunlit-core", "version": "0.1.0",
                 "license": "GPL-3.0-or-later", "repository": null, "authors": [],
                 "manifest_path": "/repo/crates/sunlit-core/Cargo.toml"},
                {"id": "normal", "name": "shipped", "version": "1.0.0",
                 "license": "MIT/Apache-2.0", "repository": "https://example.invalid/shipped",
                 "authors": ["A"], "manifest_path": "/registry/shipped-1.0.0/Cargo.toml"},
                {"id": "silent", "name": "quiet", "version": "0.2.0",
                 "license": null, "repository": null, "authors": [],
                 "manifest_path": "/registry/quiet-0.2.0/Cargo.toml"}
            ],
            "workspace_members": ["app", "core"]
        })
        .to_string()
    }

    #[test]
    fn the_catalog_covers_every_package_and_names_the_workspace_members() {
        let (catalog, members) = parse_catalog(&fabricated_metadata()).expect("the fabrication");
        assert_eq!(catalog.len(), 4);
        assert_eq!(
            members,
            vec![
                ("sunlit-earth".to_owned(), "0.1.0".to_owned()),
                ("sunlit-core".to_owned(), "0.1.0".to_owned())
            ]
        );
    }

    #[test]
    fn the_catalog_normalizes_a_legacy_expression_and_tolerates_a_missing_one() {
        let (catalog, _) = parse_catalog(&fabricated_metadata()).expect("the fabrication");
        let shipped = &catalog[&("shipped".to_owned(), "1.0.0".to_owned())];
        assert_eq!(shipped.license, "MIT OR Apache-2.0");
        let quiet = &catalog[&("quiet".to_owned(), "0.2.0".to_owned())];
        assert_eq!(quiet.license, "");
    }

    /// `cargo tree --format {p}` decorates a line three ways: a workspace
    /// member carries its path, a proc-macro says so, and a subtree printed
    /// elsewhere is `(*)`. The pair is the first two words either way.
    #[test]
    fn a_tree_line_yields_its_name_and_version_whatever_follows_them() {
        let stdout = concat!(
            "sunlit-earth v0.1.0-beta.1 (/repo/crates/sunlit-app)\n",
            "serde_derive v1.0.228 (proc-macro)\n",
            "syn v2.0.108 (*)\n",
            "\n",
            "wgpu v28.0.0\n"
        );
        assert_eq!(
            parse_tree(stdout).expect("the fabricated tree"),
            vec![
                ("sunlit-earth".to_owned(), "0.1.0-beta.1".to_owned()),
                ("serde_derive".to_owned(), "1.0.228".to_owned()),
                ("syn".to_owned(), "2.0.108".to_owned()),
                ("wgpu".to_owned(), "28.0.0".to_owned()),
            ]
        );
    }

    #[test]
    fn a_tree_line_with_no_version_is_an_error_rather_than_a_guess() {
        assert!(parse_tree("something-odd\n").is_err());
        assert!(parse_tree("name 1.2.3\n").is_err());
    }

    #[test]
    fn the_legacy_slash_spellings_normalize_to_or() {
        for legacy in [
            "MIT/Apache-2.0",
            "MIT / Apache-2.0",
            "Apache-2.0 / MIT",
            "Apache-2.0/MIT",
            "BSD-3-Clause/MIT",
        ] {
            let normalized = normalize_expression(legacy);
            assert!(
                !normalized.contains('/'),
                "{legacy} normalized to {normalized}"
            );
            assert_eq!(normalized.split(" OR ").count(), 2, "{normalized}");
        }
    }

    #[test]
    fn every_operand_of_an_expression_becomes_its_own_link() {
        let linked = link_expression("MIT OR Apache-2.0");
        assert_eq!(
            linked,
            "[MIT](https://spdx.org/licenses/MIT.html) OR \
             [Apache-2.0](https://spdx.org/licenses/Apache-2.0.html)"
        );
    }

    #[test]
    fn a_conjunction_links_both_operands_rather_than_choosing() {
        let linked = link_expression("Apache-2.0 AND ISC");
        assert!(linked.contains("[Apache-2.0](https://spdx.org/licenses/Apache-2.0.html)"));
        assert!(linked.contains("[ISC](https://spdx.org/licenses/ISC.html)"));
        assert!(linked.contains(" AND "), "{linked}");
    }

    #[test]
    fn an_exception_and_a_parenthesized_expression_keep_their_shape() {
        let with = link_expression("Apache-2.0 WITH LLVM-exception OR MIT");
        assert!(
            with.contains("WITH [LLVM-exception](https://spdx.org/licenses/LLVM-exception.html)")
        );
        let grouped = link_expression("(MIT OR Apache-2.0) AND Unicode-3.0");
        assert!(grouped.starts_with("([MIT]"), "{grouped}");
        assert!(grouped.contains(") AND [Unicode-3.0]"), "{grouped}");
    }

    /// The one case the derived URL gets wrong: `LicenseRef-*` is not an SPDX
    /// identifier and spdx.org has no page for it, so Slint's operands go to
    /// Slint's own license files and anything else gets no link at all rather
    /// than a dead one.
    #[test]
    fn the_slint_license_refs_point_at_slints_own_files() {
        let linked = link_expression(
            "GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0",
        );
        assert!(linked.contains("[GPL-3.0-only](https://spdx.org/licenses/GPL-3.0-only.html)"));
        assert!(linked.contains(
            "[LicenseRef-Slint-Royalty-free-2.0](https://github.com/slint-ui/slint/blob/master/\
             LICENSES/LicenseRef-Slint-Royalty-free-2.0.md)"
        ));
        assert!(!linked.contains("spdx.org/licenses/LicenseRef"), "{linked}");
    }

    #[test]
    fn an_unknown_license_ref_gets_no_link_at_all() {
        let linked = link_expression("LicenseRef-Something-Else");
        assert!(!linked.contains("http"), "{linked}");
        assert!(linked.contains(NO_SPDX_PAGE), "{linked}");
    }

    #[test]
    fn a_manifest_with_no_license_field_says_so_instead_of_linking_nothing() {
        let linked = link_expression("");
        assert!(!linked.contains('['), "{linked}");
        assert!(linked.contains("not declared"), "{linked}");
    }

    #[test]
    fn the_rendered_list_has_one_line_per_crate_and_no_rejected_construct() {
        let packages = vec![
            package("alpha", "1.0.0", "MIT"),
            package("beta", "2.0.0", "Apache-2.0"),
        ];
        let list = render_list(&packages);
        assert_eq!(list.matches("\n- `").count(), 2, "{list}");
        assert!(
            !list.contains("```"),
            "the tab's parser rejects code blocks"
        );
        assert!(!list.contains("\n---"), "the tab's parser rejects rules");
        assert!(!list.contains('|'), "the tab's parser rejects tables");
    }

    #[test]
    fn an_identical_text_is_carried_once_and_referenced_twice() {
        let shared = Notice {
            file: "LICENSE-APACHE".to_owned(),
            text: "the same words\n".to_owned(),
        };
        let mut first = package("alpha", "1.0.0", "Apache-2.0");
        first.notices = vec![shared.clone()];
        let mut second = package("beta", "2.0.0", "Apache-2.0");
        second.notices = vec![shared];
        let notices = render_notices(&[first, second]);
        assert_eq!(notices.matches("the same words").count(), 1, "{notices}");
        assert_eq!(notices.matches("is text 1").count(), 2, "{notices}");
        assert!(
            notices.contains("Shipped by: alpha 1.0.0, beta 2.0.0"),
            "{notices}"
        );
    }

    #[test]
    fn a_fence_is_longer_than_any_backtick_run_inside_the_text() {
        assert_eq!(fence_for("plain\n"), "```");
        assert_eq!(fence_for("``` inside\n"), "````");
        assert_eq!(fence_for("  ````` indented\n"), "``````");
    }

    #[test]
    fn a_leading_c_comment_is_taken_verbatim_and_only_when_it_leads() {
        let header = "/*\n    MIT License\n\n    Copyright (c) X\n*/\n\n#ifndef H\n";
        assert_eq!(
            leading_block_comment(header),
            Some("/*\n    MIT License\n\n    Copyright (c) X\n*/")
        );
        assert_eq!(leading_block_comment("#include <x.h>\n/* later */"), None);
        assert_eq!(leading_block_comment("/* unterminated"), None);
    }

    /// `astronomy-engine-bindings` is MIT and ships no file named like a
    /// license, so a name-only search reports the one library the rendered
    /// geometry depends on as vendoring no notice.
    #[test]
    fn the_astronomy_engine_notice_reaches_the_committed_file() {
        let path = store::repo_root().join(NOTICES_PATH);
        let notices = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        let line = notices
            .lines()
            .find(|line| line.starts_with("- `astronomy-engine-bindings "))
            .expect("the crate is not in the notice file at all");
        assert!(
            !line.contains(SILENT),
            "the vendored notice was not found: {line}"
        );
        assert!(
            notices.contains("Copyright (c) 2019-2023 Don Cross"),
            "Don Cross's copyright line is not in the notice file"
        );
    }

    #[test]
    fn a_notice_is_normalized_to_lf_with_one_trailing_newline() {
        assert_eq!(normalize_text("\u{feff}a\r\nb\r\n\r\n"), "a\nb\n");
        assert_eq!(normalize_text("a"), "a\n");
    }

    #[test]
    fn versions_sort_by_number_rather_than_by_string() {
        let mut versions = ["0.10.1", "0.9.4", "1.0.0-rc.1", "1.0.0"];
        versions.sort_by_key(|v| version_key(v));
        assert_eq!(versions, ["0.9.4", "0.10.1", "1.0.0", "1.0.0-rc.1"]);
    }

    #[test]
    fn both_commands_are_locked_and_the_tree_is_filtered_by_kind_and_target() {
        assert!(
            metadata_cmd("cargo", Path::new("/repo"))
                .args
                .contains(&"--locked".to_owned())
        );
        let tree = tree_cmd("cargo", Path::new("/repo"), "x86_64-pc-windows-msvc");
        assert!(
            tree.args.contains(&"--locked".to_owned()),
            "{:?}",
            tree.args
        );
        for pair in [
            ["--edges", "normal"],
            ["--target", "x86_64-pc-windows-msvc"],
            ["--package", "sunlit-earth"],
        ] {
            let pair = pair.map(str::to_owned);
            assert!(
                tree.args.windows(2).any(|window| window == pair),
                "{pair:?} is missing from {:?}",
                tree.args
            );
        }
    }

    /// A package that vendors nothing goes in its own section with what its
    /// manifest says, and nothing is invented to fill the gap.
    #[test]
    fn a_package_that_vendors_nothing_carries_its_manifest_and_no_more() {
        let mut bare = package("bare", "1.0.0", "MIT");
        bare.authors = vec!["Someone <s@example.invalid>".to_owned()];
        let notices = render_notices(&[bare]);
        assert!(
            notices.contains("## Packages that vendor no license text"),
            "{notices}"
        );
        assert!(
            notices.contains(
                "- `bare 1.0.0`, MIT, authors: Someone <s@example.invalid>, \
                 vendors no license text"
            ),
            "{notices}"
        );
        assert!(
            !notices.contains("Copyright (c)"),
            "a copyright line was synthesized"
        );
        assert!(
            notices.contains("1 of them vendor no license text"),
            "{notices}"
        );
    }

    /// The walk needs `cargo metadata`, `cargo tree` and the registry sources
    /// the manifests point at, so a checkout that cannot resolve the tree skips
    /// with a printed reason rather than failing.
    #[test]
    fn the_committed_bake_matches_a_fresh_one() {
        let repo = store::repo_root();
        let packages = match collect(&RealRunner, &repo) {
            Ok(packages) => packages,
            Err(reason) => {
                println!("skipping: the shipping tree could not be resolved: {reason}");
                return;
            }
        };
        for (relative, fresh) in [
            (LIST_PATH, render_list(&packages)),
            (NOTICES_PATH, render_notices(&packages)),
        ] {
            let path = repo.join(relative);
            let committed = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
            assert!(
                committed == fresh,
                "{relative} differs from a fresh bake ({} bytes on disk, {} fresh); \
                 run `cargo xtask bake licenses`",
                committed.len(),
                fresh.len()
            );
        }
    }

    /// The list is what the About window parses, so the crates the attribution
    /// document names individually have to actually be in it, and the ones
    /// that are not in the binary have to stay out.
    #[test]
    fn the_committed_list_names_the_shipping_crates_and_no_others() {
        let path = store::repo_root().join(LIST_PATH);
        let list = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        for present in ["slint", "wgpu", "astronomy-engine-bindings"] {
            assert!(
                list.contains(&format!("- `{present} ")),
                "{present} is missing"
            );
        }
        for absent in [
            "proptest",
            "i-slint-backend-testing",
            "slint-build",
            "embed-resource",
            "xtask",
            "sunlit-core",
            "sunlit-earth",
        ] {
            assert!(
                !list.contains(&format!("- `{absent} ")),
                "{absent} reached the committed list"
            );
        }
    }
}
