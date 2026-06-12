# Runtime Logs In TUI Implementation Plan

> **For agentic workers:** Execute this plan inline and serially. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an `[l] Logs` subview to the existing runtime status screen so the gateway TUI can show live logs by tailing the already-written log file, with bounded history and simple follow/scroll controls.

**Architecture:** Keep `SetupStep::RuntimeStatus` as the only runtime wizard step. Add one focused `runtime_logs` helper under `apps/gateway/src/tui/` that tails `state.log_file` on the existing 200ms TUI poll cadence, stores a bounded in-memory line buffer, and owns follow/scroll state. Keep input handling in `app.rs`, runtime-view state in `state.rs`, rendering in `screens.rs`, and leave `main.rs` plus the tracing pipeline untouched.

**Tech Stack:** Rust, ratatui, crossterm, `std::fs`, `std::io`, `std::collections::VecDeque`, existing Tokio runtime.

---

## Scope And Non-Goals

- Keep the current runtime screen in place; do not add a new `SetupStep::RuntimeLogs`.
- Do not refactor the TUI into a generic event bus first.
- Do not change `apps/gateway/src/logging.rs` to tee logs in memory for this slice.
- Do not add a file watcher dependency.
- Do not add ratatui snapshot tests or unit tests that assert log strings.
- Verification is `cargo build -p brain3` plus manual smoke on the runtime screen.

## File Map

- Create: `apps/gateway/src/tui/runtime_logs.rs`
  - Owns incremental file-tail logic, retained history, follow mode, viewport state, and non-fatal read/truncation handling.
- Modify: `apps/gateway/src/tui/state.rs`
  - Adds runtime subview state and thin delegating helpers.
- Modify: `apps/gateway/src/tui/app.rs`
  - Adds one small runtime tick path plus logs key handling.
- Modify: `apps/gateway/src/tui/screens.rs`
  - Adds a logs body renderer and mode-specific footer text.
- Modify: `apps/gateway/src/tui/mod.rs`
  - Exposes the new module.

## Data Shape

### `apps/gateway/src/tui/runtime_logs.rs`

Introduce a focused helper with a small public API:

```rust
pub enum RuntimeLogsState {
    Loading,
    Ready,
    Empty,
    Unavailable(String),
}

pub struct RuntimeLogs {
    path: PathBuf,
    byte_offset: u64,
    partial_line: String,
    lines: VecDeque<String>,
    max_lines: usize,
    follow: bool,
    scroll_from_bottom: usize,
    state: RuntimeLogsState,
}
```

Expected methods:

```rust
impl RuntimeLogs {
    pub fn new(path: PathBuf) -> Self;
    pub fn refresh(&mut self);
    pub fn lines(&self) -> &VecDeque<String>;
    pub fn state(&self) -> &RuntimeLogsState;
    pub fn is_following(&self) -> bool;
    pub fn scroll_up(&mut self, lines: usize);
    pub fn scroll_down(&mut self, lines: usize);
    pub fn jump_to_end(&mut self);
    pub fn scroll_offset_for_height(&self, height: usize) -> u16;
}
```

Behavior rules:

- Read only appended bytes from the current log file.
- If file length shrinks below `byte_offset`, treat it as truncation/recreation and reset internal offsets plus partial-line carryover.
- Keep only the last `2000` lines in memory.
- Preserve partial trailing content until a newline arrives.
- `scroll_from_bottom == 0` means follow mode.
- Read failures must become `RuntimeLogsState::Unavailable(...)` in the logs pane, not a fatal TUI error.

### `apps/gateway/src/tui/state.rs`

Add:

```rust
pub enum RuntimeView {
    Status,
    Logs,
}
```

Extend `FirstRunTuiState` with:

```rust
pub runtime_view: RuntimeView,
pub runtime_logs: RuntimeLogs,
```

Thin helpers only:

```rust
pub fn toggle_runtime_view(&mut self);
pub fn show_runtime_logs(&mut self);
pub fn show_runtime_status(&mut self);
pub fn refresh_runtime_logs(&mut self);
pub fn scroll_logs_up(&mut self, lines: usize);
pub fn scroll_logs_down(&mut self, lines: usize);
pub fn jump_logs_to_end(&mut self);
```

Keep file-tail internals out of `state.rs`. This file should only route intent to the helper.

## Implementation Tasks

### Task 1: Add The Runtime Logs Helper

**Files:**
- Create: `apps/gateway/src/tui/runtime_logs.rs`

- [ ] Add `RuntimeLogsState` and `RuntimeLogs`.
- [ ] Implement `new(path)` with defaults:
  - `max_lines = 2000`
  - `byte_offset = 0`
  - `partial_line = ""`
  - `follow = true`
  - `scroll_from_bottom = 0`
  - `state = Loading`
- [ ] Implement `refresh()` using `std::fs::metadata`, `std::fs::File`, `Seek`, and `Read`.
- [ ] On truncation/recreation, reset offsets and reread from the beginning of the new file.
- [ ] Split appended bytes into complete lines, preserve any trailing partial line, and append complete lines into `VecDeque<String>`.
- [ ] Trim retained history to `max_lines`.
- [ ] Update `state` to `Empty` when the file exists but no complete lines are retained.
- [ ] Update `state` to `Unavailable(...)` when the file cannot be read.

