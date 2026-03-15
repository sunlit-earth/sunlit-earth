# JXL Decode Performance Research

Date: 2026-03-15
Status: Research complete

## Summary

A ~1 second decode time for an 8K (8192x4096) JPEG XL texture in a debug build is plausible and probably expected given the combination of factors at play: jxl-oxide is a pure-Rust decoder that is roughly 2.8x slower than the C reference implementation (libjxl), debug builds add additional overhead on top of already-optimized dependency code, and mipmap generation after decode touches ~213 MB of pixel data. In a release build the total pipeline should complete in the 100–300 ms range on modern hardware, dominated by the JXL decode itself.

---

## Question 1: Is ~1s expected for jxl-oxide decoding an 8K image in debug mode?

### jxl-oxide vs libjxl performance

The clearest published benchmark comes from a blog post that tested decoding a single lossy JXL image (5221x2847 px, ~14.8 MP) on a Ryzen 2600:

| Decoder | Mean time | Ratio |
|---------|-----------|-------|
| djxl (libjxl) | 200 ms | 1.0x (baseline) |
| jxl-oxide | 562 ms | 2.80x slower |

This is for a 14.8 MP image. The 8K texture is 33.5 MP — roughly 2.3x more pixels. Scaling linearly (decode time is roughly proportional to pixel count for many codecs), libjxl would decode the 8K image in roughly 460 ms on Ryzen 2600 hardware. jxl-oxide would take approximately 1.3 seconds on the same hardware.

### libjxl throughput reference

OpenBenchmarking.org data for libjxl single-threaded decode across 377 machines:

- Ryzen 9 9950X / 9700X / 9900X: ~112 MP/s (top 96–99th percentile)
- Intel Core i9-14900K: ~100 MP/s (85th percentile)
- Median across all CPUs: ~77 MP/s
- Range: 6–116 MP/s

For an 8K texture (33.5 MP) decoded with libjxl:
- Fast modern CPU (100 MP/s): ~335 ms
- Median CPU (77 MP/s): ~435 ms
- Slower machine (40 MP/s): ~840 ms

Applying the 2.8x jxl-oxide penalty:
- Fast CPU: ~940 ms
- Median CPU: ~1.2 s
- Slower machine: ~2.4 s

**Conclusion: ~1s for jxl-oxide on a mid-to-fast CPU in an optimized (release) build is already realistic. In a debug build, it would be longer.**

### Debug vs release impact on jxl-oxide specifically

The project's `Cargo.toml` uses:

```toml
[profile.dev.package."*"]
opt-level = 2
```

This gives all dependencies (including jxl-oxide and all its sub-crates) opt-level 2 in debug builds. This is a significant improvement over the default dev opt-level 0. jxl-oxide is a decode-heavy library with no local-crate generic instantiation in the hot paths — the bulk of its work is in its own crates. So the `opt-level = 2` override almost certainly brings jxl-oxide close to its release-build performance in a dev build.

However, the local crate code (the pixel transforms — `fliph()`, RGBA conversion, `shift_horizontal()`) runs at opt-level 0. These operations touch the full 8K pixel buffer (128 MB). Without optimization, they will be slower than in release, but they are simpler operations that benefit less from loop unrolling/vectorization than the decoder's entropy coding.

**Rough estimate: the opt-level=2 override means jxl-oxide itself probably runs at 80–90% of release speed. The local pixel transforms run at maybe 50–70% of release speed but contribute less total time.**

---

## Question 2: Does the local crate's opt-level affect jxl-oxide decode speed?

### The monomorphization caveat

A key subtlety documented in the Rust RFC for profile overrides: when generic functions defined in a dependency are instantiated in the local crate (monomorphization), the code may be generated in the local crate and therefore compiled at the local crate's optimization level, not the dependency's.

