# RCA and remediation plan: macOS Docker Plugin MCP internal-network reachability

## Status

- Date: 2026-07-12
- Scope: Plugin MCP Containers using Docker on macOS
- Reproduced with: `fluensy_learn` plus Docker Compose Postgres/PostgREST sidecars
- User-visible result: the plugin container is briefly reported ready, exits, and
  its tools are omitted after the gateway exhausts initialize/tools-list retries
- This document is an RCA and decision plan. It does not implement a fix.

## Executive summary

The failure is caused by two coupled networking requirements that the current
macOS Docker path cannot satisfy with one internal-only network:

1. `fluensy_learn` must join `fluensy-learn-net` so Docker DNS can resolve the
   `postgrest` service and the plugin can call `http://postgrest:3000`.
2. The Brain3 gateway runs on the macOS host and currently reaches Docker plugin
   containers through a port published on `127.0.0.1`.

The current implementation chooses the loopback-published-port strategy for
Docker on macOS, but the Docker adapter only passes `--network` for the other
strategy, container-IP discovery. Therefore the managed `fluensy_learn`
container does not join its configured `fluensy-learn-net`; it joins Docker's
default bridge instead and cannot resolve `postgrest`.

Adding `--network fluensy-learn-net` is necessary for sidecar DNS, but it is not
a complete fix. In the locally reproduced Docker Desktop behavior, a container
whose only network is an internal Docker network does not receive a usable host
port publication. The plugin then runs and reaches PostgREST, but the host
gateway cannot reach its MCP endpoint through `127.0.0.1:<host-port>`.

So the answer to "why can't `fluensy_learn` just join `fluensy-learn-net`?" is:
it can, and doing so fixes plugin-to-sidecar traffic. It simultaneously removes
the host-to-plugin path on which the current macOS Docker integration depends.
The whole bug is the missing end-to-end networking design for satisfying both
directions, not only the omitted `--network` argument.

## Terms

### Container network

A container network is a virtual Layer 2/Layer 3 network managed by Docker.
Containers attached to the same user-defined network can communicate using
container ports and Docker's embedded DNS. A container can join more than one
network.

### User-defined bridge network

A user-defined bridge is a Docker network created separately from Docker's
legacy default `bridge`. It normally provides:

- DNS resolution by service name or network alias;
- direct container-to-container connectivity among its members;
- network separation from containers that are not members.

`fluensy-learn-net` and `local-supabase-host-access` are user-defined Docker
networks.

### Internal network

An internal Docker network is created with `docker network create --internal`.
It is intended for communication among containers on that network without a
normal route to external networks. In this deployment:

- `fluensy-learn-net` has `Internal=true`;
- `db` and `postgrest` can communicate with each other on it;
- another container that joins it can resolve `postgrest` through Docker DNS;
- containers outside it cannot resolve or directly address its members merely
  because they know their names;
- a container whose only network is internal has no normal outbound internet
  route.

"Internal" does not mean that the traffic inside the network is encrypted or
authenticated. It means the network is isolated from normal external routing.
Every member of that network is still trusted to reach the other members unless
the applications enforce their own authentication.

### Non-internal network

A non-internal Docker bridge has `Internal=false`. It normally gives attached
containers a default route and NAT-based outbound access through Docker. It can
also support Docker Desktop's host port publication path.

Non-internal does not automatically mean "publicly exposed." Exposure depends
on separate controls such as `--publish`, the address used for the published
port, host firewall rules, and which other containers join the network. However,
for an untrusted plugin, non-internal materially weakens containment because the
plugin generally gains outbound network access.

### Container port

A container port is where a process listens inside a container. Fluensy listens
on container port `3000`. PostgREST also listens on its own container port
`3000`. These identical port numbers are not a conflict because they are in
different network namespaces.

### Published host port

Docker's `--publish 127.0.0.1:<host-port>:3000` asks Docker Desktop to expose a
container's port `3000` through a macOS loopback port. The bind address
`127.0.0.1` limits that ingress to local host processes; it is not the same as
publishing on `0.0.0.0`, which can expose the port on LAN-facing interfaces.

