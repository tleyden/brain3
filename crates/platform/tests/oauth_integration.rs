use std::sync::Arc;

use async_trait::async_trait;
use axum_test::TestServer;
use oxide_auth::primitives::authorizer::AuthMap;
use oxide_auth::primitives::generator::RandomGenerator;
use oxide_auth::primitives::grant::{Extensions, Grant};
use oxide_auth::primitives::issuer::Issuer;
use reqwest::Url;
use serde_json::Value;
use tokio::sync::Mutex;

use brain3_core::application::proxy_mcp::ProxyMcpUseCase;
use brain3_core::domain::errors::ProxyError;
use brain3_core::domain::model::{
    GatewayConfig, HostnameValidationConfig, MCPReverseProxyConfig, OAuthConfig,
};
use brain3_core::ports::mcp_proxy::{McpProxyPort, McpProxyRequest, McpProxyResponse};

use brain3_platform::http::registrar::GatewayRegistrar;
use brain3_platform::http::router::build_router;
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
            SqliteTokenStore::in_memory().expect("in-memory issuer should initialize"),
        ));

        let proxy = Arc::new(proxy);
        let proxy_mcp = Arc::new(ProxyMcpUseCase::new(
            proxy,
            self.mcp_upstream_url,
            self.mcp_upstream_secret,
            self.hostname_validation.clone(),
        ));

        let config = Arc::new(GatewayConfig {
            port: 0,
            host: "127.0.0.1".into(),
            token_db_path: "/tmp/brain3-test-brain3.db".into(),
            oauth: self.oauth,
            mcp_reverse_proxy: MCPReverseProxyConfig {
                mcp_upstream_url: "http://127.0.0.1:8420".into(),
                upstream_secret_file: "/dev/null".into(),
            },
            hostname_validation: self.hostname_validation,
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

async fn issue_authorization_code(server: &TestServer) -> String {
    let response = server.post("/oauth/authorize").form(&authorize_form()).await;
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
    let response = exchange_code_for_token(
        server,
        &code,
        Some(CLIENT_SECRET),
        Some(CODE_VERIFIER),
    )
    .await;
    response.assert_status_ok();

    let body: Value = response.json();
    body["access_token"]
        .as_str()
        .expect("access_token should be a string")
        .to_string()
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

    let first = exchange_code_for_token(
        &server,
        &code,
        Some(CLIENT_SECRET),
        Some(CODE_VERIFIER),
    )
    .await;
    first.assert_status_ok();

    let second = exchange_code_for_token(
        &server,
        &code,
        Some(CLIENT_SECRET),
        Some(CODE_VERIFIER),
    )
    .await;

    assert_eq!(second.status_code(), 400);
    let body: Value = second.json();
    assert_eq!(body["error"], "invalid_grant");
}

#[tokio::test]
async fn mcp_rejects_expired_bearer_token() {
    let built = TestHarness::default().build_server(MockMcpProxy::should_not_be_called());
    let expired_token = {
        let mut issuer = built.issuer.lock().await;
        issuer
            .issue(Grant {
                owner_id: LOGIN_USERNAME.into(),
                client_id: CLIENT_ID.into(),
                redirect_uri: REDIRECT_URI.parse().unwrap(),
                scope: "read".parse().unwrap(),
                until: "2020-01-01T00:00:00Z".parse().unwrap(),
                extensions: Extensions::new(),
            })
            .expect("issuing an expired token should succeed")
            .token
    };

    let response = call_mcp(&built.server, &expired_token).await;

    assert_eq!(response.status_code(), 401);
    let body: Value = response.json();
    assert_eq!(body["error"], "invalid_token");
}

#[tokio::test]
async fn mcp_accepts_valid_bearer_token() {
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let built = TestHarness::default()
        .build_server(MockMcpProxy::capturing(Arc::clone(&captured)));
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
    assert!(
        requests[0]
            .headers
            .iter()
            .all(|(name, _)| name != "authorization")
    );
}
