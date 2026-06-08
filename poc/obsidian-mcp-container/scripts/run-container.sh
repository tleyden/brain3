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
CONTAINER_RUNTIME="macos-container"
HOST_BIND_ADDRESS="127.0.0.1"
CONTAINER_LISTEN_HOST="0.0.0.0"
CONTAINER_PORT="8420"
DETACH=true
REMOVE=true

usage() {
    cat <<'EOF'
Usage: ./scripts/run-container.sh [options]

Options:
  --container-runtime    Run with macos-container or docker (default: macos-container)
  --bind-source         Run the mounted host source tree instead of the code baked into the image
  --image               Run the code baked into the image (default)
  --vault-path PATH     Host vault directory to mount into /vault
  --port PORT           Host loopback port to publish as 127.0.0.1:PORT -> container port 8420
  --name NAME           Container name (default: obsidian-mcp-server)
  --image-name NAME     Image reference to run (default: obsidian-mcp-server:latest)
  --foreground          Run attached instead of detached
  --keep                Do not pass --rm
  -h, --help            Show this help

Networking:
  The server listens on 0.0.0.0:8420 inside the container so published traffic can reach it.
  The host publishes that port on 127.0.0.1 only, so it is not exposed on other host interfaces.
  VAULT_MCP_ALLOWED_HOSTS adds allowed HTTP Host headers for DNS rebinding protection; it does not
  publish the port on additional host interfaces.
EOF
}

print_networking_summary() {
    echo "Networking:"
    echo "  Inside container: ${CONTAINER_LISTEN_HOST}:${CONTAINER_PORT}"
    echo "  On host:          ${HOST_BIND_ADDRESS}:${HOST_PORT} -> container:${CONTAINER_PORT}"
    echo "  Host exposure:    loopback only; remote hosts cannot connect via other host interfaces"
    echo "  Host header ACL:  defaults to 127.0.0.1, localhost, [::1]"

    if [ -n "${VAULT_MCP_ALLOWED_HOSTS:-}" ]; then
        echo "                    plus VAULT_MCP_ALLOWED_HOSTS=${VAULT_MCP_ALLOWED_HOSTS}"
    fi

    echo "                    this changes allowed HTTP Host headers, not socket binding"

    if [ "$CONTAINER_RUNTIME" = "docker" ]; then
        echo "  Docker note:      Docker documents a localhost publishing caveat on releases older than 28.0.0"
        echo "                    where hosts on the same L2 segment may still reach the port"
    fi
}

print_mount_summary() {
    echo "Mounts:"
    echo "  ${HOST_VAULT_PATH} -> /vault"

    if [ "$MODE" = "bind" ]; then
        echo "  ${PROJECT_ROOT} -> ${SOURCE_MOUNT_PATH} (ro)"
    fi
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

ensure_runtime_image_exists() {
    if [ "$CONTAINER_RUNTIME" = "macos-container" ]; then
        if ! container image inspect "$IMAGE_NAME" >/dev/null 2>&1; then
            echo "Error: image not found in Apple container image store: $IMAGE_NAME" >&2
            echo "Build it with: ./scripts/build-container.sh --container-runtime macos-container" >&2
            exit 1
        fi
    else
        if ! docker image inspect "$IMAGE_NAME" >/dev/null 2>&1; then
            echo "Error: image not found in Docker image store: $IMAGE_NAME" >&2
            echo "Build it with: ./scripts/build-container.sh --container-runtime docker" >&2
            exit 1
        fi
    fi
}

cleanup_existing_container() {
    if [ "$CONTAINER_RUNTIME" = "macos-container" ]; then
        container stop "$CONTAINER_NAME" >/dev/null 2>&1 || true
        container delete "$CONTAINER_NAME" >/dev/null 2>&1 || true
    else
        docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
    fi
}

check_and_prompt_existing_container() {
    local exists=false
    if [ "$CONTAINER_RUNTIME" = "macos-container" ]; then
        if container inspect "$CONTAINER_NAME" >/dev/null 2>&1; then
            exists=true
        fi
    else
        if docker inspect "$CONTAINER_NAME" >/dev/null 2>&1; then
            exists=true
        fi
    fi

    if [ "$exists" = true ]; then
        echo "Warning: a container named '$CONTAINER_NAME' already exists."
        printf "Stop and remove it to start a fresh one? [y/N] "
        read -r answer
        case "$answer" in
            [yY]*)
                cleanup_existing_container
                ;;
            *)
                echo "Aborted. Use --name to specify a different container name." >&2
                exit 1
                ;;
        esac
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

case "$CONTAINER_RUNTIME" in
    macos-container)
        require_macos_container
        ;;
    docker)
        require_docker
        ;;
    *)
        echo "Error: unknown container runtime: $CONTAINER_RUNTIME" >&2
        usage >&2
        exit 1
        ;;
esac

if [ -z "$HOST_VAULT_PATH" ]; then
    echo "Error: no vault path provided. Set HOST_VAULT_PATH, VAULT_PATH, or pass --vault-path." >&2
    exit 1
fi

if [ ! -d "$HOST_VAULT_PATH" ]; then
    echo "Error: vault path does not exist: $HOST_VAULT_PATH" >&2
    exit 1
fi

ensure_runtime_image_exists
check_and_prompt_existing_container

run_args=(
    run
    --name "$CONTAINER_NAME"
    --user "$(id -u):$(id -g)"
    --publish "${HOST_BIND_ADDRESS}:${HOST_PORT}:${CONTAINER_PORT}"
    --env "VAULT_MCP_HOST=${CONTAINER_LISTEN_HOST}"
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

echo "Running $CONTAINER_NAME from $IMAGE_NAME in $MODE mode with runtime $CONTAINER_RUNTIME"
print_networking_summary
print_mount_summary

if [ "$CONTAINER_RUNTIME" = "macos-container" ]; then
    container "${run_args[@]}"
else
    docker "${run_args[@]}"
fi

if [ "$DETACH" = true ]; then
    echo "Published MCP endpoint on host loopback: http://${HOST_BIND_ADDRESS}:${HOST_PORT}/mcp"
    echo "Container listener: http://${CONTAINER_LISTEN_HOST}:${CONTAINER_PORT}/mcp"
    if [ "$CONTAINER_RUNTIME" = "macos-container" ]; then
        echo "View logs with: container logs $CONTAINER_NAME"
    else
        echo "View logs with: docker logs $CONTAINER_NAME"
    fi
fi
