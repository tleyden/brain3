# Brain3

An easy-to-use, local-first knowledge management system. Brain3 turns a pile of your notes, web clippings, ideas, and plans into a living, interconnected encyclopedia that an AI continuously maintains and you can chat with.

You access Brain3 directly from your preferred AI app (ChatGPT, Claude, Jan AI, etc. on desktop/web/mobile) — no separate app needed.

## Why This Exists

Brain3 is a user-friendly, agent-native implementation of the [LLMWiki](https://tleyden.github.io/blog/2024/12/22/llmwiki/) idea. It stores everything locally in markdown files in a fully private manner and is fully compatible with [Obsidian](https://obsidian.md).

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
│  Cloudflare Tunnel                                           │
│  (encrypted tunnel to your machine — no open ports needed)   │
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
- **MCP server** — Runs inside a macOS Container with access to your markdown vault. Provides the vault tools that let AI read, search, and edit your notes.

## Quickstart

### Prerequisites

| Requirement | Install |
|---|---|
| macOS (Apple Silicon) | — |
| Rust toolchain | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| Apple `container` CLI | `brew install container` |
| `cloudflared` | `brew install cloudflared` |
| Python 3 | Bundled with macOS or `brew install python` |
| `uv` | `brew install uv` |

### 1. Clone and build the gateway

```bash
git clone https://github.com/tleyden/brain3.git
cd brain3
cargo build --release
```

The binary is at `./target/release/brain3-gateway`.

### 2. Build the MCP server container image

```bash
./scripts/build-mcp-container.sh --container-runtime macos-container
```

### 3. Configure

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
```

The username and password are the credentials you will enter when your AI app prompts you to sign in.

### 4. Create the upstream shared secret

The gateway authenticates to the MCP server with a shared secret file. Generate it:

```bash
./scripts/ensure-mcp-upstream-secret.sh
```

This creates a secret at `/tmp/brain3-mcp-upstream-secret/upstream_secret` (the default path the gateway expects).

### 5. Start the MCP server container

```bash
./scripts/run-mcp-container.sh \
  --container-runtime macos-container \
  --vault-path /absolute/path/to/your/vault
```

Replace `/absolute/path/to/your/vault` with the absolute path to your Obsidian vault or markdown folder.

### 6. Start the gateway

```bash
./target/release/brain3-gateway
```

### 7. Verify locally

In another terminal:

```bash
# Health check
curl -s http://127.0.0.1:8421/health

# OAuth metadata (should show no registration_endpoint)
curl -s http://127.0.0.1:8421/.well-known/oauth-authorization-server | python3 -m json.tool
```

### 8. Expose via Cloudflare Tunnel

For your AI app to reach Brain3, you need a public URL. The quickest way:

```bash
cloudflared tunnel --url http://localhost:8421
```

`cloudflared` prints a temporary public URL like `https://xxx-yyy-zzz.trycloudflare.com`. Copy this URL.

### 9. Connect your AI app

In your preferred AI app (ChatGPT, Claude, Jan AI, etc. on desktop/web/mobile), add a new MCP server connection with:

| Field | Value |
|---|---|
| Server URL | The Cloudflare Tunnel URL from step 8 |
| Client ID | `oauth2-gateway-client` (or whatever you set in `.env`) |
| Client Secret | The value of `OAUTH2_GATEWAY_CLIENT_SECRET` from your `.env` |

When the AI app redirects you to sign in, enter the `USERNAME` and `PASSWORD` you configured in step 3.

Once connected, you can ask your AI to read, search, organize, and edit your notes.

## Stopping

Stop the gateway with `Ctrl+C` in its terminal.

Stop the MCP container:

```bash
./scripts/stop.sh --container-runtime macos-container
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
| `CF_TUNNEL_NAME` | *(empty)* | Named Cloudflare Tunnel name (for stable hostname) |
| `CF_DOMAIN` | *(empty)* | Cloudflare domain (used with `CF_TUNNEL_NAME`) |
| `DIRECT_PUBLIC_ORIGIN_HOSTNAME` | *(empty)* | Public hostname when using direct origin instead of tunnel |

### MCP Server Container

| Variable | Default | Description |
|---|---|---|
| `VAULT_PATH` | *(required)* | Absolute path to your markdown vault |
| `VAULT_MCP_PORT` | `8420` | Port the MCP server listens on |
| `VAULT_MCP_ALLOWED_HOSTS` | *(empty)* | Extra allowed Host headers (comma-separated) |

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

### Quick temporary tunnel (recommended for testing)

```bash
cloudflared tunnel --url http://localhost:8421
```

No DNS, no config — just a temporary public URL. The URL changes each time you restart the tunnel.

### Named tunnel on your domain

For a stable hostname like `brain3.yourdomain.com`:

1. Log into Cloudflare: `cloudflared tunnel login`
2. Set `CF_TUNNEL_NAME` and `CF_DOMAIN` in your `.env`
3. Run setup once: `./scripts/setup-cf-tunnel-with-domain.sh`
4. Start the tunnel: `./scripts/run-cf-tunnel-with-domain.sh`

### Direct public origin

If your machine already has a public IP, you can use Caddy or nginx to terminate TLS and reverse-proxy to `127.0.0.1:8421`. Set `DIRECT_PUBLIC_ORIGIN_HOSTNAME` in your `.env` to the public hostname. See the configuration reference above for details.

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
