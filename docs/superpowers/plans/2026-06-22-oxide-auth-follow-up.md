# Oxide-Auth Follow-Up Implementation Plan

**Goal:** Finish the oxide-auth migration by moving access-token persistence and MCP bearer validation onto a single SQLite-backed oxide-auth backend, then remove the remaining legacy `TokenStore`-based path.

**Architecture:** Keep the authorization-code step in memory via `AuthMap`, but make issued access and refresh tokens durable and authoritative in `SqliteTokenStore` by implementing oxide-auth’s `Issuer` interface there. Move bearer-token validation to the platform HTTP boundary so core stays unaware of OAuth mechanics and only handles validated MCP forwarding.

**Tech Stack:** Rust, Axum 0.8, oxide-auth 0.6, oxide-auth-async 0.2, oxide-auth-axum 0.6, sqlx SQLite, Tokio

---

## Current Status

Completed enough to be functional:
- `/oauth/authorize` uses oxide-auth with `BrainRegistrar` and `AuthMap`.
- `/oauth/token` uses oxide-auth `AccessTokenFlow`.
- OAuth integration tests are green again after fixing encoded auth-code parsing and normalizing oxide-auth’s `invalid_request` edge cases.

Still unfinished from the original migration:
- Gateway runtime still wires `TokenMap<RandomGenerator>` and `TokenMapStore` instead of a SQLite-backed oxide issuer.
- MCP bearer validation still depends on the legacy core `TokenStore` abstraction and `validate_bearer_token`.
- `SqliteTokenStore` is still shaped like the old token-store port, not oxide-auth’s issuer contract.
- Legacy token-store types and adapters are still live in core and platform.

## File Map

Primary files to modify:
- `crates/platform/src/token_store/sqlite.rs`
- `crates/platform/src/http/state.rs`
- `crates/platform/src/http/oauth_handlers.rs`
- `crates/platform/src/http/mcp_handlers.rs`
- `apps/gateway/src/server.rs`
- `crates/platform/tests/oauth_integration.rs`

Likely files to simplify or delete:
- `crates/platform/src/token_store/token_map.rs`
- `crates/core/src/ports/token_store.rs`
- `crates/core/src/application/proxy_mcp.rs`
- `crates/core/src/application/validate_request.rs`

Possible supporting files:
- `crates/platform/src/token_store/mod.rs`
- `crates/core/src/domain/errors.rs`
- any runtime/bootstrap wiring that currently expects a `TokenStore`

## Recommended Approach

Use the original Phase 3 goal, but split it into two serial slices:

1. **Persistence slice:** make `SqliteTokenStore` the real oxide-auth issuer and router dependency, without changing MCP behavior yet.
2. **Validation slice:** move bearer validation from core into platform, then delete the old token-store abstraction.

This is the safest cut because it keeps one moving part per slice and preserves the hexagonal boundary: platform owns OAuth details, core owns MCP proxy behavior.

## Task 1: Convert `SqliteTokenStore` into the oxide-auth issuer backend

**Files:**
- Modify: `crates/platform/src/token_store/sqlite.rs`

**Plan:**
- Extend the SQLite schema so an issued token record can reconstruct an oxide-auth `Grant`.
- Preserve the existing `access_tokens` table name unless there is a strong reason to rename it.
- Add the columns needed to rebuild grants from storage:
  - `owner_id`
  - `redirect_uri`
  - `scope`
  - keep `kind`
  - keep `expires_at`
- Verify whether refresh-token persistence also needs an explicit `refresh_token` row shape or whether the existing `kind` split is enough. The goal is to support both `recover_token()` and `recover_refresh()`.
- Implement oxide-auth `Issuer` on `SqliteTokenStore`:
  - `issue(grant)`
  - `recover_token(token)`
  - `recover_refresh(token)`
  - `refresh(refresh_token, grant)`
- Keep `cleanup_expired()` as an inherent method on `SqliteTokenStore`.
- Do not add new abstractions here unless the SQL code becomes genuinely awkward.

**Notes:**
- Be careful about migration order in `ensure_schema()`. This needs to work on an existing database created by the older token-store schema.
- If grant reconstruction needs a small helper type for row decoding, keep it local to the platform crate.

## Task 2: Replace in-memory `TokenMap` issuance with SQLite issuance in runtime wiring

**Files:**
- Modify: `crates/platform/src/http/state.rs`
- Modify: `apps/gateway/src/server.rs`
- Modify: `crates/platform/tests/oauth_integration.rs`
- Possibly modify: platform module exports for token-store construction

**Plan:**
- Change app state so `/oauth/token` no longer depends on `TokenMap<RandomGenerator>`.
- Construct a shared `SqliteTokenStore` from `config.token_db_path` in gateway wiring.
- Pass that shared store into the token endpoint as the `Issuer`.
- Remove `TokenMapStore` from gateway wiring and test harnesses.
- Update integration tests to use an isolated SQLite-backed issuer, preferably in-memory where possible.

