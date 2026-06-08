#!/usr/bin/env bash
set -euo pipefail

SECRET_FILE="${1:-/tmp/agentzoo-mcp-upstream-secret}"

mkdir -p "$(dirname "$SECRET_FILE")"

if [ ! -s "$SECRET_FILE" ]; then
    umask 077
    python3 -c 'import secrets; print(secrets.token_hex(32))' > "$SECRET_FILE"
fi

chmod 600 "$SECRET_FILE"
printf '%s\n' "$SECRET_FILE"
