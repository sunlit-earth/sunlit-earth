<!-- Reviewer notes for docs/reviews/2026-09-04-code-quality-review.md. Line numbers refer to commit 3046327. "The brief" is the shared review instruction; "the maintainer's rules" are the project's comment conventions. Runtime claims here are reasoned from the code; the measured figures are in test-timing.md. -->

# Review: `crates/sunlit-core/src/assets/`

## 1. Summary

The memory discipline holds. I verified every rule the retrospective imposed: no decoded pixel buffer is parked anywhere but the latest-value mailbox, which is capped at one frame per slot by construction, and both background producers (the cloud worker and the texture loader thread) name their consumer. No function in the area exceeds 80 lines and no `unwrap` on external data survives.

The real problems are commentary and test bulk. The code half of `cloud_fetcher.rs` is 28% comment lines and `mailbox.rs` is 40%, and large parts of those comments are a verbatim second copy of `docs/architecture.md` lines 115, 125 and 205. The test module of `cloud_fetcher.rs` (854 lines, 42 tests) carries about 13 "why this test exists" narratives that the maintainer's own rules forbid, and roughly 11 of its tests are merges of other tests.

Two duplications are worth fixing: `decode_cloud_jpeg` reimplements `texture_loader::orient` with `image::fliph` (costing an extra full-size allocation at 8K), and `texture_cache::unfinished` / `save_png` are byte-for-byte copies of `wallpaper::unfinished` / `encode_png`.

Nine `pub` items are reachable only inside the crate, one (`no_notify`) only from tests. No test in the area feeds corrupt or truncated JXL to the loader, which is the one decoder path with `no_limits()` set.

## 2. Metrics

| File | Total | Code half | Test module | Comment lines (code / test) | Blocks >3 lines: keep / move / delete | Fns >80 lines | `pub` used only in-crate |
|---|---|---|---|---|---|---|---|
| `cloud_fetcher.rs` | 1391 | 1-536 | 537-1391 (854) | 151 / 96 | 8 / 9 / 4 | 0 | `NotifyFn`, `no_notify`, `RetryBackoff`, `cloud_variant`, `PollOutcome`, `CloudUpdater` |
| `texture_cache.rs` | 702 | 1-348 | 349-702 (354) | 97 / 30 | 9 / 2 / 1 | 0 | none |
| `texture_loader.rs` | 371 | 1-153 | 154-371 (218) | 39 / 19 | 6 / 0 / 0 | 0 | `flip_horizontal` (`pub(crate)`, module-local) |
| `mailbox.rs` | 275 | 1-116 | 117-275 (159) | 47 / 14 | 2 / 3 / 1 | 0 | none |
| `stars.rs` | 231 | 1-122 | 123-231 (109) | 9 / 0 | 0 / 0 / 0 | 0 | `parse_catalog`, `StarCatalogError`, `StarCatalog::len`, `StarCatalog::is_empty` |
| `cloud_source.rs` | 153 | all | none | 35 / 0 | 4 / 1 / 0 | 0 | none |
| `mod.rs` | 9 | all | none | 2 / 0 | 0 / 0 / 0 | 0 | n/a |

Test counts: `cloud_fetcher` 42, `texture_cache` 19, `texture_loader` 13 (10 plain plus 3 proptest), `stars` 10, `mailbox` 9, `cloud_source` 0. Total 93.

The comment ratio in the code half is the number that surprised me: `mailbox.rs` has 47 comment lines against 69 lines of actual code, and `cloud_fetcher.rs` 151 against 385.

## 3. Findings

### A. Commentary

**A1. The source carries a second copy of `docs/architecture.md`. (medium)**

Four long comment blocks restate, sometimes almost sentence for sentence, text that already exists in `docs/architecture.md`:

- `mailbox.rs:60-74` (the 15-line `post` doc, generation ordering) against `docs/architecture.md:125`.
- `cloud_fetcher.rs:526-530` (why the cloud post carries no generation) against `docs/architecture.md:125`, which says the same thing in the same words ("a fetch of the old variant that lands after a switch is a cloud layer at the previous width for one poll").
- `texture_cache.rs:145-148` and `texture_cache.rs:1-13` (stamped before the decode, why PNG) against `docs/architecture.md:115`.
- `mailbox.rs:24-31` ("This replaces the unbounded channel that used to hold decoded pixel buffers ... which is exactly what hiding the window to the tray used to do to it") against `docs/architecture.md:205`, which already labels it the Phase 0 fix.

