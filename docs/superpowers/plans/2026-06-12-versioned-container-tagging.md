# Versioned Container Tagging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make new Brain3 installs default to a release-matched MCP container tag instead of `:latest`, add `--container-tag` for one-off overrides, and display the app version prominently in the CLI and TUI.

**Architecture:** Keep release/version logic in the gateway app, not in core. The gateway will expose one release metadata module that owns the app version string and official MCP image repo, then inject the default image into first-run setup and optionally override the configured image at runtime when `--container-tag` is passed.

**Tech Stack:** Rust, Clap, Ratatui, GitHub Actions

---

### Task 1: Centralize Release Metadata

**Files:**
- Create: `apps/gateway/src/release.rs`
- Modify: `apps/gateway/src/main.rs`
- Modify: `apps/gateway/Cargo.toml`

- [ ] **Step 1: Add a failing unit test for default image composition**

Add a test module in `apps/gateway/src/release.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_container_image_uses_versioned_tag() {
        assert_eq!(
            default_container_image(),
            format!("{MCP_IMAGE_REPO}:v{APP_VERSION}")
        );
    }

    #[test]
    fn container_image_for_tag_uses_official_repo() {
        assert_eq!(
            container_image_for_tag("pr-123"),
            format!("{MCP_IMAGE_REPO}:pr-123")
        );
    }
}
```

- [ ] **Step 2: Run the targeted test and verify it fails**

Run: `cargo test -p brain3 release::tests::default_container_image_uses_versioned_tag`

Expected: FAIL because `release.rs` does not exist yet.

- [ ] **Step 3: Implement release metadata helpers**

Create `apps/gateway/src/release.rs`:

```rust
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MCP_IMAGE_REPO: &str = "ghcr.io/tleyden/brain3-mcp-vault-tools";

pub fn default_container_image() -> String {
    format!("{MCP_IMAGE_REPO}:v{APP_VERSION}")
}

pub fn container_image_for_tag(tag: &str) -> String {
    format!("{MCP_IMAGE_REPO}:{}", tag.trim())
}

pub fn long_version() -> String {
    format!("Brain3 v{APP_VERSION}\nDefault MCP image: {}", default_container_image())
}
```

- [ ] **Step 4: Re-run the targeted test and verify it passes**

Run: `cargo test -p brain3 release::tests`

Expected: PASS

### Task 2: Inject Versioned Default Image Into First-Run Setup

**Files:**
- Modify: `crates/core/src/domain/setup.rs`
- Modify: `crates/core/src/application/first_run_setup.rs`
- Modify: `apps/gateway/src/main.rs`

- [ ] **Step 1: Add a failing core test for injected setup defaults**

Extend `crates/core/src/application/first_run_setup.rs` tests with:

```rust
#[tokio::test]
async fn prepare_uses_injected_default_container_image() {
    let port = Arc::new(MockSetupSystemPort::new(vec![]));
    let use_case = FirstRunSetupUseCase::new(
        port,
        SetupDefaults {
            default_container_image: "ghcr.io/tleyden/brain3-mcp-vault-tools:v9.9.9".into(),
        },
    );

    let preparation = use_case.prepare().await.unwrap();

    assert_eq!(
        preparation.draft.container_image,
        "ghcr.io/tleyden/brain3-mcp-vault-tools:v9.9.9"
    );
}
```

- [ ] **Step 2: Run the targeted test and verify it fails**

Run: `cargo test -p brain3-core prepare_uses_injected_default_container_image`

Expected: FAIL because `SetupDefaults` does not exist and `FirstRunSetupUseCase::new` does not accept it.

- [ ] **Step 3: Implement setup defaults injection**

