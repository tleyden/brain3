#!/usr/bin/env bash
set -euo pipefail

# shellcheck source=./cf-tunnel-common.sh
source "$(cd "$(dirname "$0")" && pwd)/cf-tunnel-common.sh"

echo "=== Cloudflare Named Tunnel Setup ==="
echo

cf_tunnel_load_env
cf_tunnel_require_cloudflared
cf_tunnel_require_cloudflare_login

echo "Tunnel name: $CF_TUNNEL_NAME"
echo "Public hostname: $CF_TUNNEL_HOSTNAME"
echo

cf_tunnel_ensure_named_tunnel
cf_tunnel_require_credentials_file
cf_tunnel_write_config
cf_tunnel_ensure_dns_route

echo
echo "=== Setup complete ==="
echo "Config file: $CF_TUNNEL_CONFIG_FILE"
echo "Credentials file: $CF_TUNNEL_CREDENTIALS_FILE"
echo
echo "Next:"
echo "  ./scripts/start-oauth2-server.sh"
echo "  ./scripts/run-cf-tunnel-with-domain.sh"
echo
cf_tunnel_print_quick_tunnel_hint