### Docker embedded DNS and service names

On a user-defined network, Docker resolves a Compose service name such as
`postgrest` to that service container's IP on the shared network. Therefore the
correct in-network URL is `http://postgrest:3000`. Host port `3579` is not used
for this path; it is a separate host-to-container publication.

### Docker Desktop VM boundary

On macOS, Linux Docker containers run behind Docker Desktop's Linux VM rather
than directly in the macOS host network namespace. A container IP is not assumed
to be directly routable from the macOS host. That is why the current code selects
loopback port publication on macOS instead of discovering and dialing the
container IP as it does on Linux.

## Intended traffic paths

There are two independent traffic paths:

```text
Plugin-to-sidecar:
fluensy_learn -- Docker DNS: postgrest:3000 --> PostgREST --> db:5432

Gateway-to-plugin:
macOS gateway -- 127.0.0.1:<published-port> --> fluensy_learn:3000/mcp
```

The first path requires shared network membership. The second path requires a
host-reachable transport. A design is incomplete if it tests only one path.

## Expected architecture

The intended configuration says:

```yaml
plugin_mcp_containers:
  - name: fluensy_learn
    platform: docker
    network: fluensy-learn-net
```

The expected runtime result is that Docker inspection lists
`fluensy-learn-net` under `fluensy_learn.NetworkSettings.Networks`, and that the
gateway has a supported route to the plugin's MCP endpoint.

## Actual architecture

### 1. Configuration is parsed correctly

`crates/platform/src/config/brain3_yaml.rs:120-132` requires `network`, validates
it, and stores it as `PluginMcpContainerConfig.network_name`.

### 2. The value reaches `ContainerConfig`

`crates/platform/src/container/startup.rs:370-392` logs the network and copies it
to `ContainerConfig.network_name`. The regression test proves this value differs
for plugins configured with different network names.

### 3. macOS Docker selects `PublishToLoopback`

`crates/platform/src/container/startup.rs:407-414` selects:

```rust
ContainerRuntime::Docker => ContainerNetworkIsolationStrategy::PublishToLoopback
```

when compiled on macOS. The reason is the Docker Desktop VM boundary: the host
gateway is expected to reach the plugin through a loopback-published port rather
than through a Linux container IP.

### 4. The Docker adapter does not attach that strategy to the network

`crates/platform/src/container/docker.rs:234-245` publishes ports for every
strategy except `DiscoverContainerIp`.

`crates/platform/src/container/docker.rs:270-276` adds `--network <name>` only
when the strategy is exactly `DiscoverContainerIp`.

Therefore `PublishToLoopback` produces a command equivalent to:

```text
docker run --publish 127.0.0.1:<host-port>:3000 ... fluensy-learn-mcp:latest
```

It does not produce:

```text
docker run --network fluensy-learn-net ...
```

Docker consequently attaches `fluensy_learn` to its default bridge.

### 5. An internal network is created or validated but not used

`crates/core/src/application/ensure_container.rs:86-99` calls
`ensure_internal_network(network_name)` whenever `isolation_strategy` is set.
The log therefore says that `fluensy-learn-net` was created or reused even
though the subsequent Docker command does not attach the container to it.

This produces misleading operational evidence:

```text
reusing existing compatible internal MCP network network=fluensy-learn-net
```

That line confirms only that the network exists and is compatible. It does not
confirm membership.

## Reproduction evidence

### Managed launch

The release binary logged:

```text
prepared Plugin MCP Container runtime networking configuration
  container=fluensy_learn
  network=fluensy-learn-net
  isolation_strategy=Some(PublishToLoopback)

reusing existing compatible internal MCP network network=fluensy-learn-net
```

The plugin was then reported ready, but all MCP initialize requests to the
published loopback port failed. The gateway retried for approximately 15
seconds and ended with:

```text
skipping Plugin MCP Container tools because initialize/tools-list failed
```

