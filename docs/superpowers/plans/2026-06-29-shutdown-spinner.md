# Shutdown Spinner TUI Plan

**Goal:** When the user presses `q` to quit Brain3, instead of the TUI disappearing immediately (while the container/tunnel shutdown silently locks up in the background), keep the TUI alive and show a spinner with the message: "Shutting down Brain3 container, this should only take a few seconds".

---

## Problem

Current flow when `q` is pressed:

1. `event_loop` returns `Ok(())` immediately (app.rs:161)
2. `run_gateway_tui` tears down the TUI (leaves alternate screen, shows cursor)
3. `cleanup` runs — calls `server.shutdown()` and `runtime.shutdown_managed_runtime()`, which stops the container and tunnel — **this is the slow part, can take several seconds**
4. User sees a blank terminal that appears frozen — no feedback at all

## Solution

Keep the TUI alive during cleanup. When `q` is pressed:
1. Extract `server` and `runtime` out of `state`
2. Spawn a tokio task to run cleanup in the background
3. Transition to a new `ShuttingDown` step that renders a spinner + message
4. When the cleanup task finishes (signaled via oneshot channel), return from the event loop
5. TUI tears down — cleanup already completed, so the post-loop `cleanup()` call is a no-op

---

## Files to Change

### 1. `crates/core/src/domain/setup.rs` — Add `ShuttingDown` variant

```rust
pub enum SetupStep {
    Welcome,
    DependencyDoctor,
    VaultPath,
    AccessMode,
    Auth,
    PortsAndSettings,
    Summary,
    ConnectionCard,
    RuntimeStatus,
    ShuttingDown,   // ← new
}
```

### 2. `apps/gateway/src/tui/state.rs` — Add `cleanup_rx` field

Add to `FirstRunTuiState`:
```rust
pub cleanup_rx: Option<oneshot::Receiver<()>>,
```

Initialize to `None` in both `new()` and `new_configured()`.

### 3. `apps/gateway/src/tui/app.rs` — Wire up the shutdown flow

**a) Add `initiate_shutdown` helper:**
```rust
fn initiate_shutdown(state: &mut FirstRunTuiState) {
    let server = state.server.take();
    let mut runtime = state.runtime.take();
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        if let Some(s) = server {
            let _ = s.shutdown().await;
        }
        if let Some(ref mut r) = runtime {
            r.shutdown_managed_runtime().await;
        }
        let _ = tx.send(());
    });
    state.step = SetupStep::ShuttingDown;
    state.cleanup_rx = Some(rx);
}
```

**b) Replace the `q` handler in `event_loop`:**

Change:
```rust
if key.code == KeyCode::Char('q') {
    return Ok(());
}
```

To:
```rust
if key.code == KeyCode::Char('q') && state.step != SetupStep::ShuttingDown {
    initiate_shutdown(state);
    continue;
}
```

**c) Add cleanup completion check to `handle_runtime_tick`:**

In `handle_runtime_tick`, add at the top (return a bool indicating "done"):
```rust
fn handle_runtime_tick(state: &mut FirstRunTuiState) -> bool {
    state.tick_count = state.tick_count.wrapping_add(1);
    if matches!(state.step, SetupStep::RuntimeStatus) {
        state.refresh_runtime_logs();
    }
    if let Some(rx) = &mut state.cleanup_rx {
        if rx.try_recv().is_ok() {
            state.cleanup_rx = None;
            return true; // shutdown complete
        }
    }
    // ... existing probe_rx handling ...
    false
}
```

And in `event_loop`, check the return value:
```rust
if handle_runtime_tick(state) {
    return Ok(()); // cleanup done, exit event loop
}
```

**d) The `cleanup` call after `event_loop` remains** but is now always a no-op (server and runtime were already taken by `initiate_shutdown`). Keep it as a safety net.

### 4. `apps/gateway/src/tui/screens.rs` — Add ShuttingDown screen

Add a match arm in the `draw` function (or wherever steps are dispatched). The screen shows:

```
┌─────────────────────────────────────────────────┐
│                    Brain3                        │
│                                                  │
│   ⠹ Shutting down Brain3 container,             │
│     this should only take a few seconds          │
│                                                  │
└─────────────────────────────────────────────────┘
```

Use the existing `spinner_char(state.tick_count)` for the spinner frame.

The `action_lines` match should also handle `SetupStep::ShuttingDown` — return an empty vec (no keyboard hints while shutting down).

---

## Sequence Diagram

```
User presses 'q'
    │
    ▼
initiate_shutdown(state)
    ├─ state.server.take() → spawned task
    ├─ state.runtime.take() → spawned task
    ├─ state.step = ShuttingDown
    └─ state.cleanup_rx = Some(rx)
    │
    ▼
event_loop continues ticking
    ├─ draws ShuttingDown screen with spinner
    └─ each tick: checks cleanup_rx
           │
           ▼ (a few seconds later)
       cleanup_rx resolves → return Ok(())
    │
    ▼
run_gateway_tui tears down TUI
cleanup() called — state.server = None, state.runtime = None → no-op
```

---

## Edge Cases

- **User presses `q` before runtime is running** (e.g., on Welcome screen): `state.server` and `state.runtime` are both `None`, so the spawned task completes instantly. The spinner appears briefly and disappears.
- **Ctrl-C during shutdown**: The process exits; OS-level cleanup is already underway.
- **No runtime was started** (first-run wizard bailed early): Same as above — instant no-op shutdown.

---

## Out of Scope

- Showing a timeout/error if shutdown takes too long (can be a follow-up)
- Cancellation or force-quit keybind during shutdown
