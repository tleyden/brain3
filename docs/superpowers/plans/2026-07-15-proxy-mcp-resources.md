# Proxy MCP Resources (App Widget support)

## Goal

Today the gateway's `McpRouterUseCase` special-cases three JSON-RPC methods —
`initialize`, `tools/list`, `tools/call` — so it can merge in native (in-process
Rust) tools and Plugin MCP Container tools alongside the core vault-tools
container's own tools. Everything else, including `resources/list` and
`resources/read`, falls through to a generic catch-all that forwards
byte-for-byte to the core vault-tools container (`ProxyMcpUseCase::forward_request`
in `crates/core/src/application/proxy_mcp.rs`).

That catch-all is enough for **MCP App Widgets** (per the pasted host
integration notes: the host does `resources/read` for a tool's
`_meta.ui.resourceUri`, plus `tools/call` from the widget itself) *only when
the widget's resource lives on the core vault-tools container*. It silently
breaks for:

- **Plugin MCP Containers** (`RemoteMcpContainerClient`) — if a plugin
  container declares a tool with `_meta.ui.resourceUri`, the host's
  `resources/read` for that `ui://` URI falls into the catch-all and gets sent
  to the *core vault-tools container*, not the plugin container that actually
  owns the resource. Vault-tools won't recognize the URI and will 404/error.
- **Native Rust tools** (`NativeMcpToolRegistry`) — there is no resource store
  for native tools at all today. A native tool that wants an App Widget has
  nowhere to register the HTML/JS bundle, and any `resources/read` for it would
  also incorrectly hit vault-tools.

This plan extends the existing tools/list-merge, tools/call-route pattern to
`resources/list` and `resources/read`, plus patches the `initialize` response
so hosts learn resources are supported at all.

## Non-goals

- No resource *subscriptions* (`resources/subscribe`, `notifications/resources/updated`).
  Nothing in today's codebase or the pasted host notes needs them.
- No native tool actually gets an App Widget in this plan — this is
  infrastructure only. (A follow-up plan would add the first native
  `NativeMcpResource` impl once there's a widget to ship.)
- Not touching vault-tools (Python) — it doesn't declare a `resources`
  capability today and isn't in scope; whatever it does or doesn't expose
  keeps flowing through the existing untouched catch-all.
- No SECURITY_AUDIT.MD update needed: this reuses the exact same trust
  boundary and bearer-secret mechanism already used for `tools/call` proxying
  to Plugin MCP Containers — it's a new JSON-RPC *method* being routed, not a
  new network ingress.

## Key design decision: resource URI prefixing

Tool names from Plugin MCP Containers are already namespaced with a
`{container_name}__{original_name}` prefix (see
`RemoteMcpContainerClient::fetch_prefixed_tool_schemas`) so two containers
can't collide and so `tools/call` can be routed back to the right container by
stripping the prefix. Resource URIs need the same treatment, but a URI has
structure (`scheme://authority/path`) that a bare tool name doesn't, so the
prefix goes into the **authority** component, not the whole string:

```
original:  ui://widget-name/index.html
prefixed:  ui://fluensy_learn__widget-name/index.html
```

This keeps the prefixed URI a syntactically valid URI (same scheme, authority
still a legal token), and is reversible: strip `resources/read` params to
`{container}__{rest}`, split on the first `__`, forward `rest` unprefixed to
the plugin container.

Consequence: any `_meta.ui.resourceUri` inside a Plugin MCP Container's tool
schema must be rewritten with the same prefix when we rewrite the tool's
`name` field, otherwise the host will ask for a URI our router doesn't
recognize as belonging to that container.

Native tool resource URIs are **not** prefixed — native tools already aren't
namespaced (`NativeMcpToolRegistry` matches on bare `name()`), so a native
tool's `ui://` URI is expected to already be globally unique by convention
(e.g. `ui://brain3-native/<tool>/index.html`).

## Phases

### Phase 1 — `ports::native_mcp_resource` + registry support

- New port trait `NativeMcpResource` (mirrors `NativeMcpTool`):
  ```rust
  #[async_trait::async_trait]
  pub trait NativeMcpResource: Send + Sync {
      fn uri(&self) -> &str;
      fn name(&self) -> &str;
      fn mime_type(&self) -> &str;
      async fn read(&self) -> Result<NativeMcpResourceContent, NativeMcpToolError>;
  }
  ```
- `NativeMcpToolRegistry` gains an optional `Vec<Arc<dyn NativeMcpResource>>`
  (or a sibling `NativeMcpResourceRegistry`, matching the existing one-registry-
  per-concept style), with `find_resource(uri)` and `list_resource_schemas()`
  (for `resources/list` descriptor entries: `uri`, `name`, `mimeType`).
- No concrete resource yet in this phase — this is scaffolding, unit-tested
  with a fake resource the same way `FakeNativeTool` is used in
  `mcp_router.rs` tests today.

### Phase 2 — `RemoteMcpContainerClient` resource caching

- Extend `RemoteMcpContainerClient::initialize_and_cache_tools` to also call
  `resources/list` right after `tools/list`. If the plugin container returns
  a JSON-RPC error (e.g. `-32601 Method not found` because it never declared
  a `resources` capability), treat that as "no resources" and continue —
  don't fail container initialization over it. Log at `debug`/`info` either
  way (mirrors the tolerant, best-effort tone of the rest of this file).
- Cache `Vec<RemoteMcpContainerResourceSchema { prefixed_uri, original_uri, schema }>`,
  same shape as `RemoteMcpContainerToolSchema`.
- Add `strip_resource_uri_prefix(uri) -> Option<&str>` and
  `has_resource(original_uri) -> bool`, mirroring `strip_prefix`/`has_tool`.
- Add `read_resource(request, original_uri) -> Result<McpProxyResponse, ProxyError>`
  that clones the incoming `resources/read` request, rewrites
  `params.uri` back to `original_uri`, and forwards it — mirrors `call_tool`.
- In `fetch_prefixed_tool_schemas`, after prefixing `name`, also rewrite
  `_meta.ui.resourceUri` in place if present and if it matches one of this
  container's own resource URIs (look it up in the just-fetched resource
  list; if it doesn't match anything the container declared, leave it alone
  and log a warning — a widget pointing at a resource its own server didn't
  list is a plugin bug, not ours to silently paper over).

