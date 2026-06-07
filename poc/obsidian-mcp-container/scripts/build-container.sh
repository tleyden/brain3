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
