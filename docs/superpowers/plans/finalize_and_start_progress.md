# Plan: Show progress during finalize_and_start

## Problem

`finalize_and_start` is called in the event loop body (`app.rs:186`). The loop only
draws at the top of each iteration. The `info_message = "Writing config..."` set
inside the function never renders — the terminal freezes for the entire 5-second
duration of `finalize` + `start_runtime_session`.

## Approach

Pass `terminal` into `finalize_and_start` and force `terminal.draw()` calls before
each blocking `.await`. No spinner, no refactor — user sees phase messages change
live instead of a frozen screen.

## Changes — `apps/gateway/src/tui/app.rs`

### 1. Update `finalize_and_start` signature (line ~270)

```rust
// Before:
async fn finalize_and_start(state: &mut FirstRunTuiState, use_case: &FirstRunSetupUseCase) {

// After:
async fn finalize_and_start(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    state: &mut FirstRunTuiState,
    use_case: &FirstRunSetupUseCase,
) {
```

### 2. Force draw before `use_case.finalize()` (inside finalize_and_start, line ~272)

```rust
state.clear_messages();
state.info_message = Some("Writing config…".into());
terminal.draw(|f| screens::draw(f, state)).ok();   // <-- add this line

let request = state.apply_inputs_to_draft();
let summary = match use_case.finalize(request).await ...
```

### 3. Force draw before `start_configured_runtime_session()` (line ~289)

```rust
    // after summary is obtained, before the session call:
    state.info_message = Some("Starting gateway…".into());
    terminal.draw(|f| screens::draw(f, state)).ok();   // <-- add this line

    let session = match start_configured_runtime_session(
```

### 4. Update call site in `event_loop` (line ~186)

```rust
// Before:
finalize_and_start(state, use_case).await;

// After:
finalize_and_start(terminal, state, use_case).await;
```

## Result

| Moment | User sees |
|---|---|
| Enter pressed | "Writing config…" immediately |
| `finalize` completes | "Starting gateway…" |
| Session starts | ConnectionCard / Connection Instructions screen |
