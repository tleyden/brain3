#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
POC_ROOT="$(cd "$REPO_ROOT/.." && pwd)"
ENSURE_UPSTREAM_SECRET="$POC_ROOT/scripts/ensure-mcp-upstream-secret.sh"
cd "$REPO_ROOT"

if [ -f ".env" ]; then
    set -o allexport
    # shellcheck source=../.env
    source .env
    set +o allexport
fi

missing=()
[ -z "${OAUTH2_GATEWAY_CLIENT_SECRET:-}" ] && missing+=("OAUTH2_GATEWAY_CLIENT_SECRET")
[ -z "${OAUTH2_GATEWAY_ACCESS_TOKEN:-}" ] && missing+=("OAUTH2_GATEWAY_ACCESS_TOKEN")

if [ ${#missing[@]} -gt 0 ]; then
    echo "ERROR: missing required values: ${missing[*]}"
    echo "Generate secrets with generate_secrets.sh, which will automatically update or create .env"
    exit 1
fi

if [ ! -d ".venv" ]; then
    echo "Virtual environment not found."
    read -rp "Run 'uv sync' now? [Y/n] " answer
    answer=${answer:-Y}
    if [[ "$answer" =~ ^[Yy] ]]; then
        uv sync
    fi
fi

if [ ! -x "$ENSURE_UPSTREAM_SECRET" ]; then
    echo "ERROR: missing helper script: $ENSURE_UPSTREAM_SECRET"
    exit 1
fi

export OAUTH2_GATEWAY_UPSTREAM_SECRET_FILE="$("$ENSURE_UPSTREAM_SECRET")"

uv run oauth2-gateway "$@"
