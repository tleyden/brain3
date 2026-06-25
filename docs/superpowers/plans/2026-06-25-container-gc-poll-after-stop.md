# Plan: Poll exists() after docker stop before calling docker rm in GC

## Corrected Root Cause

`docker stop` returns as soon as the container process exits. The `--rm` flag then
triggers Docker to begin removing the container **asynchronously** — `docker stop`
does NOT wait for that removal to complete before returning to the caller.

GC immediately calls `docker rm` after `docker stop` returns. On Docker (where
`--rm` is set), Docker is already mid-removal, so `docker rm` gets:

```
Error response from daemon: removal of container brain3-mcp-vault-tools 
is already in progress
```

This happens **every time** GC stops a running Docker container — not just on rapid
restarts. The many-hours gap between sessions confirms the container was still
running (brain3 was killed before `stop_mcp_container` fired), so this is the
normal GC path, not an edge case.

## Current Code Flow

```
garbage_collect_managed_containers:
  docker stop brain3-mcp-vault-tools   ← returns when container exits
                                        ← --rm removal starts async here
  docker rm brain3-mcp-vault-tools     ← races against --rm, fails
```

## Fix: Poll exists() Between stop and rm

After `docker stop` succeeds, poll `port.exists()` for up to 5 seconds (200 ms
intervals). Two outcomes:

- **`exists()` returns false** (Docker's `--rm` finished removal): skip `docker rm`,
  continue to next container. This is the normal Docker path.
- **`exists()` still true after timeout** (runtime has no `--rm`, e.g. macOS
  containers): fall through and call `docker rm` as normal. This is the macOS path.

This avoids the race without needing to know whether the container was started with
`--rm`, and without skipping the explicit `docker rm` on runtimes that need it.

## Code Change

**File:** `crates/platform/src/container/startup.rs`

Add `wait_for_container_gone` helper:

```rust
/// After `docker stop`, polls `port.exists()` to let the Docker daemon finish
/// its own `--rm` removal before we attempt an explicit `docker rm`.
/// Returns `true` if the container disappeared on its own (skip `docker rm`),
/// `false` if it is still present after the timeout (proceed with `docker rm`).
async fn wait_for_container_gone(
    port: &dyn ContainerPort,
    id: &ContainerId,
    installation_id: &str,
    poll_interval: Duration,
    timeout: Duration,
) -> Result<bool, ContainerError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut attempt = 0u32;
    loop {
        if !port.exists(id).await? {
            tracing::debug!(
                installation_id,
                container = %id.0,
                attempts = attempt,
                "container removed by Docker --rm after stop"
            );
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::debug!(
                installation_id,
                container = %id.0,
                "container still present after polling — will call docker rm explicitly"
            );
            return Ok(false);
        }
        attempt += 1;
        tokio::time::sleep(poll_interval).await;
    }
}
```

In `garbage_collect_managed_containers`, call it after a successful stop:

```rust
if container.running {
    // ... existing stop logic ...
    match port.stop(&id).await {
        Ok(()) => {
            // Give Docker's --rm a chance to finish before we call docker rm.
            let already_gone = wait_for_container_gone(
                port,
                &id,
                installation_id,
                Duration::from_millis(200),
                Duration::from_secs(5),
            )
            .await?;
            if already_gone {
                tracing::info!(installation_id, container = %container.name,
                    "removed managed orphan MCP container");
                continue; // skip explicit docker rm
            }
        }
        Err(ContainerError::CommandFailed { ref stderr, .. })
            if stderr.contains("No such container") =>
        {
            // ... existing handling ...
            continue;
        }
        Err(error) => { /* ... existing error return ... */ }
    }
}
```

The `port.remove()` call below remains unchanged — it now only runs when
`wait_for_container_gone` timed out (i.e., the container didn't self-remove,
which is expected on macOS native containers).

## Tokio `time` Feature

Verify `crates/platform/Cargo.toml` includes `time` in the tokio feature list:

```toml
tokio = { version = "1", features = ["sync", "process", "fs", "time"] }
```

## Files Changed

| File | Change |
|------|--------|
| `crates/platform/src/container/startup.rs` | Add `wait_for_container_gone`, call it after successful stop |
| `crates/platform/Cargo.toml` | Add `time` to tokio features if missing |

## Tests

1. **`stop_triggers_rm_removal_skips_explicit_rm`** — mock port: `stop` returns `Ok`,
   `exists` returns `true` once then `false`. Assert `remove` is never called and
   result is `Ok(())`.

2. **`stop_no_auto_removal_falls_through_to_explicit_rm`** — mock port: `stop`
   returns `Ok`, `exists` always returns `true` (timeout path), `remove` returns
   `Ok`. Assert `remove` is called once and result is `Ok(())`.

## Supersedes

This plan supersedes `2026-06-25-container-gc-removal-poll-wait.md`, which proposed
polling only on the error path. Polling on the success path (after stop) is cleaner
— it handles the race proactively rather than reactively.
