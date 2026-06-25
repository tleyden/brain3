# Plan: Greptile/CodeRabbit Review Followup (PR #120)

## Relevant Code Issues

### 1. Already-stopped Docker containers should also poll, never call docker rm (CodeRabbit Major — revised)

**File:** `crates/platform/src/container/startup.rs` ~line 314

**Problem:** Docker containers are always started with `--rm`. If a container is
stopped but still visible to `list_managed_containers`, it means `--rm` is already
in progress — the same situation as calling `docker stop` ourselves. We should poll
`exists()` in both cases. The current code does poll for running containers but falls
back to explicit `docker rm` on timeout, which contradicts the `--rm` invariant.
We should never call `docker rm` for Docker runtime.

**Fix:** For Docker runtime, always poll `wait_for_container_gone` regardless of
whether the container was running or already stopped. Remove the timeout fallback
to explicit `docker rm` in the Docker path entirely. If the container is still
present after 5s something is genuinely wrong with the Docker daemon — return an
error rather than racing with an ongoing `--rm`.

```rust
match runtime {
    ContainerRuntime::Docker => {
        // --rm always handles removal (either triggered by our stop above,
        // or already in progress for a stopped orphan). Poll until gone.
        let gone = wait_for_container_gone(...).await?;
        if !gone {
            return Err(ContainerError::Other(format!(
                "timed out waiting for Docker to remove container '{}' via --rm; \
                 Docker daemon may be unhealthy",
                container.name
            )));
        }
        // success — continue to next container
        continue;
    }
    ContainerRuntime::MacOSContainer => {
        // no --rm; fall through to explicit remove() below
    }
}
```

This also removes the now-dead `gc_docker_falls_back_to_explicit_rm_when_poll_times_out`
test (the fallback no longer exists) and replaces it with
`gc_docker_already_stopped_also_polls_not_rm` — stopped container, Docker runtime,
`exists()` returns false immediately, `remove()` never called.

### 2. Dead code in timeout fallback test (Greptile P2)

**File:** `crates/platform/src/container/startup.rs` ~line 712

**Problem:** `gc_docker_falls_back_to_explicit_rm_when_poll_times_out` sets
`managed_containers` in `MockState` but then bypasses `list_managed_containers`
by calling `garbage_collect_managed_containers` directly with a hand-built
`containers` vec. The `managed_containers` field is never read.

**Fix:** Remove the `managed_containers` field from that test's `MockState`.

---

## Not Relevant

- **Three comments about stale plan docs** (`-removal-in-progress-race.md`,
  `-removal-poll-wait.md`, `-poll-after-stop.md`): These are historical planning
  documents describing earlier approaches that were superseded. They don't affect
  production behavior and don't need to be updated.

## Files to Change

| File | Change |
|------|--------|
| `crates/platform/src/container/startup.rs` | Track `stopped_by_gc`, branch polling on it; add test for already-stopped Docker case; remove dead `managed_containers` from timeout test |
