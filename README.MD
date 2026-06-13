
<img width="800" alt="Brain3 Architecture Diagram" src="docs/screenshots/brain3_architecture_diagram.png" />

Brain3 let's you interact with your markdown vault ("second brain") through Claude and ChatGPT.

I built this as my second "agent native" AI app, after building a personal flashcard app for learning languages (and loving it), I realized that I really wanted to access my Obsidian markdown vault in the seame way.  

I looked into OpenClaw and Hermes, but they both felt like overkill for my needs.  I also found [obsidian-web-mcp](https://github.com/jimprosser/obsidian-web-mcp) which provided an inspiring starting point, but I wanted a container based security-first architecture.

Brain3 lets you use any AI that supports the MCP standard protocol for connectors, such as Claude, ChatGPT, and Jan.ai (open source which supports local models).  It will work on their web, desktop and mobile apps, and you can even use the built-in voice dictation features.  This makes your interactions with your markdown vault quick and effortless, especially on mobile.

Your markdown vault data stays 100% local and private.  This project is 100% designed with security and privacy in mind, and uses container technology to reduce the attack surface.  There is an [AI security audit](docs/security_audit.md) available, which will be done regularly on releases. 

Brain3 only runs in the command line as a Terminal UI (TUI) for now, and has only been tested on MacOS and Linux.  Eventually it will be available as a standalone desktop app for less technical users.

## Features

- Interact with a directory of local files with your favorite AI app from Desktop, Mobile and Web. (read, summarize, create, edit)
- Obsidian-compatible, but Obsidian not required
- TUI app for setup
- **Not AI Slop** - I'm using coding agents, but I'm following the [bettervibe best practices](https://bettervibe.org/guide/) (of which I'm a co-author) to ensure that the quality bar stays high.

## Screenshots (w/ Claude Mobile)

<table><tr>
<td><img width="280" alt="Image" src="docs/screenshots/screenshot-2.png" /></td>
<td><img width="280" alt="Image" src="docs/screenshots/screenshot-1.png" /></td>
</tr><tr>
<td colspan="2" align="center">Chatting w/ Brain3 MCP and showing tool calls</td>
</tr></table>

## Privacy and Security

- Your data stays 100% local and private - the AI assistant only sees a part of your vault based on your prompts
- Designed with security and privacy in mind, using container-based isolation and a free Cloudflare tunnel
- OAuth2.1 with PKCE is used to authenticate with the AI provider
- Host process written in Rust to avoid several classes of vulnerabilities
- Container process uses the battle-tested FastMCP server framework
- More details below in [Privacy & Security Details](#privacy--security-details)

## Requirements

* OS: MacOS, Linux - Windows would work in theory, but I don't have a testing machine.
* Cloudflare Tunnel - but no account needed, and it's a one-liner install.
* AI Assistant (e.g. ChatGPT, Claude, etc) 
* If you want it to be accessible 24/7, you'll need a dedicated home computer or cloud server (VPS), similar to openclaw.

## Quick Start

### macOS Prerequisites

- Container runtime: `brew install container && brew services start container` ([you can also use Docker Desktop for Mac](https://docs.docker.com/desktop/setup/install/mac-install/))
- Cloudflare Tunnel: `brew install cloudflared`

### Linux Prerequisites

<details>
<summary>Ubuntu/Debian install commands</summary>

```bash
# Docker
sudo apt update
sudo apt install ca-certificates curl
sudo install -m 0755 -d /etc/apt/keyrings
sudo curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
sudo chmod a+r /etc/apt/keyrings/docker.asc
sudo tee /etc/apt/sources.list.d/docker.sources <<'EOF'
Types: deb
URIs: https://download.docker.com/linux/ubuntu
Suites: $(. /etc/os-release && echo "${UBUNTU_CODENAME:-$VERSION_CODENAME}")
Components: stable
Architectures: $(dpkg --print-architecture)
Signed-By: /etc/apt/keyrings/docker.asc
EOF
sudo apt update
sudo apt install docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin

# cloudflared
sudo mkdir -p --mode=0755 /usr/share/keyrings
curl -fsSL https://pkg.cloudflare.com/cloudflare-main.gpg | sudo tee /usr/share/keyrings/cloudflare-main.gpg >/dev/null
echo 'deb [signed-by=/usr/share/keyrings/cloudflare-main.gpg] https://pkg.cloudflare.com/cloudflared any main' | sudo tee /etc/apt/sources.list.d/cloudflared.list
sudo apt-get update
sudo apt-get install cloudflared
```

</details>

### 1. Install the latest release

```bash
curl -sSfL https://brain3.s3.amazonaws.com/releases/v0.1.6/install.sh | sh
```

This installs `brain3` from the `v0.1.6` release.

### 2. Start Brain3

```bash
brain3
```

On first run, Brain3 launches an interactive setup wizard that walks you through vault selection, credential creation, and dependency checks.

On subsequent runs, Brain3 goes straight to the runtime status screen.

If you want a stable hostname on your own Cloudflare-managed domain instead of the default quick tunnel, use the separate named tunnel flow in [Advanced Configuration](#named-cloudflare-tunnel-on-your-domain) after first-run setup.

### 3. Connect your AI app

In the Brain3 TUI, press `c` to open **MCP Config Settings**.

Brain3 displays all the connection information you need:

- Server URL
- Client ID
- Client Secret
- Username
- Password

Follow the instructions below for your AI client.

<details>
<summary><strong>ChatGPT</strong></summary>

1. Open **Settings → Connectors → Create Connector**.
2. Select **Custom MCP Server**.
3. Copy the **Server URL**, **Client ID**, and **Client Secret** from Brain3's MCP Config Settings screen.
4. Complete the OAuth authorization flow when prompted.
5. When shown a login screen, enter the **Username** and **Password** displayed in Brain3's MCP Config Settings screen.

Once authorization succeeds, Brain3 will be available as a connector in ChatGPT.

</details>

<details>
<summary><strong>Claude</strong></summary>

1. Open **Settings → Connectors → Add Custom Connector**.
2. Enter the **Server URL** shown in Brain3.
3. If prompted, enter the **Client ID** and **Client Secret** from the MCP Config Settings screen.
4. Complete the OAuth authorization flow.
5. When shown a login screen, enter the **Username** and **Password** displayed in Brain3's MCP Config Settings screen.

Once authorization succeeds, Brain3 will appear as a custom connector in Claude.

</details>

<details>
<summary><strong>Other MCP-Compatible AI Apps</strong></summary>

Most MCP-compatible AI applications require the same information displayed in Brain3's MCP Config Settings screen:

- Server URL
- Client ID
- Client Secret

After connecting, complete the OAuth sign-in flow using the **Username** and **Password** shown on that screen.

Refer to your AI application's documentation for client-specific setup instructions.

</details>

Once connected, you can ask your AI to search, organize, summarize, and edit your notes directly through Brain3.

### 4. Interact with Brain3 via your AI assistant

Example prompt:
`List all the files in my Brain3 vault.`

Expected result:
Brain3 should list the files in your vault.

## Privacy & Security Details

Brain3 keeps your data on the local machine where it runs; your vault is never uploaded to any Brain3-managed cloud service. The AI assistant (Claude, ChatGPT, etc) sees only the parts of your vault exposed through your prompts and tool calls, leaving you in control of what is shared. You retain full control at all times and can stop Brain3, disable the tunnel, or disconnect your AI app whenever you choose.

### Architecture

- Host process written in Rust to minimize attack surface and reduce several classes of vulnerabilities.
- If you use Cloudflare Tunnel (default and recommended), `cloudflared` creates outbound-only connections to Cloudflare's network, so Brain3 does not need a publicly routable IP.
- ⚠ You should only use Brain3 if you trust Cloudflare: Cloudflare owns the root TLS certs and has the ability to decrypt traffic. See their [Cloudflare Transparency Report - H2 2025](https://www.cloudflare.com/transparency/).
- macOS or Docker container isolation. By default, the MCP server runs in a container with your vault mounted and a read-only upstream secret directory. No other host directories are mounted unless you enable dev mode.

### Authentication & Authorization

- Preregistered clients only. Solely the client configured via `B3_OAUTH2_GATEWAY_CLIENT_ID` and `B3_OAUTH2_GATEWAY_CLIENT_SECRET` can obtain tokens. Dynamic Client Registration (DCR) and Client ID Metadata Document (CIMD) are disabled.
- Client secret required at token exchange (`client_secret_post`).
- PKCE (`S256`) enforced by default to prevent authorization-code interception.
- Auth codes are single-use and expire after 5 minutes.

### Request Integrity

- Bearer-token validation on all `/mcp` routes.
- Host validation rejects unexpected hostnames (HTTP 421) when a public hostname is configured.
- Upstream shared secret injected by Brain3 so the MCP container rejects direct calls that bypass Brain3.
- Constant-time comparison for all secret and token checks.

## Advanced Configuration

### Named Cloudflare Tunnel on your domain

Recommended if you want a stable hostname like `brain3.yourdomain.com`. This requires a Cloudflare account and a custom domain you control.

Named tunnels are also known to be more reliable and less likely to suffer rate limits or connectivity restrictions than quick tunnels, since quick tunnels are available to anyone without a Cloudflare account and may be throttled accordingly. 

Brain3's normal first-run setup configures the app and defaults to a Cloudflare quick tunnel. Named tunnel provisioning is a separate guided flow:

1. Run `brain3` once and complete the normal interactive setup wizard.
2. Install `cloudflared` if it is not already available — see [install instructions](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/)
3. In `~/.brain3/.env`, set `B3_CF_TUNNEL_NAME` and `B3_CF_DOMAIN`. Set `B3_CF_TUNNEL_CONFIG_FILE` only if you want a non-default config path; otherwise Brain3 uses `.cloudflared/<tunnel-name>.yml`.
4. Run the named tunnel provisioning wizard:

```bash
brain3 --cf-setup
```

5. The wizard verifies `cloudflared`, handles login if needed, creates or reuses the named tunnel, writes the config file, and configures the DNS route.
6. Start Brain3 normally with `brain3`. It will start `cloudflared` automatically and log:

```
INFO tunnel started url=https://brain3.yourdomain.com
```

### Direct public origin

This is an alternative to a Cloudflare Tunnel if tunnels are not practical in your environment. It is less preferred because it involves exposing ports more directly, so only use it when a Cloudflare Tunnel is not a workable option.

If your machine already has a public IP or sits behind Cloudflare proxy, use Caddy or nginx to terminate TLS and reverse-proxy to `127.0.0.1:8421`. Set `B3_DIRECT_PUBLIC_ORIGIN_HOSTNAME` in your `.env` to the public hostname; Brain3 uses it for hostname validation.

Example minimal Caddyfile:

```
brain3.yourdomain.com {
    reverse_proxy 127.0.0.1:8421
}
```

## Configuration Reference

All configuration is via environment variables, loaded from a `.env` file.

### Brain3 App

| Variable | Default | Description |
|---|---|---|
| `B3_OAUTH2_GATEWAY_PORT` | `8421` | Port Brain3 listens on |
| `B3_OAUTH2_GATEWAY_CLIENT_ID` | `brain3-oauth2-client` | OAuth client ID accepted by Brain3 |
| `B3_OAUTH2_GATEWAY_CLIENT_SECRET` | *(required)* | OAuth client secret required at token exchange |
| `B3_OAUTH2_ACCESS_TOKEN_LIFETIME_SECS` | `3600` | Lifetime of issued access tokens in seconds |
| `B3_TOKEN_DB_PATH` | `~/.brain3/brain3.db` | Optional override for the SQLite database path used for issued one-hour access tokens |
| `B3_OAUTH2_GATEWAY_MCP_UPSTREAM_URL` | *(auto-derived from `B3_CONTAINER_HOST_PORT`)* | URL of the upstream MCP server (developers only) |
| `B3_OAUTH2_GATEWAY_UPSTREAM_SECRET_FILE` | `/tmp/brain3-mcp-upstream-secret` | Path to the shared secret file |
| `B3_OAUTH2_PKCE_REQUIRED` | `true` | Require PKCE for OAuth flow |
| `B3_USERNAME` | *(required)* | Login username for the Brain3 sign-in page |
| `B3_PASSWORD` | *(required)* | Login password for the Brain3 sign-in page |
| `B3_OAUTH2_GATEWAY_ENFORCE_HOSTNAME_CHECK` | `true` | Reject requests for unexpected hostnames |
| `B3_CF_QUICK_TUNNEL` | `false` | Set to `true` to have Brain3 start a quick Cloudflare Tunnel on startup |
| `B3_CF_TUNNEL_NAME` | *(empty)* | Named Cloudflare Tunnel name; set with `B3_CF_DOMAIN` and then run `brain3 --cf-setup` to provision it |
| `B3_CF_DOMAIN` | *(empty)* | Cloudflare zone domain (used with `B3_CF_TUNNEL_NAME`) |
| `B3_CF_TUNNEL_CONFIG_FILE` | `.cloudflared/<tunnel-name>.yml` | Optional path to the `cloudflared` config file written during named tunnel provisioning |
| `B3_DIRECT_PUBLIC_ORIGIN_HOSTNAME` | *(empty)* | Public hostname when using direct origin (Caddy/nginx) instead of Cloudflare Tunnel |
| `B3_CONTAINER_RUNTIME` | *(empty = skip)* | `macos-container` or `docker`; if set, Brain3 starts the container on startup |
| `B3_VAULT_PATH` | *(required if runtime set)* | Absolute path to your Obsidian vault or markdown folder |
| `B3_CONTAINER_IMAGE` | *(required if runtime set)* | Published container image to run, e.g. `ghcr.io/tleyden/brain3-mcp-vault-tools:vX.Y.Z`. New installs default to the Brain3 release-matched tag; `:latest` is still published but must be chosen explicitly. |
| `B3_CONTAINER_HOST_PORT` | `8420` | Host loopback port published to the container |

### MCP container image selection

Fresh installs default to the release-matched MCP image `ghcr.io/tleyden/brain3-mcp-vault-tools:vX.Y.Z`, where `X.Y.Z` matches the Brain3 app version. The registry still publishes `:latest`, but Brain3 does not select it automatically for new installs.

If an older Brain3 config still contains the exact official `ghcr.io/tleyden/brain3-mcp-vault-tools:latest` image, Brain3 now treats that as a legacy default and remaps it to the current release-matched tag for that run. Use `--container-tag latest` if you explicitly want to stay on `:latest`.

If you need a different published MCP image for a single launch or while creating a brand-new config, use `--container-tag`:

```bash
brain3 --container-tag latest
brain3 --container-tag pr-123
```

On an already configured install, `--container-tag` overrides the MCP container image for that launch only. During first-run setup, the selected tag becomes the image written into the new config.

The runtime status screen now reports whether the container is actually `Ready` or `Failed` after startup verification. If the container exits early, Brain3 shows the failure summary and log path instead of incorrectly reporting a successful start.

## MD Vault MCP Tools

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

## Development

### Running tests

```bash
cargo test
```

### Building

```bash
cargo build --release
```

## Build from Source (Developers)

### Prerequisites

Refer to the [Quick Start](#quick-start) prerequisites above.

### Install a PR build

PR install scripts are published at `https://brain3.s3.amazonaws.com/pr/<PR_NUMBER>/install.sh` while the PR is open.

```bash
curl -sSfL https://brain3.s3.amazonaws.com/pr/123/install.sh | sh
```

Replace `123` with the pull request number.

### Clone and build from source

```bash
git clone https://github.com/tleyden/brain3.git
cd brain3
cargo build --release
```

The binary is at `./target/release/brain3`.

### Start Brain3

```bash
./target/release/brain3
```

Brain3 launches the setup wizard on first run and goes straight to the runtime status screen on subsequent runs. See [Quick Start](#quick-start) fo r the full flow.

## Roadmap

* Improve search
* LLMWiki features
* NotebookLM features
* Desktop menu bar app with full system tray integration + GUI

## License 

Apache 2.0. See [LICENSE.MD](LICENSE.MD).
