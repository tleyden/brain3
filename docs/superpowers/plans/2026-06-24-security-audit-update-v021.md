# Security Audit Update Plan — v0.1.7 → v0.2.1

**Goal:** Bring `SECURITY_AUDIT.md` and `docs/security/changelog.md` current for the v0.2.1 release. The audit header still reads "Codebase version: 0.1.9" and was authored against 0.1.7 code. The significant changes between those tags and HEAD are the oxide-auth rebase (#105), signed binaries (#106), token refresh (#111), and several supply-chain improvements.

**Approach:** Edit `SECURITY_AUDIT.md` in place. Then add a changelog entry to `docs/security/changelog.md`. No new code changes — this is documentation only.

**Evidence base:** `git diff v0.1.7..HEAD` on all security-relevant files plus inspection of current codebase. Key findings from the diff:
- `crates/core/src/domain/oauth.rs` — **deleted** (held `constant_time_eq`, `AUTH_CODE_LIFETIME`, custom domain types)
- `crates/platform/src/auth_code_store/in_memory.rs` — **deleted** (held `InMemoryAuthCodeStore` with `cleanup_expired`)
- `crates/platform/src/http/oauth_handlers.rs` — **substantially rewritten** (oxide-auth endpoint structs, `check_credentials` using `subtle::ct_eq` directly, `resolve_base_url` unchanged)
- `crates/platform/src/http/mcp_handlers.rs` — **rewritten**: bearer validation now uses `oxide-auth` issuer `recover_token()`; all log calls use `elide_secret()`
- `crates/platform/src/config/upstream_secret.rs` — **unchanged** except a `rand` import; M-10 secret prefix logging is still present
- `crates/platform/src/http/router.rs`, `templates.rs`, `container/startup.rs`, `cloudflare_setup.rs`, `rate_limit.rs` — no security-logic changes

---

## Task 1 — Update audit header and executive summary

**File:** `SECURITY_AUDIT.md` (top of file, lines 1–14)

Changes:
- `**Codebase version:**` → `0.2.1`
- `**Date:**` → `2026-06-24`
- Rewrite the executive summary paragraph to say: as of v0.2.1 the core OAuth authorization-code flow is delegated to the `oxide-auth` library (rebase #105), which eliminates Brain3's custom `constant_time_eq` wrapper (M-4 removed) and its hand-rolled in-memory auth-code store (L-1 removed). The GET `/oauth/authorize` endpoint is sufficiently gated by param validation that rate-limiting is not a meaningful additional control (L-4 removed). No new HIGH findings were introduced. Most impactful remaining open findings: M-1 (host header injection), M-2 (redirect_uri not allowlisted), M-8 (upstream secret in `/tmp`), M-10 (secret prefix logged).

---

## Task 2 — Threat Model — Threat Actors table

**File:** `SECURITY_AUDIT.md` (Threat Actors table, currently lines 51–58)

Two row updates only; diagram and trust-boundary table are already accurate:

1. **"Protocol-logic attacker"** row — update the "Contained?" cell to say: "Partially — oxide-auth now owns core auth-code issuance and code↔token exchange. Brain3-owned surface reduced to: credential gate (`check_credentials`), registrar policy (`GatewayRegistrar`), token persistence (`SqliteTokenStore`), metadata doc construction, and error normalization. See M-13."

2. **"Supply-chain attacker — Rust host dependencies"** row — add to the entry point cell: "`oxide-auth` and `oxide-auth-async` are now in the critical OAuth path (added in #105)." Containment answer remains "No — full host access; see M-12."

---

## Task 3 — Remove three closed findings from the summary table and body

**File:** `SECURITY_AUDIT.md`

Remove the following rows from the Open Findings summary table and delete their full body sections:

### M-4 — `constant_time_eq` leaks secret length via early exit — REMOVE

**Reason:** `crates/core/src/domain/oauth.rs` was deleted in the oxide-auth rebase. The `constant_time_eq` wrapper is gone. All comparisons now use `subtle::ConstantTimeEq::ct_eq()` directly (`registrar.rs:61–63`, `oauth_handlers.rs:336–343`). `subtle`'s slice `ct_eq` does return `Choice(0)` immediately on length mismatch (non-CT for different-length inputs), but this is documented upstream-library behavior, not a Brain3 bug. In practice the client_secret is a fixed 64-char value and password comparison is gated by rate-limiting, making exploitation infeasible. Move closure note to changelog.

### L-1 — No background cleanup of expired auth codes — REMOVE

**Reason:** `InMemoryAuthCodeStore` and its `cleanup_expired()` method were deleted. Auth codes are now managed by oxide-auth's `AuthMap`, which evicts expired codes lazily on next access. This is the library's documented behavior, not a Brain3 concern to fix.

### L-4 — `GET /oauth/authorize` not rate-limited — REMOVE

**Reason:** The GET handler calls `validate_authorize_params()` which rejects any request that lacks the correct `client_id`, `response_type=code`, non-empty `redirect_uri`, and a valid PKCE S256 challenge. Without the configured `client_id` the handler returns 401 before rendering anything. With a valid `client_id` the handler only serves login form HTML — no credentials are processed. All credential processing is POST-only and rate-limited. There is no meaningful attack path through the GET handler that rate-limiting would close.

---

## Task 4 — Update M-3 finding

**File:** `SECURITY_AUDIT.md` — M-3 section

The `AUTH_CODE_LIFETIME = Duration::from_secs(300)` constant was in `crates/core/src/domain/oauth.rs`, which is now deleted. Auth codes are managed by `oxide-auth`'s `AuthMap<RandomGenerator>` (instantiated in `crates/platform/src/http/state.rs`). Brain3 does not configure a custom lifetime, so auth codes use oxide-auth's library default.

Update the finding to:
- Remove the old file reference (`crates/core/src/domain/oauth.rs:10`)
- New file reference: `crates/platform/src/http/state.rs` (AuthMap instantiation) and `crates/platform/Cargo.toml` (oxide-auth version)
- Update the description: the 5-minute constant is gone; Brain3 does not override oxide-auth's auth-code lifetime. The finding stays open because Brain3 could and should configure a shorter lifetime (60 s) via oxide-auth's `AuthMap` constructor, but currently does not.
- The "no session binding" clause remains unchanged.

---

## Task 5 — Update M-13 finding

**File:** `SECURITY_AUDIT.md` — M-13 section

M-13 needs a substantial rewrite to accurately reflect the post-rebase picture. Replace the body with:

**What oxide-auth now owns (no longer Brain3 code):**
- Auth-code issuance and storage (`AuthMap<RandomGenerator>`)
- PKCE S256 verification (via `AddonList` + `Pkce` extension in `AuthorizeEndpoint`)
- Core authorize and code-exchange flows (`AuthorizationFlow::prepare/execute`, `AccessTokenFlow`)
- Token recovery for bearer validation (`issuer.recover_token()` in `mcp_handlers.rs`)

**What Brain3 still owns (the remaining trusted surface):**
- Credential gate: `check_credentials()` in `oauth_handlers.rs` — validates username/password before handing off to oxide-auth
- Registrar policy: `GatewayRegistrar` in `registrar.rs` — decides how `client_id`, `client_secret`, and runtime `redirect_uri` are accepted
- Token persistence: `SqliteTokenStore` in `sqlite.rs` — issues access/refresh tokens and performs refresh-token rotation
- Metadata doc construction: `oauth_metadata()` and `protected_resource_metadata()` in `oauth_handlers.rs` — still uses `resolve_base_url()` (see M-1)
- Error normalization: `normalize_token_error_response()` in `oauth_handlers.rs`
- Request adaptation: `PostBodyRequest`, `BodyRefreshRequest` — maps HTTP bodies into oxide-auth trait objects

The concrete security issues M-1 and M-2 still live entirely in this Brain3-owned surface. Update the "Recommendation" to: keep the Brain3-owned surface small and fix M-1 and M-2; the oxide-auth rebase already meaningfully reduced the surface.

---

## Task 6 — Update L-10 finding

**File:** `SECURITY_AUDIT.md` — L-10 section

Add an inline note at the top of the finding:

> **Partial progress (#91):** A "Reporting Security Issues" section was added to `README.MD` pointing to GitHub's private vulnerability advisory system (`https://github.com/tleyden/brain3/security/advisories/new`). No root-level `SECURITY.md` file exists yet. L-10 remains open.

The recommendation is unchanged.

---

## Task 7 — Add new entries to Confirmed Good Controls table

**File:** `SECURITY_AUDIT.md` — "Confirmed Good Controls" section (inside the `<details>` block)

Add these rows:

| Area | Control | Notes |
|---|---|---|
| OAuth2 | Auth-code issuance and PKCE verification via oxide-auth | `AuthMap<RandomGenerator>` issues codes; `AddonList + Pkce` enforces S256; Brain3 no longer owns this code path (oxide-auth rebase #105) |
| OAuth2 | Bearer token recovery via oxide-auth issuer | `mcp_handlers.rs` calls `issuer.recover_token()` on the shared `SqliteTokenStore`; all log calls use `elide_secret()` |
| OAuth2 | Refresh token flow via oxide-auth issuer | `execute_refresh_token_flow()` delegates to `SqliteTokenStore`; rotation on use (#111) |
| Supply chain | Binary release signing | Release builds emit `SHA256SUMS` + RSA signature; `install.sh` verifies signed manifest with embedded public key before extraction (#106) |
| Supply chain | GitHub Actions least-privilege | All CI/release workflow jobs use explicit `permissions: contents: read` except the single release-upload job that needs `write` (#94) |
| Supply chain | Dependabot | Automated dependency-update PRs enabled for Rust and GitHub Actions (#82) |
| Supply chain | OpenSSF Scorecard | Supply-chain risk scoring via `scorecard.yml`; results visible in repo Security tab (#81) |

---

## Task 8 — Update README Claim Validation table

**File:** `SECURITY_AUDIT.md` — "README Claim Validation" section (inside the `<details>` block)

Update the row for "Constant-time comparison for all checks":

| README Claim | Status | Notes |
|---|---|---|
| Constant-time comparison for all checks | ✅ Accurate (updated) | Brain3's custom `constant_time_eq` wrapper (which early-returned on length mismatch) was deleted in the oxide-auth rebase. All comparisons now use `subtle::ConstantTimeEq::ct_eq()` directly. `subtle`'s slice implementation returns `Choice(0)` immediately on length mismatch (documented upstream behavior, not a Brain3 bug); in practice this is not exploitable given fixed-length secrets and rate-limiting. |

Also add a new row:

| Binary integrity verification | ✅ Accurate | `install.sh` verifies RSA-signed SHA256 manifest before extracting any binary (#106) |

---

## Task 9 — Update Prioritized Remediation Order

**File:** `SECURITY_AUDIT.md` — "Prioritized Remediation Order" section

Remove items 6 (M-3 auth code lifetime — now tracked differently), re-number, and drop M-4, L-1, L-4 entries. Update L-10 item to note partial closure. Final order:

1. **M-1** Fix `resolve_base_url` to use configured hostname
2. **M-2** Allowlist `redirect_uri`
3. **M-8** Move upstream secret out of `/tmp`
4. **M-10** Replace partial secret logging with `elide_secret` in `upstream_secret.rs`
5. **M-6** Verify Cloudflare credentials file permissions on startup
6. **M-3** Configure a shorter auth-code lifetime in oxide-auth's `AuthMap` (Brain3 doesn't set one; library default applies)
7. **L-2** Add CSP/security headers
8. **M-9** Change `DEFAULT_USERNAME` from `"admin"`
9. **M-12** Gateway-process sandboxing (Landlock/sandbox-exec)
10. **L-10** Add root-level `SECURITY.md` *(partial progress: README security-reporting section added in #91)*

M-13 has no standalone remediation; it tracks what's covered by fixing M-1 and M-2.

---

## Task 10 — Add changelog entry

**File:** `docs/security/changelog.md`

Add a new top-level section `## v0.1.9 → v0.2.1` before the existing `## v0.1.7 → v0.1.8` section. Content:

### ✅ RESOLVED — Brain3-owned `constant_time_eq` wrapper deleted (M-4)

The custom `constant_time_eq` function in `crates/core/src/domain/oauth.rs` was removed when that file was deleted in the oxide-auth rebase (#105). All secret comparisons now use `subtle::ConstantTimeEq::ct_eq()` directly in `registrar.rs` and `oauth_handlers.rs`. M-4 is closed.

### ✅ RESOLVED — In-memory auth-code store deleted (L-1)

`InMemoryAuthCodeStore` and its `cleanup_expired()` method were removed. Auth codes are now managed by oxide-auth's `AuthMap`, which evicts expired codes lazily on next access. L-1 is closed.

### ✅ RESOLVED — GET /oauth/authorize gated by param validation (L-4)

`validate_authorize_params()` rejects requests that lack the correct `client_id`, `response_type=code`, non-empty `redirect_uri`, and PKCE S256 challenge. No credentials are processed in the GET handler. L-4 is closed.

### 🔧 UPDATED — Brain3-owned OAuth surface reduced (M-13)

The oxide-auth rebase (#105) delegated auth-code issuance, PKCE verification, and code↔token exchange to oxide-auth. Bearer token recovery in `mcp_handlers.rs` now calls `issuer.recover_token()` on the shared oxide-auth issuer. M-13 remains open but the Brain3-owned surface is smaller; see the updated finding for the precise boundary.

### 🔒 NEW CONTROL — Binary release signing (#106)

Release builds now emit `SHA256SUMS` and an RSA signature file. `install.sh` downloads both alongside the tarball, verifies the signature against an embedded public key using `openssl dgst -verify`, and checks the tarball SHA256 before extracting. Added to Confirmed Good Controls.

### 🔒 NEW CONTROL — Token refresh via oxide-auth issuer (#111)

`execute_refresh_token_flow()` delegates refresh-token exchange to `SqliteTokenStore`; rotation on use is preserved. Added to Confirmed Good Controls.

### 🔒 NEW CONTROL — Supply-chain hardening (#81, #82, #94)

GitHub Actions workflows now use least-privilege `permissions` blocks. Dependabot is enabled for Rust crates and GitHub Actions. OpenSSF Scorecard workflow added. Added to Confirmed Good Controls.

### 🔧 PARTIAL — Vulnerability disclosure (L-10)

A "Reporting Security Issues" section was added to `README.MD` (#91) pointing to GitHub private advisory reporting. L-10 remains open until a root-level `SECURITY.md` is added.
