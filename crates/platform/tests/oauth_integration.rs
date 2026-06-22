use std::sync::Arc;

use async_trait::async_trait;
use axum_test::TestServer;
use oxide_auth::primitives::authorizer::AuthMap;
use oxide_auth::primitives::generator::RandomGenerator;
use oxide_auth::primitives::issuer::TokenMap;
use oxide_auth::primitives::registrar::{Client, ClientMap, RegisteredUrl};
use reqwest::Url;
use serde_json::Value;
use tokio::sync::Mutex;

use brain3_core::application::proxy_mcp::ProxyMcpUseCase;
use brain3_core::domain::errors::ProxyError;
use brain3_core::domain::model::{
    GatewayConfig, HostnameValidationConfig, MCPReverseProxyConfig, OAuthConfig,
};
use brain3_core::ports::mcp_proxy::{McpProxyPort, McpProxyRequest, McpProxyResponse};
use brain3_core::ports::token_store::TokenStore;

use brain3_platform::http::registrar::BrainRegistrar;
use brain3_platform::http::router::build_router;
use brain3_platform::http::state::AppState;
use brain3_platform::token_store::token_map::TokenMapStore;

const CLIENT_ID: &str = "brain3-oauth2-client";
const CLIENT_SECRET: &str = "hardcoded-secret";
const LOGIN_USERNAME: &str = "operator";
const LOGIN_PASSWORD: &str = "password-123";
const REDIRECT_URI: &str = "https://chatgpt.com/connector/oauth/test";
const CODE_VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CODE_CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

// ---------------------------------------------------------------------------
// Mock MCP proxy adapter
// ---------------------------------------------------------------------------

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
            handler: Box::new(|_req| {
                Ok(McpProxyResponse {
                    status: 200,
                    headers: vec![("content-type".into(), "application/json".into())],
                    body: serde_json::to_vec(&serde_json::json!({
                        "jsonrpc": "2.0", "id": 1, "result": {"tools": []}
                    }))
                    .unwrap(),
                })
            }),
        }
    }

    fn capturing(captured: Arc<std::sync::Mutex<Option<McpProxyRequest>>>) -> Self {
        Self {
            handler: Box::new(move |req| {
                *captured.lock().unwrap() = Some(McpProxyRequest {
                    method: req.method.clone(),
                    url: req.url.clone(),
                    headers: req.headers.clone(),
                    body: req.body.clone(),
                });
                Ok(McpProxyResponse {
                    status: 200,
                    headers: vec![
                        ("content-type".into(), "application/json".into()),
                        ("mcp-session-id".into(), "session-123".into()),
                    ],
                    body: serde_json::to_vec(&serde_json::json!({
                        "jsonrpc": "2.0", "id": 1, "result": {"tools": []}
                    }))
                    .unwrap(),
                })
            }),
        }
    }

    fn unreachable() -> Self {
        Self {
            handler: Box::new(|_req| {
                Err(ProxyError::BadGateway(
                    "dial tcp 127.0.0.1:8420: connect refused".into(),
                ))
            }),
        }
    }

    fn should_not_be_called() -> Self {
        Self {
            handler: Box::new(|_req| {
                panic!("upstream should not be called");
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

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

impl TestHarness {
    fn build_server(self, proxy: MockMcpProxy) -> TestServer {
        let auth_registrar = Arc::new(BrainRegistrar::new(&self.oauth.client_id));

        let mut client_map = ClientMap::new();
        client_map.register_client(Client::confidential(
            &self.oauth.client_id,
            RegisteredUrl::Exact(
                "https://example.com/callback"
                    .parse()
                    .expect("static URL valid"),
            ),
            "read".parse().expect("static scope valid"),
            self.oauth.client_secret.as_bytes(),
        ));
        let token_registrar = Arc::new(client_map);

        let authorizer = Arc::new(Mutex::new(AuthMap::new(RandomGenerator::new(32))));
        let issuer = Arc::new(Mutex::new(TokenMap::new(RandomGenerator::new(32))));

        let proxy = Arc::new(proxy);
        let token_store: Arc<dyn TokenStore> =
            Arc::new(TokenMapStore::new(Arc::clone(&issuer)));

        let proxy_mcp = Arc::new(ProxyMcpUseCase::new(
            proxy,
            self.mcp_upstream_url,
            self.mcp_upstream_secret,
            token_store,
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
            auth_registrar,
            token_registrar,
            authorizer,
            issuer,
            proxy_mcp,
            config,
            rate_limiter: Arc::new(brain3_platform::http::rate_limit::OAuthRateLimiter::new()),
        };

        let router = build_router(state);
        TestServer::new(router)
    }
}

fn authorize_params() -> Vec<(&'static str, &'static str)> {
    vec![
        ("response_type", "code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", REDIRECT_URI),
        ("code_challenge", CODE_CHALLENGE),
        ("code_challenge_method", "S256"),
    ]
}

fn extract_code_from_location(location: &str) -> String {
    let url = Url::parse(location).expect("redirect location should be a valid URL");
    url.query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.into_owned())
        .expect("redirect location should include code query parameter")
}

async fn login_and_get_code(server: &TestServer) -> String {
    let mut form: Vec<(&str, &str)> = authorize_params();
    form.push(("username", LOGIN_USERNAME));
    form.push(("password", LOGIN_PASSWORD));

    let resp = server.post("/oauth/authorize").form(&form).await;
    let location = resp.header("location").to_str().unwrap().to_string();
    extract_code_from_location(&location)
}

async fn login_and_get_access_token(server: &TestServer) -> String {
    let code = login_and_get_code(server).await;
    let resp = server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("redirect_uri", REDIRECT_URI),
            ("code", &code),
            ("code_verifier", CODE_VERIFIER),
        ])
        .await;
    resp.assert_status_ok();
    let body: Value = resp.json();
    body["access_token"]
        .as_str()
        .expect("access_token should be a string")
        .to_string()
}

async fn login_and_exchange_code(server: &TestServer) -> Value {
    let code = login_and_get_code(server).await;
    let resp = server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("redirect_uri", REDIRECT_URI),
            ("code", &code),
            ("code_verifier", CODE_VERIFIER),
        ])
        .await;
    resp.assert_status_ok();
    resp.json()
}

