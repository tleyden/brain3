use std::sync::Arc;

use serde_json::{json, Value};

use crate::domain::errors::ProxyError;
use crate::domain::redact::elide_secret;
use crate::ports::mcp_proxy::{McpProxyPort, McpProxyRequest, McpProxyResponse};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMcpContainerToolSchema {
    pub prefixed_name: String,
    pub original_name: String,
    pub schema: Value,
}

pub struct RemoteMcpContainerClient<P: McpProxyPort> {
    container_name: String,
    mcp_url: String,
    bearer_token: Option<String>,
    proxy: Arc<P>,
    tool_schemas: Vec<RemoteMcpContainerToolSchema>,
}

impl<P: McpProxyPort> RemoteMcpContainerClient<P> {
    pub async fn initialize_and_cache_tools(
        container_name: String,
        mcp_url: String,
        bearer_token: Option<String>,
        proxy: Arc<P>,
    ) -> Result<Self, ProxyError> {
        let client = Self {
            container_name,
            mcp_url,
            bearer_token,
            proxy,
            tool_schemas: Vec::new(),
        };

        tracing::info!(
            container = %client.container_name,
            mcp_url = %client.mcp_url,
            bearer_token_configured = client.bearer_token.is_some(),
            "initializing extra MCP container client"
        );
        client.initialize().await?;
        let tool_schemas = client.fetch_prefixed_tool_schemas().await?;

        tracing::info!(
            container = %client.container_name,
            tool_count = tool_schemas.len(),
            "cached extra MCP container tool schemas"
        );

        Ok(Self {
            tool_schemas,
            ..client
        })
    }

    pub fn container_name(&self) -> &str {
        &self.container_name
    }

    pub fn prefixed_tool_schemas(&self) -> Vec<Value> {
        self.tool_schemas
            .iter()
            .map(|tool| tool.schema.clone())
            .collect()
    }

