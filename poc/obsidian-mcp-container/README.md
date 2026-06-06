# obsidian-mcp-server

This fork is the authless MCP server half of the original `obsidian-web-mcp` codebase.

It keeps only:
- the HTTP MCP server
- vault read/write/search/list/move/delete tools
- frontmatter indexing
- filesystem safety checks

It intentionally removes:
- bearer auth middleware
- OAuth endpoints
- tunnel setup
- launchd packaging
- tests and planning docs copied from the source repo

## Configuration

Environment variables:
- `VAULT_PATH`: path to the Obsidian vault
- `VAULT_MCP_PORT`: HTTP port, defaults to `8420`
- `VAULT_MCP_ALLOWED_HOSTS`: optional comma-separated extra hosts for DNS rebinding protection

See [.env.template](.env.template).

## Entry Point

```bash
uv sync
uv run obsidian-mcp-server
```

Or:

```bash
./scripts/start-server.sh
```

## Scope

This POC fork is only the stripped server code. It does not include container packaging, OAuth, Cloudflare tunnel setup, or tests.
