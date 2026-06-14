# MCP Container Network Isolation Publish-Port Fix Implementation Plan

> **For agentic workers:** This plan supersedes the fix direction in `docs/superpowers/plans/2026-06-14-docker-internal-network-isolation-rca-and-fix-plan.md` while keeping that RCA document as the record of the failure mode. No implementation has been done yet.

**Goal:** Keep `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=true` as the default and preserve its meaning, but stop depending on host loopback port publishing when the managed MCP container runs on an internal-only network.

**Architecture:** When isolation is enabled, Brain3 should keep the container on the internal runtime network and switch gateway-to-container traffic to a Unix domain socket instead of `127.0.0.1:<host_port>`. When isolation is disabled, Brain3 should keep the current loopback TCP publish path for compatibility. Do not add iptables workarounds, direct container-IP dialing, or silent fallback from `true` to `false`.

**Tech Stack:** Rust, `tracing`, Docker CLI, Apple `container` CLI, Brain3 gateway bootstrap, Brain3 container startup flow, Python FastMCP server

---

## Decision Summary

- Keep `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION` as the only switch that decides whether Brain3 requests internal-only networking for the managed MCP container.
- Keep the default at `true`.
- When `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=true`, use a Unix domain socket for gateway-to-MCP traffic and stop relying on `--publish 127.0.0.1:<host_port>:<container_port>`.
- When `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=false`, keep the current published-loopback TCP path unchanged.
- Preserve the current security posture: no automatic downgrade to non-isolated mode, no public-client OAuth changes, no broadening of access policy.

## Why This Fix Direction Is Better

- The RCA already shows that the broken part is Docker host-side publish activation on an internal network, not the MCP app itself.
- Dialing the container IP directly would couple Brain3 to runtime-specific inspect output and network topology details on both Docker and macOS.
- A Unix socket is local-only, avoids Docker NAT/publish behavior entirely, and fits the "treat the container as untrusted" model better than TCP loopback.
- This keeps the security intent of internal-only networking instead of treating `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=true` as a best-effort hint.

## Non-Goals

- Do not add iptables or host firewall manipulation.
- Do not add container-IP discovery as the primary transport.
- Do not add an automatic retry with `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=false`.
- Do not remove `B3_CONTAINER_HOST_PORT` yet; it still matters for the compatibility path when isolation is disabled.

## File and Responsibility Map

- `crates/core/src/domain/model.rs`
  - Add an explicit upstream transport model and any runtime-neutral socket mapping types.
- `crates/platform/src/config/env_file.rs`
  - Derive the managed MCP upstream transport from container startup settings instead of always hard-coding `http://127.0.0.1:<host_port>`.
- `crates/core/src/application/ensure_container.rs`
  - Verify readiness using the selected transport instead of assuming TCP on a published host port.
- `crates/core/src/ports/container.rs`
  - Expose any new runtime-neutral readiness or socket inspection hooks needed by startup verification.
- `crates/platform/src/container/startup.rs`
  - Build a `ContainerConfig` that uses a Unix socket path in isolated mode and TCP publish only in non-isolated mode.
- `crates/platform/src/container/docker.rs`
  - Keep internal network creation for isolated mode, stop publishing a host TCP port in that mode, and mount a socket path the host and container can share.
- `crates/platform/src/container/macos_container.rs`
  - Keep internal network creation for isolated mode and switch isolated startup to socket-based exposure using `--publish-socket` or the closest supported equivalent confirmed during implementation.
- `crates/core/src/application/proxy_mcp.rs`
  - Stop assuming every upstream is just a base URL string.
- `crates/core/src/ports/mcp_proxy.rs`
  - Keep the public proxy contract transport-agnostic.
- `crates/platform/src/mcp_proxy/reqwest_proxy.rs`
  - Remain the adapter for URL-based upstreams.
- `crates/platform/src/mcp_proxy/mod.rs`
  - Export the new Unix-socket proxy adapter.
- `crates/platform/src/mcp_proxy/unix_socket_proxy.rs`
  - New adapter for forwarding MCP HTTP traffic over a Unix domain socket.
- `apps/gateway/src/server.rs`
  - Select the correct proxy adapter without bloating `apps/gateway/src/main.rs`.
- `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/config.py`
  - Add an optional socket-path setting for the MCP server.
- `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py`
  - Teach the MCP server entrypoint to serve over a Unix domain socket in isolated mode.
- `brain3-mcp-vault-tools/tests/test_server_startup.py`
  - Add one public startup-path test for socket mode.
- `crates/platform/tests/oauth_integration.rs`
  - Update integration coverage for the new upstream transport behavior.
- `.env.template`
  - Update comments so isolated mode is documented as socket-based rather than loopback-publish-based.
- `README.md`
  - Document the new transport behavior and compatibility story.

## Task 1: Model the Upstream Transport Explicitly

