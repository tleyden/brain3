# Plan: MCP Container Network Isolation (Finding 3.7)

**Date:** 2026-06-13  
**Security audit ref:** [security_audit_claude_sonnet_4_6.md — Finding 3.7](../security_audit_claude_sonnet_4_6.md)  
**Severity:** 🔴 HIGH

## Background

The MCP container currently joins Docker's default `bridge` network, which has a default route to the internet. The container only needs to accept inbound connections from the Brain3 gateway (`127.0.0.1:8420`) and access vault files via bind mount — it needs zero outbound internet connectivity. A supply-chain compromise of a Python dependency inside the container image gives an attacker a read-write handle on the vault and an unrestricted outbound exfiltration channel.

Both Docker and Apple's `container` CLI support `--internal` named networks, which remove the default route so containers cannot reach external hosts. Port mappings from the host (`-p 127.0.0.1:8420:8420`) continue to work because they are handled at the kernel/hypervisor level independently of the container's routing table.

Apple's `container network create --internal` is confirmed available:
```
container network create --internal <name>   # Restrict to host-only network
```

## Architecture decision

The domain model expresses **intent** (`network_isolated: bool`), not mechanism. Each adapter translates this into the appropriate runtime command. Graceful degradation: if network creation fails for any reason, Brain3 logs a warning and starts the container without isolation rather than refusing to launch.

## Steps

### Step 1 — Add `network_isolated` to `ContainerConfig`

**File:** `crates/core/src/domain/model.rs`

Add one field to `ContainerConfig`:

```rust
pub network_isolated: bool,
```

Default is `false` — preserves existing behaviour for any future construction sites.

---

### Step 2 — Add shared network name constant

**File:** `crates/platform/src/container/mod.rs` (or top of each adapter file)

```rust
pub(super) const MCP_NETWORK_NAME: &str = "brain3-mcp-net";
```

---

### Step 3 — Docker adapter: `ensure_internal_network` + wire into `run()`

**File:** `crates/platform/src/container/docker.rs`

Add a private async helper:

```rust
async fn ensure_internal_network(name: &str) -> Result<(), ContainerError> {
    match run_command("docker", &["network", "create", "--internal", name]).await {
        Ok(_) => Ok(()),
        Err(ContainerError::CommandFailed { ref stderr, .. })
            if stderr.contains("already exists") => Ok(()),
        Err(e) => Err(e),
    }
}
```

In `DockerContainerAdapter::run()`, after existing args are built and before the image name, add:

```rust
if config.network_isolated {
    match ensure_internal_network(MCP_NETWORK_NAME).await {
        Ok(()) => {
            args.push("--network".into());
            args.push(MCP_NETWORK_NAME.into());
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "⚠ Network isolation unavailable — MCP container will start without outbound restrictions"
            );
        }
    }
}
```

---

### Step 4 — macOS adapter: same pattern

**File:** `crates/platform/src/container/macos_container.rs`

Same structure as Step 3, using `container` instead of `docker`:

```rust
async fn ensure_internal_network(name: &str) -> Result<(), ContainerError> {
    match run_command("container", &["network", "create", "--internal", name]).await {
        Ok(_) => Ok(()),
        Err(ContainerError::CommandFailed { ref stderr, .. })
            if stderr.contains("already exists") => Ok(()),
        Err(e) => Err(e),
    }
}
```

Same match block in `MacOsContainerAdapter::run()` — push `--network` on success, warn and continue on error.

Add a `tracing::info!` before the match so logs are clear when isolation is being applied:

```rust
tracing::info!(network = MCP_NETWORK_NAME, "applying internal network isolation to MCP container");
```

**macOS notes:**
- `--internal` confirmed present in `container network create --help` (macOS 26)
- Relevant upstream issues: [#1037](https://github.com/apple/container/issues/1037) (network none too restrictive), [#1170](https://github.com/apple/container/discussions/1170) (internal networks confirmed), [#1320](https://github.com/apple/container/issues/1320) (host gateway still reachable on internal networks — desired for Brain3)

---

### Step 5 — Set `network_isolated: true` in startup

**File:** `crates/platform/src/container/startup.rs`

In the `ContainerConfig` struct literal, set:

```rust
network_isolated: true,
```

This applies to both `ContainerRuntime::Docker` and `ContainerRuntime::MacOSContainer`.

---

### Step 6 — Fix other `ContainerConfig` construction sites`

Grep for `ContainerConfig {` across the codebase. Any site not in `startup.rs` needs `network_isolated: false` added. Expected: no other sites exist today.

---

## Verification

After implementation, confirm isolation is working:

```bash
# From inside the container — should fail / time out
docker exec brain3-mcp-vault-tools curl --max-time 3 https://example.com

# From the host — must still succeed
curl http://127.0.0.1:8420/health
```

On macOS, replace `docker exec` with `container exec`.

---

## Graceful degradation behaviour

| Scenario | Outcome |
|---|---|
| Network created, port mapping works | Full isolation — container cannot reach internet |
| Network creation fails (unexpected error) | `⚠ Network isolation unavailable` warning in logs; container starts normally |
| Network created but port mapping broken (Issue #1037 on unusual macOS builds) | Container reported as `Failed` by existing startup health check; user sees logs |

---

## Files changed

| File | Change |
|---|---|
| `crates/core/src/domain/model.rs` | Add `network_isolated: bool` to `ContainerConfig` |
| `crates/platform/src/container/mod.rs` | Add `MCP_NETWORK_NAME` constant |
| `crates/platform/src/container/docker.rs` | Add `ensure_internal_network`, wire into `run()` |
| `crates/platform/src/container/macos_container.rs` | Add `ensure_internal_network`, wire into `run()` |
| `crates/platform/src/container/startup.rs` | Set `network_isolated: true` |

No new dependencies. No config variables. No breaking changes to the `ContainerPort` trait.
