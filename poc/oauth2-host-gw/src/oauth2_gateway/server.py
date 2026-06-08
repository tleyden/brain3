"""CLI entrypoint for the OAuth + MCP gateway."""

import argparse
import logging
import sys
from collections.abc import AsyncIterator, Callable
from contextlib import asynccontextmanager
from pathlib import Path

import httpx
import uvicorn
from starlette.applications import Starlette
from starlette.responses import JSONResponse
from starlette.routing import Route

from .config import (
    OAUTH2_GATEWAY_EXPECTED_HOST,
    OAUTH2_GATEWAY_MCP_UPSTREAM_URL,
    OAUTH2_GATEWAY_PORT,
    OAUTH2_GATEWAY_UPSTREAM_SECRET_FILE,
)
from .mcp_proxy import mcp_routes
from .oauth import oauth_routes

logger = logging.getLogger(__name__)
DEFAULT_HOST = "127.0.0.1"


async def health(_request) -> JSONResponse:
    return JSONResponse({"status": "ok"})


def _default_http_client_factory() -> httpx.AsyncClient:
    return httpx.AsyncClient(timeout=None, follow_redirects=False, trust_env=False)


def _read_required_upstream_secret(secret_file: str) -> str:
    if not secret_file.strip():
        raise RuntimeError("MCP upstream shared secret file path is empty")

    try:
        secret = Path(secret_file).read_text(encoding="utf-8").strip()
    except OSError as exc:
        raise RuntimeError(f"Unable to read MCP upstream shared secret file: {secret_file}") from exc

    if not secret:
        raise RuntimeError(f"MCP upstream shared secret file is empty: {secret_file}")

    return secret


def create_app(
    *,
    mcp_upstream_url: str = OAUTH2_GATEWAY_MCP_UPSTREAM_URL,
    mcp_upstream_secret: str = "",
    expected_host: str | None = None,
    http_client_factory: Callable[[], httpx.AsyncClient] | None = None,
) -> Starlette:
    client_factory = http_client_factory or _default_http_client_factory

    @asynccontextmanager
    async def lifespan(app: Starlette) -> AsyncIterator[None]:
        app.state.mcp_upstream_url = mcp_upstream_url.rstrip("/")
        app.state.mcp_upstream_secret = mcp_upstream_secret
        app.state.expected_host = expected_host
        app.state.mcp_proxy_client = client_factory()
        try:
            yield
        finally:
            await app.state.mcp_proxy_client.aclose()

    return Starlette(
        lifespan=lifespan,
        routes=[Route("/health", health, methods=["GET"]), *mcp_routes, *oauth_routes],
    )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run the OAuth2 gateway server.")
    parser.add_argument(
        "--host",
        default=DEFAULT_HOST,
        help=f"Host interface to bind to. Defaults to {DEFAULT_HOST}.",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> None:
    args = parse_args(argv)

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
        stream=sys.stderr,
    )

    logger.info("Starting OAuth2 gateway on %s:%s", args.host, OAUTH2_GATEWAY_PORT)
    logger.info("Proxying MCP traffic to %s", OAUTH2_GATEWAY_MCP_UPSTREAM_URL)
    uvicorn.run(
        create_app(
            mcp_upstream_secret=_read_required_upstream_secret(OAUTH2_GATEWAY_UPSTREAM_SECRET_FILE),
            expected_host=OAUTH2_GATEWAY_EXPECTED_HOST,
        ),
        host=args.host,
        port=OAUTH2_GATEWAY_PORT,
        log_level="info",
        proxy_headers=True,
        forwarded_allow_ips="*",
    )


if __name__ == "__main__":
    main()
