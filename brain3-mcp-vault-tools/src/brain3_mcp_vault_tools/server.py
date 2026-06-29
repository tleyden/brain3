"""Authless Obsidian MCP server."""

import hmac
import ipaddress
import logging
import socket
import sys
from importlib.metadata import PackageNotFoundError, version
from pathlib import Path

from mcp.server.fastmcp import FastMCP
from mcp.server.transport_security import TransportSecuritySettings
from starlette.requests import Request
from starlette.responses import JSONResponse
from starlette.types import ASGIApp, Message, Receive, Scope, Send

from .config import (
    VAULT_MCP_ALLOW_SELF_IP_HOSTS,
    UPSTREAM_SHARED_SECRET,
    UPSTREAM_SHARED_SECRET_FILE,
    UPSTREAM_SHARED_SECRET_HEADER,
    VAULT_MCP_EXTRA_ALLOWED_HOSTS,
    VAULT_MCP_HOST,
    VAULT_MCP_LOG_LEVEL,
    VAULT_MCP_PORT,
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

TRACE = 5
logging.addLevelName(TRACE, "TRACE")

_LOG_LEVELS = {
    "TRACE": TRACE,
    "DEBUG": logging.DEBUG,
    "INFO": logging.INFO,
    "WARNING": logging.WARNING,
    "WARN": logging.WARNING,
    "ERROR": logging.ERROR,
    "CRITICAL": logging.CRITICAL,
}


def _resolve_log_level(name: str) -> int:
    return _LOG_LEVELS.get(name.strip().upper(), logging.INFO)


frontmatter_index = FrontmatterIndex()
DEFAULT_ALLOWED_HOSTS = [
    "127.0.0.1:*",
    "localhost:*",
    "[::1]:*",
]


def _dedupe_keep_order(values: list[str]) -> list[str]:
    seen: set[str] = set()
    deduped: list[str] = []
    for value in values:
        if value in seen:
            continue
        seen.add(value)
        deduped.append(value)
    return deduped


def _format_ip_allowed_host(ip: str) -> str:
    return f"[{ip}]:*" if ":" in ip else f"{ip}:*"


def _discover_self_ips() -> list[str]:
    hostname = socket.gethostname()
    try:
        addrinfos = socket.getaddrinfo(hostname, None, type=socket.SOCK_STREAM)
    except OSError as exc:
        logger.warning(
            "Unable to resolve self IPs from hostname=%s: %s", hostname, exc
        )
        return []

    ips: list[str] = []
    for _family, _socktype, _proto, _canonname, sockaddr in addrinfos:
        host = sockaddr[0]
        try:
            ip = ipaddress.ip_address(host)
        except ValueError:
            continue
        if ip.is_loopback or ip.is_unspecified:
            continue
        ips.append(ip.compressed)

    return _dedupe_keep_order(ips)


def _resolve_allowed_hosts() -> tuple[list[str], list[str]]:
    detected_self_ips = (
        _discover_self_ips() if VAULT_MCP_ALLOW_SELF_IP_HOSTS else []
    )
    allowed_hosts = (
        DEFAULT_ALLOWED_HOSTS
        + VAULT_MCP_EXTRA_ALLOWED_HOSTS
        + [_format_ip_allowed_host(ip) for ip in detected_self_ips]
    )
    return _dedupe_keep_order(allowed_hosts), detected_self_ips


ALLOWED_HOSTS, DETECTED_SELF_IPS = _resolve_allowed_hosts()


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
    if UPSTREAM_SHARED_SECRET:
        logger.info(
            "Loaded upstream shared secret directly from env secret=%s",
            _elide(UPSTREAM_SHARED_SECRET),
        )
        return UPSTREAM_SHARED_SECRET

    if not UPSTREAM_SHARED_SECRET_FILE:
        raise RuntimeError(
            "MCP upstream shared secret is not configured; set "
            "B3_UPSTREAM_SHARED_SECRET or B3_UPSTREAM_SHARED_SECRET_FILE"
        )

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


class InboundRequestLoggingMiddleware:
    def __init__(self, app: ASGIApp, *, allowed_hosts: list[str]) -> None:
        self.app = app
        self.allowed_hosts = allowed_hosts

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http":
            await self.app(scope, receive, send)
            return

        path = scope.get("path", "")
        if not path.startswith("/mcp"):
            await self.app(scope, receive, send)
            return

        headers = {
            key.decode("latin-1"): value.decode("latin-1")
            for key, value in scope.get("headers", [])
        }
        client = scope.get("client")
        host_header = headers.get("host", "")
        forwarded_host = headers.get("x-forwarded-host", "")
        response_status: int | None = None

        logger.info(
            "Inbound MCP request method=%s path=%s client=%s host=%s x_forwarded_host=%s has_upstream_secret=%s allowed_hosts=%s",
            scope.get("method", "<unknown>"),
            path,
            client,
            host_header,
            forwarded_host,
            UPSTREAM_SHARED_SECRET_HEADER in headers,
            self.allowed_hosts,
        )

        trace_enabled = logger.isEnabledFor(TRACE)
        request_body_chunks: list[bytes] = []

        async def receive_wrapper() -> Message:
            message = await receive()
            if trace_enabled and message["type"] == "http.request":
                request_body_chunks.append(message.get("body", b""))
                if not message.get("more_body", False):
                    logger.log(
                        TRACE,
                        "MCP request body method=%s path=%s body=%s",
                        scope.get("method", "<unknown>"),
                        path,
                        b"".join(request_body_chunks).decode("utf-8", errors="replace"),
                    )
            return message

        response_body_chunks: list[bytes] = []

        async def send_wrapper(message: Message) -> None:
            nonlocal response_status
            if message["type"] == "http.response.start":
                response_status = message["status"]
            if trace_enabled and message["type"] == "http.response.body":
                response_body_chunks.append(message.get("body", b""))
                if not message.get("more_body", False):
                    logger.log(
                        TRACE,
                        "MCP response body method=%s path=%s status=%s body=%s",
                        scope.get("method", "<unknown>"),
                        path,
                        response_status,
                        b"".join(response_body_chunks).decode("utf-8", errors="replace"),
                    )
            await send(message)

        await self.app(scope, receive_wrapper, send_wrapper)

        log = logger.warning if response_status == 421 else logger.info
        log(
            "Inbound MCP response status=%s method=%s path=%s host=%s x_forwarded_host=%s allowed_hosts=%s",
            response_status,
            scope.get("method", "<unknown>"),
            path,
            host_header,
            forwarded_host,
            self.allowed_hosts,
        )


class GuardedFastMCP(FastMCP):
    def streamable_http_app(self):
        app = super().streamable_http_app()
        app.add_middleware(
            InboundRequestLoggingMiddleware,
            allowed_hosts=ALLOWED_HOSTS,
        )
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
    logger.info(f"Log level: {VAULT_MCP_LOG_LEVEL}")
    logger.info(
        "Transport security bind_host=%s bind_port=%s allowed_hosts=%s extra_allowed_hosts=%s allow_self_ip_hosts=%s detected_self_ips=%s",
        VAULT_MCP_HOST,
        VAULT_MCP_PORT,
        ALLOWED_HOSTS,
        VAULT_MCP_EXTRA_ALLOWED_HOSTS,
        VAULT_MCP_ALLOW_SELF_IP_HOSTS,
        DETECTED_SELF_IPS,
    )
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
        allowed_hosts=ALLOWED_HOSTS,
    ),
)


