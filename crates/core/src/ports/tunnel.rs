use crate::domain::errors::TunnelError;

#[derive(Debug, Clone)]
pub struct TunnelInfo {
    pub public_url: String,
}

#[derive(Debug, Clone)]
pub enum TunnelStatus {
    Running(TunnelInfo),
    Stopped,
}

#[async_trait::async_trait]
pub trait TunnelPort: Send + Sync {
    async fn start(&self) -> Result<TunnelInfo, TunnelError>;
    async fn stop(&self) -> Result<(), TunnelError>;
    async fn status(&self) -> Result<TunnelStatus, TunnelError>;
}
