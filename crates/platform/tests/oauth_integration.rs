use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use axum_test::TestServer;
use oxide_auth::primitives::authorizer::AuthMap;
use oxide_auth::primitives::generator::RandomGenerator;
use oxide_auth::primitives::issuer::Issuer;
use reqwest::Url;
use serde_json::Value;
use tokio::sync::Mutex;

use brain3_core::application::proxy_mcp::ProxyMcpUseCase;
use brain3_core::domain::errors::ProxyError;
use brain3_core::domain::model::{
    AccessMode, GatewayConfig, HostnameValidationConfig, LocalMcpConfig, MCPReverseProxyConfig,
    OAuthConfig,
};
use brain3_core::ports::mcp_proxy::{McpProxyPort, McpProxyRequest, McpProxyResponse};

use brain3_platform::http::registrar::GatewayRegistrar;
use brain3_platform::http::router::{build_local_router, build_router};
use brain3_platform::http::state::AppState;
use brain3_platform::token_store::sqlite::SqliteTokenStore;

const CLIENT_ID: &str = "brain3-oauth2-client";
const CLIENT_SECRET: &str = "hardcoded-secret";
const LOGIN_USERNAME: &str = "operator";
const LOGIN_PASSWORD: &str = "password-123";
const REDIRECT_URI: &str = "https://chatgpt.com/connector/oauth/test";
const CODE_VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CODE_CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

struct MockMcpProxy {
    handler: Box<dyn Fn(McpProxyRequest) -> Result<McpProxyResponse, ProxyError> + Send + Sync>,
}

#[async_trait]
impl McpProxyPort for MockMcpProxy {
    async fn forward(&self, request: McpProxyRequest) -> Result<McpProxyResponse, ProxyError> {
        (self.handler)(request)
    }
}

impl MockMcpProxy {
    fn success() -> Self {
        Self {
            handler: Box::new(|_request| {
                Ok(McpProxyResponse {
                    status: 200,
                    headers: vec![
                        ("content-type".into(), "application/json".into()),
                        ("mcp-session-id".into(), "session-123".into()),
                    ],
                    body: serde_json::to_vec(&serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": {"tools": []}
                    }))
                    .unwrap(),
                })
            }),
        }
    }

    fn capturing(captured: Arc<std::sync::Mutex<Vec<McpProxyRequest>>>) -> Self {
        Self {
            handler: Box::new(move |request| {
                captured.lock().unwrap().push(McpProxyRequest {
                    method: request.method.clone(),
                    url: request.url.clone(),
                    headers: request.headers.clone(),
                    body: request.body.clone(),
                });
                Ok(McpProxyResponse {
                    status: 200,
                    headers: vec![
                        ("content-type".into(), "application/json".into()),
                        ("mcp-session-id".into(), "session-123".into()),
                    ],
                    body: serde_json::to_vec(&serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": {"tools": []}
                    }))
                    .unwrap(),
                })
            }),
        }
    }

    fn should_not_be_called() -> Self {
        Self {
            handler: Box::new(|_request| {
                panic!("upstream should not be called");
            }),
        }
    }
}

struct TestHarness {
    oauth: OAuthConfig,
    hostname_validation: HostnameValidationConfig,
    mcp_upstream_url: String,
    mcp_upstream_secret: String,
    local_mcp: Option<LocalMcpConfig>,
}

impl Default for TestHarness {
    fn default() -> Self {
        Self {
            oauth: OAuthConfig {
                client_id: CLIENT_ID.into(),
                client_secret: CLIENT_SECRET.into(),
                access_token_lifetime_secs: 3600,
                refresh_token_lifetime_secs: 90 * 24 * 60 * 60,
                pkce_required: true,
                username: LOGIN_USERNAME.into(),
                password: LOGIN_PASSWORD.into(),
            },
            hostname_validation: HostnameValidationConfig {
                expected_host: None,
                enforce: true,
            },
            mcp_upstream_url: "http://127.0.0.1:8420".into(),
            mcp_upstream_secret: "shared-secret".into(),
            local_mcp: None,
        }
    }
}

struct BuiltServer {
    server: TestServer,
    issuer: Arc<Mutex<SqliteTokenStore>>,
}

