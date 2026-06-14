# MCP Container Network Recreate-On-Start Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Recreate the Brain3-owned internal MCP network on each container start so the runtime always gets the current isolation settings, without leaving noisy benign errors in the logs.

**Architecture:** Choose deterministic recreate-on-start over inspect-and-reconcile. The network is Brain3-owned, single-purpose, and should be unused once the old MCP container has been stopped and removed, so full recreation is simpler and more reliable than diffing runtime-specific network metadata. Keep the runtime-specific network mechanics in the platform adapters, but expose a small public port hook so `EnsureContainerUseCase` can decide whether the next `run()` should attach to the isolated network or degrade cleanly.

**Tech Stack:** Rust, Tokio, `tracing`, Docker CLI, Apple `container` CLI

---

### Task 1: Add an explicit network-isolation preparation hook to the container port

**Files:**
- Modify: `crates/core/src/ports/container.rs`
- Modify: `crates/core/src/application/ensure_container.rs`

- [ ] **Step 1: Add a public port hook that prepares isolation and returns whether to attach it**

Update `crates/core/src/ports/container.rs`:

```rust
#[async_trait::async_trait]
pub trait ContainerPort: Send + Sync {
    async fn image_exists(&self, image: &str) -> Result<bool, ContainerError>;
    async fn pull_image(&self, image: &str) -> Result<(), ContainerError>;
    async fn exists(&self, id: &ContainerId) -> Result<bool, ContainerError>;
    async fn is_running(&self, id: &ContainerId) -> Result<bool, ContainerError>;
    async fn logs_tail(&self, id: &ContainerId, lines: usize) -> Result<String, ContainerError>;
    async fn prepare_network_isolation(&self) -> Result<bool, ContainerError>;
    async fn run(&self, config: &ContainerConfig) -> Result<ContainerId, ContainerError>;
    async fn stop(&self, id: &ContainerId) -> Result<(), ContainerError>;
    async fn remove(&self, id: &ContainerId) -> Result<(), ContainerError>;
}
```

The `bool` means:
- `true`: attach `--network brain3-mcp-net`
- `false`: start without network isolation because preparation failed or is unavailable

- [ ] **Step 2: Make `EnsureContainerUseCase` clone the config and apply the preparation result before `run()`**

Update `crates/core/src/application/ensure_container.rs` inside `EnsureContainerUseCase::ensure()`:

```rust
tracing::info!(container = %config.name, image = %config.image, "starting container");

let mut runtime_config = config.clone();
if config.network_isolated {
    runtime_config.network_isolated = self.port.prepare_network_isolation().await?;
}

let id = self.port.run(&runtime_config).await?;
```

This keeps the domain-level intent on `ContainerConfig`, but lets the adapter decide whether the current start can safely use the isolated network.

- [ ] **Step 3: Add one focused regression test for call order and downgrade behavior**

Extend the mock state in `crates/core/src/application/ensure_container.rs`:

```rust
#[derive(Debug, Clone, Default)]
struct MockState {
    image_exists: bool,
    container_exists: bool,
    container_running: bool,
    running_checks: Vec<bool>,
    logs_tail_output: Option<String>,
    prepare_network_isolation_result: bool,
    prepare_network_isolation_count: usize,
    last_run_network_isolated: Option<bool>,
    pull_count: usize,
    stop_count: usize,
    remove_count: usize,
    run_count: usize,
    logs_tail_count: usize,
    actions: Vec<&'static str>,
}
```

Implement the new mock trait method and capture the final `network_isolated` value passed to `run()`:

```rust
async fn prepare_network_isolation(&self) -> Result<bool, ContainerError> {
    let mut state = self.state.lock().unwrap();
    state.actions.push("prepare_network_isolation");
    state.prepare_network_isolation_count += 1;
    Ok(state.prepare_network_isolation_result)
}

async fn run(&self, config: &ContainerConfig) -> Result<ContainerId, ContainerError> {
    let mut state = self.state.lock().unwrap();
    state.actions.push("run");
    state.run_count += 1;
    state.last_run_network_isolated = Some(config.network_isolated);
    state.container_exists = true;
    state.container_running = true;
    Ok(ContainerId(config.name.clone()))
}
```

Add one public-API test:

```rust
#[tokio::test]
async fn prepares_network_isolation_before_run_and_downgrades_when_unavailable() {
    let port = Arc::new(MockContainerPort::new(MockState {
        image_exists: true,
        prepare_network_isolation_result: false,
        ..Default::default()
    }));
    let use_case = short_probe_use_case(port.clone());
    let mut config = sample_config();
    config.network_isolated = true;

    let id = use_case.ensure(&config).await.unwrap();

    assert_eq!(id.0, config.name);

    let state = port.snapshot();
    assert_eq!(state.prepare_network_isolation_count, 1);
    assert_eq!(state.last_run_network_isolated, Some(false));
    assert_eq!(
        state.actions,
        vec![
            "image_exists",
            "exists",
            "prepare_network_isolation",
            "run",
            "is_running"
        ]
    );
}
```

- [ ] **Step 4: Run the focused core test**

Run:

```bash
cargo test -p brain3-core prepares_network_isolation_before_run_and_downgrades_when_unavailable -- --exact
```

Expected:

```text
test prepares_network_isolation_before_run_and_downgrades_when_unavailable ... ok
```

