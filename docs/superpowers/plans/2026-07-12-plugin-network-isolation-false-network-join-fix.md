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

---

## Addendum (2026-07-12, after landing): CI RCA — `e2e_smoke_5_plugin_mcp_container`
## shutdown-residue failure, plus a new E2E hardening phase

### Status

This addendum documents a CI failure discovered on this branch's first real
run of `e2e_smoke_5_plugin_mcp_container` (the only Plugin MCP Container
test), after the above fix landed. It is a plan only — nothing in this
addendum has been implemented yet, pending review.

### Symptom

```
2026-07-12T10:37:14.915174Z  INFO brain3: Received shutdown signal, draining connections...
2026-07-12T10:37:14.945678Z  INFO brain3_platform::container::startup: stopping managed Plugin MCP Container during shutdown container=hello_mcp runtime=Docker
2026-07-12T10:37:14.945781Z DEBUG brain3_platform::container::process: running container command command=docker stop hello_mcp
2026-07-12T10:37:39.970582Z Error: "managed MCP container residue remained after shutdown: brain3-mcp-vault-tools"
FAILED
    e2e_smoke_5_plugin_mcp_container
```

Reproduced identically on 3 consecutive CI runs (both the "Docker / Linux"
and "Cloudflare quick tunnel / Linux" jobs), always the same test, always
the same container left behind (`brain3-mcp-vault-tools`, never
`hello_mcp`), always ~25 seconds after `docker stop hello_mcp` is invoked.
This is the branch's first CI run that actually exercises
`e2e_smoke_5_plugin_mcp_container` — GH Actions shows the E2E workflow was
`skipped` (not run) on the two prior PRs that touched Plugin MCP Containers
(`extra_containers` #162, and this branch's earlier pushes), so this is a
latent bug being caught for the first time, not a regression introduced by
the network-join fix above.

### Root cause

Three independent facts compound into this failure:

1. **The `hello_mcp` E2E test fixture ignores `SIGTERM`.**
   `testdata/e2e_hello_mcp_container/server.py` runs
   `ThreadingHTTPServer(...).serve_forever()` directly as the container's
   `CMD`, i.e. as PID 1 inside its own PID namespace, with no signal
   handler installed. On Linux, a process running as PID 1 in a PID
   namespace does **not** get the default disposition for signals it
   hasn't explicitly handled — `SIGTERM` is silently ignored. `docker stop`
   sends `SIGTERM`, waits its default grace period (10s), then sends
   `SIGKILL`. So `docker stop hello_mcp` reliably takes the full ~10
   seconds, every time, regardless of anything in brain3 itself.

2. **`RuntimeBootstrap::shutdown_managed_runtime` stops containers
   sequentially, plugins before the primary** (`crates/platform/src/runtime/bootstrap.rs:82-115`):
   ```rust
   for plugin in &self.plugin_mcp_containers {
       stop_plugin_mcp_container(&plugin.config).await ...  // docker stop hello_mcp: ~10s
   }
   // primary container stop only starts after every plugin above has finished
   stop_mcp_container(startup).await ...
   ```
   Because this `.await`s each plugin stop to completion before moving on,
   one slow plugin fully starves every container after it in the list —
   here, the one and only primary container, `brain3-mcp-vault-tools`.

3. **The E2E test harness gives the whole graceful-shutdown sequence only
   10 seconds before escalating to `SIGKILL`**
   (`apps/gateway/tests/e2e_smoke.rs:477-499`, `Drop for Brain3Process`):
   ```rust
   let _ = Command::new("kill").arg("-INT").arg(&pid).status();
   let deadline = Instant::now() + Duration::from_secs(10);
   while Instant::now() < deadline { ... }
   let _ = self.child.kill();  // SIGKILL if still alive after 10s
   ```
   10 seconds is exactly consumed by step 1 alone (`docker stop hello_mcp`),
   leaving **zero** time for the primary container's own `docker stop
   brain3-mcp-vault-tools`, which hasn't even started yet. The whole brain3
   process gets `SIGKILL`'d mid-`await`, before the `stop_mcp_container`
   call for the primary container is ever reached.

The already-spawned `docker stop hello_mcp` child process is not killed
when its parent (`brain3`) is `SIGKILL`'d — orphaned child processes keep
running independently — so it finishes on its own a few hundred ms later
and `hello_mcp` is gone by the time the residue check looks. `brain3-mcp-vault-tools`
was never asked to stop at all, so it's the only one left. The arithmetic
matches exactly: 10s (test harness budget, consumed by `hello_mcp`'s stop)
+ 15s (`assert_no_container_residue`'s own retry window,
`apps/gateway/tests/e2e_smoke.rs:1599-1637`) ≈ 25s, the observed gap on all
3 CI runs.

This is a real bug independent of the network-join fix above — any plugin
container image that doesn't promptly exit on `SIGTERM` (common for naive
`CMD`s that run the app directly as PID 1, which is the normal/default way
most Dockerfiles are written) will starve the primary container's shutdown
today, in production as well as in this test.

### Fix plan

1. **`crates/platform/src/container/docker.rs` — `build_run_args`**: add
   `--init` to every `docker run` invocation (primary and plugin
   containers alike). This puts a minimal init process (`tini`, bundled
   with Docker) at PID 1 instead of the app itself; `tini` correctly
   forwards `SIGTERM`/`SIGINT` to the real child and reaps it as soon as it
   exits, so any app that would normally die on `SIGTERM` (the vast
   majority — Python's default disposition included) now actually does,
   even when the Dockerfile's `CMD` doesn't set up signal handling itself.
   This is the general-purpose fix: it protects against *any* misbehaving
   plugin image, not just the `hello_mcp` test fixture.
2. **`crates/platform/src/container/docker.rs` — `stop()`**: bound the
   worst case explicitly regardless of `--init`, by passing a shorter grace
   period: `docker stop --time 5 <name>` (default is 10s). Defense in depth
   for images that still don't exit promptly even under `tini`.
3. **`crates/platform/src/runtime/bootstrap.rs` —
   `shutdown_managed_runtime`**: stop the primary container and all plugin
   containers concurrently (e.g. `futures::future::join_all` over one
   future per container) instead of sequentially, so total shutdown time is
   `max()` across containers rather than `sum()`. This is optional given
   fixes 1–2 should make every individual stop fast, but it removes the
   "N containers, one slow one, all after it starve" failure shape
   entirely, and is a real consideration for anyone running more than one
   plugin.
4. **`testdata/e2e_hello_mcp_container/server.py`**: add a minimal
   `signal.signal(signal.SIGTERM, ...)` handler that calls
   `server.shutdown()` / exits immediately. Belt-and-suspenders for the
   test fixture specifically, independent of whether `--init` is used —
   keeps this one test fast even if `--init` regresses.
5. **`apps/gateway/tests/e2e_smoke.rs` — `Drop for Brain3Process`**: bump
   the SIGINT-to-SIGKILL grace window from 10s to something with real
   headroom over the fixed costs above (e.g. 20s), so a legitimate,
   bounded-by-design shutdown sequence is never truncated by the test
   harness itself. This is a safety margin, not a substitute for 1–4.
6. Apply the `macos_container.rs` equivalents of 1–2 if the native
   `container` CLI supports comparable flags (needs checking against
   `container run --help` / `container stop --help` on macOS — not
   confirmed here, flag as open question rather than guessing).

### New E2E test phase: shutdown-latency contract

Today the only signal that shutdown "worked" is
`assert_no_container_residue()` — a blind 15-second retry loop with no
timing assertion, called after `Brain3Process`'s `Drop` already ran. That's
exactly why this bug went unnoticed in test authoring: the test only checks
the *end state* after an unrelated, hardcoded grace period, not that
shutdown actually completes promptly. Add a new phase:

- Extend `e2e_smoke_5_plugin_mcp_container` (the only test with a Plugin
  MCP Container, so the only one that can exercise multi-container
  shutdown ordering) to assert shutdown finishes well inside a fixed SLA
  after the shutdown signal is sent — e.g. record `Instant::now()`
  immediately before dropping/signaling `gateway`, and assert both
  containers (`CONTAINER_NAME` and `HELLO_MCP_CONTAINER_NAME`) are gone
  within, say, 8 seconds — tight enough to fail immediately if any
  container falls back to a full `docker stop` grace-period wait, instead
  of silently passing within the current generous 10s (harness) + 15s
  (residue poll) ≈ 25s budget.
- This requires restructuring `Brain3Process`'s shutdown slightly so the
  test can signal-and-measure rather than only signal-inside-`Drop`: e.g.
  expose an explicit `async fn shutdown_and_wait(self) -> Duration` on
  `Brain3Process` that sends the signal, polls `try_wait()`, and returns
  elapsed time, called explicitly at the end of the test body instead of
  relying on scope-exit `Drop`. Keep `Drop` itself as the fallback safety
  net for tests that don't call this explicitly (unchanged behavior for
  every other test).
- Assert on the returned `Duration` directly (e.g. `< Duration::from_secs(8)`)
  so a future regression that reintroduces slow/sequential shutdown fails
  with a clear "shutdown took Xs, expected < 8s" message instead of the
  current confusing "residue remained" error that doesn't explain *why*.
- Keep `assert_no_container_residue()` as-is as a final belt-and-suspenders
  check after the latency assertion.

### Tests

- `crates/platform/src/container/docker.rs`: extend `build_run_args` unit
  tests to assert `--init` is present in the constructed args for both
  isolated and non-isolated configs; extend `stop()`'s (currently
  untested, since it's a thin wrapper) to assert the `--time 5` arg via a
  small args-builder helper if `stop()` gains one (mirroring how `run()`
  already separates `build_run_args` from the `run_command` call for
  testability).
- `crates/platform/src/runtime/bootstrap.rs`: extend the existing
  mock-based shutdown tests (if any exist — check first) or add one
  asserting plugin and primary container stops are issued concurrently,
  not sequentially, e.g. via a mock port that records call *start* order
  vs. call *completion* order and asserts they can interleave.
- `apps/gateway/tests/e2e_smoke.rs`: the new shutdown-latency assertion
  described above in `e2e_smoke_5_plugin_mcp_container`.

### Verification

```bash
cargo test -p brain3 --no-run
cargo test
```

On CI (this is the only environment where this can be verified for real,
per this repo's policy of never running Linux containers locally):
`e2e_smoke_5_plugin_mcp_container` passes and completes shutdown in well
under the old ~25s failure window — check the new latency assertion's
logged duration to confirm it's closer to 1-2s than 10s+.

### Completion criteria (addendum)

- `docker run` includes `--init` for every brain3-managed container
  (primary and plugin).
- `docker stop` uses a bounded `--time` shorter than the default 10s.
- `shutdown_managed_runtime` stops all managed containers concurrently.
- `hello_mcp` test fixture exits promptly on `SIGTERM` even without
  relying on `--init`.
- `Brain3Process`'s `Drop` grace window has real headroom over the sum of
  the above's worst case.
- A new E2E assertion fails fast (with a clear message) if shutdown ever
  regresses back to taking multiple seconds per container instead of
  running concurrently.
- `cargo test -p brain3 --no-run` and `cargo test` pass; CI's E2E workflow
  passes on all three jobs (Docker/Linux, Cloudflare quick tunnel/Linux,
  and any macOS job) for `e2e_smoke_5_plugin_mcp_container`.
