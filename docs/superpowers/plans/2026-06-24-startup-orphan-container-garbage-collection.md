# Startup Orphan Container Detection + Explicit Garbage Collection Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** During normal configured startup, detect Brain3-managed orphan MCP containers before launching a new runtime. By default, fail startup with a clear warning and rerun guidance. Only stop/remove those orphaned containers when the user explicitly opts in with a startup flag such as `--gc-containers`.

**Architecture:** Add explicit Brain3 ownership labels to managed containers, scope orphan discovery to the current Brain3 installation, run a startup preflight before `ensure_mcp_container()`, and gate destructive cleanup behind an explicit policy flag. Never infer ownership from image name or container name alone, and never auto-clean during setup/reconfiguration flows.

**Tech Stack:** Rust workspace (`apps/gateway`, `crates/core`, `crates/platform`), Clap CLI parsing, ratatui TUI, Docker and macOS container adapters.

---

## Investigation Summary

Current behavior leaves a gap for safe orphan cleanup:

1. `apps/gateway/src/main.rs`
   Normal configured startup goes through `run_cli_mode()` or the TUI "start runtime" path and then calls `bootstrap_configured_runtime(...)`. There is currently no startup policy flag for explicit container GC.

2. `crates/platform/src/runtime/bootstrap.rs`
   Bootstrap only tries to start the currently configured MCP container. It does not preflight for stale Brain3-managed containers from previous runs.

3. `crates/core/src/ports/container.rs`
   The container port can check one container by name and create/remove a named container, but it cannot enumerate managed containers for a scoped cleanup decision.

4. `crates/platform/src/container/docker.rs` and `crates/platform/src/container/macos_container.rs`
   Managed container launches are not labeled, so there is no reliable ownership marker for distinguishing:
   - Brain3-managed leftovers from the same installation
   - unrelated user-owned containers
   - Brain3 containers from a different install or app home

5. `crates/platform/src/container/startup.rs`
   Startup currently builds one `ContainerConfig` and delegates to `EnsureContainerUseCase`. There is no explicit orphan-preflight or GC path before startup.

That means any future "cleanup on startup" logic would be unsafe unless Brain3 first gains a trustworthy ownership model and an explicit opt-in policy.

---

### Task 1: Add an explicit startup GC policy flag

**Files:**
- Modify: `apps/gateway/src/main.rs`
- Modify: `crates/core/src/domain/setup.rs` or `crates/platform/src/runtime/bootstrap.rs`
- Test: `apps/gateway/src/main.rs`

- [x] Add a CLI flag for explicit startup cleanup, preferably `--gc-containers` with a descriptive help string.
- [x] Thread that flag into the configured runtime startup path without changing default behavior.
- [x] Keep `main.rs` lean by passing a small startup policy/value object instead of sprinkling cleanup logic in dispatch code.
- [x] Ensure both CLI startup and configured TUI startup can access the same cleanup policy.
- [x] Add argument-parsing tests and launch-plumbing tests.

**Verification:**
- Run: `cargo test -p brain3 tests::args_accept_gc_containers_flag -- --nocapture`

### Task 2: Add explicit Brain3 ownership labels to managed containers

**Files:**
- Modify: `crates/core/src/domain/model.rs`
- Modify: `crates/platform/src/container/startup.rs`
- Modify: `crates/platform/src/container/docker.rs`
- Modify: `crates/platform/src/container/macos_container.rs`
- Test: `crates/platform/src/container/docker.rs` and/or `crates/platform/src/container/macos_container.rs`

- [x] Extend `ContainerConfig` to carry runtime-agnostic container labels.
- [x] Add Brain3-managed labels to every MCP container Brain3 starts, for example:
- [x] `io.brain3.managed=true`
- [x] `io.brain3.role=mcp`
- [x] `io.brain3.installation_id=<stable scoped id>`
- [x] Derive `installation_id` from the current Brain3 installation scope, not from mutable container names.
- [x] Keep the scope narrow enough that one VPS can run multiple independent Brain3 deployments without cross-cleanup.
- [x] Update both Docker and macOS adapters to pass labels through to the runtime CLI.

**Implementation Note:**
Use a deterministic installation identifier derived from the resolved Brain3 home or env-file scope. Do not treat image names or default container names as ownership proof.

**Verification:**
- Run: `cargo test -p brain3-platform container -- --nocapture`

### Task 3: Extend the container port with managed-container discovery

**Files:**
- Modify: `crates/core/src/ports/container.rs`
- Modify: `crates/core/src/domain/errors.rs`
- Modify: `crates/platform/src/container/docker.rs`
- Modify: `crates/platform/src/container/macos_container.rs`
- Test: `crates/platform/src/container/docker.rs` and/or `crates/platform/src/container/macos_container.rs`

