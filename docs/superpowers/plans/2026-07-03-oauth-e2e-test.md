# Plan: OAuth 2.1 end-to-end smoke test

Date: 2026-07-03
Status: draft — awaiting review

## Motivation

The existing e2e suite (`apps/gateway/tests/e2e_smoke.rs`) brings up the real
`brain3` binary and the real MCP container and exercises all vault tools, but it
**bypasses the OAuth 2.1 gate entirely**:

- It connects to the *local* router (`build_local_router`, port `27640`) using
  the static `LOCAL_GATEWAY_MCP_BEARER_TOKEN` — the trusted-localhost bypass
  path (`local_mcp_proxy`).
- The *public* router (`build_router` → `mcp_reverse_proxy`, which validates
  OAuth-issued bearer tokens against the token store) is never hit.
- `/oauth/authorize`, `/oauth/token`, PKCE, and client_id/secret validation get
  zero e2e coverage — despite being the core of the remote-access security model
  described in `AGENTS.MD`.

This test closes that gap: drive the full authorization-code + PKCE flow against
the public port and use the resulting bearer token to call a vault tool through
`mcp_reverse_proxy`.

## Key question: does automating the login require a browser / Playwright?

**No. It is fully automatable with a plain HTTP client — no browser, no
Playwright, no headless anything.**

Why: the "login page" is **not** an external identity provider and contains no
JavaScript. It is a plain self-hosted HTML form served by our own gateway:

- `GET /oauth/authorize` renders `render_login_form` (`templates.rs`), which is a
  static `<form method="post" action="/oauth/authorize">` with:
  - hidden fields carrying the OAuth params (`client_id`, `redirect_uri`,
    `state`, `code_challenge`, `code_challenge_method`, `response_type`), and
  - visible `username` / `password` inputs.
- The browser's only job is to POST those fields back as
  `application/x-www-form-urlencoded`. Our test does exactly that POST directly.
- On valid credentials, `oauth_authorize_post` runs oxide-auth's
  `AuthorizationFlow`, which responds with a **302 redirect** to
  `redirect_uri?code=...&state=...`. The test does **not** follow the redirect;
  it just parses `code` out of the `Location` header.

So the browser step is pure form transport that we replicate over HTTP. There is
no interactive/JS surface that would require driving a real browser.

## The flow the test drives (all HTTP, against public port 27630)

1. **(Optional) Discovery:** `GET /.well-known/oauth-authorization-server` and
   assert `authorization_endpoint` / `token_endpoint` / `code_challenge_methods_supported: ["S256"]`.
2. **PKCE setup (via `oauth2-rs`):** `PkceCodeChallenge::new_random_sha256()`
   returns the `(challenge, verifier)` pair (challenge = `base64url_nopad(
   SHA256(verifier))`, method `S256`).
3. **(Optional) GET the form:** `GET /oauth/authorize?...` and assert 200 + HTML,
   to prove the pre-login page renders. Not strictly required for the token.
4. **Login POST:** `POST /oauth/authorize` (form-urlencoded) with
   `response_type=code`, `client_id`, `redirect_uri`, `state`,
   `code_challenge`, `code_challenge_method=S256`, plus `username` + `password`.
   Assert `302` and extract `code` (and echo of `state`) from `Location`.
   **Do not auto-follow redirects.**
5. **Token exchange (via `oauth2-rs`):** `exchange_code(code)
   .set_pkce_verifier(verifier).request_async(&http_client)` — sends
   `POST /oauth/token` with `grant_type=authorization_code`, `code`,
   `code_verifier`, `client_id`, `client_secret`, `redirect_uri` (must byte-match
   step 4 — oxide-auth rebinds it). Assert success and read `access_token`
   (+ `refresh_token`).
6. **Authenticated MCP call:** open an MCP client against
   `http://127.0.0.1:27630/mcp` with `Authorization: Bearer <access_token>` and
   call one vault tool (e.g. `vault_list` or a create+read round-trip). Assert
   success.

### Negative assertions (the security invariants worth locking down)

- `/mcp` with **no** bearer → `401` (and includes protected-resource metadata).
- `/mcp` with a **garbage** bearer → `401`.
- `/oauth/token` with a **wrong `client_secret`** → `401`/`invalid_client`.
- `/oauth/authorize` with an **unregistered `client_id`** → `401`/`invalid_client`.
- (Nice-to-have) token exchange with a **mismatched `code_verifier`** →
  `invalid_grant`, proving PKCE is enforced end-to-end.

These directly assert the `AGENTS.MD` policy: only the preregistered
client_id+secret can obtain a token and reach protected MCP data.

## Config / wiring notes (from current code)

- Client id env: `B3_OAUTH2_GATEWAY_CLIENT_ID`, default `brain3-oauth2-client`
  (`env_file.rs:124`). The existing e2e `.env` does **not** set it, so the test
  can rely on the default or set it explicitly for clarity.
- Client secret: `B3_OAUTH2_GATEWAY_CLIENT_SECRET` (already set to
  `e2e-test-client-secret` in `write_env_file`).
- Login creds: `B3_USERNAME` / `B3_PASSWORD` (already set to
  `e2e-test-user` / `e2e-test-password`).
- PKCE: `B3_OAUTH2_PKCE_REQUIRED`, default `true` (`env_file.rs:128`) — so the
  test **must** send `code_challenge`/`code_verifier`.
- `B3_OAUTH2_GATEWAY_ENFORCE_HOSTNAME_CHECK=false` is already set, so hitting the
  gateway on `127.0.0.1:27630` is fine.
- `redirect_uri`: the registrar (`GatewayRegistrar::bound_redirect`) accepts any
  runtime redirect URI for the configured client, so the test can use a fixed
  value (e.g. `https://claude.ai/api/mcp/auth_callback`). It is never followed;
  it only needs to be identical in steps 4 and 5.

