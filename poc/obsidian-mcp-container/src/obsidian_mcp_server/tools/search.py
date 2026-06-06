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
from ..vault import resolve_vault_path

logger = logging.getLogger(__name__)


def _build_ripgrep_command(
    query: str,
    search_path: Path,
    file_pattern: str,
    max_results: int,
    context_lines: int,
) -> list[str]:
    """Build the ripgrep command used for full-text search."""
    cmd = [
        "rg",
        "--json",
        f"--max-count={max_results}",
        f"--glob={file_pattern}",
        "-i",
        f"--context={context_lines}",
        query,
        str(search_path),
    ]

    for excluded in config.EXCLUDED_DIRS:
        cmd.insert(-2, f"--glob=!{excluded}/")

    return cmd


def _display_path(path: Path) -> str:
    """Return a path relative to the vault when possible for concise logging."""
    try:
        return str(path.relative_to(config.VAULT_PATH))
    except ValueError:
        return str(path)


def _iter_searchable_files(search_path: Path):
    """Yield regular files under the search path, skipping excluded directories."""
    for file_path in search_path.rglob("*"):
        if not file_path.is_file():
            continue
        if any(part in config.EXCLUDED_DIRS for part in file_path.parts):
            continue
        yield file_path


def _collect_search_scope(query: str, search_path: Path, file_pattern: str) -> dict[str, object]:
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
        candidate = fnmatch.fnmatch(file_path.name, file_pattern)

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
    search_path: Path,
    file_pattern: str,
    max_results: int,
    context_lines: int,
) -> list[dict]:
    """Search using ripgrep for performance."""
    cmd = _build_ripgrep_command(
        query=query,
        search_path=search_path,
        file_pattern=file_pattern,
        max_results=max_results,
        context_lines=context_lines,
    )

    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
    except (subprocess.TimeoutExpired, FileNotFoundError):
        return []

    matches = []
    current_match = None

    for line in result.stdout.splitlines():
        try:
            data = json.loads(line)
        except json.JSONDecodeError:
            continue

        if data.get("type") == "match":
            match_data = data["data"]
            file_path = match_data["path"]["text"]
            try:
                rel_path = str(Path(file_path).relative_to(config.VAULT_PATH))
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
    search_path: Path,
    file_pattern: str,
    max_results: int,
    context_lines: int,
) -> list[dict]:
    """Fallback Python-based search."""
    query_lower = query.lower()
    matches = []

    for file_path in _iter_searchable_files(search_path):
        if not fnmatch.fnmatch(file_path.name, file_pattern):
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
                    rel_path = str(file_path.relative_to(config.VAULT_PATH))
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
        if path_prefix:
            search_path = resolve_vault_path(path_prefix)
        else:
            search_path = config.VAULT_PATH

        if not search_path.is_dir():
            logger.error(
                "vault_search invalid path: path_prefix=%r resolved_path=%s is not a directory",
                path_prefix,
                search_path,
            )
            return json.dumps({"error": f"Search path is not a directory: {path_prefix}"})

        scope = _collect_search_scope(query, search_path, file_pattern)
        rg_available = shutil.which("rg") is not None
        backend = "ripgrep" if rg_available else "python"

        logger.info(
            "vault_search request: query=%r path_prefix=%r resolved_path=%s file_pattern=%r "
            "max_results=%d context_lines=%d backend=%s search_mode=content_only case_insensitive=true",
            query,
            path_prefix,
            search_path,
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
                        search_path=search_path,
                        file_pattern=file_pattern,
                        max_results=max_results,
                        context_lines=context_lines,
                    )
                ),
            )
            matches = _search_ripgrep(query, search_path, file_pattern, max_results, context_lines)
        else:
            logger.info("vault_search fallback: using Python line-by-line content scan")
            matches = _search_python(query, search_path, file_pattern, max_results, context_lines)

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
        })
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
        })
    except Exception as e:
        logger.error(f"vault_search_frontmatter error: {e}")
        return json.dumps({"error": str(e)})
