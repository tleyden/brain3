# Plan: Access Mode Screen in Onboarding Wizard

**Date**: 2026-06-24
**Status**: Proposed

## Overview

Insert a new `AccessMode` wizard step between `VaultPath` and `Auth`. Once the user presses Enter to leave this screen, the choice is **locked** — backward navigation skips it. The Auth step is skipped entirely for `LocalOnly`. Ports & Settings hides OAuth-only fields when `LocalOnly`. Remote modes default to `B3_CF_QUICK_TUNNEL=true`.

**New flow:**
```
Welcome → Dependencies → Vault → Access Mode → (Auth*) → Ports → Summary → …
                                     locked↑          ↑ skipped if LocalOnly
```
*Esc from Auth/Ports skips AccessMode entirely and goes to VaultPath.*

---

## 1. `crates/core/src/domain/setup.rs`

**Add `AccessModeDraft`:**
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessModeDraft { LocalOnly, RemoteOnly, Both }
```

**Add to `SetupDraftConfig`:**
```rust
pub access_mode: AccessModeDraft,  // default: Both
```

**Add `TunnelModeDraft::Disabled` variant** to express "no tunnel, don't write B3_CF_QUICK_TUNNEL=true":
```rust
pub enum TunnelModeDraft {
    Disabled,          // new
    CloudflareQuick,
    CloudflareNamed { tunnel_name: String, domain: String },
    DirectPublicOrigin { hostname: String },
}
```

**Add to `SetupStep`:**
```rust
pub enum SetupStep {
    Welcome, DependencyDoctor, VaultPath,
    AccessMode,   // new
    Auth, PortsAndSettings, Summary, ConnectionCard, RuntimeStatus,
}
```

**Remove** `LocalMcpEnabled` from `PortsField` and `SummaryField` enums — `access_mode` owns this now.

---

## 2. `crates/core/src/application/first_run_setup.rs`

In `prepare()`:
- `access_mode: AccessModeDraft::Both`
- `tunnel_mode: TunnelModeDraft::CloudflareQuick` (unchanged)

In `finalize()`, **before** calling `render_env_file`, enforce access mode policy:

| `access_mode` | `local_mcp_enabled` | `tunnel_mode` |
|---|---|---|
| `LocalOnly` | `true` | `Disabled` |
| `RemoteOnly` | `false` | `CloudflareQuick` (keep as-is) |
| `Both` | `true` | `CloudflareQuick` (keep as-is) |

`B3_CF_QUICK_TUNNEL` gets `true` when remote is involved (env template renders `CloudflareQuick` → `B3_CF_QUICK_TUNNEL=true`), and `false`/omitted for `Disabled`.

---

## 3. `apps/gateway/src/tui/state.rs`

**Add `AccessModeField`:**
```rust
pub enum AccessModeField { LocalOnly, RemoteOnly, Both }
```

**Add to `FirstRunTuiState`:**
```rust
pub access_mode_focus: AccessModeField,  // init: Both
pub access_mode_locked: bool,            // init: false
```

**Add methods:**
- `next_access_mode_focus()` / `previous_access_mode_focus()` — cycles `LocalOnly → RemoteOnly → Both → …`; also updates `draft.access_mode` to match (cursor = live selection)
- `confirm_access_mode()` — sets `access_mode_locked = true`

**`next_ports_focus()` / `previous_ports_focus()`** — take `access_mode: &AccessModeDraft` and skip these fields when `LocalOnly`:
- `AccessTokenLifetimeSecs`, `RefreshTokenLifetimeSecs`, `PkceRequired`, `EnforceHostnameCheck`

Also **remove** `LocalMcpEnabled` from the `PortsField` navigation cycle entirely (access_mode drives local_mcp_enabled for all modes).

**`previous_step()`** — updated:
- `AccessMode` → `VaultPath`
- `Auth` → `VaultPath` (skips locked AccessMode)
- `PortsAndSettings` → `Auth` if `Both`/`RemoteOnly`; `VaultPath` if `LocalOnly` (skips locked AccessMode)

**Remove** `LocalMcpEnabled` from `SummaryField` navigation cycle and `toggle_summary_field()`.

---

## 4. `apps/gateway/src/tui/screens.rs`

**Progress bar stages** — grows from 7 to 8:
```
Welcome – Dependencies – Vault – Access – Auth – Ports – Start – Running
```
(When `LocalOnly`, Auth stage still appears in the bar but was skipped — it shows as `○` rather than `✓`, acceptable since users cannot navigate back to it.)

**`screen_title`:** `AccessMode` → `"Local/Remote Access"`

**`progress_caption`:** `AccessMode` → `"Choose how AI clients will connect to Brain3."`

**`body_lines` dispatch:** add `AccessMode => access_mode_lines(state)`

**Implement `access_mode_lines(state)`:**

Renders three options. `▶` marks the focused option (cursor). `●` marks the option matching `draft.access_mode`. Cursor movement live-updates `draft.access_mode`, so cursor and selection are always in sync.

```
  Choose how Brain3 will be accessed by AI clients.