Recommendation: cut each of these to the one sentence a reader of the code needs and let `docs/architecture.md` carry the rest. For `mailbox.rs:24-31` the surviving sentence is "Latest-value: only the newest frame per slot is useful, so `post` overwrites rather than queues." The bug history goes. For `mailbox.rs:60-74`, keep the out-of-range paragraph (60-66, a real invariant about the consumer's slot array) and cut the generation paragraph (67-74) to two lines.

**A2. Design-history narratives with no code-reading value. (medium)**

- `cloud_fetcher.rs:304-316`, 13 lines on `cache_stem`. The first paragraph is the rule; the second is a counterfactual bug story ("Without that, a run with the override would leave an image of any size in `clouds_cache_4096x2048.jpg`, and the next run without the override would put that image on screen"). Classification: move. Three lines survive.
- `cloud_fetcher.rs:371-389`, 19 lines on `set_resolution`, the longest doc in the area. Three paragraphs of rationale for behavior the function's five statements already show. Classification: move. Keep "Retargets the source, moves to that variant's cache entry, and posts whatever image was left there so the switch is visible immediately" plus the last line about the no-op.
- `cloud_fetcher.rs:47-50`, on `RetryBackoff`: "Split out from the worker loop so the schedule can be tested without actually sleeping through it." That is a note about the tests, not the type. Classification: delete.
- `cloud_fetcher.rs:500-506`, 7 lines, is the third statement in this file of the same rule ("a sidecar is a claim that a decodable image is on disk"), after `208-214` and `468-475`. Classification: delete the third one; the code at 507-511 is two branches.
- `cloud_fetcher.rs:334-340`: "In production that is the URL the caller already built from the same resolution; the point is that this function, not the caller, is what decides what the entry holds." Argues with a hypothetical reviewer. Classification: move or delete.
- `texture_loader.rs:240-241`: "The flip replaced `DynamicImage::fliph`, which is what every golden reference was generated with, so it has to mean exactly the same thing." History of a refactor. See D1: the test itself should go with the duplication it guards.

**A3. Comments in the test module explaining why a test exists. (medium)**

The maintainer's rule is explicit that this belongs in commit messages. Instances over 3 lines: `cloud_fetcher.rs:1177-1186` (10 lines), `1225-1237` (13 lines), `1281-1288` (8 lines), `texture_cache.rs:641-644`, `mailbox.rs:200-203`. Shorter ones in the same category: `cloud_fetcher.rs:625-626, 638, 647-649, 681-682, 697-699, 1106-1108, 1147-1148, 1162-1163, 1334-1335, 1358-1359`; `mailbox.rs:226-227`; `texture_cache.rs:386-387, 498-500, 538-539, 555-557, 576-577, 625-626`.

Not all are equal. `texture_cache.rs:555-557` ("the cached file is replaced with different pixels of the same size, and the sidecar is left alone") explains a setup trick that is opaque from the code, and earns its place. `cloud_fetcher.rs:1234-1237` ("The write is made to fail by putting a directory where the JPEG goes, which fails on both platforms") likewise. The other 20 or so restate the test name.

Recommendation: keep only the comments that explain a non-obvious setup mechanism. That removes roughly 90 of the 96 comment lines in `cloud_fetcher.rs`'s test module.

**A4. Restating the next line. (low)**

- `texture_loader.rs:111` "Clamp neighbor coordinates to stay within source bounds" directly above `let sx1 = (sx + 1).min(sw - 1);`.
- `texture_loader.rs:143` "Walk up from the executable's directory to find a `textures/` folder" above the loop that does exactly that.
- `texture_loader.rs:358` "Truncate or skip if the random vec doesn't match the expected size" above `prop_assume!`.
- `cloud_fetcher.rs:1004-1006` "Two failed attempts, each backing off further, then a success that resets the schedule" above six lines that read the same.

**A5. Comments that earn their place. (no action)**

Being fair: `texture_loader.rs:54-60` (the flip plus quarter-shift convention) is the model for the area, a coordinate convention no reader could derive. `texture_loader.rs:86` ("3/4 width in bytes") is a unit note. `texture_cache.rs:26-31` (size and mtime rather than a content hash, because hashing costs what the cache saves) is a real why. `texture_cache.rs:307-314` (`unfinished`, why the pid and counter) is a concurrency invariant. `cloud_fetcher.rs:230-234` and `cloud_source.rs:45-47` are short and load-bearing. `stars.rs` has 9 comment lines in 231 and is the cleanest file in the area.

**A6. Out of area but adjacent. (low)** `crates/sunlit-core/src/lib.rs:4` says "(from Step 4 on)", a reference to an implementation plan step, which is the sort of external-process note the rules forbid.

### B. Length and structure

**B1. `cloud_fetcher.rs` at 1391 lines, 854 of them tests. (medium)**

The code half is 536 lines and holds three unrelated concerns. Concrete seam, as a `cloud_fetcher/` directory:

- `cloud_fetcher/cache.rs`: `CacheMeta` (95-99), `load_cache_meta` (176-179), `save_cache_meta` (181-206), `save_cache_image` (215-228), `discard_cache_meta` (235-243), `cache_stem` (317-323), `cache_paths` (326-331), plus tests 541-616 and 683-717. About 120 code lines, 100 test lines.
- `cloud_fetcher/variant.rs`: `CLOUD_URL_TEMPLATE` (39), the three env consts (88-92), `cloud_variant` (110-119), `variant_cloud_url` (122-125), `resolve_cloud_url` (128-130), `cloud_url` (133-135), `resolve_poll_interval` (141-156), `poll_interval` (159-161), `resolve_cache_dir` (164-169), `cache_dir` (172-174), plus tests 618-756. About 70 code lines, 140 test lines. This is the module the app actually imports (`main.rs:319, 334, 335`), so the split also sharpens the public surface.
- `cloud_fetcher/mod.rs`: `NotifyFn`, `no_notify`, `RetryBackoff`, `POLL_INTERVAL` and the retry consts, `decode_cloud_jpeg`, `PollOutcome`, `CloudUpdater`, plus the updater tests.

With the test trimming from F1 that leaves three files of roughly 220, 210 and 550 lines. Worth doing, but the trimming alone takes the file to about 1000 and is the cheaper half.

**B2. Item order inside `cloud_fetcher.rs` is scrambled. (low)** Consts appear in two places (39-45 and 87-92) with a type and its impl between them; `cache_stem` and `cache_paths` (304-331) sit between `struct CloudUpdater` (281-302) and `impl CloudUpdater` (333). Every other file in the area follows module doc, imports, consts, types, impls, free functions, tests. Recommendation: gather the consts, move the two free functions below the impl. The `cloud_fetcher/` split makes this moot.

**B3. No function in the area exceeds 80 lines.** Longest: `write_cache` at `texture_cache.rs:199-271` (73), `poll_once` at `cloud_fetcher.rs:450-520` (71). Both are linear sequences of failure branches and read fine. There is one `#[allow(clippy::too_many_arguments)]` in the area's caller (`engine/mod.rs:1269`), not in the area itself.

**B4. `texture_cache.rs` at 702 lines does not need splitting.** The code half is 348 lines on one subject and every helper is used by `load_at_resolution`. The 354-line test module is where the length is, and F2 covers it.

### C. Consistency

**C1. Nine `pub` items are only reachable inside the crate. (medium)**

Verified by grepping `crates/sunlit-app/src` and `crates/sunlit-core/tests`:

- `cloud_fetcher.rs:29` `NotifyFn`, `52` `RetryBackoff`, `110` `cloud_variant`, `270` `PollOutcome`, `281` `CloudUpdater`: used only from `crates/sunlit-core/src/engine/mod.rs:21,1291,1299`. Should be `pub(crate)`.
- `cloud_fetcher.rs:32` `no_notify`: used only at `cloud_fetcher.rs:914, 1095, 1116`, all inside its own `mod tests`. This is dead public API. Either delete it and inline `Arc::new(|| {})` in the tests, or move it behind `#[cfg(test)]`.
- `stars.rs:84` `parse_catalog` and `stars.rs:61` `StarCatalogError`: used only by `embedded_catalog` (119) and `stars.rs` tests. `StarCatalogError` has to stay `pub` if `parse_catalog` does; both could be `pub(crate)`.
- `stars.rs:25` `StarCatalog::is_empty`: never called anywhere in the workspace. Present, I assume, for `clippy::len_without_is_empty`, though that lint keys on a `len` returning `usize` and this one returns `u32`, so it may not even fire (unverified). `StarCatalog::len` (20) is called only from `stars.rs:182`, its own test.
- `texture_loader.rs:67` `flip_horizontal` is `pub(crate)` but reachable only from `orient` (62) and its own tests. Can be private.

Truly public and used across the crate boundary: `cloud_url`, `poll_interval`, `cache_dir` (`sunlit-app/src/main.rs:319,334,335`); `texture_loader::{load, register_jxl_hook, resolve_textures_dir, DecodedImage}`; `mailbox::{TextureMailbox, DecodedTextureMessage}`; `cloud_source::{CloudSource, CloudImage, HttpCloudSource}`; `stars::{embedded_catalog, RECORD_SIZE, StarCatalog::{visible_count, instance_bytes}}`; `texture_cache::load_at_resolution` (`tests/engine.rs:2074`).

**C2. Two different poison policies inside one area. (low)**

`mailbox.rs:56, 77, 102` use `.expect("texture mailbox lock poisoned")`; `cloud_source.rs:71, 148` use `.unwrap_or_else(std::sync::PoisonError::into_inner)` with a comment explaining that a poisoned lock is someone else's panic, not a reason to stop. Both are in the assets module and both guard a `Mutex` whose critical section cannot realistically panic. Recommendation: adopt the `cloud_source` policy in `mailbox` for consistency, and because the mailbox's consumer is the render path, where a poison-induced panic is a hard stop rather than a degraded frame.

**C3. `#[expect]` versus `#[allow]`. (low)** `stars.rs:35` is the only `#[expect]` in `sunlit-core` (1 against 110 `#[allow]`), and it is the only one carrying a `reason =`. It is the better form (it warns when the allow stops being needed) but as a single instance it is an inconsistency. Recommendation: either convert the cast allows in the area to `#[expect(..., reason = "...")]` or accept `#[allow]` everywhere and say so once.

**C4. The cast allows could be avoided rather than allowed. (low)** `texture_loader.rs:99` allows `cast_possible_truncation` for the whole of `downsample_2x`, but the only cast is `((tl + tr + bl + br + 2) / 4) as u8` at line 119, where the four terms are each at most 255, so the result is at most 255 and cannot truncate. `texture_loader.rs:243, 259` and `texture_cache.rs:358` are test-only casts of the form `(i % 251) as u8`, which `u8::try_from(...).expect(...)` or generating `u8` directly would remove. Recommendation: narrow the function-level allow at line 99 to the statement, or restructure so it is unnecessary.

**C5. Error style is consistent and appropriate.** Everything fallible in the area returns `Result<_, String>` except `stars`, which has a real error enum with `Display` and `std::error::Error`. That split is defensible: the string errors go straight into a `tracing::warn!` or a mailbox message, and only the star catalog has variants a caller might branch on. `expect` in non-test code appears twice: `stars.rs:120` (a compile-time embedded asset, covered by four tests in the same file) and `mailbox.rs:56,77,102` (C2). Neither touches external data.

**C6. Import grouping is uniform** across all six files: module doc, `std`, blank, third-party, blank, `crate::`, blank, `super::`. No action.

**C7. A doc comment that is factually wrong. (low)** `texture_loader.rs:22-23` says "The format is auto-detected by the `image` crate". `ImageReader::open` derives the format from the file extension, not the content (`image-0.25.9/src/io/image_reader_type.rs:351-356`); `with_guessed_format` is what sniffs content and is not called. A correctly formatted JXL named `.dat` fails. Recommendation: say "from the file extension".

### D. Duplication

**D1. `decode_cloud_jpeg` reimplements `texture_loader::orient`. (medium, with a real memory cost)**

`cloud_fetcher.rs:251-259` does `image::load_from_memory(bytes)?.fliph().into_rgba8()` then `texture_loader::shift_horizontal(...)`. `texture_loader::orient` (61-64) is `flip_horizontal` then `shift_horizontal`. These are the same transform written two ways, and `texture_loader.rs:242-256` (`flip_matches_the_image_crates_own`) exists specifically to keep the two spellings in agreement. If `orient` ever changes, the cloud path silently diverges and only that sync test would notice.

There is also a measurable cost. `DynamicImage::fliph` takes `&self` and returns a new image, so at the 8192 variant the sequence allocates the decoded RGB8 (8192 x 4096 x 3, about 100 MB), a flipped copy of it (another 100 MB), and then the RGBA8 conversion (134 MB), peaking near 334 MB. Decoding to RGBA8 first and flipping in place peaks near 234 MB and frees the 100 MB immediately. In a project that keeps a memory report command and a leak retrospective, 100 MB at the exact moment the app is heaviest is worth having.

Recommendation: replace lines 251-259 with `image::load_from_memory(bytes)?.into_rgba8()` into a `DecodedImage`, then `texture_loader::orient(&mut img)`. Delete `flip_matches_the_image_crates_own` with it, since the two spellings no longer both exist. Confidence: confirmed by reading both call sites and the `fliph` signature.

**D2. `texture_cache::unfinished` and `save_png` are copies of `wallpaper::unfinished` and `encode_png`. (low)**

`texture_cache.rs:315-321` and `wallpaper.rs:411-417` are identical apart from the suffix (`~` versus `.tmp`) and each keeps its own `static NEXT` counter; `wallpaper.rs:408-410` even says it is "following `crate::assets::texture_cache`". `texture_cache.rs:330-347` and `wallpaper.rs:420-431` differ only in the PNG filter type (`Adaptive` versus `Sub`) and the error message wording. Recommendation: one `fn unfinished(path: &Path, suffix: &str)` and one `fn write_png(path, pixels, w, h, filter)` in a small shared module. Low value on its own, but it is the kind of thing a first release freezes in place.

**D3. `save_cache_meta` duplicates `config::save_config_to`. (low)** `cloud_fetcher.rs:181-206` and `config.rs:517-547` are the same routine: serialize TOML, create the parent, write `path.with_extension("toml~")`, rename, `warn!` at each of the four failure points. Only the log messages differ. A shared `fn write_toml_atomically<T: Serialize>(value: &T, path: &Path) -> Result<(), String>` would remove about 25 lines and one class of divergence. Note that `texture_cache::write_cache` deliberately does not use this shape (it needs two files landing in a defined order), so the helper covers two of the three sites, not three.

### E. Correctness and resilience

**E1. The memory rules, verified. (no defect found)**

The task asked for line-by-line verification, so:

- *Decoded pixel buffers are never parked.* `CloudUpdater` (`cloud_fetcher.rs:281-302`) holds a source, a mailbox handle, a notify callback, a slot index and paths. No pixels. `poll_once` decodes at 476, posts at 518, and the `DecodedImage` is moved into the mailbox in the same statement. `texture_cache::load_at_resolution` returns its `DecodedImage` by value and stores nothing. `TextureMailbox` is the one place a decoded buffer rests, and by construction it holds at most one per slot (`mailbox.rs:34, 97`). Confirmed.
- *Every cross-thread queue is bounded or latest-value with the reasoning at the declaration.* The only queue in the area is `TextureMailbox`, and the reasoning is at `mailbox.rs:24-31`, at the declaration, as required. It is a `Vec<Option<_>>` of fixed length with an out-of-range guard at 78-85 that drops rather than grows. Confirmed.
- *Every background producer names its consumer and the condition under which it runs.* The area has no thread of its own. `cloud_fetcher.rs:3-5` names the engine as the driver; the thread lives at `engine/mod.rs:1265-1268`, whose doc names the consumer ("parks decoded frames in the mailbox and pokes the engine, which uploads them on its own schedule") and whose condition is `while rx.recv().is_ok()` at 1302. The texture loader thread is `renderer/textures.rs:203-227`, posting to the same mailbox and drained by `process_decoded_textures` (30-31). Confirmed for both.

One nit inside this: `mailbox.rs:97` (`slots[index] = Some(msg)`) drops the previous message, and therefore frees a buffer of up to 134 MB, while holding the mutex the render thread's `take_all` needs. The window is one `free`, so this is a note rather than a finding. If it ever matters, `std::mem::replace` and drop the old value after the guard.

**E2. `texture_loader::decode` disables the decoder's allocation cap. (low, but the one place external data meets an unbounded allocation)**

`texture_loader.rs:40` calls `reader.no_limits()`. `image`'s default is `max_alloc: Some(512 MB)` (`image-0.25.9/src/io/limits.rs:49-57`), and the 8K JXL sources need more than that during decode, so the call is understandable. The consequence is that a corrupt or hostile file in the textures directory (which `--textures-dir` and `SUNLIT_EARTH_TEXTURES` let a user point anywhere) can drive an allocation the process cannot refuse; a Rust allocation failure aborts rather than returning `Err`. Note the contrast with the cloud path, which uses `image::load_from_memory` and keeps the 512 MB default even though its bytes come off the network. Recommendation: replace `no_limits()` with an explicit generous cap (`Limits { max_alloc: Some(2 << 30), .. }`), so a malformed header is an error rather than an abort. Confidence: the `no_limits` call and the default are confirmed by reading; whether `jxl-oxide` checks declared dimensions before allocating is unverified.

**E3. Nothing in the area panics on corrupt or truncated input, but nothing tests it either. (medium, as a test gap)**

`texture_loader::decode` maps both `ImageReader::open` and `decode` failures to `Err(String)` (38-43), so a truncated or corrupt JXL surfaces as an error string, and `texture_cache::load_at_resolution` propagates it. `decode_cloud_jpeg` does the same for JPEG (250-252). `stars::parse_catalog` validates length, magic, version and the exact payload size before indexing (85-111), and `visible_count`'s binary search is bounded by that validated count, so `payload[middle * RECORD_SIZE + 15]` cannot go out of range.

The gap is coverage. The only corrupt-input test in the area is `decode_cloud_jpeg_invalid_bytes` (`cloud_fetcher.rs:759`), four bytes of garbage. There is no test that feeds `texture_loader::decode` a truncated JXL, a zero-byte file, or a `.jxl` that is really a PNG. `texture_cache` has `a_missing_source_is_an_error_rather_than_a_panic` (695) for the absent case only. Recommendation: three cheap tests against a truncated copy of a tiny generated file. That is the highest-value test addition in the area.

**E4. `downsample_2x` underflows on a zero-width source. (low, plausible, not reachable today)** `texture_loader.rs:112` computes `(sx + 1).min(sw - 1)` where `sw = src_w as usize`. With `src_w == 0`, `dst_w` is still 1 (line 101 uses `.max(1)`), the loop body runs, and `sw - 1` underflows: a debug panic, and in release an index far out of bounds and a panic anyway. No caller can produce a zero-width image today (both callers take dimensions from a decoded image). Recommendation: return an empty `Vec` early when `src_w == 0 || src_h == 0`, one line.

**E5. `orient`, `flip_horizontal` and `shift_horizontal` panic on an inconsistent `DecodedImage`. (low)** All three slice `pixels[y * row_bytes..(y + 1) * row_bytes]` assuming `pixels.len() == width * height * 4`. `DecodedImage` is `pub` with `pub` fields and is constructed by hand outside the crate (`crates/sunlit-core/tests/engine.rs:20` imports it; `mailbox.rs:126-130` builds one in tests), so the invariant is stated nowhere and enforced nowhere. Recommendation: a one-line doc on `DecodedImage` saying `pixels.len() == width * height * 4`, which is cheaper than a constructor and is the invariant every consumer already assumes.

**E6. Test temp directories are fixed paths with no cleanup on failure. (low)**

`cloud_fetcher.rs` uses eleven fixed names under `std::env::temp_dir()` (543, 552, 570, 584, 602, 1066, 1189, 1240, 1291, 1338, 1362) and `texture_cache.rs` builds them through `temp_dir(name)` (365-370). Within one test binary the names are distinct, so parallel tests do not collide. Across processes they do: two `cargo test` runs on the same machine, or a unit-test run beside an integration run, share `%TEMP%\sunlit_earth_test_cloud_updater_cache`. Cleanup is also inconsistent: `cloud_fetcher` removes at the start and at the end (so a panicking test leaves the directory), `texture_cache` removes only at the start (so every run leaves its directories behind, permanently). Recommendation: one shared helper that appends `std::process::id()` and a counter, with a `Drop` guard for cleanup. The same file already has exactly that idea implemented for production code in `unfinished` (`texture_cache.rs:315-321`).

### F. Tests

Answering the specific questions first.

**Does `cloud_fetcher` hit the network?** No. `HttpCloudSource` is never exercised by any test in the area. Every updater test drives `ScriptedSource` (`cloud_fetcher.rs:788-906`), an in-process `CloudSource` implementation with atomics for version, fetch count, scripted failures and scripted corrupt bodies. This is the right design and worth keeping as is.

**Injection mechanism:** a trait object, `Arc<dyn CloudSource>`, passed to `CloudUpdater::new` (341-348). Not a feature flag, not a closure, not a generic. The same trait is what `tests/engine.rs:2203` and `tests/support/mod.rs:356` implement, so the seam is used at both levels.

**Sleeps or real timers?** None. `RetryBackoff` (52-79) is a pure value type precisely so the schedule can be asserted without sleeping, and `updater_reports_failure_and_recovers` (997-1022) drives it by hand. The only clock reference in the module is `Instant::now()` at 452 and 464, used for a log field. The sleeping happens in `engine/mod.rs:1330`, outside the area.

**Disk?** Yes, in nine tests, all with tiny files: a 4x2 JPEG fixture (`ScriptedSource::new`, 802-810) and TOML sidecars of a few dozen bytes. See E6 for the naming problem.

**Slowest tests, and why.** My estimate, not a measurement (the brief forbids running cargo): nothing in `cloud_fetcher.rs` is slow despite the 42 count. The fixture is 4x2 pixels and the disk work is a handful of `remove_dir_all` and `write` calls, which on Windows are milliseconds each. The nine disk-touching tests dominate the module's wall time, and the whole module should still be well under a second. The slow test in the area is `texture_loader.rs:314-331` `downsample_output_size_invariant`: a proptest generating up to 65,536 random bytes per case, 256 cases by default, with a `prop_assume!` at line 324 that rejects a large fraction of generated cases (the required length is derived from `half_w * half_h`, independently of the generated vector's length), so proptest generates and discards several hundred large vectors on top of the 256 it keeps. It asserts only that the output length is `half_w * half_h * 4`, which `downsample_output_length` (304-312) already checks in five cases in microseconds. Delete it. `flip_twice_is_identity_for_any_size` (333-349) and `shift_four_times_is_identity` (351-370) have the same rejection-sampling waste but assert real algebraic invariants, so shrink their generators (derive the byte count from the dimensions with `prop_flat_map`, or fill deterministically) rather than deleting them.