For jxl-oxide, this concern is minimal because:
1. The public API is not heavily generic — callers call `JxlImage::builder().open(path)` or use the `image` crate hook.
2. The hot decode paths (entropy decoding, color transforms, ANS decoding) are all non-generic internal functions within jxl-oxide's own crates. They get opt-level 2.
3. The trait-based dispatch for `image::ImageDecoder` involves some generic plumbing in the `image` crate, but the actual work dispatches into jxl-oxide's concrete types.

**Conclusion: the opt-level=2 override is effective for jxl-oxide. Local crate overhead is real but small relative to decode time.**

### What the local crate does contribute

After jxl-oxide returns an `ImageBuffer`, the local pipeline does:
1. `fliph()` — horizontally flips the image (reads and writes 128 MB)
2. `into_rgba8()` — converts to RGBA8 (may be a no-op or a pixel-format conversion)
3. `shift_horizontal()` — rotates each row by 3/4 width (reads and writes 128 MB)

These three operations each touch the full pixel buffer. At opt-level 0 they lack SIMD and loop unrolling. At opt-level 2 (if you added a local override) they'd be meaningfully faster.

---

## Question 3: jxl-oxide configuration options that affect decode speed

### Rayon feature (already enabled)

The `rayon` feature enables multi-threaded decoding. It is enabled in this project:

```toml
jxl-oxide = { version = "0.12", default-features = false, features = ["image", "rayon"] }
```

As of v0.11.2, jxl-oxide defaults to using the global Rayon thread pool rather than creating a new one (a fix for stack overflow issues on Windows). The parallelism was introduced "in various places of the decoding process" with the renderer made thread-safe.

The 2.8x ratio measured in the blog benchmark was likely with a single-threaded build or single-threaded test. Multi-threaded decode should reduce jxl-oxide's wall-clock time proportionally to the number of available cores, up to the parallelism available in the image's encoding. For a large 8K image the parallelism should be substantial.

**Key question: was the 2.8x ratio measured with or without rayon? If without, multi-threaded jxl-oxide may close the gap with libjxl significantly.**

### No other user-configurable speed knobs in jxl-oxide

The jxl-oxide public API does not expose any "speed vs accuracy" tradeoff for decoding. Unlike encoders, the decoder is spec-bound to produce correct output. The only speed knob is threading.

### Color management

The project uses `features = ["image", "rayon"]` with `default-features = false`. This means the `lcms2` and `moxcms` color management backends are not enabled. Color management can add overhead when color profile conversion is required (e.g., images with embedded ICC profiles). Disabling it means the decoded pixels are returned in the image's native color space without conversion — which is faster, and appropriate for display textures.

---

## Question 4: jxl-oxide vs libjxl performance comparison

### Published data

| Source | jxl-oxide vs libjxl |
|--------|---------------------|
| Blog benchmark (Ryzen 2600, single image ~15 MP) | 2.80x slower |
| Qualitative community assessment (HN discussion, 2024) | "Not as fast as libjxl, still pretty fast" |
| Qualitative (HN, 2024) | "Not yet performant enough to replace libjxl's decoder, not necessarily because of language differences" |
| Optimization activity (Dec 2025) | 26 optimization PRs merged in a single month |

### Why the gap exists

1. libjxl has extensive hand-written SIMD (SSE2/AVX2/NEON) for its hot paths
2. jxl-oxide is a pure-Rust implementation — it relies on LLVM auto-vectorization rather than manual SIMD
3. jxl-oxide has SIMD for aarch64 (ARM) but limited x86 SIMD work at the time of research
4. jxl-oxide's architecture is still being optimized — significant work was ongoing as recently as late 2025

### The jxl-rs alternative

There is also `jxl-rs`, a separate Rust JXL decoder being developed under the libjxl organization (used by Chrome). It targets closer parity with libjxl in performance. As of early 2026 jxl-rs-based decoding was merged into Chromium. However, jxl-rs is not available as a general-purpose crate in the same way jxl-oxide is, and it is primarily targeted at browser use cases.

**For now, jxl-oxide is the practical choice for Rust projects. The 2.8x gap is the expected overhead.**

