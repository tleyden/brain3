# Extra MCP Containers (Experimental, Hidden Config)

## Goal

Let a user drop extra Docker/macOS-container MCP servers (e.g. a prototype
"fluensy_learn" container) next to the existing core vault container, purely
via a hand-edited config file. No setup-wizard UI, no docs beyond a README
"Experimental" section. Brain3 discovers them on startup, manages their
lifecycle the same way it manages the vault container, and merges their tools
into the single MCP tool list the gateway exposes.

This is a prototyping/dogfooding feature ("secret agent zoo" per the user).
Keep it best-effort and low-ceremony: if a container is misconfigured or fails
to start, log an error and continue running with whatever did come up.

## Non-goals (phase 1)

- No setup-wizard integration. No TUI screen. Config is a hand-authored YAML
  file the user places manually.
- No multiple bind mounts per container — one host directory, one container
  path.
- No OAuth between gateway and extra containers — this is an internal,
  gateway-only trust boundary (see Auth section).
- Not folding the core vault container into this same list yet. The core
  container keeps its current `.env`-driven config path. This plan only adds
  the schema and code structured so that migration is easy later, not doing
  the migration itself.
- Not moving `.env` into YAML wholesale. Just reserve the top-level shape so
  that's a additive, non-breaking change later.

## Config file

- New optional file: `<app_home>/mcp_containers.yaml` (next to the existing
  `.env`). Absence of the file is the default, normal state — nothing changes
  for existing users.
- Loaded once at gateway startup, after the core container is ensured and
  before the HTTP server starts accepting MCP traffic (tool list must be
  complete before `initialize`/`tools/list` can be served correctly).
- Parse failures or per-container startup failures are logged at `error` and
  that container is skipped. A malformed YAML file does not crash the
  gateway — it behaves as if the file were absent, but loudly logs why.

### Schema (forward-compatible root shape)

```yaml
# mcp_containers.yaml — EXPERIMENTAL, undocumented outside README "Experimental" section.
mcp_containers:
  - name: fluensy_learn
    platform: docker            # docker | macos_container
    image: ghcr.io/example/fluensy-learn
    tag: latest
    port: 8420                  # port the container's MCP server listens on inside the container
    host_directory: /Users/tleyden/fluensy-data   # single dir, mounted read-write
    auth:
      type: bearer_token        # none | bearer_token
      secret_file: /Users/tleyden/.brain3/secrets/fluensy_learn.token
```

- Root key `mcp_containers` is a list so the same file can later carry the
  core container's config too (each entry could grow an optional `role: core`
  field then). Not doing that now — just don't paint ourselves into a corner
  with a root shape that can't hold it.
- `name` must be unique, DNS/label-safe (used as the container name and
  Docker network alias — reuse whatever validation `ContainerConfig.name`
  already implies).
- `platform` reuses the existing `ContainerRuntime` enum (`Docker` |
  `MacOSContainer`) — no new runtime concept.
- `image` + `tag` are split (not one string) so we can validate/log them
  separately and because that's how most registry tooling models it.
