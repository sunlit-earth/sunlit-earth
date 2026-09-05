<!-- Reviewer notes for docs/reviews/2026-09-04-code-quality-review.md. Line numbers refer to commit 19312ba, the tree the review read, and were moved on 2026-09-05 for the files that changed on main since; the main report lists those under "Changes since the review". The changed code was not re-reviewed. "The brief" is the shared review instruction; "the maintainer's rules" are the project's comment conventions. Runtime claims here are reasoned from the code; the measured figures are in test-timing.md. -->

# Review: `desktop.rs`, `display.rs`, `display/layout.rs`, `display/watch.rs`

## 1. Summary

The logic in this area is in good shape. I found no reachable production panic, no data race, no resource leak on the normal shutdown path, and no unbounded growth. The display-change hint path is well built: the watcher has no opinion and the engine debounces the burst with a trailing settle window.

What is wrong is almost entirely volume. Of 18 long comment blocks I sampled, 17 have their prose already in `docs/platforms.md` or `docs/rendering.md`, in most cases near verbatim and in more detail. That is the single biggest lever here: roughly 220 comment lines in these four files are a second copy of documents the project already maintains.

Tests are 46 to 57 percent of every file except `watch.rs`, and about a quarter of them are subsets of a neighbor. None of them are slow, so the maintainer's runtime concern does not land in this area; the cost is reading, not seconds.

Three structural inconsistencies are worth fixing: `display` is the only multi-file module in `sunlit-core` that does not use `mod.rs`; the crate has three different ways of splitting per-OS code; and `display::monitors()` on Windows calls into `wallpaper.rs`, inverting the intended layering.

## 2. Metrics

| File | Total | Test-module lines | Comment blocks > 3 lines (keep / trim / delete) | Fns > 80 lines | `pub` used only in-crate |
|---|---|---|---|---|---|
| `desktop.rs` | 1262 | 591 (47%) | 30 (14 / 10 / 6) | 1 | `DESKTOP_ENV`, `detect`, `no_backend_message`, `Backend::degradation`, `Backend::nothing_to_run` |
| `display.rs` | 486 | 223 (46%) | 12 (7 / 3 / 2) | 0 | `parse_outputs`, `primary_monitor_of`, `Output::overlaps` |
| `display/layout.rs` | 1066 | 608 (57%) | 15 (10 / 4 / 1) | 0 | `SKY_FOV_MIN`, `screen_framing`, `Rect::right`, `Rect::bottom` |
| `display/watch.rs` | 590 | 102 (17%) | 14 (8 / 4 / 2) | 0 | `CLASS_NAME` (win32), `Watcher::hwnd` (win32) |

Other counts: 73 `#[test]` functions in total (desktop 26, display 12, layout 33, watch 2). Ten `#[allow(unsafe_code)]` sites, all in `watch.rs`. No `#[allow(clippy::too_many_lines)]` or `too_many_arguments` anywhere in the area. One `expect` in non-test code (`layout.rs:70`); zero `unwrap` in non-test code.

The `pub`-but-in-crate list needs a caveat: `desktop` and `display` are public modules of a library crate whose only consumer is `sunlit-app`, so some public surface is deliberate. The entries above are the ones with *no* caller outside `sunlit-core/src`, checked against `crates/sunlit-app` (including `tests/e2e.rs` and `tests/slint_ui.rs`), `crates/sunlit-core/tests`, and `crates/xtask`.

## 3. Findings

### A. Commentary

**A1 (high value, low risk). Roughly 220 comment lines restate `docs/` almost verbatim.**

I sampled 18 comment blocks over three lines and grepped a distinctive phrase from each against `docs/`. Seventeen matched:

