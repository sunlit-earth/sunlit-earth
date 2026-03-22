---
type: research
topic: GitHub Actions CI/CD Pipeline
date: 2026-03-20
---

# GitHub Actions CI/CD Pipeline - Research Summary

## Project Build Requirements

Sunlit Earth is a Windows desktop application rendering a 3D Earth via wgpu + Slint, set as a desktop wallpaper. There are no existing CI workflows or build scripts; this is a greenfield setup.

### Dependencies requiring special CI attention

- **astronomy-engine-bindings 2.1** — C FFI bindings requiring **clang/bindgen at build time**. This is the single biggest CI challenge on Windows (see Windows-Specific Considerations).
- **slint ~1.15** with `unstable-wgpu-28` — pinned version with unstable feature flag; compiled from `ui/main.slint` via `slint-build` in `build.rs`.
- **windows-sys 0.59** — Win32 FFI for wallpaper export, gated behind `#[cfg(windows)]`.
- **wgpu 28** — GPU rendering; tests fall back to software adapter when no hardware GPU is available.

### Test infrastructure

- **GPU integration tests** (`tests/shading.rs`, `tests/render_pipeline.rs`) run real WGSL shaders on the GPU. They share a single device via `LazyLock<Mutex<...>>` because per-test device creation crashes on Windows.
- **Software rendering fallback** — all GPU tests fall back to a CPU software adapter when no hardware GPU is available, so CI runners without GPUs will still pass.
- **Tests are self-contained** — no large assets or pre-rendered files needed. Textures are generated on the fly (1x1 solid colors, procedural grids).
- **Dev dependencies**: `approx 0.5` (float comparisons), `proptest 1` (property-based testing).
- **Coverage**: `cargo llvm-cov --lcov` generates LCOV output. Target: 60-70% overall.

### Cargo configuration relevant to CI

- `unsafe_code = "deny"` — CI must catch any new unscoped unsafe blocks.
- `clippy::all` and `clippy::pedantic` = warn with specific allows.
- `profile.dev.package."*"` — opt-level = 2 (optimizes dependencies even in dev builds).
- `profile.release` — LTO, codegen-units = 1, strip = true (slow to build; for release artifacts only).
- LF line endings enforced globally via `.gitattributes`.

## Recommended Tooling

| Purpose | Tool | Version/Ref |
|---|---|---|
| Checkout | `actions/checkout` | `@v4` |
| Rust toolchain | `dtolnay/rust-toolchain` | `@stable` |
| LLVM/Clang install | `KyleMayes/install-llvm-action` | `@v2`, version `"19"` |
| Cargo caching | `Swatinem/rust-cache` | `@v2` |
| GitHub Release | `softprops/action-gh-release` | `@v2` |
| Coverage upload | `codecov/codecov-action` | `@v4` |

**Do not use** `actions-rs/toolchain` — it was archived and deprecated in October 2023.

## Caching Strategy

Use `Swatinem/rust-cache@v2` with minimal configuration. It automatically caches `~/.cargo` and `target/`, sets `CARGO_INCREMENTAL=0`, prunes stale artifacts, and derives cache keys from `rustc` version, `Cargo.lock`, and `Cargo.toml`.

Key configuration choices:

- **`shared-key: "ci-windows"`** on the clippy and test jobs so they share a single cache, keeping total usage within the 10 GB free tier (expect 2-4 GB for one Windows target).
- **`save-if: "false"`** on the clippy job to prevent it from overwriting the cache with a partial build. Let the test job (which does a full build) be the cache writer.
- **`save-if: github.ref == 'refs/heads/main'`** is an option to prevent feature branches from polluting the shared cache, but not strictly necessary with `shared-key`.
- Toolchain setup (`dtolnay/rust-toolchain`) must run **before** `rust-cache` because the cache key includes the `rustc` version.

Expected improvement: 50-70% reduction in build time on cache-warm runs.

`sccache` (compiler-level caching) is not recommended initially — `rust-cache` alone is sufficient for a single-workspace project. It can be added later if build times become a bottleneck.

