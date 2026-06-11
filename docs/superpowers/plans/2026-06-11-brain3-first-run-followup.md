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
  dispatch first-run, configured startup, or automatic named-tunnel remediation when the loaded config requires it
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
  Keep `apps/gateway/src/cloudflare_named_tunnel_setup_tui.rs` as the separate named-tunnel provisioning flow; it can later be auto-dispatched when configured startup detects that named-tunnel assets are missing.

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
  Do not replace or fold in `apps/gateway/src/cloudflare_named_tunnel_setup_tui.rs`; that named-tunnel checklist remains a separate flow.
  It may later be entered automatically for configured named-tunnel remediation, but it is not part of the first-run wizard itself.
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

- [ ] Leave `apps/gateway/src/cloudflare_named_tunnel_setup_tui.rs` alone for now.
  Do not fold its named-tunnel checklist into the new wizard yet; keep the new first-run TUI separate so this slice stays focused.
  The desired UX is still one-command startup, but that should be achieved by auto-dispatching into this existing flow when needed, not by merging named-tunnel provisioning into the first-run wizard.

- [ ] Do not add TUI snapshot tests.
  Prefer one manual smoke test:
  start with an empty `BRAIN3_HOME`
  confirm the wizard appears
  walk through setup to the runtime status screen

### Task 10A: Add Launch-Mode Flags And Dispatch Policy

**Files:**
- Modify: `apps/gateway/src/main.rs`
- Rename: `apps/gateway/src/setup_tui.rs` -> `apps/gateway/src/cloudflare_named_tunnel_setup_tui.rs`

- [ ] Add explicit launch-mode flags:
  `--tui`
  `--cli`
  Default to `--tui` behavior when neither flag is passed.

- [ ] Remove the public `--setup` flag.
  The existing named-tunnel setup flow is not a general configuration wizard; it is a remediation/provisioning path for installs that already selected Cloudflare named tunnel mode.
  If Brain3 can auto-detect when that path is required, it should not expose a separate operator-facing launch mode for it.

- [ ] Auto-dispatch the existing named-tunnel provisioning flow when configured startup needs it.
  If the loaded config selects Cloudflare named tunnel mode and the required tunnel config/credential assets are missing, an interactive default launch should enter the existing `apps/gateway/src/cloudflare_named_tunnel_setup_tui.rs` flow automatically.
  `--cli` should not try to provision a named tunnel; it should refuse and tell the operator exactly to rerun:
  `brain3 --tui`
  from an interactive terminal.

- [ ] Rename the tunnel-specific TUI module to match its real purpose.
  Because this flow is only for Cloudflare named tunnel provisioning, rename:
  `apps/gateway/src/setup_tui.rs`
  to:
  `apps/gateway/src/cloudflare_named_tunnel_setup_tui.rs`
  and update module references accordingly.

- [ ] Treat `--env-file` as an advanced/manual path in both modes.
  If `--env-file` is provided, do not auto-create it and do not silently fall back to the first-run wizard.
  A missing custom env file should fail explicitly.

- [ ] Keep `apps/gateway/src/main.rs` as a composition root.
  After this chunk it should only:
  initialize logging
  parse args
  resolve the env path
  choose launch mode
  dispatch into:
  default/runtime TUI
  explicit CLI mode
  automatic named-tunnel remediation flow when startup detects it is required

### Task 10B: Broaden The TUI Entry Point Beyond First Run

**Files:**
- Modify: `apps/gateway/src/tui/mod.rs`
- Modify: `apps/gateway/src/tui/app.rs`
- Modify: `apps/gateway/src/tui/state.rs`
- Modify: `apps/gateway/src/tui/screens.rs`
- Modify: `apps/gateway/src/server.rs`

- [ ] Rename or widen `run_first_run_tui(...)` into a general gateway TUI entry point.
  The TUI should be able to start in one of two modes:
  first-run wizard mode when the default config is missing
  configured-runtime mode when setup already exists

- [ ] In default `--tui` mode with no default `.env`, start at the welcome screen and keep the Task 9 wizard flow as-is.

- [ ] In default `--tui` mode with an existing config, load the config, bootstrap the runtime, start the shared gateway server, and enter directly on the runtime-status screen.
  Do not force already-configured users back through the setup wizard.

- [ ] Keep the connection-card screen tied to the “wizard just completed” path.
  Ordinary configured launches can skip straight to runtime status so the default interactive mode stays lightweight.

- [ ] Update the TUI copy/chrome so it no longer reads as first-run-only.
  The same app shell now serves both initial setup and everyday runtime status.

### Task 10C: Gate `--cli` Behind Completed Setup

