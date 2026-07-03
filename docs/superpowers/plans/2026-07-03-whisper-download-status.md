# Whisper Download Status Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Do not use subagents for this repository. Do not commit; the user will commit.

**Goal:** When the user presses `Save & Start` and Brain3 needs to download a Whisper model, the TUI immediately shows a live "Downloading Whisper model..." status instead of appearing frozen.

**Architecture:** Keep the fix in the TUI/application boundary. The setup finalization phase currently blocks the TUI key handler while `FirstRunSetupUseCase::finalize()` downloads and verifies the model; move that phase into a background task and poll it from the existing runtime tick path. Do not add a generic progress framework or byte-level progress in this pass.

**Tech Stack:** Rust, Tokio `oneshot`, existing ratatui TUI state, existing `FirstRunSetupUseCase`, existing `cargo test` verification.

---

## Current Root Cause

`apps/gateway/src/tui/app.rs::finalize_and_start()` awaits `use_case.finalize(request)` before it switches `state.step` to `SetupStep::RuntimeStatus`. When native audio transcription is enabled, `finalize()` calls `ensure_whisper_model()`, which may download 78 MB to 1.5 GB and checksum-verify it. Because this await happens inside the TUI key handler, the event loop cannot redraw, so the Summary screen looks frozen.

## Target Behavior

- Pressing `Enter` on Summary should immediately switch to Runtime Status.
- If native audio transcription is enabled, the status area should show a spinner and `Downloading Whisper model...`.
- If native audio transcription is disabled, the status area should show a spinner and `Saving configuration...`.
- When finalization succeeds, existing runtime startup begins and the status changes to `Starting Brain3...`.
- If finalization fails, the UI returns to Summary with the error message visible.
- Once Runtime Status is running, it should show whether native audio transcription is enabled.
- Existing configured startup behavior remains unchanged.

## Files

- Modify: `apps/gateway/src/tui/state.rs`
  - Add a `finalize_rx` field to `FirstRunTuiState`.
- Modify: `apps/gateway/src/tui/app.rs`
  - Background setup finalization.
  - Poll finalization result from `handle_runtime_tick`.
  - Start runtime after finalization succeeds.
  - Add focused unit tests.
- Modify: `apps/gateway/src/tui/screens.rs`
  - Treat `finalize_rx` as an active spinner task in `status_lines`.
  - Optionally display a setup-finalization body line while finalize is pending.
  - Add native audio transcription enabled/disabled status to the Runtime Status body.
- Modify: `crates/core/src/application/first_run_setup.rs`
  - Derive `Clone` for `FirstRunSetupUseCase` so the TUI can move a use-case handle into a background finalize task.
  - Keep `finalize()` responsible for validation, model download, `.env` writing, and summary production.
- Modify only if needed: `crates/core/src/domain/setup.rs`
  - Derive `Clone` for `SetupDefaults` if it is not already cloneable.
- No changes: `crates/platform/src/setup/system.rs`
  - Keep existing download implementation and logging.

---

## Task 1: Add TUI State for Background Finalization

**Files:**
- Modify: `apps/gateway/src/tui/state.rs`

- [ ] **Step 1: Add the failing compile target**

Run:

```bash
cargo test -p brain3 --no-run
```

Expected before implementation: compilation fails after later tests reference `state.finalize_rx`.

- [ ] **Step 2: Add `finalize_rx` to `FirstRunTuiState`**

In `apps/gateway/src/tui/state.rs`, update imports only if needed. `oneshot` is already available in this file because `cleanup_rx`, `startup_rx`, and `probe_rx` use it.

Add this field next to the other async task receivers:

```rust
pub finalize_rx: Option<oneshot::Receiver<anyhow::Result<SetupSummary>>>,
```

The receiver type uses `SetupSummary`, already imported in this file as part of the setup domain imports.

- [ ] **Step 3: Initialize `finalize_rx` in constructors**

In `FirstRunTuiState::new(...)`, initialize:

```rust
finalize_rx: None,
```

Place it near `startup_rx: None` and `probe_rx: None`.

Check `FirstRunTuiState::new_configured(...)`. If it delegates through `Self::new(...)`, no separate change is needed. If it constructs directly, add the same `finalize_rx: None`.

- [ ] **Step 4: Run compile check**

Run:

```bash
cargo test -p brain3 --no-run
```

Expected: compile succeeds unless later tasks have already added failing tests.

---

## Task 2: Test That Save & Start No Longer Blocks Before Redraw

**Files:**
- Modify: `apps/gateway/src/tui/app.rs`

- [ ] **Step 1: Add a unit test for immediate finalization-pending state**

