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

## Non-goals (all phases)

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
  that's an additive, non-breaking change later.

## Decisions locked in

1. **Host port**: optional; auto-pick a free loopback port when omitted.
2. **Auth header**: standard `Authorization: Bearer <token>` — required so
   drop-in, not-maintained-by-us MCP server images work without modification.
3. **Container mount paths**: `container_directory` (default `/data`) and
   `auth.secret_mount_path` (default `/run/secrets/mcp_bearer_token`) are
   both optionally overridable per container.
4. **YAML crate**: `serde-saphyr` (https://github.com/bourumir-wyngs/serde-saphyr)
   — strongly typed, deserialize-only, decodes straight into Rust types with
   no intermediate `Value` tree, panic-free on malformed input, no unsafe
   code. We only ever read this config, never write it back, so
   deserialize-only is sufficient.
5. **Role label value**: `brain3-mcp-extra:{name}`.

## How this is broken into phases

Each phase below is a self-contained, independently mergeable unit of work —
do them in order, and stop after any phase to check in. Nothing in an earlier
phase depends on a later one being done. Roughly:

- **Phase 1** — config schema, domain model, YAML loader. No runtime effect;
  purely parsing + validation, unit-testable in isolation.
- **Phase 2** — container lifecycle: actually `docker run`/stop extra
  containers on gateway startup/shutdown. This is the phase that introduces
  the new ingress, so it's also where the required SECURITY_AUDIT.MD update
  happens.
- **Phase 3** — tool aggregation/routing: extra containers' tools become
  visible and callable through the gateway's MCP endpoint.
- **Phase 4** — README "Experimental" section.
- **Phase 5** (final) — end-to-end test proving the whole pipeline works
  against a real (test-only) container image.

No TDD requirement for any phase — write tests where they're cheap and
valuable (config parsing is a good unit-test target; the E2E test in Phase 5
is the main correctness gate for the feature as a whole).

---

## Phase 1 — Config schema, domain model, YAML loader

### Config file

- New optional file: `<app_home>/mcp_containers.yaml` (next to the existing
  `.env`). `app_home` defaults to `~/.brain3` (overridable via `B3_HOME`), so
  in the common case this is `~/.brain3/mcp_containers.yaml`. Absence of the
  file is the default, normal state — nothing changes for existing users.
- Parse failures or per-entry validation failures are logged at `error` and
  that entry is skipped. A malformed YAML file does not crash the gateway —
  it behaves as if the file were absent, but loudly logs why.

### Schema (forward-compatible root shape)

```yaml
# mcp_containers.yaml — EXPERIMENTAL, undocumented outside README "Experimental" section.
mcp_containers:
  - name: fluensy_learn        # must be snake_case: lowercase letters, digits, underscores only
    platform: docker            # docker | macos_container
    image: ghcr.io/example/fluensy-learn
    tag: latest
    port: 8420                  # port the container's MCP server listens on inside the container

    # Host port the gateway binds on the loopback interface to reach this
    # container. Optional — if omitted, the gateway auto-picks a free port.
    # Set this only if you want a stable, predictable port for local
    # debugging (e.g. curling the container directly).
    # host_port: 18420

    host_directory: /Users/tleyden/fluensy-data   # single dir, mounted read-write

    # Container path the host_directory is mounted at. Optional — defaults
    # to /data if omitted.
    # container_directory: /data

    auth:
      type: bearer_token        # none | bearer_token
      secret_file: /Users/tleyden/.brain3/secrets/fluensy_learn.token

      # Container path the secret_file is mounted at (read-only). Optional —
      # defaults to /run/secrets/mcp_bearer_token if omitted.
      # secret_mount_path: /run/secrets/mcp_bearer_token
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
- `port`: single container-listen port (required).
- `host_port`: **optional**. If omitted, the gateway auto-picks a free
  loopback port at startup. Set it explicitly only if you want a stable port
  for local debugging.
- `host_directory`: one path, bind-mounted read-write into the container.
  No multi-mount config in phase 1 — confirmed out of scope (SQLite DB +
  scratch markdown dir both fit under one root).
- `container_directory`: **optional**, defaults to `/data` if omitted.
- `auth.type`: `none` or `bearer_token`. Mirrors the trust model the vault
  container already uses today (a shared secret presented on every call) —
  just formalized as a named choice instead of being implicit. Confirmed by a
  quick check of current MCP ecosystem practice: full OAuth 2.1 is the
  standard only when a server must authenticate *third-party* clients; for a
  private gateway-to-container hop, a static bearer token is the standard,
  simpler pattern.
- **Header**: use the standard `Authorization: Bearer <token>` header (not
  the vault container's custom `x-brain3-upstream-secret`). Decision driver:
  extra containers are meant to be drop-in, often third-party or
  not-maintained-by-us MCP servers, so the gateway must speak the header
  convention those servers already expect rather than requiring them to
  special-case brain3's internal header.
- `auth.secret_file`: path to a file on the host containing the raw bearer
  token. Brain3 mounts it **read-only** into the container (path controlled
  by `secret_mount_path`) and does *not* pass it as an env var. The gateway
  itself reads the same file from the host to know what token to send when
  calling the container's MCP endpoint.
- `auth.secret_mount_path`: **optional**, defaults to
  `/run/secrets/mcp_bearer_token` if omitted.

### Domain model (`crates/core/src/domain/model.rs`)

```rust
pub struct ExtraMcpContainerConfig {
    pub name: String,             // validated snake_case: [a-z0-9_]+
    pub runtime: ContainerRuntime,
    pub image: String,            // "image:tag" already joined
    pub container_port: u16,
    pub host_port: Option<u16>,   // None => gateway auto-picks a free loopback port
    pub host_directory: PathBuf,
    pub container_directory: PathBuf, // defaults to "/data" when not set in YAML
    pub auth: ExtraMcpContainerAuth,
}

pub enum ExtraMcpContainerAuth {
    None,
    BearerToken {
        secret_file: PathBuf,
        secret_mount_path: PathBuf, // defaults to "/run/secrets/mcp_bearer_token"
    },
}
```

`name` must pass a `[a-z0-9_]+` check at config-load time — same character
set as the Phase 3 tool-name prefix — so a container name that fails this
check is rejected with a clear error rather than silently producing a
malformed tool name later.

These are intentionally *not* the same struct as `ContainerStartupConfig`
(that one carries vault-specific fields like `vault_path`,
`enable_sync_reindex_tool`). Both should build a `ContainerConfig` (the
runtime-agnostic one `ContainerPort::run` already takes) through their own
small builder function, same pattern as `build_container_config` in
`startup.rs`. Resist the urge to unify them into one generic struct now —
the vault container has enough special-cased fields that a shared struct
would just grow a pile of `Option`s no one else uses.

### Config loading (`crates/platform/src/config/mcp_containers_config.rs`)

- New module: parses the YAML file into `Vec<ExtraMcpContainerConfig>` (or an
  empty vec if the file doesn't exist), using `serde-saphyr`.
- Validation at load time (best-effort, one bad entry doesn't kill the rest):
  - unique `name` across entries
  - `name` matches `[a-z0-9_]+`
  - `host_directory` exists and is a directory
  - `auth.secret_file` exists and is readable, if `bearer_token`
  - Any entry that fails validation: log `tracing::error!` with the
    container name and reason, skip it, keep going.

### Phase 1 exit criteria

- Unit tests cover: file absent → empty vec; valid multi-entry file; missing
  file for a `bearer_token` entry → that entry dropped, others kept;
  duplicate `name` → later duplicate dropped; bad `name` charset → dropped;
  malformed YAML → whole file treated as absent, error logged.
- No wiring into `bootstrap.rs` yet — this phase does not start any
  containers.

---

## Phase 2 — Container lifecycle

### Reuse, don't reinvent

`ensure_mcp_container` / `stop_mcp_container` in
`crates/platform/src/container/startup.rs` already implement: image
pull-if-missing, name-conflict check, internal-network join, startup TCP
probe with timeout, managed-container labels for orphan GC, logs-on-failure.
None of that is vault-specific except the env vars and vault bind mount
(isolated in `build_container_config`).

Plan: add a second, small `build_extra_container_config(&ExtraMcpContainerConfig,
installation_id) -> ContainerConfig` function that reuses
`EnsureContainerUseCase`, `managed_container_labels`, the orphan-GC pass, and
the startup TCP probe as-is. Same `BRAIN3_MANAGED_LABEL_KEY`/
`BRAIN3_ROLE_LABEL_KEY` labeling scheme, but the role label value becomes
`brain3-mcp-extra:{name}` (e.g. `brain3-mcp-extra:fluensy_learn`) instead of
`mcp`, so orphan GC and `list_managed_containers` can tell core vs. extra
containers apart, and tell different extra containers apart from each other,
per installation.

### Networking

Reuse exactly what the core container already does per platform — same
`ContainerNetworkIsolationStrategy` (`DiscoverContainerIp` for Docker,
`PublishToLoopback` for macOS containers) via the same `EnsureContainerUseCase`.
No new isolation concept needed; extra containers join the same
`brain3-mcp-net` internal network as the core container, keeping them off
the host network by default, consistent with the existing threat model.

### Startup sequence in `bootstrap.rs`

1. Ensure core container (unchanged).
2. Load `mcp_containers.yaml` (if present) via the Phase 1 loader.
3. For each entry, `ensure_mcp_container`-equivalent call. On error: log and
   drop that entry from the "live" set — do not abort gateway startup.
4. Register `stop`s for all successfully-started extra containers in the
   same shutdown path that stops the core container.

Tool routing (Phase 3) is not part of this phase — after Phase 2, extra
containers run and are reachable on their port, but the gateway does not yet
expose their tools.

### Required: update SECURITY_AUDIT.MD threat model

Per AGENTS.MD, any new ingress needs a threat-model update before landing.
This phase is what actually introduces the new ingress: it lets a human with
filesystem access to `<app_home>/mcp_containers.yaml` cause the gateway to
`docker run` arbitrary images and mount arbitrary host directories into
them. Needs at minimum:
- A new "Assets"/"Attacker Capabilities" note: anyone who can write
  `mcp_containers.yaml` or the referenced image can execute arbitrary code
  with Docker-level access to the mounted `host_directory`.
- Explicit statement that this is opt-in, local-file-only (no remote/API way
  to add a container), and Experimental/undocumented by design.
- Note (can be added now even though tool output flows in Phase 3) that
  extra-container tool output is not sandboxed or vetted before being
  appended to `tools/list` / returned from `tools/call` — same trust level as
  the vault container's tools today.

### Phase 2 exit criteria

- With a valid `mcp_containers.yaml` present, `docker ps` shows the extra
  container running alongside the core container after gateway startup, with
  the `brain3-mcp-extra:{name}` role label.
- A misconfigured/unreachable extra container logs an error and the gateway
  still starts and serves the core vault tools normally.
- Gateway shutdown stops/removes the extra container the same way it does
  the core container.
- SECURITY_AUDIT.MD updated.

---

## Phase 3 — Tool aggregation / routing

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

- One new type, `RemoteMcpContainerClient`: does `initialize`, `tools/list`,
  `tools/call` against a single container's `http://<host>:<port>/mcp`,
  attaching `Authorization: Bearer <token>` if configured. Same JSON-RPC
  shape the existing `ProxyMcpUseCase` already round-trips. This is not a
  new HTTP library or a separate client-per-request thing — it's one
  lightweight **struct**, and the gateway holds **one instance of it per
  configured extra container** (so N extra containers ⇒ N client instances,
  each pointed at its own host/port/token). Conceptually identical to how
  `ProxyMcpUseCase` already holds one instance for the core container's
  upstream URL — this is just that same shape, made instantiable per
  container instead of hardcoded to one upstream.
- On gateway startup (after each extra container passes its Phase-2
  TCP-readiness probe), call `initialize` + `tools/list` once per client
  instance, cache the resulting tool schemas in memory (mirrors what
  `NativeMcpToolRegistry` already does for native tools — just fetched over
  HTTP instead of built in-process).
- **Tool name collisions, and scope of the change**: prefix every
  extra-container tool name with its container name and a `__` separator,
  e.g. `fluensy_learn__search_deck` — always, not just on collision.
  Both the container `name` (validated `[a-z0-9_]+`, Phase 1) and MCP tool
  names are conventionally already snake_case, so `{name}__{tool_name}`
  stays entirely lowercase/underscore — no case conversion needed.
  **This prefixing applies only to extra-container tools.** Native tools
  (whisper transcription etc.) and the core vault container's tools are
  completely untouched — same unprefixed names, same code paths, zero
  behavior change for existing users. The only new code is additive: a third
  source feeding into the same merge point.
- `McpRouterUseCase::route_request` grows a third branch: on `tools/call`,
  after checking native tools (existing, unchanged), check whether the name
  starts with `{container_name}__` for any live extra container; if so,
  forward to that container's client instance with the prefix stripped, same
  pattern as `maybe_call_native_tool`. On `tools/list`, append each live
  container's cached (prefixed) schemas the same way
  `append_native_tool_schemas` already does. The native-tool accumulation
  code itself does not need to change; extra-container schemas are appended
  alongside it, not merged into it.
- Tool-list caching means an extra container's tools are frozen at gateway
  startup; a container that changes its own tool set requires a gateway
  restart to pick up. Fine for a prototyping feature — flag as a known
  limitation, not solved here.

### Phase 3 exit criteria

- With Phase 2's extra container running, an MCP client's `tools/list` call
  against the gateway includes the extra container's tools under their
  prefixed names, alongside the unchanged native and vault tool names.
- A `tools/call` for a prefixed extra-container tool name round-trips to the
  right container and returns its result.
- Existing vault/native tool behavior is provably unchanged (existing unit
  tests in `mcp_router.rs` still pass unmodified).

---

## Phase 4 — Documentation

- `README.md` — one short "Experimental" section pointing at the YAML file
  location and schema, explicitly marked unsupported/subject to change.
  No setup-wizard mention, since there isn't one for this feature.

---

## Phase 5 (final) — End-to-end test

We already have an E2E harness (`apps/gateway/tests/e2e_smoke.rs`, driven by
`scripts/e2e_smoke.py`) that builds the real vault-tools Docker image and
spawns the actual `brain3` gateway binary against a temp app-home directory,
then drives it over a real MCP client. This phase adds a new, separate E2E
test that proves the whole extra-container pipeline (Phases 1–3) end to end
against a real (test-only) container — not mocks.

Prefer a **new test function** (e.g. `e2e_smoke_5_extra_mcp_container`)
rather than folding this into `e2e_smoke_1_local_docker`, so a failure here
doesn't cloud the existing vault-tools smoke test's signal, and so it can be
skipped/run independently the same way the other numbered smoke tests can.

### What to build

1. **A minimal "hello world" test container image**, analogous to
   `brain3-mcp-vault-tools/Containerfile` but far simpler:
   - New `Containerfile` (or `Dockerfile`) under something like
     `testdata/e2e_hello_mcp_container/` — a tiny Python (or whatever's
     fastest to stand up) MCP-over-HTTP server exposing exactly one tool,
     `hello`, taking no required arguments and returning a fixed string
     (e.g. `"hello world"`).
   - It must require the same `Authorization: Bearer <token>` auth Phase 1/3
     designed for — i.e. it validates the header itself and rejects
     unauthenticated/wrong-token calls (401/403), so the test also proves
     the gateway is actually sending the configured token, not just that an
     unauthenticated call happens to work.
   - Static test fixture, not a real product — keep it as small as possible
     (no need for a real MCP SDK if a hand-rolled JSON-RPC handler for just
     `initialize`/`tools/list`/`tools/call` is simpler; match whatever's
     least code, this doesn't need to be a general-purpose server).
   - Built by the test infrastructure the same way the vault-tools image is
     built today: extend `scripts/e2e_smoke.py`'s
     `docker_build_command()`-equivalent to also build this image (a second
     `docker build -f .../Containerfile -t brain3-e2e-hello-mcp:e2e-local
     testdata/e2e_hello_mcp_container`), and add the new test name to
     `DEFAULT_E2E_TESTS`. This keeps the "build image, then run test" shape
     the harness already uses for the vault-tools container, rather than
     having the Rust test shell out to `docker build` itself mid-test.

2. **Wire it into the test's `mcp_containers.yaml`**:
   - Add a `write_mcp_containers_yaml` helper to `TempTestDir` (alongside the
     existing `write_env_file`), writing to `self.root.join("mcp_containers.yaml")`
     — same directory `B3_HOME` already points `.env` at in these tests
     (`.env("B3_HOME", &temp.root)`), consistent with the `<app_home>/...`
     convention from Phase 1.
   - Write a bearer token to a temp secret file under `temp.root` (e.g.
     `hello_mcp.token`) and reference it from the YAML's `auth.secret_file`.
   - Point `image`/`tag` at the image built in step 1, `port` at whatever the
     hello server listens on inside the container, and `host_directory` at a
     throwaway subdirectory of `temp.root` (the tool itself doesn't need to
     use it — just exercises the mount path).

3. **Test body** (`e2e_smoke_5_extra_mcp_container`):
   - `TempTestDir::create`, write `.env` (as today) *and* the new
     `mcp_containers.yaml`.
   - Spawn `Brain3Process` as usual.
   - Connect the local MCP client (reuse `connect_local_mcp`).
   - `tools/list` and assert the response includes
     `hello_mcp__hello` (or whatever the container's `name` field in the
     test YAML is) alongside the existing vault tool names — proves Phase 3
     merging works, not just that the container started.
   - Call the prefixed tool and assert the expected `"hello world"`-style
     result — proves the full round trip: gateway → bearer-token-authed
     HTTP call → container → response → gateway → MCP client.
   - Reuse `assert_no_container_residue()` at the end (extend it if needed to
     also check the extra container is gone) to prove teardown works for
     extra containers too, matching the existing core-container check.

### Passing criteria for this phase (and for the feature as a whole)

- `uv run scripts/e2e_smoke.py e2e_smoke_5_extra_mcp_container` (or the full
  default suite) passes: the hello-world container starts, its `hello` tool
  is visible in `tools/list` under its prefixed name, calling it returns the
  expected result, and no container residue remains after the gateway
  process exits.
- This is the overall correctness gate for the feature — treat it as
  required before considering the extra-container feature "done," even
  though Phases 1–4 can each be merged incrementally beforehand.

No TDD needed here — write the container, the config wiring, and the test
together, then get it green.

---

## File/module summary

- `crates/core/src/domain/model.rs` — add `ExtraMcpContainerConfig`,
  `ExtraMcpContainerAuth`. (Phase 1)
- `crates/platform/src/config/mcp_containers_config.rs` — new, YAML loader +
  validation. (Phase 1)
- `crates/platform/src/container/startup.rs` — add
  `build_extra_container_config`, `ensure_extra_mcp_container`,
  `stop_extra_mcp_container`, extend orphan-GC role scoping. (Phase 2)
- `SECURITY_AUDIT.MD` — Threat Model section update. (Phase 2)
- `crates/core/src/application/` — new `remote_mcp_container_client.rs`
  (or extend `mcp_proxy.rs`); extend `mcp_router.rs` to route through a list
  of extra-container tool sources alongside native tools. (Phase 3)
- `crates/platform/src/runtime/bootstrap.rs` — load config, ensure extra
  containers, wire their clients into the router, register shutdown.
  (Phases 2–3)
- `README.md` — "Experimental" section. (Phase 4)
- `testdata/e2e_hello_mcp_container/Containerfile` — new test-only image.
  (Phase 5)
- `scripts/e2e_smoke.py` — build the hello-mcp image, register the new test
  name. (Phase 5)
- `apps/gateway/tests/e2e_smoke.rs` — `write_mcp_containers_yaml` helper,
  new `e2e_smoke_5_extra_mcp_container` test. (Phase 5)
