"""Search tools for the Obsidian vault MCP server."""

import fnmatch
import json
import logging
import shlex
import shutil
import subprocess
from pathlib import Path

import frontmatter

from .. import config

logger = logging.getLogger(__name__)


def _build_ripgrep_command(
    query: str,
    search_paths: list[Path],
    file_pattern: str,
    max_results: int,
    context_lines: int,
) -> list[str]:
    """Build the ripgrep command used for full-text search."""
    cmd = [
        "rg",
        "--json",
        f"--max-count={max_results}",
        f"--iglob={file_pattern}",
        "-i",
        f"--context={context_lines}",
        query,
    ]

    for excluded in config.EXCLUDED_DIRS:
        cmd.insert(-1, f"--glob=!{excluded}/")

    cmd.extend(str(path) for path in search_paths)

    return cmd


def _iglob_match(name: str, pattern: str) -> bool:
    """Case-insensitive glob match."""
    return fnmatch.fnmatch(name.lower(), pattern.lower())


def _display_path(path: Path) -> str:
    """Return a path relative to the vault when possible for concise logging."""
    try:
        return str(path.resolve().relative_to(config.VAULT_PATH.resolve()))
    except ValueError:
        return str(path)


def _resolve_case_insensitive_search_dirs(path_prefix: str | None) -> list[Path]:
    """Resolve all vault directories matching a path prefix case-insensitively."""
    if not path_prefix:
        return [config.VAULT_PATH]

    if "\x00" in path_prefix:
        raise ValueError("Path contains null bytes")

    parts = Path(path_prefix).parts
    for part in parts:
        if part.startswith("."):
            raise ValueError(
                f"Path component '{part}' starts with '.'; "
                "dotfiles and hidden directories are not allowed"
            )

    candidates = [config.VAULT_PATH.resolve()]
    for part in parts:
        part_lower = part.lower()
        next_candidates: list[Path] = []
        for directory in candidates:
            try:
                children = directory.iterdir()
            except OSError:
                continue

            for child in children:
                if not child.is_dir():
                    continue
                if child.name in config.EXCLUDED_DIRS:
                    continue
                if child.name.lower().startswith(part_lower):
                    next_candidates.append(child)
        candidates = next_candidates
        if not candidates:
            break

    vault_root = config.VAULT_PATH.resolve()
    return [
        path
        for path in candidates
        if path == vault_root or str(path.resolve()).startswith(str(vault_root) + "/")
    ]


def _iter_searchable_files(search_path: Path):
    """Yield regular files under the search path, skipping excluded directories."""
    for file_path in search_path.rglob("*"):
        if not file_path.is_file():
            continue
        if any(part in config.EXCLUDED_DIRS for part in file_path.parts):
            continue
        yield file_path


def _collect_search_scope(
    query: str, search_path: Path, file_pattern: str
) -> dict[str, object]:
    """Summarize which files are in scope for search logging."""
    query_lower = query.lower()
    total_files = 0
    candidate_files = 0
    sample_candidates: list[str] = []
    filename_matches_in_candidates: list[str] = []
    filename_matches_excluded_by_pattern: list[str] = []

    for file_path in _iter_searchable_files(search_path):
        total_files += 1
        rel_path = _display_path(file_path)
        filename_match = query_lower in rel_path.lower()
        candidate = _iglob_match(file_path.name, file_pattern)

        if candidate:
            candidate_files += 1
            if len(sample_candidates) < 5:
                sample_candidates.append(rel_path)
            if filename_match and len(filename_matches_in_candidates) < 5:
                filename_matches_in_candidates.append(rel_path)
        elif filename_match and len(filename_matches_excluded_by_pattern) < 5:
            filename_matches_excluded_by_pattern.append(rel_path)

    return {
        "total_files": total_files,
        "candidate_files": candidate_files,
        "sample_candidates": sample_candidates,
        "filename_matches_in_candidates": filename_matches_in_candidates,
        "filename_matches_excluded_by_pattern": filename_matches_excluded_by_pattern,
    }


