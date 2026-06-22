use std::borrow::Cow;
use std::collections::HashMap;

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use oxide_auth::endpoint::{
    OAuthError, OwnerConsent, QueryParameter, Scopes, Solicitation, Template, WebRequest,
};
use oxide_auth::frontends::simple::endpoint::Error as OAuthEndpointError;
use oxide_auth::frontends::simple::extensions::{AddonList, Pkce};
use oxide_auth::primitives::authorizer::AuthMap;
use oxide_auth::primitives::generator::RandomGenerator;
use oxide_auth::primitives::issuer::TokenMap;
use oxide_auth::primitives::registrar::ClientMap;
use oxide_auth_async::endpoint::Endpoint as AsyncEndpoint;
use oxide_auth_async::endpoint::{Extension as AsyncExtension, OwnerSolicitor};
use oxide_auth_async::endpoint::access_token::AccessTokenFlow;
use oxide_auth_async::endpoint::authorization::AuthorizationFlow;
use oxide_auth_axum::{OAuthRequest, OAuthResponse, WebError};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;

use brain3_core::ports::mcp_proxy::McpProxyPort;

use super::registrar::BrainRegistrar;
use super::state::AppState;
use super::templates::{render_login_form, render_misconfigured_page, LoginFormParams};

// ---------------------------------------------------------------------------
// Async endpoint structs
// ---------------------------------------------------------------------------

/// Wraps an OAuthRequest so that `query()` returns the POST body params.
///
/// The authorization form POSTs OAuth params (response_type, client_id, etc.)
/// in the body alongside credentials. oxide-auth's AuthorizationFlow reads
/// OAuth params via `request.query()`, so we redirect the body to query here.
struct PostBodyRequest(OAuthRequest);

impl std::fmt::Debug for PostBodyRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PostBodyRequest")
    }
}

impl WebRequest for PostBodyRequest {
    type Error = WebError;
    type Response = OAuthResponse;

    fn query(&mut self) -> Result<Cow<'_, dyn QueryParameter + 'static>, Self::Error> {
        self.0.body()
            .map(|b| Cow::Borrowed(b as &dyn QueryParameter))
            .ok_or(WebError::Body)
    }

    fn urlbody(&mut self) -> Result<Cow<'_, dyn QueryParameter + 'static>, Self::Error> {
        self.0.urlbody()
    }

    fn authheader(&mut self) -> Result<Option<Cow<'_, str>>, Self::Error> {
        self.0.authheader()
    }
}

struct GrantSolicitor(String);

#[async_trait]
impl OwnerSolicitor<PostBodyRequest> for GrantSolicitor {
    async fn check_consent(
        &mut self,
        _: &mut PostBodyRequest,
        _: Solicitation<'_>,
    ) -> OwnerConsent<OAuthResponse> {
        OwnerConsent::Authorized(self.0.clone())
    }
}

struct AuthorizeEndpoint<'a> {
    registrar: &'a BrainRegistrar,
    authorizer: &'a mut AuthMap<RandomGenerator>,
    issuer: &'a mut TokenMap<RandomGenerator>,
    solicitor: GrantSolicitor,
    extensions: AddonList,
}

impl<'a> AsyncEndpoint<PostBodyRequest> for AuthorizeEndpoint<'a> {
    type Error = OAuthEndpointError<PostBodyRequest>;

    fn registrar(&self) -> Option<&(dyn oxide_auth_async::primitives::Registrar + Sync)> {
        Some(self.registrar)
    }

    fn authorizer_mut(&mut self) -> Option<&mut (dyn oxide_auth_async::primitives::Authorizer + Send)> {
        Some(self.authorizer)
    }

    fn issuer_mut(&mut self) -> Option<&mut (dyn oxide_auth_async::primitives::Issuer + Send)> {
        Some(self.issuer)
    }

    fn owner_solicitor(&mut self) -> Option<&mut (dyn OwnerSolicitor<PostBodyRequest> + Send)> {
        Some(&mut self.solicitor)
    }

    fn scopes(&mut self) -> Option<&mut dyn Scopes<PostBodyRequest>> {
        None
    }

    fn response(&mut self, _: &mut PostBodyRequest, _: Template) -> Result<OAuthResponse, Self::Error> {
        Ok(OAuthResponse::default())
    }

    fn error(&mut self, err: OAuthError) -> Self::Error {
        OAuthEndpointError::OAuth(err)
    }