In the existing `#[cfg(test)] mod tests` in `apps/gateway/src/tui/app.rs`, add:

```rust
#[tokio::test]
async fn finalize_and_start_immediately_shows_download_status_when_audio_enabled() {
    let use_case = FirstRunSetupUseCase::new(
        Arc::new(PlatformSetupSystem::with_environment(
            SetupOperatingSystem::MacOS,
            None,
        )),
        SetupDefaults {
            default_container_image_repo: "ghcr.io/tleyden/brain3-mcp-vault-tools".into(),
        },
    );
    let mut state = FirstRunTuiState::new(
        "127.0.0.1".into(),
        PathBuf::from("/tmp/brain3-home/brain3.log"),
        sample_preparation(),
    );
    state.draft.native_audio_transcription_enabled = true;
    state.draft.whisper_model = "base.en".into();

    finalize_and_start(
        &mut state,
        &use_case,
        RuntimeOverrides::default(),
        RuntimeStartupPolicy::setup_or_reconfigure(),
    )
    .await;

    assert_eq!(state.step, SetupStep::RuntimeStatus);
    assert!(state.finalize_rx.is_some());
    assert!(state.startup_rx.is_none());
    assert_eq!(
        state.info_message.as_deref(),
        Some("Downloading Whisper model...")
    );
}
```

This test should not require a real download because the new implementation must return immediately after spawning the finalize task.

- [ ] **Step 2: Run the failing test**

Run:

```bash
cargo test -p brain3 finalize_and_start_immediately_shows_download_status_when_audio_enabled
```

Expected before implementation: fail because `finalize_and_start()` currently waits for `finalize()` and does not set `finalize_rx`.

---

## Task 3: Background `finalize()` and Show Download/Saving Status

**Files:**
- Modify: `apps/gateway/src/tui/app.rs`

- [ ] **Step 1: Replace blocking finalize logic with a background task**

In `finalize_and_start(...)`, replace the direct `use_case.finalize(request).await` section with this shape:

```rust
state.clear_messages();

let request: FinalizeSetupRequest = state.apply_inputs_to_draft();
let downloading_whisper_model = request.draft.native_audio_transcription_enabled;
let (tx, rx) = oneshot::channel();
let use_case = (*use_case).clone();

tokio::spawn(async move {
    let result = use_case
        .finalize(request)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"));
    let _ = tx.send(result);
});

state.finalize_rx = Some(rx);
state.startup_rx = None;
state.info_message = Some(if downloading_whisper_model {
    "Downloading Whisper model...".into()
} else {
    "Saving configuration...".into()
});
state.step = SetupStep::RuntimeStatus;
```

Important: `FirstRunSetupUseCase` currently stores only `Arc<dyn SetupSystemPort>` and `SetupDefaults`, so make it cloneable in Task 3 Step 2 before using `(*use_case).clone()`.

- [ ] **Step 2: Derive or implement `Clone` for `FirstRunSetupUseCase`**

In `crates/core/src/application/first_run_setup.rs`, update:

```rust
pub struct FirstRunSetupUseCase {
    port: Arc<dyn SetupSystemPort>,
    defaults: SetupDefaults,
}
```

to:

```rust
#[derive(Clone)]
pub struct FirstRunSetupUseCase {
    port: Arc<dyn SetupSystemPort>,
    defaults: SetupDefaults,
}
```

If `SetupDefaults` is not cloneable, derive `Clone` for it in `crates/core/src/domain/setup.rs` where it is defined. Use the existing derive style in that file.

- [ ] **Step 3: Run the focused test**

Run:

```bash
cargo test -p brain3 finalize_and_start_immediately_shows_download_status_when_audio_enabled
```

Expected: pass.

---

## Task 4: Poll Finalization and Start Runtime After Success

**Files:**
- Modify: `apps/gateway/src/tui/app.rs`

- [ ] **Step 1: Extract runtime startup spawning into a helper**

Add this helper near `finalize_and_start(...)`:

```rust
fn start_runtime_after_finalize(
    state: &mut FirstRunTuiState,
    summary: SetupSummary,
    runtime_overrides: RuntimeOverrides,
    startup_policy: RuntimeStartupPolicy,
) {
    let launch_plan = RuntimeLaunchPlan {
        paths: summary.paths.clone(),
        env_file: summary.paths.env_file.clone(),
        log_file: state.log_file.clone(),
    };
    let host = state.host.clone();
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = tx.send(
            start_configured_runtime_session(&host, launch_plan, runtime_overrides, startup_policy)
                .await,
        );
    });

    state.summary = Some(summary);
    state.startup_rx = Some(rx);
    state.info_message = Some("Starting Brain3...".into());
    state.step = SetupStep::RuntimeStatus;
}
```

