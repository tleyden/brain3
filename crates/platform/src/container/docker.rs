use brain3_core::domain::errors::ContainerError;
use brain3_core::domain::model::{
    ContainerConfig, ContainerLabel, ContainerNetworkIsolationStrategy, ManagedContainerInfo,
    ManagedContainerScope, BRAIN3_INSTALLATION_ID_LABEL_KEY, BRAIN3_MANAGED_LABEL_KEY,
    BRAIN3_MANAGED_LABEL_VALUE, BRAIN3_ROLE_LABEL_KEY,
};
use brain3_core::ports::container::{ContainerId, ContainerPort, NetworkPreparation};
use serde_json::Value;

use super::process::{command_succeeds, format_launch_command, run_command};

pub struct DockerContainerAdapter;

#[derive(Debug, PartialEq, Eq)]
enum NetworkState {
    Missing,
    Compatible,
    Incompatible,
}

fn parse_network_inspect_state(output: &str, internal: bool) -> NetworkState {
    if !internal || output.trim() == "true" {
        NetworkState::Compatible
    } else {
        NetworkState::Incompatible
    }
}

async fn network_is_internal(name: &str) -> Result<bool, ContainerError> {
    let output = run_command(
        "docker",
        &["network", "inspect", "--format", "{{.Internal}}", name],
    )
    .await?;
    Ok(output.trim() == "true")
}

async fn inspect_network_state(name: &str, internal: bool) -> Result<NetworkState, ContainerError> {
    match run_command(
        "docker",
        &["network", "inspect", "--format", "{{.Internal}}", name],
    )
    .await
    {
        Ok(out) => Ok(parse_network_inspect_state(&out, internal)),
        Err(ContainerError::CommandFailed { .. }) => Ok(NetworkState::Missing),
        Err(e) => Err(e),
    }
}

async fn create_network(name: &str, internal: bool) -> Result<(), ContainerError> {
    tracing::info!(network = name, internal, "creating fresh MCP network");
    let mut args = vec!["network", "create"];
    if internal {
        args.push("--internal");
    }
    args.push(name);
    run_command("docker", &args).await?;
    Ok(())
}

fn build_run_args(config: &ContainerConfig) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "--init".into(),
        "--name".into(),
        config.name.clone(),
    ];

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
    args.push("--network".into());
    args.push(config.network_name.clone());
    args.push(config.image.clone());
    args.extend(config.command.iter().cloned());
    args
}

fn build_stop_args(id: &ContainerId) -> Vec<String> {
    vec!["stop".into(), "--time".into(), "5".into(), id.0.clone()]
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

    fn validate_internal_network_support(
        &self,
        config: &ContainerConfig,
    ) -> Result<(), ContainerError> {
        if cfg!(target_os = "macos") && config.isolation_strategy.is_some() {
            tracing::error!(
                container = %config.name,
                network = %config.network_name,
                isolation_strategy = ?config.isolation_strategy,
                runtime = "docker",
                "rejecting unsupported internal network configuration"
            );
            return Err(ContainerError::UnsupportedConfiguration(format!(
                "container '{}' uses Docker internal-network isolation on macOS, which is not supported; use the native macos_container runtime instead",
                config.name
            )));
        }

        Ok(())
    }

    async fn ensure_network(
        &self,
        network_name: &str,
        internal: bool,
    ) -> Result<NetworkPreparation, ContainerError> {
        match inspect_network_state(network_name, internal).await? {
            NetworkState::Missing => {
                create_network(network_name, internal).await?;
                Ok(NetworkPreparation::Created)
            }
            NetworkState::Compatible => Ok(NetworkPreparation::Reused),
            NetworkState::Incompatible => Err(ContainerError::Conflict(format!(
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
        let needs_default_bridge = config.isolation_strategy.is_none()
            && network_is_internal(&config.network_name).await?;
        let args = build_run_args(config);
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        tracing::info!(
            container = %config.name,
            command = %format_launch_command("docker", &args),
            "launching container"
        );
        run_command("docker", &refs).await?;
        if needs_default_bridge {
            let bridge_args = vec![
                "network".to_string(),
                "connect".to_string(),
                "bridge".to_string(),
                config.name.clone(),
            ];
            tracing::info!(
                container = %config.name,
                network = %config.network_name,
                command = %format_launch_command("docker", &bridge_args),
                "attaching non-isolated container to default bridge"
            );
            let bridge_refs = bridge_args.iter().map(String::as_str).collect::<Vec<_>>();
            if let Err(error) = run_command("docker", &bridge_refs).await {
                tracing::error!(
                    container = %config.name,
                    network = %config.network_name,
                    error = %error,
                    "failed to attach non-isolated container to default bridge; removing partial container"
                );
                let _ = run_command("docker", &["rm", "-f", &config.name]).await;
                return Err(error);
            }
        }
        Ok(ContainerId(config.name.clone()))
    }

    async fn stop(&self, id: &ContainerId) -> Result<(), ContainerError> {
        let args = build_stop_args(id);
        let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        run_command("docker", &refs).await.map(|_| ())
    }

    async fn remove(&self, id: &ContainerId) -> Result<(), ContainerError> {
        run_command("docker", &["rm", &id.0]).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_config(strategy: ContainerNetworkIsolationStrategy) -> ContainerConfig {
        ContainerConfig {
            image: "example:latest".into(),
            name: "test_plugin".into(),
            isolation_strategy: Some(strategy),
            network_name: "test-plugin-net".into(),
            port_mappings: Vec::new(),
            env_vars: Vec::new(),
            labels: Vec::new(),
            bind_mounts: Vec::new(),
            user: None,
            detach: true,
            remove_on_exit: true,
            workdir: None,
            command: Vec::new(),
        }
    }

    #[test]
    fn run_args_attach_non_isolated_container_to_configured_network() {
        let mut config = isolated_config(ContainerNetworkIsolationStrategy::PublishToLoopback);
        config.isolation_strategy = None;

        let args = build_run_args(&config);
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--network", "test-plugin-net"]));
        assert!(args.iter().any(|arg| arg == "--init"));
    }

    #[test]
    fn run_args_enable_init_for_isolated_container() {
        let args = build_run_args(&isolated_config(
            ContainerNetworkIsolationStrategy::PublishToLoopback,
        ));
        assert!(args.iter().any(|arg| arg == "--init"));
    }

    #[test]
    fn stop_args_bound_grace_period() {
        assert_eq!(
            build_stop_args(&ContainerId("test_plugin".into())),
            ["stop", "--time", "5", "test_plugin"]
        );
    }

    #[test]
    fn open_network_accepts_existing_internal_network() {
        assert_eq!(
            parse_network_inspect_state("true\n", false),
            NetworkState::Compatible
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rejects_all_internal_network_strategies_on_macos() {
        let adapter = DockerContainerAdapter;

        for strategy in [
            ContainerNetworkIsolationStrategy::PublishToLoopback,
            ContainerNetworkIsolationStrategy::DiscoverContainerIp,
        ] {
            let error = adapter
                .validate_internal_network_support(&isolated_config(strategy))
                .expect_err("macOS Docker isolation should be rejected");

            assert!(matches!(
                error,
                ContainerError::UnsupportedConfiguration(ref message)
                    if message.contains("test_plugin") && message.contains("macos_container")
            ));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn accepts_discover_container_ip_on_linux() {
        DockerContainerAdapter
            .validate_internal_network_support(&isolated_config(
                ContainerNetworkIsolationStrategy::DiscoverContainerIp,
            ))
            .expect("Linux Docker isolation should be supported");
    }

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
