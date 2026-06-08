# Rust Codebase Design: brain3

## Overview

Replace the Python proof-of-concept (`poc/oauth2-host-gw` + `poc/obsidian-mcp-container` orchestration scripts) with a Rust codebase using hexagonal architecture. The Rust binary will handle OAuth2 gateway, MCP reverse proxying, container lifecycle, and tunnel management — all the things the shell scripts and Python code do today, unified into one process.

---

## Milestone 1 Scope (This Design)

Port the **OAuth2 gateway + MCP reverse proxy** — the core functionality in `poc/oauth2-host-gw/src/oauth2_gateway/`. This is the part that:

1. Serves `/.well-known/oauth-authorization-server` metadata
2. Handles `/oauth/authorize` (login form + auth code issuance)
3. Handles `/oauth/token` (authorization code exchange with PKCE + client secret)
4. Serves `/.well-known/oauth-protected-resource/mcp` metadata
5. Reverse-proxies `/mcp{/...}` to an upstream MCP server with bearer token validation, host validation, and shared secret injection
6. Serves `/health`

Container lifecycle and tunnel management are **deferred** to later milestones — the Rust binary will initially be a drop-in replacement for the Python gateway only.

### Not in Milestone 1

- Tauri desktop app (`apps/desktop/`)
- TUI binary (`apps/tui/`)
- Container lifecycle management (ports/adapters exist but no adapter implementation)
- Tunnel lifecycle management (ports/adapters exist but no adapter implementation)
- Persistent token storage (remains static access token, matching PoC)

---

## Directory Layout

```
brain3/
├── Cargo.toml                          # workspace root
├── crates/
│   ├── core/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── domain/
│   │       │   ├── mod.rs
│   │       │   ├── model.rs            # GatewayConfig, ContainerConfig, TunnelConfig, etc.
│   │       │   ├── oauth.rs            # AuthCode, OAuthRequest, token validation logic
│   │       │   └── errors.rs           # Domain error types
│   │       ├── ports/
│   │       │   ├── mod.rs
│   │       │   ├── config.rs           # ConfigPort trait
│   │       │   ├── container.rs        # ContainerPort trait (stub for M1)
│   │       │   ├── tunnel.rs           # TunnelPort trait (stub for M1)
│   │       │   ├── mcp_proxy.rs        # McpProxyPort trait
│   │       │   └── auth_code_store.rs  # AuthCodeStore trait
│   │       └── application/
│   │           ├── mod.rs
│   │           ├── authorize.rs        # OAuth authorize use case
│   │           ├── token_exchange.rs   # OAuth token exchange use case
│   │           ├── validate_request.rs # Host validation, bearer token checking
│   │           └── proxy_mcp.rs        # MCP proxying use case
│   │
│   └── platform/
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── config/
│           │   ├── mod.rs
│           │   └── env_file.rs         # EnvFileConfigAdapter
│           ├── auth_code_store/
│           │   ├── mod.rs
│           │   └── in_memory.rs        # InMemoryAuthCodeStore
│           ├── mcp_proxy/
│           │   ├── mod.rs
│           │   └── reqwest_proxy.rs    # ReqwestMcpProxy adapter
│           ├── http/
│           │   ├── mod.rs
│           │   ├── router.rs           # Axum router wiring
│           │   ├── oauth_handlers.rs   # /oauth/* HTTP handlers
│           │   ├── mcp_handlers.rs     # /mcp/* HTTP handlers
│           │   ├── health.rs           # /health handler
│           │   └── templates.rs        # Login form HTML
│           ├── container/
│           │   └── mod.rs              # (empty stubs for M1)
│           └── tunnel/
│               └── mod.rs              # (empty stubs for M1)
│
├── apps/
│   └── gateway/
│       ├── Cargo.toml                  # Binary crate: brain3-gateway
│       └── src/
│           └── main.rs                 # Composition root
│
├── poc/                                # Existing Python PoC (unchanged)
│   └── ...
└── ...
```

### Why This Layout

