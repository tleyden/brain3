#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
POC_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OAUTH_ROOT="$POC_ROOT/oauth2-host-gw"
GATEWAY_START_SCRIPT="$OAUTH_ROOT/scripts/start-oauth2-server.sh"
CONTAINER_RUN_SCRIPT="$POC_ROOT/obsidian-mcp-container/scripts/run-container.sh"

GATEWAY_HOST="127.0.0.1"
GATEWAY_PORT="8421"
GATEWAY_START_TIMEOUT_SECS="${GATEWAY_START_TIMEOUT_SECS:-15}"
GATEWAY_STOP_TIMEOUT_SECS="${GATEWAY_STOP_TIMEOUT_SECS:-10}"
GATEWAY_LOG_PATH="${GATEWAY_LOG_PATH:-/tmp/agentzoo-oauth2-gateway.log}"
CONTAINER_RUNTIME=""

usage() {
    cat <<'EOF'
Usage: ./scripts/run.sh --container-runtime <macos-container|docker> [container-run-args...]

Starts the local OAuth gateway if needed, then starts the MCP container.

Behavior:
  - If the gateway is already healthy, ask whether to reuse it, restart it, or abort.
  - If the gateway port is occupied by a different process, abort with guidance.
  - If the gateway cannot become healthy, abort before starting the container.
  - The container runtime must be specified explicitly on every invocation.

All arguments are passed through to:
  ./obsidian-mcp-container/scripts/run-container.sh
EOF
}

require_explicit_container_runtime() {
    local runtime=""

    while [ "$#" -gt 0 ]; do
        case "$1" in
            --container-runtime)
                if [ "$#" -lt 2 ]; then
                    echo "Error: missing value for --container-runtime" >&2
                    usage >&2
                    exit 1
                fi
                runtime="$2"
                shift 2
                ;;
            *)
                shift
                ;;
        esac
    done

    if [ -z "$runtime" ]; then
        echo "Error: --container-runtime is required. Choose one of: macos-container, docker." >&2
        usage >&2
        exit 1
    fi

    case "$runtime" in
        macos-container|docker)
            CONTAINER_RUNTIME="$runtime"
            ;;
        *)
            echo "Error: unknown container runtime: $runtime" >&2
            usage >&2
            exit 1
            ;;
    esac
}

load_gateway_config() {
    if [ -f "$OAUTH_ROOT/.env" ]; then
        set -o allexport
        # shellcheck source=../oauth2-host-gw/.env
        source "$OAUTH_ROOT/.env"
        set +o allexport
    fi

    GATEWAY_PORT="${OAUTH2_GATEWAY_PORT:-8421}"
    GATEWAY_HEALTH_URL="http://${GATEWAY_HOST}:${GATEWAY_PORT}/health"
}

require_prereqs() {
    if ! command -v curl >/dev/null 2>&1; then
        echo "Error: curl is required to probe the local OAuth gateway." >&2
        exit 1
    fi

    if [ ! -x "$GATEWAY_START_SCRIPT" ]; then
        echo "Error: missing gateway start script: $GATEWAY_START_SCRIPT" >&2
        exit 1
    fi

    if [ ! -x "$CONTAINER_RUN_SCRIPT" ]; then
        echo "Error: missing container run script: $CONTAINER_RUN_SCRIPT" >&2
        exit 1
    fi

    if [ ! -d "$OAUTH_ROOT/.venv" ]; then
        cat >&2 <<EOF
Error: OAuth gateway virtual environment not found: $OAUTH_ROOT/.venv

How to fix:
  cd $OAUTH_ROOT
  uv sync

Then run ./scripts/run.sh again.
EOF
        exit 1
    fi

    missing=()
    [ -z "${OAUTH2_GATEWAY_CLIENT_SECRET:-}" ] && missing+=("OAUTH2_GATEWAY_CLIENT_SECRET")
    [ -z "${OAUTH2_GATEWAY_ACCESS_TOKEN:-}" ] && missing+=("OAUTH2_GATEWAY_ACCESS_TOKEN")

    if [ ${#missing[@]} -gt 0 ]; then
        echo "Error: missing required OAuth gateway values: ${missing[*]}" >&2
        echo "Generate them with $OAUTH_ROOT/scripts/generate_secrets.sh" >&2
        exit 1
    fi
}

gateway_is_healthy() {
    curl -fsS --max-time 2 "$GATEWAY_HEALTH_URL" >/dev/null 2>&1
}

gateway_port_in_use() {
    if command -v lsof >/dev/null 2>&1; then
        lsof -nP -iTCP:"$GATEWAY_PORT" -sTCP:LISTEN >/dev/null 2>&1
        return
    fi

    return 1
}

gateway_listener_pids() {
    if ! command -v lsof >/dev/null 2>&1; then
        return 1
    fi

    lsof -tiTCP:"$GATEWAY_PORT" -sTCP:LISTEN 2>/dev/null | sort -u
}

print_unhealthy_listener_error() {
    cat >&2 <<EOF
Error: port $GATEWAY_PORT is already in use, but $GATEWAY_HEALTH_URL is not serving the expected OAuth gateway.

How to fix:
  Stop the process that is listening on port $GATEWAY_PORT, or change OAUTH2_GATEWAY_PORT in:
    $OAUTH_ROOT/.env
EOF

    if command -v lsof >/dev/null 2>&1; then
        echo >&2
        echo "Listener details:" >&2
        lsof -nP -iTCP:"$GATEWAY_PORT" -sTCP:LISTEN >&2 || true
    fi

    exit 1
}

prompt_existing_gateway_action() {
    while true; do
        printf "A healthy OAuth gateway is already listening on %s. Reuse it, restart it, or abort? [reuse/restart/abort] " "$GATEWAY_HEALTH_URL"
        read -r action || {
            echo >&2
            echo "Aborted." >&2
            exit 1
        }

        case "$action" in
            reuse)
                echo "Reusing existing OAuth gateway."
                return 0
                ;;
            restart)
                return 1
                ;;
            abort|"")
                echo "Aborted."
                exit 1
                ;;
            *)
                echo "Please enter 'reuse', 'restart', or 'abort'."
                ;;
        esac
    done
}

