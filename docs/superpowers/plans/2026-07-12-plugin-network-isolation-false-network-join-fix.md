# RCA + Plan: `network_isolation: false` never joins the configured `network`

## Status

- Date: 2026-07-12
- This document is a plan only. No implementation changes are included yet.
- Supersedes one specific claim in the just-landed
  `docs/superpowers/plans/2026-07-12-plugin-per-container-network-isolation-flag.md`
  (see "Relationship to prior plans" below). Does not re-litigate anything
  else in that plan or in
  `docs/superpowers/plans/2026-07-12-macos-docker-plugin-internal-network-counter-plan.md`.

## User-reported symptom

```yaml
# ~/.brain3/brain3.yaml
plugin_mcp_containers:
  - name: fluensy_learn
    platform: docker
    image: fluensy-learn-mcp
    tag: latest
    port: 3000
    network: fluensy-learn-net
    host_directory: /Users/tleyden/fluensy-data
    container_directory: /data
    network_isolation: false
    auth:
      type: none
```

Brain3 launched, but `fluensy_learn` could not reach `postgrest` (a sibling
container from an unrelated `local-supabase` Docker Compose stack, already
attached to the pre-existing `fluensy-learn-net` network, which that stack
created with `Internal: true`).

## Root cause

`network_isolation: false` produces `ContainerConfig.isolation_strategy =
None` (`crates/platform/src/container/startup.rs:372-374`). Both runtime
adapters gate the `--network` flag on `isolation_strategy`, not on whether
`network_name` is set:

- `crates/platform/src/container/docker.rs:291-297`:
  ```rust
  if matches!(config.isolation_strategy, Some(ContainerNetworkIsolationStrategy::DiscoverContainerIp)) {
      args.push("--network".into());
      args.push(config.network_name.clone());
  }
  ```
- `crates/platform/src/container/macos_container.rs:367-369`: same idea,
  gated on `config.isolation_strategy.is_some()`.

So when `isolation_strategy` is `None`, **neither adapter ever emits
`--network` at all**, regardless of what `network:` names in `brain3.yaml`.
`config.network_name` is computed and threaded all the way through
(`build_plugin_container_config` in `startup.rs:391`), but it is silently
dropped on the floor for the non-isolated path. The container lands on
Docker's default `bridge` network. It still gets `--publish
127.0.0.1:<host_port>:3000` (the publish loop only skips
`DiscoverContainerIp`), so the gateway can still reach the plugin — this is
why the container "launched" successfully — but the plugin container itself
has no path to `fluensy-learn-net` or anything on it, including `postgrest`.

Confirmed live on the user's machine: `docker network inspect
fluensy-learn-net` shows only `local-supabase-postgrest-1` and
`local-supabase-db-1` attached; no brain3-managed container has ever joined
it, on any of the runs so far.

### Relationship to prior plans

`docs/superpowers/plans/2026-07-12-plugin-per-container-network-isolation-flag.md`
(the plan that added the `network_isolation` field, already implemented) has
this in its Non-goals:

> Do not fix the separate `docker.rs` `--network` contract-violation bug
> flagged in the counter-plan (Phase 1, only relevant when isolation is
> *enabled*). Out of scope here.

That "only relevant when isolation is enabled" claim is **incorrect** — this
session's finding is that the identical class of bug (network name computed
but never passed to `--network`) also applies to the `isolation: false` path
that plan itself introduced as the documented escape hatch. The Phase 1 bug
(`PublishToLoopback` isolated strategy missing `--network` in `docker.rs`)
and this bug (`None`/non-isolated missing `--network` in both adapters) are
two instances of the same root pattern: `docker.rs`'s `--network` gating
doesn't match `macos_container.rs`'s, and neither matches "attach whenever
there's a network name to attach to." This plan fixes both instances
together rather than patching them one at a time, since the correct
condition ends up being the same in both files: always attach, in all three
states (`DiscoverContainerIp`, `PublishToLoopback`, `None`).

## What the user actually needs (design gap, not just a bug)

The doc comment on `PluginMcpContainerConfig::network_isolation`
(`crates/core/src/domain/model.rs:78-83`) currently says:

> When false, the runtime's normal default network is used and the plugin
> regains outbound egress.

That is a real, valid use case (a plugin that just needs internet access and
doesn't care what network it's on). But it is not the user's use case here,
and the field can't currently express it: the user needs the plugin
container to join a **specific, already-existing, externally-managed**
network (`fluensy-learn-net`, created by a separate Compose stack, itself
`Internal: true`) so it can reach a sibling service by container name. There
is currently no config state that means "join exactly the network I named,
whatever its properties, and don't try to manage it as an internal
brain3-owned network."

The fix below changes what `network_isolation: false` means: instead of
"ignore `network:` and use the default bridge," it becomes "join `network:`
like any other Docker network — create it as a plain (non-`--internal`)
bridge network if it doesn't exist yet, or just attach if it already exists,
regardless of who created it or whether it happens to be internal." This
still gives non-isolated plugins full egress (a custom non-`--internal`
bridge network has the same NAT'd internet access as the default bridge —
`--internal` is the only thing that removes it), while also making the
mandatory `network:` field do something for every plugin, not just isolated
ones.

## Exact CLI commands (as requested)

### What actually runs today (the bug)

```bash
docker run \
  --name fluensy_learn \
  --detach \
  --rm \
  --user 501:20 \
  --publish 127.0.0.1:<ephemeral-port>:3000 \
  --label io.brain3.managed=true \
  --label io.brain3.role=brain3-mcp-plugin:fluensy_learn \
  --label io.brain3.installation_id=<installation-id> \
  --mount type=bind,source=/Users/tleyden/fluensy-data,target=/data \
  fluensy-learn-mcp:latest
