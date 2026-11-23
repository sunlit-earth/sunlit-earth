# Feed URLs, selectable text and frame guides at any size

## Problem

Three small changes, planned together because each is a few files:

1. The update check only accepts release pages and archives under `https://github.com/sunlit-earth/sunlit-earth/releases/`. Releases may be hosted somewhere else later, and whatever `sunlit.earth` serves is trusted, so the file should be able to point anywhere.
2. No text in the About window, the update window or the older version notice can be selected, so nothing there can be copied: not the version, not a failure message, not a line of the release notes or the license.
3. The frame guides vanish when the preview is narrower than a guide's aspect ratio. Nothing on the preview then says that the guides are on.

## What exists today

- `crates/sunlit-core/src/update/feed.rs` has `NOTES_PREFIX`, `ASSET_PREFIX` and `is_under`. A `notes` or asset `url` outside its prefix, or one whose path is not plain, is read as missing (rule 6 of [2026-11-22-update-check-plan.md](2026-11-22-update-check-plan.md)). `crates/sunlit-app/src/update.rs` applies `open::is_web_url` on top, in `release_page` (line 80) and `download` (line 97). The update check is not in any tagged release yet (v0.5.0 predates it), so no shipped client depends on rule 6.
- The website's `scripts/write-update-feed.py` (repository `sunlit-earth/website`) refuses a URL outside the prefixes of rule 6 (departure 4 of the update check plan).
- `about.slint` shows prose as `StyledText` (attributions and third-party blocks) and `Text` (header, headings, license). `update.slint` and `older-version.slint` use `Text` throughout, apart from the package manager command, which is already a read-only `LineEdit` with a Copy button.
- `overlays/frame-guides.slint` draws one 1 px rectangle per aspect (the anchor screen, 16:9, 4:3), each with its short side on the displayed short edge, centered on `image-container`, with a label at the outline's top left corner plus an inset. With a landscape aspect every rectangle is as tall as the panel, so its top and bottom lines lie on the panel's own edges and only the two vertical lines show. Once the panel is narrower than the aspect, those lines are outside the panel, which clips them, and the labels go with them. Nothing is left on screen.
- The anchor screen reaches the guides through `frame-screen-width` and `frame-screen-height` on `MainWindow`, set in `engine_client.rs` (lines 305 and 306) from `PreviewFrame::screen`. The guides are their only reader in the UI.

## What Slint 1.19 offers

Read from `i-slint-compiler-1.19.0/builtin_elements.rs` (lines 1263 to 1315) and `i-slint-core-1.19.0`:

- `StyledText` has `selectable: bool`. Press and drag selects, a double click selects a word and a triple click a paragraph, and a click that does not drag still follows a link. The press takes the focus; while focused, `StandardShortcut::Copy` and `SelectAll` are handled (`items/text.rs`, `key_event`). Selection colors come from the style's palette. There are also `select-all()`, `copy()` and `clear-selection()`.
- Selection is per element: one drag cannot run across two `StyledText` elements, and at most one shows a selection at a time.
- A `string` bound to `StyledText.text` converts through `string_to_styled_text`, which is `StyledText::from_plain_text`: no markdown is parsed, so text from the feed stays literal.
- `StyledText` has `default-font-family`, `default-font-size` and `default-color`, but no `font-weight` and no `wrap`; it always wraps to its width.
- `Text` (`ComplexText`) has no selection at all. `TextInput` with `read-only: true` selects and copies, and keeps `wrap`, `font-family` and `font-size`.

## Decisions

### 1. The feed's URLs may point anywhere

Delete `NOTES_PREFIX`, `ASSET_PREFIX` and `is_under`. `read_release` keeps any string `notes`, and `read_asset` any string `url`. `open::is_web_url` stays where it is in the app: it is not a domain check but the guard for `cmd /C start`, so a URL with `&`, `%`, `^`, `,` or `;` still counts as missing and the download falls back to `https://sunlit.earth/#download`. The rule list in the module doc of `feed.rs` goes from seven rules to six.

`SUNLIT_EARTH_UPDATE_URL` can point the check at another file, and that file is now trusted as much as the real one. It is a test and development knob, so this is accepted.

The website's script drops its prefix check as well, or the first release hosted elsewhere fails the site's deploy. That edit is in `sunlit-earth/website`, not in this repository; its other checks (version, date, sizes, an archive per target for the newest release) stay.

### 2. Prose is selectable; headings and link labels are not

Built, then reverted at the user's word (departure 7): no text in these windows is selectable.