- **2 shared crates** (`core` + `platform`) per the "final recommendation" in the arch spec. No premature splitting.
- **`apps/gateway/`** is the first binary. `apps/desktop/` and `apps/tui/` come later and will depend on `core` + `platform`.
- The `apps/gateway/main.rs` is the **composition root** — the only place that knows concrete adapter types and wires them together.
- `poc/` stays untouched. The Rust binary runs alongside it until feature parity is confirmed, then the PoC can be retired.

---

## Crate Dependency Graph

```
apps/gateway
    ├── core
    └── platform
            └── core
```

`core` has zero knowledge of `platform`. `platform` implements traits defined in `core`.

---

## Domain Model (`crates/core/src/domain/model.rs`)

These are pure data types with no infrastructure dependencies.

```rust
use std::path::PathBuf;

pub struct GatewayConfig {
    pub port: u16,
    pub host: String,
    pub oauth: OAuthConfig,
    pub proxy: MCPReverseProxyConfig,
    pub hostname_validation: HostnameValidationConfig,
}

pub struct OAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub access_token: String,
    pub pkce_required: bool,
    pub username: String,
    pub password: String,
}

pub struct MCPReverseProxyConfig {
    pub mcp_upstream_url: String,
    pub upstream_secret_file: PathBuf,
}

pub struct HostnameValidationConfig {
    pub expected_host: Option<String>,
    pub enforce: bool,
}

pub enum ContainerRuntime {
    Docker,
    MacOSContainer,
}

pub struct ContainerConfig {
    pub runtime: ContainerRuntime,
    pub image: String,
    pub bind_mounts: Vec<BindMount>,
}

pub struct BindMount {
    pub host_path: PathBuf,
    pub container_path: PathBuf,
    pub readonly: bool,
}

pub enum TunnelProvider {
    Cloudflare,
}

pub struct TunnelConfig {
    pub provider: TunnelProvider,
}
```

---

## Domain: OAuth (`crates/core/src/domain/oauth.rs`)

Pure business logic for OAuth operations — no HTTP, no framework types.

```rust
use std::time::{Duration, Instant};

pub struct AuthCodeData {
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub pkce_required: bool,
    pub expires_at: Instant,
}

pub struct AuthorizeRequest {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub state: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
}

pub struct TokenRequest {
    pub grant_type: String,
    pub client_id: String,
    pub client_secret: String,
    pub code: String,
    pub redirect_uri: Option<String>,
    pub code_verifier: Option<String>,
}

pub const AUTH_CODE_LIFETIME: Duration = Duration::from_secs(300);
pub const ACCESS_TOKEN_LIFETIME_SECS: u64 = 86400;

/// Validate authorize request parameters. Returns Err with an OAuth error code.
pub fn validate_authorize_request(
    req: &AuthorizeRequest,
    expected_client_id: &str,
    pkce_required: bool,
) -> Result<(), OAuthError> {
    if req.response_type != "code" {
        return Err(OAuthError::UnsupportedResponseType);
    }
    if req.client_id != expected_client_id {
        return Err(OAuthError::InvalidClient);
    }
    if req.redirect_uri.is_empty() {
        return Err(OAuthError::InvalidRequest("redirect_uri required".into()));
    }
    if pkce_required {
        match &req.code_challenge {
            None | Some(s) if s.is_empty() => {
                return Err(OAuthError::InvalidRequest("code_challenge required".into()));
            }
            _ => {}
        }
        match &req.code_challenge_method {
            Some(m) if m != "S256" => {
                return Err(OAuthError::InvalidRequest(
                    "code_challenge_method must be S256".into(),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Verify PKCE code_verifier against stored code_challenge (S256).
pub fn verify_pkce(code_verifier: &str, code_challenge: &str) -> bool {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(code_verifier.as_bytes());
    let computed = URL_SAFE_NO_PAD.encode(digest);
    constant_time_eq(computed.as_bytes(), code_challenge.as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    subtle::ConstantTimeEq::ct_eq(a, b).into()
}
```

Key design point: **all OAuth validation logic is pure functions in the domain**. The HTTP layer just parses form data into `AuthorizeRequest`/`TokenRequest` and calls these functions.

---

## Domain Errors (`crates/core/src/domain/errors.rs`)

