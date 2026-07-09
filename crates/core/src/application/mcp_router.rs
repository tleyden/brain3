use std::sync::Arc;

use serde_json::{json, Value};

use crate::application::native_mcp_tool_registry::NativeMcpToolRegistry;
use crate::application::proxy_mcp::ProxyMcpUseCase;
use crate::application::remote_mcp_container_client::RemoteMcpContainerClient;
use crate::application::validate_request::validate_host;
use crate::domain::errors::ProxyError;
use crate::domain::model::HostnameValidationConfig;
use crate::ports::mcp_proxy::{McpProxyPort, McpProxyResponse};
use crate::ports::native_mcp_tool::{NativeMcpToolError, NativeMcpToolOutput};

pub struct McpRouterUseCase<P: McpProxyPort> {
    proxy: Arc<ProxyMcpUseCase<P>>,
    native_tools: Arc<NativeMcpToolRegistry>,
    extra_containers: Vec<Arc<RemoteMcpContainerClient<P>>>,
}

impl<P: McpProxyPort> McpRouterUseCase<P> {
    pub fn new(proxy: Arc<ProxyMcpUseCase<P>>, native_tools: Arc<NativeMcpToolRegistry>) -> Self {
        Self::new_with_extra_containers(proxy, native_tools, Vec::new())
    }

    pub fn new_with_extra_containers(
        proxy: Arc<ProxyMcpUseCase<P>>,
        native_tools: Arc<NativeMcpToolRegistry>,
        extra_containers: Vec<Arc<RemoteMcpContainerClient<P>>>,
    ) -> Self {
        Self {
            proxy,
            native_tools,
            extra_containers,
        }
    }

    pub fn hostname_validation(&self) -> &HostnameValidationConfig {
        self.proxy.hostname_validation()
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
            self.proxy.hostname_validation().expected_host.as_deref(),
            self.proxy.hostname_validation().enforce,
        )?;

        self.route_request(request_host, method, path, query, headers, body)
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
        self.route_request(request_host, method, path, query, headers, body)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn route_request(
        &self,
        request_host: &str,
        method: &str,
        path: &str,
        query: Option<&str>,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Result<McpProxyResponse, ProxyError> {
        let parsed_body = serde_json::from_slice::<Value>(&body).ok();
        let mcp_method = parsed_body
            .as_ref()
            .and_then(|json| json.get("method"))
            .and_then(Value::as_str);

        match mcp_method {
            Some("initialize") => {
                self.native_tools
                    .initialize_all()
                    .await
                    .map_err(native_tool_error_to_proxy_error)?;
                self.proxy
                    .handle_unvalidated(request_host, method, path, query, headers, body)
                    .await
            }
            Some("tools/list") => {
                let response = self
                    .proxy
                    .handle_unvalidated(request_host, method, path, query, headers, body)
                    .await?;
                Ok(self.append_tool_schemas(response))
            }
            Some("tools/call") => {
                if let Some(response) = self.maybe_call_native_tool(parsed_body.as_ref()).await? {
                    return Ok(response);
                }
                if let Some(response) = self.maybe_call_extra_tool(parsed_body.as_ref()).await? {
                    return Ok(response);
                }

                self.proxy
                    .handle_unvalidated(request_host, method, path, query, headers, body)
                    .await
            }
            _ => {
                self.proxy
                    .handle_unvalidated(request_host, method, path, query, headers, body)
                    .await
            }
        }
    }

    async fn maybe_call_native_tool(
        &self,
        request: Option<&Value>,
    ) -> Result<Option<McpProxyResponse>, ProxyError> {
        let Some(request) = request else {
            return Ok(None);
        };
        let Some(params) = request.get("params").and_then(Value::as_object) else {
            return Ok(None);
        };
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return Ok(None);
        };
        let Some(tool) = self.native_tools.find(name) else {
            return Ok(None);
        };

        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        tracing::info!(
            tool_name = name,
            request_id = ?request.get("id"),
            "MCP router: handling native tool call"
        );

        let output = match tool.call(arguments).await {
            Ok(output) => output,
            Err(error) => {
                tracing::warn!(
                    tool_name = name,
                    request_id = ?request.get("id"),
                    error = %error,
                    "MCP router: native tool call returned an error result"
                );
                NativeMcpToolOutput::error_text(error.to_string())
            }
        };

        Ok(Some(native_tool_response(request, output)?))
    }

