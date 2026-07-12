# Plan: Enforce plugin network isolation support at the container startup choke point

## Status

- Date: 2026-07-12
- Follow-up to: `docs/superpowers/plans/2026-07-12-macos-docker-plugin-internal-network-counter-plan.md`
- Scope: Add a mandatory runtime capability check immediately before Brain3 creates or reuses an internal container network.
- This document is a plan only. No implementation changes are included.

## Why this follow-up is needed

Phase 0 rejects `platform: docker` plugin MCP entries while parsing
`brain3.yaml` on macOS. That is useful early feedback, but config parsing is
not the authoritative enforcement boundary:

- `PluginMcpContainerConfig` can be constructed directly by tests or future
  callers without passing through `brain3_yaml.rs`.
- Startup currently trusts the resulting `ContainerConfig` and calls
  `EnsureContainerUseCase::ensure`.
- `EnsureContainerUseCase` is the one shared path that actually calls
  `ContainerPort::ensure_internal_network` for both primary and plugin MCP
  containers.

The deeper invariant should therefore be enforced in the shared container
startup path. The config-load guard should remain because it produces an
earlier, more actionable error for normal users, but it must not be the only
guard.

## Design

Add a runtime capability validation method to the `ContainerPort` trait and
make `EnsureContainerUseCase` call it inside the existing
`isolation_strategy.is_some()` block, immediately before
`ensure_internal_network`.

The capability decision belongs to the adapter:

- The Docker adapter knows that it represents Docker and can reject the
  unsupported macOS Docker isolation configuration.
- The macOS native-container adapter accepts its supported isolation
  configuration.
- The core use case owns call ordering and guarantees that no internal
  network is created or reused until the selected adapter accepts the
  configuration.

This avoids adding a runtime field to `ContainerConfig`, which is deliberately
runtime-agnostic, and avoids duplicating runtime checks in each startup caller.

## Implementation steps

### 1. Extend the container port contract

In `crates/core/src/ports/container.rs`, add a synchronous validation method
such as:

```rust
fn validate_internal_network_support(
    &self,
    config: &ContainerConfig,
) -> Result<(), ContainerError>;
```

Pass the complete `ContainerConfig` so diagnostics can identify the container
and the adapter can inspect the selected isolation strategy without growing
the method signature as network configuration evolves.

Do not give this method a permissive default implementation. Requiring every
adapter to implement it makes support an explicit part of the port contract.

### 2. Enforce it at the shared choke point

In `crates/core/src/application/ensure_container.rs`, update the isolated
network block to perform operations in this order:

1. Call `validate_internal_network_support(config)`.
2. Return the validation error immediately when unsupported.
3. Only then call `ensure_internal_network(&config.network_name)`.
4. Continue with `run` only after network preparation succeeds.

Keep the check immediately adjacent to `ensure_internal_network`. This makes
the safety property clear during review and prevents later startup refactors
from accidentally separating validation from the protected operation.

### 3. Implement adapter-specific policy

In `crates/platform/src/container/docker.rs`:

- On macOS, reject every isolated Docker `ContainerConfig`, regardless of
  whether its strategy is `PublishToLoopback` or `DiscoverContainerIp`. This
  exactly mirrors the existing primary-container guard and prevents callers
  from bypassing it by constructing a different strategy directly.
- The error must identify the container, state that Docker is unsupported for
  network-isolated MCP containers on macOS, and direct the user to the native
  `macos_container` runtime.
- On Linux, preserve the supported Docker isolation path.
- Log the rejected runtime, container, network, and isolation strategy at an
  appropriate error or warning level without logging secrets or environment
  values.

In `crates/platform/src/container/macos_container.rs`, explicitly accept its
supported isolation strategies. Keep this implementation small; it documents
the adapter capability rather than adding behavior.

Use a specific `ContainerError` variant if the existing variants cannot
describe an unsupported runtime configuration without presenting it as a
generic operational failure. Do not use `Conflict`, which is reserved for
existing container or network state conflicts.

### 4. Retain the Phase 0 config guard

Keep `validate_plugin_network_isolation_support` in
`crates/platform/src/config/brain3_yaml.rs`.

The two checks serve different purposes:

- Config validation gives immediate feedback for normal `brain3.yaml` usage.
- Startup validation enforces the invariant for every caller at the last safe
  point before network mutation.

Where practical, keep their user-facing guidance aligned so the same invalid
configuration does not produce contradictory remedies.

## Tests

### Core use-case tests

Extend the existing public-behavior tests for `EnsureContainerUseCase` and its
mock `ContainerPort`:

- A rejected capability check returns the adapter error.
- `ensure_internal_network` is not called after rejection.
- `run` is not called after rejection.
- For an accepted isolated configuration, action order is capability check,
  `ensure_internal_network`, then `run`.
- A non-isolated configuration does not invoke internal-network capability
  validation because no internal network operation follows.

These tests should assert observable use-case behavior and port calls, not
private helper details or log output.

### Adapter tests

Add focused tests at the `ContainerPort` API boundary:

- On macOS, Docker rejects both `PublishToLoopback` and
  `DiscoverContainerIp` before any Docker command is executed, and the error
  includes the container name and native-runtime guidance.
- The macOS native-container adapter accepts `PublishToLoopback`.
- On Linux, Docker accepts `DiscoverContainerIp`.

Use `cfg(target_os)` to keep platform expectations explicit.

### Existing config tests

Retain the Phase 0 tests proving that macOS Docker plugin entries are rejected
during config loading and native-container entries are accepted.

## Verification

Run locally:

```bash
cargo fmt --check
cargo test -p brain3 --no-run
cargo test
```

Because this changes the shared container startup contract, also run the E2E
smoke test when the local runtime can exercise a supported configuration:

```bash
uv run scripts/e2e_smoke.py
```

The macOS Docker plugin scenario is intentionally rejected and therefore
cannot be a successful local E2E case. Linux Docker success coverage remains
the responsibility of the existing GitHub Actions Docker job; do not run
Linux containers locally.

## Security and scope

- This adds a denial guard and no new ingress, OAuth capability, network path,
  or credential flow. No `SECURITY_AUDIT.MD` threat-model update is required.
- Do not change Docker network attachment behavior in this follow-up; that is
  Phase 1 of the counter-plan.
- Do not remove or weaken the existing primary-container configuration guard.
- Do not modify `apps/gateway/src/main.rs`; the shared port/use-case boundary
  is the correct enforcement point.

## Completion criteria

- No isolated container path can call `ensure_internal_network` before the
  selected runtime adapter validates support.
- macOS Docker plugin startup fails with actionable runtime guidance even when
  config parsing is bypassed.
- Rejection performs no network creation/reuse and launches no container.
- Primary-container and supported Linux Docker/macOS native-container behavior
  remains unchanged.
- Required local Rust verification passes, with platform-appropriate E2E or CI
  coverage recorded.
