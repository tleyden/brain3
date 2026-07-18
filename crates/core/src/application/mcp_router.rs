use std::sync::Arc;

use serde_json::{json, Value};

use crate::application::native_mcp_tool_registry::NativeMcpToolRegistry;
use crate::application::proxy_mcp::ProxyMcpUseCase;
use crate::application::remote_mcp_container_client::RemoteMcpContainerClient;
use crate::application::validate_request::validate_host;
use crate::domain::errors::ProxyError;
use crate::domain::model::HostnameValidationConfig;
use crate::ports::mcp_proxy::{McpProxyPort, McpProxyResponse};
use crate::ports::native_mcp_resource::{NativeMcpResourceContent, NativeMcpResourceError};
use crate::ports::native_mcp_tool::{NativeMcpToolError, NativeMcpToolOutput};

pub struct McpRouterUseCase<P: McpProxyPort> {
    proxy: Arc<ProxyMcpUseCase<P>>,
    native_tools: Arc<NativeMcpToolRegistry>,
    plugin_containers: Vec<Arc<RemoteMcpContainerClient<P>>>,
}

impl<P: McpProxyPort> McpRouterUseCase<P> {
    pub fn new(proxy: Arc<ProxyMcpUseCase<P>>, native_tools: Arc<NativeMcpToolRegistry>) -> Self {
        Self::new_with_plugin_containers(proxy, native_tools, Vec::new())
    }