Make sure `SetupSummary` is imported at the top of `apps/gateway/src/tui/app.rs` from `brain3_core::domain::setup`.

- [ ] **Step 2: Add finalization polling to `handle_runtime_tick`**

In `handle_runtime_tick(state: &mut FirstRunTuiState) -> bool`, add this block before `startup_rx` polling:

```rust
if let Some(rx) = &mut state.finalize_rx {
    match rx.try_recv() {
        Ok(Ok(summary)) => {
            state.finalize_rx = None;
            start_runtime_after_finalize(
                state,
                summary,
                state.pending_runtime_overrides.clone(),
                state.pending_startup_policy,
            );
        }
        Ok(Err(error)) => {
            state.finalize_rx = None;
            tracing::error!(error = %error, "failed to finalize setup");
            state.error_message = Some(error.to_string());
            state.info_message = None;
            state.step = SetupStep::Summary;
        }
        Err(oneshot::error::TryRecvError::Closed) => {
            state.finalize_rx = None;
            state.error_message = Some("setup finalization task stopped before completion".into());
            state.info_message = None;
            state.step = SetupStep::Summary;
        }
        Err(oneshot::error::TryRecvError::Empty) => {}
    }
}
```

This block introduces two pending fields that do not exist yet. Add them in Task 4 Step 3.

- [ ] **Step 3: Add pending runtime fields to `FirstRunTuiState`**

In `apps/gateway/src/tui/state.rs`, add:

```rust
pub pending_runtime_overrides: RuntimeOverrides,
pub pending_startup_policy: RuntimeStartupPolicy,
```

This is not ideal because `RuntimeOverrides` is currently in the gateway crate, while `state.rs` already lives in the gateway crate and can import it from `crate::RuntimeOverrides`. Initialize:

```rust
pending_runtime_overrides: RuntimeOverrides::default(),
pending_startup_policy: RuntimeStartupPolicy::setup_or_reconfigure(),
```

Then in `finalize_and_start(...)`, before spawning finalize, set:

```rust
state.pending_runtime_overrides = runtime_overrides;
state.pending_startup_policy = startup_policy;
```

If `RuntimeStartupPolicy` is not `Copy`, use `.clone()` consistently and derive `Clone` in the core setup model only if it is not already cloneable.

- [ ] **Step 4: Prefer a cleaner alternative if available during implementation**

If adding `RuntimeOverrides` into `FirstRunTuiState` causes an import cycle or awkward coupling, use this cleaner alternative instead:

```rust
pub finalize_rx: Option<
    oneshot::Receiver<anyhow::Result<(SetupSummary, RuntimeOverrides, RuntimeStartupPolicy)>>,
>,
```

Then send `(summary, runtime_overrides, startup_policy)` from the finalize task and avoid storing pending fields on state. Prefer this alternative if it compiles cleanly. The expected behavior and tests remain identical.

- [ ] **Step 5: Run compile check**

Run:

```bash
cargo test -p brain3 --no-run
```

Expected: compile succeeds.

---

## Task 5: Test Success and Failure Transitions

**Files:**
- Modify: `apps/gateway/src/tui/app.rs`

- [ ] **Step 1: Add a direct helper test for finalize success starting runtime**

Add a unit test that bypasses real download and validates state transition with a synthetic summary:

```rust
#[tokio::test]
async fn finalize_success_starts_runtime_startup_task() {
    let mut state = FirstRunTuiState::new(
        "127.0.0.1".into(),
        PathBuf::from("/tmp/brain3-home/brain3.log"),
        sample_preparation(),
    );
    let summary = SetupSummary {
        paths: state.preparation.paths.clone(),
        draft: state.draft.clone(),
        dependencies: state.preparation.dependencies.clone(),
    };

    start_runtime_after_finalize(
        &mut state,
        summary,
        RuntimeOverrides::default(),
        RuntimeStartupPolicy::setup_or_reconfigure(),
    );

    assert_eq!(state.step, SetupStep::RuntimeStatus);
    assert!(state.startup_rx.is_some());
    assert_eq!(state.info_message.as_deref(), Some("Starting Brain3..."));
}
```

- [ ] **Step 2: Add a direct helper test for finalize failure returning to Summary**

If implementation extracts a helper such as `apply_finalize_result(...)`, test it directly:

```rust
#[test]
fn finalize_failure_returns_to_summary_with_error() {
    let mut state = FirstRunTuiState::new(
        "127.0.0.1".into(),
        PathBuf::from("/tmp/brain3-home/brain3.log"),
        sample_preparation(),
    );
    state.step = SetupStep::RuntimeStatus;
    state.info_message = Some("Downloading Whisper model...".into());

    apply_finalize_result(
        &mut state,
        Err(anyhow!("download Whisper model base.en: HTTP 503")),
        RuntimeOverrides::default(),
        RuntimeStartupPolicy::setup_or_reconfigure(),
    );

    assert_eq!(state.step, SetupStep::Summary);
    assert_eq!(
        state.error_message.as_deref(),
        Some("download Whisper model base.en: HTTP 503")
    );
    assert!(state.info_message.is_none());
}
```

If no helper is extracted, add one. It should be small and should contain only the result-handling logic currently embedded in `handle_runtime_tick`.

- [ ] **Step 3: Run focused tests**

Run:

```bash
cargo test -p brain3 finalize_
```

Expected: both pass.

---

## Task 6: Make Status Rendering Treat Finalize as Active Work

**Files:**
- Modify: `apps/gateway/src/tui/screens.rs`

- [ ] **Step 1: Update `runtime_lines` for finalize phase**

In `runtime_lines(state)`, before the existing `if state.startup_rx.is_some()` block, add:

```rust
if state.finalize_rx.is_some() {
    lines.push(Line::from(vec![
        Span::styled(
            format!("{} ", spinner_char(state.tick_count)),
            accent_style(),
        ),
        Span::styled(
            state
                .info_message
                .clone()
                .unwrap_or_else(|| "Saving configuration...".into()),
            accent_style(),
        ),
    ]));
    return lines;
}
```

This makes the main body show `Downloading Whisper model...`, not just the footer/status area.

- [ ] **Step 2: Update `status_lines` active task detection**

Change:

```rust
let has_active_task = state.startup_rx.is_some() || state.probe_rx.is_some();
```

to:

```rust
let has_active_task =
    state.finalize_rx.is_some() || state.startup_rx.is_some() || state.probe_rx.is_some();
```

- [ ] **Step 3: Add or update a screen rendering test**

In `apps/gateway/src/tui/screens.rs` tests, add:

```rust
use tokio::sync::oneshot;

#[test]
fn runtime_screen_shows_downloading_whisper_model_status() {
    let mut state = FirstRunTuiState::new(
        "127.0.0.1".into(),
        PathBuf::from("/tmp/brain3-home/brain3.log"),
        sample_preparation(),
    );
    let (_tx, rx) = oneshot::channel();
    state.finalize_rx = Some(rx);
    state.step = SetupStep::RuntimeStatus;
    state.info_message = Some("Downloading Whisper model...".into());

    let lines = runtime_lines(&state);
    let rendered = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Downloading Whisper model..."));
}
```

If `sample_preparation()` is private to `app.rs`, create a minimal local test helper in `screens.rs` following existing screen test patterns. Do not test style attributes.

- [ ] **Step 4: Run focused screen test**

Run:

```bash
cargo test -p brain3 runtime_screen_shows_downloading_whisper_model_status
```

Expected: pass.

---

## Task 7: Show Native Audio Transcription on Runtime Status

**Files:**
- Modify: `apps/gateway/src/tui/screens.rs`

- [ ] **Step 1: Add a failing screen test for enabled native audio**

In `apps/gateway/src/tui/screens.rs` tests, add:

```rust
#[test]
fn runtime_screen_shows_native_audio_transcription_enabled() {
    let mut state = FirstRunTuiState::new(
        "127.0.0.1".into(),
        PathBuf::from("/tmp/brain3-home/brain3.log"),
        sample_preparation(),
    );
    state.step = SetupStep::RuntimeStatus;
    state.runtime = Some(RuntimeBootstrap::new(
        Arc::new(gateway_config_with_native_audio(true, "base.en")),
        "secret".into(),
        RuntimeLaunchPlan {
            paths: state.preparation.paths.clone(),
            env_file: state.preparation.paths.env_file.clone(),
            log_file: PathBuf::from("/tmp/brain3-home/brain3.log"),
        },
        Some("https://brain3.example.com".into()),
        StartupStatus::Ready,
        StartupStatus::Ready,
        false,
    ));

    let rendered = render_runtime_lines_to_string(&state);

    assert!(rendered.contains("Native audio transcription: Enabled"));
}
```

If `screens.rs` does not already have local helpers for this exact runtime fixture, add small test helpers in the test module:

```rust
fn render_runtime_lines_to_string(state: &FirstRunTuiState) -> String {
    runtime_lines(state)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
```

