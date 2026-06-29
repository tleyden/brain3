# Plan: Only write `.env` during the onboarding wizard

**Date:** 2026-06-29

## Problem

On every quit and restart, the `.env` file is written and its content changes. Root cause: `finalize_and_start()` is called unconditionally in the `GatewayTuiLaunch::Configured` branch (`app.rs:94`). It always calls `use_case.finalize()` → `write_env_file()`. This branch fires on every restart when an `.env` already exists.

A secondary content-change bug: `B3_CONTAINER_IMAGE_TAG` is not preserved through the config round-trip. The load path absorbs it into the fully-qualified image string; `image_repo_from_reference` strips the tag back off; `env_writer.rs:73` always writes the tag as empty. A pinned tag is silently lost on every restart.

## Rule

- **No `.env` file** → onboarding wizard → write `.env` exactly once on wizard completion.
- **`.env` file exists** → normal startup → `.env` is never touched.

---

## All changes are in one file: `apps/gateway/src/tui/app.rs`

### Change 1 — Add `start_without_writing_env()`

Add a new private async function alongside `finalize_and_start`:

```rust
async fn start_without_writing_env(
    state: &mut FirstRunTuiState,
    launch_plan: RuntimeLaunchPlan,
    runtime_overrides: RuntimeOverrides,
    startup_policy: RuntimeStartupPolicy,
) {
    state.clear_messages();
    let host = state.host.clone();
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = tx.send(
            start_configured_runtime_session(&host, launch_plan, runtime_overrides, startup_policy)
                .await,
        );
    });
    // state.summary intentionally left as None — apply_startup_result uses its None-ness
    // to know it should pull credentials from the loaded runtime config, not from a wizard summary.
    state.startup_rx = Some(rx);
    state.info_message = Some("Starting Brain3...".into());
    state.step = SetupStep::RuntimeStatus;
}
```

### Change 2 — Replace the call at `app.rs:94-100`

```rust
// Before:
finalize_and_start(
    &mut state,
    &use_case,
    runtime_overrides.clone(),
    startup_options.startup_policy,
)
.await;

// After:
start_without_writing_env(
    &mut state,
    launch_plan,   // already in scope from the GatewayTuiLaunch::Configured destructure
    runtime_overrides.clone(),
    startup_options.startup_policy,
)
.await;
```

---

## What does NOT change

- `finalize_and_start()` itself is untouched. It remains the only writer of `.env`, called only from the Summary screen's Enter-key handler (`app.rs:419`).
- `FirstRunSetupUseCase::finalize()`, `env_writer.rs`, `system.rs`, `main.rs`, `plan_launch()` — all unchanged.

---

## Invariant after this change

| Situation | Path | `.env` written? |
|---|---|---|
| No `.env` exists | `TuiFirstRun` → wizard steps → Summary Enter → `finalize_and_start` | Yes, exactly once |
| `.env` exists | `TuiConfigured` → `start_without_writing_env` | Never |

---

## Incidental UX improvement

Currently, restarting shows the Summary screen (because `new_configured` starts at `SetupStep::Summary`) and waits for Enter before launching. With this change the app jumps straight to `RuntimeStatus` before the event loop begins — no more "confirm your own existing settings" prompt on every restart.

---

## Out of scope (future)

- "Re-run onboarding wizard" override flag (e.g. `--reconfigure`) — not part of this change.
- Preserving `B3_CONTAINER_IMAGE_TAG` through the round-trip — the tag-loss bug is fixed implicitly because the configured path no longer re-renders the env file at all.
