# Export Image from the main window

Research: [2026-11-08-export-image-research.md](2026-11-08-export-image-research.md). The crate behavior this plan rests on is sourced there, and what nobody has observed yet is listed there as an open question and turned into a check in Step 10 here.

## Problem

Saving the current view as an image needs the `render` subcommand, a terminal, and the size typed as flags. The menu bar plan reserved File > Export Image… (Ctrl+E) for doing it from the window. The About window also does not close on Esc.

## Decisions

These were settled with the user on 2026-11-08.

### 1. A dialog drawn inside the main window

Export Image… opens `ExportSheet`, an overlay in `MainWindow` like the shortcut sheet and the version notice: a backdrop that dims the whole window, the settings panel and the preview alike, with a card centered on it. While it is open the main window takes no input; the user closes the sheet to change the view. Other applications are not affected.

A separate window was considered and dropped. Slint 1.18 has no modal windows: issue [#6607](https://github.com/slint-ui/slint/issues/6607) is open, the modality PR covers only the Qt backend, and winit 0.30 offers an owner only on Windows. Elsewhere the export window could end up behind a main window that ignores its input, and bringing it forward can be refused by the window manager. The sheet is modal by construction on every platform, needs no unstable Slint feature and no per platform code. The user looked at a placeholder version in the running app on 2026-11-08 and chose it as the least bad option.

- **Look.** The card uses the settings panel's colors (`Palette`) and the std widgets, so it follows the system's light or dark scheme like the panel: `Palette.background` with a `Palette.border` border, 10 px radius, the version notice's drop shadow, a 15 px semibold heading. Backdrop `#0000008c`, as the other two overlays.
- **Close button.** A × in the card's top right corner, in the heading's row: a 24 px hit area with a 12 px icon, `icons/close.svg` drawn like the other icons (black stroke, colorized with `Palette.foreground`), a faint hover and pressed background, and the accessible role of a button labeled "Close". It is in the top right on every platform, macOS included; matching each platform's habit is not worth the code. It does what the Close button and Esc do.
- **Pointer.** The backdrop's `TouchArea` takes clicks and the scroll wheel, so nothing reaches the panel or the preview. A click on the backdrop does nothing; only the × button, Close and Esc dismiss the sheet.
- **Keyboard.** Opening the sheet moves the focus into it, so the preview's `FocusScope` and its shortcuts receive nothing; closing it returns the focus to the preview.
- **Menus.** Every `MenuItem` gets `enabled: !root.export-sheet-open`. The Linux menu bar is drawn in the window and the Windows and macOS ones are native, so the binding, not the backdrop, is what blocks them. On macOS the App menu Slint builds (About, Quit) stays enabled; Quit cancels a running export.
- **Held input.** The app clears the preview's held keys and held zoom buttons when the sheet opens.
- **Hiding the window** (Close Window, the title bar's close button, the tray's Show / Hide) closes the sheet first, cancelling a running export, so a hidden window never keeps the scene held (decision 3).
- **Size.** The card is `min(520px, window width - 16px)` wide and must fit the main window's minimum size; if its content is taller than the window, the card's body scrolls and its buttons stay visible.
- **The native save dialog** is parented on the main window.

### 2. Layout

The placeholder the user looked at, `crates/sunlit-ui/ui/overlays/export-sheet.slint`, is the starting point:

```
░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░
░░░░ ┌──────────────────────────────────────────────────┐ ░░░░
░░░░ │ Export Image                                   × │ ░░░░
░░░░ │ ┌──────────────┐ Size     [ DELL U2720Q · 3840 × 2160 ▾] │
░░░░ │ │  framing     │ Width    [ 3840 ] × [ 2160 ] [Swap]     │
░░░░ │ │  thumbnail   │ Format   [ PNG ▾]                │ ░░░░
░░░░ │ └──────────────┘ Compression [=====|====]      6 │ ░░░░
░░░░ │ Save to [ C:\Users\…\Pictures\sunlit-earth.png ] [Browse…] │
░░░░ │ [██████████████░░░░░░░░░░░░░░] Saving…            │ ░░░░
░░░░ │ Saved to sunlit-earth.png          [Show in Folder] │ ░░░░
░░░░ │                               [Close]  [Export]  │ ░░░░
░░░░ └──────────────────────────────────────────────────┘ ░░░░
░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░
```

- **Size.** A `ComboBox`: each connected monitor at its native resolution (monitors of one size merged into one entry naming them), the anchor screen first and the default; then 1920×1080, 2560×1440, 3840×2160, 5120×2880, 7680×4320, 2560×1600, 3440×1440, 1290×2796 (a phone, portrait), 2048×2048; then Custom. Width and height are fields of their own; editing either selects the entry that matches, else Custom. Swap exchanges them. There is no limit shown; decision 6 says what happens past one.
- **Thumbnail.** An `ExportPixels` render about 200 px on its long edge at the chosen aspect, redrawn when the aspect has been unchanged for 300 ms. The scene is frozen while the sheet is open (decision 3), so only the aspect matters. It shows what the short edge framing crops for a portrait or wide export, which the dimmed preview behind the card does not. It is the first thing to drop if the card does not fit the window's minimum size with it.
- **Format.** A `ComboBox` with PNG and JPEG, so another format adds an entry rather than a control. Below it one slider whose meaning the format sets: Compression for PNG (decision 5), Quality for JPEG (1 to 100, default 90). Each format keeps its own value.
- **Save to.** An editable path field and Browse…. Decision 4.
- **Progress and status.** The bar and the status line appear when an export starts and stay with the outcome until the next change in the sheet. During an export Export reads Cancel.
- **No other text.** No date line, no size estimate, no device limit.
- **Keys.** Enter exports when Export is enabled. Esc cancels a running export and closes the sheet.

### 3. The scene freezes while the sheet is open, and the export is what the preview shows

Opening the sheet sends `EngineCommand::Hold(true)`; closing it sends `Hold(false)`.

- **While held**, the engine pins the instant the preview showed when the hold began: the excursion's if there is one, else the time setting's, which for a live setting is the clock at that moment. The preview, the time bar and every export draw at that instant, and the month in force follows it (`month_instant` takes the held instant instead of the setting's). A running excursion is paused and stays paused after the sheet closes; a live setting goes back to the clock.
- **Wallpaper publishes wait** until the hold ends, since they draw at the time setting and the surface holds one month. A refresh that comes due meanwhile runs at release.
- **Camera motion stops.** The app clears the preview's held keys and held zoom buttons when the sheet opens, since the focus moves into the sheet before their release arrives.
- **Export waits for the scene.** At `Hold(true)` the engine announces readiness afresh: `TexturesReady` at once if what the preview needs is resident, else when it lands. The sheet opens with Export disabled and enables it on that event, once the cloud layer has also settled when one is drawn and has a source (the condition of `headless::waits_for_clouds`; `CloudsSettled` is sent once, so the client keeps that flag from startup). With nothing moving, readiness cannot be lost again while the sheet is open, so no new event is needed.

The export itself still waits up to `TILE_WAIT` for the finer tiles of its own size, as every export does.

This replaces the research document's note that an export draws at the time setting. Outside the hold, the `render` subcommand and the wallpaper still do.

### 4. Save dialog and path

- **rfd 0.18** with default features, `AsyncFileDialog::save_file()` from `slint::spawn_local`, parented on the main window. Filters in the order of the format list, the current format's first, so Windows' default extension is the current format's. The starting folder and the file name come from the path field.
- **The path field is always editable**, also while an export runs (the running export keeps the path it started with), so a path can be typed or pasted, and desktops with no file picker still work. A recognized extension in it (`png`, `jpg`, `jpeg`, any case) selects the format; choosing a format rewrites the extension in the field. At export a path with no recognized extension gets the format's appended. A relative path, or a folder that does not exist, is an error in the status line. A typed path that names an existing file shows "This file will be replaced." under the field, since only the native dialog asks.
- **The cheap check.** If the dialog returns no path within 300 ms of opening, nobody cancelled that fast, so the status line says "No file picker opened. Type the path above." That covers a desktop with neither a portal FileChooser nor zenity. Nothing else is detected.
- **The path is remembered** across restarts as `export_path` in the config, without its extension: whenever the dialog returns a path, and whenever an export succeeds. The field and every dialog start from it, folder and name, with the current format's extension. Before the first save the name is `sunlit-earth` in the Pictures folder (`dirs::picture_dir()`), else the home folder; a remembered folder that no longer exists falls back the same way and keeps the name.

### 5. Encoding on a worker, with progress

`sunlit_core::image_file` replaces `files::write_png`:

```rust
pub enum ImageFormat { Png { compression: u8 }, Jpeg { quality: u8 } }

pub fn write_image(
    path: &Path,
    pixels: &[u8],
    width: u32,
    height: u32,
    format: ImageFormat,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<(), WriteError>
```

- It writes to `files::unfinished(path, ".tmp")` and renames onto `path` at the end; any error or a cancel removes the temporary file. `WriteError` distinguishes a cancel from a failure.
- **PNG** through png 0.18's `StreamWriter` with `Filter::Adaptive`, fed in bands of rows, reporting the fraction of rows written. png becomes a direct dependency of `sunlit-core` at the version image already uses.
- **PNG compression** is a slider from 1 to 9, the deflate level users know from other tools. 1 is `DeflateCompression::FdeflateUltraFast`, today's setting and the fastest; 2 to 9 are `DeflateCompression::Level(2..=9)` through flate2. Step 1 measures time and size at every level on a real render; a level that is no smaller than the one below it is dropped from the slider, and the default is set from the measurement (6, png's own `Balanced`, unless the numbers argue otherwise). The wallpaper keeps level 1.
- **JPEG** through image's `JpegEncoder::encode_image` with a `GenericImageView` over the RGBA buffer that reports the row it is asked for. Alpha is dropped by the encoder.
- **Cancel** through a writer wrapper that fails once `cancel` is set, which stops either encoder at its next write.
- The wallpaper path and `RenderToFile` call it as PNG at level 1 with a no-op progress and a flag that is never set, so there stays one PNG writer. The wallpaper files' bytes change (the stream writer chunks IDAT and has no stored block fallback); their pixels do not.

The sheet's controller sends `ExportPixels` and, when the pixels arrive, spawns one named thread that calls `write_image`. That thread's consumer is the sheet, it runs once per export, and there is at most one export at a time. Progress goes to the sheet through `slint::invoke_from_event_loop`, throttled to at most one update per 50 ms. The pixel buffer lives only on that thread.

The bar has fixed steps for what the app cannot observe from inside: a small step when the request is sent, a larger one when the pixels arrive, the encoder's fraction in between, and the rest when the rename is done. Step 1 measures the phases and sets the step sizes; they are constants in the controller. A cancel during the render discards the pixels when they arrive.

### 6. Limits are an error, not a control

`check_export_fits` runs for every export, `RenderToFile` and `ExportPixels` included, and returns a structured error that each caller words. The sheet says, for example, "20000 × 11250 is larger than this GPU can render (16384 pixels on a side)." or "20000 × 11250 is more than this GPU can read back at once (8192 × 8192 is the largest square)." The `render` subcommand logs the same sentence and exits with failure instead of the panic it may hit today. Tiled rendering for sizes past the limits is out of scope.

### 7. Remembered settings

New `AppConfig` keys with defaults, so no migration step: `export_width` and `export_height` (the anchor screen's size until the first export), `export_format` (`png`), `export_png_compression` (decision 5), `export_jpeg_quality` (90), `export_path` (unset). Size, format and the two slider values are saved after a successful export; the path as decision 4 says.

### 8. Esc closes the About window

A `KeyBinding` for Escape in `AboutWindow` hides it the way its close button does, through `window_state::other_window_closed`.

### 9. Code layout

- `crates/sunlit-core/src/image_file.rs`: decision 5. `files::write_png` and `engine::save_png` go.
- `crates/sunlit-ui/ui/overlays/export-sheet.slint`: `ExportSheet`, grown from the placeholder. `MainWindow` places it as its last child, over the whole window, under `export-sheet-open`.
- `crates/sunlit-app/src/export.rs`: `ExportController` (opening and closing the sheet with the hold, presets, path handling, the dialog, the worker, the config writes). Pure functions (preset list from monitors, extension normalization, the starting path) are free functions with unit tests.

### Not chosen

- **Formats beyond PNG and JPEG.** TIFF and lossless WebP are a feature flag each in image but add little; lossy WebP has no encoder in image 0.25; AVIF and JPEG XL encoding are heavy dependencies. The format list makes adding one later a new entry.
- **Formats in the `render` subcommand.** Picking the format from `--output`'s extension and a `--jpeg-quality` flag are cheap after decision 5, but the CLI flags are documented in README, which is edited only with permission. Left for a later change.
- **Tiled rendering and band readback** for sizes past the device limits.
- **Exporting the whole multi screen span** as one image.
- **Copying to the clipboard.** Slint has no image clipboard API.
- **An indeterminate bar.** Both formats report progress (research, Progress while encoding).

## Implementation steps

Each step leaves `cargo test` and `cargo clippy --all-targets` green and is its own commit or few.

1. **Measure.** In a release build on the Windows desktop, time `ExportPixels` (request to pixels), `write_png` as it is, the stream writer at every compression level with its file size, and the JPEG path at 1920×1080, 3840×2160, 7680×4320 and 8192×8192. Record the numbers in the research document and set the progress steps, the PNG levels offered and the PNG default from them. A throwaway harness, not committed.
2. **`image_file`** (decision 5) with its unit tests, and every caller of `write_png` moved to it.
3. **Engine.** `check_export_fits` on every export with a structured error (decision 6); `Hold`, the held instant for the preview, the exports and the month, deferred publishes, and the fresh readiness announcement (decision 3).
4. **Esc in About** (decision 8).
5. **The sheet's blocking** (decision 1): the backdrop, the focus, `enabled` on every `MenuItem`, the held input cleared, hiding the window closing the sheet. The placeholder's menu item is already in place.
6. **The sheet** (decisions 2 to 4, 7): `export-sheet.slint` from the placeholder, `export.rs`, rfd, the config keys, the thumbnail; the card checked at the main window's minimum size. `cargo xtask bake licenses` after adding rfd and png.
7. **Tray and Quit**: Show / Hide and Quit close the sheet and cancel a running export (decision 1).
8. **IPC and e2e.** An `export-image <path> <width> <height> <png|jpeg> [quality]` IPC command that runs the sheet's export without the dialog and prints `SIGNAL:exported path=<path>` or `SIGNAL:export_failed reason=<text>`, and the e2e cases below.
9. **Docs.**
10. **Validation in the guests** (see Tests). Run by the supervising session with the user; the implementer's constraints forbid the VMs.

## Tests

Assert behavior, not constants.

### Unit

1. `write_image` PNG: a generated buffer round trips through image's decoder pixel for pixel; progress is monotonic, starts at or above 0 and ends at 1; a buffer of the wrong size creates nothing.
2. `write_image` JPEG: decodes to the right size, and the mean channel difference to the source is small at quality 90; progress as for PNG.
3. Cancel: setting the flag from the progress callback halfway returns the cancel error, and neither the target nor a temporary file is left.
4. A failing target folder returns an error and leaves nothing.
5. Extension normalization: no extension, `.PNG`, `.jpeg`, `.jpg` with JPEG chosen, `.txt`, a dot in a folder name, a trailing dot.
6. Format from a typed path, and the path after a format change.
7. The preset list: one monitor, two of one size merged, the anchor first, no monitors at all, a monitor that matches a common size listed once.
8. The starting path: stored and its folder present, stored and its folder gone (the name survives), unset (`sunlit-earth`).
9. The folder fallback when there is no Pictures folder.
10. `check_export_fits`: past the side, past the buffer with a width not a multiple of 64, and within both.
11. PNG at every offered level decodes to the same pixels.

### Engine (`tests/engine/`)

12. Under `Hold(true)` with a running excursion and with a live setting on the test clock, the clock moving does not move the instant of the preview or of an export, and an export draws the held instant, not the setting's; after `Hold(false)` a live setting follows the clock again and the excursion is paused.
13. A wallpaper refresh that comes due during a hold runs once at release.
14. `Hold(true)` with everything resident sends `TexturesReady` at once; with tiles pending, when they land.

### `slint_ui`

15. `ExportSheet`: Export disabled until ready, enabled after; the slider reads Compression for PNG and Quality for JPEG and keeps each value across a switch; Custom selected after editing a size; Swap exchanges the two; the path field accepts edits while an export runs; the replace warning follows its property; Esc and the × button close the sheet, a click on the backdrop does not.
16. `MainWindow` with `export-sheet-open`: menu items disabled; a click, a drag and the wheel over the preview and over the panel change nothing; a preview shortcut key does nothing; after closing, the preview has the focus again.
17. At the main window's minimum size the card's buttons are inside the window.
18. `AboutWindow`: Esc hides it.

### e2e (`crates/sunlit-app/tests/e2e.rs`)

19. `export-image` over IPC writes a PNG and a JPEG of the asked size that decode; the sheet's progress reaches the end.
20. `export-image` past the device side returns `export_failed` with the sentence and the app keeps running.
21. A cancel over IPC (`export-cancel`) during a large export leaves no file.

### Step 10, in the guests and on the runner

- **Windows** (`vm up windows`, and the desktop): overwrite an existing file through Browse, with `winit-software` and the default renderer (rfd #164); Browse into Downloads; the native menu bar is disabled while the sheet is open.
- **Linux**, KDE, GNOME, XFCE and Cinnamon on X11, and KDE and GNOME on Wayland: the dialog opens parented on the main window, starts in the remembered folder after a restart, confirms an overwrite, and whether it appends the extension for each filter.
- **sway and i3**: with the image as it is, then with no portal and no zenity (the status line hint appears and a typed path exports), then with the gtk portal but no `portals.conf` routing (record whether it hangs).
- **macOS**: compiles on the runner; nobody can look at the sheet or the native menu bar.

## Docs

- `docs/architecture.md`: `image_file`, the sheet and its worker thread next to the threading bullets, `Hold` and the held instant next to the excursion, the `export-image` IPC command.
- `docs/platforms.md`: rfd's Linux runtime needs (a portal with a FileChooser, or zenity) and what sway and i3 did in Step 10.
- `docs/usage.md`: Export Image, the remembered folder, the typed path for desktops without a file picker.
- `docs/testing.md`: the new cases.
- `docs/roadmap.md`: the Export Image entry done; open entries for tiled rendering past the device limits and formats in the `render` subcommand.
- `README.md` is not edited.

## Gates

Run from the worktree root, in this order, and report the real output.

```bash
cargo fmt --check
cargo clippy --all-targets
cargo unit
cargo test -p sunlit-earth --test slint_ui
cargo test
```

`cargo clippy --all-targets` must be clean. The Linux and macOS code must compile: a `ci.yml` dispatch on the branch, run by the supervising session, is the gate for those before Step 10.

## Constraints for every agent

- Work only in `C:\Workspace\rustrover\sunlit-earth\.worktrees\export-image` on branch `feature/export-image`. Never touch the main checkout or the other worktrees under `.worktrees/`.
- Never run `cargo xtask vm`, `cargo xtask e2e`, `cargo xtask dist`, `cargo e2e`, anything elevated, anything through WSL, or any golden regeneration.
- Do not edit `README.md`. Do not push. Do not create or touch PRs. Do not amend or rewrite commits.
- Commits: on the run branch, in reasonable chunks, with plain messages. No `Co-Authored-By` trailer or any other co-author line. Do not override the git identity.
- Code comments only where the code cannot be made clear without them, and never about history, tickets, or why a test exists. Docstrings use the Sphinx style the crate uses (`:param:`, `:returns:`).
- Writing style in docs, comments, logs, UI text and commit messages: American English, no em dashes, no en dashes, avoid dashes in sentences generally, no "genuinely", "delve", "crucial", "honestly". Bold and italics only for real callouts.
- Departures from this plan are numbered from 1 and recorded in the section below with a paragraph of reasoning.

## Departures

1. **The sheet is built from this plan, not from a placeholder.** Steps 5 and 6 start from a placeholder `export-sheet.slint` and a File menu item that are not on the branch, so `ExportSheet`, its `close.svg` icon and File > Export Image… (Ctrl+E) are written here from decisions 1, 2 and 4. The layout follows the sketch in decision 2 and the look follows decision 1; nothing the user chose about the placeholder is reinterpreted, but nobody has compared the result with it.

2. **Held zoom buttons are ignored rather than released.** Decision 1 and decision 3 say the app clears the preview's held keys and held zoom buttons when the sheet opens. A held key is released by the preview's `FocusScope` when the focus moves into the sheet, through its existing `focus-lost`. A zoom button's held state is its `TouchArea`'s `pressed`, which code cannot clear, so while the sheet is open `main.slint` ignores the camera panel's `stick` and `zoom-rate` callbacks instead. The effect is the one the plan asks for: nothing moves the camera while the sheet is open, and a button still held when the sheet closes resumes only on its next press.

3. **One sentence for a limit, worded in core.** Decision 6 has `check_export_fits` return a structured error that each caller words. It does return one, `ExportLimit` inside `ExportError::TooLarge`, and the sheet, the `export-image` IPC command and the `render` subcommand all show it through its `Display`, which is the sentence decision 6 gives as its example. The wallpaper words its own, since its sentence names the wallpaper and suggests one screen at a time. Three callers wanting the same sentence is why it lives beside the type rather than in each of them; a caller that wants other words matches on the variants.

4. **PNG levels and the default from Step 1.** Decision 5 leaves the levels offered and the default to the measurement. Level 4 was larger than level 3 at every size, so the slider offers 1, 2, 3, 5, 6, 7, 8 and 9. The default is 3, not 6: level 6 took three to six times as long as level 3 for files four to six percent smaller (11.2 s against 1.92 s at 7680×4320), and an export that takes ten seconds at the default reads as broken. The numbers and the progress steps they set are in the research document's Measurements section; `export_png_compression` defaults to 3 accordingly (decision 7).

5. **Ctrl going down lets go of a held E.** Slint hands a key release that matches an enabled menu shortcut to the menu and never to the focused element (`process_menubar_shortcuts` in i-slint-core 1.18.1 accepts both the press and the release and activates only on the press). With File > Export Image… on Ctrl+E, E let go while Ctrl is down never reached `preview-keys`, so a roll held through Ctrl went on after the key was up, which `test_a_held_key_let_go_under_a_modifier_stops_the_camera` caught. The shortcut stays, as the user chose it. `track-key` already lets go of a held Q when Ctrl goes down for the same reason with Quit on Ctrl+Q, and now does the same for E on every platform, since Export Image… is in File everywhere. The cost is that holding E and then pressing Ctrl stops the roll, as it already did for Q.

6. **Case 19 does not watch the bar.** The e2e suite sees the app only through its `SIGNAL:` lines, and the plan's `export-image` command answers with the outcome alone, so "the sheet's progress reaches the end" is not asserted there. The controller sets the bar to its end on the same branch that prints `SIGNAL:exported`, and adding a progress line to the IPC contract for one assertion was not worth it. Case 19 checks the answer, the files' sizes and that they decode, and that nothing else is left in the folder.

## Validation

Round 1 (876964e..3d622f7): 3 majors and 3 minors, all fixed in b7cea96, a1029c2, 59dad28 and d5a4339.

- Major: the time bar kept running under the hold with a live time setting.
- Major: an IPC `export-image` on a hidden window left the scene held.
- Major: Enter in a text field cancelled a running export.
- Minor: a wallpaper owed at exit was drawn at the held instant when the app quit with the sheet open.
- Minor: a sheet reopened while a cancelled job still stopped ignored Export silently, then showed the old job's "Cancelled."
- Minor: the thumbnail stayed in `ExportState` after the sheet closed.

Round 2 (3d622f7..d5a4339): every round 1 fix confirmed, 0 majors and 3 minors, fixed in 3b1fd0e, 6d2b918 and 7f63943.

- Minor: an export answered at shutdown was drawn at the clock, not the held instant; the hold is now lifted only around the wallpaper owed at exit, with an engine case in `readiness.rs`.
- Minor: "The previous export is still stopping." stayed on the sheet after the job stopped; the job's final report now clears it.
- Minor: the hidden window case covered only a refused IPC export. A job's report cannot reach the sheet in `slint_ui`, whose testing backend runs without an event loop, so `docs/testing.md` now says what the case covers instead of adding one.
