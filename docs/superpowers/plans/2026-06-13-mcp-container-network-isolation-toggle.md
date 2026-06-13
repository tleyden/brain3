# MCP Container Network Isolation Toggle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep internal-only MCP container networking enabled by default, but allow operators to disable it through `.env` and the first-run TUI when Docker/macOS internal networking is broken on a VPS.

**Architecture:** Preserve the existing `network_isolated` runtime behavior and thread one new default-`true` boolean from setup/config into `ContainerStartupConfig`. When the setting is `true`, keep today's internal-network recreation flow. When it is `false`, skip network-isolation preparation entirely and let the container runtime use its normal bridged/default network.

**Tech Stack:** Rust, ratatui/crossterm TUI, dotenvy config loading, existing Docker/macOS container adapters

---

## Scope Notes

- The original isolation work is already landed in:
  - `crates/core/src/domain/model.rs`
  - `crates/core/src/application/ensure_container.rs`
  - `crates/platform/src/container/startup.rs`
  - `crates/platform/src/container/docker.rs`
  - `crates/platform/src/container/macos_container.rs`
- This plan does **not** replace that work. It adds operator configurability on top of it.
- "No need to delete and recreate it when false" is interpreted as the **internal network**, not the container. Container restart behavior stays as-is so Brain3 still refreshes the upstream secret and startup state consistently.
- Recommended env var name: `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION`
  - It matches the existing `network_isolated` terminology.
  - It is clearer than "custom internal only networking" and reads naturally as a `true`/`false` toggle.

## File Structure

- Modify: `.env.template`
  - Add the new documented env var with verbose security/compatibility comments.
- Modify: `crates/core/src/domain/model.rs`
  - Add the new startup-level boolean to `ContainerStartupConfig`.
- Modify: `crates/core/src/domain/setup.rs`
  - Add the setup-draft boolean used by the TUI and env writer.
- Modify: `crates/core/src/application/first_run_setup.rs`
  - Default the setup draft to isolated networking enabled.
- Modify: `crates/platform/src/config/env_file.rs`
  - Parse the new env var with default `true` and map it into `ContainerStartupConfig`.
- Modify: `crates/platform/src/setup/env_writer.rs`
  - Render the new env var into generated `.env` files.
- Modify: `crates/platform/src/container/startup.rs`
  - Use configured `network_isolated` instead of hardcoding `true`.
- Modify: `apps/gateway/src/tui/state.rs`
  - Add the new toggle to first-run wizard focus and summary navigation.
- Modify: `apps/gateway/src/tui/screens.rs`
  - Show the new toggle in Ports & Settings and Summary with clear wording.
- Modify: `apps/gateway/src/tui/app.rs`
  - Wire the existing toggle keys to the new field.
- Modify: `crates/platform/tests/setup_bootstrap.rs`
  - Add focused coverage for env rendering.
- Modify: `README.md`
  - Document the default-on security behavior and the compatibility escape hatch.

### Task 1: Thread the Toggle Through Setup and Runtime Config

**Files:**
- Modify: `crates/core/src/domain/model.rs`
- Modify: `crates/core/src/domain/setup.rs`
- Modify: `crates/core/src/application/first_run_setup.rs`

- [ ] **Step 1: Add the startup-level boolean to `ContainerStartupConfig`**

Add a field to `ContainerStartupConfig`:

```rust
pub network_isolated: bool,
```

This keeps the runtime-facing model explicit instead of relying on a hardcoded value in startup assembly.

- [ ] **Step 2: Add the setup-draft boolean used by the TUI and env writer**

Add a field to `SetupDraftConfig`:

```rust
pub container_network_isolated: bool,
```

Use the setup-specific name here so the meaning stays obvious in TUI code next to `container_host_port` and `container_mcp_port`.

- [ ] **Step 3: Default first-run setup to the secure value**

In `FirstRunSetupUseCase::prepare()`, initialize the draft with:

```rust
container_network_isolated: true,
```

This preserves today's secure default for all fresh installs.

