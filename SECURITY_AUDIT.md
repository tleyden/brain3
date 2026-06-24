# Security Review: brain3

| Field | Value |
|---|---|
| **Auditor** | Codex Security Scan |
| **Date** | 2026-06-24 |
| **Scope** | Full codebase — OAuth2 gateway, Cloudflare tunnel, local network / container exposure, default credentials, host process trust boundaries |
| **Codebase version** | 0.2.1 |



## Scope

Repository-wide security scan of the checked-out Brain3 git revision with prior audit context from `SECURITY_AUDIT.MD` and the 2026-06-24 security-audit update plan. High-risk review focused on OAuth policy, public ingress via Cloudflare tunnels, container boundary exposure, local credential handling, and MCP logging.

- Scan mode: repository
- Target kind: git_revision
- Target ID: target_sha256_a9005ab7bfe057b4d7f87c5c0076da4e3c5bd7926c032f5bda02bf7f58a4620a
- Revision: 8d23b3c103f9da9a2d6acdce86c9ad0c8afbc93f
- Inventory strategy: repository
- Included paths: .
- Excluded paths: none
- Runtime or test status: Static source review only; no live exploit reproduction or network probe was executed during final validation.
- Artifacts reviewed: artifacts/01_context/threat_model.md, artifacts/02_discovery/deep_review_input.jsonl, artifacts/02_discovery/work_ledger.jsonl, artifacts/02_discovery/raw_candidates.jsonl, artifacts/03_coverage/repository_coverage_ledger.md, artifacts/03_coverage/reviewed_surfaces.md, artifacts/04_reconciliation/dedupe_report.md, artifacts/04_reconciliation/deduped_candidates.jsonl, artifacts/05_findings/
- Scan context: Brain3 is a local-first Obsidian-compatible vault gateway that intentionally serves a single preregistered confidential OAuth client and can optionally expose remote access through Cloudflare tunnels.

Limitations and exclusions:
- Prompt injection was treated as out of scope for user-controlled vault content, but remains a residual risk for vaults that ingest untrusted third-party material or shared remote content.
- Some local-only setup/TUI and helper files were closed with targeted review or explicit deferred follow-up rather than exhaustive line-by-line manual review; see `artifacts/02_discovery/work_ledger.jsonl`.
- No runtime tests, browser flows, or exploit demonstrations were run during this scan-only pass.
- Excluded poc/\*\*: Repository instructions mark `poc/` as dead legacy outside the active product unless explicitly requested.

### Scan Summary

| Field | Value |
| --- | --- |
| Reportable findings | 4 |
| Severity mix | medium: 3, low: 1 |
| Confidence mix | high: 3, medium: 1 |
| Coverage | partial |
| Validation mode | Repository-wide source review with per-candidate discovery, validation, and attack-path receipts plus reconciled file-level worklist closure. |

Canonical artifacts: `scan-manifest.json`, `findings.json`, and `coverage.json`. This report is a deterministic projection of those files.

## Threat Model

Brain3’s highest-risk boundary is the optional public gateway/tunnel that fronts a local vault and containerized MCP server. The product intentionally restricts token issuance to one preregistered confidential client, but it still exposes OAuth metadata, login, token, and MCP proxy routes plus local secret files and container/runtime orchestration to different trust levels.

### Assets

- Vault markdown contents and metadata exposed through MCP tools
- OAuth client credentials, user login password, access tokens, and refresh tokens
- The upstream shared secret mounted into the MCP container
- Local `.env`, Cloudflare tunnel config, and SQLite token database files
- Gateway public origin, Cloudflare tunnel identity, and container network isolation state

### Trust Boundaries

- Unauthenticated internet clients reaching the gateway directly or via Cloudflare tunnels
- The single preregistered confidential AI client that holds Brain3’s client id and secret
- Local host filesystem and temp-directory principals
- The boundary between the Rust gateway and the `brain3-mcp-vault-tools` container
- Vault content that may be user-controlled or, in some deployments, third-party-controlled

### Attacker Capabilities

- Send arbitrary HTTP requests and headers to public gateway routes when tunneling is enabled
- Operate or compromise the preregistered OAuth client after it is provisioned with Brain3 credentials
- Read local files or logs available to the current OS principal or broader local principals
- Supply hostile vault content when the user does not fully control imported or shared notes

### Security Objectives

- Only explicitly preregistered confidential clients should obtain tokens and reach protected MCP data
- Public ingress should be opt-in and should not broaden exposure accidentally
- Local secrets and vault contents should not leak through logs or insecure default storage/permissions
- Container networking should keep the MCP server private by default

### Assumptions

- `poc/` is dead legacy and out of active scan scope
- Rust memory safety is assumed; this scan focused on logic, policy, and boundary bugs rather than memory corruption
- Prompt injection is generally out of scope for user-controlled vault content, but not for vaults the user does not fully control

## Findings

