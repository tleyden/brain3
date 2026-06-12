# Preregistered OAuth Lockdown Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lock `poc/oauth2-host-gw` down to a single preregistered confidential client by disabling DCR, rejecting every `client_id` except the configured one, and requiring the configured client secret during token exchange.

**Architecture:** Keep the existing Starlette OAuth gateway and MCP reverse proxy intact, but narrow the OAuth surface to one preregistered client. Remove all server-advertised registration paths, require an exact `client_id` match at authorization and token time, and require `client_secret_post` for the authorization-code token exchange so ChatGPT only offers the manual preregistered-client path.

**Tech Stack:** Python 3.12+, Starlette, Uvicorn, unittest, OAuth 2.0 authorization code flow, PKCE, MCP protected resource metadata

---

## Scope Notes

- This plan is intentionally narrower than the earlier CIMD work. It assumes CIMD is already reverted and must stay disabled.
- The security target is: only the single configured `OAUTH2_GATEWAY_CLIENT_ID` and `OAUTH2_GATEWAY_CLIENT_SECRET` may ever obtain tokens.
- This plan removes DCR from both discovery metadata and the actual route surface.
- This plan treats preregistered confidential-client authorization code as the only externally supported OAuth login path.
- `client_credentials` is removed from the advertised grant types and from the token handler to avoid offering extra OAuth choices. If any local smoke tests still rely on it, they must be updated to use the authorization-code flow instead.
- Residual risk that remains after this plan: the access token is still a static bearer token from env. That is a separate hardening task and is not changed here.

## File Map

- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/oauth.py`
  - Remove DCR advertisement and route, require exact preregistered client identity, and require the secret at token exchange.
- Create: `poc/oauth2-host-gw/tests/test_oauth_security.py`
  - Lock the public auth contract around no DCR, exact `client_id`, and required token secret.
- Modify: `poc/oauth2-host-gw/README.md`
  - Remove DCR language and document preregistered confidential-client-only behavior.
- Modify: `poc/oauth2-host-gw/.env.template`
  - Clarify that the configured client ID and secret are preregistered credentials, not registration outputs.

### Task 1: Lock the Security Contract with Public API Tests

**Files:**
- Create: `poc/oauth2-host-gw/tests/test_oauth_security.py`

- [ ] **Step 1: Write a failing metadata test that proves DCR is not advertised**

```python
import warnings
import unittest
from unittest.mock import patch

from starlette.exceptions import StarletteDeprecationWarning
from starlette.testclient import TestClient

from oauth2_gateway.server import create_app

warnings.filterwarnings(
    "ignore",
    message=r"Using `httpx` with `starlette\.testclient` is deprecated; install `httpx2` instead\.",
    category=StarletteDeprecationWarning,
)


class OAuthSecurityTests(unittest.TestCase):
    def test_oauth_metadata_only_advertises_preregistered_confidential_client_flow(self):
        app = create_app()

        with TestClient(app) as client:
            response = client.get("/.well-known/oauth-authorization-server")

        self.assertEqual(response.status_code, 200)
        payload = response.json()
        self.assertNotIn("registration_endpoint", payload)
        self.assertEqual(payload["grant_types_supported"], ["authorization_code"])
        self.assertEqual(payload["response_types_supported"], ["code"])
        self.assertEqual(payload["token_endpoint_auth_methods_supported"], ["client_secret_post"])
```

- [ ] **Step 2: Write a failing route-surface test that proves DCR is disabled**

```python
    def test_oauth_register_route_is_not_exposed(self):
        app = create_app()

        with TestClient(app) as client:
            response = client.post("/oauth/register", json={"client_name": "test"})

        self.assertEqual(response.status_code, 404)
