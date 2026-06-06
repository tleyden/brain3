"""Tests for frontmatter_index.py -- indexing, searching, and merging."""

import pytest
from pathlib import Path

from obsidian_vault_mcp.frontmatter_index import FrontmatterIndex


@pytest.fixture
def index(vault_dir):
    """Create and start a frontmatter index against the test vault."""
    idx = FrontmatterIndex()
    idx.start()
    yield idx
    idx.stop()


def test_index_builds_on_startup(index, vault_dir):
    """Index has entries for all .md files (not .obsidian)."""
    assert index.file_count >= 2  # test-note.md, subfolder/nested-note.md
    # no-frontmatter.md may or may not be in index (no frontmatter to parse)


def test_start_is_idempotent(vault_dir, monkeypatch):
    """Calling start() twice should not create duplicate observers."""
    idx = FrontmatterIndex()
    starts = []

    class DummyObserver:
        def schedule(self, *args, **kwargs):
            return None

        def start(self):
            starts.append("observer-start")

        def stop(self):
            return None

        def join(self):
            return None

    monkeypatch.setattr("obsidian_vault_mcp.frontmatter_index.Observer", DummyObserver)

    idx.start()
    idx.start()

    assert starts == ["observer-start"]
    idx.stop()


def test_search_exact_match(index, vault_dir):
    """Search for field=status, value=active, match_type=exact."""
    results = index.search_by_field("status", "active", "exact")
    assert len(results) >= 1
    paths = [r["path"] for r in results]
    assert "test-note.md" in paths


def test_search_contains(index, vault_dir):
    """Search for field=client, value=Test, match_type=contains."""
    results = index.search_by_field("client", "Test", "contains")
    assert len(results) >= 1
    paths = [r["path"] for r in results]
    found = any("nested-note.md" in p for p in paths)
    assert found


def test_search_exists(index, vault_dir):
    """Search for field=client, match_type=exists."""
    results = index.search_by_field("client", "", "exists")
    assert len(results) >= 1


def test_search_with_prefix(index, vault_dir):
    """Search limited to subfolder/."""
    results = index.search_by_field("status", "draft", "exact", path_prefix="subfolder/")
    assert len(results) >= 1
    for r in results:
        assert r["path"].startswith("subfolder/")


def test_frontmatter_merge(vault_dir):
    """Existing frontmatter merged with new fields, body preserved."""
    import frontmatter
    from obsidian_vault_mcp.vault import read_file, write_file_atomic

    # Read original
    content, _ = read_file("test-note.md")
    post = frontmatter.loads(content)
    original_body = post.content

    # Merge new field
    post.metadata["new_field"] = "new_value"
    write_file_atomic("test-note.md", frontmatter.dumps(post))

    # Verify
    content2, _ = read_file("test-note.md")
    post2 = frontmatter.loads(content2)
    assert post2.metadata["status"] == "active"  # preserved
    assert post2.metadata["new_field"] == "new_value"  # added
    assert original_body.strip() in post2.content  # body preserved


def test_index_updates_after_file_modify(index, vault_dir):
    """A modified file should be reflected after flushing pending changes."""
    note = vault_dir / "test-note.md"
    note.write_text(
        "---\nstatus: archived\ntype: note\n---\n\nUpdated body.\n",
        encoding="utf-8",
    )

    with index._lock:
        index._pending_paths.add(str(note))

    index._flush_pending()

    results = index.search_by_field("status", "archived", "exact")
    assert any(item["path"] == "test-note.md" for item in results)


def test_index_updates_after_file_create(index, vault_dir):
    """A created file should be indexed after flushing pending changes."""
    note = vault_dir / "created-note.md"
    note.write_text(
        "---\nstatus: active\ntype: scratch\n---\n\nCreated after startup.\n",
        encoding="utf-8",
    )

    with index._lock:
        index._pending_paths.add(str(note))

    index._flush_pending()

    results = index.search_by_field("type", "scratch", "exact")
    assert any(item["path"] == "created-note.md" for item in results)


def test_index_updates_after_file_delete(index, vault_dir):
    """A deleted file should be removed after flushing pending changes."""
    note = vault_dir / "test-note.md"
    note.unlink()

    with index._lock:
        index._pending_paths.add(str(note))

    index._flush_pending()

    results = index.search_by_field("status", "active", "exact")
    assert all(item["path"] != "test-note.md" for item in results)
