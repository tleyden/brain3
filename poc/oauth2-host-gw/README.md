# oauth2-gateway

This fork is the OAuth-only half of the original `obsidian-web-mcp` codebase.

It keeps only:
- OAuth metadata discovery
- dynamic client registration
- authorization-code redirect handling
- token exchange with PKCE support
- a tiny CLI HTTP runner
- optional helper scripts for Cloudflare Tunnel exposure

It intentionally removes:
- MCP server registration
- vault tools
- frontmatter indexing
- filesystem access
- launchd packaging
- most non-OAuth code copied from the source repo

## Configuration

Environment variables:
- `OAUTH2_GATEWAY_PORT`: HTTP port, defaults to `8421`
- `OAUTH2_GATEWAY_CLIENT_ID`: client id returned by registration
- `OAUTH2_GATEWAY_CLIENT_SECRET`: client secret returned by registration and accepted by token exchange
- `OAUTH2_GATEWAY_ACCESS_TOKEN`: static bearer token returned after successful token exchange
- `CF_TUNNEL_NAME`: optional, only for a named Cloudflare tunnel on your domain
- `CF_DOMAIN`: optional, only for a named Cloudflare tunnel on your domain

See [.env.template](.env.template).

## Prerequisites

Install `uv` first.

### macOS

```bash
brew install uv
```

### Linux

```bash
curl -LsSf https://astral.sh/uv/install.sh | sh
```

If `uv` is not on your `PATH` after the Linux install, restart your shell or add `~/.local/bin` to `PATH`.

## Entry Point

Once `uv` is installed, the run commands are the same on macOS and Linux.

```bash
uv sync
uv run oauth2-gateway
```

By default the gateway binds only to `127.0.0.1`. To expose it on all interfaces intentionally, pass an explicit host:

```bash
uv run oauth2-gateway --host 0.0.0.0
```

Or:

```bash
./scripts/start-oauth2-server.sh
```

Or with the wrapper script:

```bash
./scripts/start-oauth2-server.sh --host 0.0.0.0
```

## Cloudflare Tunnel

There are two supported tunnel flows.

### Quick temporary tunnel

Use this for quick testing. It does not need named tunnel setup, Cloudflare DNS, or extra `.env` values.

```bash
cloudflared tunnel --url http://localhost:8421
```

### Named tunnel on your domain

Use this only if you want a stable hostname such as `<tunnel-name>.<your-domain>`.

1. Install `cloudflared`.
2. Fill in `CF_TUNNEL_NAME` and `CF_DOMAIN` in `.env`.
3. Log into Cloudflare:

```bash
cloudflared tunnel login
```

4. Run setup once:

```bash
./scripts/setup-cf-tunnel-with-domain.sh
```

5. Start the OAuth server:

```bash
./scripts/start-oauth2-server.sh
```

6. Start the named tunnel:

```bash
./scripts/run-cf-tunnel-with-domain.sh
```

The setup script validates `.env`, checks that `cloudflared` is installed, checks Cloudflare login state, creates or reuses the named tunnel, writes project-local config in `.cloudflared/`, and ensures the DNS route exists.

## Scope

This POC fork is the stripped OAuth code plus helper scripts for local startup and optional Cloudflare Tunnel exposure. It does not include MCP server code, vault tooling, or the original app surface area.
