# Brain3 Security Audit

**Auditor:** Claude Sonnet 4.6  
**Date:** 2026-06-12  
**Scope:** Full codebase — OAuth2 gateway, Cloudflare tunnel, local network / container exposure, default credentials  
**Codebase version:** 0.1.5

---

## Executive Summary

The gateway's core OAuth2 implementation is well-constructed. It uses constant-time comparisons throughout, enforces PKCE by default, generates cryptographically secure tokens, properly strips authorization headers before proxying, and binds only to loopback by default. These are the things that matter most and they are done correctly.

However, several medium-to-high severity issues were found that warrant attention before broader deployment, plus a handful of lower-severity hardening gaps. The most critical finding is the **static, non-rotating access token model**, which means a single leaked token grants permanent MCP access until the operator manually rotates it; combined with the lack of rate limiting, this represents the most meaningful exploit surface.

---

## 1. OAuth2 Gateway — Findings

### 1.1 🔴 HIGH — Static Access Token with No Rotation or Expiry Enforcement

**File:** `crates/core/src/domain/oauth.rs`, `crates/core/src/application/proxy_mcp.rs`

```rust
pub const ACCESS_TOKEN_LIFETIME_SECS: u64 = 86400;   // advertised to client
// …
Ok(TokenResponse {
    access_token: self.config.access_token.clone(),   // identical static value every time
    token_type: "bearer".into(),
    expires_in: ACCESS_TOKEN_LIFETIME_SECS,
})
```

The `access_token` is a **single static string** loaded from the `.env` file at startup. Every successful OAuth login returns the **same token**. The `expires_in: 86400` figure is returned to the OAuth client (ChatGPT, Claude, etc.) as a hint, but the gateway never actually invalidates or rotates the token — it is valid forever until the operator manually changes the env var and restarts the process.

**Impact:** If the token is ever captured in transit (e.g., logged by a third-party AI platform, exposed via a misconfigured Cloudflare access log, or leaked from the AI client), an attacker has permanent access to the MCP endpoint. There is no revocation mechanism.

**Recommendation:**
- Issue per-session, time-limited bearer tokens at the `/oauth/token` step and track them in the `InMemoryAuthCodeStore` (or a new `TokenStore`).
- Honor the `expires_in` value the server itself advertises.
- Support token revocation (or at minimum, short-lived tokens that self-expire).

---

### 1.2 🔴 HIGH — No Rate Limiting on Login or Token Endpoints

**File:** `crates/platform/src/http/router.rs`, `apps/gateway/src/server.rs`

The router has no middleware for rate limiting on:
- `POST /oauth/authorize` — credential brute-force
- `POST /oauth/token` — client_secret brute-force
- `GET /oauth/authorize` — reconnaissance / enumeration

The only protection is constant-time comparison, which prevents timing attacks but does nothing to stop an automated attacker trying thousands of passwords per second over the Cloudflare tunnel.

**Recommendation:**
- Add a `tower` middleware layer (e.g., `governor` crate) with per-IP request budgets on the auth endpoints.
- Implement exponential backoff or temporary IP bans after N failed credential attempts.
- Consider adding CAPTCHA or a delay-on-failure mechanism at the login form level.

---

### 1.3 🟡 MEDIUM — `resolve_base_url` Trusts Client-Supplied Headers Without Validation

**File:** `crates/platform/src/http/oauth_handlers.rs` (L19-30), `crates/platform/src/http/mcp_handlers.rs` (L17-28)

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

This function trusts `X-Forwarded-Proto` and `X-Forwarded-Host` from any request. When Cloudflare is in front of the gateway, these headers are set by Cloudflare and are trustworthy. **However**, if the gateway is ever accessed directly (e.g., a user disables the tunnel and sets `B3_CF_QUICK_TUNNEL=false` with `B3_DIRECT_PUBLIC_ORIGIN_HOSTNAME` or via local access), a malicious request can set:
```
X-Forwarded-Host: evil.attacker.com
```
which causes the OAuth metadata and protected resource metadata to advertise attacker-controlled endpoints. This is a **Host Header Injection / OAuth Redirect Manipulation** primitive.