impl TestHarness {
    fn build_server(self, proxy: MockMcpProxy) -> BuiltServer {
        let registrar = Arc::new(GatewayRegistrar::new(
            &self.oauth.client_id,
            self.oauth.client_secret.as_bytes().to_vec(),
        ));

        let authorizer = Arc::new(Mutex::new(AuthMap::new(RandomGenerator::new(32))));
        let issuer = Arc::new(Mutex::new(
            SqliteTokenStore::in_memory(
                self.oauth.access_token_lifetime_secs,
                self.oauth.refresh_token_lifetime_secs,
            )
            .expect("in-memory issuer should initialize"),
        ));

        let proxy = Arc::new(proxy);
        let mcp_upstream_secret = self.mcp_upstream_secret.clone();
        let proxy_mcp = Arc::new(ProxyMcpUseCase::new(
            proxy,
            self.mcp_upstream_url,
            mcp_upstream_secret.clone(),
            self.hostname_validation.clone(),
        ));

        let config = Arc::new(GatewayConfig {
            port: 0,
            host: "127.0.0.1".into(),
            token_db_path: "/tmp/brain3-test-brain3.db".into(),
            oauth: self.oauth,
            mcp_reverse_proxy: MCPReverseProxyConfig {
                mcp_upstream_url: "http://127.0.0.1:8420".into(),
                upstream_secret: mcp_upstream_secret,
            },
            hostname_validation: self.hostname_validation,
            access_mode: AccessMode::Both,
            local_mcp: self.local_mcp,
            container: None,
            tunnel: None,
        });

        let state = AppState {
            registrar,
            authorizer,
            issuer: Arc::clone(&issuer),
            proxy_mcp,
            config,
            rate_limiter: Arc::new(brain3_platform::http::rate_limit::OAuthRateLimiter::new()),
        };

        BuiltServer {
            server: TestServer::new(build_router(state)),
            issuer,
        }
    }

    fn build_local_server(self, proxy: MockMcpProxy) -> BuiltServer {
        let registrar = Arc::new(GatewayRegistrar::new(
            &self.oauth.client_id,
            self.oauth.client_secret.as_bytes().to_vec(),
        ));

        let authorizer = Arc::new(Mutex::new(AuthMap::new(RandomGenerator::new(32))));
        let issuer = Arc::new(Mutex::new(
            SqliteTokenStore::in_memory(
                self.oauth.access_token_lifetime_secs,
                self.oauth.refresh_token_lifetime_secs,
            )
            .expect("in-memory issuer should initialize"),
        ));

        let proxy = Arc::new(proxy);
        let proxy_mcp = Arc::new(ProxyMcpUseCase::new(
            proxy,
            self.mcp_upstream_url,
            self.mcp_upstream_secret.clone(),
            self.hostname_validation.clone(),
        ));

        let local_mcp = self
            .local_mcp
            .clone()
            .expect("local MCP config should be present for local server tests");

        let config = Arc::new(GatewayConfig {
            port: 0,
            host: "127.0.0.1".into(),
            token_db_path: "/tmp/brain3-test-brain3.db".into(),
            oauth: self.oauth,
            mcp_reverse_proxy: MCPReverseProxyConfig {
                mcp_upstream_url: "http://127.0.0.1:8420".into(),
                upstream_secret: self.mcp_upstream_secret,
            },
            hostname_validation: self.hostname_validation,
            access_mode: AccessMode::Both,
            local_mcp: Some(local_mcp),
            container: None,
            tunnel: None,
        });

        let state = AppState {
            registrar,
            authorizer,
            issuer: Arc::clone(&issuer),
            proxy_mcp,
            config,
            rate_limiter: Arc::new(brain3_platform::http::rate_limit::OAuthRateLimiter::new()),
        };

        BuiltServer {
            server: TestServer::new(build_local_router(state)),
            issuer,
        }
    }
}

fn authorize_form() -> Vec<(&'static str, &'static str)> {
    vec![
        ("response_type", "code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", REDIRECT_URI),
        ("code_challenge", CODE_CHALLENGE),
        ("code_challenge_method", "S256"),
        ("username", LOGIN_USERNAME),
        ("password", LOGIN_PASSWORD),
    ]
}

fn extract_code_from_location(location: &str) -> String {
    let url = Url::parse(location).expect("redirect location should be a valid URL");
    url.query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.into_owned())
        .expect("redirect location should include code query parameter")
}

fn unix_now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_secs() as i64
}

async fn issue_authorization_code(server: &TestServer) -> String {
    let response = server
        .post("/oauth/authorize")
        .form(&authorize_form())
        .await;
    let location = response.header("location").to_str().unwrap().to_string();
    extract_code_from_location(&location)
}

async fn exchange_code_for_token(
    server: &TestServer,
    code: &str,
    client_secret: Option<&str>,
    code_verifier: Option<&str>,
) -> axum_test::TestResponse {
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", REDIRECT_URI),
        ("code", code),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret));
    }
    if let Some(verifier) = code_verifier {
        form.push(("code_verifier", verifier));
    }

    server.post("/oauth/token").form(&form).await
}