    fn web_error(&mut self, err: WebError) -> Self::Error {
        OAuthEndpointError::Web(err)
    }

    fn extension(&mut self) -> Option<&mut (dyn AsyncExtension + Send)> {
        Some(&mut self.extensions)
    }
}

struct TokenEndpoint<'a> {
    registrar: &'a ClientMap,
    authorizer: &'a mut AuthMap<RandomGenerator>,
    issuer: &'a mut TokenMap<RandomGenerator>,
    extensions: AddonList,
}

impl<'a> AsyncEndpoint<OAuthRequest> for TokenEndpoint<'a> {
    type Error = OAuthEndpointError<OAuthRequest>;

    fn registrar(&self) -> Option<&(dyn oxide_auth_async::primitives::Registrar + Sync)> {
        Some(self.registrar)
    }

    fn authorizer_mut(&mut self) -> Option<&mut (dyn oxide_auth_async::primitives::Authorizer + Send)> {
        Some(self.authorizer)
    }

    fn issuer_mut(&mut self) -> Option<&mut (dyn oxide_auth_async::primitives::Issuer + Send)> {
        Some(self.issuer)
    }

    fn owner_solicitor(&mut self) -> Option<&mut (dyn OwnerSolicitor<OAuthRequest> + Send)> {
        None
    }

    fn scopes(&mut self) -> Option<&mut dyn Scopes<OAuthRequest>> {
        None
    }

    fn response(&mut self, _: &mut OAuthRequest, _: Template) -> Result<OAuthResponse, Self::Error> {
        Ok(OAuthResponse::default())
    }

    fn error(&mut self, err: OAuthError) -> Self::Error {
        OAuthEndpointError::OAuth(err)
    }

    fn web_error(&mut self, err: WebError) -> Self::Error {
        OAuthEndpointError::Web(err)
    }

    fn extension(&mut self) -> Option<&mut (dyn AsyncExtension + Send)> {
        Some(&mut self.extensions)
    }
}

// ---------------------------------------------------------------------------
// Utility helpers
// ---------------------------------------------------------------------------

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

fn rate_limit_response(retry_after_secs: u64) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(
            axum::http::header::RETRY_AFTER,
            retry_after_secs.to_string(),
        )],
        Json(json!({
            "error": "rate_limit_exceeded",
            "error_description": "Too many attempts. Try again later."
        })),
    )
        .into_response()
}

fn login_configured(config: &brain3_core::domain::model::GatewayConfig) -> bool {
    !config.oauth.password.is_empty() && !config.oauth.username.is_empty()
}

fn check_credentials(
    username: &str,
    password: &str,
    config: &brain3_core::domain::model::GatewayConfig,
) -> bool {
    let u_match = username
        .as_bytes()
        .ct_eq(config.oauth.username.as_bytes())
        .into();
    let p_match = password
        .as_bytes()
        .ct_eq(config.oauth.password.as_bytes())
        .into();
    u_match && p_match
}

fn validate_authorize_params(
    params: &LoginFormParams,
    config: &brain3_core::domain::model::GatewayConfig,
) -> Result<(), Response> {
    if params.response_type != "code" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "unsupported_response_type"})),
        )
            .into_response());
    }

    if params.client_id.is_empty() || params.client_id != config.oauth.client_id {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid_client"})),
        )
            .into_response());
    }

    if params.redirect_uri.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_request", "error_description": "redirect_uri required"})),
        )
            .into_response());
    }

    if config.oauth.pkce_required {
        let challenge_empty = params
            .code_challenge
            .as_ref()
            .is_none_or(|s| s.is_empty());
        if challenge_empty {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_request", "error_description": "code_challenge required"})),
            )
                .into_response());
        }
        if let Some(method) = &params.code_challenge_method {
            if method != "S256" {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "invalid_request", "error_description": "code_challenge_method must be S256"})),
                )
                    .into_response());
            }
        }
    }

    Ok(())
}

fn parse_params_from_map(map: &HashMap<String, String>) -> LoginFormParams {
    LoginFormParams {
        response_type: map.get("response_type").cloned().unwrap_or_default(),
        client_id: map.get("client_id").cloned().unwrap_or_default(),
        redirect_uri: map.get("redirect_uri").cloned().unwrap_or_default(),
        state: map.get("state").cloned().filter(|s| !s.is_empty()),
        code_challenge: map.get("code_challenge").cloned().filter(|s| !s.is_empty()),
        code_challenge_method: map
            .get("code_challenge_method")
            .cloned()
            .filter(|s| !s.is_empty()),
    }
}

