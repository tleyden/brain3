# Oxide-Auth Follow-Up Implementation Plan

**Goal:** Remove the remaining legacy OAuth/token abstractions and finish with a single oxide-auth-based gateway design: oxide-auth for authorize/token flows, SQLite-backed oxide token issuance and recovery, and platform-owned bearer validation for MCP.

**Architecture:** Keep the code-grant flow on oxide-auth primitives end-to-end. Use one oxide-native registrar implementation for both `/oauth/authorize` and `/oauth/token`, keep authorization codes in `AuthMap`, store issued access and refresh tokens in a SQLite-backed oxide `Issuer`, and validate MCP bearer tokens at the platform HTTP boundary before delegating to a simplified core proxy use case.

**Tech Stack:** Rust, Axum 0.8, oxide-auth 0.6, oxide-auth-async 0.2, oxide-auth-axum 0.6, sqlx SQLite, Tokio

---

## Direction Change

This plan assumes:

- no backward-compatibility work for existing users or existing token DB contents
- no attempt to preserve legacy abstractions just because they already exist
- no dual-path design where oxide-auth issues tokens but old ports still validate them
- no `TokenStore` port after this work unless a genuinely oxide-free core boundary still needs one

This is now a greenfield cleanup pass over a partially migrated codebase.

## Target End State

After this work, the OAuth/MCP stack should look like this:

1. `GatewayRegistrar` is the only registrar used by the gateway.
2. `AuthMap<RandomGenerator>` remains the authorization-code store.
3. `SqliteTokenStore` becomes the oxide-auth `Issuer` for access and refresh tokens.
4. `/oauth/token` reads and writes only through that oxide issuer.
5. `/mcp` bearer validation happens in platform by parsing the bearer token and calling `recover_token()` on the shared issuer.
6. `ProxyMcpUseCase` no longer knows what a bearer token is and no longer depends on token storage.
7. All legacy token-store types, adapters, and helper functions are deleted.

## Explicit Design Decisions

### 1. Keep auth codes in memory

We are **not** trying to persist authorization codes. `AuthMap` is already an oxide primitive, it is simple, and it is good enough for current scope.

### 2. Replace the split registrar setup

The current split between `BrainRegistrar` for authorize and `ClientMap` for token is an interim migration artifact. The follow-up should replace that with one custom oxide `Registrar` implementation that:

- accepts exactly one configured `client_id`
- validates the configured `client_secret` in `check()`
- accepts the runtime `redirect_uri` presented by the client during authorize
- binds that same redirect URI into the grant
- returns the configured default scope

This keeps the security model intact while removing the static fake redirect URI hack now living in `ClientMap`.

### 3. Use a new schema, not compatibility ALTERs

Because we are treating this as greenfield, the new SQLite issuer should use a fresh schema shape for oxide grants instead of continuing to evolve the legacy `access_tokens` table with compatibility `ALTER TABLE` logic.

Recommended approach:

- create a new table, for example `oauth_tokens`
- stop reading from the old `access_tokens` table entirely
- optionally drop the old table during initialization if it simplifies reasoning

The important point is: do not spend effort on schema migration complexity for a table we no longer conceptually want.

### 4. Move OAuth mechanics out of core

Core should not parse `Authorization` headers, should not validate bearer syntax, and should not do token lookup. Those are HTTP/OAuth concerns and belong in platform.

Core should keep:

- host validation logic if we still want that shared policy there
- MCP request forwarding and header filtering
- proxy error modeling

Core should lose:

- `TokenStore` dependency
- token-kind checks
- token-expiry checks
- bearer parsing helpers

## Current Code That Must Go Away

These are not “maybe” cleanups. They are the explicit deletion list unless something unexpected blocks it.

### Delete

- `crates/platform/src/token_store/token_map.rs`
- `crates/core/src/ports/token_store.rs`

### Remove usage from

- `apps/gateway/src/server.rs`
- `crates/platform/tests/oauth_integration.rs`
- `crates/core/src/application/proxy_mcp.rs`
- `crates/core/src/application/validate_request.rs`
- `crates/platform/src/token_store/mod.rs`

### Likely rename/simplify

- `crates/platform/src/http/registrar.rs`
  - from migration-specific `BrainRegistrar` wording to a registrar that represents the actual gateway design

## File-Level End State

### `crates/platform/src/http/registrar.rs`

Should contain one oxide-auth registrar implementation for the gateway, likely renamed from `BrainRegistrar` to something like `GatewayRegistrar`.

Responsibilities:

- validate configured confidential client identity
- accept dynamic redirect URI at authorize time
- validate client secret at token time
- negotiate scope

Should not depend on `ClientMap`.

### `crates/platform/src/token_store/sqlite.rs`

Should become the single token authority for issued tokens.

Responsibilities:

- initialize the new OAuth token schema
- implement oxide-auth `Issuer`
- expose `cleanup_expired()`
- reconstruct grants from DB rows

