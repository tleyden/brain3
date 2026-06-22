# Refresh Token Fix Plan

## The Problem

Refresh tokens are **completely broken**. They are issued correctly, but can never be used.

### Root cause

`/oauth/token` routes every request through `AccessTokenFlow`. Looking at oxide-auth's
`AccessToken` state machine (`accesstoken.rs` line 364–368):

```rust
match request.grant_type() {
    Some(ref cow) if cow == "authorization_code" => (),
    None => return Err(Error::invalid()),
    Some(_) => return Err(Error::invalid_with(AccessTokenErrorType::UnsupportedGrantType)),
};
```

Any `grant_type=refresh_token` request hits the `Some(_)` arm and immediately returns
`400 {"error":"unsupported_grant_type"}`. The rest of our token store logic is never reached.

oxide-auth-async ships a separate `RefreshFlow` for the refresh grant type. We never import or
call it.

### Why users see "logged out when access token expires"

1. Access token expires → MCP returns 401
2. Client tries `POST /oauth/token` with `grant_type=refresh_token`
3. Gateway returns `400 unsupported_grant_type`
4. Client falls back to a full re-authorization (login page)

The logging improvements from the previous session will become useful once the flow is actually
wired up.

---

## Second problem: body credentials

`RefreshFlow`'s built-in `WrappedRequest` reads client credentials only from the
`Authorization: Basic …` header.

Our clients (ChatGPT, Claude) send credentials in the POST body:

```
grant_type=refresh_token
&client_id=brain3-oauth2-client
&client_secret=…
&refresh_token=…
```

If we just drop in `RefreshFlow` without a body-credential adapter, authentication will fail
via `CoAuthenticating` path → `registrar.check(client_id, None)` → our registrar returns
`Err` because passphrase is `None`.

---

## Fix Plan

### Part 1 — dispatch by grant_type in `oauth_token()`

`oauth_handlers.rs:oauth_token()` currently always creates an `AccessTokenFlow`. Change it to
inspect `grant_type` from the request body (already captured in `request_shape`) and dispatch:

| grant_type           | action |
|----------------------|--------|
| `authorization_code` | `AccessTokenFlow` (unchanged) |
| `refresh_token`      | new `RefreshFlow` path (see Part 2) |
| anything else / None | return `400 unsupported_grant_type` immediately with log |

### Part 2 — `BodyRefreshRequest` adapter

`oxide_auth_async::code_grant::refresh::refresh()` takes any type implementing
`oxide_auth::code_grant::refresh::Request`. Create a `BodyRefreshRequest` struct that:

- Extracts `grant_type`, `refresh_token`, `client_id`, `client_secret`, `scope` from the
  `OAuthRequest` body upfront (owned `Option<String>` fields, no lifetime issues)
- Implements `Request::authorization()` by returning
  `Some((client_id, client_secret.as_bytes()))` when both are present

This makes body-based credentials work identically to Basic auth, matching how
`AccessTokenFlow` already does it via `allow_credentials_in_body(true)`.

### Part 3 — `SimpleRefreshEndpoint` adapter

Create a thin `SimpleRefreshEndpoint<'a>` that holds `&'a GatewayRegistrar` and
`&'a mut SqliteTokenStore` and implements
`oxide_auth_async::code_grant::refresh::Endpoint`. Call `refresh()` directly instead of using
`RefreshFlow` (avoids the intermediate `WebRequest` wrapping problem entirely).

Build the HTTP response manually from the `Result<BearerToken, Error>`:

| result | HTTP response |
|--------|---------------|
| `Ok(bearer)` | `200 application/json` with `bearer.to_json()` |
| `Err(Invalid(desc))` | `400` with `desc.to_json()` |
| `Err(Unauthorized(desc, _))` | `401` with `desc.to_json()` |
| `Err(Primitive)` | `500 INTERNAL_SERVER_ERROR` |

### Part 4 — test coverage

Add one integration test `refresh_token_exchange_succeeds` to `oauth_integration.rs`:

1. Full authorize → code exchange to get `access_token` + `refresh_token`
2. `POST /oauth/token` with `grant_type=refresh_token` + body credentials
3. Assert `200`, new `access_token` and `refresh_token` in response
4. Assert old access token is revoked (MCP returns 401)
5. Assert new access token is accepted (MCP returns 200)

---

## Files touched

| file | change |
|------|--------|
| `crates/platform/src/http/oauth_handlers.rs` | dispatch by grant_type; add `BodyRefreshRequest`, `SimpleRefreshEndpoint`, inline response builder |
| `crates/platform/tests/oauth_integration.rs` | add refresh token exchange test |

No schema or DB changes. No changes to `registrar.rs`, `sqlite.rs`, or `mcp_handlers.rs`.
