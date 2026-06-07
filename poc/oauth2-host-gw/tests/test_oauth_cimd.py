import warnings
import unittest

import httpx
from starlette.exceptions import StarletteDeprecationWarning
from starlette.testclient import TestClient

from oauth2_gateway.server import create_app

warnings.filterwarnings(
    "ignore",
    message=r"Using `httpx` with `starlette\.testclient` is deprecated; install `httpx2` instead\.",
    category=StarletteDeprecationWarning,
)

CLIENT_ID = "https://chatgpt.example/client-metadata.json"
REDIRECT_URI = "https://chat.openai.com/aip/callback/example"
RESOURCE = "http://testserver/mcp"


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
                    "resource": RESOURCE,
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
                    "code_challenge": "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
                    "code_challenge_method": "S256",
                    "resource": RESOURCE,
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
                    "resource": RESOURCE,
                },
            )

        self.assertEqual(token.status_code, 200)
        self.assertIn("access_token", token.json())
        self.assertEqual(token.json()["token_type"], "bearer")