// ===========================================================================
// Tests ported from test_oauth_security.py
// ===========================================================================

#[tokio::test]
async fn oauth_metadata_only_advertises_preregistered_confidential_client_flow() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let resp = server.get("/.well-known/oauth-authorization-server").await;
    resp.assert_status_ok();

    let body: Value = resp.json();
    assert!(body.get("registration_endpoint").is_none());
    assert_eq!(
        body["grant_types_supported"],
        serde_json::json!(["authorization_code", "refresh_token"])
    );
    assert_eq!(
        body["response_types_supported"],
        serde_json::json!(["code"])
    );
    assert_eq!(
        body["token_endpoint_auth_methods_supported"],
        serde_json::json!(["client_secret_post"])
    );
}

#[tokio::test]
async fn oauth_register_route_is_not_exposed() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let resp = server
        .post("/oauth/register")
        .json(&serde_json::json!({"client_name": "test"}))
        .await;
    assert_eq!(resp.status_code(), 404);
}

#[tokio::test]
async fn authorize_rejects_missing_client_id() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let resp = server
        .get("/oauth/authorize")
        .add_query_param("response_type", "code")
        .add_query_param("redirect_uri", REDIRECT_URI)
        .add_query_param("code_challenge", CODE_CHALLENGE)
        .add_query_param("code_challenge_method", "S256")
        .await;

    assert_eq!(resp.status_code(), 401);
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_client");
}