struct TokenRequestShape {
    grant_type: Option<String>,
    has_client_id: bool,
    has_redirect_uri: bool,
    has_code: bool,
}

fn token_request_shape(request: &OAuthRequest) -> TokenRequestShape {
    let body = request.body();
    TokenRequestShape {
        grant_type: body
            .and_then(|body| body.unique_value("grant_type"))
            .map(|value| value.into_owned()),
        has_client_id: body
            .and_then(|body| body.unique_value("client_id"))
            .is_some(),
        has_redirect_uri: body
            .and_then(|body| body.unique_value("redirect_uri"))
            .is_some(),
        has_code: body.and_then(|body| body.unique_value("code")).is_some(),
    }
}

async fn normalize_token_error_response(
    response: OAuthResponse,
    request_shape: &TokenRequestShape,
) -> Response {
    let response = response.into_response();
    let should_normalize = response.status() == StatusCode::BAD_REQUEST
        && request_shape.grant_type.as_deref() == Some("authorization_code")
        && request_shape.has_client_id
        && request_shape.has_redirect_uri
        && request_shape.has_code;

    if !should_normalize {
        return response;
    }

    let (parts, body) = response.into_parts();
    let body_bytes = match to_bytes(body, 16 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return (parts.status, parts.headers, Body::empty()).into_response(),
    };

    let mut json_body: Value = match serde_json::from_slice(&body_bytes) {
        Ok(body) => body,
        Err(_) => {
            return (
                parts.status,
                parts.headers,
                String::from_utf8_lossy(&body_bytes).into_owned(),
            )
                .into_response()
        }
    };

    if json_body.get("error") != Some(&Value::String("invalid_request".into())) {
        return (parts.status, parts.headers, body_bytes).into_response();
    }

    json_body["error"] = Value::String("invalid_grant".into());
    (parts.status, parts.headers, Json(json_body)).into_response()
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub async fn oauth_metadata<P: McpProxyPort + 'static>(
    State(_state): State<AppState<P>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let base_url = resolve_base_url(&headers);
    tracing::info!(
        base_url = %base_url,
        host = ?headers.get("host").map(|v| v.to_str().unwrap_or("<invalid>")),
        x_forwarded_host = ?headers.get("x-forwarded-host").map(|v| v.to_str().unwrap_or("<invalid>")),
        "serving OAuth metadata"
    );
    Json(json!({
        "issuer": base_url,
        "authorization_endpoint": format!("{base_url}/oauth/authorize"),
        "token_endpoint": format!("{base_url}/oauth/token"),
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "response_types_supported": ["code"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["client_secret_post"],
    }))
}

pub async fn oauth_authorize_get<P: McpProxyPort + 'static>(
    State(state): State<AppState<P>>,
    axum::extract::Query(query): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    let params = parse_params_from_map(&query);

    tracing::info!(
        client_id = %params.client_id,
        redirect_uri = %params.redirect_uri,
        response_type = %params.response_type,
        has_code_challenge = params.code_challenge.is_some(),
        "authorize GET received"
    );

    if let Err(resp) = validate_authorize_params(&params, &state.config) {
        tracing::warn!(
            client_id = %params.client_id,
            "authorize GET rejected at validation"
        );
        return resp;
    }

    if !login_configured(&state.config) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Html(render_misconfigured_page()),
        )
            .into_response();
    }

    Html(render_login_form(&params, None)).into_response()
}

