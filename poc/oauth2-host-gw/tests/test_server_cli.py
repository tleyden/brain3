import os
import sys
import unittest
from unittest.mock import patch

from oauth2_gateway import server


class ServerCliTests(unittest.TestCase):
    def test_main_binds_to_localhost_by_default(self):
        with patch.object(sys, "argv", ["oauth2-gateway"]):
            with (
                patch("oauth2_gateway.server._read_required_upstream_secret", return_value="shared-secret"),
                patch("oauth2_gateway.server.uvicorn.run") as mock_run,
            ):
                server.main()

        self.assertEqual(mock_run.call_args.kwargs["host"], "127.0.0.1")

    def test_main_allows_host_override(self):
        with patch.object(sys, "argv", ["oauth2-gateway", "--host", "0.0.0.0"]):
            with (
                patch("oauth2_gateway.server._read_required_upstream_secret", return_value="shared-secret"),
                patch("oauth2_gateway.server.uvicorn.run") as mock_run,
            ):
                server.main()

        self.assertEqual(mock_run.call_args.kwargs["host"], "0.0.0.0")

    def test_main_uses_direct_public_origin_hostname_when_configured(self):
        with patch.object(sys, "argv", ["oauth2-gateway"]):
            with patch.dict(
                os.environ,
                {"DIRECT_PUBLIC_ORIGIN_HOSTNAME": "agentzoo.yourserver.com"},
                clear=True,
            ):
                with (
                    patch("oauth2_gateway.server._read_required_upstream_secret", return_value="shared-secret"),
                    patch("oauth2_gateway.server.create_app", return_value=object()) as mock_create_app,
                    patch("oauth2_gateway.server.uvicorn.run"),
                ):
                    server.main()

        self.assertEqual(mock_create_app.call_args.kwargs["expected_host"], "agentzoo.yourserver.com")

    def test_main_disables_hostname_validation_when_configured(self):
        with patch.object(sys, "argv", ["oauth2-gateway"]):
            with patch.dict(
                os.environ,
                {"OAUTH2_GATEWAY_ENFORCE_HOSTNAME_CHECK": "false"},
                clear=True,
            ):
                with (
                    patch("oauth2_gateway.server._read_required_upstream_secret", return_value="shared-secret"),
                    patch("oauth2_gateway.server.create_app", return_value=object()) as mock_create_app,
                    patch("oauth2_gateway.server.uvicorn.run"),
                ):
                    server.main()

        self.assertIs(mock_create_app.call_args.kwargs["enforce_host_validation"], False)

    def test_main_rejects_conflicting_public_hostname_configuration(self):
        with patch.object(sys, "argv", ["oauth2-gateway"]):
            with patch.dict(
                os.environ,
                {
                    "CF_TUNNEL_NAME": "brain3-macos",
                    "CF_DOMAIN": "mcpnative.dev",
                    "DIRECT_PUBLIC_ORIGIN_HOSTNAME": "agentzoo.yourserver.com",
                },
                clear=True,
            ):
                with (
                    patch("oauth2_gateway.server._read_required_upstream_secret", return_value="shared-secret"),
                    patch("oauth2_gateway.server.uvicorn.run") as mock_run,
                ):
                    with self.assertRaisesRegex(
                        RuntimeError,
                        "DIRECT_PUBLIC_ORIGIN_HOSTNAME",
                    ):
                        server.main()

        mock_run.assert_not_called()
