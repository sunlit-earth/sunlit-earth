//! Lazily created About window: the documents it shows and the link opener.
//!
//! Three documents reach the window, all of them `include_str!`'d at compile
//! time so nothing reads a path at runtime. The GPLv3 goes in as plain text,
//! because every markdown path through Slint reflows a document that is
//! already hard-wrapped at 76 columns with load-bearing indentation. The two
//! markdown files take the two paths their shapes ask for.
//!
//! `assets/ATTRIBUTION.md` is a document: headings, sections, nested bullets.
//! `StyledText` cannot lay one out, and that is a limit of the element rather
//! than of how it is used. It rejects a heading outright and has no font size
//! to give one, it stacks paragraphs with no gap and no spacing property, and
//! its list bullet is literal text glued to the paragraph, so a wrapped line
//! gets no hanging indent. So [`parse_blocks`] cuts the file into blocks, the
//! tab renders one element per block, and only a block's own inline markdown
//! goes through [`StyledText::from_markdown`]. A block is one paragraph by
//! construction, which is also why none of them can hold a construct the
//! subset rejects.
//!
//! `assets/third-party.md` goes through the same parser. Measured on the real
//! 557-entry document, the block layout builds its model in 2.3 ms and lays
//! out in 28.6 ms against 1.1 ms and 27.1 ms for one `StyledText` holding all
//! 557 paragraphs, so an element per line costs about a millisecond and a half
//! and there is no reason for the two tabs to render differently. The bake
//! writes that document without a title, since the tab label already says
//! what it is.

use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::{AboutBlock, AboutWindow, MainWindow};

/// The credits, the source of truth for them, and what the bundle links to.
const ATTRIBUTION: &str = include_str!("../../../assets/ATTRIBUTION.md");

/// The crate list `cargo xtask bake licenses` writes.
const THIRD_PARTY: &str = include_str!("../../../assets/third-party.md");

/// The licence this program is under, read from the repository root at compile
/// time. `git archive` of `HEAD` carries `LICENSE`, so a release build gets
/// the same bytes, and rustc records the file in the dep-info, so editing it
/// rebuilds this crate.
const LICENSE: &str = include_str!("../../../LICENSE");

/// The fixed-width family the licence tab asks for.
///
/// One real family name per platform rather than the generic "monospace",
/// which Slint does not resolve: `font-family` reaches parley through
/// `FontFamilyName::named`, so a generic keyword is looked up as a family
/// nobody has and falls back to the proportional default with no error
/// anywhere. Each name below has shipped with its platform for over a decade.
/// `the_monospace_family_resolves_on_this_platform` is what keeps that true.
pub const MONO_FAMILY: &str = if cfg!(target_os = "windows") {
    "Consolas"
} else if cfg!(target_os = "macos") {
    "Menlo"
} else {
    "DejaVu Sans Mono"
};

/// Shared handle that creates the About window on first use and reuses it.
#[derive(Clone, Default)]
pub struct AboutController {
    window: Rc<RefCell<Option<AboutWindow>>>,
}

impl AboutController {
    /// Register the settings window's About callback.
    pub fn register_settings_callback(&self, main_window: &MainWindow) {
        let about = self.clone();
        main_window.on_show_about(move || {
            let _ = about.show();
        });
    }

    /// Show the existing About window, creating and populating it if needed.
    pub fn show(&self) -> Result<(), slint::PlatformError> {
        let mut slot = self.window.borrow_mut();
        if slot.is_none() {
            let window = AboutWindow::new()?;
            window.set_version(env!("CARGO_PKG_VERSION").into());
            window.set_attributions(document_model(ATTRIBUTION));
            window.set_third_party(document_model(THIRD_PARTY));
            window.set_license_text(LICENSE.into());
            window.set_mono_family(MONO_FAMILY.into());
            window.on_open_url(|url| open_url(&url));
            *slot = Some(window);
        }
        if let Some(window) = slot.as_ref() {
            window.show()?;
        }
        Ok(())
    }
}

