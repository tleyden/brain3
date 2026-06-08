# obsidian-mcp-server

This fork is the authless MCP server half of the original `obsidian-web-mcp` codebase.

The HTTP server stays authless from an OAuth perspective, but the host gateway now authenticates to it with a private shared secret. Direct calls to the upstream port are expected to fail unless that private header is present.

It keeps only:
- the HTTP MCP server
- vault read/search/list/move/delete tools
- targeted markdown mutation tools for LLM callers
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

For the normal POC flow, you do not need to set the upstream shared secret yourself. The startup scripts create one host-side secret file, reuse it, and mount it into the container automatically.

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

Container runtime notes:
- Every container build/run command requires an explicit `--container-runtime` flag.
- macOS supports both Apple's native `container` CLI and Docker.
- Linux uses Docker. Apple's `container` CLI is macOS-only.

### Linux Docker install

```bash
sudo apt update
sudo apt install ca-certificates curl
sudo install -m 0755 -d /etc/apt/keyrings
sudo curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
sudo chmod a+r /etc/apt/keyrings/docker.asc
sudo tee /etc/apt/sources.list.d/docker.sources <<EOF
Types: deb
URIs: https://download.docker.com/linux/ubuntu
Suites: $(. /etc/os-release && echo "${UBUNTU_CODENAME:-$VERSION_CODENAME}")
Components: stable
Architectures: $(dpkg --print-architecture)
Signed-By: /etc/apt/keyrings/docker.asc
EOF
sudo apt update
sudo apt install docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
```

The run/build scripts assume `docker` is directly runnable by your user on Linux.

## Entry Point

Once `uv` is installed, the run commands are the same on macOS and Linux.

```bash
uv sync
uv run obsidian-mcp-server
```

Or:

```bash
./scripts/start-mcp-server.sh
```

## Scope

This POC fork is only the stripped server code plus a minimal container workflow: Apple `container` or Docker on macOS, and Docker on Linux. It does not include OAuth or Cloudflare tunnel setup.

## Tool Surface

- `vault_read`: read a full file or a line window. Returns the hash of the full file content so callers can patch safely without rereading the entire document. Prefer line-window reads before editing existing files.
- `vault_create_overwrite_file`: create a new note or replace an existing note with full content. This is intentionally a blunt tool. Use it for new files or deliberate whole-document replacement, not as the default way to edit an existing note.
- `vault_apply_unified_diff`: apply a unified diff to a single existing markdown/text file. This is the preferred tool for precise edits to existing notes, including one-line changes in very large files and small appends at EOF. Lean toward this when feasible because whole-file overwrite is more token-expensive and more error-prone.
- `vault_batch_frontmatter_update`: update YAML frontmatter fields across one or more files. Prefer this over full overwrite for metadata-only changes. It preserves note body semantics but may rewrite the full markdown file during serialization.
- `vault_search`, `vault_search_frontmatter`, `vault_list`, `vault_move`, `vault_delete`: unchanged management and lookup tools.

## Recommended LLM Edit Flow

For editing an existing note:

1. Use `vault_search` to find the relevant note or line.
2. Use `vault_read` with `start_line` / `end_line` or `tail_lines` to fetch only the local context you need.
3. Build a single-file unified diff against that context.
4. Call `vault_apply_unified_diff` with the `content_hash` from `vault_read` as `expected_hash`.

When feasible, prefer this diff-based flow over `vault_create_overwrite_file`. Replacing the entire file costs more tokens and makes it easier for an LLM to accidentally damage unrelated parts of a large note.

For creating a brand-new note:

1. Call `vault_create_overwrite_file` with the full desired content.

For metadata-only changes:

1. Call `vault_batch_frontmatter_update`.

## Safety Model

- All successful writes still go through atomic replace (`write-to-temp-then-rename`).
- `vault_apply_unified_diff` is limited to a single file and rejects multi-file, rename, delete, and invalid diff payloads.
- `expected_hash` provides optimistic concurrency so stale reads do not silently clobber newer file content.

## Container Build

