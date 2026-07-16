# Plan: per-plugin `env` passthrough in brain3.yaml

## Problem

`plugin_mcp_containers` entries in `brain3.yaml` have no way to set arbitrary
environment variables on the plugin container. Concretely, need to set things
like:

```
LOGFIRE_CONSOLE=true
LOGFIRE_MIN_LEVEL=debug
MCP_DEBUG_LEVEL=debug
```

on a specific plugin container without hardcoding those keys into Brain3
itself (unlike the primary MCP container, which already gets a fixed set of
`B3_*` env vars in `build_container_config`).

## Design

Add an optional `env` map to each `plugin_mcp_containers` entry:

```yaml
plugin_mcp_containers:
  - name: fluensy_learn
    platform: docker
    image: ghcr.io/example/fluensy-learn
    tag: latest
    port: 8420
    network: fluensy-learn-net
    host_directory: /Users/you/fluensy-data
    env:
      LOGFIRE_CONSOLE: "true"
      LOGFIRE_MIN_LEVEL: debug
      MCP_DEBUG_LEVEL: debug
    auth:
      type: none
```

`env` is optional and defaults to empty (matches every other optional field
in this file). Keys must look like POSIX environment variable names
(`[A-Za-z_][A-Za-z0-9_]*`); an entry with an invalid key is dropped the same
way other invalid entries are dropped today (logged and skipped, rest of the
file still loads).

This is plain, non-secret config — not a place for tokens/passwords. Secrets
already have a dedicated mechanism (`auth.type: bearer_token` +
`secret_file`, mounted as a file, not an env var). Don't try to reuse `env`
for secrets; that's out of scope here.

The plumbing to actually pass env vars into `docker run` / `container run`
already exists and is runtime-agnostic — `ContainerConfig.env_vars: Vec<(String,
String)>` is already read by both `docker.rs` and `macos_container.rs` (see
`for (key, value) in &config.env_vars` in each). Plugin containers just
currently always pass `Vec::new()` for it
(`crates/platform/src/container/startup.rs:397`). So this plan is mostly
schema + validation + one-line wiring change, not new container-runtime code.

## Changes

### 1. `crates/core/src/domain/model.rs`

Add a field to `PluginMcpContainerConfig` (around line 69-86):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMcpContainerConfig {
    pub name: String,
    pub runtime: ContainerRuntime,
    pub image: String,
    pub container_port: u16,
    pub host_port: Option<u16>,
    pub host_directory: PathBuf,
    pub container_directory: PathBuf,
    pub network_name: String,
    pub network_isolation: bool,
    pub env: Vec<(String, String)>,
    pub auth: PluginMcpContainerAuth,
}
```

(Field order doesn't matter functionally; putting it right before `auth`
keeps the diff small.)

### 2. `crates/platform/src/config/brain3_yaml.rs`

- Add `use std::collections::BTreeMap;` (keep using `BTreeMap` rather than
  `HashMap` so key order is deterministic — matters for tests and for
  readable `docker inspect` / logging output).

- Add a field to `RawPluginMcpContainerConfig` (around line 21-33):

  ```rust
  env: Option<BTreeMap<String, String>>,
  ```

  No `#[serde(default)]` needed — `Option<T>` fields are already treated as
  optional by serde on missing keys, same as every other `Option` field in
  this struct.

- Add parsing/validation helpers (near `validate_network_name`, around line
  161-174):

  ```rust
  fn parse_env(env: Option<BTreeMap<String, String>>) -> Result<Vec<(String, String)>, String> {
      let Some(env) = env else {
          return Ok(Vec::new());
      };

      for key in env.keys() {
          validate_env_var_name(key)?;
      }

      Ok(env.into_iter().collect())
  }

  fn validate_env_var_name(name: &str) -> Result<(), String> {
      let mut chars = name.chars();
      let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
      let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');

      if first_ok && rest_ok {
          Ok(())
      } else {
          Err(format!(
              "env variable name '{name}' must match [A-Za-z_][A-Za-z0-9_]*"
          ))
      }
  }
  ```

