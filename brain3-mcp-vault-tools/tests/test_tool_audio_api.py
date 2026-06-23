import importlib
import json
import os
import re
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

from starlette.testclient import TestClient

MODULE_PREFIXES = (
    "brain3_mcp_vault_tools.server",
    "brain3_mcp_vault_tools.config",
    "brain3_mcp_vault_tools.tools.audio",
)


def import_server_module():
    for module_name in tuple(sys.modules):
        if module_name in MODULE_PREFIXES:
            sys.modules.pop(module_name, None)
    return importlib.import_module("brain3_mcp_vault_tools.server")


class ToolAudioApiTests(unittest.TestCase):
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

    def tearDown(self):
        self.env_patcher.stop()
        self.temp_dir.cleanup()

    def test_tools_list_declares_openai_file_param_metadata(self):
        app = self.server.mcp.streamable_http_app()
        with TestClient(app, base_url="http://127.0.0.1:8420") as client:
            response = client.post(
                "/mcp",
                headers={
                    "accept": "application/json, text/event-stream",
                    "content-type": "application/json",
                    "x-brain3-upstream-secret": "shared-secret",
                },
                json={"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}},
            )

        self.assertEqual(response.status_code, 200)
        tools = response.json()["result"]["tools"]
        save_audio_tool = next(tool for tool in tools if tool["name"] == "save_audio_file")

        self.assertEqual(
            save_audio_tool["_meta"]["openai/fileParams"], ["audio_file"]
        )
        self.assertEqual(
            save_audio_tool["inputSchema"]["required"],
            ["audio_file"],
        )
        self.assertIn("audio_file", save_audio_tool["inputSchema"]["properties"])
        self.assertNotIn("audio_data", save_audio_tool["inputSchema"]["properties"])
        file_ref_schema = save_audio_tool["inputSchema"]["$defs"][
            "OpenAIFileReferenceInput"
        ]
        self.assertEqual(
            file_ref_schema["required"],
            ["download_url", "file_id"],
        )

    def test_save_audio_file_downloads_file_reference_and_writes_temp_file(self):
        output_dir = Path(self.temp_dir.name) / "audio-output"
        output_dir.mkdir()
        response = MagicMock()
        response.__enter__.return_value = response
        response.__exit__.return_value = False
        response.headers = {"Content-Length": "8"}
        response.read.side_effect = [b"OggSdata", b""]

        with (
            patch(
                "brain3_mcp_vault_tools.tools.audio.tempfile.mkdtemp",
                return_value=str(output_dir),
            ),
            patch("urllib.request.urlopen", return_value=response),
        ):
            result = self.server.save_audio_file(
                {
                    "download_url": "https://files.example.test/audio",
                    "file_id": "file_123",
                    "mime_type": "audio/ogg",
                    "file_name": "whatsapp-audio.opus",
                }
            )

        match = re.search(r"path:\s+(?P<path>.+)\n", result)
        self.assertIsNotNone(match)
        saved_path = Path(match.group("path").strip())
        self.assertEqual(saved_path.read_bytes(), b"OggSdata")
        self.assertEqual(saved_path.suffix, ".opus")
        self.assertEqual(saved_path.parent, output_dir)
        self.assertIn("size:       8 bytes", result)


if __name__ == "__main__":
    unittest.main()
