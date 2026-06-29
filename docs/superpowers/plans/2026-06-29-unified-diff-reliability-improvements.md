# Unified Diff Reliability Improvements

## Problem

An AI client repeatedly fails to use `vault_apply_unified_diff` correctly. The
dominant failure modes are:

1. **Off-by-N line targeting** — the client miscounts line numbers when reading a
   window and then constructing a hunk header (`@@ -L,N +L,N @@`).
2. **Hunk count mismatches** — the `,N` counts in the header don't match the
   number of body lines (the failure already captured in
   `2026-06-29-hunk-count-mismatch-fix.md`). The server correctly rejects these,
   but the error (`"Hunk line counts do not match header counts"`) is not
   actionable enough for the client to self-correct.

The client itself proposed the fixes below, ranked by leverage. This plan adapts
them to the existing hexagonal layout (`tools/read.py`, `tools/patch.py`,
`models.py`, `server.py`) and the established testing conventions in
`AGENTS.MD` (test behavior on public APIs only; never test descriptions or log
output).

## Current state (for reference)

- `tools/read.py::vault_read` returns `content` as an opaque text blob plus
  `returned_start_line` / `returned_end_line` / `total_lines` / `content_hash`.
  The client must count lines itself to build a hunk header.
- `tools/patch.py::_parse_unified_diff` raises `PatchError(code, message)`.
  `PatchError` carries only a string `code` and message — no structured detail.
- `vault_apply_unified_diff` already supports `dry_run=True`, returning
  `would_change` + hashes. On a parse failure it returns the same generic error
  as a real apply.

So a "validator" already exists in skeleton form (`dry_run`); the missing piece
is **actionable diagnostics**, not a new tool.

## Goals / non-goals

