# Obsidian MCP Container Build Runtime Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend `poc/obsidian-mcp-container/scripts/build-container.sh` so it can build the project with either Apple `container` or Docker via `--container-runtime`, while preserving the current native macOS default behavior.

**Architecture:** Keep one small shell entry point and branch only at the actual build command. Shared validation remains centralized, and runtime-specific checks happen only after argument parsing determines whether the script should use Apple `container` or Docker. This change intentionally does not bridge image stores or modify run-time behavior.

**Tech Stack:** Bash, Apple `container` CLI, Docker CLI, Markdown documentation

---

## Scope Notes

- This plan only changes `poc/obsidian-mcp-container/scripts/build-container.sh` and `poc/obsidian-mcp-container/README.md`.
- Do not modify `run-container.sh` in this change.
- Do not add automated unit tests or shell snapshot tests.
  - The change is narrow CLI integration logic, and the repo guidance explicitly says to be judicious about tests and avoid low-value test debt.
- Verification is manual and limited to the script's public interface.

## File Map

- Modify: `poc/obsidian-mcp-container/scripts/build-container.sh`
  - Add runtime argument parsing, runtime-specific binary checks, and split build commands.
- Modify: `poc/obsidian-mcp-container/README.md`
  - Document native and Docker build flows and how to build for both by invoking the script twice.

### Task 1: Add Runtime Parsing and Shared Validation

**Files:**
- Modify: `poc/obsidian-mcp-container/scripts/build-container.sh`

- [ ] **Step 1: Replace the top-level variable block and add usage text for `--container-runtime`**

```bash
#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

IMAGE_NAME="${IMAGE_NAME:-obsidian-mcp-server:latest}"
IMAGE_ARCH="${IMAGE_ARCH:-arm64}"
CONTAINERFILE_PATH="${CONTAINERFILE_PATH:-$PROJECT_ROOT/Containerfile}"
CONTAINER_RUNTIME="macos-container"

usage() {
    cat <<'EOF'
Usage: ./scripts/build-container.sh [options]

Options:
  --container-runtime RUNTIME   Build with macos-container or docker (default: macos-container)
  -h, --help                    Show this help
EOF
}
```

- [ ] **Step 2: Add strict argument parsing for the new runtime flag**

```bash
while [ "$#" -gt 0 ]; do
    case "$1" in
        --container-runtime)
            if [ "$#" -lt 2 ]; then
                echo "Error: missing value for --container-runtime" >&2
                usage >&2
                exit 1
            fi
            CONTAINER_RUNTIME="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Error: unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

case "$CONTAINER_RUNTIME" in
    macos-container|docker)
        ;;
    *)
        echo "Error: unknown container runtime: $CONTAINER_RUNTIME" >&2
        usage >&2
        exit 1
        ;;
esac
```

- [ ] **Step 3: Keep shared file validation before runtime-specific checks**

```bash
if [ ! -f "$CONTAINERFILE_PATH" ]; then
    echo "Error: Containerfile not found at $CONTAINERFILE_PATH" >&2
    exit 1
fi
```

- [ ] **Step 4: Run a syntax check after parsing changes**

Run: `bash -n poc/obsidian-mcp-container/scripts/build-container.sh`

Expected: no output and exit status `0`

- [ ] **Step 5: Commit the parsing-only milestone**

```bash
git add poc/obsidian-mcp-container/scripts/build-container.sh
git commit -m "feat: add build runtime selection"
```

### Task 2: Branch the Build Command by Runtime

**Files:**
- Modify: `poc/obsidian-mcp-container/scripts/build-container.sh`

- [ ] **Step 1: Add runtime-specific binary validation helpers**

```bash
require_macos_container() {
    if ! command -v container >/dev/null 2>&1; then
        echo "Error: Apple 'container' CLI not found in PATH." >&2
        exit 1
    fi
}

require_docker() {
    if ! command -v docker >/dev/null 2>&1; then
        echo "Error: Docker CLI not found in PATH." >&2
        exit 1
    fi
}
```

- [ ] **Step 2: Replace the single hard-coded build call with a runtime branch**

```bash
echo "Building $IMAGE_NAME from $PROJECT_ROOT with runtime $CONTAINER_RUNTIME"

if [ "$CONTAINER_RUNTIME" = "macos-container" ]; then
    require_macos_container
    container build \
        --arch "$IMAGE_ARCH" \
        --tag "$IMAGE_NAME" \
        --file "$CONTAINERFILE_PATH" \
        "$PROJECT_ROOT"
else
    require_docker
    docker build \
        --tag "$IMAGE_NAME" \
        --file "$CONTAINERFILE_PATH" \
        "$PROJECT_ROOT"
fi
```

- [ ] **Step 3: Ensure the final script matches this complete structure**

