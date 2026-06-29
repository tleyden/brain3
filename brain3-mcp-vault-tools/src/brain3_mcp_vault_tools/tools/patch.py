"""Unified diff patch tool for the Obsidian vault MCP server."""

import hashlib
import json
import logging
import re
import unicodedata

_VARIATION_SELECTORS_RE = re.compile("[︀-️\U000e0100-\U000e01ef]")

from ..vault import read_file, write_file_atomic

logger = logging.getLogger(__name__)

HUNK_HEADER_RE = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")
PATCH_START_MESSAGE = (
    "Patch must start with a hunk header (@@ ...) or unified diff file headers (--- / +++)"
)


class PatchError(ValueError):
    """Structured patch application error."""

    def __init__(self, code: str, message: str, details: dict | None = None):
        super().__init__(message)
        self.code = code
        self.details = details


def _content_hash(content: str) -> str:
    return hashlib.sha256(content.encode("utf-8")).hexdigest()


def _normalize_diff_path(raw_path: str) -> str:
    path = raw_path.split("\t", 1)[0].strip()
    if path in {"/dev/null", ""}:
        raise PatchError("invalid_patch", "Create, delete, and rename patches are not supported")
    if path.startswith("a/") or path.startswith("b/"):
        path = path[2:]
    return path


def _hunk_metadata(hunk: dict) -> dict:
    return {
        "header": hunk["header"],
        "old_start": hunk["old_start"],
        "old_count": hunk["old_count"],
        "new_count": hunk["new_count"],
    }


def _parse_unified_diff(path: str, diff_text: str) -> list[dict]:
    diff_text = diff_text.replace("\r\n", "\n")
    lines = diff_text.splitlines(keepends=True)
    if not lines:
        raise PatchError("invalid_patch", PATCH_START_MESSAGE)

    index = 0
    if lines[0].startswith("--- "):
        if len(lines) < 2 or not lines[1].startswith("+++ "):
            raise PatchError(
                "invalid_patch",
                "Patch must start with unified diff file headers (--- / +++)",
            )

        old_path = _normalize_diff_path(lines[0][4:])
        new_path = _normalize_diff_path(lines[1][4:])
        if old_path != new_path:
            raise PatchError("invalid_patch", "Rename patches are not supported")
        if old_path != path:
            raise PatchError(
                "path_mismatch", f"Patch headers target '{old_path}', expected '{path}'"
            )
        index = 2
    elif not lines[0].startswith("@@ "):
        raise PatchError("invalid_patch", PATCH_START_MESSAGE)

    hunks: list[dict] = []
    parsed_hunks: list[dict] = []
    while index < len(lines):
        line = lines[index]
        if line.startswith("--- ") or line.startswith("+++ "):
            raise PatchError("invalid_patch", "Multi-file patches are not supported")
        if not line.startswith("@@ "):
            raise PatchError("invalid_patch", "Patch contains content outside a unified diff hunk")

        match = HUNK_HEADER_RE.match(line)
        if not match:
            raise PatchError("invalid_patch", f"Invalid hunk header: {line.rstrip()}")

        header = line.rstrip("\n")
        old_start = int(match.group(1))
        old_count = int(match.group(2) or "1")
        new_count = int(match.group(4) or "1")
        index += 1

        hunk_lines: list[tuple[str, str]] = []
        while index < len(lines):
            current = lines[index]
            if current.startswith("@@ "):
                break
            if current.startswith("--- ") or current.startswith("+++ "):
                raise PatchError("invalid_patch", "Multi-file patches are not supported")
            if current.startswith("\\ "):
                raise PatchError("invalid_patch", "Patches using no-newline markers are not supported")
            if not current or current[0] not in {" ", "+", "-"}:
                raise PatchError("invalid_patch", f"Invalid patch line: {current.rstrip()}")
            hunk_lines.append((current[0], current[1:]))
            index += 1

        actual_old_count = sum(1 for op, _ in hunk_lines if op in {" ", "-"})
        actual_new_count = sum(1 for op, _ in hunk_lines if op in {" ", "+"})
        if actual_old_count != old_count or actual_new_count != new_count:
            raise PatchError(
                "invalid_patch",
                "Hunk line counts do not match header counts",
                {
                    "header": header,
                    "expected_old_count": old_count,
                    "actual_old_count": actual_old_count,
                    "expected_new_count": new_count,
                    "actual_new_count": actual_new_count,
                    "parsed_hunks": parsed_hunks,
                },
            )

        hunk = {
            "header": header,
            "old_start": old_start,
            "old_count": old_count,
            "new_count": new_count,
            "lines": hunk_lines,
        }
        hunks.append(hunk)
        parsed_hunks.append(_hunk_metadata(hunk))

    if not hunks:
        raise PatchError("invalid_patch", "Patch contains no hunks")

    return hunks


