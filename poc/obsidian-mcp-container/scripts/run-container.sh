#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

if [ -f "$PROJECT_ROOT/.env" ]; then
    set -o allexport
    # shellcheck source=../.env
    source "$PROJECT_ROOT/.env"
    set +o allexport
fi

MODE="image"
IMAGE_NAME="${IMAGE_NAME:-obsidian-mcp-server:latest}"
CONTAINER_NAME="${CONTAINER_NAME:-obsidian-mcp-server}"
HOST_PORT="${HOST_PORT:-8420}"
HOST_VAULT_PATH="${HOST_VAULT_PATH:-${VAULT_PATH:-}}"
SOURCE_MOUNT_PATH="/workspace/obsidian-mcp-container"
DETACH=true
REMOVE=true

usage() {
    cat <<'EOF'
Usage: ./scripts/run-container.sh [options]

Options:
  --bind-source         Run the mounted host source tree instead of the code baked into the image
  --image               Run the code baked into the image (default)
  --vault-path PATH     Host vault directory to mount into /vault
  --port PORT           Host port to publish to container port 8420 (default: 8420)
  --name NAME           Container name (default: obsidian-mcp-server)
  --image-name NAME     Image reference to run (default: obsidian-mcp-server:latest)
  --foreground          Run attached instead of detached
  --keep                Do not pass --rm
  -h, --help            Show this help
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --bind-source)
            MODE="bind"
            shift
            ;;
        --image)
            MODE="image"
            shift
            ;;
        --vault-path)
            HOST_VAULT_PATH="$2"
            shift 2
            ;;
        --port)
            HOST_PORT="$2"
            shift 2
            ;;
        --name)
            CONTAINER_NAME="$2"
            shift 2
            ;;
        --image-name)
            IMAGE_NAME="$2"
            shift 2
            ;;
        --foreground)
            DETACH=false
            shift
            ;;
        --keep)
            REMOVE=false
            shift
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

if ! command -v container >/dev/null 2>&1; then
    echo "Error: Apple 'container' CLI not found in PATH." >&2
    exit 1
fi

if [ -z "$HOST_VAULT_PATH" ]; then
    echo "Error: no vault path provided. Set HOST_VAULT_PATH, VAULT_PATH, or pass --vault-path." >&2
    exit 1
fi

if [ ! -d "$HOST_VAULT_PATH" ]; then
    echo "Error: vault path does not exist: $HOST_VAULT_PATH" >&2
    exit 1
fi

container stop "$CONTAINER_NAME" >/dev/null 2>&1 || true
container delete "$CONTAINER_NAME" >/dev/null 2>&1 || true

run_args=(
    run
    --name "$CONTAINER_NAME"
    --publish "127.0.0.1:${HOST_PORT}:8420"
    --env "VAULT_MCP_HOST=0.0.0.0"
    --env "VAULT_PATH=/vault"
    --mount "type=bind,source=${HOST_VAULT_PATH},target=/vault"
)

if [ -n "${VAULT_MCP_ALLOWED_HOSTS:-}" ]; then
    run_args+=(--env "VAULT_MCP_ALLOWED_HOSTS=${VAULT_MCP_ALLOWED_HOSTS}")
fi

if [ "$DETACH" = true ]; then
    run_args+=(--detach)
fi

if [ "$REMOVE" = true ]; then
    run_args+=(--rm)
fi

if [ "$MODE" = "bind" ]; then
    run_args+=(
        --env "PYTHONPATH=${SOURCE_MOUNT_PATH}/src"
        --workdir "$SOURCE_MOUNT_PATH"
        --mount "type=bind,source=${PROJECT_ROOT},target=${SOURCE_MOUNT_PATH},readonly"
        "$IMAGE_NAME"
        /opt/obsidian-mcp-container/.venv/bin/python
        -m
        obsidian_mcp_server.server
    )
else
    run_args+=("$IMAGE_NAME")
fi

echo "Running $CONTAINER_NAME from $IMAGE_NAME in $MODE mode"
container "${run_args[@]}"

if [ "$DETACH" = true ]; then
    echo "Published MCP endpoint: http://127.0.0.1:${HOST_PORT}/mcp"
    echo "View logs with: container logs $CONTAINER_NAME"
fi
