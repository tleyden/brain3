use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeMcpResourceContent {
    Text(String),
    Blob(String),
}

impl NativeMcpResourceContent {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    pub fn blob(blob: impl Into<String>) -> Self {
        Self::Blob(blob.into())
    }
}

#[derive(Debug, Error)]
pub enum NativeMcpResourceError {
    #[error("resource read failed: {0}")]
    ReadFailed(String),
}

#[async_trait::async_trait]
pub trait NativeMcpResource: Send + Sync {
    fn uri(&self) -> &str;
    fn name(&self) -> &str;
    fn mime_type(&self) -> &str;

    async fn read(&self) -> Result<NativeMcpResourceContent, NativeMcpResourceError>;
}
