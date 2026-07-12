# Plan: Per-plugin `network_isolation` flag in `brain3.yaml`

## Status

- Date: 2026-07-12
- Root cause reference: this session's RCA (see conversation) confirmed that
  `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=false` in `.env` only affects the
  primary vault MCP container (`crates/platform/src/config/env_file.rs:451`).
  Plugin MCP Containers configured in `brain3.yaml` always get
  `isolation_strategy = Some(...)` unconditionally
  (`crates/platform/src/container/startup.rs:370`,
  `plugin_isolation_strategy()` at line 407), with no opt-out. On macOS with
  `platform: docker`, the Docker adapter's existing choke-point guard
  (`crates/platform/src/container/docker.rs:159-178`,
  `validate_internal_network_support`) then unconditionally rejects the
  container, because it currently treats *any* isolated Docker config on
  macOS as unsupported — there's no way to reach it with isolation off.
- Related prior plans (already implemented, do not re-litigate or duplicate):
  - `docs/superpowers/plans/2026-07-12-macos-docker-plugin-internal-network-counter-plan.md`
  - `docs/superpowers/plans/2026-07-12-plugin-network-isolation-startup-choke-point-plan.md`
  - Both landed the current `ContainerPort::validate_internal_network_support`
    choke point that rejects isolated Docker configs on macOS. This plan does
    **not** touch that guard's logic — it just gives plugins a way to avoid
    triggering it, the same way `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=false`
    lets the primary container avoid it today.
  - `docs/superpowers/plans/2026-06-13-mcp-container-network-isolation-toggle.md`
    is the template this plan mirrors, scoped down: no TUI wizard work is
    needed because Plugin MCP Containers are config-file-only
    (README: "configured only by hand-editing `brain3.yaml`").
- This document is a plan only. No implementation changes are included yet.

## Goal

Add an optional per-plugin YAML field so a plugin like the user's
`fluensy_learn` can opt out of internal network isolation, mirroring
`B3_CONTAINER_INTERNAL_NETWORK_ISOLATION` for the primary container:

```yaml
plugin_mcp_containers:
  - name: fluensy_learn
    platform: docker
    image: fluensy-learn-mcp
    tag: latest
    port: 3000
    network: fluensy-learn-net
    host_directory: /Users/tleyden/fluensy-data
    container_directory: /data
    network_isolation: false
    auth:
      type: none
```

- Default (field omitted): `true` — unchanged, secure-by-default behavior.
- `network_isolation: false` — the plugin container skips internal-network
  creation/attachment and runs on the runtime's normal default
  bridge/network, restoring outbound egress from inside that specific
  plugin's container. This is the same tradeoff the primary container's
  toggle already documents, scoped to one plugin instead of the whole app.
- This directly unblocks `platform: docker` plugins on macOS, which today
  have no way to avoid the unconditional rejection in `docker.rs`.

## Non-goals

- Do not change `ContainerPort::validate_internal_network_support` or its
  reject-all-isolated-Docker-on-macOS policy. `network_isolation: false`
  produces `isolation_strategy: None`, which already bypasses that check
  entirely today (`ensure_container.rs:86` only validates when
  `isolation_strategy.is_some()`).
- Do not add a TUI/first-run-wizard toggle. Plugin MCP Containers are
  intentionally hand-edited config only.
- Do not change the primary vault container's `ContainerStartupConfig` /
  `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION` behavior; it already works
  correctly and is a separate config surface.
- Do not fix the separate `docker.rs` `--network` contract-violation bug
  flagged in the counter-plan (Phase 1, only relevant when isolation is
  *enabled*). Out of scope here.

## Design

### 1. Domain model: `crates/core/src/domain/model.rs`

Add a field to `PluginMcpContainerConfig`:

```rust
pub struct PluginMcpContainerConfig {
    pub name: String,
    pub runtime: ContainerRuntime,
    pub image: String,
    pub container_port: u16,
    pub host_port: Option<u16>,
    pub host_directory: PathBuf,
    pub container_directory: PathBuf,
    pub network_name: String,
    /// Mirrors `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION` for the primary
    /// container, scoped to this plugin. Default `true`. When `false`, the
    /// plugin container skips internal-network isolation and runs on the
    /// runtime's normal default network, regaining outbound egress.
    pub network_isolation: bool,
    pub auth: PluginMcpContainerAuth,
}
```