| Window | Element | Becomes |
|---|---|---|
| About | attributions and third-party paragraphs and items | `StyledText` with `selectable: true` |
| About | "Version …" and the copyright line | selectable `StyledText` |
| About | License tab body | read-only multi-line `TextInput`, mono family, 12 px, `wrap: no-wrap` |
| Update | release highlights, `failure`, `known`, `command-hint`, `archive-step` | selectable `StyledText` |
| Older version notice | the body paragraph | selectable `StyledText` |

The headings stay `Text`, unless `@markdown("**\{text}**")` renders them at the same weight and size, in which case they become selectable too; that is checked by eye during implementation, and the plan does not depend on it. The link labels ("sunlit.earth", "Source code", "Release Notes", each version that opens its page) stay `Text` with their `TouchArea`: a selectable label would turn a click into a press that takes focus.

### 3. Frame guides as a fixed overlay

Settled with the user on 2026-11-23 against a mockup (the Design artifact "Frame guides preview"). All of it is in `frame-guides.slint`:

- **Two guides, 4:3 and 16:9.** The anchor screen's guide is dropped, and with it `screen-width`, `screen-height`, `has-screen`, `screen-is-wide`, `screen-is-classic`, the `guide-screen` out property, `frame-screen-width` and `frame-screen-height` on `MainWindow`, and their two setters in `engine_client.rs`. `PreviewFrame::screen` in core goes too if nothing else reads it once the setters are gone; that is checked during implementation.
- **Thick, translucent outlines.** Each guide is a 10 px band drawn inside its rectangle, in `OverlayStyle.fg` at 18% alpha, the same color for both; the accent is not used. Both rectangles have their short side on the displayed short edge, so they are always equally tall and their top and bottom edges always lie at the same height, on the panel's own edges in a landscape panel and across the panel in a portrait one.
- **No band drawn twice.** 4:3 is narrower than 16:9 at the same height, so it always lies inside it. 4:3 is drawn as a whole rectangle; 16:9 as its two side bars plus, on each side, a top and a bottom segment that runs from its own side bar to the outer edge of the 4:3 rectangle. Every visible point of a band is therefore one layer, and the translucency is the same everywhere.
- **Labels in their own corner.** "16:9" and "4:3" each sit in the top left corner of their own rectangle, inside the band plus a 5 px inset, in `OverlayStyle.fg` at 70%. They move with their outline and leave the panel with it: no clamping, no stacking, no markers for a guide wider than the panel. The two corners are 0.22 of the short edge apart horizontally, so the labels, about 37 px wide at 16 px, only meet on a short edge under about 170 px. The mockup had 11 px; the user asked for about half again as large after seeing the app.
- **A center mark, visible at any size.** A cross at the center of the panel, two 1 px strokes with arms of 12 px in `OverlayStyle.fg` at 80% over a 3 px dark halo, and around it a 16:9 rectangle one third of the panel's height, centered, drawn as its four corners only: two arms of 22 px each, 3 px thick, `OverlayStyle.fg` at 50%. It is the part that says the guides are on when the outlines are off the panel.
- Visibility is unchanged: everything shows with `guides-visible`, fades with `idle-hidden`, and stays under every control drawn over the preview.

`FrameGuide` loses `shown`, which existed for a guide that coincided with the screen's. The component exports `guide-wide`, `guide-classic` and a new `guide-center` (the corner rectangle) through `MainWindow`, plus the width of the 16:9 top and bottom segments, so the tests can read the geometry. The alpha values and arm lengths above are the mockup's; they are checked once more in the running app, over the day side and over space.

The hint in `overlays/common.slint` ("Outline this screen, 16:9 and 4:3 on the preview, …") is rewritten for two guides and the center mark.

### Not chosen

- **A matte** (translucent shading outside each guide): the edges of the picture are mostly the black of space, where shading shows nothing.
- **Labels clamped into the panel and edge markers** for a guide wider than the panel: more machinery than a framing aid needs. The center mark covers "the guides are on".
- **Keeping the anchor screen's guide**, dropped at the user's word.
- **Letterboxing the preview while the guides are on**, so the whole screen picture is always visible. It changes how the engine sizes the preview frame, a larger change than this one.
- **Replacing the Windows opener** so URLs with `&` or `%` open too. Not needed for hosting elsewhere.
- **One selectable document for the About tabs.** Slint's selection is per element, and folding the tabs into one `StyledText` would undo the block layout of [2026-09-04-about-window-overhaul-plan.md](2026-09-04-about-window-overhaul-plan.md).

## Implementation steps

