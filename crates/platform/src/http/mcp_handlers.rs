use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Json;
use oxide_auth::primitives::issuer::Issuer;
use serde_json::json;

use brain3_core::application::proxy_mcp::ProxyMcpUseCase;
use brain3_core::application::validate_request::validate_host;
use brain3_core::domain::errors::ProxyError;
use brain3_core::domain::redact::elide_secret;
use brain3_core::ports::mcp_proxy::McpProxyPort;

use super::state::AppState;

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

fn effective_host(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost")
        .split(':')
        .next()
        .unwrap_or("localhost")
        .to_string()
}

fn resource_metadata_url(base_url: &str) -> String {
    format!("{base_url}/.well-known/oauth-protected-resource/mcp")
}

fn parse_bearer_token(headers: &HeaderMap) -> Result<&str, ProxyError> {
    let auth_header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let (scheme, token) = auth_header.split_once(' ').unwrap_or(("", ""));

    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
        return Err(ProxyError::Unauthorized(
            "Missing or invalid bearer token".into(),
        ));
    }

    Ok(token)
}

fn constant_time_eq(expected: &str, provided: &str) -> bool {
    let expected = expected.as_bytes();
    let provided = provided.as_bytes();
    let max_len = expected.len().max(provided.len());
    let mut diff = expected.len() ^ provided.len();

    for index in 0..max_len {
        let expected_byte = expected.get(index).copied().unwrap_or_default();
        let provided_byte = provided.get(index).copied().unwrap_or_default();
        diff |= usize::from(expected_byte ^ provided_byte);
    }

    diff == 0
}

async fn validate_access_token<P: McpProxyPort + 'static>(
    state: &AppState<P>,
    headers: &HeaderMap,
    method: &Method,
    uri: &Uri,
    host: &str,
) -> Result<(), ProxyError> {
    let token = match parse_bearer_token(headers) {
        Ok(token) => token,
        Err(error) => {
            tracing::info!(
                method = %method,
                path = %uri,
                host = host,
                "MCP proxy: unauthenticated probe, returning 401 with resource metadata"
            );
            return Err(error);
        }
    };

    let grant = {
        let issuer = state.issuer.lock().await;
        match issuer.recover_token(token) {
            Ok(Some(grant)) => grant,
            Ok(None) | Err(()) => {
                tracing::warn!(
                    received_token_hint = %elide_secret(token),
                    method = %method,
                    path = %uri,
                    host = host,
                    "MCP proxy rejected: bearer token not found"
                );
                return Err(ProxyError::Unauthorized(
                    "Missing or invalid bearer token".into(),
                ));
            }
        }
    };

    let now_unix = unix_now_timestamp();
    if grant.until.timestamp() <= now_unix {
        tracing::warn!(
            received_token_hint = %elide_secret(token),
            client_id = %grant.client_id,
            expired_at = %grant.until.to_rfc3339(),
            secs_expired_ago = now_unix - grant.until.timestamp(),
            method = %method,
            path = %uri,
            host = host,
            "MCP proxy rejected: bearer token EXPIRED"
        );
        return Err(ProxyError::Unauthorized(
            "Missing or invalid bearer token".into(),
        ));
    }

    tracing::debug!(
        token_hint = %elide_secret(token),
        client_id = %grant.client_id,
        expires_at = %grant.until.to_rfc3339(),
        secs_remaining = grant.until.timestamp() - now_unix,
        method = %method,
        path = %uri,
        "MCP proxy: access token valid"
    );
    Ok(())
}

fn unix_now_timestamp() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(_) => 0,
    }
}

fn proxy_error_response(err: ProxyError, headers: &HeaderMap) -> Response {
    match err {
        ProxyError::Unauthorized(desc) => {
            let base_url = resolve_base_url(headers);
            let www_authenticate = format!(
                r#"Bearer error="invalid_token", error_description="{desc}", resource_metadata="{}""#,
                resource_metadata_url(&base_url)
            );
            (
                StatusCode::UNAUTHORIZED,
                [(axum::http::header::WWW_AUTHENTICATE, www_authenticate)],
                Json(json!({
                    "error": "invalid_token",
                    "error_description": desc,
                })),
            )
                .into_response()
        }
        ProxyError::MisdirectedRequest(desc) => {
            tracing::warn!(error_description = %desc, "MCP request rejected with 421 Misdirected Request");
            (
                StatusCode::MISDIRECTED_REQUEST,
                Json(json!({
                    "error": "misdirected_request",
                    "error_description": desc,
                })),
            )
                .into_response()
        }
        ProxyError::BadGateway(desc) => {
            tracing::warn!("MCP upstream unavailable: {desc}");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": "bad_gateway",
                    "error_description": "MCP upstream unavailable",
                })),
            )
                .into_response()
        }
    }
}