def _search_ripgrep(
    query: str,
    search_paths: list[Path],
    file_pattern: str,
    max_results: int,
    context_lines: int,
) -> list[dict]:
    """Search using ripgrep for performance."""
    cmd = _build_ripgrep_command(
        query=query,
        search_paths=search_paths,
        file_pattern=file_pattern,
        max_results=max_results,
        context_lines=context_lines,
    )

    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
    except (subprocess.TimeoutExpired, FileNotFoundError):
        return []

    matches = []
    for line in result.stdout.splitlines():
        try:
            data = json.loads(line)
        except json.JSONDecodeError:
            continue

        if data.get("type") == "match":
            match_data = data["data"]
            file_path = match_data["path"]["text"]
            try:
                rel_path = str(
                    Path(file_path).resolve().relative_to(config.VAULT_PATH.resolve())
                )
            except ValueError:
                continue

            line_number = match_data["line_number"]
            line_text = match_data["lines"]["text"].rstrip("\n")

            matches.append({
                "path": rel_path,
                "line_number": line_number,
                "match_context": line_text,
            })

            if len(matches) >= max_results:
                break

    return matches


def _search_python(
    query: str,
    search_paths: list[Path],
    file_pattern: str,
    max_results: int,
    context_lines: int,
) -> list[dict]:
    """Fallback Python-based search."""
    query_lower = query.lower()
    matches = []

    for search_path in search_paths:
        for file_path in _iter_searchable_files(search_path):
            if not _iglob_match(file_path.name, file_pattern):
                continue

            try:
                content = file_path.read_text(encoding="utf-8")
            except (UnicodeDecodeError, PermissionError):
                continue

            lines = content.splitlines()
            for i, line in enumerate(lines):
                if query_lower in line.lower():
                    start = max(0, i - context_lines)
                    end = min(len(lines), i + context_lines + 1)
                    context = "\n".join(lines[start:end])

                    try:
                        rel_path = str(
                            file_path.resolve().relative_to(config.VAULT_PATH.resolve())
                        )
                    except ValueError:
                        continue

                    matches.append({
                        "path": rel_path,
                        "line_number": i + 1,
                        "match_context": context,
                    })

                    if len(matches) >= max_results:
                        return matches

    return matches


def _get_frontmatter_excerpt(file_path: Path, max_keys: int = 3) -> dict | None:
    """Read frontmatter from a file, returning first N key-value pairs."""
    try:
        content = file_path.read_text(encoding="utf-8")
        post = frontmatter.loads(content)
        if not post.metadata:
            return None
        keys = list(post.metadata.keys())[:max_keys]
        return {k: post.metadata[k] for k in keys}
    except Exception:
        return None


