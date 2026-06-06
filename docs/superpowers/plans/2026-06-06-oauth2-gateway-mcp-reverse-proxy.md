# OAuth2 Gateway MCP Reverse Proxy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn `poc/oauth2-host-gw` into the authenticated public entrypoint on port `8421` by keeping OAuth routes local and reverse-proxying protected MCP traffic at `/mcp` to the authless Obsidian MCP server on `http://127.0.0.1:8420`.

**Architecture:** Keep OAuth discovery, authorization, registration, and token exchange in the existing Starlette app. Add a dedicated MCP gateway module that exposes protected resource metadata, validates the bearer token, strips `Authorization`, and forwards `/mcp` requests to the upstream FastMCP Streamable HTTP server. Keep upstream resolution behind one helper so this single-server proxy can later evolve into an MCP-aware broker for `tools/list` and `tools/call` fan-out without rewriting the OAuth surface.

**Tech Stack:** Python 3.12+, Starlette, Uvicorn, httpx, unittest, FastMCP Streamable HTTP

---

## Scope Notes

- The current backend in `poc/obsidian-mcp-container` runs `FastMCP(..., stateless_http=True, json_response=True)` on port `8420` and serves Streamable HTTP at `/mcp`.
- The current gateway in `poc/oauth2-host-gw` only serves `/health` plus OAuth endpoints; it has no `/mcp` route today, which explains the client hang/failure once auth succeeds and the client asks for tools.
- This task does **not** replace the Python gateway with Docker MCP Gateway. That is a separate product choice. For this POC, the smallest correct fix is to make the existing Starlette gateway proxy MCP traffic.
- The Docker MCP Gateway and `mcp-oauth-proxy` notes are still useful as design guidance:
  - expose `/.well-known/oauth-protected-resource/mcp`
  - protect `/mcp` with bearer auth
  - strip `Authorization` before forwarding to the authless backend
  - keep a future seam for multi-server routing and tool aggregation
- Do **not** copy the Docker example’s `transport_type: sse` assumption. This backend currently speaks Streamable HTTP on `/mcp`, not SSE.
- Do **not** add fake `X-Forwarded-User` or `X-Forwarded-Email` headers in this phase. The current gateway returns a static bearer token and does not have real user identity claims to forward.
- Do **not** disable auth just because the backend later runs in a container. The host gateway remains the auth boundary whether the backend runs on the host or in a local container.

## File Map

- Modify: `poc/oauth2-host-gw/pyproject.toml`
  - Add `httpx` runtime dependency for proxying and for test transports.
- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/config.py`
  - Add upstream configuration for the MCP backend.
- Create: `poc/oauth2-host-gw/src/oauth2_gateway/mcp_proxy.py`
  - Own protected resource metadata, bearer token validation, header filtering, and reverse proxy logic.
- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/server.py`
  - Create and close a shared `httpx.AsyncClient`, register MCP routes, and keep existing OAuth routes intact.
- Create: `poc/oauth2-host-gw/tests/test_mcp_proxy.py`
  - Lock the protected resource metadata shape, auth behavior, proxy forwarding, and 502 handling.
- Modify: `poc/oauth2-host-gw/.env.template`
  - Document the upstream URL env var.
- Modify: `poc/oauth2-host-gw/README.md`
  - Document the new `/mcp` reverse-proxy behavior and manual verification flow.

### Task 1: Lock the HTTP Contract with Tests

**Files:**
- Modify: `poc/oauth2-host-gw/pyproject.toml`
- Create: `poc/oauth2-host-gw/tests/test_mcp_proxy.py`

- [ ] **Step 1: Add the proxy dependency before writing tests**

```toml
[project]
dependencies = [
    "httpx>=0.27.0",
    "python-multipart>=0.0.9",
    "starlette>=0.37.2",
    "uvicorn>=0.30.0",
]
```

- [ ] **Step 2: Install the dependency into the gateway virtualenv**

