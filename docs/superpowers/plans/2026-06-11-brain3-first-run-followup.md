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
- Modify: `apps/gateway/Cargo.toml`
- Modify: `apps/gateway/src/main.rs`
- Modify: `crates/core/src/domain/setup.rs`
- Modify: `crates/core/src/application/first_run_setup.rs`

- [ ] Add a small reusable gateway-side logging helper that owns temp-file allocation and tracing setup.
  Reuse `PlatformSetupSystem::create_temp_log_file()` from `crates/platform/src/setup/system.rs` rather than duplicating temp-file logic.
  If needed, add `tracing-appender` in `apps/gateway/Cargo.toml` so the helper can return a guard that keeps file logging alive for the process lifetime.

- [ ] Initialize tracing against that temp file before argument dispatch, first-run checks, config loading, setup mode, or normal runtime bootstrap.
  The log file must exist for:
  first-run failures
  named-tunnel preflight failures
  post-setup runtime
  Refactor the current early `std::process::exit(...)` branches in `apps/gateway/src/main.rs` into `Result`-based returns so the logging guard is not dropped prematurely.

- [ ] Add the log-file path to the presentation models the future TUI already depends on.
  Extend `ConnectionCard` in `crates/core/src/domain/setup.rs` with `log_file: PathBuf`.
  Update `FirstRunSetupUseCase::build_connection_card(...)` in `crates/core/src/application/first_run_setup.rs` so the gateway passes the runtime log path in when building the card.
  Keep `RuntimeLaunchPlan.log_file` as the runtime-side carrier for the later status screen.
  The TUI should eventually display the path plainly, for example:
  `Logs: /tmp/brain3-...log`

- [ ] Do not implement live tailing, in-TUI tracing sinks, or scrollback buffering in this task.
  This task ends at:
  logs go to a file
  the runtime/setup handoff includes the path
  later TUI work can render that path

- [ ] Verify this task with:
  `cargo build -p brain3`
  plus a manual smoke run confirming the file is created before first-run failure paths and is available to display in the later UI work

### Task 8: Create A Reusable Runtime Bootstrap API

**Files:**
- Create: `crates/platform/src/runtime/mod.rs`
- Create: `crates/platform/src/runtime/bootstrap.rs`
- Modify: `crates/platform/src/lib.rs`
- Modify: `apps/gateway/src/main.rs`

- [ ] Extract the configured-startup path out of `apps/gateway/src/main.rs` into a reusable platform bootstrap entry point.
  This API should own the sequence currently in `main.rs`:
  named-tunnel config-file preflight
  startup config logging
  upstream secret creation
  managed container startup
  tunnel startup
  collection of the public URL

- [ ] Keep HTTP/router creation and `axum::serve(...)` in `apps/gateway/src/main.rs`, but hide the startup sequence behind one clean call.
  After this task, `main.rs` should:
  initialize logging
  parse args and resolve env path
  load config
  dispatch `--setup` or first-run
  call runtime bootstrap for configured startup
  build `AppState`, router, listener, and server

- [ ] Return a structured runtime bootstrap state object from `crates/platform/src/runtime/bootstrap.rs`.
  It should include:
  loaded config
  upstream secret
  public URL if present
  log-file path
  enough startup status for the runtime screen
  a held tunnel adapter/guard so quick and named tunnels stay alive after bootstrap returns
  Prefer using the existing `RuntimeLaunchPlan` as the bootstrap input so `main.rs` passes the resolved env path and log-file path explicitly instead of rebuilding that state ad hoc.

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
- Create: `apps/gateway/src/server.rs`
- Modify: `apps/gateway/src/main.rs`

- [ ] Build a new ratatui app shell for the first-run path only.
  Do not replace or fold in `apps/gateway/src/setup_tui.rs`; that named-tunnel checklist remains a separate flow.
  Keep the new TUI split into focused files:
  `state.rs` for screen state and user-editable fields
  `screens.rs` for rendering
  `app.rs` for the event loop and screen transitions
  `mod.rs` for the public entry point that `main.rs` can dispatch to
  Screens for this slice:
  welcome
  dependency doctor
  vault path
  auth setup
  summary/write config
  connection card
  runtime status

