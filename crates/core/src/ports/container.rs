use crate::domain::model::ContainerConfig;

#[derive(Debug, Clone)]
pub struct ContainerId(pub String);

#[async_trait::async_trait]
pub trait ContainerPort: Send + Sync {
    async fn exists(&self, id: &ContainerId) -> Result<bool, Box<dyn std::error::Error + Send + Sync>>;
    async fn is_running(&self, id: &ContainerId) -> Result<bool, Box<dyn std::error::Error + Send + Sync>>;
    async fn create(&self, config: &ContainerConfig) -> Result<ContainerId, Box<dyn std::error::Error + Send + Sync>>;
    async fn start(&self, id: &ContainerId) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    async fn stop(&self, id: &ContainerId) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}
