# TUI Secret Masking

## Problem

Two security issues with the current TUI:

1. On the wizard path, after startup completes successfully, the app lands on `ConnectionCard` which immediately displays `client_secret`, `password`, and local MCP `bearer_token` in plain text. Anyone who can see the screen gets credentials without asking for them.

2. Even when the user navigates to `ConnectionCard` deliberately, secrets are always shown. There is no way to mask them.

## Changes

### 1. Start on RuntimeStatus, not ConnectionCard (wizard path)

**File:** `apps/gateway/src/tui/app.rs`

In `apply_startup_result`, the wizard path currently routes to `ConnectionCard` when a connection card is available:

```rust
if state.summary.is_some() {
    state.step = if state.connection_card.is_some() {
        SetupStep::ConnectionCard
    } else {
        SetupStep::RuntimeStatus
    };
}
```

Change to always land on `RuntimeStatus`:

```rust
if state.summary.is_some() {
    state.step = SetupStep::RuntimeStatus;
}
```

The `[c]` key already navigates to `ConnectionCard` from `RuntimeStatus` when `connection_card.is_some()`, so the screen is still reachable.

### 2. Add `secrets_revealed` toggle to TUI state

**File:** `apps/gateway/src/tui/state.rs`

Add field to `FirstRunTuiState`:

```rust
pub secrets_revealed: bool,
```

Initialize to `false` in `new()`.

### 3. Mask secrets in ConnectionCard

**File:** `apps/gateway/src/tui/screens.rs`

In `connection_card_lines`, replace each secret value with `"*".repeat(n)` when `!state.secrets_revealed`. The three fields to mask are:

- `card.client_secret`
- `card.password`
- `local_mcp.bearer_token`

Helper: `fn mask(s: &str) -> String { "*".repeat(s.len().max(8)) }` — always show at least 8 stars to avoid leaking length.

### 4. Wire `[s]` key on ConnectionCard

**File:** `apps/gateway/src/tui/app.rs`

In `SetupStep::ConnectionCard` key handler, add:

```rust
KeyCode::Char('s') => {
    state.secrets_revealed = !state.secrets_revealed;
}
```

### 5. Update action hints and body text

**File:** `apps/gateway/src/tui/screens.rs`

`action_lines` for `ConnectionCard`: add `[s] Reveal secrets` / `[s] Hide secrets` (toggled based on `state.secrets_revealed`).

`runtime_lines`: change "Press c to switch back to MCP config settings." to "Press c to view MCP connection details." — since we now arrive at RuntimeStatus first, "switch back" is no longer accurate.

## What does NOT change

- The Summary wizard screen still shows passwords in plain text — that is intentional since the user is actively configuring credentials and needs to see what they typed.
- The auth setup wizard screen uses `"*".repeat(n)` for password display already (line 366 in screens.rs), so no change needed there.
- The configured-launch path already starts on `RuntimeStatus` and is unaffected.

## Test impact

Existing tests in `screens.rs` that render `connection_card_lines` will still pass since the default state has `secrets_revealed = false` and the masked output doesn't break the assertions (none currently assert on secret values). Verify this by running `cargo test` after the change.

## Files touched

- `apps/gateway/src/tui/state.rs` — add `secrets_revealed: bool`
- `apps/gateway/src/tui/screens.rs` — mask secrets, update hints, update runtime hint text
- `apps/gateway/src/tui/app.rs` — change startup routing, wire `[s]` key
