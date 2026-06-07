# ChatGPT MCP CIMD Compatibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `poc/oauth2-host-gw` discoverable and interoperable with ChatGPT MCP by restoring the live `.well-known` surface, supporting CIMD-based public OAuth clients with PKCE, and issuing resource-bound bearer tokens for `/mcp`.

**Architecture:** Keep the existing Starlette gateway as both the MCP resource server and the colocated OAuth authorization server. Extend the OAuth surface so preregistered clients, DCR clients, and CIMD clients can all coexist, but treat CIMD plus `authorization_code` + PKCE + token auth method `none` as the primary ChatGPT path. Replace the single static bearer token assumption with an in-memory token store that binds issued access tokens to the canonical MCP resource URL.

**Tech Stack:** Python 3.12+, Starlette, Uvicorn, httpx, unittest, OAuth 2.1, RFC 8414 metadata, RFC 9728 protected resource metadata, RFC 8707 resource indicators, MCP authorization spec

---

## Scope Notes

- The reverse proxy contract in `poc/oauth2-host-gw/src/oauth2_gateway/mcp_proxy.py` already exists and is covered by `tests/test_mcp_proxy.py`. This plan does **not** rebuild that proxy layer.
- The current compatibility gap is the OAuth surface in `poc/oauth2-host-gw/src/oauth2_gateway/oauth.py`: it assumes one static `client_id`, only advertises `client_secret_post`, and returns one static bearer token for all callers.
- ChatGPT MCP compatibility requires three things that are currently either missing or incomplete:
  - a live, reachable discovery surface on the public hostname
  - CIMD support advertised in auth server metadata and implemented in the authorize/token handlers
  - resource-aware token issuance and validation for `/mcp`
- Keep the preregistered static client path and `/oauth/register` path for Claude or manual testing. Do not regress them while adding CIMD.
- Do **not** advertise `offline_access` until refresh token issuance and rotation actually exist. That is a separate phase at the end of this plan.
- Do **not** implement `private_key_jwt` in phase 1. ChatGPT supports `none` or `private_key_jwt` for CIMD clients; `none` with PKCE is the smallest correct first implementation.

## File Map

- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/oauth.py`
  - Add OAuth/OpenID metadata fields, CIMD-aware authorize/token flows, resource handling, and opaque token issuance.
- Create: `poc/oauth2-host-gw/src/oauth2_gateway/client_metadata.py`
  - Fetch, validate, and cache CIMD client metadata documents.
- Create: `poc/oauth2-host-gw/src/oauth2_gateway/token_store.py`
  - Store authorization codes and issued access tokens in memory with resource binding and expiry.
- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/mcp_proxy.py`
  - Validate opaque issued tokens from the token store instead of comparing against one env var.
- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/server.py`
  - Wire shared `httpx.AsyncClient` and `TokenStore` instances into app state.
- Create: `poc/oauth2-host-gw/tests/test_oauth_metadata.py`
  - Lock auth server metadata, OpenID alias metadata, and preregistered client compatibility.
- Create: `poc/oauth2-host-gw/tests/test_oauth_cimd.py`
  - Lock CIMD client metadata validation, PKCE exchange, and resource propagation.
- Modify: `poc/oauth2-host-gw/tests/test_mcp_proxy.py`
  - Assert the `WWW-Authenticate` challenge continues to expose `resource_metadata` and that valid issued tokens are accepted.
- Modify: `poc/oauth2-host-gw/.env.template`
  - Remove the static access token as the primary contract and document any dev-only override explicitly if retained.
- Modify: `poc/oauth2-host-gw/README.md`
  - Document CIMD support, preregistered fallback, the public discovery endpoints, and live verification commands.

## Runtime Design Decisions

- **Primary ChatGPT path:** `authorization_code` + PKCE + CIMD + token auth method `none`
- **Fallback path:** preregistered static client ID plus `client_credentials` or `authorization_code`
- **DCR path:** keep `/oauth/register`, but do not rely on it for ChatGPT
- **Canonical resource:** `https://<host>/mcp` or `http://127.0.0.1:8421/mcp` in local tests
- **Token format:** opaque random bearer tokens stored server-side, not one static env var
- **Metadata endpoints:**
  - `/.well-known/oauth-authorization-server`
  - `/.well-known/openid-configuration`
  - `/.well-known/oauth-protected-resource/mcp`

### Task 1: Lock the Discovery and Registration Contract with Tests