---

## Question 5: Does encoding effort level affect decode speed?

### The short answer: no meaningful effect

JPEG XL's encode effort (1–10, expert 11) primarily controls:
- How exhaustively the encoder searches for optimal coding parameters
- Encoding speed
- Compression ratio (file size)

It does **not** select a different bitstream format that would be harder or easier to decode. The decoder always follows the same code paths determined by the bitstream content.

However, there is an indirect effect:
- Higher-effort encodes may use more complex coding tools (e.g., more adaptive contexts, better VarDCT parameters) that the decoder must process
- Lower-effort encodes may use simpler/faster coding tools

In practice, this indirect effect is small — a well-optimized decoder handles all coding tool combinations efficiently. The dominant factor in decode time is the image's dimensions and bit depth, not the encoding effort.

### Lossless vs lossy

Lossless JPEG XL uses a different codec path internally (Modular mode vs VarDCT for lossy). Lossless JXL has been benchmarked as decoding 12x slower than libpng in some scenarios. For a texture that does not need pixel-perfect lossless fidelity (e.g., a satellite photo), lossy JXL is strongly preferable for decode performance. The NASA Blue Marble textures are already lossy JXL (converted from JPEG), so this is not an issue here.

---

## Question 6: Debug vs release difference for the full pipeline

The full pipeline for loading an 8K texture involves:

| Stage | Where it runs | opt-level in dev | Notes |
|-------|--------------|------------------|-------|
| JXL decode (jxl-oxide) | dependency | 2 | Benefits strongly from optimization |
| `fliph()` / `into_rgba8()` (image crate) | dependency | 2 | Benefit from optimization |
| `shift_horizontal()` | local crate | 0 | Pure Rust, no SIMD at opt-level 0 |
| `create_mipmapped_texture()` / `downsample_2x()` | local crate | 0 | Major work at opt-level 0 |
| `queue.write_texture()` (wgpu) | dependency | 2 | GPU transfer, likely fast |

The current dev setup (`opt-level = 2` for all `"*"` packages) is already the best practical configuration short of a release build for the dependency-heavy parts. The local crate functions remain at opt-level 0.

**In a release build (`cargo build --release`):**
- LTO = true, codegen-units = 1: enables cross-crate inlining
- Local crate functions compile at opt-level 3 (release default)
- `downsample_2x` and `shift_horizontal` get loop unrolling and potential SIMD
- Expected total speedup over dev build: 2–5x for the local-crate portions

**Estimated total pipeline times (rough order-of-magnitude):**

| Build | JXL decode | Pixel transforms | Mipmap gen | GPU upload | Total |
|-------|-----------|-----------------|------------|------------|-------|
| Debug (opt-level=0) | ~2–3 s | ~300 ms | ~500 ms | ~50 ms | ~3–4 s |
| Dev (opt-level=2 deps) | ~600 ms–1.2 s | ~100 ms | ~200 ms | ~50 ms | ~1–1.5 s |
| Release | ~200–600 ms | ~50 ms | ~50–100 ms | ~50 ms | ~300–750 ms |

These are estimates based on scaling the benchmark data. The actual numbers depend heavily on CPU, whether rayon is using multiple cores, and image content complexity.

---

## Question 7: Is mipmap generation a significant part of the ~1s?

### Pixel data analysis

For an 8192x4096 RGBA8 texture, `create_mipmapped_texture` calls `downsample_2x` 13 times:

| Operation | Pixels read | Memory read | Pixels written | Memory written |
|-----------|------------|-------------|---------------|----------------|
| Level 0→1 | 33,554,432 | 128 MB | 8,388,608 | 32 MB |
| Level 1→2 | 8,388,608 | 32 MB | 2,097,152 | 8 MB |
| Level 2→3 | 2,097,152 | 8 MB | 524,288 | 2 MB |
| Levels 3–13 | ~524,287 | ~2 MB | (negligible) | |
| **Total** | **44.7M pixels** | **170 MB read** | **11.2M pixels** | **42 MB written** |

