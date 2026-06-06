# Frontmatter Startup Timeout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove frontmatter index construction from the MCP request/session critical path so ChatGPT/Claude can connect reliably even on large vaults and low-CPU hosts.

**Architecture:** Treat the frontmatter index as a process-lifetime resource instead of a FastMCP request/session resource. Start it once when the server process boots, stop it once when the process exits, and keep `vault_search_frontmatter` functional against the long-lived in-memory index. Preserve the existing `watchdog`-based incremental update path so file create/modify/delete events continue updating the in-memory index while the process is running.

**Tech Stack:** Python 3.12, FastMCP, Uvicorn, pytest, python-frontmatter, watchdog

---

### Task 1: Lock In the Regression With Tests

**Files:**
- Modify: `tests/test_frontmatter.py`
- Create: `tests/test_server_lifecycle.py`
- Modify: `src/obsidian_vault_mcp/frontmatter_index.py`

- [ ] **Step 1: Add an idempotency test for the frontmatter index lifecycle**

```python
def test_start_is_idempotent(vault_dir, monkeypatch):
    idx = FrontmatterIndex()
    starts = []

    class DummyObserver:
        def schedule(self, *args, **kwargs):
            pass

        def start(self):
            starts.append("observer-start")

        def stop(self):
            pass

        def join(self):
            pass

    monkeypatch.setattr("obsidian_vault_mcp.frontmatter_index.Observer", DummyObserver)

    idx.start()
    idx.start()

    assert starts == ["observer-start"]
    idx.stop()
```

- [ ] **Step 2: Add a server lifecycle regression test**

```python
import obsidian_vault_mcp.server as server


def test_request_path_does_not_start_index(monkeypatch):
    calls = []

    def fake_start():
        calls.append("start")

    def fake_stop():
        calls.append("stop")

    monkeypatch.setattr(server.frontmatter_index, "start", fake_start)
    monkeypatch.setattr(server.frontmatter_index, "stop", fake_stop)

    server._start_process_resources()
    assert calls == ["start"]

    calls.clear()
    server.vault_list(path="", depth=1)
    server.vault_list(path="", depth=1)

    assert calls == []

    server._stop_process_resources()
    assert calls == ["stop"]
```

- [ ] **Step 3: Run the targeted tests and confirm they fail before the refactor**

Run: `uv run pytest tests/test_frontmatter.py tests/test_server_lifecycle.py -v`

Expected before implementation:
- `test_start_is_idempotent` fails because `FrontmatterIndex.start()` always creates a new observer.
- `test_request_path_does_not_start_index` fails because `server._start_process_resources()` does not exist yet.

- [ ] **Step 4: Commit the failing tests**

```bash
git add tests/test_frontmatter.py tests/test_server_lifecycle.py
git commit -m "test: capture frontmatter lifecycle timeout regression"
```

### Task 2: Move Index Startup to Process Lifetime

**Files:**
- Modify: `src/obsidian_vault_mcp/server.py`
- Modify: `src/obsidian_vault_mcp/frontmatter_index.py`
- Test: `tests/test_server_lifecycle.py`

- [ ] **Step 1: Make `FrontmatterIndex.start()` and `stop()` safe to call multiple times**

```python
class FrontmatterIndex:
    def __init__(self) -> None:
        self._index = {}
        self._lock = threading.Lock()
        self._observer = None
        self._debounce_timer = None
        self._pending_paths = set()
        self._started = False

    def start(self) -> None:
        with self._lock:
            if self._started:
                return
            self._started = True

        t0 = time.monotonic()
        new_index = {}
        count = 0

        for md_path in config.VAULT_PATH.rglob("*.md"):
            if self._is_excluded(md_path):
                continue
            rel = str(md_path.relative_to(config.VAULT_PATH))
            fm = self._parse_frontmatter(md_path)
            if fm is not None:
                new_index[rel] = fm
                count += 1

        with self._lock:
            self._index = new_index

        logger.info(
            "Frontmatter index built: %d files in %.2f seconds",
            count,
            time.monotonic() - t0,
        )

        observer = Observer()
        handler = _VaultEventHandler(self)
        observer.schedule(handler, str(config.VAULT_PATH), recursive=True)
        observer.start()

        with self._lock:
            self._observer = observer

    def stop(self) -> None:
        with self._lock:
            if not self._started:
                return
            self._started = False
            debounce_timer = self._debounce_timer
            observer = self._observer
            self._debounce_timer = None
            self._observer = None

        if debounce_timer is not None:
            debounce_timer.cancel()

        if observer is not None:
            observer.stop()
            observer.join()
```

- [ ] **Step 2: Introduce explicit process-lifetime startup/shutdown helpers in `server.py`**