#[tokio::test]
async fn authorize_rejects_wrong_client_id() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let resp = server
        .get("/oauth/authorize")
        .add_query_param("response_type", "code")
        .add_query_param("client_id", "wrong-client")
        .add_query_param("redirect_uri", REDIRECT_URI)
        .add_query_param("code_challenge", CODE_CHALLENGE)
        .add_query_param("code_challenge_method", "S256")
        .await;

    assert_eq!(resp.status_code(), 401);
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_client");
}

#[tokio::test]
async fn authorization_code_exchange_requires_client_secret() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let code = login_and_get_code(&server).await;

    let resp = server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", REDIRECT_URI),
            ("code", &code),
            ("code_verifier", CODE_VERIFIER),
        ])
        .await;

    assert_eq!(resp.status_code(), 401);
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_client");
}

#[tokio::test]
async fn authorization_code_exchange_succeeds_with_exact_client_id_and_secret() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let body = login_and_exchange_code(&server).await;
    assert_eq!(body["token_type"], "bearer");
    assert!(body["access_token"].is_string());
    assert!(body["expires_in"].is_number());
}

#[tokio::test]
async fn authorization_code_exchange_issues_fresh_access_token_per_exchange() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let first_code = login_and_get_code(&server).await;
    let first_resp = server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("redirect_uri", REDIRECT_URI),
            ("code", &first_code),
            ("code_verifier", CODE_VERIFIER),
        ])
        .await;
    first_resp.assert_status_ok();
    let first_body: Value = first_resp.json();
    let first_token = first_body["access_token"]
        .as_str()
        .expect("access_token should be a string")
        .to_string();

    let second_code = login_and_get_code(&server).await;
    let second_resp = server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("redirect_uri", REDIRECT_URI),
            ("code", &second_code),
            ("code_verifier", CODE_VERIFIER),
        ])
        .await;
    second_resp.assert_status_ok();
    let second_body: Value = second_resp.json();
    let second_token = second_body["access_token"]
        .as_str()
        .expect("access_token should be a string")
        .to_string();

    assert_ne!(first_token, second_token);
}

#[tokio::test]
async fn authorization_code_exchange_rejects_client_id_mismatch_with_bound_code() {
    let harness = TestHarness {
        oauth: OAuthConfig {
            client_id: "client-one".into(),
            ..TestHarness::default().oauth
        },
        ..TestHarness::default()
    };
    let server = harness.build_server(MockMcpProxy::success());

    let form: Vec<(&str, &str)> = vec![
        ("response_type", "code"),
        ("client_id", "client-one"),
        ("redirect_uri", REDIRECT_URI),
        ("code_challenge", CODE_CHALLENGE),
        ("code_challenge_method", "S256"),
        ("username", LOGIN_USERNAME),
        ("password", LOGIN_PASSWORD),
    ];

    let resp = server.post("/oauth/authorize").form(&form).await;
    let location = resp.header("location").to_str().unwrap().to_string();
    let code = extract_code_from_location(&location);

    let resp = server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", "client-two"),
            ("client_secret", CLIENT_SECRET),
            ("redirect_uri", REDIRECT_URI),
            ("code", &code),
            ("code_verifier", CODE_VERIFIER),
        ])
        .await;

    assert_eq!(resp.status_code(), 401);
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_client");
}

#[tokio::test]
async fn authorize_shows_login_form_with_credential_guidance() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let resp = server
        .get("/oauth/authorize")
        .add_query_param("response_type", "code")
        .add_query_param("client_id", CLIENT_ID)
        .add_query_param("redirect_uri", REDIRECT_URI)
        .add_query_param("code_challenge", CODE_CHALLENGE)
        .add_query_param("code_challenge_method", "S256")
        .await;

    resp.assert_status_ok();
    let text = resp.text();
    assert!(text.contains("ChatGPT, Claude, or another AI app is requesting access"));
    assert!(text.contains("B3_USERNAME"));
    assert!(text.contains("B3_PASSWORD"));
    assert!(text.contains(".env"));
}

