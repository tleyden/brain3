use std::sync::Arc;

use crate::domain::errors::ContainerError;
use crate::domain::model::ContainerConfig;
use crate::ports::container::{ContainerId, ContainerPort};

pub struct EnsureContainerUseCase {
    port: Arc<dyn ContainerPort>,
}

impl EnsureContainerUseCase {
    pub fn new(port: Arc<dyn ContainerPort>) -> Self {
        Self { port }
    }

    pub async fn ensure(&self, config: &ContainerConfig) -> Result<ContainerId, ContainerError> {
        if !self.port.image_exists(&config.image).await? {
            return Err(ContainerError::ImageNotFound(config.image.clone()));
        }

        let id = ContainerId(config.name.clone());

        if self.port.exists(&id).await? {
            if self.port.is_running(&id).await? {
                tracing::info!(container = %config.name, "container already running");
                return Ok(id);
            }
            tracing::info!(container = %config.name, "removing stopped container before restarting");
            self.port.remove(&id).await?;
        }

        tracing::info!(container = %config.name, image = %config.image, "starting container");
        let id = self.port.run(config).await?;
        tracing::info!(container = %config.name, "container started");
        Ok(id)
    }
}
