#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
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
    echo "Generate secrets with generate_secrets.sh and add to .env"
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

uv run oauth2-gateway
