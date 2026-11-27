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
    def test_main_binds_to_default_and_configured_addresses(self):
        cases = (
            ({}, "127.0.0.1", 2765),
            (
                {"B3_VAULT_MCP_HOST": "0.0.0.0", "B3_VAULT_MCP_PORT": "4567"},
                "0.0.0.0",
                4567,
            ),
        )
        for env, host, port in cases:
            with self.subTest(host=host, port=port), patch.dict(
                os.environ,
                {
                    "B3_VAULT_PATH": str(TEST_VAULT),
                    "B3_UPSTREAM_SHARED_SECRET": "shared-secret",
                    **env,
                },
                clear=True,
            ):
                server = import_server_module()

                with (
                    patch.object(server, "_start_process_resources"),
                    patch.object(server, "_stop_process_resources"),
                    patch.object(server.uvicorn, "run") as run_mock,
                ):
                    server.main()

                run_mock.assert_called_once()
                self.assertEqual(run_mock.call_args.kwargs, {"host": host, "port": port})

    def test_upstream_shared_secret_can_be_provided_directly_via_env_var(self):
        with patch.dict(
            os.environ,
            {
                "B3_VAULT_PATH": str(TEST_VAULT),
                "B3_UPSTREAM_SHARED_SECRET": "shared-secret",
            },
            clear=False,
        ):
            server = import_server_module()

        self.assertEqual(server._load_upstream_shared_secret(), "shared-secret")


if __name__ == "__main__":
    unittest.main()
