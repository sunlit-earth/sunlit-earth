# Research: Export Image from the main window

## Question

The menu bar plan reserved File > Export Image… (Ctrl+E) for saving the current view as an image, which today only the `render` subcommand does. This document collects what the app already has, what limits an export, how to show progress while one runs, which save dialog to use, and how to keep the export window in front of the main window. The plan is [2026-11-08-export-image-plan.md](2026-11-08-export-image-plan.md).

Everything below was read in the source of the versions in `Cargo.lock` (image 0.25.9, png 0.18.1, slint 1.18.1, winit 0.30.13, wgpu-types 28.0.0) or in the published rfd 0.18.0 crate, unless it is marked unverified. Nothing was run.

## What the app has today

- `run_render` (`crates/sunlit-app/src/headless.rs`) starts an engine at the requested size, waits for `TexturesReady` (two minutes at most) and for `CloudsSettled` when a cloud layer is drawn and has a source (one minute at most), then calls `EngineHandle::render_to_file`.
- `EngineCommand::RenderToFile { path, width, height, reply }` and `EngineCommand::ExportPixels { width, height, reply }` both go through `export()` in `engine/publish.rs`. An export waits for its own surface tiles, up to `TILE_WAIT` (5 s), and is then drawn with whatever is resident. `RenderToFile` encodes the PNG on the engine thread through `save_png` and `files::write_png` (`CompressionType::Fast`, `FilterType::Adaptive`). `ExportPixels` hands the RGBA buffer back and is used today only by the e2e `export-test` probe.
- Every export is framed as a screen of its own size, measured on the short edge, and drawn at the time setting, never at the excursion the preview may be playing.
- `TexturesReady` is a latch that follows readiness both ways inside the engine (a drag, new tiles, a month or a resolution switch reopen it), but only the rising edge is sent to clients. `CloudsSettled { drawn }` is sent once per engine.
- `files::unfinished` gives a unique temporary name beside a target, and `files::sweep_unfinished` removes the leftovers of killed writers.

## Device limits

`check_export_fits` (`engine/publish.rs`) refuses a render the device cannot make, with a sentence instead of a wgpu validation error or a panic in the readback. Only the wallpaper path calls it today, so `RenderToFile` and `ExportPixels` past the limits fail the unreadable way its comment describes.

1. **Texture side.** `wgpu_init.rs` requests `downlevel_webgl2_defaults().using_resolution(adapter.limits())`, so `max_texture_dimension_2d` is the adapter's own, 16384 on typical desktop GPUs.
2. **Readback buffer.** `using_resolution` copies only the three texture dimensions. `max_buffer_size` stays at the default, 256 MiB (`defaults()` in wgpu-types 28, which `downlevel_defaults` and `downlevel_webgl2_defaults` inherit). The readback pads rows to 256 bytes, so the cap is about 67 MP: 8192×8192, or about 10900×6100 at 16:9.
3. **GPU memory.** The color, depth and MSAA targets of a large export are several times the image. Where that fails depends on the card.

Ultra high resolutions would need tiled rendering with off-center projections for the globe and the stereographic sky. That is out of scope; an export past the limits gets the error message.

## Progress while encoding

### PNG

png 0.18.1 has `Writer::into_stream_writer()`, a `StreamWriter` that implements `Write`, takes the raw scanlines in chunks of any size, filters and compresses them as they come, and writes IDAT chunks as it goes. Feeding it a few rows at a time and counting the bytes written against `width * height * 4` gives exact progress.

`Compression::Fast` maps to `DeflateCompression::FdeflateUltraFast` (`common.rs`, `from_simple`), and the stream writer builds `fdeflate::Compressor` for that (`Wrapper::from_level`), the same compressor `write_image_data` uses, which is what image's `CompressionType::Fast` reaches today. Two differences: the stream writer has no fallback to stored blocks when compression made the data larger (the doc comment on `Compression::Fast` says so), and it does not hold the whole compressed image in memory, which `write_image_data` does. The pixels are identical; the file bytes are not.

png 0.18.1 is already in the lock through image, so a direct dependency adds no crate.

The compression effort can be set per encoder with `set_deflate_compression(DeflateCompression)`: `NoCompression`, `FdeflateUltraFast` (what `Compression::Fast` and `Fastest` map to), or `Level(u8)` from 1 to 9 through flate2, whose documentation says higher values compress better at the cost of speed and that the exact meaning of each level may change in semver compatible releases. The stream writer supports all three (`Wrapper::from_level`). Whether flate2's low levels compress better or worse than fdeflate on our renders is not known and is measured in the plan's Step 1.