async fn issue_access_token(server: &TestServer) -> String {
    let code = issue_authorization_code(server).await;
    let response =
        exchange_code_for_token(server, &code, Some(CLIENT_SECRET), Some(CODE_VERIFIER)).await;
    response.assert_status_ok();

    let body: Value = response.json();
    body["access_token"]
        .as_str()
        .expect("access_token should be a string")
        .to_string()
}

async fn exchange_authorization_code(server: &TestServer) -> Value {
    let code = issue_authorization_code(server).await;
    let response =
        exchange_code_for_token(server, &code, Some(CLIENT_SECRET), Some(CODE_VERIFIER)).await;
    response.assert_status_ok();
    response.json()
}

async fn exchange_refresh_token(
    server: &TestServer,
    refresh_token: &str,
    client_secret: Option<&str>,
) -> axum_test::TestResponse {
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("client_id", CLIENT_ID),
        ("refresh_token", refresh_token),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret));
    }

    server.post("/oauth/token").form(&form).await
}

async fn call_mcp(server: &TestServer, token: &str) -> axum_test::TestResponse {
    server
        .post("/mcp")
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        )
        .content_type("application/json")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }))
        .await
}

async fn call_local_mcp(server: &TestServer, token: Option<&str>) -> axum_test::TestResponse {
    let mut request =
        server
            .post("/mcp")
            .content_type("application/json")
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list",
                "params": {}
            }));
    if let Some(token) = token {
        request = request.add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
    }
    request.await
}

#[tokio::test]
async fn authorize_token_and_mcp_flow_succeeds() {
    let built = TestHarness::default().build_server(MockMcpProxy::success());
    let server = &built.server;

    let response = server
        .get("/oauth/authorize")
        .add_query_param("response_type", "code")
        .add_query_param("client_id", CLIENT_ID)
        .add_query_param("redirect_uri", REDIRECT_URI)
        .add_query_param("code_challenge", CODE_CHALLENGE)
        .add_query_param("code_challenge_method", "S256")
        .await;
    response.assert_status_ok();
    assert!(response.text().contains("Sign In"));

    let token = issue_access_token(&server).await;

    let response = call_mcp(&server, &token).await;
    response.assert_status_ok();
}

#[tokio::test]
async fn authorize_get_rejects_missing_code_challenge_method_when_pkce_required() {
    let built = TestHarness::default().build_server(MockMcpProxy::success());
    let server = &built.server;

    let response = server
        .get("/oauth/authorize")
        .add_query_param("response_type", "code")
        .add_query_param("client_id", CLIENT_ID)
        .add_query_param("redirect_uri", REDIRECT_URI)
        .add_query_param("code_challenge", CODE_CHALLENGE)
        .await;

    assert_eq!(response.status_code(), 400);
    let body: Value = response.json();
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(
        body["error_description"],
        "code_challenge_method must be S256"
    );
}

#[tokio::test]
async fn authorize_get_rejects_plain_code_challenge_method_when_pkce_required() {
    let built = TestHarness::default().build_server(MockMcpProxy::success());
    let server = &built.server;

    let response = server
        .get("/oauth/authorize")
        .add_query_param("response_type", "code")
        .add_query_param("client_id", CLIENT_ID)
        .add_query_param("redirect_uri", REDIRECT_URI)
        .add_query_param("code_challenge", CODE_CHALLENGE)
        .add_query_param("code_challenge_method", "plain")
        .await;

    assert_eq!(response.status_code(), 400);
    let body: Value = response.json();
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(
        body["error_description"],
        "code_challenge_method must be S256"
    );
}

#[tokio::test]
async fn authorize_post_rejects_missing_code_challenge_method_when_pkce_required() {
    let built = TestHarness::default().build_server(MockMcpProxy::should_not_be_called());
    let server = &built.server;

    let form = vec![
        ("response_type", "code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", REDIRECT_URI),
        ("code_challenge", CODE_CHALLENGE),
        ("username", LOGIN_USERNAME),
        ("password", LOGIN_PASSWORD),
    ];

    let response = server.post("/oauth/authorize").form(&form).await;

    assert_eq!(response.status_code(), 400);
    let body: Value = response.json();
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(
        body["error_description"],
        "code_challenge_method must be S256"
    );
}

#[tokio::test]
async fn authorization_code_exchange_requires_client_secret() {
    let built = TestHarness::default().build_server(MockMcpProxy::success());
    let server = &built.server;
    let code = issue_authorization_code(&server).await;

    let response = exchange_code_for_token(&server, &code, None, Some(CODE_VERIFIER)).await;

    assert_eq!(response.status_code(), 401);
    let body: Value = response.json();
    assert_eq!(body["error"], "invalid_client");
}

