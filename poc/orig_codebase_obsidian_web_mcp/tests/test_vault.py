"""Tests for vault.py -- path resolution, file operations, and safety checks."""

import pytest
from pathlib import Path

from obsidian_vault_mcp.vault import (
    resolve_vault_path,
    read_file,
    write_file_atomic,
    delete_path,
    move_path,
    list_directory,
)


def test_resolve_valid_path(vault_dir):
    """Normal relative path resolves correctly."""
    result = resolve_vault_path("test-note.md")
    assert result.exists()
    assert result.name == "test-note.md"


def test_resolve_dotdot_rejected(vault_dir):
    """Path with .. that escapes vault is rejected."""
    with pytest.raises(ValueError):
        resolve_vault_path("../../etc/passwd")


def test_resolve_dotfile_rejected(vault_dir):
    """Path starting with .obsidian is rejected."""
    with pytest.raises(ValueError, match="hidden"):
        resolve_vault_path(".obsidian/config.json")


def test_resolve_null_byte_rejected(vault_dir):
    """Path with null byte is rejected."""
    with pytest.raises(ValueError, match="null"):
        resolve_vault_path("test\x00note.md")


def test_read_file(vault_dir):
    """Read a file, verify content and metadata."""
    content, metadata = read_file("test-note.md")
    assert "test note" in content
    assert "size" in metadata
    assert "modified" in metadata
    assert "created" in metadata
    assert metadata["size"] > 0


def test_read_missing_file(vault_dir):
    """Reading a nonexistent file raises FileNotFoundError."""
    with pytest.raises(FileNotFoundError):
        read_file("nonexistent.md")


def test_write_atomic_new_file(vault_dir):
    """Write a new file and verify it exists."""
    is_new, size = write_file_atomic("new-file.md", "# Hello\n\nNew content.")
    assert is_new is True
    assert size > 0
    assert (vault_dir / "new-file.md").exists()
    assert (vault_dir / "new-file.md").read_text() == "# Hello\n\nNew content."


def test_write_atomic_overwrite(vault_dir):
    """Overwrite an existing file."""
    is_new, _ = write_file_atomic("test-note.md", "Overwritten content.")
    assert is_new is False
    assert (vault_dir / "test-note.md").read_text() == "Overwritten content."


def test_write_atomic_creates_dirs(vault_dir):
    """Write to a nonexistent directory with create_dirs=True."""
    is_new, _ = write_file_atomic("new-dir/deep/file.md", "Content", create_dirs=True)
    assert is_new is True
    assert (vault_dir / "new-dir" / "deep" / "file.md").exists()


def test_write_respects_size_limit(vault_dir):
    """Content exceeding MAX_CONTENT_SIZE is rejected."""
    from obsidian_vault_mcp.config import MAX_CONTENT_SIZE
    big_content = "x" * (MAX_CONTENT_SIZE + 1)
    with pytest.raises(ValueError, match="size"):
        write_file_atomic("big-file.md", big_content)


def test_delete_moves_to_trash(vault_dir):
    """Delete moves file to .trash/, not hard delete."""
    write_file_atomic("to-delete.md", "Delete me.")
    assert (vault_dir / "to-delete.md").exists()

    deleted = delete_path("to-delete.md")
    assert deleted is True
    assert not (vault_dir / "to-delete.md").exists()
    assert (vault_dir / ".trash" / "to-delete.md").exists()


def test_list_excludes_dotdirs(vault_dir):
    """Listing excludes .obsidian directory."""
    items = list_directory("", depth=1, include_files=True, include_dirs=True, pattern=None)
    names = [item["name"] for item in items]
    assert ".obsidian" not in names
    assert ".trash" not in names
    assert "test-note.md" in names


def test_move_file(vault_dir):
    """Move a file and verify old path is gone, new path exists."""
    write_file_atomic("source.md", "Move me.")
    moved = move_path("source.md", "destination.md")
    assert moved is True
    assert not (vault_dir / "source.md").exists()
    assert (vault_dir / "destination.md").exists()
    assert (vault_dir / "destination.md").read_text() == "Move me."
