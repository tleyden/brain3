# Brain3 Security Audit

**Auditor:** Claude Sonnet 4.6  
**Date:** 2026-06-16  
**Scope:** Full codebase — OAuth2 gateway, Cloudflare tunnel, local network / container exposure, default credentials, host process trust boundaries  
**Codebase version:** 0.1.7

---

## Executive Summary

The HIGH-severity finding from the prior audit (v0.1.6) — the MCP container having unrestricted outbound internet access — is **resolved in v0.1.7**. Network isolation (`B3_CONTAINER_INTERNAL_NETWORK_ISOLATION`) is now `true` by default in both the first-run setup wizard and the config-file fallback, so a fresh install gets a container with no outbound route and its own dedicated Docker/`container` network (also resolving the related peer-container exposure finding, 3.8, as a side effect). A previously tracked tunnel-lifecycle risk (dangling `cloudflared` process after an unclean exit) is also confirmed resolved — see [Changes Since Prior Audit](#changes-since-prior-audit-v016--v017).

This audit also formalizes two **architecture-level risks that are not new but were not previously called out explicitly**: the Rust gateway process itself is unsandboxed (no filesystem or network jail — see finding 6.3), and Brain3 implements its own OAuth2.1 server rather than using an established library (finding 6.4). Neither is being fixed in this pass; they're documented so the trade-off is explicit. A new finding (4.9) documents that generated passwords lack symbol/uppercase character classes; this is also deferred, not fixed, per the operator's request.

The remaining open surface is unchanged from v0.1.6 and is mostly medium-severity hardening gaps. The most impactful remaining issues are: the `resolve_base_url` host header injection (finding 1.3), `redirect_uri` not being allowlisted (finding 1.4), the upstream shared secret defaulting to a predictable `/tmp` path (finding 3.4), and the install script lacking checksum verification (finding 5.1).

All README security claims were re-validated against the current code. The container isolation claim, previously qualified, is now fully accurate. One claim — "Constant-time comparison for all secret and token checks" — remains partially overstated due to an early-exit on length mismatch in `constant_time_eq` (finding 1.6, unchanged from v0.1.6).

---

## Changes Since Prior Audit (v0.1.6 → v0.1.7)

### ✅ RESOLVED — 3.7 / 3.8: MCP Container Unrestricted Outbound Access / Default-Bridge Exposure

Network isolation infrastructure (`docker network create --internal`, `container network create --internal` on macOS) already existed in v0.1.6 but defaulted to `false` everywhere — the setup wizard (`crates/core/src/application/first_run_setup.rs`) wrote `container_network_isolated: false` into every new `.env` with the comment "disabled by default since this is still experimental," and the config-loading fallback (`crates/platform/src/config/env_file.rs`) used the same `false` default for any `.env` missing the variable. The `.env.template` shipped with the repo claimed `true` was the default, but neither code path actually applied it — a discrepancy this audit caught and fixed.

Both defaults are now `true`. A fresh install gets a container with no default outbound route, placed on a dedicated internal network rather than the shared default bridge. Verified via `cargo test` (full suite, both crates) and by re-reading `docker.rs`'s `run()`, which attaches `--network <internal>` whenever `isolation_strategy` is set.

### ✅ RESOLVED — Dangling Cloudflare Tunnel on Unclean Shutdown

Previously tracked as an open item in `POTENTIAL_SECURITY_RISKS.md` (not in this audit). Verified present in current code: `crates/platform/src/tunnel/cloudflare_quick.rs` and `cloudflare_named.rs` write a PID file (`lifecycle::write_pid_file`), set `kill_on_drop(true)` on the spawned `cloudflared` child process, configure `PR_SET_PDEATHSIG` on Linux, and call `lifecycle::graceful_kill` on shutdown. The checklist item has been removed from `POTENTIAL_SECURITY_RISKS.md`.

### ℹ️ Status Change — 6.2: `SECURITY.MD` Stub

The stub file `docs/SECURITY.MD` referenced in the v0.1.6 audit no longer exists — it was deleted rather than filled in. The underlying gap (no vulnerability disclosure policy or documented threat model at a discoverable path) remains; see updated finding 6.2.

### ✅ RESOLVED — 1.1: Static Non-Expiring Access Token *(carried forward from v0.1.5 → v0.1.6, still resolved)*

The static `.env`-loaded access token is gone. `token_exchange.rs` calls `generate_secure_token()` on every successful authorization code exchange, stores the result in the SQLite `access_tokens` table with an `expires_at` timestamp, and returns a refresh token alongside the access token. The proxy (`proxy_mcp.rs`) validates the token against the store, checks expiry, and checks that the token kind is `access` (not `refresh`). Refresh tokens are revoked on use and replaced with a new pair.

### ✅ RESOLVED — 1.2: No Rate Limiting on Auth Endpoints *(carried forward, still resolved)*

`crates/platform/src/http/rate_limit.rs`'s `OAuthRateLimiter` (backed by `governor`) enforces 20 burst attempts per IP with one token replenished every 45 seconds (~20/15 min) on `POST /oauth/authorize` and `POST /oauth/token`. Client IP is extracted preferentially from `CF-Connecting-IP`, with `X-Forwarded-For` as a fallback.

---

## Threat Model

### Architecture & Trust Boundaries

This diagram intentionally simplifies the prior version. The goal is to make the handful of boundaries that actually matter easy to reason about, rather than enumerating every internal data flow.

```mermaid
graph TD
    Internet["Internet (untrusted)\nAI client + any attacker"]
    CF["Cloudflare Edge\nTLS termination — trusted 3rd party"]
    Gateway["Rust Gateway (host)\nOAuth2.1 (hand-rolled) · static client id/secret\nloopback-only, LAN locked down\n⚠️ UNSANDBOXED — full host access if compromised"]
    Container["MCP Container\ninternal-only network, no egress"]
    Vault["Vault files\n(host filesystem)"]
    HostOther["Everything else on the host\n(other files, processes, credentials)"]

    Internet -- "HTTPS" --> CF
    CF -- "outbound-only tunnel\nMUST terminate at Gateway — auth lives here\n(THE scariest boundary)" --> Gateway
    Gateway -- "loopback + upstream secret" --> Container
    Container -- "bind mounts only, no egress" --> Vault
    Gateway -. "⚠️ if compromised: ambient access,\nno fs/network jail" .-> HostOther
    Gateway -. "⚠️ if compromised: ambient access" .-> Vault
```

### Trust Boundaries

| # | Boundary | Why it matters |
|---|---|---|
| B1 | Internet ↔ Cloudflare Edge | Cloudflare sees plaintext (TLS terminates there) — a trusted third party, not zero-trust. See the Cloudflare Tunnels section below. |
| B2 | Cloudflare Edge ↔ Rust Gateway | **Primary attack surface, by architectural necessity.** The tunnel must terminate at the gateway, not the container, because OAuth/token validation lives in the gateway. This is the only process directly reachable from the internet — and it is also the process holding every secret and (per B5) unrestricted host access. |
| B3 | Gateway ↔ MCP Container | Loopback-only + upstream shared secret (`x-brain3-upstream-secret`). The host *could* read the vault directly (same filesystem, no kernel-level barrier) but no code path does — the container is the only mechanism Brain3 itself uses to touch vault data. |
| B4 | Container ↔ Host filesystem | Container has no egress by default (`B3_CONTAINER_INTERNAL_NETWORK_ISOLATION=true`, finding 3.7/3.8 resolved) and can only reach the bind-mounted vault and upstream-secret directories — nothing else on the host. |
| B5 | Gateway process ↔ rest of the host | **Not currently a boundary at all.** The gateway runs as a normal, unsandboxed OS process: no filesystem jail (chroot/Landlock), no network egress restriction, no capability dropping. If the gateway is compromised by any means — a malicious dependency, a logic bug in the hand-rolled OAuth implementation, anything — the attacker has the same access as the user account running it: the entire host, not just the vault. Unlike the container (B4), there is no containment here today. See findings 6.3 and 6.4. |

### Threat Actors

| Actor | Entry point | Goal | Contained? |
|---|---|---|---|
| Remote unauthenticated attacker | B2 (Cloudflare tunnel) | Steal vault data, forge/steal OAuth tokens, enumerate the server | Mitigated by OAuth2.1 + PKCE + rate limiting, not by sandboxing |
| Compromised or malicious AI platform | Holds valid OAuth client credentials | Abuse legitimate MCP tool calls to exfiltrate or corrupt vault data | Bounded to whatever the MCP vault tools expose |
| Supply-chain attacker — **container dependencies** | Malicious Python package in the MCP container image | RCE inside the container | **Yes** — blocked at B4: no egress, mount-only filesystem access (finding 3.7/3.8 resolved) |
| Supply-chain attacker — **Rust host dependencies** | Malicious or compromised crate in the gateway's dependency tree | RCE in the unsandboxed gateway process → read/exfiltrate **any file the host user can access**, not just the vault | **No** — full host access; this is the gap named in B5 / finding 6.3 |
| Protocol-logic attacker against the custom OAuth2.1 server | B2, sends malformed or adversarial requests to the hand-rolled implementation | Trigger unintended behavior (auth bypass, state confusion, token issuance for the wrong identity, etc.) via a logic bug. Rust's memory safety rules out buffer overflows, use-after-free, and similar — it does **not** rule out logic flaws specific to a protocol implementation that wasn't built on a battle-tested library | Partially — PKCE, rate limiting, and constant-time checks reduce blast radius, but a novel logic bug is not ruled out by the language. See finding 6.4 |
| Local/LAN actor | B2/B3, only if loopback binding were ever misconfigured | Bypass OAuth entirely by talking to the gateway or container directly | Mitigated today by hardcoded `127.0.0.1` binds (findings 3.1, 3.2) |

### Assets

| Asset | Sensitivity | Location |
|---|---|---|
| Vault markdown files | HIGH — personal knowledge base | Host filesystem (bind-mounted rw into container; also directly readable by the unsandboxed gateway process, see B5) |
| Everything else on the host (other files, processes, credentials reachable by the user account) | HIGH — not Brain3-specific, but in scope because B5 is not a boundary | Host filesystem / OS, reachable if the gateway process is compromised |
| OAuth client secret | HIGH — grants token issuance | `~/.brain3/.env` (`0600`) |
| Brain3 password | HIGH — gate to OAuth login | `~/.brain3/.env` (`0600`) |
| Upstream shared secret | MEDIUM — gates direct MCP access | Host filesystem (default `/tmp`) |
| OAuth access/refresh tokens | MEDIUM — session credentials | SQLite `~/.brain3/brain3.db` |
| Cloudflare tunnel credentials | MEDIUM — controls tunnel routing | `~/.cloudflared/<tunnel-id>.json` |

---

## 1. OAuth2 Gateway — Findings

### 1.3 🟡 MEDIUM — `resolve_base_url` Trusts Client-Supplied Headers Without Validation

**Files:** `crates/platform/src/http/oauth_handlers.rs` (L19-30), `crates/platform/src/http/mcp_handlers.rs` (L17-28)

Both handlers still contain:
```rust
fn resolve_base_url(headers: &HeaderMap) -> String {
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    format!("{proto}://{host}")
}
```

This function trusts `X-Forwarded-Proto` and `X-Forwarded-Host` from any request without comparing them against the configured `expected_host`. A malicious request can set `X-Forwarded-Host: evil.attacker.com`, causing the OAuth authorization server metadata to advertise attacker-controlled endpoints — a Host Header Injection / OAuth Redirect Manipulation primitive.

When `B3_OAUTH2_GATEWAY_ENFORCE_HOSTNAME_CHECK=true` and a named tunnel or direct origin hostname is configured, the MCP proxy path correctly calls `validate_host()`. However, that check does not gate the URL construction used in metadata responses or login redirects. Quick-tunnel mode disables hostname enforcement entirely (see finding 2.1), making this issue more impactful in the common default configuration.

**Recommendation:** For canonical URL construction (metadata documents, redirect target assembly), use the configured `expected_host` rather than request-supplied headers. Forwarded headers should only be used for request-local context like logging.

---

### 1.4 🟡 MEDIUM — `redirect_uri` Not Allowlisted

**File:** `crates/core/src/domain/oauth.rs` (L64-66)

```rust
if req.redirect_uri.is_empty() {
    return Err(OAuthError::InvalidRequest("redirect_uri required".into()));
}
```

Only emptiness is checked. RFC 6749 §3.1.2 and RFC 9700 require the redirect URI to be compared against a pre-registered set. Without this:

1. **Open redirect**: After login, the user's browser is sent to any URL the caller specifies.
2. **Authorization-code interception**: An attacker who injects a `redirect_uri` into the authorize URL receives the auth code at an endpoint they control.

The single-client model (`client_id` is validated) provides partial mitigation, but a compromised AI platform or MITM could exploit this.

**Recommendation:** Add a `B3_OAUTH2_REDIRECT_URI_ALLOWLIST` config variable. Reject any `redirect_uri` not in the allowlist.

---

### 1.5 🟡 MEDIUM — Auth Code Lifetime is 5 Minutes; No Session Binding

**File:** `crates/core/src/domain/oauth.rs` (L10)

```rust
pub const AUTH_CODE_LIFETIME: Duration = Duration::from_secs(300);
```

Five minutes exceeds the ~60–120 second lifetime common in practice. The auth code is not bound to the originating IP or session cookie. PKCE (enabled by default) is the primary mitigation and makes code interception significantly harder to exploit. The risk is still present if PKCE is disabled via `B3_OAUTH2_PKCE_REQUIRED=false`.

**Recommendation:** Reduce `AUTH_CODE_LIFETIME` to 60 seconds. When `pkce_required=false`, add a compensating control such as IP binding.

---

### 1.6 🟡 MEDIUM — `constant_time_eq` Leaks Secret Length via Early Exit

**File:** `crates/core/src/domain/oauth.rs` (L89-94)

```rust
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}
```

The early return on length mismatch leaks the byte length of the stored secret as a timing side-channel. An attacker with sub-microsecond network access (LAN or co-located) could enumerate the lengths of `B3_PASSWORD`, `B3_OAUTH2_GATEWAY_CLIENT_SECRET`, and the upstream secret through repeated probing. Cloudflare tunnel jitter makes this impractical over the public internet, but it is a real risk from local network access.

The README claims "Constant-time comparison for all secret and token checks" — this overstates the guarantee.

**Recommendation:** Use HMAC-based comparison (`HMAC(key, a) == HMAC(key, b)` using `subtle`) or pad inputs to a fixed length before comparison. Alternatively, update the README to qualify this claim.

---

### 1.7 🟢 LOW — No Background Cleanup of Expired Auth Codes

**File:** `crates/platform/src/auth_code_store/in_memory.rs`

`cleanup_expired()` is triggered only at `issue_code` and `token_exchange` time. Under high abuse, expired codes accumulate in memory. This is a minor concern given that code issuance requires valid credentials, and the rate limiter now throttles the attack surface considerably.

**Recommendation:** Spawn a background Tokio task that calls `cleanup_expired()` every 60 seconds.

---

### 1.8 🟢 LOW — No Content-Security-Policy or Security Headers on Login Page

**File:** `crates/platform/src/http/templates.rs`, `crates/platform/src/http/router.rs`

The login HTML page is served without:
- `Content-Security-Policy` (XSS mitigation)
- `X-Frame-Options` (clickjacking protection)
- `Referrer-Policy` (prevents OAuth `state`/`code` leakage via referrer)
- `X-Content-Type-Options`

The login form embeds hidden fields containing `redirect_uri` and `code_challenge`, so XSS on this page would be especially damaging.

**Recommendation:** Add a `tower_http::set_header::SetResponseHeaderLayer` for HTML responses. Minimum:
```
Content-Security-Policy: default-src 'self'; style-src 'self'; img-src 'self' data:
X-Frame-Options: DENY
X-Content-Type-Options: nosniff
Referrer-Policy: no-referrer
```

---

### 1.9 🟢 LOW — `state` Parameter Not Required or Validated

**File:** `crates/platform/src/http/oauth_handlers.rs` (L69)

The `state` parameter is stripped if empty and echoed back, but is never required. An AI client sending no `state` silently proceeds. The OAuth spec relies on client-supplied high-entropy `state` to prevent CSRF; the server cannot enforce this by itself, but it can log a warning when `state` is absent to aid diagnosis.

**Recommendation:** Log a warning when `state` is absent; document that state is required for CSRF protection.

---

### 1.10 🟢 LOW — `GET /oauth/authorize` Not Rate-Limited

**File:** `crates/platform/src/http/oauth_handlers.rs` (L104-138)

The `POST /oauth/authorize` and `POST /oauth/token` endpoints are rate-limited (see resolved finding 1.2). However, `GET /oauth/authorize` — which validates the authorization request and renders the login form — is not. Since the GET handler does not process credentials, direct credential brute-force is not possible through it, but an attacker can enumerate valid `client_id` values or probe request validation without cost.

**Recommendation:** Apply the same `OAuthRateLimiter` check to `oauth_authorize_get`. The incremental implementation cost is low.

---

## 2. Cloudflare Tunnel — Findings

### 2.1 🟡 MEDIUM — Quick Tunnel Disables All Hostname Enforcement

**File:** `crates/platform/src/config/env_file.rs` (L333-349)

```rust
fn resolve_expected_host() -> Result<Option<String>, ConfigError> {
    let quick_explicit = env::var("B3_CF_QUICK_TUNNEL")…
    if quick_explicit {
        // …
        return Ok(None);  // hostname validation disabled
    }
```

When `B3_CF_QUICK_TUNNEL=true` (or the default when no named tunnel is configured), the expected host is `None`, so `validate_host()` is a no-op. Any request reaching the gateway with any `Host` header is accepted. This makes the host header injection issue (finding 1.3) more impactful because there is no configured hostname to even compare against.

This is architecturally inherent to quick tunnels (the URL changes on every restart), but the downstream consequences are under-documented.

**Recommendation:** Document this trade-off prominently in the README's security section. When using a quick tunnel, consider parsing the `cloudflared` stdout URL and using it as a soft expected-host for warning-level logging, even without enforcement.

---

### 2.2 🟡 MEDIUM — Cloudflare Credentials File Permissions Not Verified

**File:** `crates/platform/src/tunnel/cloudflare_setup.rs` (L100-108)

```rust
pub fn find_credentials_file(tunnel_id: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let path = PathBuf::from(format!("{home}/.cloudflared/{tunnel_id}.json"));
    if path.exists() { Some(path) } else { None }
}
```

The Cloudflare named tunnel credentials file grants full control of the named tunnel (it is a service account token equivalent). The code reads and uses it without verifying that the file's Unix permissions are `0600` or stricter. A world-readable credentials file on a shared system is a silent security failure.

**Recommendation:** On startup, check `~/.cloudflared/*.json` file permissions and warn (or refuse to start) if looser than `0600`. Use `std::os::unix::fs::MetadataExt::mode()` for the check.

---

### 2.3 🟢 LOW — `cloudflared` Binary Located via PATH — No Integrity Check

**File:** `crates/platform/src/tunnel/cloudflare_setup.rs` (L6-13)

The `cloudflared` binary is resolved via `which cloudflared` over the shell `PATH`. A PATH-hijacking attack (a malicious `cloudflared` earlier in `PATH`) would route all tunnel traffic through an attacker-controlled process. The risk is limited to local privilege escalation scenarios but is worth documenting.

**Recommendation:** Check that the resolved binary path is under a trusted directory (e.g., `/usr/local/bin`, `/opt/homebrew/bin`). Document this as a local privilege escalation surface.

---

## 3. Local Network / Container Isolation — Findings

### 3.1 ✅ GOOD — Gateway Binds to Loopback Only

**File:** `crates/platform/src/config/env_file.rs` (L104)

```rust
host: "127.0.0.1".to_string(),
```

The gateway is hard-coded to bind to `127.0.0.1`. There is no env var to override this. The gateway is not accessible from the LAN by default. ✅

---

### 3.2 ✅ GOOD — MCP Container Port Bound to 127.0.0.1

**File:** `crates/platform/src/container/startup.rs` (L82-86)

```rust
port_mappings: vec![PortMapping {
    host_address: "127.0.0.1".into(),
    host_port: startup.host_port,
    container_port: startup.container_port,
}],
```

The container's published port maps to `127.0.0.1` only, equivalent to `docker run -p 127.0.0.1:8420:8420`. The MCP container is not reachable from the LAN. ✅

---

### 3.3 🟡 MEDIUM — MCP Container Vault Mount Is Read-Write

**File:** `crates/platform/src/container/startup.rs` (L48-53)

```rust
BindMount {
    host_path: startup.vault_path.clone(),
    container_path: "/vault".into(),
    readonly: false,   // writable
},
```

The vault is mounted read-write into the MCP container. If the container is compromised (e.g., RCE through a malicious MCP tool call), an attacker can modify or delete vault files on the host. The upstream secret directory is correctly mounted read-only.

**Recommendation:** Evaluate whether all vault tools require write access. Consider a `B3_VAULT_READONLY=true` flag that mounts the vault read-only, suitable for vault-query-only use cases.

---

### 3.4 🟡 MEDIUM — Upstream Secret Stored in `/tmp` with Predictable Name

**File:** `crates/platform/src/config/env_file.rs` (L78-81)

```rust
let upstream_secret_file = PathBuf::from(env_var_or(
    "B3_OAUTH2_GATEWAY_UPSTREAM_SECRET_FILE",
    "/tmp/brain3-mcp-upstream-secret",
));
```

The default path is predictable. The file is created with `0600` permissions, so other users cannot read it, but:

1. Symlink attack: if an attacker creates `/tmp/brain3-mcp-upstream-secret` as a symlink before Brain3 starts, the `path.exists()` check in `upstream_secret.rs` returns `true` and the attacker-controlled symlink target is read as the shared secret.
2. On multi-user systems, other local users can observe the file's existence and creation time.

**Recommendation:**
- Change the default path to `~/.brain3/run/upstream-secret` or `$XDG_RUNTIME_DIR/brain3/upstream-secret`.
- Before reading, verify the path is not a symlink: add a `!path.is_symlink()` guard in `upstream_secret.rs`.
- Create the parent directory with `0700` permissions.

---

### 3.5 🟢 LOW — No Seccomp/AppArmor Profile Applied to MCP Container

**File:** `crates/platform/src/container/startup.rs`

The `ContainerConfig` does not set any seccomp profile, AppArmor label, or capability-dropping flags. The container runs with Docker's default seccomp profile, which is better than nothing but does not restrict to the minimal syscall set needed by an MCP server.

**Recommendation:** Add `--security-opt seccomp=/path/to/profile.json` and `--cap-drop ALL` to the Docker adapter. This becomes more important if arbitrary MCP tool containers are ever supported.

---

### 3.6 🟢 LOW — No Resource Limits on MCP Container

**File:** `crates/core/src/domain/model.rs`, `crates/platform/src/container/docker.rs`

`ContainerConfig` has no CPU, memory, or PID limit fields. A compromised or buggy MCP tool could exhaust host resources.

**Recommendation:** Add optional `memory_limit`, `cpu_limit`, and `pids_limit` fields to `ContainerConfig` and apply them via `--memory`, `--cpus`, `--pids-limit` in the adapters.

---

### 3.7 ✅ RESOLVED (v0.1.7) — MCP Container Outbound Internet Access

**Files:** `crates/platform/src/container/docker.rs`, `crates/platform/src/container/macos_container.rs`, `crates/platform/src/config/env_file.rs`, `crates/core/src/application/first_run_setup.rs`

The isolation mechanism (create an `--internal` Docker/`container` network, attach the MCP container to it) was already implemented in v0.1.6 but defaulted to `false` in both the place that matters: the setup wizard wrote `container_network_isolated: false` into every new install's `.env`, and the config loader's fallback for a missing env var was also `false`. The `.env.template` document already claimed `true`, but no code path applied it.

As of this audit, both defaults are flipped to `true`:
- `crates/core/src/application/first_run_setup.rs` — fresh installs now get `container_network_isolated: true`.
- `crates/platform/src/config/env_file.rs` — `env_bool("B3_CONTAINER_INTERNAL_NETWORK_ISOLATION", true)`.

Verified in `docker.rs::run()`: when `isolation_strategy` is set, the container is started with `--network <internal-network>`, and `prepare_network_isolation()` creates that network with `docker network create --internal` before the container starts. The macOS `container` adapter mirrors this. `validate_network_isolation_support()` still guards the one known-incompatible combination (`B3_CONTAINER_RUNTIME=docker` on macOS) with a clear startup error directing the operator to use `macos-container` instead.

Full `cargo test` (both crates) passes with the new defaults. This is a greenfield product with no installed base, so no migration path was added for pre-existing `.env` files — not applicable.

---

### 3.8 ✅ RESOLVED (v0.1.7) — MCP Container Shared Default Bridge

**File:** `crates/platform/src/container/docker.rs`

This was always a side effect of 3.7: without `--network`, Docker places the container on the shared default bridge, reachable by any other container on that bridge at its container IP. Since the container is now placed on a dedicated `--internal` network by default (3.7), other containers cannot reach it unless explicitly joined to that same named network. No separate code change was needed.

---

## 4. Default Credentials and Secrets — Audit

### 4.1 ✅ GOOD — No Hardcoded Default Passwords

The password field starts empty in the setup draft and the setup wizard either generates a 24-character cryptographically random password or requires user input. The server refuses to start if `B3_PASSWORD` is empty (`require_nonempty` in `env_file.rs`). ✅

---

### 4.2 🟡 MEDIUM — Default Username is `"admin"` — Predictable

**File:** `crates/core/src/domain/setup.rs` (L7)

```rust
pub const DEFAULT_USERNAME: &str = "admin";
```

The username is not a secret in an OAuth login form, but `"admin"` removes one layer of defense-in-depth: an attacker who reaches the login page needs only to guess the password. With rate limiting now in place, brute-forcing the password is significantly harder, which reduces the practical severity of this issue.

**Recommendation:** Change `DEFAULT_USERNAME` to `"brain3"` or a random value such as `"user-<4-chars>"`. At minimum, document that users should change the username from `admin` after setup.

---

### 4.3 ✅ GOOD — Client Secret and Tokens Generated with CSPRNG

```rust
// crates/platform/src/setup/system.rs
fn generate_secret_hex(&self, num_bytes: usize) -> Result<String, SetupError> {
    let mut bytes = vec![0u8; num_bytes];
    rand::rng().fill_bytes(&mut bytes);
    // hex encode
}
```

With `DEFAULT_GENERATED_SECRET_BYTES = 32`, this produces 256-bit random secrets (64 hex chars). The RNG is `rand::rng()` (ChaCha12 seeded from `getrandom`/OS CSPRNG). Per-session access tokens are generated the same way in `generate_secure_token()`. ✅

---

### 4.4 ✅ GOOD — Generated Passwords Use Cryptographic Randomness

```rust
fn generate_password(&self, length: usize) -> Result<String, SetupError> {
    rand::rng().sample_iter(rand::distr::Alphanumeric).take(length).collect()
}
```

`DEFAULT_GENERATED_PASSWORD_LENGTH = 24` characters of base-62 alphanumeric ≈ 143 bits of entropy. ✅

---

### 4.5 ✅ GOOD — Upstream Shared Secret Is 380-bit CSPRNG

**File:** `crates/platform/src/config/upstream_secret.rs` (L46-50)

```rust
let secret: String = rand::rng()
    .sample_iter(rand::distr::Alphanumeric)
    .take(64)
    .map(char::from)
    .collect();
```

64 base-62 characters ≈ 380 bits of entropy. ✅

---

### 4.6 🟡 MEDIUM — 7-Character Secret Prefix Logged in Tracing Output

**File:** `crates/platform/src/config/upstream_secret.rs` (L26-27, L63-64)

```rust
tracing::info!(
    secret_hint = &secret[..secret.len().min(7)],
    "Read existing upstream shared secret"
);
// …
tracing::warn!(
    secret_hint = &secret[..secret.len().min(7)],
    "Generated NEW upstream shared secret …"
);
```

The first 7 characters of the upstream shared secret are written to the tracing output. For a 64-character alphanumeric secret the entropy reduction is ~41 bits (still ~339 bits remaining), making brute-force infeasible. However, the principle of not logging any secret material in production stands. The `elide_secret()` helper is used correctly elsewhere in the codebase but was not applied here.

**Recommendation:** Replace `&secret[..secret.len().min(7)]` with `elide_secret(&secret)` on both log calls.

---

### 4.7 ✅ GOOD — `.env` File Written with `0600` Permissions

**File:** `crates/platform/src/setup/system.rs` (L193-202)

```rust
#[cfg(unix)]
{
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await …
}
```

The `.env` file containing all secrets is written owner-read-only. ✅

---

### 4.8 ✅ GOOD — Refresh Token Rotation Implemented

**File:** `crates/core/src/application/token_exchange.rs` (L209-216)

```rust
self.token_store.revoke(refresh_token).await.map_err(|error| { … })?;
```

On a successful refresh token exchange, the old refresh token is revoked before the new pair is issued. This prevents replay of a captured refresh token.

Default refresh token lifetime is 90 days (`DEFAULT_REFRESH_TOKEN_LIFETIME_SECS = 90 * 24 * 60 * 60`). This is long but expected for OAuth sessions; the operator can reduce it via `B3_OAUTH2_REFRESH_TOKEN_LIFETIME_SECS`. ✅

---

### 4.9 🟢 LOW — Generated Passwords Lack Symbol/Uppercase Character Classes

**File:** `crates/platform/src/setup/system.rs` (L163-172)

```rust
fn generate_password(&self, length: usize) -> Result<String, SetupError> {
    rand::rng().sample_iter(rand::distr::Alphanumeric).take(length).collect()
}
```

`rand::distr::Alphanumeric` draws from a 62-character set (`[A-Za-z0-9]`) but case is not separately controlled — the practical concern raised is that the generator does not guarantee a mix of symbol/uppercase/lowercase/digit classes the way many password-complexity policies expect. At `DEFAULT_GENERATED_PASSWORD_LENGTH = 24`, the actual entropy (~143 bits) is already far beyond what's brute-forceable, so this is a low-severity, mostly cosmetic/compliance gap rather than a practical weakness — tracked in [`docs/POTENTIAL_SECURITY_RISKS.md`](POTENTIAL_SECURITY_RISKS.md).

**Recommendation:** Sample from a combined alphanumeric + symbol distribution, or explicitly mix character classes, if matching common password-policy expectations matters. **Deferred — not fixed in this pass per operator instruction; tracked as an open item.**

---

## 5. Install Script — Findings

### 5.1 🟡 MEDIUM — Install Script Fetches Binary Over HTTPS Without Checksum Verification

**File:** `scripts/install.sh`

```sh
curl -sSfL "$URL" -o "$TMPDIR/$TARBALL"
tar -xzf "$TMPDIR/$TARBALL" -C "$TMPDIR"
chmod +x "$TMPDIR/$BINARY"
mv "$TMPDIR/$BINARY" "$BIN_DIR/$BINARY"
```

The install script downloads and executes a binary from S3 without verifying a SHA256 checksum or signature. A compromised S3 bucket, DNS hijack, or CDN cache poisoning could deliver a malicious binary. The `S3_BASE_URL` override env var further widens the surface.

**Recommendation:**
- Publish a `SHA256SUMS` file alongside release tarballs and verify with `sha256sum -c` in the script.
- Consider Sigstore/cosign signing for release artifacts.
- Emit a warning if `S3_BASE_URL` is overridden from the default.

---

## 6. Miscellaneous Findings

### 6.1 🟢 LOW — `/health` Endpoint Unauthenticated and Externally Reachable

**File:** `crates/platform/src/http/router.rs` (L33), `crates/platform/src/http/health.rs`

```rust
.route("/health", get(health))
```

The `/health` endpoint returns `{"status": "ok"}` without authentication. Through the Cloudflare tunnel, an external observer can determine that Brain3 is running and confirm the server fingerprint. This is minor — health endpoints are commonly public — but it enables passive reconnaissance.

**Recommendation:** Accept as intended behavior and document it, or restrict `/health` to loopback-only access.

---

### 6.2 🟢 LOW — No Vulnerability Disclosure Policy

**File:** *(none — `docs/SECURITY.MD`, previously a stub, has since been deleted entirely)*

There is no vulnerability disclosure policy, contact for security reports, or discoverable threat-model document at a path GitHub or researchers would expect (e.g. a root-level `SECURITY.md`). This audit's threat model (above) covers the documentation gap in substance, but it isn't surfaced where a security researcher would look first.

**Recommendation:** Add a `SECURITY.md` at the repo root (GitHub surfaces this automatically in the repo's "Security" tab) with a contact email or private GitHub issue template, a link to this audit's threat model, and the supported scope.

---

### 6.3 🟡 MEDIUM — Gateway Process Is Unsandboxed (Threat Model B5)

**Files:** N/A — this is an absence of a control, not a specific code location

The Rust gateway runs as a normal OS process with no filesystem jail (chroot/Landlock on Linux, sandbox-exec/App Sandbox on macOS), no network egress restriction, and no capability dropping. The MCP container (B4) is deliberately confined to bind-mounted directories with no outbound network access; the gateway that fronts it has no equivalent containment. If the gateway process is compromised by any means — a malicious or vulnerable Rust dependency, a logic bug in the hand-rolled OAuth implementation (finding 6.4), or any other RCE vector — the attacker inherits the full filesystem and network access of the user account running Brain3, not just the vault.

This is a known, accepted trade-off documented here for the first time rather than a regression — the host process needs broad access today (reading the vault for the rare direct-access path, talking to `cloudflared`, talking to the container runtime API) and no sandboxing work has been scoped.

**Recommendation (not undertaken in this pass):** Filesystem restriction is the more tractable option — Landlock (Linux 5.13+) or a macOS sandbox profile could restrict the gateway to only the paths it actually needs (`.env`, the SQLite token DB, the upstream secret file, the Cloudflare credentials directory) without touching the vault directly. Network egress restriction is harder, since the gateway legitimately needs outbound access to manage `cloudflared` and the container runtime; scoping this is a future investigation, not a quick fix.

---

### 6.4 🟡 MEDIUM — Custom OAuth2.1 Server Implementation Carries Inherent Protocol-Logic Risk

**Files:** `crates/core/src/domain/oauth.rs`, `crates/platform/src/http/oauth_handlers.rs`, `crates/core/src/application/token_exchange.rs`

Brain3 implements its own OAuth2.1 authorization server (authorize, token exchange, PKCE, refresh rotation) rather than building on an established, widely-audited server-side OAuth library. Rust's memory safety rules out entire bug classes — buffer overflows, use-after-free, data races — but it provides no protection against protocol-level logic errors specific to this implementation: subtle deviations from RFC 6749/9700, edge cases in state handling, or auth-bypass conditions that a maintained library, audited by a much wider community, might already have caught.

The existing controls (mandatory PKCE, rate limiting, constant-time comparisons, single static client) reduce the blast radius of most classes of attack, and several of this audit's open findings (1.3–1.6) are specific instances of this broader risk. This finding exists to name the category explicitly rather than leave it implicit.

**Recommendation:** No action proposed in this pass beyond what's already tracked in section 1. Worth revisiting if/when the OAuth surface grows (e.g. multiple clients, dynamic registration) — at that point, the cost/benefit of a vetted library changes.

---

## README Validation & Suggested Updates

All security claims in the README were verified against the current codebase. Results:

| README Claim | Status | Notes |
|---|---|---|
| Vault data stays 100% local | ✅ Accurate | Nothing is uploaded to Brain3-managed cloud services |
| Docker + Apple native container support | ✅ Accurate | Both `ContainerRuntime::Docker` and `ContainerRuntime::MacOSContainer` are implemented |
| OAuth 2.1 with PKCE | ✅ Accurate | Mandatory PKCE (`S256`), `client_secret_post`, no open registration — functionally OAuth 2.1 compliant |
| Only pre-registered client gets tokens | ✅ Accurate | `client_id` validated by constant-time comparison at every step |
| Client secret required at token exchange | ✅ Accurate | `client_secret_post` enforced; empty `client_secret` in config causes startup refusal |
| PKCE S256 enforced by default | ✅ Accurate | `B3_OAUTH2_PKCE_REQUIRED` defaults to `true` |
| Auth codes single-use, expire after 5 min | ✅ Accurate | `take()` atomically removes the code; `AUTH_CODE_LIFETIME = 300s` |
| Bearer-token validation on all `/mcp` routes | ✅ Accurate | `proxy_mcp.rs` validates token existence, expiry, and kind before proxying |
| Host validation returns HTTP 421 | ✅ Accurate | `validate_host()` is called in `proxy_mcp.rs` and `protected_resource_metadata` |
| Upstream shared secret rejects direct bypass | ✅ Accurate | `x-brain3-upstream-secret` header is injected and the container checks it |
| Constant-time comparison for all checks | ⚠️ Partially accurate | `constant_time_eq` short-circuits on length mismatch, leaking secret byte length (see finding 1.6, unchanged) |
| Rust host process | ✅ Accurate | Note: this audit's threat model (B5, finding 6.3) clarifies that "minimizes attack surface" does not mean the process is sandboxed — Rust prevents memory-safety bugs, not all RCE vectors, and the process has full ambient host access if compromised |
| Container-based filesystem isolation plus internal-only networking by default | ✅ Accurate (upgraded from "partially accurate") | Now correctly reflects the code: `B3_CONTAINER_INTERNAL_NETWORK_ISOLATION` defaults to `true` in both the setup wizard and config loader as of v0.1.7 (finding 3.7 resolved) |
| Cloudflare tunnels with TLS | ✅ Accurate | Both quick and named tunnel paths are implemented |
| OAuth2.1 with PKCE; no open registration (DCR/CIMD disabled) | ✅ Accurate | Confirmed no `/oauth/register` route exists in `router.rs`; `oauth_register_route_is_not_exposed` integration test passes |

**Still-pending README additions** (carried forward from v0.1.6, not yet added by the maintainer; this audit only re-confirms they remain accurate and outstanding):

1. **Per-session short-lived tokens** — Every OAuth login issues a fresh 256-bit access token with a 1-hour lifetime (default), persisted in SQLite. The prior static token model is gone.
2. **Refresh token rotation** — The refresh token is rotated on every use; the old token is revoked before the new pair is issued.
3. **Per-IP rate limiting on credential endpoints** — `POST /oauth/authorize` and `POST /oauth/token` are limited to 20 attempts per 15 minutes per client IP. Cloudflare's `CF-Connecting-IP` header is used for accurate IP identification behind the tunnel.

**Suggested README configuration table addition (still outstanding):**

| Variable | Default | Description |
|---|---|---|
| `B3_OAUTH2_REFRESH_TOKEN_LIFETIME_SECS` | `7776000` | Lifetime of issued refresh tokens in seconds (default: 90 days) |

---

## Summary Table

| # | Severity | Area | Finding | Status |
|---|----------|------|---------|--------|
| 1.1 | ✅ RESOLVED | OAuth2 | Static, non-expiring access token | Fixed in v0.1.6 |
| 1.2 | ✅ RESOLVED | OAuth2 | No rate limiting on auth/token endpoints | Fixed in v0.1.6 |
| 1.3 | 🟡 MEDIUM | OAuth2 | Host header injection in `resolve_base_url` | Open |
| 1.4 | 🟡 MEDIUM | OAuth2 | `redirect_uri` not allowlisted | Open |
| 1.5 | 🟡 MEDIUM | OAuth2 | 5-minute auth code lifetime; no session binding | Open |
| 1.6 | 🟡 MEDIUM | OAuth2 | `constant_time_eq` leaks secret length | Open |
| 1.7 | 🟢 LOW | OAuth2 | No background cleanup of expired auth codes | Open |
| 1.8 | 🟢 LOW | OAuth2 | No CSP or security headers on login HTML | Open |
| 1.9 | 🟢 LOW | OAuth2 | `state` parameter not required or validated | Open |
| 1.10 | 🟢 LOW | OAuth2 | `GET /oauth/authorize` not rate-limited | Open |
| 2.1 | 🟡 MEDIUM | Tunnel | Quick tunnel disables all hostname enforcement | Open |
| 2.2 | 🟡 MEDIUM | Tunnel | Cloudflare credentials file permissions not verified | Open |
| 2.3 | 🟢 LOW | Tunnel | `cloudflared` binary not integrity-checked | Open |
| 3.1 | ✅ GOOD | Network | Gateway binds to loopback only | — |
| 3.2 | ✅ GOOD | Network | Container port bound to 127.0.0.1 | — |
| 3.3 | 🟡 MEDIUM | Container | Vault bind-mount is read-write | Open |
| 3.4 | 🟡 MEDIUM | Container | Upstream secret stored in `/tmp` with predictable name | Open |
| 3.5 | 🟢 LOW | Container | No seccomp / capability-dropping profile | Open |
| 3.6 | 🟢 LOW | Container | No CPU/memory resource limits | Open |
| 3.7 | ✅ RESOLVED | Container | MCP container had unrestricted outbound internet access | Fixed in v0.1.7 |
| 3.8 | ✅ RESOLVED | Container | MCP container on default bridge — reachable from peer containers | Fixed in v0.1.7 (side effect of 3.7) |
| 4.1 | ✅ GOOD | Credentials | No hardcoded default passwords | — |
| 4.2 | 🟡 MEDIUM | Credentials | Default username is predictable (`"admin"`) | Open |
| 4.3 | ✅ GOOD | Credentials | Secrets generated with 256-bit CSPRNG | — |
| 4.4 | ✅ GOOD | Credentials | Passwords are 143-bit CSPRNG | — |
| 4.5 | ✅ GOOD | Credentials | Upstream secret is 380-bit CSPRNG | — |
| 4.6 | 🟡 MEDIUM | Credentials | 7-char secret prefix logged in tracing output | Open |
| 4.7 | ✅ GOOD | Credentials | `.env` written with `0600` permissions | — |
| 4.8 | ✅ GOOD | Credentials | Refresh token rotation implemented | — |
| 4.9 | 🟢 LOW | Credentials | Generated passwords lack symbol/uppercase character classes | Open — deferred by operator request |
| 5.1 | 🟡 MEDIUM | Install | Binary installed without checksum verification | Open |
| 6.1 | 🟢 LOW | HTTP | `/health` unauthenticated and externally reachable | Open |
| 6.2 | 🟢 LOW | Ops | No vulnerability disclosure policy (`SECURITY.MD` stub deleted, never replaced) | Open |
| 6.3 | 🟡 MEDIUM | Architecture | Gateway process is unsandboxed — full host access if compromised (threat model B5) | Open — documented, not fixed |
| 6.4 | 🟡 MEDIUM | Architecture | Hand-rolled OAuth2.1 implementation carries inherent protocol-logic risk | Open — documented, not fixed |

---

## Prioritized Remediation Order

1. **Fix `resolve_base_url` to use configured hostname** (1.3) — prevents host header injection across OAuth metadata and MCP protected-resource metadata. Now the highest-impact open finding following 3.7/3.8's resolution.
2. **Allowlist `redirect_uri`** (1.4) — blocks open redirects and code interception; straightforward config addition.
3. **Move upstream secret out of `/tmp`** (3.4) — easy default path change plus symlink guard.
4. **Replace partial secret logging with `elide_secret`** (4.6) — one-line fix per log call.
5. **Add checksum verification to install script** (5.1) — supply chain hygiene.
6. **Verify Cloudflare credentials file permissions** (2.2) — startup check, low effort.
7. **Reduce auth code lifetime to 60s** (1.5) — one-constant change.
8. **Add CSP/security headers** (1.8) — middleware layer addition.
9. **Rate-limit `GET /oauth/authorize`** (1.10) — reuse existing `OAuthRateLimiter`.
10. **Change default username from `"admin"`** (4.2) — one-constant change in `setup.rs`.
11. **Strengthen generated password character classes** (4.9) — deferred by operator request; revisit later.
12. **Scope gateway-process sandboxing** (6.3) — larger investigation (Landlock/sandbox-exec for filesystem restriction), not a quick fix; revisit once the rest of this list is clear.
13. **Add a root-level `SECURITY.md`** (6.2) — disclosure policy and a pointer to this audit's threat model.

Finding 6.4 (custom OAuth2.1 implementation risk) has no standalone remediation — it's mitigated incrementally as findings in section 1 are addressed.
