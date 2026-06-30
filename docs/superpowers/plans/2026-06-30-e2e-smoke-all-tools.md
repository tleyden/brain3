# E2E Smoke Test — Exercise ALL Vault Tools

## Problem

The current E2E smoke test (`apps/gateway/tests/e2e_smoke.rs`) only touches **5 of
the 10** MCP tools, and its `list_tools` assertion only checks for those same 5
names. That is why commenting out `vault_list` in `server.py` (lines 523–549) did
not fail the test: nothing lists the tool, and nothing calls it.

Two distinct gaps:

1. **Discovery gap** — the `list_tools` loop asserts a 5-name subset, so removing
   any of the other 5 tools is invisible.
2. **Behavior gap** — only `vault_create_overwrite_file`, `vault_read`,
   `vault_apply_unified_diff`, `vault_search`, `vault_delete` are actually called.
   `vault_batch_read`, `vault_batch_frontmatter_update`, `vault_search_frontmatter`,
   `vault_list`, `vault_move` are never invoked.

## Goal

Extend the single `e2e_smoke_local_docker` test so it drives **all 10 tools** as a
realistic user session, and so that disabling *any* tool causes a failure. No new
test binary, no subagents — extend the existing flow in place. Keep it one
end-to-end narrative against the live gateway → container path.

## Complete tool inventory & verified return shapes

(Confirmed by reading `server.py` + the `tools/` impls — assertion targets are real
field names.)

| Tool | Returns | Currently exercised? |
|------|---------|----------------------|
| `vault_read` | `{path, content, content_hash, total_lines, numbered_lines?}` | yes |
| `vault_batch_read` | `{files:[{path,content}|{path,error}], found, missing}` | **no** |
| `vault_create_overwrite_file` | `{path, created, size}` | yes |
| `vault_apply_unified_diff` | `{applied, ...}` (supports `dry_run`, `expected_hash`) | yes (partial) |
| `vault_batch_frontmatter_update` | `{results:[{path, updated}]}` | **no** |
| `vault_search` | `{results, total}` | yes |
| `vault_search_frontmatter` | `{results, total}` | **no** |
| `vault_list` | `{items:[{name,path,type,size,modified}], total}` | **no** |
| `vault_move` | `{source, destination, moved}` | **no** |
| `vault_delete` | `{path, deleted}` | yes |

## Design principle: every tool must be *load-bearing*

For each tool, assert against an effect that can only be true if the tool actually
ran — verify writes with an independent follow-up read/search, not just the tool's
own success flag. That guarantees commenting a tool out breaks the test.

## Realistic user narrative (single session)

A user builds a small project knowledge base, edits it, reorganizes it, then cleans
up. Tool calls in order:

1. **Seed content — `vault_create_overwrite_file` ×3.** Create:
   - `projects/alpha.md` with frontmatter (`status: draft`, `tags: [work]`) + body
   - `projects/beta.md` with frontmatter (`status: draft`)
   - `daily/2026-06-30.md` plain note
   Assert each returns `created == true`.

2. **`vault_list`** on `projects` (depth 1). Assert `total >= 2` and that the
   `items[].path` set contains `projects/alpha.md` and `projects/beta.md`. Also do
   one call with `pattern: "*.md"` to exercise the filter path.

3. **`vault_read`** `projects/alpha.md` with `numbered: true`. Assert content
   contains the seeded text and capture `content_hash` for step 4.

4. **`vault_apply_unified_diff`** on `projects/alpha.md`, passing the
   `expected_hash` captured in step 3 (exercises the hash-guard path the current
   test skips). Edit a body line. Assert `applied == true`, then **re-read** and
   assert the new text is present.

5. **`vault_batch_read`** of `[projects/alpha.md, projects/beta.md,
   does/not/exist.md]`. Assert `found == 2`, `missing == 1`, and that the alpha
   entry's content reflects the step-4 edit (proves batch path reads live state, not
   a stale single-read).

6. **`vault_batch_frontmatter_update`** setting `status: active` on
   `projects/alpha.md` and `projects/beta.md` in one call. Assert both `results[].updated == true`.

7. **`vault_search_frontmatter`** `field: status, value: active`. Assert `total == 2`
   and the result paths are exactly the two project notes — this independently
   verifies step 6 actually mutated frontmatter.

8. **`vault_search`** full-text for the string introduced by the step-4 diff. Assert
   results reference `projects/alpha.md`.

9. **`vault_move`** `projects/beta.md` → `archive/beta.md`. Assert `moved == true`,
   then assert a `vault_read` of the old path returns an `error` and a read of the
   new path succeeds.

10. **`vault_delete`** `projects/alpha.md` with `confirm: true`. Assert `deleted == true`,
    then a follow-up `vault_read` returns an `error` payload (matches existing
    post-delete assertion).

This is 10/10 tools, each verified by an independent observation.

## Strengthen the discovery assertion

Replace the 5-name subset check with **set equality** against the full expected set
of 10 tool names (sorted compare): assert the names returned by `list_tools` are
*exactly* the 10, no more and no less.

- Catches **removals** — commenting out a tool shrinks the set and fails.
- Catches **additions** as a deliberate tripwire — adding an 11th tool fails the
  test until the author updates the expected set *and* adds a call exercising the
  new tool. This is intentional: it prevents the exact class of bug that motivated
  this work (a tool silently going untested).

Decision confirmed at review: use set equality (not the looser "contains all 10").

## Implementation notes

- All changes are within `apps/gateway/tests/e2e_smoke.rs`. The `call_tool_json` /
  `tool_result_json` helpers already cover the call+parse pattern; reuse them.
- Add a small helper to extract a string field (e.g. `content_hash`) from a
  `vault_read` result to thread into the `expected_hash` arg.
- Keep paths under disposable prefixes (`projects/`, `daily/`, `archive/`) inside the
  per-test temp vault; existing `TempTestDir` teardown handles cleanup.
- The `.trash/` and excluded dirs are skipped by `vault_list`, so listing after a
  delete won't see trashed files — rely on the post-delete `vault_read` error instead.
- `assert_no_container_residue` and shutdown flow are unchanged.

## Out of scope

- No OAuth / public-ingress path changes (covered elsewhere; security policy unchanged).
- No new Python unit tests — this is the Rust E2E layer only.
- Error-injection / malformed-input cases beyond the one missing-file case in
  `vault_batch_read`. Keep the test a happy-path realistic session; deep negative
  testing belongs in the Python unit suite.

## Verification

Run the wrapper, which builds the container then runs the feature-gated test:

```
uv run scripts/e2e_smoke.py
```

Acceptance: test passes with all 10 tools enabled, and **fails** if any single
`@mcp.tool` is commented out in `server.py` (spot-check `vault_list` to confirm the
original gap is closed).