def vault_search(
    query: str,
    path_prefix: str | None = None,
    file_pattern: str = "*.md",
    max_results: int = 20,
    context_lines: int = 2,
) -> str:
    """Search for text across vault files."""
    try:
        search_paths = _resolve_case_insensitive_search_dirs(path_prefix)

        if not search_paths:
            logger.error(
                "vault_search invalid path: path_prefix=%r is not a directory",
                path_prefix,
            )
            return json.dumps({"error": f"Search path is not a directory: {path_prefix}"})

        scopes = [
            _collect_search_scope(query, search_path, file_pattern)
            for search_path in search_paths
        ]
        scope = {
            "total_files": sum(item["total_files"] for item in scopes),
            "candidate_files": sum(item["candidate_files"] for item in scopes),
            "sample_candidates": [
                candidate
                for item in scopes
                for candidate in item["sample_candidates"]
            ][:5],
            "filename_matches_in_candidates": [
                candidate
                for item in scopes
                for candidate in item["filename_matches_in_candidates"]
            ][:5],
            "filename_matches_excluded_by_pattern": [
                candidate
                for item in scopes
                for candidate in item["filename_matches_excluded_by_pattern"]
            ][:5],
        }
        rg_available = shutil.which("rg") is not None
        backend = "ripgrep" if rg_available else "python"

        logger.info(
            "vault_search request: query=%r path_prefix=%r resolved_paths=%s file_pattern=%r "
            "max_results=%d context_lines=%d backend=%s search_mode=content_only case_insensitive=true",
            query,
            path_prefix,
            [_display_path(path) for path in search_paths],
            file_pattern,
            max_results,
            context_lines,
            backend,
        )
        logger.info(
            "vault_search scope: total_files=%d candidate_files=%d sample_candidates=%s",
            scope["total_files"],
            scope["candidate_files"],
            scope["sample_candidates"],
        )

        if rg_available:
            logger.info(
                "vault_search ripgrep command: %s",
                shlex.join(
                    _build_ripgrep_command(
                        query=query,
                        search_paths=search_paths,
                        file_pattern=file_pattern,
                        max_results=max_results,
                        context_lines=context_lines,
                    )
                ),
            )
            matches = _search_ripgrep(
                query, search_paths, file_pattern, max_results, context_lines
            )
        else:
            logger.info("vault_search fallback: using Python line-by-line content scan")
            matches = _search_python(
                query, search_paths, file_pattern, max_results, context_lines
            )

        for match in matches:
            file_full_path = config.VAULT_PATH / match["path"]
            match["frontmatter_excerpt"] = _get_frontmatter_excerpt(file_full_path)

        truncated = len(matches) >= max_results

        logger.info(
            "vault_search result: total_matches=%d truncated=%s",
            len(matches),
            truncated,
        )
        if not matches:
            logger.info("vault_search result: no content matches found for query=%r", query)
            if scope["filename_matches_in_candidates"]:
                logger.info(
                    "vault_search hint: query matched candidate filenames %s, but vault_search does not search filenames, only file contents",
                    scope["filename_matches_in_candidates"],
                )
            if scope["filename_matches_excluded_by_pattern"]:
                logger.info(
                    "vault_search hint: query matched filenames excluded by file_pattern=%r: %s",
                    file_pattern,
                    scope["filename_matches_excluded_by_pattern"],
                )

        return json.dumps({
            "results": matches,
            "total_matches": len(matches),
            "truncated": truncated,
        }, default=str)
    except ValueError as e:
        return json.dumps({"error": str(e)})
    except Exception as e:
        logger.exception("vault_search error: %s", e)
        return json.dumps({"error": str(e)})


def vault_search_frontmatter(
    field: str,
    value: str = "",
    match_type: str = "exact",
    path_prefix: str | None = None,
    max_results: int = 20,
) -> str:
    """Search vault files by frontmatter field values using the in-memory index."""
    from ..server import frontmatter_index

    try:
        results = frontmatter_index.search_by_field(
            field=field,
            value=value,
            match_type=match_type,
            path_prefix=path_prefix,
        )
        snapshot = frontmatter_index.debug_snapshot(path_prefix=path_prefix)
        logger.info(
            "vault_search_frontmatter index query result: field=%r value=%r "
            "match_type=%r path_prefix=%r result_count=%d result_paths=%s "
            "index_file_count=%d prefix_file_count=%d sample_keys=%s",
            field,
            value,
            match_type,
            path_prefix,
            len(results),
            [item["path"] for item in results[:max_results]],
            snapshot["file_count"],
            snapshot["prefix_file_count"],
            snapshot["sample_keys"],
        )

        formatted = []
        for item in results[:max_results]:
            path = item["path"]
            fm = item["frontmatter"]
            title = fm.get("title", Path(path).stem)
            formatted.append({
                "path": path,
                "frontmatter": fm,
                "title": title,
            })

        truncated = len(results) > max_results

        return json.dumps({
            "results": formatted,
            "total": len(formatted),
            "truncated": truncated,
        }, default=str)
    except Exception as e:
        logger.error(f"vault_search_frontmatter error: {e}")
        return json.dumps({"error": str(e)})
