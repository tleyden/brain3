use crate::domain::errors::ContainerError;
use crate::domain::model::ContainerConfig;

#[derive(Debug, Clone)]
pub struct ContainerId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkPreparation {
    Created,
    Reused,
}

#[async_trait::async_trait]
pub trait ContainerPort: Send + Sync {
    async fn image_exists(&self, image: &str) -> Result<bool, ContainerError>;
    async fn pull_image(&self, image: &str) -> Result<(), ContainerError>;
    async fn exists(&self, id: &ContainerId) -> Result<bool, ContainerError>;
    async fn is_running(&self, id: &ContainerId) -> Result<bool, ContainerError>;
    async fn logs_tail(&self, id: &ContainerId, lines: usize) -> Result<String, ContainerError>;
    async fn ensure_internal_network(
        &self,
        network_name: &str,
    ) -> Result<NetworkPreparation, ContainerError>;
    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<String>, ContainerError>;
    async fn run(&self, config: &ContainerConfig) -> Result<ContainerId, ContainerError>;
    async fn stop(&self, id: &ContainerId) -> Result<(), ContainerError>;
    async fn remove(&self, id: &ContainerId) -> Result<(), ContainerError>;
}
