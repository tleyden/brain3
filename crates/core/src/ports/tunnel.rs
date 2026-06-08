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
    async fn start(&self) -> Result<TunnelInfo, Box<dyn std::error::Error + Send + Sync>>;
    async fn stop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    async fn status(&self) -> Result<TunnelStatus, Box<dyn std::error::Error + Send + Sync>>;
}