    pub fn new_with_plugin_containers(
        proxy: Arc<ProxyMcpUseCase<P>>,
        native_tools: Arc<NativeMcpToolRegistry>,
        plugin_containers: Vec<Arc<RemoteMcpContainerClient<P>>>,
    ) -> Self {
        Self {
            proxy,
            native_tools,
            plugin_containers,
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
                let response = self
                    .proxy
                    .handle_unvalidated(request_host, method, path, query, headers, body)
                    .await?;
                Ok(self.patch_initialize_resources_capability(response))
            }
            Some("tools/list") => {
                let response = self
                    .proxy
                    .handle_unvalidated(request_host, method, path, query, headers, body)
                    .await?;
                Ok(self.append_tool_schemas(response))
            }
            Some("resources/list") => {
                let response = self
                    .proxy
                    .handle_unvalidated(request_host, method, path, query, headers, body)
                    .await?;
                Ok(self.append_resource_schemas(response))
            }
            Some("tools/call") => {
                if let Some(response) = self.maybe_call_native_tool(parsed_body.as_ref()).await? {
                    return Ok(response);
                }
                if let Some(response) = self.maybe_call_plugin_tool(parsed_body.as_ref()).await? {
                    return Ok(response);
                }

                self.proxy
                    .handle_unvalidated(request_host, method, path, query, headers, body)
                    .await
            }
            Some("resources/read") => {
                if let Some(response) = self
                    .maybe_read_native_resource(parsed_body.as_ref())
                    .await?
                {
                    return Ok(response);
                }
                if let Some(response) = self
                    .maybe_read_plugin_resource(parsed_body.as_ref())
                    .await?
                {
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

    async fn maybe_call_plugin_tool(
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

        for client in &self.plugin_containers {
            if let Some(unprefixed_name) = client.strip_prefix(name) {
                // Verify that the unprefixed tool name was advertised during initialization
                if !client.has_tool(unprefixed_name) {
                    tracing::warn!(
                        container = %client.container_name(),
                        prefixed_tool_name = name,
                        tool_name = unprefixed_name,
                        request_id = ?request.get("id"),
                        "MCP router: rejecting call to unadvertised Plugin MCP tool"
                    );
                    return Ok(Some(tool_not_found_response(request, name)));
                }

                tracing::info!(
                    container = %client.container_name(),
                    prefixed_tool_name = name,
                    tool_name = unprefixed_name,
                    request_id = ?request.get("id"),
                    "MCP router: routing Plugin MCP tool call"
                );

                // Handle transport errors gracefully and convert them to JSON-RPC errors
                match client.call_tool(request, unprefixed_name).await {
                    Ok(response) => return Ok(Some(response)),
                    Err(error) => {
                        tracing::error!(
                            container = %client.container_name(),
                            tool_name = unprefixed_name,
                            request_id = ?request.get("id"),
                            error = %error,
                            "MCP router: Plugin MCP tool call failed"
                        );
                        return Ok(Some(plugin_transport_error_response(request, name, &error)));
                    }
                }
            }
        }

        Ok(None)
    }

    async fn maybe_read_native_resource(
        &self,
        request: Option<&Value>,
    ) -> Result<Option<McpProxyResponse>, ProxyError> {
        let Some(request) = request else {
            return Ok(None);
        };
        let Some(params) = request.get("params").and_then(Value::as_object) else {
            return Ok(None);
        };
        let Some(uri) = params.get("uri").and_then(Value::as_str) else {
            return Ok(None);
        };
        let Some(resource) = self.native_tools.find_resource(uri) else {
            return Ok(None);
        };

        tracing::info!(
            resource_uri = uri,
            request_id = ?request.get("id"),
            "MCP router: handling native resource read"
        );

        match resource.read().await {
            Ok(content) => Ok(Some(native_resource_response(
                request,
                resource.uri(),
                resource.mime_type(),
                content,
            )?)),
            Err(error) => {
                tracing::warn!(
                    resource_uri = uri,
                    request_id = ?request.get("id"),
                    error = %error,
                    "MCP router: native resource read failed"
                );
                Ok(Some(native_resource_error_response(request, uri, &error)))
            }
        }
    }

    async fn maybe_read_plugin_resource(
        &self,
        request: Option<&Value>,
    ) -> Result<Option<McpProxyResponse>, ProxyError> {
        let Some(request) = request else {
            return Ok(None);
        };
        let Some(params) = request.get("params").and_then(Value::as_object) else {
            return Ok(None);
        };
        let Some(uri) = params.get("uri").and_then(Value::as_str) else {
            return Ok(None);
        };

        for client in &self.plugin_containers {
            if let Some(original_uri) = client.strip_resource_uri_prefix(uri) {
                if !client.has_resource(&original_uri) {
                    tracing::warn!(
                        container = %client.container_name(),
                        prefixed_resource_uri = uri,
                        resource_uri = original_uri,
                        request_id = ?request.get("id"),
                        "MCP router: rejecting read of unadvertised Plugin MCP resource"
                    );
                    return Ok(Some(resource_not_found_response(request, uri)));
                }

                tracing::info!(
                    container = %client.container_name(),
                    prefixed_resource_uri = uri,
                    resource_uri = original_uri,
                    request_id = ?request.get("id"),
                    "MCP router: routing Plugin MCP resource read"
                );

                match client.read_resource(request, &original_uri).await {
                    Ok(response) => return Ok(Some(response)),
                    Err(error) => {
                        tracing::error!(
                            container = %client.container_name(),
                            resource_uri = original_uri,
                            request_id = ?request.get("id"),
                            error = %error,
                            "MCP router: Plugin MCP resource read failed"
                        );
                        return Ok(Some(plugin_resource_transport_error_response(
                            request, uri, &error,
                        )));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Logs the full set of MCP tools brain3 is proxying, split into core
    /// (upstream vault-tools container + native tools) and plugin (one line
    /// per Plugin MCP Container) groups. Intended to be called once, right
    /// after startup, so the tool inventory is visible without a manual
    /// `tools/list` call.
    pub async fn log_startup_tool_inventory(&self) {
        let mut core_tool_names = match self.fetch_core_tool_names().await {
            Ok(names) => names,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "startup tool inventory: failed to fetch core vault MCP tools"
                );
                Vec::new()
            }
        };
        core_tool_names.extend(tool_names(&self.native_tools.list_schemas()));

        tracing::info!(
            tool_count = core_tool_names.len(),
            tools = ?core_tool_names,
            "brain3 tool inventory: core tools (vault-tools + native)"
        );

        for client in &self.plugin_containers {
            let plugin_tool_names = tool_names(&client.prefixed_tool_schemas());
            tracing::info!(
                container = client.container_name(),
                tool_count = plugin_tool_names.len(),
                tools = ?plugin_tool_names,
                "brain3 tool inventory: plugin tools"
            );
        }
    }

    async fn fetch_core_tool_names(&self) -> Result<Vec<String>, ProxyError> {
        let initialize_body = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": "brain3-startup-initialize",
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "brain3-gateway-startup",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }
        }))
        .expect("startup initialize request should serialize");
        self.proxy
            .handle_unvalidated(
                "localhost",
                "POST",
                "/mcp",
                None,
                vec![("content-type".into(), "application/json".into())],
                initialize_body,
            )
            .await?;

        let tools_list_body = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": "brain3-startup-tools-list",
            "method": "tools/list",
            "params": {}
        }))
        .expect("startup tools/list request should serialize");
        let response = self
            .proxy
            .handle_unvalidated(
                "localhost",
                "POST",
                "/mcp",
                None,
                vec![("content-type".into(), "application/json".into())],
                tools_list_body,
            )
            .await?;

