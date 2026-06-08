use crate::domain::errors::ProxyError;

#[derive(Debug)]
pub struct McpProxyRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug)]
pub struct McpProxyResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[async_trait::async_trait]
pub trait McpProxyPort: Send + Sync {
    async fn forward(&self, request: McpProxyRequest) -> Result<McpProxyResponse, ProxyError>;
}
