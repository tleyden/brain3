# Plan: B3_ACCESS_MODE env variable + least-privilege port binding

**Date**: 2026-06-24
**Status**: Proposed
**Branch**: easier_local_mcp

## Problem

Three related gaps after the access-mode wizard was added:

1. **Missing comments on generated env fields.** When local MCP is enabled, `env_writer.rs`
   appends `B3_LOCAL_MCP_PORT` and `LOCAL_GATEWAY_MCP_REVERSE_PROXY_BEARER_TOKEN` at the end
   of the file (after the template) with no explanatory comments. The user cannot tell what
   they mean from the file alone.

2. **Access mode not persisted.** The wizard captures `LocalOnly / RemoteOnly / Both` in
   `SetupDraftConfig.access_mode`, but that choice is never written to the `.env` file. After
   setup, `GatewayConfig` has no `access_mode` field — the only downstream effects are whether
   `B3_LOCAL_MCP_PORT` is set and whether a tunnel is configured.

3. **Port binding ignores chosen mode.** On startup the gateway always attempts to bind port
   8421 (OAuth) regardless of access mode. If the user chose `LocalOnly`, binding 8421 is
   unnecessary and violates least-privilege.

---

## Solution overview

1. Move the local MCP fields into the template as proper (uncommented) entries so comments
   are always present in the rendered `.env`.
2. Add `B3_ACCESS_MODE=local|remote|both` to the template, persist it from `SetupDraftConfig`,
   and parse it into `GatewayConfig` at startup.
3. At startup, check `config.access_mode` and skip binding whichever port is not needed:
   - `LocalOnly` → bind 8422 only; skip 8421
   - `RemoteOnly` → bind 8421 only; skip 8422 (already handled by `local_mcp` being `None`,
     but now also gated by the explicit enum)
   - `Both` → bind both (current behaviour)

---

## Changes

### 1. `.env.template`

**Remove** the commented-out block:
```
# B3_LOCAL_MCP_PORT=8422
# LOCAL_GATEWAY_MCP_REVERSE_PROXY_BEARER_TOKEN=
```

**Replace** with proper (uncommented) entries and better comments, placed in the same
location in the file:

```
# Port Brain3 binds on 127.0.0.1 for direct bearer-token-authenticated MCP requests
# (Claude Code / desktop clients). Leave empty to disable local MCP access.
B3_LOCAL_MCP_PORT=

# Static bearer token AI desktop clients send in the Authorization header when
# connecting to B3_LOCAL_MCP_PORT. Required when B3_LOCAL_MCP_PORT is set.
LOCAL_GATEWAY_MCP_REVERSE_PROXY_BEARER_TOKEN=
```

**Add** a new section above or near the local MCP block:

```
# Which MCP access modes Brain3 should activate.
# local  — listen on B3_LOCAL_MCP_PORT only; do not bind the OAuth gateway port
# remote — listen on B3_OAUTH2_GATEWAY_PORT only; B3_LOCAL_MCP_PORT is ignored
# both   — listen on both ports
B3_ACCESS_MODE=both
```

### 2. `crates/platform/src/setup/env_writer.rs`

**Remove** the post-template append block (lines ~30-42) that appends
`B3_LOCAL_MCP_PORT` / `LOCAL_GATEWAY_MCP_REVERSE_PROXY_BEARER_TOKEN` after the rendered
template. Now that these keys exist as regular template entries, the normal per-line render
loop handles them.

**Update `build_overrides`**:

- Always insert `B3_LOCAL_MCP_PORT` (value when enabled, empty `String::new()` when not).
- Always insert `LOCAL_GATEWAY_MCP_REVERSE_PROXY_BEARER_TOKEN` (value when enabled, empty
  when not).
- Add `B3_ACCESS_MODE` derived from `draft.access_mode`:

```rust
let access_mode_str = match draft.access_mode {
    AccessModeDraft::LocalOnly  => "local",
    AccessModeDraft::RemoteOnly => "remote",
    AccessModeDraft::Both       => "both",
};
values.insert("B3_ACCESS_MODE", access_mode_str.to_string());
```

### 3. `crates/core/src/domain/model.rs`

Add a new `AccessMode` enum (runtime counterpart of `AccessModeDraft`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    Local,
    Remote,
    Both,
}
```

Add `access_mode: AccessMode` to `GatewayConfig`:

```rust
pub struct GatewayConfig {
    // ...existing fields...
    pub access_mode: AccessMode,
}
```

### 4. `crates/platform/src/config/env_file.rs`

Add a `load_access_mode()` helper:

```rust
fn load_access_mode() -> Result<AccessMode, ConfigError> {
    match env::var("B3_ACCESS_MODE").as_deref() {
        Ok("local")  => Ok(AccessMode::Local),
        Ok("remote") => Ok(AccessMode::Remote),
        Ok("both") | Err(_) => Ok(AccessMode::Both),   // default to both when unset
        Ok(other) => Err(ConfigError::Invalid(format!(
            "B3_ACCESS_MODE must be 'local', 'remote', or 'both'; got '{other}'"
        ))),
    }
}
```

Call it during config load and set `config.access_mode`. Add `"B3_ACCESS_MODE"` to the
`CONFIG_KEYS` constant used by tests.

### 5. `apps/gateway/src/server.rs`

**`run_gateway_server_until`** — make the OAuth (8421) listener conditional:

```rust
let oauth_listener = if config.access_mode != AccessMode::Local {
    Some(bind_listener(host, config.port).await?)
} else {
    None
};
```

When `oauth_listener` is `None`:
- Skip building the OAuth axum router / app state for the main serve.
  (We still build `app_state` for the local MCP router, which also uses it. Keep
  `build_gateway_state` call unconditional but only bind the listeners that are needed.)
- Block on the local MCP task instead of the OAuth serve future.
- Log that the OAuth gateway port is intentionally not bound.

The simplest restructuring:
- Bind local MCP listener if `local_mcp` is Some (unchanged).
- Bind OAuth listener only when `access_mode != Local`.
- Await whichever serve futures are actually started; propagate shutdown to all.

**`spawn_gateway_server`** — same gate: skip binding 8421 when `access_mode == Local`.

Add a log line in both paths:

```rust
tracing::info!(access_mode = ?config.access_mode, "access mode: binding only needed ports");
```

---

## What does NOT change

- `SetupDraftConfig.access_mode` type stays `AccessModeDraft` (setup-only type).
- `apply_access_mode_policy` in `first_run_setup.rs` is untouched — it already sets
  `local_mcp_enabled` / `tunnel_mode` correctly; the new `B3_ACCESS_MODE` is additive.
- `env_file.rs` `load_local_mcp_config()` is unchanged — it still returns `None` for
  `RemoteOnly` because `B3_LOCAL_MCP_PORT` will be empty.
- Container startup, tunnel, TUI, and screens are untouched.
- Tests for `finalize_local_only_*` and `finalize_remote_only_*` are unchanged.

---

## Backwards compatibility

Users with old `.env` files lacking `B3_ACCESS_MODE` default to `Both` — all ports bind as
before. No breaking change.

---

## Implementation order

1. `.env.template` — move local MCP fields out of commented block; add `B3_ACCESS_MODE`.
2. `env_writer.rs` — remove append block; always write the three keys in `build_overrides`.
3. `domain/model.rs` — add `AccessMode` enum + `access_mode` field on `GatewayConfig`.
4. `env_file.rs` — add `load_access_mode()` + wire into `GatewayConfig` construction.
5. `server.rs` — gate OAuth listener on `access_mode != Local`.
6. `cargo test`.