```rust
#[derive(Debug)]
pub enum OAuthError {
    UnsupportedResponseType,
    InvalidClient,
    InvalidRequest(String),
    InvalidGrant(String),
    ServerError(String),
    UnsupportedGrantType,
}

#[derive(Debug)]
pub enum ProxyError {
    Unauthorized(String),
    MisdirectedRequest(String),
    BadGateway(String),
}

#[derive(Debug)]
pub enum ConfigError {
    Missing(String),
    Invalid(String),
    Conflict(String),
}
```

These are domain errors, not HTTP status codes. The HTTP adapter layer maps them to responses.

---

## Ports (`crates/core/src/ports/`)

### AuthCodeStore (`auth_code_store.rs`)

```rust
#[async_trait::async_trait]
pub trait AuthCodeStore: Send + Sync {
    async fn store(&self, code: String, data: AuthCodeData);
    async fn take(&self, code: &str) -> Option<AuthCodeData>;
    async fn cleanup_expired(&self);
}
```

This is a port (not inline in the domain) because the PoC uses an in-memory HashMap, but a future implementation could use SQLite, Redis, etc. The Milestone 1 adapter is `InMemoryAuthCodeStore`.

### McpProxyPort (`mcp_proxy.rs`)

```rust
pub struct McpProxyRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub struct McpProxyResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[async_trait::async_trait]
pub trait McpProxyPort: Send + Sync {
    async fn forward(&self, request: McpProxyRequest) -> Result<McpProxyResponse, ProxyError>;
}
```

The domain doesn't know about `reqwest`, `hyper`, or HTTP framing. Just method + URL + headers + body.

### ConfigPort (`config.rs`)

```rust
pub trait ConfigPort {
    fn load(&self) -> Result<GatewayConfig, ConfigError>;
}
```

### ContainerPort (`container.rs`) — Stub for M1

```rust
pub struct ContainerId(pub String);

#[async_trait::async_trait]
pub trait ContainerPort: Send + Sync {
    async fn exists(&self, id: &ContainerId) -> Result<bool, Box<dyn std::error::Error>>;
    async fn is_running(&self, id: &ContainerId) -> Result<bool, Box<dyn std::error::Error>>;
    async fn create(&self, config: &ContainerConfig) -> Result<ContainerId, Box<dyn std::error::Error>>;
    async fn start(&self, id: &ContainerId) -> Result<(), Box<dyn std::error::Error>>;
    async fn stop(&self, id: &ContainerId) -> Result<(), Box<dyn std::error::Error>>;
}
```

### TunnelPort (`tunnel.rs`) — Stub for M1

```rust
pub struct TunnelInfo {
    pub public_url: String,
}

pub enum TunnelStatus {
    Running(TunnelInfo),
    Stopped,
}

#[async_trait::async_trait]
pub trait TunnelPort: Send + Sync {
    async fn start(&self) -> Result<TunnelInfo, Box<dyn std::error::Error>>;
    async fn stop(&self) -> Result<(), Box<dyn std::error::Error>>;
    async fn status(&self) -> Result<TunnelStatus, Box<dyn std::error::Error>>;
}
```

---

## Application Use Cases (`crates/core/src/application/`)

Use cases orchestrate domain logic + ports. They are the service layer.

### `authorize.rs`

```rust
pub struct AuthorizeUseCase<S: AuthCodeStore> {
    config: Arc<OAuthConfig>,
    store: Arc<S>,
}

impl<S: AuthCodeStore> AuthorizeUseCase<S> {
    /// Validate the authorize request. On success, returns Ok(()) meaning
    /// "show the login form" (or issue the code if credentials are provided).
    pub fn validate(&self, req: &AuthorizeRequest) -> Result<(), OAuthError> {
        validate_authorize_request(req, &self.config.client_id, self.config.pkce_required)
    }

    pub fn login_configured(&self) -> bool {
        !self.config.username.is_empty() && !self.config.password.is_empty()
    }

    pub fn check_credentials(&self, username: &str, password: &str) -> bool {
        constant_time_eq(username.as_bytes(), self.config.username.as_bytes())
            && constant_time_eq(password.as_bytes(), self.config.password.as_bytes())
    }

    pub async fn issue_code(&self, req: &AuthorizeRequest) -> String {
        self.store.cleanup_expired().await;
        let code = generate_secure_token();
        let data = AuthCodeData {
            client_id: req.client_id.clone(),
            redirect_uri: req.redirect_uri.clone(),
            code_challenge: req.code_challenge.clone(),
            code_challenge_method: req.code_challenge_method.clone(),
            pkce_required: self.config.pkce_required,
            expires_at: Instant::now() + AUTH_CODE_LIFETIME,
        };
        self.store.store(code.clone(), data).await;
        code
    }
}
```

