Problem
When `network_isolated=true`, Docker silently ignores `--publish` (by design for `--internal` networks), so `http://127.0.0.1:<host_port>` is unreachable. The startup probe and the reverse proxy both target this dead address.

## Plan

**7 file changes, no new files:**

### 1. `crates/core/src/ports/container.rs`
Add `get_container_ip(&self, id: &ContainerId) -> Result<Option<String>, ContainerError>` to the `ContainerPort` trait.

### 2. `crates/platform/src/container/docker.rs`
- Implement `get_container_ip` via `docker inspect --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}'`
- Skip `--publish` args when `config.network_isolated == true` (they're silently ignored anyway, so better to be explicit)

### 3. `crates/platform/src/container/macos_container.rs`
Same two changes, using the `container` CLI instead of `docker`.

### 4. `crates/core/src/application/ensure_container.rs`
- Change `ensure()` return type to `Result<(ContainerId, Option<String>), ContainerError>` — second element is the container's internal IP when isolated
- After `run()`, call `get_container_ip()` only when `runtime_config.network_isolated == true`
- Pass the IP into `verify_startup` — when isolated, probe `ip:container_port` instead of `host_address:host_port`
- Update the mock in tests to add `get_container_ip` (returns `None` by default, keeping all existing tests green)

### 5. `crates/platform/src/container/startup.rs`
Change return type to `Result<Option<String>, ContainerError>`, extract and return the container IP from `ensure()`.

### 6. `crates/platform/src/runtime/bootstrap.rs`
When `ensure_mcp_container` returns `Some(container_ip)`: clone the `GatewayConfig`, set `mcp_reverse_proxy.mcp_upstream_url = "http://<ip>:<container_port>"`, and store the updated `Arc<GatewayConfig>` in `RuntimeBootstrap.config`. This way all downstream code reading `runtime.config` gets the correct URL automatically.

### 7. `apps/gateway/src/main.rs` (`run_cli_mode`)
Change the `run_gateway_server_until` call from `config` to `Arc::clone(&runtime.config)` so it picks up the updated URL (the TUI path via `spawn_gateway_server` already uses `runtime.config`).

---

**Key design decisions:**
- No new function parameters propagated — the corrected URL flows through the existing `RuntimeBootstrap.config` field
- Port mappings remain in `ContainerConfig` as metadata so the readiness probe knows which container port to check; we just skip the `--publish` CLI flag when isolated
- Fallback: if `get_container_ip` returns `None` when isolated (unexpected), startup probe falls back to `host_address:host_port` — which will timeout and surface a clear error rather than silently serving a dead proxy

Want me to proceed with implementing this?