        let body = serde_json::from_slice::<Value>(&response.body).map_err(|error| {
            ProxyError::BadGateway(format!("invalid core tools/list JSON: {error}"))
        })?;
        let tools = body
            .get("result")
            .and_then(|result| result.get("tools"))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ProxyError::BadGateway("core tools/list response missing result.tools array".into())
            })?;

        Ok(tool_names(tools))
    }

    fn append_tool_schemas(&self, response: McpProxyResponse) -> McpProxyResponse {
        let native_schemas = self.native_tools.list_schemas();
        let plugin_schemas = self
            .plugin_containers
            .iter()
            .flat_map(|client| client.prefixed_tool_schemas())
            .collect::<Vec<_>>();

        if native_schemas.is_empty() && plugin_schemas.is_empty() {
            return response;
        }

        let native_tool_count = native_schemas.len();
        let plugin_tool_count = plugin_schemas.len();
        let mut schemas = native_schemas;
        schemas.extend(plugin_schemas);

        let (response, total_tool_count) =
            append_result_array(response, "tools/list", "tools", schemas);
        let Some(total_tool_count) = total_tool_count else {
            return response;
        };

        tracing::info!(
            native_tool_count = native_tool_count,
            plugin_tool_count = plugin_tool_count,
            total_tool_count = total_tool_count,
            "MCP router: appended native and Plugin MCP tools to tools/list response"
        );

        response
    }

    fn append_resource_schemas(&self, response: McpProxyResponse) -> McpProxyResponse {
        let native_schemas = self.native_tools.list_resource_schemas();
        let plugin_schemas = self
            .plugin_containers
            .iter()
            .flat_map(|client| client.prefixed_resource_schemas())
            .collect::<Vec<_>>();

        if native_schemas.is_empty() && plugin_schemas.is_empty() {
            return response;
        }

        let native_resource_count = native_schemas.len();
        let plugin_resource_count = plugin_schemas.len();
        let mut schemas = native_schemas;
        schemas.extend(plugin_schemas);

        let (response, total_resource_count) =
            append_result_array(response, "resources/list", "resources", schemas);
        let Some(total_resource_count) = total_resource_count else {
            return response;
        };

        tracing::info!(
            native_resource_count = native_resource_count,
            plugin_resource_count = plugin_resource_count,
            total_resource_count = total_resource_count,
            "MCP router: appended native and Plugin MCP resources to resources/list response"
        );

        response
    }

    fn patch_initialize_resources_capability(
        &self,
        response: McpProxyResponse,
    ) -> McpProxyResponse {
        if !self.has_resources() {
            tracing::debug!(
                "MCP router: initialize response resources capability patch skipped; no resources are registered"
            );
            return response;
        }

        let Ok(mut body) = serde_json::from_slice::<Value>(&response.body) else {
            tracing::warn!("MCP router: could not parse initialize response body as JSON");
            return response;
        };
        if body.get("error").is_some() {
            tracing::debug!(
                "MCP router: initialize response resources capability patch skipped; response contains JSON-RPC error"
            );
            return response;
        }

        let Some(result) = body.get_mut("result").and_then(Value::as_object_mut) else {
            tracing::warn!("MCP router: initialize response did not contain result object");
            return response;
        };
        let capabilities = result.entry("capabilities").or_insert_with(|| json!({}));
        let Some(capabilities) = capabilities.as_object_mut() else {
            tracing::warn!("MCP router: initialize response capabilities was not an object");
            return response;
        };

        if capabilities.contains_key("resources") {
            tracing::debug!(
                "MCP router: initialize response already includes resources capability; no patch needed"
            );
            return response;
        }
        capabilities.insert("resources".into(), json!({}));

        let Ok(new_body) = serde_json::to_vec(&body) else {
            tracing::warn!("MCP router: could not serialize patched initialize response");
            return response;
        };

        tracing::info!("MCP router: added resources capability to initialize response");

        McpProxyResponse {
            status: response.status,
            headers: strip_content_length(response.headers),
            body: new_body,
        }
    }

    fn has_resources(&self) -> bool {
        self.native_tools.has_resources()
            || self
                .plugin_containers
                .iter()
                .any(|client| !client.prefixed_resource_schemas().is_empty())
    }
}

fn append_result_array(
    response: McpProxyResponse,
    method_name: &str,
    array_key: &str,
    additions: Vec<Value>,
) -> (McpProxyResponse, Option<usize>) {
    let Ok(mut body) = serde_json::from_slice::<Value>(&response.body) else {
        tracing::warn!(
            method = method_name,
            "MCP router: could not parse response body as JSON"
        );
        return (response, None);
    };

    let Some(values) = body
        .get_mut("result")
        .and_then(|result| result.get_mut(array_key))
        .and_then(Value::as_array_mut)
    else {
        tracing::warn!(
            method = method_name,
            array_key,
            "MCP router: response did not contain expected result array"
        );
        return (response, None);
    };

    values.extend(additions);
    let total_count = values.len();

    let Ok(new_body) = serde_json::to_vec(&body) else {
        tracing::warn!(
            method = method_name,
            "MCP router: could not serialize augmented response"
        );
        return (response, None);
    };

    (
        McpProxyResponse {
            status: response.status,
            headers: strip_content_length(response.headers),
            body: new_body,
        },
        Some(total_count),
    )
}

