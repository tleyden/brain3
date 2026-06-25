# Plan: Remove redundant docker rm from GC; poll exists() for Docker runtime

## Root Cause

Docker containers are always started with `--rm`:

```rust
// crates/platform/src/container/startup.rs:211
remove_on_exit: matches!(startup.runtime, ContainerRuntime::Docker),
```

`docker stop` returns as soon as the container process exits. `--rm` then auto-removes
the container asynchronously. GC immediately calls `docker rm` after `docker stop`,
which races against `--rm` and gets:

```
Error response from daemon: removal of container brain3-mcp-vault-tools 
is already in progress
```

The explicit `docker rm` in GC is redundant for Docker — `--rm` already handles it —
and the combination causes the race.

macOS native containers do NOT use `--rm` (`remove_on_exit: false`), so they still
need an explicit remove call.

## Fix

**Docker runtime:** after `docker stop`, poll `port.exists()` until false. `--rm`
will remove the container; we just wait for it. Never call `port.remove()`.

**macOS runtime:** no `--rm`, so call `port.remove()` explicitly as before.

This requires threading `ContainerRuntime` into `garbage_collect_managed_containers`
so it knows which path to take.

## Code Change

**File:** `crates/platform/src/container/startup.rs`

1. Add `runtime: ContainerRuntime` parameter to `garbage_collect_managed_containers`
   and pass it from `maybe_handle_managed_container_orphans` (which has
   `startup.runtime`).

2. Add `wait_for_container_gone` helper (polls `port.exists()` until false):

```rust
async fn wait_for_container_gone(
    port: &dyn ContainerPort,
    id: &ContainerId,
    installation_id: &str,
    poll_interval: Duration,
    timeout: Duration,
) -> Result<(), ContainerError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut attempt = 0u32;
    loop {
        if !port.exists(id).await? {
            tracing::debug!(
                installation_id,
                container = %id.0,
                attempts = attempt,
                "container removed by Docker --rm"
            );
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ContainerError::Other(format!(
                "timed out waiting for container '{}' to be removed by Docker --rm",
                id.0
            )));
        }
        attempt += 1;
        tokio::time::sleep(poll_interval).await;
    }
}
```

3. In `garbage_collect_managed_containers`, replace the stop+remove block with
   runtime-aware logic:

```rust
if container.running {
    match port.stop(&id).await {
        Ok(()) => {}
        Err(ContainerError::CommandFailed { ref stderr, .. })
            if stderr.contains("No such container") =>
        {
            continue; // already gone
        }
        Err(error) => return Err(...),
    }
}

match runtime {
    ContainerRuntime::Docker => {
        // --rm handles removal; just wait for it to finish
        wait_for_container_gone(
            port, &id, installation_id,
            Duration::from_millis(200),
            Duration::from_secs(5),
        ).await?;
    }
    ContainerRuntime::MacOSContainer => {
        // no --rm, explicit removal needed
        match port.remove(&id).await {
            Ok(()) => {}
            Err(ContainerError::CommandFailed { ref stderr, .. })
                if stderr.contains("No such container") => { continue; }
            Err(error) => return Err(...),
        }
    }
}
tracing::info!(installation_id, container = %container.name,
    "removed managed orphan MCP container");
```

## Tokio `time` Feature

Add `time` to tokio features in `crates/platform/Cargo.toml` if not already present:

```toml
tokio = { version = "1", features = ["sync", "process", "fs", "time"] }
```

## Files Changed

| File | Change |
|------|--------|
| `crates/platform/src/container/startup.rs` | Thread runtime through GC, add `wait_for_container_gone`, split stop/remove logic by runtime |
| `crates/platform/Cargo.toml` | Add `time` to tokio features if missing |

## Tests

1. **`docker_gc_polls_after_stop_no_explicit_rm`** — Docker runtime, mock port: `stop`
   returns `Ok`, `exists` returns `true` once then `false`, `remove` never called.
   Assert `Ok(())`.

2. **`docker_gc_timeout_waiting_for_rm`** — Docker runtime, mock port: `stop` returns
   `Ok`, `exists` always `true`. Use short timeout. Assert `Err` with "timed out".

3. **`macos_gc_calls_explicit_remove`** — macOS runtime, mock port: `stop` returns
   `Ok`, `remove` returns `Ok`. Assert `remove` called once, `exists` never called.