Update `crates/core/src/domain/setup.rs` to remove the hardcoded `DEFAULT_CONTAINER_IMAGE` constant and add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupDefaults {
    pub default_container_image: String,
}
```

Update `crates/core/src/application/first_run_setup.rs` so `FirstRunSetupUseCase` stores `defaults: SetupDefaults` and `prepare()` uses `self.defaults.default_container_image.clone()`.

Update all `FirstRunSetupUseCase::new(...)` call sites in `apps/gateway/src/main.rs` and tests to pass:

```rust
SetupDefaults {
    default_container_image: release::default_container_image(),
}
```

- [ ] **Step 4: Re-run the targeted core test and verify it passes**

Run: `cargo test -p brain3-core prepare_uses_injected_default_container_image`

Expected: PASS

### Task 3: Add `--container-tag` Override

**Files:**
- Modify: `apps/gateway/src/main.rs`
- Modify: `apps/gateway/src/server.rs`

- [ ] **Step 1: Add a failing unit test for argument parsing**

Add a parser test near `apps/gateway/src/main.rs`:

```rust
#[test]
fn parses_container_tag_override() {
    let args = Args::parse_from(["brain3", "--container-tag", "pr-123"]);
    assert_eq!(args.container_tag.as_deref(), Some("pr-123"));
}
```

- [ ] **Step 2: Run the targeted parser test and verify it fails**

Run: `cargo test -p brain3 parses_container_tag_override`

Expected: FAIL because `Args` has no `container_tag` field.

- [ ] **Step 3: Implement the override path**

Add `container_tag: Option<String>` to `Args`:

```rust
#[arg(long, help = "Override the Brain3 MCP container tag for this run or new setup, e.g. latest, v0.1.5, pr-123")]
container_tag: Option<String>,
```

Add a small helper struct in `main.rs` or `server.rs`:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RuntimeOverrides {
    container_tag: Option<String>,
}
```

Behavior:
- First-run setup uses `release::container_image_for_tag(tag)` when building `SetupDefaults`.
- Configured launches load the env config, then if `container_tag` is set, rewrite `config.container.as_mut().unwrap().image` to `release::container_image_for_tag(tag)` before bootstrap.

- [ ] **Step 4: Re-run the parser test and verify it passes**

Run: `cargo test -p brain3 parses_container_tag_override`

Expected: PASS

### Task 4: Display Version Prominently In CLI And TUI

**Files:**
- Modify: `apps/gateway/src/main.rs`
- Modify: `apps/gateway/src/tui/screens.rs`

- [ ] **Step 1: Add a failing TUI rendering test for the version header**

Add a small rendering-focused unit test around the header helper in `apps/gateway/src/tui/screens.rs`:

```rust
#[test]
fn header_mentions_brain3_version() {
    let state = sample_state();
    let text = header_line(&state).to_string();
    assert!(text.contains("Brain3 v"));
}
```

- [ ] **Step 2: Run the targeted test and verify it fails**

Run: `cargo test -p brain3 header_mentions_brain3_version`

Expected: FAIL because the current header omits the version.

- [ ] **Step 3: Implement version display**

In `main.rs`, set Clap metadata from `release`:

```rust
#[command(
    name = "brain3",
    about = "OAuth2 gateway for MCP servers",
    version = release::APP_VERSION,
    long_version = release::APP_VERSION
)]
```

Update help text or `after_help` so `--help` visibly includes the release version and the `--container-tag` examples.

In `apps/gateway/src/tui/screens.rs`, update the header to render `Brain3 v{APP_VERSION}` prominently on every screen.

- [ ] **Step 4: Re-run the targeted test and verify it passes**

Run: `cargo test -p brain3 header_mentions_brain3_version`

Expected: PASS

### Task 5: Tighten Release Workflow And Docs

**Files:**
- Modify: `.github/workflows/release.yml`
- Modify: `README.MD`
- Modify: `brain3-mcp-vault-tools/README.md`

- [ ] **Step 1: Add a failing release-tag validation script invocation**

Add a workflow step before build:

```bash
APP_VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' apps/gateway/Cargo.toml | head -n1)
if [ "v${APP_VERSION}" != "${GITHUB_REF_NAME}" ]; then
  echo "Tag ${GITHUB_REF_NAME} does not match apps/gateway version v${APP_VERSION}" >&2
  exit 1
fi
```

- [ ] **Step 2: Update docs**

Document:
- default new-install image is `ghcr.io/tleyden/brain3-mcp-vault-tools:vX.Y.Z`
- `--container-tag latest`
- `--container-tag pr-123`
- `:latest` is published but not the default

- [ ] **Step 3: Run doc/workflow-adjacent verification**

Run: `cargo test -p brain3`

Expected: PASS

Run: `cargo test -p brain3-core`

Expected: PASS

### Task 6: Final Verification

**Files:**
- No new files

- [ ] **Step 1: Run focused verification for the touched packages**

Run:

```bash
cargo test -p brain3
cargo test -p brain3-core
cargo test -p brain3-platform
```

Expected: PASS

- [ ] **Step 2: Manually verify the new CLI surface**

Run:

```bash
cargo run -p brain3 -- --help
cargo run -p brain3 -- --version
```

Expected:
- help shows the app version and `--container-tag`
- version prints `0.1.x`

- [ ] **Step 3: Summarize final behavior**

Verify these outcomes in the final report:
- new installs default to `v{brain3_version}`
- `--container-tag latest` works
- `--container-tag pr-123` works
- TUI header shows the app version