### 2. YAML parsing and validation: `crates/platform/src/config/brain3_yaml.rs`

- Add `network_isolation: Option<bool>` to `RawPluginMcpContainerConfig`.
- In `validate_plugin_mcp_container`, resolve
  `let network_isolation = entry.network_isolation.unwrap_or(true);` and pass
  it through to the constructed `PluginMcpContainerConfig`.
- Add a validation step, mirroring `validate_network_isolation_support` in
  `env_file.rs`, that fails this entry (not the whole file — matches the
  existing "skip this one invalid entry, keep the rest" pattern used by every
  other check in this function) when isolation is requested but unsupported:

  ```rust
  fn validate_plugin_network_isolation_support(
      runtime: ContainerRuntime,
      network_isolation: bool,
  ) -> Result<(), String> {
      if network_isolation
          && matches!(runtime, ContainerRuntime::Docker)
          && env::consts::OS == "macos"
      {
          return Err(
              "network_isolation: true is not supported with platform: docker on macOS; \
               set network_isolation: false or platform: macos_container for this plugin"
                  .to_string(),
          );
      }
      Ok(())
  }
  ```

  Call it from `validate_plugin_mcp_container` after `runtime` is parsed and
  before constructing the returned config, so the failure surfaces as one of
  the existing `tracing::error!` "skipping invalid Plugin MCP Container
  config" log lines at config-load time — before any container launch is
  attempted — consistent with the already-landed Phase 0 guard philosophy for
  the primary container.

  This is a fail-fast convenience, not the enforcement boundary — the
  already-implemented `ContainerPort::validate_internal_network_support`
  choke point in `docker.rs` remains the authoritative last-resort guard for
  any config that reaches `EnsureContainerUseCase` some other way (e.g.
  tests constructing `PluginMcpContainerConfig` directly). Keep both, exactly
  as the choke-point plan intended for the primary container's guard.

### 3. Container startup: `crates/platform/src/container/startup.rs`

In `build_plugin_container_config`, change:

```rust
let isolation_strategy = Some(plugin_isolation_strategy(plugin.runtime));
```

to:

```rust
let isolation_strategy = plugin
    .network_isolation
    .then(|| plugin_isolation_strategy(plugin.runtime));
```

No other changes needed in this file — `EnsureContainerUseCase` and the
Docker/macOS adapters already handle `isolation_strategy: None` correctly for
the primary container's existing `network_isolated: false` path (no
`ensure_internal_network` call, no `--network` flag, `--publish` still
applied since the publish loop only skips `DiscoverContainerIp`). Update the
existing `tracing::info!` "resolved Plugin MCP Container network isolation
mode" log call to log `isolation_strategy` (which is now `Option<...>` again,
same shape as the primary container's log) instead of the always-`Some`
value it references today.

### 4. Update existing call sites that construct `PluginMcpContainerConfig`

All of these need the new field added explicitly (no `Default` impl exists
for this struct, and none should be added just to paper over this):

- `crates/platform/src/config/brain3_yaml.rs:124` (the real constructor —
  covered by step 2 above)
- `crates/platform/src/config/brain3_yaml.rs` test fixtures (`valid_entry`
  helper and the two literal `PluginMcpContainerConfig { ... }` expected
  values in `valid_multi_entry_file_loads_configs_with_defaults`)
- `crates/platform/src/container/startup.rs:822` (`sample_plugin_config` test
  fixture)
- `apps/gateway/src/server.rs:704` (`sample_plugin_container_config` test
  fixture)

Default all of these to `network_isolation: true` unless a test specifically
needs `false` (see Tests below).

## Tests

### `crates/platform/src/config/brain3_yaml.rs`

- Extend `valid_entry()` / the multi-entry test to assert `network_isolation`
  defaults to `true` when the YAML field is omitted (covers the existing
  fixtures once the field is added to the expected struct literals).
- New test: a plugin entry with `network_isolation: false` on
  `platform: docker` parses successfully with `network_isolation == false`,
  regardless of host OS (this combination is always valid).
