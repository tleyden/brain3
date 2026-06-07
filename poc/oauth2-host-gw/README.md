# oauth2-gateway

This fork is the OAuth-only half of the original `obsidian-web-mcp` codebase.

It keeps only:
- OAuth metadata discovery
- dynamic client registration
- authorization-code redirect handling
- token exchange with PKCE support
- a tiny CLI HTTP runner

It intentionally removes:
- MCP server registration
- vault tools
- frontmatter indexing
- filesystem access
- tunnel setup
- launchd packaging
- tests and planning docs copied from the source repo

## Configuration

Environment variables:
- `OAUTH2_GATEWAY_PORT`: HTTP port, defaults to `8421`
- `OAUTH2_GATEWAY_CLIENT_ID`: client id returned by registration
- `OAUTH2_GATEWAY_CLIENT_SECRET`: client secret returned by registration and accepted by token exchange
- `OAUTH2_GATEWAY_ACCESS_TOKEN`: static bearer token returned after successful token exchange

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

## Scope

This POC fork is only the stripped OAuth code. It does not include MCP server code, proxying, tunnel setup, or tests.
