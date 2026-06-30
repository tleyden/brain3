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
    "brain3_mcp_vault_tools.frontmatter_index",
    "brain3_mcp_vault_tools.tools.read",
    "brain3_mcp_vault_tools.tools.manage",
    "brain3_mcp_vault_tools.tools.search",
)


def import_server_module():
    for module_name in tuple(sys.modules):
        if any(
            module_name == p or module_name.startswith(p + ".")
            for p in MODULE_PREFIXES
        ):
            sys.modules.pop(module_name, None)
    return importlib.import_module("brain3_mcp_vault_tools.server")


class CaseInsensitiveSearchTests(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.vault = Path(self.temp_dir.name).resolve()
        self._write_fixture_files()
        self.env_patcher = patch.dict(
            os.environ,
            {
                "B3_VAULT_PATH": str(self.vault),
                "B3_VAULT_MCP_PORT": "2765",
            },
            clear=False,
        )
        self.env_patcher.start()
        self.server = import_server_module()
        self.server.frontmatter_index.start()

    def tearDown(self):
        self.server.frontmatter_index.stop()
        self.env_patcher.stop()
        self.temp_dir.cleanup()

    def _write_fixture_files(self):
        (self.vault / "plan").mkdir()
        (self.vault / "Planning").mkdir()

        (self.vault / "plan" / "roadmap.md").write_text(
            "---\nStatus: Draft\ntitle: Lower plan\n---\n\nShared Search Needle.\n",
            encoding="utf-8",
        )
        (self.vault / "Planning" / "strategy.md").write_text(
            "---\nstatus: draft\ntitle: Upper plan\n---\n\nShared Search Needle.\n",
            encoding="utf-8",
        )
        (self.vault / "Planning" / "dual.md").write_text(
            "---\nStatus: Draft\nstatus: Archived\ntitle: Dual status\n---\n\nOther text.\n",
            encoding="utf-8",
        )
        (self.vault / "mixedcase.MD").write_text(
            "Mixed extension needle.\n", encoding="utf-8"
        )
        (self.vault / "lowercase.md").write_text(
            "Lower extension needle.\n", encoding="utf-8"
        )
        (self.vault / "upperfront.MD").write_text(
            "---\nstatus: published\ntitle: Upper extension frontmatter\n---\n\nBody.\n",
            encoding="utf-8",
        )

    def _search_paths(self, **kwargs) -> list[str]:
        result = json.loads(self.server.vault_search(**kwargs))
        self.assertNotIn("error", result)
        return sorted(item["path"] for item in result["results"])

    def _frontmatter_paths(self, **kwargs) -> list[str]:
        result = json.loads(self.server.vault_search_frontmatter(**kwargs))
        self.assertNotIn("error", result)
        return sorted(item["path"] for item in result["results"])

    def test_vault_search_file_pattern_is_case_insensitive(self):
        md_paths = self._search_paths(query="Mixed extension", file_pattern="*.md")
        upper_paths = self._search_paths(query="Lower extension", file_pattern="*.MD")

        self.assertEqual(md_paths, ["mixedcase.MD"])
        self.assertEqual(upper_paths, ["lowercase.md"])

    def test_vault_search_path_prefix_unions_case_variants(self):
        lower_prefix = self._search_paths(query="Shared Search", path_prefix="plan")
        upper_prefix = self._search_paths(query="Shared Search", path_prefix="PLAN")

        self.assertEqual(lower_prefix, ["Planning/strategy.md", "plan/roadmap.md"])
        self.assertEqual(upper_prefix, lower_prefix)

    def test_frontmatter_search_field_exact_and_path_prefix_are_case_insensitive(self):
        paths = self._frontmatter_paths(
            field="status",
            value="draft",
            match_type="exact",
            path_prefix="PLAN",
        )

        self.assertEqual(
            paths, ["Planning/dual.md", "Planning/strategy.md", "plan/roadmap.md"]
        )

    def test_frontmatter_search_returns_file_once_when_multiple_keys_match(self):
        paths = self._frontmatter_paths(
            field="status",
            value="draft",
            match_type="contains",
            path_prefix="Plan",
        )

        self.assertEqual(paths.count("Planning/dual.md"), 1)
        self.assertIn("Planning/dual.md", paths)

    def test_frontmatter_search_indexes_markdown_extensions_case_insensitively(self):
        paths = self._frontmatter_paths(
            field="status",
            value="published",
            match_type="exact",
        )

        self.assertEqual(paths, ["upperfront.MD"])

    def test_vault_list_pattern_is_case_insensitive(self):
        result = json.loads(self.server.vault_list(pattern="*.MD"))

        self.assertNotIn("error", result)
        paths = sorted(item["path"] for item in result["items"])
        self.assertIn("lowercase.md", paths)
        self.assertIn("mixedcase.MD", paths)

    def test_vault_read_keeps_exact_case_path_resolution(self):
        result = json.loads(self.server.vault_read("planning/strategy.md"))

        if "error" not in result:
            self.skipTest("Filesystem resolves wrong-case paths case-insensitively")
        self.assertEqual(result["error"], "File not found: planning/strategy.md")


if __name__ == "__main__":
    unittest.main()