```python
def _start_process_resources() -> None:
    logger.info("Starting vault MCP server. Vault: %s", VAULT_PATH)
    frontmatter_index.start()
    logger.info(
        "Frontmatter index built: %d files indexed",
        frontmatter_index.file_count,
    )


def _stop_process_resources() -> None:
    frontmatter_index.stop()
    logger.info("Vault MCP server shut down.")
```

- [ ] **Step 3: Remove the FastMCP `lifespan` hook**

```python
mcp = FastMCP(
    "obsidian_web_mcp",
    stateless_http=True,
    json_response=True,
    transport_security=TransportSecuritySettings(
        enable_dns_rebinding_protection=True,
        allowed_hosts=[
            "127.0.0.1:*",
            "localhost:*",
            "[::1]:*",
        ],
    ),
)
```

- [ ] **Step 4: Start and stop process resources around the Uvicorn server lifecycle**

```python
        app.add_middleware(BearerAuthMiddleware)
        logger.info(f"Starting server on port {VAULT_MCP_PORT} with bearer auth + OAuth")
        _start_process_resources()

        try:
            uvicorn.run(
                app,
                host="0.0.0.0",
                port=VAULT_MCP_PORT,
                log_level="info",
                proxy_headers=True,
                forwarded_allow_ips="*",
            )
        finally:
            _stop_process_resources()
```

- [ ] **Step 5: Preserve the live-update path inside `FrontmatterIndex`**

Do not remove or bypass these behaviors during the refactor:

```python
def _schedule_debounce(self, abs_path: str) -> None:
    with self._lock:
        self._pending_paths.add(abs_path)
        if self._debounce_timer is not None:
            self._debounce_timer.cancel()
        self._debounce_timer = threading.Timer(
            config.FRONTMATTER_INDEX_DEBOUNCE, self._flush_pending
        )
        self._debounce_timer.start()

def _flush_pending(self) -> None:
    with self._lock:
        paths = self._pending_paths.copy()
        self._pending_paths.clear()
        self._debounce_timer = None

    for abs_path_str in paths:
        abs_path = Path(abs_path_str)
        rel = str(abs_path.relative_to(config.VAULT_PATH))
        if abs_path.exists():
            fm = self._parse_frontmatter(abs_path)
            with self._lock:
                if fm is not None:
                    self._index[rel] = fm
                else:
                    self._index.pop(rel, None)
        else:
            with self._lock:
                self._index.pop(rel, None)

class _VaultEventHandler(FileSystemEventHandler):
    def on_created(self, event: FileSystemEvent) -> None:
        self._handle(event)

    def on_modified(self, event: FileSystemEvent) -> None:
        self._handle(event)

    def on_deleted(self, event: FileSystemEvent) -> None:
        self._handle(event)
```

The observer should be started once per process and remain attached until `_stop_process_resources()` runs.

- [ ] **Step 6: Run the lifecycle tests and confirm they pass**

Run: `uv run pytest tests/test_frontmatter.py tests/test_server_lifecycle.py -v`

Expected after implementation:
- `test_start_is_idempotent` passes
- `test_request_path_does_not_start_index` passes

- [ ] **Step 7: Commit the lifecycle fix**

```bash
git add src/obsidian_vault_mcp/server.py src/obsidian_vault_mcp/frontmatter_index.py tests/test_frontmatter.py tests/test_server_lifecycle.py
git commit -m "fix: remove index build from MCP request lifecycle"
```

### Task 3: Preserve Live Updates When Vault Files Change

**Files:**
- Modify: `tests/test_frontmatter.py`
- Modify: `src/obsidian_vault_mcp/frontmatter_index.py`

- [ ] **Step 1: Add a deterministic regression test for file modification**

```python
def test_index_updates_after_file_modify(index, vault_dir):
    note = vault_dir / "test-note.md"
    note.write_text(
        "---\nstatus: archived\ntype: note\n---\n\nUpdated body.\n",
        encoding="utf-8",
    )

    with index._lock:
        index._pending_paths.add(str(note))

    index._flush_pending()

    results = index.search_by_field("status", "archived", "exact")
    assert any(item["path"] == "test-note.md" for item in results)
```

- [ ] **Step 2: Add a deterministic regression test for file creation**

```python
def test_index_updates_after_file_create(index, vault_dir):
    note = vault_dir / "created-note.md"
    note.write_text(
        "---\nstatus: active\ntype: scratch\n---\n\nCreated after startup.\n",
        encoding="utf-8",
    )

    with index._lock:
        index._pending_paths.add(str(note))

    index._flush_pending()

    results = index.search_by_field("type", "scratch", "exact")
    assert any(item["path"] == "created-note.md" for item in results)
```

- [ ] **Step 3: Add a deterministic regression test for file deletion**