1. **Feed URLs.** `feed.rs`: delete the prefixes and `is_under`, simplify `read_release` and `read_asset`, rewrite the module doc and the field docs of `Asset::url` and `Release::notes`. Tests: remove `a_url_outside_its_prefix_is_missing`, `a_url_that_could_climb_out_of_its_prefix_is_missing` and the rejection cases of the notes test; add `a_url_on_any_host_is_kept` for both fields. `update.rs` is unchanged.
2. **Selectable text** (reverted, departure 7). Convert the elements of decision 2. Keep the element ids the tests use (`release-line`, `attributions-paragraph`, `license-body` and the rest). Where `StyledText` does not expose its text as `accessible-label` the way `Text` does, set `accessible-role: text` and `accessible-label` explicitly, so the tests and screen readers keep finding it.
3. **Frame guides.** Remove the screen guide and its plumbing; replace the 1 px outlines with the 10 px bands and the split 16:9 segments; move the labels into their corners; add the center mark; update the hint.
4. **Docs**, below.

## Tests

### Unit, `sunlit-core`

- A `notes` URL and an asset `url` on another host, `http` or `https`, are kept as given.

### `slint_ui`

- (Reverted with decision 2, departure 7.) Each element of decision 2 has `selectable` set, found by its id. Where the testing backend delivers pointer events, a drag across a release highlight inside the update window's `ScrollView` leaves `has-selection` true and the view's `viewport-y` unchanged; if it does not, that case is covered in the manual check below instead.
- Escape still closes the About window and the update window after a click into a selectable paragraph has taken the focus.
- The license body still carries the license text and does not wrap: its width is wider than the tab.
- `test_the_frame_guides_outline_each_aspect_on_the_displayed_short_edge` drops its screen cases and covers a wide panel, a 4:3 panel, a panel narrower than 4:3 and a portrait panel: both rectangles are centered with their short side on the displayed short edge, the 16:9 segments end exactly at the 4:3 rectangle's outer edge (so no point is covered by two bands), and each label lies at its own rectangle's top left corner plus the band and the inset.
- The center rectangle is 16:9, one third of the panel's height and centered, at each of those panels, and the center mark is visible exactly when `guides-visible` is.
- `test_the_frame_guides_follow_the_frame_shown_while_a_resize_settles` reads `guide-wide` instead of `guide-screen`.

### Manual, on this desktop

- (Reverted with decision 2.) Select and copy a paragraph from each About tab, a highlight and the failure text from the update window (`SUNLIT_EARTH_UPDATE_URL` at an unreachable address gives a failure), and paste them elsewhere.
- Look at the guides at a wide, a 4:3, a narrow and a portrait window, windowed and in full screen, over the day side and over space.

## Docs

- `docs/architecture.md`: the `feed.rs` paragraph (line 115) loses the prefix rule; the `AboutWindow` and `UpdateWindow` paragraphs say what is selectable; the frame guides sentence at line 429 describes the two guides, their bands and the center mark, and no longer mentions the screen's outline.
- `docs/plans/2026-11-22-update-check-plan.md`: rule 6 and departure 4 get a note that the prefix rule was dropped on 2026-11-23, pointing here.
- `docs/plans/2026-10-24-short-edge-framing-plan.md` describes the guides as built then; it is left as history, and this plan is the current word.
- `docs/roadmap.md`: an entry for each of the three, checked off when done.

## Gates

`cargo fmt --check`, `cargo test`, `cargo clippy --all-targets`, and the manual checks above. The website change lands before the first release whose `v1.json` names a URL outside GitHub.

## Departures

1. **A string reaches `StyledText.text` only through `@markdown`.** "What Slint 1.19 offers" says a `string` bound to `StyledText.text` converts implicitly. It does not: the compiler refuses with "Cannot convert string to styled-text. Wrap the expression in `@markdown("\{...}")` to convert it explicitly" (`expression_tree.rs`, `maybe_convert_to`). Every converted element therefore binds `@markdown("\{value}")`. The point the plan relied on still holds: `from_at_markdown` in `passes/resolving.rs` passes each interpolated value through `StringToStyledText`, which is `StyledText::from_plain_text`, so text from the feed is never parsed as markdown, and only the literal parts around it (such as "Version ") are.

2. **The headings question was settled from the sources, and the About headings became selectable.** The plan left it to a look at the running app. Reading Slint 1.19 instead: markdown's `**…**` is `Style::Strong`, which `sharedparley/shaping.rs` renders as `FontWeight::BOLD`, 700, the weight the About window's `Text` headings had; `default-font-size` gives the size, and `apply_default_properties_from_style` gives `StyledText.default-color` the same `Palette.foreground` a `Text` gets. So the tab headings of both markdown tabs and the header's "Sunlit Earth" are `StyledText` with `@markdown("**\{heading}**")` and `selectable: true`. The update window's headline, the date after a release's version and the older version notice's heading are weight 600, which markdown cannot express, so they stay `Text`. The look in the running app remains part of the manual check.

