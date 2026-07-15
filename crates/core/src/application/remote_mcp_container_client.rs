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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMcpContainerResourceSchema {
    pub prefixed_uri: String,
    pub original_uri: String,
    pub schema: Value,
}

pub struct RemoteMcpContainerClient<P: McpProxyPort> {
    container_name: String,
    mcp_url: String,
    bearer_token: Option<String>,
    proxy: Arc<P>,
    tool_schemas: Vec<RemoteMcpContainerToolSchema>,
    resource_schemas: Vec<RemoteMcpContainerResourceSchema>,
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
            resource_schemas: Vec::new(),
        };

        tracing::info!(
            container = %client.container_name,
            mcp_url = %client.mcp_url,
            bearer_token_configured = client.bearer_token.is_some(),
            "initializing Plugin MCP Container client"
        );
        client.initialize().await?;
        let tool_schemas = client.fetch_tool_schemas().await?;
        let resource_schemas = client.fetch_prefixed_resource_schemas().await?;
        let tool_schemas = client.prefix_tool_schemas(&tool_schemas, &resource_schemas);

        tracing::info!(
            container = %client.container_name,
            tool_count = tool_schemas.len(),
            resource_count = resource_schemas.len(),
            "cached Plugin MCP Container tool schemas"
        );

        Ok(Self {
            tool_schemas,
            resource_schemas,
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

    pub fn prefixed_resource_schemas(&self) -> Vec<Value> {
        self.resource_schemas
            .iter()
            .map(|resource| resource.schema.clone())
            .collect()
    }

    pub fn strip_prefix<'a>(&self, tool_name: &'a str) -> Option<&'a str> {
        tool_name
            .strip_prefix(self.container_name.as_str())
            .and_then(|rest| rest.strip_prefix("__"))
            .filter(|name| !name.is_empty())
    }

    pub fn has_tool(&self, unprefixed_tool_name: &str) -> bool {
        self.tool_schemas
            .iter()
            .any(|schema| schema.original_name == unprefixed_tool_name)
    }

    pub fn strip_resource_uri_prefix(&self, uri: &str) -> Option<String> {
        strip_resource_uri_prefix_for_container(&self.container_name, uri)
    }

    pub fn has_resource(&self, original_uri: &str) -> bool {
        self.resource_schemas
            .iter()
            .any(|schema| schema.original_uri == original_uri)
    }

    pub async fn call_tool(
        &self,
        request: &Value,
        unprefixed_tool_name: &str,
    ) -> Result<McpProxyResponse, ProxyError> {
        let mut forwarded = request.clone();
        let Some(params) = forwarded.get_mut("params").and_then(Value::as_object_mut) else {
            return Err(ProxyError::BadGateway(
                "Plugin MCP tools/call request missing params object".into(),
            ));
        };
        params.insert(
            "name".into(),
            Value::String(unprefixed_tool_name.to_string()),
        );

        let body = serde_json::to_vec(&forwarded).map_err(|error| {
            ProxyError::BadGateway(format!(
                "failed to serialize Plugin MCP tools/call: {error}"
            ))
        })?;

        tracing::info!(
            container = %self.container_name,
            tool_name = unprefixed_tool_name,
            request_id = ?request.get("id"),
            "forwarding tools/call to Plugin MCP Container"
        );

        self.forward_json(body).await
    }

    pub async fn read_resource(
        &self,
        request: &Value,
        original_uri: &str,
    ) -> Result<McpProxyResponse, ProxyError> {
        let mut forwarded = request.clone();
        let Some(params) = forwarded.get_mut("params").and_then(Value::as_object_mut) else {
            return Err(ProxyError::BadGateway(
                "Plugin MCP resources/read request missing params object".into(),
            ));
        };
        params.insert("uri".into(), Value::String(original_uri.to_string()));

        let body = serde_json::to_vec(&forwarded).map_err(|error| {
            ProxyError::BadGateway(format!(
                "failed to serialize Plugin MCP resources/read: {error}"
            ))
        })?;

        tracing::info!(
            container = %self.container_name,
            resource_uri = original_uri,
            request_id = ?request.get("id"),
            "forwarding resources/read to Plugin MCP Container"
        );

        self.forward_json(body).await
    }

    async fn initialize(&self) -> Result<(), ProxyError> {
        let response = self
            .forward_json(
                serde_json::to_vec(&json!({
                    "jsonrpc": "2.0",
                    "id": format!("brain3-plugin-{}-initialize", self.container_name),
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
                        "failed to serialize Plugin MCP initialize: {error}"
                    ))
                })?,
            )
            .await?;
        self.require_success("initialize", &response)
    }

    async fn fetch_tool_schemas(&self) -> Result<Vec<Value>, ProxyError> {
        let response = self
            .forward_json(
                serde_json::to_vec(&json!({
                    "jsonrpc": "2.0",
                    "id": format!("brain3-plugin-{}-tools-list", self.container_name),
                    "method": "tools/list",
                    "params": {}
                }))
                .map_err(|error| {
                    ProxyError::BadGateway(format!(
                        "failed to serialize Plugin MCP tools/list: {error}"
                    ))
                })?,
            )
            .await?;
        self.require_success("tools/list", &response)?;

        let body = serde_json::from_slice::<Value>(&response.body).map_err(|error| {
            ProxyError::BadGateway(format!(
                "Plugin MCP Container '{}' returned invalid tools/list JSON: {error}",
                self.container_name
            ))
        })?;
        let tools = body
            .get("result")
            .and_then(|result| result.get("tools"))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ProxyError::BadGateway(format!(
                    "Plugin MCP Container '{}' tools/list response missing result.tools array",
                    self.container_name
                ))
            })?;

        Ok(tools.clone())
    }

    fn prefix_tool_schemas(
        &self,
        tools: &[Value],
        resource_schemas: &[RemoteMcpContainerResourceSchema],
    ) -> Vec<RemoteMcpContainerToolSchema> {
        let mut schemas = Vec::new();
        for tool in tools {
            let Some(original_name) = tool.get("name").and_then(Value::as_str) else {
                tracing::warn!(
                    container = %self.container_name,
                    schema = %tool,
                    "skipping Plugin MCP tool schema without string name"
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
                    "skipping non-object Plugin MCP tool schema"
                );
                continue;
            }
            self.rewrite_tool_resource_uri(&mut schema, resource_schemas);

            schemas.push(RemoteMcpContainerToolSchema {
                prefixed_name: format!("{}__{}", self.container_name, original_name),
                original_name: original_name.to_string(),
                schema,
            });
        }

        schemas
    }

    async fn fetch_prefixed_resource_schemas(
        &self,
    ) -> Result<Vec<RemoteMcpContainerResourceSchema>, ProxyError> {
        let response = self
            .forward_json(
                serde_json::to_vec(&json!({
                    "jsonrpc": "2.0",
                    "id": format!("brain3-plugin-{}-resources-list", self.container_name),
                    "method": "resources/list",
                    "params": {}
                }))
                .map_err(|error| {
                    ProxyError::BadGateway(format!(
                        "failed to serialize Plugin MCP resources/list: {error}"
                    ))
                })?,
            )
            .await?;
        self.require_success("resources/list", &response)?;

        let body = serde_json::from_slice::<Value>(&response.body).map_err(|error| {
            ProxyError::BadGateway(format!(
                "Plugin MCP Container '{}' returned invalid resources/list JSON: {error}",
                self.container_name
            ))
        })?;
        if let Some(error) = body.get("error") {
            tracing::info!(
                container = %self.container_name,
                error = %error,
                "Plugin MCP Container resources/list returned JSON-RPC error; continuing without resources"
            );
            return Ok(Vec::new());
        }

        let Some(resources) = body
            .get("result")
            .and_then(|result| result.get("resources"))
            .and_then(Value::as_array)
        else {
            tracing::debug!(
                container = %self.container_name,
                "Plugin MCP Container resources/list response missing result.resources array; continuing without resources"
            );
            return Ok(Vec::new());
        };

        let mut schemas = Vec::new();
        for resource in resources {
            let Some(original_uri) = resource.get("uri").and_then(Value::as_str) else {
                tracing::warn!(
                    container = %self.container_name,
                    schema = %resource,
                    "skipping Plugin MCP resource schema without string uri"
                );
                continue;
            };
            let Some(prefixed_uri) =
                prefix_resource_uri_for_container(&self.container_name, original_uri)
            else {
                tracing::warn!(
                    container = %self.container_name,
                    resource_uri = original_uri,
                    "skipping Plugin MCP resource schema with unprefixable URI"
                );
                continue;
            };
            let mut schema = resource.clone();
            if let Some(object) = schema.as_object_mut() {
                object.insert("uri".into(), Value::String(prefixed_uri.clone()));
            } else {
                tracing::warn!(
                    container = %self.container_name,
                    schema = %resource,
                    "skipping non-object Plugin MCP resource schema"
                );
                continue;
            }

            schemas.push(RemoteMcpContainerResourceSchema {
                prefixed_uri,
                original_uri: original_uri.to_string(),
                schema,
            });
        }

        tracing::debug!(
            container = %self.container_name,
            resource_count = schemas.len(),
            "cached Plugin MCP Container resource schemas"
        );

        Ok(schemas)
    }

    fn rewrite_tool_resource_uri(
        &self,
        schema: &mut Value,
        resource_schemas: &[RemoteMcpContainerResourceSchema],
    ) {
        let Some(resource_uri) = get_tool_resource_uri(schema).map(str::to_string) else {
            return;
        };
        let Some(resource_schema) = resource_schemas
            .iter()
            .find(|resource| resource.original_uri == resource_uri)
        else {
            tracing::warn!(
                container = %self.container_name,
                resource_uri,
                "Plugin MCP tool schema references a UI resource URI not declared by resources/list"
            );
            return;
        };

        set_tool_resource_uri(schema, &resource_schema.prefixed_uri);
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
            "sending request to Plugin MCP Container"
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
            "Plugin MCP Container '{}' {operation} failed with HTTP {}",
            self.container_name, response.status
        )))
    }
}