#[tokio::test]
async fn authorize_returns_503_when_login_credentials_are_not_configured() {
    let harness = TestHarness {
        oauth: OAuthConfig {
            password: "".into(),
            ..TestHarness::default().oauth
        },
        ..TestHarness::default()
    };
    let server = harness.build_server(MockMcpProxy::success());

    let resp = server
        .get("/oauth/authorize")
        .add_query_param("response_type", "code")
        .add_query_param("client_id", CLIENT_ID)
        .add_query_param("redirect_uri", REDIRECT_URI)
        .add_query_param("code_challenge", CODE_CHALLENGE)
        .add_query_param("code_challenge_method", "S256")
        .await;

    assert_eq!(resp.status_code(), 503);
    let text = resp.text();
    assert!(text.contains("B3_PASSWORD"));
    assert!(text.contains(".env"));
}

#[tokio::test]
async fn authorize_rejects_missing_code_challenge_when_pkce_required() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let resp = server
        .get("/oauth/authorize")
        .add_query_param("response_type", "code")
        .add_query_param("client_id", CLIENT_ID)
        .add_query_param("redirect_uri", REDIRECT_URI)
        .await;

    assert_eq!(resp.status_code(), 400);
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(body["error_description"], "code_challenge required");
}

#[tokio::test]
async fn authorize_rejects_non_s256_code_challenge_method_when_pkce_required() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let resp = server
        .get("/oauth/authorize")
        .add_query_param("response_type", "code")
        .add_query_param("client_id", CLIENT_ID)
        .add_query_param("redirect_uri", REDIRECT_URI)
        .add_query_param("code_challenge", CODE_CHALLENGE)
        .add_query_param("code_challenge_method", "plain")
        .await;

    assert_eq!(resp.status_code(), 400);
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(
        body["error_description"],
        "code_challenge_method must be S256"
    );
}

#[tokio::test]
async fn authorize_allows_missing_code_challenge_when_pkce_is_not_required() {
    let harness = TestHarness {
        oauth: OAuthConfig {
            pkce_required: false,
            ..TestHarness::default().oauth
        },
        ..TestHarness::default()
    };
    let server = harness.build_server(MockMcpProxy::success());

    let resp = server
        .get("/oauth/authorize")
        .add_query_param("response_type", "code")
        .add_query_param("client_id", CLIENT_ID)
        .add_query_param("redirect_uri", REDIRECT_URI)
        .await;

    resp.assert_status_ok();
    let text = resp.text();
    assert!(text.contains("ChatGPT, Claude, or another AI app is requesting access"));
}

#[tokio::test]
async fn authorize_rejects_bad_login_credentials() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let resp = server
        .post("/oauth/authorize")
        .form(&[
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", REDIRECT_URI),
            ("code_challenge", CODE_CHALLENGE),
            ("code_challenge_method", "S256"),
            ("username", LOGIN_USERNAME),
            ("password", "wrong-password"),
        ])
        .await;

    assert_eq!(resp.status_code(), 401);
    let text = resp.text();
    assert!(text.contains("Invalid username or password"));
}

#[tokio::test]
async fn authorization_code_exchange_requires_code_verifier_when_pkce_is_required() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let code = login_and_get_code(&server).await;

    let resp = server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("redirect_uri", REDIRECT_URI),
            ("code", &code),
        ])
        .await;

    assert_eq!(resp.status_code(), 400);
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_grant");
}

// ===========================================================================
// Tests ported from test_mcp_proxy.py
// ===========================================================================

#[tokio::test]
async fn protected_resource_metadata_points_to_gateway_mcp() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let resp = server
        .get("/.well-known/oauth-protected-resource/mcp")
        .await;
    resp.assert_status_ok();

    let body: Value = resp.json();
    assert!(body["resource"].as_str().unwrap().ends_with("/mcp"));
    assert!(body["authorization_servers"].is_array());
}

