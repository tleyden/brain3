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

        let mut builder = self.client.request(method, &request.url);

        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }

        builder = builder.body(request.body);

        let response = builder
            .send()
            .await
            .map_err(|e| ProxyError::BadGateway(format!("MCP upstream unavailable: {e}")))?;

        let status = response.status().as_u16();
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
            .map_err(|e| ProxyError::BadGateway(format!("failed to read upstream body: {e}")))?
            .to_vec();

        Ok(McpProxyResponse {
            status,
            headers,
            body,
        })
    }
}