```python
def test_index_updates_after_file_delete(index, vault_dir):
    note = vault_dir / "test-note.md"
    note.unlink()

    with index._lock:
        index._pending_paths.add(str(note))

    index._flush_pending()

    results = index.search_by_field("status", "active", "exact")
    assert all(item["path"] != "test-note.md" for item in results)
```

- [ ] **Step 4: If the tests expose shutdown or reentrancy bugs, make the smallest fix in `frontmatter_index.py`**

Allowed fixes:

- Wrap transitions of `_started`, `_observer`, `_debounce_timer`, and `_pending_paths` in `with self._lock:` so start/stop/flush remain race-safe.
- In `stop()`, set `self._observer = None` and `self._debounce_timer = None` before releasing the lock.
- In `stop()` or `_flush_pending()`, call `self._pending_paths.clear()` once the pending snapshot has been copied or shutdown begins.

Do not replace the watcher with a polling loop or request-time rescan.

- [ ] **Step 5: Run the frontmatter tests**

Run: `uv run pytest tests/test_frontmatter.py -v`

Expected: PASS

- [ ] **Step 6: Commit the live-update regression coverage**

```bash
git add src/obsidian_vault_mcp/frontmatter_index.py tests/test_frontmatter.py
git commit -m "test: cover frontmatter live updates after lifecycle refactor"
```

### Task 4: Preserve Behavior for Frontmatter Queries

**Files:**
- Modify: `src/obsidian_vault_mcp/tools/search.py`
- Create: `tests/test_search_frontmatter.py`

- [ ] **Step 1: Add a regression test that frontmatter queries still return results**

```python
def test_vault_search_frontmatter_uses_live_index(index):
    payload = json.loads(
        vault_search_frontmatter(field="status", value="active", match_type="exact")
    )

    assert payload["total"] >= 1
    assert any(item["path"] == "test-note.md" for item in payload["results"])
```

- [ ] **Step 2: If needed, add a readiness guard that fails clearly instead of hanging**

```python
if frontmatter_index.file_count == 0 and match_type != "exists":
    logger.warning("Frontmatter index is empty during search")
```

This step is intentionally conservative. Do not add a background warm-up or fallback scan unless post-fix verification shows process-start latency is still operationally unacceptable.

- [ ] **Step 3: Run search-specific tests**

Run: `uv run pytest tests/test_frontmatter.py tests/test_search_frontmatter.py -v`

Expected: PASS

- [ ] **Step 4: Commit the query regression coverage**

```bash
git add src/obsidian_vault_mcp/tools/search.py tests/test_search_frontmatter.py
git commit -m "test: cover frontmatter search after lifecycle refactor"
```

### Task 5: Document the Operational Change

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update the architecture/configuration docs**

Add text like:

```markdown
The frontmatter index now starts once when the server process boots and stays resident for the lifetime of the process. MCP requests no longer rebuild the index on each stateless HTTP session.
```

- [ ] **Step 2: Document live updates explicitly**

Add text like:

```markdown
The index continues to update while the server is running via the existing filesystem watcher, so changes made by Obsidian, Obsidian Sync, or MCP writes are reflected without restarting the process.
```

- [ ] **Step 3: Add a note about cold boot behavior**

Add text like:

```markdown
Large vaults may still take time to index on initial process startup, especially on low-CPU hosts or cold disks, but this no longer blocks each MCP request.
```

- [ ] **Step 4: Run the full test suite**

Run: `uv run pytest tests/ -v`

Expected: PASS

- [ ] **Step 5: Commit docs and verification**

```bash
git add README.md
git commit -m "docs: explain frontmatter index lifecycle"
```

### Task 6: Post-Fix Validation Against the Reported Failure Mode

**Files:**
- No code changes required unless validation fails

- [ ] **Step 1: Reproduce with a large vault on a low-CPU host**

Run the server and verify logs show this order once per process:

```text
Starting vault MCP server. Vault: /path/to/vault
Frontmatter index built: 2269 files indexed
```

They should not repeat on every `POST /mcp`.

- [ ] **Step 2: Edit a note while the server stays up and verify the index changes without restart**

Expected:
- Updating frontmatter on an existing note changes `vault_search_frontmatter` results
- Creating a new note makes it discoverable
- Deleting a note removes it from frontmatter search results

- [ ] **Step 3: Validate connection behavior from the MCP client**

Expected:
- Initial integration request returns without timing out once the process is already running
- Subsequent MCP requests do not trigger index rebuilds
- CPU no longer spikes on every request

- [ ] **Step 4: Decide whether a second phase is needed**

Only if process boot itself is still too slow:
- Add background index warm-up
- Add explicit readiness state
- Optionally add a fallback scan for `vault_search_frontmatter`

- [ ] **Step 5: Final integration commit**

```bash
git commit --allow-empty -m "chore: validate frontmatter timeout fix"
```
