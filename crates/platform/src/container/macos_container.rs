use brain3_core::domain::errors::ContainerError;
use brain3_core::domain::model::{ContainerConfig, ContainerNetworkIsolationStrategy};
use brain3_core::ports::container::{ContainerId, ContainerPort, NetworkPreparation};

use super::process::{command_succeeds, run_command};

pub struct MacOsContainerAdapter;

enum InternalNetworkState {
    Missing,
    Compatible,
    Incompatible,
}

async fn inspect_internal_network_state(name: &str) -> Result<InternalNetworkState, ContainerError> {
    match run_command("container", &["network", "inspect", name]).await {
        Ok(out) => {
            let normalized: String = out
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<String>()
                .to_ascii_lowercase();
            if normalized.contains("\"internal\":true")
                || normalized.contains("\"isinternal\":true")
            {
                Ok(InternalNetworkState::Compatible)
            } else {
                Ok(InternalNetworkState::Incompatible)
            }
        }
        Err(ContainerError::CommandFailed { .. }) => Ok(InternalNetworkState::Missing),
        Err(e) => Err(e),
    }
}

async fn create_internal_network(name: &str) -> Result<(), ContainerError> {
    tracing::info!(network = name, "creating fresh internal MCP network");
    run_command("container", &["network", "create", "--internal", name]).await?;
    Ok(())
}

#[async_trait::async_trait]
impl ContainerPort for MacOsContainerAdapter {
    async fn image_exists(&self, image: &str) -> Result<bool, ContainerError> {
        command_succeeds("container", &["image", "inspect", image]).await
    }

    async fn pull_image(&self, image: &str) -> Result<(), ContainerError> {
        run_command("container", &["image", "pull", image])
            .await
            .map(|_| ())
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

    async fn logs_tail(&self, id: &ContainerId, lines: usize) -> Result<String, ContainerError> {
        let lines = lines.to_string();
        run_command("container", &["logs", "-n", &lines, &id.0]).await
    }

    async fn ensure_internal_network(
        &self,
        network_name: &str,
    ) -> Result<NetworkPreparation, ContainerError> {
        match inspect_internal_network_state(network_name).await? {
            InternalNetworkState::Missing => {
                create_internal_network(network_name).await?;
                Ok(NetworkPreparation::Created)
            }
            InternalNetworkState::Compatible => Ok(NetworkPreparation::Reused),
            InternalNetworkState::Incompatible => Err(ContainerError::Conflict(format!(
                "container network name '{}' already exists and is not a compatible internal Brain3 network; choose a different container network name",
                network_name
            ))),
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
        // DiscoverContainerIp: skip --publish; reach container via its internal IP.
        // All other cases (None or PublishToLoopback): bind host loopback port.
        if !matches!(
            config.isolation_strategy,
            Some(ContainerNetworkIsolationStrategy::DiscoverContainerIp)
        ) {
            for pm in &config.port_mappings {
                args.push("--publish".into());
                args.push(format!(
                    "{}:{}:{}",
                    pm.host_address, pm.host_port, pm.container_port
                ));
            }
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
        if let Some(ref wd) = config.workdir {
            args.push("--workdir".into());
            args.push(wd.clone());
        }
        if config.isolation_strategy.is_some() {
            args.push("--network".into());
            args.push(config.network_name.clone());
        }
        args.push(config.image.clone());
        for c in &config.command {
            args.push(c.clone());
        }

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
                if stderr.contains("notFound") =>
            {
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<String>, ContainerError> {
        match run_command("container", &["inspect", &id.0]).await {
            Ok(out) => {
                // Parse the first IP address from the JSON output.
                // Look for `"IPAddress": "x.x.x.x"` pattern.
                let ip = out.lines().find_map(|line| {
                    let trimmed = line.trim();
                    if trimmed.starts_with("\"IPAddress\"") || trimmed.starts_with("\"ipAddress\"")
                    {
                        trimmed
                            .split(':')
                            .nth(1)
                            .map(|s| s.trim().trim_matches('"').trim_matches(',').to_string())
                            .filter(|s| !s.is_empty() && s != "null")
                    } else {
                        None
                    }
                });
                Ok(ip)
            }
            Err(ContainerError::CommandFailed { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