**`texture_cache` decoding in tests:** yes, real PNG encode and decode through the `image` crate, but the fixture is 32x16 (`write_source`, 355-363), and 64x32 once at line 586. That is tens of microseconds. The downscale cache tests are not a runtime problem.

**F1. `cloud_fetcher.rs`: about 11 of 42 tests are merges. (medium)**

| Remove or merge | Into | Why |
|---|---|---|
| `save_cache_meta_creates_parent_directories` (569) | delete | `save_and_load_cache_meta_round_trip` (551) removes the directory first, so it already proves the parent is created |
| `cache_meta_etag_only_round_trip` (583), `cache_meta_both_none_round_trip` (601) | fold into `save_and_load_cache_meta_round_trip` as a table of three `CacheMeta` values | three tests, one behavior |
| `every_offered_resolution_has_its_own_variant` (619) | delete | covered by `the_variant_grows_with_the_resolution` (628) plus `a_width_outside_the_offered_three_takes_the_widest_that_fits` (651), and it pins presets (F4) |
| `the_url_names_the_variant` (660) | merge into `resolve_cloud_url_without_override_uses_the_variant` (667) | both assert the URL names the variant |
| `resolve_cache_dir_without_override_ends_in_app_folder` (748) | delete | it is `if let Some(dir) = ...`, so it passes vacuously when `dirs` returns `None`, and otherwise it tests the `dirs` crate |
| `updater_posts_a_frame_on_first_poll` (958), `updater_skips_unchanged_sources` (970), `updater_picks_up_a_new_publication` (986) | one `updater_polls_download_revalidate_and_republish` | three sequential steps of one lifecycle, each rebuilding the same fixture |
| `updater_failure_does_not_poison_the_cached_etag` (1025) | merge into `updater_reports_failure_and_recovers` (998) | the second is the first plus a cached-ETag assertion. While merging, drop lines 1009-1012 and 1019-1020, which re-assert `backoff_starts_short_and_doubles` (926) and `backoff_resets_after_a_success` (944) |
| `set_resolution_to_the_variant_in_force_does_nothing` (1150) | merge into `two_resolutions_with_one_variant_do_not_retarget` (1165) | both assert `retargets().is_empty()` after a no-op switch |
| `a_switch_back_to_a_fresh_cache_entry_puts_its_pixels_on_screen` (1188) | merge with `each_variant_keeps_its_own_cached_image` (1361) | both run 8192, then 2048, then 8192 against a disk cache and both assert `source.fetches() == 2`; the second's own comment at 1383-1384 points at the first. One test can assert both the fetch count and the posted frame |