## CI Workflow Design

**File**: `.github/workflows/ci.yml`
**Triggers**: push to `main`, pull requests targeting `main`

### Global environment

```yaml
env:
  CARGO_TERM_COLOR: always
  CARGO_INCREMENTAL: 0
  RUSTFLAGS: "-D warnings"
```

`RUSTFLAGS: "-D warnings"` causes the build to fail on any compiler or Clippy warning. `CARGO_INCREMENTAL: 0` is set explicitly even though `rust-cache` also sets it, for clarity.

### Jobs

**1. Format check** (`fmt`) — runs on `ubuntu-latest`

- Requires only `rustfmt` component; no FFI build, no LLVM, no cache needed.
- `cargo fmt --check`

**2. Clippy** (`clippy`) — runs on `windows-latest`

- Runs on Windows because this is a Windows-only app with platform-specific code (`wallpaper.rs`, `windows-sys` calls, wgpu adapter selection).
- Requires LLVM for the astronomy-engine-bindings build.
- Uses shared cache with `save-if: "false"`.
- `cargo clippy --all-targets --locked`

**3. Test** (`test`) — runs on `windows-latest`

- Requires LLVM for the same reason as clippy.
- Uses shared cache; this job writes the cache.
- `cargo test --locked`
- GPU tests will use the software adapter (no hardware GPU on GitHub runners).

**4. Coverage** (optional, can be combined with test or run separately)

- `cargo llvm-cov --locked --lcov --output-path lcov.info`
- Upload via `codecov/codecov-action@v4`

### All cargo commands use `--locked`

This prevents Cargo from silently resolving dependencies differently from the committed `Cargo.lock`, ensuring reproducible builds.

## Release Workflow Design

**File**: `.github/workflows/release.yml`
**Trigger**: push of a semver tag matching `v[0-9]+.[0-9]+.[0-9]+`

### Permissions

```yaml
permissions:
  contents: write
```

Required for `softprops/action-gh-release` to create releases and upload assets using the default `GITHUB_TOKEN`.

### Single job: build and release

1. Checkout, install Rust toolchain, install LLVM 19, set `LIBCLANG_PATH`, warm cache.
2. `cargo build --release --locked` — produces the optimized, LTO-enabled, stripped binary.
3. Package: `7z a sunlit-earth-${VERSION}-x86_64-windows.zip ./target/release/sunlit-earth.exe`
4. Publish via `softprops/action-gh-release@v2` with `generate_release_notes: true` (auto-generates notes from PR titles since previous tag).

If multi-job build/test/release is needed later, use `actions/upload-artifact@v4` to pass the binary between jobs, then `actions/download-artifact@v4` in the release job.

### Matrix builds

Not needed. The app is Windows-only. If a Linux port is added, expand to a matrix over `[ubuntu-latest, windows-latest]`.

## Windows-Specific Considerations

### CRITICAL: LLVM/Clang and bindgen

The `astronomy-engine-bindings` crate uses `bindgen`, which depends on `clang-sys`, which requires `libclang.dll` at build time. This is the most significant CI challenge.