More concretely, the OAuth authorization server metadata at `/.well-known/oauth-authorization-server` would point to `https://evil.attacker.com/oauth/authorize`, potentially redirecting a victim AI client to the attacker's phishing endpoint.

**Recommendation:**
- Validate that the value derived from `X-Forwarded-Host` / `Host` matches the configured `expected_host` before using it to construct public-facing URLs.
- Or: require `B3_DIRECT_PUBLIC_ORIGIN_HOSTNAME` / `B3_CF_TUNNEL_NAME` and use only the configured hostname for canonical URL construction, ignoring forwarded headers for this purpose.

---

### 1.4 🟡 MEDIUM — `redirect_uri` Not Allowlisted

**File:** `crates/core/src/domain/oauth.rs` (L61-63), `crates/core/src/application/authorize.rs`

```rust
if req.redirect_uri.is_empty() {
    return Err(OAuthError::InvalidRequest("redirect_uri required".into()));
}
```

The code only checks that `redirect_uri` is non-empty. OAuth 2.0 best practice (RFC 6749, RFC 9700) requires that the redirect URI be **compared against a pre-registered allowlist**. Without this, any redirect URI is accepted, enabling:

1. **Open redirect**: After login, the user/browser is sent to any arbitrary URL the attacker specifies.
2. **Authorization code interception**: The auth code is delivered to an attacker-controlled endpoint if they can inject a `redirect_uri` into the OAuth flow.

In this single-client setup the `client_id` is validated, which provides partial mitigation (only one known client can initiate flows). But a compromised AI platform or a MITM that can inject parameters into the authorize URL could exploit this.

**Recommendation:**
- Add a `B3_OAUTH2_REDIRECT_URI_ALLOWLIST` config variable (comma-separated).
- Reject any `redirect_uri` not in the allowlist.
- At minimum, enforce that the `redirect_uri` scheme is `https://` (block `http://` and custom schemes unless explicitly allowed).

---

### 1.5 🟡 MEDIUM — Auth Code Lifetime is 5 Minutes — No Binding to Session/IP

**File:** `crates/core/src/domain/oauth.rs` (L10)

```rust
pub const AUTH_CODE_LIFETIME: Duration = Duration::from_secs(300);
```

Five minutes is longer than the RFC 6749 recommendation of a "short lifetime" (the spec suggests "several minutes at most" with common practice being 60–120 seconds). More importantly, the auth code is not bound to any session context (the originating IP or a session cookie). If an attacker intercepts the code-bearing redirect (e.g., via a referrer header or log file), they have a 5-minute window to exchange it.

The PKCE check is the primary mitigation here and is enabled by default — **this significantly reduces the practical risk**. But PKCE can be disabled via `B3_OAUTH2_PKCE_REQUIRED=false`.

**Recommendation:**
- Reduce `AUTH_CODE_LIFETIME` to 60 seconds.
- When `pkce_required=false`, add another mitigating check (e.g., bind code to redirect URI or IP).

---

### 1.6 🟡 MEDIUM — `constant_time_eq` Returns `false` for Different-Length Inputs Without Constant-Time Behavior

**File:** `crates/core/src/domain/oauth.rs` (L86-91)

```rust
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}
```

The early exit on length mismatch leaks the length of the stored secret as timing information. An attacker performing sub-microsecond timing analysis could enumerate the byte length of `B3_PASSWORD`, `B3_OAUTH2_GATEWAY_CLIENT_SECRET`, and `B3_OAUTH2_GATEWAY_ACCESS_TOKEN`.

In practice this is very hard to exploit over a network with jitter, and Cloudflare tunnel latency makes it practically infeasible over the public internet. However, from the local network or a co-located attacker, it is a real if low-probability concern.

**Recommendation:**
- Use `subtle::ConstantTimeEq` on padded / fixed-length representations, or HMAC-based comparison (e.g., compare `HMAC(key, a) == HMAC(key, b)` with a known-at-compile-time key).
- Alternatively, document this as a known acceptable limitation given the Cloudflare jitter baseline.

