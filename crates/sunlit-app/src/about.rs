//! Lazily created About window: the documents it shows and the link opener.
//!
//! Three documents reach the window, all of them `include_str!`'d at compile
//! time so nothing reads a path at runtime. `assets/ATTRIBUTION.md` and
//! `assets/third-party.md` are markdown and go through
//! [`StyledText::from_markdown`]; the GPLv3 goes in as plain text, because
//! every markdown path through Slint reflows a document that is already
//! hard-wrapped at 76 columns with load-bearing indentation.
//!
//! Slint's markdown subset rejects headings and horizontal rules, and both
//! belong in a file that is also read on its own. [`flatten_markdown`] is the
//! reconciliation: a heading becomes a bold line, a rule is dropped, and
//! everything else passes through untouched.

use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::{AboutWindow, MainWindow};

/// The credits, the source of truth for them, and what the bundle links to.
const ATTRIBUTION: &str = include_str!("../../../assets/ATTRIBUTION.md");

/// The crate list `cargo xtask bake licenses` writes.
const THIRD_PARTY: &str = include_str!("../../../assets/third-party.md");

/// The licence this program is under, read from the repository root at compile
/// time. `git archive` of `HEAD` carries `LICENSE`, so a release build gets
/// the same bytes, and rustc records the file in the dep-info, so editing it
/// rebuilds this crate.
const LICENSE: &str = include_str!("../../../LICENSE");

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
            window.set_attributions(styled(ATTRIBUTION));
            window.set_third_party(styled(THIRD_PARTY));
            window.set_license_text(LICENSE.into());
            window.on_open_url(|url| open_url(&url));
            *slot = Some(window);
        }
        if let Some(window) = slot.as_ref() {
            window.show()?;
        }
        Ok(())
    }
}

/// One document, flattened and parsed, or its plain text if it will not parse.
///
/// A parse failure means a committed document grew a construct Slint's subset
/// rejects, which `every_shipped_document_parses_as_markdown` is there to
/// catch first. If one ever gets past that, an unstyled tab is a better
/// outcome than an empty one.
fn styled(document: &str) -> slint::StyledText {
    let flattened = flatten_markdown(document);
    slint::StyledText::from_markdown(&flattened).unwrap_or_else(|error| {
        tracing::warn!("an attribution document did not parse as markdown: {error}");
        slint::StyledText::from_plain_text(&flattened)
    })
}