```

Note there is **no `--network` flag at all** — this is exactly the bug.
`<ephemeral-port>` is OS-assigned (`pick_free_loopback_port` in
`startup.rs:446-449`) since `host_port` wasn't set in the config; the actual
value is in `brain3.log` on the "Plugin MCP Container ready" line.

### What should run after this plan's fix

```bash
# only if fluensy-learn-net doesn't already exist — here it does, so this is skipped:
docker network create fluensy-learn-net

docker run \
  --name fluensy_learn \
  --detach \
  --rm \
  --user 501:20 \
  --publish 127.0.0.1:<ephemeral-port>:3000 \
  --label io.brain3.managed=true \
  --label io.brain3.role=brain3-mcp-plugin:fluensy_learn \
  --label io.brain3.installation_id=<installation-id> \
  --mount type=bind,source=/Users/tleyden/fluensy-data,target=/data \
  --network fluensy-learn-net \
  fluensy-learn-mcp:latest
```

You can reproduce/verify this manually right now, independent of any code
change, to confirm it fixes reachability before the fix lands:

```bash
docker rm -f fluensy_learn 2>/dev/null
docker run -d --name fluensy_learn --network fluensy-learn-net \
  --mount type=bind,source=/Users/tleyden/fluensy-data,target=/data \
  fluensy-learn-mcp:latest
docker exec fluensy_learn curl -sf http://host.docker.internal:3579 || \
  docker exec fluensy_learn getent hosts local-supabase-postgrest-1
