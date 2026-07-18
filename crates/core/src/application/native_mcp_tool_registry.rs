use std::sync::Arc;

use serde_json::{json, Value};

use crate::ports::native_mcp_resource::NativeMcpResource;
use crate::ports::native_mcp_tool::{NativeMcpTool, NativeMcpToolError};

pub struct NativeMcpToolRegistry {
    tools: Vec<Arc<dyn NativeMcpTool>>,
    resources: Vec<Arc<dyn NativeMcpResource>>,
}

impl NativeMcpToolRegistry {
    pub fn new(tools: Vec<Arc<dyn NativeMcpTool>>) -> Self {
        Self {
            tools,
            resources: Vec::new(),
        }
    }

    pub fn new_with_resources(
        tools: Vec<Arc<dyn NativeMcpTool>>,
        resources: Vec<Arc<dyn NativeMcpResource>>,
    ) -> Self {
        Self { tools, resources }
    }

    pub fn find(&self, name: &str) -> Option<Arc<dyn NativeMcpTool>> {
        self.tools.iter().find(|tool| tool.name() == name).cloned()
    }

    pub fn find_resource(&self, uri: &str) -> Option<Arc<dyn NativeMcpResource>> {
        self.resources
            .iter()
            .find(|resource| resource.uri() == uri)
            .cloned()
    }

    pub fn list_schemas(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|tool| {
                let mut schema = json!({
                    "name": tool.name(),
                    "description": tool.description(),
                    "inputSchema": tool.input_schema(),
                });
                if let Some(meta) = tool.meta() {
                    schema
                        .as_object_mut()
                        .expect("native tool schema should be a JSON object")
                        .insert("_meta".into(), meta);
                }
                schema
            })
            .collect()
    }

    pub fn list_resource_schemas(&self) -> Vec<Value> {
        self.resources
            .iter()
            .map(|resource| {
                json!({
                    "uri": resource.uri(),
                    "name": resource.name(),
                    "mimeType": resource.mime_type(),
                })
            })
            .collect()
    }

    pub fn has_resources(&self) -> bool {
        !self.resources.is_empty()
    }

    pub async fn initialize_all(&self) -> Result<(), NativeMcpToolError> {
        for tool in &self.tools {
            tool.on_initialize().await?;
        }

        Ok(())
    }
}
