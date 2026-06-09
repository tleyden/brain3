use brain3_core::domain::errors::ProxyError;
use brain3_core::ports::mcp_proxy::{McpProxyPort, McpProxyRequest, McpProxyResponse};

pub struct ReqwestMcpProxy {
    client: reqwest::Client,
}

impl Default for ReqwestMcpProxy {
    fn default() -> Self {
        Self::new()
    }
}

impl ReqwestMcpProxy {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .expect("failed to build reqwest client");
        Self { client }
    }
}

#[async_trait::async_trait]
impl McpProxyPort for ReqwestMcpProxy {
    async fn forward(&self, request: McpProxyRequest) -> Result<McpProxyResponse, ProxyError> {
        let method = request
            .method
            .parse::<reqwest::Method>()
            .map_err(|e| ProxyError::BadGateway(format!("invalid method: {e}")))?;

        tracing::debug!(
            method = %method,
            url = %request.url,
            header_count = request.headers.len(),
            body_bytes = request.body.len(),
            "reqwest: sending request to MCP container"
        );

        let mut builder = self.client.request(method, &request.url);

        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }

        builder = builder.body(request.body);

        let start = std::time::Instant::now();
        let response = builder.send().await.map_err(|e| {
            tracing::warn!(
                url = %request.url,
                error = %e,
                "reqwest: failed to reach MCP container"
            );
            ProxyError::BadGateway(format!("MCP upstream unavailable: {e}"))
        })?;
        let elapsed = start.elapsed();

        let status = response.status().as_u16();
        tracing::debug!(
            status = status,
            elapsed_ms = elapsed.as_millis() as u64,
            "reqwest: MCP container responded"
        );

        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    value.to_str().unwrap_or("").to_string(),
                )
            })
            .collect();
        let body = response
            .bytes()
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "reqwest: failed to read MCP container response body");
                ProxyError::BadGateway(format!("failed to read upstream body: {e}"))
            })?
            .to_vec();

        Ok(McpProxyResponse {
            status,
            headers,
            body,
        })
    }
}