```

## Plan

### 1. `crates/core/src/ports/container.rs`

Generalize the network-preparation port method to take an `internal: bool`
(or add a small `NetworkMode { Internal, Open }` enum — either works;
`bool` is fine given there are exactly two states and both adapters already
use booleans elsewhere like `config.detach`):

```rust
async fn ensure_network(
    &self,
    network_name: &str,
    internal: bool,
) -> Result<NetworkPreparation, ContainerError>;
```

Replacing `ensure_internal_network`. Rename, don't add a second method —
every call site is about to change anyway.

### 2. `crates/platform/src/container/docker.rs`

- Rename `inspect_internal_network_state` → parameterize on `internal`:
  - `internal == true`: unchanged behavior — network must report
    `Internal: true` to be `Compatible`; otherwise `Incompatible`.
  - `internal == false`: any existing network is `Compatible` regardless of
    its actual `Internal` flag (this is the case that must accept the
    user's pre-existing `Internal: true` `fluensy-learn-net`). Missing is
    still `Missing`.
- `create_internal_network` → `create_network(name, internal)`, passing
  `--internal` to `docker network create` only when `internal == true`.
- `ensure_internal_network` → `ensure_network(name, internal)` wired to the
  above.
- **The core fix**: in `run()`, change the `--network` gating from
  ```rust
  if matches!(config.isolation_strategy, Some(ContainerNetworkIsolationStrategy::DiscoverContainerIp)) {
  ```
  to unconditionally:
  ```rust
  args.push("--network".into());
  args.push(config.network_name.clone());
  ```
  (no `if` at all — `network_name` is always a real, validated value on
  `ContainerConfig` for both the primary container and every plugin; there
  is no longer a case where it should be omitted).
- Leave the `--publish` gating (`!matches!(... DiscoverContainerIp)`)
  untouched — that's an orthogonal, correct piece of logic about how the
  gateway reaches the container, not about network membership.
- Add the requested command-visibility logging (see "Logging" section
  below).

### 3. `crates/platform/src/container/macos_container.rs`

- Same `internal: bool` parameterization of
  `inspect_internal_network_state` / `create_internal_network` /
  `ensure_internal_network` → `ensure_network`.
- `run()` already attaches `--network` whenever
  `config.isolation_strategy.is_some()` — change this to unconditional,
  same as docker.rs, for consistency (today it already works for both
  isolated strategies; this just extends it to `None` too).
- Add the same command-visibility logging.

### 4. `crates/core/src/application/ensure_container.rs`

Change `ensure()` from:

```rust
if config.isolation_strategy.is_some() {
    self.port.validate_internal_network_support(config)?;
    let preparation = self.port.ensure_internal_network(&config.network_name).await?;
    match preparation { ... }
}
```

to always preparing the network, using `isolation_strategy.is_some()` only
to decide `internal` and whether the macOS+Docker guard applies:

```rust
let internal = config.isolation_strategy.is_some();
if internal {
    self.port.validate_internal_network_support(config)?;
}
let preparation = self.port.ensure_network(&config.network_name, internal).await?;
match preparation {
    NetworkPreparation::Created => tracing::info!(network = %config.network_name, internal, "created MCP network"),
    NetworkPreparation::Reused => tracing::info!(network = %config.network_name, internal, "reusing existing compatible MCP network"),
}
```

`validate_internal_network_support` keeps running only for the isolated
case, so the macOS+Docker rejection guard (the whole point of
`network_isolation: false` existing as an escape hatch) is unaffected.

### 5. Behavior change to flag explicitly for review

This also changes the **primary** vault-tools container
(`ContainerStartupConfig`) when `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=false`:
today it silently lands on the default `bridge` network; after this fix it
will create/join the named network (`brain3-mcp-net` by default, or
`B3_CONTAINER_NETWORK_NAME` override) as a plain non-internal bridge
network instead. Functionally equivalent for egress (both give full NAT'd
internet access; only `--internal` removes it), but it is a real behavior
change in *which* network the container ends up on, so call it out in the
PR/commit description rather than let it be an implicit side effect.

### 6. Logging: always print the effective `docker run` / `container run` command

Per your ask — this RCA took longer than it should have partly because
there was no single log line showing the actual command that ran. Add one,
scoped tightly so it doesn't spam the startup-poll logs (which call
`is_running`/`get_container_ip` every 200ms for up to 120s —
`crates/core/src/application/ensure_container.rs`, `DEFAULT_STARTUP_POLL_INTERVAL`).
Don't touch the general `run_command` debug log in
`crates/platform/src/container/process.rs` (that would flood logs from the
poll loop). Instead, add one targeted `tracing::info!` in exactly the two
`run()` implementations (`docker.rs`, `macos_container.rs`), immediately
before the `run_command` call, logging the fully assembled, copy-pasteable
command line:

```rust
let command_line = std::iter::once(bin)
    .chain(refs.iter().copied())
    .map(|part| if part.contains(' ') { format!("'{part}'") } else { part.to_string() })
    .collect::<Vec<_>>()
    .join(" ");
tracing::info!(container = %config.name, command = %command_line, "launching container");
```

This fires once per container start (not per poll tick), at `info` (visible
by default, matching AGENTS.MD's "verbose logging, no black boxes"
guidance), and gives exactly the copy-pasteable line shown in the "Exact CLI
commands" section above — so next time this happens, the log itself answers
"what network did it actually try to join" without a manual RCA.

## Tests

- `crates/platform/src/container/docker.rs`: extend the existing
  `#[cfg(test)]` module — a test asserting `run()`'s built args include
  `--network <name>` when `isolation_strategy: None`, alongside the existing
  `rejects_all_internal_network_strategies_on_macos` /
  `accepts_publish_to_loopback_internal_network`-style tests. Also a test
  for `ensure_network(name, internal: false)` treating an existing
  `Internal: true` network as `Compatible` (the exact shape of the user's
  `fluensy-learn-net`).
- `crates/platform/src/container/macos_container.rs`: mirror the same
  `--network`-present-when-`None` assertion.
- `crates/core/src/application/ensure_container.rs`: extend the existing
  mock-based test suite (`MockNetworkResult`, `ensure_internal_network_count`
  etc.) — rename/generalize the mock's tracked call to `ensure_network`, add
  a case asserting it's called (with `internal: false`) even when
  `isolation_strategy` is `None`, where today's
  `non_isolated_container_skips_internal_network_validation` test asserts
  the *opposite* (0 calls) — that test's name and assertion need to change
  to reflect the new contract.
- `crates/platform/src/container/startup.rs`: no change expected —
  `build_plugin_container_config` already sets `network_name` unconditionally;
  this plan doesn't touch that function.

## Verification

```bash
cargo test -p brain3 --no-run
cargo test
```

Manual, on this machine:

1. Apply the fix, restart brain3.
2. `docker network inspect fluensy-learn-net` should now list `fluensy_learn`
   as an attached container.
3. Confirm the new info log shows the `docker run ... --network
   fluensy-learn-net ...` command line.
4. Confirm `fluensy_learn`'s tools work end-to-end through the gateway and
   that its onboarding/save/practice tools can actually reach `postgrest`
   (check `fluensy_learn`'s own container logs for the postgrest calls
   succeeding, not just MCP `tools/list`).

## Completion criteria

- `network_isolation: false` plugins join their configured `network`
  (created as a plain non-`--internal` network if missing, reused as-is if
  it already exists) instead of silently landing on the default bridge.
- Isolated plugins (`network_isolation: true`, both `DiscoverContainerIp`
  and `PublishToLoopback` strategies) are unaffected — same `--internal`
  network semantics as today.
- The primary container's `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=false`
  path gets the same fix, documented as a called-out behavior change.
- A single `info`-level log line shows the exact `docker run` /
  `container run` command for every container start.
- `cargo test -p brain3 --no-run` and `cargo test` pass.
