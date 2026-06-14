use std::collections::HashSet;
use std::sync::{Arc, LazyLock};
use std::time::SystemTime;

use crate::domain::errors::ProxyError;
use crate::domain::model::HostnameValidationConfig;
use crate::domain::redact::elide_secret;
use crate::ports::mcp_proxy::{McpProxyPort, McpProxyRequest, McpProxyResponse};
use crate::ports::token_store::{StoredTokenKind, TokenStore};

use super::validate_request::{validate_bearer_token, validate_host};

static HOP_BY_HOP_HEADERS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailers",
        "transfer-encoding",
        "upgrade",
    ]
    .into_iter()
    .collect()
});

static REQUEST_STRIP_HEADERS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut set = HOP_BY_HOP_HEADERS.clone();
    set.insert("authorization");
    set.insert("content-length");
    set.insert("host");
    set.insert("x-brain3-upstream-secret");
    set
});

pub struct ProxyMcpUseCase {
    proxy: Arc<dyn McpProxyPort>,
    upstream_url: String,
    upstream_secret: String,
    token_store: Arc<dyn TokenStore>,
    hostname_validation: HostnameValidationConfig,
}

impl ProxyMcpUseCase {
    pub fn new(
        proxy: Arc<dyn McpProxyPort>,
        upstream_url: String,
        upstream_secret: String,
        token_store: Arc<dyn TokenStore>,
        hostname_validation: HostnameValidationConfig,
    ) -> Self {
        Self {
            proxy,
            upstream_url: upstream_url.trim_end_matches('/').to_string(),
            upstream_secret,
            token_store,
            hostname_validation,
        }
    }

    pub fn hostname_validation(&self) -> &HostnameValidationConfig {
        &self.hostname_validation
    }

    #[allow(clippy::too_many_arguments)]
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

        let received_token = match validate_bearer_token(auth_header) {
            Ok(token) => token,
            Err(e) => {
                tracing::info!(
                    method = method,
                    path = path,
                    host = request_host,
                    "MCP proxy: unauthenticated probe, returning 401 with resource metadata"
                );
                return Err(e);
            }
        };

        let token_data = match self.token_store.get(received_token).await {
            Ok(Some(data)) => data,
            Ok(None) => {
                tracing::warn!(
                    received_token_hint = %elide_secret(received_token),
                    method = method,
                    path = path,
                    host = request_host,
                    "MCP proxy rejected: bearer token not found"
                );
                return Err(ProxyError::Unauthorized(
                    "Missing or invalid bearer token".into(),
                ));
            }
            Err(error) => {
                tracing::error!(
                    %error,
                    method = method,
                    path = path,
                    host = request_host,
                    "MCP proxy rejected: token store lookup failed"
                );
                return Err(ProxyError::Unauthorized(
                    "Missing or invalid bearer token".into(),
                ));
            }
        };

        if token_data.expires_at <= SystemTime::now() {
            tracing::warn!(
                received_token_hint = %elide_secret(received_token),
                client_id = %token_data.client_id,
                method = method,
                path = path,
                host = request_host,
                "MCP proxy rejected: bearer token expired"
            );
            return Err(ProxyError::Unauthorized(
                "Missing or invalid bearer token".into(),
            ));
        }

        if token_data.kind != StoredTokenKind::Access {
            tracing::warn!(
                received_token_hint = %elide_secret(received_token),
                client_id = %token_data.client_id,
                method = method,
                path = path,
                host = request_host,
                "MCP proxy rejected: token kind was not access"
            );
            return Err(ProxyError::Unauthorized(
                "Missing or invalid bearer token".into(),
            ));
        }

        let upstream_url = self.build_upstream_url(path, query);
        let filtered_headers = self.filter_request_headers(headers);

        let header_count = filtered_headers.len() + 1; // +1 for upstream secret
        tracing::info!(
            method = method,
            path = path,
            upstream_url = %upstream_url,
            forwarded_headers = header_count,
            body_bytes = body.len(),
            upstream_secret_hint = %elide_secret(&self.upstream_secret),
            "MCP proxy: forwarding authenticated request to upstream"
        );
        tracing::debug!(
            forwarded_header_names = ?filtered_headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            "MCP proxy: forwarded request headers"
        );
        tracing::trace!(
            body = %String::from_utf8_lossy(&body[..body.len().min(1024)]),
            "MCP proxy: request body"
        );

        let mut final_headers = filtered_headers;
        final_headers.push((
            "x-brain3-upstream-secret".into(),
            self.upstream_secret.clone(),
        ));

        let response = self
            .proxy
            .forward(McpProxyRequest {
                method: method.into(),
                url: upstream_url.clone(),
                headers: final_headers,
                body,
            })
            .await?;

        tracing::debug!(
            upstream_url = %upstream_url,
            status = response.status,
            response_headers = ?response.headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            body_bytes = response.body.len(),
            "MCP proxy: upstream response received"
        );
        tracing::trace!(
            body = %String::from_utf8_lossy(&response.body[..response.body.len().min(1024)]),
            "MCP proxy: response body"
        );

        Ok(response)
    }

    fn build_upstream_url(&self, path: &str, query: Option<&str>) -> String {
        let normalized_path = if path == "/mcp/" { "/mcp" } else { path };
        let query_part = match query {
            Some(q) if !q.is_empty() => format!("?{q}"),
            _ => String::new(),
        };
        format!("{}{}{}", self.upstream_url, normalized_path, query_part)
    }

    fn filter_request_headers(&self, headers: Vec<(String, String)>) -> Vec<(String, String)> {
        headers
            .into_iter()
            .filter(|(name, _)| !REQUEST_STRIP_HEADERS.contains(name.to_lowercase().as_str()))
            .collect()
    }

    pub fn filter_response_headers(headers: Vec<(String, String)>) -> Vec<(String, String)> {
        headers
            .into_iter()
            .filter(|(name, _)| !HOP_BY_HOP_HEADERS.contains(name.to_lowercase().as_str()))
            .collect()
    }
}