### `token_exchange.rs`

```rust
pub struct TokenExchangeUseCase<S: AuthCodeStore> {
    config: Arc<OAuthConfig>,
    store: Arc<S>,
}

impl<S: AuthCodeStore> TokenExchangeUseCase<S> {
    pub async fn exchange(&self, req: &TokenRequest) -> Result<TokenResponse, OAuthError> {
        if req.grant_type != "authorization_code" {
            return Err(OAuthError::UnsupportedGrantType);
        }

        self.store.cleanup_expired().await;

        if req.client_id != self.config.client_id {
            return Err(OAuthError::InvalidClient);
        }
        if self.config.client_secret.is_empty() {
            return Err(OAuthError::ServerError("client secret not configured".into()));
        }
        if !constant_time_eq(req.client_secret.as_bytes(), self.config.client_secret.as_bytes()) {
            return Err(OAuthError::InvalidClient);
        }

        let code_data = self.store.take(&req.code).await
            .ok_or(OAuthError::InvalidGrant("Invalid or expired code".into()))?;

        if !constant_time_eq(req.client_id.as_bytes(), code_data.client_id.as_bytes()) {
            return Err(OAuthError::InvalidGrant("client_id mismatch".into()));
        }

        // Redirect URI check
        if let Some(ref redirect_uri) = req.redirect_uri {
            if !redirect_uri.is_empty()
                && !code_data.redirect_uri.is_empty()
                && redirect_uri != &code_data.redirect_uri
            {
                return Err(OAuthError::InvalidGrant("redirect_uri mismatch".into()));
            }
        }

        // PKCE verification
        if let Some(ref challenge) = code_data.code_challenge {
            if !challenge.is_empty() {
                let verifier = req.code_verifier.as_deref().unwrap_or("");
                if verifier.is_empty() {
                    return Err(OAuthError::InvalidGrant("code_verifier required".into()));
                }
                if !verify_pkce(verifier, challenge) {
                    return Err(OAuthError::InvalidGrant("PKCE verification failed".into()));
                }
            }
        } else if code_data.pkce_required {
            return Err(OAuthError::InvalidGrant("code_challenge required".into()));
        }

        Ok(TokenResponse {
            access_token: self.config.access_token.clone(),
            token_type: "bearer".into(),
            expires_in: ACCESS_TOKEN_LIFETIME_SECS,
        })
    }
}
```

### `validate_request.rs`

```rust
pub fn validate_bearer_token(auth_header: &str, expected_token: &str) -> Result<(), ProxyError> {
    let (scheme, token) = auth_header.split_once(' ').unwrap_or(("", ""));
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
        return Err(ProxyError::Unauthorized("Missing or invalid bearer token".into()));
    }
    if expected_token.is_empty() || !constant_time_eq(token.as_bytes(), expected_token.as_bytes()) {
        return Err(ProxyError::Unauthorized("Missing or invalid bearer token".into()));
    }
    Ok(())
}

pub fn validate_host(
    request_host: &str,
    expected_host: Option<&str>,
    enforce: bool,
) -> Result<(), ProxyError> {
    if !enforce {
        return Ok(());
    }
    let Some(expected) = expected_host else {
        return Ok(());
    };
    if request_host == expected {
        return Ok(());
    }
    Err(ProxyError::MisdirectedRequest(
        "Request host does not match the configured public hostname".into(),
    ))
}
```

### `proxy_mcp.rs`

