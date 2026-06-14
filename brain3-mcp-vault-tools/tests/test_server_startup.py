import importlib
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import ANY, MagicMock, patch

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
                "B3_VAULT_PATH": str(TEST_VAULT),
            },
            clear=False,
        ):
            server = import_server_module()

        self.assertEqual(server.mcp.settings.host, "127.0.0.1")

    def test_fastmcp_host_is_configured_from_vault_mcp_host(self):
        with patch.dict(
            os.environ,
            {
                "B3_VAULT_PATH": str(TEST_VAULT),
                "B3_VAULT_MCP_HOST": "0.0.0.0",
            },
            clear=False,
        ):
            server = import_server_module()

        self.assertEqual(server.mcp.settings.host, "0.0.0.0")

    def test_fastmcp_port_is_configured_from_vault_mcp_port(self):
        with patch.dict(
            os.environ,
            {
                "B3_VAULT_PATH": str(TEST_VAULT),
                "B3_VAULT_MCP_PORT": "8420",
            },
            clear=False,
        ):
            server = import_server_module()

        self.assertEqual(server.mcp.settings.port, 8420)

    def test_main_runs_streamable_http_without_port_keyword(self):
        with patch.dict(
            os.environ,
            {
                "B3_VAULT_PATH": str(TEST_VAULT),
                "B3_VAULT_MCP_PORT": "8420",
            },
            clear=False,
        ):
            server = import_server_module()

        with (
            patch.object(server, "_start_process_resources"),
            patch.object(server, "_stop_process_resources"),
            patch.object(server, "_package_version", return_value="0.1.6"),
            patch.object(server.logger, "info") as info_mock,
            patch.object(server.mcp, "run") as run_mock,
        ):
            server.main()

        run_mock.assert_called_once_with(transport="streamable-http")
        info_mock.assert_any_call(
            "Starting authless MCP server version=%s on port %s", "0.1.6", ANY
        )


    def test_unix_socket_mode_binds_uvicorn_to_socket_path(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            socket_path = os.path.join(tmpdir, "mcp.sock")
            with patch.dict(
                os.environ,
                {
                    "B3_VAULT_PATH": str(TEST_VAULT),
                    "B3_VAULT_MCP_UNIX_SOCKET": socket_path,
                },
                clear=False,
            ):
                server = import_server_module()

            uvicorn_calls = []

            def fake_uvicorn_run(app, **kwargs):
                uvicorn_calls.append(kwargs)

            with (
                patch.object(server, "_start_process_resources"),
                patch.object(server, "_stop_process_resources"),
                patch.object(server, "_package_version", return_value="0.1.6"),
                patch.object(server, "_load_upstream_shared_secret", return_value="test-secret"),
                patch("uvicorn.run", side_effect=fake_uvicorn_run),
            ):
                server.main()

        self.assertEqual(len(uvicorn_calls), 1)
        self.assertEqual(uvicorn_calls[0]["uds"], socket_path)


if __name__ == "__main__":
    unittest.main()