**Files:**
- Modify: `crates/core/src/domain/model.rs`
- Modify: `crates/platform/src/config/env_file.rs`
- Modify: `apps/gateway/src/server.rs`
- Modify: `crates/core/src/application/proxy_mcp.rs`

- [ ] Replace the implicit "managed upstream always means `http://127.0.0.1:<host_port>`" assumption with an explicit transport model.
- [ ] Keep explicit `B3_OAUTH2_GATEWAY_MCP_UPSTREAM_URL` support for externally managed upstreams, but make managed-container defaults runtime-aware.
- [ ] Derive the default managed upstream like this:
  - isolated managed container: Unix socket transport
  - non-isolated managed container: existing loopback TCP transport
  - no managed container configured: existing explicit/default URL behavior
- [ ] Keep logging clear by emitting either `upstream_url` or `upstream_socket_path` instead of forcing everything through a URL-shaped field.

## Task 2: Add a Runtime-Neutral Socket Mapping to Managed Container Startup

**Files:**
- Modify: `crates/core/src/domain/model.rs`
- Modify: `crates/platform/src/container/startup.rs`
- Modify: `crates/platform/src/config/env_file.rs`

- [ ] Extend the container startup/domain model so isolated mode can describe a Unix socket endpoint without smuggling it through `port_mappings`.
- [ ] Add a dedicated writable runtime directory for the managed MCP socket on the host, separate from the existing read-only upstream-secret mount.
- [ ] Use a stable in-container socket path such as `/run/brain3-runtime/mcp.sock` and a deterministic host socket path under a Brain3-owned temp/runtime directory.
- [ ] Keep `B3_CONTAINER_HOST_PORT` in the model for non-isolated mode; do not remove it from config or setup flows in this pass.
- [ ] Track two distinct path values in the Rust adapter: the **host-side socket directory** (used in the volume mount, e.g. `--volume /host/brain3-runtime:/run/brain3-runtime`) and the **in-container socket path** (e.g. `/run/brain3-runtime/mcp.sock`). The in-container path is what gets passed as the `B3_VAULT_MCP_UNIX_SOCKET` env var to the container. These are distinct values derived from the same base configuration and must not be conflated.
- [ ] Create the host-side socket directory before launching the container and set permissions so the container's runtime UID can write to it.

## Task 3: Teach the Python MCP Server to Serve on a Unix Socket

**Files:**
- Modify: `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/config.py`
- Modify: `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py`
- Modify: `brain3-mcp-vault-tools/tests/test_server_startup.py`
- Modify: `brain3-mcp-vault-tools/pyproject.toml` only if an additional dependency is truly required

- [ ] Add an optional env var for a Unix socket path, for example `B3_VAULT_MCP_UNIX_SOCKET`. The value must be the **in-container** socket path (e.g. `/run/brain3-runtime/mcp.sock`), not the host path.
- [ ] When the socket env var is set, start the existing Streamable HTTP app on that socket instead of binding a TCP host/port.
- [ ] If `B3_VAULT_MCP_UNIX_SOCKET` is set and the socket path cannot be bound (directory missing, permission denied, or any other error), the server **must exit immediately with a non-zero exit code and a clear error message**. Do not fall back to TCP — TCP is not published in isolated mode and a silent TCP fallback produces a container that appears running but is completely unreachable.
- [ ] Before binding the socket, unlink any existing socket file at that path to avoid "address already in use" on container restart after a crash or unclean shutdown.
- [ ] Preserve the current upstream-secret middleware and transport-security behavior in both modes.
- [ ] Prefer a native ASGI/uvicorn Unix-socket startup path over adding `socat` or a second long-running bridge process.
- [ ] Treat a bridge process as contingency only if FastMCP cannot be made to serve the existing app over a Unix socket cleanly.

## Task 4: Change Container Runtime Adapters to Use Sockets in Isolated Mode

**Files:**
- Modify: `crates/platform/src/container/startup.rs`
- Modify: `crates/platform/src/container/docker.rs`
- Modify: `crates/platform/src/container/macos_container.rs`

- [ ] Docker isolated mode:
  - keep `docker network create --internal` behavior
  - stop passing `--publish`
  - mount the host-side socket directory read-write into the container (e.g. `--volume /host/brain3-runtime:/run/brain3-runtime`)
  - pass `B3_VAULT_MCP_UNIX_SOCKET=<in-container socket path>` as a container env var (e.g. `--env B3_VAULT_MCP_UNIX_SOCKET=/run/brain3-runtime/mcp.sock`); the value must be the in-container path, not the host path
- [ ] Docker non-isolated mode:
  - keep the current `--publish 127.0.0.1:<host_port>:<container_port>` path unchanged