Use the existing screen-test fixture style for `gateway_config_with_native_audio(...)`; do not duplicate large config construction if a local helper already exists in `screens.rs`.

- [ ] **Step 2: Add a failing screen test for disabled native audio**

Add:

```rust
#[test]
fn runtime_screen_shows_native_audio_transcription_disabled() {
    let mut state = FirstRunTuiState::new(
        "127.0.0.1".into(),
        PathBuf::from("/tmp/brain3-home/brain3.log"),
        sample_preparation(),
    );
    state.step = SetupStep::RuntimeStatus;
    state.runtime = Some(RuntimeBootstrap::new(
        Arc::new(gateway_config_with_native_audio(false, "base.en")),
        "secret".into(),
        RuntimeLaunchPlan {
            paths: state.preparation.paths.clone(),
            env_file: state.preparation.paths.env_file.clone(),
            log_file: PathBuf::from("/tmp/brain3-home/brain3.log"),
        },
        Some("https://brain3.example.com".into()),
        StartupStatus::Ready,
        StartupStatus::Ready,
        false,
    ));

    let rendered = render_runtime_lines_to_string(&state);

    assert!(rendered.contains("Native audio transcription: Disabled"));
}
```

- [ ] **Step 3: Run the failing tests**

Run:

```bash
cargo test -p brain3 runtime_screen_shows_native_audio_transcription
```

Expected before implementation: tests fail because Runtime Status does not render native audio transcription status.

- [ ] **Step 4: Implement runtime status lines**

In `runtime_lines(state)`, inside the `if let Some(runtime) = &state.runtime` block and near the existing container/vault fields, add:

```rust
let transcription = &runtime.config.native_audio_transcription;
lines.push(key_badge_line(
    "Native audio transcription",
    if transcription.enabled {
        badge_span("Enabled", Color::Green)
    } else {
        badge_span("Disabled", Color::Yellow)
    },
));
if transcription.enabled {
    lines.push(key_value_line("Whisper model", transcription.model.clone()));
}
```

Do not show the model path by default; it is long and already available from config/logs when needed.

- [ ] **Step 5: Run the focused tests**

Run:

```bash
cargo test -p brain3 runtime_screen_shows_native_audio_transcription
```

Expected: both enabled and disabled tests pass.

---

## Task 8: Preserve Configured Startup Behavior

**Files:**
- Modify: `apps/gateway/src/tui/app.rs`

- [ ] **Step 1: Keep `start_without_writing_env` unchanged**

Do not route configured startup through `finalize_rx`. Configured startup should still:

```rust
state.startup_rx = Some(rx);
state.info_message = Some("Starting Brain3...".into());
state.step = SetupStep::RuntimeStatus;
```

- [ ] **Step 2: Run existing configured startup test**

Run:

```bash
cargo test -p brain3 configured_startup_begins_without_wizard_summary
```

Expected: pass.

---

## Task 9: Verification

**Files:**
- No additional file changes.

- [ ] **Step 1: Format**

Run:

```bash
cargo fmt --check
```

Expected: exit 0.

- [ ] **Step 2: Compile test targets**

Run:

```bash
cargo test -p brain3 --no-run
```

Expected: exit 0.

- [ ] **Step 3: Full local Rust test suite**

Run:

```bash
cargo test
```

Expected: all tests pass.

- [ ] **Step 4: Manual TUI verification**

Use a throwaway app home so the real install is not modified:

```bash
tmp_home="$(mktemp -d /tmp/brain3-whisper-status.XXXXXX)"
mkdir -p "$tmp_home/vault"
B3_HOME="$tmp_home" cargo run -p brain3 -- --tui
```

Manual steps:

1. Complete wizard with vault path set to `"$tmp_home/vault"`.
2. Enable native audio transcription.
3. Select `base.en` or `tiny.en`.
4. Press `Enter` on Summary.
5. Confirm the TUI immediately changes to Runtime Status and displays `Downloading Whisper model...` with an animated spinner.
6. Press `q` if you do not want to wait for the full download.

Expected: the Summary screen does not appear frozen after pressing `Save & Start`.

Do not run Docker-backed E2E smoke tests unless explicitly requested.

---

## Non-Goals

- No byte-level progress bar.
- No cancellation support for model downloads.
- No retry policy changes.
- No changes to Whisper model URLs, hashes, or size validation.
- No changes to `.env` write ordering.
- No new setup screens.

## Review Notes

The plan intentionally keeps model download in `finalize()` so `.env` is still written only after the model exists and verifies. The only behavior change is that finalization runs in the background and the TUI reports the phase that is currently blocking startup.
