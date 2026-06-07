"""CLI entrypoint for the OAuth + MCP gateway."""

import argparse
import logging
import sys
from collections.abc import AsyncIterator, Callable
from contextlib import asynccontextmanager

import httpx
import uvicorn
from starlette.applications import Starlette
from starlette.responses import JSONResponse
from starlette.routing import Route

from .config import OAUTH2_GATEWAY_MCP_UPSTREAM_URL, OAUTH2_GATEWAY_PORT
from .mcp_proxy import mcp_routes
from .oauth import oauth_routes
from .token_store import TokenStore

logger = logging.getLogger(__name__)
DEFAULT_HOST = "127.0.0.1"


async def health(_request) -> JSONResponse:
    return JSONResponse({"status": "ok"})


def _default_http_client_factory() -> httpx.AsyncClient:
    return httpx.AsyncClient(timeout=None, follow_redirects=False, trust_env=False)


def create_app(
    *,
    mcp_upstream_url: str = OAUTH2_GATEWAY_MCP_UPSTREAM_URL,
    http_client_factory: Callable[[], httpx.AsyncClient] | None = None,
) -> Starlette:
    client_factory = http_client_factory or _default_http_client_factory

    @asynccontextmanager
    async def lifespan(app: Starlette) -> AsyncIterator[None]:
        app.state.mcp_upstream_url = mcp_upstream_url.rstrip("/")
        app.state.http_client = client_factory()
        app.state.mcp_proxy_client = app.state.http_client
        app.state.token_store = TokenStore()
        try:
            yield
        finally:
            await app.state.http_client.aclose()

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
        create_app(),
        host=args.host,
        port=OAUTH2_GATEWAY_PORT,
        log_level="info",
        proxy_headers=True,
        forwarded_allow_ips="*",
    )


if __name__ == "__main__":
    main()
