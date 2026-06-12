use std::collections::HashMap;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum::Json;
use serde_json::json;

use brain3_core::domain::errors::OAuthError;
use brain3_core::domain::oauth::{AuthorizeRequest, TokenRequest};
use brain3_core::domain::redact::elide_secret;
use brain3_core::ports::auth_code_store::AuthCodeStore;
use brain3_core::ports::mcp_proxy::McpProxyPort;

use super::state::AppState;
use super::templates::{render_login_form, render_misconfigured_page};

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
        Json(serde_json::json!({
            "error": "rate_limit_exceeded",
            "error_description": "Too many attempts. Try again later."
        })),
    )
        .into_response()
}

fn oauth_error_response(err: OAuthError) -> Response {
    let status =
        StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut body = serde_json::Map::new();
    body.insert(
        "error".into(),
        serde_json::Value::String(err.error_code().into()),
    );
    if let Some(desc) = err.error_description() {
        body.insert(
            "error_description".into(),
            serde_json::Value::String(desc.into()),
        );
    }
    (status, Json(serde_json::Value::Object(body))).into_response()
}

fn parse_authorize_request(source: &HashMap<String, String>) -> AuthorizeRequest {
    AuthorizeRequest {
        response_type: source.get("response_type").cloned().unwrap_or_default(),
        client_id: source.get("client_id").cloned().unwrap_or_default(),
        redirect_uri: source.get("redirect_uri").cloned().unwrap_or_default(),
        state: source.get("state").cloned().filter(|s| !s.is_empty()),
        code_challenge: source
            .get("code_challenge")
            .cloned()
            .filter(|s| !s.is_empty()),
        code_challenge_method: source
            .get("code_challenge_method")
            .cloned()
            .filter(|s| !s.is_empty())
            .or(Some("S256".into())),
    }
}

pub async fn oauth_metadata<S: AuthCodeStore + 'static, P: McpProxyPort + 'static>(
    State(_state): State<AppState<S, P>>,
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
        "grant_types_supported": ["authorization_code"],
        "response_types_supported": ["code"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["client_secret_post"],
    }))
}

pub async fn oauth_authorize_get<S: AuthCodeStore + 'static, P: McpProxyPort + 'static>(
    State(state): State<AppState<S, P>>,
    axum::extract::Query(query): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    let req = parse_authorize_request(&query);

    tracing::info!(
        client_id = %req.client_id,
        redirect_uri = %req.redirect_uri,
        response_type = %req.response_type,
        has_code_challenge = req.code_challenge.is_some(),
        state = ?req.state,
        "authorize GET received"
    );

    if let Err(e) = state.authorize.validate(&req) {
        tracing::warn!(
            client_id = %req.client_id,
            redirect_uri = %req.redirect_uri,
            response_type = %req.response_type,
            error = ?e,
            "authorize request rejected"
        );
        return oauth_error_response(e);
    }

    if !state.authorize.login_configured() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Html(render_misconfigured_page()),
        )
            .into_response();
    }

    Html(render_login_form(&req, None)).into_response()
}

pub async fn oauth_authorize_post<S: AuthCodeStore + 'static, P: McpProxyPort + 'static>(
    State(state): State<AppState<S, P>>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    if let Err(retry_after) = state.rate_limiter.check(&headers) {
        tracing::warn!(
            retry_after_secs = retry_after,
            "rate limit exceeded on /oauth/authorize POST"
        );
        return rate_limit_response(retry_after);
    }

    let req = parse_authorize_request(&form);

    if let Err(e) = state.authorize.validate(&req) {
        tracing::warn!(
            client_id = %req.client_id,
            redirect_uri = %req.redirect_uri,
            response_type = %req.response_type,
            error = ?e,
            "authorize POST rejected at validation"
        );
        return oauth_error_response(e);
    }

    if !state.authorize.login_configured() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Html(render_misconfigured_page()),
        )
            .into_response();
    }

    let username = form.get("username").cloned().unwrap_or_default();
    let password = form.get("password").cloned().unwrap_or_default();

    if !state.authorize.check_credentials(&username, &password) {
        tracing::warn!(username = %username, "authorize POST rejected: invalid credentials");
        return (
            StatusCode::UNAUTHORIZED,
            Html(render_login_form(
                &req,
                Some("Invalid username or password"),
            )),
        )
            .into_response();
    }

    let code = state.authorize.issue_code(&req).await;
    tracing::info!(
        "OAuth authorization code issued, redirecting to {}...",
        &req.redirect_uri[..req.redirect_uri.len().min(50)]
    );

    let separator = if req.redirect_uri.contains('?') {
        "&"
    } else {
        "?"
    };
    let mut redirect_url = format!("{}{}code={}", req.redirect_uri, separator, code);
    if let Some(ref state_val) = req.state {
        redirect_url.push_str(&format!("&state={}", urlencoding::encode(state_val)));
    }

    Redirect::to(&redirect_url).into_response()
}

pub async fn oauth_token<S: AuthCodeStore + 'static, P: McpProxyPort + 'static>(
    State(state): State<AppState<S, P>>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    if let Err(retry_after) = state.rate_limiter.check(&headers) {
        tracing::warn!(
            retry_after_secs = retry_after,
            "rate limit exceeded on /oauth/token"
        );
        return rate_limit_response(retry_after);
    }

    let req = TokenRequest {
        grant_type: form.get("grant_type").cloned().unwrap_or_default(),
        client_id: form.get("client_id").cloned().unwrap_or_default(),
        client_secret: form.get("client_secret").cloned().unwrap_or_default(),
        code: form.get("code").cloned().unwrap_or_default(),
        redirect_uri: form.get("redirect_uri").cloned().filter(|s| !s.is_empty()),
        code_verifier: form.get("code_verifier").cloned().filter(|s| !s.is_empty()),
    };

    match state.token_exchange.exchange(&req).await {
        Ok(token_response) => {
            tracing::info!(
                access_token_hint = %elide_secret(&token_response.access_token),
                token_type = %token_response.token_type,
                expires_in = token_response.expires_in,
                "OAuth token issued via authorization_code grant"
            );
            Json(json!({
                "access_token": token_response.access_token,
                "token_type": token_response.token_type,
                "expires_in": token_response.expires_in,
            }))
            .into_response()
        }
        Err(e) => oauth_error_response(e),
    }
}