/// `AboutBlock::kind` for a paragraph, the one kind that carries no depth.
pub const PARAGRAPH: i32 = 0;
/// `AboutBlock::kind` for a heading, whose depth is its level, 1 through 6.
pub const HEADING: i32 = 1;
/// `AboutBlock::kind` for a list item, whose depth is its nesting.
pub const LIST_ITEM: i32 = 2;

/// The deepest nesting a list item is laid out at.
///
/// Every level costs 14 px of indentation, and a document nested deeper than
/// this is not a document; without the cap a stray run of spaces would push a
/// bullet's text out of a narrow window.
const MAX_DEPTH: i32 = 4;

/// The Attributions tab's model, one row per block of a markdown document.
pub fn document_model(document: &str) -> slint::ModelRc<AboutBlock> {
    let rows: Vec<AboutBlock> = parse_blocks(document).iter().map(row).collect();
    slint::ModelRc::new(slint::VecModel::from(rows))
}

/// One block as the Slint side reads it.
///
/// A heading carries a plain string, because the tab renders it as a `Text` to
/// give it a weight and a size that `StyledText` has no property for.
fn row(block: &Block) -> AboutBlock {
    let heading = matches!(block.kind, BlockKind::Heading);
    AboutBlock {
        kind: block.kind.tag(),
        depth: block.depth,
        heading: if heading {
            block.text.as_str().into()
        } else {
            slint::SharedString::new()
        },
        body: if heading {
            slint::StyledText::default()
        } else {
            styled_or_plain(&block.text)
        },
    }
}

/// One block of a markdown document, as the Attributions tab lays it out.
#[derive(Debug, PartialEq, Eq)]
struct Block {
    kind: BlockKind,
    /// A heading's level, a list item's nesting, and zero for a paragraph.
    depth: i32,
    /// The block's inline markdown, its marker and its indentation gone.
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Paragraph,
    Heading,
    ListItem,
}

impl BlockKind {
    /// The number the tab's three branches compare against. The layout test in
    /// `slint_ui.rs` drives all three, which is what keeps the two sides
    /// agreeing about them.
    const fn tag(self) -> i32 {
        match self {
            Self::Paragraph => PARAGRAPH,
            Self::Heading => HEADING,
            Self::ListItem => LIST_ITEM,
        }
    }
}

/// The blocks of a markdown document.
///
/// What it recognizes: an ATX or setext heading, a bulleted list item at any
/// nesting, and a run of anything else as one paragraph. A blank line ends a
/// block, a thematic break ends one and renders as nothing, and any other line
/// joins the open block as a soft wrap, which is what makes a wrapped bullet
/// one text rather than two. Inline markdown is never touched. An ordered list
/// is not a shape this recognizes, so it stays a paragraph and Slint's own
/// list rendering has it.
fn parse_blocks(document: &str) -> Vec<Block> {
    let lines: Vec<&str> = document.lines().collect();
    let mut blocks: Vec<Block> = Vec::new();
    let mut open: Option<Block> = None;
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || is_thematic_break(trimmed) {
            blocks.extend(open.take());
            continue;
        }
        let underline = lines.get(index).map(|next| next.trim());
        if can_carry_a_setext_underline(trimmed) && underline.is_some_and(is_setext_underline) {
            blocks.extend(open.take());
            index += 1;
            blocks.push(Block {
                kind: BlockKind::Heading,
                depth: if underline.is_some_and(|next| next.starts_with('=')) {
                    1
                } else {
                    2
                },
                text: trimmed.to_owned(),
            });
            continue;
        }
        if let Some((level, text)) = atx_heading(trimmed) {
            blocks.extend(open.take());
            if !text.is_empty() {
                blocks.push(Block {
                    kind: BlockKind::Heading,
                    depth: level,
                    text: text.to_owned(),
                });
            }
            continue;
        }
        if let Some((depth, text)) = bullet_item(line) {
            blocks.extend(open.take());
            open = Some(Block {
                kind: BlockKind::ListItem,
                depth,
                text: text.to_owned(),
            });
            continue;
        }
        match open.as_mut() {
            Some(block) => {
                block.text.push(' ');
                block.text.push_str(trimmed);
            }
            None => {
                open = Some(Block {
                    kind: BlockKind::Paragraph,
                    depth: 0,
                    text: trimmed.to_owned(),
                });
            }
        }
    }
    blocks.extend(open);
    blocks
}