The Docker container used `--rm`; after the Fluensy process exited it was
automatically deleted, so it no longer appeared in `docker ps -a`.

### Image and sidecar configuration

The rebuilt image correctly contains:

```text
FLUENSY_LEARN_POSTGREST_URL=http://postgrest:3000
HOST=0.0.0.0
```

Docker inspection confirmed:

```text
fluensy-learn-net: Internal=true
members: local-supabase-db-1, local-supabase-postgrest-1
```

The Compose services also join the non-internal
`local-supabase-host-access` network so their explicit host publications work.

### Manual plugin on `fluensy-learn-net`

Running the Fluensy image manually with all relevant managed-container settings,
including the host UID/GID and data bind mount, plus:

```text
--network fluensy-learn-net
```

proved the plugin application itself is healthy:

- Docker DNS resolved `postgrest`;
- the store loaded through `http://postgrest:3000`;
- the MCP server listened on `0.0.0.0:3000`;
- all four Fluensy tools were registered;
- the process remained running.

This rules out the rebuilt image, filesystem ownership, and PostgREST itself as
the current root cause.

### Host reachability on internal-only network

The same manual run requested:

```text
--publish 127.0.0.1:59999:3000
```

while the container's only network was `fluensy-learn-net`. On this Docker
Desktop installation:

- `docker inspect` showed no effective host binding for `3000/tcp`;
- `curl http://127.0.0.1:59999/mcp` could not connect;
- the application remained healthy inside the container.

This is the reason adding `--network fluensy-learn-net` alone is not an
end-to-end fix on the tested macOS Docker environment.

### Relaxed host-path control test

A control run left Fluensy on Docker's default non-internal bridge, published
its MCP port on `127.0.0.1`, and used:

```text
FLUENSY_LEARN_POSTGREST_URL=http://host.docker.internal:3579
```

That topology succeeded:

- storage loaded;
- MCP initialize returned HTTP 200;
- all four tools were exposed.

It proves that the loopback transport works on a non-internal network, but it
does so by giving up the plugin's internal-only network isolation and routing
sidecar traffic through a host-published port.

## Root cause

### Primary code defect

The abstraction conflates two different decisions in one enum:

1. whether/how the container joins an isolation network; and
2. how the host reaches the container.

`DiscoverContainerIp` currently means both "join the configured network" and
"dial the container IP." `PublishToLoopback` currently means both "publish a
host port" and, accidentally, "do not join the configured network."

Network membership and gateway reachability are independent concerns and must
not be encoded as mutually exclusive branches.

### Platform constraint

The secure topology needs an internal-only dependency network, but the macOS
host gateway needs a host-reachable MCP transport across the Docker Desktop VM
boundary. On the tested Docker Desktop runtime, an internal-only attachment does
not provide a usable loopback-published port.

This constraint means correcting network membership alone exposes the missing
return path rather than completing the design.

### Secondary diagnostic weaknesses

These are not the networking root cause, but they made the incident confusing:

1. The logs say `network_isolated=true` and report the configured network before
   actual membership is verified.
2. The startup probe in
   `crates/core/src/application/ensure_container.rs:144-219` checks only whether
   a TCP connection can be opened. Docker's port-forwarding layer can briefly
   satisfy that probe before the application completes its own startup or exits.
3. Docker plugins use `--rm`, so a fast process exit removes the container and
   its logs before later tool-initialization failures are diagnosed.
4. Existing regression tests prove configuration propagation into
   `ContainerConfig`; they do not prove the final Docker arguments or real
   network membership for each strategy.

## Is the omitted `--network` the whole bug?

No.

It is the immediate reason `fluensy_learn` cannot resolve `postgrest` during the
managed macOS Docker launch. It is a real bug because a required, logged network
setting is not honored.

But a patch that unconditionally adds `--network fluensy-learn-net` leaves the
host gateway unable to reach the plugin on the tested Docker Desktop runtime.
The complete bug is the absence of a supported topology that provides:

- plugin-to-sidecar DNS and connectivity;
- host-gateway-to-plugin MCP connectivity;
- per-plugin separation from unrelated plugins;
- the intended outbound-access policy;
- accurate verification and diagnostics.

