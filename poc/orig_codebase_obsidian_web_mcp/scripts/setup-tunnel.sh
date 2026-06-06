#!/usr/bin/env bash
set -euo pipefail

echo "=== Obsidian Vault MCP -- Cloudflare Tunnel Setup ==="
echo ""

# Check prerequisites
command -v cloudflared >/dev/null 2>&1 || {
    echo "cloudflared not found. Install with: brew install cloudflare/cloudflare/cloudflared"
    exit 1
}

# Step 1: Auth (if not already done)
echo "Step 1: Authenticate with Cloudflare (opens browser if needed)"
cloudflared tunnel login

# Step 2: Create tunnel
echo ""
echo "Step 2: Creating tunnel 'vault-mcp'"
cloudflared tunnel create vault-mcp || echo "Tunnel may already exist, continuing..."

# Step 3: Get tunnel ID
TUNNEL_ID=$(cloudflared tunnel list | grep vault-mcp | awk '{print $1}')
echo "Tunnel ID: $TUNNEL_ID"

if [ -z "$TUNNEL_ID" ]; then
    echo "ERROR: Could not determine tunnel ID"
    exit 1
fi

# Step 4: Write config
# Replace vault-mcp.example.com with your actual domain
HOSTNAME="${VAULT_MCP_HOSTNAME:-vault-mcp.example.com}"

CONFIG_DIR="$HOME/.cloudflared"
mkdir -p "$CONFIG_DIR"
cat > "$CONFIG_DIR/config-vault-mcp.yml" << EOF
tunnel: $TUNNEL_ID
credentials-file: $CONFIG_DIR/$TUNNEL_ID.json

ingress:
  - hostname: $HOSTNAME
    service: http://localhost:8420
  - service: http_status:404
EOF

echo "Config written to $CONFIG_DIR/config-vault-mcp.yml"

# Step 5: DNS
echo ""
echo "Step 5: Adding DNS record"
cloudflared tunnel route dns vault-mcp "$HOSTNAME" || echo "DNS record may already exist."

echo ""
echo "=== Setup complete ==="
echo ""
echo "To run the tunnel manually:"
echo "  cloudflared tunnel --config $CONFIG_DIR/config-vault-mcp.yml run vault-mcp"
echo ""
echo "To generate an auth token, run:"
echo "  python -c \"import secrets; print(secrets.token_hex(32))\""
echo ""
echo "Set that token as VAULT_MCP_TOKEN in your environment."
