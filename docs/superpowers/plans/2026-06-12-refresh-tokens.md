# Refresh Tokens Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add OAuth refresh tokens so the gateway can mint new short-lived access tokens without another login, with a configurable refresh-token TTL defaulting to 90 days.

**Architecture:** Extend the existing SQLite-backed token store to persist both access tokens and refresh tokens as separate token records with distinct expiry windows. Keep grant handling and token issuance in `core`, keep SQLite details and HTTP form parsing in `platform`, and preserve the preregistered confidential-client-only policy by requiring the same hardcoded client ID and client secret for the refresh flow.

**Tech Stack:** Rust, Tokio, Axum, sqlx/sqlite, existing `TokenStore` port, existing OAuth handlers and integration-test harness.

---

## Design choices to lock before implementation

- Refresh tokens are **single-use and rotate on every successful refresh**.
- Refresh-token expiry is configurable with a new env var, default **90 days** (`7_776_000` seconds).
- `/oauth/token` continues to require `client_secret_post`; refresh requests must still include the preregistered `client_id` and `client_secret`.
- Refreshing issues **both** a new access token and a new refresh token.
- The old refresh token is revoked atomically as part of a successful refresh exchange.

---

### Task 1: Extend the domain token model for refresh tokens

**Files:**
- Modify: `crates/core/src/domain/oauth.rs`
- Modify: `crates/core/src/ports/token_store.rs`
- Modify: `crates/core/src/domain/model.rs`
- Modify: `crates/core/src/domain/errors.rs`

- [ ] **Step 1: Add a refresh-token TTL constant and extend OAuth request/response types**

Add a default refresh-token lifetime alongside the existing access-token lifetime, and update the token request/response structs so the core layer can represent both authorization-code and refresh-token exchanges.

```rust
pub const DEFAULT_ACCESS_TOKEN_LIFETIME_SECS: u64 = 3600;
pub const DEFAULT_REFRESH_TOKEN_LIFETIME_SECS: u64 = 90 * 24 * 60 * 60;

#[derive(Debug, Clone)]
pub struct TokenRequest {
    pub grant_type: String,
    pub client_id: String,
    pub client_secret: String,
    pub code: Option<String>,
    pub refresh_token: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_verifier: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub refresh_token: Option<String>,
}
```

- [ ] **Step 2: Generalize the token-store port to support token kind**

Replace the access-token-specific data model with a shared token-record type so the same port can persist and retrieve either access or refresh tokens.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredTokenKind {
    Access,
    Refresh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTokenData {
    pub client_id: String,
    pub kind: StoredTokenKind,
    pub expires_at: SystemTime,
}

#[async_trait]
pub trait TokenStore: Send + Sync {
    async fn store(&self, token: String, data: StoredTokenData) -> Result<(), TokenStoreError>;
    async fn get(&self, token: &str) -> Result<Option<StoredTokenData>, TokenStoreError>;
    async fn revoke(&self, token: &str) -> Result<(), TokenStoreError>;
    async fn cleanup_expired(&self) -> Result<(), TokenStoreError>;
}
```

- [ ] **Step 3: Add refresh-token TTL to config**

Extend `OAuthConfig` so user config carries both lifetimes.

```rust
pub struct OAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub access_token_lifetime_secs: u64,
    pub refresh_token_lifetime_secs: u64,
    pub pkce_required: bool,
    pub username: String,
    pub password: String,
}
```

- [ ] **Step 4: Add any new domain errors only if they are genuinely needed**

Prefer keeping refresh failures mapped to existing OAuth errors:

```rust
OAuthError::UnsupportedGrantType
OAuthError::InvalidGrant("Invalid or expired refresh token".into())
OAuthError::InvalidClient
OAuthError::ServerError("failed to persist refresh token".into())
```

Do **not** add a separate refresh-token-specific error enum unless implementation proves the current error vocabulary is insufficient.

---

### Task 2: Teach `TokenExchangeUseCase` both grant types

**Files:**
- Modify: `crates/core/src/application/token_exchange.rs`

- [ ] **Step 1: Factor common client authentication into a helper inside `TokenExchangeUseCase`**

Extract the preregistered confidential-client validation so both grant paths share one code path.

```rust
fn validate_client(&self, req: &TokenRequest) -> Result<(), OAuthError> {
    if req.client_id != self.config.client_id {
        return Err(OAuthError::InvalidClient);
    }
    if self.config.client_secret.is_empty() {
        return Err(OAuthError::ServerError("client secret not configured".into()));
    }
    if !constant_time_eq(
        req.client_secret.as_bytes(),
        self.config.client_secret.as_bytes(),
    ) {
        return Err(OAuthError::InvalidClient);
    }
    Ok(())
}
```

- [ ] **Step 2: Split `exchange()` by grant type**

Dispatch on `grant_type` instead of rejecting everything except `authorization_code`.

```rust
match req.grant_type.as_str() {
    "authorization_code" => self.exchange_authorization_code(req).await,
    "refresh_token" => self.exchange_refresh_token(req).await,
    _ => Err(OAuthError::UnsupportedGrantType),
}
```

- [ ] **Step 3: Update authorization-code exchange to mint both tokens**

After the existing auth-code validation succeeds, create and persist both a short-lived access token and a long-lived refresh token.

```rust
let access_token = generate_secure_token();
let refresh_token = generate_secure_token();

