# JXL Texture Support — External Research

Date: 2026-03-15
Topic: JPEG-XL decoding libraries in the Rust ecosystem for GPU texture upload via wgpu

---

## Executive Summary

The Rust JXL ecosystem has two credible options: **jxl-oxide** (pure Rust, MIT/Apache-2.0,
actively maintained, v0.12.5) and **jpegxl-rs** (C FFI wrapper over libjxl, GPL-3.0,
v0.13.1). For this project, jxl-oxide is the clear choice. It integrates with the `image`
crate via a decoding hook (added in image v0.25.8 + jxl-oxide v0.12.5), meaning the
existing `ImageReader::open(...)?decode()?` call path can be extended to transparently handle
`.jxl` files with minimal code changes. The project already depends on `image = "0.25"` and
only needs the version floor raised to `0.25.8` and the `jxl-oxide` optional feature added.

---

## 1. Crates Surveyed

### 1.1 jxl-oxide

| Attribute | Detail |
|---|---|
| Crates.io | `jxl-oxide` |
| Latest version | 0.12.5 (released 2025-09-30) |
| License | MIT OR Apache-2.0 |
| Implementation | Pure Rust — zero C/C++ dependencies |
| Rust edition | 2024 (requires Rust 1.85+) |
| Maintenance | Active; maintained by tirr-c (Wonwoo Choi) |
| Downloads | High; widely used in the ecosystem |

**What it does.** A spec-conforming JPEG XL decoder. Internally split into sub-crates
(`jxl-bitstream`, `jxl-color`, `jxl-frame`, `jxl-render`, etc.) with `jxl-oxide` as the
public API blanket.

**Feature flags:**

- `rayon` (default) — multithreaded decoding via Rayon
- `image` — integration with the `image` crate (requires image ≥ 0.25.8)
- `lcms2` — Little CMS 2 color management (C dependency, optional)
- `moxcms` — pure-Rust color management alternative (added in 0.12.0)
- `bytemuck` — typed byte-buffer access to pixel data

**Pixel output.** `JxlImage::render_frame(idx)` returns a `RenderedImage`. Raw channel
data is accessible as typed buffers. The `bytemuck` feature enables zero-copy casting to
`&[u8]`. The `image` feature wraps output as `image::DynamicImage`, which can be converted
to `Rgba<u8>` via `.into_rgba8()` — exactly what is needed for GPU texture upload.

**image crate integration (v0.12.5+).** PR #480 (merged 2025-09-07, shipped in 0.12.5)
adds a decoding hook for the `image` crate's new external format registration API
(image v0.25.8+). After the hook is registered, the standard call:

```rust
jxl_oxide::integration::register_decoding_hook();
let img = image::ImageReader::open("earth.jxl")?.decode()?;
let rgba = img.into_rgba8(); // ImageBuffer<Rgba<u8>, Vec<u8>>
```

works without any other changes. The `JxlDecoder` type in
`jxl_oxide::integration` is made available when the `image` feature flag is enabled.

**Large image handling.** `JxlImageBuilder::force_wide_buffers()` was added in v0.12.3 for
images that exceed default buffer sizing. The streaming init API (`build_uninit()` +
`feed_bytes()` + `try_init()`) supports incremental loading if memory is a concern. Rayon
parallelism is on by default and will utilize all available CPU cores during decode.

**Performance.** The pure-Rust decoder is approximately 2× slower than the libjxl C++
reference implementation on equivalent hardware. For a one-time texture load at startup this
is unlikely to matter; a typical 4K equirectangular JXL might take a few hundred
milliseconds on a modern CPU.

**Windows support.** No special requirements. Pure Rust, no build-time C toolchain needed.
Confirmed to target Windows in its CI matrix.

**Known issues.** docs.rs build failed for 0.12.5 (use 0.12.4 docs). Color management for
CMYK or arbitrary ICC profiles requires lcms2 or moxcms; standard sRGB textures work
without any CMS.

---

### 1.2 jpegxl-rs

| Attribute | Detail |
|---|---|
| Crates.io | `jpegxl-rs` |
| Latest version | 0.13.1+libjxl-0.11.2 (released 2026-02-11) |
| License | GPL-3.0-or-later |
| Implementation | Safe Rust bindings over libjxl (C++) |
| Maintenance | Active; maintained by `inflation` |

**What it does.** Wraps the official libjxl reference implementation via `jpegxl-sys`.
Decoder builder pattern; output pixels typed as `u8`, `u16`, `f16`, or `f32`. Supports RGBA
channel selection.

