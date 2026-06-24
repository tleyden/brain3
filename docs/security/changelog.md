# Security Changelog

Resolved security findings, listed by the version in which they were closed. See [SECURITY_AUDIT.md](../../SECURITY_AUDIT.md) for open findings.

---

## v0.1.9 → v0.2.1

### ✅ RESOLVED — Brain3-owned `constant_time_eq` wrapper deleted (M-4)

The custom `constant_time_eq` helper in `crates/core/src/domain/oauth.rs` was removed when that file was deleted in the oxide-auth rebase (#105). Secret comparisons now call `subtle::ConstantTimeEq::ct_eq()` directly in `registrar.rs` and `oauth_handlers.rs`. M-4 is closed.

### ✅ RESOLVED — In-memory auth-code store deleted (L-1)

`InMemoryAuthCodeStore` and its `cleanup_expired()` method were removed. Authorization codes are now issued by oxide-auth's `AuthMap`, so the old Brain3-owned cleanup finding no longer applies. L-1 is closed.

### ✅ RESOLVED — `GET /oauth/authorize` no longer warrants a standalone rate-limit finding (L-4)

`validate_authorize_params()` now rejects requests that lack the configured `client_id`, `response_type=code`, a non-empty `redirect_uri`, or the required PKCE S256 parameters. The GET handler serves only the login form; credential processing remains POST-only and already rate-limited. L-4 is closed.

### 🔧 UPDATED — Brain3-owned OAuth surface reduced (M-13)

The oxide-auth rebase (#105) delegated auth-code issuance, PKCE verification, code exchange, and bearer token recovery to oxide-auth. Brain3 still owns the remaining policy layer (`check_credentials`, `GatewayRegistrar`, `SqliteTokenStore`, metadata construction, and error normalization), so M-13 stays open but now covers a smaller trusted surface.

### 🔒 NEW CONTROL — Binary release signing (#106)

Release assets now include `SHA256SUMS` and `SHA256SUMS.sig`, and `install.sh` verifies the signed manifest before checking the downloaded tarball hash and extracting the binary.

### 🔒 NEW CONTROL — Refresh token flow through oxide-auth issuer (#111)

`execute_refresh_token_flow()` now routes refresh-token exchange through the shared issuer while preserving refresh-token rotation on use.

### 🔒 NEW CONTROL — Supply-chain hardening (#81, #82, #94)

OpenSSF Scorecard was added, workflows now default to least-privilege permissions with targeted write scopes only where needed, and Dependabot is enabled for Cargo and uv dependency updates.

### 🔧 PARTIAL — Vulnerability disclosure (L-10)

`README.MD` now includes a "Reporting Security Issues" section pointing to GitHub private advisory reporting. L-10 remains open until a root-level `SECURITY.md` is added.

---

## v0.1.7 → v0.1.8

### ✅ RESOLVED — Generated Passwords Lacked Symbol Character Class (L-8)

`generate_password` in `crates/platform/src/setup/system.rs` previously used `rand::distr::Alphanumeric`, drawing only from the 62-character set `[A-Za-z0-9]`. As of v0.1.8 it samples from a 78-character set (`[A-Za-z0-9!#%^&*-_+=;:,.?~]`) and guarantees at least one symbol: the first character is drawn from the symbol-only subset, the remaining 23 from the full charset, then all 24 positions are Fisher-Yates shuffled. The symbols chosen (`!#%^&*-_+=;:,.?~`) are safe in double-quoted env values — `quote_env_value` only needs to escape `\` and `"`, neither of which is in the set.

### 🔧 PARTIAL IMPROVEMENT — Default Username Prompt (M-9)

The setup wizard Auth screen now explicitly tells users: "Username defaults to 'admin' — edit the field below to use a different login name." A keyboard input bug was also fixed: the `'g'` key was being intercepted to toggle password generation even when the cursor was in the Username or Client ID text fields, preventing users from typing the letter 'g' in those fields. Both issues are in `apps/gateway/src/tui/`. `DEFAULT_USERNAME` itself remains `"admin"` in `crates/core/src/domain/setup.rs` — M-9 stays open until that constant is changed.

---

## v0.1.6 → v0.1.7

### ✅ RESOLVED — MCP Container Unrestricted Outbound Access / Default-Bridge Exposure

Network isolation infrastructure (`docker network create --internal`, `container network create --internal` on macOS) already existed in v0.1.6 but defaulted to `false` everywhere — the setup wizard (`crates/core/src/application/first_run_setup.rs`) wrote `container_network_isolated: false` into every new `.env`, and the config-loading fallback (`crates/platform/src/config/env_file.rs`) used the same `false` default for any `.env` missing the variable. The `.env.template` shipped with the repo claimed `true` was the default, but neither code path actually applied it.

As of v0.1.7, both defaults are `true`. A fresh install gets a container with no default outbound route, placed on a dedicated internal network rather than the shared default bridge. Verified via `cargo test` (full suite, both crates) and by re-reading `docker.rs`'s `run()`, which attaches `--network <internal>` whenever `isolation_strategy` is set. The macOS `container` adapter mirrors this. `validate_network_isolation_support()` guards the one known-incompatible combination (`B3_CONTAINER_RUNTIME=docker` on macOS) with a clear startup error.

### ✅ RESOLVED — Dangling Cloudflare Tunnel on Unclean Shutdown

Previously tracked in `POTENTIAL_SECURITY_RISKS.md`. Resolved: `crates/platform/src/tunnel/cloudflare_quick.rs` and `cloudflare_named.rs` write a PID file (`lifecycle::write_pid_file`), set `kill_on_drop(true)` on the spawned `cloudflared` child process, configure `PR_SET_PDEATHSIG` on Linux, and call `lifecycle::graceful_kill` on shutdown.

### ✅ RESOLVED — Install Script Verifies Signed Checksum Manifest Before Extraction

**File:** `scripts/install.sh`

`install.sh` now downloads the release tarball together with `SHA256SUMS` and `SHA256SUMS.sig`, verifies the signed manifest with the embedded public key via `openssl dgst -verify`, then checks the tarball's SHA256 before extracting or installing the binary.

---

## v0.1.5 → v0.1.6

### ✅ RESOLVED — Static Non-Expiring Access Token

The static `.env`-loaded access token is gone. `token_exchange.rs` calls `generate_secure_token()` on every successful authorization code exchange, stores the result in the SQLite `access_tokens` table with an `expires_at` timestamp, and returns a refresh token alongside the access token. The proxy (`proxy_mcp.rs`) validates the token against the store, checks expiry, and checks that the token kind is `access` (not `refresh`). Refresh tokens are revoked on use and replaced with a new pair.

### ✅ RESOLVED — No Rate Limiting on Auth Endpoints

`crates/platform/src/http/rate_limit.rs`'s `OAuthRateLimiter` (backed by `governor`) enforces 20 burst attempts per IP with one token replenished every 45 seconds (~20/15 min) on `POST /oauth/authorize` and `POST /oauth/token`. Client IP is extracted preferentially from `CF-Connecting-IP`, with `X-Forwarded-For` as a fallback.
