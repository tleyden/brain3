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

This POC fork is only the stripped server code. It does not include container packaging, OAuth, or Cloudflare tunnel setup.

## Tool Surface

- `vault_read`: read a full file or a line window. Returns the hash of the full file content so callers can patch safely without rereading the entire document.
- `vault_create_overwrite_file`: create a new note or replace an existing note with full content. This is intentionally a blunt tool and is not the preferred edit path for existing notes.
- `vault_apply_unified_diff`: apply a unified diff to a single existing markdown/text file. This is the preferred tool for precise edits to existing notes, including one-line changes in very large files and small appends at EOF.
- `vault_batch_frontmatter_update`: update YAML frontmatter fields across one or more files. This preserves note body semantics but may rewrite the full markdown file during serialization.
- `vault_search`, `vault_search_frontmatter`, `vault_list`, `vault_move`, `vault_delete`: unchanged management and lookup tools.

## Recommended LLM Edit Flow

For editing an existing note:

1. Use `vault_search` to find the relevant note or line.
2. Use `vault_read` with `start_line` / `end_line` or `tail_lines` to fetch only the local context you need.
3. Build a single-file unified diff against that context.
4. Call `vault_apply_unified_diff` with the `content_hash` from `vault_read` as `expected_hash`.

For creating a brand-new note:

1. Call `vault_create_overwrite_file` with the full desired content.

For metadata-only changes:

1. Call `vault_batch_frontmatter_update`.

## Safety Model

- All successful writes still go through atomic replace (`write-to-temp-then-rename`).
- `vault_apply_unified_diff` is limited to a single file and rejects multi-file, rename, delete, and invalid diff payloads.
- `expected_hash` provides optimistic concurrency so stale reads do not silently clobber newer file content.

## Tests

Public API tests live in `tests/test_tool_write_patch_api.py` and cover the stable tool-level behaviors for reading windows, hashing, full replacement, unified diff patching, and frontmatter updates.