#[tokio::test]
async fn authorization_code_exchange_rejects_wrong_client_secret() {
    let built = TestHarness::default().build_server(MockMcpProxy::success());
    let server = &built.server;
    let code = issue_authorization_code(&server).await;

    let response =
        exchange_code_for_token(&server, &code, Some("wrong-secret"), Some(CODE_VERIFIER)).await;

    assert_eq!(response.status_code(), 401);
    let body: Value = response.json();
    assert_eq!(body["error"], "invalid_client");
}

#[tokio::test]
async fn authorization_code_exchange_rejects_wrong_code_verifier() {
    let built = TestHarness::default().build_server(MockMcpProxy::success());
    let server = &built.server;
    let code = issue_authorization_code(&server).await;

    let response = exchange_code_for_token(
        &server,
        &code,
        Some(CLIENT_SECRET),
        Some("completely-wrong-verifier-value-here"),
    )
    .await;

    assert_eq!(response.status_code(), 400);
    let body: Value = response.json();
    assert_eq!(body["error"], "invalid_grant");
}

#[tokio::test]
async fn authorization_code_exchange_rejects_missing_code_verifier() {
    let built = TestHarness::default().build_server(MockMcpProxy::success());
    let server = &built.server;
    let code = issue_authorization_code(&server).await;

    let response = exchange_code_for_token(&server, &code, Some(CLIENT_SECRET), None).await;

    assert_eq!(response.status_code(), 400);
    let body: Value = response.json();
    assert_eq!(body["error"], "invalid_grant");
}

#[tokio::test]
async fn authorization_code_is_single_use() {
    let built = TestHarness::default().build_server(MockMcpProxy::success());
    let server = &built.server;
    let code = issue_authorization_code(&server).await;

    let first =
        exchange_code_for_token(&server, &code, Some(CLIENT_SECRET), Some(CODE_VERIFIER)).await;
    first.assert_status_ok();

    let second =
        exchange_code_for_token(&server, &code, Some(CLIENT_SECRET), Some(CODE_VERIFIER)).await;

    assert_eq!(second.status_code(), 400);
    let body: Value = second.json();
    assert_eq!(body["error"], "invalid_grant");
}

#[tokio::test]
async fn refresh_token_exchange_succeeds() {
    let built = TestHarness::default().build_server(MockMcpProxy::success());
    let token_response = exchange_authorization_code(&built.server).await;
    let original_access_token = token_response["access_token"]
        .as_str()
        .expect("access_token should be present")
        .to_string();
    let original_refresh_token = token_response["refresh_token"]
        .as_str()
        .expect("refresh_token should be present")
        .to_string();

    let refresh_response =
        exchange_refresh_token(&built.server, &original_refresh_token, Some(CLIENT_SECRET)).await;
    refresh_response.assert_status_ok();

    let refreshed_body: Value = refresh_response.json();
    let refreshed_access_token = refreshed_body["access_token"]
        .as_str()
        .expect("refreshed access_token should be present")
        .to_string();
    let refreshed_refresh_token = refreshed_body["refresh_token"]
        .as_str()
        .expect("refreshed refresh_token should be present")
        .to_string();

    assert_ne!(refreshed_access_token, original_access_token);
    assert_ne!(refreshed_refresh_token, original_refresh_token);

    let replay_response =
        exchange_refresh_token(&built.server, &original_refresh_token, Some(CLIENT_SECRET)).await;
    assert_eq!(replay_response.status_code(), 400);
    let replay_body: Value = replay_response.json();
    assert_eq!(replay_body["error"], "invalid_grant");

    let revoked_response = call_mcp(&built.server, &original_access_token).await;
    assert_eq!(revoked_response.status_code(), 401);
    let revoked_body: Value = revoked_response.json();
    assert_eq!(revoked_body["error"], "invalid_token");

    let refreshed_response = call_mcp(&built.server, &refreshed_access_token).await;
    refreshed_response.assert_status_ok();
}

#[tokio::test]
async fn mcp_rejects_expired_bearer_token() {
    let built = TestHarness {
        oauth: OAuthConfig {
            client_id: CLIENT_ID.into(),
            client_secret: CLIENT_SECRET.into(),
            access_token_lifetime_secs: 1,
            refresh_token_lifetime_secs: 90 * 24 * 60 * 60,
            pkce_required: true,
            username: LOGIN_USERNAME.into(),
            password: LOGIN_PASSWORD.into(),
        },
        ..TestHarness::default()
    }
    .build_server(MockMcpProxy::should_not_be_called());
    let token = issue_access_token(&built.server).await;

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let response = call_mcp(&built.server, &token).await;

    assert_eq!(response.status_code(), 401);
    let body: Value = response.json();
    assert_eq!(body["error"], "invalid_token");
}