| Source | Lines | Already in |
|---|---|---|
| `desktop.rs` module doc, "Not the XDG desktop portal" | 10-13 | `docs/platforms.md` |
| `desktop.rs` `Kind::Kde` doc, `plasma-apply-wallpaperimage` | 74-84 | `docs/platforms.md` |
| `desktop.rs` `xfce_live_property`, the XFCE guest incident | 210-219 | `docs/platforms.md`, `docs/vm-setup.md`, a plan doc |
| `desktop.rs` `plasma_script`, the sort and the null holes | 469-480 | `docs/platforms.md`, a plan doc |
| `desktop.rs` `BACKENDS`, "four verified live, phase 5 decision 9" | 531-541 | `docs/platforms.md` |
| `desktop.rs` `detect`, `Budgie:GNOME` vs `ubuntu:GNOME` | 625-630 | `docs/platforms.md` |
| `display.rs` module doc, XWayland scaling and the D-Bus roadmap item | 10-16 | `docs/platforms.md` |
| `watch.rs` module doc, "a watcher that cannot start is not an error" | 16-18 | `docs/platforms.md` |
| `watch.rs` `mod x11` doc, the XWayland paragraph | 52-59 | `docs/platforms.md`, a plan doc |
| `watch.rs` `mod x11` doc, "RandR rather than the alternatives" | 61-65 | `docs/platforms.md` |
| `watch.rs` `OUTPUT_PROPERTY` and EDID/backlight | 186-188 | `docs/platforms.md` |
| `watch.rs` `mod win32` doc, message-only windows get no broadcast | 232-239 | `docs/platforms.md` |
| `layout.rs` module doc, the two lens axes | 14-21 | `docs/rendering.md:139`, a plan doc |
| `layout.rs` `SKY_FOV_MAX`, "eight screens 331, a thousand 359.7" | 245-259 | `docs/rendering.md:151` |
| `layout.rs` `contain_camera_fov`, "a sky that was never in trouble" | 263-274 | `docs/rendering.md:141` |
| `layout.rs` `canvas_framing`, "carries the user's own pan" | 306-316 | `docs/rendering.md:147` |

Only `desktop.rs:133-141` (the `by_position` holes) had no match, and even that idea appears in `platforms.md` in different words.

Recommendation: for each of these, keep the one or two sentences that state the invariant a reader of *this function* needs, and delete the rest with a pointer. For example `watch.rs:61-65` becomes nothing (the `mod x11` doc already says what it does; the alternatives-considered paragraph belongs where it already is), and `layout.rs:245-259` drops from 15 lines to about 5, keeping "what is linear in canvas pixels is `tan(sky_fov / 4)`, so the derived value self-limits" and the sentence that says 330 clears a seven-wide span. Estimated saving 200 to 240 lines with no loss of information anywhere in the repository.

**A2 (medium). Delete: comments that narrate an incident.** These are the clearest cases against the maintainer's own rule.

- `desktop.rs:216-219`: "...which is what the XFCE guest did: `last-image` held the right path under `monitor0` and the desktop went on showing xfdesktop's built-in default." The first six lines of that doc comment state the invariant; these three are the bug report.
- `desktop.rs:965-969` (test doc): "The XFCE guest set its wallpaper, reported success, and went on showing xfdesktop's own default." The test name `xfce_creates_the_property_its_own_monitor_is_named_after` already says the behavior.
- `layout.rs:744-747` and the test name `the_two_screen_span_the_old_cap_refused_is_inside_the_new_one` (759): "the shader's old 180" is a fact about a version that no longer exists.
- `desktop.rs:536-541`: "Four of these are verified live, one boot per desktop in the Linux test guest (phase 5 decision 9)... `docs/roadmap.md` says so." A phase decision number in source is exactly what the rule forbids.
- `display.rs:4-5`: "Two questions need the same answer and used to have two placeholders for it." History of the refactor.
- `layout.rs:1015-1016` (test comment): fine, keep; it states the invariant, not the history.

**A3 (low). Delete: comments that restate the next line.** These files are mostly clean here, but a few:

- `display.rs:323-327` (in `a_disconnected_output_is_not_a_connected_one`) repeats the `DESK` fixture doc at 278-284 nearly word for word.
- `desktop.rs:740-743` restates what the test name and the assertion already say.
- `watch.rs:495-499` (test doc) explains why the test exists, which the brief and the maintainer's rules both put out of bounds. One line ("only the start and the stop can be asserted without a real layout change") would do.

**A4. Comments that earn their place, for fairness.** Do not touch these:

- `layout.rs:9-26`, the two-axes explanation. Load-bearing: nothing in the code says which lens is anchored to which axis, and the whole module depends on it. Even though `docs/rendering.md` repeats it, this one belongs here too.
- `desktop.rs:50-55`: "a URI in the MATE key produces a desktop with no wallpaper and no error." External behavior, unguessable.
- `desktop.rs:332-333`: "`-n` creates a property and fails on one that exists, so the two cases cannot share one command line."
- `watch.rs:120-124`: "an empty event mask delivers the event to the client that created the destination window." Non-obvious X11 semantics that the stop path depends on.
- `watch.rs:314-321`: why `Mutex` rather than `OnceLock`, and why the lock is released before the callback. Two real invariants.
- `display.rs:61-67` and `106-110`: the token-vs-substring parsing rule and why a stray token must not be taken as geometry.
- `desktop.rs:186-188` and `199-207`: the xfdesktop property names and the single-workspace-mode default.

### B. Length and structure

