"""Read tools for the Obsidian vault MCP server."""

import hashlib
import json
import logging

import frontmatter

from ..vault import read_file

logger = logging.getLogger(__name__)


def _content_hash(content: str) -> str:
    return hashlib.sha256(content.encode("utf-8")).hexdigest()


def _slice_content(
    content: str,
    start_line: int | None = None,
    end_line: int | None = None,
    tail_lines: int | None = None,
) -> tuple[str, int, int, int]:
    lines = content.splitlines(keepends=True)
    total_lines = len(lines)

    if total_lines == 0:
        return "", 0, 0, 0

    if tail_lines is not None:
        start = max(0, total_lines - tail_lines)
        end = total_lines
    else:
        start = 0 if start_line is None else start_line - 1
        end = total_lines if end_line is None else min(end_line, total_lines)

    start = min(start, total_lines)
    end = max(start, min(end, total_lines))

    sliced_content = "".join(lines[start:end])
    returned_start_line = start + 1 if end > start else 0
    returned_end_line = end if end > start else 0
    return sliced_content, total_lines, returned_start_line, returned_end_line


def _parse_frontmatter(content: str) -> dict | None:
    try:
        post = frontmatter.loads(content)
        if post.metadata:
            return post.metadata
    except Exception:
        pass
    return None


def vault_read(
    path: str,
    start_line: int | None = None,
    end_line: int | None = None,
    tail_lines: int | None = None,
) -> str:
    """Read a file from the vault, optionally returning only a line window."""
    try:
        content, metadata = read_file(path)
        content_window, total_lines, returned_start_line, returned_end_line = _slice_content(
            content,
            start_line=start_line,
            end_line=end_line,
            tail_lines=tail_lines,
        )

        fm_data = _parse_frontmatter(content)

        return json.dumps({
            "path": path,
            "content": content_window,
            "metadata": metadata,
            "frontmatter": fm_data,
            "content_hash": _content_hash(content),
            "total_lines": total_lines,
            "returned_start_line": returned_start_line,
            "returned_end_line": returned_end_line,
            "has_trailing_newline": content.endswith("\n"),
        }, default=str)
    except ValueError as e:
        return json.dumps({"error": str(e), "path": path})
    except FileNotFoundError:
        return json.dumps({"error": f"File not found: {path}", "path": path})
    except Exception as e:
        logger.error(f"vault_read error for {path}: {e}")
        return json.dumps({"error": str(e), "path": path})


def vault_batch_read(paths: list[str], include_content: bool = True) -> str:
    """Read multiple files from the vault in one call."""
    results = []
    found = 0
    missing = 0

    for path in paths:
        try:
            content, metadata = read_file(path)

            entry = {
                "path": path,
                "metadata": metadata,
                "frontmatter": _parse_frontmatter(content),
            }
            if include_content:
                entry["content"] = content

            results.append(entry)
            found += 1
        except (ValueError, FileNotFoundError) as e:
            results.append({"path": path, "error": str(e)})
            missing += 1
        except Exception as e:
            results.append({"path": path, "error": str(e)})
            missing += 1

    return json.dumps({"files": results, "found": found, "missing": missing}, default=str)
