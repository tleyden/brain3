#!/usr/bin/env bash

CF_TUNNEL_SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CF_TUNNEL_REPO_ROOT="$(cd "$CF_TUNNEL_SCRIPT_DIR/.." && pwd)"
CF_TUNNEL_ENV_FILE="$CF_TUNNEL_REPO_ROOT/.env"
CF_TUNNEL_TEMPLATE_FILE="$CF_TUNNEL_REPO_ROOT/.env.template"
CF_TUNNEL_CONFIG_DIR="$CF_TUNNEL_REPO_ROOT/.cloudflared"
CF_TUNNEL_PORT=8421

cf_tunnel_print_quick_tunnel_hint() {
    cat <<EOF
For a temporary Cloudflare tunnel with no named tunnel setup, use:
  cloudflared tunnel --url http://localhost:${CF_TUNNEL_PORT}
EOF
}

cf_tunnel_fail() {
    echo "ERROR: $*" >&2
    exit 1
}

cf_tunnel_require_env_file() {
    if [ -f "$CF_TUNNEL_ENV_FILE" ]; then
        return 0
    fi

    cat <<EOF >&2
ERROR: $CF_TUNNEL_ENV_FILE was not found.

Named tunnels require a .env file with:
  CF_TUNNEL_NAME=
  CF_DOMAIN=

Create it from the template:
  cp "$CF_TUNNEL_TEMPLATE_FILE" "$CF_TUNNEL_ENV_FILE"
EOF
    echo >&2
    cf_tunnel_print_quick_tunnel_hint >&2
    exit 1
}

cf_tunnel_load_env() {
    cf_tunnel_require_env_file

    set -o allexport
    # shellcheck source=../.env
    source "$CF_TUNNEL_ENV_FILE"
    set +o allexport

    cf_tunnel_validate_required_env

    CF_TUNNEL_HOSTNAME="${CF_TUNNEL_NAME}.${CF_DOMAIN}"
    CF_TUNNEL_CONFIG_FILE="$CF_TUNNEL_CONFIG_DIR/${CF_TUNNEL_NAME}.yml"
}