- [ ] Keep the TUI state rooted in the shared models that already exist instead of inventing a second config model.
  The TUI should carry:
  `SetupPreparation` from `FirstRunSetupUseCase::prepare()`
  a mutable `SetupDraftConfig` copy for form edits
  the current `SetupStep`
  transient validation/install error text
  optional `SetupSummary`
  optional `ConnectionCard`
  optional `RuntimeBootstrap`
  gateway server status for the runtime screen
  This keeps the TUI as an inbound adapter over existing core/platform APIs rather than embedding business rules.

- [ ] Wire each TUI transition to the existing application/runtime APIs instead of adding setup logic in the UI layer.
  The flow should be:
  call `FirstRunSetupUseCase::prepare()` once at startup
  use `SetupPreparation.dependencies` to drive the dependency doctor
  call `SetupSystemPort::run_install_action(...)` when the user chooses an installable dependency action
  call `FirstRunSetupUseCase::finalize(...)` from the summary screen
  reload the written env file through `EnvFileConfigAdapter`
  build a `RuntimeLaunchPlan` with the resolved app-home/env path/log-file path
  call `bootstrap_configured_runtime(...)`
  then start the actual gateway server through a shared gateway-side helper in `apps/gateway/src/server.rs`

- [ ] Add a small reusable gateway-server helper so the first-run TUI and the normal runtime path do not duplicate `main.rs` server composition.
  Move the current gateway-specific composition steps out of `main.rs` into `apps/gateway/src/server.rs`:
  auth-code store setup
  MCP proxy setup
  use-case wiring
  `AppState` construction
  router construction
  listener bind
  server spawn/run entry point
  This keeps `apps/gateway/src/main.rs` lean, which matches the project guidance for the gateway binary.

- [ ] Show the connection card before the runtime status screen.
  Required fields:
  server URL
  client ID
  client secret
  username
  log-file path
  Build it through `FirstRunSetupUseCase::build_connection_card(...)`, passing the display-ready server URL chosen by the runtime/bootstrap path and the existing temp log-file path.

- [ ] Show runtime status after setup completes and Brain3 is running.
  The runtime screen should display:
  container status from `RuntimeBootstrap`
  tunnel/public URL status from `RuntimeBootstrap`
  gateway bind/startup status from the new gateway-server helper
  log-file path from `RuntimeLaunchPlan`
  No log tailing or in-TUI log streaming in this slice.

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
- Modify: `apps/gateway/src/server.rs`
- Modify: `crates/platform/src/runtime/bootstrap.rs`

- [ ] Replace the current first-run stub in `apps/gateway/src/main.rs` with the new TUI entry point.
  Behavior should be:
  if `--env-file` is provided, treat it as an advanced/manual path and do not auto-create it
  if no `--env-file` is provided and `~/.brain3/.env` is missing, launch the first-run TUI
  otherwise, run the configured runtime/bootstrap path through the shared gateway-server helper
  keep `--setup` mapped to the existing named-tunnel flow

- [ ] Keep `main.rs` as a composition root.
  After this task it should:
  initialize logging
  parse args
  resolve the env path
  detect first-run vs manual env-file usage
  dispatch into either:
  first-run TUI
  configured runtime bootstrap
  existing named-tunnel `--setup` flow

- [ ] Extend `RuntimeBootstrap` just enough to keep server-URL selection logic out of the TUI.
  Right now the bootstrap result exposes `public_url`, but the connection card needs one display-ready `server_url`.
  Update `crates/platform/src/runtime/bootstrap.rs` so the runtime path can provide a single URL choice for the card:
  public tunnel URL when present
  otherwise a local fallback derived from the gateway host/port
  This keeps the TUI from re-implementing connection URL policy.

- [ ] Add a non-TTY fallback for first-run mode.
  If the default env file is missing and stdout/stderr are not interactive, print a clear setup-required message instead of trying to enter ratatui.
  Use a simple terminal-capability check in `main.rs` and preserve the current explicit error output for headless runs.

- [ ] Re-run the narrow verification set after the dispatch refactor:
  `cargo fmt --all --check`
  `cargo test -p brain3-core first_run_setup`
  `cargo test -p brain3-platform --test setup_bootstrap`
  `cargo build -p brain3`

---

**Recommended execution order**
1. Task 9 first, because the shared setup API, log-file handoff, and runtime bootstrap now exist and the first-run TUI can build directly on them.
2. Task 10 last, as the dispatch/integration pass that swaps out the current first-run stub and adds the non-TTY fallback.