```rust
pub struct ProxyMcpUseCase<P: McpProxyPort> {
    proxy: Arc<P>,
    upstream_url: String,
    upstream_secret: String,
    access_token: String,
    hostname_validation: HostnameValidationConfig,
}

impl<P: McpProxyPort> ProxyMcpUseCase<P> {
    pub async fn handle(
        &self,
        request_host: &str,
        auth_header: &str,
        method: &str,
        path: &str,
        query: Option<&str>,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Result<McpProxyResponse, ProxyError> {
        validate_host(
            request_host,
            self.hostname_validation.expected_host.as_deref(),
            self.hostname_validation.enforce,
        )?;
        validate_bearer_token(auth_header, &self.access_token)?;

        let upstream_url = build_upstream_url(&self.upstream_url, path, query);
        let filtered_headers = filter_request_headers(headers);
        let mut final_headers = filtered_headers;
        final_headers.push((
            "x-brain3-upstream-secret".into(),
            self.upstream_secret.clone(),
        ));

        self.proxy.forward(McpProxyRequest {
            method: method.into(),
            url: upstream_url,
            headers: final_headers,
            body,
        }).await
    }
}
```

---

## Platform Adapters (`crates/platform/src/`)

### `config/env_file.rs` — EnvFileConfigAdapter

Reads `.env` file + environment variables, produces a `GatewayConfig`. Direct port of `config.py`. Uses the `dotenvy` crate.

Hostname resolution logic (ported from Python):
- If both `CF_TUNNEL_NAME`+`CF_DOMAIN` and `DIRECT_PUBLIC_ORIGIN_HOSTNAME` are set → error
- Named tunnel host = `{CF_TUNNEL_NAME}.{CF_DOMAIN}`
- Direct origin host = `DIRECT_PUBLIC_ORIGIN_HOSTNAME`
- Expected host = whichever is set (or None)

### `auth_code_store/in_memory.rs` — InMemoryAuthCodeStore

```rust
pub struct InMemoryAuthCodeStore {
    codes: RwLock<HashMap<String, AuthCodeData>>,
}
```

Direct port of `_auth_codes: dict[str, dict]` from `oauth.py`. Uses `tokio::sync::RwLock`.

### `mcp_proxy/reqwest_proxy.rs` — ReqwestMcpProxy

Implements `McpProxyPort` using `reqwest::Client`. Maintains a long-lived client (connection pooling, no timeout, no redirect following — matching the Python `httpx.AsyncClient` config).

### `http/` — Axum HTTP Adapter Layer

This is the **thin HTTP layer**. It:
1. Deserializes requests (query params, form data, headers)
2. Calls application use cases
3. Serializes responses (JSON, HTML, redirects)

Contains **no business logic**.

#### `router.rs`

```rust
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/.well-known/oauth-authorization-server", get(oauth_metadata))
        .route("/oauth/authorize", get(oauth_authorize).post(oauth_authorize))
        .route("/oauth/token", post(oauth_token))
        .route("/.well-known/oauth-protected-resource/mcp", get(protected_resource_metadata))
        .route("/mcp", get(mcp_proxy).post(mcp_proxy).delete(mcp_proxy))
        .route("/mcp/", get(mcp_proxy).post(mcp_proxy).delete(mcp_proxy))
        .route("/mcp/{*path}", get(mcp_proxy).post(mcp_proxy).delete(mcp_proxy))
        .with_state(state)
}
```

#### `AppState`

Holds `Arc`-wrapped use cases, injected from `main.rs`:

```rust
#[derive(Clone)]
pub struct AppState {
    pub authorize: Arc<AuthorizeUseCase<InMemoryAuthCodeStore>>,
    pub token_exchange: Arc<TokenExchangeUseCase<InMemoryAuthCodeStore>>,
    pub proxy_mcp: Arc<ProxyMcpUseCase<ReqwestMcpProxy>>,
    pub config: Arc<GatewayConfig>,
}
```

#### Handler Example: `oauth_handlers.rs::oauth_authorize`

