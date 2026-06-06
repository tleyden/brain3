"""Regression tests for the frontmatter search tool."""

import json

import obsidian_vault_mcp.server as server

from obsidian_vault_mcp.frontmatter_index import FrontmatterIndex
from obsidian_vault_mcp.tools.search import vault_search_frontmatter


def test_vault_search_frontmatter_uses_live_index(vault_dir, monkeypatch):
    idx = FrontmatterIndex()
    idx.start()
    monkeypatch.setattr(server, "frontmatter_index", idx)

    try:
        payload = json.loads(
            vault_search_frontmatter(field="status", value="active", match_type="exact")
        )
    finally:
        idx.stop()

    assert payload["total"] >= 1
    assert any(item["path"] == "test-note.md" for item in payload["results"])