That is 42 down to about 31, and with the A3 comment removals the test module drops from 854 lines to roughly 520.

**F2. `texture_cache.rs`: 6 of 19. (low)**

`a_source_at_the_target_width_is_not_halved` (375), `each_step_halves_until_the_target_is_reached` (380), `a_source_narrower_than_the_target_is_left_alone` (389) and `halving_a_target_of_zero_terminates_at_one_pixel` (395) are 8 assertions on a pure function across four `#[test]`s; one table-driven test covers all of them. `each_width_gets_its_own_cache_file` (413) is implied by `cache_paths_name_the_source_and_the_width` (402), which shows the width in the name. `without_a_cache_directory_the_source_is_still_halved` (616) is implied by `the_cached_path_and_the_direct_path_agree` (628), which calls `load_at_resolution(..., None)` and asserts the dimensions and the pixels. `a_source_that_vanished_during_the_decode_is_not_cached` (678) and `a_source_that_changed_during_the_decode_is_not_cached` (646) are the two arms of the same guard at line 206 and read naturally as one test.

**F3. `texture_loader.rs`: 5 of 13; `mailbox.rs`: 2 of 9. (low)**

`texture_loader`: delete `downsample_output_size_invariant` (see above); `flip_twice_is_identity` (260) is the fixed-size case of `flip_twice_is_identity_for_any_size` (335); `downsample_2x2_uniform_red` (274) and `downsample_4x4_uniform_white` (295) are one property (a uniform input survives downsampling) at two sizes; of the four shift tests (170, 185, 213, 221) the 4-pixel and 8-pixel single-row cases are the same assertion at two widths.

