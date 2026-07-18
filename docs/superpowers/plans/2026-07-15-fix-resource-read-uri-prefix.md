# RCA + Fix Plan: Plugin MCP widget UI not appearing after resource-proxying work

## Summary

The `resources/read` round trip for Plugin MCP Container widgets is silently
broken: the gateway correctly requests the resource from the plugin
container, correctly gets a 200 back, but returns the plugin's response
**unmodified** — including its own un-prefixed `uri` field — to the host,
instead of rewriting it to match the prefixed URI the host actually asked
for. Hosts that verify the returned resource's `uri` matches what they
requested (a reasonable thing for any MCP Apps host to do, since it's how a
confused-deputy attack would be prevented) will discard the response, and
the widget never renders — even though every other hop in the chain is
working.

This was found by cross-referencing `/Users/tleyden/.brain3/brain3.log`
against the code, not by any purpose-built tooling — see "How I verified
propagation" below for a reusable checklist, and "Diagnosability gaps" for
what to add so this doesn't require manual log archaeology next time.

## Evidence trail (from `/Users/tleyden/.brain3/brain3.log`, run starting 22:11:39)

1. `fluensy_learn` container started and initialized cleanly:
   `cached Plugin MCP Container tool schemas container=fluensy_learn tool_count=5 resource_count=1` (line 104).
2. `tools/list` correctly merged in all 5 prefixed plugin tools + 1 native
   tool (line 134, `total_tool_count=16`).
3. `resources/list` correctly merged in the 1 plugin resource (line 151,
   `plugin_resource_count=1 total_resource_count=1`).
4. The host (`brain3-macos.mcpnative.dev`) then actually called
   `resources/read` for `ui://fluensy_learn__widget/practice-widget.html` —
   i.e. it saw the widget, recognized the (correctly prefixed) resourceUri,
   and asked for it (line 166). This proves `_meta.ui.resourceUri` rewriting
   in `RemoteMcpContainerClient::prefix_tool_schemas` /
   `rewrite_tool_resource_uri` is working.
5. The gateway correctly stripped the prefix and forwarded to the plugin
   container as `ui://widget/practice-widget.html` (line 167), and got back
   HTTP 200 / 1087 bytes (line 169, "MCP upstream responded OK").
6. **But**: `RemoteMcpContainerClient::read_resource()`
   (`crates/core/src/application/remote_mcp_container_client.rs:146-173`)
   forwards the *request* with the URI rewritten back to the plugin's
   original form (correct), then returns the plugin's raw JSON-RPC response
   completely unmodified via `self.forward_json(body).await` (no rewriting
   at all on the way out). The plugin's response body's
   `result.contents[0].uri` therefore still says
   `ui://widget/practice-widget.html` — the plugin's own un-prefixed URI —
   not `ui://fluensy_learn__widget/practice-widget.html`, which is what the
   host asked for and is the only URI the host knows how to look up a widget
   by.

