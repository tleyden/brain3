# Plan: Split B3_CONTAINER_IMAGE into repo + tag

## Problem

Users who set `B3_CONTAINER_IMAGE=ghcr.io/tleyden/brain3-mcp-vault-tools:v0.1.7` in their env
file get stale images after upgrading the binary. The correct version is already baked into the
binary as `CURRENT_RELEASE` — it just isn't used at runtime config load.

## Goal

- Store only the image repo (no tag) in the env file and domain model
- At runtime, resolve the tag: use `B3_CONTAINER_IMAGE_TAG` if set, otherwise fall back to
  `CURRENT_RELEASE` baked into the binary
- Validate on startup that `B3_CONTAINER_IMAGE_REPO` contains no `:` (i.e. no tag accidentally
  included), with a clear error message pointing to `B3_CONTAINER_IMAGE_TAG`

## New env variable names

| Variable | Required? | Meaning |
|---|---|---|
| `B3_CONTAINER_IMAGE_REPO` | Yes (when container runtime set) | Repo path only — no tag, no colon |
| `B3_CONTAINER_IMAGE_TAG` | No | Tag to use; empty = binary's built-in `CURRENT_RELEASE` |

The name `B3_CONTAINER_IMAGE_REPO` makes it unambiguous that this is a repo path, not a full
image ref. The `.env.template` comment reinforces this.

## Files to change

### 1. `crates/core/src/domain/setup.rs`

- `SetupDefaults.default_container_image: String` → `default_container_image_repo: String`
- `SetupDraftConfig.container_image: String` → `container_image_repo: String`

### 2. `crates/core/src/application/first_run_setup.rs`

- Update all field references: `container_image` → `container_image_repo`,
  `default_container_image` → `default_container_image_repo`
- In tests, strip the tag from literal image strings:
  `"ghcr.io/tleyden/brain3-mcp-vault-tools:v9.9.9"` → `"ghcr.io/tleyden/brain3-mcp-vault-tools"`
- In the test default at line ~301, same: drop `:latest` from the literal

### 3. `crates/platform/src/config/env_file.rs`

Replace the single `require_nonempty_env("B3_CONTAINER_IMAGE", ...)` block with:

```rust
let repo = require_nonempty_env("B3_CONTAINER_IMAGE_REPO", "when B3_CONTAINER_RUNTIME is set")?;
if repo.contains(':') {
    return Err(ConfigError::Invalid(format!(
        "B3_CONTAINER_IMAGE_REPO must not include a tag (found ':'). \
         Use B3_CONTAINER_IMAGE_TAG to pin a version, or leave it empty \
         to automatically use the version matching this binary ({CURRENT_RELEASE})"
    )));
}
let tag_override = env_var_or("B3_CONTAINER_IMAGE_TAG", "");
let tag = if tag_override.trim().is_empty() {
    CURRENT_RELEASE.to_string()
} else {
    tag_override.trim().to_string()
};
let image = format!("{repo}:{tag}");
```

`CURRENT_RELEASE` is already importable from `brain3_core::application::first_run_setup`.

Update the test env strings in this file: replace `B3_CONTAINER_IMAGE=...` with
`B3_CONTAINER_IMAGE_REPO=ghcr.io/tleyden/brain3-mcp-vault-tools` (no tag).

### 4. `crates/platform/src/setup/env_writer.rs`

Replace the `B3_CONTAINER_IMAGE` write with two writes:

```rust
values.insert("B3_CONTAINER_IMAGE_REPO", draft.container_image_repo.clone());
values.insert("B3_CONTAINER_IMAGE_TAG", "".to_string());  // empty = auto-match binary version
```

### 5. `apps/gateway/src/release.rs`

- `default_container_image()` is no longer used by first-run setup; remove it or leave it for
  any remaining callers (check with `grep`)
- Keep `MCP_IMAGE_REPO`, `container_image_for_tag()`, `official_latest_container_image()`

### 6. `apps/gateway/src/tui/screens.rs`

- Line ~587: display `state.draft.container_image_repo`
- Line ~1211: inject `MCP_IMAGE_REPO.to_string()` (from `release`) instead of
  `release::default_container_image()`

### 7. `.env.template`

Replace the `B3_CONTAINER_IMAGE` block (~line 87–89) with:

```
# Container image repository — must NOT include a tag (no colon allowed).
# The tag is set separately by B3_CONTAINER_IMAGE_TAG below.
B3_CONTAINER_IMAGE_REPO=ghcr.io/tleyden/brain3-mcp-vault-tools

# Container image tag (optional). Leave empty to automatically use the version
# matching this binary. Set explicitly to pin a specific version or test a
# pre-release build (e.g. v0.1.7, pr-123).
B3_CONTAINER_IMAGE_TAG=
```

## Runtime behavior

| `B3_CONTAINER_IMAGE_REPO` | `B3_CONTAINER_IMAGE_TAG` | Result |
|---|---|---|
| `ghcr.io/tleyden/brain3-mcp-vault-tools` | `` (empty) | `...brain3-mcp-vault-tools:v0.1.9` (auto) |
| `ghcr.io/tleyden/brain3-mcp-vault-tools` | `v0.1.7` | `...brain3-mcp-vault-tools:v0.1.7` (pinned) |
| `ghcr.io/tleyden/brain3-mcp-vault-tools:v0.1.7` | anything | **startup error** with fix instruction |

## Verification

Run `cargo test` after changes. Key tests to check:

- `first_run_setup.rs`: `prepare_uses_injected_default_container_image`
- `env_file.rs`: existing container config parse tests (update env strings to use new var names)
- `release.rs`: update or remove `default_container_image_uses_versioned_tag` if the function
  is removed
