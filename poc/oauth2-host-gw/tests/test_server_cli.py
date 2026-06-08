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