```

- [ ] **Step 3: Write failing authorize tests for exact preregistered `client_id` matching**

```python
    def test_authorize_rejects_missing_client_id(self):
        app = create_app()

        with TestClient(app) as client:
            response = client.get(
                "/oauth/authorize",
                params={
                    "response_type": "code",
                    "redirect_uri": "https://chatgpt.com/connector/oauth/test",
                    "code_challenge": "challenge",
                    "code_challenge_method": "S256",
                },
                follow_redirects=False,
            )

        self.assertEqual(response.status_code, 401)
        self.assertEqual(response.json()["error"], "invalid_client")

    def test_authorize_rejects_wrong_client_id(self):
        app = create_app()

        with TestClient(app) as client:
            response = client.get(
                "/oauth/authorize",
                params={
                    "response_type": "code",
                    "client_id": "wrong-client",
                    "redirect_uri": "https://chatgpt.com/connector/oauth/test",
                    "code_challenge": "challenge",
                    "code_challenge_method": "S256",
                },
                follow_redirects=False,
            )

        self.assertEqual(response.status_code, 401)
        self.assertEqual(response.json()["error"], "invalid_client")
```

- [ ] **Step 4: Write failing token tests that require the secret for authorization-code exchange**

```python
    def test_authorization_code_exchange_requires_client_secret(self):
        app = create_app()

        with patch("oauth2_gateway.config.OAUTH2_GATEWAY_CLIENT_SECRET", "hardcoded-secret"):
            with TestClient(app) as client:
                authorize = client.get(
                    "/oauth/authorize",
                    params={
                        "response_type": "code",
                        "client_id": "brain3-oauth2-client",
                        "redirect_uri": "https://chatgpt.com/connector/oauth/test",
                        "code_challenge": "X48E9qOokqqrvdts8nOJRJN3OWDUoyWxBf7kbu9DBPE",
                        "code_challenge_method": "S256",
                    },
                    follow_redirects=False,
                )
                code = authorize.headers["location"].split("code=")[1]

                missing_secret = client.post(
                    "/oauth/token",
                    data={
                        "grant_type": "authorization_code",
                        "client_id": "brain3-oauth2-client",
                        "redirect_uri": "https://chatgpt.com/connector/oauth/test",
                        "code": code,
                        "code_verifier": "test-verifier",
                    },
                )

        self.assertEqual(missing_secret.status_code, 401)
        self.assertEqual(missing_secret.json()["error"], "invalid_client")
```

- [ ] **Step 5: Write a failing happy-path token test for the preregistered confidential client**

```python
    def test_authorization_code_exchange_succeeds_with_exact_client_id_and_secret(self):
        app = create_app()

        with patch("oauth2_gateway.config.OAUTH2_GATEWAY_CLIENT_SECRET", "hardcoded-secret"):
            with TestClient(app) as client:
                authorize = client.get(
                    "/oauth/authorize",
                    params={
                        "response_type": "code",
                        "client_id": "brain3-oauth2-client",
                        "redirect_uri": "https://chatgpt.com/connector/oauth/test",
                        "code_challenge": "X48E9qOokqqrvdts8nOJRJN3OWDUoyWxBf7kbu9DBPE",
                        "code_challenge_method": "S256",
                    },
                    follow_redirects=False,
                )
                code = authorize.headers["location"].split("code=")[1]

                token = client.post(
                    "/oauth/token",
                    data={
                        "grant_type": "authorization_code",
                        "client_id": "brain3-oauth2-client",
                        "client_secret": "hardcoded-secret",
                        "redirect_uri": "https://chatgpt.com/connector/oauth/test",
                        "code": code,
                        "code_verifier": "test-verifier",
                    },
                )

        self.assertEqual(token.status_code, 200)
        self.assertEqual(token.json()["token_type"], "bearer")
