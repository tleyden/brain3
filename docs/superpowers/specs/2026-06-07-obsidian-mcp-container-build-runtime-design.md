# Obsidian MCP Container Build Runtime Design

**Date:** 2026-06-07

**Goal:** Extend the Obsidian MCP container build workflow so the same `build-container.sh` entry point can build with either Apple `container` or Docker, without trying to bridge image stores between runtimes.

## Context

The current build flow in `poc/obsidian-mcp-container/scripts/build-container.sh` only targets Apple's native `container` CLI:

- it assumes the host has the `container` binary
- it always passes `--arch "$IMAGE_ARCH"` to `container build`
- it produces an image only in the Apple `container` image store

That matches the POC's macOS-native bias, but it does not cover two real usage modes:

1. A macOS machine where the user wants to build a native `container` image sometimes and a Docker image other times.
2. A Linux machine where only Docker is installed.

The user explicitly wants to keep the interface simple:

- `--container-runtime` accepts only `macos-container` or `docker`
- `all` is removed
- building for both runtimes is handled by invoking the script twice

## Requirements

### Functional

- Keep `poc/obsidian-mcp-container/scripts/build-container.sh` as the single build entry point.
- Add `--container-runtime macos-container|docker`.
- Default to `macos-container` when the flag is omitted, preserving current behavior.
- In `macos-container` mode:
  - require the Apple `container` CLI
  - keep using `container build`
  - keep honoring `IMAGE_ARCH`
- In `docker` mode:
  - require Docker
  - build the same project with Docker using the existing `Containerfile`
  - do not force `IMAGE_ARCH=arm64` by default
- Keep existing `IMAGE_NAME` and `CONTAINERFILE_PATH` environment-variable overrides.
- Update project docs so users know how to build for each runtime.

### Non-Functional

- Do not introduce Docker Compose.
- Do not attempt image export/import between Apple `container` and Docker.
- Do not add automated unit tests for this shell-script change.
  - Rationale: this is runtime integration logic with limited stable public API surface, and shell test scaffolding would add more maintenance burden than value for this POC.

## Chosen Design

### CLI Contract

`build-container.sh` will accept:

```bash
./scripts/build-container.sh --container-runtime macos-container
./scripts/build-container.sh --container-runtime docker
```

If omitted, `--container-runtime` defaults to `macos-container`.

Invalid values fail fast with a usage error.

### Runtime-Specific Build Behavior

#### `macos-container`

The script keeps the current native build path:

```bash
container build \
    --arch "$IMAGE_ARCH" \
    --tag "$IMAGE_NAME" \
    --file "$CONTAINERFILE_PATH" \
    "$PROJECT_ROOT"
```

This preserves the current Apple-silicon-first workflow and keeps `IMAGE_ARCH` meaningful where it already applies.

#### `docker`

The script builds the same source tree and `Containerfile` through Docker:

```bash
docker build \
    --tag "$IMAGE_NAME" \
    --file "$CONTAINERFILE_PATH" \
    "$PROJECT_ROOT"
```

The Docker branch deliberately does not thread `IMAGE_ARCH` through this first change. That keeps the interface smaller and avoids inventing a platform contract the user did not ask for. If multi-platform or explicit `--platform` support becomes necessary later, it can be added as a separate change.

## Validation and Error Handling

The script will validate shared prerequisites first:

- `CONTAINERFILE_PATH` exists

It will then validate runtime-specific prerequisites:

- `macos-container`: `container` must be on `PATH`
- `docker`: `docker` must be on `PATH`

Failure messages should be explicit and tell the user which runtime dependency is missing.

Examples:

- `Error: Apple 'container' CLI not found in PATH.`
- `Error: Docker CLI not found in PATH.`
- `Error: missing value for --container-runtime`
- `Error: unknown container runtime: foo`

## Documentation Changes

`poc/obsidian-mcp-container/README.md` will be updated to document:

- native macOS build usage
- Docker build usage on macOS
- Docker build usage on Linux
- the fact that building for both runtimes means invoking the script twice

## Verification Strategy

No automated tests will be added in this change.

Verification will be manual and limited to the script's public behavior:

```bash
./scripts/build-container.sh --help
./scripts/build-container.sh
./scripts/build-container.sh --container-runtime macos-container
./scripts/build-container.sh --container-runtime docker
```

Expected outcomes:

- `--help` shows the new flag and allowed values
- default invocation still uses Apple `container`
- explicit Docker invocation uses `docker build`
- missing runtime binaries fail with targeted error messages

## Tradeoffs

### Why this approach

- It preserves the current default behavior for the existing macOS-native workflow.
- It supports Linux cleanly without forcing Apple runtime concepts there.
- It avoids fragile local image handoff logic between separate runtimes.
- It keeps the public interface small enough to understand at a glance.

### What we are explicitly not doing

- no `all` mode
- no `CONTAINER_RUNTIME` environment variable
- no Docker-specific platform flag
- no runtime autodetection beyond preserving the current default
- no changes to `run-container.sh` in this design

## Impacted Files

- Modify: `poc/obsidian-mcp-container/scripts/build-container.sh`
- Modify: `poc/obsidian-mcp-container/README.md`

## Open Questions

None for this scope. The behavior is intentionally narrow and matches the user's simplified requirement.