## Workaround options

### Option A: host-path workaround on Docker's default bridge

Keep the managed plugin on Docker's default non-internal bridge and build
Fluensy with:

```text
FLUENSY_LEARN_POSTGREST_URL=http://host.docker.internal:3579
```

Keep PostgREST published through the Compose host-access network.

Advantages:

- verified working locally;
- no Brain3 code change;
- MCP ingress remains bound to `127.0.0.1`.

Costs:

- the plugin is not on its configured per-plugin network;
- the plugin has normal outbound access;
- sidecar traffic takes a host-published path rather than private Docker DNS;
- `network: fluensy-learn-net` is operationally misleading on this path;
- any sidecar host publication must be secured separately.

This is an acceptable temporary development workaround only if the relaxed
egress policy is explicit and understood.

### Option B: one non-internal per-plugin network

Make the configured per-plugin network non-internal, attach the plugin and its
sidecars to it, and publish the MCP port to loopback.

Advantages:

- simple topology;
- Docker DNS works;
- loopback MCP publication works;
- unrelated plugins remain separated if every plugin gets a unique network.

Costs:

- plugin outbound access is enabled;
- current `ensure_internal_network` rejects non-internal networks, so this needs
  an explicit product/configuration change rather than a Compose-only change;
- changing an existing security default silently would violate the threat-model
  requirement.

If implemented, this must be an explicit opt-in mode, not an implicit macOS
fallback.

### Option C: dual-home the plugin

Attach the plugin to:

1. its internal dependency network (`fluensy-learn-net`); and
2. a dedicated non-internal transport network used to support loopback port
   publication.

Advantages:

- sidecar DNS remains on the named per-plugin network;
- unrelated plugins can remain separated by using dedicated networks;
- host loopback access can work.

Costs:

- the plugin still gains outbound access through the non-internal network;
- the model currently supports only one network name;
- Docker launch and cleanup must manage multiple networks deterministically;
- a second network does not preserve the original no-outbound guarantee.

This improves segmentation but not egress isolation.

### Option D: trusted loopback relay, plugin remains internal-only

Keep the untrusted plugin only on `fluensy-learn-net`. Add a small Brain3-managed
relay that is dual-homed:

- one side on the plugin's internal network;
- one side on a dedicated transport network with a loopback-published host port;
- a fixed forwarding rule from the host port to only the plugin MCP endpoint.

Advantages:

- the plugin itself retains no normal outbound route;
- sidecar DNS works directly;
- the host gateway receives a loopback transport;
- the trusted relay can be minimal and tightly constrained.

Costs:

- additional managed container/process and lifecycle complexity;
- new image or runtime dependency;
- more startup, logging, cleanup, and failure modes;
- the relay becomes security-sensitive infrastructure and needs threat modeling.

This is the strongest candidate if strict plugin egress isolation must be
preserved on macOS Docker.

### Option E: run the gateway inside the Docker network

Move the component that calls the plugin into Docker so it can call the plugin
directly over the internal network without host port publication.

Advantages:

- clean internal-only container networking;
- no loopback relay required.

Costs:

- major architecture and packaging change;
- conflicts with the current host-process orchestration model;
- much larger blast radius than the plugin feature warrants.

This is not recommended as a targeted fix.

## Recommended decision

Use Option A only as a documented short-term local workaround.

For the permanent product behavior, choose between:

- Option B if explicit per-plugin outbound access is acceptable and operational
  simplicity is the priority; or
- Option D if the original internal-only/no-outbound security guarantee is a
  hard requirement on macOS Docker.

Do not ship a patch that only adds `--network` for `PublishToLoopback`; it fixes
sidecar DNS while breaking the gateway transport. Do not silently convert
`fluensy-learn-net` to non-internal, because that changes the plugin threat model
and contradicts the documented security guarantee.

## Permanent remediation plan after a decision

### Phase 1: encode independent networking decisions

Refactor the container model so it separately represents:

- network attachments;
- whether each network is internal;
- gateway reachability mode;
- published ports;
- container-IP discovery where supported.

Do not use `ContainerNetworkIsolationStrategy` as a proxy for network membership.
Every configured network attachment must be rendered into runtime arguments and
verified after launch.

### Phase 2: implement the selected macOS Docker topology

For Option B:

- add an explicit YAML policy selecting an outbound-capable per-plugin network;
- create/reuse the correct network type;
- attach the plugin to it for both reachability strategies;
- continue binding MCP publication to `127.0.0.1` only.

For Option D:

- define a relay port/adapter and lifecycle contract;
- keep the plugin and sidecars only on their internal network;
- create a dedicated transport network for the relay;
- publish only the relay's MCP ingress to loopback;
- constrain forwarding to the selected plugin name and container port.

### Phase 3: make runtime state observable and verifiable

- Log the final network attachment plan before launch.
- Log the actual inspected networks after launch.
- Treat a configured-but-unattached network as startup failure.
- Include network names, internal flags, and effective published bindings in
  diagnostic output without logging secrets.
- Preserve or capture recent logs when an auto-removed plugin exits during
  initialization.

### Phase 4: improve readiness semantics

- Keep the generic TCP readiness probe for generic containers.
- Add an MCP initialization readiness stage before declaring a Plugin MCP
  Container fully ready, or clearly distinguish "TCP reachable" from "MCP
  initialized."
- Detect that the container exited between TCP readiness and tool discovery and
  report its captured logs rather than only a generic bad-gateway error.

### Phase 5: tests

Add focused tests for:

- Docker argument construction for every isolation/reachability strategy;
- configured network attachment under `PublishToLoopback`;
- rejection of configured-but-unused network plans;
- internal versus non-internal network policy;
- multiple network attachments if Option C or D is selected;
- loopback bind address remaining `127.0.0.1`;
- plugin process exit after TCP readiness but before MCP initialization;
- distinct plugins never sharing a dependency network unless explicitly
  configured with the same name.

Add a macOS Docker E2E test for both required paths:

```text
gateway -> plugin /mcp
plugin -> sidecar by Docker DNS
```

Checking only container creation, network existence, or a TCP handshake is not
sufficient.

## Security requirements before implementation

Any option that creates or uses a non-internal plugin network changes plugin
egress and must update the Threat Model section of `SECURITY_AUDIT.MD` before
code changes, as required by the repository policy.

The threat-model update must cover:

- plugin outbound internet access;
- access to host-published services;
- access to other containers on every attached network;
- DNS-based service discovery;
- relay trust and forwarding restrictions, if selected;
- whether a compromised plugin can reach unrelated plugin data;
- why loopback publication does or does not add remote ingress.

No implementation should silently weaken `--internal` semantics.

## Verification checklist

### Required local checks

1. `cargo test -p brain3 --no-run`
2. `cargo test`
3. Inspect the plugin container and assert its actual networks match config.
4. Verify the plugin resolves and calls `postgrest:3000`.
5. Verify the gateway completes MCP initialize and tools/list.
6. Verify the MCP host port binds only to `127.0.0.1` when publication is used.
7. Verify an unrelated plugin cannot resolve or connect to Fluensy's sidecars.
8. Verify the selected outbound policy: blocked for an internal-only design,
   intentionally available for an explicit relaxed design.

### Platform coverage

- macOS Docker Desktop: required for this regression.
- macOS native `container`: verify unchanged behavior, but do not assume it can
  share a Docker network.
- Linux Docker: verify container-IP discovery and internal network behavior on
  CI; do not infer Linux behavior from Docker Desktop.

## Open decision

Is internal-only/no-outbound isolation a hard security requirement for Docker
plugins on macOS?

- If yes, design and implement the trusted relay approach (Option D).
- If no, add an explicit, threat-modeled opt-in for an outbound-capable
  per-plugin network (Option B).

Until that decision is made, the verified host-path topology is a development
workaround, not a completion of the per-plugin internal-network feature.
