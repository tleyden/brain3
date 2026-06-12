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
            tracing::warn!(
                image = %config.image,
                "container image not found locally; will pull from registry"
            );
            self.port.pull_image(&config.image).await?;

            if !self.port.image_exists(&config.image).await? {
                return Err(ContainerError::ImageNotFound(config.image.clone()));
            }
        }

        let id = ContainerId(config.name.clone());

        if self.port.exists(&id).await? {
            if self.port.is_running(&id).await? {
                tracing::info!(container = %config.name, "stopping running container to pick up fresh shared secret");
                self.port.stop(&id).await?;
            }
            tracing::info!(container = %config.name, "removing container before fresh start");
            self.port.remove(&id).await?;
        }

        tracing::info!(container = %config.name, image = %config.image, "starting container");
        let id = self.port.run(config).await?;
        tracing::info!(container = %config.name, "container started");
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Debug, Clone, Default)]
    struct MockState {
        image_exists: bool,
        container_exists: bool,
        container_running: bool,
        pull_count: usize,
        stop_count: usize,
        remove_count: usize,
        run_count: usize,
        actions: Vec<&'static str>,
    }

    struct MockContainerPort {
        state: Mutex<MockState>,
    }

    impl MockContainerPort {
        fn new(state: MockState) -> Self {
            Self {
                state: Mutex::new(state),
            }
        }

        fn snapshot(&self) -> MockState {
            self.state.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl ContainerPort for MockContainerPort {
        async fn image_exists(&self, _image: &str) -> Result<bool, ContainerError> {
            let mut state = self.state.lock().unwrap();
            state.actions.push("image_exists");
            Ok(state.image_exists)
        }

        async fn pull_image(&self, _image: &str) -> Result<(), ContainerError> {
            let mut state = self.state.lock().unwrap();
            state.actions.push("pull_image");
            state.pull_count += 1;
            state.image_exists = true;
            Ok(())
        }

        async fn exists(&self, _id: &ContainerId) -> Result<bool, ContainerError> {
            let mut state = self.state.lock().unwrap();
            state.actions.push("exists");
            Ok(state.container_exists)
        }

        async fn is_running(&self, _id: &ContainerId) -> Result<bool, ContainerError> {
            let mut state = self.state.lock().unwrap();
            state.actions.push("is_running");
            Ok(state.container_running)
        }

        async fn run(&self, config: &ContainerConfig) -> Result<ContainerId, ContainerError> {
            let mut state = self.state.lock().unwrap();
            state.actions.push("run");
            state.run_count += 1;
            state.container_exists = true;
            state.container_running = true;
            Ok(ContainerId(config.name.clone()))
        }

        async fn stop(&self, _id: &ContainerId) -> Result<(), ContainerError> {
            let mut state = self.state.lock().unwrap();
            state.actions.push("stop");
            state.stop_count += 1;
            state.container_running = false;
            Ok(())
        }

        async fn remove(&self, _id: &ContainerId) -> Result<(), ContainerError> {
            let mut state = self.state.lock().unwrap();
            state.actions.push("remove");
            state.remove_count += 1;
            state.container_exists = false;
            Ok(())
        }
    }

    fn sample_config() -> ContainerConfig {
        ContainerConfig {
            image: "ghcr.io/tleyden/brain3-mcp-vault-tools:latest".into(),
            name: "brain3-mcp-vault-tools".into(),
            port_mappings: vec![],
            env_vars: vec![],
            bind_mounts: vec![],
            user: None,
            detach: true,
            remove_on_exit: false,
            workdir: None,
            command: vec![],
        }
    }

    #[tokio::test]
    async fn pulls_missing_image_before_running_container() {
        let port = Arc::new(MockContainerPort::new(MockState::default()));
        let use_case = EnsureContainerUseCase::new(port.clone());
        let config = sample_config();

        let id = use_case.ensure(&config).await.unwrap();

        assert_eq!(id.0, config.name);

        let state = port.snapshot();
        assert_eq!(state.pull_count, 1);
        assert_eq!(state.run_count, 1);
        assert_eq!(state.stop_count, 0);
        assert_eq!(state.remove_count, 0);
        assert_eq!(
            state.actions,
            vec![
                "image_exists",
                "pull_image",
                "image_exists",
                "exists",
                "run"
            ]
        );
    }

    #[tokio::test]
    async fn restarts_existing_running_container_without_repulling_existing_image() {
        let port = Arc::new(MockContainerPort::new(MockState {
            image_exists: true,
            container_exists: true,
            container_running: true,
            ..Default::default()
        }));
        let use_case = EnsureContainerUseCase::new(port.clone());
        let config = sample_config();

        let id = use_case.ensure(&config).await.unwrap();

        assert_eq!(id.0, config.name);

        let state = port.snapshot();
        assert_eq!(state.pull_count, 0);
        assert_eq!(state.run_count, 1);
        assert_eq!(state.stop_count, 1);
        assert_eq!(state.remove_count, 1);
        assert_eq!(
            state.actions,
            vec![
                "image_exists",
                "exists",
                "is_running",
                "stop",
                "remove",
                "run"
            ]
        );
    }
}
