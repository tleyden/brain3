"""Authless Obsidian MCP server."""

import hmac
import logging
import sys
from importlib.metadata import PackageNotFoundError, version
from pathlib import Path

from mcp.server.fastmcp import FastMCP
from mcp.server.transport_security import TransportSecuritySettings
from starlette.requests import Request
from starlette.responses import JSONResponse
from starlette.types import ASGIApp, Message, Receive, Scope, Send

from .config import (
    UPSTREAM_SHARED_SECRET_FILE,
    UPSTREAM_SHARED_SECRET_HEADER,
    VAULT_MCP_EXTRA_ALLOWED_HOSTS,
    VAULT_MCP_HOST,
    VAULT_MCP_PORT,
    VAULT_MCP_UNIX_SOCKET,
    VAULT_PATH,
)
from .frontmatter_index import FrontmatterIndex
from .models import (
    VaultApplyUnifiedDiffInput,
    VaultBatchFrontmatterUpdateInput,
    VaultBatchReadInput,
    VaultCreateOverwriteFileInput,
    VaultDeleteInput,
    VaultListInput,
    VaultMoveInput,
    VaultReadInput,
    VaultSearchFrontmatterInput,
    VaultSearchInput,
)
from .tools.manage import vault_delete as _vault_delete
from .tools.manage import vault_list as _vault_list
from .tools.manage import vault_move as _vault_move
from .tools.patch import vault_apply_unified_diff as _vault_apply_unified_diff
from .tools.read import vault_batch_read as _vault_batch_read
from .tools.read import vault_read as _vault_read
from .tools.search import vault_search as _vault_search
from .tools.search import vault_search_frontmatter as _vault_search_frontmatter
from .tools.write import (
    vault_batch_frontmatter_update as _vault_batch_frontmatter_update,
)
from .tools.write import (
    vault_create_overwrite_file as _vault_create_overwrite_file,
)

logger = logging.getLogger(__name__)

frontmatter_index = FrontmatterIndex()


def _package_version() -> str:
    try:
        return version("brain3-mcp-vault-tools")
    except PackageNotFoundError:
        return "unknown"


def _elide(s: str) -> str:
    if len(s) <= 2:
        return f"***({len(s)} chars)"
    return f"{s[0]}***{s[-1]}({len(s)} chars)"


def _load_upstream_shared_secret() -> str:
    try:
        secret = Path(UPSTREAM_SHARED_SECRET_FILE).read_text(encoding="utf-8").strip()
    except OSError as exc:
        raise RuntimeError(
            f"Unable to read MCP upstream shared secret file: {UPSTREAM_SHARED_SECRET_FILE}"
        ) from exc

    if not secret:
        raise RuntimeError(
            f"MCP upstream shared secret file is empty: {UPSTREAM_SHARED_SECRET_FILE}"
        )

    logger.info(
        "Loaded upstream shared secret file=%s secret=%s",
        UPSTREAM_SHARED_SECRET_FILE,
        _elide(secret),
    )
    return secret


class UpstreamSharedSecretMiddleware:
    def __init__(
        self, app: ASGIApp, *, shared_secret: str, header_name: str, protected_path: str
    ) -> None:
        self.app = app
        self.shared_secret = shared_secret
        self.header_name = header_name
        self.protected_path = protected_path

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http" or scope.get("path") != self.protected_path:
            await self.app(scope, receive, send)
            return

        request = Request(scope, receive=receive)
        provided_secret = request.headers.get(self.header_name, "")
        if not provided_secret or not hmac.compare_digest(
            provided_secret, self.shared_secret
        ):
            logger.warning(
                "Upstream secret mismatch: provided=%s expected=%s",
                _elide(provided_secret),
                _elide(self.shared_secret),
            )
            response = JSONResponse({"error": "unauthorized"}, status_code=401)
            await response(scope, receive, send)
            return

        await self.app(scope, receive, send)


class GuardedFastMCP(FastMCP):
    def streamable_http_app(self):
        app = super().streamable_http_app()
        app.add_middleware(
            UpstreamSharedSecretMiddleware,
            shared_secret=_load_upstream_shared_secret(),
            header_name=UPSTREAM_SHARED_SECRET_HEADER,
            protected_path=self.settings.streamable_http_path,
        )
        return app


