# Runtime Startup Health And Legacy Image Selection Follow-Up Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Why this follow-up exists:** Manual testing found two gaps after the versioned-tagging work landed:

1. Gateway startup logs showed `image=ghcr.io/tleyden/brain3-mcp-vault-tools:latest`, so that run did **not** attempt `:v0.1.5`.
2. The TUI rendered `Container: Started`, but container logs showed `Vault path does not exist: /Obsidian/MyVault`, so startup success is currently based on “container process spawned” rather than “container stayed up and is actually serving”.

**Goal:** Ensure Brain3 uses the release-matched MCP image by default even when older configs still contain the old official `:latest` image, and accurately report container startup failures in the TUI/CLI instead of optimistic `Started` status.

**Architecture:** Add an explicit “effective container image” resolution step in the gateway before runtime bootstrap. Treat the exact official `ghcr.io/tleyden/brain3-mcp-vault-tools:latest` value in saved config as a legacy-default candidate, while keeping explicit `--container-tag` overrides highest priority. Separately, extend container startup from fire-and-forget to verified startup: after `run`, poll container liveness and local port readiness, collect recent container logs on failure, and surface structured startup states through `RuntimeBootstrap` into the TUI.

**Tech Stack:** Rust, Clap, Ratatui, Docker / macOS container CLI, GitHub Actions

> **Decision note:** This plan assumes the exact official `:latest` image in legacy Brain3 config should be treated as an old default and remapped in-memory to `v{APP_VERSION}` unless the user explicitly passes `--container-tag latest`. If you prefer warning-only behavior instead of remapping, adjust Task 1 before implementation.

---

### Task 1: Resolve The Effective Container Image Deliberately

**Files:**
- Modify: `apps/gateway/src/release.rs`
- Modify: `apps/gateway/src/main.rs`
- Modify: `apps/gateway/src/tui/screens.rs`
- Modify: `README.MD`

- [ ] **Step 1: Add failing tests for legacy-latest resolution and explicit overrides**

Add focused tests near the gateway startup helpers covering:

```rust
#[test]
fn legacy_official_latest_resolves_to_release_tag() {
    assert_eq!(
        resolve_effective_container_image(
            Some("ghcr.io/tleyden/brain3-mcp-vault-tools:latest"),
            None,
        )
        .image,
        release::default_container_image()
    );
}

#[test]
fn explicit_container_tag_latest_wins_over_legacy_remap() {
    assert_eq!(
        resolve_effective_container_image(
            Some("ghcr.io/tleyden/brain3-mcp-vault-tools:latest"),
            Some("latest"),
        )
        .image,
        release::container_image_for_tag("latest")
    );
}

#[test]
fn custom_configured_image_is_left_unchanged() {
    let custom = "ghcr.io/acme/custom-mcp:dev";
    assert_eq!(
        resolve_effective_container_image(Some(custom), None).image,
        custom
    );
}
```

- [ ] **Step 2: Implement effective-image resolution helpers**

Add a small helper in `main.rs` or `release.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum EffectiveContainerImageSource {
    FreshInstallDefault,
    ConfiguredImage,
    LegacyLatestConfig,
    ExplicitTagOverride,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EffectiveContainerImage {
    image: String,
    source: EffectiveContainerImageSource,
}
```

Resolution rules:
1. `--container-tag` always wins.
2. The exact official `ghcr.io/tleyden/brain3-mcp-vault-tools:latest` value in saved config is treated as a legacy default and remapped to `release::default_container_image()` for that run.
3. Any other configured image stays untouched.
4. Fresh-install setup defaults still use `release::default_container_image()`.

- [ ] **Step 3: Log and display the resolved image source clearly**

Add startup logging such as:

```rust
tracing::info!(
    image = %effective.image,
    source = ?effective.source,
    "resolved MCP container image"
);
```

Show the effective image on the setup summary and runtime status screens so the user can see the exact tag without opening logs.

- [ ] **Step 4: Re-run the targeted gateway tests and verify they pass**

Run:

```bash
cargo test -p brain3 legacy_official_latest_resolves_to_release_tag
cargo test -p brain3 explicit_container_tag_latest_wins_over_legacy_remap
cargo test -p brain3 custom_configured_image_is_left_unchanged
```

Expected: PASS

### Task 2: Verify Container Startup Instead Of Assuming `run` Means Ready

**Files:**
- Modify: `crates/core/src/ports/container.rs`
- Modify: `crates/core/src/application/ensure_container.rs`
- Modify: `crates/core/src/domain/errors.rs`
- Modify: `crates/platform/src/container/docker.rs`
- Modify: `crates/platform/src/container/macos_container.rs`
- Modify: `crates/platform/src/container/startup.rs`

- [ ] **Step 1: Add failing tests for immediate container exit and log-tail inclusion**

Extend `crates/core/src/application/ensure_container.rs` tests with cases where:
- `run()` succeeds
- `is_running()` becomes `false` during a short readiness window
- the returned error includes recent container logs

Example intent:

```rust
#[tokio::test]
async fn returns_error_when_container_exits_during_startup_probe() {
    // run succeeds, but is_running becomes false before readiness completes
}

#[tokio::test]
async fn startup_failure_includes_recent_container_logs() {
    // logs_tail returns the Python vault-path error and the error surfaces it
}
```

- [ ] **Step 2: Extend the container port with log-tail support**

Add to `ContainerPort`:

```rust
async fn logs_tail(&self, id: &ContainerId, lines: usize) -> Result<String, ContainerError>;
```

Implement it via:
- Docker: `docker logs --tail N <container>`
- macOS container CLI: corresponding `container logs` command

- [ ] **Step 3: Add a readiness probe after `run()`**

