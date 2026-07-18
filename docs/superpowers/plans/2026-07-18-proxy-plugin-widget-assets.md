# Proxy plugin MCP widget static assets through the gateway

Status: DRAFT — awaiting review
Date: 2026-07-18

## Problem

The Fluensy Learn plugin container renders an MCP UI widget (`practice-widget`).
Through Brain3, the drill tool call and the widget **HTML** resource load fine,
but the widget paints blank because its JS/CSS bundle fails to load.

Observed in a live ChatGPT session (host = "Brain3 MacOS Fluensy"):

Browser console:
```
Access to script at
'https://brain3-macos.mcpnative.dev/mcp-use/widgets/practice-widget/assets/index-T4KqNWzc.js'
... has been blocked by CORS policy: No 'Access-Control-Allow-Origin' header ...
index-T4KqNWzc.js: Failed to load resource: net::ERR_FAILED
index-DyhWf25U.css: Failed to load resource: the server responded with a status of 404
```

Gateway log (repeats on every drill):
```
WARN brain3_platform::http::router: no matching route — returning 404
  method=GET path=/mcp-use/widgets/practice-widget/assets/index-T4KqNWzc.js
  host=Some("brain3-macos.mcpnative.dev")
```

## Root cause

The widget shell HTML (served via `resources/read`) references its bundle at
**absolute URLs on the public origin**, because the container bakes `MCP_URL`
(`https://brain3-macos.mcpnative.dev`) into the asset/base URLs:

```
https://brain3-macos.mcpnative.dev/mcp-use/widgets/practice-widget/assets/index-*.js
https://brain3-macos.mcpnative.dev/mcp-use/widgets/practice-widget/assets/index-*.css
```

The plugin container **does** serve these assets (verified: `GET :3000/mcp-use/
widgets/practice-widget/assets/index-*.js` → 200). But the Brain3 gateway
router (`crates/platform/src/http/router.rs`) only has routes for `/health`,
OAuth paths, and `/mcp[/...]`. There is **no route for `/mcp-use/**`**, so the
fallback returns 404. Because nothing handles the route, there is also no
`Access-Control-Allow-Origin` header, which is the CORS error above. The 404
and the CORS failure are the same missing-route bug.

Verified reachability:

| Path | Container (`:59020`) | Gateway (`:2763`) |
| --- | --- | --- |
| `/mcp-use/widgets/practice-widget/assets/index-*.js` | 200 | 404 |
| `/mcp-use/widgets/practice-widget/assets/index-*.css` | 200 | 404 |

Note: the plugin container itself, the drill generation, and the DB write/save
path are all healthy — this is purely a gateway static-asset routing gap.

## Goal

Serve plugin widget static assets (`/mcp-use/widgets/**`) through the gateway by
reverse-proxying GET requests to the owning plugin container, with CORS headers
so the MCP host's widget sandbox can load them.

Out of scope for now (explicitly deferred): making Fluensy emit relative asset
URLs. The browser resolves the asset URLs against the public gateway origin
regardless, so the gateway proxy is the real fix.

## Design

### New route

In `build_router` (`crates/platform/src/http/router.rs`), add a GET route:

```
/mcp-use/{*path}  ->  plugin_widget_asset_proxy
```

A new handler in `mcp_handlers.rs` (e.g. `plugin_widget_asset_proxy`) that:
1. Resolves which plugin container owns the request (see "Container resolution").
2. Reverse-proxies `GET {container_http_base}/mcp-use/{path}` (query string
   preserved) to the container.
3. Returns the container's response body + `Content-Type`, adding CORS headers.

Only expose GET (and OPTIONS for preflight). Do **not** forward POST/DELETE or
any non-`/mcp-use/` path — keep the unauthenticated surface minimal.

### Container resolution

The asset path (`/mcp-use/widgets/practice-widget/assets/...`) carries **no**
`container__` prefix, unlike tool names and resource URIs, so the owner is not
directly encoded in the path. Options:

- **(Preferred) Map the widget-name path segment to the owning container.**
  The 2nd path segment (`practice-widget`) is the widget name. Brain3 already
  knows each container's widget resource URIs (it rewrote
  `ui://widget/practice-widget.html` → `ui://fluensy_learn__widget/
  practice-widget.html`). Build a lookup: widget-name → container from the
  cached `prefixed_resource_schemas()` on each `RemoteMcpContainerClient`, and
  proxy to that container's HTTP base. Robust with multiple plugins.
- **(Fallback) Single-plugin shortcut.** If exactly one plugin container is
  configured, proxy `/mcp-use/**` straight to it. Simpler, but breaks the moment
  a second plugin registers a widget. Acceptable only as an interim step.

Recommend implementing the widget-name → container map; fall back to the single
container if the segment can't be matched.

### Container HTTP base

`RemoteMcpContainerClient` holds `mcp_url` (e.g. `http://127.0.0.1:59020/mcp`).
Derive the HTTP base by stripping the trailing `/mcp`. Note the host port is
dynamic (was `55607`, now `59020`) — always read it from the client, never
hardcode. Expose a small accessor (e.g. `http_base_url()`) on the client.

### CORS (host-agnostic)

These are public, credential-free static bundles. Do **not** hardcode any
specific host's sandbox origin (e.g. no OpenAI `*.oaiusercontent.com`). Set:

```
Access-Control-Allow-Origin: *
Access-Control-Allow-Methods: GET, OPTIONS
Access-Control-Allow-Headers: *
```

Respond to `OPTIONS /mcp-use/**` with 204 + the same headers for preflight.
Using `*` avoids coupling Brain3 to any particular MCP client. Revisit only if
we ever need credentialed asset requests (then reflect the `Origin` instead).

## Implementation steps

1. `remote_mcp_container_client.rs`: add `http_base_url()` (strip `/mcp` from
   `mcp_url`) and a helper to list this container's widget names from its cached
   resource schemas.
2. `mcp_router.rs` / core: expose a resolver `container_for_widget(widget_name)
   -> Option<&RemoteMcpContainerClient>` (walk `plugin_containers`).
3. `mcp_handlers.rs`: add `plugin_widget_asset_proxy` — parse widget name from
   path, resolve container, reverse-proxy GET via the existing reqwest proxy
   layer, attach CORS headers, handle OPTIONS.
4. `router.rs`: register `GET/OPTIONS /mcp-use/{*path}` → new handler in
   `build_router` (public origin only; the local router does not need it).
5. Logging: info-level "proxying plugin widget asset" with `container`,
   `widget`, `path`, upstream status; keep noise reasonable.

## Security

Adding an unauthenticated ingress path requires updating the Threat Model in
`SECURITY_AUDIT.md` before merge (per AGENTS.MD). Constraints to document:
- Route is GET/OPTIONS only, scoped strictly to `/mcp-use/**`.
- Proxies only to already-managed plugin container HTTP bases (no arbitrary
  upstream; path is not user-controllable beyond the asset subtree).
- Serves static assets the container already exposes; no auth bypass to `/mcp`
  or vault data. `Access-Control-Allow-Origin: *` applies to these static
  assets only, not to the authenticated `/mcp` endpoint.

## Testing

- Unit: widget-name → container resolution (hit, miss, multi-container).
- Unit/integration: `GET /mcp-use/widgets/<widget>/assets/<file>` returns 200
  with body, correct `Content-Type`, and `Access-Control-Allow-Origin: *`;
  `OPTIONS` returns 204 with CORS headers; unknown widget → 404.
- Manual E2E: reload tools in the MCP host, run a Fluensy drill, confirm the
  widget paints and the console shows no 404/CORS errors for `/mcp-use/**`.

## Open questions

- Multiple plugins exposing a widget of the **same name** — is widget-name alone
  a safe key, or should we prefix asset paths per-container too? (Rare today;
  single-plugin setup. Flag if multi-plugin widget collisions become real.)
- Do any MCP hosts require `Access-Control-Allow-Origin` to reflect a specific
  Origin rather than `*` (e.g. for credentialed fetches)? Assumed no for static
  bundles.
