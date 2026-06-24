# Plan: Comment out OAuth fields in .env when B3_ACCESS_MODE=local

## Goal

When `render_env_file` writes a `.env` with `AccessModeDraft::LocalOnly`, fields that are
irrelevant to local-only mode are emitted as commented-out lines (with the user's entered values
preserved) instead of active assignments. This avoids confusing users with required-looking
fields that are never read, while making it trivial to switch back to `remote`/`both` by
uncommenting the lines.

## Fields to comment out in local mode

These OAuth/remote-gateway fields serve no purpose when there is no OAuth gateway:

- `B3_OAUTH2_GATEWAY_PORT`
- `B3_OAUTH2_GATEWAY_CLIENT_ID`
- `B3_OAUTH2_GATEWAY_CLIENT_SECRET`
- `B3_OAUTH2_PKCE_REQUIRED`
- `B3_OAUTH2_ACCESS_TOKEN_LIFETIME_SECS`
- `B3_OAUTH2_REFRESH_TOKEN_LIFETIME_SECS`
- `B3_USERNAME`
- `B3_PASSWORD`
- `B3_OAUTH2_GATEWAY_ENFORCE_HOSTNAME_CHECK`
- `B3_CF_QUICK_TUNNEL`
- `B3_CF_TUNNEL_NAME`
- `B3_CF_DOMAIN`
- `B3_CF_TUNNEL_CONFIG_FILE`
- `B3_DIRECT_PUBLIC_ORIGIN_HOSTNAME`

## Changes

### 1. `crates/platform/src/setup/env_writer.rs`

Add a helper:

```rust
fn is_remote_only_key(key: &str) -> bool {
    matches!(
        key,
        "B3_OAUTH2_GATEWAY_PORT"
            | "B3_OAUTH2_GATEWAY_CLIENT_ID"
            | "B3_OAUTH2_GATEWAY_CLIENT_SECRET"
            | "B3_OAUTH2_PKCE_REQUIRED"
            | "B3_OAUTH2_ACCESS_TOKEN_LIFETIME_SECS"
            | "B3_OAUTH2_REFRESH_TOKEN_LIFETIME_SECS"
            | "B3_USERNAME"
            | "B3_PASSWORD"
            | "B3_OAUTH2_GATEWAY_ENFORCE_HOSTNAME_CHECK"
            | "B3_CF_QUICK_TUNNEL"
            | "B3_CF_TUNNEL_NAME"
            | "B3_CF_DOMAIN"
            | "B3_CF_TUNNEL_CONFIG_FILE"
            | "B3_DIRECT_PUBLIC_ORIGIN_HOSTNAME"
    )
}
```

In `render_env_file`, pass `draft.access_mode` into the line-rendering loop. When:
- the current line is a `KEY=VALUE` assignment
- the key is found in `overrides`
- `access_mode == AccessModeDraft::LocalOnly`
- `is_remote_only_key(key)` is true

emit `# KEY=<quoted-value>` instead of `KEY=<quoted-value>`, preserving the user's entered
value so re-enabling remote mode is a single uncomment.

The comment lines above each key in the template pass through unchanged (they are not
`KEY=VALUE` lines so the existing logic already handles them correctly).

### 2. `crates/platform/tests/setup_bootstrap.rs`

Add a new test `render_env_file_comments_out_oauth_fields_in_local_mode`:
- Use `AccessModeDraft::LocalOnly` with real username/password/client_secret values.
- Assert that `B3_USERNAME`, `B3_PASSWORD`, `B3_OAUTH2_GATEWAY_CLIENT_SECRET`, and a
  sample of the other remote-only keys appear as `# KEY="value"` (commented with value
  preserved).
- Assert that local-mode keys (`B3_LOCAL_MCP_PORT`, `LOCAL_GATEWAY_MCP_BEARER_TOKEN`,
  `B3_ACCESS_MODE`, `B3_VAULT_PATH`) remain as active uncommented assignments.

Update the existing `render_env_file_disables_quick_tunnel_for_disabled_mode` test (which
already uses `AccessModeDraft::LocalOnly`) to also assert that `B3_USERNAME` and
`B3_PASSWORD` appear commented out.

## Value preservation decision

Commented-out lines keep the user-entered value, e.g.:

```
# B3_PASSWORD="hunter2"
```

Rationale: switching from `local` to `remote`/`both` later requires only uncommenting the
lines, not re-entering all credentials.