The level 0→1 pass dominates: it reads 128 MB and writes 32 MB, totaling 160 MB of memory access. This is a memory-bandwidth-bound operation.

### Current implementation analysis (`downsample_2x`)

```rust
fn downsample_2x(src: &[u8], src_w: u32, src_h: u32) -> Vec<u8> {
    // ...
    for y in 0..dst_h {
        for x in 0..dst_w {
            for c in 0..4 {
                let tl = u16::from(src[(sy * sw + sx) * 4 + c]);
                // ... etc
            }
        }
    }
}
```

At opt-level 0 (local crate in dev builds), this loop:
- Does not auto-vectorize
- Has bounds checks on every array access
- Computes index arithmetic without hoisting
- Allocates a new `Vec<u8>` for each mip level (13 allocations totaling ~42 MB)

At opt-level 2 or 3 with LLVM, this loop would auto-vectorize with SSE2/AVX2 on x86_64, processing 16–32 bytes per instruction. The speedup could be 4–16x.

**Estimated contribution to ~1s dev build time:**
- Level 0→1 at opt-level 0 on a modern CPU: ~100–300 ms (memory-bound + unoptimized loop)
- All subsequent levels: ~50 ms total
- Total mipmap generation: ~150–350 ms

**This is a material fraction of the observed ~1s total pipeline time.** It is likely the second-largest contributor after JXL decode.

### Comparison: `shift_horizontal` also touches 128 MB

The pixel shift operation (rotate each row by 3/4 width) is implemented as `row.rotate_right(shift_bytes)` on each of 4096 rows. At opt-level 0 this is also unoptimized. It likely adds another 50–150 ms in dev builds.

---

## Recommendations

### Short term: profile before optimizing

Add timing instrumentation to identify the actual breakdown. The renderer already logs the `texture_loader::load` time. Add similar timing around:
1. `downsample_2x` / `create_mipmapped_texture`
2. The pixel transforms (`fliph`, `shift_horizontal`)
3. `queue.write_texture` calls

### Option A: Optimize the local crate pixel operations only (lowest effort)

Add to `Cargo.toml`:

```toml
[profile.dev]
opt-level = 1  # Optimize local crate slightly in dev
```

Or more targeted: move `downsample_2x` and `shift_horizontal` into a separate crate and add an opt-level override for it.

### Option B: GPU-based mipmap generation (recommended for production)

Instead of CPU box-filter mipmaps, generate mipmaps on the GPU using a compute shader or render-to-texture blit. The `wgpu-mipmap` crate provides this. Benefits:
- Avoids the 170 MB CPU-side memory traversal entirely
- GPU can generate all mip levels in parallel
- Eliminates the 13 intermediate `Vec<u8>` allocations
- Only requires `TEXTURE_BINDING | COPY_DST | RENDER_ATTACHMENT` (or `STORAGE`)

