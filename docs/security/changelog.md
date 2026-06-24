# Security Changelog

Resolved security findings, listed by the version in which they were closed. See [SECURITY_AUDIT.md](../../SECURITY_AUDIT.md) for open findings.

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
