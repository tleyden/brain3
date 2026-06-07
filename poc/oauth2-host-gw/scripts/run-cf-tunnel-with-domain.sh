#!/usr/bin/env bash
set -euo pipefail

# shellcheck source=./cf-tunnel-common.sh
source "$(cd "$(dirname "$0")" && pwd)/cf-tunnel-common.sh"

cf_tunnel_load_env
cf_tunnel_require_cloudflared
cf_tunnel_require_config_file
cf_tunnel_load_credentials_from_config
cf_tunnel_require_credentials_file
cf_tunnel_require_local_service

echo "Starting Cloudflare tunnel: $CF_TUNNEL_NAME"
echo "Public hostname: $CF_TUNNEL_HOSTNAME"
echo "Forwarding to: http://localhost:${CF_TUNNEL_PORT}"
echo

exec cloudflared tunnel --config "$CF_TUNNEL_CONFIG_FILE" run "$CF_TUNNEL_NAME"
