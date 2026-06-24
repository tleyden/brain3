# Plan: Show Local MCP Port (8422) and Hide Gateway Port (8421) in Local-Only Mode

**Date**: 2026-06-24
**Status**: Proposed

## Problem

In local-only mode the Ports step and Summary step both display "Gateway port: 8421" as
an editable field. The gateway does not bind that port in local-only mode, so showing it
is misleading. The actual useful port — the local MCP port (8422) — is never shown or
editable anywhere in the TUI, even though it is what the user needs to configure Claude
Code's `.mcp.json`.

The draft already carries `local_mcp_enabled` and `local_mcp_bearer_token`, but has no
`local_mcp_port` field. The port is hardcoded via `DEFAULT_LOCAL_MCP_PORT = 8422` in
`env_writer.rs` and never surfaces in the TUI.

---

## Changes

### 1. `crates/core/src/domain/setup.rs`

Add `local_mcp_port: u16` to `SetupDraftConfig`, defaulting to `DEFAULT_LOCAL_MCP_PORT`:

```rust
pub struct SetupDraftConfig {
    // ... existing fields ...
    pub local_mcp_port: u16,   // new
}
```

### 2. All `SetupDraftConfig` construction sites

Add `local_mcp_port: DEFAULT_LOCAL_MCP_PORT` to every struct literal. The compiler will
flag all sites:
- `crates/core/src/application/first_run_setup.rs` (multiple)
- `crates/platform/tests/setup_bootstrap.rs`
- `apps/gateway/src/tui/screens.rs` (test helpers)

### 3. `crates/platform/src/setup/env_writer.rs`

Replace the hardcoded constant with `draft.local_mcp_port` when writing `B3_LOCAL_MCP_PORT`:

```rust
// before
values.insert("B3_LOCAL_MCP_PORT", DEFAULT_LOCAL_MCP_PORT.to_string());
// after
values.insert("B3_LOCAL_MCP_PORT", draft.local_mcp_port.to_string());
```

### 4. `apps/gateway/src/tui/state.rs` — enums

Add `LocalMcpPort` to both `PortsField` and `SummaryField`:

```rust
pub enum PortsField {
    LocalMcpPort,   // new
    GatewayPort,
    // ...
}

pub enum SummaryField {
    LocalMcpPort,   // new
    GatewayPort,
    // ...
}
```

Update `ports_focus_order`:
- `LOCAL_ONLY_ORDER`: replace `GatewayPort` with `LocalMcpPort`
- `REMOTE_ORDER`: keep `GatewayPort`; add `LocalMcpPort` after it if local MCP is enabled
  (pass `local_mcp_enabled` flag into the function, or use a dynamic `Vec` for this case)

Update `summary_focus_order`:
- `LOCAL_ONLY_ORDER`: replace `GatewayPort` with `LocalMcpPort`
- Remote orders: keep `GatewayPort` unchanged

### 5. `apps/gateway/src/tui/state.rs` — `FirstRunTuiState`

Add `local_mcp_port_input: String`, initialized to `DEFAULT_LOCAL_MCP_PORT.to_string()`.

### 6. `apps/gateway/src/tui/state.rs` — `ports_focus_is_text_field`

Add `PortsField::LocalMcpPort` to the `matches!` list.

### 7. `apps/gateway/src/tui/app.rs` — keyboard handler

Add `PortsField::LocalMcpPort` to the `Backspace` and `Char(ch)` match arms (digit-only,
same as gateway port), pushing/popping from `state.local_mcp_port_input`.

Add `validate_port_input(&state.local_mcp_port_input, "Local MCP port")` to the Ports
step "Next" validation block (always validate; it's always shown in local-only mode and
optionally in both mode).

### 8. `apps/gateway/src/tui/screens.rs` — Ports step (`ports_and_settings_lines`)

Branch on `access_mode`:

- **LocalOnly**: show `Local MCP port` (editable) instead of `Gateway port`; drop
  `AccessTokenLifetimeSecs`, `RefreshTokenLifetimeSecs`, `PkceRequired`,
  `EnforceHostnameCheck` (already hidden by the existing `!= LocalOnly` guard).
- **RemoteOnly**: keep existing layout (gateway port only, no local MCP port row).
- **Both**: show gateway port first, then local MCP port below it.

### 9. `apps/gateway/src/tui/screens.rs` — Summary step (`summary_lines`)

Same branching logic:

- **LocalOnly**: replace the `Gateway port` row (currently shows "Disabled" badge) with
  `Local MCP port` (editable field using `SummaryField::LocalMcpPort`).
- **RemoteOnly / Both**: keep gateway port; add local MCP port row when
  `local_mcp_enabled`.

### 10. Draft sync

When the user edits `local_mcp_port_input` and advances past the Ports step, copy the
parsed value into `state.draft.local_mcp_port` (same place gateway port is synced in
`finalize_ports` or equivalent).

---

## What stays the same

- OAuth2.1 mode on the public port is unchanged for Remote/Both modes.
- `env_writer.rs` token-writing logic is untouched.
- Runtime behavior is unchanged — we're only making the TUI reflect and allow editing the
  port that `env_writer.rs` already writes.
- No changes to container networking, health probes, or the MCP proxy.

---

## Implementation order

1. `setup.rs` — add `local_mcp_port` field (triggers compiler errors at all call sites).
2. Fix all `SetupDraftConfig` literals.
3. `env_writer.rs` — use `draft.local_mcp_port`.
4. `state.rs` — add enum variants, update focus orders, add `local_mcp_port_input`.
5. `app.rs` — wire keyboard input and validation.
6. `screens.rs` — update Ports and Summary display.
7. `cargo test`.