▶ [ ● ] Local MCP only
        Most secure, but only works from apps running locally on this
        machine. May require directly editing a JSON config file for
        some apps.

  [   ] Remote MCP only
        Your Brain3 gateway will be reachable via the public internet.
        Enables commercial AI mobile and web apps with GUI configuration.
        Higher security risk.

  [   ] Both local and remote MCP
        Combines the tradeoffs of both options.
```

**`action_lines` for `AccessMode`:**
```
[↑↓] Move    [Space] Select    [Esc] Back    [Enter] Continue
```

**`ports_and_settings_lines(state)`** — conditionally render based on `state.draft.access_mode`:
- Always shown: Gateway port, Container host port, Container MCP port, Internal-only container networking
- Only shown when **not** `LocalOnly`: Access token lifetime, Refresh token lifetime, PKCE required, Enforce hostname check
- **Removed entirely**: Local MCP access toggle (was `LocalMcpEnabled` field — no longer user-configurable here)

**`summary_lines(state)`** — replace the toggleable `field_badge_line("Local MCP access", ...)` row with a **read-only** `key_value_line("Access mode", format_access_mode(&state.draft.access_mode))`. No cursor, no toggle.

**`format_access_mode()` helper:**
```rust
fn format_access_mode(mode: &AccessModeDraft) -> &'static str {
    match mode {
        AccessModeDraft::LocalOnly => "Local MCP only",
        AccessModeDraft::RemoteOnly => "Remote MCP only",
        AccessModeDraft::Both => "Both local and remote MCP",
    }
}
```

---

## 5. `apps/gateway/src/tui/app.rs`

**`advance_from_vault_path()`** — change destination from `SetupStep::Auth` to `SetupStep::AccessMode`.

**Add `SetupStep::AccessMode` arm in event loop:**
```rust
SetupStep::AccessMode => match key.code {
    KeyCode::Esc => {
        state.clear_messages();
        state.step = SetupStep::VaultPath;
    }
    KeyCode::Up => state.previous_access_mode_focus(),
    KeyCode::Down => state.next_access_mode_focus(),
    KeyCode::Char(' ') => {
        // draft.access_mode is already in sync with focus via movement;
        // Space provides the expected selection affordance without extra logic
    }
    KeyCode::Enter => {
        state.confirm_access_mode();
        state.step = match state.draft.access_mode {
            AccessModeDraft::LocalOnly => SetupStep::PortsAndSettings,
            _ => SetupStep::Auth,
        };
    }
    _ => {}
}
```

**`SetupStep::Auth` Esc:** `state.step = SetupStep::VaultPath` (skips locked AccessMode).

**`SetupStep::PortsAndSettings` Esc:**
```rust
state.step = match state.draft.access_mode {
    AccessModeDraft::LocalOnly => SetupStep::VaultPath,  // skips locked AccessMode
    _ => SetupStep::Auth,
};
```

**`SetupStep::PortsAndSettings` Enter validation:** skip validation for `AccessTokenLifetimeSecs` and `RefreshTokenLifetimeSecs` when `LocalOnly` (fields are hidden, inputs may be stale from defaults).

**`SetupStep::PortsAndSettings` Tab/Up/Down handlers:** pass `&state.draft.access_mode` to `next_ports_focus()` / `previous_ports_focus()`.

---

## 6. `crates/platform/src/setup/env_template.rs`

Ensure `TunnelModeDraft::Disabled` renders `B3_CF_QUICK_TUNNEL=false` (or is omitted with a comment). `CloudflareQuick` continues to render as `B3_CF_QUICK_TUNNEL=true`.

---

## What is removed relative to prior design

- `LocalMcpEnabled` is no longer a focusable/togglable field in Ports or Summary — `access_mode` owns it
- No backward navigation through the AccessMode screen after it is locked
- OAuth-only Ports fields (token lifetimes, PKCE, hostname check) are hidden for `LocalOnly`