## Implementation approach (decisions resolved)

### Two separate tests, run serially, local first, fail fast

- Keep the existing `e2e_smoke_local_docker` (local trusted-bearer path) as-is.
- Add a second `#[tokio::test]`, e.g. `e2e_smoke_oauth_public_flow`, for the
  OAuth path. Both reuse `TempTestDir` / `Brain3Process` (same spawn, same
  container image, same cleanup guards). No new binary or env plumbing beyond
  what `write_env_file` already emits.
- **Rationale for two tests:** we explicitly want to verify the local path and
  the OAuth path independently, so a failure points at exactly one path.
- **Ordering / fail-fast:** run them in serial with the **local test first, then
  the OAuth test**, and stop on the first failure. Enforce this by running the
  e2e binary single-threaded so ordering is deterministic and the run aborts as
  soon as one test fails:
  `cargo test ... -- --nocapture --test-threads=1 --fail-fast`
  (`--fail-fast` is libtest's default, but state it explicitly). Update
  `scripts/e2e_smoke.py`'s `cargo_test_command` to pass `--test-threads=1`.
  Name the tests so alphabetical order = desired order (e.g.
  `e2e_smoke_1_local_docker`, `e2e_smoke_2_oauth_public_flow`) or otherwise
  guarantee local runs first. They are both fast, so serial is fine.
- Each test spawns its own gateway + container. Accept the extra bring-up for
  clean isolation; do not try to share one gateway across both tests.

### OAuth client: use `oauth2-rs` as the test client (one new dev-dependency)

Context: the repo has **no OAuth client library** — it is the OAuth *server*,
built on `oxide-auth` / `oxide-auth-async` / `oxide-auth-axum` (see
`oauth_handlers.rs`, `registrar.rs`). The e2e test plays the client role, so
rather than hand-roll PKCE + token requests, use the standard
`oauth2` crate (`ramosbugs/oauth2-rs`) as the test client.

- Add `oauth2` as a **single new dev-dependency** of `apps/gateway`, with its
  `reqwest` feature (async). This replaces the previously-planned trio of
  `reqwest` + `sha2` + `base64`: `oauth2` provides PKCE and the token exchange,
  and re-exports `oauth2::reqwest` which we reuse for the one manual step below.

What `oauth2` does for us (standard-conformant, no manual param assembly):
- **PKCE:** `let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();`
  — no more `sha2`/`base64`. Internally this is exactly
  `base64url_nopad(SHA256(verifier))` with method `S256`.
- **Token exchange:** `client.exchange_code(AuthorizationCode::new(code))
  .set_pkce_verifier(verifier).request_async(&http_client).await` — builds and
  sends `POST /oauth/token` with `grant_type=authorization_code`, `code`,
  `code_verifier`, `client_id`, `client_secret`, `redirect_uri`, and parses the
  `access_token` / `refresh_token` response.

The one step `oauth2` cannot do (app-specific), done manually with the
re-exported `oauth2::reqwest`:
- **The login-form POST.** Our `/oauth/authorize` renders our own HTML login
  form and only issues the code once credentials are POSTed. `oauth2`'s
  `authorize_url()` merely *builds* the redirect URL; it has no notion of
  submitting username/password. So the test:
  1. takes `challenge.as_str()` (+ method `S256`) from the pair above,
  2. `POST /oauth/authorize` (form-urlencoded) with `response_type=code`,
     `client_id`, `redirect_uri`, `state`, `code_challenge`,
     `code_challenge_method=S256`, `username`, `password`, using a
     `reqwest::Client` built with `.redirect(reqwest::redirect::Policy::none())`
     so the `302 Location: <redirect_uri>?code=...` is *read* rather than
     followed,
  3. parses `code` (and echoed `state`) out of the `Location` header,
  4. hands `code` to `oauth2`'s `exchange_code(...)` above.

Client configuration notes:
- Point the `oauth2` client at the running gateway:
  `AuthUrl = http://127.0.0.1:27630/oauth/authorize`,
  `TokenUrl = http://127.0.0.1:27630/oauth/token`,
  `ClientId = brain3-oauth2-client`, `ClientSecret = e2e-test-client-secret`,
  `RedirectUrl` = the same fixed URI used in the login POST (must byte-match).
- **Set `AuthType::RequestBody`** on the client so the client credentials are
  sent in the token request body (`client_secret_post`), matching what the
  server advertises in its metadata (`token_endpoint_auth_methods_supported:
  ["client_secret_post"]`) and what `oauth_token` expects
  (`flow.allow_credentials_in_body(true)`). `oauth2`'s default is
  `BasicAuth`, so this must be set explicitly.
- We only use `oauth2` for authorize-URL/PKCE/token plumbing; we do **not** rely
  on its CSRF/`authorize_url` redirect handling since we drive the login POST
  ourselves.

## Explicit non-goals

- **No Cloudflare tunnel.** Deferred; keep `B3_CF_QUICK_TUNNEL=false` and the
  `cloudflared` shim as-is.
- No DCR / CIMD / public-client / `token_endpoint_auth_method=none` flows —
  those remain intentionally unsupported (`AGENTS.MD` security rule). The test
  only exercises the preregistered confidential client.
- No browser automation tooling (Playwright/headless Chromium) — see above,
  it's unnecessary.

## Verification

- `cargo test -p brain3 --no-run` (compile the e2e target).
- `uv run scripts/e2e_smoke.py` (runs the container build + both e2e tests).
  This change touches the gateway/OAuth surface, so run the full e2e locally
  before considering it done.
</content>
</invoke>