Run: `uv sync`

Expected: `httpx` is added to `.venv` and the lockfile updates cleanly.

- [ ] **Step 3: Write the failing gateway tests**

```python
import unittest
from unittest.mock import patch

import httpx
from starlette.testclient import TestClient

from oauth2_gateway.server import create_app


class GatewayProxyTests(unittest.TestCase):
    def test_protected_resource_metadata_points_to_gateway_mcp(self):
        app = create_app()

        with TestClient(app) as client:
            response = client.get("/.well-known/oauth-protected-resource/mcp")

        self.assertEqual(response.status_code, 200)
        self.assertEqual(response.json()["resource"], "http://testserver/mcp")
        self.assertEqual(response.json()["authorization_servers"], ["http://testserver"])

    def test_mcp_requires_bearer_token(self):
        app = create_app()

        with TestClient(app) as client:
            response = client.post(
                "/mcp",
                headers={
                    "accept": "application/json, text/event-stream",
                    "content-type": "application/json",
                },
                json={"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}},
            )

        self.assertEqual(response.status_code, 401)
        self.assertIn("resource_metadata=", response.headers["www-authenticate"])

    def test_mcp_proxy_strips_authorization_and_forwards_mcp_headers(self):
        captured = {}

        def handler(request: httpx.Request) -> httpx.Response:
            captured["url"] = str(request.url)
            captured["authorization"] = request.headers.get("authorization")
            captured["accept"] = request.headers.get("accept")
            captured["session"] = request.headers.get("mcp-session-id")
            captured["protocol"] = request.headers.get("mcp-protocol-version")
            return httpx.Response(
                200,
                headers={
                    "content-type": "application/json",
                    "mcp-session-id": "session-123",
                },
                json={"jsonrpc": "2.0", "id": 1, "result": {"tools": []}},
            )

        app = create_app(
            mcp_upstream_url="http://127.0.0.1:8420",
            http_client_factory=lambda: httpx.AsyncClient(
                transport=httpx.MockTransport(handler),
                timeout=None,
                follow_redirects=False,
                trust_env=False,
            ),
        )

        with patch("oauth2_gateway.config.OAUTH2_GATEWAY_ACCESS_TOKEN", "test-token"):
            with TestClient(app) as client:
                response = client.post(
                    "/mcp",
                    headers={
                        "authorization": "Bearer test-token",
                        "accept": "application/json, text/event-stream",
                        "content-type": "application/json",
                        "mcp-session-id": "session-123",
                        "mcp-protocol-version": "2025-03-26",
                    },
                    json={"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}},
                )

        self.assertEqual(response.status_code, 200)
        self.assertEqual(captured["url"], "http://127.0.0.1:8420/mcp")
        self.assertIsNone(captured["authorization"])
        self.assertEqual(captured["session"], "session-123")
        self.assertEqual(captured["protocol"], "2025-03-26")
        self.assertEqual(response.headers["mcp-session-id"], "session-123")

    def test_mcp_proxy_returns_502_when_upstream_is_unreachable(self):
        def handler(request: httpx.Request) -> httpx.Response:
            raise httpx.ConnectError("dial tcp 127.0.0.1:8420: connect refused", request=request)

        app = create_app(
            mcp_upstream_url="http://127.0.0.1:8420",
            http_client_factory=lambda: httpx.AsyncClient(
                transport=httpx.MockTransport(handler),
                timeout=None,
                follow_redirects=False,
                trust_env=False,
            ),
        )

        with patch("oauth2_gateway.config.OAUTH2_GATEWAY_ACCESS_TOKEN", "test-token"):
            with TestClient(app) as client:
                response = client.post(
                    "/mcp",
                    headers={
                        "authorization": "Bearer test-token",
                        "accept": "application/json, text/event-stream",
                        "content-type": "application/json",
                    },
                    json={"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}},
                )

        self.assertEqual(response.status_code, 502)
        self.assertEqual(response.json()["error"], "bad_gateway")
```