- In `validate_plugin_mcp_container` (around line 101-140), parse the env map
  and add it to the constructed config:

  ```rust
  let env = parse_env(entry.env)?;
  ...
  Ok(PluginMcpContainerConfig {
      name,
      runtime,
      image: format!("{image}:{tag}"),
      container_port,
      host_port: entry.host_port,
      host_directory,
      container_directory,
      network_name,
      network_isolation,
      env,
      auth,
  })
  ```

  An invalid env key causes `Err(...)` to bubble up through the existing
  `?`-based flow, which drops just that one plugin entry (same behavior as
  today's invalid `name`/`network`/`auth` handling) — no new error-handling
  pattern needed.

- Update existing tests that construct `PluginMcpContainerConfig` literals
  directly (around lines 331 and 349) to add `env: Vec::new()` (or `env:
  vec![...]` for a new env-specific test).

- Add new unit tests:
  - An entry with `env: { FOO: "bar", BAZ: "qux" }` loads with
    `configs[0].env == vec![("BAZ".into(), "qux".into()), ("FOO".into(), "bar".into())]`
    (alphabetical, since `BTreeMap` iteration is sorted).
  - An entry with an invalid env key (e.g. `"1BAD": "x"` or `"has-dash": "x"`)
    is dropped, same pattern as `bad_name_charset_is_dropped`.
  - An entry with no `env` key at all still loads, with `env == Vec::new()`
    (covered implicitly by existing tests once the struct literals are
    updated, but worth asserting explicitly in one test).

### 3. `crates/platform/src/container/startup.rs`

- In `build_plugin_container_config` (around line 342-409), change:

  ```rust
  env_vars: Vec::new(),
  ```

  to:

  ```rust
  env_vars: plugin.env.clone(),
  ```

- Update `sample_plugin_config()` (around line 827-843) to add `env:
  Vec::new()` to the struct literal so the crate compiles.

- Add a test (alongside
  `build_plugin_container_config_adds_plugin_role_labels_and_mounts`) that
  sets `plugin.env = vec![("LOGFIRE_CONSOLE".into(), "true".into())]` and
  asserts `config.env_vars` contains that pair — i.e. confirms the plumbing
  from `PluginMcpContainerConfig.env` to `ContainerConfig.env_vars` actually
  happens, not just that parsing works.

### 4. `README.md`

Update the "Experimental: Plugin MCP Containers" section (around line
395-445):

- Add `env:` to the example YAML block, e.g.:

  ```yaml
  plugin_mcp_containers:
    - name: hello_mcp
      platform: docker
      image: ghcr.io/example/hello-mcp
      tag: latest
      port: 8420
      network: hello-mcp-net
      host_directory: /Users/you/hello-mcp-data
      container_directory: /data
      network_isolation: false
      env:
        LOGFIRE_CONSOLE: "true"
        MCP_DEBUG_LEVEL: debug
      auth:
        type: bearer_token
        secret_file: /Users/you/.brain3/secrets/hello_mcp.token
        secret_mount_path: /run/secrets/mcp_bearer_token
  ```

- Add a short paragraph: `env` is optional and sets plain (non-secret)
  environment variables on the plugin container. Keys must be valid
  environment variable names; secrets belong in `auth.secret_file`, not
  here.

## Explicitly out of scope

- No reserved/blocked key prefixes (e.g. rejecting `B3_*` or `BRAIN3_*`).
  Plugin containers currently get zero Brain3-injected env vars, so there's
  nothing to collide with yet. If that changes later, add the guard then.
- No support for referencing host env vars or `.env` file interpolation
  inside `brain3.yaml` values — `env` values are literal strings only, same
  as every other string field in this file.
- No changes to `ContainerConfig`, `docker.rs`, or `macos_container.rs` —
  their `env_vars` handling is already generic and already covers this case.

## Verification

- `cargo test -p brain3 --no-run` then `cargo test` (per AGENTS.MD) covering
  the new/updated unit tests in `brain3_yaml.rs` and `startup.rs`.
- Manual sanity check: add an `env` block to a local `~/.brain3/brain3.yaml`
  plugin entry, start Brain3, and confirm via `docker inspect
  <plugin_name>` (or `container inspect` on macOS) that the env vars are
  present on the running container.
