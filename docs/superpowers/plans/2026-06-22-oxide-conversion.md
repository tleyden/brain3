Final Plan: Migrate to oxide-auth

### Phase 1 — Add dependencies

```toml
# crates/platform/Cargo.toml
oxide-auth = "0.6"
oxide-auth-async = "0.3"
oxide-auth-axum = "0.6"
```

`cargo test` — no behavior change, tests stay green.

---

### Phase 2 — Authorization code flow

**What's being replaced:** `InMemoryAuthCodeStore`, `AuthCodeStore` port, `AuthorizeUseCase`, hand-rolled PKCE validation, hand-rolled client credential checks.

1. Register hardcoded client via `ClientMap::Client::confidential` at startup — consolidates client_id, client_secret, and redirect_uri validation that is currently scattered across `authorize.rs` and `token_exchange.rs`

2. Replace `InMemoryAuthCodeStore` with `Arc<Mutex<AuthMap<RandomGenerator>>>` — oxide-auth's built-in in-memory auth code store; delete `platform/src/auth_code_store/` and `core/src/ports/auth_code_store.rs`

3. Implement `Solicitor` for credential checking — stateless struct that checks submitted username/password against config and returns `OwnerConsent::Authorized("operator")` or redirects back to the login form; bridges our existing login form into oxide-auth's flow

4. Wire `AuthorizationFlow` + `OAuthRequest`/`OAuthResponse` from oxide-auth-axum into `oauth_handlers.rs`; login form GET stays unchanged

5. PKCE: `Pkce::required()` / `Pkce::optional()` based on `config.pkce_required`

6. Delete `AuthorizeUseCase`, PKCE validation functions from `domain/oauth.rs`

7. Update `build_gateway_router` in `apps/gateway/src/server.rs`

`cargo test`

---

### Phase 3 — Token issuance + MCP validation

**What's being replaced:** `TokenExchangeUseCase`, `TokenStore` port, `StoredTokenKind`, `StoredTokenData`.

- **3a** — SQLite schema migration: add `owner_id`, `redirect_uri`, `scope` columns to existing `access_tokens` table via `ALTER TABLE` checks inside the existing `ensure_schema()` pattern

- **3b** — Implement oxide-auth's `Issuer` trait on `SqliteTokenStore`:

  | Issuer method | what it does |
  |---|---|
  | `issue(grant)` | store access + refresh tokens with all `Grant` fields |
  | `recover_token(token)` | SELECT where `kind='access'`, reconstruct `Grant` |
  | `recover_refresh(token)` | SELECT where `kind='refresh'`, reconstruct `Grant` |
  | `refresh(old_rt, grant)` | DELETE old refresh, INSERT new pair — rotation preserved |

  Keep inherent `cleanup_expired()` method on `SqliteTokenStore` (not part of `Issuer` trait) for the background task.

- **3c** — Wire `AccessTokenFlow` + `OAuthRequest`/`OAuthResponse` into `/oauth/token` handler; delete `TokenExchangeUseCase`

- **3d** — Update MCP bearer validation: use `OAuthResource` extractor (validates bearer without consuming request body) + `issuer.recover_token()`, check `grant.until > Utc::now()`; delete `validate_bearer_token`

- **3e** — Update `spawn_token_cleanup_task` to call `SqliteTokenStore::cleanup_expired()` directly — no longer goes through the `TokenStore` port trait

- **3f** — Update `build_gateway_router` to wire `Arc<Mutex<SqliteTokenStore>>` as Issuer, `Arc<ClientMap>`, `Arc<Mutex<AuthMap>>`

- **3g** — Delete `TokenStore` port, `StoredTokenKind`, `StoredTokenData`, `TokenExchangeUseCase`

`cargo test`

---

### Phase 4 — Cleanup

Remove remaining dead code in `domain/oauth.rs` and `core/src/application/` if empty. Remove `sha2`, `subtle`, `base64`, `rand` from `core/Cargo.toml` if no longer used.

`cargo test`

---

### Risk register

| Risk | Severity | Mitigation |
|---|---|---|
| ~~Axum version compat~~ | ~~High~~ | **Cleared** — oxide-auth-axum 0.6 uses axum 0.8 |
| SQLite schema migration on existing installs | Medium | `ALTER TABLE ... ADD COLUMN` with safe defaults in `ensure_schema()` |
| Refresh rotation correctness | Low | oxide-auth's `refresh()` revokes-then-issues; mirror exactly in `Issuer` impl |
| oxide-auth error → HTTP status mapping | Low | Verify 400/401 split matches existing test assertions after each phase