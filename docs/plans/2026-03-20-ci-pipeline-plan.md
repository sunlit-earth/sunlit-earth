# Plan: GitHub Actions CI/CD Pipeline (2026-03-20)

## Summary

Add two GitHub Actions workflow files to automate the build, lint, test, and release process for Sunlit Earth. The CI workflow (`ci.yml`) runs on every push and pull request, with a fast format check on Ubuntu and Clippy + test jobs on Windows. The release workflow (`release.yml`) builds an optimized Windows binary and publishes it as a GitHub Release when a version tag is pushed. Both workflows pin LLVM 19 explicitly on Windows jobs to work around known runner-image Clang version instability, and use `Swatinem/rust-cache` with a shared cache key to keep Rust build times manageable within the 10 GB free cache tier.

## Stakes Classification

**Level**: Medium
**Rationale**: The change creates two new files (`.github/workflows/ci.yml` and `.github/workflows/release.yml`) and does not modify any existing source code. There is no risk of breaking existing functionality. However, the Windows + LLVM + bindgen interaction is a known source of CI failures (documented in the research as the single biggest challenge), and getting the release packaging right requires careful attention to artifact naming and permissions. Rollback is trivial: delete the workflow files.

## Context

**Research**: [`docs/plans/2026-03-20-ci-pipeline-research.md`](2026-03-20-ci-pipeline-research.md)
**Supporting documents**: [`docs/plans/2026-03-20-ci-pipeline-codebase.md`](2026-03-20-ci-pipeline-codebase.md), [`docs/plans/2026-03-20-ci-pipeline-external.md`](2026-03-20-ci-pipeline-external.md)
**Affected Areas**:

- `.github/workflows/ci.yml` -- new file
- `.github/workflows/release.yml` -- new file

No existing source files are modified.

## Success Criteria

- [ ] `cargo fmt --check` runs on Ubuntu and fails the workflow on formatting violations
- [ ] `cargo clippy --all-targets --locked` runs on Windows with LLVM 19 and fails on any warning
- [ ] `cargo test --locked` runs on Windows with LLVM 19 and reports pass/fail accurately
- [ ] GPU integration tests pass on the GitHub runner using the software adapter (no hardware GPU)
- [ ] Clippy and test jobs share a single Rust cache via `shared-key`, staying within 10 GB
- [ ] Only the test job writes the cache (clippy uses `save-if: "false"`)
- [ ] A push to `main` or a PR targeting `main` triggers the CI workflow
- [ ] Pushing a `v*` tag (e.g., `v0.1.0`) triggers the release workflow
- [ ] The release workflow produces a zip file containing the optimized binary
- [ ] A GitHub Release is created with auto-generated release notes and the zip attached
- [ ] `RUSTFLAGS: "-D warnings"` is set globally so any compiler/Clippy warning is a CI failure
- [ ] All `cargo` commands use `--locked` for reproducible builds
- [ ] The LLVM installation is explicit (not relying on runner-image defaults)

## Implementation Steps

### Phase 1: Create Directory Structure

#### Step 1.1: Create `.github/workflows/` directory

- **Files**: `.github/workflows/` (new directory)
- **Action**: Create the directory that GitHub Actions expects for workflow definitions.
- **Verify**: Directory exists at `.github/workflows/`
- **Complexity**: Small

### Phase 2: CI Workflow *(parallel with Phase 3)*

#### Step 2.1: Create the CI workflow file with global configuration

- **Files**: `.github/workflows/ci.yml`
- **Action**: Create the CI workflow file with the following top-level structure:

  Name: `CI`

  Triggers:
  - `push` to `main` branch
  - `pull_request` targeting `main` branch

  Global environment variables:
  - `CARGO_TERM_COLOR: always` -- colored cargo output in logs
  - `CARGO_INCREMENTAL: 0` -- disable incremental compilation (wasted in CI, bloats cache)
  - `RUSTFLAGS: "-D warnings"` -- promote all warnings to errors

  Define three jobs: `fmt`, `clippy`, `test` (detailed in subsequent steps).

- **Verify**: YAML syntax is valid (no tabs, correct indentation)
- **Complexity**: Small

#### Step 2.2: Add the `fmt` job (Ubuntu)

