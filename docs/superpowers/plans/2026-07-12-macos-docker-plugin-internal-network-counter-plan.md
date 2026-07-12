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

1. **Fix the `docker.rs` contract violation first, independent of any other
   decision.** Change the `--network` gating in
   `crates/platform/src/container/docker.rs` from "only for
   `DiscoverContainerIp`" to "whenever `isolation_strategy.is_some()`,"
   matching `macos_container.rs` and the documented contract in
   `crates/core/src/domain/model.rs`. This alone fixes plugin-to-sidecar DNS
   (`fluensy_learn` resolving `postgrest`) regardless of what happens with
   host reachability.
2. **Run the minimal scripted repro in point 2 above on the actual Docker
   Desktop version in use, with output captured verbatim**, before spending
   any effort on Option B or Option D. This is roughly five minutes and
   settles the one unverified claim the whole remediation path depends on.
3. **Branch on the result:**
   - If the published port does bind: no relay, no threat-model change is
     needed. Add a regression test asserting the real `docker run` arguments
     include `--network` for both strategies, add the MCP-initialize
     readiness check the original RCA proposes in its Phase 4, and close this
     out.
   - If the published port does not bind: the original RCA's Option B vs.
     Option D framing is legitimate and the two-path problem is real. That is
     a security-policy decision for the user (accept plugin egress vs. build
     a constrained relay), not something to route around in code, and it
     should proceed through the original RCA's Phase 1-5 plan with the
     now-cited, reproducible evidence backing it.
4. **Separately confirm whether the native `container` runtime needs the same
   scrutiny.** It doesn't share the Docker adapter's bug, but nothing has
   independently verified that its "publish + internal network" combination
   works for reasons other than "it apparently works today for the vault-tools
   container." Understanding *why* Apple's `container` tool can do this while
   Docker Desktop apparently cannot (if the repro in step 2 confirms that) is
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
