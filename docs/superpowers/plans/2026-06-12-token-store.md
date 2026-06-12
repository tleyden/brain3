Plan: Per-session Time-Limited Access Tokens (Security Audit 1.1)

### Overview

Replace the static `access_token` env var with dynamically issued, time-limited bearer tokens. On each successful OAuth code exchange a fresh token is generated, stored in SQLite, and validated against the store on every MCP proxy request. The static `B3_OAUTH2_GATEWAY_ACCESS_TOKEN` env var is removed entirely.

---

### Step 1 — Define the `TokenStore` port in `crates/core`

**File:** `crates/core/src/ports/token_store.rs`

```rust
pub struct AccessTokenData {
    pub client_id: String,
    pub expires_at: std::time::SystemTime,
}

#[async_trait]
pub trait TokenStore: Send + Sync {
    async fn store(&self, token: String, data: AccessTokenData) -> Result<(), TokenStoreError>;
    async fn get(&self, token: &str) -> Result<Option<AccessTokenData>, TokenStoreError>;
    async fn revoke(&self, token: &str) -> Result<(), TokenStoreError>;
    async fn cleanup_expired(&self) -> Result<(), TokenStoreError>;
}
```

`TokenStoreError` is a small domain error enum in `crates/core/src/domain/errors.rs` (no sqlx types leak in).

---

### Step 2 — Update `TokenExchangeUseCase` to issue and store dynamic tokens

**File:** `crates/core/src/application/token_exchange.rs`

- Add `Arc<dyn TokenStore>` field (or generic `TS: TokenStore`).
- Remove `self.config.access_token.clone()`.
- Call `generate_secure_token()` (already in `oauth.rs`), compute `expires_at = now + ACCESS_TOKEN_LIFETIME_SECS`, call `token_store.store(token, data).await`, return the fresh token in `TokenResponse`.

---

### Step 3 — Update `ProxyMcpUseCase` to validate against the store

**File:** `crates/core/src/application/proxy_mcp.rs`

- Replace the `access_token: String` field with `Arc<dyn TokenStore>` (or generic).
- In `handle()`, call `token_store.get(received_token).await`, check `expires_at > now`. On miss/expiry return `ProxyError::Unauthorized`.
- Remove `validate_bearer_token` comparison against a static string; the store lookup *is* the validation.
- Remove the `access_token()` accessor — it no longer makes sense.

---

### Step 4 — Implement `SqliteTokenStore` in `crates/platform`

**New file:** `crates/platform/src/token_store/sqlite.rs`

- Add `sqlx = { version = "0.8", features = ["sqlite", "runtime-tokio", "time"] }` to `crates/platform/Cargo.toml`.
- Schema (created at startup via `sqlx::migrate!` or inline `CREATE TABLE IF NOT EXISTS`):

```sql
CREATE TABLE IF NOT EXISTS access_tokens (
    token      TEXT PRIMARY KEY,
    client_id  TEXT NOT NULL,
    expires_at INTEGER NOT NULL  -- unix timestamp seconds
);
```

- `store`: `INSERT OR REPLACE`.
- `get`: `SELECT` + check not expired.
- `revoke`: `DELETE`.
- `cleanup_expired`: `DELETE WHERE expires_at < now`.
- DB file path comes from config (e.g., `B3_TOKEN_DB_PATH`, default `~/.brain3/tokens.db`).

---

### Step 5 — Wire everything together in `AppState` / `main.rs`

**Files:** `crates/platform/src/http/state.rs`, `apps/gateway/src/main.rs`

- Construct `SqliteTokenStore`, wrap in `Arc`, inject into both `TokenExchangeUseCase` and `ProxyMcpUseCase`.
- `AppState` gains a `TS2: TokenStore` generic parameter, or both use cases share the same `Arc<dyn TokenStore>` (the simpler path — use `Arc<dyn TokenStore>` not a new generic to avoid blowing up the type parameters).
- Spawn a background Tokio task that calls `token_store.cleanup_expired()` every ~5 min.

---

### Step 6 — Remove static `access_token` from config and env

**Files:** `crates/core/src/domain/model.rs`, `crates/platform/src/config/env_file.rs`, `crates/platform/src/setup/env_writer.rs`, `crates/core/src/application/first_run_setup.rs`, `crates/core/src/domain/setup.rs`

- Drop `access_token` field from `OAuthConfig` and `SetupDraft`.
- Remove `B3_OAUTH2_GATEWAY_ACCESS_TOKEN` from env loading and env writer.
- Update `first_run_setup.rs` which currently generates and stores this value.

---

### Step 7 — Update integration tests

**File:** `crates/platform/tests/oauth_integration.rs`

- Replace any `InMemory` token store stub with a real `SqliteTokenStore` backed by an in-memory SQLite (`sqlite::memory:` URL) or a temp-file DB, so tests stay integration-style without hitting disk.
- Verify that a token returned by `/oauth/token` is accepted by the MCP proxy handler, and that an expired/unknown token is rejected.

---

### Key design decisions

| Decision | Rationale |
|---|---|
| `Arc<dyn TokenStore>` over a new generic | Avoids adding a 3rd type parameter to `AppState` and all the downstream bounds |
| SQLite via `sqlx` | Fits the existing `sqlx` best-practice called out in `AGENTS.MD`; zero extra infra |
| In-memory SQLite for tests | Keeps tests fast and self-contained; no temp-file cleanup needed |
| Port defined in `core`, impl in `platform` | Follows hexagonal rule — core is ignorant of sqlx |
| Static `access_token` fully removed | Eliminates the attack surface; no fallback to the old behaviour |

---

Ready to implement once you approve. Which step do you want to start with?