| Finding | Severity | Confidence |
| --- | --- | --- |
| [OAuth metadata and bearer challenges trust request-supplied host headers](#finding-1) | medium | high |
| [OAuth authorization accepts arbitrary redirect URIs for the preregistered client](#finding-2) | medium | high |
| [Cloudflare quick tunnel is enabled by default on first run](#finding-3) | medium | high |
| [Trace logging can record MCP request and response bodies to temp-backed logs](#finding-4) | low | medium |

### Confidence Scale

| Label | Meaning |
| --- | --- |
| high | Direct evidence supports the finding with no material unresolved blocker. |
| medium | Evidence supports a plausible issue, but material runtime or reachability proof remains. |
| low | Evidence is incomplete and the item is retained only for explicit follow-up. |

<a id="finding-1"></a>

### [1] OAuth metadata and bearer challenges trust request-supplied host headers

| Field | Value |
| --- | --- |
| Severity | medium |
| Confidence | high |
| Confidence rationale | Both metadata builders and the unauthenticated 401 bearer-challenge path show the same request-derived base-URL behavior directly in source. |
| Category | Host header injection / OAuth metadata trust |
| CWE | CWE-346 |
| Affected lines | crates/platform/src/http/oauth_handlers.rs:289-299, crates/platform/src/http/oauth_handlers.rs:647-666, crates/platform/src/http/mcp_handlers.rs:17-27, crates/platform/src/http/mcp_handlers.rs:137-145 |

#### Summary

Brain3 derives its public `base_url` from `X-Forwarded-Host` and `Host` request headers, then reuses that value in OAuth metadata and bearer-challenge `resource_metadata` output.

#### Root Cause

The violated invariant is that OAuth metadata should advertise Brain3’s configured public origin, not whichever host headers a request presents. Brain3 instead reconstructs its public identity from request headers and emits that value on unauthenticated metadata and challenge paths.

**OAuth metadata base URL comes from forwarded host headers** — `crates/platform/src/http/oauth_handlers.rs:289-299`

The public origin is constructed from request headers rather than a configured trusted hostname.

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
```

**The derived base URL feeds OAuth metadata output** — `crates/platform/src/http/oauth_handlers.rs:647-666`

The request-derived base URL becomes the advertised issuer and OAuth endpoint set.

```rust
pub async fn oauth_metadata<P: McpProxyPort + 'static>(
    State(_state): State<AppState<P>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let base_url = resolve_base_url(&headers);
    tracing::info!(/* ... */);
    Json(json!({
        "issuer": base_url,
        "authorization_endpoint": format!("{base_url}/oauth/authorize"),
        "token_endpoint": format!("{base_url}/oauth/token"),
        /* ... */
    }))
```

**The MCP 401 path repeats the same base-URL derivation** — `crates/platform/src/http/mcp_handlers.rs:17-27`

The bearer-challenge path duplicates the same request-derived public origin logic.

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
```

**Bearer challenge reflects the derived base URL into `resource_metadata`** — `crates/platform/src/http/mcp_handlers.rs:137-145`

Unauthenticated bearer challenges publish attacker-influenced metadata URLs before Brain3 authenticates the caller.

```rust
fn proxy_error_response(err: ProxyError, headers: &HeaderMap) -> Response {
    match err {
        ProxyError::Unauthorized(desc) => {
            let base_url = resolve_base_url(headers);
            let www_authenticate = format!(
                r#"Bearer error="invalid_token", error_description="{desc}", resource_metadata="{}""#,
                resource_metadata_url(&base_url)
            );
```

#### Validation

Validation followed both the OAuth metadata route and the MCP unauthorized-error path and found no configured-host binding before either output path emits public URLs.

Validation method: static source trace

**OAuth metadata base URL comes from forwarded host headers** — `crates/platform/src/http/oauth_handlers.rs:289-299`

The public origin is constructed from request headers rather than a configured trusted hostname.

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
```

**The derived base URL feeds OAuth metadata output** — `crates/platform/src/http/oauth_handlers.rs:647-666`

The request-derived base URL becomes the advertised issuer and OAuth endpoint set.

```rust
pub async fn oauth_metadata<P: McpProxyPort + 'static>(
    State(_state): State<AppState<P>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let base_url = resolve_base_url(&headers);
    tracing::info!(/* ... */);
    Json(json!({
        "issuer": base_url,
        "authorization_endpoint": format!("{base_url}/oauth/authorize"),
        "token_endpoint": format!("{base_url}/oauth/token"),
        /* ... */
    }))
```

**The MCP 401 path repeats the same base-URL derivation** — `crates/platform/src/http/mcp_handlers.rs:17-27`

The bearer-challenge path duplicates the same request-derived public origin logic.

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
```

**Bearer challenge reflects the derived base URL into `resource_metadata`** — `crates/platform/src/http/mcp_handlers.rs:137-145`

Unauthenticated bearer challenges publish attacker-influenced metadata URLs before Brain3 authenticates the caller.

```rust
fn proxy_error_response(err: ProxyError, headers: &HeaderMap) -> Response {
    match err {
        ProxyError::Unauthorized(desc) => {
            let base_url = resolve_base_url(headers);
            let www_authenticate = format!(
                r#"Bearer error="invalid_token", error_description="{desc}", resource_metadata="{}""#,
                resource_metadata_url(&base_url)
            );
```

#### Dataflow

Inbound `Host` / `X-Forwarded-Host` header -\> `resolve_base_url()` -\> OAuth metadata or bearer challenge output -\> client trust decision

- **Source:** attacker-controlled request host headers

- **Sink:** publicly emitted OAuth metadata fields and `resource_metadata` challenge URLs

- **Outcome:** OAuth clients can be misdirected during metadata discovery or invalid-token recovery flows

**OAuth metadata base URL comes from forwarded host headers** — `crates/platform/src/http/oauth_handlers.rs:289-299`

The public origin is constructed from request headers rather than a configured trusted hostname.

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
```

**The derived base URL feeds OAuth metadata output** — `crates/platform/src/http/oauth_handlers.rs:647-666`

The request-derived base URL becomes the advertised issuer and OAuth endpoint set.

```rust
pub async fn oauth_metadata<P: McpProxyPort + 'static>(
    State(_state): State<AppState<P>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let base_url = resolve_base_url(&headers);
    tracing::info!(/* ... */);
    Json(json!({
        "issuer": base_url,
        "authorization_endpoint": format!("{base_url}/oauth/authorize"),
        "token_endpoint": format!("{base_url}/oauth/token"),
        /* ... */
    }))
```

**Bearer challenge reflects the derived base URL into `resource_metadata`** — `crates/platform/src/http/mcp_handlers.rs:137-145`

Unauthenticated bearer challenges publish attacker-influenced metadata URLs before Brain3 authenticates the caller.

```rust
fn proxy_error_response(err: ProxyError, headers: &HeaderMap) -> Response {
    match err {
        ProxyError::Unauthorized(desc) => {
            let base_url = resolve_base_url(headers);
            let www_authenticate = format!(
                r#"Bearer error="invalid_token", error_description="{desc}", resource_metadata="{}""#,
                resource_metadata_url(&base_url)
            );
```

#### Reachability

The issue is reachable before authentication on real gateway routes. It does not require the attacker to compromise a confidential client first.

- **Attacker:** unauthenticated internet client on a public Brain3 deployment

- **Entry point:** `/oauth/metadata` and unauthorized `/mcp` responses

- **Outcome:** clients that trust Brain3’s emitted metadata can be pointed at attacker-chosen origins or follow-up metadata URLs

#### Severity

**Medium** — The bug sits on a real OAuth trust boundary and is reachable before authentication, but it misdirects metadata consumers rather than directly minting tokens or bypassing authorization.

Severity would rise if public clients automatically follow Brain3 metadata without an operator trust check, and would fall if Brain3 binds metadata output to a configured public origin instead of request headers.

#### Remediation

Bind metadata and bearer-challenge output to a configured public origin or trusted proxy configuration instead of request-supplied host headers.

Tests:
- Add a metadata test that ignores attacker-supplied `Host`/`X-Forwarded-Host` values and emits the configured public origin.
- Add a 401 challenge test that `resource_metadata` matches the configured public origin even when request host headers differ.

Preventive controls:
- Centralize public-origin resolution behind a trusted configuration source.
- Keep host-header validation and metadata emission on the same invariant path.

<a id="finding-2"></a>

### [2] OAuth authorization accepts arbitrary redirect URIs for the preregistered client

| Field | Value |
| --- | --- |
| Severity | medium |
| Confidence | high |
| Confidence rationale | The authorize path and registrar binding logic both show the missing redirect allowlist directly, and the surviving attack path does not depend on speculative code paths. |
| Category | Open redirect / OAuth redirect URI trust |
| CWE | CWE-601 |
| Affected lines | crates/platform/src/http/oauth_handlers.rs:347-374, crates/platform/src/http/registrar.rs:32-45 |

#### Summary

Brain3 checks that the caller uses the fixed `client_id`, but it only requires a non-empty `redirect_uri` and then binds the caller-supplied URI directly into the authorization flow.

#### Root Cause

The violated invariant is that a confidential client’s redirect endpoint must still be constrained to Brain3-approved callback URLs. Brain3 instead treats any non-empty runtime `redirect_uri` as authoritative once the caller uses the fixed `client_id`.

**Authorize validation only rejects an empty redirect URI** — `crates/platform/src/http/oauth_handlers.rs:347-374`

Brain3 validates `response_type`, the fixed `client_id`, and non-emptiness, but it does not constrain the callback origin or path.

```rust
fn validate_authorize_params(
    params: &LoginFormParams,
    config: &brain3_core::domain::model::GatewayConfig,
) -> Result<(), Response> {
    if params.response_type != "code" { /* ... */ }

    if params.client_id.is_empty() || params.client_id != config.oauth.client_id { /* ... */ }

    if params.redirect_uri.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_request", "error_description": "redirect_uri required"})),
        )
            .into_response());
    }
```

**Registrar preserves the submitted redirect URI unchanged** — `crates/platform/src/http/registrar.rs:32-45`

Once the client id matches, Brain3 binds whichever redirect URI the caller supplied instead of comparing it against an approved set.

```rust
fn bound_redirect<'a>(&self, bound: ClientUrl<'a>) -> Result<BoundClient<'a>, RegistrarError> {
    if bound.client_id.as_ref() != self.client_id {
        /* ... */
        return Err(RegistrarError::Unspecified);
    }
    let redirect_uri = bound.redirect_uri.ok_or(RegistrarError::Unspecified)?;
    Ok(BoundClient {
        client_id: bound.client_id,
        redirect_uri: Cow::Owned(redirect_uri.into_owned().into()),
    })
```

#### Validation

Validation traced the authorization request from parameter checks into the registrar binding step and found no allowlist, hostname restriction, or exact callback matching in Brain3-owned code.

Validation method: static source trace

**Authorize validation only rejects an empty redirect URI** — `crates/platform/src/http/oauth_handlers.rs:347-374`

Brain3 validates `response_type`, the fixed `client_id`, and non-emptiness, but it does not constrain the callback origin or path.

```rust
fn validate_authorize_params(
    params: &LoginFormParams,
    config: &brain3_core::domain::model::GatewayConfig,
) -> Result<(), Response> {
    if params.response_type != "code" { /* ... */ }

    if params.client_id.is_empty() || params.client_id != config.oauth.client_id { /* ... */ }

    if params.redirect_uri.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_request", "error_description": "redirect_uri required"})),
        )
            .into_response());
    }
```

**Registrar preserves the submitted redirect URI unchanged** — `crates/platform/src/http/registrar.rs:32-45`

Once the client id matches, Brain3 binds whichever redirect URI the caller supplied instead of comparing it against an approved set.

```rust
fn bound_redirect<'a>(&self, bound: ClientUrl<'a>) -> Result<BoundClient<'a>, RegistrarError> {
    if bound.client_id.as_ref() != self.client_id {
        /* ... */
        return Err(RegistrarError::Unspecified);
    }
    let redirect_uri = bound.redirect_uri.ok_or(RegistrarError::Unspecified)?;
    Ok(BoundClient {
        client_id: bound.client_id,
        redirect_uri: Cow::Owned(redirect_uri.into_owned().into()),
    })
```

#### Dataflow

Authorization request parameters -\> `validate_authorize_params()` -\> `GatewayRegistrar::bound_redirect()` -\> authorization grant redirect URI -\> code delivery to callback

- **Source:** caller-controlled `redirect_uri` in the authorize request

- **Sink:** bound authorization redirect URI

- **Outcome:** authorization codes are delivered to attacker-chosen callback endpoints for the configured client

**Authorize validation only rejects an empty redirect URI** — `crates/platform/src/http/oauth_handlers.rs:347-374`

Brain3 validates `response_type`, the fixed `client_id`, and non-emptiness, but it does not constrain the callback origin or path.

```rust
fn validate_authorize_params(
    params: &LoginFormParams,
    config: &brain3_core::domain::model::GatewayConfig,
) -> Result<(), Response> {
    if params.response_type != "code" { /* ... */ }

    if params.client_id.is_empty() || params.client_id != config.oauth.client_id { /* ... */ }

    if params.redirect_uri.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_request", "error_description": "redirect_uri required"})),
        )
            .into_response());
    }