- **Files**: `.github/workflows/ci.yml`
- **Action**: Add the `fmt` job within the CI workflow. This job validates code formatting on Ubuntu, which is the cheapest and fastest runner. No compilation or FFI dependencies are needed -- `cargo fmt --check` performs text comparison only.

  Job configuration:
  - `runs-on: ubuntu-latest`
  - Steps:
    1. `actions/checkout@v4`
    2. `dtolnay/rust-toolchain@stable` with `components: rustfmt`
    3. `cargo fmt --check` (shell: bash)

  No caching is needed -- this job does not compile anything.

- **Test cases** (manual, via CI run):
  - Push a commit with correctly formatted code -> `fmt` job passes
  - Push a commit with a deliberate formatting violation (e.g., extra spaces) -> `fmt` job fails with a diff showing the violation
- **Verify**: Job appears in GitHub Actions UI when triggered; passes on well-formatted code
- **Complexity**: Small

#### Step 2.3: Add the `clippy` job (Windows)

- **Files**: `.github/workflows/ci.yml`
- **Action**: Add the `clippy` job within the CI workflow. This job runs the Clippy linter on Windows because the project has platform-specific code (`wallpaper.rs`, `windows-sys` FFI) that can only be linted on Windows. It requires LLVM 19 for the `astronomy-engine-bindings` crate's bindgen step.

  Job configuration:
  - `runs-on: windows-latest`
  - Steps:
    1. `actions/checkout@v4`
    2. `dtolnay/rust-toolchain@stable` with `components: clippy`
    3. `KyleMayes/install-llvm-action@v2` with `version: "19"`
    4. Set `LIBCLANG_PATH` -- `echo "LIBCLANG_PATH=${{ env.LLVM_PATH }}/lib" >> $GITHUB_ENV` (shell: bash)
    5. `Swatinem/rust-cache@v2` with `shared-key: ci-windows` and `save-if: "false"` (reads cache but does not write, so it cannot overwrite the full build cache from the test job with a partial clippy-only build)
    6. `cargo clippy --all-targets --locked` (shell: bash)

  Key design decisions:
  - `--all-targets` lints tests, benches, and examples in addition to the library and binary
  - `--locked` ensures `Cargo.lock` is respected exactly
  - `save-if: "false"` prevents the clippy job from polluting the shared cache with a check-only build
  - Toolchain step runs before rust-cache because the cache key includes the `rustc` version

- **Test cases** (manual, via CI run):
  - Push code with no Clippy warnings -> `clippy` job passes
  - Push code with a Clippy warning (e.g., `let x = x;` redundant binding) -> `clippy` job fails because `RUSTFLAGS: "-D warnings"` promotes warnings to errors
  - Push code with a `windows-sys` API call that Clippy flags -> caught on Windows runner but would be missed on Ubuntu
- **Verify**: Job passes on the current codebase; LLVM installs without errors; cache is restored but not saved
- **Complexity**: Medium

#### Step 2.4: Add the `test` job (Windows)

- **Files**: `.github/workflows/ci.yml`
- **Action**: Add the `test` job within the CI workflow. This job compiles and runs the full test suite on Windows, including GPU integration tests that fall back to the software adapter. It shares a cache with the clippy job and is the sole cache writer.

  Job configuration:
  - `runs-on: windows-latest`
  - Steps:
    1. `actions/checkout@v4`
    2. `dtolnay/rust-toolchain@stable`
    3. `KyleMayes/install-llvm-action@v2` with `version: "19"`
    4. Set `LIBCLANG_PATH` -- `echo "LIBCLANG_PATH=${{ env.LLVM_PATH }}/lib" >> $GITHUB_ENV` (shell: bash)
    5. `Swatinem/rust-cache@v2` with `shared-key: ci-windows` (no `save-if` restriction -- this job writes the cache)
    6. `cargo test --locked` (shell: bash)

  Key design decisions:
  - GPU integration tests (`tests/shading.rs`, `tests/render_pipeline.rs`) will use the wgpu software adapter since GitHub runners lack hardware GPUs. The existing `create_gpu_context()` helper handles this fallback automatically.
  - This job is the cache writer because it performs a full `cargo test` build (compiling both library and test targets), producing the most complete `target/` directory. The clippy job only does a check build, so its `target/` would be less useful as a cache seed.
  - No `--no-fail-fast` flag: if one test fails, the job stops early. This is acceptable for a small test suite and produces clearer error messages.

