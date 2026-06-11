# Brain3 First-Run Follow-Up Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the first-run Brain3 experience by adding the dependency doctor, setup orchestration, log-file handoff, runtime bootstrap, and the new first-run/runtime TUI on top of the app-home and env-generation foundation already implemented.

**Architecture:** Keep setup and validation logic in `brain3-core`, keep OS/filesystem/process adapters in `brain3-platform`, and keep ratatui confined to `apps/gateway`. The new first-run wizard should call shared setup services and runtime bootstrap APIs rather than owning business logic directly. The existing named-tunnel provisioning TUI stays isolated and untouched in this slice.

**Tech Stack:** Rust, Tokio, Ratatui, Crossterm, tracing, existing `brain3-core` and `brain3-platform` crates

---

**Already complete and out of scope**
- App home resolution via `~/.brain3` with `BRAIN3_HOME` override
- Default env path detection in `apps/gateway/src/main.rs`
- Shared setup models and `SetupSystemPort`
- Embedded `.env.template` rendering and env-file writing primitives
- First-run detection stub when the default env file is missing

**Deferred and out of scope**
- README rewrite
- Named Cloudflare tunnel onboarding and integration into the new wizard
- Live log streaming into the dashboard

### Task 5: Finish The Dependency Doctor

**Files:**
- Modify: `crates/core/src/domain/setup.rs`
- Modify: `crates/core/src/ports/setup_system.rs`
- Modify: `crates/platform/src/setup/system.rs`
- Modify: `crates/platform/tests/setup_bootstrap.rs`

- [ ] Extend the setup-domain types only where needed for the actual doctor flow.
  Add any missing distinction between “dependency missing”, “install available”, and “manual install required”, but do not create a second config model or TUI-specific enums.

- [ ] Implement supported install actions in `PlatformSetupSystem::run_install_action()`.
  macOS:
  `brew install cloudflared`
  `brew install container`
  Linux with `apt`:
  install `cloudflared`
  install Docker/runtime prerequisites
  Unsupported Linux distros should return a structured `SetupError::Unsupported(...)`.

- [ ] Keep command construction in the platform adapter, not in core or TUI code.
  The TUI should ask “do you want me to install X?” and then call the shared port action; it should never assemble shell commands itself.

- [ ] Add only focused public-API tests.
  Good candidates:
  one test that app-home detection still works
  one test that unsupported install action paths return a structured error
  Avoid testing ratatui rendering or shell output text.

- [ ] Verify this task with:
  `cargo test -p brain3-platform --test setup_bootstrap`

### Task 6: Add The First-Run Setup Use Case

**Files:**
- Create: `crates/core/src/application/first_run_setup.rs`
- Modify: `crates/core/src/application/mod.rs`
- Modify: `crates/core/src/domain/setup.rs`
- Modify: `crates/core/src/ports/setup_system.rs`

- [ ] Introduce a core setup use case that owns defaults, validation, and env-write orchestration.
  The public API should be shaped around setup inputs and outputs, for example:
  `prepare()` for app-home + dependency status + defaults
  `finalize(request)` for validation, secret generation, env rendering, directory creation, and env write
  `build_connection_card(...)` for the later runtime screen

- [ ] Default behavior in the use case:
  tunnel mode: quick tunnel
  username: `admin`
  client ID: `oauth2-gateway-client`
  runtime: `macos-container` on macOS, `docker` on Linux
  client secret: generated automatically
  access token: generated automatically and never user-editable in v1

- [ ] Validate only the core public requirements.
  Vault path must be absolute and exist.
  Username and client ID must be non-empty.
  Password must be non-empty unless auto-generation was chosen.
  Do not add named-tunnel validation in this slice.

- [ ] Return setup results as shared domain types, not raw strings.
  The TUI should get back a `SetupSummary` and a `ConnectionCard`, not a bag of ad hoc fields.

- [ ] Add a small set of core tests around the public use case API.
  Good candidates:
  rejects relative vault paths
  generates secrets when user does not provide them
  writes env via the port and returns a connection card

- [ ] Verify this task with:
  `cargo test -p brain3-core first_run_setup`

### Task 7: Add Log-File Allocation And Tracing Handoff

**Files:**
- Create: `apps/gateway/src/logging.rs`
- Modify: `apps/gateway/src/main.rs`
- Modify: `crates/core/src/domain/setup.rs`
- Modify: `crates/platform/src/setup/system.rs`

- [ ] Move log-file allocation into a small reusable gateway-side logging helper.
  `PlatformSetupSystem::create_temp_log_file()` already exists; use it rather than duplicating temp-file logic elsewhere.

- [ ] Initialize tracing against that temp file before either first-run setup or normal runtime start.
  The log file should exist for both setup failures and post-setup runtime.

- [ ] Add the log-file path to the shared runtime/setup presentation model.
  The TUI runtime screen should display the path plainly, for example:
  `Logs: /tmp/brain3-...log`

