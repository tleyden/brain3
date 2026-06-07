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

By default the gateway binds only to `127.0.0.1`. That default is correct for:
- Cloudflare Tunnel
- a same-host reverse proxy such as Caddy or nginx

To expose it on all interfaces intentionally, pass an explicit host:

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

Do this only if you intentionally want the app itself listening on the network. It is not required for either the tunnel flow or a same-host reverse proxy.

## Public Exposure Options

If this machine already has a public IPv4 address, Cloudflare Tunnel is not your only option.

There are two main ways to expose this gateway publicly:

1. Cloudflare Tunnel
   Recommended default for this repo. Public HTTPS terminates at Cloudflare, and `cloudflared` forwards to local `http://localhost:8421`. No inbound ports need to be open on the host.

2. Direct public origin behind Cloudflare proxy
   Use this when the machine is already publicly reachable and you want Cloudflare DNS to point at the server directly. In that setup, you open inbound HTTPS on the host and run Caddy or another reverse proxy in front of this app.

Cloudflare Tunnel is usually the safer default because it avoids inbound ports on the host entirely. Direct origin can also be secure, but it requires more careful firewalling, TLS, and reverse proxy setup.

## Cloudflare Tunnel

There are two supported tunnel flows in this repo.

### Quick temporary tunnel

Use this for quick testing. It does not need named tunnel setup, Cloudflare DNS, or extra `.env` values.

```bash
cloudflared tunnel --url http://localhost:8421
```

### Named tunnel on your domain

Use this only if you want a stable hostname such as `<tunnel-name>.<your-domain>`.

For this flow, the public HTTPS connection terminates at Cloudflare. `cloudflared` then forwards requests to the local gateway over `http://localhost:8421`. You do not need Caddy or local TLS for this path.

1. Install `cloudflared`.
2. Fill in `CF_TUNNEL_NAME` and `CF_DOMAIN` in `.env`.
3. Log into Cloudflare:

```bash
cloudflared tunnel login
```

When Cloudflare asks which domain to authorize, choose the zone that matches `CF_DOMAIN`.

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

### What `cert.pem` is

After `cloudflared tunnel login`, Cloudflare saves a file like:

```text
~/.cloudflared/cert.pem
```

This file is easy to misunderstand.

- It is not the TLS certificate your users see in the browser.
- It is not an origin certificate for Caddy, nginx, or this Python app.
- It is an account-scoped credential used by `cloudflared` to manage locally managed tunnels and create DNS routes.

Treat `cert.pem` as sensitive. Do not casually copy it to other machines. Anyone with that file can create, list, delete, or reroute locally managed tunnels for the associated Cloudflare account/zone.

By contrast, the tunnel-specific credentials file (`~/.cloudflared/<tunnel-uuid>.json`) is narrower in scope and is used to run that specific tunnel.

## Direct Public Origin Behind Cloudflare Proxy

Use this if the machine already has a public IPv4 address and you want to expose the gateway without Cloudflare Tunnel.

Typical shape:

1. Create a hostname in Cloudflare DNS pointing at the server's public IP.
2. Open inbound `443` on the host. Optionally open `80` only to redirect to HTTPS.
3. Run Caddy or another reverse proxy on the machine.
4. Terminate TLS at that reverse proxy.
5. Reverse-proxy to this gateway on `127.0.0.1:8421`.

Notes:

- This repo does not currently ship helper scripts for the direct-origin path.
- In the direct-origin path, Caddy or nginx is the component that handles local TLS, not `cloudflared`.
- If you proxy through Cloudflare, use an origin TLS setup that matches your Cloudflare SSL/TLS mode. For example, `Full (Strict)` requires a valid origin certificate on the server.

## Scope

This POC fork is the stripped OAuth code plus helper scripts for local startup and optional Cloudflare Tunnel exposure. It does not include MCP server code, vault tooling, or the original app surface area.