**Files:**
- Modify: `apps/gateway/src/main.rs`
- Modify: `apps/gateway/src/logging.rs`

- [ ] Keep the current lightweight stdout/stderr startup path behind explicit `--cli`.
  This mode should behave like the current gateway launch once it is allowed to run:
  run in the foreground
  keep startup/server logs visible in the terminal
  exit on `Ctrl-C` through the existing shutdown path

- [ ] Add a CLI-ready preflight before entering that path.
  Refuse `--cli` when:
  the env/config file is missing
  the dependency doctor still reports installable or manual-install requirements for the needed runtime pieces
  the installation is otherwise not ready for ordinary startup

- [ ] Reuse the existing doctor path rather than inventing parallel shell checks.
  Use the same dependency status collection that powers the setup wizard so “interactive setup needed” is decided consistently.

- [ ] Preserve operator-visible logging in `--cli` mode.
  If the process still writes to the temp log file for postmortem/debugging, mirror tracing output to the terminal too.
  Do not make `--cli` a silent file-only mode.

- [ ] On refusal, print a clear message telling the operator that `--cli` only works after interactive setup is complete and to rerun without `--cli`.
  Keep this message short and operational.

### Task 10D: Keep Shared Startup Policy Out Of The TUI And Verify

**Files:**
- Modify: `apps/gateway/src/main.rs`
- Modify: `apps/gateway/src/server.rs`
- Modify: `crates/platform/src/runtime/bootstrap.rs`

- [ ] Keep named-tunnel remediation as shared startup policy, not a user-managed flag path.
  If configured startup selects Cloudflare named tunnel mode but the required local tunnel assets are missing, the default interactive launch path should enter the existing named-tunnel provisioning flow automatically.
  `--cli` and non-interactive launches should not dead-end with “existing config required” or “run --setup” copy; they should tell the operator exactly to run:
  `brain3 --tui`
  in an interactive terminal.

- [ ] Keep server-URL selection and runtime/server handoff glue out of the TUI screen layer.
  Use one shared policy for the display-ready server URL:
  public tunnel URL when present
  otherwise the bound local gateway URL

- [ ] If `RuntimeBootstrap` needs a small addition to support that shared policy, keep it narrow and reusable.
  Do not make the TUI re-derive connection URL policy from raw config.

- [ ] Add a non-interactive fallback for the default `--tui` path.
  If Brain3 would launch the TUI by default but stdout/stderr are not interactive, print clear setup/runtime guidance instead of trying to enter ratatui.
  Do not print vague “wizard not implemented” or “create the config manually” copy.
  The message must tell the operator exactly what command to run next.
  For first-run / missing default config, tell them to run:
  `brain3 --tui`
  from an interactive terminal to launch the setup wizard.
  For configured installs, tell them to run:
  `brain3 --tui`
  for the interactive status dashboard, or:
  `brain3 --cli`
  for the foreground non-TUI startup path.

- [ ] Re-run the narrow verification set after the dispatch refactor:
  `cargo fmt --all --check`
  `cargo test -p brain3-core first_run_setup`
  `cargo test -p brain3-platform --test setup_bootstrap`
  `cargo build -p brain3`

- [ ] Do one manual smoke pass for each launch mode:
  empty `BRAIN3_HOME`, run without flags, confirm the wizard appears
  empty `BRAIN3_HOME`, run without flags in a non-interactive context, confirm the fallback message includes the exact `brain3 --tui` command
  empty `BRAIN3_HOME`, run with `--cli`, confirm it refuses and tells you to rerun without `--cli`
  configured install, run with `--cli`, confirm the current lightweight foreground startup still works, logs remain visible, and `Ctrl-C` stops it cleanly
  configured install, run without flags, confirm the TUI runtime screen appears and Brain3 starts
  configured install, run without flags in a non-interactive context, confirm the fallback message includes exact commands for both `brain3 --tui` and `brain3 --cli`
  configured install with Cloudflare named tunnel mode but missing local tunnel assets, run without flags, confirm Brain3 enters the named-tunnel provisioning flow automatically
  configured install with Cloudflare named tunnel mode but missing local tunnel assets, run with `--cli`, confirm it refuses and tells the operator exactly to run `brain3 --tui`

---

**Recommended execution order**
1. Task 9 first, because the shared setup API, log-file handoff, and runtime bootstrap now exist and the first-run TUI can build directly on them.
2. Task 10A next, to lock in the `--tui` / `--cli` dispatch contract in `main.rs`.
3. Task 10B after that, to widen the new TUI from first-run-only into the default interactive shell.
4. Task 10C next, to keep the old lightweight path available only for fully prepared installs.
5. Task 10D last, as the shared startup-policy cleanup and verification pass.