Should not implement the legacy `TokenStore` port.

### `crates/platform/src/http/state.rs`

Should hold:

- `registrar: Arc<GatewayRegistrar>`
- `authorizer: Arc<Mutex<AuthMap<RandomGenerator>>>`
- `issuer: Arc<Mutex<SqliteTokenStore>>`
- proxy use case
- config
- rate limiter

Should not hold a separate `token_registrar`.

### `crates/platform/src/http/oauth_handlers.rs`

Should:

- use the single registrar for both authorize and token flows
- use SQLite issuer in the token flow
- keep the existing request-shape/error normalization only if still needed after the refactor

Should not know about `TokenMap`.

### `crates/platform/src/http/mcp_handlers.rs`

Should:

- parse bearer token from request headers
- validate token against the shared oxide issuer
- reject malformed, unknown, expired, or wrong-kind tokens before calling core
- call the simplified proxy use case without passing auth state

### `crates/core/src/application/proxy_mcp.rs`

Should become a pure authenticated request forwarder.

Responsibilities:

- host validation
- upstream URL assembly
- request/response header filtering
- forwarding to upstream adapter

Should not:

- read auth headers
- parse bearer tokens
- query token storage
- compare expiry times

## Data Model

Recommended SQLite table shape:

`oauth_tokens`

- `token TEXT PRIMARY KEY`
- `kind TEXT NOT NULL`
  - values: `access` or `refresh`
- `owner_id TEXT NOT NULL`
- `client_id TEXT NOT NULL`
- `redirect_uri TEXT NOT NULL`
- `scope TEXT NOT NULL`
- `expires_at INTEGER NOT NULL`

Notes:

- store one row per token
- reconstruct `Grant` from the non-token columns
- `kind='access'` backs `recover_token()`
- `kind='refresh'` backs `recover_refresh()`
- `refresh(old, new_grant)` should remove the old refresh token and write the rotated tokens expected by oxide-auth

If oxide-auth requires issuing both access and refresh tokens together, encode that directly in the SQLite implementation rather than via an intermediate app-specific type.

## Detailed Execution Plan

## Task 1: Consolidate registrars into one oxide-native gateway registrar

**Files:**
- Modify: `crates/platform/src/http/registrar.rs`
- Modify: `crates/platform/src/http/state.rs`
- Modify: `apps/gateway/src/server.rs`
- Modify: `crates/platform/tests/oauth_integration.rs`

**Work:**
- Replace the current `BrainRegistrar`/`ClientMap` split with one gateway registrar type.
- Implement all three relevant `Registrar` behaviors in that one type:
  - `bound_redirect()`
  - `negotiate()`
  - `check()`
- Remove `ClientMap` from app state and test harness state.
- Remove the fake static registered callback URL from gateway wiring.
- Update tests and state constructors to pass the new registrar everywhere.

**Review focus:**
- Does this still preserve the preregistered confidential-client model?
- Are we comfortable accepting any redirect URI for the configured client, as long as it is round-tripped into the grant and checked again during token exchange?

## Task 2: Rewrite `SqliteTokenStore` as the oxide issuer

**Files:**
- Modify: `crates/platform/src/token_store/sqlite.rs`

**Work:**
- Remove the legacy `TokenStore` trait implementation.
- Create the new OAuth-token schema for oxide grants.
- Add small private helpers for:
  - serializing scope
  - parsing scope back into oxide `Scope`
  - converting DB rows into `Grant`
  - mapping token kind strings
- Implement oxide-auth `Issuer` methods:
  - `issue(grant)`
  - `recover_token(token)`
  - `recover_refresh(token)`
  - `refresh(refresh_token, grant)`
- Keep `cleanup_expired()` as an inherent async method.

**Review focus:**
- Whether refresh rotation semantics exactly match what oxide-auth expects.
- Whether we want a dedicated `struct TokenRow` helper for clarity.

## Task 3: Rewire gateway runtime and handler state to the SQLite issuer

**Files:**
- Modify: `apps/gateway/src/server.rs`
- Modify: `crates/platform/src/http/state.rs`
- Modify: `crates/platform/tests/oauth_integration.rs`
- Modify: any platform module exports needed for construction

**Work:**
- Replace `TokenMap<RandomGenerator>` issuer wiring with `SqliteTokenStore`.
- Keep `AuthMap<RandomGenerator>` only for the authorizer.
- Build the shared SQLite issuer from `config.token_db_path` in production wiring.
- Build an isolated in-memory SQLite issuer in tests.
- Remove `TokenMapStore` from all runtime and test wiring.

**Review focus:**
- Whether handler state needs `Arc<Mutex<SqliteTokenStore>>` or whether a different sharing strategy is cleaner with oxide-auth’s trait shape.

## Task 4: Move bearer validation to platform and delete auth parsing from core

