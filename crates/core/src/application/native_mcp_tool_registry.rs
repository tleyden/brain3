use std::sync::Arc;

use serde_json::{json, Value};

use crate::ports::native_mcp_tool::{NativeMcpTool, NativeMcpToolError};

pub struct NativeMcpToolRegistry {
    tools: Vec<Arc<dyn NativeMcpTool>>,
}

impl NativeMcpToolRegistry {
    pub fn new(tools: Vec<Arc<dyn NativeMcpTool>>) -> Self {
        Self { tools }
    }

    pub fn find(&self, name: &str) -> Option<Arc<dyn NativeMcpTool>> {
        self.tools.iter().find(|tool| tool.name() == name).cloned()
    }

    pub fn list_schemas(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name(),
                    "description": tool.description(),
                    "inputSchema": tool.input_schema(),
                })
            })
            .collect()
    }

    pub async fn initialize_all(&self) -> Result<(), NativeMcpToolError> {
        for tool in &self.tools {
            tool.on_initialize().await?;
        }

        Ok(())
    }
}
