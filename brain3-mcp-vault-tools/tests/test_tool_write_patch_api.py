import difflib
import hashlib
import importlib
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

MODULE_PREFIXES = (
    "brain3_mcp_vault_tools.server",
    "brain3_mcp_vault_tools.config",
    "brain3_mcp_vault_tools.vault",
    "brain3_mcp_vault_tools.tools.read",
    "brain3_mcp_vault_tools.tools.write",
    "brain3_mcp_vault_tools.tools.patch",
)


def import_server_module():
    for module_name in tuple(sys.modules):
        if module_name in MODULE_PREFIXES:
            sys.modules.pop(module_name, None)
    return importlib.import_module("brain3_mcp_vault_tools.server")


def sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


class ToolWritePatchApiTests(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.vault = Path(self.temp_dir.name)
        self._write_fixture_files()
        self.env_patcher = patch.dict(
            os.environ,
            {
                "VAULT_PATH": str(self.vault),
                "VAULT_MCP_PORT": "8420",
            },
            clear=False,
        )
        self.env_patcher.start()
        self.server = import_server_module()

    def tearDown(self):
        self.env_patcher.stop()
        self.temp_dir.cleanup()

    def _write_fixture_files(self):
        (self.vault / "test-note.md").write_text(
            "---\nstatus: active\ntype: note\n---\n\nLine one.\nLine two.\n",
            encoding="utf-8",
        )

        large_lines = "".join(f"Line {index}\n" for index in range(1, 101))
        (self.vault / "large-note.md").write_text(large_lines, encoding="utf-8")

    def _unified_diff(self, path: str, updated_content: str) -> str:
        original_content = (self.vault / path).read_text(encoding="utf-8")
        original_lines = original_content.splitlines(keepends=True)
        updated_lines = updated_content.splitlines(keepends=True)
        return "".join(
            difflib.unified_diff(
                original_lines,
                updated_lines,
                fromfile=path,
                tofile=path,
                n=3,
            )
        )

    def test_create_overwrite_file_creates_new_file(self):
        result = json.loads(
            self.server.vault_create_overwrite_file("new-note.md", "# Title\n")
        )

        self.assertNotIn("error", result)
        self.assertTrue(result["created"])
        self.assertEqual(
            (self.vault / "new-note.md").read_text(encoding="utf-8"), "# Title\n"
        )

    def test_create_overwrite_file_replaces_existing_file(self):
        result = json.loads(
            self.server.vault_create_overwrite_file(
                "test-note.md", "# Replaced\nBody replaced.\n"
            )
        )

        self.assertNotIn("error", result)
        self.assertFalse(result["created"])
        self.assertEqual(
            (self.vault / "test-note.md").read_text(encoding="utf-8"),
            "# Replaced\nBody replaced.\n",
        )

    def test_vault_read_returns_middle_window_and_full_file_hash(self):
        full_content = (self.vault / "large-note.md").read_text(encoding="utf-8")

        result = json.loads(
            self.server.vault_read("large-note.md", start_line=40, end_line=42)
        )

        self.assertEqual(result["content"], "Line 40\nLine 41\nLine 42\n")
        self.assertEqual(result["returned_start_line"], 40)
        self.assertEqual(result["returned_end_line"], 42)
        self.assertEqual(result["total_lines"], 100)
        self.assertEqual(result["content_hash"], sha256_text(full_content))
        self.assertTrue(result["has_trailing_newline"])

    def test_vault_read_returns_tail_window_and_full_file_hash(self):
        full_content = (self.vault / "large-note.md").read_text(encoding="utf-8")

        result = json.loads(self.server.vault_read("large-note.md", tail_lines=2))

        self.assertEqual(result["content"], "Line 99\nLine 100\n")
        self.assertEqual(result["returned_start_line"], 99)
        self.assertEqual(result["returned_end_line"], 100)
        self.assertEqual(result["total_lines"], 100)
        self.assertEqual(result["content_hash"], sha256_text(full_content))

    def test_apply_unified_diff_dry_run_reports_change_without_writing(self):
        updated_content = (
            (self.vault / "large-note.md")
            .read_text(encoding="utf-8")
            .replace(
                "Line 50\n",
                "Updated line 50\n",
            )
        )
        diff_text = self._unified_diff("large-note.md", updated_content)
        original_content = (self.vault / "large-note.md").read_text(encoding="utf-8")

        result = json.loads(
            self.server.vault_apply_unified_diff(
                "large-note.md", diff_text, dry_run=True
            )
        )

        self.assertNotIn("error", result)
        self.assertTrue(result["dry_run"])
        self.assertTrue(result["would_change"])
        self.assertFalse(result["applied"])
        self.assertEqual(
            (self.vault / "large-note.md").read_text(encoding="utf-8"), original_content
        )

    def test_apply_unified_diff_changes_one_middle_line(self):
        original_content = (self.vault / "large-note.md").read_text(encoding="utf-8")
        updated_content = original_content.replace("Line 50\n", "Updated line 50\n")
        diff_text = self._unified_diff("large-note.md", updated_content)
        read_result = json.loads(
            self.server.vault_read("large-note.md", start_line=48, end_line=52)
        )

        result = json.loads(
            self.server.vault_apply_unified_diff(
                "large-note.md",
                diff_text,
                expected_hash=read_result["content_hash"],
            )
        )

        self.assertNotIn("error", result)
        self.assertTrue(result["applied"])
        self.assertEqual(
            (self.vault / "large-note.md").read_text(encoding="utf-8"), updated_content
        )

    def test_apply_unified_diff_appends_lines_at_end_of_file(self):
        original_content = (self.vault / "large-note.md").read_text(encoding="utf-8")
        updated_content = original_content + "Append A\nAppend B\n"
        diff_text = self._unified_diff("large-note.md", updated_content)

        result = json.loads(
            self.server.vault_apply_unified_diff("large-note.md", diff_text)
        )

        self.assertNotIn("error", result)
        self.assertTrue(result["applied"])
        self.assertEqual(
            (self.vault / "large-note.md").read_text(encoding="utf-8"), updated_content
        )

    def test_apply_unified_diff_rejects_stale_expected_hash(self):
        original_content = (self.vault / "large-note.md").read_text(encoding="utf-8")
        read_result = json.loads(self.server.vault_read("large-note.md"))

        (self.vault / "large-note.md").write_text(
            original_content + "External change.\n", encoding="utf-8"
        )

        updated_content = original_content.replace("Line 20\n", "Updated line 20\n")
        diff_text = "".join(
            difflib.unified_diff(
                original_content.splitlines(keepends=True),
                updated_content.splitlines(keepends=True),
                fromfile="large-note.md",
                tofile="large-note.md",
                n=3,
            )
        )

        result = json.loads(
            self.server.vault_apply_unified_diff(
                "large-note.md",
                diff_text,
                expected_hash=read_result["content_hash"],
            )
        )

        self.assertEqual(result["error_code"], "hash_mismatch")

    def test_apply_unified_diff_rejects_multi_file_diffs(self):
        diff_text = (
            "--- large-note.md\n"
            "+++ large-note.md\n"
            "@@ -1 +1 @@\n"
            "-a\n"
            "+b\n"
            "--- b.md\n"
            "+++ b.md\n"
            "@@ -1 +1 @@\n"
            "-c\n"
            "+d\n"
        )

        result = json.loads(
            self.server.vault_apply_unified_diff("large-note.md", diff_text)
        )

        self.assertEqual(result["error_code"], "invalid_patch")

    def test_batch_frontmatter_update_preserves_body_content(self):
        result = json.loads(
            self.server.vault_batch_frontmatter_update(
                [{"path": "test-note.md", "fields": {"priority": "high"}}]
            )
        )
        read_result = json.loads(self.server.vault_read("test-note.md"))

        self.assertEqual(result["results"][0]["updated"], True)
        self.assertEqual(read_result["frontmatter"]["priority"], "high")
        self.assertIn("Line one.\nLine two.\n", read_result["content"])


if __name__ == "__main__":
    unittest.main()