`mailbox`: `mailbox_keeps_the_newer_parked_message_over_a_stale_arrival` (205) and `mailbox_replaces_a_stale_parked_message_with_a_newer_arrival` (218) are the two directions of one rule and read better as one test with two posts each way. `mailbox_keeps_one_message_per_slot_independently` (187) overlaps `mailbox_take_all_empties_the_mailbox` (157) and `mailbox_replaces_unconsumed_message_in_same_slot` (175). Keep `only_two_stamped_messages_are_ordered` (253): it is the complete truth table of `supersedes` in six lines and costs nothing.

**F4. Tests that pin presets or baked constants. (medium, it is an explicit project rule)**

- `cloud_fetcher.rs:619-623` `every_offered_resolution_has_its_own_variant` hardcodes 2048, 4096 and 8192, which are exactly the contents of `config::TEXTURE_RESOLUTIONS`. Adding or removing a tier breaks it. Same for `660-664` `the_url_names_the_variant`. The sibling test at 628 does it right, iterating `TEXTURE_RESOLUTIONS` and asserting injectivity.
- `stars.rs:181-183` `shipped_catalog_has_expected_count` asserts `15_597` exactly. That is a property of the bake, not of the code, and it breaks on any change to the magnitude cutoff or the HYG revision. The three tests beside it (186, 207, 221) already assert the structural invariants that matter. Recommendation: assert a floor (`> 10_000`) or, if the point is that the committed blob is the one the bake produces, leave that to the existing bake-comparison test the project already has.
- `stars.rs:216` uses `7.02` as the magnitude ceiling, a bake parameter with a fudge factor. Recommendation: derive it from a named constant next to the bake, or widen it to the encodable range `-2.0..=8.0`, which is what the encoding at line 42 actually clamps to.