```rust
async fn oauth_authorize(
    State(state): State<AppState>,
    method: Method,
    query: Query<HashMap<String, String>>,
    form: Option<Form<HashMap<String, String>>>,
) -> impl IntoResponse {
    let source = if method == Method::POST {
        form.unwrap().0
    } else {
        query.0
    };

    let req = AuthorizeRequest { /* parse from source */ };

    if let Err(e) = state.authorize.validate(&req) {
        return oauth_error_response(e);
    }

    if !state.authorize.login_configured() {
        return Html(MISCONFIGURED_PAGE.to_string()).into_response();
    }

    if method == Method::GET {
        return Html(render_login_form(&req, None)).into_response();
    }

    let username = source.get("username").cloned().unwrap_or_default();
    let password = source.get("password").cloned().unwrap_or_default();
    if !state.authorize.check_credentials(&username, &password) {
        return Html(render_login_form(&req, Some("Invalid username or password")))
            .into_response();
    }

    let code = state.authorize.issue_code(&req).await;
    redirect_with_code(&req.redirect_uri, &code, req.state.as_deref())
}
```

---

## Composition Root (`apps/gateway/src/main.rs`)

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::init();

    let args = parse_cli_args();

    // 1. Load config
    let config_adapter = EnvFileConfigAdapter::new()?;
    let config = Arc::new(config_adapter.load()?);

    // 2. Read upstream secret
    let upstream_secret = read_upstream_secret(&config.proxy.upstream_secret_file)?;

    // 3. Build adapters
    let auth_code_store = Arc::new(InMemoryAuthCodeStore::new());
    let mcp_proxy = Arc::new(ReqwestMcpProxy::new());

    // 4. Build use cases
    let authorize = Arc::new(AuthorizeUseCase::new(
        Arc::clone(&config).oauth.into(),
        Arc::clone(&auth_code_store),
    ));
    let token_exchange = Arc::new(TokenExchangeUseCase::new(
        Arc::clone(&config).oauth.into(),
        Arc::clone(&auth_code_store),
    ));
    let proxy_mcp = Arc::new(ProxyMcpUseCase::new(
        mcp_proxy,
        config.proxy.mcp_upstream_url.clone(),
        upstream_secret,
        config.oauth.access_token.clone(),
        config.hostname_validation.clone(),
    ));

    // 5. Build router
    let app_state = AppState { authorize, token_exchange, proxy_mcp, config };
    let router = build_router(app_state);

    // 6. Start server
    let addr = format!("{}:{}", args.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Starting OAuth2 gateway on {}", addr);
    axum::serve(listener, router).await?;

    Ok(())
}
```

This is the **only file** that knows `InMemoryAuthCodeStore`, `ReqwestMcpProxy`, `EnvFileConfigAdapter`, and `Axum`. Swap any adapter here and the domain doesn't change.

---

## Dependency Table

### `crates/core/Cargo.toml`

| Dependency | Purpose |
|---|---|
| `async-trait` | Async trait definitions for ports |
| `sha2` | PKCE S256 challenge computation |
| `base64` | PKCE challenge encoding |
| `subtle` | Constant-time comparison |
| `rand` | Secure token generation |
| `thiserror` | Domain error derives |
| `tracing` | Logging (trait, not implementation) |

**No** `axum`, `reqwest`, `tokio`, `dotenvy`, `serde`, or framework crates.

### `crates/platform/Cargo.toml`

| Dependency | Purpose |
|---|---|
| `core` (path) | Implements core's ports |
| `axum` | HTTP adapter |
| `reqwest` | MCP upstream proxy |
| `tokio` | Async runtime primitives (RwLock, etc.) |
| `serde` + `serde_json` | Request/response serialization |
| `dotenvy` | .env file loading |
| `tracing` | Structured logging |

### `apps/gateway/Cargo.toml`

| Dependency | Purpose |
|---|---|
| `core` (path) | Domain types |
| `platform` (path) | All adapters |
| `tokio` | Runtime (`#[tokio::main]`) |
| `tracing-subscriber` | Log output formatting |
| `anyhow` | Top-level error handling |
| `clap` | CLI argument parsing |

---

## Config Mapping (Python → Rust)