**Files:**
- Create: `poc/oauth2-host-gw/tests/test_oauth_metadata.py`
- Create: `poc/oauth2-host-gw/tests/test_oauth_cimd.py`
- Modify: `poc/oauth2-host-gw/tests/test_mcp_proxy.py`

- [ ] **Step 1: Write a failing auth metadata test for ChatGPT-visible fields**

```python
import warnings
import unittest

from starlette.exceptions import StarletteDeprecationWarning
from starlette.testclient import TestClient

from oauth2_gateway.server import create_app

warnings.filterwarnings(
    "ignore",
    message=r"Using `httpx` with `starlette\.testclient` is deprecated; install `httpx2` instead\.",
    category=StarletteDeprecationWarning,
)


class OAuthMetadataTests(unittest.TestCase):
    def test_oauth_metadata_advertises_cimd_and_public_client_support(self):
        app = create_app()

        with TestClient(app) as client:
            response = client.get("/.well-known/oauth-authorization-server")

        self.assertEqual(response.status_code, 200)
        payload = response.json()
        self.assertEqual(payload["issuer"], "http://testserver")
        self.assertEqual(payload["authorization_endpoint"], "http://testserver/oauth/authorize")
        self.assertEqual(payload["token_endpoint"], "http://testserver/oauth/token")
        self.assertEqual(payload["registration_endpoint"], "http://testserver/oauth/register")
        self.assertTrue(payload["client_id_metadata_document_supported"])
        self.assertIn("none", payload["token_endpoint_auth_methods_supported"])
        self.assertIn("client_secret_post", payload["token_endpoint_auth_methods_supported"])
```

- [ ] **Step 2: Write a failing OpenID alias test**

```python
    def test_openid_configuration_matches_oauth_metadata(self):
        app = create_app()

        with TestClient(app) as client:
            oauth_response = client.get("/.well-known/oauth-authorization-server")
            openid_response = client.get("/.well-known/openid-configuration")

        self.assertEqual(openid_response.status_code, 200)
        self.assertEqual(openid_response.json(), oauth_response.json())
```

- [ ] **Step 3: Write failing CIMD authorize/token tests**

```python
import unittest
from unittest.mock import patch

import httpx
from starlette.testclient import TestClient

from oauth2_gateway.server import create_app


CLIENT_ID = "https://chatgpt.example/client-metadata.json"
REDIRECT_URI = "https://chat.openai.com/aip/callback/example"


class OAuthCimdTests(unittest.TestCase):
    def _build_app(self):
        def handler(request: httpx.Request) -> httpx.Response:
            if str(request.url) == CLIENT_ID:
                return httpx.Response(
                    200,
                    json={
                        "client_id": CLIENT_ID,
                        "client_name": "ChatGPT MCP Client",
                        "redirect_uris": [REDIRECT_URI],
                        "grant_types": ["authorization_code"],
                        "response_types": ["code"],
                        "token_endpoint_auth_method": "none",
                    },
                )
            raise AssertionError(f"unexpected url: {request.url}")

        return create_app(
            http_client_factory=lambda: httpx.AsyncClient(
                transport=httpx.MockTransport(handler),
                timeout=None,
                follow_redirects=False,
                trust_env=False,
            ),
        )

    def test_authorize_accepts_cimd_client_id_url_and_redirect_uri(self):
        app = self._build_app()

        with TestClient(app) as client:
            response = client.get(
                "/oauth/authorize",
                params={
                    "response_type": "code",
                    "client_id": CLIENT_ID,
                    "redirect_uri": REDIRECT_URI,
                    "code_challenge": "verifier-challenge",
                    "code_challenge_method": "S256",
                    "resource": "http://testserver/mcp",
                    "state": "abc123",
                },
                follow_redirects=False,
            )

        self.assertEqual(response.status_code, 302)
        self.assertIn("code=", response.headers["location"])
        self.assertIn("state=abc123", response.headers["location"])

    def test_token_exchange_accepts_public_cimd_client_without_secret(self):
        app = self._build_app()

        with TestClient(app) as client:
            authorize = client.get(
                "/oauth/authorize",
                params={
                    "response_type": "code",
                    "client_id": CLIENT_ID,
                    "redirect_uri": REDIRECT_URI,
                    "code_challenge": "Z_P4EKbGwIkA01e3Y5fp4tMCvn_Ae5nUw7qY7XwkTrQ",
                    "code_challenge_method": "S256",
                    "resource": "http://testserver/mcp",
                },
                follow_redirects=False,
            )
            code = authorize.headers["location"].split("code=")[1].split("&")[0]

            token = client.post(
                "/oauth/token",
                data={
                    "grant_type": "authorization_code",
                    "client_id": CLIENT_ID,
                    "redirect_uri": REDIRECT_URI,
                    "code": code,
                    "code_verifier": "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
                    "resource": "http://testserver/mcp",
                },
            )

        self.assertEqual(token.status_code, 200)
        self.assertIn("access_token", token.json())
        self.assertEqual(token.json()["token_type"], "bearer")
```