**Windows build requirements.** Requires MSVC, Clang, Ninja, and CMake to compile libjxl.
The `vendored` feature triggers a source build; without it the developer must provide a
prebuilt libjxl. This significantly complicates Windows CI and developer onboarding.

**Feature flags.**

- `vendored` — build and statically link libjxl from source
- `image` (default) — image crate integration
- `threads` — multi-threaded decoding (can be disabled)
- `bench` — benchmarking helpers

**License note.** GPL-3.0-or-later. Since sunlit-earth is itself GPL-3.0-or-later, there is
no license compatibility problem. However the heavy Windows build requirements and C++
toolchain dependency make this less attractive than jxl-oxide regardless.

---

### 1.3 libjxl/jxl-rs (official Rust reimplementation)

| Attribute | Detail |
|---|---|
| GitHub | `libjxl/jxl-rs` |
| Latest release | v0.3.0 (2026-01-21) |
| License | BSD-3-Clause |
| Status | Work-in-progress / experimental |
| Crates.io | Published (under `jxl` crate name) |

This is the official libjxl team's Rust reimplementation. It is not production-ready as of
early 2026. The README describes it as a conformance-focused WIP and actively solicits bug
reports. Not suitable for production use at this time.

---

### 1.4 zune-jpegxl

| Attribute | Detail |
|---|---|
| Crates.io | `zune-jpegxl` |
| Latest version | 0.5.2 (released 2026-01-24) |
| License | MIT OR Apache-2.0 (part of the zune-image family) |
| Implementation | Pure Rust |

**Important limitation.** zune-jpegxl is a JXL **encoder only**, not a decoder. It handles
lossless JXL encoding at 8-bit and 16-bit depth. Not relevant for this use case.

---

### 1.5 image crate (built-in JXL support)

The `image` crate (v0.25.10 as of 2026-03-10) has **no built-in JXL support**. Its
documented format list is: AVIF, BMP, DDS, EXR, FF, GIF, HDR, ICO, JPEG, PNG, PNM, QOI,
TGA, TIFF, WebP. There is no `jxl` feature flag.

JXL support is provided exclusively via the external hook mechanism introduced in
image v0.25.8, which allows third-party crates (specifically jxl-oxide) to register a
decoder at runtime.

---

## 2. Comparison Table

| Crate | Type | License | RGBA8 | image hook | Windows | Status |
|---|---|---|---|---|---|---|
| jxl-oxide 0.12.5 | Pure Rust | MIT/Apache-2.0 | Yes (via `.into_rgba8()`) | Yes (v0.12.5+) | Yes, no extras | Stable, active |
| jpegxl-rs 0.13.1 | C FFI (libjxl) | GPL-3.0 | Yes (u8 pixel type) | Yes (default) | Needs MSVC+Clang+CMake | Stable, active |
| libjxl/jxl-rs 0.3.0 | Pure Rust | BSD-3-Clause | Unknown | No | Unknown | Experimental WIP |
| zune-jpegxl 0.5.2 | Pure Rust | MIT/Apache-2.0 | N/A (encoder only) | No | Yes | Active (encoder only) |
| image crate built-in | N/A | N/A | N/A | N/A | N/A | Not supported |

---

## 3. Integration Path for This Project

The project already has `image = { version = "0.25", default-features = false, features = ["jpeg"] }`.
The path of least resistance:

**Step 1 — Raise the image version floor to 0.25.8.**

```toml
image = { version = "0.25.8", default-features = false, features = ["jpeg"] }
```

**Step 2 — Add jxl-oxide with the `image` feature.**

```toml
jxl-oxide = { version = "0.12.5", default-features = false, features = ["image", "rayon"] }
```

`default-features = false` drops the lcms2/moxcms CMS dependencies. The `rayon` feature
keeps multithreaded decode. For NASA Blue Marble / Black Marble equirectangular maps in sRGB
or Display P3, no external CMS is needed.

**Step 3 — Register the hook once at startup, before any image load.**

```rust
jxl_oxide::integration::register_decoding_hook();
```

This call is idempotent and can live in `main()` before the Slint window is created.

**Step 4 — Load the texture exactly as today.**

The existing `image::ImageReader::open(path)?.decode()?` call in `earth_texture.rs` (or
wherever texture loading lives) will transparently handle `.jxl` files after the hook is
registered. Convert to RGBA8 for GPU upload:

```rust
let rgba_image = dynamic_image.into_rgba8();
let width = rgba_image.width();
let height = rgba_image.height();
let pixels: &[u8] = rgba_image.as_raw(); // flat RGBA8 bytes, ready for wgpu
```

No changes to the wgpu texture upload path are needed; the output is identical to what
`.into_rgba8()` already produces for JPEG inputs.