- [ ] **Step 4: Run the tests to confirm the current gateway is missing the required behavior**

Run: `uv run python -m unittest discover -s tests -v`

Expected: FAIL because `/.well-known/oauth-protected-resource/mcp` and `/mcp` do not exist yet, and `create_app()` does not yet accept injected proxy settings.

- [ ] **Step 5: Commit the failing test baseline**

```bash
git add pyproject.toml uv.lock tests/test_mcp_proxy.py
git commit -m "test: lock oauth gateway mcp proxy contract"
```

### Task 2: Implement Protected Resource Metadata and Reverse Proxy

**Files:**
- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/config.py`
- Create: `poc/oauth2-host-gw/src/oauth2_gateway/mcp_proxy.py`
- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/server.py`

- [ ] **Step 1: Add upstream configuration**

```python
import os

OAUTH2_GATEWAY_PORT = int(os.environ.get("OAUTH2_GATEWAY_PORT", "8421"))
OAUTH2_GATEWAY_CLIENT_ID = os.environ.get("OAUTH2_GATEWAY_CLIENT_ID", "oauth2-gateway-client")
OAUTH2_GATEWAY_CLIENT_SECRET = os.environ.get("OAUTH2_GATEWAY_CLIENT_SECRET", "")
OAUTH2_GATEWAY_ACCESS_TOKEN = os.environ.get("OAUTH2_GATEWAY_ACCESS_TOKEN", "")
OAUTH2_GATEWAY_MCP_UPSTREAM_URL = os.environ.get("OAUTH2_GATEWAY_MCP_UPSTREAM_URL", "http://127.0.0.1:8420")
```

- [ ] **Step 2: Create the MCP gateway module**

```python
"""Protected resource metadata and reverse proxy for MCP requests."""

import hmac
import logging

import httpx
from starlette.background import BackgroundTask
from starlette.requests import Request
from starlette.responses import JSONResponse, StreamingResponse
from starlette.routing import Route

from . import config

logger = logging.getLogger(__name__)

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

REQUEST_STRIP_HEADERS = HOP_BY_HOP_HEADERS | {"authorization", "content-length", "host"}


def _base_url(request: Request) -> str:
    return str(request.base_url).rstrip("/")


def _resource_metadata_url(request: Request) -> str:
    return f"{_base_url(request)}/.well-known/oauth-protected-resource/mcp"


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
    base_url = _base_url(request)
    return JSONResponse(
        {
            "resource": f"{base_url}/mcp",
            "authorization_servers": [base_url],
        }
    )


async def mcp_reverse_proxy(request: Request) -> StreamingResponse | JSONResponse:
    if not _is_authorized(request):
        return _unauthorized(request, "Missing or invalid bearer token")

    client: httpx.AsyncClient = request.app.state.mcp_proxy_client
    upstream_url = _build_upstream_url(request)

    try:
        upstream_request = client.build_request(
            request.method,
            upstream_url,
            headers=_filter_request_headers(request),
            content=await request.body(),
        )
        upstream_response = await client.send(upstream_request, stream=True)
    except httpx.HTTPError as exc:
        logger.warning("MCP upstream unavailable: %s", exc)
        return JSONResponse(
            {"error": "bad_gateway", "error_description": "MCP upstream unavailable"},
            status_code=502,
        )

    return StreamingResponse(
        upstream_response.aiter_raw(),
        status_code=upstream_response.status_code,
        headers=_filter_response_headers(upstream_response),
        background=BackgroundTask(upstream_response.aclose),
    )


mcp_routes = [
    Route("/.well-known/oauth-protected-resource/mcp", protected_resource_metadata, methods=["GET"]),
    Route("/mcp", mcp_reverse_proxy, methods=["GET", "POST", "DELETE"]),
    Route("/mcp/", mcp_reverse_proxy, methods=["GET", "POST", "DELETE"]),
    Route("/mcp/{path:path}", mcp_reverse_proxy, methods=["GET", "POST", "DELETE"]),
]
```