```

- [ ] **Step 6: Run the tests to verify the current gateway fails in the expected places**

Run: `uv run python -m unittest discover -s tests -v`

Expected: FAIL because metadata still advertises `registration_endpoint`, `/oauth/register` still exists, authorize currently allows a missing `client_id`, and the authorization-code token exchange does not currently require `client_secret`.

### Task 2: Narrow the OAuth Surface to One Preregistered Confidential Client

**Files:**
- Modify: `poc/oauth2-host-gw/src/oauth2_gateway/oauth.py`

- [ ] **Step 1: Remove DCR from discovery metadata**

```python
async def oauth_metadata(request: Request) -> JSONResponse:
    base_url = str(request.base_url).rstrip("/")
    return JSONResponse(
        {
            "issuer": base_url,
            "authorization_endpoint": f"{base_url}/oauth/authorize",
            "token_endpoint": f"{base_url}/oauth/token",
            "grant_types_supported": ["authorization_code"],
            "response_types_supported": ["code"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["client_secret_post"],
        }
    )
```

- [ ] **Step 2: Require an exact `client_id` during authorization**

```python
async def oauth_authorize(request: Request) -> JSONResponse | RedirectResponse:
    response_type = request.query_params.get("response_type", "")
    client_id = request.query_params.get("client_id", "")
    redirect_uri = request.query_params.get("redirect_uri", "")
    state = request.query_params.get("state", "")
    code_challenge = request.query_params.get("code_challenge", "")
    code_challenge_method = request.query_params.get("code_challenge_method", "S256")

    if response_type != "code":
        return JSONResponse({"error": "unsupported_response_type"}, status_code=400)

    if client_id != config.OAUTH2_GATEWAY_CLIENT_ID:
        return JSONResponse({"error": "invalid_client"}, status_code=401)

    if not redirect_uri:
        return JSONResponse({"error": "invalid_request", "error_description": "redirect_uri required"}, status_code=400)
```

- [ ] **Step 3: Require the exact `client_id` and exact secret during token exchange**

```python
async def _handle_authorization_code(form, client_id: str, client_secret: str) -> JSONResponse:
    code = form.get("code", "")
    redirect_uri = form.get("redirect_uri", "")
    code_verifier = form.get("code_verifier", "")

    _cleanup_codes()

    if code not in _auth_codes:
        return JSONResponse({"error": "invalid_grant", "error_description": "Invalid or expired code"}, status_code=400)

    code_data = _auth_codes.pop(code)

    if client_id != config.OAUTH2_GATEWAY_CLIENT_ID:
        return JSONResponse({"error": "invalid_client"}, status_code=401)

    if not config.OAUTH2_GATEWAY_CLIENT_SECRET:
        return JSONResponse({"error": "server_error"}, status_code=500)

    if not hmac.compare_digest(client_secret, config.OAUTH2_GATEWAY_CLIENT_SECRET):
        return JSONResponse({"error": "invalid_client"}, status_code=401)

    if redirect_uri and code_data["redirect_uri"] and redirect_uri != code_data["redirect_uri"]:
        return JSONResponse({"error": "invalid_grant", "error_description": "redirect_uri mismatch"}, status_code=400)

    if code_data["code_challenge"]:
        if not code_verifier:
            return JSONResponse({"error": "invalid_grant", "error_description": "code_verifier required"}, status_code=400)

        digest = hashlib.sha256(code_verifier.encode("ascii")).digest()
        computed_challenge = base64.urlsafe_b64encode(digest).rstrip(b"=").decode("ascii")
        if not hmac.compare_digest(computed_challenge, code_data["code_challenge"]):
            return JSONResponse({"error": "invalid_grant", "error_description": "PKCE verification failed"}, status_code=400)
```

- [ ] **Step 4: Remove `client_credentials` from the token endpoint**

```python
async def oauth_token(request: Request) -> JSONResponse:
    try:
        form = await request.form()
    except Exception:
        return JSONResponse({"error": "invalid_request"}, status_code=400)

    grant_type = form.get("grant_type", "")
    client_id = form.get("client_id", "")
    client_secret = form.get("client_secret", "")

    if grant_type == "authorization_code":
        return await _handle_authorization_code(form, client_id, client_secret)

    return JSONResponse({"error": "unsupported_grant_type"}, status_code=400)
```

- [ ] **Step 5: Remove the DCR handler and route**

```python
oauth_routes = [
    Route("/.well-known/oauth-authorization-server", oauth_metadata, methods=["GET"]),
    Route("/oauth/authorize", oauth_authorize, methods=["GET"]),
    Route("/oauth/token", oauth_token, methods=["POST"]),
]
```

- [ ] **Step 6: Run the tests again**

Run: `uv run python -m unittest discover -s tests -v`

Expected: PASS for the new security tests plus the existing proxy and CLI tests.

### Task 3: Update the Docs and Config Contract

**Files:**
- Modify: `poc/oauth2-host-gw/README.md`
- Modify: `poc/oauth2-host-gw/.env.template`

- [ ] **Step 1: Rewrite the README scope to remove DCR language**

```markdown
It keeps only:
- OAuth metadata discovery
- preregistered confidential-client authorization-code handling
- token exchange with PKCE support
- a tiny CLI HTTP runner
- optional helper scripts for Cloudflare Tunnel exposure
```

- [ ] **Step 2: Document the security boundary explicitly**

```markdown
Security model:

- Only the single preregistered client configured by `OAUTH2_GATEWAY_CLIENT_ID` and `OAUTH2_GATEWAY_CLIENT_SECRET` may obtain tokens.
- Dynamic client registration is disabled.
- CIMD and other public-client registration methods are disabled.
- The token endpoint requires `client_secret_post` for the authorization-code exchange.
```

- [ ] **Step 3: Clarify the env var descriptions**

```dotenv
# Required: preregistered OAuth client ID accepted by /oauth/authorize and /oauth/token
OAUTH2_GATEWAY_CLIENT_ID=brain3-oauth2-client

# Required: preregistered OAuth client secret required by /oauth/token
OAUTH2_GATEWAY_CLIENT_SECRET=
```

- [ ] **Step 4: Add a manual verification section**

```bash
curl -s http://127.0.0.1:8421/.well-known/oauth-authorization-server
```

Expected:
- no `registration_endpoint`
- `grant_types_supported` is exactly `["authorization_code"]`
- `token_endpoint_auth_methods_supported` is exactly `["client_secret_post"]`

```bash
curl -i -X POST http://127.0.0.1:8421/oauth/register
```

Expected: `404 Not Found`

### Task 4: Final Verification

**Files:**
- Test only: `poc/oauth2-host-gw`

- [ ] **Step 1: Run the full unit-test suite**

Run: `uv run python -m unittest discover -s tests -v`

Expected: PASS

- [ ] **Step 2: Verify discovery metadata manually**

Run: `curl -s http://127.0.0.1:8421/.well-known/oauth-authorization-server`

Expected JSON shape:

```json
{
  "issuer": "http://127.0.0.1:8421",
  "authorization_endpoint": "http://127.0.0.1:8421/oauth/authorize",
  "token_endpoint": "http://127.0.0.1:8421/oauth/token",
  "grant_types_supported": ["authorization_code"],
  "response_types_supported": ["code"],
  "code_challenge_methods_supported": ["S256"],
  "token_endpoint_auth_methods_supported": ["client_secret_post"]
}
```

- [ ] **Step 3: Verify `/oauth/register` is gone**

Run: `curl -s -o /dev/null -w "%{http_code}\n" -X POST http://127.0.0.1:8421/oauth/register`

Expected: `404`

- [ ] **Step 4: Verify missing or wrong client credentials are rejected**

Run:

```bash
curl -s -o /dev/null -w "%{http_code}\n" -X POST http://127.0.0.1:8421/oauth/token \
  -d "grant_type=authorization_code" \
  -d "client_id=wrong-client" \
  -d "client_secret=wrong-secret"
```

Expected: `401`

## Self-Review

- Spec coverage: this plan disables DCR in both metadata and routing, locks authorization and token exchange to the single preregistered client ID, and requires the configured secret during token exchange.
- Placeholder scan: there are no `TODO` or `TBD` markers in the plan body.
- Type consistency: the plan consistently uses `authorization_code`, `client_secret_post`, `registration_endpoint`, `OAUTH2_GATEWAY_CLIENT_ID`, and `OAUTH2_GATEWAY_CLIENT_SECRET`.