def _apply_hunks(content: str, hunks: list[dict]) -> str:
    result_lines = content.splitlines(keepends=True)
    offset = 0

    for hunk in hunks:
        hunk_lines = hunk["lines"]
        old_segment = [text for op, text in hunk_lines if op in {" ", "-"}]

        start_index = max(0, hunk["old_start"] - 1 + offset)
        end_index = start_index + len(old_segment)
        actual_segment = result_lines[start_index:end_index]

        # Normalize to NFC and strip trailing \n before comparing so that
        # NFC/NFD mismatches, emoji variation-selector differences, a patch
        # whose last line lacks a newline, and a file whose last line lacks a
        # newline all match correctly.
        def _norm(s: str) -> str:
            s = unicodedata.normalize("NFC", s)
            s = _VARIATION_SELECTORS_RE.sub("", s)
            return s.rstrip("\n")

        if [_norm(s) for s in actual_segment] != [_norm(s) for s in old_segment]:
            raise PatchError("context_mismatch", "Patch context does not match current file content")

        # For context lines use the file's original content so the file's exact
        # newline characters (including trailing-newline status) are preserved.
        new_segment = []
        file_pos = start_index
        for op, text in hunk_lines:
            if op == " ":
                new_segment.append(result_lines[file_pos])
                file_pos += 1
            elif op == "-":
                file_pos += 1
            else:  # "+"
                new_segment.append(text)

        result_lines[start_index:end_index] = new_segment
        offset += len(new_segment) - len(old_segment)

    return "".join(result_lines)


def vault_apply_unified_diff(
    path: str,
    diff: str,
    dry_run: bool = False,
    expected_hash: str | None = None,
) -> str:
    """Apply a single-file unified diff patch to an existing text file."""
    try:
        content, _ = read_file(path)
        current_hash = _content_hash(content)
        if expected_hash is not None and expected_hash != current_hash:
            raise PatchError("hash_mismatch", "Expected content hash does not match current file content")

        hunks = _parse_unified_diff(path, diff)
        updated_content = _apply_hunks(content, hunks)
        updated_hash = _content_hash(updated_content)
        would_change = updated_content != content

        if dry_run:
            return json.dumps(
                {
                    "path": path,
                    "dry_run": True,
                    "applied": False,
                    "would_change": would_change,
                    "previous_content_hash": current_hash,
                    "content_hash": updated_hash,
                    "hunks": [_hunk_metadata(hunk) for hunk in hunks],
                }
            )

        if would_change:
            write_file_atomic(path, updated_content, create_dirs=False)

        return json.dumps(
            {
                "path": path,
                "dry_run": False,
                "applied": would_change,
                "would_change": would_change,
                "previous_content_hash": current_hash,
                "content_hash": updated_hash,
            }
        )
    except FileNotFoundError:
        return json.dumps({"error": f"File not found: {path}", "error_code": "file_not_found", "path": path})
    except PatchError as e:
        result = {"error": str(e), "error_code": e.code, "path": path}
        if e.details is not None:
            result["details"] = e.details
        return json.dumps(result)
    except ValueError as e:
        return json.dumps({"error": str(e), "error_code": "invalid_patch", "path": path})
    except Exception as e:
        logger.error("vault_apply_unified_diff error for %s: %s", path, e)
        return json.dumps({"error": str(e), "error_code": "internal_error", "path": path})