def _start_process_resources() -> None:
    """Start process-scoped resources before serving requests."""
    logger.info(f"Starting vault MCP server. Vault: {VAULT_PATH}")
    frontmatter_index.start()
    logger.info(
        f"Frontmatter index built: {frontmatter_index.file_count} files indexed"
    )


def _stop_process_resources() -> None:
    """Stop process-scoped resources on server shutdown."""
    frontmatter_index.stop()
    logger.info("Vault MCP server shut down.")


mcp = GuardedFastMCP(
    "brain3_mcp_vault_tools",
    host=VAULT_MCP_HOST,
    port=VAULT_MCP_PORT,
    stateless_http=True,
    json_response=True,
    transport_security=TransportSecuritySettings(
        enable_dns_rebinding_protection=True,
        allowed_hosts=[
            "127.0.0.1:*",
            "localhost:*",
            "[::1]:*",
        ]
        + VAULT_MCP_EXTRA_ALLOWED_HOSTS,
    ),
)


@mcp.tool(
    name="vault_read",
    description="Read a vault file. Prefer line-window reads when preparing an edit to an existing file, then follow with vault_apply_unified_diff using the returned full-file content hash.",
    annotations={
        "readOnlyHint": True,
        "destructiveHint": False,
        "idempotentHint": True,
        "openWorldHint": False,
    },
)
def vault_read(
    path: str,
    start_line: int | None = None,
    end_line: int | None = None,
    tail_lines: int | None = None,
) -> str:
    inp = VaultReadInput(
        path=path, start_line=start_line, end_line=end_line, tail_lines=tail_lines
    )
    return _vault_read(inp.path, inp.start_line, inp.end_line, inp.tail_lines)


@mcp.tool(
    name="vault_batch_read",
    description="Read multiple files from the Obsidian vault in one call. Handles missing files gracefully.",
    annotations={
        "readOnlyHint": True,
        "destructiveHint": False,
        "idempotentHint": True,
        "openWorldHint": False,
    },
)
def vault_batch_read(paths: list[str], include_content: bool = True) -> str:
    inp = VaultBatchReadInput(paths=paths, include_content=include_content)
    return _vault_batch_read(inp.paths, inp.include_content)


@mcp.tool(
    name="vault_create_overwrite_file",
    description="Create a new file or replace an existing file with the full provided content. Use this for new notes or deliberate whole-document replacement only. Prefer vault_apply_unified_diff for feasible edits to existing files because full overwrite is more token-expensive and more error-prone.",
    annotations={
        "readOnlyHint": False,
        "destructiveHint": True,
        "idempotentHint": False,
        "openWorldHint": False,
    },
)
def vault_create_overwrite_file(
    path: str, content: str, create_dirs: bool = True
) -> str:
    inp = VaultCreateOverwriteFileInput(
        path=path, content=content, create_dirs=create_dirs
    )
    return _vault_create_overwrite_file(inp.path, inp.content, inp.create_dirs)


@mcp.tool(
    name="vault_apply_unified_diff",
    description="Apply a unified diff to a single existing text file. This is the default edit path for existing notes when feasible, including one-line changes in large files and small EOF appends. Lean toward this instead of vault_create_overwrite_file because it is cheaper in tokens and safer.",
    annotations={
        "readOnlyHint": False,
        "destructiveHint": True,
        "idempotentHint": False,
        "openWorldHint": False,
    },
)
def vault_apply_unified_diff(
    path: str,
    diff: str,
    dry_run: bool = False,
    expected_hash: str | None = None,
) -> str:
    inp = VaultApplyUnifiedDiffInput(
        path=path, diff=diff, dry_run=dry_run, expected_hash=expected_hash
    )
    return _vault_apply_unified_diff(inp.path, inp.diff, inp.dry_run, inp.expected_hash)


@mcp.tool(
    name="vault_batch_frontmatter_update",
    description="Update YAML frontmatter fields on multiple files. Prefer this over whole-file replacement for metadata-only changes. It preserves note body semantics but may rewrite the full markdown file to serialize the updated frontmatter.",
    annotations={
        "readOnlyHint": False,
        "destructiveHint": False,
        "idempotentHint": True,
        "openWorldHint": False,
    },
)
def vault_batch_frontmatter_update(updates: list[dict]) -> str:
    inp = VaultBatchFrontmatterUpdateInput(updates=updates)
    return _vault_batch_frontmatter_update(inp.updates)