**Problem**: The `windows-latest` runner image has been inconsistent with LLVM versions. Runner images have shipped Clang 16, 18, and 19 at different points. In mid-2025, a documented issue (actions/runner-images #12435) caused build failures where Microsoft's STL headers required Clang 19.0.0+ but the runner provided 18.1, producing `error STL1000: Unexpected compiler version`. The LLVM/Clang that ships as a Visual Studio component is not on `PATH` by default.

**Solution**: Pin LLVM explicitly using `KyleMayes/install-llvm-action@v2`:

```yaml
- name: Install LLVM and Clang
  uses: KyleMayes/install-llvm-action@v2
  with:
    version: "19"

- name: Set LIBCLANG_PATH
  shell: bash
  run: echo "LIBCLANG_PATH=${{ env.LLVM_PATH }}/lib" >> $GITHUB_ENV
```

This downloads prebuilt LLVM 19 binaries, adds them to `PATH`, and sets `LLVM_PATH`. The second step derives `LIBCLANG_PATH` so `clang-sys` can locate `libclang.dll`. This isolates the build from runner image churn entirely.

**Alternative** (fragile, not recommended): locate the MSVC-bundled Clang via `where.exe clang` and add its directory to `PATH`. This breaks whenever the runner image changes its LLVM version.

### Shell behavior

Windows runners default to `pwsh` (PowerShell Core). Use `shell: bash` on steps that need Unix path syntax or standard shell tools. The bash on Windows runners is provided by Git for Windows.

### Toolchain target

The Rust toolchain defaults to `x86_64-pc-windows-msvc`, which is correct. Do not use the `x86_64-pc-windows-gnu` target — it links against MinGW and does not reliably support `windows-sys`.

### GPU availability

GitHub-hosted Windows runners do not have hardware GPUs. All wgpu-based tests fall back to the software adapter automatically via the existing `create_gpu_context()` helper in `tests/common/mod.rs`. No special configuration needed.

## Key Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| LLVM version mismatch on `windows-latest` breaking bindgen | High (has happened) | Build failure | Pin LLVM 19 via `KyleMayes/install-llvm-action@v2` |
| `KyleMayes/install-llvm-action` download URL breaks or version unavailable | Low | Build failure | Pin to a known working LLVM release; monitor action releases |
| GPU tests fail differently on software adapter vs hardware | Medium | False positives/negatives | Tests already assert behavioral invariants (monotonicity, bounds) with EPS = 1e-5 tolerance, not pixel-exact values |
| Per-test GPU device creation crashes on Windows | Known issue | Test crash | Already mitigated: tests use `LazyLock<Mutex<...>>` for device sharing |
| Cache exceeds 10 GB free tier | Low (single platform) | Slower builds or costs | `shared-key` keeps all jobs on one cache; expect 2-4 GB total |
| Slint unstable feature flag breaks on minor version bump | Medium | Build failure | Version pinned to `~1.15`; `Cargo.lock` prevents unexpected updates; `--locked` in CI enforces this |
| Release binary missing runtime assets | Low | Broken release | Textures are in the git repo and bundled at build time; procedural textures are generated at runtime |

## Recommendations

1. **Start with two workflow files**: `ci.yml` (push/PR) and `release.yml` (version tags). Keep them separate and simple.

2. **Always install LLVM 19 explicitly** on Windows jobs. Do not rely on the runner image's pre-installed LLVM. Set `LIBCLANG_PATH` in a follow-up step. This is the single most important action to prevent intermittent CI failures.

3. **Run clippy and test on `windows-latest`**, format checks on `ubuntu-latest`. The app is Windows-only; platform-specific code must be validated on Windows. Format checking has no platform-specific behavior and is faster on Linux.

4. **Use `--locked` on every cargo invocation** to guarantee reproducible builds from the committed `Cargo.lock`.

5. **Use `Swatinem/rust-cache@v2` with `shared-key: "ci-windows"`** across the clippy and test jobs. Set `save-if: "false"` on clippy so only the test job writes the cache.

6. **Set `RUSTFLAGS: "-D warnings"` globally** so any compiler or Clippy warning is a CI failure. This enforces the project's existing pedantic lint configuration.

7. **Add coverage reporting** with `cargo llvm-cov --lcov` and `codecov/codecov-action@v4` to track the 60-70% overall coverage target.

8. **For releases**, use `softprops/action-gh-release@v2` with `generate_release_notes: true` and `permissions: contents: write`. Package the binary as a zip with the version in the filename.

9. **Do not add `cargo nextest` initially.** Standard `cargo test` works correctly with the existing `LazyLock<Mutex<...>>` device-sharing pattern. Nextest's per-test process isolation could actually conflict with the shared GPU device approach. Evaluate later if test suite execution time becomes a concern.

10. **Monitor `KyleMayes/install-llvm-action` releases** for breaking changes. If the action becomes unavailable, the fallback is to install LLVM manually via `choco install llvm --version=19.1.0` or direct download.