- [ ] **Step 3: Wire the proxy client and routes into the gateway app**

```python
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
```

- [ ] **Step 4: Run the tests to verify the proxy contract**

Run: `uv run python -m unittest discover -s tests -v`

Expected: PASS for protected resource metadata, bearer auth rejection, header stripping/forwarding, and 502 handling.

- [ ] **Step 5: Commit the proxy implementation**

```bash
git add src/oauth2_gateway/config.py src/oauth2_gateway/mcp_proxy.py src/oauth2_gateway/server.py tests/test_mcp_proxy.py
git commit -m "feat: proxy mcp traffic through oauth gateway"
```

### Task 3: Document the New Runtime Contract

**Files:**
- Modify: `poc/oauth2-host-gw/.env.template`
- Modify: `poc/oauth2-host-gw/README.md`

- [ ] **Step 1: Document the upstream env var**

```dotenv
# Optional: upstream MCP server base URL (default: http://127.0.0.1:8420)
OAUTH2_GATEWAY_MCP_UPSTREAM_URL=http://127.0.0.1:8420
```

- [ ] **Step 2: Update the README to reflect the new scope**

```markdown
It keeps:
- OAuth metadata discovery
- dynamic client registration
- authorization-code redirect handling
- token exchange with PKCE support
- protected resource metadata at `/.well-known/oauth-protected-resource/mcp`
- reverse proxying of authenticated MCP traffic from `/mcp` to a local authless MCP server

It intentionally does not include yet:
- multi-server routing
- tool aggregation / scatter-gather
- container packaging

Environment variables:
- `OAUTH2_GATEWAY_PORT`: HTTP port, defaults to `8421`
- `OAUTH2_GATEWAY_CLIENT_ID`: client id returned by registration
- `OAUTH2_GATEWAY_CLIENT_SECRET`: client secret returned by registration and accepted by token exchange
- `OAUTH2_GATEWAY_ACCESS_TOKEN`: static bearer token required on `/mcp`
- `OAUTH2_GATEWAY_MCP_UPSTREAM_URL`: upstream MCP base URL, defaults to `http://127.0.0.1:8420`
```

- [ ] **Step 3: Add a README smoke test that proves the exact client flow**

```bash
cd /Users/tleyden/Development/agentzoo/poc/obsidian-mcp-container
VAULT_PATH="$PWD/test_vault" uv run obsidian-mcp-server
```

```bash
cd /Users/tleyden/Development/agentzoo/poc/oauth2-host-gw
OAUTH2_GATEWAY_CLIENT_SECRET=dev-secret \
OAUTH2_GATEWAY_ACCESS_TOKEN=dev-token \
OAUTH2_GATEWAY_MCP_UPSTREAM_URL=http://127.0.0.1:8420 \
uv run oauth2-gateway
```

```bash
curl -i -X POST http://127.0.0.1:8421/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'
```

Expected: `401 Unauthorized` with a `WWW-Authenticate` header that includes `resource_metadata="http://127.0.0.1:8421/.well-known/oauth-protected-resource/mcp"`.

```bash
TOKEN=$(curl -s -X POST http://127.0.0.1:8421/oauth/token \
  -d "grant_type=client_credentials" \
  -d "client_id=oauth2-gateway-client" \
  -d "client_secret=dev-secret" | python3 -c "import json,sys; print(json.load(sys.stdin)['access_token'])")