### Task 2: Add Runtime View State Without Broad Refactoring

**Files:**
- Modify: `apps/gateway/src/tui/state.rs`

- [ ] Add `RuntimeView`.
- [ ] Add `runtime_view` and `runtime_logs` to `FirstRunTuiState`.
- [ ] Initialize `runtime_view` to `RuntimeView::Status` in both `new()` and `new_runtime()`.
- [ ] Initialize `runtime_logs` from the existing `log_file` path in both constructors.
- [ ] Add the thin delegating helpers listed above.
- [ ] Keep `previous_step()`, connection-card behavior, and gateway-status reporting unchanged.

### Task 3: Add A Small Runtime Tick Path In The TUI Loop

**Files:**
- Modify: `apps/gateway/src/tui/app.rs`

- [ ] Extract one small helper such as:

```rust
fn handle_runtime_tick(state: &mut FirstRunTuiState) {
    if state.step == SetupStep::RuntimeStatus {
        state.refresh_runtime_logs();
    }
}
```

- [ ] Call that helper once per loop on the existing cadence. Keep the current redraw/poll structure; do not introduce channels, background readers, or a general event-processing refactor.
- [ ] Extend `SetupStep::RuntimeStatus` key handling:
  - `l` toggles between `RuntimeView::Status` and `RuntimeView::Logs`
  - first entry into logs view jumps to end so the newest retained lines are visible
  - `Up` / `Down` scroll one line in logs view
  - `PageUp` / `PageDown` scroll a page in logs view
  - `End` re-enables follow mode and jumps to the live tail
  - `c` keeps the existing connection-card return behavior
  - `q` remains global quit
- [ ] When the user scrolls up, stop auto-follow by increasing `scroll_from_bottom`.
- [ ] When scrolling back down to `0`, resume follow mode automatically.

### Task 4: Render Logs As A Dedicated Runtime Subview

**Files:**
- Modify: `apps/gateway/src/tui/screens.rs`

- [ ] Keep the current runtime status summary renderer for `RuntimeView::Status`.
- [ ] Add a dedicated logs renderer instead of trying to force logs through the generic wrapped `body_lines()` path.
- [ ] In `draw()`, branch the body widget for:
  - standard screens
  - runtime status mode
  - runtime logs mode
- [ ] For logs mode:
  - title the panel `Logs`
  - render retained log lines in a `Paragraph`
  - disable wrapping for log content
  - apply vertical scroll using `RuntimeLogs::scroll_offset_for_height(...)`
  - append one short top/bottom status hint such as `LIVE` or `SCROLLED`
- [ ] Render empty/unavailable states inside the body panel with muted or warning styling.
- [ ] Update footer hints:
  - status mode: show `[l] Logs`
  - logs mode: show `[l] Status`, `[Up/Down]`, `[PgUp/PgDn]`, `[End] Live`
  - keep `[c]` when the connection card exists
  - keep `[q] Quit`

### Task 5: Wire The New Module

**Files:**
- Modify: `apps/gateway/src/tui/mod.rs`

- [ ] Add `mod runtime_logs;`.
- [ ] Update imports in `state.rs`, `app.rs`, and `screens.rs` to use the new helper cleanly.

## Behavior Checklist

- [ ] Pressing `l` from runtime status opens the logs view at the newest retained lines.
- [ ] While `scroll_from_bottom == 0`, new log lines appear automatically.
- [ ] Scrolling up pauses follow so the viewport does not jump.
- [ ] `End` jumps back to the tail and resumes follow.
- [ ] If the log file is empty, the pane says so clearly.
- [ ] If the log file is temporarily unreadable or missing, the pane shows that as a non-fatal runtime-logs state.
- [ ] Pressing `c` from either runtime subview still returns to the connection card when present.

## Verification

### Required

- [ ] Run: `cargo build -p brain3`
- [ ] Expected: successful build with no new warnings introduced by this change set

### Manual Smoke

- [ ] Launch the gateway TUI and reach `SetupStep::RuntimeStatus`.
- [ ] Press `l` and confirm the logs view opens with existing log content or a clear empty-state message.
- [ ] Generate new log lines and confirm they stream in while the view is at the live tail.
- [ ] Scroll up with `Up` or `PageUp` and confirm new lines do not yank the viewport.
- [ ] Press `End` and confirm the view jumps back to the live tail and resumes auto-follow.
- [ ] Press `l` again and confirm the runtime status summary returns.
- [ ] Press `c` and confirm the connection-card path still works exactly as before.

## Deferred Follow-Up, Not In This Slice

- Replace file tailing with an in-memory bounded log tee in `apps/gateway/src/logging.rs`.
- Introduce a richer runtime model if the TUI later needs live tunnel/container/server events beyond log output.
- Refactor the TUI loop into explicit `Tick` and `Key` event types if more live runtime widgets arrive.
