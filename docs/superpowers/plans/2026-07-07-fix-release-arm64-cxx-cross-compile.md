# Fix Linux ARM64 release build: missing C++ cross compiler for whisper-rs-sys

## Context

Release workflow run for tag `v0.2.11` (run `28881582003`) failed. The `Build Linux ARM64` job failed with exit code 101, which cancelled the other three build matrix jobs (`Build macOS ARM64`, `Build Windows x86_64`, `Build Linux x86_64`) via the shared strategy, and also failed `Build macOS x86_64`.

## Root cause

`whisper-rs-sys` (pulled in by the native audio transcription feature, PR #154) builds `whisper.cpp` via CMake at build time. When cross-compiling for `aarch64-unknown-linux-gnu` on an `ubuntu-latest` runner, CMake needs a C++ cross compiler (`aarch64-linux-gnu-g++`).

`.github/workflows/release.yml`'s `Install cross-compilation tools` step only installed `gcc-aarch64-linux-gnu` (a C cross compiler) and only configured the linker env var (`CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER`). There was no C++ cross compiler installed and no `CXX`/`CC` env var telling `cc`/`cmake` crates which cross compiler to use for that target, so CMake's `project()` call failed with:

```
CMake Error at CMakeLists.txt:2 (project):
  The CMAKE_CXX_COMPILER:
    aarch64-linux-gnu-g++
  is not a full path and was not found in the PATH.
```

This regression was invisible until the actual release run because no other CI workflow (e.g. `ci.yml`) cross-compiles for `aarch64-unknown-linux-gnu` — the gap was never exercised until `bump-version` tagged and pushed `v0.2.11`.

## Fix

In `.github/workflows/release.yml`, under the `aarch64-unknown-linux-gnu` matrix entry:

1. Install `g++-aarch64-linux-gnu` in addition to `gcc-aarch64-linux-gnu`.
2. Export `CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc` and `CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++` so the `cc`/`cmake` crates pick the correct cross compilers for the target (in addition to the existing `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER`).

This has been applied to `.github/workflows/release.yml` on branch `bump_version_0211`.

## Remaining steps

1. User commits the workflow fix (Claude does not commit, per project rules).
2. Decide how to re-trigger the release for `v0.2.11`:
   - Option A: delete the local + remote `v0.2.11` tag, re-create it pointing at the new commit, and push — re-triggers `.github/workflows/release.yml`.
   - Option B: some other approach the user prefers.
3. Re-run/monitor the release workflow (`gh run watch`) until all five build targets, asset preparation, GitHub Release creation, and S3 publish succeed.
4. Verify the published release (`gh release view vX.Y.Z`) and pull the GHCR Docker image to confirm it published correctly.

## Follow-up consideration (not yet actioned)

No existing CI workflow cross-compiles for `aarch64-unknown-linux-gnu`, so a similar gap could resurface for other new native dependencies. Consider adding an ARM64 cross-compile check to the regular CI workflow so this class of bug is caught before a release tag is pushed. Not implemented as part of this fix — flagged for future consideration only.
