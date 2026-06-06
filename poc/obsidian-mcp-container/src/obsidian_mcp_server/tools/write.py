"""Write tools for the Obsidian vault MCP server."""

import json
import logging

import frontmatter

from ..vault import resolve_vault_path, read_file, write_file_atomic

logger = logging.getLogger(__name__)


def _preserve_trailing_newline(original_content: str, new_content: str) -> str:
    if original_content.endswith("\n") and not new_content.endswith("\n"):
        return new_content + "\n"
    if not original_content.endswith("\n") and new_content.endswith("\n"):
        return new_content[:-1]
    return new_content


def vault_create_overwrite_file(path: str, content: str, create_dirs: bool = True) -> str:
    """Create a new file or replace an existing file with the provided full content."""
    try:
        resolve_vault_path(path)

        is_new, size = write_file_atomic(path, content, create_dirs=create_dirs)

        return json.dumps({"path": path, "created": is_new, "size": size})
    except ValueError as e:
        return json.dumps({"error": str(e), "path": path})
    except Exception as e:
        logger.error(f"vault_create_overwrite_file error for {path}: {e}")
        return json.dumps({"error": str(e), "path": path})


def vault_batch_frontmatter_update(updates: list[dict]) -> str:
    """Update frontmatter fields on multiple files without changing body content."""
    results = []

    for update in updates:
        file_path = update.get("path", "")
        fields = update.get("fields", {})

        try:
            content, _ = read_file(file_path)
            post = frontmatter.loads(content)

            for key, value in fields.items():
                post.metadata[key] = value

            new_content = _preserve_trailing_newline(content, frontmatter.dumps(post))
            write_file_atomic(file_path, new_content, create_dirs=False)

            results.append({"path": file_path, "updated": True})
        except FileNotFoundError:
            results.append({"path": file_path, "updated": False, "error": "File not found"})
        except ValueError as e:
            results.append({"path": file_path, "updated": False, "error": str(e)})
        except Exception as e:
            results.append({"path": file_path, "updated": False, "error": str(e)})

    return json.dumps({"results": results})
