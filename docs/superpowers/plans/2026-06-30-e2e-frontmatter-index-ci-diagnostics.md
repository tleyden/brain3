# E2E frontmatter-index CI failure: diagnostics plan

Status: **diagnostics only — no fix until root cause is confirmed**
Date: 2026-06-30
Branch context: `e2e_test_verify_loop`

## Symptom

`e2e_smoke_local_docker` fails on CI (GitHub Actions, native Docker on
`ubuntu`) but passes locally (Docker Desktop on macOS). The failing assertion:

```
Error: frontmatter search did not return expected paths
{"projects/alpha.md", "projects/beta.md"};
last result: {"results":[],"total":0,"truncated":false}
```

The test writes `status: active` frontmatter to two notes, then polls
`vault_search_frontmatter` (field=status, value=active, path_prefix=projects/)
for up to 12s expecting both paths. On CI it always gets zero results.

## What is already ruled out

- **Not networking / reachability.** CI gateway logs show every `/mcp` POST
  returning HTTP 200 from the container at `127.0.0.1:2765`, including the
  failing frontmatter search (`status=200 ... body_bytes=230`). The container is
  reachable and answering; it just returns an empty result set.
- **Not a write failure.** `vault_batch_frontmatter_update` returned
  `updated:true` for both files, and the earlier `vault_batch_read` in the test
  confirmed on-disk content. Files + frontmatter exist on disk in the container.
- **The mechanism:** `vault_search_frontmatter` reads from an **in-memory
  index** (`FrontmatterIndex`, `brain3-mcp-vault-tools/src/.../frontmatter_index.py`),
  not from disk. The index is built once at startup via `rglob` and thereafter
  kept fresh **only** by watchdog/inotify filesystem events inside the container.
  On CI the index is empty/stale for the just-written files.

## Why local passes but CI fails (hypotheses, unconfirmed)

Both run the same Linux container; the difference is the host kernel/filesystem
under the in-container inotify watcher:
- Local = Docker Desktop VM (virtiofs/gRPC-FUSE) over macOS `/tmp`.
- CI = native Docker on a GitHub runner, bind-mounting the runner's `/tmp`.

The vault is a Docker **bind mount** (`host vault -> /vault`, see
`crates/platform/src/container/startup.rs`), and the watchdog `Observer` runs
inside the container watching `/vault`. Candidate causes, in rough priority:

1. **inotify instance/watch limits** exhausted on the shared runner; Observer
   silently drops or fails to register watches.
2. **inotify event propagation** for the runner's `/tmp` filesystem / storage
   driver not reaching the in-container watcher.
3. **Debounce/timing race**: `FRONTMATTER_INDEX_DEBOUNCE = 5.0s` vs the test's
   12s deadline under slower CI I/O.
4. **Latent code gap**: `_VaultEventHandler` implements
   `on_created/on_modified/on_deleted` but **not `on_moved`**, while every write
   is an atomic `os.replace` (`write_file_atomic` in `vault.py`) and every move
   uses `shutil.move` — both surface on Linux inotify as
   `IN_MOVED_FROM/IN_MOVED_TO` -> watchdog `FileMovedEvent`, which the handler
   ignores. This alone would also fail locally, so it is not the whole story,
   but it may interact with the environment difference.

We do **not** fix yet — diagnostics first decide between these.

## Primary blind spot

The CI output is entirely **gateway (Rust)** logs. The decisive line — the
Python server's `"Frontmatter index built: N files"` and any watchdog activity —
comes from the **container's** stdout, which the harness never captures
(`apps/gateway/tests/e2e_smoke.rs` only inherits the gateway's stdio; the Python
container runs under `docker run` and its logs are never dumped). We are blind to
the one process that matters.

## Plan (diagnostics + pre-test validation only)

### A. Surface the container's own logs in CI (highest value)
1. CI workflow: add an `if: always()` step after the test running
   `docker logs brain3-mcp-vault-tools`, plus `docker inspect <name>` and
   `docker info | grep -i 'storage driver'`.
2. Backup that survives harness teardown: have `e2e_smoke.rs` dump
   `docker logs <name>` to stdout on the failure path (and before
   `assert_no_container_residue`).

### B. Instrument `FrontmatterIndex` (Python)
3. In `start()`: log the concrete `Observer`/emitter class actually used
   (InotifyObserver vs PollingObserver) and the values of
   `/proc/sys/fs/inotify/max_user_watches` and `max_user_instances` read from
   inside the container.