fn local_proxy_error_response(err: ProxyError) -> Response {
    match err {
        ProxyError::Unauthorized(desc) => (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "invalid_token",
                "error_description": desc,
            })),
        )
            .into_response(),
        ProxyError::MisdirectedRequest(desc) => (
            StatusCode::MISDIRECTED_REQUEST,
            Json(json!({
                "error": "misdirected_request",
                "error_description": desc,
            })),
        )
            .into_response(),
        ProxyError::BadGateway(desc) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "error": "bad_gateway",
                "error_description": desc,
            })),
        )
            .into_response(),
    }
}

fn header_pairs(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn upstream_response_to_axum_response<P: McpProxyPort + 'static>(
    result: Result<brain3_core::ports::mcp_proxy::McpProxyResponse, ProxyError>,
    headers: &HeaderMap,
    oauth_error_hints: bool,
) -> Response {
    match result {
        Ok(upstream_response) => {
            let status = StatusCode::from_u16(upstream_response.status)
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

            let body_preview = if status.is_client_error() || status.is_server_error() {
                String::from_utf8_lossy(
                    &upstream_response.body[..upstream_response.body.len().min(512)],
                )
                .to_string()
            } else {
                String::new()
            };

            if status.is_success() {
                tracing::info!(
                    status = status.as_u16(),
                    body_bytes = upstream_response.body.len(),
                    "MCP upstream responded OK"
                );
            } else {
                tracing::warn!(
                    status = status.as_u16(),
                    body_bytes = upstream_response.body.len(),
                    body_preview = %body_preview,
                    "MCP upstream responded with error"
                );
            }

            let filtered_headers =
                ProxyMcpUseCase::<P>::filter_response_headers(upstream_response.headers);

            let mut response_builder = Response::builder().status(status);
            for (name, value) in &filtered_headers {
                response_builder = response_builder.header(name.as_str(), value.as_str());
            }
            response_builder
                .body(axum::body::Body::from(upstream_response.body))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(error) if oauth_error_hints => proxy_error_response(error, headers),
        Err(error) => local_proxy_error_response(error),
    }
}

pub async fn protected_resource_metadata<P: McpProxyPort + 'static>(
    State(state): State<AppState<P>>,
    headers: HeaderMap,
) -> Response {
    let host = effective_host(&headers);
    let validation = state.proxy_mcp.hostname_validation();
    if let Err(e) = validate_host(
        &host,
        validation.expected_host.as_deref(),
        validation.enforce,
    ) {
        return proxy_error_response(e, &headers);
    }

    let base_url = resolve_base_url(&headers);
    Json(json!({
        "resource": format!("{base_url}/mcp"),
        "authorization_servers": [base_url],
    }))
    .into_response()
}

pub async fn mcp_reverse_proxy<P: McpProxyPort + 'static>(
    State(state): State<AppState<P>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let host = effective_host(&headers);
    let raw_auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let auth_header = raw_auth.unwrap_or("");

    tracing::info!(
        method = %method,
        path = %uri,
        host = %host,
        has_auth_header = raw_auth.is_some(),
        auth_scheme = auth_header.split_once(' ').map(|(s, _)| s).unwrap_or("<none>"),
        token_hint = %elide_secret(auth_header.split_once(' ').map(|(_, t)| t).unwrap_or("")),
        "MCP request received"
    );

    if let Err(error) = validate_access_token(&state, &headers, &method, &uri, &host).await {
        return proxy_error_response(error, &headers);
    }

    let header_pairs = header_pairs(&headers);

    let path = uri.path();
    let query = uri.query();

    upstream_response_to_axum_response::<P>(
        state
            .proxy_mcp
            .handle(
                &host,
                method.as_str(),
                path,
                query,
                header_pairs,
                body.to_vec(),
            )
            .await,
        &headers,
        true,
    )
}

pub async fn local_mcp_proxy<P: McpProxyPort + 'static>(
    State(state): State<AppState<P>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let host = effective_host(&headers);
    let raw_auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let auth_header = raw_auth.unwrap_or("");

    tracing::info!(
        method = %method,
        path = %uri,
        host = %host,
        has_auth_header = raw_auth.is_some(),
        auth_scheme = auth_header.split_once(' ').map(|(s, _)| s).unwrap_or("<none>"),
        token_hint = %elide_secret(auth_header.split_once(' ').map(|(_, t)| t).unwrap_or("")),
        "Local MCP request received"
    );

    let token = match parse_bearer_token(&headers) {
        Ok(token) => token,
        Err(error) => return local_proxy_error_response(error),
    };

    let Some(local_mcp) = state.config.local_mcp.as_ref() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };

    if !constant_time_eq(&local_mcp.bearer_token, token) {
        tracing::warn!(
            received_token_hint = %elide_secret(token),
            "Local MCP proxy rejected: static bearer token mismatch"
        );
        return local_proxy_error_response(ProxyError::Unauthorized(
            "Missing or invalid bearer token".into(),
        ));
    }

    let header_pairs = header_pairs(&headers);
    let path = uri.path();
    let query = uri.query();

    upstream_response_to_axum_response::<P>(
        state
            .proxy_mcp
            .handle_unvalidated(
                &host,
                method.as_str(),
                path,
                query,
                header_pairs,
                body.to_vec(),
            )
            .await,
        &headers,
        false,
    )
}
