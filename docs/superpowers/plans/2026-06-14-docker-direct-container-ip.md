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


## Testing issues

- on macos, docker doesnt work in this mode, due to VM being in the middle.

on linux, it almost works, but:

The current RCA is:

`network_isolated=true` changed the upstream from `127.0.0.1:<host_port>` to `http://<container_ip>:8420`, and the gateway proxy already strips the incoming `Host` header before forwarding. That means `reqwest` synthesizes `Host: <container_ip>:8420`. Inside the Python MCP server, DNS-rebinding protection only allows `127.0.0.1:*`, `localhost:*`, and `[::1]:*` by default, so `Host: 172.18.0.2:8420` is rejected with `421 Misdirected Request`. Your existing container log already shows that exact failure. One extra important detail: the Rust-managed container startup path does not set `B3_VAULT_MCP_ALLOWED_HOSTS` at all, so the container stays on the localhost-only ACL. Also, passing the public domain alone would not fix this as long as the forwarded `Host` is still derived from the container IP.

I added diagnostic logs on both sides so you can prove that path end to end without changing behavior:
- Host/container startup config in [crates/platform/src/container/startup.rs](/Users/tleyden/Development/brain3_workspace2/crates/platform/src/container/startup.rs:86) and startup probe target in [crates/core/src/application/ensure_container.rs](/Users/tleyden/Development/brain3_workspace2/crates/core/src/application/ensure_container.rs:103)
- Proxy forwarding details, including original `Host`, stripped/forwarded `Host`, and upstream authority, in [crates/core/src/application/proxy_mcp.rs](/Users/tleyden/Development/brain3_workspace2/crates/core/src/application/proxy_mcp.rs:152)
- The actual `reqwest` send path, including the fact that it derives `Host` from the URL when no explicit `Host` is forwarded, in [crates/platform/src/mcp_proxy/reqwest_proxy.rs](/Users/tleyden/Development/brain3_workspace2/crates/platform/src/mcp_proxy/reqwest_proxy.rs:27)
- Container-side startup ACL logging plus per-request `/mcp` request/response host logging in [brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py](/Users/tleyden/Development/brain3_workspace2/brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py:54)

Verification: `cargo check` passed, and `python3 -m py_compile brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py` passed.

When you rerun, the confirming sequence should be:
1. Host logs show `allowed_hosts_env=None` and isolated upstream targeting `<container-ip>:8420`.
2. Proxy logs show original public/localhost host, but `forwarded_host_header=None`.
3. Reqwest logs show it derives upstream `Host` from `<container-ip>:8420`.
4. Container logs show `allowed_hosts=['127.0.0.1:*', 'localhost:*', '[::1]:*']` and inbound `host=<container-ip>:8420`, followed by `421`.