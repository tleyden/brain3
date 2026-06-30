# Plan: Case-insensitive, broad search & query

## Goal

MCP clients (Claude, ChatGPT) inconsistently send casing. Make every
**search/query** surface case-insensitive and broad, so searching `plan` or
`Plan` finds both. Keep every **mutating/addressing** surface
(read/write/move/delete by path) case-sensitive, so `plan/` and `Plan/` remain
distinct folders that are never merged or written to by accident.

Guiding principle:
- **Look across the vault → case-insensitive, broad.** This includes the search
  `path_prefix` scope. Every search response returns full vault-relative paths
  (e.g. `plan/roadmap.md` vs `Plan/strategy.md`), so when a broad scope unions
  two same-spelled folders the AI consuming the response can disambiguate from
  the paths. Broad is safe here because search only *reads and returns*.
- **Act on a single named path → exact case, unchanged.** read/write/move/delete
  must hit exactly the file named; acting on the wrong same-spelled file is
  dangerous, so these stay case-sensitive. `plan/` and `Plan/` stay distinct.

## Scope (files)

All changes are in `brain3-mcp-vault-tools` (the Python MCP tools). The Rust
gateway performs no searching and is untouched.

- `src/brain3_mcp_vault_tools/tools/search.py`
- `src/brain3_mcp_vault_tools/frontmatter_index.py`
- `src/brain3_mcp_vault_tools/vault.py`
- `src/brain3_mcp_vault_tools/models.py` (description text only)
- `tests/` (new behavior coverage)