self.token_store.store(
    access_token.clone(),
    StoredTokenData {
        client_id: req.client_id.clone(),
        kind: StoredTokenKind::Access,
        expires_at: SystemTime::now()
            + Duration::from_secs(self.config.access_token_lifetime_secs),
    },
).await?;

self.token_store.store(
    refresh_token.clone(),
    StoredTokenData {
        client_id: req.client_id.clone(),
        kind: StoredTokenKind::Refresh,
        expires_at: SystemTime::now()
            + Duration::from_secs(self.config.refresh_token_lifetime_secs),
    },
).await?;

Ok(TokenResponse {
    access_token,
    token_type: "bearer".into(),
    expires_in: self.config.access_token_lifetime_secs,
    refresh_token: Some(refresh_token),
})
```

- [ ] **Step 4: Add a refresh-token exchange path with rotation**

Validate the client, require `refresh_token`, load it from the store, verify `kind == Refresh`, verify the stored `client_id` matches, mint replacement tokens, persist them, then revoke the used refresh token.

```rust
let provided_refresh_token = req
    .refresh_token
    .as_deref()
    .filter(|value| !value.is_empty())
    .ok_or_else(|| OAuthError::InvalidRequest("refresh_token required".into()))?;

let stored = self
    .token_store
    .get(provided_refresh_token)
    .await?
    .ok_or_else(|| OAuthError::InvalidGrant("Invalid or expired refresh token".into()))?;

if stored.kind != StoredTokenKind::Refresh {
    return Err(OAuthError::InvalidGrant("Invalid refresh token".into()));
}

if !constant_time_eq(req.client_id.as_bytes(), stored.client_id.as_bytes()) {
    return Err(OAuthError::InvalidGrant("client_id mismatch".into()));
}
```

- [ ] **Step 5: Make rotation safe against partial failure**

Use this order inside refresh exchange:

1. Validate client and stored refresh token.
2. Generate new access token and new refresh token.
3. Persist the new access token.
4. Persist the new refresh token.
5. Revoke the old refresh token.
6. Return the new pair.

If step 5 fails, return `server_error` and leave cleanup to manual investigation plus background expiry cleanup. Do **not** revoke first, because that can strand a client if a later write fails.

---

### Task 3: Keep proxy validation access-token-only

**Files:**
- Modify: `crates/core/src/application/proxy_mcp.rs`

- [ ] **Step 1: Update the proxy use case to handle the generalized token record**

The proxy should continue accepting only bearer access tokens, even though the store now contains both token kinds.

```rust
let token = self
    .token_store
    .get(received_token)
    .await
    .map_err(|_| ProxyError::Unauthorized("invalid token".into()))?;

let token = token.ok_or_else(|| ProxyError::Unauthorized("invalid token".into()))?;

if token.kind != StoredTokenKind::Access {
    return Err(ProxyError::Unauthorized("invalid token".into()));
}

if token.expires_at <= SystemTime::now() {
    return Err(ProxyError::Unauthorized("token expired".into()));
}
```

- [ ] **Step 2: Preserve the current security boundary**

Do not add any route or behavior that allows refresh tokens to reach MCP upstream requests. Refresh tokens are only for `/oauth/token` refresh exchanges.

---

### Task 4: Extend the SQLite token store schema without adding a second store

**Files:**
- Modify: `crates/platform/src/token_store/sqlite.rs`

- [ ] **Step 1: Add `kind` to the schema**

Update schema creation so one table can hold both token types.

```sql
CREATE TABLE IF NOT EXISTS access_tokens (
    token TEXT PRIMARY KEY,
    client_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    expires_at INTEGER NOT NULL
)
```

- [ ] **Step 2: Add a lightweight migration path for existing local databases**

Because existing users already have a table without `kind`, detect that shape and add the column at startup before normal queries run.

```sql
ALTER TABLE access_tokens ADD COLUMN kind TEXT NOT NULL DEFAULT 'access'
```

Use a pragmatic startup check such as:

```rust
let has_kind = sqlx::query_scalar::<_, i64>(
    "SELECT COUNT(*) FROM pragma_table_info('access_tokens') WHERE name = 'kind'"
)
.fetch_one(&self.pool)
.await?;