**B1 (medium). `desktop.rs` at 1262 lines: split by desktop family.** Production code is 671 lines; the rest is tests. The natural seam is the two desktops with real machinery inside them.

| New file | Moves | Approx. lines |
|---|---|---|
| `desktop/xfce.rs` | 183-222 (consts, `xfce_live_property`), the `Kind::Xfce` arm of `commands` at 322-398 lifted to `pub(super) fn commands`, 423-446 (`xfce_monitor_of`, `xfce_properties`), tests 914-1029 and 1096-1161 | ~235 |
| `desktop/kde.rs` | 455-512 (`plasma_command`, `plasma_script`, `js_string`), the `Kind::Kde` doc at 74-84, tests 824-901 | ~145 |
| `desktop/mod.rs` | everything else: `DESKTOP_ENV`, `Invocation`, `Kind`, `Reach`, `Placement`, `Backend`, `file_uri`, `BACKENDS`, `detect`, `no_backend_message` | ~450 |

This also fixes B2 for free.

**B2 (medium). `Backend::commands` is 115 lines (`desktop.rs:290-404`).** The `Kind::Xfce` arm alone is 77 of them (322-398) and holds two nested closures (`write`, `plan`) plus the refusal check. Lifting each arm to a free function per family leaves `commands` as a five-line dispatch. Everything involved is a pure function over `&Placement` and `&str`, so the move is mechanical.

**B3. `layout.rs` at 1066 lines is long for a good reason.** Production code is 456 lines and reads as one coherent model: modes, rectangles, anchors, the two lens derivations, the render grouping, the crop. Splitting it would separate `canvas_framing` from `SKY_FOV_MAX` and `contain_camera_fov`, which are one idea. The length is 608 test lines; the fix is F1, not a split.

**B4. `watch.rs` at 590 lines is fine.** Only one platform's half compiles at a time (x11 ~163 lines, win32 ~232, fallback ~17). After A1 removes ~50 lines of duplicated prose it is comfortably under 550.

### C. Consistency

**C1 (medium). `display.rs` + `display/` is the only module in `sunlit-core` not using `mod.rs`.** `assets/`, `engine/`, `geometry/`, `renderer/` and `scene/` all use `mod.rs`. (`assets/stars/` is a data directory holding `hyg_v4_4_mag7.bin` and an attribution file, not a module, so `assets/stars.rs` is not a second instance.)

Recommendation: `git mv crates/sunlit-core/src/display.rs crates/sunlit-core/src/display/mod.rs`. No code change, no import change, five minutes, zero risk. The alternative, converting the other five modules to the sibling-file form, reaches the same consistency for far more churn and would be the newer-idiom choice; given the maintainer named file-structure inconsistency as the concern, the cheap direction is better.

**C2 (medium). Three different per-OS split patterns in one crate.**

1. `desktop.rs`: no `cfg` at all. Per-OS behavior is data in `BACKENDS`, and the platform boundary lives in the caller (`engine/wallpaper_sink.rs`). This is the best of the three where it applies, and it is what lets the table be tested on Windows.
2. `display.rs`, `memory.rs`, `wallpaper.rs`: inline `#[cfg]` on individual free functions. `display.rs` has three copies of `monitors()` and two of `outputs()`; `memory.rs` has four `#[cfg]` blocks; `wallpaper.rs` has fifteen scattered `#[cfg(windows)]` items plus `#[cfg(windows)] use ...` lines mixed into the header at 15-22, and one private `mod shell`.
3. `watch.rs`: one private `mod` per OS (`mod x11`, `mod win32`) plus a top-level `cfg(not(any(...)))` fallback, re-exported with `pub use`.

Recommendation: pattern 3 as the crate standard whenever a module has more than about two per-OS functions. It puts one platform's code in one contiguous block, gives each platform its own `use` list instead of `cfg`-annotated imports in the shared header, and reduces the public surface to a single `pub use` line per platform. `wallpaper.rs` benefits most and already has half the shape. `display.rs` has only two functions per platform, so leaving it inline is defensible, but see C3.

`watch.rs` is modelled on `sunlit-app/src/session_end.rs`, which uses `mod platform` / `mod unix`. The `x11` / `win32` naming in `watch.rs` is clearer than `platform` / `unix`; if the two are ever aligned, align on `watch.rs`'s.