| Env Var | Python Location | Rust Domain Field |
|---|---|---|
| `OAUTH2_GATEWAY_PORT` | `config.py:45` | `GatewayConfig.port` |
| `OAUTH2_GATEWAY_CLIENT_ID` | `config.py:46` | `OAuthConfig.client_id` |
| `OAUTH2_GATEWAY_CLIENT_SECRET` | `config.py:47` | `OAuthConfig.client_secret` |
| `OAUTH2_GATEWAY_ACCESS_TOKEN` | `config.py:48` | `OAuthConfig.access_token` |
| `OAUTH2_GATEWAY_MCP_UPSTREAM_URL` | `config.py:49` | `MCPReverseProxyConfig.mcp_upstream_url` |
| `OAUTH2_GATEWAY_UPSTREAM_SECRET_FILE` | `config.py:50-53` | `MCPReverseProxyConfig.upstream_secret_file` |
| `OAUTH2_GATEWAY_ENFORCE_HOSTNAME_CHECK` | `config.py:54` | `HostnameValidationConfig.enforce` |
| `OAUTH2_PKCE_REQUIRED` | `config.py:55` | `OAuthConfig.pkce_required` |
| `USERNAME` | `config.py:56` | `OAuthConfig.username` |
| `PASSWORD` | `config.py:57` | `OAuthConfig.password` |
| `CF_TUNNEL_NAME` + `CF_DOMAIN` | `config.py:15-20` | `HostnameValidationConfig.expected_host` |
| `DIRECT_PUBLIC_ORIGIN_HOSTNAME` | `config.py:23-25` | `HostnameValidationConfig.expected_host` |

---

## Route Mapping (Python → Rust)

| Route | Method | Python | Rust Handler |
|---|---|---|---|
| `/health` | GET | `server.py:32` | `health.rs` |
| `/.well-known/oauth-authorization-server` | GET | `oauth.py:148` | `oauth_handlers.rs::oauth_metadata` |
| `/oauth/authorize` | GET, POST | `oauth.py:163` | `oauth_handlers.rs::oauth_authorize` |
| `/oauth/token` | POST | `oauth.py:185` | `oauth_handlers.rs::oauth_token` |
| `/.well-known/oauth-protected-resource/mcp` | GET | `mcp_proxy.py:150` | `mcp_handlers.rs::protected_resource_metadata` |
| `/mcp`, `/mcp/`, `/mcp/{*path}` | GET, POST, DELETE | `mcp_proxy.py:165` | `mcp_handlers.rs::mcp_reverse_proxy` |

No `/oauth/register` route — security rule preserved.

---

## Header Filtering (Ported from Python)

The proxy strips these headers from client → upstream requests:

**Hop-by-hop:** `connection`, `keep-alive`, `proxy-authenticate`, `proxy-authorization`, `te`, `trailers`, `transfer-encoding`, `upgrade`

**Request-specific:** `authorization`, `content-length`, `host`, `x-brain3-upstream-secret`

The proxy strips hop-by-hop headers from upstream → client responses.

The proxy **injects** `x-brain3-upstream-secret` with the shared secret read from the secret file.

---

## Security Invariants (Carried Forward)

1. **Preregistered clients only.** No DCR, no `/oauth/register`, no `token_endpoint_auth_method=none`.
2. **Client secret required** at token exchange. Empty client secret on server → 500.
3. **PKCE S256 required** by default (configurable via `OAUTH2_PKCE_REQUIRED`).
4. **Host validation** rejects requests for unexpected hostnames (HTTP 421) before they reach upstream.
5. **Bearer token validation** on all `/mcp` routes.
6. **Upstream shared secret** injected by gateway, stripped from client requests (prevents spoofing).
7. **Constant-time comparison** for all secret/token checks (using `subtle` crate).
8. **Auth codes expire** after 300 seconds and are single-use (taken, not read).

---

## Testing Strategy

Per AGENTS.MD: "Be very judicious about writing unit tests... It must just be core functionality on public APIs."

### What to Test

1. **Domain OAuth logic** (unit tests in `core`):
   - `validate_authorize_request` — valid/invalid cases
   - `verify_pkce` — correct/incorrect verifier
   - `validate_bearer_token` — present/absent/wrong
   - `validate_host` — match/mismatch/disabled/unconfigured

