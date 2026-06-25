# Prevent TUI Container/Network Interference Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make interactive Brain3 launches non-destructive so opening the TUI or reconfiguration flow never stops/removes existing containers or recreates shared networks before the user has explicitly chosen the new container and network names.

**Architecture:** Split interactive configured launch into a safe review/edit path before runtime bootstrap, make container/network conflict handling fail-safe instead of destructive, and track whether this session actually created the managed container before stopping it on shutdown. Preserve the security model by refusing unsafe reuse rather than broadening ingress or ownership.

**Tech Stack:** Rust workspace (`apps/gateway`, `crates/core`, `crates/platform`), ratatui TUI, async Tokio runtime, Docker and macOS container adapters.

---

## Investigation Summary

Root cause was confirmed in three places:

1. `apps/gateway/src/main.rs`
   `LaunchDispatch::TuiConfigured` starts the configured runtime immediately when the env file already exists. That means `brain3 --tui` is not a harmless editor/status screen; it eagerly boots the managed container path before the user can edit the new `container_name` or `container_network_name`.

2. `crates/core/src/application/ensure_container.rs`
   `EnsureContainerUseCase::ensure()` unconditionally stops and removes any existing container with the configured name before starting a new one.

3. `crates/platform/src/container/docker.rs` and `crates/platform/src/container/macos_container.rs`
   `prepare_network_isolation()` unconditionally removes and recreates the configured internal network if it already exists. That can disrupt other running containers sharing the same network name.

There is also a follow-on ownership bug:

4. `crates/platform/src/runtime/bootstrap.rs`
   `RuntimeBootstrap::shutdown_managed_runtime()` stops the configured container by name on TUI exit even when the container predated the current session.

---

### Task 1: Make interactive configured launch safe before bootstrap

**Files:**
- Modify: `apps/gateway/src/main.rs`
- Modify: `apps/gateway/src/tui/app.rs`
- Modify: `apps/gateway/src/tui/state.rs`
- Modify: `apps/gateway/src/tui/screens.rs`
- Test: `apps/gateway/src/main.rs`
- Test: `apps/gateway/src/tui/state.rs`
- Test: `apps/gateway/src/tui/screens.rs`

- [ ] Add a distinct interactive launch path for “configured but not yet started”.
- [ ] Change `LaunchDispatch::TuiConfigured` so opening `brain3 --tui` with an existing env file does not immediately call `spawn_configured_gateway_session()`.
- [ ] Seed the TUI draft from the existing config/env file so the user can review and edit the saved `container_name` and `container_network_name` before startup.
- [ ] Start the runtime only after explicit confirmation from the TUI.
- [ ] Keep the current non-interactive CLI behavior unchanged.
- [ ] Add tests proving that configured interactive launch no longer starts runtime during TUI initialization.

**Verification:**
- Run: `cargo test -p brain3 tests::launch_dispatch_uses_wizard_only_for_missing_default_env_in_tui_mode -- --nocapture`
- Run: `cargo test -p brain3 tui::state -- --nocapture`
- Run: `cargo test -p brain3 tui::screens -- --nocapture`

### Task 2: Replace destructive container conflict handling with fail-safe detection

**Files:**
- Modify: `crates/core/src/application/ensure_container.rs`
- Modify: `crates/core/src/ports/container.rs`
- Modify: `crates/platform/src/container/docker.rs`
- Modify: `crates/platform/src/container/macos_container.rs`
- Test: `crates/core/src/application/ensure_container.rs`

- [ ] Extend the container port interface so startup can distinguish “container exists and is ours to replace” from “container exists but should be treated as a conflict”.
- [ ] Remove the unconditional “stop then remove by name” behavior from `EnsureContainerUseCase::ensure()`.
- [ ] If an existing container with the target name is detected before startup, surface a structured conflict error instead of mutating it.
- [ ] Make the TUI show that conflict clearly and direct the user to change `container name` before retrying.
- [ ] Add tests proving `ensure()` does not call stop/remove when an existing conflicting container is present.

**Verification:**
- Run: `cargo test -p brain3-core ensure_container -- --nocapture`

### Task 3: Make internal network setup non-destructive

**Files:**
- Modify: `crates/core/src/ports/container.rs`
- Modify: `crates/platform/src/container/docker.rs`
- Modify: `crates/platform/src/container/macos_container.rs`
- Modify: `crates/core/src/application/ensure_container.rs`
- Test: `crates/core/src/application/ensure_container.rs`

- [ ] Change the network-isolation contract so “prepare” never removes an existing named network as a side effect.
- [ ] If the configured network does not exist, create it.
- [ ] If it already exists and is compatible, reuse it.
- [ ] If it already exists but is incompatible or cannot be safely verified, return a conflict error and require the user to choose a different `container network name`.
- [ ] Add tests proving startup no longer removes/recreates an existing network name during preflight.

**Verification:**
- Run: `cargo test -p brain3-core ensure_container -- --nocapture`

### Task 4: Track session ownership so shutdown only stops containers this session created

**Files:**
- Modify: `crates/platform/src/runtime/bootstrap.rs`
- Modify: `crates/platform/src/container/startup.rs`
- Modify: `crates/core/src/application/ensure_container.rs`
- Test: `crates/platform/src/runtime/bootstrap.rs` or `crates/platform/src/container/startup.rs`

- [ ] Change container bootstrap to return startup metadata describing whether this session created/replaced the container or merely observed a conflict/no-op.
- [ ] Store that ownership metadata in `RuntimeBootstrap`.
- [ ] Update `shutdown_managed_runtime()` to stop the MCP container only when this session explicitly started it.
- [ ] Add tests proving exiting the TUI does not stop a pre-existing container that Brain3 did not create in the current session.

**Verification:**
- Run: `cargo test -p brain3-platform runtime -- --nocapture`

### Task 5: Preserve the edited names across the safe reconfigure flow

**Files:**
- Modify: `crates/platform/src/config/env_file.rs`
- Modify: `crates/platform/src/setup/env_writer.rs`
- Modify: `crates/core/src/application/first_run_setup.rs`
- Modify: `apps/gateway/src/tui/state.rs`
- Test: `crates/platform/tests/setup_bootstrap.rs`
- Test: `apps/gateway/src/tui/state.rs`

- [ ] Confirm the new safe interactive flow round-trips `B3_CONTAINER_NAME` and `B3_CONTAINER_NETWORK_NAME` from saved env -> TUI draft -> saved env without falling back to legacy defaults.
- [ ] Add regression tests for reading, editing, and writing both names.

**Verification:**
- Run: `cargo test -p brain3-platform setup_bootstrap -- --nocapture`
- Run: `cargo test -p brain3 tui::state -- --nocapture`

### Task 6: Full regression verification

**Files:**
- No code changes

- [ ] Run targeted crate tests for `brain3`, `brain3-core`, and `brain3-platform`.
- [ ] Run the full workspace test suite.
- [ ] Manually verify this scenario in an interactive shell:
  1. Start a container manually with the old default name.
  2. Start Brain3 TUI against an existing config.
  3. Confirm Brain3 does not stop/remove that container on launch.
  4. Change `container name` and `container network name` in the TUI.
  5. Start Brain3 and confirm it launches a separate container/network.
  6. Exit the TUI and confirm the pre-existing container remains running.

**Verification:**
- Run: `cargo test`

---

## Expected Outcome

After this change:

- Opening `brain3 --tui` with an existing config is non-destructive.
- Brain3 never silently stops/removes a container just because its name matches the current config.
- Brain3 never silently removes/recreates a named internal network that may be shared by another running deployment.
- Shutdown only stops the container created by the current Brain3 session.