**F5. Tests that duplicate an integration test.** `tests/engine.rs:2278` `the_cloud_variant_follows_the_texture_resolution` covers the same switch behavior as the unit tests at `cloud_fetcher.rs:1131` and `1361`, but through the engine and against a different fixture, so it is a layer test rather than a duplicate. No action. `tests/engine.rs:2072-2074` exercises `texture_cache::load_at_resolution` against the real 8K assets, which is what the unit tests deliberately do not do. Also no action.

### G. Best practices

**G1. `HttpCloudSource::url()` clones the URL on every request. (low, correct as written)** `cloud_source.rs:69-74` returns an owned `String` and the doc at 64-68 explains why (the request outlives any sensible lock hold). That is the right call. Noting it only so it is not flagged later: this clone is deliberate and the comment earns its place.

**G2. `retarget` clears and refills instead of assigning. (low)** `cloud_source.rs:150-151` does `current.clear(); current.push_str(url);` where `*current = url.to_owned();` says the same thing. The clear-and-push form reuses the allocation, which for a URL under a lock is not a saving worth the extra line. Recommendation: assign.

**G3. `cache_stem` returns an owned `String` per call. (low)** `cloud_fetcher.rs:317-323` allocates on every `set_resolution` and every construction, which is at most a handful of times per session. No action; noting it as considered and dismissed.