- [ ] macOS isolated mode:
  - keep internal-network creation
  - replace isolated TCP publish with socket-based exposure using `--publish-socket` or the closest supported equivalent verified during implementation
  - **before starting Task 4 for macOS**, confirm whether the Apple `container` CLI supports Unix socket sharing (volume mounts of a host directory containing a socket file); if it does not, define the contingency inside this task before writing any code — do not discover this as a blocker mid-implementation
  - keep the same public Brain3 transport contract even if the macOS adapter needs a slightly different CLI wiring than Docker
- [ ] macOS non-isolated mode:
  - keep the current `--publish` path unchanged

## Task 5: Make Startup Verification Transport-Aware

**Files:**
- Modify: `crates/core/src/application/ensure_container.rs`
- Modify: `crates/core/src/ports/container.rs`
- Modify: `crates/platform/src/container/docker.rs`
- Modify: `crates/platform/src/container/macos_container.rs`

- [ ] Stop assuming readiness always means a TCP connect to `127.0.0.1:<host_port>`.
- [ ] In isolated mode, verify the container using the selected socket transport.
- [ ] Prefer an actual HTTP readiness probe over the Unix socket if practical; otherwise treat socket creation plus a running container as the minimum first-pass readiness gate.
- [ ] Keep the existing TCP readiness probe for non-isolated mode.
- [ ] Return socket-specific startup errors so failures read like transport failures, not "port did not become reachable on 127.0.0.1".

## Task 6: Add a Unix-Socket MCP Proxy Adapter in the Gateway

**Files:**
- Modify: `crates/core/src/ports/mcp_proxy.rs`
- Modify: `crates/core/src/application/proxy_mcp.rs`
- Modify: `crates/platform/src/mcp_proxy/mod.rs`
- Modify: `crates/platform/src/mcp_proxy/reqwest_proxy.rs`
- Create: `crates/platform/src/mcp_proxy/unix_socket_proxy.rs`
- Modify: `apps/gateway/src/server.rs`

- [ ] Keep `ProxyMcpUseCase` focused on request validation, header filtering, and upstream request construction rather than transport details.
- [ ] Introduce a platform adapter that can forward HTTP requests over a Unix domain socket.
- [ ] Continue using `ReqwestMcpProxy` for URL-based upstreams.
- [ ] Select the proxy adapter in `apps/gateway/src/server.rs` so `apps/gateway/src/main.rs` stays lean.
- [ ] Make logs clearly identify whether upstream forwarding is going to a URL or to a local socket path.

## Task 7: Update Operator-Facing Config and Compatibility Guidance

**Files:**
- Modify: `.env.template`
- Modify: `README.md`

- [ ] Update the `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION` comments to say that isolated mode now uses a Unix socket between Brain3 and the managed MCP container.
- [ ] Update the `B3_CONTAINER_HOST_PORT` comments to make clear that this is the compatibility transport for non-isolated mode, not the primary path when isolation is enabled.
- [ ] Update the `B3_OAUTH2_GATEWAY_MCP_UPSTREAM_URL` comments to explain that managed-container defaults are automatic and explicit URL override is mainly for externally managed upstreams.
- [ ] Keep the secure default at `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=true`.

## Task 8: Add Focused Tests Only at Public Boundaries

**Files:**
- Modify: `crates/core/src/application/ensure_container.rs`
- Modify: `crates/platform/tests/oauth_integration.rs`
- Modify: `brain3-mcp-vault-tools/tests/test_server_startup.py`

- [ ] Add one public `EnsureContainerUseCase` test that covers isolated startup choosing the socket readiness path and returning a precise failure if the socket never becomes usable.
- [ ] Add one gateway/platform integration-style test that covers managed isolated mode selecting the Unix-socket upstream transport instead of `http://127.0.0.1:<host_port>`.
- [ ] Add one Python server startup test that covers the public socket-mode startup path.
- [ ] Do not add log snapshot tests or private-helper tests.

## Verification Checklist

- Docker + isolated mode:
  - container joins the internal network
  - no host TCP publish is required for MCP reachability
  - Brain3 gateway can reach the MCP server over the Unix socket
  - container still lacks outbound internet access
- Docker + non-isolated mode:
  - existing loopback published-port behavior still works
- macOS container + isolated mode:
  - socket-based local transport works without relying on host TCP publish
- Gateway behavior:
  - proxy still injects the upstream shared secret
  - user-facing failures mention socket transport when socket startup fails

## Recommendation

Implement Tasks 1 through 8 in order, with one constraint: keep the first pass narrowly focused on managed-container transport and startup verification. Do not bundle container-IP fallback, iptables rules, or automatic non-isolated retry into the same change.

If the Python FastMCP stack turns out to block native Unix-socket serving, the fallback inside this same plan should still preserve the public contract: Brain3 talks to the managed MCP container over a Unix socket in isolated mode. The implementation detail may become an internal bridge, but the product behavior and security posture should remain the same.
