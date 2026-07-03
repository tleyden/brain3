# Plan: OAuth 2.1 over a real Cloudflare quick tunnel — end-to-end smoke test

Date: 2026-07-03
Status: draft — awaiting review

## Motivation

The two existing e2e tests (`apps/gateway/tests/e2e_smoke.rs`) both drive the
gateway on **loopback** (`127.0.0.1`):

- `e2e_smoke_1_local_docker` — trusted-localhost bearer path (`local_mcp_proxy`).
- `e2e_smoke_2_oauth_public_flow` — full OAuth 2.1 + PKCE flow, but still against
  `http://127.0.0.1:27630`.

Both deliberately **disable the Cloudflare tunnel** (`B3_CF_QUICK_TUNNEL=false`)
and stub `cloudflared` with a shim that exits `0`
(`TempTestDir::write_cloudflared_shim`). So the app's actual remote-access
mechanism — a real `cloudflared` quick tunnel giving a public
`https://<random>.trycloudflare.com` ingress in front of the OAuth-protected MCP
router — has **zero e2e coverage**, even though "open it up to remote access"
is a stated core purpose of the app (`AGENTS.MD`).

This test closes that gap: bring up the gateway with a **real quick tunnel**,
discover the public HTTPS URL from the logs, and run the full OAuth 2.1 + PKCE
flow **through the public tunnel URL** (not loopback), then make an authenticated
MCP tool call over that same public URL.

## What this proves that test 2 does not

- `cloudflared` is actually resolved, spawned, and its `trycloudflare.com` URL is
  parsed correctly from logs.
- The gateway is reachable end-to-end over **public HTTPS** through Cloudflare's
  edge, not just loopback.
- The OAuth authorize/token endpoints and `mcp_reverse_proxy` work when the
  request arrives via the tunnel (different `Host` header, TLS-terminated at the
  edge, forwarded as `http://localhost:{port}`).
- Confirms the quick-tunnel branch disables hostname enforcement correctly:
  `resolve_expected_host()` returns `None` for `B3_CF_QUICK_TUNNEL=true`, so
  `validate_host` passes for the ephemeral `trycloudflare.com` host
  (`env_file.rs:511`, `validate_request.rs:8-13`). Test 2 never exercises this.

## Key facts established from the current code

- **Enabling the tunnel:** set `B3_CF_QUICK_TUNNEL=true`. `load_tunnel_config`
  then returns `TunnelConfig::CloudflareQuick { local_port: gateway_port }`
  (`env_file.rs:462`), i.e. the tunnel points at
  `http://localhost:27630` — the **public** OAuth router.
- **Real binary required:** `CloudflareQuickTunnelAdapter::start` calls
  `crate::util::find_cloudflared()` and spawns `cloudflared tunnel --url
  http://localhost:{port}`, then scrapes stderr for the `*.trycloudflare.com`
  URL, waiting up to 30s (`cloudflare_quick.rs:33-117`). The existing exit-0
  shim would produce **no URL** and time out, so this test must **not** use the
  shim — it needs a genuine `cloudflared` on `PATH` and outbound network access
  to Cloudflare.
- **Startup ordering makes URL discovery deterministic:** the tunnel is started
  inside `RuntimeBootstrap` *before* the HTTP server begins serving
  (`bootstrap.rs:295-311`; server starts after bootstrap in `main.rs`). So once
  `/health` returns 200, the public URL is already known and already logged.
- **Where the URL is written:** tracing goes to the **log file**
  `<brain3_home>/brain3.log` (`logging.rs`, `system.rs:251`), *not* stdout. In
  `--cli` mode it is also mirrored to stderr, but the test inherits stderr
  (`.stderr(Stdio::inherit())`) so it cannot capture it programmatically. The
  URL appears in the log via `cloudflare_quick.rs:112`
  (`"cloudflared quick tunnel URL ready" url=...`) and `bootstrap.rs:298`
  (`"tunnel started" url=...`) and `main.rs:634`
  (`"runtime public URL ready" url=...`).