@mcp.tool(
    name="vault_read",
    description="Read a vault file. Use numbered=true for line-window reads before vault_apply_unified_diff; use the returned full-file content hash.",
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
    numbered: bool = False,
) -> str:
    inp = VaultReadInput(
        path=path,
        start_line=start_line,
        end_line=end_line,
        tail_lines=tail_lines,
        numbered=numbered,
    )
    return _vault_read(
        inp.path, inp.start_line, inp.end_line, inp.tail_lines, inp.numbered
    )


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
    description=(
        "Apply a unified diff to an existing vault file (target = `path` arg). "
        "Prefer over vault_create_overwrite_file — cheaper and safer.\n\n"
        "Submit just the hunk(s); file headers (--- / +++) are optional and inferred from `path`. "
        "Standard full diffs with headers are also accepted.\n\n"
        "CRITICAL: the counts in @@ -L,N +L,N @@ must exactly match the lines in the hunk body. "
        "N counts context lines (' ') AND changed lines ('-'/'+'); context lines count toward both old and new. "
        "Common mistake: writing ,3 but omitting the context lines — the body then has only 1 line, not 3, and the patch is rejected.\n\n"
        "Example (no context): @@ -5,1 +5,1 @@\\n-old\\n+new\\n\n"
        "Example (1 context each side): @@ -4,3 +4,3 @@\\n ctx\\n-old\\n+new\\n ctx"
    ),
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
    """Run the authless MCP server over Streamable HTTP."""
    logging.basicConfig(
        level=_resolve_log_level(VAULT_MCP_LOG_LEVEL),
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
        stream=sys.stderr,
    )

    if not VAULT_PATH.is_dir():
        logger.error(f"Vault path does not exist: {VAULT_PATH}")
        sys.exit(1)

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


if __name__ == "__main__":
    main()