#[tokio::test]
async fn mcp_requires_bearer_token() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let resp = server
        .post("/mcp")
        .content_type("application/json")
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
        }))
        .await;

    assert_eq!(resp.status_code(), 401);
    let www_auth = resp
        .header("www-authenticate")
        .to_str()
        .unwrap()
        .to_string();
    assert!(www_auth.contains("resource_metadata="));
}

#[tokio::test]
async fn mcp_proxy_allows_matching_named_tunnel_host() {
    let captured = Arc::new(std::sync::Mutex::new(None));
    let harness = TestHarness {
        hostname_validation: HostnameValidationConfig {
            expected_host: Some("brain3-macos.mcpnative.dev".into()),
            enforce: true,
        },
        ..TestHarness::default()
    };
    let server = harness.build_server(MockMcpProxy::capturing(Arc::clone(&captured)));
    let access_token = login_and_get_access_token(&server).await;

    let resp = server
        .post("/mcp")
        .add_header(
            axum::http::header::HOST,
            axum::http::HeaderValue::from_static("brain3-macos.mcpnative.dev"),
        )
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_str(&format!("Bearer {access_token}")).unwrap(),
        )
        .add_header(
            "x-brain3-upstream-secret",
            axum::http::HeaderValue::from_static("attacker-value"),
        )
        .add_header("accept", "application/json, text/event-stream")
        .add_header("mcp-session-id", "session-123")
        .add_header("mcp-protocol-version", "2025-03-26")
        .content_type("application/json")
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
        }))
        .await;

    resp.assert_status_ok();

    let req = captured.lock().unwrap().take().unwrap();
    assert_eq!(req.url, "http://127.0.0.1:8420/mcp");

    let has_auth = req.headers.iter().any(|(k, _)| k == "authorization");
    assert!(!has_auth, "authorization header should be stripped");

    let upstream_secret = req
        .headers
        .iter()
        .find(|(k, _)| k == "x-brain3-upstream-secret")
        .map(|(_, v)| v.as_str());
    assert_eq!(upstream_secret, Some("shared-secret"));

    let session = req
        .headers
        .iter()
        .find(|(k, _)| k == "mcp-session-id")
        .map(|(_, v)| v.as_str());
    assert_eq!(session, Some("session-123"));

    let protocol = req
        .headers
        .iter()
        .find(|(k, _)| k == "mcp-protocol-version")
        .map(|(_, v)| v.as_str());
    assert_eq!(protocol, Some("2025-03-26"));

    assert_eq!(
        resp.header("mcp-session-id").to_str().unwrap(),
        "session-123"
    );
}

#[tokio::test]
async fn mcp_proxy_rejects_mismatched_named_tunnel_host() {
    let harness = TestHarness {
        hostname_validation: HostnameValidationConfig {
            expected_host: Some("brain3-macos.mcpnative.dev".into()),
            enforce: true,
        },
        ..TestHarness::default()
    };
    let server = harness.build_server(MockMcpProxy::should_not_be_called());
    let access_token = login_and_get_access_token(&server).await;

    let resp = server
        .post("/mcp")
        .add_header(
            axum::http::header::HOST,
            axum::http::HeaderValue::from_static("wrong-host.mcpnative.dev"),
        )
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_str(&format!("Bearer {access_token}")).unwrap(),
        )
        .content_type("application/json")
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
        }))
        .await;

    assert_eq!(resp.status_code(), 421);
    let body: Value = resp.json();
    assert_eq!(body["error"], "misdirected_request");
}

#[tokio::test]
async fn mcp_proxy_allows_matching_direct_public_origin_host() {
    let captured = Arc::new(std::sync::Mutex::new(None));
    let harness = TestHarness {
        hostname_validation: HostnameValidationConfig {
            expected_host: Some("brain3.yourserver.com".into()),
            enforce: true,
        },
        ..TestHarness::default()
    };
    let server = harness.build_server(MockMcpProxy::capturing(Arc::clone(&captured)));
    let access_token = login_and_get_access_token(&server).await;

    let resp = server
        .post("/mcp")
        .add_header(
            axum::http::header::HOST,
            axum::http::HeaderValue::from_static("brain3.yourserver.com"),
        )
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_str(&format!("Bearer {access_token}")).unwrap(),
        )
        .content_type("application/json")
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
        }))
        .await;

    resp.assert_status_ok();
    let req = captured.lock().unwrap().take().unwrap();
    assert_eq!(req.url, "http://127.0.0.1:8420/mcp");
}

