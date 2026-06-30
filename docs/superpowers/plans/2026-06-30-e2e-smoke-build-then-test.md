# E2E Smoke: Build Image First, Then Run Test

## Problem

Running only the `cargo test` half of the documented E2E command fails:

```
ERROR ... container command failed cmd="docker" code=1
  stderr="Error response from daemon: pull access denied for brain3-mcp-vault-tools,
  repository does not exist or may require 'docker login'"
Error: Brain3 startup failed: command failed (exit 1): ...
```

### Root cause

The `docker build ... -t brain3-mcp-vault-tools:e2e-local` step never ran, so the
image was not present locally. `EnsureContainerUseCase::ensure`
(`crates/core/src/application/ensure_container.rs:63`) treats any missing image as a
cue to `docker pull` it (`crates/platform/src/container/docker.rs:127`). Because
`brain3-mcp-vault-tools:e2e-local` only ever exists as a local build, the pull fails
with the misleading `pull access denied` error.

## Goal

A single command that **builds the image first** and only runs the E2E test **if the
build succeeds** — never falling through to a registry pull.

## Plan

### 1. Add `scripts/e2e_smoke.py` (run via `uv`)

No bash. A self-contained Python script run with `uv` — using a
[PEP 723](https://peps.python.org/pep-0723/) inline-metadata header and the
`#!/usr/bin/env -S uv run --script` shebang so it needs no venv or `pip install`
(stdlib `subprocess` only, so the dependency list is empty). The script:

1. Resolves the repo root from `__file__` so it works from any cwd.
2. Runs the docker build via `subprocess.run(..., check=True)`:
   `docker build -f ./brain3-mcp-vault-tools/Containerfile -t brain3-mcp-vault-tools:e2e-local ./brain3-mcp-vault-tools`
3. If the build exits non-zero, **prints a clear message and `sys.exit`s** before
   running the test — never falling through to a registry pull.
4. On build success, runs:
   `cargo test -p brain3 --test e2e_smoke --features e2e -- --nocapture`
5. Forwards any extra CLI args (e.g. test filters) to the cargo invocation.
6. Propagates the test's exit code as the script's exit code.

Invoked as either `./scripts/e2e_smoke.py` (shebang) or `uv run scripts/e2e_smoke.py`.

### 2. Update `AGENTS.MD`

Replace the inline two-part command in the E2E bullet with a pointer to
`uv run scripts/e2e_smoke.py`, keeping the raw build + test commands documented
underneath for transparency.

### 3. (Optional / recommended) Fail-fast guard for the `e2e-local` tag

Currently any missing image triggers a registry pull. As a separate follow-up, make
the missing-image path skip the pull (or fail with an actionable "build the image
first" message) for the local-only `e2e-local` build, so a forgotten build step
produces a clear error instead of `pull access denied`. This is a `crates/core`
change and is kept distinct from steps 1–2.

## Non-goals

- The Rust test will **not** shell out to `docker build` itself. Keeping build and
  test concerns separate avoids slow/fragile test setup.

## Verification

Run `uv run scripts/e2e_smoke.py` end-to-end and confirm the test passes against the
freshly built image. Also confirm that a deliberately broken build aborts before the
test runs.