- **Hostname enforcement is auto-disabled:** confirmed above — no need to set an
  expected host; leave `B3_OAUTH2_GATEWAY_ENFORCE_HOSTNAME_CHECK` as-is.
- **redirect_uri:** unchanged from test 2 — the fixed
  `https://claude.ai/api/mcp/auth_callback`, never followed, only echoed. The
  registrar accepts any runtime redirect for the configured client.

## How the test discovers the public URL

After `wait_for_health` succeeds, poll `<brain3_home>/brain3.log` (path known:
`temp.root/brain3.log`) and extract the first `https://<...>.trycloudflare.com`
match. Reuse the exact matching logic already proven in the app:
`extract_trycloudflare_url` in `cloudflare_quick.rs` (find `https://`, stop at
whitespace/`|`, require `.trycloudflare.com`). Poll with a short deadline
(a few seconds) purely as a guard; in practice the line is present the moment
health is up because of the startup ordering above.

> **Note:** Log scraping is clunky but adequate for v1. A future milestone will
> upgrade to **named tunnels** (`e2e-<uuid>.mcpnative.dev`) which eliminates log
> scraping entirely — the URL is known upfront. Named tunnels require Cloudflare
> credentials (API token, account ID), which adds complexity unsuitable for the
> initial implementation.

## The flow the test drives (all against the public tunnel URL)

Let `base = https://<random>.trycloudflare.com` (discovered above). **Reuse
existing test helpers** from test 2, just pointed at `base` instead of
`http://127.0.0.1:27630`:

1. `assert_public_mcp_rejects_missing_and_invalid_bearers(base)` — 401 sanity
   check that OAuth is active over the tunnel.
2. PKCE pair via `PkceCodeChallenge::new_random_sha256()`.
3. `submit_login_for_authorization_code(base, challenge, state)` — POST the login
   form, parse `code` + `state` from `Location`.
4. Token exchange via `oauth_client(base)` with
   `exchange_code(...).set_pkce_verifier(verifier)`.
5. `connect_public_mcp` pointed at `{base}/mcp` with the bearer — create + read a
   note and assert the round-trip.

