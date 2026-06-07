#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

IMAGE_NAME="${IMAGE_NAME:-obsidian-mcp-server:latest}"
IMAGE_ARCH="${IMAGE_ARCH:-arm64}"
CONTAINERFILE_PATH="${CONTAINERFILE_PATH:-$PROJECT_ROOT/Containerfile}"

if ! command -v container >/dev/null 2>&1; then
    echo "Error: Apple 'container' CLI not found in PATH." >&2
    exit 1
fi

if [ ! -f "$CONTAINERFILE_PATH" ]; then
    echo "Error: Containerfile not found at $CONTAINERFILE_PATH" >&2
    exit 1
fi

echo "Building $IMAGE_NAME from $PROJECT_ROOT"
container build \
    --arch "$IMAGE_ARCH" \
    --tag "$IMAGE_NAME" \
    --file "$CONTAINERFILE_PATH" \
    "$PROJECT_ROOT"
