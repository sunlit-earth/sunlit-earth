# Rust Testing Best Practices: External Research (2026-03-15)

This document records external research on Rust testing practices relevant to the
sunlit-earth project. The companion codebase assessment is in
`2026-03-15-test-coverage-codebase.md`.

Sources are cited inline. All searches were conducted in March 2026.

---

## 1. General Rust Testing Best Practices

### 1.1 Unit Test Organization

The canonical Rust approach is a `#[cfg(test)] mod tests { ... }` block inside
each source file. The `cfg(test)` gate means the test code is compiled only when
running `cargo test`, not in release builds, keeping binary size and compile time
down.

For larger files, the community converges on a few conventions:

- Keep the `tests` module at the bottom of the file.
- Split logically into nested sub-modules when the test count grows, e.g.,
  `mod tests { mod edge_cases { ... } mod happy_path { ... } }`.
- Limit individual test files to roughly 100–200 lines; extract helpers into a
  `tests/common/mod.rs` once they are shared by multiple integration tests.
- Private functions are directly accessible from `#[cfg(test)]` modules in the
  same file; this is one of the primary advantages over integration tests.

Source: [The Rust Programming Language, ch11-03](https://doc.rust-lang.org/book/ch11-03-test-organization.html),
[Rust Forum: real world tips for organising unit tests](https://users.rust-lang.org/t/real-world-tips-for-organising-unit-tests-for-larger-projects-and-files/130749)

### 1.2 Integration Test Patterns

Integration tests live in `tests/` at the project root. Each `.rs` file in that
directory compiles as a separate crate and can only access your crate's public API.

Recommended layout:

```
tests/
├── common/
│   └── mod.rs       # shared fixtures and helpers (use tests/common/mod.rs
│                    # not tests/common.rs to avoid Cargo treating it as a test)
├── shading.rs       # GPU shader tests (already exists)
└── <future files>   # one file per integration boundary
```

Sharing state between tests within a file: group related tests in nested
`mod` blocks. For expensive setup (like GPU context initialization), use
`std::sync::LazyLock<Mutex<T>>` — the pattern already used in `tests/shading.rs`.

For tests that must run serially (shared mutable state, specific environment
variables), the `serial_test` crate provides `#[serial]` to prevent parallel
execution of specific tests while letting others run concurrently.

Source: [Integration testing - Rust By Example](https://doc.rust-lang.org/rust-by-example/testing/integration_testing.html),
[How to Test Rust Applications with Integration Tests (2026)](https://oneuptime.com/blog/post/2026-01-26-rust-integration-tests/view)

### 1.3 Test Naming Conventions

Rust convention is snake_case function names that read as sentences describing
what the test verifies, without a `test_` prefix (the `#[test]` attribute is
already the marker). Examples from idiomatic Rust projects:

```rust
#[test]
fn eye_at_zero_longitude_zero_latitude() { ... }  // good: describes precondition and subject
#[test]
fn mvp_is_not_identity() { ... }                  // good: describes invariant
#[test]
fn test_camera_matrix() { ... }                   // avoid: redundant "test_" prefix
```

For parameterized tests, the `test-case` crate provides a `#[test_case(...)]`
attribute that generates named variants without manual repetition:

```rust
#[test_case(0.0, 0.0 ; "origin")]
#[test_case(90.0, 0.0 ; "right")]
fn eye_is_on_unit_sphere(lon: f32, lat: f32) { ... }
```

Source: [Rust Testing Patterns for Reliable Releases (March 2026)](https://dasroot.net/posts/2026/03/rust-testing-patterns-reliable-releases/),
[Complete Guide To Testing Code In Rust](https://zerotomastery.io/blog/complete-guide-to-testing-code-in-rust/)

### 1.4 When to Use Unit Tests vs. Integration Tests

| Situation | Preferred test type |
|---|---|
| Pure function with no I/O or external state | Unit test (`#[cfg(test)]` in same file) |
| Private helper function | Unit test (only option that can access private items) |
| Cross-module interaction through public API | Integration test |
| GPU resource creation, shader execution | Integration test (needs real or software GPU) |
| CLI argument parsing | Integration test on `main.rs` binary |
| Shader logic (WGSL compute) | Integration test with GPU compute harness |
| UI interaction flow | Integration test with `i-slint-backend-testing` |

The rule of thumb: unit tests verify individual functions in isolation; integration
tests verify that components work together correctly.

### 1.5 Mocking Strategies

**Mockall** is the standard mocking library for Rust, recommended in AOSP (Android
Open Source Project) and covered in Comprehensive Rust. It uses procedural macros
to generate mock structs from traits:

```rust
#[automock]
trait TextureSource {
    fn load_rgba(&self, path: &Path) -> Result<Vec<u8>>;
}

// In tests: MockTextureSource is auto-generated
let mut mock = MockTextureSource::new();
mock.expect_load_rgba()
    .returning(|_| Ok(vec![255u8; 256 * 256 * 4]));
```

**Key mockall features:**
- `.expect_<method>()` builder for configuring call counts, arguments, and returns
- Predicate matching (`predicate::eq()`, `predicate::ge()`)
- Call sequencing for testing ordered interactions
- Expectation verification at drop time

**When mockall is not the right tool:**
- When the code under test uses concrete structs rather than traits, mockall
  requires restructuring. The `faux` crate mocks structs without traits.
- When you need HTTP service stubs, `wiremock` is more ergonomic.
- When the interface is simple, hand-written fakes (implementing the trait manually)
  are often clearer than mock macros and have no compile-time overhead.

**Trait-based injection** (designing code around traits rather than concrete types)
is the prerequisite for effective mocking in Rust. If a module takes
`impl TextureSource` rather than `TextureLoader`, the mock can be injected at
the call site. This is the most impactful architectural change for testability.

For sunlit-earth specifically: `wgpu::Device` and `wgpu::Queue` are concrete types
with no trait abstraction, making them inherently hard to mock. The GPU integration
test approach (running on a real or software adapter) is the practical alternative.

Source: [mockall docs.rs](https://docs.rs/mockall/latest/mockall/),
[Mocking in Rust: Mockall and alternatives (LogRocket)](https://blog.logrocket.com/mocking-rust-mockall-alternatives/),
[Rust Mock Shootout](https://asomers.github.io/mock_shootout/)

### 1.6 Property-Based Testing

**Proptest** (version 1.1.3 as of 2025) is the leading property-based testing
library for Rust, inspired by Python's Hypothesis. It generates random inputs based
on explicit `Strategy` objects rather than type derivation (the approach used by
the older `quickcheck`). The key advantage: strategies can encode constraints that
prevent the generator from creating invalid inputs, and shrinking understands the
constraints.

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn shift_horizontal_is_invertible(
        width in 1usize..1024,
        height in 1usize..512,
        shift in 0usize..1024,
    ) {
        let pixels = vec![128u8; width * height * 4];
        let shifted = shift_horizontal(&pixels, width, shift % width);
        let unshifted = shift_horizontal(&shifted, width, width - (shift % width));
        prop_assert_eq!(pixels, unshifted);
    }
}
```

Good candidates for proptest in sunlit-earth:
- `shift_horizontal()`: invertibility, idempotency at shift=0 and shift=width
- `downsample_2x()`: output size formula, all pixels in valid RGBA range
- `build_aa_options()`: returned counts are a subset of input, default index is in
  bounds, labels contain the count as a string

The `proptest-derive` crate adds `#[derive(Arbitrary)]` for automatic strategy
generation on structs and enums.

Source: [Property-based testing in Rust with Proptest (LogRocket)](https://blog.logrocket.com/property-based-testing-in-rust-with-proptest/),
[proptest GitHub](https://github.com/proptest-rs/proptest),
[Rust Testing Patterns (March 2026)](https://dasroot.net/posts/2026/03/rust-testing-patterns-reliable-releases/)

### 1.7 Snapshot Testing

**insta** (with the `cargo-insta` CLI) is the standard snapshot testing library for
Rust. It captures output to a `.snap` file on first run; subsequent runs compare
against the stored snapshot. The `cargo insta review` command opens an interactive
review UI.

```rust
use insta::assert_snapshot;

#[test]
fn grid_texture_debug_output() {
    let tex = generate_grid_texture(64, 32);
    assert_snapshot!(format!("{:?}", &tex[0..32]));
}
```

Insta supports multiple serialization formats (text, JSON, YAML, TOML, CSV) and
redactions (for non-deterministic fields like timestamps or random IDs).

Relevant use cases for sunlit-earth:
- Snapshot the pixel output of `generate_grid_texture()` at a small resolution to
  detect accidental color/layout regressions.
- Snapshot the WGSL shader source (after concatenation) to detect unintended
  changes during refactoring.
- Snapshot the vertex/index buffers of `generate_sphere()` to catch mesh
  generation regressions.

**Limitation**: Snapshot tests need the `.snap` files checked into the repository.
They are most valuable for complex outputs where writing explicit assertions would
be tedious, not for simple scalar checks.

Source: [insta.rs](https://insta.rs/),
[cargo-insta crates.io](https://crates.io/crates/cargo-insta),
[Snapshot Testing - Rust Project Primer](https://www.rustprojectprimer.com/testing/snapshot.html)

### 1.8 Coverage Tools

Three main options exist, with meaningfully different trade-offs:

| Tool | Platform | Method | Branch coverage | Recommendation |
|---|---|---|---|---|
| `cargo-llvm-cov` | Linux, macOS, Windows | LLVM instrumentation | Yes (experimental) | **Recommended** |
| `cargo-tarpaulin` | Linux only | LLVM or ptrace | No | Linux-only CI |
| `grcov` | Cross-platform | LLVM / gcov aggregation | Limited | Multi-run aggregation |

**cargo-llvm-cov** is the officially recommended tool per the Rust Project Primer.
It wraps `rustc`'s `-C instrument-coverage` flag with a developer-friendly CLI:

```bash
cargo install cargo-llvm-cov
cargo llvm-cov                      # summary to stdout
cargo llvm-cov --html               # HTML report in target/llvm-cov/html/
cargo llvm-cov --lcov --output-path lcov.info  # for Codecov/Coveralls
```

It works on Windows (important for this project), supports `cargo-nextest`, and
produces line, region, and branch coverage. The `--no-report` / `cargo llvm-cov report`
split allows merging coverage from multiple test runs (e.g., unit tests + integration
tests with a software GPU).

**cargo-tarpaulin** does not support Windows and has limited macOS support, which
rules it out as the primary tool for this project. Its `--engine llvm` flag makes
it more accurate than the older ptrace engine on Linux.

**grcov** (Mozilla) is useful when aggregating coverage across multiple test
environments or matrix builds, but adds complexity not justified for a single-binary
project at this stage.

Source: [Coverage - Rust Project Primer](https://rustprojectprimer.com/measure/coverage.html),
[cargo-llvm-cov GitHub](https://github.com/taiki-e/cargo-llvm-cov),
[cargo-tarpaulin GitHub](https://github.com/xd009642/tarpaulin)

---

## 2. Testing GPU/wgpu Code

### 2.1 Core Strategy: Render to Texture Without a Window

The windowless render approach removes the dependency on a display surface for
testing. The steps are:

1. Request an adapter with `compatible_surface: None`.
2. Create a texture with `TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC`.
3. Render into that texture.
4. Create an output buffer with `BufferUsages::MAP_READ | BufferUsages::COPY_DST`.
5. Copy texture to buffer via `encoder.copy_texture_to_buffer()`.
6. Map the buffer, read bytes, assert on pixel values or save as PNG.

This is the pattern demonstrated in Learn Wgpu's "Wgpu without a window" tutorial
and is what the existing `tests/shading.rs` uses for compute shaders (steps 3–6 via
storage buffers rather than texture).

Source: [Learn Wgpu: Wgpu without a window](https://sotrh.github.io/learn-wgpu/showcase/windowless/)

### 2.2 Software Adapter Options

**force_fallback_adapter**: Setting `force_fallback_adapter: true` in
`RequestAdapterOptions` forces wgpu to select a software renderer. On Linux this
is typically llvmpipe (Mesa GL) or lavapipe (Mesa Vulkan). On Windows it is WARP
(D3D12). This is the most portable CI strategy.

**Environment variables**: wgpu respects `WGPU_BACKEND` (comma-separated list:
`vulkan`, `metal`, `dx12`, `gl`) and `WGPU_ADAPTER_NAME` to force a specific
adapter by name. Useful in CI to pin to `llvmpipe` or `WARP`.

**The Noop backend**: wgpu now includes a Noop dummy backend
(`wgpu::Backends::NOOP`) that accepts API calls and creates stub resource handles
but performs no computation. This is useful for testing resource management logic
(allocation, binding, lifetime) without any GPU or software renderer at all. It is
not useful for testing shader correctness.

**Practical matrix for CI:**

| Environment | Strategy |
|---|---|
| GitHub Actions Linux | `force_fallback_adapter: true` → llvmpipe via `WGPU_BACKEND=gl` |
| GitHub Actions Windows | `force_fallback_adapter: true` → WARP via `WGPU_BACKEND=dx12` |
| No GPU at all (pure logic) | Noop backend — validates API calls, no shader results |
| Real GPU available | Use `force_fallback_adapter: false`, prefer discrete |

The existing `software_adapter_produces_correct_results` test in `tests/shading.rs`
already implements this pattern correctly for the blend shader.

Source: [wgpu RequestAdapterOptionsBase docs](https://docs.rs/wgpu/latest/wgpu/struct.RequestAdapterOptionsBase.html),
[wgpu Backends docs](https://docs.rs/wgpu/latest/wgpu/struct.Backends.html),
[gfx-rs/wgpu GitHub](https://github.com/gfx-rs/wgpu),
[DeepWiki: Instances and Adapters](https://deepwiki.com/gfx-rs/wgpu/2.1-instances-and-adapters)

### 2.3 Compute Shader Testing Strategy

The pattern used in `tests/shading.rs` is the community best practice: construct
a compute shader that loads test parameters from a storage buffer, calls the
function under test, and writes results to an output buffer. Read back with buffer
mapping and assert in Rust.

Benefits of this approach:
- Tests actual WGSL, not a Rust mirror — eliminates drift between tested and
  production logic.
- Can sweep thousands of parameter combinations in a single dispatch (the
  `never_below_min_across_color_range` test runs 18,036 cases in one GPU dispatch).
- Works on software adapters (llvmpipe, WARP) so CI works without real hardware.

For the render pipeline (vertex + fragment shader), the equivalent is:
1. Create a render pipeline with the production shaders.
2. Render to a texture using a known camera matrix and sun direction.
3. Copy to CPU buffer.
4. Assert on specific pixel values (e.g., center of sphere should be day color
   when sun is directly overhead).

This is more fragile than compute shader testing because the output depends on
rasterization and texture sampling, which vary slightly between GPU implementations.
A tolerance of ±1 per channel (0–3 out of 255) is typically sufficient to allow
for rounding differences between hardware and software renderers.

### 2.4 Shared GPU Context Pattern

Creating a `wgpu::Device` per test thread crashes on Windows (a known wgpu
limitation). The correct pattern — already in use in this project — is:

```rust
static GPU: LazyLock<Mutex<GpuContext>> = LazyLock::new(|| {
    Mutex::new(pollster::block_on(GpuContext::new()))
});
```

All tests lock the mutex, submit GPU work, and release. This serializes GPU
submissions without requiring `--test-threads 1` on the command line, allowing
other (non-GPU) tests to run in parallel.

Source: [CLAUDE.md constraints section],
[tests/shading.rs existing implementation]

### 2.5 Software Renderer Stability in CI

llvmpipe pulled from rolling PPAs (e.g., `ppa:oibaf/graphics-drivers` on Ubuntu)
can introduce unstable behavior when a buggy version is released. Observed issues:
- Validation layer failures unrelated to the code under test.
- Rendering output that differs from stable Mesa.

Recommendation: pin to the Ubuntu LTS mesa package (`mesa-vulkan-drivers` from
the default `apt` repository, not a PPA) or use WARP on Windows CI runners.
GitHub Actions' `ubuntu-24.04` runner includes Mesa llvmpipe by default.

Source: [llvmpipe CI stability issues in wgpu tracker](https://github.com/gfx-rs/wgpu/issues/2594),
[install-vulkan-sdk-action (Lavapipe)](https://github.com/jakoch/install-vulkan-sdk-action)

---

## 3. Testing Desktop UI Applications

### 3.1 Slint's Testing Backend

Slint provides the `i-slint-backend-testing` crate with a dedicated testing
backend. The key functions:

```rust
// For unit tests that don't need an event loop:
slint::testing::init_no_event_loop();

// For integration tests needing the event loop with real time:
slint::testing::init_integration_test_with_system_time();

// For tests with controlled time (animations, timers):
slint::testing::init_integration_test_with_mock_time();
slint::testing::mock_elapsed_time(std::time::Duration::from_millis(500));
```

These must be called before any other Slint initialization and only once per
process. This is a significant constraint: integration tests involving the event
loop must be wrapped in a single `#[test]` function (not split across multiple
`#[test]` functions in the same binary) to avoid the one-initialization-per-process
limit.

**ElementHandle** provides simulated user input:
- `single_click()` — move pointer, press, release, idle
- `double_click()` — two click sequences

Elements are found by accessible label or ID. These are async methods that must be
run via `slint::spawn_local()`.

**Important note**: The crate name `i-slint-backend-testing` is an internal crate
and its public name in user code is accessed via `slint::testing::*` after enabling
the `slint/testing` feature flag. The version must match the pinned Slint version
exactly.

Source: [i-slint-backend-testing docs.rs](https://docs.rs/i-slint-backend-testing/latest/i_slint_backend_testing/)

### 3.2 What Slint Testing Can and Cannot Verify

**Can test:**
- Callback invocations (do slider changes invoke the right callback?)
- Property updates (does setting `longitude` change the displayed value?)
- Timer and animation advancement via `mock_elapsed_time()`
- Element visibility and enabled state

**Cannot test (or very difficult):**
- Actual pixel rendering (Slint testing backend does not render to a framebuffer)
- wgpu rendering output within the Slint window (the custom rendering notifier
  is bypassed in the testing backend)
- Platform-specific window chrome (title bars, borders)

For pixel-level verification of the rendered Earth, the GPU render-to-texture
approach (section 2.1 above) is needed independently of Slint.

### 3.3 Architecture for Testability: Separating UI Logic from Rendering

The most practical pattern for making a Slint + wgpu application testable is to
extract all logic that does not require a live GPU or event loop into pure functions
or structs with no wgpu/Slint dependencies. This aligns with MVVM:

**Model** (pure, no wgpu/Slint, fully unit-testable):
- Camera matrix computation — already done in `camera.rs`
- Sun direction computation — already done in `sun.rs`
- Sphere mesh generation — already done in `sphere.rs`
- Texture pixel generation — `grid_texture.rs` is nearly there
- `build_aa_options()`, `downsample_2x()`, `shift_horizontal()` in `renderer.rs`

**ViewModel** (logic that depends on state but not on GPU or UI objects):
- Dirty-checking logic (compare old vs new frame state)
- MSAA sample count selection
- Texture directory resolution

**View** (GPU pipeline, Slint window — integration-tested only):
- `renderer.rs` rendering callback
- `wgpu_init.rs` device creation
- `main.rs` event handling

The key insight: if `build_aa_options()`, `downsample_2x()`, and
`dirty_check_frame_state()` were free functions (not closures inside the rendering
callback), they could be tested with zero GPU involvement.

Source: [Desktop app MVVM testability patterns](https://www.spaceteams.de/en/insights/mvvm-as-a-complementary-pattern-for-clean-architecture-applications/),
[Slint testing.md](https://github.com/slint-ui/slint/blob/master/docs/testing.md)

### 3.4 Visual Regression / Screenshot Testing

For a rendering application, screenshot comparison is the highest-value integration
test. The general approach in Rust:

1. Render to a texture using the windowless pipeline.
2. Read pixel data back to a `Vec<u8>`.
3. Compare against a stored reference image (golden file).
4. On mismatch, save `expected.png`, `actual.png`, and `diff.png`.

The `image` crate handles PNG encoding/decoding. A diff image can be produced by
`ImageBuffer::from_fn()` marking mismatched pixels.

**Key challenges:**
- GPU implementation variance: the same shader can produce ±1 per channel
  differences between hardware GPU, llvmpipe, and WARP. A per-pixel threshold
  (e.g., max difference ≤ 2 per channel, and less than 0.1% of pixels differ) is
  more robust than byte-exact comparison.
- Platform variance: screenshot tests that pass on Windows WARP may fail on Linux
  llvmpipe. Run golden image generation and comparison on the same backend.

For sunlit-earth, the most useful screenshots to snapshot would be:
- Earth at a known camera position with sun overhead → day side fully lit
- Earth with sun behind → night side fully lit
- Earth at terminator with known `terminator_width` → gradient visible

Source: [Screenshot testing with Rust (Tony Finn)](https://tonyfinn.com/blog/rust-screenshot-testing/)

---

## 4. Testing FFI Code

### 4.1 Safe Wrapper Testing Strategy

The standard practice is to concentrate all `unsafe` FFI calls inside a thin,
auditable wrapper module. The `sun.rs` module already follows this pattern: a
`#[allow(unsafe_code)]` annotation on individual call sites, with a safe public
API (`sun_direction()`, `sun_direction_now()`) that hides the FFI entirely.

Testing strategy for safe wrappers:
- Test the safe public API with known inputs and expected outputs (already done
  in `sun.rs` with astronomical reference dates).
- Test invariants that must hold regardless of the C library's internals (the
  `unit_vector` test in `sun.rs` is a good example).
- Test error paths if the C library can return error codes (the astronomy engine
  returns a status; `sun.rs` asserts on it but does not test the error case).

### 4.2 Miri for Unsafe Code

Miri is an interpreter for Rust MIR that detects undefined behavior (use-after-free,
out-of-bounds access, uninitialized reads, data races):

```bash
rustup component add miri
cargo miri test
```

**Critical limitation**: Miri cannot execute code behind FFI boundaries. It stops
at the first FFI call. For `sun.rs`, this means Miri can verify the Rust wrapper
code but cannot verify the behavior of `astronomy_engine.c`.

### 4.3 Sanitizers for FFI Code

For mixed Rust/C code, AddressSanitizer (ASan) and ThreadSanitizer (TSan) are the
appropriate tools. They require the C library to be compiled with the sanitizer
flags:

```bash
# nightly only
RUSTFLAGS="-Z sanitizer=address" cargo +nightly test --target x86_64-unknown-linux-gnu
```

For the astronomy-engine C bindings: if `build.rs` compiles the C source, it can
pass `-fsanitize=address` to the C compiler in test builds via a `cfg(test)` check.

Sanitizers require:
- Linux or macOS (no Windows support for most sanitizers)
- Nightly Rust toolchain
- The target must be a native target (not cross-compiled)

Recent research (SafeFFI, 2025) shows ASan + TSan together catch 9 of 15 known
FFI-related UB classes. They are not a complete solution but are the best available
automated tool for production FFI code.

Source: [Making Unsafe Rust a Little Safer (Colin Breck)](https://blog.colinbreck.com/making-unsafe-rust-a-little-safer-tools-for-verifying-unsafe-code/),
[SafeFFI paper (arXiv 2025)](https://arxiv.org/html/2510.20688v1),
[Sanitizer - The Rust Unstable Book](https://doc.rust-lang.org/beta/unstable-book/compiler-flags/sanitizer.html)

### 4.4 Bindgen and Struct Layout Safety

The astronomy-engine bindings use `bindgen` to auto-generate FFI bindings. Bindgen
eliminates the largest class of FFI bugs: mismatched struct layout. The `#[repr(C)]`
on generated structs and the use of sized integer types (`u32`, `i64`) rather than
`int`/`long` (which are platform-width-dependent) are the key correctness invariants.

The compile-time `const _: () = assert!(size_of::<Uniforms>() == 96)` pattern
used in `renderer.rs` for the GPU uniform buffer is directly analogous and equally
important for FFI structs. Bindgen generates this automatically, but handwritten
FFI structs should add it manually.

Source: [Item 34: Control What Crosses FFI Boundaries (Effective Rust)](https://effective-rust.com/ffi.html)

---

## 5. Coverage Standards and CI Setup

### 5.1 What Coverage Level Is Reasonable

For a desktop rendering application, the coverage target must account for the
inherent untestability of the GPU pipeline and UI event loop. A tiered approach:

| Layer | Target | Rationale |
|---|---|---|
| Pure functions (camera, sphere, grid, math helpers) | 90–100% | No barriers to testing; regressions are silent |
| Business logic (sun, dirty-checking, AA options, shift) | 80–90% | Some setup cost but high value |
| GPU pipeline (renderer.rs core loop) | 20–40% | Only GPU integration tests are practical |
| Main / event handlers | <20% | Inherently UI-driven, test manually |

An **overall target of 60–70%** is realistic for a project of this type. Enforcing
80%+ (the standard for a pure library crate) against a binary with significant GPU
and UI surface area will produce coverage inflation through superficial tests rather
than meaningful quality improvement.

The Rust Project Primer distinguishes: "Library crates should generally aim for
high test coverage, ideally approaching 100%. Binary crates may present greater
challenges, particularly when dependencies have limited testability."

Source: [Coverage - Rust Project Primer](https://rustprojectprimer.com/measure/coverage.html),
[Rust Testing Patterns for Reliable Releases (March 2026)](https://dasroot.net/posts/2026/03/rust-testing-patterns-reliable-releases/)

### 5.2 Which Coverage Metrics Matter

**Line coverage** is the most commonly reported metric and the baseline for any
coverage tool. It is the minimum useful metric.

**Branch coverage** (do both true and false branches of each `if` get exercised?)
is more meaningful for catching logic bugs. cargo-llvm-cov supports it
experimentally:

```bash
cargo llvm-cov --branch
```

Branch coverage is especially valuable for the dirty-checking logic in
`renderer.rs`, where many `if old != new` conditions exist. Line coverage alone
could show 100% if the condition is always true in tests.

**Function coverage** (every function is called at least once) is the weakest
metric — a single call exercises all lines of a one-branch function — but it
catches dead code.

For sunlit-earth, the priority order is: branch coverage on business logic >
line coverage on pure functions > function coverage everywhere.

### 5.3 CI Coverage Setup

Recommended GitHub Actions configuration for coverage:

```yaml
- name: Install cargo-llvm-cov
  uses: taiki-e/install-action@cargo-llvm-cov

# Run unit tests (no GPU needed)
- name: Coverage (unit tests)
  run: cargo llvm-cov --no-report --lib

# Run GPU integration tests with software adapter
- name: Coverage (integration tests)
  env:
    WGPU_BACKEND: gl          # Linux: use llvmpipe via GL
    # WGPU_BACKEND: dx12      # Windows: use WARP
  run: cargo llvm-cov --no-report --test shading

# Merge and export
- name: Generate coverage report
  run: cargo llvm-cov report --lcov --output-path lcov.info

- name: Upload to Codecov
  uses: codecov/codecov-action@v4
  with:
    files: lcov.info
```

The `--no-report` + `report` split allows merging coverage from test runs that
need different environment variables (e.g., GPU tests with `WGPU_BACKEND=gl` and
unit tests without).

Source: [cargo-llvm-cov README](https://github.com/taiki-e/cargo-llvm-cov)

---

## 6. Additional Tools Worth Knowing

### 6.1 cargo-nextest

cargo-nextest is a faster test runner that runs each test in a separate process,
achieving up to 3× speedup on large test suites. Key advantages:

- Per-process isolation: a panic in one test does not affect others.
- Parallel test target execution (unit + integration tests run concurrently by default).
- XML/JSON output for CI test reporting.
- Flaky test detection.

**For this project**: cargo-nextest is directly compatible with the existing
`LazyLock<Mutex<GpuContext>>` pattern in `tests/shading.rs` — each test in the
file gets its own process, so the static is freshly initialized in each process.
This is actually safer than `cargo test --test-threads=1` for GPU tests.

**Limitation**: Doctests are not supported; run `cargo test --doc` separately.

Installation: `cargo install cargo-nextest`
Usage: `cargo nextest run` (drop-in replacement for `cargo test`)

Source: [cargo-nextest home](https://nexte.st/),
[Why process-per-test?](https://nexte.st/docs/design/why-process-per-test/)

### 6.2 cargo-mutants

cargo-mutants is a mutation testing tool: it modifies the source code (flips
comparisons, removes return values, changes operators) and checks whether any test
catches the change. Tests that do not detect mutations are weak.

Most useful for validating test quality on pure functions before spending time on
coverage tools. For sunlit-earth, running it against `camera.rs`, `sphere.rs`, and
`sun.rs` would identify which test assertions are meaningful vs. decorative.

```bash
cargo install cargo-mutants
cargo mutants --file src/camera.rs
```

Source: [cargo-mutants](https://mutants.rs/)

---

## 7. Summary Recommendations for Sunlit-Earth

In priority order based on impact vs. effort:

1. **Add `cargo-llvm-cov` to CI** (low effort, immediate visibility into gaps).
   Use `--lcov` output and upload to Codecov or display in PR summaries.

2. **Test the three pure functions with no barriers** (`build_aa_options`,
   `downsample_2x`, `shift_horizontal`). These are in `renderer.rs` and
   `texture_loader.rs`. Add as `#[cfg(test)]` unit tests in the same files.
   High value, zero setup cost.

3. **Add `proptest` for `shift_horizontal` and `downsample_2x`**. Invertibility
   and size invariants are natural properties. Add `proptest` to `[dev-dependencies]`.

4. **Extract renderer dirty-check logic** into a testable pure function
   (`fn frame_state_changed(old: &FrameState, new: &FrameState) -> bool`). Then
   test it comprehensively with unit tests covering each field.

5. **Add a render pipeline integration test** (`tests/render.rs`). Render the Earth
   at a fixed camera position and sun direction using the software adapter, copy
   pixels back, and assert that the center pixel is within expected color range.
   This validates the full vertex + fragment shader pipeline including
   `sphere.wgsl`.

6. **Add `insta` snapshots for `generate_grid_texture`** at 64×32 resolution.
   Catches grid spacing, color, and boundary regressions with no ongoing maintenance
   overhead once the initial snapshot is accepted.

7. **Test `wgpu_init::select_adapter`** by extracting the ranking function and
   testing it with a hand-crafted slice of `AdapterInfo` structs. This validates
   the discrete > integrated > CPU preference without needing a real GPU.

8. **Slint testing backend**: Add `slint/testing` to `[dev-dependencies]` features
   and write one test that constructs the main window, fires the longitude slider
   callback, and verifies the `dirty` flag is set. Validates the UI→render callback
   wiring without a real render.

---

## Sources

- [The Rust Programming Language, ch11-03: Test Organization](https://doc.rust-lang.org/book/ch11-03-test-organization.html)
- [Integration testing - Rust By Example](https://doc.rust-lang.org/rust-by-example/testing/integration_testing.html)
- [Rust Forum: real world tips for organising unit tests for larger projects](https://users.rust-lang.org/t/real-world-tips-for-organising-unit-tests-for-larger-projects-and-files/130749)
- [How to Test Rust Applications with Integration Tests (Jan 2026)](https://oneuptime.com/blog/post/2026-01-26-rust-integration-tests/view)
- [Rust Testing Patterns for Reliable Releases (Mar 2026)](https://dasroot.net/posts/2026/03/rust-testing-patterns-reliable-releases/)
- [Complete Guide To Testing Code In Rust — Zero To Mastery](https://zerotomastery.io/blog/complete-guide-to-testing-code-in-rust/)
- [mockall docs.rs](https://docs.rs/mockall/latest/mockall/)
- [mockall GitHub](https://github.com/asomers/mockall)
- [Mocking in Rust: Mockall and alternatives — LogRocket](https://blog.logrocket.com/mocking-rust-mockall-alternatives/)
- [Rust Mock Shootout](https://asomers.github.io/mock_shootout/)
- [Mocking — Comprehensive Rust (Google)](https://google.github.io/comprehensive-rust/android/testing/mocking.html)
- [Property-based testing in Rust with Proptest — LogRocket](https://blog.logrocket.com/property-based-testing-in-rust-with-proptest/)
- [proptest GitHub](https://github.com/proptest-rs/proptest)
- [insta.rs snapshot testing](https://insta.rs/)
- [cargo-insta crates.io](https://crates.io/crates/cargo-insta)
- [Snapshot Testing — Rust Project Primer](https://www.rustprojectprimer.com/testing/snapshot.html)
- [Coverage — Rust Project Primer](https://rustprojectprimer.com/measure/coverage.html)
- [cargo-llvm-cov GitHub](https://github.com/taiki-e/cargo-llvm-cov)
- [cargo-tarpaulin GitHub](https://github.com/xd009642/tarpaulin)
- [grcov GitHub](https://github.com/mozilla/grcov)
- [Learn Wgpu: Wgpu without a window](https://sotrh.github.io/learn-wgpu/showcase/windowless/)
- [wgpu RequestAdapterOptionsBase docs](https://docs.rs/wgpu/latest/wgpu/struct.RequestAdapterOptionsBase.html)
- [wgpu Backends docs](https://docs.rs/wgpu/latest/wgpu/struct.Backends.html)
- [gfx-rs/wgpu GitHub](https://github.com/gfx-rs/wgpu)
- [DeepWiki: wgpu Instances and Adapters](https://deepwiki.com/gfx-rs/wgpu/2.1-instances-and-adapters)
- [wgpu llvmpipe CI stability issue #2594](https://github.com/gfx-rs/wgpu/issues/2594)
- [jakoch/install-vulkan-sdk-action (Lavapipe for CI)](https://github.com/jakoch/install-vulkan-sdk-action)
- [i-slint-backend-testing docs.rs](https://docs.rs/i-slint-backend-testing/latest/i_slint_backend_testing/)
- [slint/testing.md on GitHub](https://github.com/slint-ui/slint/blob/master/docs/testing.md)
- [Screenshot testing with Rust — Tony Finn](https://tonyfinn.com/blog/rust-screenshot-testing/)
- [Making Unsafe Rust a Little Safer — Colin Breck](https://blog.colinbreck.com/making-unsafe-rust-a-little-safer-tools-for-verifying-unsafe-code/)
- [SafeFFI: Efficient Sanitization at FFI Boundaries (arXiv 2025)](https://arxiv.org/html/2510.20688v1)
- [Sanitizer — The Rust Unstable Book](https://doc.rust-lang.org/beta/unstable-book/compiler-flags/sanitizer.html)
- [Item 34: Control What Crosses FFI Boundaries — Effective Rust](https://effective-rust.com/ffi.html)
- [cargo-nextest home](https://nexte.st/)
- [Why process-per-test? — cargo-nextest](https://nexte.st/docs/design/why-process-per-test/)
- [cargo-mutants](https://mutants.rs/)