    pub fn strip_prefix<'a>(&self, tool_name: &'a str) -> Option<&'a str> {
        tool_name
            .strip_prefix(self.container_name.as_str())
            .and_then(|rest| rest.strip_prefix("__"))
            .filter(|name| !name.is_empty())
    }

    pub async fn call_tool(
        &self,
        request: &Value,
        unprefixed_tool_name: &str,
    ) -> Result<McpProxyResponse, ProxyError> {
        let mut forwarded = request.clone();
        let Some(params) = forwarded.get_mut("params").and_then(Value::as_object_mut) else {
            return Err(ProxyError::BadGateway(
                "extra MCP tools/call request missing params object".into(),
            ));
        };
        params.insert(
            "name".into(),
            Value::String(unprefixed_tool_name.to_string()),
        );

        let body = serde_json::to_vec(&forwarded).map_err(|error| {
            ProxyError::BadGateway(format!("failed to serialize extra MCP tools/call: {error}"))
        })?;

        tracing::info!(
            container = %self.container_name,
            tool_name = unprefixed_tool_name,
            request_id = ?request.get("id"),
            "forwarding tools/call to extra MCP container"
        );

        self.forward_json(body).await
    }

    async fn initialize(&self) -> Result<(), ProxyError> {
        let response = self
            .forward_json(
                serde_json::to_vec(&json!({
                    "jsonrpc": "2.0",
                    "id": format!("brain3-extra-{}-initialize", self.container_name),
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {},
                        "clientInfo": {
                            "name": "brain3-gateway",
                            "version": env!("CARGO_PKG_VERSION"),
                        }
                    }
                }))
                .map_err(|error| {
                    ProxyError::BadGateway(format!(
                        "failed to serialize extra MCP initialize: {error}"
                    ))
                })?,
            )
            .await?;
        self.require_success("initialize", &response)
    }

    async fn fetch_prefixed_tool_schemas(
        &self,
    ) -> Result<Vec<RemoteMcpContainerToolSchema>, ProxyError> {
        let response = self
            .forward_json(
                serde_json::to_vec(&json!({
                    "jsonrpc": "2.0",
                    "id": format!("brain3-extra-{}-tools-list", self.container_name),
                    "method": "tools/list",
                    "params": {}
                }))
                .map_err(|error| {
                    ProxyError::BadGateway(format!(
                        "failed to serialize extra MCP tools/list: {error}"
                    ))
                })?,
            )
            .await?;
        self.require_success("tools/list", &response)?;

        let body = serde_json::from_slice::<Value>(&response.body).map_err(|error| {
            ProxyError::BadGateway(format!(
                "extra MCP container '{}' returned invalid tools/list JSON: {error}",
                self.container_name
            ))
        })?;
        let tools = body
            .get("result")
            .and_then(|result| result.get("tools"))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ProxyError::BadGateway(format!(
                    "extra MCP container '{}' tools/list response missing result.tools array",
                    self.container_name
                ))
            })?;

        let mut schemas = Vec::new();
        for tool in tools {
            let Some(original_name) = tool.get("name").and_then(Value::as_str) else {
                tracing::warn!(
                    container = %self.container_name,
                    schema = %tool,
                    "skipping extra MCP tool schema without string name"
                );
                continue;
            };
            let mut schema = tool.clone();
            if let Some(object) = schema.as_object_mut() {
                object.insert(
                    "name".into(),
                    Value::String(format!("{}__{}", self.container_name, original_name)),
                );
            } else {
                tracing::warn!(
                    container = %self.container_name,
                    schema = %tool,
                    "skipping non-object extra MCP tool schema"
                );
                continue;
            }

            schemas.push(RemoteMcpContainerToolSchema {
                prefixed_name: format!("{}__{}", self.container_name, original_name),
                original_name: original_name.to_string(),
                schema,
            });
        }

        Ok(schemas)
    }

    async fn forward_json(&self, body: Vec<u8>) -> Result<McpProxyResponse, ProxyError> {
        let mut headers = vec![
            ("content-type".into(), "application/json".into()),
            (
                "accept".into(),
                "application/json, text/event-stream".into(),
            ),
        ];
        if let Some(token) = &self.bearer_token {
            headers.push(("authorization".into(), format!("Bearer {token}")));
        }

        tracing::debug!(
            container = %self.container_name,
            mcp_url = %self.mcp_url,
            bearer_token_hint = ?self.bearer_token.as_ref().map(|token| elide_secret(token)),
            body_bytes = body.len(),
            "sending request to extra MCP container"
        );

        self.proxy
            .forward(McpProxyRequest {
                method: "POST".into(),
                url: self.mcp_url.clone(),
                headers,
                body,
            })
            .await
    }

    fn require_success(
        &self,
        operation: &str,
        response: &McpProxyResponse,
    ) -> Result<(), ProxyError> {
        if (200..300).contains(&response.status) {
            return Ok(());
        }

        Err(ProxyError::BadGateway(format!(
            "extra MCP container '{}' {operation} failed with HTTP {}",
            self.container_name, response.status
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use serde_json::json;

    use crate::ports::mcp_proxy::McpProxyRequest;

    use super::*;

    #[derive(Default)]
    struct CapturingProxy {
        requests: Arc<Mutex<Vec<McpProxyRequest>>>,
        responses: Arc<Mutex<Vec<McpProxyResponse>>>,
    }

    impl CapturingProxy {
        fn with_responses(responses: Vec<McpProxyResponse>) -> Self {
            Self {
                responses: Arc::new(Mutex::new(responses)),
                ..Default::default()
            }
        }
    }

    #[async_trait]
    impl McpProxyPort for CapturingProxy {
        async fn forward(&self, request: McpProxyRequest) -> Result<McpProxyResponse, ProxyError> {
            self.requests
                .lock()
                .expect("requests lock should succeed")
                .push(request);
            Ok(self
                .responses
                .lock()
                .expect("responses lock should succeed")
                .remove(0))
        }
    }

    fn json_response(value: Value) -> McpProxyResponse {
        McpProxyResponse {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: serde_json::to_vec(&value).expect("test JSON should serialize"),
        }
    }

    #[tokio::test]
    async fn initialize_and_cache_tools_prefixes_tool_names_and_sends_bearer_auth() {
        let proxy = Arc::new(CapturingProxy::with_responses(vec![
            json_response(json!({"jsonrpc": "2.0", "id": 1, "result": {}})),
            json_response(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": {
                    "tools": [
                        {
                            "name": "search_deck",
                            "description": "Search deck",
                            "inputSchema": { "type": "object" }
                        }
                    ]
                }
            })),
        ]));

        let client = RemoteMcpContainerClient::initialize_and_cache_tools(
            "fluensy_learn".into(),
            "http://127.0.0.1:18420/mcp".into(),
            Some("secret-token".into()),
            proxy.clone(),
        )
        .await
        .expect("client should initialize");

        let schemas = client.prefixed_tool_schemas();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0]["name"], "fluensy_learn__search_deck");
        assert_eq!(schemas[0]["description"], "Search deck");

        let requests = proxy.requests.lock().expect("requests lock should succeed");
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].url, "http://127.0.0.1:18420/mcp");
        assert!(requests[0]
            .headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("authorization")
                && value == "Bearer secret-token"));
    }

    #[tokio::test]
    async fn call_tool_strips_container_prefix_before_forwarding() {
        let proxy = Arc::new(CapturingProxy::with_responses(vec![
            json_response(json!({"jsonrpc": "2.0", "id": 1, "result": {}})),
            json_response(json!({"jsonrpc": "2.0", "id": 2, "result": {"tools": []}})),
            json_response(json!({
                "jsonrpc": "2.0",
                "id": 7,
                "result": {
                    "content": [{ "type": "text", "text": "extra response" }]
                }
            })),
        ]));
        let client = RemoteMcpContainerClient::initialize_and_cache_tools(
            "fluensy_learn".into(),
            "http://127.0.0.1:18420/mcp".into(),
            None,
            proxy.clone(),
        )
        .await
        .expect("client should initialize");

        let response = client
            .call_tool(
                &json!({
                    "jsonrpc": "2.0",
                    "id": 7,
                    "method": "tools/call",
                    "params": {
                        "name": "fluensy_learn__search_deck",
                        "arguments": { "query": "rust" }
                    }
                }),
                "search_deck",
            )
            .await
            .expect("tool call should forward");

        assert_eq!(response.status, 200);
        let requests = proxy.requests.lock().expect("requests lock should succeed");
        let body: Value =
            serde_json::from_slice(&requests[2].body).expect("forwarded body should be JSON");
        assert_eq!(body["params"]["name"], "search_deck");
        assert_eq!(body["params"]["arguments"]["query"], "rust");
        assert!(!requests[2]
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("authorization")));
    }
}