**Files:**
- Modify: `crates/platform/src/http/mcp_handlers.rs`
- Modify: `crates/platform/src/http/state.rs`
- Modify: `crates/core/src/application/proxy_mcp.rs`
- Modify: `crates/core/src/application/validate_request.rs`

**Work:**
- In platform:
  - parse the `Authorization` header
  - require `Bearer <token>`
  - call `recover_token()` on the shared issuer
  - reject missing, malformed, unknown, expired, or wrong-kind tokens
- In core:
  - remove `auth_header` from `ProxyMcpUseCase::handle()`
  - remove token lookup and expiry logic
  - keep host validation and forwarding logic
- Keep external HTTP behavior stable for MCP clients:
  - still return `401`
  - still include `resource_metadata=` in `WWW-Authenticate`

**Review focus:**
- Whether to use `OAuthResource` as an extractor, or keep explicit header parsing for clarity and tighter control over the error response shape.

## Task 5: Delete legacy token abstractions and modules

**Files:**
- Delete: `crates/platform/src/token_store/token_map.rs`
- Modify: `crates/platform/src/token_store/mod.rs`
- Delete: `crates/core/src/ports/token_store.rs`
- Modify: `crates/core/src/application/proxy_mcp.rs`
- Modify: any imports, constructors, and tests referencing those modules

**Work:**
- Remove `TokenMapStore`.
- Remove `StoredTokenKind`.
- Remove `StoredTokenData`.
- Remove the `TokenStore` port.
- Remove any dead error variants or imports created only for that port.

**Review focus:**
- Verify nothing outside the OAuth/MCP path still uses those types before deletion.

## Task 6: Simplify cleanup and maintenance paths

**Files:**
- Modify: `crates/platform/src/token_store/sqlite.rs`
- Modify: any runtime/bootstrap code that performs token cleanup

**Work:**
- Make cleanup call `SqliteTokenStore::cleanup_expired()` directly.
- Remove any cleanup plumbing shaped around the deleted `TokenStore` trait.
- If no cleanup scheduler actually exists today, document that and avoid inventing one unless required by current runtime behavior.

**Review focus:**
- Prefer less machinery here. Only add scheduling code if something already expects token cleanup to happen in-process.

## Task 7: Tighten and rebaseline tests

**Files:**
- Modify: `crates/platform/tests/oauth_integration.rs`
- Add only if necessary: focused public-behavior tests around `SqliteTokenStore`

**Work:**
- Rewire the test harness to use the single registrar and SQLite issuer.
- Keep tests focused on public behavior:
  - valid authorize -> token -> MCP flow
  - client secret required
  - wrong client secret rejected
  - wrong verifier rejected
  - missing verifier rejected
  - code single-use enforced
  - expired token rejected by MCP
  - valid token accepted by MCP
- Avoid tests for private SQL helpers or logging.

**Review focus:**
- Add only the minimum new tests needed to pin the new end state.

## Task 8: Final codebase cleanup

**Files:**
- Modify as needed:
  - `crates/platform/Cargo.toml`
  - `crates/core/Cargo.toml`
  - module export files
  - imports across touched files

**Work:**
- Remove unused dependencies and imports left behind by the migration.
- Re-check whether the token error normalization in `oauth_handlers.rs` is still necessary after the registrar/issuer cleanup.
- Rename comments and identifiers that still describe the “migration” state rather than the intended architecture.

## Verification Checklist

Required before calling the work done:

- `cargo test -p brain3-platform --test oauth_integration`
- `cargo test`

Manual code review checklist:

- no `TokenStore` port remains
- no `TokenMapStore` remains
- no `ClientMap` placeholder callback wiring remains in gateway runtime
- core MCP proxy path is bearer-token agnostic
- platform is the only layer that knows bearer token syntax and oxide issuer recovery

## Main Risks

### Refresh-token semantics

This is the one place where oxide-auth’s expectations matter more than our old code. The implementation should follow the trait contract exactly instead of trying to preserve any old token-store shape.

### Hidden remaining `TokenStore` consumers

If something else in core still depends on the old port, we need to decide whether it is truly part of OAuth token handling. If yes, it should probably be deleted or moved; if no, it may deserve a new narrower abstraction with a different name.

### Over-refactoring during cleanup

The point is to delete the legacy path, not to redesign unrelated modules. Keep the blast radius focused on OAuth, MCP authentication, and gateway wiring.

## Definition of Done

- one oxide-native registrar is used across gateway OAuth flows
- `SqliteTokenStore` is the only issued-token authority
- MCP bearer validation uses oxide issuer recovery in platform
- core proxy use case no longer depends on token storage or auth-header parsing
- legacy token-store modules and types are deleted
- gateway/runtime/tests no longer reference `TokenMapStore`
- `cargo test -p brain3-platform --test oauth_integration` passes
- `cargo test` passes