**C3 (medium). Layering inversion: `display::monitors()` on Windows calls `wallpaper::enumerate_monitors()`** (`display.rs:217`, implemented at `wallpaper.rs:490`). The Linux enumeration lives in `display.rs` and the Windows one in `wallpaper.rs`, so the module that answers "what screens are there" depends on the module that answers "put a picture on them". `docs/platforms.md` describes the intended shape as two thin functions, enumerate and hand over, which this does not match. The obstacle is that `adopt_device_paths` needs `shell::DesktopWallpaperApi`, which lives in `wallpaper.rs`. Moving the enumeration to `display/win32.rs` means either moving `mod shell` next to it or exposing a `pub(crate)` handle to it. Worth doing but not free; flag rather than insist.

**C4 (low). `pub` items with no caller outside `sunlit-core/src`.**

- `desktop.rs:631 detect` has no caller at all outside its own file (`detect_current` and the tests). Make it private; the tests are in the same module.
- `desktop.rs:29 DESKTOP_ENV`, `654 no_backend_message`, `261 degradation`, `413 nothing_to_run` are used only by `engine/wallpaper_sink.rs`. `pub(crate)` would be accurate.
- `display.rs:68 parse_outputs` is called only by `outputs()` and its own tests (`e2e.rs:2526` mentions it in a comment only). `display.rs:198 primary_monitor_of` is used only by `wallpaper.rs:652`; `Output::overlaps` only by `config.rs:623`.
- `layout.rs:260 SKY_FOV_MIN` is used only at `layout.rs:354`. See D2, which gives it a real second caller.
- `layout.rs:133 Rect::right` and `138 Rect::bottom` are used only inside `layout.rs`.
- `watch.rs:275 CLASS_NAME` is `pub` and re-exported at line 32 but used only inside `mod win32`. Contrast `session_end::CLASS_NAME`, which `e2e.rs:1242` really does use; this one is dead public API. `watch.rs:287 Watcher::hwnd` is used only by the file's own `windows_tests`.

**C5 (low). Test module naming.** `watch.rs` is the only file in `sunlit-core/src` with test modules not named `tests`: `linux_tests` (490) and `windows_tests` (531). Both could be `mod tests` since their `cfg`s are mutually exclusive, matching the other nineteen files in the crate.

**C6 (low). `#[allow]` everywhere, `#[expect]` nowhere.** 110 `#[allow]` in `sunlit-core/src` against 2 `#[expect]` in the whole workspace. That is consistent, so no change needed, but `#[expect]` would catch the moment an allow stops being necessary. If the project ever switches, `layout.rs` is a good first candidate: it carries `#[allow(clippy::cast_precision_loss)]` three times (275, 317, 458) plus `cast_possible_truncation` at 742.

**C7 (low). Cast allows could be one helper.** All three `cast_precision_loss` allows in `layout.rs` exist for `u32 as f32` on pixel dimensions. A private `fn px(v: u32) -> f32` carrying the single allow would remove all three and name the conversion.

**C8 (low). Doc comments attached to only one `cfg` variant.** `display.rs:205-209` documents `monitors()` above the Linux variant; the Windows one at 216 and the fallback at 227 have none, so `cargo doc` on Windows shows an undocumented function. Same for `outputs()` (238 documented, 259 not). Put the doc on all variants, or restructure per C2 so there is one documented signature.

**C9 (low). Item ordering in `desktop.rs`.** `struct Backend` is declared at 171-181, then four XFCE constants and a free function intervene (183-222), then `impl Backend` at 224. `GNOME_BACKGROUND` (618) is declared after `BACKENDS` (542), which references it. `file_uri` (514) sits after both of its callers. `layout.rs` groups by topic and reads well; `desktop.rs` does not follow either that or a strict types-then-impls-then-functions order. The B1 split is the natural moment to fix it.

**C10 (low). Import style.** `desktop.rs`, `layout.rs` and `watch.rs` all use std / external / crate groups separated by blank lines. `display.rs` has no `use` at all and spells `crate::env_override`, `crate::wallpaper::enumerate_monitors` and `std::process::Command` inline. Not wrong, but it is the odd one out.

### D. Duplication

**D1 (high value). Documentation duplication.** See A1. This is the largest duplication in the area by an order of magnitude.

**D2 (medium, confirmed). The sky clamp bounds are written three times.**

- `layout.rs:260-261`: `SKY_FOV_MIN = 60.0`, `SKY_FOV_MAX = 330.0`.
- `scene/sun_occlusion.rs:238`: `(sky_fov_deg.clamp(60.0, 330.0) * PI / 720.0).tan()`, whose doc comment at 234-235 says "`display::layout::SKY_FOV_MAX` is the same number" rather than using it.
- `shaders/sphere.wgsl:136`: `tan(clamp(uniforms.sky_fov, 60.0, 330.0) * PI / 720.0)`.