- **Goal:** eliminate line-counting mistakes (client proposal #1) and make every
  rejection self-correctable (proposals #3, #4, #7).
- **Goal:** keep the unified-diff engine as the single apply path. Any new
  higher-level edit operation must generate a diff internally and run it through
  the existing `_apply_hunks`, so we don't fork the apply logic.
- **Non-goal:** no new network ingress, no auth changes, no changes to the
  security posture. This is all inside the vault-tools MCP surface.
- **Non-goal (deliberately cut):** no new `vault_apply_text_edit` tool (client
  proposal #5) and no LLM-side "generate from instructions" helper (proposal #6).
  Both add a whole new tool + schema + description to the MCP surface, and we are
  **hard-constrained by prompt size** — every tool and description string is
  permanent context cost for every client request. Phases 1–2 fix the actual
  failure modes by improving *existing* tools, with near-zero added surface.
- **Non-goal:** no `patch_context` metadata on reads (proposal #5b) — redundant
  once numbered reads land, and not worth the extra payload.

---

## Phase 1 — Line-numbered reads (proposal #1, highest leverage)

**Why first:** removes the root cause of off-by-N errors. The client never has to
count again — it copies the exact line numbers the read returned into the hunk
header.

**Design:** keep the raw `content` field unchanged (the diff body still needs
verbatim text to copy into context lines), and **add** a parallel numbered view
rather than replacing `content`.

`numbered_lines` is **gated behind a `numbered: bool` param, default `False`**
(decided in review). Always-on would double the payload of every read and is pure
context cost — and full-document reads don't need it. The edit workflow always
reads a window first, so the client opts in there.

When `numbered=True`, add to `vault_read` output:

```json
"numbered_lines": [
  {"line": 101, "text": "foo"},
  {"line": 102, "text": "bar"}
]
```

- Each `text` is the logical line **without** its trailing `\n`;
  `has_trailing_newline` already conveys file-level newline status.

**Changes:**
- `tools/read.py`: extend `_slice_content` (or `vault_read`) to emit the
  `numbered_lines` array for the returned window when requested.
- `models.py::VaultReadInput`: add `numbered: bool = False` with a description
  that says **when** to use it — i.e. when preparing a `vault_apply_unified_diff`
  edit, so the exact line numbers can be copied into the `@@` hunk header without
  counting.
- `server.py::vault_read`: thread the new param through, and **update the
  `vault_read` tool description** to mention that `numbered=True` should be used
  when reading a window in preparation for a unified-diff edit. Keep the addition
  to the description as short as possible (it is permanent prompt cost).

**Tests (behavior only):**
- Windowed read with `numbered=True` returns correct `{line, text}` pairs whose
  `line` values match `returned_start_line..returned_end_line`.
- `numbered=False` (default) omits the array — no payload regression.

---

## Phase 2 — Actionable patch diagnostics (proposals #3, #4, #7)

**Why second:** combined with Phase 1, the client's own estimate is this
eliminates ~95% of failures. `dry_run` already lets the client validate before
applying; this phase makes the *failure* output tell it exactly what to fix.

**Changes in `tools/patch.py`:**
- Extend `PatchError` to carry an optional structured `details: dict`.
- Make `_parse_unified_diff` accumulate parsed hunk metadata **as it goes**, so
  that even a mid-parse failure can report the hunks already validated plus the
  one that failed. When counts mismatch (current line 86–87), raise with details:
  ```json
  {
    "header": "@@ -117,7 +117,4 @@",
    "expected_old_count": 7,
    "actual_old_count": 8,
    "expected_new_count": 4,
    "actual_new_count": 5,
    "parsed_hunks": [
      {"header": "@@ -10,3 +10,3 @@", "old_start": 10, "old_count": 3, "new_count": 3}
    ]
  }
  ```
  (`parsed_hunks` = hunks successfully parsed before the failing one; empty if the
  first hunk fails.)
- On a successful `dry_run`, surface parsed hunk metadata so the client can
  confirm targeting before committing the apply:
  ```json
  "hunks": [{"header": "...", "old_start": 117, "old_count": 7, "new_count": 4}]
  ```
- In `vault_apply_unified_diff`'s `except PatchError` handler, include
  `e.details` in the returned JSON when present.

**Where the metadata appears (answering the review question):**
- **`dry_run=True`, success** → `hunks` metadata array (the "validator" view).
- **Any apply, parse failure** (both `dry_run=True` and real applies) → the
  `details` object, which includes the failing header, expected-vs-actual counts,
  AND the `parsed_hunks` accumulated before the failure. So yes — **the error
  path returns parsed-hunk metadata, not just the single failing header.**
- **`dry_run=False`, success** → unchanged lean response (no `hunks`), to keep
  the hot path's payload small. Decided in review: dry-run-only for the success
  metadata.

This makes `dry_run=True` the "patch validator" the client asked for (proposal
#2) without adding a separate tool — `would_change` + parsed hunks on success,
precise count diagnostics + partial hunk metadata on failure.

**Tests (behavior only):**
- A header/body count mismatch returns `error_code="invalid_patch"` AND a
  `details` object with the four expected/actual counts, the offending header,
  and a `parsed_hunks` array.
- A multi-hunk patch whose *second* hunk has bad counts returns the first hunk in
  `parsed_hunks`.
- A successful `dry_run` returns the `hunks` metadata array with correct
  `old_start`/`old_count`/`new_count`.
- A successful real apply (`dry_run=False`) does **not** include `hunks` (hot
  path stays lean).
- Existing mismatch RCA tests still pass (they only assert the message substring,
  which we keep).

---

## Out of scope (cut in review)

The client also proposed a structured `vault_apply_text_edit` tool (server-side
edit ops that emit a diff) and `patch_context` metadata on reads. **Both were cut
to protect prompt size** — a new tool means a new permanent schema + description
in every request's context. Phases 1–2 address the real failure modes by
improving existing tools instead. Revisit only if numbered reads + diagnostics
prove insufficient in practice.

## Sequencing & rollout

Phase 1 + Phase 2 ship together — they're the 95% fix, they're small, and they
add essentially no MCP surface (one optional read param + richer response fields
on an existing tool).

After the change, per `AGENTS.MD`:
- Run Python tests:
  `cd brain3-mcp-vault-tools && uv run --with mcp python -m unittest discover -s tests -v`
- Run `cargo test` at repo root (no Rust changes expected, but confirm nothing
  broke in the gateway that surfaces these tools).

## Files touched

- `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/tools/read.py`
- `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/tools/patch.py`
- `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/models.py`
- `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py`
- `brain3-mcp-vault-tools/tests/test_tool_write_patch_api.py`

## Decisions locked in review

1. Phase 1 `numbered_lines` is **gated by a `numbered` param, default off**
   (prompt-size cost). Tool description explains it's for preparing unified-diff
   edits.
2. Phase 2 success-side `hunks` metadata is **`dry_run`-only**; the failure path
   returns `details` (with `parsed_hunks`) for both dry-run and real applies.
3. Structured edit ops (proposal #5) and read `patch_context` (#5b) are **cut** —
   too much added prompt surface for the benefit.