fn prefix_resource_uri_for_container(container_name: &str, uri: &str) -> Option<String> {
    let (prefix, authority, suffix) = split_uri_authority(uri)?;
    Some(format!("{prefix}{container_name}__{authority}{suffix}"))
}

fn strip_resource_uri_prefix_for_container(container_name: &str, uri: &str) -> Option<String> {
    let (prefix, authority, suffix) = split_uri_authority(uri)?;
    let original_authority = authority
        .strip_prefix(container_name)
        .and_then(|rest| rest.strip_prefix("__"))
        .filter(|authority| !authority.is_empty())?;
    Some(format!("{prefix}{original_authority}{suffix}"))
}

fn split_uri_authority(uri: &str) -> Option<(&str, &str, &str)> {
    let authority_start = uri.find("://")? + 3;
    let rest = &uri[authority_start..];
    let authority_end = rest
        .find(|character| matches!(character, '/' | '?' | '#'))
        .unwrap_or(rest.len());
    if authority_end == 0 {
        return None;
    }
    let authority_absolute_end = authority_start + authority_end;
    Some((
        &uri[..authority_start],
        &uri[authority_start..authority_absolute_end],
        &uri[authority_absolute_end..],
    ))
}

fn get_tool_resource_uri(schema: &Value) -> Option<&str> {
    schema
        .get("_meta")
        .and_then(|meta| {
            meta.get("ui")
                .and_then(|ui| ui.get("resourceUri"))
                .or_else(|| meta.get("ui.resourceUri"))
        })
        .and_then(Value::as_str)
}