cf_tunnel_validate_required_env() {
    local missing=()
    local key

    [ -z "${CF_TUNNEL_NAME:-}" ] && missing+=("CF_TUNNEL_NAME")
    [ -z "${CF_DOMAIN:-}" ] && missing+=("CF_DOMAIN")

    if [ ${#missing[@]} -gt 0 ]; then
        echo "ERROR: missing required values in $CF_TUNNEL_ENV_FILE:" >&2
        for key in "${missing[@]}"; do
            echo "  - $key" >&2
        done
        echo >&2
        echo "Named tunnels require both CF_TUNNEL_NAME and CF_DOMAIN." >&2
        echo >&2
        cf_tunnel_print_quick_tunnel_hint >&2
        exit 1
    fi

    cf_tunnel_validate_tunnel_name "$CF_TUNNEL_NAME"
    cf_tunnel_validate_domain "$CF_DOMAIN"
}

cf_tunnel_validate_tunnel_name() {
    local tunnel_name="$1"

    if [[ "$tunnel_name" == *" "* ]]; then
        cf_tunnel_fail "CF_TUNNEL_NAME must not contain spaces"
    fi

    if [ ${#tunnel_name} -gt 63 ]; then
        cf_tunnel_fail "CF_TUNNEL_NAME must be 63 characters or fewer"
    fi

    if [[ ! "$tunnel_name" =~ ^[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?$ ]]; then
        cf_tunnel_fail "CF_TUNNEL_NAME must use only letters, digits, or hyphens, and cannot start or end with a hyphen"
    fi
}

cf_tunnel_validate_domain() {
    local domain="$1"
    local label
    local -a labels

    if [[ "$domain" == *" "* ]]; then
        cf_tunnel_fail "CF_DOMAIN must not contain spaces"
    fi

    if [[ "$domain" == .* || "$domain" == *. ]]; then
        cf_tunnel_fail "CF_DOMAIN must not start or end with a dot"
    fi

    if [ ${#domain} -gt 253 ]; then
        cf_tunnel_fail "CF_DOMAIN must be 253 characters or fewer"
    fi

    IFS='.' read -r -a labels <<< "$domain"

    if [ ${#labels[@]} -lt 2 ]; then
        cf_tunnel_fail "CF_DOMAIN must look like a zone name such as example.com"
    fi

    for label in "${labels[@]}"; do
        if [ -z "$label" ]; then
            cf_tunnel_fail "CF_DOMAIN must not contain empty labels"
        fi

        if [ ${#label} -gt 63 ]; then
            cf_tunnel_fail "Each CF_DOMAIN label must be 63 characters or fewer"
        fi

        if [[ ! "$label" =~ ^[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?$ ]]; then
            cf_tunnel_fail "CF_DOMAIN labels must use only letters, digits, or hyphens, and cannot start or end with a hyphen"
        fi
    done
}

cf_tunnel_require_cloudflared() {
    if command -v cloudflared >/dev/null 2>&1; then
        return 0
    fi

    cat <<EOF >&2
ERROR: cloudflared is not installed.

Install it with:
  brew install cloudflare/cloudflare/cloudflared
EOF
    echo >&2
    cf_tunnel_print_quick_tunnel_hint >&2
    exit 1
}

cf_tunnel_require_cloudflare_login() {
    if cloudflared tunnel list >/dev/null 2>&1; then
        return 0
    fi

    cat <<EOF >&2
ERROR: cloudflared is not logged into Cloudflare for locally managed named tunnels.

Run:
  cloudflared tunnel login

When Cloudflare asks which domain to authorize for the tunnel, choose:
  $CF_DOMAIN

Then rerun this script.
EOF
    echo >&2
    cf_tunnel_print_quick_tunnel_hint >&2
    exit 1
}

cf_tunnel_refresh_state() {
    CF_TUNNEL_ID="$(cloudflared tunnel list -n "$CF_TUNNEL_NAME" | awk -v name="$CF_TUNNEL_NAME" 'NR > 1 && $2 == name { print $1; exit }')"
    if [ -n "${CF_TUNNEL_ID:-}" ]; then
        CF_TUNNEL_CREDENTIALS_FILE="$HOME/.cloudflared/${CF_TUNNEL_ID}.json"
    else
        CF_TUNNEL_CREDENTIALS_FILE=""
    fi
}

cf_tunnel_ensure_named_tunnel() {
    cf_tunnel_refresh_state

    if [ -n "${CF_TUNNEL_ID:-}" ]; then
        echo "Using existing Cloudflare tunnel: $CF_TUNNEL_NAME"
        return 0
    fi

    echo "Creating Cloudflare tunnel: $CF_TUNNEL_NAME"
    if ! cloudflared tunnel create "$CF_TUNNEL_NAME"; then
        cf_tunnel_fail "cloudflared could not create tunnel '$CF_TUNNEL_NAME'"
    fi

    cf_tunnel_refresh_state
    if [ -z "${CF_TUNNEL_ID:-}" ]; then
        cf_tunnel_fail "tunnel '$CF_TUNNEL_NAME' was created but its ID could not be determined"
    fi
}

cf_tunnel_require_credentials_file() {
    if [ -n "${CF_TUNNEL_CREDENTIALS_FILE:-}" ] && [ -f "$CF_TUNNEL_CREDENTIALS_FILE" ]; then
        return 0
    fi

    cat <<EOF >&2
ERROR: the tunnel credentials file is missing:
  $CF_TUNNEL_CREDENTIALS_FILE

If this tunnel was created on another machine, restore the credentials file or recreate the tunnel on this machine.

Then run:
  ./scripts/setup-cf-tunnel-with-domain.sh
EOF
    exit 1
}

cf_tunnel_write_config() {
    mkdir -p "$CF_TUNNEL_CONFIG_DIR"

    cat > "$CF_TUNNEL_CONFIG_FILE" <<EOF
tunnel: $CF_TUNNEL_ID
credentials-file: $CF_TUNNEL_CREDENTIALS_FILE

ingress:
  - hostname: $CF_TUNNEL_HOSTNAME
    service: http://localhost:${CF_TUNNEL_PORT}
  - service: http_status:404
EOF

    echo "Wrote Cloudflare tunnel config: $CF_TUNNEL_CONFIG_FILE"
}

cf_tunnel_require_config_file() {
    if [ -f "$CF_TUNNEL_CONFIG_FILE" ]; then
        return 0
    fi

    cat <<EOF >&2
ERROR: the tunnel config file is missing:
  $CF_TUNNEL_CONFIG_FILE

Run the setup script first:
  ./scripts/setup-cf-tunnel-with-domain.sh
EOF
    exit 1
}

cf_tunnel_load_credentials_from_config() {
    CF_TUNNEL_CREDENTIALS_FILE="$(awk '
        /^credentials-file:/ {
            sub(/^credentials-file:[[:space:]]*/, "")
            print
            exit
        }
    ' "$CF_TUNNEL_CONFIG_FILE")"

    if [ -n "${CF_TUNNEL_CREDENTIALS_FILE:-}" ]; then
        return 0
    fi

    cat <<EOF >&2
ERROR: credentials-file was not found in:
  $CF_TUNNEL_CONFIG_FILE

Run the setup script again:
  ./scripts/setup-cf-tunnel-with-domain.sh
EOF
    exit 1
}

cf_tunnel_ensure_dns_route() {
    echo "Ensuring DNS route: $CF_TUNNEL_HOSTNAME"
    if ! cloudflared tunnel route dns --overwrite-dns "$CF_TUNNEL_NAME" "$CF_TUNNEL_HOSTNAME"; then
        cf_tunnel_fail "cloudflared could not route $CF_TUNNEL_HOSTNAME to tunnel '$CF_TUNNEL_NAME'"
    fi
}

cf_tunnel_require_local_service() {
    if command -v lsof >/dev/null 2>&1 && lsof -nP -iTCP:${CF_TUNNEL_PORT} -sTCP:LISTEN >/dev/null 2>&1; then
        return 0
    fi

    if command -v nc >/dev/null 2>&1 && nc -z 127.0.0.1 "${CF_TUNNEL_PORT}" >/dev/null 2>&1; then
        return 0
    fi

    cat <<EOF >&2
ERROR: nothing appears to be listening on 127.0.0.1:${CF_TUNNEL_PORT}.

Start the OAuth server first:
  ./scripts/start-oauth2-server.sh
EOF
    exit 1
}
