# Counter-plan: macOS Docker Plugin MCP internal-network reachability

## Status

- Date: 2026-07-12
- Responds to: `docs/superpowers/plans/2026-07-12-macos-docker-plugin-internal-network-rca.md`
- This is a review-driven counter-plan. No code has been changed. It confirms
  parts of the original RCA against the actual source, disputes how it frames
  the root cause, and narrows what must be verified before picking a
  remediation option.

## Why this document exists

The original RCA concludes that `fluensy_learn` ended up on Docker's default
bridge network instead of `fluensy-learn-net`, and explains this as basically
an unavoidable consequence of the macOS Docker Desktop VM boundary: the code
"chooses the loopback-published-port strategy... but the adapter only passes
`--network` for the other strategy." That phrasing reads like a tradeoff
inherent to the chosen strategy. Reading the actual code and the user's own
running system shows it is not a tradeoff — it is a plain implementation bug
in one adapter, and the RCA's harder empirical claim (that Docker cannot
publish a port for a container on an internal-only network) has not actually
been isolated and confirmed. This document verifies what can be verified from
the source and the local system, and lays out a tighter path to a decision.

## What the original RCA got right (verified against source)

1. **The Docker container really does skip `--network` for the strategy
   macOS uses.** `crates/platform/src/container/docker.rs:270-276` only adds
   `--network <name>` when `isolation_strategy == DiscoverContainerIp`.
   `crates/platform/src/container/startup.rs:407-414` selects
   `PublishToLoopback` for `ContainerRuntime::Docker` on macOS. So on macOS
   Docker, `--network` is never emitted, and Docker attaches the container to
   the default bridge. This is exactly what was observed for
   `fluensy_learn`, and it is deterministic, not intermittent.
2. **`ensure_internal_network` is called regardless of which strategy is
   selected.** `crates/core/src/application/ensure_container.rs:86-90` calls
   it whenever `isolation_strategy.is_some()`, which is why the logs said the
   network was "reused" even though the container never joined it. The log
   line proves the network exists; it does not prove membership.

## Where the original RCA's framing is wrong or unverified

### 1. This is a contract violation, not a design tradeoff

`crates/core/src/domain/model.rs:100-103` documents the `PublishToLoopback`
variant's intended behavior directly in the type:

```text
/// Container joins the internal network; `--publish` **is** added to bind
/// the host loopback port. The gateway reaches the container via
/// `127.0.0.1:host_port` as normal. Default for macOS native containers.
PublishToLoopback,
```

The Docker adapter does not implement this contract: it never joins the
network under `PublishToLoopback`. The macOS-native `container` adapter,
`crates/platform/src/container/macos_container.rs:360-363`, implements the
contract correctly — it attaches `--network` whenever
`isolation_strategy.is_some()`, for both strategies.

Framing this as "the current implementation chooses strategy X, but the
adapter only supports Y" undersells the defect. It is a straightforward bug:
one adapter diverges from its own type's documented contract, and a sibling
adapter proves the contract is implementable. The fix for this half of the
problem is unambiguous and does not depend on any further Docker Desktop
research: make `docker.rs` attach `--network` under the same condition
`macos_container.rs` already uses.

### 2. The harder claim — internal network + published port never works on
   Docker — rests on one manual, narratively described test, not a scripted,
   reproducible one