2. **Token exchange use case** (unit test with mock `AuthCodeStore`):
   - Happy path: valid code + PKCE + client secret → token
   - Invalid code, wrong client, PKCE failure, client_id mismatch

3. **Integration tests** (in `platform`, matching Python test_mcp_proxy.py / test_oauth_security.py):
   - Full HTTP roundtrips through Axum using `axum::test` or `reqwest` against a test server
   - Port of key tests from the Python suite

### What NOT to Test

- Log output
- HTML template content (beyond what the Python tests already cover)
- Private functions
- Adapter internals (reqwest config, dotenvy parsing details)

---

## Implementation Order

1. **Workspace setup**: `Cargo.toml` workspace, three crate skeletons, `cargo check` passes
2. **Domain model + errors**: All types in `core/src/domain/`
3. **Ports**: All trait definitions in `core/src/ports/`
4. **OAuth domain logic**: Pure functions in `core/src/domain/oauth.rs`
5. **Application use cases**: `authorize.rs`, `token_exchange.rs`, `validate_request.rs`, `proxy_mcp.rs`
6. **Domain unit tests**: Test the pure logic
7. **Platform: InMemoryAuthCodeStore**
8. **Platform: EnvFileConfigAdapter**
9. **Platform: ReqwestMcpProxy**
10. **Platform: Axum HTTP handlers + router**
11. **Composition root**: `apps/gateway/main.rs`
12. **Integration tests**: Port Python test suite
13. **Manual E2E**: Run Rust gateway alongside the existing Python MCP container, test with Claude/ChatGPT

---

## Resolved Design Decisions

### 1. Streaming SSE: Buffered only (M1)

The Python PoC reads the full upstream response body before forwarding (`await upstream_response.aread()`). The Rust version will do the same — buffer the entire upstream response, then send it to the client. No streaming.

This matches the working PoC exactly. Streaming SSE support for MCP's Server-Sent Events transport is deferred to a future milestone.

### 2. Graceful Shutdown: Yes

The Python server doesn't handle SIGTERM gracefully, but the Rust version will. Use `axum::serve` with `with_graceful_shutdown` and `tokio::signal::ctrl_c()`. This is trivial to add in Rust and is good practice — it lets in-flight requests complete before the process exits.

```rust
// In main.rs, after building the router:
let listener = tokio::net::TcpListener::bind(&addr).await?;
axum::serve(listener, router)
    .with_graceful_shutdown(shutdown_signal())
    .await?;

async fn shutdown_signal() {
    tokio::signal::ctrl_c().await.expect("failed to install CTRL+C handler");
    tracing::info!("Received shutdown signal, draining connections...");
}
```

### 3. Proxy Headers: Parse X-Forwarded-Host / X-Forwarded-Proto

The Python server runs behind uvicorn with `proxy_headers=True` and `forwarded_allow_ips="*"`, which means Starlette's `request.base_url` automatically uses `X-Forwarded-Host` and `X-Forwarded-Proto` when present.

The Rust HTTP adapter layer will do the same. A helper function in the HTTP adapter reconstructs the effective base URL:

```rust
fn resolve_base_url(headers: &HeaderMap, original_uri: &Uri) -> String {
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    format!("{}://{}", proto, host)
}
```

This is used by:
- `oauth_metadata` handler — to build `issuer`, `authorization_endpoint`, `token_endpoint`
- `protected_resource_metadata` handler — to build `resource` and `authorization_servers`
- `mcp_reverse_proxy` handler — to build the `resource_metadata` URL in `WWW-Authenticate` headers

The domain and use case layers never see this — it's purely an HTTP adapter concern. The `X-Forwarded-*` headers are trusted unconditionally (matching the Python behavior of `forwarded_allow_ips="*"`), since the gateway is designed to sit behind Cloudflare Tunnel or a reverse proxy that sets these headers.

### 4. Host Validation: Uses Effective Host

The host validation logic (`validate_host` in the domain) receives the **effective** request host, which the HTTP adapter resolves from `X-Forwarded-Host` (if present) or the `Host` header. This matches the Python behavior where `request.url.hostname` reflects proxy headers.