#[tokio::test]
async fn mcp_proxy_rejects_mismatched_direct_public_origin_host() {
    let harness = TestHarness {
        hostname_validation: HostnameValidationConfig {
            expected_host: Some("brain3.yourserver.com".into()),
            enforce: true,
        },
        ..TestHarness::default()
    };
    let server = harness.build_server(MockMcpProxy::should_not_be_called());
    let access_token = login_and_get_access_token(&server).await;

    let resp = server
        .post("/mcp")
        .add_header(
            axum::http::header::HOST,
            axum::http::HeaderValue::from_static("wrong-host.yourserver.com"),
        )
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_str(&format!("Bearer {access_token}")).unwrap(),
        )
        .content_type("application/json")
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
        }))
        .await;

    assert_eq!(resp.status_code(), 421);
    let body: Value = resp.json();
    assert_eq!(body["error"], "misdirected_request");
}

#[tokio::test]
async fn mcp_proxy_allows_mismatched_host_when_validation_disabled() {
    let captured = Arc::new(std::sync::Mutex::new(None));
    let harness = TestHarness {
        hostname_validation: HostnameValidationConfig {
            expected_host: Some("brain3.yourserver.com".into()),
            enforce: false,
        },
        ..TestHarness::default()
    };
    let server = harness.build_server(MockMcpProxy::capturing(Arc::clone(&captured)));
    let access_token = login_and_get_access_token(&server).await;

    let resp = server
        .post("/mcp")
        .add_header(
            axum::http::header::HOST,
            axum::http::HeaderValue::from_static("wrong-host.yourserver.com"),
        )
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_str(&format!("Bearer {access_token}")).unwrap(),
        )
        .content_type("application/json")
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
        }))
        .await;

    resp.assert_status_ok();
    let req = captured.lock().unwrap().take().unwrap();
    assert_eq!(req.url, "http://127.0.0.1:8420/mcp");
}

#[tokio::test]
async fn mcp_proxy_returns_502_when_upstream_is_unreachable() {
    let server = TestHarness::default().build_server(MockMcpProxy::unreachable());
    let access_token = login_and_get_access_token(&server).await;

    let resp = server
        .post("/mcp")
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_str(&format!("Bearer {access_token}")).unwrap(),
        )
        .content_type("application/json")
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
        }))
        .await;

    assert_eq!(resp.status_code(), 502);
    let body: Value = resp.json();
    assert_eq!(body["error"], "bad_gateway");
}

// ===========================================================================
// Additional OAuth 2.1 / PKCE security tests
// ===========================================================================

#[tokio::test]
async fn authorization_code_is_single_use() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let code = login_and_get_code(&server).await;

    let first = server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("redirect_uri", REDIRECT_URI),
            ("code", &code),
            ("code_verifier", CODE_VERIFIER),
        ])
        .await;
    first.assert_status_ok();

    let second = server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("redirect_uri", REDIRECT_URI),
            ("code", &code),
            ("code_verifier", CODE_VERIFIER),
        ])
        .await;
    assert_eq!(second.status_code(), 400);
    let body: Value = second.json();
    assert_eq!(body["error"], "invalid_grant");
}

#[tokio::test]
async fn token_exchange_rejects_wrong_code_verifier() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let code = login_and_get_code(&server).await;

    let resp = server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("redirect_uri", REDIRECT_URI),
            ("code", &code),
            ("code_verifier", "completely-wrong-verifier-value-here"),
        ])
        .await;

    assert_eq!(resp.status_code(), 400);
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_grant");
}

