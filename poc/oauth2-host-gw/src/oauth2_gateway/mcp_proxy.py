"""Protected resource metadata and reverse proxy for MCP requests."""

import hmac
import logging

import httpx
from starlette.requests import Request
from starlette.responses import JSONResponse, Response
from starlette.routing import Route

from . import config

logger = logging.getLogger(__name__)
UPSTREAM_SHARED_SECRET_HEADER = "x-agentzoo-upstream-secret"

HOP_BY_HOP_HEADERS = {
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
}

REQUEST_STRIP_HEADERS = HOP_BY_HOP_HEADERS | {
    "authorization",
    "content-length",
    "host",
    UPSTREAM_SHARED_SECRET_HEADER,
}


def _base_url(request: Request) -> str:
    return str(request.base_url).rstrip("/")


def _resource_metadata_url(request: Request) -> str:
    return f"{_base_url(request)}/.well-known/oauth-protected-resource/mcp"


def _misdirected_request(description: str) -> JSONResponse:
    return JSONResponse(
        {"error": "misdirected_request", "error_description": description},
        status_code=421,
    )


def _log_request_host_details(request: Request, *, route_name: str) -> None:
    logger.info(
        (
            "MCP request host details route=%s method=%s path=%s "
            "host=%r x_forwarded_host=%r x_forwarded_proto=%r url=%s base_url=%s client=%s"
        ),
        route_name,
        request.method,
        request.url.path,
        request.headers.get("host"),
        request.headers.get("x-forwarded-host"),
        request.headers.get("x-forwarded-proto"),
        request.url,
        request.base_url,
        request.client,
    )


def _validate_request_host(request: Request) -> JSONResponse | None:
    expected_host = getattr(request.app.state, "expected_host", None)
    if not expected_host:
        logger.warning(
            "Skipping hostname validation; configure either a named Cloudflare tunnel on your own domain or DIRECT_PUBLIC_ORIGIN_HOSTNAME for a direct public origin deployment."
        )
        return None

    request_host = request.url.hostname
    if request_host == expected_host:
        return None

    logger.warning(
        "Rejecting MCP request for unexpected host: got=%r expected=%r",
        request_host,
        expected_host,
    )
    return _misdirected_request("Request host does not match the configured public hostname")


def _unauthorized(request: Request, description: str) -> JSONResponse:
    www_authenticate = (
        'Bearer error="invalid_token", '
        f'error_description="{description}", '
        f'resource_metadata="{_resource_metadata_url(request)}"'
    )
    return JSONResponse(
        {"error": "invalid_token", "error_description": description},
        status_code=401,
        headers={"WWW-Authenticate": www_authenticate},
    )


def _is_authorized(request: Request) -> bool:
    auth_header = request.headers.get("authorization", "")
    scheme, _, token = auth_header.partition(" ")
    if scheme.lower() != "bearer" or not token:
        return False
    return bool(config.OAUTH2_GATEWAY_ACCESS_TOKEN) and hmac.compare_digest(
        token,
        config.OAUTH2_GATEWAY_ACCESS_TOKEN,
    )


def _build_upstream_url(request: Request) -> str:
    path = request.url.path
    if path == "/mcp/":
        path = "/mcp"
    query = f"?{request.url.query}" if request.url.query else ""
    return f"{request.app.state.mcp_upstream_url}{path}{query}"


def _filter_request_headers(request: Request) -> dict[str, str]:
    return {
        key: value
        for key, value in request.headers.items()
        if key.lower() not in REQUEST_STRIP_HEADERS
    }


def _filter_response_headers(response: httpx.Response) -> dict[str, str]:
    return {
        key: value
        for key, value in response.headers.items()
        if key.lower() not in HOP_BY_HOP_HEADERS
    }


async def protected_resource_metadata(request: Request) -> JSONResponse:
    _log_request_host_details(request, route_name="protected_resource_metadata")
    host_validation_error = _validate_request_host(request)
    if host_validation_error is not None:
        return host_validation_error

    base_url = _base_url(request)
    return JSONResponse(
        {
            "resource": f"{base_url}/mcp",
            "authorization_servers": [base_url],
        }
    )


async def mcp_reverse_proxy(request: Request) -> Response | JSONResponse:
    _log_request_host_details(request, route_name="mcp_reverse_proxy")
    host_validation_error = _validate_request_host(request)
    if host_validation_error is not None:
        return host_validation_error

    if not _is_authorized(request):
        return _unauthorized(request, "Missing or invalid bearer token")

    client: httpx.AsyncClient = request.app.state.mcp_proxy_client
    upstream_url = _build_upstream_url(request)
    upstream_headers = _filter_request_headers(request)
    upstream_headers[UPSTREAM_SHARED_SECRET_HEADER] = request.app.state.mcp_upstream_secret

    try:
        upstream_request = client.build_request(
            request.method,
            upstream_url,
            headers=upstream_headers,
            content=await request.body(),
        )
        upstream_response = await client.send(upstream_request, stream=True)
    except httpx.HTTPError as exc:
        logger.warning("MCP upstream unavailable: %s", exc)
        return JSONResponse(
            {"error": "bad_gateway", "error_description": "MCP upstream unavailable"},
            status_code=502,
        )

    body = await upstream_response.aread()
    await upstream_response.aclose()

    return Response(
        content=body,
        status_code=upstream_response.status_code,
        headers=_filter_response_headers(upstream_response),
    )


mcp_routes = [
    Route("/.well-known/oauth-protected-resource/mcp", protected_resource_metadata, methods=["GET"]),
    Route("/mcp", mcp_reverse_proxy, methods=["GET", "POST", "DELETE"]),
    Route("/mcp/", mcp_reverse_proxy, methods=["GET", "POST", "DELETE"]),
    Route("/mcp/{path:path}", mcp_reverse_proxy, methods=["GET", "POST", "DELETE"]),
]