/// Markdown that Slint's subset accepts, from markdown that reads well on its
/// own.
///
/// Slint 1.17 rejects headings and horizontal rules outright, so an ATX
/// heading becomes a bold paragraph and a thematic break is dropped. Nothing
/// else is touched, which is what keeps the shipped files ordinary documents
/// rather than a dialect.
fn flatten_markdown(document: &str) -> String {
    let mut out = String::with_capacity(document.len());
    for line in document.lines() {
        let trimmed = line.trim();
        if is_thematic_break(trimmed) {
            continue;
        }
        match heading_text(trimmed) {
            Some(text) if !text.is_empty() => {
                out.push_str("**");
                out.push_str(text);
                out.push_str("**");
            }
            Some(_) => {}
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// The text of an ATX heading line, or `None` when the line is not one.
fn heading_text(trimmed: &str) -> Option<&str> {
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    Some(rest.trim().trim_end_matches('#').trim())
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
/// guards nothing that is reachable today. It is three lines, and it makes the
/// argument-injection question unaskable rather than answerable by reading the
/// three documents.
fn is_web_url(url: &str) -> bool {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            matches!(scheme, "http" | "https")
                && !rest.is_empty()
                && !url.chars().any(char::is_whitespace)
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
    use super::*;

    /// Every document the window renders through the markdown parser, and the
    /// tab it lands in.
    const RENDERED: [(&str, &str); 2] = [
        ("assets/ATTRIBUTION.md", ATTRIBUTION),
        ("assets/third-party.md", THIRD_PARTY),
    ];

    /// The one test that stops a future edit from silently blanking a tab:
    /// Slint's subset rejects headings, rules, code blocks and tables, and a
    /// rejected document renders as unstyled text with no error anywhere.
    #[test]
    fn every_shipped_document_parses_as_markdown() {
        for (name, document) in RENDERED {
            let flattened = flatten_markdown(document);
            if let Err(error) = slint::StyledText::from_markdown(&flattened) {
                panic!("{name} does not parse after flattening: {error}");
            }
        }
    }

    #[test]
    fn a_heading_becomes_a_bold_line_and_a_rule_disappears() {
        let flattened = flatten_markdown("# Title\n\ntext\n\n---\n\n### Deeper ###\n");
        assert_eq!(flattened, "**Title**\n\ntext\n\n\n**Deeper**\n");
    }

    /// The constructs the probe found unsupported, one row each, so a Slint
    /// upgrade that changes the subset shows up here rather than in a tab.
    #[test]
    fn the_flattener_covers_every_construct_the_parser_rejects() {
        let cases: [(&str, &str); 8] = [
            ("# One", "**One**"),
            ("###### Six", "**Six**"),
            ("####### Seven", "####### Seven"),
            ("#NoSpace", "#NoSpace"),
            ("---", ""),
            ("***", ""),
            ("___", ""),
            ("- - -", ""),
        ];
        for (input, expected) in cases {
            let flattened = flatten_markdown(input);
            assert_eq!(flattened.trim_end_matches('\n'), expected, "{input:?}");
            assert!(
                slint::StyledText::from_markdown(&flattened).is_ok(),
                "{input:?} still does not parse"
            );
        }
    }

    #[test]
    fn a_paragraph_and_a_list_pass_through_untouched() {
        let document = "text with *emphasis* and a [link](https://example.invalid)\n\n- item\n";
        assert_eq!(flatten_markdown(document), document);
    }

    #[test]
    fn two_dashes_are_not_a_rule() {
        assert_eq!(flatten_markdown("--\n"), "--\n");
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

    /// EUMETSAT's data licensing asks for this sentence and does not leave the
    /// wording to us. It is the one hard legal requirement among the credits,
    /// so it gets the same treatment the SVS and Gaia lines have.
    #[test]
    fn the_cloud_credit_carries_the_eumetsat_notice_verbatim() {
        assert!(
            ATTRIBUTION.contains("Contains modified EUMETSAT data"),
            "the mandatory EUMETSAT notice is missing"
        );
    }

    /// CC BY-SA 4.0 section 3(a)(1) asks for the creator, a URI to the
    /// license, and, in 3(a)(1)(B), a statement that the material was
    /// modified. The catalog was cut to magnitude 7, so it plainly was.
    #[test]
    fn the_star_credit_carries_the_license_uri_and_says_it_was_modified() {
        assert!(
            ATTRIBUTION.contains("HYG Database v4.4"),
            "the creator's catalog"
        );
        assert!(ATTRIBUTION.contains("David Nash"), "the creator");
        assert!(
            ATTRIBUTION.contains("https://creativecommons.org/licenses/by-sa/4.0/"),
            "the license URI, not just the license name"
        );
        assert!(
            ATTRIBUTION.contains("The catalog was modified"),
            "the statement that the catalog was modified"
        );
    }

    /// A redistributor cannot work out their own position on a BY-SA
    /// adaptation inside a GPLv3 program without knowing that Creative
    /// Commons declared the two one-way compatible.
    #[test]
    fn the_share_alike_resolution_is_written_down() {
        assert!(
            ATTRIBUTION.contains(
                "https://creativecommons.org/2015/10/08/\
                 cc-by-sa-4-0-now-one-way-compatible-with-gplv3/"
            ),
            "the compatibility declaration's link"
        );
    }

    /// The CGI Moon Kit is public domain and asks for a credit line; this is
    /// where it appears, and the asset's provenance file names the same words.
    #[test]
    fn the_moon_kit_credit_names_the_visualization_studio() {
        assert!(ATTRIBUTION.contains("CGI Moon Kit"));
        assert!(ATTRIBUTION.contains("NASA's Scientific Visualization Studio"));
    }

    /// The SVS asks for its own credit and Gaia DR2 for a second one, so the
    /// Milky Way's credit has to carry both.
    #[test]
    fn the_milky_way_credit_names_the_studio_and_gaia() {
        assert!(ATTRIBUTION.contains("NASA/GSFC/SVS"));
        assert!(ATTRIBUTION.contains("ESA/Gaia/DPAC"));
    }

    /// Astronomy Engine is one of the two libraries the rendered geometry
    /// depends on for its correctness, and it earns a named line rather than
    /// one row among five hundred.
    #[test]
    fn astronomy_engine_is_named_with_its_author() {
        assert!(ATTRIBUTION.contains("Astronomy Engine"));
        assert!(ATTRIBUTION.contains("Don Cross"));
    }

    #[test]
    fn only_http_and_https_urls_are_opened() {
        for allowed in [
            "http://example.invalid/x",
            "https://example.invalid",
            "https://spdx.org/licenses/MIT.html",
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
        ] {
            assert!(!is_web_url(refused), "{refused} should be refused");
        }
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
        assert_ne!(about.get_attributions(), slint::StyledText::default());
        assert_ne!(about.get_third_party(), slint::StyledText::default());
    }
}