- [ ] **Step 4: Tighten the existing MCP challenge test around `resource_metadata`**

```python
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
        challenge = response.headers["www-authenticate"]
        self.assertIn('resource_metadata="http://testserver/.well-known/oauth-protected-resource/mcp"', challenge)
```

- [ ] **Step 5: Run the tests to confirm the current gateway fails on the missing ChatGPT contract**

Run: `uv run python -m unittest discover -s tests -v`

Expected: FAIL because `client_id_metadata_document_supported` is absent, `/.well-known/openid-configuration` does not exist, CIMD client IDs are rejected, and the token endpoint only advertises `client_secret_post`.

### Task 2: Add CIMD Metadata Fetching and Validation

**Files:**
- Create: `poc/oauth2-host-gw/src/oauth2_gateway/client_metadata.py`
- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/server.py`

- [ ] **Step 1: Create a focused helper for CIMD documents**

```python
"""Fetch and validate OAuth Client ID Metadata Documents."""

from __future__ import annotations

from dataclasses import dataclass
from urllib.parse import urlparse

import httpx


@dataclass(slots=True)
class ClientMetadata:
    client_id: str
    client_name: str
    redirect_uris: list[str]
    token_endpoint_auth_method: str


def is_cimd_client_id(client_id: str) -> bool:
    parsed = urlparse(client_id)
    return parsed.scheme == "https" and bool(parsed.netloc) and bool(parsed.path)


async def fetch_client_metadata(client: httpx.AsyncClient, client_id: str) -> ClientMetadata:
    response = await client.get(client_id)
    response.raise_for_status()
    payload = response.json()

    if payload.get("client_id") != client_id:
        raise ValueError("client_id metadata document mismatch")

    redirect_uris = payload.get("redirect_uris")
    if not isinstance(redirect_uris, list) or not redirect_uris:
        raise ValueError("redirect_uris required")

    token_auth_method = payload.get("token_endpoint_auth_method", "none")
    if token_auth_method != "none":
        raise ValueError(f"unsupported CIMD token auth method: {token_auth_method}")

    return ClientMetadata(
        client_id=client_id,
        client_name=payload.get("client_name", ""),
        redirect_uris=redirect_uris,
        token_endpoint_auth_method=token_auth_method,
    )
```

- [ ] **Step 2: Expose the shared HTTP client to both the proxy and OAuth layers**

```python
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
        try:
            yield
        finally:
            await app.state.http_client.aclose()
```

- [ ] **Step 3: Run only the new CIMD tests**

Run: `uv run python -m unittest tests.test_oauth_cimd -v`

Expected: still FAIL because the OAuth handlers are not using `client_metadata.py` yet.

### Task 3: Replace Static Bearer Tokens with a Resource-Bound Token Store

**Files:**
- Create: `poc/oauth2-host-gw/src/oauth2_gateway/token_store.py`
- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/oauth.py`
- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/mcp_proxy.py`
- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/server.py`

- [ ] **Step 1: Create an in-memory store for authorization codes and access tokens**

