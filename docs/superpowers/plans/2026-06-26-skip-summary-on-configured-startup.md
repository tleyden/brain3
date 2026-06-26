# Plan: Skip Summary Confirmation Screen on Configured Startup

## Problem

Every time Brain3 starts with an existing `.env`, the TUI opens at Step 7 of 8
("Confirm what Brain3 will write before startup begins") and waits for the user to
press Enter before doing anything. This happens even on routine restarts where nothing
has changed. The user must manually confirm every single launch.

## Root Cause (confirmed by debug logs)

The startup decision tree:

1. `.env` exists → `plan_launch` returns `TuiConfigured` (`env_exists=true`)
2. Cloudflared config file exists at the path stored in the env file →
   `named_tunnel_setup_config` returns `None` (it only returns `Some` when the config
   file is **missing**, meaning cf-setup is still needed)
3. `GatewayTuiLaunch::Configured` is passed to `run_gateway_tui`
4. `FirstRunTuiState::new_configured` unconditionally sets `step = SetupStep::Summary`
5. The event loop starts and waits for `KeyCode::Enter` before calling `finalize_and_start`

`finalize_and_start` does two things when called from Summary:
- Calls `use_case.finalize(request)` — which re-writes the `.env` file
- Spawns the runtime startup task

On a configured launch the env file already contains valid, complete config. Re-writing
it from the draft (which was just loaded from the same file) is a no-op. The
confirmation screen adds no value on routine restarts.

Debug log evidence:

```
DEBUG brain3: launch dispatch resolved env_file=.../.env env_exists=true dispatch="TuiConfigured"
DEBUG brain3: TuiConfigured: CloudflareNamed tunnel — cf-setup screen shown only when config_file is missing
              tunnel_name=brain3-dev config_file=.../brain3-dev.yml config_file_exists=true
DEBUG brain3: TuiConfigured: cloudflared config file present → launching main TUI at Summary confirmation step
DEBUG brain3::tui::app: configured launch: TUI will open at Summary confirmation step before auto-starting
              env_file=.../.env initial_step="Summary"
```

## Goal

On a configured launch (existing `.env`, no pending cf-setup), skip the Summary screen
entirely and go straight to `SetupStep::RuntimeStatus`. The screen should still appear
when the user explicitly navigates to it (Esc back from RuntimeStatus → Summary for
reconfiguration), and during first-run wizard flow.

## What We Already Have

- `GatewayTuiLaunch::Configured` vs `GatewayTuiLaunch::FirstRun` — already
  distinguishes the two startup modes.
- `RuntimeStartupPolicy` is already passed through `event_loop` and into
  `finalize_and_start` — no new plumbing needed.
- `finalize_and_start` in `apps/gateway/src/tui/app.rs` already does the right thing:
  call `use_case.finalize`, spawn the runtime, advance to `RuntimeStatus`.

## Plan

### Step 1 — Auto-start in `run_gateway_tui` for configured launches

File: `apps/gateway/src/tui/app.rs`

After building `state` via `new_configured`, immediately call `finalize_and_start`
before entering the event loop:

```rust
GatewayTuiLaunch::Configured { launch_plan, startup_options } => {
    // ... load config, build preparation as today ...
    tracing::debug!(
        env_file = %launch_plan.env_file.display(),
        "configured launch: auto-starting, skipping Summary confirmation step"
    );
    let mut state = FirstRunTuiState::new_configured(host.to_string(), log_file.clone(), preparation);
    // Kick off startup immediately without waiting for Enter.
    finalize_and_start(&mut state, &use_case, runtime_overrides.clone(), startup_options.startup_policy).await;
    (state, startup_options.startup_policy, Some(startup_options.orphan_gc_rerun_command))
}
```

Because `finalize_and_start` sets `state.step = SetupStep::RuntimeStatus` and spawns
the startup task on a background channel (`startup_rx`), the event loop will enter
already at the Running screen with startup in progress.

### Step 2 — Keep Summary reachable for reconfiguration

The event loop already handles `KeyCode::Esc` at `RuntimeStatus` → navigate back through
the wizard. No change needed there. Summary will still be reachable for a user who wants
to reconfigure while the app is running.

### Step 3 — Update the debug log in `app.rs`

Change the existing debug log added during the RCA phase from:

```
"configured launch: TUI will open at Summary confirmation step before auto-starting"
```

To:

```
"configured launch: auto-starting, skipping Summary confirmation step"
```

### Step 4 — Verify

Run `cargo test` — no test changes expected since the auto-start path is not unit tested
(it is an integration-level TUI flow).

Then manually start Brain3 with an existing `.env`:
- Expect: TUI opens directly at Step 8 of 8 (Running) with "Starting Brain3..." visible,
  no confirmation step.
- Expect: log file shows the new debug line confirming auto-start.
- Verify: Esc navigation from the Running screen still reaches the Summary screen for
  reconfiguration.

## Files Changed

| File | Change |
|------|--------|
| `apps/gateway/src/tui/app.rs` | Call `finalize_and_start` immediately after `new_configured` in the `Configured` arm; update debug log message |

## No Behaviour Change for First-Run

`GatewayTuiLaunch::FirstRun` continues to start at `SetupStep::Welcome` and walk
through all wizard steps. Only the `Configured` arm changes.