This project includes a `Containerfile` that can be built with Apple's native `container` CLI on macOS or Docker on Linux. Docker examples also work on macOS. Every build command must specify `--container-runtime` explicitly.

Build with Apple `container` on macOS:

```bash
./scripts/build-container.sh --container-runtime macos-container
```

Build with Docker on Linux or macOS:

```bash
./scripts/build-container.sh --container-runtime docker
```

If you want the image available in both runtimes on the same macOS machine, run the script twice, once per runtime.

This uses:

- base image: `python:3.14.5-slim-bookworm`
- build context: this `poc/obsidian-mcp-container` directory only
- image name: `obsidian-mcp-server:latest` by default

If you want a different tag:

```bash
IMAGE_NAME=obsidian-mcp-server:dev ./scripts/build-container.sh --container-runtime docker
```

## Container Run

The Obsidian MCP server is the only process that runs inside the container. The OAuth gateway stays outside the container and talks to the MCP server over the published local HTTP port. Every run command must specify `--container-runtime` explicitly.

Run with Apple `container` on macOS:

```bash
./scripts/run-container.sh --container-runtime macos-container --vault-path /absolute/path/to/vault
```

Run with Docker on Linux or macOS:

```bash
./scripts/run-container.sh --container-runtime docker --vault-path /absolute/path/to/vault
```

Each runtime expects its image to exist in that runtime's local image store first:

- native macOS run mode expects `./scripts/build-container.sh --container-runtime macos-container`
- Docker run mode expects `./scripts/build-container.sh --container-runtime docker`

Default run mode uses the code baked into the image. If you change files under `src/` and run `./scripts/run-container.sh` again in the default mode, you will still get the old code until you rebuild the image first.

Typical default-mode edit loop:

```bash
./scripts/build-container.sh --container-runtime macos-container
./scripts/run-container.sh --container-runtime macos-container --vault-path /absolute/path/to/vault
```

This:

- mounts the host vault into the container at `/vault`
- mounts the host directory containing the shared-secret file read-only into `/run/agentzoo`
- exposes the shared-secret file inside the container at `/run/agentzoo/upstream_secret`
- sets `VAULT_PATH=/vault` inside the container
- publishes `127.0.0.1:8420` on the host to port `8420` in the container

If your local `.env` already sets `VAULT_PATH` to a host directory, the run script will use that path by default.

### Bind-Mounted Source Development Mode

For faster Python edit loops during local development, use `--bind-mount-sourcecode`. This is mainly a development-mode workflow: it runs the mounted host source tree instead of the code baked into the image, so normal `src/` edits do not require rebuilding the image.

For normal runs, prefer the default image-backed mode above.

Apple `container` bind-mounted source mode:

```bash
./scripts/run-container.sh --container-runtime macos-container --bind-mount-sourcecode --vault-path /absolute/path/to/vault
```

Docker bind-mounted source mode:

```bash
./scripts/run-container.sh --container-runtime docker --bind-mount-sourcecode --vault-path /absolute/path/to/vault
```

In this development mode:

- dependencies still come from the image
- source code comes from the mounted host checkout
- changes under `src/` are picked up on the next container restart

Typical bind-source edit loop:

```bash
./scripts/run-container.sh --container-runtime macos-container --bind-mount-sourcecode --vault-path /absolute/path/to/vault
# edit files under src/
./scripts/run-container.sh --container-runtime macos-container --bind-mount-sourcecode --vault-path /absolute/path/to/vault
```

If you change dependencies or packaging metadata (`pyproject.toml`, `uv.lock`), rebuild the image.

### Useful Variations

Run on a different host port:

```bash
./scripts/run-container.sh --container-runtime macos-container --vault-path /absolute/path/to/vault --port 8422
```

Run in the foreground:

```bash
./scripts/run-container.sh --container-runtime macos-container --vault-path /absolute/path/to/vault --foreground
```

## Tests

Public API tests live in `tests/test_tool_write_patch_api.py` and cover the stable tool-level behaviors for reading windows, hashing, full replacement, unified diff patching, and frontmatter updates.
