# Plan: Poll-wait for in-progress Docker container removal during GC

## Problem

When Brain3 starts with `--gc-containers` immediately after a previous session exits,
`garbage_collect_managed_containers` sometimes calls `docker rm` on a container that
Docker is already auto-removing (due to `--rm` on the `docker run`). Docker returns:

```
Error response from daemon: removal of container brain3-mcp-vault-tools 
is already in progress
```

The RCA (see `2026-06-25-container-gc-removal-in-progress-race.md`) established that
simply treating this as success is safe, but the user prefers to keep `--rm` as a
garbage-collection backstop AND have GC wait until the removal actually finishes
before proceeding — giving a stronger guarantee that the container slot is free.

## Approach: Poll `port.exists()` Until Gone

When `docker rm` returns "already in progress", instead of swallowing the error, we
enter a tight poll loop calling `port.exists()` (which issues `docker container
inspect`) until it returns `false`. Once `exists()` returns `false` the container
slot is definitively free and GC can continue to the next container.

`port.exists()` is already implemented on both `DockerContainerAdapter` and
`MacOsContainerAdapter` via `docker container inspect` / `container inspect`.

## Poll Parameters

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Poll interval | 200 ms | Short enough to be responsive; Docker `--rm` removal typically completes in < 500 ms |
| Timeout | 10 s | Should never be hit in practice; signals a genuinely stuck Docker daemon |
| Timeout error | `ContainerError::Other("timed out waiting for container removal")` | Hard failure — something is wrong with Docker |

## Code Change

**File:** `crates/platform/src/container/startup.rs`

Add a helper function `wait_for_container_removal`:

```rust
/// Polls `port.exists()` until the container is gone or the timeout elapses.
async fn wait_for_container_removal(
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
                "container gone after polling for removal completion"
            );
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ContainerError::Other(format!(
                "timed out waiting for container '{}' removal to complete after {}s",
                id.0,
                timeout.as_secs()
            )));
        }
        attempt += 1;
        tracing::debug!(
            installation_id,
            container = %id.0,
            attempt,
            "container removal in progress, polling..."
        );
        tokio::time::sleep(poll_interval).await;
    }
}
```

In `garbage_collect_managed_containers`, change the `port.remove()` match arm to
call the helper when "already in progress" is detected:

```rust
match port.remove(&id).await {
    Ok(()) => {}
    Err(ContainerError::CommandFailed { ref stderr, .. })
        if stderr.contains("No such container") =>
    {
        tracing::debug!(
            installation_id,
            container = %container.name,
            "orphan container already gone during remove — skipping"
        );
        continue;
    }
    Err(ContainerError::CommandFailed { ref stderr, .. })
        if stderr.contains("already in progress") =>
    {
        tracing::info!(
            installation_id,
            container = %container.name,
            "Docker is already removing this container — waiting for completion"
        );
        wait_for_container_removal(
            port,
            &id,
            installation_id,
            Duration::from_millis(200),
            Duration::from_secs(10),
        )
        .await?;
        continue;
    }
    Err(error) => {
        return Err(ContainerError::Other(format!(
            "failed to remove managed orphan container '{}': {}",
            container.name,
            error.summary()
        )));
    }
}
```

## Imports Needed

`tokio::time::{Duration, Instant, sleep}` — `tokio` is already a dependency with the
`sync` and `process` features. Need to add `time` feature to
`crates/platform/Cargo.toml`:

```toml
tokio = { version = "1", features = ["sync", "process", "fs", "time"] }
```

(Check first — `time` may already be enabled transitively, but make it explicit.)

## Tests

Add to `startup.rs` `#[cfg(test)]` block:

1. **`removal_in_progress_polls_until_gone`** — mock port whose `remove` returns
   `CommandFailed { stderr: "removal of container foo is already in progress" }` and
   whose `exists` returns `true` twice then `false`. Assert `Ok(())` and that
   `exists` was called 3 times.

2. **`removal_in_progress_timeout`** — mock port whose `remove` returns "already in
   progress" and whose `exists` always returns `true`. Use a very short timeout (e.g.
   300 ms) and assert `Err` is returned containing "timed out".

## Files Changed

| File | Change |
|------|--------|
| `crates/platform/src/container/startup.rs` | Add `wait_for_container_removal`, extend `remove` match arm |
| `crates/platform/Cargo.toml` | Add `time` feature to `tokio` if not already present |
