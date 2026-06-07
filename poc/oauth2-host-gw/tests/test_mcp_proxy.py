import warnings
import unittest
from unittest.mock import patch

import httpx
from starlette.exceptions import StarletteDeprecationWarning

warnings.filterwarnings(
    "ignore",
    message=r"Using `httpx` with `starlette\.testclient` is deprecated; install `httpx2` instead\.",
    category=StarletteDeprecationWarning,
)

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
        challenge = response.headers["www-authenticate"]
        self.assertIn(
            'resource_metadata="http://testserver/.well-known/oauth-protected-resource/mcp"',
            challenge,
        )

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

    def test_mcp_accepts_token_issued_by_client_credentials(self):
        def handler(request: httpx.Request) -> httpx.Response:
            self.assertEqual(str(request.url), "http://127.0.0.1:8420/mcp")
            return httpx.Response(
                200,
                headers={"content-type": "application/json"},
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

        with patch("oauth2_gateway.config.OAUTH2_GATEWAY_CLIENT_SECRET", "dev-secret"):
            with TestClient(app) as client:
                token_response = client.post(
                    "/oauth/token",
                    data={
                        "grant_type": "client_credentials",
                        "client_id": "oauth2-gateway-client",
                        "client_secret": "dev-secret",
                    },
                )
                access_token = token_response.json()["access_token"]
                response = client.post(
                    "/mcp",
                    headers={
                        "authorization": f"Bearer {access_token}",
                        "accept": "application/json, text/event-stream",
                        "content-type": "application/json",
                    },
                    json={"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}},
                )

        self.assertEqual(token_response.status_code, 200)
        self.assertTrue(access_token)
        self.assertEqual(response.status_code, 200)

    def test_mcp_rejects_token_bound_to_different_resource(self):
        def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(
                200,
                headers={"content-type": "application/json"},
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

        with patch("oauth2_gateway.config.OAUTH2_GATEWAY_CLIENT_SECRET", "dev-secret"):
            with TestClient(app) as client:
                token_response = client.post(
                    "/oauth/token",
                    data={
                        "grant_type": "client_credentials",
                        "client_id": "oauth2-gateway-client",
                        "client_secret": "dev-secret",
                        "resource": "http://testserver/not-mcp",
                    },
                )
                access_token = token_response.json()["access_token"]
                response = client.post(
                    "/mcp",
                    headers={
                        "authorization": f"Bearer {access_token}",
                        "accept": "application/json, text/event-stream",
                        "content-type": "application/json",
                    },
                    json={"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}},
                )

        self.assertEqual(token_response.status_code, 200)
        self.assertTrue(access_token)
        self.assertEqual(response.status_code, 401)

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
            with self.assertLogs("oauth2_gateway.mcp_proxy", level="WARNING"):
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