3. **`PreviewFrame::screen` is gone from core, and with it the frame sent again for a same-shaped anchor.** Once the window's setters went, nothing read it but the engine test that checked it. `Engine::drawn_framing` became `drawn_short_edge`, and `update_preview_screen` no longer marks the drawn frame as owed when a new anchor frames the preview the same way, since that frame was sent again only to say which screen it was for. `the_preview_frame_says_which_screen_it_was_framed_for` is now `the_preview_frame_is_framed_for_the_anchor_screen`, asserts short edges, and keeps its unplug case: the portrait anchor frames the preview on a short edge of 144 and the ultrawide primary that replaces it on 256, so the engine draws a new frame. Only a new anchor that frames the preview as the one before it did now sends no frame.

4. **The tests check what selection does, not the `selectable` flag.** The testing backend reads built-in properties of an element only through accessibility or `@testable`, an experimental feature. Each element of decision 2 is found by its id, clicked, and sent Ctrl+A and Ctrl+C, and the clipboard, read back through the `ClipboardProbe` the update tests already had (now multi-line, with a `put` that leaves a known text first), must hold its text. Text that is not selectable never takes the focus and leaves the known text there. The drag case reads the copied part of the highlight instead of `has-selection`, and the highlight's position instead of `viewport-y`. The center mark's visibility is read from an element query, which skips what is not visible.

5. **The center rectangle's corners leave a narrow portrait panel.** One third of the panel's height at 16:9 is wider than a panel narrower than 16:27 of its height, so there the four corners are off the panel and only the cross stays; the test expects exactly that. The plan's "visible at any size" holds for the cross. The cross's halo, which the plan calls dark without a value, is black at 50%.

6. **The guides' bands carry element ids.** `classic-band`, `wide-side`, `wide-top`, `wide-bottom`, `wide-label`, `classic-label`, `center-mark`, `center-corner` and `center-cross` let the guide test measure the bands in a wide panel, where all are on screen, so "no point under two bands" is checked on what is drawn rather than only on the exported numbers.

7. **Selectable text is reverted.** Trying it in the app, the user found that Ctrl+A selects only the clicked paragraph and a drag cannot run on into the next one. That is Slint 1.19's model: a selectable `StyledText` takes the focus on a press and its selection goes with the focus, so at most one element holds a selection, and nothing reports which character lies under the pointer, so a selection across elements cannot be built on top either. 1.19.0 is the newest release. The ways around it were a Copy button per document, or one element per document (a single `StyledText`, or a read-only `TextInput` as plain text), which gives up the block layout of the About tabs, the gaps and hanging indents of the release notes, and the links on each version. The user chose none of them, so `about.slint`, `update.slint` and `older-version.slint` are back to how they were, with the selection tests, `test_feed_text_is_copied_as_written` and the docs of decision 2. Departures 1, 2 and 4 and the selection items of the validation rounds below describe the reverted work. The license tab is plain `Text` again, as before this plan.

## Validation

Round 1: 0 major, 6 minor, each fixed.

1. `docs/rendering.md` described the anchor screen's guide and a frame that carries its screen; it now describes the two guides and the center mark, and the frame carries only its short edge.
2. `docs/testing.md` described the removed guide cases; it now describes the current guide and center mark cases and, in a new paragraph, the select and copy cases, Esc after a click and the drag without a scroll.
3. `docs/architecture.md` said the whole center mark stays on the panel at any size; it now says the cross does and the corners stay while the rectangle fits, as departure 5 has it.
4. No test held the feed's text literal through `@markdown("\{...}")`. `test_feed_text_is_copied_as_written` gives a release highlight and the failure text markdown and HTML characters and requires Ctrl+A, Ctrl+C to copy them as written. Swapping the highlight, locally and uncommitted, for the same text in a markdown literal made it copy `bold x a y * z` and fail.
5. The vertical stroke's halo of the center cross was drawn over the horizontal stroke. The cross is now two halos and then two strokes, `cross-halo` and `center-cross`, and the center mark test counts both.
6. The branch's own lines said "licence"; they say "license" now, and the lines that were there before are left alone. Departure 3 gave a wrong reason for moving the unplug case to a portrait primary: run locally, the original ultrawide primary still gets a frame, since its short edge differs from the portrait anchor's. The ultrawide case is restored and departure 3 says so.

Round 2: all six fixes of round 1 verified; 0 major, 4 minor, each fixed.

1. `docs/usage.md` still described the screen's outline; it now describes the two bands, the labels and the center mark, and says the paragraphs of the update and About windows can be selected and copied.
2. A sentence of `docs/architecture.md` the branch rewrote still said "licence tab"; it says "license tab" now.
3. `docs/roadmap.md` and the header comment of `frame-guides.slint` said the whole center mark stays on the panel; they now say the cross does.
4. `docs/architecture.md` still counted seven rules in the update check plan; it now says rule 6 was dropped and `feed.rs` lists the six that remain.