```bash
#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

IMAGE_NAME="${IMAGE_NAME:-obsidian-mcp-server:latest}"
IMAGE_ARCH="${IMAGE_ARCH:-arm64}"
CONTAINERFILE_PATH="${CONTAINERFILE_PATH:-$PROJECT_ROOT/Containerfile}"
CONTAINER_RUNTIME="macos-container"

usage() {
    cat <<'EOF'
Usage: ./scripts/build-container.sh [options]

Options:
  --container-runtime RUNTIME   Build with macos-container or docker (default: macos-container)
  -h, --help                    Show this help
EOF
}

require_macos_container() {
    if ! command -v container >/dev/null 2>&1; then
        echo "Error: Apple 'container' CLI not found in PATH." >&2
        exit 1
    fi
}

require_docker() {
    if ! command -v docker >/dev/null 2>&1; then
        echo "Error: Docker CLI not found in PATH." >&2
        exit 1
    fi
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --container-runtime)
            if [ "$#" -lt 2 ]; then
                echo "Error: missing value for --container-runtime" >&2
                usage >&2
                exit 1
            fi
            CONTAINER_RUNTIME="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Error: unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

case "$CONTAINER_RUNTIME" in
    macos-container|docker)
        ;;
    *)
        echo "Error: unknown container runtime: $CONTAINER_RUNTIME" >&2
        usage >&2
        exit 1
        ;;
esac

if [ ! -f "$CONTAINERFILE_PATH" ]; then
    echo "Error: Containerfile not found at $CONTAINERFILE_PATH" >&2
    exit 1
fi

echo "Building $IMAGE_NAME from $PROJECT_ROOT with runtime $CONTAINER_RUNTIME"

if [ "$CONTAINER_RUNTIME" = "macos-container" ]; then
    require_macos_container
    container build \
        --arch "$IMAGE_ARCH" \
        --tag "$IMAGE_NAME" \
        --file "$CONTAINERFILE_PATH" \
        "$PROJECT_ROOT"
else
    require_docker
    docker build \
        --tag "$IMAGE_NAME" \
        --file "$CONTAINERFILE_PATH" \
        "$PROJECT_ROOT"
fi
```

- [ ] **Step 4: Re-run syntax validation after the runtime branch is added**

Run: `bash -n poc/obsidian-mcp-container/scripts/build-container.sh`

Expected: no output and exit status `0`

- [ ] **Step 5: Commit the runtime build branch**

```bash
git add poc/obsidian-mcp-container/scripts/build-container.sh
git commit -m "feat: support docker builds for obsidian mcp container"
```

### Task 3: Update the README and Verify the Public Interface

**Files:**
- Modify: `poc/obsidian-mcp-container/README.md`

- [ ] **Step 1: Replace the build section so it documents both supported runtimes**

````md
## Container Build

This project includes a `Containerfile` that can be built with either Apple's native `container` CLI or Docker.

Build with the default native macOS runtime:

```bash
./scripts/build-container.sh
```

Build explicitly with Apple `container`:

```bash
./scripts/build-container.sh --container-runtime macos-container
```

Build explicitly with Docker:

```bash
./scripts/build-container.sh --container-runtime docker
```

If you want the image available in both runtimes on the same machine, run the script twice, once per runtime.

This uses:

- base image: `python:3.14.5-slim-bookworm`
- build context: this `poc/obsidian-mcp-container` directory only
- image name: `obsidian-mcp-server:latest` by default

If you want a different tag:

```bash
IMAGE_NAME=obsidian-mcp-server:dev ./scripts/build-container.sh --container-runtime docker
```
````

- [ ] **Step 2: Add one Linux-focused example near the build section**

````md
On a Linux machine that only has Docker installed, build with:

```bash
./scripts/build-container.sh --container-runtime docker
```
````

- [ ] **Step 3: Run manual verification against the public CLI**

Run: `poc/obsidian-mcp-container/scripts/build-container.sh --help`

Expected: usage output includes `--container-runtime` and the allowed values.

Run: `poc/obsidian-mcp-container/scripts/build-container.sh --container-runtime docker`

Expected: the script prints `Building obsidian-mcp-server:latest from ... with runtime docker` and invokes `docker build`.

Run: `poc/obsidian-mcp-container/scripts/build-container.sh --container-runtime macos-container`

Expected: the script prints `Building obsidian-mcp-server:latest from ... with runtime macos-container` and invokes `container build`.

- [ ] **Step 4: Commit the docs and verification changes**

```bash
git add poc/obsidian-mcp-container/README.md
git commit -m "docs: document container build runtimes"
```

## Self-Review

- Spec coverage: the plan covers the new flag, native runtime retention, Docker build support, shared `Containerfile` usage, and README updates.
- Placeholder scan: no `TBD`, `TODO`, or implied follow-up work is left in the implementation steps.
- Type consistency: the plan uses one runtime flag name, one default runtime value, and one Docker command shape consistently throughout.
