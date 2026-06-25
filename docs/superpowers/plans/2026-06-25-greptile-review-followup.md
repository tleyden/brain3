# Plan: Greptile/CodeRabbit Review Followup (PR #120)

## Relevant Code Issues

### 1. Only poll after GC actually called stop (CodeRabbit Major)

**File:** `crates/platform/src/container/startup.rs` ~line 314

**Problem:** The `ContainerRuntime::Docker` branch in `garbage_collect_managed_containers`
always calls `wait_for_container_gone` — even for containers that were already
stopped when GC ran. For a stopped Docker orphan, this GC pass never called
`docker stop`, so `--rm` was never triggered by us. There is nothing to poll for.
We sit waiting 5s before falling back to explicit `docker rm`, adding unnecessary
latency per stopped orphan.

**Fix:** Track whether this GC pass actually called `stop()`. Only poll if it did.
If the container was already stopped, skip polling and call `docker rm` immediately.

```rust
let stopped_by_gc = if container.running {
    // ... existing stop logic, returns true on success ...
    true
} else {
    false
};

match runtime {
    ContainerRuntime::Docker if stopped_by_gc => {
        // --rm was triggered by our stop; poll for it to finish
        let gone = wait_for_container_gone(...).await?;
        if gone { continue; }
        // fallback to explicit rm
    }
    ContainerRuntime::Docker => {
        // Container was already stopped; --rm should have fired at exit time
        // but clearly didn't (otherwise it wouldn't be listed). Remove immediately.
    }
    ContainerRuntime::MacOSContainer => {
        // always explicit rm
    }
}
// explicit docker rm / macos rm below
```

**Tests to add/update:**
- Rename `gc_docker_stops_running_and_waits_for_auto_removal` to make "running → polled" explicit
- Add `gc_docker_already_stopped_removes_immediately` — stopped container, Docker runtime,
  assert `exists()` never called, `remove()` called immediately

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
