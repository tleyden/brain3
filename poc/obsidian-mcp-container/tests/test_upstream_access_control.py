import importlib
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from starlette.testclient import TestClient


MODULE_PREFIXES = (
    "obsidian_mcp_server.server",
    "obsidian_mcp_server.config",
)


def import_server_module():
    for module_name in tuple(sys.modules):
        if module_name in MODULE_PREFIXES:
            sys.modules.pop(module_name, None)
    return importlib.import_module("obsidian_mcp_server.server")


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
                "VAULT_PATH": str(self.vault),
                "VAULT_MCP_PORT": "8420",
                "UPSTREAM_SHARED_SECRET_FILE": str(self.secret_file),
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
            headers["x-agentzoo-upstream-secret"] = secret

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


if __name__ == "__main__":
    unittest.main()