    async fn maybe_call_extra_tool(
        &self,
        request: Option<&Value>,
    ) -> Result<Option<McpProxyResponse>, ProxyError> {
        let Some(request) = request else {
            return Ok(None);
        };
        let Some(params) = request.get("params").and_then(Value::as_object) else {
            return Ok(None);
        };
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return Ok(None);
        };

        for client in &self.extra_containers {
            if let Some(unprefixed_name) = client.strip_prefix(name) {
                tracing::info!(
                    container = %client.container_name(),
                    prefixed_tool_name = name,
                    tool_name = unprefixed_name,
                    request_id = ?request.get("id"),
                    "MCP router: routing extra MCP tool call"
                );
                return Ok(Some(client.call_tool(request, unprefixed_name).await?));
            }
        }

        Ok(None)
    }

    fn append_tool_schemas(&self, response: McpProxyResponse) -> McpProxyResponse {
        let native_schemas = self.native_tools.list_schemas();
        let extra_schemas = self
            .extra_containers
            .iter()
            .flat_map(|client| client.prefixed_tool_schemas())
            .collect::<Vec<_>>();

        if native_schemas.is_empty() && extra_schemas.is_empty() {
            return response;
        }

        let Ok(mut body) = serde_json::from_slice::<Value>(&response.body) else {
            tracing::warn!("MCP router: could not parse tools/list response body as JSON");
            return response;
        };

        let Some(tools) = body
            .get_mut("result")
            .and_then(|result| result.get_mut("tools"))
            .and_then(Value::as_array_mut)
        else {
            tracing::warn!("MCP router: tools/list response did not contain result.tools array");
            return response;
        };

        let native_tool_count = native_schemas.len();
        let extra_tool_count = extra_schemas.len();
        tools.extend(native_schemas);
        tools.extend(extra_schemas);
        let total_tool_count = tools.len();

        let Ok(new_body) = serde_json::to_vec(&body) else {
            tracing::warn!("MCP router: could not serialize augmented tools/list response");
            return response;
        };

        tracing::info!(
            native_tool_count = native_tool_count,
            extra_tool_count = extra_tool_count,
            total_tool_count = total_tool_count,
            "MCP router: appended native and extra MCP tools to tools/list response"
        );

        McpProxyResponse {
            status: response.status,
            headers: strip_content_length(response.headers),
            body: new_body,
        }
    }
}

fn strip_content_length(headers: Vec<(String, String)>) -> Vec<(String, String)> {
    headers
        .into_iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case("content-length"))
        .collect()
}

fn native_tool_response(
    request: &Value,
    output: NativeMcpToolOutput,
) -> Result<McpProxyResponse, ProxyError> {
    let content = output
        .content
        .into_iter()
        .map(|block| {
            json!({
                "type": "text",
                "text": block.text,
            })
        })
        .collect::<Vec<_>>();

    let body = json!({
        "jsonrpc": "2.0",
        "id": request.get("id").cloned().unwrap_or(Value::Null),
        "result": {
            "content": content,
            "isError": output.is_error,
        }
    });

    let body = serde_json::to_vec(&body)
        .map_err(|error| ProxyError::BadGateway(format!("native MCP response error: {error}")))?;

    Ok(McpProxyResponse {
        status: 200,
        headers: vec![("content-type".into(), "application/json".into())],
        body,
    })
}