#[tokio::test]
async fn mcp_accepts_valid_bearer_token() {
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let built = TestHarness::default().build_server(MockMcpProxy::capturing(Arc::clone(&captured)));
    let token = issue_access_token(&built.server).await;

    let response = call_mcp(&built.server, &token).await;

    response.assert_status_ok();
    assert_eq!(
        response.header("mcp-session-id").to_str().unwrap(),
        "session-123"
    );

    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url, "http://127.0.0.1:8420/mcp");
    assert!(requests[0]
        .headers
        .iter()
        .all(|(name, _)| name != "authorization"));
}

#[tokio::test]
async fn local_mcp_rejects_missing_or_invalid_static_bearer_without_oauth_hint() {
    let built = TestHarness {
        local_mcp: Some(LocalMcpConfig {
            port: 8422,
            bearer_token: "local-secret".into(),
        }),
        ..TestHarness::default()
    }
    .build_local_server(MockMcpProxy::should_not_be_called());

    let response = call_local_mcp(&built.server, None).await;
    assert_eq!(response.status_code(), 401);
    assert!(response.maybe_header("www-authenticate").is_none());
    let body: Value = response.json();
    assert_eq!(body["error"], "invalid_token");

    let wrong = call_local_mcp(&built.server, Some("wrong-secret")).await;
    assert_eq!(wrong.status_code(), 401);
    assert!(wrong.maybe_header("www-authenticate").is_none());
}

#[tokio::test]
async fn local_mcp_accepts_configured_static_bearer_token() {
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let built = TestHarness {
        local_mcp: Some(LocalMcpConfig {
            port: 8422,
            bearer_token: "local-secret".into(),
        }),
        ..TestHarness::default()
    }
    .build_local_server(MockMcpProxy::capturing(Arc::clone(&captured)));

    let response = call_local_mcp(&built.server, Some("local-secret")).await;

    response.assert_status_ok();
    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url, "http://127.0.0.1:8420/mcp");
}

#[tokio::test]
async fn authorization_code_exchange_uses_configured_access_and_refresh_lifetimes() {
    let access_lifetime_secs = 123;
    let refresh_lifetime_secs = 456;
    let built = TestHarness {
        oauth: OAuthConfig {
            client_id: CLIENT_ID.into(),
            client_secret: CLIENT_SECRET.into(),
            access_token_lifetime_secs: access_lifetime_secs,
            refresh_token_lifetime_secs: refresh_lifetime_secs,
            pkce_required: true,
            username: LOGIN_USERNAME.into(),
            password: LOGIN_PASSWORD.into(),
        },
        ..TestHarness::default()
    }
    .build_server(MockMcpProxy::success());

    let token_response = exchange_authorization_code(&built.server).await;
    let access_token = token_response["access_token"]
        .as_str()
        .expect("access_token should be present")
        .to_string();
    let refresh_token = token_response["refresh_token"]
        .as_str()
        .expect("refresh_token should be present")
        .to_string();
    let expires_in = token_response["expires_in"]
        .as_i64()
        .expect("expires_in should be present");

    assert!(
        expires_in >= access_lifetime_secs as i64 - 2 && expires_in <= access_lifetime_secs as i64,
        "expected expires_in near configured access lifetime, got {expires_in}",
    );

    let (access_grant, refresh_grant) = {
        let issuer = built.issuer.lock().await;
        let access_grant = issuer
            .recover_token(&access_token)
            .expect("access token recovery should succeed")
            .expect("access token should exist");
        let refresh_grant = issuer
            .recover_refresh(&refresh_token)
            .expect("refresh token recovery should succeed")
            .expect("refresh token should exist");
        (access_grant, refresh_grant)
    };

    let now = unix_now_timestamp();
    let access_remaining = access_grant.until.timestamp() - now;
    let refresh_remaining = refresh_grant.until.timestamp() - now;

    assert!(
        access_remaining >= access_lifetime_secs as i64 - 2
            && access_remaining <= access_lifetime_secs as i64,
        "expected access grant lifetime near configured value, got {access_remaining}",
    );
    assert!(
        refresh_remaining >= refresh_lifetime_secs as i64 - 2
            && refresh_remaining <= refresh_lifetime_secs as i64,
        "expected refresh grant lifetime near configured value, got {refresh_remaining}",
    );
}