- New test, `#[cfg(target_os = "macos")]`: a plugin entry with
  `platform: docker` and `network_isolation` omitted (or explicitly `true`)
  is dropped, with the error naming `network_isolation` and
  `platform: docker`, mirroring
  `load_rejects_internal_network_isolation_for_docker_on_macos` in
  `env_file.rs`. Assert the file still yields other valid entries (existing
  drop-only-the-bad-entry pattern).
- New test, `#[cfg(target_os = "linux")]`: the same `platform: docker` +
  `network_isolation: true` (or omitted) entry loads successfully on Linux,
  mirroring `load_allows_internal_network_isolation_for_docker_on_linux`.
- Existing `platform: macos_container` fixtures are unaffected by the new
  guard (only the `docker` + macOS combination is restricted) — no new
  assertion needed beyond field defaulting.

### `crates/platform/src/container/startup.rs`

- Extend `build_plugin_container_config_adds_plugin_role_labels_and_mounts`
  (or add a sibling test) asserting that `network_isolation: false` produces
  `config.isolation_strategy == None` regardless of platform.
- Confirm the existing `network_isolation: true` (default) assertions in
  `build_plugin_container_config_adds_plugin_role_labels_and_mounts` and
  `plugin_isolation_strategy_matches_runtime` still pass unchanged.

### `apps/gateway/src/server.rs`

- No new test required unless an existing test asserts on
  `PluginMcpContainerConfig` field count/equality in a way the compiler
  won't already catch by requiring the new field.

## Documentation

### README.md — "Experimental: Plugin MCP Containers" section (~line 395)

Add `network_isolation: false` to the example or a follow-up sentence, and
one paragraph analogous to the primary-container security wording:

> `network_isolation` is optional and defaults to `true`. Set it to `false`
> to run this plugin's container on the runtime's normal default network
> instead of an internal-only network, restoring outbound internet access
> from inside that container. This is required for `platform: docker`
> plugins on macOS, where Docker Desktop cannot combine internal-only
> networking with host-reachable published ports — use
> `network_isolation: false` there, or switch to
> `platform: macos_container` if the plugin does not need outbound egress.

### SECURITY_AUDIT.md — Threat Model

Per `AGENTS.MD`'s instruction to keep the threat model current, add one
clause to the existing "Assumptions" bullet about Plugin MCP Containers
(the one starting "Plugin MCP Containers are Experimental..."), noting that
individual plugins may opt out of network isolation via
`network_isolation: false` in `brain3.yaml`, with the same trust level as
today (local-file-config-only, no new remote ingress, no change to the
"opt-in, local-file-only experimental surface" security objective). This is
a small addition, not a new Threat Model section — it's the same category of
change as the existing, currently-undocumented
`B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=false` escape hatch for the primary
container, which also has no dedicated threat-model entry today.

## Verification

```bash
cargo test -p brain3 --no-run
cargo test
```

Manual local verification (macOS, Docker Desktop):

1. Set `network_isolation: false` for `fluensy_learn` in
   `~/.brain3/brain3.yaml`.
2. Start Brain3 and confirm the startup log no longer shows "rejecting
   unsupported internal network configuration" for `fluensy_learn`, and
   instead shows "Plugin MCP Container ready".
3. Confirm `fluensy_learn`'s tools are reachable through the gateway (e.g. a
   `tools/list` call shows the `fluensy_learn__`-prefixed tools).

No E2E smoke test changes are required — the E2E suite does not currently
exercise `brain3.yaml` plugin containers (confirm this is still true before
implementing; if it does, add a `network_isolation: false` case there too).

## Completion criteria

- `brain3.yaml` supports an optional `network_isolation` boolean per plugin,
  default `true`, with behavior identical to today when omitted.
- A `platform: docker` plugin with `network_isolation: false` starts
  successfully on macOS instead of being rejected.
- A `platform: docker` plugin with `network_isolation` omitted/`true` is
  still rejected on macOS at config-load time, with an actionable message —
  the existing choke-point guard in `docker.rs` is untouched and remains the
  authoritative backstop.
- `cargo test -p brain3 --no-run` and `cargo test` pass.
- README and `SECURITY_AUDIT.md` reflect the new field.
