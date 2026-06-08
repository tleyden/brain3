#!/usr/bin/env bash
set -euo pipefail

SECRET_FILE="${1:-/tmp/brain3-mcp-upstream-secret/upstream_secret}"
SECRET_DIR="$(dirname "$SECRET_FILE")"

if [ -f "$SECRET_DIR" ]; then
    LEGACY_SECRET_FILE="$SECRET_DIR"
    LEGACY_SECRET_BACKUP="${LEGACY_SECRET_FILE}.legacy"
    mv "$LEGACY_SECRET_FILE" "$LEGACY_SECRET_BACKUP"
    mkdir -p "$SECRET_DIR"
    if [ ! -s "$SECRET_FILE" ]; then
        cp "$LEGACY_SECRET_BACKUP" "$SECRET_FILE"
    fi
    rm -f "$LEGACY_SECRET_BACKUP"
else
    mkdir -p "$SECRET_DIR"
fi

if [ ! -s "$SECRET_FILE" ]; then
    umask 077
    python3 -c 'import secrets; print(secrets.token_hex(32))' > "$SECRET_FILE"
fi

chmod 600 "$SECRET_FILE"
printf '%s\n' "$SECRET_FILE"