if has_kind == 0 {
    sqlx::query(
        "ALTER TABLE access_tokens ADD COLUMN kind TEXT NOT NULL DEFAULT 'access'"
    )
    .execute(&self.pool)
    .await?;
}
```

- [ ] **Step 3: Store and load token kind**

Write the `kind` column on insert and decode it on reads.

```rust
let kind = match row.try_get::<String, _>("kind")?.as_str() {
    "access" => StoredTokenKind::Access,
    "refresh" => StoredTokenKind::Refresh,
    other => {
        return Err(TokenStoreError::Unavailable(format!(
            "unknown token kind '{other}' in sqlite store"
        )));
    }
};
```

- [ ] **Step 4: Keep cleanup generic**

`cleanup_expired()` should continue deleting expired rows without caring about token kind.

```rust
sqlx::query("DELETE FROM access_tokens WHERE expires_at < ?1")
```

---

### Task 5: Add configurable refresh-token TTL to config and setup flows

**Files:**
- Modify: `crates/platform/src/config/env_file.rs`
- Modify: `crates/platform/src/setup/env_writer.rs`
- Modify: `crates/core/src/domain/setup.rs`
- Modify: `crates/core/src/application/first_run_setup.rs`
- Modify: `crates/platform/tests/setup_bootstrap.rs`
- Check: any embedded `.env` template used by `env_writer`

- [ ] **Step 1: Load the new env var with a 90-day default**

Mirror the existing access-token TTL behavior with a separate config field.

```rust
let refresh_token_lifetime_secs = env_var_or(
    "B3_OAUTH2_REFRESH_TOKEN_LIFETIME_SECS",
    &DEFAULT_REFRESH_TOKEN_LIFETIME_SECS.to_string(),
)
.parse::<u64>()
.map_err(|e| ConfigError::Invalid(format!(
    "B3_OAUTH2_REFRESH_TOKEN_LIFETIME_SECS: {e}"
)))?;