```python
"""In-memory storage for auth codes and opaque bearer tokens."""

from __future__ import annotations

from dataclasses import dataclass
import secrets
import time


@dataclass(slots=True)
class AuthorizationCode:
    client_id: str
    redirect_uri: str
    resource: str
    code_challenge: str
    expires_at: float


@dataclass(slots=True)
class AccessToken:
    token: str
    client_id: str
    resource: str
    expires_at: float


class TokenStore:
    def __init__(self) -> None:
        self._codes: dict[str, AuthorizationCode] = {}
        self._tokens: dict[str, AccessToken] = {}

    def issue_code(self, *, client_id: str, redirect_uri: str, resource: str, code_challenge: str) -> str:
        code = secrets.token_urlsafe(32)
        self._codes[code] = AuthorizationCode(
            client_id=client_id,
            redirect_uri=redirect_uri,
            resource=resource,
            code_challenge=code_challenge,
            expires_at=time.time() + 300,
        )
        return code

    def pop_code(self, code: str) -> AuthorizationCode | None:
        code_data = self._codes.pop(code, None)
        if code_data and code_data.expires_at >= time.time():
            return code_data
        return None

    def issue_access_token(self, *, client_id: str, resource: str) -> AccessToken:
        token = secrets.token_urlsafe(32)
        access_token = AccessToken(
            token=token,
            client_id=client_id,
            resource=resource,
            expires_at=time.time() + 86400,
        )
        self._tokens[token] = access_token
        return access_token

    def get_access_token(self, token: str) -> AccessToken | None:
        token_data = self._tokens.get(token)
        if token_data and token_data.expires_at >= time.time():
            return token_data
        return None
```

- [ ] **Step 2: Wire the token store into app state**

```python
from .token_store import TokenStore


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
```

- [ ] **Step 3: Validate issued opaque tokens in the MCP proxy**

```python
def _is_authorized(request: Request) -> bool:
    auth_header = request.headers.get("authorization", "")
    scheme, _, token = auth_header.partition(" ")
    if scheme.lower() != "bearer" or not token:
        return False

    token_data = request.app.state.token_store.get_access_token(token)
    if token_data is None:
        return False

    requested_resource = f"{_base_url(request)}/mcp"
    return hmac.compare_digest(token_data.resource, requested_resource)
```

- [ ] **Step 4: Run the existing proxy tests**

Run: `uv run python -m unittest tests.test_mcp_proxy -v`

Expected: FAIL until the test fixture uses a real issued token or a patched `TokenStore`.

### Task 4: Implement the CIMD-Aware OAuth Handlers and Metadata

