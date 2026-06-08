use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use crate::domain::errors::ProxyError;
use crate::domain::model::HostnameValidationConfig;
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
        validate_bearer_token(auth_header, &self.access_token)?;

        let upstream_url = self.build_upstream_url(path, query);
        let mut filtered_headers = self.filter_request_headers(headers);
        filtered_headers.push((
            "x-brain3-upstream-secret".into(),
            self.upstream_secret.clone(),
        ));

        self.proxy
            .forward(McpProxyRequest {
                method: method.into(),
                url: upstream_url,
                headers: filtered_headers,
                body,
            })
            .await
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
