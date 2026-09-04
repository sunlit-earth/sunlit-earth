# Plan: About window overhaul (2026-09-04)

## Summary

The About window is a `ScrollView` around a `VerticalLayout` of seven plain `Text` lines, and the OS draws it with the default window icon because `AboutWindow` never sets one. This replaces it with a fixed header (the mark on the left; title, version, copyright and repository link on the right) over a tabbed scroll area with three tabs: attributions, the GPLv3, and the third-party crate list.

Both of the questions the overhaul turns on have an answer, and both were measured rather than assumed:

- **The license text does not have to live in Slint.** `include_str!("../../../LICENSE")` from `crates/sunlit-app/src/about.rs` reaches the repository root at compile time, rustc records the file in the dep-info so cargo rebuilds when it changes, and `git archive` of `HEAD` carries `LICENSE`, so `cargo xtask dist` builds it in unchanged. Nothing reads a path at runtime and nothing new enters the build.
- **The attributions can be a markdown file.** Slint 1.17 has a `StyledText` element that renders a subset of CommonMark with clickable links and a `link-clicked(link)` callback, and `slint::StyledText::from_markdown` builds its value from Rust. So the attribution document is an ordinary markdown file, `include_str!`'d, parsed once, and rendered with live links.

Riding along, because the audit that the header's copyright line invited turned up real gaps: the cloud layer is missing a mandatory EUMETSAT notice, and no third-party crate notices travel with the binary at all.

## Stakes

Medium, and unusually asymmetric. The code is confined to `crates/sunlit-app` plus one new xtask subcommand, touches no shader, no persisted setting, no IPC contract and nothing in the engine, so the *engineering* risk is low. The attribution half is what raises the stakes: this is the last release before the repository goes public, and a missing notice is the kind of defect that no test catches and that costs more to fix after distribution than before it.

Nothing below is legal advice. Every claim about an obligation is followed by where the obligation is written down, so it can be checked rather than taken on trust.

## What Slint 1.17 can do, measured

Probed against this workspace's pinned `slint ~1.17` (resolved 1.17.1) with `i-slint-backend-testing`, then thrown away.

| Question | Answer |
|---|---|
| `StyledText` element exists | Yes, `i-slint-compiler/builtins.slint:684`ff, exported as `StyledText` |
| Set its value from Rust | Yes, `slint::StyledText::from_markdown(&str) -> Result<_, _>` and `from_plain_text(&str)` |
| Links clickable, with a callback | Yes, `callback link-clicked(link: string)` |
| Link hit-testing compiled in | Yes. It is `#[cfg(feature = "shared-parley")]` in `i-slint-core/items/text.rs:283`, and both `i-slint-renderer-femtovg` and `i-slint-renderer-software` turn on `i-slint-core/shared-parley`; the app takes both through slint's default features |
| Markdown parser compiled in | Yes. `from_markdown` needs `i-slint-common/markdown`, which `i-slint-core/std` enables |
| `TabWidget` | Yes, `std-widgets.slint`, with `Tab { title: ... }` children |
| Emphasis, strong, lists, inline code, `[text](url)`, `<https://url>` | All parse |
| **Headings** | **Rejected**: `Markdown headings are not supported` |
| **Horizontal rules** | **Rejected**: `Markdown horizontal rules are not supported` |
| **Code blocks** (fenced or indented) | **Rejected**: `Markdown code blocks are not supported` |
| Tables, block quotes, images, footnotes | Not supported, per the element's own doc comment |
| `StyledText` wrapping | Always `word-wrap`; `fn wrap` returns it unconditionally (`items/text.rs:428`) and the element exposes no override |
| `TextEdit { read-only: true }` | Yes, `widgets/common/textedit-base.slint:11` |

Two consequences fall straight out of that table.

**The GPLv3 cannot go through the markdown parser.** `StyledText::from_markdown` on `LICENSE` fails in 358 µs with `Markdown code blocks are not supported`: the license's indented blocks read as indented code. `from_plain_text` accepts it, but puts all 35,149 bytes in a single paragraph rendered under a forced `word-wrap`, which reflows text that is already hard-wrapped at 76 columns with load-bearing indentation. So the license tab is not a `StyledText` at all. It is a plain `Text` with `wrap: no-wrap` inside a `ScrollView`, which preserves the file's own line breaks exactly and lets the scroll area deal with a window too narrow for 76 columns.

**The attribution document keeps its headings, and Rust flattens them.** A markdown file with no headings is a worse document to read on GitHub and a worse file to ship beside the binary, and the whole point of the exercise is that the file is the source of truth. So `about.rs` runs a small preprocessor before `from_markdown`: an ATX heading line becomes a bold line, a `---` rule is dropped, and everything else passes through. Ten lines, table-driven test, and the file stays a normal document.