- **Test cases** (manual, via CI run):
  - Push code where all tests pass -> `test` job passes, cache is saved
  - Push code where a unit test fails -> `test` job fails with the test failure output
  - Push code where a GPU integration test fails -> `test` job fails, confirming software adapter tests are running
- **Verify**: All existing tests pass on the GitHub runner; cache is saved on success
- **Complexity**: Medium

### Phase 3: Release Workflow *(parallel with Phase 2)*

#### Step 3.1: Create the release workflow file

- **Files**: `.github/workflows/release.yml`
- **Action**: Create the release workflow file with the following structure:

  Name: `Release`

  Triggers:
  - `push` with tag filter `v[0-9]+.[0-9]+.[0-9]+` (semver tags like `v0.1.0`)

  Permissions:
  - `contents: write` (required for `softprops/action-gh-release` to create releases and upload assets using the default `GITHUB_TOKEN`)

  Global environment variables:
  - `CARGO_TERM_COLOR: always`
  - `CARGO_INCREMENTAL: 0`

  Single job: `release`
  - `runs-on: windows-latest`
  - Steps:
    1. `actions/checkout@v4`
    2. `dtolnay/rust-toolchain@stable`
    3. `KyleMayes/install-llvm-action@v2` with `version: "19"`
    4. Set `LIBCLANG_PATH` -- `echo "LIBCLANG_PATH=${{ env.LLVM_PATH }}/lib" >> $GITHUB_ENV` (shell: bash)
    5. `Swatinem/rust-cache@v2` with `shared-key: release-windows` (separate cache key from CI to avoid interference between CI and release builds, since release profile produces different artifacts)
    6. `cargo build --release --locked` (shell: bash)
    7. Package the binary -- extract the version from the tag reference (`${{ github.ref_name }}`), create a zip archive:
       ```
       7z a sunlit-earth-${{ github.ref_name }}-x86_64-windows.zip ./target/release/sunlit-earth.exe
       ```
       (shell: bash)
    8. `softprops/action-gh-release@v2` with:
       - `files: sunlit-earth-${{ github.ref_name }}-x86_64-windows.zip`
       - `generate_release_notes: true` (auto-generates notes from PR titles since previous tag)

  Key design decisions:
  - Single job (not multi-job build + release) because there is only one target platform. Multi-job with artifact passing adds complexity for no benefit.
  - `RUSTFLAGS: "-D warnings"` is intentionally omitted from the release workflow. The CI workflow catches warnings; the release workflow should not fail on a warning that was already present when the tag was created. This avoids a scenario where a new Clippy lint added in a Rust update breaks a release of code that was previously green.
  - Separate `shared-key: release-windows` from the CI cache because release builds use LTO and `codegen-units = 1`, producing a fundamentally different `target/` layout that would not benefit CI cache consumers.
  - The zip filename includes the tag name (e.g., `sunlit-earth-v0.1.0-x86_64-windows.zip`) for clarity when multiple releases exist.

- **Test cases** (manual):
  - Push a `v0.1.0` tag -> release workflow triggers, builds, packages, creates GitHub Release with the zip attached and auto-generated notes
  - Push a non-matching tag (e.g., `test-123`) -> release workflow does not trigger
  - Push a branch (not a tag) -> release workflow does not trigger
- **Verify**: Release workflow triggers on a test tag; GitHub Release page shows the zip asset and release notes
- **Complexity**: Medium

### Phase 4: Validation

#### Step 4.1: Verify CI workflow on a test push

- **Files**: N/A (manual verification)
- **Action**: Push the branch containing the new workflow files and open a PR to trigger the CI workflow. Verify all three jobs execute correctly.
- **Manual test cases**:
  - [ ] `fmt` job runs on Ubuntu and passes (the codebase is currently well-formatted)
  - [ ] `clippy` job runs on Windows, installs LLVM 19, restores or creates cache, passes with no warnings
  - [ ] `test` job runs on Windows, installs LLVM 19, runs all tests (including GPU integration tests on software adapter), passes
  - [ ] Cache is saved by the test job (visible in the job logs as "Post Swatinem/rust-cache" saving step)
  - [ ] Clippy job does NOT save the cache (logs show "not saving cache" or "save-if evaluated to false")
  - [ ] A second push to the same PR shows faster build times (cache hit)
  - [ ] The workflow summary shows all three jobs green