The RCA's real crux is this sentence: "a container whose only network is an
internal Docker network does not receive a usable host port publication."
Everything past that point (Option B vs. Option D, a relay container, a
threat-model rewrite) is downstream of this one claim. As written, the RCA
supports it with a single manual session's narrative and partial `docker
inspect` output, not a minimal, independently repeatable script with captured
output. Before committing to the heavier remediation (a relay container is a
new piece of security-sensitive infrastructure — Option D — or a policy
change to allow plugin egress — Option B), this claim needs to be nailed down
with something like:

```bash
docker network create --internal test-net
docker run -d --name t --network test-net -p 127.0.0.1:9999:80 nginx
docker inspect t --format '{{json .NetworkSettings.Ports}}'
curl -m 2 http://127.0.0.1:9999
docker rm -f t; docker network rm test-net
```

If the port binding never appears and curl fails, that confirms the
constraint cleanly and cheaply, and it should be cited as a specific,
reproducible Docker behavior — not an anecdote — in any future revision of
the RCA. If it doesn't reproduce, the entire justification for Option B/D
collapses and the fix is limited to point 1 above.

### 3. The user's own working counterexample looked like a contradiction, but
   isn't — it's a different runtime, not evidence against the Docker claim

The user's objection to this specific document
(`docs/superpowers/plans/2026-07-12-macos-docker-plugin-internal-network-rca.md#L35`)
was: "we already have an mcp vault container on its own net, and the host can
reach it — so how does joining the network remove the host-to-plugin path?"

Checked directly against the local system:

- `~/.brain3/.env` sets `B3_CONTAINER_RUNTIME="macos-container"` and
  `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION="true"`.
- The vault-tools container's network only exists under
  `~/Library/Application Support/com.apple.container/networks/brain3-mcp-net`
  — Apple's native `container` tool's network store. `docker network ls` on
  this machine has no `brain3-mcp-net` at all.
- `crates/platform/src/config/env_file.rs:505-515` actively rejects
  `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=true` when the runtime is Docker on
  macOS: *"not supported with B3_CONTAINER_RUNTIME=docker on macos... set
  B3_CONTAINER_RUNTIME=macos-container or
  B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=false."*

So it is currently impossible to even configure the vault-tools container
with internal-network isolation on Docker+macOS. The user's working example is
guaranteed to be running on the native `container` runtime, whose network
implementation (vmnet-based hostOnly networking) is architecturally unrelated
to Docker Desktop's Linux-VM bridge/iptables NAT. It also runs through the
adapter that correctly implements the `PublishToLoopback` contract (point 1
above), so of course it works end-to-end. This is fully consistent with the
original RCA's own "Platform Coverage" section, which already flags: *"macOS
native `container`: verify unchanged behavior, but do not assume it can share
a Docker network."* The RCA just never connected that caveat back to explain
why the user's own working instance doesn't disprove the Docker-specific
claim — worth stating explicitly so the counterexample doesn't get
re-litigated later.

## Counter-plan

### Phase 0: fail fast instead of silently misconfiguring (do this immediately)

Today, the primary vault-tools container and plugin MCP containers are
guarded inconsistently:

- The **primary** container already has a guard.
  `crates/platform/src/config/env_file.rs:505-515`
  (`validate_network_isolation_support`) rejects
  `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=true` when
  `B3_CONTAINER_RUNTIME=docker` on macOS, with a clear error message pointing
  the user at `macos-container` or disabling isolation.
- **Plugin** MCP containers have no equivalent guard. `brain3_yaml.rs` has no
  reference to any isolation toggle at all, and
  `crates/platform/src/container/startup.rs:407-414`
  (`plugin_isolation_strategy`) unconditionally selects `PublishToLoopback`
  for every Docker plugin, on every OS, with no opt-out. On macOS this is
  exactly the combination that silently produces the broken behavior in the
  original RCA: the plugin starts, looks briefly ready, and only fails later
  when the gateway can't complete MCP initialize. There is currently no way
  to configure a plugin such that this check would even run.

This is worth fixing before anything else, independent of which remediation
option (B or D) is eventually chosen for Docker+macOS+internal-network
reachability: refusing to start with a clear error is strictly better than
the current silent fallback to the default bridge network, no matter how
that deeper problem eventually gets resolved.

Concrete steps:

1. Extend (or add a sibling to) `validate_network_isolation_support` so it
   also runs for every configured plugin MCP container, not just the primary
   container. The natural place is at config-load time in
   `crates/platform/src/config/brain3_yaml.rs`, alongside the existing
   `network` field validation, so the failure happens at startup/config-parse
   time — before any container launch is attempted — rather than being
   discovered mid-launch.
