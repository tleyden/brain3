# obsidian-mcp-server

This fork is the authless MCP server half of the original `obsidian-web-mcp` codebase.

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

## Entry Point

```bash
uv sync
uv run obsidian-mcp-server
```

Or:

```bash
./scripts/start-mcp-server.sh
```

## Scope

This POC fork is only the stripped server code plus a minimal native macOS container workflow. It does not include OAuth or Cloudflare tunnel setup.

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

This project includes a `Containerfile` that can be built with either Apple's native `container` CLI or Docker.

Build the image from the latest local code in this directory with the default native macOS runtime:

```bash
./scripts/build-container.sh
```

Build explicitly with Apple `container`:

```bash
./scripts/build-container.sh --container-runtime macos-container
```

Build explicitly with Docker:

```bash
./scripts/build-container.sh --container-runtime docker
```

If you want the image available in both runtimes on the same machine, run the script twice, once per runtime.

On a Linux machine that only has Docker installed, build with:

```bash
./scripts/build-container.sh --container-runtime docker
```

This uses:

- base image: `python:3.14.5-slim-bookworm`
- build context: this `poc/obsidian-mcp-container` directory only
- image name: `obsidian-mcp-server:latest` by default

If you want a different tag:

```bash
IMAGE_NAME=obsidian-mcp-server:dev ./scripts/build-container.sh --container-runtime docker
```

## Container Run

The Obsidian MCP server is the only process that runs inside the container. The OAuth gateway stays outside the container and talks to the MCP server over the published local HTTP port.

Run the baked image against a host vault:

```bash
./scripts/run-container.sh --vault-path /absolute/path/to/vault
```

This:

- mounts the host vault into the container at `/vault`
- sets `VAULT_PATH=/vault` inside the container
- publishes `127.0.0.1:8420` on the host to port `8420` in the container

If your local `.env` already sets `VAULT_PATH` to a host directory, the run script will use that path by default.

### Bind-Mounted Source Mode

For faster Python edit loops, you can run the mounted source tree instead of rebuilding the image on every code change:

```bash
./scripts/run-container.sh --bind-source --vault-path /absolute/path/to/vault
```

In bind mode:

- dependencies still come from the image
- source code comes from the mounted host checkout
- changes under `src/` are picked up on the next container restart

If you change dependencies or packaging metadata (`pyproject.toml`, `uv.lock`), rebuild the image.

### Useful Variations

Run on a different host port:

```bash
./scripts/run-container.sh --vault-path /absolute/path/to/vault --port 8422
```

Run in the foreground:

```bash
./scripts/run-container.sh --vault-path /absolute/path/to/vault --foreground
```

## Tests

Public API tests live in `tests/test_tool_write_patch_api.py` and cover the stable tool-level behaviors for reading windows, hashing, full replacement, unified diff patching, and frontmatter updates.