- [ ] **Step 4: Update any setup test fixtures that construct `SetupDraftConfig`**

Any existing `SetupDraftConfig { ... }` literals in tests need the new field added with an explicit value, usually:

```rust
container_network_isolated: true,
```

### Task 2: Add the Env Variable and Generated `.env` Support

**Files:**
- Modify: `.env.template`
- Modify: `crates/platform/src/setup/env_writer.rs`
- Modify: `crates/platform/tests/setup_bootstrap.rs`

- [ ] **Step 1: Add the new variable to `.env.template` with verbose comments**

Place it near the other container settings, after runtime/image/ports:

```dotenv
# Restrict the managed MCP container to an internal-only network with no default outbound route.
# Default: true. Recommended for maximum security.
# Set this to false only if your VPS or container runtime cannot use Docker/macOS internal networks correctly.
# When false, Brain3 skips internal network creation and runs the MCP container on the runtime's
# normal default bridge/network, which restores outbound internet access from inside the container.
# Host loopback publishing still applies either way; this only changes the container's outbound routing.
B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=true
```

- [ ] **Step 2: Render the new variable from setup output**

In `build_overrides()` add:

```rust
values.insert(
    "B3_CONTAINER_INTERNAL_NETWORK_ISOLATION",
    draft.container_network_isolated.to_string(),
);
```

This is required because setup rendering only replaces keys that already exist in `.env.template`.

- [ ] **Step 3: Extend the focused env rendering test**

Update `render_env_file_applies_setup_defaults_and_quotes_values()` to assert:

```rust
assert!(rendered.contains("B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=\"true\""));
```

If the resulting test gets crowded, split out a second test that sets:

```rust
container_network_isolated: false,
```

and asserts the rendered `.env` contains `"false"`.

### Task 3: Load the Toggle From Config and Use It at Startup

**Files:**
- Modify: `crates/platform/src/config/env_file.rs`
- Modify: `crates/platform/src/container/startup.rs`

- [ ] **Step 1: Parse the env var with a secure default**

In `load_container_startup_config()` add:

```rust
let network_isolated = env_bool("B3_CONTAINER_INTERNAL_NETWORK_ISOLATION", true);
```

Then populate:

```rust
network_isolated,
```

inside the `ContainerStartupConfig` literal.

- [ ] **Step 2: Stop hardcoding isolation in container startup assembly**

In `ensure_mcp_container()`, replace:

```rust
network_isolated: true,
```

with:

```rust
network_isolated: startup.network_isolated,
```

- [ ] **Step 3: Add one explicit log line for the compatibility path**

Before building `ContainerConfig`, log the chosen mode:

```rust
tracing::info!(
    container = %startup.container_name,
    network_isolated = startup.network_isolated,
    "resolved MCP container network isolation mode"
);
```

This satisfies the repo preference for diagnosable startup behavior when a VPS needs the insecure fallback.

- [ ] **Step 4: Do not change the adapters or `EnsureContainerUseCase` logic**

No code changes should be needed in:

- `crates/platform/src/container/docker.rs`
- `crates/platform/src/container/macos_container.rs`
- `crates/core/src/application/ensure_container.rs`

Reason:

- when `network_isolated == true`, existing behavior already recreates the internal network and attaches the container to it
- when `network_isolated == false`, existing behavior already skips `prepare_network_isolation()` and omits `--network`, which naturally falls back to bridged/default runtime networking

### Task 4: Add the Toggle to the First-Run TUI

**Files:**
- Modify: `apps/gateway/src/tui/state.rs`
- Modify: `apps/gateway/src/tui/screens.rs`
- Modify: `apps/gateway/src/tui/app.rs`

- [ ] **Step 1: Add the new focus enums**

Extend the enum lists:

```rust
pub enum PortsField {
    GatewayPort,
    ContainerHostPort,
    ContainerMcpPort,
    AccessTokenLifetimeSecs,
    RefreshTokenLifetimeSecs,
    PkceRequired,
    EnforceHostnameCheck,
    ContainerNetworkIsolation,
}
```

