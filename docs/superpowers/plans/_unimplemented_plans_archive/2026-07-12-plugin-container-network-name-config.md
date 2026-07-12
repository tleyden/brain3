# Plugin MCP Container: per-plugin isolated network instead of a shared hardcoded one

## Problem

`crates/platform/src/container/startup.rs:31` has:

```rust
const DEFAULT_PLUGIN_MCP_NETWORK_NAME: &str = "brain3-mcp-net";
```

Every Plugin MCP Container, regardless of which plugin it is, joins this one
hardcoded network unconditionally (`build_plugin_container_config`, line 388).
Concretely this means every plugin container can currently reach every other
plugin container, since Docker's embedded DNS resolves any container name on
a shared network. That's an unwanted security exposure: e.g. a `fluensy_learn`
plugin and its Postgres sidecar should not be reachable from an unrelated
plugin container, and an unrelated plugin shouldn't be able to reach
`fluensy_learn`'s Postgres either.

## Decision

Each plugin defines its **own** network name in `brain3.yaml`. There is no
default/shared plugin network anymore. Two plugins that legitimately want to
share a network (e.g. a plugin's app container + its own Postgres container)
just use the same network name in their respective config — Brain3 doesn't
manage the Postgres container, so the user creates/joins it to that network
themselves, or lets Brain3 create it when the plugin container starts first.

Network creation stays **auto-create-if-missing**, matching the existing
behavior for the main container's network
(`crates/core/src/application/ensure_container.rs:86-99`,
`ensure_internal_network` on the `ContainerPort` adapters). No new
"require existing / fail with instructions" policy — that would be
inconsistent with how the rest of the codebase already handles network setup,
and it's not needed: an `--internal` Docker/`container` network with no
special config is safe to create automatically per plugin, since being
`--internal` already blocks it from reaching anything outside itself (no
default route out, no route to `brain3-mcp-net` or other plugin networks).

## Change

### 1. `crates/core/src/domain/model.rs`

Add a required field to `PluginMcpContainerConfig`:

```rust
pub struct PluginMcpContainerConfig {
    pub name: String,
    pub runtime: ContainerRuntime,
    pub image: String,
    pub container_port: u16,
    pub host_port: Option<u16>,
    pub host_directory: PathBuf,
    pub container_directory: PathBuf,
    pub network_name: String,   // new
    pub auth: PluginMcpContainerAuth,
}
```

### 2. `crates/platform/src/config/brain3_yaml.rs`

- `RawPluginMcpContainerConfig`: add `network: Option<String>`.
- `validate_plugin_mcp_container`: make it required via `required_string`,
  same pattern as `platform`/`image`/`tag`. Docker/`container` network names
  allow hyphens and dots (unlike the existing container `name` field, which
  is restricted to `[a-z0-9_]+`), so this needs its own validator rather than
  reusing `validate_name`. Something like:
  ```rust
  fn validate_network_name(name: &str) -> Result<(), String> {
      let mut chars = name.chars();
      let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphanumeric());
      let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
      if first_ok && rest_ok {
          Ok(())
      } else {
          Err("network must start with a letter/digit and contain only letters, digits, '_', '.', '-'".to_string())
      }
  }
  ```
- Wire `network_name` into the constructed `PluginMcpContainerConfig`.

### 3. `crates/platform/src/container/startup.rs`

- `build_plugin_container_config` (line 342-406): replace
  `network_name: DEFAULT_PLUGIN_MCP_NETWORK_NAME.into()` with
  `network_name: plugin.network_name.clone()`.
- Delete the `DEFAULT_PLUGIN_MCP_NETWORK_NAME` constant (line 31) — no longer
  used anywhere; there is no fallback case since the field is required at
  config-load time.
- No changes needed in `ensure_plugin_mcp_container`,
  `EnsureContainerUseCase::ensure`, or either `ContainerPort` adapter
  (`docker.rs`, `macos_container.rs`) — the existing
  `ensure_internal_network` auto-create-if-missing / reuse-if-compatible /
  conflict-if-incompatible logic already works per network name and needs no
  change to support multiple distinct plugin networks.

### 4. Tests

- `brain3_yaml.rs`: update `valid_entry()` test helper and the raw YAML
  fixtures across all tests to include a `network:` field (currently 4 tests
  build YAML without one — `valid_multi_entry_file_loads_configs_with_defaults`,
  `missing_bearer_token_secret_file_drops_only_that_entry`,
  `duplicate_name_drops_later_duplicate`, `bad_name_charset_is_dropped`).
  Give the two plugins in the multi-entry test *different* network names to
  make the isolation intent explicit in the fixture.
  Add a test asserting a config missing `network` is dropped.
  Add a test asserting invalid network name charset is dropped (e.g. leading
  hyphen, space).
- `startup.rs`: `sample_plugin_config()` and the two
  `build_plugin_container_config_*` tests (line ~833-898) need a
  `network_name` on the fixture; update the assertion at line 848 to check
  against that fixture's network name instead of
  `DEFAULT_PLUGIN_MCP_NETWORK_NAME`.
  Add a test with two different `PluginMcpContainerConfig`s (different
  `network_name`) asserting `build_plugin_container_config` produces two
  different `ContainerConfig.network_name` values — this is the regression
  test for plugin network isolation.

### 5. `README.md` (Experimental: Plugin MCP Containers section, ~line 395-424)

- Add `network: fluensy-learn` to the example YAML.
- Document that `network` is required, is the Docker/`container` network the
  plugin container joins, is created automatically (internal-only, no
  outbound access) if it doesn't already exist, and that containers on
  different plugin networks cannot reach each other. Mention that a plugin
  wanting to talk to its own sidecar (e.g. Postgres) should put that sidecar
  on the same network name.

## Non-goals

- Not making network creation fail-closed ("must already exist"). Auto-create
  matches existing behavior and is simpler; the `--internal` flag is what
  actually provides the isolation guarantee, not who creates it.
- Not adding a mechanism for Brain3 to manage plugin sidecar containers
  (e.g. Postgres) — those remain the user's responsibility to start and
  attach to the matching network name.
- Not touching the main container's `brain3-mcp-net` /
  `DEFAULT_CONTAINER_NETWORK_NAME` default or its configurability
  (`B3_CONTAINER_NETWORK_NAME`) — unrelated to this change.

## Verification

- `cargo test -p brain3 --no-run` then `cargo test` (per AGENTS.MD).
- Manually: configure two plugins in `brain3.yaml` with different `network`
  values, start Brain3, and confirm via `docker network ls` / `docker
  inspect` that each plugin container is on its own network and that one
  plugin container cannot resolve/reach the other by container name.