- **Verify**: All three CI jobs pass on the PR
- **Complexity**: Small

#### Step 4.2: Verify release workflow on a test tag

- **Files**: N/A (manual verification)
- **Action**: Create and push a test tag (e.g., `v0.0.1-rc.1` or the actual first release tag) to trigger the release workflow. Verify the binary is built, packaged, and published.
- **Manual test cases**:
  - [ ] Release workflow triggers on tag push
  - [ ] LLVM 19 installs successfully
  - [ ] `cargo build --release --locked` completes without errors
  - [ ] The zip file is created with the correct name (e.g., `sunlit-earth-v0.0.1-rc.1-x86_64-windows.zip`)
  - [ ] A GitHub Release is created on the repository's Releases page
  - [ ] The zip file is attached as a release asset
  - [ ] Release notes are auto-generated from PR titles
  - [ ] Downloading and extracting the zip produces a working `sunlit-earth.exe`
- **Verify**: GitHub Release page shows the release with the attached zip
- **Complexity**: Small

#### Step 4.3: Update CLAUDE.md

- **Files**: `CLAUDE.md`
- **Action**: Add a brief section documenting the CI/CD setup. Include:
  - The two workflow files and their triggers
  - That LLVM 19 is pinned explicitly on Windows jobs
  - That `--locked` is required on all cargo commands
  - That GPU tests use the software adapter on CI runners
- **Verify**: CLAUDE.md accurately describes the CI/CD setup
- **Complexity**: Small

## Test Strategy

### Automated Tests

No new automated tests are needed. The workflow files themselves are the test infrastructure -- they run the project's existing test suite in CI. The correctness of the workflow files is validated by their successful execution on GitHub Actions.

### Manual Verification

- [ ] Push to `main` triggers the CI workflow with three jobs (fmt, clippy, test)
- [ ] PR targeting `main` triggers the CI workflow
- [ ] `fmt` job runs on Ubuntu, completes in under 1 minute, catches formatting violations
- [ ] `clippy` job runs on Windows with LLVM 19, uses shared cache, does not save cache
- [ ] `test` job runs on Windows with LLVM 19, runs all tests (including GPU integration), saves shared cache
- [ ] Second CI run on same branch is faster (cache warm)
- [ ] `v*` tag push triggers the release workflow
- [ ] Non-matching tags and branch pushes do not trigger the release workflow
- [ ] Release workflow builds an optimized binary with LTO
- [ ] Release workflow creates a GitHub Release with a zip artifact and auto-generated notes
- [ ] The zip contains a working `sunlit-earth.exe`
- [ ] `RUSTFLAGS: "-D warnings"` causes CI to fail on any warning

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| LLVM 19 download URL breaks in `KyleMayes/install-llvm-action` | CI and release workflows fail at LLVM install step | Pin to action version `@v2` (stable). Fallback: install via `choco install llvm --version=19.1.0` or direct download from LLVM releases page. Monitor action repository for breaking changes. |
| `windows-latest` runner image changes break build (independent of LLVM) | Build failure from MSVC or system library changes | The `--locked` flag and pinned LLVM isolate from most runner changes. If MSVC headers break bindgen, update to a newer LLVM version. |
| GPU integration tests behave differently on software adapter | False test failures in CI that pass locally | Tests already assert behavioral invariants (monotonicity, bounds) with tolerance, not pixel-exact values. This is documented in `CLAUDE.md` and the test conventions. |
| Shared cache exceeds 10 GB GitHub Actions free tier | Builds become slower as cache eviction occurs | Single platform (Windows) with `shared-key` keeps cache to 2-4 GB. Monitor via GitHub Actions cache usage page. |
| Release workflow creates a broken binary | Users download a non-functional executable | The CI workflow validates the codebase on every push to `main`. Tags should only be created from green `main` commits. Consider adding a CI gate to the release workflow in the future (run tests before building the release). |
| `generate_release_notes: true` produces low-quality notes | Release page has unhelpful descriptions | Auto-generated notes from PR titles are a reasonable starting point. Release notes can be manually edited after creation. |
| `7z` not available on the Windows runner | Packaging step fails | `7z` is pre-installed on all GitHub-hosted Windows runners (provided by 7-Zip). This has been stable for years. |
| CI workflow runs on all pushes to `main`, including documentation-only changes | Wasted compute minutes on changes that cannot affect the build | Acceptable for a small project. If CI minutes become a concern, add `paths-ignore` for `docs/**`, `*.md`, and `LICENSE`. Not worth the complexity initially. |

