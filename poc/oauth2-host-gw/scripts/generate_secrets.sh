#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="$REPO_ROOT/.env"

if [ -f "$ENV_FILE" ]; then
    echo ".env already exists at $ENV_FILE"
    read -rp "Overwrite? [y/N] " answer
    answer=${answer:-N}
    if [[ ! "$answer" =~ ^[Yy] ]]; then
        echo "Aborted."
        exit 0
    fi
fi

cat > "$ENV_FILE" <<EOF
OAUTH2_GATEWAY_PORT=8421
OAUTH2_GATEWAY_CLIENT_ID=oauth2-gateway-client
OAUTH2_GATEWAY_CLIENT_SECRET=$(python3 -c "import secrets; print(secrets.token_hex(32))")
OAUTH2_GATEWAY_ACCESS_TOKEN=$(python3 -c "import secrets; print(secrets.token_hex(32))")
EOF

chmod 600 "$ENV_FILE"
echo "Secrets written to $ENV_FILE (mode 600)"
