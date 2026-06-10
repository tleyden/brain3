
<img width="800" alt="Image" src="https://github.com/user-attachments/assets/752e5eed-84c0-40cb-a3b3-104d79dd5210" />

An easy-to-use, local-first knowledge management system. Brain3 turns a pile of your notes, web clippings, ideas, and plans into a living, interconnected encyclopedia that an AI continuously maintains and you can chat with.

You access Brain3 directly from your preferred AI app (ChatGPT, Claude, Jan AI, etc. on desktop/web/mobile) — no separate app needed.

There is no cloud service required other than Cloudflare Tunnel, which is free to use.  Support for other cloud tunneling solutions like ngrok and tailscale funnels is planned. 

## Why This Exists

Brain3 is a user-friendly, agent-native implementation of the LLMWiki idea. It stores everything locally in markdown files in a fully private manner and is fully compatible with [Obsidian](https://obsidian.md).

You can easily migrate your traditional "second brain" document repository into Brain3. Currently only markdown is supported, but you can request imports from other formats via GitHub.

You can also view and update your Brain3 repository via Obsidian directly, however you need to use an AI app to harness the full LLMWiki capabilities.

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│  Your AI App                                                 │
│  (ChatGPT, Claude, Jan AI, etc. on desktop/web/mobile)       │
└──────────────────┬───────────────────────────────────────────┘
                   │ HTTPS (MCP over OAuth2)
                   ▼
┌──────────────────────────────────────────────────────────────┐
│  Cloudflare Proxy + Caddy/nginx                              │
│  TLS termination; reverse-proxies to localhost:8421          │
└──────────────────┬───────────────────────────────────────────┘
                   │ HTTP localhost:8421
                   ▼
┌──────────────────────────────────────────────────────────────┐
│  brain3-gateway  (Rust)                                      │
│  OAuth2 authorization server + MCP reverse proxy             │
│  Runs on host, handles auth, proxies MCP traffic             │
└──────────────────┬───────────────────────────────────────────┘
                   │ HTTP localhost:8420 + shared secret
                   ▼
┌──────────────────────────────────────────────────────────────┐
│  MCP Server  (macOS / Docker Container)                      │
│  Vault tools: read, write, search, list, move, delete        │
│  Read/write access to your local markdown vault              │
└──────────────────────────────────────────────────────────────┘
```

Two components run on your machine:

- **brain3-gateway** — A Rust binary that handles OAuth2 authentication and reverse-proxies MCP traffic to the container. Your AI app talks to this.
- **MCP server** — Runs inside a macOS Container or Docker container with access to your markdown vault. Provides the vault tools that let AI read, search, and edit your notes.

## Quickstart

### Prerequisites

| Requirement | Install |
|---|---|
| macOS (Apple Silicon) | — |
| Rust toolchain | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| Container runtime | Apple `container`: `brew install container` or Docker Desktop |
| `cloudflared` | `brew install cloudflared` *(optional — only needed for Cloudflare Tunnel approach)* |

### 1. Clone and build the gateway

```bash
git clone https://github.com/tleyden/brain3.git
cd brain3
cargo build --release
```

The binary is at `./target/release/brain3-gateway`.

No local MCP container build is required. Set `BRAIN3_CONTAINER_IMAGE` in `.env` to a published image such as `ghcr.io/tleyden/brain3-mcp-server:latest`. When the gateway starts, it pulls that configured image into your selected container runtime if it is not already present.

### 2. Configure

Copy the environment template:

```bash
cp .env.template .env
```

Generate the two secret values and set your login credentials:

```bash
# Generate secrets (run each command, paste the output into .env)
openssl rand -hex 32   # → paste as OAUTH2_GATEWAY_CLIENT_SECRET
openssl rand -hex 32   # → paste as OAUTH2_GATEWAY_ACCESS_TOKEN
```

Edit `.env` and fill in:

```
OAUTH2_GATEWAY_CLIENT_SECRET=<paste first secret>
OAUTH2_GATEWAY_ACCESS_TOKEN=<paste second secret>
USERNAME=your_username
PASSWORD=your_password
BRAIN3_CONTAINER_RUNTIME=macos-container
BRAIN3_VAULT_PATH=/absolute/path/to/your/vault
BRAIN3_CONTAINER_IMAGE=ghcr.io/tleyden/brain3-mcp-server:latest
DIRECT_PUBLIC_ORIGIN_HOSTNAME=brain3.yourdomain.com
```

The username and password are the credentials you will enter when your AI app prompts you to sign in.

Replace `/absolute/path/to/your/vault` with the absolute path to your Obsidian vault or markdown folder. Use `docker` instead of `macos-container` for `BRAIN3_CONTAINER_RUNTIME` if you prefer Docker.

Set `BRAIN3_CONTAINER_IMAGE` to the published image you want the gateway to run. If you want to pin the container version instead of following `latest`, use a published tag such as `ghcr.io/tleyden/brain3-mcp-server:v0.1.0`.

Set `DIRECT_PUBLIC_ORIGIN_HOSTNAME` to the public hostname your Caddy or nginx instance is serving TLS on (e.g. `brain3.yourdomain.com`). The gateway uses this for hostname validation.

### 3. Start the gateway

```bash
./target/release/brain3-gateway
```

The gateway starts the MCP server container automatically on startup, pulling the configured image first if it is not already cached locally. You will see log lines confirming the container is running before the HTTP server binds.

### 4. Verify locally

In another terminal:

```bash
# Health check
curl -s http://127.0.0.1:8421/health

# OAuth metadata (should show no registration_endpoint)
curl -s http://127.0.0.1:8421/.well-known/oauth-authorization-server | python3 -m json.tool
```

### 5. Expose via direct public origin

This setup uses a direct public origin: Caddy or nginx terminates TLS and reverse-proxies to `127.0.0.1:8421`, optionally sitting behind Cloudflare proxy for DDoS protection.

Example minimal Caddyfile:

```
brain3.yourdomain.com {
    reverse_proxy 127.0.0.1:8421
}
```

Make sure `DIRECT_PUBLIC_ORIGIN_HOSTNAME` in your `.env` matches the hostname Caddy is serving (e.g. `brain3.yourdomain.com`). The gateway enforces this to reject misdirected requests.

If you'd rather use a Cloudflare Tunnel instead, see [Exposing to the Public Internet](#exposing-to-the-public-internet) below.

### 6. Connect your AI app

In your preferred AI app (ChatGPT, Claude, Jan AI, etc. on desktop/web/mobile), add a new MCP server connection with:

| Field | Value |
|---|---|
| Server URL | `https://brain3.yourdomain.com` (your public hostname from step 5) |
| Client ID | `oauth2-gateway-client` (or whatever you set in `.env`) |
| Client Secret | The value of `OAUTH2_GATEWAY_CLIENT_SECRET` from your `.env` |

When the AI app redirects you to sign in, enter the `USERNAME` and `PASSWORD` you configured in step 3.

Once connected, you can ask your AI to read, search, organize, and edit your notes.

## Stopping

Stop the gateway with `Ctrl+C` in its terminal.

The MCP container keeps running after the gateway exits (it was started detached). Its name is derived from `BRAIN3_CONTAINER_IMAGE` by taking the final repository segment and dropping any tag or digest. For example, `ghcr.io/tleyden/brain3-mcp-server:latest` becomes `brain3-mcp-server`.

Stop it directly:

```bash
# macOS Container
container stop brain3-mcp-server

# Docker
docker stop brain3-mcp-server
```

## Configuration Reference

All configuration is via environment variables, loaded from a `.env` file.

### Gateway (brain3-gateway)

| Variable | Default | Description |
|---|---|---|
| `OAUTH2_GATEWAY_PORT` | `8421` | Port the gateway listens on |
| `OAUTH2_GATEWAY_CLIENT_ID` | `oauth2-gateway-client` | OAuth client ID accepted by the gateway |
| `OAUTH2_GATEWAY_CLIENT_SECRET` | *(required)* | OAuth client secret required at token exchange |
| `OAUTH2_GATEWAY_ACCESS_TOKEN` | *(required)* | Bearer token returned after successful auth |
| `OAUTH2_GATEWAY_MCP_UPSTREAM_URL` | `http://127.0.0.1:8420` | URL of the MCP server |
| `OAUTH2_GATEWAY_UPSTREAM_SECRET_FILE` | `/tmp/brain3-mcp-upstream-secret` | Path to the shared secret file |
| `OAUTH2_PKCE_REQUIRED` | `true` | Require PKCE for OAuth flow |
| `USERNAME` | *(required)* | Login username for the sign-in page |
| `PASSWORD` | *(required)* | Login password for the sign-in page |
| `OAUTH2_GATEWAY_ENFORCE_HOSTNAME_CHECK` | `true` | Reject requests for unexpected hostnames |
| `CF_QUICK_TUNNEL` | `false` | Set to `true` to have the gateway start a quick Cloudflare Tunnel on startup |
| `CF_TUNNEL_NAME` | *(empty)* | Named Cloudflare Tunnel name; gateway starts `cloudflared` automatically when set with `CF_DOMAIN` |
| `CF_DOMAIN` | *(empty)* | Cloudflare zone domain (used with `CF_TUNNEL_NAME`) |
| `CF_TUNNEL_CONFIG_FILE` | *(required if `CF_TUNNEL_NAME` set)* | Path to the `cloudflared` config file written during tunnel provisioning |
| `DIRECT_PUBLIC_ORIGIN_HOSTNAME` | *(empty)* | Public hostname when using direct origin (Caddy/nginx) instead of Cloudflare Tunnel |
| `BRAIN3_CONTAINER_RUNTIME` | *(empty = skip)* | `macos-container` or `docker`; if set, gateway starts the container on startup |
| `BRAIN3_VAULT_PATH` | *(required if runtime set)* | Absolute path to your Obsidian vault or markdown folder |
| `BRAIN3_CONTAINER_IMAGE` | *(required if runtime set)* | Published container image to run, e.g. `ghcr.io/tleyden/brain3-mcp-server:latest` |
| `BRAIN3_CONTAINER_HOST_PORT` | `8420` | Host loopback port published to the container |

## Tool Surface

Once connected, your AI app has access to these vault tools:

| Tool | Description |
|---|---|
| `vault_read` | Read a file or line range. Returns a content hash for safe patching. |
| `vault_create_overwrite_file` | Create a new note or replace an existing one entirely. |
| `vault_apply_unified_diff` | Apply a unified diff to an existing file. Preferred for precise edits. |
| `vault_batch_frontmatter_update` | Update YAML frontmatter fields across one or more files. |
| `vault_search` | Full-text search across the vault. |
| `vault_search_frontmatter` | Search by frontmatter field values. |
| `vault_list` | List files and directories in the vault. |
| `vault_move` | Move or rename a file. |
| `vault_delete` | Delete a file. |

## Security Model

- **Preregistered clients only.** Only the single client configured by `OAUTH2_GATEWAY_CLIENT_ID` and `OAUTH2_GATEWAY_CLIENT_SECRET` can obtain tokens. Dynamic client registration is disabled.
- **Client secret required** at token exchange. The token endpoint requires `client_secret_post`.
- **PKCE S256 required** by default. Protects against authorization code interception.
- **Host validation** rejects requests for unexpected hostnames (HTTP 421) when a public hostname is configured.
- **Bearer token validation** on all `/mcp` routes.
- **Upstream shared secret** injected by the gateway so the MCP container can reject direct calls that bypass the gateway.
- **Constant-time comparison** for all secret and token checks.
- **Auth codes** are single-use and expire after 5 minutes.
- **macOS Container isolation** — the MCP server runs in a container with only your vault directory mounted. All other access to your Mac is disabled by default.

## Exposing to the Public Internet

### Direct public origin (recommended)

If your machine already has a public IP or sits behind Cloudflare proxy, use Caddy or nginx to terminate TLS and reverse-proxy to `127.0.0.1:8421`. Set `DIRECT_PUBLIC_ORIGIN_HOSTNAME` in your `.env` to the public hostname; the gateway uses it for hostname validation.

Example minimal Caddyfile:

```
brain3.yourdomain.com {
    reverse_proxy 127.0.0.1:8421
}
```

### Quick temporary Cloudflare Tunnel (for testing)

Set `CF_QUICK_TUNNEL=true` in your `.env`. The gateway will start `cloudflared` automatically on startup and log the assigned public URL:

```
INFO tunnel started url=https://some-random-name.trycloudflare.com
```

No DNS or config needed. The URL changes each time you restart the gateway.

Requires `cloudflared` to be installed — see [install instructions](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/).

### Named Cloudflare Tunnel on your domain

For a stable hostname like `brain3.yourdomain.com` via Cloudflare Tunnel, provision the tunnel once (outside the gateway), then let the gateway run it:

1. Install `cloudflared` — see [install instructions](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/)
2. Log in: `cloudflared tunnel login`
3. Create the tunnel and DNS route (one-time setup — see Cloudflare docs)
4. In your `.env`, set `CF_TUNNEL_NAME`, `CF_DOMAIN`, and `CF_TUNNEL_CONFIG_FILE` (path to the `.yml` config file written during setup)
5. Start the gateway — it will start `cloudflared` automatically and log:

```
INFO tunnel started url=https://brain3.yourdomain.com
```

## Development

### Project structure

```
brain3/
├── apps/gateway/          # brain3-gateway binary (composition root)
├── crates/
│   ├── core/              # Domain model, business logic, port traits
│   └── platform/          # Adapters: Axum HTTP, reqwest proxy, config, auth store
├── scripts/               # Build, run, and stop scripts
└── RUST_DESIGN.md         # Detailed architecture documentation
```

The codebase uses hexagonal architecture. `core` contains pure business logic with no framework dependencies. `platform` implements the port traits using Axum, reqwest, and other infrastructure crates.

### Running tests

```bash
cargo test
```

### Building

```bash
cargo build --release
```

## Roadmap 

TODO

## Credits

- obsidian-web-mcp repo
