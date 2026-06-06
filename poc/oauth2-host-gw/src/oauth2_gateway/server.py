"""CLI entrypoint for the OAuth-only gateway."""

import logging
import sys

import uvicorn
from starlette.applications import Starlette
from starlette.responses import JSONResponse
from starlette.routing import Route

from .config import OAUTH2_GATEWAY_PORT
from .oauth import oauth_routes

logger = logging.getLogger(__name__)


async def health(_request) -> JSONResponse:
    return JSONResponse({"status": "ok"})


def create_app() -> Starlette:
    return Starlette(routes=[Route("/health", health, methods=["GET"]), *oauth_routes])


def main() -> None:
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
        stream=sys.stderr,
    )

    logger.info(f"Starting OAuth2 gateway on port {OAUTH2_GATEWAY_PORT}")
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
