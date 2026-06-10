import importlib
import os
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

PROJECT_ROOT = Path(__file__).resolve().parents[1]
TEST_VAULT = PROJECT_ROOT / "test_vault"


def import_server_module():
    for module_name in (
        "brain3_mcp_vault_tools.server",
        "brain3_mcp_vault_tools.config",
    ):
        sys.modules.pop(module_name, None)
    return importlib.import_module("brain3_mcp_vault_tools.server")


class ServerStartupTests(unittest.TestCase):
    def test_fastmcp_host_defaults_to_loopback(self):
        with patch.dict(
            os.environ,
            {
                "VAULT_PATH": str(TEST_VAULT),
            },
            clear=False,
        ):
            server = import_server_module()

        self.assertEqual(server.mcp.settings.host, "127.0.0.1")

    def test_fastmcp_host_is_configured_from_vault_mcp_host(self):
        with patch.dict(
            os.environ,
            {
                "VAULT_PATH": str(TEST_VAULT),
                "VAULT_MCP_HOST": "0.0.0.0",
            },
            clear=False,
        ):
            server = import_server_module()

        self.assertEqual(server.mcp.settings.host, "0.0.0.0")

    def test_fastmcp_port_is_configured_from_vault_mcp_port(self):
        with patch.dict(
            os.environ,
            {
                "VAULT_PATH": str(TEST_VAULT),
                "VAULT_MCP_PORT": "8420",
            },
            clear=False,
        ):
            server = import_server_module()

        self.assertEqual(server.mcp.settings.port, 8420)

    def test_main_runs_streamable_http_without_port_keyword(self):
        with patch.dict(
            os.environ,
            {
                "VAULT_PATH": str(TEST_VAULT),
                "VAULT_MCP_PORT": "8420",
            },
            clear=False,
        ):
            server = import_server_module()

        with (
            patch.object(server, "_start_process_resources"),
            patch.object(server, "_stop_process_resources"),
            patch.object(server.mcp, "run") as run_mock,
        ):
            server.main()

        run_mock.assert_called_once_with(transport="streamable-http")


if __name__ == "__main__":
    unittest.main()