**Files:**
- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/oauth.py`

- [ ] **Step 1: Expand the metadata response to advertise CIMD and an OpenID alias**

```python
def _metadata_payload(base_url: str) -> dict:
    return {
        "issuer": base_url,
        "authorization_endpoint": f"{base_url}/oauth/authorize",
        "token_endpoint": f"{base_url}/oauth/token",
        "registration_endpoint": f"{base_url}/oauth/register",
        "grant_types_supported": ["authorization_code", "client_credentials"],
        "response_types_supported": ["code"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none", "client_secret_post"],
        "client_id_metadata_document_supported": True,
    }


async def oauth_metadata(request: Request) -> JSONResponse:
    return JSONResponse(_metadata_payload(str(request.base_url).rstrip("/")))


async def openid_metadata(request: Request) -> JSONResponse:
    return JSONResponse(_metadata_payload(str(request.base_url).rstrip("/")))
```

- [ ] **Step 2: Accept CIMD client IDs during authorization**

```python
from .client_metadata import fetch_client_metadata, is_cimd_client_id


def _canonical_resource(request: Request) -> str:
    return request.query_params.get("resource") or f"{str(request.base_url).rstrip('/')}/mcp"


async def oauth_authorize(request: Request) -> JSONResponse | RedirectResponse:
    response_type = request.query_params.get("response_type", "")
    client_id = request.query_params.get("client_id", "")
    redirect_uri = request.query_params.get("redirect_uri", "")
    state = request.query_params.get("state", "")
    code_challenge = request.query_params.get("code_challenge", "")

    if response_type != "code":
        return JSONResponse({"error": "unsupported_response_type"}, status_code=400)

    if not redirect_uri:
        return JSONResponse({"error": "invalid_request", "error_description": "redirect_uri required"}, status_code=400)

    if is_cimd_client_id(client_id):
        metadata = await fetch_client_metadata(request.app.state.http_client, client_id)
        if redirect_uri not in metadata.redirect_uris:
            return JSONResponse({"error": "invalid_request", "error_description": "redirect_uri not allowed"}, status_code=400)
    elif client_id and client_id != config.OAUTH2_GATEWAY_CLIENT_ID:
        return JSONResponse({"error": "invalid_client"}, status_code=401)

    code = request.app.state.token_store.issue_code(
        client_id=client_id,
        redirect_uri=redirect_uri,
        resource=_canonical_resource(request),
        code_challenge=code_challenge,
    )
```

- [ ] **Step 3: Accept a public CIMD token exchange with `token_endpoint_auth_method=none`**

```python
async def _handle_authorization_code(request: Request, form, client_id: str) -> JSONResponse:
    code = form.get("code", "")
    redirect_uri = form.get("redirect_uri", "")
    code_verifier = form.get("code_verifier", "")
    requested_resource = form.get("resource", "")

    code_data = request.app.state.token_store.pop_code(code)
    if code_data is None:
        return JSONResponse({"error": "invalid_grant", "error_description": "Invalid or expired code"}, status_code=400)

    if client_id != code_data.client_id:
        return JSONResponse({"error": "invalid_client"}, status_code=401)

    if redirect_uri != code_data.redirect_uri:
        return JSONResponse({"error": "invalid_grant", "error_description": "redirect_uri mismatch"}, status_code=400)

    if requested_resource and requested_resource != code_data.resource:
        return JSONResponse({"error": "invalid_target"}, status_code=400)

    if is_cimd_client_id(client_id):
        metadata = await fetch_client_metadata(request.app.state.http_client, client_id)
        if metadata.token_endpoint_auth_method != "none":
            return JSONResponse({"error": "invalid_client"}, status_code=401)
    elif client_id != config.OAUTH2_GATEWAY_CLIENT_ID:
        return JSONResponse({"error": "invalid_client"}, status_code=401)

    # Existing PKCE verification stays here.

    access_token = request.app.state.token_store.issue_access_token(
        client_id=client_id,
        resource=code_data.resource,
    )
    return JSONResponse(
        {
            "access_token": access_token.token,
            "token_type": "bearer",
            "expires_in": 86400,
        }
    )
```

- [ ] **Step 4: Keep preregistered `client_credentials` working, but issue opaque tokens**

```python
async def _handle_client_credentials(request: Request, client_id: str, client_secret: str) -> JSONResponse:
    if not config.OAUTH2_GATEWAY_CLIENT_SECRET:
        return JSONResponse({"error": "server_error"}, status_code=500)

    id_match = hmac.compare_digest(client_id, config.OAUTH2_GATEWAY_CLIENT_ID)
    secret_match = hmac.compare_digest(client_secret, config.OAUTH2_GATEWAY_CLIENT_SECRET)
    if not (id_match and secret_match):
        return JSONResponse({"error": "invalid_client"}, status_code=401)

    resource = str(request.base_url).rstrip("/") + "/mcp"
    access_token = request.app.state.token_store.issue_access_token(
        client_id=client_id,
        resource=resource,
    )
    return JSONResponse(
        {
            "access_token": access_token.token,
            "token_type": "bearer",
            "expires_in": 86400,
        }
    )
```

- [ ] **Step 5: Register the OpenID discovery route**

```python
oauth_routes = [
    Route("/.well-known/oauth-authorization-server", oauth_metadata, methods=["GET"]),
    Route("/.well-known/openid-configuration", openid_metadata, methods=["GET"]),
    Route("/oauth/authorize", oauth_authorize, methods=["GET"]),
    Route("/oauth/token", oauth_token, methods=["POST"]),
    Route("/oauth/register", oauth_register, methods=["POST"]),
]
```

- [ ] **Step 6: Run all OAuth-focused tests**

Run: `uv run python -m unittest tests.test_oauth_metadata tests.test_oauth_cimd -v`

Expected: PASS for metadata discovery, OpenID aliasing, CIMD public-client authorize/token flow, and preregistered compatibility.

### Task 5: Update the Runtime Contract and Remove the Static Access Token Assumption

**Files:**
- Modify: `poc/oauth2-host-gw/.env.template`
- Modify: `poc/oauth2-host-gw/README.md`

- [ ] **Step 1: Remove the static access token from the primary docs contract**

```dotenv
# Optional: preregistered client ID used for manual testing and fallback flows
OAUTH2_GATEWAY_CLIENT_ID=oauth2-gateway-client

# Optional: preregistered client secret used for manual testing and fallback flows
OAUTH2_GATEWAY_CLIENT_SECRET=

# Optional: upstream MCP server base URL (default: http://127.0.0.1:8420)
OAUTH2_GATEWAY_MCP_UPSTREAM_URL=http://127.0.0.1:8420
```

- [ ] **Step 2: Document the supported registration modes**

```markdown
This gateway supports three client registration modes:

- CIMD for ChatGPT-style public clients. The client uses an HTTPS URL as `client_id`, and the gateway fetches the metadata document and validates `redirect_uri`.
- Preregistered clients for manual testing or clients that already have a configured `client_id` and `client_secret`.
- Dynamic client registration at `/oauth/register` as a compatibility fallback.

Discovery endpoints:

- `/.well-known/oauth-protected-resource/mcp`
- `/.well-known/oauth-authorization-server`
- `/.well-known/openid-configuration`

For ChatGPT-compatible CIMD flows, the token endpoint supports public-client token exchange with `token_endpoint_auth_method=none` and PKCE.
```

- [ ] **Step 3: Add local verification commands for both discovery and CIMD**

```bash
cd /Users/tleyden/Development/agentzoo/poc/oauth2-host-gw
uv run python -m unittest discover -s tests -v
```

```bash
curl -s http://127.0.0.1:8421/.well-known/oauth-authorization-server | jq
curl -s http://127.0.0.1:8421/.well-known/openid-configuration | jq
curl -s http://127.0.0.1:8421/.well-known/oauth-protected-resource/mcp | jq
```

Expected:
- auth metadata includes `client_id_metadata_document_supported: true`
- auth metadata includes `token_endpoint_auth_methods_supported` with `none`
- protected resource metadata points to `/mcp`

- [ ] **Step 4: Add a live hostname probe checklist**

```bash
curl -i -sS https://agentbrain.mcpnative.dev/.well-known/oauth-authorization-server
curl -i -sS https://agentbrain.mcpnative.dev/.well-known/openid-configuration
curl -i -sS https://agentbrain.mcpnative.dev/.well-known/oauth-protected-resource/mcp
curl -i -sS https://agentbrain.mcpnative.dev/mcp
```

Expected:
- all three `.well-known` endpoints return `200`
- `/mcp` without a bearer token returns `401`
- the `/mcp` `WWW-Authenticate` header includes `resource_metadata="https://agentbrain.mcpnative.dev/.well-known/oauth-protected-resource/mcp"`

### Task 6: Full Validation Against the Local Gateway

**Files:**
- Test only: `poc/oauth2-host-gw`

- [ ] **Step 1: Run the full test suite**

Run: `uv run python -m unittest discover -s tests -v`

Expected: PASS

- [ ] **Step 2: Verify preregistered fallback still works**

Run:

```bash
cd /Users/tleyden/Development/agentzoo/poc/oauth2-host-gw
OAUTH2_GATEWAY_CLIENT_SECRET=dev-secret uv run oauth2-gateway
```

In another shell:

```bash
curl -s -X POST http://127.0.0.1:8421/oauth/token \
  -d "grant_type=client_credentials" \
  -d "client_id=oauth2-gateway-client" \
  -d "client_secret=dev-secret" | jq
```

Expected: JSON with an opaque `access_token`

- [ ] **Step 3: Verify the issued token can call `/mcp`**

Run:

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

Expected: `200` with a JSON-RPC `tools/list` response from the upstream MCP server

- [ ] **Step 4: Verify the unauthorized discovery path remains intact**

Run:

```bash
curl -i -X POST http://127.0.0.1:8421/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'
```

Expected: `401 Unauthorized` and `WWW-Authenticate` with `resource_metadata=...`

## Phase 2 Follow-On: Refresh Tokens and `offline_access`

Do not advertise `offline_access` until these steps are implemented.

### Phase 2 Scope

- Add `refresh_token` issuance for `authorization_code` grants.
- Add refresh token rotation for public clients.
- Add `grant_types_supported: ["authorization_code", "client_credentials", "refresh_token"]`.
- Add `scopes_supported` including `offline_access` to auth metadata only after refresh token issuance exists.
- Persist refresh token state in a store separate from authorization codes.
- Decide whether access/refresh token persistence can remain in memory for this POC or needs a local database before public rollout.

### Phase 2 Validation

- `/.well-known/oauth-authorization-server` includes `offline_access` in `scopes_supported`
- ChatGPT app recreation picks up the updated metadata
- refreshed access tokens remain bound to the same canonical `/mcp` resource

## Self-Review

- Spec coverage: this plan adds the previously missing OpenAI/MCP requirements called out in review: CIMD discovery support, URL-form `client_id` handling, token auth method `none`, `resource` propagation, and continued `WWW-Authenticate` discovery on `/mcp`.
- Placeholder scan: there are no `TODO` or `TBD` markers in the plan body.
- Type consistency: the plan consistently uses `client_id_metadata_document_supported`, `resource`, `token_endpoint_auth_methods_supported`, `redirect_uris`, `AuthorizationCode`, `AccessToken`, and `TokenStore`.