2. Guard condition: reject any plugin entry where `runtime: docker` (or the
   config default resolves to Docker) while `cfg(target_os = "macos")`, since
   every plugin unconditionally requires a `network` and therefore
   unconditionally requests network isolation — there is no "isolation
   disabled" case to distinguish for plugins the way there is for the primary
   container.
3. Error message should name the specific plugin, state that
   `runtime: docker` is not supported for network-isolated MCP plugin
   containers on macOS, and point at `runtime: macos-container` as the
   supported alternative for that plugin — mirroring the wording and
   specificity of the existing primary-container error.
4. Add a unit test mirroring
   `load_rejects_internal_network_isolation_for_docker_on_macos` in
   `env_file.rs`, but for plugin config parsing: a plugin with
   `runtime: docker` on a `cfg(target_os = "macos")` build should fail to
   load with a clear message; the same plugin with
   `runtime: macos-container` should load cleanly.
5. This is a guard, not a new ingress or capability, so it does not require a
   `SECURITY_AUDIT.MD` threat-model update — it only makes an existing failure
   mode loud instead of silent.

### Phase 1: fix the `docker.rs` contract violation

Independent of Phase 0, change the `--network` gating in
`crates/platform/src/container/docker.rs` from "only for
`DiscoverContainerIp`" to "whenever `isolation_strategy.is_some()`," matching
`macos_container.rs` and the documented contract in
`crates/core/src/domain/model.rs`. This fixes plugin-to-sidecar DNS
(`fluensy_learn` resolving `postgrest`) on Linux Docker today, and is a
prerequisite for testing the macOS Docker case in Phase 2 once Phase 0's
guard is lifted or bypassed for that experiment.

### Phase 2: verify the unresolved empirical claim

Run the minimal scripted repro below on the actual Docker Desktop version in
use, with output captured verbatim, before spending any effort on Option B or
Option D:

```bash
docker network create --internal test-net
docker run -d --name t --network test-net -p 127.0.0.1:9999:80 nginx
docker inspect t --format '{{json .NetworkSettings.Ports}}'
curl -m 2 http://127.0.0.1:9999
docker rm -f t; docker network rm test-net
```

This is roughly five minutes and settles the one unverified claim the whole
remediation path depends on.

### Phase 3: branch on the result

- If the published port does bind: no relay, no threat-model change is
  needed. Lift the Phase 0 guard for Docker+macOS (or narrow it to only the
  cases still known to be broken), add a regression test asserting the real
  `docker run` arguments include `--network` for both strategies, add the
  MCP-initialize readiness check the original RCA proposes in its Phase 4,
  and close this out.
- If the published port does not bind: the original RCA's Option B vs.
  Option D framing is legitimate and the two-path problem is real. That is a
  security-policy decision for the user (accept plugin egress vs. build a
  constrained relay), not something to route around in code, and it should
  proceed through the original RCA's Phase 1-5 plan with the now-cited,
  reproducible evidence backing it. The Phase 0 guard stays in place until
  whichever option is implemented and verified.

### Phase 4: confirm the native `container` runtime for the right reasons

Separately confirm whether the native `container` runtime needs the same
scrutiny. It doesn't share the Docker adapter's bug, but nothing has
independently verified that its "publish + internal network" combination
works for reasons other than "it apparently works today for the vault-tools
container." Understanding *why* Apple's `container` tool can do this while
Docker Desktop apparently cannot (if the repro in Phase 2 confirms that) is
worth a short note for future maintainers, even if it doesn't change any
code.

## Bottom line

The original RCA's diagnosis of *why* `fluensy_learn` landed on the default
bridge is correct and independently verifiable — it is a real, deterministic
bug in `docker.rs`, not a mystery. But the RCA frames that bug as inseparable
from a deeper, harder-to-fix Docker Desktop networking limitation, and that
harder claim is not yet backed by a reproducible test. Fix the verified bug
first, verify the unverified claim second, and only then decide between
Option B and Option D — don't let an unverified claim drive a threat-model
change or new relay infrastructure before it's actually confirmed.
