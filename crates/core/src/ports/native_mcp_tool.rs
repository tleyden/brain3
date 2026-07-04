use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeMcpToolOutput {
    pub content: Vec<NativeMcpToolTextContent>,
    pub is_error: bool,
}

impl NativeMcpToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![NativeMcpToolTextContent { text: text.into() }],
            is_error: false,
        }
    }

    pub fn error_text(text: impl Into<String>) -> Self {
        Self {
            content: vec![NativeMcpToolTextContent { text: text.into() }],
            is_error: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeMcpToolTextContent {
    pub text: String,
}

#[derive(Debug, Error)]
pub enum NativeMcpToolError {
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("tool execution failed: {0}")]
    ExecutionFailed(String),
}

#[async_trait::async_trait]
pub trait NativeMcpTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;
    fn meta(&self) -> Option<Value> {
        None
    }

    async fn call(&self, arguments: Value) -> Result<NativeMcpToolOutput, NativeMcpToolError>;

    async fn on_initialize(&self) -> Result<(), NativeMcpToolError> {
        Ok(())
    }
}
