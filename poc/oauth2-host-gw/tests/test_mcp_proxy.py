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
        app = create_app(expected_host=None)

        with TestClient(app) as client:
            response = client.get("/.well-known/oauth-protected-resource/mcp")

        self.assertEqual(response.status_code, 200)
        self.assertEqual(response.json()["resource"], "http://testserver/mcp")
        self.assertEqual(response.json()["authorization_servers"], ["http://testserver"])

    def test_mcp_requires_bearer_token(self):
        app = create_app(expected_host=None)

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

    def test_protected_resource_metadata_allows_quick_tunnel_without_named_host_config(self):
        app = create_app(expected_host=None)

        with TestClient(app, base_url="https://quick-tunnel.trycloudflare.com") as client:
            response = client.get("/.well-known/oauth-protected-resource/mcp")

        self.assertEqual(response.status_code, 200)
        self.assertEqual(response.json()["resource"], "https://quick-tunnel.trycloudflare.com/mcp")
        self.assertEqual(
            response.json()["authorization_servers"],
            ["https://quick-tunnel.trycloudflare.com"],
        )

    def test_mcp_proxy_allows_matching_named_tunnel_host(self):
        captured = {}

        def handler(request: httpx.Request) -> httpx.Response:
            captured["url"] = str(request.url)
            captured["authorization"] = request.headers.get("authorization")
            captured["upstream_secret"] = request.headers.get("x-agentzoo-upstream-secret")
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
            mcp_upstream_secret="shared-secret",
            expected_host="brain3-macos.mcpnative.dev",
            http_client_factory=lambda: httpx.AsyncClient(
                transport=httpx.MockTransport(handler),
                timeout=None,
                follow_redirects=False,
                trust_env=False,
            ),
        )

        with patch("oauth2_gateway.config.OAUTH2_GATEWAY_ACCESS_TOKEN", "test-token"):
            with TestClient(app, base_url="https://brain3-macos.mcpnative.dev") as client:
                response = client.post(
                    "/mcp",
                    headers={
                        "authorization": "Bearer test-token",
                        "x-agentzoo-upstream-secret": "attacker-value",
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
        self.assertEqual(captured["upstream_secret"], "shared-secret")
        self.assertEqual(captured["session"], "session-123")
        self.assertEqual(captured["protocol"], "2025-03-26")
        self.assertEqual(response.headers["mcp-session-id"], "session-123")

    def test_mcp_proxy_rejects_mismatched_named_tunnel_host_before_upstream_call(self):
        def handler(request: httpx.Request) -> httpx.Response:
            raise AssertionError("upstream should not be called for a misdirected request")

        app = create_app(
            mcp_upstream_url="http://127.0.0.1:8420",
            mcp_upstream_secret="shared-secret",
            expected_host="brain3-macos.mcpnative.dev",
            http_client_factory=lambda: httpx.AsyncClient(
                transport=httpx.MockTransport(handler),
                timeout=None,
                follow_redirects=False,
                trust_env=False,
            ),
        )

        with patch("oauth2_gateway.config.OAUTH2_GATEWAY_ACCESS_TOKEN", "test-token"):
            with TestClient(app, base_url="https://wrong-host.mcpnative.dev") as client:
                response = client.post(
                    "/mcp",
                    headers={
                        "authorization": "Bearer test-token",
                        "accept": "application/json, text/event-stream",
                        "content-type": "application/json",
                    },
                    json={"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}},
                )

        self.assertEqual(response.status_code, 421)
        self.assertEqual(response.json()["error"], "misdirected_request")

    def test_mcp_proxy_allows_matching_direct_public_origin_host(self):
        captured = {}

        def handler(request: httpx.Request) -> httpx.Response:
            captured["url"] = str(request.url)
            return httpx.Response(
                200,
                headers={"content-type": "application/json"},
                json={"jsonrpc": "2.0", "id": 1, "result": {"tools": []}},
            )

        app = create_app(
            mcp_upstream_url="http://127.0.0.1:8420",
            mcp_upstream_secret="shared-secret",
            expected_host="agentzoo.yourserver.com",
            http_client_factory=lambda: httpx.AsyncClient(
                transport=httpx.MockTransport(handler),
                timeout=None,
                follow_redirects=False,
                trust_env=False,
            ),
        )

        with patch("oauth2_gateway.config.OAUTH2_GATEWAY_ACCESS_TOKEN", "test-token"):
            with TestClient(app, base_url="https://agentzoo.yourserver.com") as client:
                response = client.post(
                    "/mcp",
                    headers={
                        "authorization": "Bearer test-token",
                        "accept": "application/json, text/event-stream",
                        "content-type": "application/json",
                    },
                    json={"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}},
                )

        self.assertEqual(response.status_code, 200)
        self.assertEqual(captured["url"], "http://127.0.0.1:8420/mcp")

    def test_mcp_proxy_rejects_mismatched_direct_public_origin_host_before_upstream_call(self):
        def handler(request: httpx.Request) -> httpx.Response:
            raise AssertionError("upstream should not be called for a misdirected request")

        app = create_app(
            mcp_upstream_url="http://127.0.0.1:8420",
            mcp_upstream_secret="shared-secret",
            expected_host="agentzoo.yourserver.com",
            http_client_factory=lambda: httpx.AsyncClient(
                transport=httpx.MockTransport(handler),
                timeout=None,
                follow_redirects=False,
                trust_env=False,
            ),
        )

        with patch("oauth2_gateway.config.OAUTH2_GATEWAY_ACCESS_TOKEN", "test-token"):
            with TestClient(app, base_url="https://wrong-host.yourserver.com") as client:
                response = client.post(
                    "/mcp",
                    headers={
                        "authorization": "Bearer test-token",
                        "accept": "application/json, text/event-stream",
                        "content-type": "application/json",
                    },
                    json={"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}},
                )

        self.assertEqual(response.status_code, 421)
        self.assertEqual(response.json()["error"], "misdirected_request")

    def test_mcp_proxy_returns_502_when_upstream_is_unreachable(self):
        def handler(request: httpx.Request) -> httpx.Response:
            raise httpx.ConnectError("dial tcp 127.0.0.1:8420: connect refused", request=request)

        app = create_app(
            mcp_upstream_url="http://127.0.0.1:8420",
            mcp_upstream_secret="shared-secret",
            expected_host=None,
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
