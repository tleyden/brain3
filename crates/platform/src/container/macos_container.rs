use brain3_core::domain::errors::ContainerError;
use brain3_core::domain::model::ContainerConfig;
use brain3_core::ports::container::{ContainerId, ContainerPort};

use super::process::{command_succeeds, run_command};

pub struct MacOsContainerAdapter;

#[async_trait::async_trait]
impl ContainerPort for MacOsContainerAdapter {
    async fn image_exists(&self, image: &str) -> Result<bool, ContainerError> {
        command_succeeds("container", &["image", "inspect", image]).await
    }

    async fn exists(&self, id: &ContainerId) -> Result<bool, ContainerError> {
        command_succeeds("container", &["inspect", &id.0]).await
    }

    async fn is_running(&self, id: &ContainerId) -> Result<bool, ContainerError> {
        // `container inspect` outputs JSON; status field is "running" when active.
        match run_command("container", &["inspect", &id.0]).await {
            Ok(out) => Ok(out.contains("\"running\"")),
            Err(ContainerError::CommandFailed { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn run(&self, config: &ContainerConfig) -> Result<ContainerId, ContainerError> {
        let mut args: Vec<String> = vec!["run".into(), "--name".into(), config.name.clone()];

        if config.detach {
            args.push("--detach".into());
        }
        // macOS `container` has no --rm; caller is expected to remove explicitly if needed.
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
        run_command("container", &refs).await?;
        Ok(ContainerId(config.name.clone()))
    }

    async fn stop(&self, id: &ContainerId) -> Result<(), ContainerError> {
        run_command("container", &["stop", &id.0]).await.map(|_| ())
    }

    async fn remove(&self, id: &ContainerId) -> Result<(), ContainerError> {
        // macOS container CLI uses `delete`, not `rm`.
        // Treat notFound as success — goal is "container does not exist".
        match run_command("container", &["delete", &id.0]).await {
            Ok(_) => Ok(()),
            Err(ContainerError::CommandFailed { ref stderr, .. })
                if stderr.contains("notFound") => Ok(()),
            Err(e) => Err(e),
        }
    }
}