The WGSL copy is unavoidable and is guarded by `the_shader_and_the_cpu_agree_on_the_three_shared_rules` in `tests/render_pipeline.rs:1419`. The `sun_occlusion.rs` copy is plain Rust in the same crate and should be `sky_fov_deg.clamp(SKY_FOV_MIN, SKY_FOV_MAX)`. One-line fix, and it gives `SKY_FOV_MIN` the caller that justifies its `pub`.

**D3 (low). Three near-copies of the `Monitor` test builder.**

- `layout.rs:464-482`: `fn monitor(id, x, y, width, height, primary)` and `fn side_by_side()`.
- `sunlit-app/src/displays.rs:439-456`: byte-identical `monitor` and `side_by_side`.
- `engine/wallpaper_sink.rs:519`: `fn monitor(id, x, width, height)`, a narrower variant.

Merging the two inside `sunlit-core` is easy (a `pub(crate)` builder in `display.rs` under `#[cfg(test)]`). The cross-crate copy in `displays.rs` cannot see a `cfg(test)` item in `sunlit-core` and is not worth a test-only feature flag; leave it.

**D4 (low). `js_string` escapes characters `file_uri` cannot produce** (`desktop.rs:509-512`). The comment at 505-508 admits this: "belt and braces over `file_uri`, which already percent-encodes everything outside an unreserved set and so can produce neither a quote nor a backslash." Either drop the `replace` calls and keep one line of comment, or keep them and cut the comment to one line. Either way, not both.

### E. Correctness and resilience

**E1 (medium, confirmed). A doc comment is attached to the wrong function.** `desktop.rs:448-454`:

```rust
/// A `file://` URI for a local path, which is what the gsettings keys want.
///
/// Percent-encoding is limited to the characters that would otherwise change
/// what the URI means. The path this is called with is one the app wrote itself,
/// under a directory named by the OS, so the general case is not the case here;
/// a space in a home directory is, and that is the one that has to work.
/// The one call that puts this publish on Plasma's screens.
fn plasma_command(placement: &Placement) -> Invocation {
```

`file_uri` at line 514 has no doc comment at all, and is the only undocumented free function in the file. Line 454 is `plasma_command`'s real one-liner, glued onto the end of a paragraph about a different function. Fix: move 448-453 above `fn file_uri` at 514 and leave 454 on `plasma_command`.

**E2 (medium, confirmed). `DisplayMode::index` can panic and the compiler will not catch the cause.** `layout.rs:66-71`:

```rust
pub fn index(self) -> usize {
    Self::ALL.iter().position(|mode| *mode == self).expect("every mode is in ALL")
}
```

This is the only `expect` in non-test code across the four files. It is unreachable today, but nothing links `ALL` to the variant list: adding a fourth variant and forgetting `ALL` compiles cleanly and then panics on the UI path (`displays.rs:648`, `mode_options`). A `match self { Self::OneScreen => 0, ... }` is exhaustiveness-checked and removes the `expect`. `ALL` still needs the same care, but the panic goes away.

**E3 (medium, confirmed). Neither `Watcher` implements `Drop`, and on Windows a leaked one wedges the process.** `watch.rs` has no `impl Drop` for either platform's `Watcher`.

- On Windows, `start` (335-368) refuses if `NOTIFY` is already `Some`, and only `stop()` calls `clear_notify()`. A `Watcher` dropped without `stop()` leaves the slot filled and the message-pump thread alive for the life of the process, and every later `start` returns `None` with "a display watcher is already running".
- On Linux, a dropped `Watcher` leaves the pump thread blocked in `wait_for_event` holding an `Arc<RustConnection>` forever.

`main.rs:810-812` does call `stop()` on the normal shutdown path, so this is not a live bug. It is a footgun for the next caller. Add `impl Drop` that does what `stop` does (moving the body into a `fn shutdown(&mut self)` that both call), or at minimum `#[must_use]` on `start`'s return.

**E4 (medium, unverified in practice). The Windows unit test can hang the whole `cargo unit` run.** `watch.rs:562-565`:

```rust
unsafe { SendMessageW(hwnd as _, WM_DISPLAYCHANGE, 0, 0); }
```

`SendMessageW` across threads blocks with no timeout until the target thread's pump dispatches. The test's *stop* path was written defensively (a spawned thread plus `recv_timeout(5s)` at 579-588, with a comment saying exactly why), but the two `SendMessageW` calls were not. If the watcher thread panics inside `notify()` or never reaches `GetMessageW`, the test blocks forever and the harness has no timeout. `SendMessageTimeoutW` with a few seconds and an assertion on the result would close it. I did not run the test, so this is a reading of the API contract rather than an observed hang.

**E5 (low, confirmed). A `// SAFETY:` comment is missing at one of the ten `#[allow(unsafe_code)]` sites.** Nine of ten carry one. The exception is `watch.rs:568-572`:

```rust
// A message the watcher does not answer must not become a hint.
#[allow(unsafe_code)]
unsafe {
    SendMessageW(hwnd as _, 0x001A, 0, 0);
}
```

The comment present describes the assertion, not the safety argument. The site 8 lines above (560-565) has a proper one; copy its reasoning here. CLAUDE.md states the rule as "FFI call sites carry a scoped `#[allow(unsafe_code)]` and a `// SAFETY:` comment", so this is a rule violation, not a preference.

Related and lower: `watch.rs:456` puts `#[allow(unsafe_code)]` on `unsafe extern "system" fn wndproc` itself, and the item has no `// SAFETY:` stating the contract a caller must uphold (the two `unsafe` blocks *inside* it, at 475 and 484, do have theirs). Worth one line saying that Windows is the only caller and it passes what the procedure signature promises.

Also low: `zeroed<T>()` at `watch.rs:377-385` is a generic safe function wrapping `std::mem::zeroed()`. Its SAFETY comment argues the case for `WNDCLASSW` and `MSG`, which are the only two instantiations, but the signature admits any `T`, including types for which all-zero is invalid. Making it `unsafe fn` or restricting it to the two types would make the signature honest. The same pattern appears at `wallpaper.rs:539-544` written inline with its own SAFETY comment, which is arguably the safer shape.

**E6 (verified sound). The X11 stop path.** `pump` (`watch.rs:210-229`) blocks in `wait_for_event`; `stop` sends a `ClientMessage` to the watcher's own 1x1 `InputOnly` window with `EventMask::NO_EVENT`. The X protocol delivers an event with an empty mask to the client that created the destination window, which is this connection, so the blocked reader receives it, and `pump` returns on `message.window == window && message.type_ == stop`. The `InputOnly` window is created with depth 0 and `COPY_FROM_PARENT` as its visual, which is what the protocol requires. If `wake()` fails, `stop` logs and returns without joining, which is documented at 108-111 and is the right call: joining a thread that will never be woken would block until process exit. `RustConnection` used from two threads at once is x11rb's documented case and the struct doc at 87-92 says so; I did not verify it against x11rb's source.

**E7 (verified sound, worth saying). The hint path does not storm.** The notify closure at `main.rs:663` sends on an unbounded `std::sync::mpsc::Sender`, so it never blocks the wndproc or the X pump. `engine/mod.rs:807-815` turns each hint into a trailing deadline (`DISPLAY_SETTLE`), so a burst of RandR CRTC and output events, or Windows sending one `WM_DISPLAYCHANGE` per applied step, costs one monitor query at the end. This is the right design and needs no change.

**E8 (verified sound). `crop` cannot read out of bounds** (`layout.rs:433-455`). The length check at 435 pins `pixels.len() == canvas_width * 4 * canvas_height`, and the rectangle check at 438-445 rejects anything reaching outside, so the maximum `start + row` is exactly `pixels.len()`. `stride` uses `checked_mul`; `row` does not, but `rect.width <= canvas_width` is enforced first, so `row <= stride` and the multiplication cannot wrap even on a 32-bit target.

**E9 (verified sound). `parse_outputs` offset parsing.** `split_offsets` (`display.rs:123-133`) skips the first character before looking for the separator, which is what lets a negative x through: `1920x1080+-1920+0` splits into `-1920` and `+0` correctly. The `DESK` fixture exercises `+0-200`. Byte indices from `char_indices` are used to slice, which is safe here because xrandr geometry is ASCII.

### F. Unit tests

**F1 (medium). Nine of 33 tests in `layout.rs` are subsets of a neighbor.** None are slow: every one is pure arithmetic on fabricated data, the largest allocation in the module is a 128-byte fake canvas, and the widest fixture builds 12 monitors. The cost is 608 lines to read, not runtime. Concrete merges:

- `bounds_of` has seven tests (494, 507, 524, 541, 560, 577, 596) that are all "these monitors, that `Rect`". Five of them (494, 507, 524, 541, 596) are the identical assertion shape with different data and belong in one table-driven test. `a_gap_between_two_monitors_is_inside_the_canvas` (560) and `a_monitor_with_no_pixels_does_not_pull_the_canvas_to_its_origin` (577) each add a real invariant and stay. Net minus 4, about 90 lines.
- `equal_heights_leave_the_earth_lens_untouched` (776) is a strict subset of `the_anchors_framing_survives_being_put_on_a_canvas` (689): same fixture, and since `canvas.height == anchor.height` the scale identity in 689 *is* the fov equality in 776. Remove.
- `the_two_screen_span_the_old_cap_refused_is_inside_the_new_one` (759): the `!sky_clamped` half is already covered by the `screens == 2` iteration of `a_span_of_ordinary_screens_keeps_the_sky_scale_too` (722). The surviving assertion (`sky_fov > 180`) belongs in that loop. Remove, and with it the "old cap" name and comment from A2.
- `two_identical_monitors_cost_one_render_and_two_files` (840) is a subset of `different_resolutions_are_one_render_each_at_their_own_size` (860), which groups screens 0 and 2 at 1920x1080. Remove.
- `mirrored_monitors_share_one_render` (847): its `render_groups` half duplicates 840/860 exactly (grouping is by size, and position does not enter it), and its unique assertion is about `bounds_of`. Fold that one line into the bounds table. Remove.
- `a_crop_of_the_whole_canvas_is_the_canvas` (973) folds into `the_crops_of_a_layout_tile_the_canvas` (921). Remove.

33 to 24, roughly 150 test lines.

**F2 (medium). One test pins a default, which the project's own conventions forbid.** `layout.rs:1059`:

```rust
assert_eq!(DisplayMode::default(), DisplayMode::EveryScreen);
```

CLAUDE.md: "changing a preset or a default must not break a test." Delete that line. The rest of `the_modes_round_trip_through_their_positions_and_their_names` is fine, including `toml::from_str::<Wrapper>("mode = \"every-other-screen\"") == DisplayMode::default()`, which asserts the *rule* rather than the value.

Two lesser cases, both defensible because the value is an external contract rather than a project default: `desktop.rs:951` asserts the literal `"5"` where `XFCE_ZOOMED` exists and is used by name eight lines later at 1009, so use the constant for consistency; `desktop.rs:768-769` asserts `"picture-options"` and `"zoom"`, which are gsettings' names and must be pinned.

**F3 (low). `desktop.rs`: 26 tests, about five mergeable.**

- `the_dark_key_is_set_too_because_gnome_chooses_between_them` (740): its GNOME half is a strict subset of `gnome_sets_both_keys_as_uris` (721), which already asserts both keys. Its unique value is the Cinnamon negative; fold that into 721.
- `kde_reaches_every_monitor_and_lxqt_does_not` (887): the two reach assertions are repeated verbatim at `every_row_says_how_far_it_reaches` (1198, 1199), and the KDE degradation loop is repeated in substance at 1205-1216. Fold.
- `an_xfce_session_with_a_style_but_no_image_property_is_still_a_refusal` (1032) and `an_xfce_session_with_no_backdrop_property_is_a_refusal_not_a_success` (1045) both assert the same emptiness for three listings. One test with three fixtures.
- `xfce_asks_which_backdrops_exist_before_setting_any` (915) overlaps `every_backend_names_the_program_its_commands_run` (1078), which also checks `discovery().program`. Minor; keep both if you want the negative loop over the six non-XFCE desktops in one place.

26 to about 21.

**F4 (low). `display.rs`: 12 tests, four mergeable.** `the_guests_single_output_is_read_with_its_mode` (297) and `the_free_text_after_the_geometry_is_not_mistaken_for_more_of_it` (385) both parse `GUEST` and assert one output. `a_disconnected_output_is_not_a_connected_one` (317), `an_output_with_no_mode_assigned_is_not_somewhere_to_put_a_window` (332) and `negative_offsets_keep_their_sign` (339) are three one-assertion tests over the same `DESK` fixture; one "the `DESK` fixture parses to exactly these two outputs with these rectangles" test covers all three and is more legible. `the_monitor_rectangle_is_the_outputs_own_geometry` (420) is one assertion that belongs inside `an_output_becomes_a_monitor_with_its_connector_as_both_names` (400). 12 to about 8.

**F5 (low). `watch.rs`'s two tests are the only ones in the area that touch the OS.** They create a real X connection or a real Win32 window and thread inside `cargo unit`. That is a defensible call (they cover the connect, the extension check, the `select_input` and the wake, which is every call the platform half makes) and both skip cleanly when the environment cannot support them. Two things follow: `NOTIFY` is a process global, so a second test calling `watch::start` would be flaky against this one under cargo's parallel harness, and E4's unbounded `SendMessageW` is the one thing here that could stall a run.

**F6. Overall test count.** 73 across the four files; I would land at about 55 with no coverage I can identify lost, and roughly 250 fewer test lines. None of that is a runtime saving.

### G. Best practices with a real effect

**G1 (low). A wasted allocation per publish.** `desktop.rs:291`: `let single = placement.single.to_string_lossy().into_owned();` is computed before the match but consumed only by the `Lxqt` arm and the non-URI `Gsettings` arm. KDE and XFCE allocate it and drop it. Move it into the two arms that use it.

**G2 (low). `detect` allocates per token.** `desktop.rs:634`: `.map(|token| token.trim().to_ascii_lowercase())` allocates a `String` for every colon-separated token, then compares against a `&'static str` table. `token.trim()` plus `names.iter().any(|n| n.eq_ignore_ascii_case(token))` would allocate nothing. Called once per publish at most, so this is clarity rather than speed.

**G3 (low). `render_groups` scans to test one index.** `layout.rs:389-390`: `(0..monitors.len()).filter(|index| *index == anchor)` is an O(n) walk to decide one membership. `(anchor < monitors.len()).then_some(anchor).into_iter()` says the same thing directly. Cosmetic at n <= 8.

**G4 (low). `DisplayMode::deserialize` allocates a `String`** (`layout.rs:110`) to compare against three static names. `Cow<'de, str>` or `&str` would avoid it. Config load only, so no measurable effect; mentioned because the file otherwise avoids this.

## 4. Recommended refactors, ordered by value over effort

| # | Change | Effort | Risk | Why it is first |
|---|---|---|---|---|
| 1 | `git mv display.rs display/mod.rs` | 5 min | none | Removes the one module-layout outlier in the crate. No code change. |
| 2 | Fix the misplaced doc comment (E1), delete `desktop.rs:951`'s literal in favor of `XFCE_ZOOMED`, add the missing SAFETY at `watch.rs:568` (E5), delete the default-pinning assert at `layout.rs:1059` (F2) | 20 min | none | Four small confirmed defects, each a one-line or two-line fix. |
| 3 | Point `sun_occlusion.rs:238` at `SKY_FOV_MIN` / `SKY_FOV_MAX` (D2) | 5 min | very low | Removes a silent-drift hazard between two Rust files and gives `SKY_FOV_MIN` its only external caller. |
| 4 | Cut the ~220 duplicated comment lines (A1) and the six incident narratives (A2) | 2-3 h | none | The largest single reduction available, and nothing is lost: `docs/platforms.md` and `docs/rendering.md` already hold every one of these paragraphs in more detail. |
| 5 | Replace `DisplayMode::index`'s `expect` with an exhaustive `match` (E2) | 10 min | very low | Removes the only non-test `expect` in the area and makes the compiler enforce what the comment currently promises. |
| 6 | Merge the redundant tests: layout 33 to 24, desktop 26 to 21, display 12 to 8 (F1, F3, F4) | 3-4 h | low | About 250 fewer test lines. Do it after 4 so the merged tests do not carry the deleted comments forward. |
| 7 | `impl Drop` for both `Watcher`s, or `#[must_use]` on `start` (E3) | 30 min | low | Stops a leaked watcher from permanently wedging the Windows global. |
| 8 | `SendMessageTimeoutW` in the Windows test (E4) | 20 min | low | Removes the one way this area can stall `cargo unit`. |
| 9 | Split `desktop.rs` into `desktop/{mod,xfce,kde}.rs`, lifting the `commands` arms to free functions (B1, B2) | 2-3 h | low | 1262 lines to three files of 450, 235 and 145; the 115-line `commands` becomes a five-line dispatch. Everything moved is a pure function with its tests. Do it after 4 and 6 so less code moves. |
| 10 | Tighten visibility per C4: `detect` private, the wallpaper-sink-only items `pub(crate)`, drop the dead `watch::CLASS_NAME` export, `Watcher::hwnd` to `pub(crate)` or `cfg(test)` | 30 min | low | Shrinks the public surface to what is actually consumed. Do it after 9, since the split moves several of these. |
| 11 | Adopt `watch.rs`'s per-OS submodule pattern in `wallpaper.rs` (C2) | 4-6 h | medium | Outside this area's files but it is where the pattern pays off most: 15 scattered `cfg(windows)` items and `cfg`-annotated imports in the shared header. Only worth doing if `wallpaper.rs` is being touched anyway. |
| 12 | Move the Windows monitor enumeration out of `wallpaper.rs` into the display module (C3) | 3-5 h | medium | Fixes a real layering inversion, but `adopt_device_paths` needs `mod shell`, so it drags COM plumbing with it. Lowest value per hour of anything on this list. |

Items 1 through 8 total roughly one working day, carry near-zero risk, and address every confirmed defect plus the bulk of the volume problem.
