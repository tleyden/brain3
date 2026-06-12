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

if [ -z "${B3_VAULT_MCP_ALLOWED_HOSTS:-}" ]; then
    echo "INFO: B3_VAULT_MCP_ALLOWED_HOSTS not set -- server only reachable via localhost"
fi

if [ ! -d ".venv" ]; then
    echo "Virtual environment not found."
    read -rp "Run 'uv sync' now? [Y/n] " answer
    answer=${answer:-Y}
    if [[ "$answer" =~ ^[Yy] ]]; then
        uv sync
    fi
fi

B3_VAULT_PATH=${B3_VAULT_PATH:-"$HOME/obsidian_vaults"} uv run brain3-mcp-vault-tools