/// The nesting and the text of a bulleted list item, or `None` for a line that
/// opens none.
///
/// Two columns to a level, which is the least a nested item under a `- ` marker
/// can be indented by, and a tab counts as four.
fn bullet_item(line: &str) -> Option<(i32, &str)> {
    let text = line.trim_start().strip_prefix(['-', '*', '+'])?;
    if !text.is_empty() && !text.starts_with(' ') {
        return None;
    }
    let depth = i32::try_from(indent_width(line) / 2).unwrap_or(MAX_DEPTH);
    Some((depth.min(MAX_DEPTH), text.trim()))
}

/// The width of a line's leading whitespace, a tab counting as four columns.
fn indent_width(line: &str) -> usize {
    line.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

/// Markdown, or its plain text if it will not parse.
///
/// A parse failure means the text grew a construct Slint's subset rejects, and
/// unstyled text is a better outcome than an empty tab.
fn styled_or_plain(markdown: &str) -> slint::StyledText {
    slint::StyledText::from_markdown(markdown).unwrap_or_else(|error| {
        tracing::warn!("an attribution document did not parse as markdown: {error}");
        slint::StyledText::from_plain_text(markdown)
    })
}

/// The level and the text of an ATX heading line, or `None` when the line is
/// not one.
fn atx_heading(trimmed: &str) -> Option<(i32, &str)> {
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    Some((
        i32::try_from(hashes).ok()?,
        rest.trim().trim_end_matches('#').trim(),
    ))
}

/// The text of an ATX heading line, or `None` when the line is not one.
fn heading_text(trimmed: &str) -> Option<&str> {
    atx_heading(trimmed).map(|(_, text)| text)
}

/// Whether the line is a `CommonMark` thematic break.
fn is_thematic_break(trimmed: &str) -> bool {
    let mut marker = None;
    let mut count = 0;
    for character in trimmed.chars() {
        match character {
            '-' | '_' | '*' => {
                if *marker.get_or_insert(character) != character {
                    return false;
                }
                count += 1;
            }
            ' ' | '\t' => {}
            _ => return false,
        }
    }
    count >= 3
}

/// Whether the line is a `CommonMark` setext underline.
///
/// A run of `=` or a run of `-`, of any length and nothing else. `***` and
/// `___` are thematic breaks only and never underline anything.
fn is_setext_underline(trimmed: &str) -> bool {
    !trimmed.is_empty() && (trimmed.chars().all(|c| c == '=') || trimmed.chars().all(|c| c == '-'))
}

/// Whether a setext underline below this line would make it a heading.
///
/// A paragraph line does; a blank line, another heading, a thematic break and
/// a list item do not, and for those the underline is whatever it is on its
/// own.
fn can_carry_a_setext_underline(trimmed: &str) -> bool {
    !trimmed.is_empty()
        && heading_text(trimmed).is_none()
        && !is_thematic_break(trimmed)
        && !is_setext_underline(trimmed)
        && !starts_a_list_item(trimmed)
}

/// Whether the line opens a list item, bulleted or ordered.
fn starts_a_list_item(trimmed: &str) -> bool {
    let bulleted = trimmed
        .strip_prefix(['-', '*', '+'])
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '));
    let ordered = {
        let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
        digits > 0
            && trimmed[digits..]
                .strip_prefix(['.', ')'])
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
    };
    bulleted || ordered
}

/// Hand a URL to the platform's own handler.
///
/// `std::process::Command` rather than `ShellExecuteW`, which would be the
/// idiomatic Win32 call and would cost a scoped `#[allow(unsafe_code)]` for a
/// hyperlink. On Windows the opener is `cmd /C start`, whose console window is
/// suppressed through the safe `creation_flags`.
fn open_url(url: &str) {
    if !is_web_url(url) {
        tracing::warn!("refusing to open {url:?}: only http and https are opened");
        return;
    }
    let spawned = platform_open(url);
    if let Err(error) = spawned {
        tracing::warn!("could not open {url:?}: {error}");
    }
}

/// Whether a URL is one this program will hand to a shell.
///
/// Every URL in the window comes from a file compiled into the binary, so this
/// guards nothing that is reachable today. It exists so that the
/// argument-injection question is unaskable rather than answerable by reading
/// the documents, and that takes more than a scheme check: the Windows opener
/// goes through `cmd /C`, which reparses its argument and reads `&`, `|`, `<`,
/// `>`, `^` and `%` as its own. So the rest of the URL has to be spelled out
/// of a set that holds nothing a shell looks at.
///
/// What that set admits and what it costs, exactly. A path, a fragment, a
/// port, userinfo and a query string of one parameter all pass, since `?` and
/// `=` are in. A second query parameter does not, because `&` is out. Neither
/// does a percent-escape, because `%` is out. `,` and `;` are out as well:
/// they are legal in a URL and `start` reads both as argument delimiters, so a
/// URL carrying one would be truncated there and the wrong page opened, which
/// is a worse failure than a refusal. No URL in the shipped documents needs
/// any of the four, and a test walks every one of them through this check.
fn is_web_url(url: &str) -> bool {
    /// URL characters that are not shell characters anywhere the opener runs.
    const SAFE: &str = "-._~:/?#[]@+='";

    match url.split_once("://") {
        Some((scheme, rest)) => {
            matches!(scheme, "http" | "https")
                && !rest.is_empty()
                && rest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || SAFE.contains(c))
        }
        None => false,
    }
}