**Keep it lean.** This is a **smoke test** — it proves the tunnel transport
works with OAuth, not a comprehensive OAuth validation (that's test 2). Reuse
existing code, no duplication. Skip redundant negative cases (wrong secret,
mismatched PKCE, etc.) — those are already covered by test 2.

## Implementation approach

### One new test, minimal refactor of the existing harness

Add `#[tokio::test] async fn e2e_smoke_3_oauth_quick_tunnel`. It reuses
`Brain3Process`, the OAuth/MCP helpers, `assert_no_container_residue`, and the
diagnostics guard. Two small harness changes are needed:

1. **Quick-tunnel env + no shim.** The tunnel test needs
   `B3_CF_QUICK_TUNNEL=true` and a real `cloudflared` on `PATH` (not the exit-0
   shim). Parameterize the bring-up rather than duplicating it. Concretely:
   - Add a `tunnel: bool` (or small `TunnelMode` enum) to `TempTestDir::create` /
     `write_env_file`, controlling `B3_CF_QUICK_TUNNEL` (`true` vs `false`) and
     whether the `cloudflared` shim is written / prepended to `PATH`. When
     `tunnel == true`: do **not** write the shim and do **not** prepend the shim
     dir, so `find_cloudflared` resolves the real binary from the inherited
     `PATH`.
   - Thread the same flag through `Brain3Process::spawn` for the
     `.env("B3_CF_QUICK_TUNNEL", ...)` line (currently hard-coded to `"false"`
     at `e2e_smoke.rs:201`) and the `PATH` it sets (`temp.path_with_shim()` vs
     plain inherited `PATH`).
   - Keep tests 1 and 2 calling the existing (shimmed, tunnel-off) path so their
     behaviour is byte-for-byte unchanged.

2. **URL discovery helper.** Add `read_public_tunnel_url(&temp) ->
   Result<String>` that polls `temp.root/brain3.log` and applies the
   `extract_trycloudflare_url` matcher (reuse the app's existing regex logic).

No new Rust dependencies: `oauth2`, `rmcp`, `reqwest`, `serde_json` are already
dev-deps and used by test 2.

### Gating: separate test, runs in CI whenever e2e tests run

This test is **networked** (hits Cloudflare's edge) and may be flaky, but it runs
as a **separate test** in CI alongside the other e2e tests.

- **Self-skip locally** at the top of the test: if `cloudflared` is not on `PATH`,
  `println!` a clear `SKIP:` line explaining why and `return Ok()`. Rationale:
  keeps the default run green for developers without `cloudflared` installed.
  Quick tunnels require **no credentials** (unlike named tunnels), so only the
  binary presence is checked.
- **`scripts/e2e_smoke.py`:** add this to the `DEFAULT_E2E_TESTS` list so it runs
  by default alongside tests 1 and 2. It will self-skip locally if `cloudflared`
  is missing.
- **CI:** add a **separate job** to `.github/workflows/e2e.yml` that:
  1. Installs `cloudflared` (provide a script: `scripts/install_cloudflared.sh`).
  2. Runs all e2e tests (including this one).
  
  This job runs **whenever e2e tests run** (same triggers as the main e2e job),
  but as a separate job so tunnel flakiness is isolated and visible. It's
  **not** blocking for merges initially — mark it as `continue-on-error: true` or
  a separate check. The goal is visibility, not a hard gate.

- **Flakiness tolerance:** if this test flakes in CI, that's **visible** and
  tracked, but it doesn't block PRs. Monitor the failure rate and decide later
  whether to make it blocking.

### Robustness for the networked hop

- **Bump timeouts** for the over-the-internet round-trips: the tunnel adds
  edge-to-localhost latency. Give the OAuth HTTP client and the MCP connect/call
  a generous timeout (the loopback tests are effectively instant; here allow
  10-15 seconds).
- **One retry on tunnel-edge errors is out of scope** for v1 — prefer a clean
  single attempt with clear failure output (dump the discovered URL + the log tail
  via the existing diagnostics guard) so a flake is obvious rather than silently
  retried.
- **URL discovery:** poll the log file for up to 5 seconds after health check
  passes. In practice the URL is logged immediately, but the poll provides a
  guard. If not found, fail with a clear message.
- **Tunnel startup:** the gateway already waits up to 30s for the tunnel URL
  internally (`cloudflare_quick.rs`), so no additional timeout needed.

## Explicit non-goals

- **No named tunnel** in v1. Named tunnels (`e2e-<uuid>.mcpnative.dev`) are
  cleaner (no log scraping) but require Cloudflare API credentials, adding
  complexity unsuitable for the initial implementation. **Future milestone:**
  upgrade to named tunnels once credential management is addressed.
- **No new app endpoints / ingress.** URL discovery is via the log file only, so
  `SECURITY_AUDIT.MD`'s threat model is untouched (no new ingress point).
- **No DCR / CIMD / public-client flows** — unchanged security posture
  (`AGENTS.MD`). Still only the preregistered confidential client.
- **No browser automation** — same as test 2, the login form is plain HTML we POST
  directly.
- **No comprehensive OAuth testing** — this is a **smoke test** for tunnel
  transport, not a repeat of test 2's OAuth validation.

## Verification

- `cargo test -p brain3 --no-run` (compile the e2e target, incl. the refactor).
- `cargo test` (unit tests still pass — the `TempTestDir` refactor must not
  regress the shim-based tests).
- Default e2e run: `uv run scripts/e2e_smoke.py` (runs tests 1, 2, and 3; test 3
  self-skips if `cloudflared` is missing).
- The new test explicitly, with `cloudflared` installed:
  `uv run scripts/e2e_smoke.py e2e_smoke_3_oauth_quick_tunnel`.
  This change touches the gateway/tunnel surface, so run it locally (with
  `cloudflared` on `PATH`) before considering it done.
- CI: verify the separate e2e job runs and either passes or has a clear
  failure/skip message.