**G4. `render_pass.rs:482` re-parses the embedded catalog per frame.** `crate::assets::stars::embedded_catalog()` runs `parse_catalog` (four length and equality checks) on every call. It is O(1) and trivially cheap, but if the pattern spreads a `LazyLock<StarCatalog<'static>>` would be the obvious form. No action for this release.

## 4. Recommended refactors, by value over effort

| # | Refactor | Effort | Risk | Value |
|---|---|---|---|---|
| 1 | Cut the test-module comments in `cloud_fetcher.rs` per A3, and the four "why this test exists" blocks in `mailbox.rs`, `texture_cache.rs` | 1 h | none | Removes about 100 lines and brings the file's biggest rule violation in line. Pure deletion, no behavior touched |
| 2 | Trim A1 and A2: the eight long narrative blocks whose content is already in `docs/architecture.md` | 1.5 h | none | Removes roughly 80 more lines from the code halves, where the comment ratio is 28% to 40%. Verify each sentence survives in `docs/architecture.md` before deleting it |
| 3 | Merge the 11 `cloud_fetcher` tests in F1 and the 6 in F2 | 3 h | low, tests only | 93 tests to about 68, and `cloud_fetcher.rs` from 1391 to roughly 1000 lines without moving an item |
| 4 | D1: make `decode_cloud_jpeg` call `texture_loader::orient`, delete `flip_matches_the_image_crates_own` | 0.5 h | low; the golden tests are the safety net and the two spellings are provably equal | Removes a real duplication and about 100 MB of peak allocation on the 8K cloud path |
| 5 | E3: three tests for truncated, empty and mislabeled image input to `texture_loader::decode` | 1 h | none | The area's one real coverage gap, on the path that reads user-supplied files |
| 6 | C1: demote the nine crate-internal `pub` items, delete `no_notify` | 0.5 h | low; `cargo build` finds every miss | Narrows the public surface before a first release freezes it |
| 7 | F4: replace the three preset-pinning assertions | 0.5 h | none | Removes tests that will break on a harmless preset change, which the project forbids |
| 8 | Delete `downsample_output_size_invariant`, fix the generators of the two surviving proptests | 0.5 h | none | The only measurable runtime win in the area |
| 9 | E6: one temp-directory helper with a pid suffix and a `Drop` guard | 1 h | low | Fixes cross-process collisions and the directories `texture_cache` tests leave behind forever |
| 10 | B1: split `cloud_fetcher.rs` into `cloud_fetcher/{mod,cache,variant}.rs` | 2.5 h | low, mechanical, but touches imports in `engine/mod.rs` and `main.rs` | Do this after 1 to 3; if those land, the file is about 1000 lines and the split is optional rather than urgent |
| 11 | E2, E4, E5: an explicit decode limit, the zero-width guard, the `DecodedImage` invariant doc | 1 h | low | Three small resilience fixes, none of them reachable today |
| 12 | D2 and D3: share `unfinished`, `write_png` and the atomic TOML write | 2 h | medium; `wallpaper.rs` and `config.rs` are outside this area and have their own tests | Lowest value of the set. Worth doing only if someone is already in those files |

Items 1, 2, 3, 4 and 6 together answer the maintainer's stated concerns directly and carry no behavioral risk. I would do those five first and treat the rest as optional before the release.