### Task 2: Recreate the Docker internal network on each start

**Files:**
- Modify: `crates/platform/src/container/docker.rs`

- [ ] **Step 1: Replace `ensure_internal_network` with a deterministic recreate helper**

Use an existence probe first so benign “missing network” cases do not emit `ERROR` logs from `run_command()`:

```rust
async fn recreate_internal_network(name: &str) -> Result<(), ContainerError> {
    if command_succeeds("docker", &["network", "inspect", name]).await? {
        tracing::info!(network = name, "removing existing MCP network before recreation");
        run_command("docker", &["network", "rm", name]).await?;
    }

    tracing::info!(network = name, "creating fresh internal MCP network");
    run_command("docker", &["network", "create", "--internal", name]).await?;
    Ok(())
}
```

- [ ] **Step 2: Implement `prepare_network_isolation()` for Docker**

Add this method to `impl ContainerPort for DockerContainerAdapter`:

```rust
async fn prepare_network_isolation(&self) -> Result<bool, ContainerError> {
    match recreate_internal_network(MCP_NETWORK_NAME).await {
        Ok(()) => Ok(true),
        Err(e) => {
            tracing::warn!(
                error = %e,
                network = MCP_NETWORK_NAME,
                "network recreation failed; starting MCP container without outbound restrictions"
            );
            Ok(false)
        }
    }
}
```

- [ ] **Step 3: Keep `run()` limited to attaching the prepared network**

Inside `DockerContainerAdapter::run()`, remove the network creation match block and leave only the attach behavior:

```rust
if config.network_isolated {
    args.push("--network".into());
    args.push(MCP_NETWORK_NAME.into());
}
```

- [ ] **Step 4: Run the Docker-facing core regression test**

Run:

```bash
cargo test -p brain3-core prepares_network_isolation_before_run_and_downgrades_when_unavailable -- --exact
```

Expected:

```text
test prepares_network_isolation_before_run_and_downgrades_when_unavailable ... ok
```

### Task 3: Mirror recreate-on-start behavior in the macOS container adapter

**Files:**
- Modify: `crates/platform/src/container/macos_container.rs`

- [ ] **Step 1: Add the same recreate helper pattern for the Apple runtime**

Mirror the Docker flow, using the macOS container CLI network commands:

```rust
async fn recreate_internal_network(name: &str) -> Result<(), ContainerError> {
    if command_succeeds("container", &["network", "inspect", name]).await? {
        tracing::info!(network = name, "removing existing MCP network before recreation");
        run_command("container", &["network", "rm", name]).await?;
    }

    tracing::info!(network = name, "creating fresh internal MCP network");
    run_command("container", &["network", "create", "--internal", name]).await?;
    Ok(())
}
```

- [ ] **Step 2: Implement `prepare_network_isolation()` and simplify `run()`**

Add the same degrade-cleanly method:

```rust
async fn prepare_network_isolation(&self) -> Result<bool, ContainerError> {
    match recreate_internal_network(MCP_NETWORK_NAME).await {
        Ok(()) => Ok(true),
        Err(e) => {
            tracing::warn!(
                error = %e,
                network = MCP_NETWORK_NAME,
                "network recreation failed; starting MCP container without outbound restrictions"
            );
            Ok(false)
        }
    }
}
```

Then make `run()` only attach the network:

```rust
if config.network_isolated {
    args.push("--network".into());
    args.push(MCP_NETWORK_NAME.into());
}
```

- [ ] **Step 3: Run the focused core regression test again**

Run:

```bash
cargo test -p brain3-core prepares_network_isolation_before_run_and_downgrades_when_unavailable -- --exact
```

Expected:

```text
test prepares_network_isolation_before_run_and_downgrades_when_unavailable ... ok
```

### Task 4: Verify real runtime behavior and logging

**Files:**
- No code changes

- [ ] **Step 1: Start Brain3 with container mode enabled and confirm the network is recreated cleanly**

Run the normal startup path you already use. Expected log shape:

```text
INFO ... removing existing MCP network before recreation
INFO ... creating fresh internal MCP network
INFO ... starting container
INFO ... container ready
```

Not expected:

```text
ERROR ... network with name brain3-mcp-net already exists
```

- [ ] **Step 2: Confirm the container still comes up on the local port**

Run:

```bash
curl -fsS http://127.0.0.1:8520/
```

Expected: an HTTP response from the MCP upstream, or the same success signal the current startup health check relies on.

- [ ] **Step 3: Confirm the fresh network exists after startup**

Run:

```bash
docker network inspect brain3-mcp-net
```

On macOS runtime, use:

```bash
container network inspect brain3-mcp-net
```

Expected: the runtime reports a single fresh internal network named `brain3-mcp-net`.

## Design note: why not inspect-and-reconcile?

Inspect-and-reconcile is possible, but it is not the better first move here.

- The desired state is tiny: Brain3 wants one named internal network with no drift.
- Runtime inspection output is adapter-specific and more brittle than the create path.
- Full recreation is deterministic after the container is removed, which is already how `EnsureContainerUseCase` behaves.
- Recreate-on-start removes the current noisy “already exists” branch entirely instead of teaching the code about more tolerated stale states.

If later you need runtime-preserved network metadata that must survive restarts, revisit reconcile-in-place. For the current Brain3 MCP container, recreate-on-start is the simpler and safer policy.
