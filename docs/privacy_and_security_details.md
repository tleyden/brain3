# Privacy & Security Details

- 🛡️ Includes an [AI security audit](security_audit_claude_sonnet_4_6.md) available, which will be done regularly on releases.
- 🔒 Data isolation - the AI sees only the parts of your vault exposed through your prompts; your vault is never uploaded to any Brain3-managed cloud service
- <img height="16" src="logos/docker.svg" alt="Docker"> <img height="16" src="logos/apple.svg" alt="Apple"> Container-based filesystem and network isolation; no other host directories are mounted unless you enable dev mode
- <img height="16" src="logos/cloudflare.svg" alt="Cloudflare"> Secure Cloudflare tunnels with TLS; `cloudflared` creates outbound-only connections so Brain3 does not need a publicly routable IP
- <img height="16" src="logos/oauth.svg" alt="OAuth"> OAuth2.1 with PKCE to authenticate with the AI provider; only the client you configure can get tokens — no open registration (DCR/CIMD disabled)
- 🦀 Host process written in Rust to minimize attack surface and reduce several classes of vulnerabilities
- <img height="16" src="logos/fastmcp.svg" alt="FastMCP"> The MCP server running in the container uses the battle-tested FastMCP server framework
- You retain full control and can stop Brain3, disable the tunnel, or disconnect your AI app whenever you choose

## Trusted 3rd Parties

- ⚠ You should only use Brain3 if you trust Cloudflare: Cloudflare owns the root TLS certs and has the ability to decrypt traffic. See their [Cloudflare Transparency Report - H2 2025](https://www.cloudflare.com/transparency/).

## Authentication & Authorization

- Client secret is required at token exchange (`client_secret_post`).
- Auth codes are single-use and expire after 5 minutes.

## Request Integrity

- Bearer-token validation on all `/mcp` routes.
- Host validation rejects unexpected hostnames (HTTP 421) when a public hostname is configured.
- Upstream shared secret injected by Brain3 so the MCP container rejects direct calls that bypass Brain3.
- Constant-time comparison for all secret and token checks.