Parse cost is not a concern. A 411-entry crate list with a link on every line, 33,033 bytes, parses in 772 µs; twenty grouped paragraphs parse in 64 µs. The real list is 559 entries, so scale that to roughly a millisecond. What is *not* measured is layout and paint of 559 separate `StyledText` paragraphs in a real window, which no headless test reaches. Step 6 measures it on the desktop before the shape is fixed, and the fallback is in the decisions below.

## The attribution audit

### Where credits live today

Four places, and they disagree about how much they say:

- `about.rs:11`, `ATTRIBUTIONS: [&str; 7]`: the seven lines the window shows.
- `crates/sunlit-core/src/assets/stars/ATTRIBUTION.md`: generated by `bake_stars.rs:14`, sits beside `hyg_v4_4_mag7.bin` so the blob never travels without its notice, and ships in the bundle as `ATTRIBUTION.md` (`bundle.rs:202`).
- `textures/PROVENANCE.md`: the fullest record by a wide margin: SVS IDs, file hashes, download dates, named visualizers and scientists, underlying instruments, and what each source asks for. Ships in the bundle.
- `README.md:213`: one paragraph pointing at the other three.

The bundle already ships `LICENSE`, `ATTRIBUTION.md` and `textures/PROVENANCE.md` (`bundle.rs:184`-`204`). What it ships nothing about is the crate tree.

### Legally required, and missing today

**1. "Contains modified EUMETSAT data".** The cloud composite comes from `https://clouds.matteason.co.uk/images/{size}/clouds.jpg` (`cloud_fetcher.rs:39`). That project's README is explicit, and the wording is not ours to paraphrase:

> The cloud data used for these images is provided by EUMETSAT. You must provide the following attribution to them, per their [licensing](https://www.eumetsat.int/eumetsat-data-licensing):
>
> > Contains modified EUMETSAT data

The current line, "Live cloud composite provided by clouds.matteason.co.uk", carries none of it. EUMETSAT's own data policy asks for credit, a link to the licence, and an indication that changes were made, and states that satellite data is not released under a Creative Commons licence. This is the clearest gap in the set and the cheapest to close: one sentence, verbatim.

**2. Third-party crate notices.** `cargo tree -p sunlit-earth --edges normal --target <triple>` resolves 411 distinct crates into the Windows binary, 504 into the Linux one and 422 into the macOS one, 559 across the union of the three, which by decision 8 is what the single list covers. Counted on Windows, where the licenses fall out as:

| Family | Crates | What the license asks of a distributor |
|---|---|---|
| MIT (incl. `OR Apache-2.0` and the legacy `MIT/Apache-2.0` spellings) | ~290 | "The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software" |
| Apache-2.0 | ~50 offered it | §4, carry the license copy, retain notices, propagate `NOTICE` if present |
| Unicode-3.0 (the `icu_*` tree) | 25 | Notice and license text |
| BSD-3-Clause, BSD-2-Clause | 13 | Retain the copyright notice and conditions |
| ISC, Zlib, 0BSD, BSL-1.0, Unlicense | ~11 | Notice retention, except 0BSD and Unlicense which ask for nothing |
| MPL-2.0 (`option-ext`) | 1 | Source availability for that file; notices |
| CDLA-Permissive-2.0 (`webpki-roots`, the Mozilla CA set) | 1 | Include the license text with the data |
| CC0-1.0 (`hexf-parse`) | 1 | Nothing |
| `Apache-2.0 AND ISC` (`ring`), `Apache-2.0 AND MIT` (`dpi`) | 2 | Both notices, not a choice between them |
| `GPL-3.0-only OR LicenseRef-Slint-*` (the Slint tree) | 10 | We take the GPLv3 option; compatible with this project's GPL-3.0-or-later |

Nothing in the tree carries any of these notices. That is the largest legal gap, and it is a distribution obligation rather than a courtesy: the notice has to travel with the binary.

**A caveat on the chosen shape.** The third tab lists every crate with its license name hyperlinked to the canonical text online. That is a good tab and it is what gets built. It does not, on its own, discharge the obligation above: MIT says the notice "shall be *included*", and a hyperlink is not inclusion. Nor can a canonical text carry the per-crate copyright lines, which differ crate by crate and are the part MIT and BSD actually require. So the same generator that produces the tab's list also writes the full notices, copyright lines included, into `THIRD-PARTY-LICENSES.md`, and the bundle ships that file beside the binary the way it already ships `LICENSE`. The tab is how a user browses it; the file is what satisfies the license. Neither is redundant.

**3. HYG's CC BY-SA 4.0 attribution is thinner in the window than in the file.** BY-SA 4.0 §3(a)(1) asks a licensee to retain identification of the creator, the copyright notice, a notice referring to the license *with a URI to it*, the disclaimer notice, and a URI to the material, and §3(a)(1)(B) to indicate that the material was modified. The shipped `ATTRIBUTION.md` has the creator, the source URI, the license URI, and "derived from", so the file is in reasonable shape. The window's line has the creator, the license *name*, and the source host, with no license URI and no statement that the catalog was cut down (`hyg_v4_4_mag7.bin` is 249,564 bytes of magnitude-7-and-brighter stars out of a 119,614-star catalog: unmistakably modified). The window should say what the file says.

**4. The ShareAlike resolution should be written down.** The star blob is an adaptation of BY-SA 4.0 data shipped inside a GPL-3.0-or-later program. Creative Commons declared BY-SA 4.0 one-way compatible with GPLv3 in October 2015, which is what permits it: contributions to an adaptation of BY-SA 4.0 material may be licensed under GPLv3, and a downstream reuser then looks only to GPLv3 to satisfy BY-SA's attribution and ShareAlike conditions. Nothing in the tree says so. A redistributor cannot work out their own position without it, so it belongs in the attribution document with a link to the declaration.

**5. Astronomy Engine needs its notice, not just its license name.** `astronomy-engine-bindings` is MIT and vendors Don Cross's `astronomy` C sources, also MIT. "Astronomy Engine by Don Cross, MIT License" names the license without reproducing the notice MIT requires. It is the same class of gap as the crate tree and closes the same way, through `THIRD-PARTY-LICENSES.md`, but it earns a named line in the attributions too: it is one of the two libraries the rendered image actually depends on for its correctness.

### Deserved but not required

Public-domain sources ask for nothing enforceable and are the reason the picture exists. Every credit below is checked against the source's own page; the ones already in `textures/PROVENANCE.md` are marked, since for those the work is promoting a fact the repository already knows into the window.

- **Blue Marble Next Generation** (the day map): produced by **Reto Stöckli**, NASA Earth Observatory, NASA Goddard Space Flight Center, from Terra/MODIS. NEO asks that republication credit "NASA Earth Observatory". The current line says only "NASA Blue Marble 2004 surface imagery".
- **Black Marble 2016** (the night map): the credit line NASA publishes is "**NASA Earth Observatory images by Joshua Stevens, using Suomi NPP VIIRS data from Miguel Román, NASA GSFC**", the satellite being the NASA-NOAA Suomi NPP. The current line says only "NASA Black Marble 2016 nighttime imagery".
- **CGI Moon Kit** (SVS 4720): visualizer **Ernie Wright** (USRA), scientist **Noah Petro** (NASA/GSFC). The map underneath is the **LROC WAC** natural-color Hapke-normalized mosaic from **Arizona State University**, 70N to 70S, with the poles filled from **LOLA** altimetry [in `PROVENANCE.md`]. The ASU credit is absent from the window, and ASU built the mosaic.
- **Deep Star Maps 2020** (SVS 4851): the SVS asks for "**NASA/Goddard Space Flight Center Scientific Visualization Studio**" and, for the DR2 data, "**ESA/Gaia/DPAC**"; visualizer **Ernie Wright** (USRA). The window has the abbreviated `NASA/GSFC/SVS` and both institutional credits, which is enough; what is missing is Wright's name and the fact that the layer is Gaia DR2 flux for stars fainter than magnitude 11.5 [in `PROVENANCE.md`].
- **HYG's own sources**: the catalog combines **Hipparcos**, the **Yale Bright Star Catalogue** and the **Gliese** nearby-star catalogue, and since 2023 carries the proper names adopted by the **IAU Working Group on Star Names**. Three centuries of astrometry, currently credited as one line about a CSV.
- **Matt Eason**, for the live cloud maps. Released CC0, and the README says "I'd appreciate an attribution if you use the code or images, but it's not necessary." It costs a name.
- **Slint** and **wgpu**, named individually rather than lost among 411 rows: they are the window and the renderer. Slint under its GPLv3 option; wgpu under Apache-2.0 OR MIT.
- **jxl-oxide** and, behind it, **libjxl** and the JPEG XL committee, for the format all four textures are stored in.
- **The Rust project**, for the toolchain.

### Checked, and does not apply

Recorded so nobody adds a credit for something we do not ship, or removes one thinking it is decorative.

- **The Inter font is not embedded.** Slint vendors `Inter-VariableFont.ttf` (874,708 bytes, `OFL-1.1-RFN`) in `i-slint-common/sharedfontique/`, but `sharedfontique.rs:33` `include_bytes!`es it under `#[cfg(any(target_family = "wasm", target_os = "nto"))]` only. Windows, Linux and macOS builds resolve system fonts and embed no font at all, so the OFL's notice requirement is not triggered. If a wasm or QNX target is ever added, it is.
- **No NASA logo, and none may be added.** NASA's media guidelines put NASA content outside US copyright and ask only that NASA be acknowledged as the source, but state that the NASA Insignia, logotype and identifiers are *not* in the public domain and are protected, and that content must not imply endorsement. The About window must credit NASA in words and must not draw the meatball or the worm.
- **Slint's widget icons** (`_chevron-down.svg` and the rest of the fluent set) are Slint's own work under the license we already take the GPLv3 option on. No separate credit.
- **`tools/texture-pipeline`** is a Python tool run by hand offline. Its dependencies never reach a user's machine and are out of scope.
- **`world.topo.200405.original.jxl`** is read by nothing (`PROVENANCE.md`). It needs no credit beyond the day map's, which it shares.

## Decisions

1. **The logo is the SVG master**, `assets/icon/sunlit-earth.svg`, the same source `MainWindow` uses at `main.slint:202`. Slint carries resvg and rasterizes at the element's real pixel size, so the header mark is crisp at 100%, 150% and 200% scaling, where a 256 px raster is resampled at everything but one size. The premultiplied-alpha artifact documented in `docs/app-icon.md` is specific to winit's `set_window_icon` path and does not apply to an in-window `Image`. **Consequence:** `about-256.png` loses its only intended consumer. It keeps being baked, since dropping it means touching `bake_icon.rs`, `HICOLOR_SIZES`' neighbours and `the_committed_bake_matches_a_fresh_one` for no gain, and `docs/app-icon.md` gets one sentence saying it is a spare rather than the About window's source.
2. **`AboutWindow` sets `icon:` to the same SVG.** This is the actual bug the request opened with: `MainWindow` has the property and `AboutWindow` never did, so the OS falls back to its default.
3. **The header is fixed and only the tabs scroll.** The version, copyright and repository link stay on screen at any window size; a `VerticalLayout` of a header row and a `TabWidget` gets that with no scroll math.
4. **Three tabs:** Attributions, License, Third-party. The first two are what was asked for; the third is where the crate list goes, per the answer to the scope question.
5. **The attribution source of truth is `assets/ATTRIBUTION.md`.** One markdown file, `include_str!`'d by `about.rs` for the tab, and linked from `README.md`. Amendment A2 keeps it out of the release archive: the tab is how those credits travel now. `about.rs:11`'s `ATTRIBUTIONS` array is deleted. **The star blob's own `crates/sunlit-core/src/assets/stars/ATTRIBUTION.md` stays exactly where it is**, generated by `bake stars`, so the blob does not sit in the source tree without its BY-SA notice beside it; amendment A2 takes it out of the archive, where the tab's own notice replaces it. `textures/PROVENANCE.md` also stays and keeps being the fuller record; the new file is the user-facing summary and links to it.
6. **The license tab is a plain `Text`, `wrap: no-wrap`, in a `ScrollView`.** Forced word-wrap on `StyledText` would reflow the GPLv3's hard-wrapped, indentation-bearing layout. `TextEdit { read-only: true }` was considered for the selection and copy it would give: rejected for now, because it draws an edit control's chrome and focus ring around a document nobody edits. Revisit if anyone asks to copy a clause.
7. **`cargo xtask bake licenses` generates both third-party artifacts** (`bake-licenses` before amendment A1), following `bake icon` and `bake stars` exactly: it reads `cargo metadata`, walks the shipping tree, and writes two committed files: `THIRD-PARTY-LICENSES.md` with the full texts and copyright lines, and a compact `assets/third-party.md` list for the tab. Committed rather than built, so `cargo build` gains no dependency and nothing runs a metadata walk on a user's machine; a test compares committed against fresh, the way `the_committed_bake_matches_a_fresh_one` does, so a dependency added without a re-bake fails the suite instead of shipping a stale notice. No new crate is needed: `cargo metadata` is a subprocess and `serde_json` is already a workspace dependency.
8. **One list, not one per build.** The tab names the crates that reach the binary: the normal dependency edges of `sunlit-earth` and, through it, `sunlit-core`. Build-dependencies (`slint-build`, `embed-resource`), dev-dependencies (`proptest`, `i-slint-backend-testing`, `image`, `serial_test`) and the `xtask` crate are all excluded, because none of their code is in the shipped binary. There is one list and one notice file for every platform, so `about.rs` has a single `include_str!` and no `cfg`. The set is the union of `cargo tree --locked --package sunlit-earth --edges normal --target <triple>` over the three triples this project ships (`x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`), deduplicated on name and version, minus this workspace's own two crates. That is the method, and it is reproducible: it comes to 411, 504 and 422 crates for the three triples and 557 in the union, which is the 559 of the first probe minus `sunlit-earth` and `sunlit-core`. Departure 1 says why it is `cargo tree` rather than the `cargo metadata --filter-platform` this decision first named. Where a crate is in the list but not in one particular build, that is the direction to err: a notice for a crate that is absent misleads nobody, a missing notice for one that is present is the defect this whole item exists to prevent. The list says so in one line at the top.

9. **License names link to their SPDX page**, `https://spdx.org/licenses/<ID>.html`, one rule for every identifier and no table to maintain. SPDX has a page per identifier by construction, each carrying the canonical text, so the URL is derived from the identifier rather than looked up, and a dependency that introduces a license nobody anticipated gets a working link with no code change. Two identifiers need care: an expression is split on `OR`/`AND` and each operand linked separately, so `MIT OR Apache-2.0` shows two links rather than one dead one; and `LicenseRef-Slint-*` is not an SPDX identifier and has no page, so the Slint operands link to Slint's own license file instead. The generator also normalizes the legacy slash spellings (`MIT/Apache-2.0`, `Apache-2.0 / MIT`, `BSD-3-Clause/MIT`) to `OR`, so the tab does not show four spellings of one thing.
10. **One line per crate in the third tab, pending step 6's measurement.** 559 paragraphs parse in about a millisecond, but their layout and paint in a real window is unmeasured. If the tab is visibly slow to open, the fallback is grouping by license: about seventeen paragraphs, each naming its crates with the license linked once, which parses in 64 µs and is a shorter scroll. The generator emits whichever shape the measurement picks; the file format is an implementation detail of one xtask and one `include_str!`.
11. **The copyright line becomes `© 2026 Sebastian Straub`.** First commit 2026-03-08, and a notice conventionally carries the year.
12. **The repository link is `https://github.com/klamann/sunlit-earth`**, from `git remote`. It 404s until the repository is made public, which is a release-day event and not a reason to leave it out.
13. **Links open through the platform's handler, with no `unsafe`.** `std::process::Command`: `xdg-open` on Linux, `open` on macOS, and on Windows `cmd /C start "" <url>` with `CREATE_NO_WINDOW` through the safe `CommandExt::creation_flags`, since a bare `start` flashes a console. `ShellExecuteW` would be the idiomatic Win32 call and is rejected because it costs a scoped `#[allow(unsafe_code)]` for a link. Every URL is validated `http`/`https` before it reaches a shell, even though all of them come from a compile-time file: the check is three lines and makes the argument-injection question unaskable.

## Amendments (2026-09-04)

A1. **The bake subcommands become one `bake` command with subcommands.** `cargo xtask bake-icon` and `cargo xtask bake-stars` become `cargo xtask bake icon` and `cargo xtask bake stars`, and the new one is `cargo xtask bake licenses` rather than `bake-licenses`. Three sibling top-level verbs that all mean "regenerate a committed asset from its source" read better as one verb with three objects, and the nested form is already in this CLI: `BakeCommand` mirrors `VmCommand` exactly. Renaming `commands::bake_icon` and `commands::bake_stars` is not part of this; the module names stay and only the CLI surface changes.

A2. **The dist archive carries the binary, the textures and `LICENSE`, plus the new third-party notice, and nothing else.** `ATTRIBUTION.md`, `textures/PROVENANCE.md` and `build-info.json` come out of it. `build-info.json` keeps being written beside the archive in `target/dist/<target>/`, which is where it was always the more useful of its two copies. The two documents stay in the repository, unmoved; they simply stop being packed. What makes that safe is step 1: the About window's attributions tab now carries the credits those two files carried, delivered in the app rather than as a file next to it, and for the star catalog that includes the full CC BY-SA 4.0 notice with its license URI and the statement that the catalog was modified. `THIRD-PARTY-LICENSES.md` is the one addition and it stays in the archive, because the third tab carries crate names and SPDX links only: drop the file and the distribution would contain no copy of the per-crate copyright notices at all, which is the whole reason decision 9's caveat exists.

What A2 means in code:

- `bundle::layout` drops `TEXTURE_PROVENANCE` from the `TEXTURE_FILES` chain, drops the `RECORD` item and drops the `STAR_ATTRIBUTION` item, and adds `THIRD-PARTY-LICENSES.md` beside the `LICENSE` item.
- `dist::seal_bundle` stops writing `bundle::RECORD` into the bundle root, with the call ordering left exactly as it is: `seal_bundle` runs after verification so that the in-bundle copy of the record was not the stale one of two, and that reason is gone, but resequencing the run is a behavioral change outside this plan. The comment that gave the reason is corrected rather than left.
- `dist::publish` already writes `build-info.json` into `dist_dir(target)`, and that is the copy that stays. It names the file through `bundle::RECORD` rather than a repeated literal.
- The two doc comments in dist.rs that say the record travels inside the bundle are corrected. The opening the second one creates, that a record beside the archive could now carry the archive's size, is not taken: that is scope creep.
- The bundle tests that assert `textures/PROVENANCE.md`, `build-info.json` and the star `ATTRIBUTION.md` are present invert into absence assertions rather than being deleted, so a future re-add fails a test instead of passing quietly.
- A const left unused by the removals goes rather than staying for clippy.

Both amendments are the user's calls, made after the plan was committed, so they are recorded here rather than as departures: a departure is where reality disagreed with the plan, and these are the plan being changed on purpose.

## Steps

1. **`assets/ATTRIBUTION.md`.** Write the document the audit calls for: the mandatory EUMETSAT sentence verbatim, HYG with its license URI and a statement that the catalog was cut to magnitude 7, the BY-SA-to-GPLv3 compatibility note with its link, Astronomy Engine, the four imagery sources with the people and institutions named above, HYG's own three catalogs and the IAU working group, Matt Eason, Slint, wgpu, jxl-oxide, and Rust. Links on every source. No tables, no code blocks, no horizontal rules; headings are fine, since the preprocessor flattens them.
2. **The markdown preprocessor in `about.rs`.** ATX heading to bold line, drop `---`, pass everything else through, then `StyledText::from_markdown`. Table-driven test over the constructs the probe found unsupported, plus one test that the real `assets/ATTRIBUTION.md` parses without error, which is what stops a future edit from silently blanking the tab.
3. **`cargo xtask bake licenses`.** The three metadata walks and their union, the SPDX URL derivation, the legacy-spelling normalizer, and the two output files. Wire it into `commands/mod.rs` and the CLI beside `bake icon` and `bake stars`, which amendment A1 moves under a `bake` verb at the same time.
4. **The `AboutWindow` rewrite in `ui/main.slint`.** `icon`, the header row (`Image` from the SVG, then title, version, copyright, repository link), and the `TabWidget` with the three tabs. New `in` properties: `logo` is not needed (the `@image-url` is static), `version` stays, and `attributions`, `third-party` and `license-text` replace the `[string]` model. One `open-url(string)` callback, wired from all three tabs and the header link.
5. **`about.rs`.** Delete `ATTRIBUTIONS`, feed the three `include_str!`s through the preprocessor, register `on_open_url`, and add the URL opener with its scheme check.
6. **Measure the third tab.** Half of it turned out to be reachable headlessly, which the plan did not expect: the testing backend lays text out through the same shared-parley stack a real window does, so switching to the third tab and measuring what the layout costs needs no screen. Measured on 2026-09-04 with the real 557-line list, three runs: the document parses in 1.4 to 1.6 ms, and the tab switch plus layout takes 10 to 11 ms, producing a body 7,949 px tall, so every line is laid out rather than deferred. Decision 10 keeps its default shape, one line per crate. What is still unmeasured is GPU paint and scrolling on a real adapter, and that stays an open item for the user to look at with `cargo run`.
7. **The bundle.** Amendment A2 turns this into three removals and one addition: `textures/PROVENANCE.md`, `build-info.json` and the star `ATTRIBUTION.md` come out of `bundle::layout`, and `THIRD-PARTY-LICENSES.md` goes in next to the `LICENSE` entry. It is target-independent, so it goes in unconditionally. `assets/ATTRIBUTION.md` is not packed: the attributions tab is how those credits now travel.
8. **Docs.** `docs/roadmap.md`'s About-window item, which currently describes the seven-line array; `README.md:213`'s license paragraph, to point at the new file and the third-party file; `docs/app-icon.md`, for the `about-256.png` sentence and the SVG's second consumer; and `CLAUDE.md`'s rare-commands line, for `bake licenses`. Amendment A1 adds the renames: `CLAUDE.md:84`, `docs/app-icon.md:5` and `:18` (which names `bake-icon --review DIR`), and `crates/sunlit-app/src/tray.rs:24`. `docs/plans/2026-08-23-app-icon-integration-plan.md` is left alone: a dated plan records what was true when it was written and is not a live reference.

## Tests

Slint-side, in `crates/sunlit-app/tests/slint_ui.rs`. `test_the_about_window_content_starts_at_its_left_edge` is deleted rather than replaced; departure 9 says why the header equivalent this section first named was not worth keeping.

- The header does not scroll: the version line's `y` is unchanged after the tab area is scrolled.
- Each of the three tabs is reachable and its content is non-empty.
- A click on a link in the attributions tab reaches `open-url` with the URL from the file.

Rust-side, in `about.rs`:

- The preprocessor's table-driven case list.
- `assets/ATTRIBUTION.md` parses as markdown with no error.
- The license tab's text is the `LICENSE` file byte for byte, which is what proves the `include_str!` path rather than a copy.
- The URL opener rejects a non-`http(s)` scheme.
- **The EUMETSAT sentence is present verbatim**, in the shape of the two existing tests at `about.rs:63` and `:76` that pin the Moon Kit and Gaia credits. This one is a hard legal requirement, so it gets the same treatment.
- One test per required-notice class, in the same style: the HYG line carries the license URI and says the catalog was modified; the BY-SA-to-GPLv3 note is present.

xtask-side, in the `bake-licenses` module:

- Committed output matches a fresh generation, the `bake-icon` pattern.
- Every operand of every license expression in the tree becomes its own link, and a compound expression yields one link per operand.
- The `LicenseRef-Slint-*` operands link to Slint's license rather than to a nonexistent SPDX page, which is the one case the derived URL gets wrong.
- The legacy slash spellings normalize to `OR`.
- No dev-dependency, build-dependency or workspace member reaches the list: `proptest`, `i-slint-backend-testing`, `slint-build`, `embed-resource` and `xtask` are each asserted absent, and `slint`, `wgpu` and `astronomy-engine-bindings` asserted present.

## Settled, and not to be relitigated

The three questions this plan opened with were answered on 2026-09-04:

- **`about-256.png` stays.** It keeps being baked and keeps its test coverage; `docs/app-icon.md` gains one sentence saying it is a spare rather than the About window's source. No change to `bake_icon.rs`.
- **The repository link stays**, dead until the repository is made public. No test checks it, and nothing is gated on it.
- **The third tab names only the crates in the release build**, which is decision 8: the normal dependency edges of `sunlit-earth` and `sunlit-core`, no dev-dependencies, no build-dependencies, no `xtask`. One list for every platform, not one per build; the per-target variant was considered on 2026-09-04 and rejected as complexity without a payoff.

One thing is deliberately left out rather than open: the third tab does not name the toolchain or the commit. `RECORD` in the bundle already carries both, and duplicating them in the window is not what was asked for.

## Departures

1. **The shipping set comes from `cargo tree --edges normal`, not from `cargo metadata --filter-platform`.** Decision 8 named the latter, and it cannot express what the decision asks for. The resolve graph `cargo metadata` prints is feature-unified across dependency kinds, so an optional dependency that only a dev-dependency turns on appears in it as an ordinary normal edge. Measured on this tree: the app's dev-dependency on `i-slint-backend-testing` switches on `i-slint-backend-selector`'s optional feature of the same name, and `cargo metadata`'s resolve therefore carries a `kind: null` edge from `i-slint-backend-selector` to `i-slint-backend-testing`. A walk from `sunlit-earth` that follows only normal edges, which is what decision 8 describes, reaches the testing backend through it, along with about a dozen of its own dependencies, and none of that is in a release build. The plan's own test list requires `i-slint-backend-testing` to be provably absent, so the mechanism and the requirement contradicted each other. `cargo tree` resolves features per dependency kind and does not have the problem; it also reproduces the counts the plan's first probe recorded, 411, 504 and 422, which the metadata walk did not. The generator therefore runs one unfiltered `cargo metadata` for the lookup table (license, repository, and the source directory each notice is read from) and three `cargo tree` calls for membership. Four subprocess calls rather than three, still entirely inside the xtask and still invisible to the app.
2. **`THIRD-PARTY-LICENSES.md` carries each distinct license text once rather than once per package.** Reproducing every notice file inline under its own package came to 5.1 MB across 946 files, which is not a file to commit or to read. Deduplicating on the exact bytes takes it to under a megabyte: the Apache-2.0 text is byte for byte identical in nearly three hundred packages. Each text is numbered, carries the list of packages that ship it, and every package's line names the texts that are its own, so nothing is dropped and no package's notice is missing from the document. Nothing is summarized or substituted.
3. **Fifty-two packages vendor no license text at all, and the file says so rather than filling the gap.** They declare a license in `Cargo.toml` and ship no copy of it and no copyright line, `accesskit` and the `objc2` family among them. A canonical SPDX text carries `Copyright (c) <year> <holder>` as a placeholder and filling it in from the `authors` field would put a fabricated copyright attribution into a legal notice, which is worse than a recorded gap. They are grouped in their own section with the identifier, the `authors` field verbatim where there is one, and the repository, and the file's opening paragraph says how many there are. That is the whole of what the upstream data supports, and a later re-bake can show the number falling.
4. **One notice in this tree is not in a file named like a license, and one entry in the generator says where it is.** `astronomy-engine-bindings` is MIT, ships no `LICENSE` file, and carries Don Cross's MIT notice in the leading comment of the vendored `astronomy/source/c/astronomy.h`. A name-only search reports it as vendoring nothing, which would leave the audit's item 5 open, so `VENDORED_NOTICES` names the file and the generator takes the leading `/* ... */` verbatim. A bake that cannot find the notice there fails rather than writing the gap.
5. **The bake is committed but the committed-versus-fresh test is skippable.** `the_committed_bake_matches_a_fresh_one` needs `cargo metadata`, `cargo tree` and the unpacked registry sources the manifests point at. On a checkout that cannot resolve the tree it prints why and returns rather than failing, which is the pattern the tests that need the real 8K textures already use. It was verified to be doing the real comparison: it failed on a stale file and passed after the re-bake.
6. **The URL check is an allowlist, not only a scheme test.** Decision 13 says the check "makes the argument-injection question unaskable", and a scheme test does not: the Windows opener is `cmd /C start`, and `cmd` reparses its argument and reads `&`, `|`, `<`, `>`, `^` and `%` as its own, whatever quoting Rust applied on the way in. So the check requires `http` or `https` and then that every remaining character be alphanumeric or one of `-._~:/?#[]@+='`, which holds nothing a shell looks at. What that admits and what it costs, exactly: a path, a fragment, a port, userinfo and a query string of one parameter all pass, a second query parameter does not because `&` is out, and a percent-escape does not because `%` is out. `,` and `;` are out as well, not for injection but for truncation: both are legal in a URL and `start` reads both as argument delimiters, so a URL carrying one would open the wrong page. No URL in the two rendered documents needs any of the four, and a test walks every link in both through the check so a future one that would be refused fails the suite rather than logging a refusal in front of a user.
7. **The star count in `assets/ATTRIBUTION.md` is 15,597, not the plan's figure.** Step 1 asks the document to state that the catalog was cut to magnitude 7. The blob's own header says how many stars that came to: 249,564 bytes, a 12-byte header and 16-byte records, so 15,597.
8. **Six links came out of `assets/ATTRIBUTION.md` and step 1's "Links on every source" is met in words instead.** The document as first written linked Arizona State University for the LROC mosaic, Hipparcos, the Yale Bright Star Catalogue and Gliese for HYG's own three catalogs, the IAU Working Group on Star Names, and Gaia, and pointed the two Earth maps at deep Earth Observatory article paths. Those URLs were written from memory: this environment has no network, so not one of them could be resolved, and a dead link in an attribution document is a defect rather than a decoration. Each is now a named credit with no link, and the two Earth maps point at `earthobservatory.nasa.gov`, which is the organization NEO asks to be credited. What survives is traceable to something checkable: the two SVS IDs from `textures/PROVENANCE.md`, HYG and its license URI from the star catalog's own notice, the compatibility declaration from the research behind this plan, EUMETSAT's licensing page from the same, the Astronomy Engine repository from the vendored `astronomy.h` header, and the crates' own declared home pages. Every legally required URI survived: BY-SA's license URI, HYG's source URI, EUMETSAT's licensing link and the ShareAlike compatibility declaration.
9. **The header's replacement test is deleted rather than kept.** The Tests section named `test_the_about_window_header_holds_its_place_under_a_long_version` as the successor to `test_the_about_window_content_starts_at_its_left_edge`, and it does not carry the weight the original did. `header-x` read the header row's `x` inside the parent layout, which is that layout's 16 px padding whatever the version string is, so wrapping the whole window back in a `ScrollView`, exactly the shape the original guarded against, left it green. The property that matters is that the header does not move when a tab scrolls, and `test_scrolling_a_tab_leaves_the_header_where_it_was` pins it by driving a real scroll and asserting the tab body moved before asserting the version line did not. One honest test rather than two with a decoy, so the weak one and the `header-x` property it read are both gone.

A3. **`THIRD-PARTY-LICENSES.md` lives under `assets/`, not at the repository root.** A generated document a megabyte long was the largest thing at the root, five times `Cargo.lock`, and that is not where it belongs. It moves beside the two documents it is generated with, `assets/ATTRIBUTION.md` and `assets/third-party.md`. `NOTICES_PATH` is now the repository-relative path and `NOTICES_NAME` the name inside the bundle, because the file still belongs at the top level of a release archive next to `LICENSE`, which is the first place someone unpacking one looks; `Item` already separates its in-archive `path` from its on-disk `source`, so that costs nothing.

Generating it on demand instead was considered and rejected. The bake itself takes 4.4 seconds and `dist` already needs cargo and a populated registry, so it is easy. What it costs is three properties that a legal artifact is exactly the wrong thing to lose them on: the diff that shows which notices changed when a dependency is added, and which nobody would ever see again; `the_committed_bake_matches_a_fresh_one`, which is the only thing that catches a dependency added without a re-bake and which has nothing to compare against if the file is not committed; and reproducibility of `dist` from a commit rather than from the state of one machine's registry cache. Committed and out of the way gets the complaint fixed and keeps all three. For the record on size, git stores the file at 108 KB, 11.4 percent of its 978 KB, since license text compresses hard.

## Declined findings

1. **The BY-SA 4.0 disclaimer notice is not added.** The audit paragraph on HYG lists "the disclaimer notice" among what BY-SA 4.0 section 3(a)(1) asks a licensee to retain, and `assets/ATTRIBUTION.md` carries no such notice. Nothing is owed. Section 3(a)(1)(A)(v) conditions retention on a disclaimer having been supplied with the licensed material, and HYG supplies none, so there is nothing to retain. Writing one would attribute words to the upstream licensor that the licensor never wrote, which is worse than the gap it would close. Step 1's own requirement list never asked for it. Raised in validation on 2026-09-04 and declined by the user the same day.