```

**Registrar preserves the submitted redirect URI unchanged** — `crates/platform/src/http/registrar.rs:32-45`

Once the client id matches, Brain3 binds whichever redirect URI the caller supplied instead of comparing it against an approved set.

```rust
fn bound_redirect<'a>(&self, bound: ClientUrl<'a>) -> Result<BoundClient<'a>, RegistrarError> {
    if bound.client_id.as_ref() != self.client_id {
        /* ... */
        return Err(RegistrarError::Unspecified);
    }
    let redirect_uri = bound.redirect_uri.ok_or(RegistrarError::Unspecified)?;
    Ok(BoundClient {
        client_id: bound.client_id,
        redirect_uri: Cow::Owned(redirect_uri.into_owned().into()),
    })
```

#### Reachability

The attacker must control or compromise the single preregistered confidential client. Within that trust boundary, Brain3 hands the attacker the callback sink instead of enforcing an allowlist.

- **Attacker:** malicious or compromised preregistered confidential client

- **Entry point:** `/oauth/authorize` request for the fixed client id

- **Outcome:** Brain3 delivers the code to the attacker-controlled callback, and the same client can then redeem it with the configured secret

#### Severity

**Medium** — Exploitation requires control of the preregistered confidential client or its secret, but within that threat model the bug lets Brain3 deliver authorization codes to attacker-chosen callback endpoints.

Severity would rise if Brain3 adds more client identities or broader token privileges without tightening redirect binding, and would fall if redirect URIs are pinned to an explicit allowlist.

#### Remediation

Bind the preregistered client to an explicit allowlist of approved redirect URIs or approved callback origins instead of trusting runtime-supplied redirect values.

Tests:
- Add an authorize-flow test that rejects an unapproved `redirect_uri` even when `client_id` matches.
- Add a token-flow regression test that only previously approved redirect URIs can be bound and redeemed.

Preventive controls:
- Keep redirect binding policy in Brain3-owned configuration rather than in client-supplied request parameters.
- Re-review redirect policy before introducing any multi-client or broader-scope OAuth modes.

<a id="finding-3"></a>

### [3] Cloudflare quick tunnel is enabled by default on first run

| Field | Value |
| --- | --- |
| Severity | medium |
| Confidence | high |
| Confidence rationale | The default is directly visible in first-run setup, env parsing, and the checked-in template, with no material ambiguity about the resulting posture. |
| Category | Insecure default configuration |
| CWE | CWE-1188 |
| Affected lines | crates/core/src/application/first_run_setup.rs:30-40, crates/platform/src/config/env_file.rs:428-437, .env.template:47-50 |

#### Summary

Brain3’s first-run setup and environment fallback both default to `CloudflareQuick`, so a new installation can become internet-reachable without an explicit remote-access opt-in.

#### Root Cause

The violated invariant is that remote ingress should require an explicit opt-in because Brain3 treats public tunneling as its highest-risk boundary. The default setup and env parsing paths instead select the public quick tunnel automatically.

**First-run setup seeds Cloudflare quick tunnel mode** — `crates/core/src/application/first_run_setup.rs:30-40`

A fresh setup draft is initialized with `CloudflareQuick` before the operator explicitly chooses a local-only mode.

```rust
let draft = SetupDraftConfig {
    gateway_port: DEFAULT_GATEWAY_PORT,
    client_id: DEFAULT_CLIENT_ID.to_string(),
    client_secret: self
        .port
        .generate_secret_hex(DEFAULT_GENERATED_SECRET_BYTES)?,
    access_token_lifetime_secs: DEFAULT_ACCESS_TOKEN_LIFETIME_SECS,
    refresh_token_lifetime_secs: DEFAULT_REFRESH_TOKEN_LIFETIME_SECS,
    username: DEFAULT_USERNAME.to_string(),
    password: String::new(),
    tunnel_mode: TunnelModeDraft::CloudflareQuick,
```

**Env-file parsing defaults `B3_CF_QUICK_TUNNEL` to true** — `crates/platform/src/config/env_file.rs:428-437`

When the operator leaves the quick-tunnel flag unset, Brain3 still resolves to a public quick tunnel instead of local-only mode.

```rust
// Default: quick tunnel unless explicitly disabled.
let quick_default = env_bool("B3_CF_QUICK_TUNNEL", true);
if quick_default {
    tracing::info!("B3_CF_QUICK_TUNNEL defaulting to true — using Cloudflare quick tunnel");
    return Ok(Some(TunnelConfig::CloudflareQuick {
        local_port: gateway_port,
    }));
}

tracing::info!("B3_CF_QUICK_TUNNEL=false and no named tunnel vars set — no tunnel configured");
```

#### Validation

The source trace followed setup draft generation into env-file parsing and the shipped template, showing a consistent default to `CloudflareQuick` when tunneling is not explicitly disabled.

Validation method: static source trace

**First-run setup seeds Cloudflare quick tunnel mode** — `crates/core/src/application/first_run_setup.rs:30-40`

A fresh setup draft is initialized with `CloudflareQuick` before the operator explicitly chooses a local-only mode.

```rust
let draft = SetupDraftConfig {
    gateway_port: DEFAULT_GATEWAY_PORT,
    client_id: DEFAULT_CLIENT_ID.to_string(),
    client_secret: self
        .port
        .generate_secret_hex(DEFAULT_GENERATED_SECRET_BYTES)?,
    access_token_lifetime_secs: DEFAULT_ACCESS_TOKEN_LIFETIME_SECS,
    refresh_token_lifetime_secs: DEFAULT_REFRESH_TOKEN_LIFETIME_SECS,
    username: DEFAULT_USERNAME.to_string(),
    password: String::new(),
    tunnel_mode: TunnelModeDraft::CloudflareQuick,
```

**Env-file parsing defaults `B3_CF_QUICK_TUNNEL` to true** — `crates/platform/src/config/env_file.rs:428-437`

When the operator leaves the quick-tunnel flag unset, Brain3 still resolves to a public quick tunnel instead of local-only mode.

```rust
// Default: quick tunnel unless explicitly disabled.
let quick_default = env_bool("B3_CF_QUICK_TUNNEL", true);
if quick_default {
    tracing::info!("B3_CF_QUICK_TUNNEL defaulting to true — using Cloudflare quick tunnel");
    return Ok(Some(TunnelConfig::CloudflareQuick {
        local_port: gateway_port,
    }));
}

tracing::info!("B3_CF_QUICK_TUNNEL=false and no named tunnel vars set — no tunnel configured");
```

**The shipped env template documents quick tunnel as the default** — `.env.template:47-50`

The template reinforces that operators start from an internet-reachable tunnel unless they override it.

```dotenv
# Option A: Quick tunnel — no DNS or config needed, URL changes on each restart.
# Default: true. The gateway will start cloudflared automatically on startup.
# Set to false (and leave B3_CF_TUNNEL_NAME/B3_CF_DOMAIN empty) to disable all tunneling.
B3_CF_QUICK_TUNNEL=true
```

#### Dataflow

First-run draft or unset env flag -\> `TunnelModeDraft::CloudflareQuick` / `TunnelConfig::CloudflareQuick` -\> cloudflared startup -\> public gateway ingress

- **Source:** first-run defaults and unset tunnel configuration

- **Sink:** public Cloudflare tunnel startup

- **Outcome:** gateway OAuth, metadata, login, and health routes become internet-reachable without an explicit remote-access opt-in

**First-run setup seeds Cloudflare quick tunnel mode** — `crates/core/src/application/first_run_setup.rs:30-40`

A fresh setup draft is initialized with `CloudflareQuick` before the operator explicitly chooses a local-only mode.

```rust
let draft = SetupDraftConfig {
    gateway_port: DEFAULT_GATEWAY_PORT,
    client_id: DEFAULT_CLIENT_ID.to_string(),
    client_secret: self
        .port
        .generate_secret_hex(DEFAULT_GENERATED_SECRET_BYTES)?,
    access_token_lifetime_secs: DEFAULT_ACCESS_TOKEN_LIFETIME_SECS,
    refresh_token_lifetime_secs: DEFAULT_REFRESH_TOKEN_LIFETIME_SECS,
    username: DEFAULT_USERNAME.to_string(),
    password: String::new(),
    tunnel_mode: TunnelModeDraft::CloudflareQuick,
```

**Env-file parsing defaults `B3_CF_QUICK_TUNNEL` to true** — `crates/platform/src/config/env_file.rs:428-437`

When the operator leaves the quick-tunnel flag unset, Brain3 still resolves to a public quick tunnel instead of local-only mode.

```rust
// Default: quick tunnel unless explicitly disabled.
let quick_default = env_bool("B3_CF_QUICK_TUNNEL", true);
if quick_default {
    tracing::info!("B3_CF_QUICK_TUNNEL defaulting to true — using Cloudflare quick tunnel");
    return Ok(Some(TunnelConfig::CloudflareQuick {
        local_port: gateway_port,
    }));
}

tracing::info!("B3_CF_QUICK_TUNNEL=false and no named tunnel vars set — no tunnel configured");
```

**The shipped env template documents quick tunnel as the default** — `.env.template:47-50`

The template reinforces that operators start from an internet-reachable tunnel unless they override it.

```dotenv
# Option A: Quick tunnel — no DNS or config needed, URL changes on each restart.
# Default: true. The gateway will start cloudflared automatically on startup.
# Set to false (and leave B3_CF_TUNNEL_NAME/B3_CF_DOMAIN empty) to disable all tunneling.
B3_CF_QUICK_TUNNEL=true
```

#### Reachability

No attacker interaction is needed to widen exposure. The risk materializes when an operator accepts defaults on a new deployment or leaves the tunnel flag unset.

- **Attacker:** internet-origin attacker after default public deployment

- **Entry point:** public gateway routes made reachable by the quick tunnel

- **Outcome:** any present or future gateway logic bug sits on a broader remote attack surface than a local-only operator may expect

#### Severity

**Medium** — This default materially broadens attacker reachability at the project’s primary threat boundary, but it does not by itself bypass Brain3’s OAuth or hostname controls.

Severity would rise if unauthenticated or weakly authenticated gateway routes remain exposed remotely, and would fall if Brain3 switches to a local-only default with an explicit remote-access confirmation step.

#### Remediation

Make local-only operation the default and require an explicit operator action before Brain3 starts any Cloudflare tunnel.

Tests:
- Add an integration test that a fresh setup draft resolves to no tunnel until the operator explicitly selects a tunnel mode.
- Add a config test that an unset `B3_CF_QUICK_TUNNEL` value produces local-only startup instead of `CloudflareQuick`.

Preventive controls:
- Keep remote-access mode changes behind explicit operator confirmation.
- Require threat-model updates before adding new public-ingress defaults.

<a id="finding-4"></a>

### [4] Trace logging can record MCP request and response bodies to temp-backed logs

| Field | Value |
| --- | --- |
| Severity | low |
| Confidence | medium |
| Confidence rationale | The body logging is explicit in source, but practical exposure still depends on log level, operator behavior, and host-file permissions. |
| Category | Sensitive data exposure through logs |
| CWE | CWE-532 |
| Affected lines | crates/core/src/application/proxy_mcp.rs:104-106, crates/core/src/application/proxy_mcp.rs:132-134, brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py:242-250, brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py:260-269, crates/platform/src/setup/system.rs:242-263 |

#### Summary

Both the Rust gateway and the Python MCP server log full MCP request and response bodies at trace level, and the gateway writes logs to a temp file whose permissions are not explicitly tightened after creation.

#### Root Cause

The violated invariant is that Brain3 should allow verbose diagnostics without logging secrets or vault content. The gateway and Python MCP server instead log full MCP bodies, and the gateway writes them to temp-backed log files without an explicit permission clamp.

**Gateway traces MCP request bodies** — `crates/core/src/application/proxy_mcp.rs:104-106`

The gateway writes the request body itself into trace logs instead of a redacted summary.

```rust
tracing::trace!(
    body = %String::from_utf8_lossy(&body[..body.len().min(1024)]),
    "MCP proxy: request body"
```

**Gateway traces MCP response bodies** — `crates/core/src/application/proxy_mcp.rs:132-134`

The same gateway path logs response bodies, which can include vault content or tool output.

```rust
tracing::trace!(
    body = %String::from_utf8_lossy(&response.body[..response.body.len().min(1024)]),
    "MCP proxy: response body"
```

**Python MCP server traces full request bodies** — `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py:242-250`

The Python server reconstructs and logs the full decoded request body when trace logging is enabled.

```python
if trace_enabled and message["type"] == "http.request":
    request_body_chunks.append(message.get("body", b""))
    if not message.get("more_body", False):
        logger.log(
            TRACE,
            "MCP request body method=%s path=%s body=%s",
            scope.get("method", "<unknown>"),
            path,
            b"".join(request_body_chunks).decode("utf-8", errors="replace"),
```

**Python MCP server traces full response bodies** — `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py:260-269`

The Python server also logs full decoded response bodies, which can contain vault note text or other sensitive tool output.

```python
if trace_enabled and message["type"] == "http.response.body":
    response_body_chunks.append(message.get("body", b""))
    if not message.get("more_body", False):
        logger.log(
            TRACE,
            "MCP response body method=%s path=%s status=%s body=%s",
            scope.get("method", "<unknown>"),
            path,
            response_status,
            b"".join(response_body_chunks).decode("utf-8", errors="replace"),
```

**Gateway temp log file is created without an explicit permission clamp** — `crates/platform/src/setup/system.rs:242-263`

The gateway allocates a temp log file but does not apply an explicit `0600` permission fixup after creation, so actual access depends on host defaults.

```rust
async fn create_temp_log_file(&self) -> Result<PathBuf, SetupError> {
    let temp_dir = env::temp_dir();
    /* ... */
    let path = temp_dir.join(format!("brain3-{suffix}.log"));
    match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .await
    {
        Ok(_) => return Ok(path),
```

#### Validation

Validation traced both logging implementations and the gateway log-file allocation path, confirming that body bytes are serialized directly and temp log-file permissions are left to host defaults.

Validation method: static source trace

**Gateway traces MCP request bodies** — `crates/core/src/application/proxy_mcp.rs:104-106`

The gateway writes the request body itself into trace logs instead of a redacted summary.

```rust
tracing::trace!(
    body = %String::from_utf8_lossy(&body[..body.len().min(1024)]),
    "MCP proxy: request body"
```

**Gateway traces MCP response bodies** — `crates/core/src/application/proxy_mcp.rs:132-134`

The same gateway path logs response bodies, which can include vault content or tool output.

```rust
tracing::trace!(
    body = %String::from_utf8_lossy(&response.body[..response.body.len().min(1024)]),
    "MCP proxy: response body"
```

**Python MCP server traces full request bodies** — `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py:242-250`

The Python server reconstructs and logs the full decoded request body when trace logging is enabled.

```python
if trace_enabled and message["type"] == "http.request":
    request_body_chunks.append(message.get("body", b""))
    if not message.get("more_body", False):
        logger.log(
            TRACE,
            "MCP request body method=%s path=%s body=%s",
            scope.get("method", "<unknown>"),
            path,
            b"".join(request_body_chunks).decode("utf-8", errors="replace"),
```

**Python MCP server traces full response bodies** — `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py:260-269`

The Python server also logs full decoded response bodies, which can contain vault note text or other sensitive tool output.

```python
if trace_enabled and message["type"] == "http.response.body":
    response_body_chunks.append(message.get("body", b""))
    if not message.get("more_body", False):
        logger.log(
            TRACE,
            "MCP response body method=%s path=%s status=%s body=%s",
            scope.get("method", "<unknown>"),
            path,
            response_status,
            b"".join(response_body_chunks).decode("utf-8", errors="replace"),
```

**Gateway temp log file is created without an explicit permission clamp** — `crates/platform/src/setup/system.rs:242-263`

The gateway allocates a temp log file but does not apply an explicit `0600` permission fixup after creation, so actual access depends on host defaults.

```rust
async fn create_temp_log_file(&self) -> Result<PathBuf, SetupError> {
    let temp_dir = env::temp_dir();
    /* ... */
    let path = temp_dir.join(format!("brain3-{suffix}.log"));
    match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .await
    {
        Ok(_) => return Ok(path),
```

#### Dataflow

MCP request/response body -\> trace logging in gateway or Python server -\> temp-backed gateway log or process logs -\> local reader or shared support artifact

- **Source:** MCP request and response bodies, including vault content and tool payloads

- **Sink:** trace logs written to local log files or process logs

- **Outcome:** vault content and sensitive tool data can be exposed outside the intended trust boundary

**Gateway traces MCP request bodies** — `crates/core/src/application/proxy_mcp.rs:104-106`

The gateway writes the request body itself into trace logs instead of a redacted summary.

```rust
tracing::trace!(
    body = %String::from_utf8_lossy(&body[..body.len().min(1024)]),
    "MCP proxy: request body"
```

**Python MCP server traces full request bodies** — `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py:242-250`

The Python server reconstructs and logs the full decoded request body when trace logging is enabled.

```python
if trace_enabled and message["type"] == "http.request":
    request_body_chunks.append(message.get("body", b""))
    if not message.get("more_body", False):
        logger.log(
            TRACE,
            "MCP request body method=%s path=%s body=%s",
            scope.get("method", "<unknown>"),
            path,
            b"".join(request_body_chunks).decode("utf-8", errors="replace"),
```

**Gateway temp log file is created without an explicit permission clamp** — `crates/platform/src/setup/system.rs:242-263`

The gateway allocates a temp log file but does not apply an explicit `0600` permission fixup after creation, so actual access depends on host defaults.

```rust
async fn create_temp_log_file(&self) -> Result<PathBuf, SetupError> {
    let temp_dir = env::temp_dir();
    /* ... */
    let path = temp_dir.join(format!("brain3-{suffix}.log"));
    match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .await
    {
        Ok(_) => return Ok(path),
```

#### Reachability

The issue is local rather than remotely triggerable on its own. It matters when operators enable verbose logging or share logs while diagnosing real MCP traffic.

- **Attacker:** local principal or downstream log recipient

- **Entry point:** trace-level gateway and MCP server logging

- **Outcome:** sensitive vault contents or tool outputs can leak through logs

#### Severity

**Low** — The issue is local and requires verbose logging or log sharing, but it directly violates Brain3’s own rule that sensitive data must never be logged.

Severity would rise if trace logging is routinely enabled in production or logs are shipped off-host automatically, and would fall if body logging is removed or aggressively redacted and log-file permissions are clamped.

#### Remediation

Remove full-body logging from both MCP implementations, log only bounded metadata, and explicitly clamp gateway log-file permissions to `0600`.

Tests:
- Add a focused logging regression test or helper-level assertion that trace logging never serializes raw MCP bodies.
- Add a platform test that created gateway log files receive explicit owner-only permissions on Unix-like hosts.

Preventive controls:
- Redact or hash secret-bearing values before they reach structured logs.
- Treat log sinks as sensitive local storage with explicit permission hardening.

## Reviewed Surfaces

| Surface | Risk Area | Outcome | Notes |
| --- | --- | --- | --- |
| Tunnel bootstrap and setup defaults | Public ingress by default | Reported | First-run setup and env parsing default Brain3 to a Cloudflare quick tunnel when tunneling is not explicitly disabled. Evidence: artifacts/03_coverage/repository_coverage_ledger.md, artifacts/05_findings/brain3-quick-tunnel-default-public-ingress/candidate_ledger.jsonl, artifacts/05_findings/brain3-quick-tunnel-default-public-ingress/validation_report.md, artifacts/05_findings/brain3-quick-tunnel-default-public-ingress/attack_path_analysis_report.md |
| OAuth redirect URI binding policy | Redirect URI allowlisting | Reported | The preregistered client id is fixed, but Brain3 accepts caller-supplied redirect URIs and binds them into the authorization flow. Evidence: artifacts/03_coverage/repository_coverage_ledger.md, artifacts/05_findings/brain3-unallowlisted-redirect-uri/candidate_ledger.jsonl, artifacts/05_findings/brain3-unallowlisted-redirect-uri/validation_report.md, artifacts/05_findings/brain3-unallowlisted-redirect-uri/attack_path_analysis_report.md |
| OAuth metadata and bearer challenge metadata | Host/header trust | Reported | Metadata output and 401 bearer challenges derive public URLs from request-supplied forwarded host values instead of a configured public origin. Evidence: artifacts/03_coverage/repository_coverage_ledger.md, artifacts/05_findings/brain3-host-header-metadata/candidate_ledger.jsonl, artifacts/05_findings/brain3-host-header-metadata/validation_report.md, artifacts/05_findings/brain3-host-header-metadata/attack_path_analysis_report.md |
| Gateway and MCP logging | Sensitive data in logs | Reported | Both the gateway and the Python MCP server log full MCP bodies at trace level, and gateway temp-log permissions are not explicitly clamped after creation. Evidence: artifacts/03_coverage/repository_coverage_ledger.md, artifacts/05_findings/brain3-sensitive-mcp-trace-logs/candidate_ledger.jsonl, artifacts/05_findings/brain3-sensitive-mcp-trace-logs/validation_report.md, artifacts/05_findings/brain3-sensitive-mcp-trace-logs/attack_path_analysis_report.md |
| OAuth registration surface | Public-client or DCR expansion | Not applicable | No `/oauth/register` route or public-client token flow was present in the checked revision. Evidence: artifacts/03_coverage/repository_coverage_ledger.md |
| Named tunnel ingress config writer | Accidental proxy to unrelated localhost ports | Rejected | The checked-in example and config writer both pin ingress to the loopback gateway port and terminate with `http_status:404`. Evidence: artifacts/03_coverage/repository_coverage_ledger.md |
| Vault filesystem path controls | Path traversal / vault escape | Rejected | Vault path resolution rejects null bytes, dot-prefixed components, and paths that resolve outside the configured vault root. Evidence: artifacts/03_coverage/repository_coverage_ledger.md |
| OAuth authorization-code lifetime | Replay window / architectural code lifetime | Needs follow-up | The underlying `oxide-auth` authorizer still mints 10-minute authorization codes. This remains open, but stronger directly Brain3-owned policy issues took priority in this pass. Evidence: artifacts/03_coverage/repository_coverage_ledger.md |
| Cloudflare credential-file permissions | Local credential exposure | Needs follow-up | Named-tunnel credential lookup uses `~/.cloudflared/<id>.json` without an explicit permission check in the reviewed revision. Evidence: artifacts/03_coverage/repository_coverage_ledger.md |
| Local secret storage and token retention | Plaintext local secrets | Needs follow-up | `.env`, the upstream shared secret, and the SQLite token database remain local plaintext storage surfaces with partial mitigations but without a stronger system secret store. Evidence: artifacts/03_coverage/repository_coverage_ledger.md |

## Previously Tracked Open Findings

These findings were open in prior audits and are not fully covered by the Codex scan findings above. They are carried forward to prevent silent loss.

| ID | Severity | Area | Summary | Status |
| --- | --- | --- | --- | --- |
| M-5 | 🟡 Medium | Tunnel | Quick tunnel disables all hostname enforcement | Open |
| M-6 | 🟡 Medium | Tunnel | Cloudflare credentials file permissions not verified | Needs follow-up (see Reviewed Surfaces) |
| M-9 | 🟡 Medium | Credentials | Default username is predictable (`"admin"`) | Open |
| M-10 | 🟡 Medium | Credentials | 7-character secret prefix logged in tracing output | Needs follow-up (see Reviewed Surfaces) |
| M-12 | 🟡 Medium | Architecture | Gateway process is unsandboxed — full host access if compromised | Open |
| L-2 | 🟢 Low | OAuth2 | No Content-Security-Policy or security headers on login page | Open |
| L-10 | 🟢 Low | Ops | No vulnerability disclosure policy / root-level `SECURITY.md` | Open |

### M-5 — Quick Tunnel Disables All Hostname Enforcement

**File:** `crates/platform/src/config/env_file.rs`

When `B3_CF_QUICK_TUNNEL=true` (or when no named tunnel is configured), the expected host resolves to `None`, so `validate_host()` is a no-op. Any request with any `Host` header is accepted. This compounds the host-header injection issue (Codex finding #1) because there is no configured hostname to compare against.

**Recommendation:** Document this trade-off prominently. When using a quick tunnel, consider parsing the `cloudflared` stdout URL and using it for at minimum warning-level logging.

### M-6 — Cloudflare Credentials File Permissions Not Verified

**File:** `crates/platform/src/tunnel/cloudflare_setup.rs`

The named tunnel credentials file (`~/.cloudflared/<id>.json`) grants full tunnel control but is used without verifying Unix permissions are `0600` or stricter. A world-readable credentials file on a shared system is a silent security failure.

**Recommendation:** On startup, check the credentials file mode with `std::os::unix::fs::MetadataExt::mode()` and warn or refuse to start if permissions are looser than `0600`.

### M-9 — Default Username is `"admin"` — Predictable

**File:** `crates/core/src/domain/setup.rs`

`DEFAULT_USERNAME` is `"admin"`, which removes one layer of defense-in-depth. The setup wizard prompts users to change it, but the constant still defaults to `"admin"`.

**Recommendation:** Change `DEFAULT_USERNAME` to `"brain3"` or a random value such as `"user-<4-chars>"`.

### M-10 — 7-Character Secret Prefix Logged in Tracing Output

**File:** `crates/platform/src/config/upstream_secret.rs`

The first 7 characters of the upstream shared secret are written to tracing output via `secret_hint`. The `elide_secret()` helper is used correctly elsewhere but was not applied here.

**Recommendation:** Replace `&secret[..secret.len().min(7)]` with `elide_secret(&secret)` on both log call sites.

### M-12 — Gateway Process Is Unsandboxed

**Files:** N/A — absence of a control

The Rust gateway runs as a normal OS process with no filesystem jail, no network egress restriction, and no capability dropping. If compromised, the attacker inherits full filesystem and network access of the user account running Brain3. This is a known accepted trade-off; the host process currently needs broad access (vault, `cloudflared`, container runtime API).

**Recommendation (deferred):** Investigate Landlock (Linux 5.13+) or a macOS sandbox profile to restrict the gateway to only the paths it actually needs.

### L-2 — No Content-Security-Policy or Security Headers on Login Page

**Files:** `crates/platform/src/http/templates.rs`, `crates/platform/src/http/router.rs`

The login page is served without `Content-Security-Policy`, `X-Frame-Options`, `Referrer-Policy`, or `X-Content-Type-Options`. The login form embeds hidden fields containing `redirect_uri` and `code_challenge`, making XSS on this page especially damaging.

**Recommendation:** Add a `tower_http::set_header::SetResponseHeaderLayer` for HTML responses with at minimum:
```
Content-Security-Policy: default-src 'self'; style-src 'self'; img-src 'self' data:
X-Frame-Options: DENY
X-Content-Type-Options: nosniff
Referrer-Policy: no-referrer
```

### L-10 — No Vulnerability Disclosure Policy

There is no root-level `SECURITY.md`, no contact for security reports, and no discoverable threat-model document at a path GitHub or researchers would expect.

**Recommendation:** Add a `SECURITY.md` at the repo root with a contact email or private GitHub issue template, a link to this audit's threat model, and the supported scope.

---

## Open Questions And Follow Up

- How should Brain3 constrain prompt-injection risk when vault contents are not fully user-controlled, such as shared, synced, or third-party-imported vault material?
  - Follow-up prompt: Review the MCP read/search exposure model for untrusted vault content in `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py`, `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/tools/read.py`, and `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/tools/search.py`, then update `SECURITY_AUDIT.MD` threat-model guidance for non-user-controlled vault inputs.
- Can Brain3 move local secrets out of plaintext files and reduce credential hints in logs without breaking the low-friction setup flow?
  - Follow-up prompt: Re-review `crates/platform/src/config/upstream_secret.rs`, `crates/platform/src/token_store/sqlite.rs`, `crates/platform/src/setup/env_writer.rs`, and `brain3-mcp-vault-tools/src/brain3_mcp_vault_tools/server.py` for a follow-on hardening pass focused on secret-at-rest storage and log minimization.
- Are there any local-only setup/TUI flows that unintentionally expose credentials or change tunnel posture after the validated gateway defaults are fixed?
  - Follow-up prompt: Perform a dedicated local-UI review of `apps/gateway/src/main.rs`, `apps/gateway/src/setup_tui.rs`, `apps/gateway/src/tui/app.rs`, `apps/gateway/src/tui/screens.rs`, and `apps/gateway/src/tui/state.rs`, focusing on credential display, log mirroring, and tunnel-mode transitions.
- Large local-only setup/TUI surfaces were closed with targeted OAuth/tunnel/secret review instead of full line-by-line manual review in this pass.
  - Follow-up prompt: Review deferred unit setup-ui-local-review and close its stated proof gap. Paths: apps/gateway/src/main.rs, apps/gateway/src/setup_tui.rs, apps/gateway/src/tui/app.rs, apps/gateway/src/tui/runtime_logs.rs, apps/gateway/src/tui/screens.rs, apps/gateway/src/tui/state.rs.
- Authorization-code lifetime remains a third-party-library architectural question and was not promoted over stronger Brain3-owned issues in this pass.
  - Follow-up prompt: Review deferred unit oauth-code-lifetime and close its stated proof gap. Paths: apps/gateway/src/server.rs, crates/platform/src/http/state.rs. Surfaces: oauth-code-lifetime.
- Cloudflare credential-file permission enforcement still needs a dedicated follow-up review.
  - Follow-up prompt: Review deferred unit tunnel-credentials-perms and close its stated proof gap. Paths: crates/platform/src/tunnel/cloudflare_named.rs, crates/platform/src/tunnel/cloudflare_setup.rs. Surfaces: tunnel-credentials-perms.
- Local plaintext secret and token storage remain intentionally accepted but only partially mitigated design choices that merit a separate storage-hardening review.
  - Follow-up prompt: Review deferred unit local-credential-storage and close its stated proof gap. Paths: crates/platform/src/config/upstream_secret.rs, crates/platform/src/token_store/sqlite.rs, crates/platform/src/setup/env_writer.rs. Surfaces: local-credential-storage.
