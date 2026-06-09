use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use crate::domain::errors::ProxyError;
use crate::domain::model::HostnameValidationConfig;
use crate::domain::redact::elide_secret;
use crate::ports::mcp_proxy::{McpProxyPort, McpProxyRequest, McpProxyResponse};

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

pub struct ProxyMcpUseCase<P: McpProxyPort> {
    proxy: Arc<P>,
    upstream_url: String,
    upstream_secret: String,
    access_token: String,
    hostname_validation: HostnameValidationConfig,
}

impl<P: McpProxyPort> ProxyMcpUseCase<P> {
    pub fn new(
        proxy: Arc<P>,
        upstream_url: String,
        upstream_secret: String,
        access_token: String,
        hostname_validation: HostnameValidationConfig,
    ) -> Self {
        Self {
            proxy,
            upstream_url: upstream_url.trim_end_matches('/').to_string(),
            upstream_secret,
            access_token,
            hostname_validation,
        }
    }

    pub fn hostname_validation(&self) -> &HostnameValidationConfig {
        &self.hostname_validation
    }

    pub fn access_token(&self) -> &str {
        &self.access_token
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

        let received_token = auth_header.split_once(' ').map(|(_, t)| t).unwrap_or("");
        if let Err(e) = validate_bearer_token(auth_header, &self.access_token) {
            if received_token.is_empty() {
                tracing::info!(
                    method = method,
                    path = path,
                    host = request_host,
                    "MCP proxy: unauthenticated probe, returning 401 with resource metadata"
                );
            } else {
                tracing::warn!(
                    received_token_hint = %elide_secret(received_token),
                    expected_token_hint = %elide_secret(&self.access_token),
                    method = method,
                    path = path,
                    host = request_host,
                    "MCP proxy rejected: bearer token mismatch"
                );
            }
            return Err(e);
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

        let response = self.proxy
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