- `port`: single container-listen port. Host port is **not** user-specified —
  the gateway auto-picks a free loopback port the same way it would for any
  ad hoc port mapping, to avoid asking the user to manage a second port
  number and to avoid collisions between multiple extra containers. (Open
  question below if you'd rather make host port explicit too.)
- `host_directory`: one path, bind-mounted read-write at a fixed, documented
  container path (proposal: `/data`). No multi-mount config in phase 1 —
  confirmed out of scope per your notes (SQLite DB + scratch markdown dir
  both fit under one root).
- `auth.type`: `none` or `bearer_token`. This mirrors what the vault
  container already does today (`B3_UPSTREAM_SHARED_SECRET` /
  `x-brain3-upstream-secret` header) — same trust model, just formalized as a
  named choice instead of being implicit. Confirmed by a quick check of
  current MCP ecosystem practice: full OAuth 2.1 is the standard only when a
  server must authenticate *third-party* clients; for a private
  gateway-to-container hop where brain3 is the only caller, a static bearer
  token is the standard, simpler pattern (e.g. Docker's own MCP gateway
  writeups, mcp-auth.dev bearer-auth docs). OAuth is explicitly not an option
  here, consistent with your instinct.
- `auth.secret_file`: path to a file on the host containing the raw bearer
  token. Brain3 mounts it **read-only** into the container at a fixed path
  (proposal: `/run/secrets/mcp_bearer_token`) and does *not* pass it as an
  env var — matches your "mount it, don't env-var it" preference and is more
  in line with how Docker/K8s secrets are conventionally delivered. The
  gateway itself reads the same file from the host to know what token to
  send as `Authorization: Bearer <token>` (or whatever header the container
  expects — see open question) when calling the container's MCP endpoint.

## Domain model changes (`crates/core/src/domain/model.rs`)

Add, alongside the existing `ContainerStartupConfig`:

```rust
pub struct ExtraMcpContainerConfig {
    pub name: String,
    pub runtime: ContainerRuntime,
    pub image: String,           // "image:tag" already joined
    pub container_port: u16,
    pub host_directory: PathBuf,
    pub auth: ExtraMcpContainerAuth,
}

pub enum ExtraMcpContainerAuth {
    None,
    BearerToken { secret_file: PathBuf },
}
```

These are intentionally *not* the same struct as `ContainerStartupConfig`
(that one carries vault-specific fields like `vault_path`,
`enable_sync_reindex_tool`). Both should build a `ContainerConfig` (the
runtime-agnostic one `ContainerPort::run` already takes) through their own
small builder function, same pattern as `build_container_config` in
`startup.rs`. Resist the urge to unify them into one generic struct now —
the vault container has enough special-cased fields that a shared struct
would just grow a pile of `Option`s no one else uses.

## Config loading (`crates/platform/src/config/`)

- New module `mcp_containers_config.rs`: parses the YAML file into
  `Vec<ExtraMcpContainerConfig>` (or an empty vec if the file doesn't exist).
- Needs a YAML crate — none is currently a dependency. `serde_yaml` is
  unmaintained upstream; recommend `serde_yml` or `serde_norway` (active
  forks) — pick one during implementation, not a phase-1 blocker either way.
- Validation at load time (best-effort, one bad entry doesn't kill the rest):
  - unique `name` across entries
  - `host_directory` exists and is a directory
  - `auth.secret_file` exists and is readable, if `bearer_token`
  - Any entry that fails validation: log `tracing::error!` with the
    container name and reason, skip it, keep going.

## Lifecycle (reuse, don't reinvent)

`ensure_mcp_container` / `stop_mcp_container` in
`crates/platform/src/container/startup.rs` already implement: image
pull-if-missing, name-conflict check, internal-network join, startup TCP
probe with timeout, managed-container labels for orphan GC, logs-on-failure.
None of that is vault-specific except the env vars and vault bind mount.

Plan: extract the runtime-agnostic parts (already mostly separated — `build_container_config`
is the only vault-specific piece) so a second, small
`build_extra_container_config(&ExtraMcpContainerConfig, installation_id)
-> ContainerConfig` function can reuse `EnsureContainerUseCase`,
`managed_container_labels`, the orphan-GC pass, and the startup TCP probe
as-is. Same `BRAIN3_MANAGED_LABEL_KEY`/`BRAIN3_ROLE_LABEL_KEY` labeling scheme,
but `role` becomes e.g. `mcp-extra` (or `mcp-extra:{name}`) instead of `mcp`,
so orphan GC and `list_managed_containers` can still tell core vs. extra
containers apart per installation.

Startup sequence in `bootstrap.rs`:
1. Ensure core container (unchanged).
2. Load `mcp_containers.yaml` (if present).
3. For each entry, `ensure_mcp_container`-equivalent call. On error: log and
   drop that entry from the "live" set — do not abort gateway startup.
4. Build the merged tool-routing table (see below) only from containers that
   are actually up.
5. Register `stop`s for all successfully-started extra containers in the
   same shutdown path that stops the core container.

## Tool aggregation / routing (the hard part)

Today `McpRouterUseCase` (`crates/core/src/application/mcp_router.rs`) only
knows two tool sources:
- **native tools** (in-process Rust, e.g. whisper transcription) — schemas
  appended to `tools/list`, `tools/call` intercepted by exact name match.
- **the proxy** — a single upstream URL (the vault container), everything
  else falls through to it.

Extra containers need a **third kind of source**: another MCP server reached
over HTTP, just like the proxy target, but there can be N of them and their
schemas must be merged into `tools/list` the same way native tool schemas
already are.

Proposed approach — generalize instead of special-casing per container:

- New port trait (or reuse `McpProxyPort` per container instance) —
  `RemoteMcpContainerClient`: does `initialize`, `tools/list`, `tools/call`
  against one container's `http://<host>:<port>/mcp`, attaching the bearer
  token header if configured. Same JSON-RPC shape the existing
  `ProxyMcpUseCase` already round-trips.
- On gateway startup (after each extra container passes its TCP-readiness
  probe), call `initialize` + `tools/list` once per container, cache the
  resulting tool schemas in memory (mirrors what `NativeMcpToolRegistry`
  already does for native tools — just fetched over HTTP instead of built
  in-process).
- **Tool name collisions**: prefix every extra-container tool name with its
  container name, e.g. `fluensy_learn__search_deck` — always, not just on
  collision. Rationale: per AGENTS.MD, the AI composing tool calls is smart
  but limited on ambiguity; a stable, predictable prefix costs nothing and
  avoids silent shadowing if two containers both expose e.g. `search`. Native
  tools and the core vault tools keep their current unprefixed names
  (unchanged behavior for existing users).
- `McpRouterUseCase::route_request` grows a third branch: on `tools/call`,
  after checking native tools, check whether the name matches
  `{container_name}__` for any live extra container; if so, forward to that
  container's client with the prefix stripped, same pattern as
  `maybe_call_native_tool`. On `tools/list`, append each live container's
  cached (prefixed) schemas the same way `append_native_tool_schemas` already
  does — this generalizes naturally to "append schemas from every registered
  tool source," so native tools and extra-container tools can likely share
  one accumulation code path.
- Tool-list caching means an extra container's tools are frozen at gateway
  startup; a container that changes its own tool set requires a gateway
  restart to pick up. Fine for a prototyping feature — flag as a known
  limitation, not solved here.

## Networking

Reuse exactly what the core container already does per platform — same
`ContainerNetworkIsolationStrategy` (`DiscoverContainerIp` for Docker,
`PublishToLoopback` for macOS containers) via the same `EnsureContainerUseCase`.
No new isolation concept needed; extra containers join the same
`brain3-mcp-net` internal network as the core container, keeping them off
the host network by default, consistent with the existing threat model.

## Required: update SECURITY_AUDIT.MD threat model

Per AGENTS.MD, any new ingress needs a threat-model update before landing.
This feature is a new ingress point: it lets a human with filesystem access
to `<app_home>/mcp_containers.yaml` cause the gateway to `docker run`
arbitrary images, mount arbitrary host directories into them, and forward
their tool output straight into the AI's context (which the AI then acts on
with the same trust as vault tools). Needs at minimum:
- A new "Assets"/"Attacker Capabilities" note: anyone who can write
  `mcp_containers.yaml` or the referenced image can execute arbitrary code
  with Docker-level access to the mounted `host_directory`.
- Explicit statement that this is opt-in, local-file-only (no remote/API way
  to add a container), and Experimental/undocumented by design in phase 1.
- Note that extra-container tool output is not sandboxed or vetted before
  being appended to `tools/list` / returned from `tools/call` — same trust
  level as the vault container's tools today.

## Open questions for you (before implementation starts)

1. **Host port**: auto-pick a free loopback port (proposed above) vs. let the
   user pin it in YAML like they do for the vault container's `host_port`?
   Auto-pick is simpler and avoids collisions across multiple extra
   containers, but is less predictable if you want to `curl` the container
   directly while developing it.
2. **Bearer header name**: reuse the existing convention
   (`x-brain3-upstream-secret`) so extra containers can share code/tooling
   with the vault container, or use the more standard `Authorization: Bearer
   <token>` since these are arbitrary third-party-ish MCP servers (e.g. your
   own fluensy_learn one) that might expect the conventional header if you
   ever run them outside brain3 too?
3. **Container path for the mounted directory and secret file**: proposed
   `/data` and `/run/secrets/mcp_bearer_token` — fine as fixed conventions,
   or do you want these configurable per container too?
4. **YAML crate choice**: `serde_yml` vs `serde_norway` vs something else —
   no strong preference from the codebase today, pick at implementation
   time.
5. **Role label value** for orphan-GC scoping (`mcp-extra` vs
   `mcp-extra:{name}`) — affects how `list_managed_containers` filters extra
   containers from the core one; needs a concrete value before
   `managed_container_labels`-equivalent code is written.

## File/module summary

- `crates/core/src/domain/model.rs` — add `ExtraMcpContainerConfig`,
  `ExtraMcpContainerAuth`.
- `crates/platform/src/config/mcp_containers_config.rs` — new, YAML loader +
  validation.
- `crates/platform/src/container/startup.rs` — add
  `build_extra_container_config`, `ensure_extra_mcp_container`,
  `stop_extra_mcp_container`, extend orphan-GC role scoping.
- `crates/core/src/application/` — new `remote_mcp_container_client.rs`
  (port trait) or extend `mcp_proxy.rs`; extend `mcp_router.rs` to route
  through a list of extra-container tool sources alongside native tools.
- `crates/platform/src/runtime/bootstrap.rs` — load config, ensure extra
  containers, wire their clients into the router, register shutdown.
- `SECURITY_AUDIT.MD` — Threat Model section update (required, see above).
- `README.md` — one short "Experimental" section pointing at the YAML file
  and schema, explicitly marked unsupported/subject to change.

No setup wizard, TUI, or `.env`-migration work in this phase.
