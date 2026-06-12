Recommended Shape**
- Keep the existing `SetupStep::RuntimeStatus` flow and add a runtime subview, instead of creating a whole new wizard step. That fits the current event loop in [app.rs](/Users/tleyden/Development/brain3/apps/gateway/src/tui/app.rs:209) and keeps the screen “in place” as requested.
- Add a small log-tail helper under `apps/gateway/src/tui/` that incrementally reads the existing log file, keeps a bounded line buffer, and exposes scroll/follow state. That keeps `main.rs` untouched and avoids shoving file-tail logic into [state.rs](/Users/tleyden/Development/brain3/apps/gateway/src/tui/state.rs:24).
- Use `l` as a toggle between the current runtime summary and a logs view. In logs mode: `Up/Down` scroll one line, `PgUp/PgDn` scroll a page, `End` jumps back to live-follow.

**Why this approach**
- It matches “add a new `[l] Logs` button” without duplicating the runtime-status screen.
- It avoids a file-watcher dependency. The existing TUI already redraws on a 200ms poll loop, so log refresh can piggyback on that.
- It keeps the current architecture clean: render in [screens.rs](/Users/tleyden/Development/brain3/apps/gateway/src/tui/screens.rs:16), input handling in [app.rs](/Users/tleyden/Development/brain3/apps/gateway/src/tui/app.rs:68), state in [state.rs](/Users/tleyden/Development/brain3/apps/gateway/src/tui/state.rs:24), log-tail details in a new focused module.

**Concrete File Plan**
1. Create `apps/gateway/src/tui/runtime_logs.rs`.
   This will own:
   - incremental reads from `state.log_file`
   - byte offset tracking
   - partial-line carryover between reads
   - bounded retained history, e.g. last `1000-2000` lines
   - scroll/follow helpers
   - truncation handling if the log file is recreated

2. Modify [apps/gateway/src/tui/state.rs](/Users/tleyden/Development/brain3/apps/gateway/src/tui/state.rs:24).
   Add:
   - `RuntimeView { Status, Logs }`
   - `runtime_view`
   - `runtime_logs` helper state
   - small methods like `toggle_runtime_view()`, `scroll_logs_up()`, `scroll_logs_down()`, `jump_logs_to_end()`

3. Modify [apps/gateway/src/tui/app.rs](/Users/tleyden/Development/brain3/apps/gateway/src/tui/app.rs:68).
   Add:
   - per-loop log refresh while `state.step == SetupStep::RuntimeStatus`
   - `l` key handling to toggle views
   - log-scroll keys when logs view is active
   - keep `c` working exactly as it does now for the connection-card return path

4. Modify [apps/gateway/src/tui/screens.rs](/Users/tleyden/Development/brain3/apps/gateway/src/tui/screens.rs:16).
   Change:
   - body panel title to switch between `Runtime Status` and `Logs`
   - runtime body rendering so logs mode uses a scrollable `Paragraph`
   - footer/actions so the runtime screen shows `[l] Logs` in status mode and log navigation hints in logs mode

5. Modify `apps/gateway/src/tui/mod.rs`.
   Add the new module declaration.

**Behavior Details**
- First press of `l`: open logs view, positioned at the newest retained lines.
- While at bottom: new log entries auto-follow.
- If the user scrolls up: auto-follow pauses so the viewport does not jump.
- `End`: re-enable follow and jump back to live tail.
- If the log file is temporarily empty or unreadable: show that in the logs pane, not as a fatal screen error.

**Verification**
- `cargo build -p brain3`
- Manual smoke only. I would not add ratatui snapshot tests or log-string unit tests here.
- Smoke checklist:
  - reach runtime screen
  - press `l` and see existing log content
  - generate new log lines and confirm they stream in
  - scroll up and confirm new lines do not yank the viewport
  - press `End` and confirm live-follow resumes
  - press `c` and confirm the existing config-card navigation still works

**Alternatives I would not choose**
- New `SetupStep::RuntimeLogs`: works, but duplicates screen wiring and footer logic.
- Always-visible split logs panel: uses space poorly and does not match the requested `[l]` action.