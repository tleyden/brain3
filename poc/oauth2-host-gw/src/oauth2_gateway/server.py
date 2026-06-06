"""CLI entrypoint for the OAuth + MCP gateway."""

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

logger = logging.getLogger(__name__)


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
        app.state.mcp_proxy_client = client_factory()
        try:
            yield
        finally:
            await app.state.mcp_proxy_client.aclose()

    return Starlette(
        lifespan=lifespan,
        routes=[Route("/health", health, methods=["GET"]), *mcp_routes, *oauth_routes],
    )


def main() -> None:
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
        stream=sys.stderr,
    )

    logger.info("Starting OAuth2 gateway on port %s", OAUTH2_GATEWAY_PORT)
    logger.info("Proxying MCP traffic to %s", OAUTH2_GATEWAY_MCP_UPSTREAM_URL)
    uvicorn.run(
        create_app(),
        host="0.0.0.0",
        port=OAUTH2_GATEWAY_PORT,
        log_level="info",
        proxy_headers=True,
        forwarded_allow_ips="*",
    )


if __name__ == "__main__":
    main()