### Phase 3 — `McpRouterUseCase` routing

In `crates/core/src/application/mcp_router.rs`, add two new match arms in
`route_request` alongside the existing `tools/list` / `tools/call` ones:

- **`resources/list`**: forward to the core proxy first (unchanged upstream
  call), then append native resource schemas and each plugin container's
  cached prefixed resource schemas — same pattern as `append_tool_schemas`.
  Extract the shared "parse body, mutate `result.<array>`, re-serialize,
  strip content-length" logic into a small helper both `tools/list` and
  `resources/list` call, rather than copy-pasting the whole function body.
- **`resources/read`**: parse `params.uri` from the request body.
  1. If it matches a native resource, serve it locally (analogous to
     `maybe_call_native_tool` — build the JSON-RPC `result.contents[]`
     response directly, no proxy round-trip).
  2. Else, for each plugin container, check `strip_resource_uri_prefix`; if
     it matches, call `read_resource` on that container and return its
     response (analogous to `maybe_call_plugin_tool`).
  3. Else, fall through to the core proxy unchanged (today's behavior) — this
     is the path that keeps vault-tools' own resources working exactly as
     they do now.

### Phase 4 — `initialize` response capability patch

Hosts likely gate whether they ever attempt `resources/read` on the server
advertising a `resources` capability in its `initialize` response. Today
`mcp_router.rs`'s `initialize` arm forwards the core proxy's response
untouched. Patch it: if native resources or any plugin container resources
exist, and the parsed response's `result.capabilities` doesn't already have a
`resources` key, insert `"resources": {}`. Same "parse, mutate, re-serialize,
strip content-length" helper as Phase 3.

This behavior is a best guess at what real MCP Apps hosts (ChatGPT, Claude)
require — flag it for a quick manual check against a real host once there's
an actual widget to test with, since we have no host simulator in this repo.

### Phase 5 — tests

Follow the existing style in `mcp_router.rs`'s `#[cfg(test)] mod tests`
(`CapturingProxy`, `FakeNativeTool`, `initialized_plugin_client` helpers):

- `resources_list_forwards_to_proxy_and_appends_native_and_plugin_resources`
- `resources_read_for_native_resource_bypasses_proxy`
- `resources_read_for_prefixed_plugin_resource_routes_to_container_and_strips_prefix`
- `resources_read_falls_through_to_core_proxy_for_unrecognized_uri`
- `initialize_response_gains_resources_capability_when_resources_exist`
- In `remote_mcp_container_client.rs`: a test asserting `_meta.ui.resourceUri`
  gets rewritten with the container prefix when it matches a cached resource,
  and left alone (with a warning, not a failure) when it doesn't.
- In `remote_mcp_container_client.rs`: a test asserting a plugin container
  that returns `-32601` for `resources/list` still initializes successfully
  with zero cached resources.

## Verification

- `cargo test -p brain3 --no-run` then `cargo test` after each phase.
- No E2E fixture today ships a `ui://` resource, so `e2e_smoke.rs` isn't
  expected to change — note this gap rather than fabricate an E2E test around
  it; a real widget-bearing plugin container would be the natural trigger to
  add one later.