fn tool_names(schemas: &[Value]) -> Vec<String> {
    schemas
        .iter()
        .filter_map(|schema| schema.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
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

fn native_resource_response(
    request: &Value,
    uri: &str,
    mime_type: &str,
    content: NativeMcpResourceContent,
) -> Result<McpProxyResponse, ProxyError> {
    let mut resource_content = json!({
        "uri": uri,
        "mimeType": mime_type,
    });
    let object = resource_content
        .as_object_mut()
        .expect("native resource content should be a JSON object");
    match content {
        NativeMcpResourceContent::Text(text) => {
            object.insert("text".into(), Value::String(text));
        }
        NativeMcpResourceContent::Blob(blob) => {
            object.insert("blob".into(), Value::String(blob));
        }
    }

    let body = json!({
        "jsonrpc": "2.0",
        "id": request.get("id").cloned().unwrap_or(Value::Null),
        "result": {
            "contents": [resource_content],
        }
    });

    let body = serde_json::to_vec(&body).map_err(|error| {
        ProxyError::BadGateway(format!("native MCP resource response error: {error}"))
    })?;

    Ok(McpProxyResponse {
        status: 200,
        headers: vec![("content-type".into(), "application/json".into())],
        body,
    })
}

fn native_tool_error_to_proxy_error(error: NativeMcpToolError) -> ProxyError {
    ProxyError::BadGateway(format!("native MCP tool initialization failed: {error}"))
}

fn tool_not_found_response(request: &Value, tool_name: &str) -> McpProxyResponse {
    let body = json!({
        "jsonrpc": "2.0",
        "id": request.get("id").cloned().unwrap_or(Value::Null),
        "error": {
            "code": -32601,
            "message": format!("Tool not found: {tool_name}"),
        }
    });

    let body = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());

    McpProxyResponse {
        status: 200,
        headers: vec![("content-type".into(), "application/json".into())],
        body,
    }
}

fn resource_not_found_response(request: &Value, resource_uri: &str) -> McpProxyResponse {
    let body = json!({
        "jsonrpc": "2.0",
        "id": request.get("id").cloned().unwrap_or(Value::Null),
        "error": {
            "code": -32602,
            "message": format!("Resource not found: {resource_uri}"),
        }
    });

    let body = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());

    McpProxyResponse {
        status: 200,
        headers: vec![("content-type".into(), "application/json".into())],
        body,
    }
}

fn native_resource_error_response(
    request: &Value,
    resource_uri: &str,
    error: &NativeMcpResourceError,
) -> McpProxyResponse {
    let body = json!({
        "jsonrpc": "2.0",
        "id": request.get("id").cloned().unwrap_or(Value::Null),
        "error": {
            "code": -32603,
            "message": format!("Native resource error reading {resource_uri}: {error}"),
        }
    });

    let body = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());

    McpProxyResponse {
        status: 200,
        headers: vec![("content-type".into(), "application/json".into())],
        body,
    }
}

fn plugin_transport_error_response(
    request: &Value,
    tool_name: &str,
    error: &ProxyError,
) -> McpProxyResponse {
    let body = json!({
        "jsonrpc": "2.0",
        "id": request.get("id").cloned().unwrap_or(Value::Null),
        "error": {
            "code": -32603,
            "message": format!("Plugin container error calling {tool_name}: {error}"),
        }
    });

    let body = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());

    McpProxyResponse {
        status: 200,
        headers: vec![("content-type".into(), "application/json".into())],
        body,
    }
}

