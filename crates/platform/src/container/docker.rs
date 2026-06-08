use brain3_core::domain::errors::ContainerError;
use brain3_core::domain::model::ContainerConfig;
use brain3_core::ports::container::{ContainerId, ContainerPort};

use super::process::{command_succeeds, run_command};

pub struct DockerContainerAdapter;

#[async_trait::async_trait]
impl ContainerPort for DockerContainerAdapter {
    async fn image_exists(&self, image: &str) -> Result<bool, ContainerError> {
        command_succeeds("docker", &["image", "inspect", image]).await
    }

    async fn exists(&self, id: &ContainerId) -> Result<bool, ContainerError> {
        command_succeeds("docker", &["inspect", &id.0]).await
    }

    async fn is_running(&self, id: &ContainerId) -> Result<bool, ContainerError> {
        match run_command("docker", &["inspect", "--format", "{{.State.Running}}", &id.0]).await {
            Ok(out) => Ok(out.trim() == "true"),
            Err(ContainerError::CommandFailed { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn run(&self, config: &ContainerConfig) -> Result<ContainerId, ContainerError> {
        let mut args: Vec<String> = vec!["run".into(), "--name".into(), config.name.clone()];

        if config.detach {
            args.push("--detach".into());
        }
        if config.remove_on_exit {
            args.push("--rm".into());
        }
        if let Some(ref user) = config.user {
            args.push("--user".into());
            args.push(user.clone());
        }
        for pm in &config.port_mappings {
            args.push("--publish".into());
            args.push(format!("{}:{}:{}", pm.host_address, pm.host_port, pm.container_port));
        }
        for (k, v) in &config.env_vars {
            args.push("--env".into());
            args.push(format!("{k}={v}"));
        }
        for bm in &config.bind_mounts {
            let mut spec = format!(
                "type=bind,source={},target={}",
                bm.host_path.display(),
                bm.container_path.display()
            );
            if bm.readonly {
                spec.push_str(",readonly");
            }
            args.push("--mount".into());
            args.push(spec);
        }
        args.push(config.image.clone());

        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        run_command("docker", &refs).await?;
        Ok(ContainerId(config.name.clone()))
    }

    async fn stop(&self, id: &ContainerId) -> Result<(), ContainerError> {
        run_command("docker", &["stop", &id.0]).await.map(|_| ())
    }

    async fn remove(&self, id: &ContainerId) -> Result<(), ContainerError> {
        run_command("docker", &["rm", &id.0]).await.map(|_| ())
    }
}
