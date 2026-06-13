# Privacy & Security Details

Brain3 keeps your data on the local machine where it runs; your vault is never uploaded to any Brain3-managed cloud service. The AI assistant (Claude, ChatGPT, etc) sees only the parts of your vault exposed through your prompts and tool calls, leaving you in control of what is shared. You retain full control at all times and can stop Brain3, disable the tunnel, or disconnect your AI app whenever you choose.

## Architecture

- Host process written in Rust to minimize attack surface and reduce several classes of vulnerabilities.
- If you use Cloudflare Tunnel (default and recommended), `cloudflared` creates outbound-only connections to Cloudflare's network, so Brain3 does not need a publicly routable IP.
- ⚠ You should only use Brain3 if you trust Cloudflare: Cloudflare owns the root TLS certs and has the ability to decrypt traffic. See their [Cloudflare Transparency Report - H2 2025](https://www.cloudflare.com/transparency/).
- macOS or Docker container isolation. By default, the MCP server runs in a container with your vault mounted and a read-only upstream secret directory. No other host directories are mounted unless you enable dev mode.

## Authentication & Authorization

- Only the client you configure via `B3_OAUTH2_GATEWAY_CLIENT_ID` and `B3_OAUTH2_GATEWAY_CLIENT_SECRET` can get tokens — no open registration (DCR/CIMD are disabled).
- Client secret is required at token exchange (`client_secret_post`).
- PKCE (`S256`) enforced by default to prevent authorization-code interception.
- Auth codes are single-use and expire after 5 minutes.

## Request Integrity

- Bearer-token validation on all `/mcp` routes.
- Host validation rejects unexpected hostnames (HTTP 421) when a public hostname is configured.
- Upstream shared secret injected by Brain3 so the MCP container rejects direct calls that bypass Brain3.
- Constant-time comparison for all secret and token checks.
