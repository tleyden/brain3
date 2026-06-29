# Unified Diff: Accept Hunk-Only Patches (synthesize file headers)

## Problem

An AI client still fails on first attempt with `vault_apply_unified_diff`. The
server rejects the patch with:

```
Patch must start with unified diff file headers   (error_code=invalid_patch)
```

Root cause is a **mismatch between the tool description and the implementation**:

- `tools/patch.py::_parse_unified_diff` (line 52) requires the diff to begin with
  `--- ` and `+++ ` file headers.
- The tool description in `server.py` (lines 421–426) shows **only hunk-only
  examples** (`@@ -5,1 +5,1 @@\n-old\n+new`) with no headers. LLMs copy examples
  verbatim, so the model emits exactly the header-less form the parser rejects.

So the description teaches the failing format. The client confirmed that once it
saw the error and manually prepended `--- a/<path>` / `+++ b/<path>`, the call
succeeded immediately.

## Goals / non-goals

- **Goal:** make first-attempt success the default by accepting the form the
  model naturally emits, and by making the description match the implementation.
- **Goal:** keep the unified-diff engine as the single apply path — no new tool,
  no fork of `_apply_hunks`.
- **Non-goal:** no new MCP surface (no new tool/schema), no auth/ingress changes.
- **Non-goal:** does not touch the hunk-count-mismatch logic or the numbered-read
  / diagnostics work (separate, already-shipped plans). The `CRITICAL` count note
  in the description stays.

## Chosen approach: synthesize headers for hunk-only patches (client proposal #1)

The `path` argument already identifies the target file, so the `--- a/path` /
`+++ b/path` headers are pure redundancy when present and a footgun when absent.
We make the parser accept **both** forms:

1. **Hunk-only** (preferred, cheaper): the diff starts directly with `@@ `. The
   parser uses `path` as the target and parses hunks from line 0.
2. **Full unified diff** (still accepted, backward-compatible): the diff starts
   with `--- `/`+++ ` headers. Existing path-mismatch / multi-file / rename
   validation is preserved.

This is strictly better than merely documenting the header requirement:
- It removes the most common failure class entirely (and the `path_mismatch`
  class for the hunk-only form).
- It makes the **existing** description examples correct, so the description
  barely changes.
- It saves tokens (no duplicated path in headers).

### Changes in `tools/patch.py`

In `_parse_unified_diff(path, diff_text)`, replace the strict header check
(lines 51–60) with a mode detection:

- Normalize CRLF (unchanged). Determine the first content line.
- **If it starts with `--- `:** require the `+++ ` second header (as today),
  normalize both paths, enforce `old_path == new_path` and `old_path == path`
  (keep `path_mismatch` / rename errors), and begin hunk parsing at index 2.
- **Else if it starts with `@@ `:** treat as hunk-only. Skip header parsing;
  begin hunk parsing at index 0. No path validation needed — `path` is authoritative.
- **Else:** raise `invalid_patch` with a clearer message than today, e.g.
  `"Patch must start with a hunk header (@@ ...) or unified diff file headers (--- / +++)"`.

Refactor so the hunk-parsing loop (current lines 65–125) starts from a computed
`index` rather than the hard-coded `2`. Everything downstream (`_apply_hunks`,
count validation, `details`/`parsed_hunks`) is unchanged because it only consumes
parsed hunks.

Keep the existing guards inside the loop (multi-file `--- `/`+++ ` mid-body →
`Multi-file patches are not supported`; no-newline markers; invalid line prefixes).

### Changes in `server.py` (tool description) — keep it lean

Rewrite the description so the **preferred form is hunk-only** and headers are
documented as optional. Keep the `CRITICAL` count guidance (it's the other real
failure mode). Proposed shape (final wording tuned in review, stay terse —
permanent prompt cost):

```
Apply a unified diff to an existing vault file (target = `path` arg).
Prefer over vault_create_overwrite_file — cheaper and safer.

Submit just the hunk(s); file headers (--- / +++) are optional and inferred
from `path`. Standard full diffs with headers are also accepted.

CRITICAL: the counts in @@ -L,N +L,N @@ must exactly match the lines in the hunk
body. N counts context lines (' ') AND changed lines ('-'/'+'); context lines
count toward both old and new.

Example (no context): @@ -5,1 +5,1 @@\n-old\n+new
Example (1 context each side): @@ -4,3 +4,3 @@\n ctx\n-old\n+new\n ctx
```

### Changes in `models.py`

Update the `diff` field description (currently "Unified diff text for a single
file") to: hunk(s) for a single file; file headers optional and inferred from
`path`.

## Tests (behavior only, public API — per AGENTS.MD)

Add to `tests/test_tool_write_patch_api.py`:

- **Hunk-only apply succeeds:** a patch that starts with `@@ ` (no `---`/`+++`)
  changes a middle line and is written. Mirror `test_apply_unified_diff_changes_one_middle_line`.
- **Hunk-only dry_run** reports `would_change=true` and the `hunks` metadata.
- **Multi-hunk hunk-only patch** applies correctly.
- **Full-header form still works** — keep/confirm existing header-based tests
  (the `_unified_diff` helper emits headers via `difflib`), proving backward compat.
- **Garbage prefix** (neither `@@ ` nor `--- `) returns `error_code=invalid_patch`
  with the new clearer message substring.
- **Fenced patch rejected:** a diff wrapped in a ` ```diff ` markdown fence
  returns `error_code=invalid_patch` (no fence stripping — strict).
- Do **not** test the description string (per AGENTS.MD).

## Verification

- `cd brain3-mcp-vault-tools && uv run --with mcp python -m unittest discover -s tests -v`
- `cargo test` at repo root (no Rust change expected; confirm nothing broke in the
  gateway surfacing this tool).

## Files touched

- `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/tools/patch.py`
- `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py`
- `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/models.py`
- `brain3-mcp-vault-tools/tests/test_tool_write_patch_api.py`

## Decisions locked in review

1. **Accept both forms.** Hunk-only is preferred/documented, but full unified
   diffs with `---`/`+++` headers remain accepted for backward compatibility and
   zero risk to existing clients.
2. **No code-fence stripping — strict.** If the `diff` argument is wrapped in a
   markdown fence (e.g. ` ```diff\n@@ ...\n``` `), the first line is ` ```diff `,
   which matches neither `@@ ` nor `--- `, so the parser rejects it with
   `invalid_patch` and the clearer message. The model is expected to send a raw
   diff. Add a test asserting a fenced patch is rejected.
3. **Strict on whitespace.** No tolerance for leading/trailing blank lines before
   the first `@@`. The detection branch requires the first content line to start
   with `@@ ` or `--- `; anything else (including a leading blank line) →
   `invalid_patch` with the clearer message.