#[tokio::test]
async fn token_exchange_rejects_wrong_client_secret() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let code = login_and_get_code(&server).await;

    let resp = server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("client_secret", "wrong-secret"),
            ("redirect_uri", REDIRECT_URI),
            ("code", &code),
            ("code_verifier", CODE_VERIFIER),
        ])
        .await;

    assert_eq!(resp.status_code(), 401);
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_client");
}

#[tokio::test]
async fn token_exchange_rejects_invalid_code() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let resp = server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("redirect_uri", REDIRECT_URI),
            ("code", "bogus-code-that-was-never-issued"),
            ("code_verifier", CODE_VERIFIER),
        ])
        .await;

    assert_eq!(resp.status_code(), 400);
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_grant");
}

#[tokio::test]
async fn token_exchange_rejects_unsupported_grant_type() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let resp = server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
        ])
        .await;

    assert_eq!(resp.status_code(), 400);
    let body: Value = resp.json();
    assert_eq!(body["error"], "unsupported_grant_type");
}

#[tokio::test]
async fn authorize_rejects_unsupported_response_type() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let resp = server
        .get("/oauth/authorize")
        .add_query_param("response_type", "token")
        .add_query_param("client_id", CLIENT_ID)
        .add_query_param("redirect_uri", REDIRECT_URI)
        .add_query_param("code_challenge", CODE_CHALLENGE)
        .add_query_param("code_challenge_method", "S256")
        .await;

    assert_eq!(resp.status_code(), 400);
    let body: Value = resp.json();
    assert_eq!(body["error"], "unsupported_response_type");
}

#[tokio::test]
async fn authorize_rejects_missing_redirect_uri() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let resp = server
        .get("/oauth/authorize")
        .add_query_param("response_type", "code")
        .add_query_param("client_id", CLIENT_ID)
        .add_query_param("code_challenge", CODE_CHALLENGE)
        .add_query_param("code_challenge_method", "S256")
        .await;

    assert_eq!(resp.status_code(), 400);
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(body["error_description"], "redirect_uri required");
}

#[tokio::test]
async fn authorize_preserves_state_parameter_in_redirect() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let resp = server
        .post("/oauth/authorize")
        .form(&[
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", REDIRECT_URI),
            ("code_challenge", CODE_CHALLENGE),
            ("code_challenge_method", "S256"),
            ("state", "random-csrf-state"),
            ("username", LOGIN_USERNAME),
            ("password", LOGIN_PASSWORD),
        ])
        .await;

    let location = resp.header("location").to_str().unwrap().to_string();
    assert!(location.contains("code="));
    assert!(location.contains("state=random-csrf-state"));
}

#[tokio::test]
async fn mcp_rejects_bearer_token_with_wrong_value() {
    let server = TestHarness::default().build_server(MockMcpProxy::should_not_be_called());

    let resp = server
        .post("/mcp")
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer wrong-token"),
        )
        .content_type("application/json")
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
        }))
        .await;

    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn mcp_rejects_non_bearer_auth_scheme() {
    let server = TestHarness::default().build_server(MockMcpProxy::should_not_be_called());

    let resp = server
        .post("/mcp")
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Basic placeholder-token"),
        )
        .content_type("application/json")
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
        }))
        .await;

    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn mcp_proxy_strips_client_upstream_secret_header() {
    let captured = Arc::new(std::sync::Mutex::new(None));
    let server =
        TestHarness::default().build_server(MockMcpProxy::capturing(Arc::clone(&captured)));
    let access_token = login_and_get_access_token(&server).await;

    let resp = server
        .post("/mcp")
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_str(&format!("Bearer {access_token}")).unwrap(),
        )
        .add_header(
            "x-brain3-upstream-secret",
            axum::http::HeaderValue::from_static("attacker-spoofed-value"),
        )
        .content_type("application/json")
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
        }))
        .await;

    resp.assert_status_ok();

    let req = captured.lock().unwrap().take().unwrap();
    let secret_headers: Vec<_> = req
        .headers
        .iter()
        .filter(|(k, _)| k == "x-brain3-upstream-secret")
        .collect();
    assert_eq!(secret_headers.len(), 1);
    assert_eq!(secret_headers[0].1, "shared-secret");
}