- [ ] Do not implement live tailing or in-TUI log streaming.
  This task ends at “logs go to a file and the user can see the path”.

- [ ] Verify this task with:
  `cargo build -p brain3`
  plus a manual smoke run confirming the file is created and mentioned in the UI

### Task 8: Create A Reusable Runtime Bootstrap API

**Files:**
- Create: `crates/platform/src/runtime/mod.rs`
- Create: `crates/platform/src/runtime/bootstrap.rs`
- Modify: `crates/platform/src/lib.rs`
- Modify: `apps/gateway/src/main.rs`

- [ ] Extract the configured-startup path out of `main.rs` into a reusable runtime bootstrap entry point.
  This API should own:
  upstream secret creation
  managed container startup
  tunnel startup
  collection of the public URL
  return of the runtime metadata the TUI needs

- [ ] Keep HTTP/router creation where it already naturally belongs, but hide the startup sequence behind one clean call.
  `main.rs` should stop manually stepping through container, tunnel, and server setup line by line.

- [ ] Return a structured runtime state object.
  It should include:
  loaded config
  public URL if present
  log-file path
  enough startup status for the runtime screen

- [ ] Leave named-tunnel provisioning logic untouched.
  The new runtime bootstrap should still be able to run a named tunnel if the config already says so, but this task must not merge that path into the first-run wizard.

- [ ] Verify this task with:
  `cargo build -p brain3`

### Task 9: Build The New First-Run And Runtime TUI

**Files:**
- Create: `apps/gateway/src/tui/mod.rs`
- Create: `apps/gateway/src/tui/app.rs`
- Create: `apps/gateway/src/tui/state.rs`
- Create: `apps/gateway/src/tui/screens.rs`
- Modify: `apps/gateway/src/main.rs`

- [ ] Build a new ratatui app shell for first-run setup.
  Screens for this slice:
  welcome
  dependency doctor
  vault path
  auth setup
  summary/write config
  connection card
  runtime status

- [ ] Keep the TUI as an inbound adapter only.
  It gathers input, calls the setup use case or runtime bootstrap, and renders results.
  It must not know how to generate secrets, choose default runtime values, write env files, or install dependencies.

- [ ] Show the connection card before the runtime status screen.
  Required fields:
  server URL
  client ID
  client secret
  username
  log-file path

- [ ] Show runtime status after setup completes and Brain3 is running.
  Display:
  container status
  tunnel/public URL status
  gateway bind/startup status
  log-file path
  No log tailing in this slice.

- [ ] Leave `apps/gateway/src/setup_tui.rs` alone for now.
  Do not fold its named-tunnel checklist into the new wizard yet; keep the new first-run TUI separate so this slice stays focused.

- [ ] Do not add TUI snapshot tests.
  Prefer one manual smoke test:
  start with an empty `BRAIN3_HOME`
  confirm the wizard appears
  walk through setup to the runtime status screen

### Task 10: Finish Main Dispatch Cleanup

**Files:**
- Modify: `apps/gateway/src/main.rs`
- Modify: `apps/gateway/src/tui/mod.rs`
- Modify: `crates/platform/src/runtime/bootstrap.rs`

- [ ] Replace the current first-run stub with the new TUI entry point.
  Behavior should be:
  if `--env-file` is provided, treat it as an advanced/manual path and do not auto-create it
  if no `--env-file` is provided and `~/.brain3/.env` is missing, launch the first-run TUI
  otherwise, run the configured runtime bootstrap path

- [ ] Keep `main.rs` as a composition root.
  It should initialize logging, parse args, resolve the env path, and dispatch into either:
  first-run TUI
  configured runtime bootstrap
  existing named-tunnel `--setup` flow

- [ ] Add a non-TTY fallback for first-run mode.
  If the default env file is missing and stdout/stderr are not interactive, print a clear setup-required message instead of trying to enter ratatui.

- [ ] Re-run the narrow verification set after the dispatch refactor:
  `cargo fmt --all --check`
  `cargo test -p brain3-core first_run_setup`
  `cargo test -p brain3-platform --test setup_bootstrap`
  `cargo build -p brain3`

---

**Recommended execution order**
1. Task 6 first, because the TUI and the dependency doctor both need a stable setup API.
2. Task 5 next, so the TUI can call real install actions.
3. Task 7 next, because the runtime and the TUI both need the log-file path.
4. Task 8 after that, to pull the existing configured startup out of `main.rs`.
5. Task 9 once the shared services exist.
6. Task 10 last, as the final dispatch and integration pass.

**Primary review questions**
- Keep `apps/gateway/src/setup_tui.rs` intact for now, or fold it into the new TUI immediately?
- For Linux guided install in this slice, should `apt` actually run after confirmation, or should the doctor stop at showing the exact commands?