fn set_tool_resource_uri(schema: &mut Value, resource_uri: &str) {
    let Some(meta) = schema.get_mut("_meta") else {
        return;
    };
    if let Some(ui) = meta.get_mut("ui").and_then(Value::as_object_mut) {
        if ui.contains_key("resourceUri") {
            ui.insert(
                "resourceUri".into(),
                Value::String(resource_uri.to_string()),
            );
            return;
        }
    }
    if let Some(meta_object) = meta.as_object_mut() {
        if meta_object.contains_key("ui.resourceUri") {
            meta_object.insert(
                "ui.resourceUri".into(),
                Value::String(resource_uri.to_string()),
            );
        }
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
            json_response(json!({"jsonrpc": "2.0", "id": 3, "result": {"resources": []}})),
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
        assert_eq!(requests.len(), 3);
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
            json_response(json!({"jsonrpc": "2.0", "id": 3, "result": {"resources": []}})),
            json_response(json!({
                "jsonrpc": "2.0",
                "id": 7,
                "result": {
                    "content": [{ "type": "text", "text": "plugin response" }]
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
            serde_json::from_slice(&requests[3].body).expect("forwarded body should be JSON");
        assert_eq!(body["params"]["name"], "search_deck");
        assert_eq!(body["params"]["arguments"]["query"], "rust");
        assert!(!requests[3]
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("authorization")));
    }

    #[tokio::test]
    async fn initialize_and_cache_tools_rewrites_matching_ui_resource_uri() {
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
                            "inputSchema": { "type": "object" },
                            "_meta": {
                                "ui": {
                                    "resourceUri": "ui://widget-name/index.html"
                                }
                            }
                        }
                    ]
                }
            })),
            json_response(json!({
                "jsonrpc": "2.0",
                "id": 3,
                "result": {
                    "resources": [
                        {
                            "uri": "ui://widget-name/index.html",
                            "name": "Plugin widget",
                            "mimeType": "text/html"
                        }
                    ]
                }
            })),
        ]));

        let client = RemoteMcpContainerClient::initialize_and_cache_tools(
            "fluensy_learn".into(),
            "http://127.0.0.1:18420/mcp".into(),
            None,
            proxy,
        )
        .await
        .expect("client should initialize");

        let resources = client.prefixed_resource_schemas();
        assert_eq!(resources.len(), 1);
        assert_eq!(
            resources[0]["uri"],
            "ui://fluensy_learn__widget-name/index.html"
        );

        let tools = client.prefixed_tool_schemas();
        assert_eq!(
            tools[0]["_meta"]["ui"]["resourceUri"],
            "ui://fluensy_learn__widget-name/index.html"
        );
    }

    #[tokio::test]
    async fn initialize_and_cache_tools_leaves_unknown_ui_resource_uri_unchanged() {
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
                            "inputSchema": { "type": "object" },
                            "_meta": {
                                "ui.resourceUri": "ui://other-widget/index.html"
                            }
                        }
                    ]
                }
            })),
            json_response(json!({
                "jsonrpc": "2.0",
                "id": 3,
                "result": {
                    "resources": [
                        {
                            "uri": "ui://widget-name/index.html",
                            "name": "Plugin widget",
                            "mimeType": "text/html"
                        }
                    ]
                }
            })),
        ]));

        let client = RemoteMcpContainerClient::initialize_and_cache_tools(
            "fluensy_learn".into(),
            "http://127.0.0.1:18420/mcp".into(),
            None,
            proxy,
        )
        .await
        .expect("client should initialize");

        let tools = client.prefixed_tool_schemas();
        assert_eq!(
            tools[0]["_meta"]["ui.resourceUri"],
            "ui://other-widget/index.html"
        );
    }

    #[tokio::test]
    async fn initialize_and_cache_tools_tolerates_resources_list_method_not_found() {
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
            json_response(json!({
                "jsonrpc": "2.0",
                "id": 3,
                "error": {
                    "code": -32601,
                    "message": "Method not found"
                }
            })),
        ]));

        let client = RemoteMcpContainerClient::initialize_and_cache_tools(
            "fluensy_learn".into(),
            "http://127.0.0.1:18420/mcp".into(),
            None,
            proxy,
        )
        .await
        .expect("client should initialize");

        assert_eq!(client.prefixed_resource_schemas().len(), 0);
        assert_eq!(client.prefixed_tool_schemas().len(), 1);
    }
}
