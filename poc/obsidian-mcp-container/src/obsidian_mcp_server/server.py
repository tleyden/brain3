"""Authless Obsidian MCP server."""

import logging
import sys

from mcp.server.fastmcp import FastMCP
from mcp.server.transport_security import TransportSecuritySettings

from .config import VAULT_MCP_EXTRA_ALLOWED_HOSTS, VAULT_MCP_PORT, VAULT_PATH
from .frontmatter_index import FrontmatterIndex
from .models import (
    VaultBatchFrontmatterUpdateInput,
    VaultBatchReadInput,
    VaultDeleteInput,
    VaultListInput,
    VaultMoveInput,
    VaultReadInput,
    VaultSearchFrontmatterInput,
    VaultSearchInput,
    VaultWriteInput,
)
from .tools.manage import vault_delete as _vault_delete, vault_list as _vault_list, vault_move as _vault_move
from .tools.read import vault_batch_read as _vault_batch_read, vault_read as _vault_read
from .tools.search import vault_search as _vault_search, vault_search_frontmatter as _vault_search_frontmatter
from .tools.write import (
    vault_batch_frontmatter_update as _vault_batch_frontmatter_update,
    vault_write as _vault_write,
)

logger = logging.getLogger(__name__)

frontmatter_index = FrontmatterIndex()


def _start_process_resources() -> None:
    """Start process-scoped resources before serving requests."""
    logger.info(f"Starting vault MCP server. Vault: {VAULT_PATH}")
    frontmatter_index.start()
    logger.info(f"Frontmatter index built: {frontmatter_index.file_count} files indexed")


def _stop_process_resources() -> None:
    """Stop process-scoped resources on server shutdown."""
    frontmatter_index.stop()
    logger.info("Vault MCP server shut down.")


mcp = FastMCP(
    "obsidian_mcp_server",
    port=VAULT_MCP_PORT,
    stateless_http=True,
    json_response=True,
    transport_security=TransportSecuritySettings(
        enable_dns_rebinding_protection=True,
        allowed_hosts=[
            "127.0.0.1:*",
            "localhost:*",
            "[::1]:*",
        ] + VAULT_MCP_EXTRA_ALLOWED_HOSTS,
    ),
)


@mcp.tool(
    name="vault_read",
    description="Read a file from the Obsidian vault, returning content, metadata, and parsed YAML frontmatter.",
    annotations={"readOnlyHint": True, "destructiveHint": False, "idempotentHint": True, "openWorldHint": False},
)
def vault_read(path: str) -> str:
    inp = VaultReadInput(path=path)
    return _vault_read(inp.path)


@mcp.tool(
    name="vault_batch_read",
    description="Read multiple files from the Obsidian vault in one call. Handles missing files gracefully.",
    annotations={"readOnlyHint": True, "destructiveHint": False, "idempotentHint": True, "openWorldHint": False},
)
def vault_batch_read(paths: list[str], include_content: bool = True) -> str:
    inp = VaultBatchReadInput(paths=paths, include_content=include_content)
    return _vault_batch_read(inp.paths, inp.include_content)


@mcp.tool(
    name="vault_write",
    description="Write a file to the Obsidian vault. Supports frontmatter merging with existing files. Creates parent directories by default.",
    annotations={"readOnlyHint": False, "destructiveHint": True, "idempotentHint": False, "openWorldHint": False},
)
def vault_write(path: str, content: str, create_dirs: bool = True, merge_frontmatter: bool = False) -> str:
    inp = VaultWriteInput(path=path, content=content, create_dirs=create_dirs, merge_frontmatter=merge_frontmatter)
    return _vault_write(inp.path, inp.content, inp.create_dirs, inp.merge_frontmatter)


@mcp.tool(
    name="vault_batch_frontmatter_update",
    description="Update YAML frontmatter fields on multiple files without changing body content.",
    annotations={"readOnlyHint": False, "destructiveHint": False, "idempotentHint": True, "openWorldHint": False},
)
def vault_batch_frontmatter_update(updates: list[dict]) -> str:
    inp = VaultBatchFrontmatterUpdateInput(updates=updates)
    return _vault_batch_frontmatter_update(inp.updates)


@mcp.tool(
    name="vault_search",
    description="Search for text across vault files. Uses ripgrep if available, falls back to Python.",
    annotations={"readOnlyHint": True, "destructiveHint": False, "idempotentHint": True, "openWorldHint": False},
)
def vault_search(
    query: str,
    path_prefix: str | None = None,
    file_pattern: str = "*.md",
    max_results: int = 20,
    context_lines: int = 2,
) -> str:
    inp = VaultSearchInput(
        query=query,
        path_prefix=path_prefix,
        file_pattern=file_pattern,
        max_results=max_results,
        context_lines=context_lines,
    )
    return _vault_search(inp.query, inp.path_prefix, inp.file_pattern, inp.max_results, inp.context_lines)


@mcp.tool(
    name="vault_search_frontmatter",
    description="Search vault files by YAML frontmatter field values via the in-memory index.",
    annotations={"readOnlyHint": True, "destructiveHint": False, "idempotentHint": True, "openWorldHint": False},
)
def vault_search_frontmatter(
    field: str,
    value: str = "",
    match_type: str = "exact",
    path_prefix: str | None = None,
    max_results: int = 20,
) -> str:
    inp = VaultSearchFrontmatterInput(
        field=field,
        value=value,
        match_type=match_type,
        path_prefix=path_prefix,
        max_results=max_results,
    )
    return _vault_search_frontmatter(inp.field, inp.value, inp.match_type, inp.path_prefix, inp.max_results)


@mcp.tool(
    name="vault_list",
    description="List directory contents in the vault.",
    annotations={"readOnlyHint": True, "destructiveHint": False, "idempotentHint": True, "openWorldHint": False},
)
def vault_list(
    path: str = "",
    depth: int = 1,
    include_files: bool = True,
    include_dirs: bool = True,
    pattern: str | None = None,
) -> str:
    inp = VaultListInput(path=path, depth=depth, include_files=include_files, include_dirs=include_dirs, pattern=pattern)
    return _vault_list(inp.path, inp.depth, inp.include_files, inp.include_dirs, inp.pattern)


@mcp.tool(
    name="vault_move",
    description="Move a file or directory within the vault.",
    annotations={"readOnlyHint": False, "destructiveHint": True, "idempotentHint": False, "openWorldHint": False},
)
def vault_move(source: str, destination: str, create_dirs: bool = True) -> str:
    inp = VaultMoveInput(source=source, destination=destination, create_dirs=create_dirs)
    return _vault_move(inp.source, inp.destination, inp.create_dirs)


@mcp.tool(
    name="vault_delete",
    description="Delete a file by moving it to .trash/ in the vault root. Requires confirm=true.",
    annotations={"readOnlyHint": False, "destructiveHint": True, "idempotentHint": False, "openWorldHint": False},
)
def vault_delete(path: str, confirm: bool = False) -> str:
    inp = VaultDeleteInput(path=path, confirm=confirm)
    return _vault_delete(inp.path, inp.confirm)


def main() -> None:
    """Run the authless MCP server over Streamable HTTP."""
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
        stream=sys.stderr,
    )

    if not VAULT_PATH.is_dir():
        logger.error(f"Vault path does not exist: {VAULT_PATH}")
        sys.exit(1)

    _start_process_resources()
    try:
        logger.info(f"Starting authless MCP server on port {VAULT_MCP_PORT}")
        mcp.run(transport="streamable-http")
    finally:
        _stop_process_resources()


if __name__ == "__main__":
    main()