---

### 1.7 🟢 LOW — `cleanup_expired` is Triggered Only on Auth Code Issue/Exchange (No Background Task)

**File:** `crates/platform/src/auth_code_store/in_memory.rs`

The `cleanup_expired()` call is only invoked synchronously at `issue_code` and `token_exchange` time. Under heavy abuse (many unauthenticated requests or a flood of failed flows), expired codes accumulate in the HashMap until a successful flow triggers cleanup.

This is a minor memory exhaustion risk. A DoS scenario: an attacker repeatedly hits `/oauth/authorize` with valid parameters to generate codes (requires valid credentials) or forces code generation by triggering failed cleanups. 

In the current design, only a successfully authenticated user can generate codes, which limits this to a self-DoS or a compromised-credential scenario.

**Recommendation:**
- Spawn a background Tokio task that calls `cleanup_expired()` on a periodic timer (e.g., every 60 seconds).

---

### 1.8 🟢 LOW — No `Content-Security-Policy` or Security Headers on Login Page

**File:** `crates/platform/src/http/templates.rs`, `crates/platform/src/http/router.rs`

The login HTML page is served without security response headers:
- `Content-Security-Policy` (protects against XSS if the page is ever rendered in a browser context)
- `X-Frame-Options` (protects against clickjacking)
- `Referrer-Policy` (prevents leaking state/code params via referrer)
- `X-Content-Type-Options`

Since the login page's inline HTML is server-rendered and contains hidden form fields with OAuth state (`redirect_uri`, `code_challenge`, `state`), XSS on this page would be particularly damaging.

**Recommendation:**
- Add a `tower_http::set_header::SetResponseHeaderLayer` or a dedicated middleware that applies these headers to all HTML responses.
- Minimum recommended headers:
  ```
  Content-Security-Policy: default-src 'self'; style-src 'self'; img-src 'self' data:
  X-Frame-Options: DENY
  X-Content-Type-Options: nosniff
  Referrer-Policy: no-referrer
  ```

---

### 1.9 🟢 LOW — `state` Parameter Not Validated for Minimum Entropy

**File:** `crates/platform/src/http/oauth_handlers.rs` (L54)

The oauth `state` parameter is echoed back to the redirect URI without any validation. The OAuth spec requires clients to use high-entropy state values to prevent CSRF, but the server doesn't enforce this. An AI client sending no `state` (or an empty string) is silently allowed — the filter `filter(|s| !s.is_empty())` only strips empty strings, it doesn't require a `state`.