stop_existing_gateway_listener() {
    local pids
    local deadline

    pids="$(gateway_listener_pids || true)"

    if [ -z "$pids" ]; then
        return 0
    fi

    echo "Stopping existing OAuth gateway listener on port $GATEWAY_PORT."
    # shellcheck disable=SC2086
    kill $pids

    deadline=$((SECONDS + GATEWAY_STOP_TIMEOUT_SECS))
    while [ "$SECONDS" -lt "$deadline" ]; do
        if ! gateway_port_in_use; then
            return 0
        fi
        sleep 1
    done

    echo "Error: existing listener on port $GATEWAY_PORT did not stop within ${GATEWAY_STOP_TIMEOUT_SECS}s." >&2
    if command -v lsof >/dev/null 2>&1; then
        lsof -nP -iTCP:"$GATEWAY_PORT" -sTCP:LISTEN >&2 || true
    fi
    exit 1
}

wait_for_gateway_health() {
    local pid="$1"
    local deadline

    deadline=$((SECONDS + GATEWAY_START_TIMEOUT_SECS))
    while [ "$SECONDS" -lt "$deadline" ]; do
        if gateway_is_healthy; then
            return 0
        fi

        if ! kill -0 "$pid" >/dev/null 2>&1; then
            return 1
        fi

        sleep 1
    done

    return 1
}

start_gateway() {
    local gateway_pid

    echo "Starting OAuth gateway on $GATEWAY_HEALTH_URL"
    : >"$GATEWAY_LOG_PATH"
    "$GATEWAY_START_SCRIPT" >"$GATEWAY_LOG_PATH" 2>&1 </dev/null &
    gateway_pid=$!

    if wait_for_gateway_health "$gateway_pid"; then
        echo "OAuth gateway is healthy."
        return 0
    fi

    echo "Error: OAuth gateway did not become healthy within ${GATEWAY_START_TIMEOUT_SECS}s." >&2
    if kill -0 "$gateway_pid" >/dev/null 2>&1; then
        kill "$gateway_pid" >/dev/null 2>&1 || true
        wait "$gateway_pid" >/dev/null 2>&1 || true
    fi

    echo "Gateway log: $GATEWAY_LOG_PATH" >&2
    if [ -f "$GATEWAY_LOG_PATH" ]; then
        echo "Recent gateway log output:" >&2
        tail -40 "$GATEWAY_LOG_PATH" >&2 || true
    fi
    exit 1
}

main() {
    require_explicit_container_runtime "$@"
    load_gateway_config
    require_prereqs

    if gateway_is_healthy; then
        if prompt_existing_gateway_action; then
            :
        else
            stop_existing_gateway_listener
            start_gateway
        fi
    else
        if gateway_port_in_use; then
            print_unhealthy_listener_error
        fi
        start_gateway
    fi

    echo "Starting MCP container via $CONTAINER_RUN_SCRIPT with runtime $CONTAINER_RUNTIME"
    "$CONTAINER_RUN_SCRIPT" "$@"
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
    usage
    exit 0
fi

main "$@"
