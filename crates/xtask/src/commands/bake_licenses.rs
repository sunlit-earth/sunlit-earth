//! `cargo xtask bake licenses`: the dependency tree to the two documents it
//! owes.
//!
//! Every license in the tree except the handful that ask for nothing wants its
//! text carried by whoever distributes the binary, and until this existed
//! nothing in the tree carried one. Two files come out of here.
//! `assets/third-party.md` is the compact list the About window's third tab
//! renders, one line per crate with each license identifier linked to its SPDX
//! page. `assets/THIRD-PARTY-LICENSES.md` is the document that travels in the
//! release archive: the same crates, each identifier linked to a section of
//! that same file, and the canonical text of every identifier the tree
//! references, verbatim and exactly once.
//!
//! The canonical texts come from `assets/licenses/`, a committed corpus of
//! `<identifier>.txt` files, and nothing here fetches anything. An identifier
//! the corpus has no text for fails the bake with the URL to fetch it from and
//! the path to save it at, which is the whole mechanism for adding a license.
//!
//! Both outputs are committed rather than generated during `cargo build`,
//! following `bake_icon` and `bake_stars`: a build gains no metadata walk and
//! nothing runs one on a user's machine. [`render_list`] and [`render_notices`]
//! are pure functions of a collected tree, which is what lets
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

/// The document that travels in the release bundle, relative to the same root.
pub const NOTICES_PATH: &str = "assets/THIRD-PARTY-LICENSES.md";

/// What the same file is called inside the bundle, where it sits at the top
/// level next to `LICENSE`: the first place someone unpacking a release looks
/// for a notice, and no reason to make them open a directory for it.
pub const NOTICES_NAME: &str = "THIRD-PARTY-LICENSES.md";

/// The committed corpus of canonical license texts, one `<identifier>.txt` per
/// identifier, relative to the repository root.
pub const CORPUS_DIR: &str = "assets/licenses";

/// Where an unrecognized `LicenseRef-` operand sends a reader instead of a
/// link, since SPDX has no page for one.
const NO_SPDX_PAGE: &str = "no SPDX page";

/// What a package's line says in place of an expression when its manifest
/// declares none.
const UNDECLARED: &str = "license not declared in the manifest";

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
/// against: the license expression and the repository.
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
type Catalog = (BTreeMap<(String, String), Package>, Vec<(String, String)>);

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
            Package {
                name: package.name.clone(),
                version: package.version.clone(),
                license: package
                    .license
                    .as_deref()
                    .map(normalize_expression)
                    .unwrap_or_default(),
                repository: package.repository.clone(),
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

/// One piece of an SPDX expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    /// A license or exception identifier.
    Operand(&'a str),
    /// An operator, or one character of the punctuation and spacing between
    /// operands, carried through as it was written.
    Other(&'a str),
}

/// An expression split into its operands and everything else.
///
/// One tokenizer for the three things that read an expression: the two link
/// renderers and the walk that collects the identifiers a document needs texts
/// for. A second implementation is a second answer to "what is an operand".
fn tokens(license: &str) -> Vec<Token<'_>> {
    let identifier = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_');
    let mut out = Vec::new();
    let mut start = None;
    for (index, character) in license.char_indices() {
        if identifier(character) {
            start.get_or_insert(index);
        } else {
            if let Some(from) = start.take() {
                out.push(classify(&license[from..index]));
            }
            out.push(Token::Other(&license[index..index + character.len_utf8()]));
        }
    }
    if let Some(from) = start {
        out.push(classify(&license[from..]));
    }
    out
}

fn classify(word: &str) -> Token<'_> {
    if matches!(word.to_ascii_uppercase().as_str(), "OR" | "AND" | "WITH") {
        Token::Other(word)
    } else {
        Token::Operand(word)
    }
}

/// Every identifier an expression names, in the order it names them.
fn identifiers(license: &str) -> Vec<&str> {
    tokens(license)
        .into_iter()
        .filter_map(|token| match token {
            Token::Operand(identifier) => Some(identifier),
            Token::Other(_) => None,
        })
        .collect()
}