**Recommendation:**
- Log a warning if `state` is absent.
- Consider documenting that `state` is required for CSRF protection and rejecting flows with no state (noting this would break any client that doesn't send it).

---

## 2. Cloudflare Tunnel — Findings

### 2.1 🟡 MEDIUM — Quick Tunnel Disables Hostname Enforcement

**File:** `crates/platform/src/config/env_file.rs` (L283-299)

```rust
fn resolve_expected_host() -> Result<Option<String>, ConfigError> {
    let quick_explicit = env::var("B3_CF_QUICK_TUNNEL")…
    if quick_explicit {
        // …
        return Ok(None);  // hostname validation disabled
    }
```

When `B3_CF_QUICK_TUNNEL=true`, the quick tunnel URL is ephemeral (changes on every restart) so hostname validation is **disabled entirely**. This is architecturally correct but creates the following exposure:

- Any request reaching the gateway with _any_ Host header will be accepted.
- A misconfigured or compromised Cloudflare quick-tunnel that routes multiple hostnames to the same origin could allow unexpected Host routing to pass hostname validation.
- The `resolve_base_url` host-injection issue (finding 1.3) is more impactful under this configuration because there is no expected hostname to compare against.

**Recommendation:**
- Document this trade-off prominently in the README and TUI setup flow.
- When using a quick tunnel, verify via Cloudflare API or stdout parsing that the tunnel URL is `*.trycloudflare.com` and consider storing it + using it as a soft expected-host for logging purposes, even if not enforced.

---

### 2.2 🟡 MEDIUM — Cloudflare Credentials Stored in User Home Directory Without Audit

**File:** `crates/platform/src/tunnel/cloudflare_setup.rs` (L100-108)

```rust
pub fn find_credentials_file(tunnel_id: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let path = PathBuf::from(format!("{home}/.cloudflared/{tunnel_id}.json"));
```

The Cloudflare named tunnel credentials JSON file lives at `~/.cloudflared/<uuid>.json`. This file grants full control of the named tunnel (it is a service account token). The code reads and uses it but does not:
- Verify the file permissions are restrictive (e.g., `0600`).
- Warn if the file is world-readable.
- Rotate or re-provision the credentials.

**Recommendation:**
- On startup, check `~/.cloudflared/*.json` file permissions and warn (or refuse to start) if they are looser than `0600`.
- Document that this file is equivalent to an API key and should be treated accordingly.

---

### 2.3 🟢 LOW — `cloudflared` Binary Located via PATH — No Integrity Check

**File:** `crates/platform/src/tunnel/cloudflare_named.rs` (L34-40)

```rust
fn cloudflared_on_path() -> bool {
    std::process::Command::new("which")
        .arg("cloudflared")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
```

The `cloudflared` binary is resolved via PATH and executed without any checksum or signature verification. A PATH-hijacking attack (e.g., a malicious `cloudflared` earlier in the PATH) would cause the gateway to proxy traffic through an attacker-controlled tunnel.

**Recommendation:**
- Check that the resolved binary path is under a trusted directory (e.g., `/usr/local/bin`, `/opt/homebrew/bin`).
- Consider pinning the expected binary hash in a release manifest.
- At minimum, document this as a local privilege escalation surface.

---

## 3. Local Network / Container Isolation — Findings

### 3.1 ✅ GOOD — Gateway Binds to Loopback Only by Default

**File:** `apps/gateway/src/main.rs` (L28), `crates/platform/src/config/env_file.rs` (L78)

```rust
const DEFAULT_HOST: &str = "127.0.0.1";
// …
host: "127.0.0.1".to_string(),
```

The gateway is hard-coded to bind to `127.0.0.1` at the config level. The `host` field in `GatewayConfig` is set directly to `"127.0.0.1"` by the config loader and there is no env var to override it. This is correct and means the gateway is **not accessible from the local network** by default.

**Note:** The `--host` CLI flag does exist and could in principle override this if wired up, but `apply_runtime_overrides` does not currently modify `config.host`, so this is safe today. Verify this remains true as the codebase evolves.

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

The MCP container's published port uses `127.0.0.1` as the `host_address`, which maps to `docker run -p 127.0.0.1:8420:8420`. This means the container port is **not reachable from the LAN** — only from the gateway process on the same host. ✅

---

### 3.3 🟡 MEDIUM — MCP Container Vault Mount is Read-Write

**File:** `crates/platform/src/container/startup.rs` (L48-52)

```rust
BindMount {
    host_path: startup.vault_path.clone(),
    container_path: "/vault".into(),
    readonly: false,   // <-- writable
},
```

The user's Obsidian vault is mounted into the MCP container with write permissions. If the MCP container is compromised (e.g., via a malicious MCP tool call that achieves RCE inside the container), an attacker can **modify or delete vault files** on the host.

The upstream secret directory is correctly mounted read-only:
```rust
BindMount {
    host_path: startup.upstream_secret_dir.clone(),
    container_path: "/run/brain3".into(),
    readonly: true,   // ✅
},
```

**Recommendation:**
- Evaluate whether the MCP server actually needs write access to the vault (some tools like file creation/editing do, but read-only tools do not).
- Consider adding a config option `B3_VAULT_READONLY=true` that mounts the vault read-only, defaulting to read-write only for tool categories that require it.
- Alternatively, mount a specific subdirectory of the vault as writable rather than the entire vault root.

---

### 3.4 🟡 MEDIUM — Upstream Secret Stored in `/tmp` (World-Accessible Directory)

**File:** `crates/platform/src/config/env_file.rs` (L50-53)

```rust
let upstream_secret_file = PathBuf::from(env_var_or(
    "B3_OAUTH2_GATEWAY_UPSTREAM_SECRET_FILE",
    "/tmp/brain3-mcp-upstream-secret",
));
```

The default path for the upstream shared secret file is `/tmp/brain3-mcp-upstream-secret`. While the file is created with `0600` permissions (see `upstream_secret.rs` L55-60), `/tmp` is a sticky directory and the **filename is predictable**. Issues:

1. On a multi-user system, another local user cannot read the file (due to `0600`), but they can observe the file's existence, creation time, and size.
2. A symlink attack: if an attacker creates `/tmp/brain3-mcp-upstream-secret` as a symlink to a file they control before Brain3 starts, the `path.exists()` check returns `true` and the code reads their controlled content as the shared secret.

**Recommendation:**
- Change the default path to `$XDG_RUNTIME_DIR/brain3/upstream-secret` or a directory under the Brain3 app home (`~/.brain3/run/upstream-secret`), which is out of `/tmp`.
- Before reading, verify that the file is not a symlink (`!path.is_symlink()`).
- Create the parent directory with `0700` permissions.

---

### 3.5 🟢 LOW — No Seccomp/AppArmor Profile Applied to MCP Container

**File:** `crates/platform/src/container/startup.rs`, `crates/core/src/domain/model.rs`

The `ContainerConfig` struct and the Docker/macOS container adapters do not apply any seccomp profile, AppArmor/SELinux label, or capability-dropping flags to the container. The container runs with the default Docker seccomp profile (which is better than nothing) but does not further restrict syscalls to the minimal set needed by an MCP server.

**Recommendation:**
- Add `--security-opt seccomp=/path/to/profile.json` for Docker.
- Evaluate adding `--cap-drop ALL` and re-adding only required capabilities.
- This becomes more important if/when arbitrary MCP tool containers are supported.

---

### 3.6 🟢 LOW — No Resource Limits on MCP Container

**File:** `crates/core/src/domain/model.rs`, `crates/platform/src/container/docker.rs`

`ContainerConfig` has no `cpu_limit`, `memory_limit`, or `ulimit` fields. A compromised or buggy MCP tool could consume all available host CPU or memory (denial of service).

**Recommendation:**
- Add optional resource limit fields to `ContainerConfig` and apply them via `--memory`, `--cpus`, `--pids-limit` in the Docker/macOS adapters.

---

## 4. Default Credentials and Secrets — Audit

### 4.1 ✅ GOOD — No Hardcoded Default Passwords in Production Code

The `DEFAULT_USERNAME` is `"admin"` (a predictable name, see 4.2 below), but the **password field starts empty** in the setup draft:

```rust
// crates/core/src/application/first_run_setup.rs
password: String::new(),
```

The setup wizard either generates a 24-character cryptographically random alphanumeric password or requires the user to input one. The server refuses to start if `B3_PASSWORD` is empty:

```rust
// crates/platform/src/config/env_file.rs
let password = require_nonempty("B3_PASSWORD", &mut missing);
```

This is correct behavior. ✅

---

### 4.2 🟡 MEDIUM — Default Username is `"admin"` — Predictable

**File:** `crates/core/src/domain/setup.rs` (L7)

```rust
pub const DEFAULT_USERNAME: &str = "admin";
```

The username is not a secret in an OAuth login form (it is entered by the user and displayed in the TUI), but using `"admin"` means that an attacker who reaches the login page only needs to guess the **password**, not both factors. The username being well-known removes one layer of defense.

**Recommendation:**
- Change `DEFAULT_USERNAME` to `"brain3"` or generate a random username (e.g., `"user-<4-random-chars>"`).
- Or: document that users should change the username from `admin` after setup.

---

### 4.3 ✅ GOOD — `client_secret` and `access_token` Are Randomly Generated

Both secrets are generated at setup time using:

```rust
// crates/platform/src/setup/system.rs
fn generate_secret_hex(&self, num_bytes: usize) -> Result<String, SetupError> {
    use rand::RngCore;
    let mut bytes = vec![0u8; num_bytes];
    rand::rng().fill_bytes(&mut bytes);
    // … hex encode
}
```

With `DEFAULT_GENERATED_SECRET_BYTES = 32`, this produces 256-bit random secrets (64 hex chars). The RNG is `rand::rng()` which uses the OS CSPRNG (ChaCha12 seeded from `getrandom`). ✅

---

### 4.4 ✅ GOOD — Generated Passwords Use Cryptographic Randomness

```rust
// crates/platform/src/setup/system.rs
fn generate_password(&self, length: usize) -> Result<String, SetupError> {
    let password: String = rand::rng()
        .sample_iter(rand::distr::Alphanumeric)
        .take(length)
        .collect();
}
```

`DEFAULT_GENERATED_PASSWORD_LENGTH = 24` characters of base-62 alphanumeric = ~143 bits of entropy. More than sufficient. ✅

---

### 4.5 ✅ GOOD — Upstream Shared Secret Generated with 64 Alphanumeric Characters

**File:** `crates/platform/src/config/upstream_secret.rs` (L46-50)

```rust
let secret: String = rand::rng()
    .sample_iter(rand::distr::Alphanumeric)
    .take(64)
    .map(char::from)
    .collect();
```

64 base-62 characters ≈ 380 bits of entropy. Correct use of CSPRNG. ✅

---

### 4.6 🟡 MEDIUM — Secrets Logged at `warn` Level with Partial Reveal (`secret_hint`)

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

The first 7 characters of the upstream shared secret are logged to the tracing output (including log files). The `elide_secret()` helper is used correctly elsewhere, but here a raw slice is used. For a 64-character alphanumeric secret, exposing 7 chars reduces the brute-force space:

- Full secret: ~380 bits
- After 7-char leak: attacker knows first 7 chars of base-62 → secret strength reduced by ~41 bits → still ~339 bits, practically unbreakable.

However, the principle of not logging even partial secrets in production logs stands. An attacker with log access shouldn't get _any_ secret material.

**Recommendation:**
- Replace `&secret[..secret.len().min(7)]` with `elide_secret(&secret)` in these log calls.
- Or: log only "secret file exists and was loaded" without any hint.

---

### 4.7 ✅ GOOD — `.env` File Written with `0600` Permissions

**File:** `crates/platform/src/setup/system.rs` (L193-202)

```rust
#[cfg(unix)]
{
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .await …
}
```

The `.env` file containing all secrets is written with owner-read-only permissions. ✅

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

The install script downloads a binary from S3 and installs it without:
1. Comparing a SHA256 checksum from a separate, signed manifest.
2. Verifying a GPG or Sigstore signature on the binary.

If the S3 bucket or CDN is compromised, or if DNS is hijacked, a malicious binary could be delivered to users.

**Recommendation:**
- Publish a `SHA256SUMS` file alongside the release tarballs and verify with `sha256sum -c` in the install script.
- Consider Sigstore/cosign signing for release artifacts.
- The `S3_BASE_URL` override env var makes this worse — a user could be tricked into fetching from a malicious URL. Add a warning if `S3_BASE_URL` is overridden.

---

## 6. Miscellaneous Findings

### 6.1 🟢 LOW — `health` Endpoint Returns `200 OK` Without Authentication

**File:** `crates/platform/src/http/health.rs`, `crates/platform/src/http/router.rs`

```rust
.route("/health", get(health))
```

The `/health` endpoint returns `{"status": "ok"}` without any authentication. Accessed via the Cloudflare tunnel, this allows an unauthenticated external observer to determine that Brain3 is running and responsive. This is a minor **information disclosure** / **fingerprinting** risk.

**Recommendation:**
- Restrict `/health` to loopback-only requests, or require at minimum the `x-brain3-upstream-secret` header (or any non-public token) for public-facing health checks.
- Or: accept this as intended behavior (health endpoints are conventionally public) and document it.

---

### 6.2 🟢 LOW — `SECURITY.MD` is a Stub

**File:** `docs/SECURITY.MD`

```
## Threat Vector Model
TODO
```

There is no vulnerability disclosure policy, no contact for security reports, and no documented threat model. This is an operational gap for a publicly reachable internet service.

**Recommendation:**
- Add a `SECURITY.md` at the repo root (GitHub automatically surfaces this) with:
  - A contact email or private GitHub issue template for vulnerability reports.
  - The documented threat model.
  - The scope of what is and isn't supported.

---

## Summary Table

| # | Severity | Area | Finding |
|---|----------|------|---------|
| 1.1 | 🔴 HIGH | OAuth2 | Static, non-expiring access token |
| 1.2 | 🔴 HIGH | OAuth2 | No rate limiting on auth/token endpoints |
| 1.3 | 🟡 MEDIUM | OAuth2 | Host header injection in `resolve_base_url` |
| 1.4 | 🟡 MEDIUM | OAuth2 | `redirect_uri` not allowlisted |
| 1.5 | 🟡 MEDIUM | OAuth2 | 5-minute auth code lifetime; no session binding |
| 1.6 | 🟡 MEDIUM | OAuth2 | `constant_time_eq` leaks secret length |
| 1.7 | 🟢 LOW | OAuth2 | No background cleanup of expired auth codes |
| 1.8 | 🟢 LOW | OAuth2 | No CSP or security headers on login HTML |
| 1.9 | 🟢 LOW | OAuth2 | `state` parameter not required / not validated |
| 2.1 | 🟡 MEDIUM | Tunnel | Quick tunnel disables all hostname enforcement |
| 2.2 | 🟡 MEDIUM | Tunnel | Cloudflare credentials file permissions not verified |
| 2.3 | 🟢 LOW | Tunnel | `cloudflared` binary not integrity-checked |
| 3.1 | ✅ GOOD | Network | Gateway binds to loopback only |
| 3.2 | ✅ GOOD | Network | Container port bound to 127.0.0.1 |
| 3.3 | 🟡 MEDIUM | Container | Vault bind-mount is read-write |
| 3.4 | 🟡 MEDIUM | Container | Upstream secret stored in `/tmp` with predictable name |
| 3.5 | 🟢 LOW | Container | No seccomp / capability-dropping profile |
| 3.6 | 🟢 LOW | Container | No CPU/memory resource limits |
| 4.1 | ✅ GOOD | Credentials | No hardcoded default passwords |
| 4.2 | 🟡 MEDIUM | Credentials | Default username is predictable (`"admin"`) |
| 4.3 | ✅ GOOD | Credentials | Secrets generated with 256-bit CSPRNG |
| 4.4 | ✅ GOOD | Credentials | Passwords are 143-bit CSPRNG |
| 4.5 | ✅ GOOD | Credentials | Upstream secret is 380-bit CSPRNG |
| 4.6 | 🟡 MEDIUM | Credentials | 7-char secret prefix logged in tracing output |
| 4.7 | ✅ GOOD | Credentials | `.env` written with `0600` permissions |
| 5.1 | 🟡 MEDIUM | Install | Binary installed without checksum verification |
| 6.1 | 🟢 LOW | HTTP | `/health` unauthenticated and externally reachable |
| 6.2 | 🟢 LOW | Ops | `SECURITY.MD` is a stub; no disclosure policy |

---

## Prioritized Remediation Order

1. **Add rate limiting** (1.2) — highest leverage, low implementation cost with the `governor` crate.
2. **Move to per-session short-lived tokens** (1.1) — eliminates the permanent-token risk.
3. **Allowlist `redirect_uri`** (1.4) — straightforward config addition.
4. **Fix `resolve_base_url` to use configured hostname** (1.3) — prevents host header injection.
5. **Move upstream secret out of `/tmp`** (3.4) — easy default path change.
6. **Replace partial secret logging with `elide_secret`** (4.6) — one-line fix.
7. **Add checksum verification to install script** (5.1) — supply chain hygiene.
8. **Reduce auth code lifetime to 60s** (1.5) — one-constant change.
9. **Verify Cloudflare credentials file permissions** (2.2) — startup check.
10. **Add CSP/security headers** (1.8) — middleware layer addition.
