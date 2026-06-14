# Docker Internal Network Isolation RCA and Fix Plan

> **For agentic workers:** Use this as a plain planning doc. No implementation has been done yet.

**Goal:** Document the local Docker failure mode seen with `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=true` and outline a small, practical fix plan.

**Architecture:** Treat this as a container-runtime compatibility issue, not an MCP app startup bug. The fix should first make Brain3 detect and explain the failure precisely. Any automatic fallback should be a separate, explicit choice after the clearer diagnosis lands.

**Tech Stack:** Rust, `tracing`, Docker CLI, Brain3 container startup flow

---

## RCA

### Symptom

With `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=false`, Brain3 starts the managed MCP container and reaches it on `127.0.0.1:8420`.

With `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=true`, Brain3 fails startup with:

```text
container 'brain3-mcp-vault-tools' did not become reachable on 127.0.0.1:8420 before timeout
```

### What Brain3 Does

- `crates/platform/src/container/startup.rs`
  - Builds a `ContainerConfig` that publishes `127.0.0.1:<host_port>` to the MCP container port.
- `crates/platform/src/container/docker.rs`
  - Recreates `brain3-mcp-net` with `docker network create --internal`.
  - Runs the container with both `--publish` and `--network brain3-mcp-net`.
- `crates/core/src/application/ensure_container.rs`
  - Waits for a TCP connect to the published host port.

### What Was Verified

- The isolated container stays running.
- Container logs show the Python MCP server starts cleanly and listens on `0.0.0.0:8420`.
- `HostConfig.PortBindings` contains the requested publish on `127.0.0.1:8420`.
- `NetworkSettings.Ports` is empty for the isolated container.
- `docker ps` shows `8420/tcp`, not `127.0.0.1:8420->8420/tcp`.
- `curl http://127.0.0.1:8420/mcp` fails from the host.

### Control Case

Running the same image, env, and mounts on Docker's normal bridge network works:

- `docker ps` shows `127.0.0.1:18420->8420/tcp`
- `docker port` reports the publish correctly
- `NetworkSettings.Ports` is populated
- `curl http://127.0.0.1:18420/mcp` returns `401 Unauthorized`, which is expected for a healthy server without the upstream secret

### Root Cause

On this local Docker runtime, attaching the container to an internal bridge network with `--network brain3-mcp-net` causes Docker to keep the requested port binding in config but not activate the host-side publish.

In other words:

- Brain3 requests `--publish 127.0.0.1:8420:8420`
- Docker accepts that request in `HostConfig.PortBindings`
- but the publish never becomes live in `NetworkSettings.Ports`

The MCP app is healthy inside the container. The failure is specifically the missing host publish when Docker runs the container on the recreated internal network.

### What This Is Not

- Not an MCP app crash
- Not an upstream secret mount problem
- Not a vault mount problem
- Not just a short startup timeout

The port never becomes reachable even after the container has fully started.

## Recommended Fix Direction

Do not try to "force" Docker internal networking to work on this runtime. Treat this as a compatibility failure and make Brain3 detect it clearly.

That means the first fix should be better diagnosis and operator guidance, not a silent security downgrade.

## Fix Plan

### Task 1: Make startup failures distinguish app boot failures from Docker publish failures

**Files:**
- Modify: `crates/core/src/ports/container.rs`
- Modify: `crates/core/src/application/ensure_container.rs`
- Modify: `crates/platform/src/container/docker.rs`

- [ ] Add a small public container-port diagnostic hook that can report startup-relevant state for a running container.
- [ ] On startup timeout, collect:
  - container running state
  - requested port bindings
  - active published ports
  - network mode
- [ ] If the container is still running but active published ports are empty, return a specific startup error instead of the generic "did not become reachable" timeout.

### Task 2: Log the Docker mismatch directly

**Files:**
- Modify: `crates/platform/src/container/docker.rs`
- Modify: `crates/core/src/application/ensure_container.rs`

- [ ] Add clear error logging that shows the mismatch between:
  - requested `HostConfig.PortBindings`
  - actual `NetworkSettings.Ports`
- [ ] Include the network mode in the log output so the failure is obviously tied to `brain3-mcp-net`.

### Task 3: Make the operator action explicit

**Files:**
- Modify: `crates/core/src/application/ensure_container.rs`
- Modify: `README.MD`

- [ ] If this exact mismatch is detected, return an actionable error message that says Docker internal networking on this runtime did not activate the published host port.
- [ ] Tell the operator to set `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=false` as the compatibility fallback.
- [ ] Keep the existing secure default of `true`.

### Task 4: Add one focused regression test at the public API level

**Files:**
- Modify: `crates/core/src/application/ensure_container.rs`

- [ ] Add one test that covers the public behavior:
  - container remains running
  - startup verification times out on the host port
  - diagnostics show no active published ports
  - Brain3 returns the new specific compatibility error

### Task 5: Decide separately whether auto-fallback is acceptable

**Files:**
- None in the first pass

- [ ] After the clearer diagnosis lands, decide whether Brain3 should optionally retry once without isolation.
- [ ] Do not bundle auto-fallback into the first fix, because it changes the security posture from the operator's requested setting.

## Recommendation

Implement Tasks 1 through 4 first.

That gives:

- correct RCA in logs
- a precise user-facing error
- a documented workaround
- no silent downgrade from the requested isolation mode
