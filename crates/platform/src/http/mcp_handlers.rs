use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Json;
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
    let raw_auth = headers.get("authorization").and_then(|v| v.to_str().ok());
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

    let header_pairs: Vec<(String, String)> = headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or("").to_string(),
            )
        })
        .collect();

    let path = uri.path();
    let query = uri.query();

    match state
        .proxy_mcp
        .handle(
            &host,
            auth_header,
            method.as_str(),
            path,
            query,
            header_pairs,
            body.to_vec(),
        )
        .await
    {
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
        Err(e) => proxy_error_response(e, &headers),
    }
}