### JPEG

image 0.25.9's `JpegEncoder::encode_image<I: GenericImageView>` is documented to "iterate over 8-by-8 blocks of pixels at a time, inspecting each pixel exactly once. You can rely on this behaviour when calling this method." The loop is rows of blocks from the top (`for y in (0..height).step_by(8)`, then `x`). A `GenericImageView` over the RGBA buffer whose `get_pixel` notes the row it is asked for therefore gives progress in steps of eight rows, at the cost of one comparison per pixel.

`encode_image` takes `Rgba<u8>` pixels and drops alpha through `to_rgb()` in `rgb_to_ycbcr`, so no RGB copy of the buffer is needed. `encode()` with a byte slice rejects `Rgba8`. Quality is 1 to 100 (`new_with_quality`). The chroma components are built with `h: 1, v: 1`, so there is no chroma subsampling. Width and height must fit in a `u16`, which is above every size a GPU renders.

### Cancelling

Both encoders write through a `Write` the caller supplies. A writer that returns an `io::Error` once a cancel flag is set stops either encoder at its next write, and the temporary file is then removed. The GPU render and the readback cannot be interrupted; a cancel during them discards the pixels when they arrive.

### The bar

Slint 1.18's std `ProgressIndicator` has `progress` (0 to 1) and `indeterminate`. Both formats get a determinate bar, so `indeterminate` is not needed.

## Save dialog

Researched by a subagent from upstream sources and issues; the directory handling below was then read in the rfd 0.18.0 crate source.

### Candidates

