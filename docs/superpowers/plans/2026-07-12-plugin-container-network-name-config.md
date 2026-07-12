# Plugin MCP Container: stop hardcoding the Docker network name

## Problem

`crates/platform/src/container/startup.rs:31` has:

```rust
const DEFAULT_PLUGIN_MCP_NETWORK_NAME: &str = "brain3-mcp-net";
```

used unconditionally in `build_plugin_container_config` (line 388) for every
Plugin MCP Container. This value is a compile-time constant with no override.

Meanwhile, the *main* vault-tools container's network name is already
user-configurable:

- `crates/core/src/domain/setup.rs:11` — `DEFAULT_CONTAINER_NETWORK_NAME =
  "brain3-mcp-net"` (same default value, but overridable)
- Persisted to the env file as `B3_CONTAINER_NETWORK_NAME`
  (`crates/platform/src/setup/env_writer.rs`, read back in
  `crates/platform/src/config/env_file.rs:435`)
- Surfaces as `ContainerStartupConfig.network_name`
  (`crates/core/src/domain/model.rs:149`), part of `GatewayConfig.container`

So if a user's env file sets `B3_CONTAINER_NETWORK_NAME=brain3-dev-mcp-net`,
the main container joins `brain3-dev-mcp-net`, but every Plugin MCP Container
still joins the hardcoded `brain3-mcp-net` — a silent network mismatch, since
Docker's embedded DNS won't resolve containers across networks. This is the
same failure class already fixed once for `fluensy_learn` / `local-supabase-rest-1`.

## Decision: no new config needed

Plugin containers must share a network with the main container to resolve
each other by container name. The correct fix is to make plugin containers
follow the *same already-configurable* network name the main container uses
— not to add a second, independent config knob that could drift from the
first and reintroduce this exact bug.

## Change

Thread `network_name` from `ContainerStartupConfig` (the main container's
config) down into Plugin MCP Container startup, replacing the hardcoded
constant. Fall back to the existing default only when there's no main
container configured at all (`config.container == None` — plugin containers
still start in this case per `allows_gateway_start()`).

### 1. `crates/platform/src/runtime/bootstrap.rs`

- `start_plugin_mcp_containers` (line 369): add a `network_name: &str`
  parameter.
- At the call site (line 306-312), pass the network name from `config`:
  ```rust
  let network_name = config
      .container
      .as_ref()
      .map(|c| c.network_name.as_str())
      .unwrap_or(DEFAULT_PLUGIN_MCP_NETWORK_NAME); // fallback, see below
  ```

### 2. `crates/platform/src/container/startup.rs`

- `ensure_plugin_mcp_container` (line 107): add a `network_name: &str`
  parameter, pass through to `build_plugin_container_config`.
- `build_plugin_container_config` (line 342): add a `network_name: &str`
  parameter; use it instead of `DEFAULT_PLUGIN_MCP_NETWORK_NAME` at line 388.
- Keep `DEFAULT_PLUGIN_MCP_NETWORK_NAME` as the fallback constant for the
  "no main container configured" case, OR reuse
  `brain3_core::domain::setup::DEFAULT_CONTAINER_NETWORK_NAME` directly and
  delete the local constant (they're identical today — one source of truth
  is better). Leaning toward deleting the local constant.

### 3. Tests to update

- `startup.rs` test `build_plugin_container_config_adds_plugin_role_labels_and_mounts`
  (line 833-834) and `..._omits_secret_mount_for_no_auth` (line 894-898):
  pass an explicit network name arg.
- `startup.rs` line 848: `assert_eq!(config.network_name,
  DEFAULT_PLUGIN_MCP_NETWORK_NAME)` — update to assert against whatever
  network name the test passes in.
- Add a test asserting that when the main container is configured with a
  non-default network name, the plugin container config uses that same name
  (this is the regression test for the bug we just hit).
- Add/keep a test for the fallback path (no main container configured).

## Non-goals

- Not adding a distinct env var like `B3_PLUGIN_MCP_NETWORK_NAME`. Two knobs
  for one Docker network is the bug, not the fix.
- Not changing `DEFAULT_CONTAINER_NETWORK_NAME`'s default value.
- Not touching `brain3-dev-mcp-vault-tools`'s separate network
  (`brain3-dev-mcp-net`), which is unrelated (that's the vault-tools dev
  container, a different container entirely).

## Verification

- `cargo test -p brain3 --no-run` then `cargo test` (per AGENTS.MD).
- Manually confirm (as already done for this bug): start with a non-default
  `B3_CONTAINER_NETWORK_NAME` in the dev env file, start a plugin container,
  confirm `docker inspect <plugin-container>` shows it on the same network
  as the main container, and that it can resolve a sibling container by name.
