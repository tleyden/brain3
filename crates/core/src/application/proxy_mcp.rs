use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use crate::domain::errors::ProxyError;
use crate::domain::model::HostnameValidationConfig;
use crate::domain::redact::elide_secret;
use crate::ports::mcp_proxy::{McpProxyPort, McpProxyRequest, McpProxyResponse};

use super::validate_request::validate_host;

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
    hostname_validation: HostnameValidationConfig,
}

impl<P: McpProxyPort> ProxyMcpUseCase<P> {
    pub fn new(
        proxy: Arc<P>,
        upstream_url: String,
        upstream_secret: String,
        hostname_validation: HostnameValidationConfig,
    ) -> Self {
        Self {
            proxy,
            upstream_url: upstream_url.trim_end_matches('/').to_string(),
            upstream_secret,
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

        self.forward_request(request_host, method, path, query, headers, body)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn handle_unvalidated(
        &self,
        request_host: &str,
        method: &str,
        path: &str,
        query: Option<&str>,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Result<McpProxyResponse, ProxyError> {
        self.forward_request(request_host, method, path, query, headers, body)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn forward_request(
        &self,
        request_host: &str,
        method: &str,
        path: &str,
        query: Option<&str>,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Result<McpProxyResponse, ProxyError> {
        let upstream_url = self.build_upstream_url(path, query);
        let original_host_header = header_value(&headers, "host").map(str::to_owned);
        let original_x_forwarded_host =
            header_value(&headers, "x-forwarded-host").map(str::to_owned);
        let filtered_headers = self.filter_request_headers(headers);
        let forwarded_host_header = header_value(&filtered_headers, "host").map(str::to_owned);
        let upstream_authority = url_authority(&upstream_url);

        let header_count = filtered_headers.len() + 1; // +1 for upstream secret
        tracing::info!(
            method = method,
            path = path,
            request_host = request_host,
            upstream_url = %upstream_url,
            upstream_authority = ?upstream_authority,
            original_host_header = ?original_host_header,
            original_x_forwarded_host = ?original_x_forwarded_host,
            forwarded_host_header = ?forwarded_host_header,
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

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn url_authority(url: &str) -> Option<&str> {
    url.split_once("://")
        .map(|(_, rest)| rest.split('/').next().unwrap_or(rest))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::*;

    struct CapturingProxy {
        captured: Arc<Mutex<Option<McpProxyRequest>>>,
    }

    #[async_trait]
    impl McpProxyPort for CapturingProxy {
        async fn forward(&self, request: McpProxyRequest) -> Result<McpProxyResponse, ProxyError> {
            *self.captured.lock().expect("capture lock should succeed") = Some(request);
            Ok(McpProxyResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: br#"{"jsonrpc":"2.0","result":{}}"#.to_vec(),
            })
        }
    }

    #[tokio::test]
    async fn handle_forwards_request_without_auth_dependency() {
        let captured = Arc::new(Mutex::new(None));
        let proxy = Arc::new(CapturingProxy {
            captured: Arc::clone(&captured),
        });
        let use_case = ProxyMcpUseCase::new(
            proxy,
            "http://127.0.0.1:8420".into(),
            "shared-secret".into(),
            HostnameValidationConfig {
                expected_host: None,
                enforce: true,
            },
        );

        let response = use_case
            .handle(
                "brain3.example.com",
                "POST",
                "/mcp",
                None,
                vec![("content-type".into(), "application/json".into())],
                br#"{"jsonrpc":"2.0","method":"ping"}"#.to_vec(),
            )
            .await
            .expect("proxy forwarding should succeed");

        assert_eq!(response.status, 200);

        let request = captured
            .lock()
            .expect("capture lock should succeed")
            .take()
            .expect("request should be forwarded");
        assert_eq!(request.url, "http://127.0.0.1:8420/mcp");
        assert!(!request
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("authorization")));
    }
}