- [x] Add a container-port API for listing Brain3-managed containers within an installation scope.
- [x] Introduce a small domain type for managed-container metadata such as name, running state, and labels needed for startup decisions.
- [x] Add a structured startup error for orphaned containers so callers can render a useful summary instead of a vague conflict string.
- [x] Make the adapter discovery logic filter by Brain3 ownership labels rather than broad image/name matching.
- [x] Ensure unlabeled containers are invisible to this GC path.

**Verification:**
- Run: `cargo test -p brain3-platform container -- --nocapture`

### Task 4: Detect orphaned managed containers before normal startup

**Files:**
- Modify: `crates/platform/src/runtime/bootstrap.rs`
- Modify: `crates/platform/src/container/startup.rs`
- Modify: `crates/core/src/application/ensure_container.rs` if needed
- Test: `crates/platform/src/runtime/bootstrap.rs`

- [x] Add a preflight step before `ensure_mcp_container()` that discovers Brain3-managed containers for the current installation.
- [x] Treat any pre-existing managed MCP container in that scope as an orphan candidate for this startup, including the configured container name if it was left behind by a prior session.
- [x] When orphan candidates are found and `--gc-containers` is not set, fail startup before starting anything new.
- [x] Surface a structured error that includes the orphan container names/states and instructs the user to rerun with `--gc-containers`.
- [x] Keep setup and reconfiguration flows non-destructive; this preflight only applies to normal configured startup.

**Implementation Note:**
Do not silently reuse or replace pre-existing managed containers in this change. The safe default is fail closed with explicit operator action.

**Verification:**
- Run: `cargo test -p brain3-platform runtime -- --nocapture`

### Task 5: Implement explicit GC for scoped orphan containers

**Files:**
- Modify: `crates/platform/src/container/startup.rs`
- Modify: `crates/core/src/ports/container.rs`
- Modify: `crates/platform/src/container/docker.rs`
- Modify: `crates/platform/src/container/macos_container.rs`
- Test: `crates/platform/src/runtime/bootstrap.rs` and/or `crates/platform/src/container/startup.rs`

- [x] If `--gc-containers` is set, stop and remove only the labeled orphan containers discovered for the current installation scope.
- [x] Remove stopped containers directly; stop running containers first, then remove them.
- [x] Log each cleanup decision at `info`/`warn` with container name and reason.
- [x] If cleanup partially fails, abort startup with a clear error rather than continuing in an ambiguous state.
- [x] Do not touch:
- [x] unlabeled containers
- [x] containers from another Brain3 installation scope
- [x] shared networks in this change

**Verification:**
- Run: `cargo test -p brain3-platform runtime -- --nocapture`

### Task 6: Show exact rerun guidance in CLI and TUI failure paths

**Files:**
- Modify: `apps/gateway/src/main.rs`
- Modify: `apps/gateway/src/tui/app.rs`
- Modify: `apps/gateway/src/tui/screens.rs`
- Test: `apps/gateway/src/main.rs`
- Test: `apps/gateway/src/tui/app.rs` and/or `apps/gateway/src/tui/screens.rs`

- [x] Reuse the existing command-rendering helpers in `main.rs` to produce an exact rerun command with `--gc-containers`.
- [x] Make CLI startup failures print actionable guidance rather than only "see logs".
- [x] Make the TUI show the same explicit rerun guidance when startup fails due to orphan containers.
- [x] Ensure the message makes it clear Brain3 refused to auto-delete containers without explicit approval.

**Verification:**
- Run: `cargo test -p brain3 tests::configured_startup_orphan_failure_includes_gc_rerun_guidance -- --nocapture`
- Run: `cargo test -p brain3 tui::app -- --nocapture`

### Task 7: Full regression verification

**Files:**
- No code changes

- [x] Run targeted tests for `brain3` and `brain3-platform`.
- [x] Run the full workspace test suite.
- [ ] Manually verify the intended operator flow:
1. Start Brain3 once so it creates a labeled managed container.
2. Simulate an orphan by leaving that managed container behind.
3. Start Brain3 normally and confirm startup fails before launching a new container.
4. Confirm the failure message instructs the user to rerun with `--gc-containers`.
5. Rerun with `--gc-containers` and confirm Brain3 removes only the scoped labeled orphan container(s).
6. Confirm startup proceeds afterward.
7. Confirm unrelated or unlabeled containers on the same host remain untouched.

**Verification:**
- Run: `cargo test`

---

## Expected Outcome

After this change:

- Normal configured startup refuses to auto-delete orphaned Brain3-managed containers.
- The user gets a clear error and an exact rerun command with `--gc-containers`.
- Explicit GC only cleans containers Brain3 can prove belong to the current installation.
- Unrelated containers and shared networks are not touched.
- Setup/reconfiguration remains non-destructive and out of scope for this cleanup path.