#[cfg(windows)]
fn platform_open(url: &str) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt as _;

    /// `CREATE_NO_WINDOW`. A bare `start` flashes a console otherwise.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // The empty argument is `start`'s title parameter: without it `start`
    // reads a quoted URL as the window title and opens nothing.
    std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "macos")]
fn platform_open(url: &str) -> std::io::Result<()> {
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn platform_open(url: &str) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use slint::Model as _;

    use super::*;

    /// Every document the window renders through the markdown parser, and the
    /// tab it lands in.
    const RENDERED: [(&str, &str); 2] = [
        ("assets/ATTRIBUTION.md", ATTRIBUTION),
        ("assets/third-party.md", THIRD_PARTY),
    ];

    /// What stops a future edit from silently blanking either markdown tab.
    /// Neither document meets the parser whole any more: every block of it has
    /// to parse on its own, and a heading is the one block the parser never
    /// sees, since a tab draws it as a plain `Text`.
    #[test]
    fn every_block_of_both_documents_parses_as_markdown() {
        for (name, document) in RENDERED {
            let blocks = parse_blocks(document);
            assert!(
                blocks.iter().any(|block| block.kind == BlockKind::ListItem),
                "no list item came out of {name}"
            );
            for block in blocks
                .iter()
                .filter(|block| block.kind != BlockKind::Heading)
            {
                if let Err(error) = slint::StyledText::from_markdown(&block.text) {
                    panic!(
                        "a block of {name} does not parse: {error}\n{:?}",
                        block.text
                    );
                }
            }
        }
    }

    /// The attributions carry the section headings the block layout exists to
    /// size; the crate list carries none, since its own title was dropped from
    /// the bake once the tab label said the same thing.
    #[test]
    fn only_the_attributions_document_carries_headings() {
        assert!(
            parse_blocks(ATTRIBUTION)
                .iter()
                .any(|block| block.kind == BlockKind::Heading),
            "no heading came out of the attributions document"
        );
        assert!(
            !parse_blocks(THIRD_PARTY)
                .iter()
                .any(|block| block.kind == BlockKind::Heading),
            "the crate list grew a heading, which the bake is supposed to omit"
        );
    }

    /// One row per block shape, since the parser is the whole of what turns a
    /// markdown file into the tab's layout.
    #[test]
    fn the_parser_cuts_a_document_into_the_shapes_the_tab_renders() {
        use BlockKind::{Heading, ListItem, Paragraph};

        /// One block as the table below names it.
        type Shape<'a> = (BlockKind, i32, &'a str);

        let cases: &[(&str, &[Shape])] = &[
            ("# Attributions", &[(Heading, 1, "Attributions")]),
            ("## Imagery ##", &[(Heading, 2, "Imagery")]),
            ("###### Six", &[(Heading, 6, "Six")]),
            ("####### Seven", &[(Paragraph, 0, "####### Seven")]),
            ("#NoSpace", &[(Paragraph, 0, "#NoSpace")]),
            ("- item", &[(ListItem, 0, "item")]),
            (
                "* star\n+ plus",
                &[(ListItem, 0, "star"), (ListItem, 0, "plus")],
            ),
            (
                "- top\n  - nested",
                &[(ListItem, 0, "top"), (ListItem, 1, "nested")],
            ),
            // A wrapped bullet is one text, or the second line would lose the
            // hanging indent the bullet column exists for.
            ("- item\n  wrapped on", &[(ListItem, 0, "item wrapped on")]),
            ("- item\nlazily on", &[(ListItem, 0, "item lazily on")]),
            ("\t- tabbed", &[(ListItem, 2, "tabbed")]),
            // Five levels of indentation, capped at four.
            ("          - far in", &[(ListItem, 4, "far in")]),
            (
                "one\ntwo\n\nthree",
                &[(Paragraph, 0, "one two"), (Paragraph, 0, "three")],
            ),
            (
                "# Head\nbody",
                &[(Heading, 1, "Head"), (Paragraph, 0, "body")],
            ),
            (
                "- item\n\n- another",
                &[(ListItem, 0, "item"), (ListItem, 0, "another")],
            ),
            ("a\n\n---\n\nb", &[(Paragraph, 0, "a"), (Paragraph, 0, "b")]),
            ("Clouds\n---", &[(Heading, 2, "Clouds")]),
            ("Clouds\n===", &[(Heading, 1, "Clouds")]),
            ("", &[]),
            (
                "- [a](https://example.invalid) and *this*",
                &[(ListItem, 0, "[a](https://example.invalid) and *this*")],
            ),
        ];

        for (document, expected) in cases {
            let blocks = parse_blocks(document);
            let shapes: Vec<Shape> = blocks
                .iter()
                .map(|block| (block.kind, block.depth, block.text.as_str()))
                .collect();
            assert_eq!(shapes, *expected, "{document:?}");
            for block in blocks
                .iter()
                .filter(|block| block.kind != BlockKind::Heading)
            {
                assert!(
                    slint::StyledText::from_markdown(&block.text).is_ok(),
                    "{document:?} made a block that does not parse: {:?}",
                    block.text
                );
            }
        }
    }

    /// A heading's text reaches the tab as a plain string and everything else
    /// as parsed markdown, which is the model's half of the layout.
    #[test]
    fn a_heading_row_carries_a_string_and_every_other_row_carries_markdown() {
        let model = document_model("# Imagery\n\nbody\n\n- item\n");
        let rows: Vec<AboutBlock> = model.iter().collect();

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].kind, HEADING);
        assert_eq!(rows[0].depth, 1);
        assert_eq!(rows[0].heading, "Imagery");
        assert_eq!(rows[0].body, slint::StyledText::default());

        assert_eq!(rows[1].kind, PARAGRAPH);
        assert_eq!(rows[2].kind, LIST_ITEM);
        for row in &rows[1..] {
            assert!(row.heading.is_empty(), "{row:?} carries a heading string");
            assert_ne!(row.body, slint::StyledText::default());
        }
    }

    /// The license tab shows the repository's `LICENSE`, which is what proves
    /// the `include_str!` path rather than a copy that drifted.
    #[test]
    fn the_license_tab_is_the_repositorys_license_file() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("LICENSE");
        let on_disk = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        assert_eq!(LICENSE, on_disk);
    }

    /// The only content assertion left on this document, because it is the
    /// only one where the wording is not ours: EUMETSAT's data licensing
    /// requires this sentence and does not leave the phrasing to us, so
    /// asserting it pins a requirement rather than a way of writing. Every
    /// other credit was pinned by phrase and rewriting the document broke
    /// those tests without changing what it credited, which is what makes
    /// them overfitting.
    #[test]
    fn the_cloud_credit_carries_the_eumetsat_notice_verbatim() {
        assert!(
            ATTRIBUTION.contains("Contains modified EUMETSAT data"),
            "the mandatory EUMETSAT notice is missing"
        );
    }

    #[test]
    fn only_http_and_https_urls_are_opened() {
        for allowed in [
            "http://example.invalid/x",
            "https://example.invalid",
            "https://spdx.org/licenses/MIT.html",
            "https://example.invalid:8443/a/b#c",
            "https://example.invalid/search?q=one",
        ] {
            assert!(is_web_url(allowed), "{allowed} should be opened");
        }
        for refused in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "mailto:someone@example.invalid",
            "https://",
            "example.invalid",
            "",
            "https://example.invalid/a b",
            // What `cmd /C` would read as its own rather than as a URL.
            "https://example.invalid/x&calc",
            "https://example.invalid/x|calc",
            "https://example.invalid/x>out",
            "https://example.invalid/x^y",
            "https://example.invalid/%PATH%",
            "https://example.invalid/\"x\"",
            "https://example.invalid/$(x)",
            "https://example.invalid/x\ny",
            // Legal in a URL, and `start` reads both as argument delimiters.
            "https://example.invalid/a,b",
            "https://example.invalid/a;b",
        ] {
            assert!(!is_web_url(refused), "{refused} should be refused");
        }
    }

    /// Every URL the two markdown documents carry has to survive the check, or
    /// a link in the window would log a refusal instead of opening.
    #[test]
    fn every_link_in_the_shipped_documents_is_one_the_opener_accepts() {
        for (name, document) in RENDERED {
            for link in links_in(document) {
                assert!(
                    is_web_url(&link),
                    "{name} carries {link}, which the opener refuses"
                );
            }
        }
    }

    /// The `(url)` half of every `[text](url)` in a document.
    fn links_in(document: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut rest = document;
        while let Some(start) = rest.find("](") {
            rest = &rest[start + 2..];
            if let Some(end) = rest.find(')') {
                found.push(rest[..end].to_owned());
                rest = &rest[end..];
            }
        }
        found
    }

    #[test]
    fn settings_callback_shows_the_version_and_all_three_documents() {
        i_slint_backend_testing::init_no_event_loop();
        let main_window = MainWindow::new().expect("main window");
        let controller = AboutController::default();
        controller.register_settings_callback(&main_window);

        main_window.invoke_show_about();

        let slot = controller.window.borrow();
        let about = slot.as_ref().expect("About window was created");
        assert!(about.window().is_visible());
        assert_eq!(about.get_version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(about.get_license_text(), LICENSE);
        assert!(
            about.get_attributions().row_count() > 10,
            "the attributions tab got no document"
        );
        assert!(
            about.get_third_party().row_count() > 10,
            "the third-party tab got no document"
        );
    }
}