This is the mirror-image gap of `call_tool()`, which *does* rewrite the
outbound request's tool `name` back to unprefixed before forwarding, but
(correctly, since tool call results don't echo the tool name) doesn't need
to rewrite anything in the response. `read_resource()` needs the same
treatment `call_tool()` gets on the way in, plus a rewrite on the way out
that `call_tool()` doesn't need.

Also confirmed as *not* the problem, despite initially looking suspicious:

- The Phase 4 `initialize` capability patch
  (`patch_initialize_resources_capability` in `mcp_router.rs:513-556`) never
  logs "added resources capability to initialize response" in this run. This
  looked like a bug but isn't: vault-tools is built on
  `mcp.server.fastmcp.FastMCP`, which unconditionally registers a
  `list_resources` handler in `_setup_handlers()`
  (`mcp/server/fastmcp/server.py:309`) regardless of whether any resources
  are actually registered. That means vault-tools' own `initialize` response
  already advertises `"resources": {"subscribe": false, "listChanged": false}`
  today, so the `capabilities.contains_key("resources")` early-return in our
  patch fires every time — silently, since that branch has no log line (see
  diagnosability gap #1 below). The capability is present either way; this
  is not why the widget fails to render.

## Fix

In `crates/core/src/application/remote_mcp_container_client.rs`:

- `read_resource()`: after `forward_json` returns, parse the response body
  as JSON. If it has a `result.contents` array, rewrite each element's `uri`
  field from this container's `original_uri` back to `prefixed_uri` (reuse
  `prefix_resource_uri_for_container`, already defined at module scope) —
  only rewrite entries whose `uri` matches this resource's `original_uri`,
  leave anything else untouched. Re-serialize and strip `content-length`,
  mirroring the exact `append_result_array` / `strip_content_length` pattern
  already used in `mcp_router.rs`. If the body is a JSON-RPC error or fails
  to parse, pass it through unmodified (same tolerant style as
  `patch_initialize_resources_capability`).
- Add a unit test mirroring `call_tool_strips_container_prefix_before_forwarding`,
  but asserting the *response* body's `result.contents[0].uri` comes back as
  `ui://fluensy_learn__widget-name/index.html`, not the plugin's raw
  `ui://widget-name/index.html`.
- Extend the existing router-level test
  `resources_read_for_prefixed_plugin_resource_routes_to_container_and_strips_prefix`
  in `mcp_router.rs` to assert on `response.body`'s returned `uri`, not just
  the outbound request — today it only checks what was sent to the plugin,
  never what comes back to the host, which is exactly how this bug slipped
  through review.

## Diagnosability gaps to close (the "no way to debug this" problem)

These didn't cause the bug, but they're why confirming/denying it required
manually correlating six different log lines by timestamp instead of reading
one clear signal:

1. `patch_initialize_resources_capability` (`mcp_router.rs:513-556`) has
   three silent no-op return paths (`!has_resources()`, response already has
   an `error`, capabilities already contains `resources`) but only two of
   the four total branches log anything. Add a `tracing::debug!` on every
   branch stating *why* no patch was applied (or that none was needed),
   so you can tell from logs alone whether the `resources` capability came
   from brain3's patch or was already present upstream.
2. `rewrite_tool_resource_uri()` (`remote_mcp_container_client.rs:375-396`)
   only logs on the failure branch (URI not found in resources/list). Add an
   `tracing::info!` on success with container, tool name, and
   before/after resourceUri — today success is only inferable indirectly
   (by the *absence* of a warning, or by watching what URI the host later
   requests).
3. Neither `call_tool()` nor `read_resource()` log the plugin container's
   response status/body length at the point they receive it — only the
   outbound forward is logged in `remote_mcp_container_client.rs`, and the
   generic "MCP upstream responded OK" line in `mcp_handlers.rs` doesn't
   distinguish plugin-routed traffic from vault-tools traffic. Add a
   `tracing::debug!` in both functions logging status + body length of the
   plugin's raw response, before any router-level rewriting happens, so a
   "plugin container gave us garbage" failure mode is distinguishable from a
   "we mangled a good response" failure mode.

## How I verified propagation end-to-end (reusable checklist)

Since there's no purpose-built tool for this yet, this is the manual
procedure that established the facts above — worth turning into a script if
this comes up again:

1. Confirm the plugin container declared the resource in the first place:
   `grep "cached Plugin MCP Container tool schemas" brain3.log` — check
   `resource_count` > 0.
2. Confirm `tools/list` includes the tool with a **prefixed**
   `_meta.ui.resourceUri` — currently only checkable by grepping for
   `"MCP router: appended native and Plugin MCP tools"` (confirms the merge
   happened) plus manually calling `tools/list` and inspecting the JSON, since
   there's no dedicated log line for the URI rewrite itself (gap #2 above).
3. Confirm `resources/list` includes the same prefixed URI:
   `grep "appended native and Plugin MCP resources" brain3.log`.
4. Confirm the **host** actually calls `resources/read` with that exact
   prefixed URI (not the plugin's raw URI) — if it doesn't, the widget was
   never recognized as belonging to that tool in the first place, which
   points back at step 2. `grep "routing Plugin MCP resource read" brain3.log`.
5. Confirm the plugin container returns 200 for that read:
   `grep "forwarding resources/read to Plugin MCP Container" brain3.log`,
   then find the matching "MCP upstream responded OK" line by timestamp.
6. **This is the step that was missing and hid the bug**: confirm the `uri`
   field *inside* the response body that goes back to the host matches the
   prefixed URI from step 4, not the plugin's raw URI. There is currently no
   log line for this — you have to reproduce the call by hand (e.g. drive
   the local MCP listener or the plugin container directly with `curl`, feed
   it through the same code path, and inspect the JSON) or add a temporary
   log statement. Once the fix above ships, this becomes checkable purely
   from logs if diagnosability gap #3 is also closed.

## Verification after fix

- `cargo test -p brain3 --no-run` then `cargo test`.
- Manually re-run the host flow against the real `fluensy_learn` container
  and confirm the widget renders, or at minimum re-run step 6 above and
  confirm the returned `uri` is now prefixed.