- **rfd 0.18.0** (MIT, MSRV 1.92, active). Windows: `IFileDialog` through COM. macOS: `NSSavePanel` through objc2. Linux: the XDG desktop portal over libdbus, loaded at runtime, with a zenity fallback. Default features `xdg-portal` and `wayland`; `gtk3` and `gtk4` are opt in. No async runtime since 0.17, and no zbus.
- **native-dialog 0.9.7** (MIT) shells out to zenity, kdialog or yad on Linux and never uses the portal.
- **tinyfiledialogs** (last release 2022, a nonstandard zlib variant license) and **nfde** (0.0.9) are not credible.
- **ashpd** is portal only and depends on zbus 6, beside the zbus 5.14 the tree has.
- Slint has no file dialog. Slint issue [#9781](https://github.com/slint-ui/slint/issues/9781) tracks one for the other language bindings, and the maintainers recommend rfd for Rust.

### rfd on Linux

- The portal backend needs nothing at build time. At runtime it needs `libdbus-1.so.3`, xdg-desktop-portal, and a FileChooser implementation: the kde, gnome or gtk portal. xdg-desktop-portal-wlr implements only Screenshot and ScreenCast, so a sway session needs the gtk portal and `portals.conf` routing FileChooser to it.
- Without a session bus or libdbus, or when the portal call fails, rfd logs "Using zenity fallback" and runs `zenity --file-selection --save --confirm-overwrite`. When zenity cannot be spawned either, it returns `None`, which is what a cancel returns.
- Once a portal answers, rfd waits for the response signal without a timeout.
- The gtk3 feature hangs in a host with its own event loop (rfd #198) and is of no use under winit.

### Threading and parenting

- `AsyncFileDialog::save_file()` from `slint::spawn_local` is the pattern Slint users report. On macOS with a parent it runs as a sheet.
- `set_parent` takes `HasWindowHandle + HasDisplayHandle` from raw-window-handle 0.6. `slint::Window::window_handle()` implements both, and the app already enables `slint/raw-window-handle-06` and calls it in `app.rs`. The handle exists only once the window has been shown and the event loop has run. On Wayland rfd hands the portal an xdg-foreign handle.

### Known problems (unverified on 0.18)

- [rfd #164](https://github.com/PolyMeilex/rfd/issues/164), [Slint #4084](https://github.com/slint-ui/slint/issues/4084), open: on Windows 10 and 11 the overwrite prompt can freeze the app until Alt is pressed. Reported up to rfd 0.15.1.
- [Slint #4773](https://github.com/slint-ui/slint/issues/4773): the Downloads folder stuck on "Working on it" with the sync dialog; a thread worked around it.

### Behavior the plan relies on

- **Overwrite confirmation** is native: `FOS_OVERWRITEPROMPT` is a default of the Windows save dialog, NSSavePanel prompts, the portal backends prompt, and the zenity fallback passes `--confirm-overwrite`. The macOS and portal halves are from documentation, not from rfd's code.
- **The chosen filter is not returned**, only the path. On Windows rfd sets the default extension from the first filter, and the dialog follows the type the user picks. On macOS rfd swaps the allowed types with a format popup. Whether a portal backend appends an extension is unverified, so the app normalizes the extension itself.
- **No folder memory of its own.** rfd passes `set_directory` to `IFileDialog::SetFolder` (forced; a comment in `dialog_ffi.rs` says `SetDefaultFolder` did not hold), to `setDirectoryURL` on macOS, as `current_folder` to the portal, and to zenity's `--filename`. Without a directory each platform falls back to its own memory, which differs between them and between portal backends. A folder that must survive a restart is the app's to store and pass every time.

## A window that blocks the main window

Slint 1.18 has no modal top level window: `PopupWindow` lives inside its parent, and `Window` has no owner or modality property. What winit 0.30.13 offers, per platform:

- **Windows.** `WindowAttributesExtWindows::with_owner_window(HWND)` makes an owned window: always above its owner, hidden with it when the owner is minimized. `WindowExtWindows::set_enable(false)` on the owner blocks its mouse and keyboard input, and winit's documentation names the pair as the way to build a modal dialog; the owner must be enabled again before the dialog is hidden, or another application gets the focus. Both are safe methods.
- **X11.** `with_x11_window_type(vec![WindowType::Dialog])` marks the window as a dialog for the window manager. winit exposes no `WM_TRANSIENT_FOR`; `with_parent_window` makes a child confined to the parent's client area, which is not a dialog.
- **Wayland.** No parent and no modality in winit 0.30. Raising a window needs an activation token.
- **macOS.** `with_parent_window` is `unsafe` and adds a child window; there is no sheet for a winit window.

Slint applies winit attributes through `BackendSelector::with_winit_window_attributes_hook`, behind the `unstable-winit-030` feature, which Slint says may change in a minor release. The hook is one closure called for every window the backend creates. `BackendSelector::select` still reads `SLINT_BACKEND`, so the guests' `winit-software` keeps working. The same feature gives `WinitWindowAccessor::with_winit_window` for `set_enable` and `focus_window` on a live window.

Precedent: Slint issue [#6607](https://github.com/slint-ui/slint/issues/6607), "Modal windows / dialogs", has been open since October 2024. Its proposal is the Windows owner and `set_enable` pair above, "ineffective" elsewhere. In June 2025 a Slint maintainer wrote that the work in progress, PR [#8135](https://github.com/slint-ui/slint/pull/8135) "Window modality", implements `Window::show_modal` for the Qt backend only, that the winit implementation "is more complicated as there is no direct equivalent", and that on X11 it "doesn't seem possible at this point" without changes to winit. The PR is still open (last updated 2026-11-06). A commenter's project implemented modality on X11, Wayland and macOS with platform calls of its own beside winit, which here would mean new FFI on three platforms.

That gives the owner relation and real input blocking on Windows only. Everywhere else the main window has to block itself: Slint can lay a `TouchArea` over the whole main window, disable the `MenuBar` items (`MenuItem` has `enabled`), and disable the preview's `FocusScope` while the export window is open. None of this affects other applications.

Slint 1.18 has `KeyBinding` (the preview's shortcuts use it), so Esc in the About and Export windows is a binding each.

## Recommendation

- Encode on a worker thread from `ExportPixels`, through a png `StreamWriter` and a progress-reporting `GenericImageView` for JPEG, both behind a cancelling writer into a temporary file that is renamed at the end.
- Call `check_export_fits` for every export and word its message for an export.
- rfd 0.18 with default features, `AsyncFileDialog` from `spawn_local`, parented on the main window, the path stored in the config.
- No separate window: a dialog drawn inside the main window, over a backdrop that blocks its input, is modal on every platform without the unstable Slint feature or any per platform code. The Windows owner and `set_enable` pair would give a real modal window on Windows alone. The plan records the choice.

## Open questions

1. Whether the Windows overwrite freeze still happens with rfd 0.18, with a parent, and with the async dialog.
2. What sway and i3 do with no portal, with zenity only, with neither, and with the gtk portal but no `portals.conf` routing.
3. Whether a portal backend appends the extension of the chosen filter.
4. How long the render, the readback and each encoder take at 4K, 8K and near the 67 MP cap in a release build. That sets the progress bar's fixed steps.
