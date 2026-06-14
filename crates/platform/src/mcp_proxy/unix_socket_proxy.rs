use std::path::PathBuf;
use std::time::Instant;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::client::conn::http1;
use hyper::{Method, Request, Uri};
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;

use brain3_core::domain::errors::ProxyError;
use brain3_core::ports::mcp_proxy::{McpProxyPort, McpProxyRequest, McpProxyResponse};

pub struct UnixSocketMcpProxy {
    socket_path: PathBuf,
}

impl UnixSocketMcpProxy {
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }
}

#[async_trait::async_trait]
impl McpProxyPort for UnixSocketMcpProxy {
    async fn forward(&self, request: McpProxyRequest) -> Result<McpProxyResponse, ProxyError> {
        // Extract the path+query from the URL; we connect via the socket, not the host.
        let uri: Uri = request
            .url
            .parse()
            .map_err(|e| ProxyError::BadGateway(format!("invalid upstream URL: {e}")))?;
        let path_and_query = uri
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/")
            .to_string();

        let stream = UnixStream::connect(&self.socket_path).await.map_err(|e| {
            tracing::warn!(
                socket = %self.socket_path.display(),
                error = %e,
                "unix socket: failed to connect to MCP container"
            );
            ProxyError::BadGateway(format!(
                "MCP unix socket unavailable ({}): {e}",
                self.socket_path.display()
            ))
        })?;

        let (mut sender, conn) = http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| ProxyError::BadGateway(format!("HTTP/1.1 handshake failed: {e}")))?;

        tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::debug!(error = %e, "unix socket: connection driver closed");
            }
        });

        let method: Method = request
            .method
            .parse()
            .map_err(|e| ProxyError::BadGateway(format!("invalid HTTP method: {e}")))?;

        tracing::debug!(
            method = %method,
            path = %path_and_query,
            socket = %self.socket_path.display(),
            body_bytes = request.body.len(),
            "unix socket: sending request to MCP container"
        );

        let mut builder = Request::builder()
            .method(method)
            .uri(&path_and_query)
            .header("host", "localhost");

        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }

        let hyper_request = builder
            .body(Full::new(Bytes::from(request.body)))
            .map_err(|e| ProxyError::BadGateway(format!("failed to build HTTP request: {e}")))?;

        let start = Instant::now();
        let response = sender.send_request(hyper_request).await.map_err(|e| {
            tracing::warn!(
                socket = %self.socket_path.display(),
                error = %e,
                "unix socket: request to MCP container failed"
            );
            ProxyError::BadGateway(format!("MCP unix socket upstream unavailable: {e}"))
        })?;
        let elapsed = start.elapsed();

        let status = response.status().as_u16();
        tracing::debug!(
            status,
            elapsed_ms = elapsed.as_millis() as u64,
            "unix socket: MCP container responded"
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
            .into_body()
            .collect()
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "unix socket: failed to read MCP container response body");
                ProxyError::BadGateway(format!("failed to read unix socket upstream body: {e}"))
            })?
            .to_bytes()
            .to_vec();

        Ok(McpProxyResponse {
            status,
            headers,
            body,
        })
    }
}