pub async fn oauth_authorize_post<P: McpProxyPort + 'static>(
    State(state): State<AppState<P>>,
    headers: HeaderMap,
    request: OAuthRequest,
) -> Response {
    if let Err(retry_after) = state.rate_limiter.check(&headers) {
        tracing::warn!(
            retry_after_secs = retry_after,
            "rate limit exceeded on /oauth/authorize POST"
        );
        return rate_limit_response(retry_after);
    }

    // Read body fields before passing request into the flow.
    // OAuthRequest caches the parsed body so the flow can re-read it.
    let (username, password, params) = {
        let body = match request.body() {
            Some(b) => b,
            None => return StatusCode::BAD_REQUEST.into_response(),
        };
        let username = body
            .unique_value("username")
            .map(|v| v.into_owned())
            .unwrap_or_default();
        let password = body
            .unique_value("password")
            .map(|v| v.into_owned())
            .unwrap_or_default();
        let params = LoginFormParams {
            response_type: body
                .unique_value("response_type")
                .map(|v| v.into_owned())
                .unwrap_or_default(),
            client_id: body
                .unique_value("client_id")
                .map(|v| v.into_owned())
                .unwrap_or_default(),
            redirect_uri: body
                .unique_value("redirect_uri")
                .map(|v| v.into_owned())
                .unwrap_or_default(),
            state: body
                .unique_value("state")
                .map(|v| v.into_owned())
                .filter(|s| !s.is_empty()),
            code_challenge: body
                .unique_value("code_challenge")
                .map(|v| v.into_owned())
                .filter(|s| !s.is_empty()),
            code_challenge_method: body
                .unique_value("code_challenge_method")
                .map(|v| v.into_owned())
                .filter(|s| !s.is_empty()),
        };
        (username, password, params)
    };

    if let Err(resp) = validate_authorize_params(&params, &state.config) {
        tracing::warn!(
            client_id = %params.client_id,
            "authorize POST rejected at validation"
        );
        return resp;
    }

    if !login_configured(&state.config) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Html(render_misconfigured_page()),
        )
            .into_response();
    }

    if !check_credentials(&username, &password, &state.config) {
        tracing::warn!(username = %username, "authorize POST: invalid credentials");
        return (
            StatusCode::UNAUTHORIZED,
            Html(render_login_form(&params, Some("Invalid username or password"))),
        )
            .into_response();
    }

    tracing::info!(
        client_id = %params.client_id,
        redirect_uri_prefix = %&params.redirect_uri[..params.redirect_uri.len().min(50)],
        "credentials valid, issuing authorization code"
    );

    let mut authorizer = state.authorizer.lock().await;
    let mut issuer = state.issuer.lock().await;

    let mut extensions = AddonList::new();
    extensions.push_code(Pkce::optional());

    let endpoint = AuthorizeEndpoint {
        registrar: state.auth_registrar.as_ref(),
        authorizer: &mut *authorizer,
        issuer: &mut *issuer,
        solicitor: GrantSolicitor(username),
        extensions,
    };

    // Wrap request so oxide-auth's AuthorizationFlow sees OAuth params via query().
    let request_for_flow = PostBodyRequest(request);

    match AuthorizationFlow::prepare(endpoint) {
        Err(e) => {
            tracing::error!("AuthorizationFlow::prepare failed: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Ok(mut flow) => match flow.execute(request_for_flow).await {
            Ok(response) => response.into_response(),
            Err(e) => {
                tracing::error!("AuthorizationFlow::execute failed: {:?}", e);
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        },
    }
}

pub async fn oauth_token<P: McpProxyPort + 'static>(
    State(state): State<AppState<P>>,
    headers: HeaderMap,
    request: OAuthRequest,
) -> Response {
    if let Err(retry_after) = state.rate_limiter.check(&headers) {
        tracing::warn!(
            retry_after_secs = retry_after,
            "rate limit exceeded on /oauth/token"
        );
        return rate_limit_response(retry_after);
    }

    let mut authorizer = state.authorizer.lock().await;
    let mut issuer = state.issuer.lock().await;
    let request_shape = token_request_shape(&request);
    if let Some(body) = request.body() {
        tracing::debug!(
            grant_type = ?body.unique_value("grant_type"),
            client_id = ?body.unique_value("client_id"),
            redirect_uri = ?body.unique_value("redirect_uri"),
            has_client_secret = body.unique_value("client_secret").is_some(),
            has_code = body.unique_value("code").is_some(),
            has_code_verifier = body.unique_value("code_verifier").is_some(),
            has_authorization = request.authorization_header().is_some(),
            "oauth token request parsed"
        );
    }

    let mut extensions = AddonList::new();
    extensions.push_access_token(Pkce::optional());

    let endpoint = TokenEndpoint {
        registrar: state.token_registrar.as_ref(),
        authorizer: &mut *authorizer,
        issuer: &mut *issuer,
        extensions,
    };

    match AccessTokenFlow::prepare(endpoint) {
        Err(e) => {
            tracing::error!("AccessTokenFlow::prepare failed: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Ok(mut flow) => {
            flow.allow_credentials_in_body(true);
            match flow.execute(request).await {
                Ok(response) => normalize_token_error_response(response, &request_shape).await,
                Err(e) => {
                    tracing::error!("AccessTokenFlow::execute failed: {:?}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
    }
}
