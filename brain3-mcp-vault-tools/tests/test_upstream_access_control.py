import importlib
import os
import socket
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from starlette.testclient import TestClient

MODULE_PREFIXES = (
    "brain3_mcp_vault_tools.server",
    "brain3_mcp_vault_tools.config",
)


def import_server_module():
    for module_name in tuple(sys.modules):
        if module_name in MODULE_PREFIXES:
            sys.modules.pop(module_name, None)
    return importlib.import_module("brain3_mcp_vault_tools.server")


class UpstreamAccessControlTests(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.vault = Path(self.temp_dir.name) / "vault"
        self.vault.mkdir()
        self.secret_file = Path(self.temp_dir.name) / "upstream-secret"
        self.secret_file.write_text("shared-secret\n", encoding="utf-8")

        self.env_patcher = patch.dict(
            os.environ,
            {
                "B3_VAULT_PATH": str(self.vault),
                "B3_VAULT_MCP_PORT": "8420",
                "B3_UPSTREAM_SHARED_SECRET_FILE": str(self.secret_file),
            },
            clear=False,
        )
        self.env_patcher.start()
        self.server = import_server_module()
        self.app = self.server.mcp.streamable_http_app()

    def tearDown(self):
        self.env_patcher.stop()
        self.temp_dir.cleanup()

    def _tools_list_request(self, client: TestClient, *, secret: str | None = None):
        headers = {
            "accept": "application/json, text/event-stream",
            "content-type": "application/json",
        }
        if secret is not None:
            headers["x-brain3-upstream-secret"] = secret

        return client.post(
            "/mcp",
            headers=headers,
            json={"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}},
        )

    def test_mcp_rejects_requests_without_shared_secret(self):
        with TestClient(self.app, base_url="http://127.0.0.1:8420") as client:
            response = self._tools_list_request(client)

        self.assertEqual(response.status_code, 401)

    def test_mcp_rejects_requests_with_wrong_shared_secret(self):
        with TestClient(self.app, base_url="http://127.0.0.1:8420") as client:
            response = self._tools_list_request(client, secret="wrong-secret")

        self.assertEqual(response.status_code, 401)

    def test_mcp_allows_requests_with_correct_shared_secret(self):
        with TestClient(self.app, base_url="http://127.0.0.1:8420") as client:
            response = self._tools_list_request(client, secret="shared-secret")

        self.assertEqual(response.status_code, 200)

    def test_mcp_allows_requests_to_detected_self_ip_when_enabled(self):
        with (
            patch.dict(
                os.environ,
                {
                    "B3_VAULT_PATH": str(self.vault),
                    "B3_VAULT_MCP_PORT": "8420",
                    "B3_UPSTREAM_SHARED_SECRET_FILE": str(self.secret_file),
                    "B3_VAULT_MCP_ALLOW_SELF_IP_HOSTS": "true",
                },
                clear=False,
            ),
            patch(
                "socket.getaddrinfo",
                return_value=[
                    (
                        socket.AF_INET,
                        socket.SOCK_STREAM,
                        6,
                        "",
                        ("172.18.0.2", 0),
                    ),
                    (
                        socket.AF_INET,
                        socket.SOCK_STREAM,
                        6,
                        "",
                        ("127.0.0.1", 0),
                    ),
                ],
            ),
        ):
            server = import_server_module()
            app = server.mcp.streamable_http_app()

        with TestClient(app, base_url="http://172.18.0.2:8420") as client:
            response = self._tools_list_request(client, secret="shared-secret")

        self.assertEqual(response.status_code, 200)


if __name__ == "__main__":
    unittest.main()