and:

```rust
pub enum SummaryField {
    VaultPath,
    Username,
    ClientId,
    PasswordMode,
    PasswordValue,
    GatewayPort,
    ContainerHostPort,
    ContainerMcpPort,
    AccessTokenLifetimeSecs,
    RefreshTokenLifetimeSecs,
    PkceRequired,
    HostnameCheck,
    ContainerNetworkIsolation,
}
```

- [ ] **Step 2: Update focus navigation and toggling**

Wire the new enum value into:

- `next_ports_focus()`
- `previous_ports_focus()`
- `toggle_ports_boolean()`
- `next_summary_focus()`
- `previous_summary_focus()`
- `toggle_summary_field()`

The boolean mutations should be:

```rust
self.draft.container_network_isolated = !self.draft.container_network_isolated;
```

- [ ] **Step 3: Show the toggle on the Ports & Settings screen**

Add a focused badge row after the existing security toggles. Recommended label:

```text
Internal-only container networking: Enabled | Disabled
```

Add one short explanatory hint line nearby so users understand the tradeoff:

- enabled: maximum isolation, no default outbound route from the MCP container
- disabled: compatibility fallback for broken VPS/runtime internal networking

- [ ] **Step 4: Show the toggle on the Summary screen**

Add a summary badge row so the final review screen clearly shows whether setup will write:

```text
B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=true|false
```

- [ ] **Step 5: Reuse the current keybindings**

Do not add new controls. Keep:

- Ports & Settings: `t` to toggle the focused boolean
- Summary: `space` or `t` to toggle the focused boolean

This keeps the wizard behavior consistent with the existing PKCE and hostname-check toggles.

### Task 5: Update README and Keep Tests Minimal

**Files:**
- Modify: `README.md`
- Modify: `crates/platform/tests/setup_bootstrap.rs`
- Optionally modify: `crates/core/src/application/first_run_setup.rs`

- [ ] **Step 1: Update README security/config wording**

Adjust the privacy/security language so it remains accurate after this change. Recommended wording change:

- default behavior still uses container filesystem isolation plus internal-only networking
- compatibility mode exists for runtimes where internal networks do not work correctly
- disabling it increases risk because the container regains outbound internet access

- [ ] **Step 2: Add only public-behavior tests**

Stay within the repo's testing guidance:

- test env rendering/public setup output
- optionally test first-run default draft value
- do not add tests for logs
- do not add tests for private helper functions

If an extra test is needed in `first_run_setup.rs`, keep it focused on:

```rust
assert!(preparation.draft.container_network_isolated);
```

- [ ] **Step 3: Avoid redundant container-runtime tests unless a real gap appears**

`EnsureContainerUseCase` already covers the public `network_isolated` behavior, including the downgrade path when preparation fails. Do not add new adapter-level or internal-helper tests unless implementation uncovers an actual regression risk.

## Verification

- [ ] Run focused Rust tests:
  - `cargo test -p brain3-platform setup_bootstrap`
  - `cargo test -p brain3-core first_run_setup`
- [ ] Manual first-run TUI verification:
  - new install shows `Internal-only container networking` as enabled by default
  - toggling it to disabled updates the Summary screen
  - finalizing setup writes `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION="false"` into the generated `.env`
- [ ] Manual runtime verification with isolation enabled:
  - startup logs show network isolation enabled
  - runtime still recreates/uses `brain3-mcp-net`
  - `curl http://127.0.0.1:8420/health` from the host still works
- [ ] Manual runtime verification with isolation disabled:
  - startup logs show network isolation disabled
  - no `docker network rm/create` or `container network rm/create` attempt is made
  - `curl http://127.0.0.1:8420/health` from the host still works

## Expected Outcome

- Secure default remains unchanged for fresh installs and hand-written configs that omit the new variable.
- VPS users with broken internal-network support can set one boolean to `false` and continue using Brain3.
- The insecure fallback is visible in both `.env` and the setup TUI, so users are making an explicit tradeoff rather than silently losing isolation.
