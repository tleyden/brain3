#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
POC_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OAUTH_ROOT="$POC_ROOT/oauth2-host-gw"
CONTAINER_ROOT="$POC_ROOT/obsidian-mcp-container"

GATEWAY_HOST="127.0.0.1"
GATEWAY_PORT="8421"
GATEWAY_STOP_TIMEOUT_SECS="${GATEWAY_STOP_TIMEOUT_SECS:-10}"
DEFAULT_CONTAINER_NAME="obsidian-mcp-server"
TARGET_CONTAINER_NAME=""
CONTAINER_RUNTIME=""

usage() {
    cat <<'EOF'
Usage: ./scripts/stop.sh --container-runtime <macos-container|docker> [options]

Stops the local OAuth gateway started by ./scripts/run.sh and removes the MCP container.

Options:
  --container-runtime    Required: macos-container or docker
  --name NAME            Container name (defaults to CONTAINER_NAME from
                         ./obsidian-mcp-container/.env or obsidian-mcp-server)
  -h, --help             Show this help
EOF
}

require_explicit_container_runtime() {
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
            --name)
                if [ "$#" -lt 2 ]; then
                    echo "Error: missing value for --name" >&2
                    usage >&2
                    exit 1
                fi
                TARGET_CONTAINER_NAME="$2"
                shift 2
                ;;
            *)
                echo "Error: unknown option: $1" >&2
                usage >&2
                exit 1
                ;;
        esac
    done

    if [ -z "$CONTAINER_RUNTIME" ]; then
        echo "Error: --container-runtime is required. Choose one of: macos-container, docker." >&2
        usage >&2
        exit 1
    fi

    case "$CONTAINER_RUNTIME" in
        macos-container|docker)
            ;;
        *)
            echo "Error: unknown container runtime: $CONTAINER_RUNTIME" >&2
            usage >&2
            exit 1
            ;;
    esac
}

load_config() {
    if [ -f "$OAUTH_ROOT/.env" ]; then
        set -o allexport
        # shellcheck source=../oauth2-host-gw/.env
        source "$OAUTH_ROOT/.env"
        set +o allexport
    fi

    if [ -f "$CONTAINER_ROOT/.env" ]; then
        set -o allexport
        # shellcheck source=../obsidian-mcp-container/.env
        source "$CONTAINER_ROOT/.env"
        set +o allexport
    fi

    GATEWAY_PORT="${OAUTH2_GATEWAY_PORT:-8421}"
    GATEWAY_HEALTH_URL="http://${GATEWAY_HOST}:${GATEWAY_PORT}/health"

    if [ -z "$TARGET_CONTAINER_NAME" ]; then
        TARGET_CONTAINER_NAME="${CONTAINER_NAME:-$DEFAULT_CONTAINER_NAME}"
    fi
}

require_prereqs() {
    if ! command -v curl >/dev/null 2>&1; then
        echo "Error: curl is required to probe the local OAuth gateway." >&2
        exit 1
    fi

    if ! command -v lsof >/dev/null 2>&1; then
        echo "Error: lsof is required to locate the OAuth gateway listener." >&2
        exit 1
    fi

    if [ "$CONTAINER_RUNTIME" = "macos-container" ]; then
        if ! command -v container >/dev/null 2>&1; then
            echo "Error: Apple 'container' CLI not found in PATH." >&2
            exit 1
        fi
    else
        if ! command -v docker >/dev/null 2>&1; then
            echo "Error: Docker CLI not found in PATH." >&2
            exit 1
        fi
    fi
}

gateway_is_healthy() {
    curl -fsS --max-time 2 "$GATEWAY_HEALTH_URL" >/dev/null 2>&1
}

gateway_port_in_use() {
    lsof -nP -iTCP:"$GATEWAY_PORT" -sTCP:LISTEN >/dev/null 2>&1
}

gateway_listener_pids() {
    lsof -tiTCP:"$GATEWAY_PORT" -sTCP:LISTEN 2>/dev/null | sort -u
}

gateway_listener_looks_like_oauth_gateway() {
    local pids="$1"
    local pid
    local command

    while IFS= read -r pid; do
        [ -z "$pid" ] && continue
        command="$(ps -p "$pid" -o command= 2>/dev/null || true)"
        case "$command" in
            *oauth2-gateway*|*start-oauth2-server.sh*)
                return 0
                ;;
        esac
    done <<<"$pids"

    return 1
}

wait_for_gateway_stop() {
    local deadline

    deadline=$((SECONDS + GATEWAY_STOP_TIMEOUT_SECS))
    while [ "$SECONDS" -lt "$deadline" ]; do
        if ! gateway_port_in_use; then
            return 0
        fi
        sleep 1
    done

    return 1
}

stop_gateway() {
    local pids

    pids="$(gateway_listener_pids || true)"

    if [ -z "$pids" ]; then
        echo "OAuth gateway is not listening on port $GATEWAY_PORT."
        return 0
    fi

    if ! gateway_is_healthy && ! gateway_listener_looks_like_oauth_gateway "$pids"; then
        echo "Skipping OAuth gateway shutdown: port $GATEWAY_PORT is in use by a different process." >&2
        lsof -nP -iTCP:"$GATEWAY_PORT" -sTCP:LISTEN >&2 || true
        return 1
    fi

    echo "Stopping OAuth gateway listener on port $GATEWAY_PORT."
    # shellcheck disable=SC2086
    kill $pids >/dev/null 2>&1 || true

    if wait_for_gateway_stop; then
        echo "OAuth gateway stopped."
        return 0
    fi

    echo "Error: OAuth gateway listener on port $GATEWAY_PORT did not stop within ${GATEWAY_STOP_TIMEOUT_SECS}s." >&2
    lsof -nP -iTCP:"$GATEWAY_PORT" -sTCP:LISTEN >&2 || true
    return 1
}

stop_container() {
    if [ "$CONTAINER_RUNTIME" = "macos-container" ]; then
        if ! container inspect "$TARGET_CONTAINER_NAME" >/dev/null 2>&1; then
            echo "Container '$TARGET_CONTAINER_NAME' is not present in Apple container runtime."
            return 0
        fi

        echo "Stopping container '$TARGET_CONTAINER_NAME' in Apple container runtime."
        container stop "$TARGET_CONTAINER_NAME" >/dev/null 2>&1 || true
        container delete "$TARGET_CONTAINER_NAME" >/dev/null
        echo "Container '$TARGET_CONTAINER_NAME' removed."
        return 0
    fi

    if ! docker inspect "$TARGET_CONTAINER_NAME" >/dev/null 2>&1; then
        echo "Container '$TARGET_CONTAINER_NAME' is not present in Docker."
        return 0
    fi

    echo "Stopping container '$TARGET_CONTAINER_NAME' in Docker."
    docker rm -f "$TARGET_CONTAINER_NAME" >/dev/null
    echo "Container '$TARGET_CONTAINER_NAME' removed."
}

main() {
    local status=0

    require_explicit_container_runtime "$@"
    load_config
    require_prereqs

    if ! stop_gateway; then
        status=1
    fi

    if ! stop_container; then
        status=1
    fi

    return "$status"
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
    usage
    exit 0
fi

main "$@"
