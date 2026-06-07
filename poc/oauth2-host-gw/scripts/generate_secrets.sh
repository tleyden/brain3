#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="$REPO_ROOT/.env"
DEFAULT_PORT=8421
DEFAULT_CLIENT_ID=oauth2-gateway-client
NEW_CLIENT_SECRET="$(python3 -c "import secrets; print(secrets.token_hex(32))")"
NEW_ACCESS_TOKEN="$(python3 -c "import secrets; print(secrets.token_hex(32))")"
TMP_FILE="$(mktemp "${TMPDIR:-/tmp}/oauth2-host-gw-env.XXXXXX")"

cleanup() {
    rm -f "$TMP_FILE"
}

trap cleanup EXIT

if [ ! -f "$ENV_FILE" ]; then
    : > "$ENV_FILE"
fi

awk \
    -v default_port="$DEFAULT_PORT" \
    -v default_client_id="$DEFAULT_CLIENT_ID" \
    -v new_client_secret="$NEW_CLIENT_SECRET" \
    -v new_access_token="$NEW_ACCESS_TOKEN" \
    '
    BEGIN {
        saw_port = 0
        saw_client_id = 0
        saw_client_secret = 0
        saw_access_token = 0
    }
    /^OAUTH2_GATEWAY_PORT=/ || /^export OAUTH2_GATEWAY_PORT=/ {
        print $0
        saw_port = 1
        next
    }
    /^OAUTH2_GATEWAY_CLIENT_ID=/ || /^export OAUTH2_GATEWAY_CLIENT_ID=/ {
        print $0
        saw_client_id = 1
        next
    }
    /^OAUTH2_GATEWAY_CLIENT_SECRET=/ || /^export OAUTH2_GATEWAY_CLIENT_SECRET=/ {
        print "OAUTH2_GATEWAY_CLIENT_SECRET=" new_client_secret
        saw_client_secret = 1
        next
    }
    /^OAUTH2_GATEWAY_ACCESS_TOKEN=/ || /^export OAUTH2_GATEWAY_ACCESS_TOKEN=/ {
        print "OAUTH2_GATEWAY_ACCESS_TOKEN=" new_access_token
        saw_access_token = 1
        next
    }
    {
        print $0
    }
    END {
        if (!saw_port) {
            print "OAUTH2_GATEWAY_PORT=" default_port
        }
        if (!saw_client_id) {
            print "OAUTH2_GATEWAY_CLIENT_ID=" default_client_id
        }
        if (!saw_client_secret) {
            print "OAUTH2_GATEWAY_CLIENT_SECRET=" new_client_secret
        }
        if (!saw_access_token) {
            print "OAUTH2_GATEWAY_ACCESS_TOKEN=" new_access_token
        }
    }
    ' "$ENV_FILE" > "$TMP_FILE"

mv "$TMP_FILE" "$ENV_FILE"
chmod 600 "$ENV_FILE"
echo "Secrets updated in $ENV_FILE (mode 600)"