In `EnsureContainerUseCase::ensure(...)`:
- keep the current image pull / restart behavior
- after `run()`, poll for a short bounded window (for example 5–10 seconds)
- require both:
  - container still reports running
  - the mapped loopback host port accepts TCP connections
- if the container exits early or never becomes reachable, tail the logs and return a structured startup error

If needed, thread the mapped host address / port from `ContainerConfig.port_mappings` into the readiness check rather than introducing separate config plumbing.

- [ ] **Step 4: Re-run the targeted core tests and verify they pass**

Run:

```bash
cargo test -p brain3-core returns_error_when_container_exits_during_startup_probe
cargo test -p brain3-core startup_failure_includes_recent_container_logs
```

Expected: PASS

### Task 3: Surface Failure States In The TUI And CLI

**Files:**
- Modify: `crates/platform/src/runtime/bootstrap.rs`
- Modify: `apps/gateway/src/server.rs`
- Modify: `apps/gateway/src/tui/state.rs`
- Modify: `apps/gateway/src/tui/screens.rs`
- Modify: `apps/gateway/src/tui/app.rs`

- [ ] **Step 1: Add failing rendering/status tests for failed container startup**

Add TUI-focused tests asserting that a failed runtime shows `Failed` rather than `Started`, and that a short failure detail is rendered.

Example:

```rust
#[test]
fn runtime_screen_shows_failed_container_status() {
    let state = sample_failed_runtime_state();
    let text = runtime_lines(&state)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Container:  Failed"));
    assert!(text.contains("Vault path does not exist"));
}
```

- [ ] **Step 2: Replace the binary startup status enum with a richer model**

Replace:

```rust
pub enum StartupStatus {
    NotConfigured,
    Started,
}
```

with something like:

```rust
pub enum StartupStatus {
    NotConfigured,
    Ready,
    Failed { summary: String },
}
```

Keep it minimal — synchronous startup does not need a long-lived `Starting` state unless the implementation naturally benefits from it.

- [ ] **Step 3: Preserve partial runtime results instead of collapsing everything into `Err` too early**

Behavior goals:
- In TUI mode, if the container fails startup verification, keep the user on a status screen that shows the failure instead of exiting with a generic error.
- Only show `Gateway: Running` after the container is truly ready and the gateway server has started.
- In CLI mode, exit non-zero with the same failure summary and log-file path.

This may require introducing a small runtime bootstrap report / session result type instead of assuming startup is all-or-nothing.

- [ ] **Step 4: Show actionable failure details**

On the runtime screen, add:
- `Container: Failed`
- effective image tag
- short failure summary
- config path and log path
- guidance to open the logs view when available

- [ ] **Step 5: Re-run the targeted gateway tests and verify they pass**

Run:

```bash
cargo test -p brain3 runtime_screen_shows_failed_container_status
```

Expected: PASS

### Task 4: Add Release Smoke Checks So Broken Images Are Caught Earlier

**Files:**
- Modify: `.github/workflows/container.yml`
- Modify: `.github/workflows/release.yml`
- Modify: `brain3-mcp-vault-tools/README.md`
- Modify: `README.MD`

- [ ] **Step 1: Add a failing smoke-test step for the just-built container image**

Before publishing, run the image with:
- a temporary host vault directory mounted into `/vault`
- the expected `B3_VAULT_PATH=/vault`
- the expected upstream-secret mount / env file
- a loopback port publish

Then verify one of the following succeeds within a timeout:
- container remains running for a short window, and/or
- the mapped port accepts TCP connections

- [ ] **Step 2: On tagged releases, verify the expected version tag exists**

Add a release-adjacent check that the app release version and the published MCP image tag line up. At minimum, confirm that `v${APP_VERSION}` is the tag Brain3 expects to run and that release automation publishes or validates that exact tag.

- [ ] **Step 3: Update docs for the new behavior**

Document:
- legacy official `:latest` configs are treated specially by Brain3
- `--container-tag latest` remains the explicit opt-in path
- runtime status now distinguishes ready vs failed container startup

- [ ] **Step 4: Run doc/workflow-adjacent verification**

Run:

```bash
cargo test -p brain3
cargo test -p brain3-core
```

Expected: PASS

### Task 5: Final Verification

**Files:**
- No new files

- [ ] **Step 1: Clean-install smoke test**

Run a fresh-install scenario with a temporary Brain3 app home and verify:
- generated `.env` contains `B3_CONTAINER_IMAGE="ghcr.io/tleyden/brain3-mcp-vault-tools:v0.1.5"`
- runtime logs show the same versioned image

- [ ] **Step 2: Legacy-config smoke test**

Seed config with:

```env
B3_CONTAINER_IMAGE="ghcr.io/tleyden/brain3-mcp-vault-tools:latest"
```

Run Brain3 without `--container-tag` and verify:
- effective runtime image resolves to `v0.1.5`
- a warning or info message explains that a legacy default was remapped

- [ ] **Step 3: Explicit opt-in smoke test**

Run:

```bash
brain3 --container-tag latest
brain3 --container-tag pr-123
```

Verify the runtime uses those exact tags.

- [ ] **Step 4: Failure-reporting smoke test**

Run with an intentionally bad vault path and verify:
- TUI shows `Container: Failed` instead of `Started`
- the failure summary mentions the bad vault path or the recent container-log tail
- CLI mode exits non-zero with the same failure context

- [ ] **Step 5: Summarize final behavior**

Verify these outcomes in the final report:
- fresh installs default to `v{brain3_version}`
- legacy official `:latest` configs no longer silently keep Brain3 on `:latest`
- `--container-tag latest` still works as an explicit opt-in
- TUI no longer reports `Started` for containers that crash immediately after launch