## Rollback Strategy

All changes are in new files (`.github/workflows/ci.yml` and `.github/workflows/release.yml`). No existing source code is modified. To rollback:

1. Delete both workflow files
2. Push the deletion to `main`
3. GitHub Actions stops running immediately -- there is no persistent state to clean up
4. Optionally delete the GitHub Actions cache via the repository Settings > Actions > Caches page

## Technical Notes

### Why fmt runs on Ubuntu but clippy/test on Windows

The `cargo fmt --check` command performs pure text comparison against rustfmt's formatting rules. It does not compile code, invoke FFI, or evaluate `#[cfg(...)]` attributes. Running it on Ubuntu is faster (Linux runners boot faster and have lower latency) and cheaper (Linux runner minutes cost less on GitHub Actions). In contrast, Clippy and tests must run on Windows because:

- `wallpaper.rs` uses `windows-sys` FFI calls gated behind `#[cfg(windows)]` -- these are only compiled and linted on Windows
- `wgpu_init.rs` has Windows-specific adapter selection behavior
- GPU integration tests use `LazyLock<Mutex<...>>` specifically because per-test device creation crashes on Windows -- this must be validated on Windows
- The `astronomy-engine-bindings` crate's bindgen step links against `libclang.dll`, which has different behavior on Windows vs. Linux

### Cache sharing strategy

The clippy and test jobs use `shared-key: ci-windows` to share a single cache entry. This works because both jobs target the same platform (`x86_64-pc-windows-msvc`) and use the same Rust toolchain version. The key design choice is that only the test job writes the cache (`save-if` defaults to `true`), while the clippy job sets `save-if: "false"`. This prevents the clippy job -- which only does a check build (`cargo clippy` does not produce full build artifacts) -- from overwriting the cache with a less useful artifact set.

The release workflow uses a separate `shared-key: release-windows` because release builds produce fundamentally different artifacts (LTO, single codegen unit, stripped) that would not benefit CI consumers and would waste cache space if mixed.

### LLVM and LIBCLANG_PATH

The `KyleMayes/install-llvm-action@v2` action downloads prebuilt LLVM binaries, adds them to `PATH`, and sets the `LLVM_PATH` environment variable. However, `clang-sys` (used by `bindgen`, used by `astronomy-engine-bindings`) looks for `libclang.dll` via the `LIBCLANG_PATH` environment variable, not `PATH`. The explicit `LIBCLANG_PATH=${{ env.LLVM_PATH }}/lib` step bridges this gap.

This is preferable to relying on the runner image's pre-installed LLVM because the `windows-latest` image has historically shipped inconsistent Clang versions. In mid-2025, the runner image provided Clang 18.1 while Microsoft's STL headers required Clang 19.0.0+, causing `error STL1000: Unexpected compiler version` (actions/runner-images #12435).

### Tag pattern

The release workflow triggers on tags matching `v[0-9]+.[0-9]+.[0-9]+`. This matches standard semver tags like `v0.1.0`, `v1.0.0`, `v2.3.14`. It does not match pre-release tags like `v0.1.0-rc.1` or `v0.1.0-beta.2`. If pre-release support is needed later, expand the pattern to `v[0-9]+.*`.

## Status

- [ ] Plan approved
- [ ] Phase 1 complete (directory structure)
- [ ] Phase 2 complete (CI workflow)
- [ ] Phase 3 complete (release workflow)
- [ ] Phase 4 complete (validation and CLAUDE.md update)
- [ ] Implementation complete