#[tokio::test]
async fn health_endpoint_returns_ok() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let resp = server.get("/health").await;
    resp.assert_status_ok();
}

#[tokio::test]
async fn full_oauth_flow_authorize_through_token() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    // Step 1: GET authorize shows login form
    let resp = server
        .get("/oauth/authorize")
        .add_query_param("response_type", "code")
        .add_query_param("client_id", CLIENT_ID)
        .add_query_param("redirect_uri", REDIRECT_URI)
        .add_query_param("code_challenge", CODE_CHALLENGE)
        .add_query_param("code_challenge_method", "S256")
        .await;
    resp.assert_status_ok();
    assert!(resp.text().contains("Sign In"));

    // Step 2: POST authorize with credentials → redirect with code
    let code = login_and_get_code(&server).await;
    assert!(!code.is_empty());

    // Step 3: Exchange code for token
    let resp = server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("redirect_uri", REDIRECT_URI),
            ("code", &code),
            ("code_verifier", CODE_VERIFIER),
        ])
        .await;
    resp.assert_status_ok();
    let body: Value = resp.json();
    assert_eq!(body["token_type"], "bearer");
    let token = body["access_token"].as_str().unwrap();

    // Step 4: Use token to access MCP
    let resp = server
        .post("/mcp")
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        )
        .content_type("application/json")
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
        }))
        .await;
    resp.assert_status_ok();
}

#[tokio::test]
async fn token_exchange_rejects_redirect_uri_mismatch() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let code = login_and_get_code(&server).await;

    let resp = server
        .post("/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("redirect_uri", "https://evil.com/callback"),
            ("code", &code),
            ("code_verifier", CODE_VERIFIER),
        ])
        .await;

    assert_eq!(resp.status_code(), 400);
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_grant");
}

#[tokio::test]
async fn mcp_proxy_forwards_subpath_to_upstream() {
    let captured = Arc::new(std::sync::Mutex::new(None));
    let server =
        TestHarness::default().build_server(MockMcpProxy::capturing(Arc::clone(&captured)));
    let access_token = login_and_get_access_token(&server).await;

    let resp = server
        .post("/mcp/sse")
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_str(&format!("Bearer {access_token}")).unwrap(),
        )
        .content_type("application/json")
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
        .await;

    resp.assert_status_ok();
    let req = captured.lock().unwrap().take().unwrap();
    assert_eq!(req.url, "http://127.0.0.1:8420/mcp/sse");
}

#[tokio::test]
async fn mcp_proxy_strips_authorization_header_from_upstream_request() {
    let captured = Arc::new(std::sync::Mutex::new(None));
    let server =
        TestHarness::default().build_server(MockMcpProxy::capturing(Arc::clone(&captured)));
    let access_token = login_and_get_access_token(&server).await;

    let resp = server
        .post("/mcp")
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_str(&format!("Bearer {access_token}")).unwrap(),
        )
        .content_type("application/json")
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
        .await;

    resp.assert_status_ok();
    let req = captured.lock().unwrap().take().unwrap();
    let auth_headers: Vec<_> = req
        .headers
        .iter()
        .filter(|(k, _)| k == "authorization")
        .collect();
    assert!(
        auth_headers.is_empty(),
        "authorization must not be forwarded upstream"
    );
}

#[tokio::test]
async fn oauth_metadata_advertises_s256_code_challenge_method() {
    let server = TestHarness::default().build_server(MockMcpProxy::success());

    let resp = server.get("/.well-known/oauth-authorization-server").await;
    resp.assert_status_ok();

    let body: Value = resp.json();
    assert_eq!(
        body["code_challenge_methods_supported"],
        serde_json::json!(["S256"])
    );
}