@mcp.tool(
    name="vault_search",
    description="Search for text across vault files. Uses ripgrep if available, falls back to Python.",
    annotations={
        "readOnlyHint": True,
        "destructiveHint": False,
        "idempotentHint": True,
        "openWorldHint": False,
    },
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
    return _vault_search(
        inp.query, inp.path_prefix, inp.file_pattern, inp.max_results, inp.context_lines
    )


@mcp.tool(
    name="vault_search_frontmatter",
    description="Search vault files by YAML frontmatter field values via the in-memory index.",
    annotations={
        "readOnlyHint": True,
        "destructiveHint": False,
        "idempotentHint": True,
        "openWorldHint": False,
    },
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
    return _vault_search_frontmatter(
        inp.field, inp.value, inp.match_type, inp.path_prefix, inp.max_results
    )


@mcp.tool(
    name="vault_list",
    description="List directory contents in the vault.",
    annotations={
        "readOnlyHint": True,
        "destructiveHint": False,
        "idempotentHint": True,
        "openWorldHint": False,
    },
)
def vault_list(
    path: str = "",
    depth: int = 1,
    include_files: bool = True,
    include_dirs: bool = True,
    pattern: str | None = None,
) -> str:
    inp = VaultListInput(
        path=path,
        depth=depth,
        include_files=include_files,
        include_dirs=include_dirs,
        pattern=pattern,
    )
    return _vault_list(
        inp.path, inp.depth, inp.include_files, inp.include_dirs, inp.pattern
    )


@mcp.tool(
    name="vault_move",
    description="Move a file or directory within the vault.",
    annotations={
        "readOnlyHint": False,
        "destructiveHint": True,
        "idempotentHint": False,
        "openWorldHint": False,
    },
)
def vault_move(source: str, destination: str, create_dirs: bool = True) -> str:
    inp = VaultMoveInput(
        source=source, destination=destination, create_dirs=create_dirs
    )
    return _vault_move(inp.source, inp.destination, inp.create_dirs)


@mcp.tool(
    name="vault_delete",
    description="Delete a file by moving it to .trash/ in the vault root. Requires confirm=true.",
    annotations={
        "readOnlyHint": False,
        "destructiveHint": True,
        "idempotentHint": False,
        "openWorldHint": False,
    },
)
def vault_delete(path: str, confirm: bool = False) -> str:
    inp = VaultDeleteInput(path=path, confirm=confirm)
    return _vault_delete(inp.path, inp.confirm)


def main() -> None:
    """Run the authless MCP server over Streamable HTTP (TCP or Unix socket)."""
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
        stream=sys.stderr,
    )

    if not VAULT_PATH.is_dir():
        logger.error(f"Vault path does not exist: {VAULT_PATH}")
        sys.exit(1)

    if VAULT_MCP_UNIX_SOCKET:
        _run_on_unix_socket()
    else:
        _run_on_tcp()


def _run_on_tcp() -> None:
    _start_process_resources()
    try:
        logger.info(
            "Starting authless MCP server version=%s on port %s",
            _package_version(),
            VAULT_MCP_PORT,
        )
        mcp.run(transport="streamable-http")
    finally:
        _stop_process_resources()


def _run_on_unix_socket() -> None:
    import uvicorn

    socket_path = Path(VAULT_MCP_UNIX_SOCKET)

    # Fail immediately if the parent directory does not exist.
    if not socket_path.parent.exists():
        logger.error(
            "Unix socket parent directory does not exist: %s — "
            "check that the host runtime dir is bind-mounted into the container",
            socket_path.parent,
        )
        sys.exit(1)

    # Unlink any stale socket file so bind() always succeeds on restart.
    if socket_path.exists():
        logger.info("Removing stale socket file: %s", socket_path)
        socket_path.unlink()

    logger.info(
        "Starting authless MCP server version=%s on Unix socket %s",
        _package_version(),
        VAULT_MCP_UNIX_SOCKET,
    )
    app = mcp.streamable_http_app()
    _start_process_resources()
    try:
        uvicorn.run(app, uds=str(socket_path))
    finally:
        _stop_process_resources()


if __name__ == "__main__":
    main()
