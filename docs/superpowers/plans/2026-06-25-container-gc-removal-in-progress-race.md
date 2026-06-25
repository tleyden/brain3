# RCA: "removal of container is already in progress" during GC

## Error

```
Error response from daemon: removal of container brain3-mcp-vault-tools 
is already in progress
```

Seen during Brain3 startup with `--gc-containers` immediately after a previous
Brain3 session was stopped.

## Timeline of the Race

1. Brain3 session N is running. The MCP container was started with `docker run --rm`
   (Docker only — `remove_on_exit: true` in `build_container_config`, line 211 of
   `crates/platform/src/container/startup.rs`).

2. Brain3 session N receives SIGTERM (user kills it). `shutdown_managed_runtime()`
   calls `stop_mcp_container()` → `docker stop brain3-mcp-vault-tools`.

3. `docker stop` signals the container to exit. When it exits, the Docker daemon
   **automatically begins removing** the container because of the `--rm` flag. This
   removal is **asynchronous** — it may take hundreds of milliseconds to complete.

4. Brain3 session N+1 starts before Docker finishes the removal.

5. `list_managed_containers` queries Docker and still sees the container (Docker
   lists it while it is mid-removal).

6. GC enters `garbage_collect_managed_containers`. The container shows `running: true`
   OR `running: false` depending on timing.

7. If `running: true`: GC calls `docker stop` → may get "No such container" (already
   handled by the fix in this branch) or succeed.

8. GC then calls `docker rm brain3-mcp-vault-tools` → Docker returns:
   **"removal of container brain3-mcp-vault-tools is already in progress"**
   This error string is NOT matched by the current "No such container" guard, so
   it propagates as a hard `ContainerError::Other`, failing the entire startup.

## Root Cause

The container is launched with `--rm`, which delegates its removal to the Docker
daemon on exit. Our GC code assumes it owns the removal lifecycle, but Docker may
already be removing the container by the time GC issues `docker rm`. The two removal
attempts (Docker's automatic `--rm` cleanup and Brain3's explicit `docker rm`) race
against each other.

The existing "No such container" guard (added in the previous commit) handles the
case where removal has *completed* by the time GC runs. This new error handles the
case where removal is *in progress* — a narrower window but still reachable on any
host where container teardown is slow (high load, large filesystem layer sync, etc.).

## Options Considered

**Option A — Treat "already in progress" as success (minimal fix)**  
In `garbage_collect_managed_containers`, add "already in progress" to the set of
`docker rm` stderr strings that are treated as a no-op. Symmetric with the existing
"No such container" handling. No behaviour change for the common path.

**Option B — Remove `--rm`; always do explicit `docker rm` in `stop_mcp_container`**  
Don't delegate removal to Docker. After `docker stop`, call `docker rm` explicitly.
This eliminates the race entirely because there is now only one actor doing removal.
Downside: if Brain3 crashes without calling `stop_mcp_container`, the container is
left stopped-but-present (not auto-cleaned). GC handles this correctly, so it is
acceptable.

**Option C — Retry with backoff**  
If GC sees "already in progress", sleep briefly and retry `docker rm`. Works, but
adds latency and complexity for an edge case.

## Recommended Fix: Option A

Option A is the right surgical fix. The "removal in progress" state is transient and
safe to ignore from GC's perspective — someone else is already doing what we want.
Treating it as success is semantically correct and keeps the fix localised to
`garbage_collect_managed_containers`.

Option B is worth considering as a follow-up to fully eliminate the race, but it
changes container lifecycle behaviour and belongs in a separate PR.

## Fix Location

`crates/platform/src/container/startup.rs` — `garbage_collect_managed_containers()`:

In the `port.remove(&id).await` match arm, extend the existing "No such container"
guard to also match "removal of container" and "already in progress":

```rust
Err(ContainerError::CommandFailed { ref stderr, .. })
    if stderr.contains("No such container")
        || stderr.contains("already in progress") =>
{
    tracing::debug!(
        installation_id,
        container = %container.name,
        "orphan container removal already underway — skipping"
    );
    continue;
}
```

No change needed to the `port.stop()` arm — Docker will not return "already in
progress" for a stop, only for a remove.

## Files to Change

| File | Change |
|------|--------|
| `crates/platform/src/container/startup.rs` | Extend remove error guard to also match "already in progress" |

## Test

Add a unit test in `startup.rs` that exercises `garbage_collect_managed_containers`
with a mock port whose `remove` returns
`CommandFailed { stderr: "removal of container foo is already in progress" }` and
asserts `Ok(())` is returned.