if refresh_token_lifetime_secs == 0 {
    return Err(ConfigError::Invalid(
        "B3_OAUTH2_REFRESH_TOKEN_LIFETIME_SECS must be greater than 0".into(),
    ));
}
```

- [ ] **Step 2: Thread the value into `OAuthConfig` and setup drafts**

Add `refresh_token_lifetime_secs` anywhere setup defaults or rendered env files already manage `access_token_lifetime_secs`.

```rust
pub const DEFAULT_REFRESH_TOKEN_LIFETIME_SECS: u64 = 90 * 24 * 60 * 60;
```

- [ ] **Step 3: Emit the new env var in setup output**

Update env rendering so first-run setup writes:

```dotenv
B3_OAUTH2_REFRESH_TOKEN_LIFETIME_SECS="7776000"
```

- [ ] **Step 4: Keep defaults user-configurable but conservative**

Do not make refresh TTL mandatory in existing `.env` files. The loader default should preserve backwards compatibility.

---

### Task 6: Expand HTTP handling and OAuth metadata carefully

**Files:**
- Modify: `crates/platform/src/http/oauth_handlers.rs`

- [ ] **Step 1: Parse refresh-token requests without breaking auth-code requests**

Update form parsing to populate the new optional fields.

```rust
let req = TokenRequest {
    grant_type: form.get("grant_type").cloned().unwrap_or_default(),
    client_id: form.get("client_id").cloned().unwrap_or_default(),
    client_secret: form.get("client_secret").cloned().unwrap_or_default(),
    code: form.get("code").cloned().filter(|s| !s.is_empty()),
    refresh_token: form.get("refresh_token").cloned().filter(|s| !s.is_empty()),
    redirect_uri: form.get("redirect_uri").cloned().filter(|s| !s.is_empty()),
    code_verifier: form.get("code_verifier").cloned().filter(|s| !s.is_empty()),
};
```

- [ ] **Step 2: Return refresh tokens in successful token responses**

Only include the field when present.

```rust
let mut body = serde_json::Map::new();
body.insert("access_token".into(), token_response.access_token.into());
body.insert("token_type".into(), token_response.token_type.into());
body.insert("expires_in".into(), token_response.expires_in.into());
if let Some(refresh_token) = token_response.refresh_token {
    body.insert("refresh_token".into(), refresh_token.into());
}
Json(serde_json::Value::Object(body))
```

- [ ] **Step 3: Advertise the new grant type, but nothing broader**

Update OAuth metadata:

```json
"grant_types_supported": ["authorization_code", "refresh_token"]
```

Do **not** add `client_credentials`, dynamic registration, public clients, or any broader auth method advertisement.

---

### Task 7: Update integration tests around the real SQLite token store

**Files:**
- Modify: `crates/platform/tests/oauth_integration.rs`

- [ ] **Step 1: Update test harness config**

Any place constructing `OAuthConfig` must include `refresh_token_lifetime_secs`.

```rust
oauth: OAuthConfig {
    client_id: CLIENT_ID.into(),
    client_secret: CLIENT_SECRET.into(),
    access_token_lifetime_secs: 3600,
    refresh_token_lifetime_secs: 90 * 24 * 60 * 60,
    pkce_required: true,
    username: LOGIN_USERNAME.into(),
    password: LOGIN_PASSWORD.into(),
},
```

- [ ] **Step 2: Update existing token-response assertions**

Tests that already call `/oauth/token` for `authorization_code` should now assert a `refresh_token` is present.

```rust
assert!(body["refresh_token"].as_str().is_some());
```

- [ ] **Step 3: Add the core refresh-flow integration test**

Cover the happy path end to end:

1. Exchange auth code for `access_token_1` and `refresh_token_1`.
2. Call `/oauth/token` with `grant_type=refresh_token` and `refresh_token_1`.
3. Assert response contains `access_token_2`, `refresh_token_2`, and `expires_in`.
4. Assert `access_token_2 != access_token_1` and `refresh_token_2 != refresh_token_1`.
5. Assert `refresh_token_1` no longer works.
6. Assert `access_token_2` is accepted by the MCP proxy.

- [ ] **Step 4: Add rejection tests for bad refresh requests**

Add integration coverage for:

```text
- unknown refresh token -> invalid_grant
- expired refresh token -> invalid_grant
- wrong client secret -> invalid_client
- wrong client_id for stored refresh token -> invalid_grant
- missing refresh_token form field with grant_type=refresh_token -> invalid_request
```

- [ ] **Step 5: Keep tests integration-style and focused**

Do not add unit tests for logging, parsing minutiae, or private helpers. The public surface to test is still `/oauth/token` and MCP proxy authorization behavior.

---

### Task 8: Verify backward compatibility and migration behavior

**Files:**
- Test: `crates/platform/tests/oauth_integration.rs`
- Test: `crates/platform/tests/setup_bootstrap.rs`

- [ ] **Step 1: Add a regression test for existing SQLite schema upgrade**

Create an in-memory or temp SQLite database with the old three-column `access_tokens` table, initialize `SqliteTokenStore`, then verify storing and reading a refresh token succeeds after startup migration.

- [ ] **Step 2: Add a regression test for config defaults**

Verify config loading still succeeds when `B3_OAUTH2_REFRESH_TOKEN_LIFETIME_SECS` is absent and the default 90-day TTL is applied.

- [ ] **Step 3: Run focused verification commands**

Run:

```bash
cargo test -p brain3-platform oauth_integration -- --nocapture
cargo test -p brain3-platform setup_bootstrap -- --nocapture
cargo test -p brain3-core token_exchange -- --nocapture
```

Expected:

```text
test result: ok
```

If there is no standalone `brain3-core` token-exchange test target yet, skip the last command rather than creating low-value tests only to satisfy the plan.

---

## Risks and notes

- The only materially tricky part is **SQLite schema migration for existing local DBs**. The plan keeps it small by using a single-column additive migration instead of introducing a second table or a full migration framework.
- Refresh-token rotation adds a **partial-failure edge** if the old token cannot be revoked after new tokens are stored. The plan favors client continuity over perfect atomicity and relies on expiry plus logs to clean up the rare residual old token.
- Keeping one `TokenStore` port is simpler than adding a separate `RefreshTokenStore`, and it fits the current codebase better because the existing app wiring already shares one store across token issuance and proxy validation.
