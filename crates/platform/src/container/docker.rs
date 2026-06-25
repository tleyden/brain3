use brain3_core::domain::errors::ContainerError;
use brain3_core::domain::model::{
    ContainerConfig, ContainerLabel, ContainerNetworkIsolationStrategy, ManagedContainerInfo,
    ManagedContainerScope, BRAIN3_INSTALLATION_ID_LABEL_KEY, BRAIN3_MANAGED_LABEL_KEY,
    BRAIN3_MANAGED_LABEL_VALUE, BRAIN3_ROLE_LABEL_KEY,
};
use brain3_core::ports::container::{ContainerId, ContainerPort, NetworkPreparation};
use serde_json::Value;

use super::process::{command_succeeds, run_command};

pub struct DockerContainerAdapter;

enum InternalNetworkState {
    Missing,
    Compatible,
    Incompatible,
}

async fn inspect_internal_network_state(
    name: &str,
) -> Result<InternalNetworkState, ContainerError> {
    match run_command(
        "docker",
        &["network", "inspect", "--format", "{{.Internal}}", name],
    )
    .await
    {
        Ok(out) => {
            if out.trim() == "true" {
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
    run_command("docker", &["network", "create", "--internal", name]).await?;
    Ok(())
}

fn docker_label_filters(scope: &ManagedContainerScope) -> Vec<String> {
    vec![
        format!("{BRAIN3_MANAGED_LABEL_KEY}={BRAIN3_MANAGED_LABEL_VALUE}"),
        format!("{BRAIN3_ROLE_LABEL_KEY}={}", scope.role),
        format!(
            "{BRAIN3_INSTALLATION_ID_LABEL_KEY}={}",
            scope.installation_id
        ),
    ]
}

fn parse_docker_container_refs(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_docker_inspect_output(output: &str) -> Result<Vec<ManagedContainerInfo>, ContainerError> {
    let value: Value = serde_json::from_str(output).map_err(|error| {
        ContainerError::Other(format!("failed to parse docker inspect output: {error}"))
    })?;
    let entries = value.as_array().ok_or_else(|| {
        ContainerError::Other("docker inspect output was not a JSON array".into())
    })?;

    let mut containers = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = entry
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim_start_matches('/')
            .to_string();
        let running = entry
            .get("State")
            .and_then(|state| state.get("Running"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let state = entry
            .get("State")
            .and_then(|state| state.get("Status"))
            .and_then(Value::as_str)
            .unwrap_or(if running { "running" } else { "unknown" })
            .to_string();
        let mut labels = entry
            .get("Config")
            .and_then(|config| config.get("Labels"))
            .and_then(Value::as_object)
            .map(|labels| {
                labels
                    .iter()
                    .map(|(key, value)| ContainerLabel {
                        key: key.clone(),
                        value: value.as_str().unwrap_or_default().to_string(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        labels.sort_by(|left, right| left.key.cmp(&right.key));

        containers.push(ManagedContainerInfo {
            name,
            running,
            state,
            labels,
        });
    }

    Ok(containers)
}

#[async_trait::async_trait]
impl ContainerPort for DockerContainerAdapter {
    async fn image_exists(&self, image: &str) -> Result<bool, ContainerError> {
        command_succeeds("docker", &["image", "inspect", image]).await
    }

    async fn pull_image(&self, image: &str) -> Result<(), ContainerError> {
        run_command("docker", &["pull", image]).await.map(|_| ())
    }

    async fn exists(&self, id: &ContainerId) -> Result<bool, ContainerError> {
        command_succeeds("docker", &["container", "inspect", &id.0]).await
    }

    async fn is_running(&self, id: &ContainerId) -> Result<bool, ContainerError> {
        match run_command(
            "docker",
            &[
                "container",
                "inspect",
                "--format",
                "{{.State.Running}}",
                &id.0,
            ],
        )
        .await
        {
            Ok(out) => Ok(out.trim() == "true"),
            Err(ContainerError::CommandFailed { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn logs_tail(&self, id: &ContainerId, lines: usize) -> Result<String, ContainerError> {
        let lines = lines.to_string();
        run_command("docker", &["logs", "--tail", &lines, &id.0]).await
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

    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<String>, ContainerError> {
        match run_command(
            "docker",
            &[
                "inspect",
                "--format",
                "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}",
                &id.0,
            ],
        )
        .await
        {
            Ok(out) => {
                let ip = out.trim().to_string();
                Ok(if ip.is_empty() { None } else { Some(ip) })
            }
            Err(ContainerError::CommandFailed { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn list_managed_containers(
        &self,
        scope: &ManagedContainerScope,
    ) -> Result<Vec<ManagedContainerInfo>, ContainerError> {
        let label_filters = docker_label_filters(scope);
        let mut args = vec!["ps".to_string(), "-a".to_string()];
        for filter in &label_filters {
            args.push("--filter".into());
            args.push(format!("label={filter}"));
        }
        args.push("--format".into());
        args.push("{{.ID}}".into());
        let refs: Vec<&str> = args.iter().map(|arg| arg.as_str()).collect();
        let ids = parse_docker_container_refs(&run_command("docker", &refs).await?);
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut inspect_args = vec!["inspect".to_string()];
        inspect_args.extend(ids);
        let inspect_refs: Vec<&str> = inspect_args.iter().map(|arg| arg.as_str()).collect();
        parse_docker_inspect_output(&run_command("docker", &inspect_refs).await?)
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
        for (key, value) in &config.env_vars {
            args.push("--env".into());
            args.push(format!("{key}={value}"));
        }
        for label in &config.labels {
            args.push("--label".into());
            args.push(format!("{}={}", label.key, label.value));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_docker_inspect_output_reads_state_and_labels() {
        let output = r#"
[
  {
    "Name": "/brain3-mcp-vault-tools",
    "State": {
      "Running": true,
      "Status": "running"
    },
    "Config": {
      "Labels": {
        "io.brain3.managed": "true",
        "io.brain3.role": "mcp",
        "io.brain3.installation_id": "abc123"
      }
    }
  }
]
"#;

        let containers = parse_docker_inspect_output(output).expect("inspect should parse");

        assert_eq!(
            containers,
            vec![ManagedContainerInfo {
                name: "brain3-mcp-vault-tools".into(),
                running: true,
                state: "running".into(),
                labels: vec![
                    ContainerLabel {
                        key: "io.brain3.installation_id".into(),
                        value: "abc123".into(),
                    },
                    ContainerLabel {
                        key: "io.brain3.managed".into(),
                        value: "true".into(),
                    },
                    ContainerLabel {
                        key: "io.brain3.role".into(),
                        value: "mcp".into(),
                    },
                ],
            }]
        );
    }

    #[test]
    fn parse_docker_container_refs_skips_blank_lines() {
        assert_eq!(
            parse_docker_container_refs("abc\n\nxyz\n"),
            vec!["abc".to_string(), "xyz".to_string()]
        );
    }
}