Trade-off: requires `STORAGE` or `RENDER_ATTACHMENT` texture usage, and not all platforms support blit-based mip generation (though wgpu's `wgpu-mipmap` crate handles fallbacks).

### Option C: Move pixel transforms to before mipmap generation (minor)

Currently: decode → fliph → rgba8 → shift → mipmap (all on 8K data)

The horizontal flip and shift only need to be applied to the base level (level 0). The mipmap levels are derived from the transformed base, so this is already correct. No change needed here.

### Option D: Pre-generate mipmaps offline

Store JXL files with embedded mipmaps (JXL supports this via `extra_channel_info` and multi-frame images). At load time, decode each mip level directly. This moves mipmap generation to the texture preprocessing pipeline and eliminates it entirely from the runtime. This is the approach used by GPU texture formats like DDS/KTX2 with BCn compression.

For the Python preprocessing pipeline already in this project, this would mean generating mip levels before encoding to JXL, and either storing them in separate files or as JXL animation frames.

### Option E: Switch from jxl-oxide to libjxl via jpegxl-rs

The `jpegxl-rs` crate wraps libjxl (the C++ reference implementation) for use from Rust. libjxl decodes 2.8x faster than jxl-oxide based on benchmarks. However, this requires:
- A C++ toolchain and libjxl shared library (or static linking)
- `unsafe` code or an FFI boundary
- The project's `unsafe_code = "deny"` policy would need an exception or a wrapper crate

Given the `unsafe_code = "deny"` policy, this option is currently blocked unless a thin safe wrapper is used. Not recommended for now.

### Option F: Accept the latency and load asynchronously

If the texture is loaded lazily on first use (as the current code does), the 1 second load is a one-time cost per texture per session. If it happens on a background thread (currently it happens on the render/UI thread), it would not block the UI. The async texture loading research (`2026-03-15-async-texture-loading-research.md`) covers this.

---

## Confidence Assessment

| Finding | Confidence | Notes |
|---------|-----------|-------|
| jxl-oxide is ~2.8x slower than libjxl | Medium-High | Based on a single blog benchmark; image content matters |
| ~1s total pipeline time is expected in dev | High | Consistent with the benchmark data and pixel math |
| opt-level=2 override effectively speeds up jxl-oxide | High | jxl-oxide has no generic hot paths instantiated locally |
| Mipmap generation contributes 150–350 ms in dev | Medium | Estimate from memory bandwidth, no direct timing |
| Release build will be 3–5x faster than dev | Medium | Depends on how well downsample_2x vectorizes |
| Encoding effort level has negligible effect on decode speed | High | Well-established JXL design principle |
| rayon feature is active and parallelizes decode | High | Confirmed from source/docs; speedup depends on core count |

## Knowledge Gaps

1. No benchmark data for jxl-oxide specifically with rayon enabled at 8K resolution
2. No timing breakdown between JXL decode and pixel transforms in this specific project
3. Whether the 2.8x libjxl ratio holds for multi-threaded decode (rayon may narrow the gap)
4. Actual throughput achievable from the project's specific JXL files (depends on file content, encoding settings, color space)

---

## Sources

- [quackdoc blog: Jpeg-xl is kinda cool](https://quackdoc.github.io/blog/hidden-jxl-benefits/) — jxl-oxide vs djxl timing benchmark (2.80x), Ryzen 2600
- [OpenBenchmarking.org: JPEG-XL Decoding libjxl Benchmark](https://openbenchmarking.org/test/pts/jpegxl-decode) — 77 MP/s median, 112 MP/s top-end, 377 results
- [GitHub: tirr-c/jxl-oxide](https://github.com/tirr-c/jxl-oxide) — rayon feature, release notes
- [GitHub: tirr-c/jxl-oxide releases](https://github.com/tirr-c/jxl-oxide/releases) — v0.11.2 global rayon pool fix
- [Hacker News: Firefox will consider a Rust JPEG-XL implementation](https://news.ycombinator.com/item?id=41443336) — qualitative performance assessment, single-developer concern
- [GitHub: Cykooz/fast_image_resize](https://github.com/Cykooz/fast_image_resize) — image crate resize 83ms vs 5ms with SSE4.1 (15.6x faster), showing cost of unoptimized image ops
- [libjxl benchmarking docs](https://github.com/libjxl/libjxl/blob/main/doc/benchmarking.md) — decode throughput measurement methodology
- [libjxl/doc/encode_effort.md](https://github.com/libjxl/libjxl/blob/main/doc/encode_effort.md) — effort levels affect encode, not decode
- [The Rust Performance Book: Build Configuration](https://nnethercote.github.io/perf-book/build-configuration.html) — opt-level guidance
- [Cargo Profiles RFC 2282](https://rust-lang.github.io/rfcs/2282-profile-dependencies.html) — profile dependencies, monomorphization caveat
- [wgpu-mipmap crate](https://crates.io/crates/wgpu-mipmap) — GPU mipmap generation for wgpu
