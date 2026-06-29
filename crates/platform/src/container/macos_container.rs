use brain3_core::domain::errors::ContainerError;
use brain3_core::domain::model::{
    ContainerConfig, ContainerLabel, ContainerNetworkIsolationStrategy, ManagedContainerInfo,
    ManagedContainerScope, BRAIN3_INSTALLATION_ID_LABEL_KEY, BRAIN3_MANAGED_LABEL_KEY,
    BRAIN3_MANAGED_LABEL_VALUE, BRAIN3_ROLE_LABEL_KEY,
};
use brain3_core::ports::container::{ContainerId, ContainerPort, NetworkPreparation};
use serde_json::Value;

use super::process::{command_succeeds, run_command};

pub struct MacOsContainerAdapter;

enum InternalNetworkState {
    Missing,
    Compatible,
    Incompatible,
}

async fn inspect_internal_network_state(
    name: &str,
) -> Result<InternalNetworkState, ContainerError> {
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

fn macos_managed_labels_match(labels: &[ContainerLabel], scope: &ManagedContainerScope) -> bool {
    let managed = labels.iter().any(|label| {
        label.key == BRAIN3_MANAGED_LABEL_KEY && label.value == BRAIN3_MANAGED_LABEL_VALUE
    });
    let role = labels
        .iter()
        .any(|label| label.key == BRAIN3_ROLE_LABEL_KEY && label.value == scope.role);
    let installation = labels.iter().any(|label| {
        label.key == BRAIN3_INSTALLATION_ID_LABEL_KEY && label.value == scope.installation_id
    });

    managed && role && installation
}

fn parse_macos_container_refs(output: &str) -> Vec<String> {
    if let Ok(value) = serde_json::from_str::<Value>(output) {
        if let Some(entries) = value.as_array() {
            let refs = entries
                .iter()
                .filter_map(|entry| {
                    entry
                        .get("id")
                        .or_else(|| entry.get("ID"))
                        .or_else(|| entry.get("name"))
                        .or_else(|| entry.get("Name"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .collect::<Vec<_>>();
            if !refs.is_empty() {
                return refs;
            }
        }
    }

    output
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_labels_from_value(value: &Value) -> Vec<ContainerLabel> {
    for key in ["labels", "Labels", "annotations", "Annotations"] {
        if let Some(labels) = value.get(key).and_then(Value::as_object) {
            let mut labels = labels
                .iter()
                .map(|(label_key, label_value)| ContainerLabel {
                    key: label_key.clone(),
                    value: label_value.as_str().unwrap_or_default().to_string(),
                })
                .collect::<Vec<_>>();
            labels.sort_by(|left, right| left.key.cmp(&right.key));
            return labels;
        }
    }

    Vec::new()
}

fn parse_macos_inspect_output(output: &str) -> Result<Vec<ManagedContainerInfo>, ContainerError> {
    let value: Value = serde_json::from_str(output).map_err(|error| {
        ContainerError::Other(format!(
            "failed to parse macOS container inspect output: {error}"
        ))
    })?;
    let entries = value.as_array().ok_or_else(|| {
        ContainerError::Other("macOS container inspect output was not a JSON array".into())
    })?;

    let mut containers = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = entry
            .get("name")
            .or_else(|| entry.get("Name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let status = entry
            .get("status")
            .or_else(|| entry.get("Status"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let running = status.eq_ignore_ascii_case("running");
        let labels = parse_labels_from_value(entry);

        containers.push(ManagedContainerInfo {
            name,
            running,
            state: status,
            labels,
        });
    }

    Ok(containers)
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
        match run_command("container", &["inspect", &id.0]).await {
            Ok(out) => {
                // Do not simplify this back to a plain exit-code check.
                //
                // Apple's `container inspect <name>` currently differs from Docker:
                // for a missing container it can exit 0 and print `[]` instead of
                // failing. If we only trust the status code, callers conclude the
                // container exists and then raise a false name-conflict before the
                // container has ever been created. Treat inspect output as the
                // source of truth here: a non-empty JSON array means the container
                // was found, while `[]` means "not found" despite the successful
                // process exit.
                let json: Value = serde_json::from_str(&out).unwrap_or(Value::Array(vec![]));
                Ok(json.as_array().map_or(false, |arr| !arr.is_empty()))
            }
            Err(ContainerError::CommandFailed { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn is_running(&self, id: &ContainerId) -> Result<bool, ContainerError> {
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

    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<String>, ContainerError> {
        match run_command("container", &["inspect", &id.0]).await {
            Ok(out) => {
                let ip = out.lines().find_map(|line| {
                    let trimmed = line.trim();
                    if trimmed.starts_with("\"IPAddress\"") || trimmed.starts_with("\"ipAddress\"")
                    {
                        trimmed
                            .split(':')
                            .nth(1)
                            .map(|value| {
                                value.trim().trim_matches('"').trim_matches(',').to_string()
                            })
                            .filter(|value| !value.is_empty() && value != "null")
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

    async fn list_managed_containers(
        &self,
        scope: &ManagedContainerScope,
    ) -> Result<Vec<ManagedContainerInfo>, ContainerError> {
        let refs = parse_macos_container_refs(
            &run_command("container", &["list", "--all", "--format", "json"]).await?,
        );
        if refs.is_empty() {
            return Ok(Vec::new());
        }

        let mut inspect_args = vec!["inspect".to_string()];
        inspect_args.extend(refs);
        let inspect_refs: Vec<&str> = inspect_args.iter().map(|arg| arg.as_str()).collect();
        let containers =
            parse_macos_inspect_output(&run_command("container", &inspect_refs).await?)?;

        Ok(containers
            .into_iter()
            .filter(|container| macos_managed_labels_match(&container.labels, scope))
            .collect())
    }

    async fn run(&self, config: &ContainerConfig) -> Result<ContainerId, ContainerError> {
        let mut args: Vec<String> = vec!["run".into(), "--name".into(), config.name.clone()];

        if config.detach {
            args.push("--detach".into());
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
        run_command("container", &refs).await?;
        Ok(ContainerId(config.name.clone()))
    }

    async fn stop(&self, id: &ContainerId) -> Result<(), ContainerError> {
        run_command("container", &["stop", &id.0]).await.map(|_| ())
    }

    async fn remove(&self, id: &ContainerId) -> Result<(), ContainerError> {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_macos_inspect_output_reads_labels_and_status() {
        let output = r#"
[
  {
    "name": "brain3-mcp-vault-tools",
    "status": "exited",
    "labels": {
      "io.brain3.managed": "true",
      "io.brain3.role": "mcp",
      "io.brain3.installation_id": "scope-1"
    }
  }
]
"#;

        let containers = parse_macos_inspect_output(output).expect("inspect should parse");
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].name, "brain3-mcp-vault-tools");
        assert!(!containers[0].running);
        assert_eq!(containers[0].state, "exited");
        assert!(macos_managed_labels_match(
            &containers[0].labels,
            &ManagedContainerScope::mcp("scope-1".into())
        ));
    }

    #[test]
    fn parse_macos_container_refs_supports_json_arrays() {
        let output = r#"[{"id":"one"},{"ID":"two"},{"name":"three"}]"#;
        assert_eq!(
            parse_macos_container_refs(output),
            vec!["one".to_string(), "two".to_string(), "three".to_string()]
        );
    }
}