**Notes:**
- The auth-code store can remain `AuthMap<RandomGenerator>`. The unfinished work is about issued tokens, not auth-code persistence.
- Keep `apps/gateway/src/main.rs` lean by doing construction in platform/gateway wiring, not by adding logic to `main.rs`.

## Task 3: Move MCP bearer validation into platform HTTP handlers

**Files:**
- Modify: `crates/platform/src/http/mcp_handlers.rs`
- Modify: `crates/platform/src/http/state.rs`
- Modify: `crates/core/src/application/proxy_mcp.rs`
- Modify: `crates/core/src/application/validate_request.rs`

**Plan:**
- Stop making core parse and look up bearer tokens.
- Validate bearer tokens in platform before calling the core MCP proxy use case.
- Recommended implementation shape:
  - read the `Authorization` header at the platform handler boundary
  - parse `Bearer <token>`
  - recover the grant through the shared SQLite-backed oxide issuer
  - reject missing, malformed, expired, or non-access tokens there
- After validation succeeds, call `ProxyMcpUseCase` with an already-authenticated request.
- Keep host-validation behavior unchanged.

**Notes:**
- The original plan called out `OAuthResource`. That is still a reasonable option if it fits cleanly with the current handler signature, but the important architectural point is platform-owned bearer validation, not the specific extractor type.
- Do not let OAuth-specific types leak into core APIs.

## Task 4: Delete the legacy token-store path

**Files:**
- Delete or stop using: `crates/platform/src/token_store/token_map.rs`
- Delete or simplify: `crates/core/src/ports/token_store.rs`
- Modify: `crates/core/src/application/proxy_mcp.rs`
- Modify: `crates/core/src/domain/errors.rs`
- Modify any constructors and tests that still depend on `TokenStore`

**Plan:**
- Remove `StoredTokenKind`, `StoredTokenData`, and the `TokenStore` port if nothing still needs them after Task 3.
- Remove `TokenMapStore`.
- Remove the old `validate_bearer_token()` helper if its only remaining use was MCP auth.
- Re-check whether any remaining code still needs a minimal token-related abstraction; if not, delete it rather than renaming it.

**Notes:**
- This is the right cleanup point because Tasks 1 through 3 replace the functionality with a single oxide-auth source of truth.

## Task 5: Reconcile cleanup and background maintenance

**Files:**
- Modify any runtime/bootstrap code that schedules token cleanup
- Modify: `crates/platform/src/token_store/sqlite.rs`

**Plan:**
- Find the current cleanup trigger path and make it call `SqliteTokenStore::cleanup_expired()` directly.
- Remove any background cleanup plumbing that still assumes the old `TokenStore` trait.
- Ensure cleanup removes both expired access and refresh tokens.

**Notes:**
- This is small, but it is part of the original Phase 3 intent and should not be left half-migrated.

## Task 6: Test the migration seams, not internal details

**Files:**
- Modify: `crates/platform/tests/oauth_integration.rs`
- Add only if needed: focused tests around `SqliteTokenStore` public behavior

**Plan:**
- Keep tests focused on public behavior:
  - authorization-code exchange still works
  - token reuse fails
  - wrong verifier fails
  - MCP accepts valid bearer tokens
  - MCP rejects expired or missing bearer tokens
- Add migration coverage for `SqliteTokenStore::ensure_schema()` if there is not already a public-facing way to exercise old-schema upgrade behavior.
- Do not add tests for logging or private helper functions.
- End with:
  - `cargo test -p brain3-platform --test oauth_integration`
  - `cargo test`

## Task 7: Final cleanup and dependency review

**Files:**
- Modify only if still needed after earlier tasks:
  - `crates/core/Cargo.toml`
  - `crates/platform/Cargo.toml`
  - module exports referencing removed files

**Plan:**
- Remove any now-dead imports, modules, and wiring.
- Re-check whether earlier migration leftovers are still needed:
  - `subtle`, `base64`, `sha2`, `rand`
- Only remove dependencies that are truly unused after the final pass.

## Risks and Guardrails

- **Schema migration risk:** existing databases may only have `token`, `client_id`, `kind`, and `expires_at`. Keep `ensure_schema()` additive and idempotent.
- **Boundary drift risk:** if OAuth validation logic starts leaking into core, stop and move it back to platform.
- **Refresh-token ambiguity:** verify actual current behavior before changing metadata or response shape. If refresh tokens are already part of the public contract, preserve that.
- **Test overreach:** keep new tests on public APIs and core behavior only, per repo guidance.

## Suggested Execution Order

1. Task 1
2. Task 2
3. Task 3
4. Task 4
5. Task 5
6. Task 6
7. Task 7

## Definition of Done

- Gateway no longer constructs `TokenMapStore` for issued-token validation.
- `/oauth/token` and MCP bearer auth both use the same SQLite-backed oxide-auth token source.
- Core no longer owns bearer-token parsing or token lookup.
- Legacy token-store types are removed if unreferenced.
- `cargo test -p brain3-platform --test oauth_integration` passes.
- `cargo test` passes.