fn native_tool_error_to_proxy_error(error: NativeMcpToolError) -> ProxyError {
    ProxyError::BadGateway(format!("native MCP tool initialization failed: {error}"))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use serde_json::{json, Value};

    use crate::application::native_mcp_tool_registry::NativeMcpToolRegistry;
    use crate::application::proxy_mcp::ProxyMcpUseCase;
    use crate::application::remote_mcp_container_client::RemoteMcpContainerClient;
    use crate::domain::errors::ProxyError;
    use crate::domain::model::HostnameValidationConfig;
    use crate::ports::mcp_proxy::{McpProxyPort, McpProxyRequest, McpProxyResponse};
    use crate::ports::native_mcp_tool::{NativeMcpTool, NativeMcpToolError, NativeMcpToolOutput};

    use super::*;

    struct CapturingProxy {
        captured: Arc<Mutex<Vec<McpProxyRequest>>>,
        response_body: Vec<u8>,
    }

    #[async_trait]
    impl McpProxyPort for CapturingProxy {
        async fn forward(&self, request: McpProxyRequest) -> Result<McpProxyResponse, ProxyError> {
            self.captured
                .lock()
                .expect("capture lock should succeed")
                .push(request);
            Ok(McpProxyResponse {
                status: 200,
                headers: vec![
                    ("content-type".into(), "application/json".into()),
                    (
                        "content-length".into(),
                        self.response_body.len().to_string(),
                    ),
                ],
                body: self.response_body.clone(),
            })
        }
    }

    struct FakeNativeTool {
        calls: Arc<Mutex<Vec<Value>>>,
        initialize_count: Arc<Mutex<usize>>,
    }

    impl FakeNativeTool {
        fn new() -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                initialize_count: Arc::new(Mutex::new(0)),
            }
        }
    }

    #[async_trait]
    impl NativeMcpTool for FakeNativeTool {
        fn name(&self) -> &str {
            "fake_native_tool"
        }

        fn description(&self) -> &str {
            "Fake native tool"
        }

        fn input_schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            })
        }

        async fn call(&self, arguments: Value) -> Result<NativeMcpToolOutput, NativeMcpToolError> {
            self.calls
                .lock()
                .expect("calls lock should succeed")
                .push(arguments);
            Ok(NativeMcpToolOutput::text("native response"))
        }

        async fn on_initialize(&self) -> Result<(), NativeMcpToolError> {
            *self
                .initialize_count
                .lock()
                .expect("initialize lock should succeed") += 1;
            Ok(())
        }
    }

    fn router_with_tool(
        proxy_body: Vec<u8>,
    ) -> (
        McpRouterUseCase<CapturingProxy>,
        Arc<Mutex<Vec<McpProxyRequest>>>,
        Arc<FakeNativeTool>,
    ) {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let proxy = Arc::new(CapturingProxy {
            captured: Arc::clone(&captured),
            response_body: proxy_body,
        });
        let proxy_use_case = Arc::new(ProxyMcpUseCase::new(
            proxy,
            "http://127.0.0.1:8420".into(),
            "shared-secret".into(),
            HostnameValidationConfig {
                expected_host: None,
                enforce: true,
            },
        ));
        let tool = Arc::new(FakeNativeTool::new());
        let registry = NativeMcpToolRegistry::new(vec![tool.clone() as Arc<dyn NativeMcpTool>]);
        (
            McpRouterUseCase::new(proxy_use_case, Arc::new(registry)),
            captured,
            tool,
        )
    }

    fn router_with_tool_and_extra(
        proxy_body: Vec<u8>,
        extra_proxy: Arc<CapturingProxy>,
        extra_client: Arc<RemoteMcpContainerClient<CapturingProxy>>,
    ) -> (
        McpRouterUseCase<CapturingProxy>,
        Arc<Mutex<Vec<McpProxyRequest>>>,
        Arc<FakeNativeTool>,
        Arc<Mutex<Vec<McpProxyRequest>>>,
    ) {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let proxy = Arc::new(CapturingProxy {
            captured: Arc::clone(&captured),
            response_body: proxy_body,
        });
        let proxy_use_case = Arc::new(ProxyMcpUseCase::new(
            proxy,
            "http://127.0.0.1:8420".into(),
            "shared-secret".into(),
            HostnameValidationConfig {
                expected_host: None,
                enforce: true,
            },
        ));
        let tool = Arc::new(FakeNativeTool::new());
        let registry = NativeMcpToolRegistry::new(vec![tool.clone() as Arc<dyn NativeMcpTool>]);
        let extra_captured = Arc::clone(&extra_proxy.captured);
        (
            McpRouterUseCase::new_with_extra_containers(
                proxy_use_case,
                Arc::new(registry),
                vec![extra_client],
            ),
            captured,
            tool,
            extra_captured,
        )
    }

    async fn initialized_extra_client(
        response_body: Vec<u8>,
    ) -> (
        Arc<CapturingProxy>,
        Arc<RemoteMcpContainerClient<CapturingProxy>>,
    ) {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let proxy = Arc::new(CapturingProxy {
            captured,
            response_body,
        });
        let client = RemoteMcpContainerClient::initialize_and_cache_tools(
            "fluensy_learn".into(),
            "http://127.0.0.1:18420/mcp".into(),
            None,
            Arc::clone(&proxy),
        )
        .await
        .expect("extra client should initialize");
        (proxy, Arc::new(client))
    }

    #[tokio::test]
    async fn tools_call_for_native_tool_bypasses_proxy_and_returns_local_result() {
        let (router, captured, tool) =
            router_with_tool(br#"{"jsonrpc":"2.0","result":{}}"#.to_vec());

        let response = router
            .handle(
                "brain3.example.com",
                "POST",
                "/mcp",
                None,
                vec![("content-type".into(), "application/json".into())],
                json!({
                    "jsonrpc": "2.0",
                    "id": 7,
                    "method": "tools/call",
                    "params": {
                        "name": "fake_native_tool",
                        "arguments": { "message": "hello" }
                    }
                })
                .to_string()
                .into_bytes(),
            )
            .await
            .expect("native call should succeed");

        assert_eq!(response.status, 200);
        assert!(captured
            .lock()
            .expect("capture lock should succeed")
            .is_empty());
        assert_eq!(
            *tool.calls.lock().expect("calls lock should succeed"),
            vec![json!({ "message": "hello" })]
        );

        let body: Value =
            serde_json::from_slice(&response.body).expect("response body should be JSON");
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 7);
        assert_eq!(body["result"]["isError"], false);
        assert_eq!(body["result"]["content"][0]["type"], "text");
        assert_eq!(body["result"]["content"][0]["text"], "native response");
    }

    #[tokio::test]
    async fn tools_list_forwards_to_proxy_and_appends_native_tool_schemas() {
        let (router, captured, _) = router_with_tool(
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "result": {
                    "tools": [
                        {
                            "name": "container_tool",
                            "description": "Container tool",
                            "inputSchema": { "type": "object" }
                        }
                    ]
                }
            })
            .to_string()
            .into_bytes(),
        );

        let response = router
            .handle(
                "brain3.example.com",
                "POST",
                "/mcp",
                None,
                vec![("content-type".into(), "application/json".into())],
                json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/list"
                })
                .to_string()
                .into_bytes(),
            )
            .await
            .expect("tools/list should succeed");

        assert_eq!(
            captured.lock().expect("capture lock should succeed").len(),
            1
        );

        let body: Value =
            serde_json::from_slice(&response.body).expect("response body should be JSON");
        let tools = body["result"]["tools"]
            .as_array()
            .expect("tools should be an array");
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["name"], "container_tool");
        assert_eq!(tools[1]["name"], "fake_native_tool");
        assert_eq!(tools[1]["description"], "Fake native tool");
        assert_eq!(tools[1]["inputSchema"]["required"][0], "message");
        assert!(
            response
                .headers
                .iter()
                .all(|(name, _)| !name.eq_ignore_ascii_case("content-length")),
            "augmented tools/list responses must not retain the upstream content-length"
        );
    }

    #[tokio::test]
    async fn tools_list_appends_prefixed_extra_container_tool_schemas() {
        let (extra_proxy, extra_client) = initialized_extra_client(
            json!({
                "jsonrpc": "2.0",
                "id": 99,
                "result": {
                    "tools": [
                        {
                            "name": "search_deck",
                            "description": "Search deck",
                            "inputSchema": { "type": "object" }
                        }
                    ]
                }
            })
            .to_string()
            .into_bytes(),
        )
        .await;
        let (router, captured, _, extra_captured) = router_with_tool_and_extra(
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "result": {
                    "tools": [
                        {
                            "name": "container_tool",
                            "description": "Container tool",
                            "inputSchema": { "type": "object" }
                        }
                    ]
                }
            })
            .to_string()
            .into_bytes(),
            extra_proxy,
            extra_client,
        );

        let response = router
            .handle(
                "brain3.example.com",
                "POST",
                "/mcp",
                None,
                vec![("content-type".into(), "application/json".into())],
                json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/list"
                })
                .to_string()
                .into_bytes(),
            )
            .await
            .expect("tools/list should succeed");

        assert_eq!(
            captured.lock().expect("capture lock should succeed").len(),
            1,
            "vault proxy should still receive tools/list"
        );
        assert_eq!(
            extra_captured
                .lock()
                .expect("extra capture lock should succeed")
                .len(),
            2,
            "extra client should only have startup initialize and tools/list calls"
        );

        let body: Value =
            serde_json::from_slice(&response.body).expect("response body should be JSON");
        let tool_names = body["result"]["tools"]
            .as_array()
            .expect("tools should be an array")
            .iter()
            .map(|tool| tool["name"].as_str().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(
            tool_names,
            vec![
                "container_tool",
                "fake_native_tool",
                "fluensy_learn__search_deck"
            ]
        );
    }

    #[tokio::test]
    async fn tools_call_for_prefixed_extra_tool_bypasses_vault_proxy_and_strips_prefix() {
        let (extra_proxy, extra_client) = initialized_extra_client(
            br#"{"jsonrpc":"2.0","id":99,"result":{"tools":[]}}"#.to_vec(),
        )
        .await;
        let (router, captured, _, extra_captured) = router_with_tool_and_extra(
            br#"{"jsonrpc":"2.0","result":{}}"#.to_vec(),
            extra_proxy,
            extra_client,
        );

        let response = router
            .handle(
                "brain3.example.com",
                "POST",
                "/mcp",
                None,
                vec![("content-type".into(), "application/json".into())],
                json!({
                    "jsonrpc": "2.0",
                    "id": 7,
                    "method": "tools/call",
                    "params": {
                        "name": "fluensy_learn__search_deck",
                        "arguments": { "query": "rust" }
                    }
                })
                .to_string()
                .into_bytes(),
            )
            .await
            .expect("extra tools/call should succeed");

        assert_eq!(response.status, 200);
        assert!(
            captured
                .lock()
                .expect("vault capture lock should succeed")
                .is_empty(),
            "vault proxy should not receive prefixed extra tool call"
        );
        let extra_requests = extra_captured
            .lock()
            .expect("extra capture lock should succeed");
        assert_eq!(extra_requests.len(), 3);
        let body: Value =
            serde_json::from_slice(&extra_requests[2].body).expect("request should be JSON");
        assert_eq!(body["params"]["name"], "search_deck");
        assert_eq!(body["params"]["arguments"]["query"], "rust");
    }

    #[tokio::test]
    async fn initialize_forwards_to_proxy_and_initializes_native_tools() {
        let (router, captured, tool) =
            router_with_tool(br#"{"jsonrpc":"2.0","id":1,"result":{}}"#.to_vec());

        let response = router
            .handle(
                "brain3.example.com",
                "POST",
                "/mcp",
                None,
                vec![("content-type".into(), "application/json".into())],
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {}
                })
                .to_string()
                .into_bytes(),
            )
            .await
            .expect("initialize should succeed");

        assert_eq!(response.status, 200);
        assert_eq!(
            captured.lock().expect("capture lock should succeed").len(),
            1
        );
        assert_eq!(
            *tool
                .initialize_count
                .lock()
                .expect("initialize lock should succeed"),
            1
        );
    }
}