fn plugin_resource_transport_error_response(
    request: &Value,
    resource_uri: &str,
    error: &ProxyError,
) -> McpProxyResponse {
    let body = json!({
        "jsonrpc": "2.0",
        "id": request.get("id").cloned().unwrap_or(Value::Null),
        "error": {
            "code": -32603,
            "message": format!("Plugin container error reading {resource_uri}: {error}"),
        }
    });

    let body = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());

    McpProxyResponse {
        status: 200,
        headers: vec![("content-type".into(), "application/json".into())],
        body,
    }
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
    use crate::ports::native_mcp_resource::{
        NativeMcpResource, NativeMcpResourceContent, NativeMcpResourceError,
    };
    use crate::ports::native_mcp_tool::{NativeMcpTool, NativeMcpToolError, NativeMcpToolOutput};

    use super::*;

    struct CapturingProxy {
        captured: Arc<Mutex<Vec<McpProxyRequest>>>,
        response_bodies: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl CapturingProxy {
        fn new(response_body: Vec<u8>) -> Self {
            Self::new_sequence(vec![response_body])
        }

        fn new_sequence(response_bodies: Vec<Vec<u8>>) -> Self {
            Self {
                captured: Arc::new(Mutex::new(Vec::new())),
                response_bodies: Arc::new(Mutex::new(response_bodies)),
            }
        }
    }

    #[async_trait]
    impl McpProxyPort for CapturingProxy {
        async fn forward(&self, request: McpProxyRequest) -> Result<McpProxyResponse, ProxyError> {
            self.captured
                .lock()
                .expect("capture lock should succeed")
                .push(request);
            let response_body = {
                let mut response_bodies = self
                    .response_bodies
                    .lock()
                    .expect("response bodies lock should succeed");
                if response_bodies.len() > 1 {
                    response_bodies.remove(0)
                } else {
                    response_bodies
                        .first()
                        .expect("test proxy should have a response body")
                        .clone()
                }
            };
            Ok(McpProxyResponse {
                status: 200,
                headers: vec![
                    ("content-type".into(), "application/json".into()),
                    ("content-length".into(), response_body.len().to_string()),
                ],
                body: response_body,
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

    struct FakeNativeResource {
        reads: Arc<Mutex<usize>>,
    }

    impl FakeNativeResource {
        fn new() -> Self {
            Self {
                reads: Arc::new(Mutex::new(0)),
            }
        }
    }

    #[async_trait]
    impl NativeMcpResource for FakeNativeResource {
        fn uri(&self) -> &str {
            "ui://brain3-native/fake/index.html"
        }

        fn name(&self) -> &str {
            "Fake native resource"
        }

        fn mime_type(&self) -> &str {
            "text/html"
        }

        async fn read(&self) -> Result<NativeMcpResourceContent, NativeMcpResourceError> {
            *self.reads.lock().expect("reads lock should succeed") += 1;
            Ok(NativeMcpResourceContent::text("<main>native widget</main>"))
        }
    }

    fn router_with_tool(
        proxy_body: Vec<u8>,
    ) -> (
        McpRouterUseCase<CapturingProxy>,
        Arc<Mutex<Vec<McpProxyRequest>>>,
        Arc<FakeNativeTool>,
    ) {
        let proxy = Arc::new(CapturingProxy::new(proxy_body));
        let proxy_use_case = Arc::new(ProxyMcpUseCase::new(
            Arc::clone(&proxy),
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
            Arc::clone(&proxy.captured),
            tool,
        )
    }

    fn router_with_tool_and_resource(
        proxy_body: Vec<u8>,
    ) -> (
        McpRouterUseCase<CapturingProxy>,
        Arc<Mutex<Vec<McpProxyRequest>>>,
        Arc<FakeNativeResource>,
    ) {
        let proxy = Arc::new(CapturingProxy::new(proxy_body));
        let proxy_use_case = Arc::new(ProxyMcpUseCase::new(
            Arc::clone(&proxy),
            "http://127.0.0.1:8420".into(),
            "shared-secret".into(),
            HostnameValidationConfig {
                expected_host: None,
                enforce: true,
            },
        ));
        let tool = Arc::new(FakeNativeTool::new());
        let resource = Arc::new(FakeNativeResource::new());
        let registry = NativeMcpToolRegistry::new_with_resources(
            vec![tool as Arc<dyn NativeMcpTool>],
            vec![resource.clone() as Arc<dyn NativeMcpResource>],
        );
        (
            McpRouterUseCase::new(proxy_use_case, Arc::new(registry)),
            Arc::clone(&proxy.captured),
            resource,
        )
    }

    fn router_with_tool_resource_and_plugin(
        proxy_body: Vec<u8>,
        plugin_proxy: Arc<CapturingProxy>,
        plugin_client: Arc<RemoteMcpContainerClient<CapturingProxy>>,
    ) -> (
        McpRouterUseCase<CapturingProxy>,
        Arc<Mutex<Vec<McpProxyRequest>>>,
        Arc<FakeNativeResource>,
        Arc<Mutex<Vec<McpProxyRequest>>>,
    ) {
        let proxy = Arc::new(CapturingProxy::new(proxy_body));
        let proxy_use_case = Arc::new(ProxyMcpUseCase::new(
            Arc::clone(&proxy),
            "http://127.0.0.1:8420".into(),
            "shared-secret".into(),
            HostnameValidationConfig {
                expected_host: None,
                enforce: true,
            },
        ));
        let tool = Arc::new(FakeNativeTool::new());
        let resource = Arc::new(FakeNativeResource::new());
        let registry = NativeMcpToolRegistry::new_with_resources(
            vec![tool as Arc<dyn NativeMcpTool>],
            vec![resource.clone() as Arc<dyn NativeMcpResource>],
        );
        let plugin_captured = Arc::clone(&plugin_proxy.captured);
        (
            McpRouterUseCase::new_with_plugin_containers(
                proxy_use_case,
                Arc::new(registry),
                vec![plugin_client],
            ),
            Arc::clone(&proxy.captured),
            resource,
            plugin_captured,
        )
    }

    fn router_with_tool_and_plugin(
        proxy_body: Vec<u8>,
        plugin_proxy: Arc<CapturingProxy>,
        plugin_client: Arc<RemoteMcpContainerClient<CapturingProxy>>,
    ) -> (
        McpRouterUseCase<CapturingProxy>,
        Arc<Mutex<Vec<McpProxyRequest>>>,
        Arc<FakeNativeTool>,
        Arc<Mutex<Vec<McpProxyRequest>>>,
    ) {
        let proxy = Arc::new(CapturingProxy::new(proxy_body));
        let proxy_use_case = Arc::new(ProxyMcpUseCase::new(
            Arc::clone(&proxy),
            "http://127.0.0.1:8420".into(),
            "shared-secret".into(),
            HostnameValidationConfig {
                expected_host: None,
                enforce: true,
            },
        ));
        let tool = Arc::new(FakeNativeTool::new());
        let registry = NativeMcpToolRegistry::new(vec![tool.clone() as Arc<dyn NativeMcpTool>]);
        let plugin_captured = Arc::clone(&plugin_proxy.captured);
        (
            McpRouterUseCase::new_with_plugin_containers(
                proxy_use_case,
                Arc::new(registry),
                vec![plugin_client],
            ),
            Arc::clone(&proxy.captured),
            tool,
            plugin_captured,
        )
    }

    async fn initialized_plugin_client(
        response_body: Vec<u8>,
    ) -> (
        Arc<CapturingProxy>,
        Arc<RemoteMcpContainerClient<CapturingProxy>>,
    ) {
        let proxy = Arc::new(CapturingProxy::new_sequence(vec![
            br#"{"jsonrpc":"2.0","id":1,"result":{}}"#.to_vec(),
            response_body,
            br#"{"jsonrpc":"2.0","id":2,"result":{"resources":[]}}"#.to_vec(),
        ]));
        let client = RemoteMcpContainerClient::initialize_and_cache_tools(
            "fluensy_learn".into(),
            "http://127.0.0.1:18420/mcp".into(),
            None,
            Arc::clone(&proxy),
        )
        .await
        .expect("plugin client should initialize");
        (proxy, Arc::new(client))
    }

    async fn initialized_plugin_client_with_resources(
        resources_body: Vec<u8>,
        tools_body: Vec<u8>,
        read_body: Vec<u8>,
    ) -> (
        Arc<CapturingProxy>,
        Arc<RemoteMcpContainerClient<CapturingProxy>>,
    ) {
        let proxy = Arc::new(CapturingProxy::new_sequence(vec![
            br#"{"jsonrpc":"2.0","id":1,"result":{}}"#.to_vec(),
            tools_body,
            resources_body,
            read_body,
        ]));
        let client = RemoteMcpContainerClient::initialize_and_cache_tools(
            "fluensy_learn".into(),
            "http://127.0.0.1:18420/mcp".into(),
            None,
            Arc::clone(&proxy),
        )
        .await
        .expect("plugin client should initialize");
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
    async fn tools_list_appends_prefixed_plugin_container_tool_schemas() {
        let (plugin_proxy, plugin_client) = initialized_plugin_client(
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
        let (router, captured, _, plugin_captured) = router_with_tool_and_plugin(
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
            plugin_proxy,
            plugin_client,
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
            plugin_captured
                .lock()
                .expect("plugin capture lock should succeed")
                .len(),
            3,
            "plugin client should only have startup initialize, tools/list, and resources/list calls"
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
    async fn tools_call_for_prefixed_plugin_tool_bypasses_vault_proxy_and_strips_prefix() {
        let (plugin_proxy, plugin_client) = initialized_plugin_client(
            br#"{"jsonrpc":"2.0","id":99,"result":{"tools":[{"name":"search_deck","description":"Search deck","inputSchema":{"type":"object"}}]}}"#.to_vec(),
        )
        .await;
        let (router, captured, _, plugin_captured) = router_with_tool_and_plugin(
            br#"{"jsonrpc":"2.0","result":{}}"#.to_vec(),
            plugin_proxy,
            plugin_client,
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
            .expect("plugin tools/call should succeed");

        assert_eq!(response.status, 200);
        assert!(
            captured
                .lock()
                .expect("vault capture lock should succeed")
                .is_empty(),
            "vault proxy should not receive prefixed plugin tool call"
        );
        let plugin_requests = plugin_captured
            .lock()
            .expect("plugin capture lock should succeed");
        assert_eq!(plugin_requests.len(), 4);
        let body: Value =
            serde_json::from_slice(&plugin_requests[3].body).expect("request should be JSON");
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

    #[tokio::test]
    async fn tools_call_rejects_unadvertised_plugin_tool() {
        let (plugin_proxy, plugin_client) = initialized_plugin_client(
            br#"{"jsonrpc":"2.0","id":99,"result":{"tools":[{"name":"search_deck","description":"Search deck","inputSchema":{"type":"object"}}]}}"#.to_vec(),
        )
        .await;
        let (router, _, _, plugin_captured) = router_with_tool_and_plugin(
            br#"{"jsonrpc":"2.0","result":{}}"#.to_vec(),
            plugin_proxy,
            plugin_client,
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
                        "name": "fluensy_learn__unadvertised_tool",
                        "arguments": {}
                    }
                })
                .to_string()
                .into_bytes(),
            )
            .await
            .expect("should return error response");

        assert_eq!(response.status, 200);
        let body: Value = serde_json::from_slice(&response.body).expect("response should be JSON");
        assert!(body.get("error").is_some(), "response should contain error");
        assert_eq!(body["error"]["code"], -32601);

        // Plugin should only have initialization calls, not the tool call
        let plugin_requests = plugin_captured
            .lock()
            .expect("plugin capture lock should succeed");
        assert_eq!(
            plugin_requests.len(),
            3,
            "plugin should only receive startup calls, not the rejected tool call"
        );
    }

    #[tokio::test]
    async fn resources_list_forwards_to_proxy_and_appends_native_and_plugin_resources() {
        let (plugin_proxy, plugin_client) = initialized_plugin_client_with_resources(
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": {
                    "resources": [
                        {
                            "uri": "ui://widget-name/index.html",
                            "name": "Plugin widget",
                            "mimeType": "text/html"
                        }
                    ]
                }
            })
            .to_string()
            .into_bytes(),
            br#"{"jsonrpc":"2.0","id":3,"result":{"tools":[]}}"#.to_vec(),
            br#"{"jsonrpc":"2.0","id":4,"result":{"contents":[]}}"#.to_vec(),
        )
        .await;
        let (router, captured, _, plugin_captured) = router_with_tool_resource_and_plugin(
            json!({
                "jsonrpc": "2.0",
                "id": 5,
                "result": {
                    "resources": [
                        {
                            "uri": "ui://core/resource.html",
                            "name": "Core resource",
                            "mimeType": "text/html"
                        }
                    ]
                }
            })
            .to_string()
            .into_bytes(),
            plugin_proxy,
            plugin_client,
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
                    "id": 5,
                    "method": "resources/list"
                })
                .to_string()
                .into_bytes(),
            )
            .await
            .expect("resources/list should succeed");

        assert_eq!(
            captured.lock().expect("capture lock should succeed").len(),
            1,
            "vault proxy should still receive resources/list"
        );
        assert_eq!(
            plugin_captured
                .lock()
                .expect("plugin capture lock should succeed")
                .len(),
            3,
            "plugin client should only have startup calls"
        );

        let body: Value =
            serde_json::from_slice(&response.body).expect("response body should be JSON");
        let resources = body["result"]["resources"]
            .as_array()
            .expect("resources should be an array");
        assert_eq!(resources.len(), 3);
        assert_eq!(resources[0]["uri"], "ui://core/resource.html");
        assert_eq!(resources[1]["uri"], "ui://brain3-native/fake/index.html");
        assert_eq!(
            resources[2]["uri"],
            "ui://fluensy_learn__widget-name/index.html"
        );
        assert!(
            response
                .headers
                .iter()
                .all(|(name, _)| !name.eq_ignore_ascii_case("content-length")),
            "augmented resources/list responses must not retain the upstream content-length"
        );
    }

    #[tokio::test]
    async fn resources_read_for_native_resource_bypasses_proxy() {
        let (router, captured, resource) =
            router_with_tool_and_resource(br#"{"jsonrpc":"2.0","result":{}}"#.to_vec());

        let response = router
            .handle(
                "brain3.example.com",
                "POST",
                "/mcp",
                None,
                vec![("content-type".into(), "application/json".into())],
                json!({
                    "jsonrpc": "2.0",
                    "id": 8,
                    "method": "resources/read",
                    "params": { "uri": "ui://brain3-native/fake/index.html" }
                })
                .to_string()
                .into_bytes(),
            )
            .await
            .expect("native resource read should succeed");

        assert!(captured
            .lock()
            .expect("capture lock should succeed")
            .is_empty());
        assert_eq!(
            *resource.reads.lock().expect("reads lock should succeed"),
            1
        );

        let body: Value =
            serde_json::from_slice(&response.body).expect("response body should be JSON");
        assert_eq!(body["id"], 8);
        assert_eq!(
            body["result"]["contents"][0]["uri"],
            "ui://brain3-native/fake/index.html"
        );
        assert_eq!(body["result"]["contents"][0]["mimeType"], "text/html");
        assert_eq!(
            body["result"]["contents"][0]["text"],
            "<main>native widget</main>"
        );
    }

    #[tokio::test]
    async fn resources_read_for_prefixed_plugin_resource_routes_to_container_and_strips_prefix() {
        let (plugin_proxy, plugin_client) = initialized_plugin_client_with_resources(
            br#"{"jsonrpc":"2.0","id":2,"result":{"resources":[{"uri":"ui://widget-name/index.html","name":"Plugin widget","mimeType":"text/html"}]}}"#.to_vec(),
            br#"{"jsonrpc":"2.0","id":3,"result":{"tools":[]}}"#.to_vec(),
            br#"{"jsonrpc":"2.0","id":9,"result":{"contents":[{"uri":"ui://widget-name/index.html","mimeType":"text/html","text":"<main>plugin</main>"}]}}"#.to_vec(),
        )
        .await;
        let (router, captured, _, plugin_captured) = router_with_tool_and_plugin(
            br#"{"jsonrpc":"2.0","result":{}}"#.to_vec(),
            plugin_proxy,
            plugin_client,
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
                    "id": 9,
                    "method": "resources/read",
                    "params": { "uri": "ui://fluensy_learn__widget-name/index.html" }
                })
                .to_string()
                .into_bytes(),
            )
            .await
            .expect("plugin resource read should succeed");

        assert!(captured
            .lock()
            .expect("vault capture lock should succeed")
            .is_empty());
        assert_eq!(response.status, 200);
        let plugin_requests = plugin_captured
            .lock()
            .expect("plugin capture lock should succeed");
        assert_eq!(plugin_requests.len(), 4);
        let body: Value =
            serde_json::from_slice(&plugin_requests[3].body).expect("request should be JSON");
        assert_eq!(body["method"], "resources/read");
        assert_eq!(body["params"]["uri"], "ui://widget-name/index.html");
        drop(plugin_requests);

        let response_body: Value =
            serde_json::from_slice(&response.body).expect("response body should be JSON");
        assert_eq!(
            response_body["result"]["contents"][0]["uri"],
            "ui://fluensy_learn__widget-name/index.html"
        );
        assert_eq!(
            response_body["result"]["contents"][0]["text"],
            "<main>plugin</main>"
        );
        assert!(
            response
                .headers
                .iter()
                .all(|(name, _)| !name.eq_ignore_ascii_case("content-length")),
            "rewritten plugin resources/read responses must not retain the upstream content-length"
        );
    }

    #[tokio::test]
    async fn resources_read_falls_through_to_core_proxy_for_unrecognized_uri() {
        let (router, captured, _) =
            router_with_tool_and_resource(br#"{"jsonrpc":"2.0","id":10,"result":{}}"#.to_vec());

        let response = router
            .handle(
                "brain3.example.com",
                "POST",
                "/mcp",
                None,
                vec![("content-type".into(), "application/json".into())],
                json!({
                    "jsonrpc": "2.0",
                    "id": 10,
                    "method": "resources/read",
                    "params": { "uri": "ui://unknown/resource.html" }
                })
                .to_string()
                .into_bytes(),
            )
            .await
            .expect("unrecognized resource should fall through");

        assert_eq!(response.status, 200);
        assert_eq!(
            captured.lock().expect("capture lock should succeed").len(),
            1
        );
    }

    #[tokio::test]
    async fn initialize_response_gains_resources_capability_when_resources_exist() {
        let (router, captured, _) = router_with_tool_and_resource(
            json!({
                "jsonrpc": "2.0",
                "id": 11,
                "result": {
                    "capabilities": {
                        "tools": {}
                    }
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
                    "id": 11,
                    "method": "initialize",
                    "params": {}
                })
                .to_string()
                .into_bytes(),
            )
            .await
            .expect("initialize should succeed");

        assert_eq!(
            captured.lock().expect("capture lock should succeed").len(),
            1
        );
        let body: Value =
            serde_json::from_slice(&response.body).expect("response body should be JSON");
        assert_eq!(body["result"]["capabilities"]["tools"], json!({}));
        assert_eq!(body["result"]["capabilities"]["resources"], json!({}));
        assert!(
            response
                .headers
                .iter()
                .all(|(name, _)| !name.eq_ignore_ascii_case("content-length")),
            "patched initialize response must not retain upstream content-length"
        );
    }
}