Out of scope / deliberately left case-sensitive (these *act on* a single named
path): `read_file`, `vault_read`, `vault_batch_read`, `vault_move`,
`vault_delete`, write/patch tools, and the path-resolution used by them
(`resolve_vault_path`). Note: search now resolves `path_prefix`
case-insensitively (change #4) via its own helper, separate from the exact-case
`resolve_vault_path` the mutating tools use.

## Already case-insensitive (verified, no change)

- `vault_search` content matching: ripgrep `-i` and Python fallback `.lower()`.
- `vault_search_frontmatter` `contains` value match: already lowercases both sides.

## Changes

### 1. `vault_search` file_pattern — case-insensitive glob

- **Ripgrep** (`_build_ripgrep_command`, search.py:31,39): replace `--glob=` with
  `--iglob=` for both the file pattern and the excluded-dir globs, so `*.md`
  matches `NOTE.MD`.
- **Python fallback** (`_search_python`, search.py:160): replace
  `fnmatch.fnmatch(file_path.name, file_pattern)` with a case-insensitive match.
  `fnmatch.fnmatch` is case-sensitive on Linux (POSIX `normcase` is identity), so
  force it: `fnmatch.fnmatch(name.lower(), pattern.lower())`.
- **Scope/logging helper** (`_collect_search_scope`, search.py:75): same
  case-insensitive fnmatch fix so the diagnostic counts stay accurate.
- Add a small shared helper, e.g. `_iglob_match(name, pattern) -> bool`, in
  search.py to avoid duplicating the `.lower()` logic.

### 2. `vault_search_frontmatter` — case-insensitive field name + exact value

In `FrontmatterIndex.search_by_field` (frontmatter_index.py:113-127):

- Match the **field name** case-insensitively. A doc may legitimately carry both
  `Status` and `status` (distinct keys, like distinct folders); a query for
  `status` should match either. Implement by scanning the frontmatter dict for
  every key whose `.lower()` equals `field.lower()` and considering all matches.
- `exact`: compare case-insensitively — `str(fm[k]).lower() == value.lower()`.
- `contains`: already CI; just route through the same matched-key logic.
- `exists`: true if any key matches `field` case-insensitively.

Implementation sketch (per indexed file):
```
matched_keys = [k for k in fm if k.lower() == field.lower()]
if not matched_keys:
    continue
if match_type == "exists":
    hit = True
elif match_type == "exact":
    hit = any(str(fm[k]).lower() == value.lower() for k in matched_keys)
elif match_type == "contains":
    hit = any(value.lower() in str(fm[k]).lower() for k in matched_keys)
```
Return the file once if `hit`. The returned `frontmatter` dict is unchanged
(full original casing preserved for the AI to read).

### 3. `vault_list` pattern — case-insensitive glob

In `list_directory` (vault.py:200): replace
`fnmatch.fnmatch(entry.name, pattern)` with the case-insensitive variant
(`fnmatch.fnmatch(entry.name.lower(), pattern.lower())`). Consider a tiny local
helper or importing the search.py helper; keep it simple to avoid a cross-module
dependency — a 2-line inline form is fine here.

### 4. `path_prefix` scope — case-insensitive / broad (both search tools)

`path_prefix` selects *where* to search. Make it case-insensitive so
`path_prefix="plan"` scopes over both `plan/` and `Plan/` (unioned). This is safe
because results carry full distinct paths for the AI to disambiguate; search only
reads and returns, it never mutates.

- **`vault_search_frontmatter`** (frontmatter_index.py:116): change
  `rel_path.startswith(path_prefix)` to
  `rel_path.lower().startswith(path_prefix.lower())`. One line, naturally unions.
- **`vault_search`** (search.py:215): today it does
  `search_path = resolve_vault_path(path_prefix)` — a single concrete dir that, on
  a case-sensitive FS, errors if the casing differs. Replace with a helper that
  resolves **all** case-insensitively-matching directories:
  - Walk from `config.VAULT_PATH`, matching each `path_prefix` component against
    child directory names case-insensitively, yielding a list of concrete dirs
    (e.g. both `plan/` and `Plan/`). Preserve the existing safety invariants from
    `resolve_vault_path` (reject null bytes, reject any `.`-prefixed component,
    never escape the vault — the walk stays within the vault by construction).
  - Pass the matched dirs to ripgrep as multiple positional path args (ripgrep
    accepts many); for the Python fallback, iterate the dirs. This keeps the
    perf benefit of scoping (only matched subtrees scanned) while unioning.
  - If no directory matches, return the existing "not a directory" style result.
- **Known limitation (pre-existing, not introduced here):** prefix matching is
  substring-on-prefix, so `path_prefix="plan"` already also matches a sibling
  folder like `planning/`. This is current behavior (case-sensitively too) and is
  out of scope; note it but do not change it.

### 5. Decisions (confirmed)

- **`path_prefix` becomes CASE-INSENSITIVE / broad (confirmed).** See change #4.
  Earlier draft kept it case-sensitive; overridden — unioning is fine because the
  full paths in the response let the AI tell `plan/` from `Plan/`.
- **`exact` value match becomes CASE-INSENSITIVE — confirmed, no new param.**
  This removes the only case-sensitive value match, which is the intended "broad
  search" behavior. We will NOT add a `case_sensitive` parameter — keeps the tool
  surface lean per AGENTS.MD.
- **`EXCLUDED_DIRS`** matching stays exact — these are real dir names
  (`.obsidian`, `.git`); fuzzing them adds risk for no benefit. *(No change.)*

## Doc / description updates

- `models.py:200` (`file_pattern` description) and `:233` (`match_type`): note
  matching is case-insensitive. Keep descriptions lean (context-window cost).
- No new tool, no new parameter → no tool count growth.

## Testing

Add focused behavior tests (public tool behavior only, per AGENTS.MD — no private
APIs, no description-string assertions):

- `vault_search`: `file_pattern="*.MD"` finds a `.md` file and vice-versa; both
  ripgrep-present and fallback paths if feasible.
- `vault_search_frontmatter`: querying `status` finds a doc with `Status:`;
  `exact` matches `Draft` vs `draft`; a doc with both `Status` and `status` keys
  is returned once.
- `vault_list`: `pattern="*.MD"` matches mixed-case filenames.
- `path_prefix` scope (both search tools): with both `plan/` and `Plan/` present,
  `path_prefix="plan"` returns hits from *both*, each with its full distinct path
  (`plan/...` and `Plan/...`). `path_prefix="PLAN"` behaves identically.
- Regression: addressing tools (`vault_read` on wrong-case path) still behave as
  before (no fuzzy resolution) — confirms distinct-folder rule preserved.

Run:
```
cd brain3-mcp-vault-tools && uv run --with mcp python -m unittest discover -s tests -v
cargo test   # from repo root, must pass before done
```

## Risks

- More files match a broadened glob → slightly larger result sets; bounded by
  existing `max_results`.
- `exact` semantics change (now CI). Acceptable per goal; documented above.
- No new ingress, no auth changes → SECURITY_AUDIT.md threat model unaffected.