/// The expression with each operand rendered by `operand` and the operators,
/// parentheses and spacing left where they are.
fn render_expression(license: &str, operand: impl Fn(&str) -> String) -> String {
    if license.is_empty() {
        return UNDECLARED.to_owned();
    }
    tokens(license)
        .into_iter()
        .map(|token| match token {
            Token::Operand(identifier) => operand(identifier),
            Token::Other(text) => text.to_owned(),
        })
        .collect()
}

/// Where the canonical text of one identifier lives online, or `None` when
/// nothing derivable does.
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

/// The expression with every operand turned into a link to its SPDX page.
///
/// The operators are kept where they are, so `MIT OR Apache-2.0` reads as two
/// links joined by `OR` rather than as one link that would have to lie about
/// one of them, and `Apache-2.0 WITH LLVM-exception` links the exception too,
/// which has an SPDX page of its own.
pub fn link_expression(license: &str) -> String {
    render_expression(license, |identifier| match license_url(identifier) {
        Some(url) => format!("[{identifier}]({url})"),
        None => format!("{identifier} ({NO_SPDX_PAGE})"),
    })
}

/// The expression with every operand linked to the section of the notice
/// document that carries its text.
///
/// An operand with no section is left unlinked rather than pointed at a
/// section that does not exist, which cannot happen for a document built from
/// [`license_texts`] of the same packages and is what
/// `every_operand_link_in_the_committed_document_resolves` would catch.
fn anchor_expression(license: &str, numbers: &BTreeMap<&str, usize>) -> String {
    render_expression(license, |identifier| match numbers.get(identifier) {
        Some(number) => format!("[{identifier}](#license-{number})"),
        None => identifier.to_owned(),
    })
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

/// Walk the three trees and union them.
pub fn collect(runner: &dyn Runner, repo: &Path) -> Result<Vec<Package>, String> {
    let cargo = cargo_program();
    let metadata = runner
        .capture(&metadata_cmd(&cargo, repo))
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !metadata.success() {
        return Err(format!("cargo metadata failed: {}", metadata.stderr.trim()));
    }
    let (catalog, members) = parse_catalog(&metadata.stdout)?;

    let mut shipping: BTreeMap<(String, String), Package> = BTreeMap::new();
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

    let mut packages: Vec<Package> = shipping.into_values().collect();
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
    // No title: the About window renders this document under a tab already
    // labelled Third-party, and a heading repeating the label is noise there.
    let mut out = String::new();
    out.push_str(
        "Every crate Sunlit Earth depends on, as the union over the three platforms it \
         builds for, so a few here are absent from any one build. Each license name links \
         to its SPDX page.\n\n",
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

/// One canonical license text, as the document carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenseText {
    /// The identifier the corpus file is named after.
    pub identifier: String,
    /// The corpus file's contents, verbatim.
    pub text: String,
}

/// The canonical text of every identifier the tree references, in the order
/// the document numbers the sections.
///
/// Case-insensitive by identifier with byte order as the tiebreak, so the two
/// `LicenseRef-Slint-*` texts file with the letter a reader looks for them
/// under rather than ahead of `LLVM-exception`, which is where ASCII would put
/// them.
pub fn license_texts(repo: &Path, packages: &[Package]) -> Result<Vec<LicenseText>, String> {
    let mut referenced: BTreeMap<(String, &str), String> = BTreeMap::new();
    for package in packages {
        for identifier in identifiers(&package.license) {
            referenced
                .entry((identifier.to_ascii_lowercase(), identifier))
                .or_insert_with(|| format!("{} {}", package.name, package.version));
        }
    }
    referenced
        .into_iter()
        .map(|((_, identifier), referrer)| {
            let path = corpus_path(repo, identifier);
            let text = std::fs::read_to_string(&path)
                .map_err(|e| no_corpus_text(identifier, &referrer, &path, &e))?;
            Ok(LicenseText {
                identifier: identifier.to_owned(),
                text,
            })
        })
        .collect()
}

fn corpus_path(repo: &Path, identifier: &str) -> PathBuf {
    repo.join(CORPUS_DIR).join(format!("{identifier}.txt"))
}

/// What to do about an identifier the corpus has no text for.
///
/// This message is the whole mechanism for adding a license: a dependency that
/// introduces one fails the bake here and nowhere else, so what it prints has
/// to be everything a person needs to close the gap.
fn no_corpus_text(identifier: &str, referrer: &str, path: &Path, error: &std::io::Error) -> String {
    format!(
        "`{referrer}` is licensed under {identifier} and the corpus has no text for it: \
         cannot read {} ({error}).\n\
         Fetch https://spdx.org/licenses/{identifier}.json and save its `licenseText` field \
         verbatim, with LF line endings, as {CORPUS_DIR}/{identifier}.txt in this repository. \
         For a license exception the field is `licenseExceptionText` instead. A `LicenseRef-` \
         identifier has no SPDX page at all: take its text from the package's own copy, name \
         the file after the identifier the same way, and say in the document's preamble where \
         it came from. Then rerun `cargo xtask bake licenses`.",
        path.display()
    )
}

/// The document that travels with the binary.
///
/// The canonical text of each identifier, carried once however many crates
/// declare it, and one line per crate whose license links into it. Amendment
/// A5: a canonical text is SPDX's template, so this file carries no crate's
/// copyright holder, and its preamble says so rather than letting a reader
/// assume otherwise.
pub fn render_notices(packages: &[Package], texts: &[LicenseText]) -> String {
    let numbers: BTreeMap<&str, usize> = texts
        .iter()
        .enumerate()
        .map(|(index, text)| (text.identifier.as_str(), index + 1))
        .collect();

    let mut out = String::new();
    out.push_str("# Third-party licenses\n\n");
    let _ = write!(
        out,
        "Sunlit Earth is licensed under the GPL-3.0-or-later, whose text is in `LICENSE` beside \
         this file. This file contains the license of each of the {} third-party crates the \
         program links. Each text below is the canonical text of its identifier as SPDX \
         publishes it. `LicenseRef-Slint-Royalty-free-2.0` and `LicenseRef-Slint-Software-3.0` \
         are the two texts SPDX does not publish, having no page for either, and those are the \
         copies from Slint's own repository.\n\n",
        packages.len()
    );

    out.push_str("## Packages\n\n");
    for package in packages {
        let _ = write!(
            out,
            "- `{} {}`, {}",
            package.name,
            package.version,
            anchor_expression(&package.license, &numbers)
        );
        if let Some(repository) = &package.repository {
            let _ = write!(out, ", {repository}");
        }
        out.push('\n');
    }

    out.push_str("\n## License texts\n\n");
    for (index, text) in texts.iter().enumerate() {
        let number = index + 1;
        let _ = write!(out, "<a id=\"license-{number}\"></a>\n\n");
        let _ = write!(out, "### License {number}: {}\n\n", text.identifier);
        let fence = fence_for(&text.text);
        out.push_str(&fence);
        out.push('\n');
        out.push_str(&text.text);
        if !text.text.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&fence);
        out.push_str("\n\n");
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// Write both files into the repository.
pub fn run(runner: &dyn Runner) -> Result<u8, String> {
    let repo = store::repo_root();
    let packages = collect(runner, &repo)?;
    let texts = license_texts(&repo, &packages)?;
    let list = render_list(&packages);
    let notices = render_notices(&packages, &texts);
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
        "{} third-party crates across {}, {} license texts",
        packages.len(),
        SHIPPING_TARGETS.join(", "),
        texts.len()
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::RealRunner;
    use std::collections::BTreeSet;

    fn package(name: &str, version: &str, license: &str) -> Package {
        Package {
            name: name.to_owned(),
            version: version.to_owned(),
            license: normalize_expression(license),
            repository: None,
        }
    }

    /// A fabricated `cargo metadata` answer: the two workspace members and
    /// two registry crates, one of them with a legacy license spelling.
    fn fabricated_metadata() -> String {
        serde_json::json!({
            "packages": [
                {"id": "app", "name": "sunlit-earth", "version": "0.1.0",
                 "license": "GPL-3.0-or-later", "repository": null},
                {"id": "core", "name": "sunlit-core", "version": "0.1.0",
                 "license": "GPL-3.0-or-later", "repository": null},
                {"id": "normal", "name": "shipped", "version": "1.0.0",
                 "license": "MIT/Apache-2.0", "repository": "https://example.invalid/shipped"},
                {"id": "quiet", "name": "quiet", "version": "0.2.0",
                 "license": null, "repository": null}
            ],
            "workspace_members": ["app", "core"]
        })
        .to_string()
    }

    /// The committed document, which several of these tests parse rather than
    /// asking the renderer what it would have written.
    fn committed_notices() -> String {
        let path = store::repo_root().join(NOTICES_PATH);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
    }

    /// The Packages half and the License texts half of a document.
    fn halves(document: &str) -> (&str, &str) {
        document
            .split_once("\n## License texts\n")
            .expect("the document has a License texts section")
    }

    /// Every `[label](target)` in a string, in the order they appear.
    fn markdown_links(text: &str) -> Vec<(&str, &str)> {
        let mut found = Vec::new();
        let mut rest = text;
        while let Some(open) = rest.find('[') {
            rest = &rest[open + 1..];
            let Some(close) = rest.find("](") else { break };
            let label = &rest[..close];
            rest = &rest[close + 2..];
            let Some(end) = rest.find(')') else { break };
            found.push((label, &rest[..end]));
            rest = &rest[end + 1..];
        }
        found
    }

    /// The same string with every `[label](target)` reduced to its label, so an
    /// operand that was never linked reads the same as one that was.
    fn without_links(text: &str) -> String {
        let mut out = String::new();
        let mut rest = text;
        while let Some(open) = rest.find('[') {
            out.push_str(&rest[..open]);
            rest = &rest[open + 1..];
            let Some(close) = rest.find("](") else {
                out.push('[');
                break;
            };
            out.push_str(&rest[..close]);
            rest = &rest[close + 2..];
            let Some(end) = rest.find(')') else { break };
            rest = &rest[end + 1..];
        }
        out.push_str(rest);
        out
    }

    /// The `id` of every explicit anchor in a string.
    fn anchors_in(text: &str) -> BTreeSet<&str> {
        let mut found = BTreeSet::new();
        let mut rest = text;
        while let Some(open) = rest.find("<a id=\"") {
            rest = &rest[open + 7..];
            let Some(end) = rest.find('"') else { break };
            found.insert(&rest[..end]);
            rest = &rest[end + 1..];
        }
        found
    }

    /// The number and identifier of every license section in a string.
    fn sections_in(text: &str) -> Vec<(usize, &str)> {
        text.lines()
            .filter_map(|line| line.strip_prefix("### License "))
            .map(|heading| {
                let (number, identifier) = heading
                    .split_once(": ")
                    .unwrap_or_else(|| panic!("a section heading reads {heading:?}"));
                (
                    number
                        .parse()
                        .unwrap_or_else(|e| panic!("{heading:?} is not numbered: {e}")),
                    identifier,
                )
            })
            .collect()
    }

    /// The invariant the whole design rests on: every operand of every
    /// package's expression is a link, and it lands on the section that carries
    /// that operand's own text.
    fn assert_every_operand_resolves(document: &str) -> usize {
        let (packages, texts) = halves(document);
        let anchors = anchors_in(texts);
        assert!(!anchors.is_empty(), "the document defines no anchors");
        let mut resolved = 0;
        for line in packages.lines().filter(|line| line.starts_with("- `")) {
            let fields: Vec<&str> = line.split(", ").collect();
            let expression = fields
                .get(1)
                .unwrap_or_else(|| panic!("{line:?} names no license expression"));
            let links = markdown_links(expression);
            let plain = without_links(expression);
            for identifier in identifiers(&plain) {
                let Some((_, target)) = links.iter().find(|(label, _)| *label == identifier) else {
                    panic!("{line:?} does not link {identifier}");
                };
                let anchor = target.strip_prefix('#').unwrap_or_else(|| {
                    panic!("{line:?} sends {identifier} to {target}, which is not in this file")
                });
                assert!(
                    anchors.contains(anchor),
                    "{line:?} sends {identifier} to #{anchor}, which no section defines"
                );
                let section = format!("<a id=\"{anchor}\"></a>\n\n### License ");
                let Some((_, after)) = texts.split_once(&section) else {
                    panic!("#{anchor} heads no section");
                };
                let heading = after.lines().next().unwrap_or_default();
                assert!(
                    heading.ends_with(&format!(": {identifier}")),
                    "{line:?} sends {identifier} to the section headed {heading:?}"
                );
                resolved += 1;
            }
        }
        resolved
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
    fn an_expression_yields_its_operands_and_none_of_its_operators() {
        assert_eq!(
            identifiers("(MIT OR Apache-2.0) AND Unicode-3.0"),
            ["MIT", "Apache-2.0", "Unicode-3.0"]
        );
        assert_eq!(
            identifiers("Apache-2.0 WITH LLVM-exception OR MIT"),
            ["Apache-2.0", "LLVM-exception", "MIT"]
        );
        assert!(identifiers("").is_empty());
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
    fn a_fence_is_longer_than_any_backtick_run_inside_the_text() {
        assert_eq!(fence_for("plain\n"), "```");
        assert_eq!(fence_for("``` inside\n"), "````");
        assert_eq!(fence_for("  ````` indented\n"), "``````");
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

    /// The two expressions in this tree that an operand walk is most likely to
    /// get wrong: one parenthesized, one carrying an exception through `WITH`.
    /// Rendered here against the real corpus and then parsed back out of the
    /// document, so a renderer that dropped the exception or swallowed a paren
    /// fails even though `MIT OR Apache-2.0` would still look right.
    #[test]
    fn the_hard_expressions_link_every_operand_they_name() {
        let repo = store::repo_root();
        let packages = vec![
            package("grouped", "1.0.0", "(MIT OR Apache-2.0) AND Unicode-3.0"),
            package(
                "excepted",
                "2.0.0",
                "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT",
            ),
            package(
                "slinty",
                "3.0.0",
                "GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR \
                 LicenseRef-Slint-Software-3.0",
            ),
        ];
        let texts = license_texts(&repo, &packages).expect("the committed corpus");
        let document = render_notices(&packages, &texts);
        assert_eq!(assert_every_operand_resolves(&document), 10, "{document}");
        assert!(
            document.contains("- `grouped 1.0.0`, ([MIT](#license-"),
            "the parenthesis moved: {document}"
        );
        assert!(
            document.contains("WITH [LLVM-exception](#license-"),
            "the exception lost its link: {document}"
        );
    }

    /// The sections are ordered case-insensitively, which is the one thing
    /// keeping `LLVM-exception` from filing ahead of the `LicenseRef-` texts a
    /// reader looks for under L-i.
    #[test]
    fn the_sections_are_ordered_case_insensitively_and_numbered_from_one() {
        let repo = store::repo_root();
        let packages = vec![package(
            "mixed",
            "1.0.0",
            "MIT OR LLVM-exception OR ISC OR LicenseRef-Slint-Software-3.0",
        )];
        let texts = license_texts(&repo, &packages).expect("the committed corpus");
        let order: Vec<&str> = texts.iter().map(|text| text.identifier.as_str()).collect();
        assert_eq!(
            order,
            [
                "ISC",
                "LicenseRef-Slint-Software-3.0",
                "LLVM-exception",
                "MIT"
            ]
        );
        let document = render_notices(&packages, &texts);
        assert_eq!(
            sections_in(halves(&document).1),
            vec![
                (1, "ISC"),
                (2, "LicenseRef-Slint-Software-3.0"),
                (3, "LLVM-exception"),
                (4, "MIT")
            ]
        );
    }

    /// An identifier with no corpus file is the only way a license enters the
    /// tree unnoticed, so the bake stops and says what to fetch, where the
    /// field is, and where the file goes.
    #[test]
    fn an_identifier_with_no_corpus_text_names_the_url_and_the_path() {
        let repo = store::repo_root();
        let invented = package("invented", "1.0.0", "MIT AND Fictitious-1.0");
        let error =
            license_texts(&repo, &[invented]).expect_err("Fictitious-1.0 has no corpus file");
        for expected in [
            "Fictitious-1.0",
            "invented 1.0.0",
            "https://spdx.org/licenses/Fictitious-1.0.json",
            "licenseText",
            "licenseExceptionText",
            "assets/licenses/Fictitious-1.0.txt",
            "cargo xtask bake licenses",
        ] {
            assert!(error.contains(expected), "{expected:?} is missing: {error}");
        }
    }

    /// The same invariant as `the_hard_expressions_link_every_operand_they_name`
    /// over the document that actually ships, which is the one whose links a
    /// reader clicks.
    #[test]
    fn every_operand_link_in_the_committed_document_resolves() {
        let document = committed_notices();
        let resolved = assert_every_operand_resolves(&document);
        assert!(
            resolved > 500,
            "only {resolved} operands were checked, so the document is not the shipping one"
        );
    }

    #[test]
    fn every_referenced_identifier_has_exactly_one_section() {
        let document = committed_notices();
        let (packages, texts) = halves(&document);
        let referenced: BTreeSet<&str> = packages
            .lines()
            .filter(|line| line.starts_with("- `"))
            .flat_map(markdown_links)
            .map(|(label, _)| label)
            .collect();
        let sections = sections_in(texts);
        assert_eq!(
            sections.len(),
            referenced.len(),
            "{} sections for {} identifiers",
            sections.len(),
            referenced.len()
        );
        for identifier in &referenced {
            let mine: Vec<usize> = sections
                .iter()
                .filter(|(_, named)| named == identifier)
                .map(|(number, _)| *number)
                .collect();
            assert_eq!(mine.len(), 1, "{identifier} has sections {mine:?}");
        }
        assert_eq!(
            sections
                .iter()
                .map(|(number, _)| *number)
                .collect::<Vec<_>>(),
            (1..=sections.len()).collect::<Vec<_>>()
        );
    }

    /// Nothing may happen to a license text between the corpus and the
    /// document: no rewrapping, no trimming, no line-ending conversion.
    #[test]
    fn each_corpus_text_reaches_the_document_byte_for_byte() {
        let repo = store::repo_root();
        let document = committed_notices();
        let (_, texts) = halves(&document);
        let sections = sections_in(texts);
        assert!(sections.len() > 10, "{} sections", sections.len());
        for (number, identifier) in sections {
            let path = corpus_path(&repo, identifier);
            let corpus = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
            assert_eq!(
                texts.matches(corpus.as_str()).count(),
                1,
                "license {number}, {identifier}, is not in the document exactly as \
                 {} has it",
                path.display()
            );
        }
    }

    /// The walk needs `cargo metadata` and `cargo tree`, so a checkout that
    /// cannot resolve the tree skips with a printed reason rather than failing.
    /// The corpus is committed, so a failure to read it is not skippable.
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
        let texts = license_texts(&repo, &packages).unwrap_or_else(|e| panic!("{e}"));
        for (relative, fresh) in [
            (LIST_PATH, render_list(&packages)),
            (NOTICES_PATH, render_notices(&packages, &texts)),
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