curl -s -X POST http://127.0.0.1:8421/mcp \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'
```

Expected: `200 OK` with a JSON-RPC result containing Obsidian tools such as `vault_read`, `vault_search`, and `vault_write`.

- [ ] **Step 4: Commit the docs update**

```bash
git add .env.template README.md
git commit -m "docs: explain oauth gateway mcp proxy flow"
```

### Task 4: End-to-End Validation Against the Real Backend

**Files:**
- Test only: `poc/oauth2-host-gw`
- Test only: `poc/obsidian-mcp-container`

- [ ] **Step 1: Run the gateway unit tests one more time**

Run: `uv run python -m unittest discover -s tests -v`

Expected: PASS

- [ ] **Step 2: Verify the protected resource discovery endpoint**

Run: `curl -s http://127.0.0.1:8421/.well-known/oauth-protected-resource/mcp`

Expected:

```json
{
  "resource": "http://127.0.0.1:8421/mcp",
  "authorization_servers": ["http://127.0.0.1:8421"]
}
```

- [ ] **Step 3: Verify a real authenticated MCP request through the gateway**

Run:

```bash
cd /Users/tleyden/Development/agentzoo/poc/oauth2-host-gw
uv run python - <<'PY'
import json

import httpx

BASE = "http://127.0.0.1:8421"
TOKEN = "dev-token"

headers = {
    "authorization": f"Bearer {TOKEN}",
    "accept": "application/json, text/event-stream",
    "content-type": "application/json",
}

with httpx.Client(base_url=BASE, timeout=30.0) as client:
    response = client.post(
        "/mcp",
        headers=headers,
        json={"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}},
    )
    response.raise_for_status()
    payload = response.json()
    tool_names = [tool["name"] for tool in payload["result"]["tools"]]
    assert "vault_read" in tool_names, tool_names
    assert "vault_search" in tool_names, tool_names
    print(json.dumps(tool_names, indent=2))
PY
```

Expected: exit code `0` and printed tool names from the backend server reached via `8421 -> 8420`.

- [ ] **Step 4: Confirm the intended trust boundary**

Run: `curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:8421/health`

Expected: `200`

Run: `curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:8421/mcp`

Expected: `401`

Meaning: health remains public, MCP stays protected, and the backend remains authless behind the host gateway.

- [ ] **Step 5: Commit the validated runtime changes**

```bash
git add pyproject.toml uv.lock src/oauth2_gateway/config.py src/oauth2_gateway/mcp_proxy.py src/oauth2_gateway/server.py tests/test_mcp_proxy.py .env.template README.md
git commit -m "feat: add authenticated mcp reverse proxy to oauth gateway"
```

## Phase 2 Follow-On: Multi-Server Gateway

Do not implement this in the single-server task, but preserve the seam for it now.

### Future Direction

- Replace the single `mcp_upstream_url` string with a registry of named upstreams that are explicitly marked as exposable.
- Move from a raw HTTP reverse proxy to an MCP-aware broker only when you actually need aggregation. A true 25-server scatter-gather gateway must understand `tools/list`, `resources/list`, `prompts/list`, and `tools/call`; pure path-based proxying is not enough.
- Add a collision policy for duplicate tool names before multi-server aggregation exists. Examples: prefix tools by server name, reject duplicates at startup, or keep a static mapping table.
- Cache aggregated capability catalogs to keep `tools/list` fast and avoid fan-out on every client request.

### Entry Condition for Phase 2

Only start the multi-server design once the single-server flow is stable and the following work is complete:

- `8421` is the only public/authenticated endpoint clients need
- `/mcp` requests proxy reliably to `8420`
- protected resource metadata is in place
- the gateway can be pointed at a host process today and a containerized backend later just by changing `OAUTH2_GATEWAY_MCP_UPSTREAM_URL`

## Self-Review

- Spec coverage: the plan covers the missing `/mcp` route, best-practice protected resource metadata, bearer auth on the proxy, forwarding to `8420`, docs, and end-to-end verification.
- Placeholder scan: no `TODO`, `TBD`, or “handle appropriately later” steps remain in the implementation tasks.
- Type consistency: all code snippets use the same `create_app(..., mcp_upstream_url=..., http_client_factory=...)` seam and the same `OAUTH2_GATEWAY_MCP_UPSTREAM_URL` config name.