---

## 4. Memory Considerations for High-Resolution Equirectangular Maps

A 16384×8192 RGBA8 image consumes 512 MB of RAM during decode. A 8192×4096 image uses 128 MB.
These are transient peaks — the buffer can be released after GPU upload (via `queue.write_texture`).

jxl-oxide's `force_wide_buffers()` on `JxlImageBuilder` should be used if decoding images
wider than roughly 32768 pixels (an edge case for even the highest-res NASA textures), but
standard Blue Marble / Black Marble resolutions (up to 21600×10800) should decode without it.

JXL's progressive decoding (`feed_bytes` / streaming init) is not needed for disk files
opened synchronously — `JxlImage::builder().open(path)` is the right API. The `image` crate
hook internally uses this path.

---

## 5. Confidence Assessment

| Question | Confidence | Notes |
|---|---|---|
| jxl-oxide is the best pure-Rust option | High | No serious competition in pure-Rust space |
| jxl-oxide image hook works in v0.12.5 | High | Merged PR, shipped in release |
| image crate has no native JXL support | High | Confirmed from official repo |
| jpegxl-rs Windows build is complex | High | CMake + Clang required; documented in README |
| RGBA8 output via `.into_rgba8()` works | High | Standard image crate DynamicImage API |
| `force_wide_buffers` needed for 16K+ images | Medium | Added in 0.12.3 release notes; exact threshold not documented |
| Performance is acceptable for startup load | Medium | ~2× slower than libjxl; no benchmarks on specific texture sizes found |

---

## 6. Knowledge Gaps

- Exact decode time for a typical 8192×4096 or 16384×8192 JXL file on a mid-range CPU was
  not found. Worth benchmarking before committing to the approach if startup latency matters.
- The exact pixel format output by the image hook (does it always produce sRGB, or does it
  pass through HDR?) was not confirmed. For NASA textures (sRGB), this should not be an
  issue, but HDR JXL files would need color-space consideration.
- Whether jxl-oxide's `image` feature re-exports a `JxlDecoder` struct for manual use (vs.
  only a `register_decoding_hook` function) was not fully confirmed from public docs.
  The PR description mentions `jxl_oxide::integration::JxlDecoder` but the primary user-facing
  API appears to be the hook registration function.

---

## 7. Recommended Next Steps

1. Bump `image` to `"0.25.8"` in Cargo.toml and add `jxl-oxide = "0.12.5"` with
   `features = ["image", "rayon"]`.
2. Call `jxl_oxide::integration::register_decoding_hook()` in `main()` before window creation.
3. Update texture loading to handle both `.jpg`/`.jpeg` and `.jxl` paths (the hook makes this
   transparent at the `ImageReader` level).
4. Add a simple bench or timed log statement around the JXL decode of the real texture files
   to measure startup impact.
5. Optionally test with `default-features = false` on jxl-oxide to avoid pulling in lcms2's
   C dependency, then add `moxcms` feature only if ICC profile handling turns out to be needed.

---

## Sources

- [jxl-oxide on GitHub (tirr-c/jxl-oxide)](https://github.com/tirr-c/jxl-oxide)
- [jxl-oxide on crates.io](https://crates.io/crates/jxl-oxide)
- [jxl-oxide on lib.rs](https://lib.rs/crates/jxl-oxide)
- [jxl-oxide docs.rs (v0.12.4)](https://docs.rs/jxl-oxide/0.12.4/jxl_oxide/)
- [jxl-oxide releases page](https://github.com/tirr-c/jxl-oxide/releases)
- [PR #480: Add image crate decoding hook](https://github.com/tirr-c/jxl-oxide/pull/480)
- [jxl-render docs.rs](https://docs.rs/jxl-render/0.12.3/jxl_render/)
- [jpegxl-rs on lib.rs](https://lib.rs/crates/jpegxl-rs)
- [jpegxl-rs on GitHub (inflation/jpegxl-rs)](https://github.com/inflation/jpegxl-rs)
- [jpegxl-rs docs.rs](https://docs.rs/jpegxl-rs/latest/jpegxl_rs/)
- [jpegxl-sys on crates.io](https://crates.io/crates/jpegxl-sys)
- [libjxl/jxl-rs on GitHub](https://github.com/libjxl/jxl-rs)
- [zune-jpegxl on lib.rs](https://lib.rs/crates/zune-jpegxl)
- [image crate on GitHub (image-rs/image)](https://github.com/image-rs/image)
- [image crate on lib.rs](https://lib.rs/crates/image)
- [JXL keyword on crates.io](https://crates.io/keywords/jxl)