4. In `_VaultEventHandler`: log **every** event received — type, `src_path`,
   `dest_path` — and add an `on_moved` log. Confirms whether any inotify events
   arrive on CI and whether writes appear as moves.
5. In `_schedule_debounce` / `_flush_pending`: log each schedule and flush with
   timestamps, pending paths, and resulting index size. Distinguishes a debounce
   race from total event silence.
6. In `search_by_field` / `vault_search_frontmatter`: when results are empty, log
   current index `file_count` and a sample of keys — "index empty" vs "index has
   files but not these / wrong field value".

### C. Pre-test and at-failure validation in the harness (disk-vs-index split)
7. Pre-test: assert container is `Running` and
   `docker exec brain3-mcp-vault-tools ls -la /vault` shows the mount from inside.
8. When `wait_for_frontmatter_paths` is about to fail, before erroring:
   (a) `vault_read` on `projects/alpha.md` to confirm `status: active` is on disk
   via MCP, and (b) `docker exec ... cat /vault/projects/alpha.md` + `stat`. If
   disk has it but the index does not, root cause is conclusively event delivery.

### D. Controlled experiment to confirm the inotify hypothesis (env-gated, not a fix)
9. Add an env switch (e.g. `B3_FRONTMATTER_OBSERVER=polling`) selecting watchdog's
   `PollingObserver`, and optionally an env override for the debounce interval.
   Run CI once with polling. If CI then passes, root cause is confirmed as inotify
   event delivery in the CI container; the real fix is then chosen deliberately
   (handle `on_moved`, switch observer, or add an on-write index reconcile). Stays
   env-gated/diagnostic until approved.

### E. Optional runner-side probe
10. In the CI job (outside the container), log `cat /proc/sys/fs/inotify/*`,
    `df -T /tmp`, and `mount | grep /tmp` to correlate with limits/filesystem.

## Expected outcome

After A–C, one failing CI run should reveal: whether the index built at startup
(and with how many files), whether watchdog events arrived at all, and whether
the files were on-disk-but-not-indexed. That pins the cause to one of
{events never delivered, delivered-as-`on_moved`-and-ignored, debounce race}.
Step D confirms before any fix is written.

## Local diagnostic finding added 2026-06-30

A local E2E run after adding A-C diagnostics reproduced the frontmatter-index
symptom: `vault_search_frontmatter(status=active, path_prefix=projects/)`
returned only `projects/beta.md` after the existing 12s polling window, while
`projects/alpha.md` was missing. This confirms the index freshness path is
asynchronous:

- startup indexing is synchronous in `FrontmatterIndex.start()`;
- later freshness depends on watchdog events plus the debounce timer;
- write tools return after disk writes, before the index is guaranteed fresh;
- `wait_for_frontmatter_paths` already retries for 12 seconds, so the observed
  miss is not explained by the absence of a retry loop.

Treat "increase the retry window" only as a diagnostic experiment, not as the
primary fix. If a file is still missing after multiple debounce intervals, the
more likely causes are missed/ignored watchdog events, especially atomic writes
surfacing as `FileMovedEvent`, or an event ordering issue that leaves one path
stale.

## Follow-up fix candidates after diagnostics

Do not implement these until CI logs from A-C confirm the failing path:

1. If logs show `moved` events for updated markdown files with no corresponding
   debounce schedule for the destination, add a tested `on_moved` handler that
   removes the old relative path and schedules/reindexes the destination path.
2. If logs show no useful watchdog events in CI, run the env-gated polling
   observer experiment from D to confirm inotify delivery as the root cause.
3. If logs show scheduled events but flushes happen after the assertion, only
   then consider reducing the debounce or extending the harness wait as a
   timing fix.

## Suggested sequencing

- Start with **A** alone (low risk, likely diagnostic on its own).
- Then **B + C** in one change.
- **D** only if A–C are inconclusive or to confirm before committing a fix.

## Notes / files involved

- `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/frontmatter_index.py`
  (index, watchdog handler, debounce)
- `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/tools/search.py`
  (`vault_search_frontmatter`)
- `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/vault.py`
  (`write_file_atomic` via `os.replace`; `shutil.move`)
- `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/config.py`
  (`FRONTMATTER_INDEX_DEBOUNCE = 5.0`)
- `apps/gateway/tests/e2e_smoke.rs` (harness, `wait_for_frontmatter_paths`)
- `crates/platform/src/container/startup.rs` (vault bind mount -> `/vault`)
- CI workflow running `uv run scripts/e2e_smoke.py`
